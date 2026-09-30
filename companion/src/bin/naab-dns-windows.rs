//! Source-only, time-limited Windows system DNS preview; never a native host.
use naab_companion::dns::{
    diagnostics::Diagnostics,
    server,
    system::{
        health::LiveProbe,
        journal::FileJournal,
        trial,
        windows::{self, PowerShell, WindowsPlatform},
        Check, Controller, Health, HealthProbe, Journal, Platform, Report, State,
    },
    DnsConfig,
};
use std::{collections::BTreeMap, net::SocketAddr, path::Path, sync::Arc, time::Duration};

const HELP: &str = "Windows DNS preview (source build, one adapter, maximum 5 minutes)
  naab-dns-windows inspect
  naab-dns-windows plan --adapter GUID
  naab-dns-windows serve --config PATH
  naab-dns-windows trial --adapter GUID --token TOKEN --upstreams IP:PORT,IP:PORT --seconds 300 --apply
  naab-dns-windows recover
  naab-dns-windows recover-if-needed
  naab-dns-windows recovery-dir

inspect and plan are read-only. Run serve in NORMAL PowerShell; it binds loopback UDP/TCP
port 53 for IPv4 and IPv6 without changing Windows DNS. Run trial/recover in an
ADMINISTRATOR PowerShell. trial saves the original settings before applying DNS,
monitors the separate resolver and restores on Ctrl+C, timeout or local failure.
Keep the helper window open. Install the optional recovery task before testing
forced helper termination or reboot; without it those cases need manual recover.
Do not use this preview on your everyday network before the controlled test gate.
Recovery/report files: the protected ProgramData\\NAAB-DNS-Preview directory.
Full guide: docs/windows-dns-preview.md";

fn arguments() -> Result<(String, BTreeMap<String, String>), String> {
    let mut args = std::env::args().skip(1);
    let action = args.next().unwrap_or_else(|| "--help".into());
    let mut options = BTreeMap::new();
    while let Some(key) = args.next() {
        let value = if key == "--apply" {
            "true".into()
        } else {
            args.next().ok_or("Missing option value")?
        };
        if options.insert(key, value).is_some() {
            return Err("Duplicate option".into());
        }
    }
    let expected: &[&str] = match action.as_str() {
        "--help" | "inspect" | "recover" | "recover-if-needed" | "recovery-dir" => &[],
        "serve" => &["--config"],
        "plan" => &["--adapter"],
        "trial" => &[
            "--adapter",
            "--token",
            "--upstreams",
            "--seconds",
            "--apply",
        ],
        _ => return Err("Unknown command; use --help".into()),
    };
    if options.len() != expected.len() || expected.iter().any(|k| !options.contains_key(*k)) {
        return Err("Unexpected or missing option; use --help".into());
    }
    Ok((action, options))
}

fn show(report: &Report) -> Result<(), String> {
    println!(
        "{:?}: {:?} ({:?})",
        report.action, report.state, report.reason
    );
    for target in &report.targets {
        println!("  {:?}: {:?}", target.target.family, target.outcome);
    }
    if report.report_write_failed {
        eprintln!("Warning: the diagnostic report could not be saved. Recovery uses its separate journal.");
    }
    if report.state == State::RecoveryRequired {
        Err("Recovery needs attention. Keep the saved record; reconnect the original network if needed, then run 'naab-dns-windows recover' as Administrator.".into())
    } else {
        Ok(())
    }
}

struct Offline;
impl HealthProbe for Offline {
    fn check(&mut self, _: &[naab_companion::dns::system::Change]) -> Health {
        Health {
            local: Check::Unknown,
            upstream: Check::Unknown,
        }
    }
}

fn run() -> Result<(), String> {
    let (action, args) = arguments()?;
    if action == "--help" {
        println!("{HELP}");
        return Ok(());
    }
    if !cfg!(windows) {
        return Err("This preview requires Windows".into());
    }
    if action == "inspect" {
        println!(
            "{}",
            serde_json::to_string_pretty(&windows::inspect()?).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if action == "plan" {
        let mut platform = WindowsPlatform::new(PowerShell, Some(args["--adapter"].clone()))?;
        let snapshot = platform.snapshot()?;
        println!("Read-only plan: these settings would be saved and restored after the trial.");
        println!(
            "{}",
            serde_json::to_string_pretty(&snapshot).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if action == "serve" {
        windows::require_unelevated_resolver()?;
        let (config, policy) = DnsConfig::load(Path::new(&args["--config"]))?;
        let upstreams = config
            .upstreams
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let token = format!("{:032x}", rand::random::<u128>());
        println!("Starting loopback DNS on 127.0.0.1:53 and [::1]:53. This window does not change Windows DNS.");
        println!("In an Administrator PowerShell, after selecting an adapter from inspect:\n.\\companion\\target\\debug\\naab-dns-windows.exe trial --adapter ADAPTER_GUID --token {token} --upstreams {upstreams} --seconds 300 --apply");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let diagnostics = Arc::new(Diagnostics::new(config.activity_capacity));
        return runtime
            .block_on(server::serve_system_preview(
                config,
                Arc::new(policy),
                diagnostics,
                token,
                async {
                    let _ = tokio::signal::ctrl_c().await;
                },
            ))
            .map_err(|e| e.to_string());
    }
    if action == "recovery-dir" {
        println!("{}", windows::prepare_journal()?.display());
        return Ok(());
    }
    if action == "recover" || action == "recover-if-needed" {
        let directory = windows::prepare_journal()?;
        let mut journal = FileJournal::open(&directory)?;
        if action == "recover-if-needed" && journal.load()?.is_none() {
            return Ok(());
        }
        let platform = WindowsPlatform::new(PowerShell, None)?;
        let mut controller = Controller::new(platform, journal, Offline);
        println!("Offline recovery using {}", directory.display());
        return show(&controller.recover());
    }
    // Validate everything before even creating the protected recovery directory.
    let seconds: u64 = args["--seconds"]
        .parse()
        .map_err(|_| "seconds must be 30–300")?;
    if !(30..=300).contains(&seconds) {
        return Err("seconds must be 30–300".into());
    }
    let upstreams: Vec<SocketAddr> = args["--upstreams"]
        .split(',')
        .map(|v| {
            v.parse()
                .map_err(|_| "Use explicit upstream IP:port values")
        })
        .collect::<Result<_, _>>()?;
    let config: DnsConfig = serde_json::from_value(serde_json::json!({"upstreams":upstreams}))
        .map_err(|e| e.to_string())?;
    config.validate()?;
    if upstreams
        .iter()
        .any(|s| s.ip().to_canonical().is_loopback())
    {
        return Err("System preview upstreams must not be loopback addresses".into());
    }
    let probe = LiveProbe::new(args["--token"].clone(), upstreams, 53)?;
    let platform = WindowsPlatform::new(PowerShell, Some(args["--adapter"].clone()))?;
    let directory = windows::prepare_journal()?;
    let mut journal = FileJournal::open(&directory)?;
    if journal.load()?.is_some() {
        return Err(
            "An unfinished session exists. Run recover before starting another trial.".into(),
        );
    }
    let mut controller = Controller::new(platform, journal, probe);
    println!("Guarded trial, at most {seconds} seconds plus in-flight checks/restoration. Keep this window open. Ctrl+C restores DNS.");
    println!("Recovery and reports: {}", directory.display());
    trial::run(&mut controller, Duration::from_secs(seconds), show, || {
        println!("Restoring saved DNS settings; please wait for the final status.");
    })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("naab-dns-windows: {error}");
        std::process::exit(1);
    }
}
