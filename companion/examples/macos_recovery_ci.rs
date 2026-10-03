//! Settings-changing recovery fixture for disposable GitHub-hosted Macs only.
//! No user-facing activation command is exposed. --run retains working upstreams;
//! the separately opted-in --loopback cases exercise the ordinary DNS resolver.

#[cfg(target_os = "macos")]
#[path = "support/macos_loopback.rs"]
mod loopback;

#[cfg(target_os = "macos")]
#[path = "support/macos_system_query.rs"]
mod system_query;

#[cfg(target_os = "macos")]
#[path = "support/macos_controller.rs"]
mod controller;

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This live recovery fixture requires a disposable macOS runner.");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
mod live {
    use naab_companion::dns::system::{
        macos_admission, macos_identity, macos_preflight,
        macos_recovery::{
            Configuration, DnsState, FileStore, NativeSettings, Observation, Record, Settings,
            Store,
        },
    };
    use std::{
        io::{BufRead, BufReader, Write},
        net::SocketAddr,
        process::{Child, Command, Stdio},
        sync::mpsc,
        time::{Duration, Instant},
    };

    const HELPER: &str = "/Library/Application Support/NAAB-DNS-Preview/recovery-helper";
    const RECORD: &str = "/Library/Application Support/NAAB-DNS-Preview/recovery.json";
    const REPORT: &str = "/Library/Application Support/NAAB-DNS-Preview/last-report.json";
    const ACK: &str = "disposable-mac-dns-recovery";

