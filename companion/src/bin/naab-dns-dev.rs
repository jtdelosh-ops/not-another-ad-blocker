use naab_companion::dns::{config::DnsConfig, diagnostics::Diagnostics, server};
use serde_json::json;
use std::io::{BufRead, Read};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

fn help() {
    println!(
        "NAAB DNS core — optional development resolver\n\
Usage: naab-dns-dev --config FILE [--check]\n\
--check validates configuration and compiles filters without opening sockets.\n\
Runtime commands: status, activity, clear, quit. Ctrl+C or stdin EOF also stops.\n\
No system DNS settings or native-host registration are changed."
    );
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
    while let Some(arg) = arguments.next() {
        if arg == "--help" || arg == "-h" {
            help();
            return Ok(());
        }
        if arg == "--config" && path.is_none() {
            path = Some(arguments.next().ok_or("--config needs a file path")?.into());
        } else if arg == "--check" && !check {
            check = true;
        } else {
            return Err("Unknown or repeated argument; use --help".into());
        }
    }
    let path = path.ok_or("Use --config FILE (or --help)")?;
    let (config, policy) = DnsConfig::load(&path)?;
    println!(
        "{}",
        json!({
            "event": if check { "validated" } else { "starting" },
            "listen": config.listen.to_string(),
            "upstreams": config.upstreams.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "compilation": policy.report()
        })
    );
    if check {
        return Ok(());
    }
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
