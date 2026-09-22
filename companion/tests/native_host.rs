use naab_companion::messaging::{read_frame, MAX_INCOMING_BYTES};
use serde_json::{json, Value};
use std::io::{Cursor, Write};
use std::process::{Command, Output, Stdio};

fn frame(bytes: &[u8]) -> Vec<u8> {
    let mut frame = (bytes.len() as u32).to_ne_bytes().to_vec();
    frame.extend_from_slice(bytes);
    frame
}

fn run(input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_naab-companion"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn executable_handles_multiple_requests_and_recovers_from_invalid_json() {
    let status = json!({"version":1,"id":"health","type":"status.get","payload":{}});
    let compile = json!({"version":1,"id":"compile","type":"rules.compile","payload":{"text":"||ads.test^\n##.ad"}});
    let mut input = frame(&serde_json::to_vec(&status).unwrap());
    input.extend(frame(b"invalid JSON"));
    input.extend(frame(&serde_json::to_vec(&compile).unwrap()));
    let output = run(&input);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let mut stdout = Cursor::new(output.stdout);
    let status: Value = serde_json::from_slice(&read_frame(&mut stdout).unwrap().unwrap()).unwrap();
    let error: Value = serde_json::from_slice(&read_frame(&mut stdout).unwrap().unwrap()).unwrap();
    let compile: Value =
        serde_json::from_slice(&read_frame(&mut stdout).unwrap().unwrap()).unwrap();
    assert_eq!(status["id"], "health");
    assert_eq!(status["payload"]["healthy"], true);
    assert_eq!(error["error"]["code"], "INVALID_JSON");
    assert_eq!(compile["id"], "compile");
    assert_eq!(compile["payload"]["stats"]["network"], 1);
    assert_eq!(compile["payload"]["stats"]["cosmetic"], 1);
    assert!(
        read_frame(&mut stdout).unwrap().is_none(),
        "stdout must contain frames only"
    );
}

#[test]
fn executable_exits_cleanly_without_output_at_eof() {
    let output = run(&[]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn executable_reports_framing_failures_only_to_stderr() {
    for bytes in [
        vec![1, 2],
        (MAX_INCOMING_BYTES as u32 + 1).to_ne_bytes().to_vec(),
        frame(b"secret")[..7].to_vec(),
    ] {
        let output = run(&bytes);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).starts_with("naab-companion:"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("secret"));
    }
}