    pub(super) fn open_store() -> Result<FileStore, String> {
        // launchd may be finishing its report immediately after clearing the
        // journal. Give that bounded critical section time to release the lock.
        let started = Instant::now();
        loop {
            match FileStore::open_fixed() {
                Ok(store) => return Ok(store),
                Err(error)
                    if error.contains("Mac recovery is already running")
                        && started.elapsed() < Duration::from_secs(5) =>
                {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub(super) fn require_ci_environment() -> Result<(), String> {
        // Accident prevention, not a security boundary: these variables can be
        // spoofed. The workflow must use a GitHub-hosted, disposable VM.
        for (name, expected) in [
            ("GITHUB_ACTIONS", "true"),
            ("RUNNER_ENVIRONMENT", "github-hosted"),
            ("NAAB_LIVE_DNS_TEST", ACK),
        ] {
            if std::env::var(name).as_deref() != Ok(expected) {
                return Err(format!(
                    "Refusing live test: {name} acknowledgement is missing"
                ));
            }
        }
        Ok(())
    }

    fn require_ci() -> Result<(), String> {
        require_ci_environment()?;
        // Production store validates root identity, trusted paths, and locking.
        let mut store = open_store()?;
        if store.load()?.is_some() {
            return Err("Refusing live test with a pending recovery record".into());
        }
        Ok(())
    }

    fn context_matches(record: &Record, observed: &Observation) -> bool {
        record.set_id == observed.set_id
            && record.service_id == observed.service_id
            && record.device == observed.device
            && record.network_context_sha256 == observed.network_context_sha256
    }

    pub(super) fn expect_state(record: &Record, expected: &DnsState) -> Result<(), String> {
        let mut settings = NativeSettings;
        let observed = settings.observe(record)?;
        if !context_matches(record, &observed) || !settings.equivalent(&observed.dns, expected)? {
            return Err(
                "Native read-back differs from the complete expected DNS state or context".into(),
            );
        }
        Ok(())
    }

    fn capture() -> Result<Record, String> {
        capture_with_upstreams().map(|(record, _)| record)
    }

    pub(super) fn capture_with_upstreams() -> Result<(Record, Vec<SocketAddr>), String> {
        let preflight = macos_preflight::preflight()?;
        eprintln!(
            "Runner primary DNS protocol: {}",
            serde_json::to_string(&preflight.primary_service_dns_protocol)
                .map_err(|e| e.to_string())?
        );
        let candidate = macos_admission::candidate(&preflight)?;
        let (device, service_id, set_id, servers) = (
            candidate.device,
            candidate.service_id,
            candidate.set_id,
            candidate.servers,
        );
        // IP parsing above makes interpolation into XML safe. The marker makes
        // the dictionary genuinely different even if DNS was already static.
        let addresses = servers
            .iter()
            .map(|ip| format!("<string>{ip}</string>"))
            .collect::<String>();
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>\
             <key>ServerAddresses</key><array>{addresses}</array>\
             <key>SearchDomains</key><array><string>naab-ci.invalid</string></array>\
             </dict></plist>"
        );
        let mut record = Record {
            version: 1,
            session_id: format!("{:032x}", rand::random::<u128>()),
            set_id,
            service_id,
            device,
            network_context_sha256: "0".repeat(64),
            original: DnsState {
                enabled: true,
                configuration: Configuration::NoSavedConfiguration,
            },
            applied: DnsState {
                enabled: true,
                configuration: Configuration::Saved(xml.into_bytes()),
            },
        };
        // observe reads only the target fields. Replace the placeholders with
        // the real complete original state and gateway identity before saving.
        let mut settings = NativeSettings;
        let observed = settings.observe(&record)?;
        if observed.set_id != record.set_id || !observed.dns.enabled {
            return Err("Runner location changed or its DNS protocol is disabled".into());
        }
        record.original = observed.dns;
        let Configuration::Saved(xml) = &record.original.configuration else {
            return Err("Missing complete original DNS dictionary".into());
        };
        if macos_identity::trial_dns_servers(xml)? != candidate.configured_servers {
            return Err("Original DNS dictionary disagrees with service inventory".into());
        }
        record.network_context_sha256 = observed.network_context_sha256;
        record.validate()?;
        if settings.equivalent(&record.original, &record.applied)? {
            return Err("Fixture would not change the runner DNS configuration".into());
        }
        Ok((
            record,
            servers
                .into_iter()
                .map(|ip| SocketAddr::new(ip, 53))
                .collect(),
        ))
    }

    pub(super) fn activate(store: &mut FileStore, record: &Record) -> Result<(), String> {
        // This must complete (including file and directory sync) before write.
        store.create(record)?;
        // Test-only inverse transaction uses the existing native locked compare
        // and write primitive. Production recovery still restores applied ->
        // original; no public activation command is added by this fixture.
        let mut inverse = record.clone();
        std::mem::swap(&mut inverse.original, &mut inverse.applied);
        macos_identity::restore_dns_protocol(&inverse, || {
            Ok(context_matches(record, &NativeSettings.observe(record)?))
        })?;
        expect_state(record, &record.applied)
    }

    pub(super) fn run_helper() -> Result<(), String> {
        let started = Instant::now();
        loop {
            let output = Command::new(HELPER)
                .arg("recover-if-needed")
                .output()
                .map_err(|e| e.to_string())?;
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
                return Ok(());
            }
            let error = String::from_utf8_lossy(&output.stderr);
            if error.contains("Mac recovery is already running")
                && started.elapsed() < Duration::from_secs(5)
            {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            return Err(format!("Independent recovery failed: {error}"));
        }
    }

    pub(super) fn verify_restored(record: &Record) -> Result<(), String> {
        let mut store = open_store()?;
        if store.load()?.is_some() {
            return Err("Recovery journal is still pending".into());
        }
        expect_state(record, &record.original)?;
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(REPORT).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if report["outcome"] != "restored"
            || report["recoveryPending"] != false
            || report["reportWriteFailed"] != false
        {
            return Err(format!(
                "Expected a successful real restore report, got {report}"
            ));
        }
        Ok(())
    }

    pub(super) struct Holder(pub(super) Child);
    impl Drop for Holder {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn hold() -> Result<(), String> {
        let mut store = open_store()?;
        let record = capture()?;
        activate(&mut store, &record)?;
        println!("ACTIVE");
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        // A fixture timeout releases the lock and invokes offline cleanup. The
        // actual case kills this process earlier to exercise launchd recovery.
        std::thread::sleep(Duration::from_secs(30));
        drop(store);
        run_helper()?;
        Err("Holder timed out without being terminated by the driver".into())
    }

    pub(super) fn require_recovery_task() -> Result<(), String> {
        if !Command::new("/bin/launchctl")
            .args(["print", "system/com.naab.dns-recovery"])
            .stdout(Stdio::null())
            .status()
            .map_err(|e| e.to_string())?
            .success()
        {
            return Err("The independent recovery task is not installed".into());
        }
        Ok(())
    }

    fn test_cases() -> Result<(), String> {
        require_recovery_task()?;
        let baseline = capture()?;
        println!(
            "Baseline saved DNS configuration: {}",
            match baseline.original.configuration {
                Configuration::NoSavedConfiguration => "absent (automatic service setting)",
                Configuration::Saved(_) => "present (complete dictionary captured)",
            }
        );
        println!("Case 1: durable record, native DNS change, normal independent restore");
        {
            let mut store = open_store()?;
            activate(&mut store, &baseline)?;
            std::thread::sleep(Duration::from_secs(2));
            expect_state(&baseline, &baseline.applied)?;
        }
        run_helper()?;
        verify_restored(&baseline)?;
        println!(
            "PASS: exact original DNS dictionary and protocol state restored; journal cleared"
        );

        println!("Case 2: terminate the holder; wait for the scheduled helper to restore DNS");
        let mut holder = Holder(
            Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
                .arg("--hold")
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?,
        );
        let stdout = holder.0.stdout.take().ok_or("Missing holder pipe")?;
        let (send, receive) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
            let _ = send.send(result);
        });
        let line = receive
            .recv_timeout(Duration::from_secs(20))
            .map_err(|_| "Holder did not become active within 20 seconds")?
            .map_err(|e| e.to_string())?;
        if line.trim() != "ACTIVE" {
            return Err(format!("Holder activation failed: {line}"));
        }
        // The holder owns the store lock. Read its protected journal without
        // taking that lock to independently prove what was saved and applied.
        let record: Record =
            serde_json::from_slice(&std::fs::read(RECORD).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        record.validate()?;
        if record.set_id != baseline.set_id
            || record.service_id != baseline.service_id
            || record.device != baseline.device
            || record.network_context_sha256 != baseline.network_context_sha256
            || !NativeSettings.equivalent(&record.original, &baseline.original)?
        {
            return Err("Second case no longer has the original runner configuration".into());
        }
        expect_state(&record, &record.applied)?;
        holder.0.kill().map_err(|e| e.to_string())?;
        let status = holder.0.wait().map_err(|e| e.to_string())?;
        use std::os::unix::process::ExitStatusExt;
        if status.signal() != Some(9) {
            return Err(format!("Holder was not killed with SIGKILL: {status}"));
        }
        // No manual recover or launchctl kickstart here: this specifically
        // measures the independently installed 60-second schedule.
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(100) {
            if !std::path::Path::new(RECORD)
                .try_exists()
                .map_err(|e| e.to_string())?
            {
                verify_restored(&record)?;
                println!("PASS: scheduled recovery restored DNS after SIGKILL; journal cleared");
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        Err("Scheduled recovery did not clear the journal within 100 seconds".into())
    }

    pub fn run() -> Result<(), String> {
        let args: Vec<_> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some("--resolver") {
            return super::loopback::resolver_child(&args[1..]);
        }
        require_ci()?;
        let result = if args == ["--hold"] {
            hold()
        } else if args == ["--run"] {
            test_cases()
        } else if args == ["--loopback"] {
            super::loopback::test_cases()
        } else if args == ["--controller"] {
            super::controller::test_cases()
        } else if args.len() == 2 && args[0] == "--controller-child" {
            super::loopback::controller_case(&args[1])
        } else {
            return Err("Use --run only on a disposable GitHub-hosted Mac".into());
        };
        if let Err(error) = &result {
            eprintln!("FAIL: {error}");
            // Cleanup happens after scope guards drop and release store locks.
            // This cannot turn a failed scheduled-recovery case into a pass.
            if let Err(cleanup) = run_helper() {
                eprintln!("Cleanup needs attention: {cleanup}");
            }
        }
        result
    }
}

#[cfg(target_os = "macos")]
fn main() {
    if let Err(error) = live::run() {
        eprintln!("macos-recovery-ci: {error}");
        std::process::exit(1);
    }
}
