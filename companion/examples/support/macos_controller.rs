//! Actual signal delivery and static baselines on the disposable hosted VM.
use super::{live, loopback};
use naab_companion::dns::system::macos_recovery::{Configuration, Store};
use naab_companion::dns::system::{macos_admission, macos_preflight};
use std::{
    io::{BufRead, BufReader, Read},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

fn signal_case(mode: &str, signal: &str) -> Result<(), String> {
    let mut child = live::Holder(
        Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .args(["--controller-child", mode])
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?,
    );
    let stdout = child.0.stdout.take().ok_or("Missing controller output")?;
    let (send, receive) = mpsc::sync_channel(128);
    std::thread::spawn(move || {
        for line in BufReader::new(stdout.take(128 * 1024)).lines() {
            match line {
                Ok(line) => {
                    if send.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let started = Instant::now();
    let mut active_signal = false;
    let mut cleanup_signal = false;
    let mut passed = false;
    loop {
        if started.elapsed() > Duration::from_secs(75) {
            return Err("Controller child exceeded driver deadline".into());
        }
        match receive.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                println!("{line}");
                let should_signal = (line == "TRIAL_ACTIVE" && !active_signal)
                    || (line == "TRIAL_RESTORING" && active_signal && !cleanup_signal);
                if should_signal {
                    if !Command::new("/bin/kill")
                        .args([signal, &child.0.id().to_string()])
                        .status()
                        .map_err(|e| e.to_string())?
                        .success()
                    {
                        return Err("Could not deliver test signal".into());
                    }
                    if line == "TRIAL_ACTIVE" {
                        active_signal = true;
                    } else {
                        cleanup_signal = true;
                    }
                }
                if line
                    == format!(
                        "PASS: controller {mode} restored exact original DNS; journal cleared"
                    )
                {
                    passed = true;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = loop {
        if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > Duration::from_secs(75) {
            return Err("Controller child did not exit after closing output".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if !status.success() || !active_signal || !cleanup_signal || !passed {
        return Err(format!("Signal case failed: {status}, active={active_signal}, cleanup={cleanup_signal}, pass={passed}"));
    }
    println!("PASS: actual {signal} during active trial and again during cleanup");
    Ok(())
}

fn static_case(ordered: bool) -> Result<(), String> {
    let (mut baseline, _) = live::capture_with_upstreams()?;
    let extra = if ordered {
        "<string>1.0.0.1</string><string>2606:4700:4700::1111</string>"
    } else {
        ""
    };
    baseline.applied.configuration = Configuration::Saved(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>\
         <key>ServerAddresses</key><array><string>1.1.1.1</string>{extra}</array>\
         <key>SearchDomains</key><array><string>naab-ci.invalid</string><string>second.invalid</string></array>\
         </dict></plist>"
    ).into_bytes());
    let result = (|| {
        let mut store = live::open_store()?;
        live::activate(&mut store, &baseline)?;
        // Intentional fixture baseline on a disposable VM. The upcoming trial
        // saves this complete static dictionary as its own original. This is
        // not a production rebase or a supported nested recovery transaction.
        store.clear()?;
        drop(store);
        println!("Static controller baseline: ordered={ordered}, including search domains");
        // Preferences read-back does not establish that configd has published
        // matching effective resolvers. Wait only in this disposable setup;
        // production admission still rejects inconsistent snapshots immediately.
        let started = Instant::now();
        let mut ready_since = None;
        loop {
            live::expect_state(&baseline, &baseline.applied)?;
            let report = macos_preflight::preflight()?;
            let admission = macos_admission::candidate(&report);
            eprintln!(
                "Static readiness: configured={:?}, effective={:?}, admission={:?}",
                report
                    .services
                    .iter()
                    .map(|s| (&s.name, &s.configured_dns))
                    .collect::<Vec<_>>(),
                report.default_dns_servers,
                admission.as_ref().map(|_| ()),
            );
            // A preflight has its own bounded utility budget; native framework
            // calls are not hard timed. Never accept a result after this budget.
            if started.elapsed() >= Duration::from_secs(10) {
                return Err(format!(
                    "Static DNS readiness exceeded ten seconds: {admission:?}"
                ));
            }
            if admission.is_ok() {
                let ready = ready_since.get_or_insert_with(Instant::now);
                if ready.elapsed() >= Duration::from_millis(500) {
                    break;
                }
            } else {
                ready_since = None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        loopback::controller_case("deadline")?;
        live::expect_state(&baseline, &baseline.applied)
    })();
    // Never replace a retained trial journal, or mask a failed assertion. Recover
    // its static baseline first, then restore the initial runner configuration.
    live::run_helper()?;
    live::expect_state(&baseline, &baseline.applied)?;
    {
        let mut store = live::open_store()?;
        store.create(&baseline)?;
    }
    live::run_helper()?;
    live::verify_restored(&baseline)?;
    result?;
    println!("PASS: static ordered={ordered} restored exactly; runner baseline restored");
    Ok(())
}

pub(super) fn test_cases() -> Result<(), String> {
    live::require_ci_environment()?;
    live::require_recovery_task()?;
    signal_case("interrupt", "-INT")?;
    signal_case("terminate", "-TERM")?;
    static_case(false)?;
    static_case(true)
}
