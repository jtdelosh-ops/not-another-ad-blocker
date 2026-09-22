use naab_companion::dns::{
    blocklist::CompileReport, config::DnsConfig, diagnostics::Diagnostics, server,
};
use serde_json::json;
use std::io::{BufRead, Read};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

fn help() {
    println!(
        "NAAB DNS core — optional development resolver\n\
Usage: naab-dns-dev --config FILE [--check [--pretty]]\n\
--check validates configuration and prints a JSON DNS coverage report without opening sockets.\n\
--pretty makes --check easier to read while retaining JSON as the default.\n\
Runtime commands: status, activity, clear, quit. Ctrl+C or stdin EOF also stops.\n\
No system DNS settings or native-host registration are changed."
    );
}

fn print_pretty_report(config: &DnsConfig, report: &CompileReport) {
    let coverage = &report.coverage;
    let status = if coverage.list_blocks_suppressed {
        "SAFE MODE — list blocking suppressed"
    } else if coverage.effective_block_rules == 0 {
        "NO ACTIVE LIST BLOCKS"
    } else {
        "READY — list blocking active"
    };
    println!("DNS coverage report");
    println!("Status: {status}");
    println!("Listener: {}", config.listen);
    println!(
        "Upstreams: {}",
        config
            .upstreams
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!();
    println!("Input lines:             {}", coverage.input_lines);
    println!(
        "Candidate block lines:   {}",
        coverage.candidate_block_lines
    );
    println!(
        "Unique block rules:      {}",
        coverage.candidate_block_rules
    );
    println!(
        "Active block rules:      {}",
        coverage.effective_block_rules
    );
    println!("Allow rules:             {}", coverage.list_allow_rules);
    println!(
        "Conservative exceptions: {} lines",
        coverage.conservative_allow_lines
    );
    println!("Ignored lines:           {}", coverage.ignored_lines);
    println!("Unsupported lines:       {}", coverage.unsupported_lines);
    println!();
    if report.diagnostics.is_empty() {
        println!("Diagnostics: none");
    } else {
        println!("Diagnostics:");
        for diagnostic in &report.diagnostics {
            println!(
                "  {}:{} — {} ({})",
                diagnostic.source, diagnostic.line, diagnostic.message, diagnostic.rule
            );
        }
        if report.diagnostics_omitted > 0 {
            println!("  ... {} more omitted", report.diagnostics_omitted);
        }
    }
    if report.suppression_reasons_total > 0 {
        println!(
            "Safety suppression reasons: {} ({} shown)",
            report.suppression_reasons_total,
            report.suppression_reasons.len()
        );
        for reason in &report.suppression_reasons {
            println!(
                "  {}:{} — {} ({})",
                reason.source, reason.line, reason.message, reason.rule
            );
        }
    }
}

fn commands() -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel(4);
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(std::io::stdin());
        loop {
            let mut line = Vec::new();
            match (&mut reader).take(257).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) if line.len() > 256 => break,
                Ok(_) => {
                    let Ok(line) = String::from_utf8(line) else {
                        break;
                    };
                    if sender.blocking_send(line.trim().to_owned()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    receiver
}

async fn run() -> Result<(), String> {
    let mut arguments = std::env::args_os().skip(1);
    let mut path: Option<PathBuf> = None;
    let mut check = false;
    let mut pretty = false;
    while let Some(arg) = arguments.next() {
        if arg == "--help" || arg == "-h" {
            help();
            return Ok(());
        }
        if arg == "--config" && path.is_none() {
            path = Some(arguments.next().ok_or("--config needs a file path")?.into());
        } else if arg == "--check" && !check {
            check = true;
        } else if arg == "--pretty" && !pretty {
            pretty = true;
        } else {
            return Err("Unknown or repeated argument; use --help".into());
        }
    }
    if pretty && !check {
        return Err("--pretty requires --check".into());
    }
    let path = path.ok_or("Use --config FILE (or --help)")?;
    let (config, policy) = DnsConfig::load(&path)?;
    if check {
        if pretty {
            print_pretty_report(&config, policy.report());
        } else {
            println!(
                "{}",
                json!({
                    "event": "validated",
                    "listen": config.listen.to_string(),
                    "upstreams": config.upstreams.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "compilation": policy.report()
                })
            );
        }
        return Ok(());
    }
    println!(
        "{}",
        json!({
            "event": "starting",
            "listen": config.listen.to_string(),
            "upstreams": config.upstreams.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "compilation": policy.report()
        })
    );
    eprintln!(
        "DNS development resolver starting. Commands: status, activity, clear, quit. Ctrl+C stops."
    );
    let diagnostics = Arc::new(Diagnostics::new(config.activity_capacity));
    let (stop, stopped) = oneshot::channel();
    let task = server::serve(config, Arc::new(policy), diagnostics.clone(), async {
        let _ = stopped.await;
    });
    tokio::pin!(task);
    let mut commands = commands();
    let signal = tokio::signal::ctrl_c();
    tokio::pin!(signal);
    loop {
        tokio::select! {
            result = &mut task => return result.map_err(|error| error.to_string()),
            result = &mut signal => { result.map_err(|error| error.to_string())?; break; }
            command = commands.recv() => {
                match command.as_deref() {
                    None | Some("quit") => break,
                    Some("status") => {
                        let snapshot = diagnostics.snapshot();
                        println!("{}", json!({"event":"status", "total":snapshot.total,"outcomes":snapshot.outcomes,"retained":snapshot.recent.len()}));
                    }
                    Some("activity") => println!("{}", json!({"event":"activity","data":diagnostics.snapshot()})),
                    Some("clear") => { diagnostics.clear(); println!("{}", json!({"event":"activity-cleared"})); }
                    Some("") => {},
                    _ => eprintln!("Commands: status, activity, clear, quit"),
                }
            }
        }
    }
    let _ = stop.send(());
    task.await.map_err(|error| error.to_string())?;
    println!("{}", json!({"event":"stopped"}));
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("naab-dns-dev: {error}");
        std::process::exit(1);
    }
}
