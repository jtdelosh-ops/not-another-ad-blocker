//! Real Windows console events, with simulated DNS only. The subprocess helpers
//! are test functions so no test-only command is added to the privileged CLI.
#![cfg(windows)]
use naab_companion::dns::system::{
    journal::FileJournal,
    simulation::{SimulatedPlatform, SimulatedProbe},
    trial, Action, Controller, DnsSetting, Platform, State, Target, TargetState,
};
use std::{path::PathBuf, time::Duration};

#[test]
fn console_shutdown_restores_or_retains_recovery_record() {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/test-windows-dns-console.ps1");
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .arg("-FixtureExecutable")
        .arg(std::env::current_exe().unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "console fixtures failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct SlowPlatform {
    inner: SimulatedPlatform,
    directory: PathBuf,
    case: String,
}
impl Platform for SlowPlatform {
    fn snapshot(&mut self) -> Result<Vec<TargetState>, String> {
        self.inner.snapshot()
    }
    fn read(&mut self, target: &Target) -> Result<Option<DnsSetting>, String> {
        self.inner.read(target)
    }
    fn compare_and_set(
        &mut self,
        target: &Target,
        expected: &DnsSetting,
        replacement: &DnsSetting,
    ) -> Result<(), String> {
        let restoring = replacement == &DnsSetting::Automatic
            || matches!(replacement, DnsSetting::Static(ips) if !ips[0].is_loopback());
        if restoring {
            std::fs::write(self.directory.join("restoring"), "ready").unwrap();
            // Model slow Windows cmdlets. The second signal must arrive here.
            std::thread::sleep(Duration::from_secs(2));
            if self.case == "restore-failure" {
                return Err("Injected restore failure".into());
            }
        } else if self.case == "during-enable" {
            std::fs::write(self.directory.join("applying"), "ready").unwrap();
            std::thread::sleep(Duration::from_secs(2));
        }
        self.inner.compare_and_set(target, expected, replacement)
    }
}

#[test]
fn console_fixture() {
    let Some(directory) = std::env::var_os("NAAB_CONSOLE_FIXTURE") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let case = std::env::var("NAAB_CONSOLE_CASE").unwrap();
    let inner = SimulatedPlatform::example();
    let original = inner.0.lock().unwrap().targets.clone();
    let platform = SlowPlatform {
        inner: inner.clone(),
        directory: directory.clone(),
        case: case.clone(),
    };
    let journal = FileJournal::open(&directory).unwrap();
    let mut controller = Controller::new(platform, journal, SimulatedProbe::default());
    let mut final_state = State::Active;
    let result = trial::run(
        &mut controller,
        if case == "timeout" {
            Duration::from_millis(1)
        } else {
            Duration::from_secs(30)
        },
        |report| {
            if report.action == Action::Enable && report.state == State::Active {
                std::fs::write(directory.join("active"), std::process::id().to_string()).unwrap();
                if case == "panic" {
                    panic!("Injected observer failure");
                }
            }
            final_state = report.state;
            if report.state == State::RecoveryRequired {
                Err("Recovery required".into())
            } else {
                Ok(())
            }
        },
        || {},
    )
    .err();
    let restored = inner.0.lock().unwrap().targets == original;
    let retained = directory.join("recovery.json").exists();
    if case == "restore-failure" {
        assert_eq!(final_state, State::RecoveryRequired);
        assert!(result.is_some() && retained && !restored);
    } else {
        assert_eq!(final_state, State::Inactive);
        assert!(restored && !retained);
        assert_eq!(result.is_some(), case == "panic");
    }
    std::fs::write(directory.join("done.json"), serde_json::to_vec(&serde_json::json!({
        "case":case, "state":final_state, "restored":restored, "retained":retained, "error":result
    })).unwrap()).unwrap();
}

#[link(name = "kernel32")]
extern "system" {
    fn FreeConsole() -> i32;
    fn AttachConsole(pid: u32) -> i32;
    fn GetConsoleProcessList(pids: *mut u32, count: u32) -> u32;
    fn SetConsoleCtrlHandler(handler: *const std::ffi::c_void, add: i32) -> i32;
    fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
}

#[test]
fn console_signal_helper() {
    let Ok(pid) = std::env::var("NAAB_CONSOLE_TARGET_PID") else {
        return;
    };
    let pid: u32 = pid.parse().unwrap();
    let parent: u32 = std::env::var("NAAB_CONSOLE_PARENT_PID")
        .unwrap()
        .parse()
        .unwrap();
    let event: u32 = std::env::var("NAAB_CONSOLE_EVENT")
        .unwrap()
        .parse()
        .unwrap();
    assert!(event <= 1);
    // SAFETY: only this disposable helper detaches/attaches. Verify that the
    // newly created target console excludes the harness before broadcasting.
    unsafe {
        FreeConsole();
        assert_ne!(AttachConsole(pid), 0, "{}", std::io::Error::last_os_error());
        let mut pids = [0u32; 32];
        let count = GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) as usize;
        assert!(count > 0 && count <= pids.len());
        assert!(pids[..count].contains(&pid) && !pids[..count].contains(&parent));
        assert_ne!(SetConsoleCtrlHandler(std::ptr::null(), 1), 0);
        // The helper ignores Ctrl+C. For Ctrl+Break the default handler may exit
        // it; use a callback for both signals so delivery can finish reliably.
        assert_ne!(
            SetConsoleCtrlHandler(ignore_signal as *const std::ffi::c_void, 1),
            0
        );
        assert_ne!(GenerateConsoleCtrlEvent(event, 0), 0);
        std::thread::sleep(Duration::from_millis(200));
        FreeConsole();
    }
}
unsafe extern "system" fn ignore_signal(_: u32) -> i32 {
    1
}
