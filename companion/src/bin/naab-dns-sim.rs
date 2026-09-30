//! Safe, offline demonstration; intentionally has no real system DNS adapter.
use naab_companion::dns::system::{journal::FileJournal, simulation};
use std::path::PathBuf;

fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.is_empty() || arguments == ["--help"] || arguments == ["-h"] {
        println!("Offline DNS recovery simulation (no network or OS DNS changes)\nUsage: naab-dns-sim SCENARIO --output-dir NEW_DIRECTORY\nScenarios: normal, upstream-outage, local-failure, interrupted, conflict\nA new output directory is required for each run. Reports use simulated settings.");
        return Ok(());
    }
    if arguments.len() != 3 || arguments[1] != "--output-dir" {
        return Err("Use naab-dns-sim SCENARIO --output-dir NEW_DIRECTORY (see --help)".into());
    }
    if ![
        "normal",
        "upstream-outage",
        "local-failure",
        "interrupted",
        "conflict",
    ]
    .contains(&arguments[0].as_str())
    {
        return Err("Unknown simulation scenario (see --help)".into());
    }
    let directory = PathBuf::from(&arguments[2]);
    // create_dir refuses an existing location, including an outstanding journal.
    std::fs::create_dir(&directory)
        .map_err(|e| format!("Choose a new output directory under an existing parent: {e}"))?;
    let reports = simulation::run(&arguments[0], FileJournal::open(&directory)?)?;
    println!(
        "SIMULATION ONLY - no network or OS DNS changes\nScenario: {}",
        arguments[0]
    );
    for report in &reports {
        println!(
            "{:?}: {:?} ({:?})",
            report.action, report.state, report.reason
        );
        for target in &report.targets {
            println!(
                "  {} {:?}: {:?}",
                target.target.interface_id, target.target.family, target.outcome
            );
        }
        if report.report_write_failed {
            return Err("Could not save the local simulation report".into());
        }
    }
    println!(
        "Latest report: {}",
        directory.join("last-report.json").display()
    );
    if directory.join("last-incident.json").exists() {
        println!(
            "Latest incident: {}",
            directory.join("last-incident.json").display()
        );
    }
    if directory.join("recovery.json").exists() {
        println!("Recovery record retained: manual attention would be required on a real machine.");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("naab-dns-sim: {error}");
        std::process::exit(1);
    }
}
