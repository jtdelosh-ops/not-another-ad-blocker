use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn dns_development_cli_checks_sample_without_listening_and_rejects_bad_arguments() {
    let executable = env!("CARGO_BIN_EXE_naab-dns-dev");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let result = Command::new(executable)
        .args(["--config", "examples/dns-dev.json", "--check"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["event"], "validated");
    assert_eq!(report["listen"], "127.0.0.1:5354");
    assert!(result.stderr.is_empty());
    let pretty = Command::new(executable)
        .args(["--config", "examples/dns-dev.json", "--check", "--pretty"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(pretty.status.success());
    let pretty_stdout = String::from_utf8(pretty.stdout).unwrap();
    assert!(pretty_stdout.starts_with("DNS coverage report\n"));
    assert!(pretty_stdout.contains("Status: READY — list blocking active"));
    assert!(pretty_stdout.contains("Active block rules:      1"));
    assert!(pretty_stdout.contains("Source coverage:\n  Example rules"));
    assert!(pretty_stdout.contains("    Unique block rules:      1"));
    assert!(pretty_stdout.contains("Diagnostics:"));
    assert!(pretty_stdout.contains("this rule was omitted"));
    assert!(serde_json::from_str::<serde_json::Value>(&pretty_stdout).is_err());

    let suppression_root = std::env::temp_dir().join(format!(
        "naab-dns-pretty-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&suppression_root).unwrap();
    fs::write(
        suppression_root.join("rules.txt"),
        format!("{}!#if false\n", "unsupported\n".repeat(200)),
    )
    .unwrap();
    fs::write(
        suppression_root.join("config.json"),
        serde_json::json!({
            "upstreams": ["192.0.2.1:53"],
            "filterLists": [{"name": "Suppression fixture", "path": "rules.txt"}]
        })
        .to_string(),
    )
    .unwrap();
    let suppression_config = suppression_root.join("config.json");
    let suppression_config = suppression_config.to_str().unwrap();
    let suppression = Command::new(executable)
        .args(["--config", suppression_config, "--check", "--pretty"])
        .output()
        .unwrap();
    assert!(suppression.status.success());
    let suppression_stdout = String::from_utf8(suppression.stdout).unwrap();
    assert!(suppression_stdout.contains("Safety suppression reasons: 1 (1 shown)"));
    assert!(suppression_stdout.contains("Unsupported preprocessing directive"));
    fs::remove_dir_all(&suppression_root).unwrap();

    for arguments in [
        vec![],
        vec!["--wat"],
        vec!["--config"],
        vec!["--config", "examples/dns-dev.json", "--pretty"],
    ] {
        let result = Command::new(executable).args(arguments).output().unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
    }
}

#[test]
fn dns_config_relative_paths_are_relative_to_configuration_and_input_is_bounded() {
    let root = std::env::temp_dir().join(format!(
        "naab-dns-config-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("rules.txt"), "||ads.test^").unwrap();
    fs::write(
        root.join("config.json"),
        r#"{"upstreams":["192.0.2.1:53"],"filterLists":[{"name":"Fixture","path":"rules.txt"}]}"#,
    )
    .unwrap();
    let (_, policy) = naab_companion::dns::DnsConfig::load(&root.join("config.json")).unwrap();
    assert!(policy.decide("ads.test").blocked);
    fs::write(root.join("config.json"), vec![b' '; 1024 * 1024 + 1]).unwrap();
    assert!(
        naab_companion::dns::DnsConfig::load(&root.join("config.json"))
            .unwrap_err()
            .contains("exceeds")
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn dns_cli_blocks_reports_clears_and_releases_listeners() {
    use hickory_proto::{
        op::{Message, Query, ResponseCode},
        rr::{Name, RecordType},
    };
    use std::{
        io::Write,
        net::{TcpListener, UdpSocket},
        process::Stdio,
        time::{Duration, Instant},
    };

    struct ChildGuard(Option<std::process::Child>);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(child) = &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    let root = std::env::temp_dir().join(format!(
        "naab-dns-cli-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let reserve = TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = reserve.local_addr().unwrap();
    let upstream = UdpSocket::bind("127.0.0.1:0").unwrap();
    let config = serde_json::json!({"listen":listen,"upstreams":[upstream.local_addr().unwrap()],"blocklist":["blocked.test"]});
    fs::write(
        root.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    drop(reserve);
    let mut child = ChildGuard(Some(
        Command::new(env!("CARGO_BIN_EXE_naab-dns-dev"))
            .arg("--config")
            .arg(root.join("config.json"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let mut query = Message::new();
    query
        .set_id(42)
        .set_recursion_desired(true)
        .add_query(Query::query(
            Name::from_ascii("blocked.test.").unwrap(),
            RecordType::A,
        ));
    let bytes = query.to_vec().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut reply = [0; 512];
    loop {
        assert!(Instant::now() < deadline, "DNS executable did not start");
        socket.send_to(&bytes, listen).unwrap();
        if let Ok((length, _)) = socket.recv_from(&mut reply) {
            let response = Message::from_vec(&reply[..length]).unwrap();
            assert_eq!(response.id(), 42);
            assert_eq!(response.response_code(), ResponseCode::NXDomain);
            break;
        }
    }
    let process = child.0.as_mut().unwrap();
    process
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"status\nactivity\nclear\nactivity\nquit\n")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while process.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "DNS executable did not stop");
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.0.take().unwrap().wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["event"], "starting");
    assert_eq!(rows[1]["event"], "status");
    assert!(rows[1]["total"].as_u64().unwrap() >= 1);
    assert_eq!(rows[2]["data"]["recent"][0]["hostname"], "blocked.test");
    assert_eq!(rows[3]["event"], "activity-cleared");
    assert_eq!(rows[4]["data"]["recent"], serde_json::json!([]));
    assert_eq!(rows[5]["event"], "stopped");
    assert!(TcpListener::bind(listen).is_ok());
    assert!(UdpSocket::bind(listen).is_ok());
    upstream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    assert!(
        upstream.recv_from(&mut reply).is_err(),
        "Blocked query must not reach upstream"
    );
    fs::remove_dir_all(&root).unwrap();
}
