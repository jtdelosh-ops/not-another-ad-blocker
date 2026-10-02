//! Settings-changing recovery fixture for disposable GitHub-hosted Macs only.
//! This is not a user-facing activation command or a loopback resolver trial.
//! Both cases retain working upstreams and exercise the production file store,
//! native compare/write, and separately installed recovery executable.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This live recovery fixture requires a disposable macOS runner.");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
mod live {
    use naab_companion::dns::system::{
        macos_identity, macos_preflight,
        macos_recovery::{
            Configuration, DnsState, FileStore, NativeSettings, Observation, Record, Settings,
            Store,
        },
    };
    use std::{
        io::{BufRead, BufReader, Write},
        net::IpAddr,
        process::{Child, Command, Stdio},
        sync::mpsc,
        time::{Duration, Instant},
    };

    const HELPER: &str = "/Library/Application Support/NAAB-DNS-Preview/recovery-helper";
    const RECORD: &str = "/Library/Application Support/NAAB-DNS-Preview/recovery.json";
    const REPORT: &str = "/Library/Application Support/NAAB-DNS-Preview/last-report.json";
    const ACK: &str = "disposable-mac-dns-recovery";

    fn open_store() -> Result<FileStore, String> {
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

    fn require_ci() -> Result<(), String> {
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

    fn expect_state(record: &Record, expected: &DnsState) -> Result<(), String> {
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
        let preflight = macos_preflight::preflight()?;
        eprintln!(
            "Runner primary DNS protocol: {}",
            serde_json::to_string(&preflight.primary_service_dns_protocol)
                .map_err(|e| e.to_string())?
        );
        let device = preflight
            .primary_ipv4_interface
            .ok_or("Missing primary interface")?;
        let service_id = preflight
            .primary_ipv4_service_id
            .ok_or("Missing primary service ID")?;
        let set_id = preflight
            .current_set_id
            .ok_or("Missing current location ID")?;
        if !preflight.other_services_with_configured_dns.is_empty() {
            return Err("Other services control explicit DNS on this runner".into());
        }
        for resolver in &preflight.resolvers {
            if resolver.nameservers.is_empty() && resolver.options.as_deref() == Some("mdns") {
                continue;
            }
            if resolver.nameservers.is_empty()
                || resolver.domain.is_some()
                || resolver.options.is_some()
                || resolver
                    .interface
                    .as_ref()
                    .is_some_and(|value| value != &device)
            {
                return Err("Runner has unsupported scoped or supplemental DNS policy".into());
            }
        }
        let servers: Vec<IpAddr> = preflight
            .default_dns_servers
            .iter()
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| "Runner DNS server is not an explicit IP")
            })
            .collect::<Result<_, _>>()?;
        if servers.is_empty()
            || servers.len() > 8
            || servers
                .iter()
                .any(|ip| ip.is_loopback() || ip.is_unspecified() || ip.is_multicast())
        {
            return Err("Runner DNS upstreams are unsupported".into());
        }
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
        record.network_context_sha256 = observed.network_context_sha256;
        record.validate()?;
        if settings.equivalent(&record.original, &record.applied)? {
            return Err("Fixture would not change the runner DNS configuration".into());
        }
        Ok(record)
    }

    fn activate(store: &mut FileStore, record: &Record) -> Result<(), String> {
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

    fn run_helper() -> Result<(), String> {
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

    fn verify_restored(record: &Record) -> Result<(), String> {
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

    struct Holder(Child);
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

    fn test_cases() -> Result<(), String> {
        if !Command::new("/bin/launchctl")
            .args(["print", "system/com.naab.dns-recovery"])
            .stdout(Stdio::null())
            .status()
            .map_err(|e| e.to_string())?
            .success()
        {
            return Err("The independent recovery task is not installed".into());
        }
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
        require_ci()?;
        let args: Vec<_> = std::env::args().skip(1).collect();
        let result = if args == ["--hold"] {
            hold()
        } else if args == ["--run"] {
            test_cases()
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
