use naab_companion::dns::system::{
    windows::{BridgeError, Runner, WindowsPlatform},
    DnsSetting, Family, Platform, Target, TargetState,
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const ID: &str = "00000000-0000-0000-0000-000000000001";
fn target() -> Target {
    Target {
        interface_id: ID.into(),
        network_id: format!("{ID}:{ID}"),
        family: Family::Ipv6,
    }
}

struct Fake {
    replies: VecDeque<Result<Value, BridgeError>>,
    calls: Arc<Mutex<Vec<Value>>>,
}
impl Runner for Fake {
    fn call(&mut self, request: Value) -> Result<Value, BridgeError> {
        self.calls.lock().unwrap().push(request);
        self.replies.pop_front().expect("unexpected bridge call")
    }
}
fn fake(replies: Vec<Result<Value, BridgeError>>) -> (Fake, Arc<Mutex<Vec<Value>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    (
        Fake {
            replies: replies.into(),
            calls: calls.clone(),
        },
        calls,
    )
}

#[test]
fn windows_snapshot_and_mutation_keep_guid_family_mode_and_order() {
    let original = DnsSetting::Static(vec![
        "2001:db8::2".parse().unwrap(),
        "2001:db8::1".parse().unwrap(),
    ]);
    let (runner, calls) = fake(vec![
        Ok(json!({"states":[TargetState { target: target(), setting: original.clone() }]})),
        Ok(json!({"setting":original})),
        Ok(json!({"setting":original})),
    ]);
    let mut platform = WindowsPlatform::new(runner, Some(ID.into())).unwrap();
    assert_eq!(platform.snapshot().unwrap()[0].setting, original);
    assert_eq!(platform.read(&target()).unwrap(), Some(original.clone()));
    platform
        .compare_and_set(&target(), &original, &DnsSetting::Automatic)
        .unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls[2]["target"]["family"], "ipv6");
    assert_eq!(calls[2]["target"]["networkId"], format!("{ID}:{ID}"));
    assert_eq!(
        calls[2]["expected"]["servers"],
        json!(["2001:db8::2", "2001:db8::1"])
    );
    assert_eq!(calls[2]["replacement"], json!({"mode":"automatic"}));
}

#[test]
fn unknown_identity_foreign_snapshot_and_uncertain_writes_fail_closed() {
    let (runner, _) = fake(vec![]);
    assert!(WindowsPlatform::new(runner, Some("Wi-Fi; arbitrary command".into())).is_err());
    let (runner, calls) = fake(vec![Err(BridgeError {
        message: "timeout".into(),
        uncertain: true,
    })]);
    let mut platform = WindowsPlatform::new(runner, None).unwrap();
    assert!(platform
        .compare_and_set(
            &target(),
            &DnsSetting::Automatic,
            &DnsSetting::Static(vec!["::1".parse().unwrap()])
        )
        .is_err());
    assert!(platform.read(&target()).is_err());
    assert_eq!(calls.lock().unwrap().len(), 1); // No rollback race after a timed-out OS write.
    let mut foreign = target();
    foreign.interface_id = "00000000-0000-0000-0000-000000000002".into();
    let (runner, _) = fake(vec![Ok(
        json!({"states":[TargetState { target: foreign, setting: DnsSetting::Automatic }]}),
    )]);
    assert!(WindowsPlatform::new(runner, Some(ID.into()))
        .unwrap()
        .snapshot()
        .is_err());
    let (runner, calls) = fake(vec![]);
    let mut platform = WindowsPlatform::new(runner, None).unwrap();
    let mut damaged = target();
    damaged.network_id = "unbound".into();
    assert!(platform.read(&damaged).is_err());
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn preview_cli_requires_explicit_complete_command_before_any_action() {
    use std::process::Command;
    let binary = env!("CARGO_BIN_EXE_naab-dns-windows");
    let help = Command::new(binary).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("maximum 5 minutes"));
    for args in [
        vec!["recover", "--config", "wrong"],
        vec!["recover-if-needed", "--config", "wrong"],
        vec!["recovery-dir", "--apply"],
        vec!["trial", "--adapter", ID],
        vec!["serve", "--config", "a", "--config", "b"],
        vec!["inspect", "--apply"],
    ] {
        assert!(!Command::new(binary)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
}

#[test]
#[cfg(windows)]
fn embedded_windows_adapter_passes_isolated_powershell_fixtures() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let executable = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap()
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let result = std::process::Command::new(executable)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(root.join("scripts/test-windows-dns-adapter.ps1"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("Windows DNS adapter fixtures passed"));
}
