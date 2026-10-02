//! macOS DNS discovery and guarded offline recovery preview.
use naab_companion::dns::system::{macos, macos_preflight};

#[cfg(target_os = "macos")]
fn recover(only_if_needed: bool) -> Result<(), String> {
    use naab_companion::dns::system::macos_recovery::{self, FileStore, NativeSettings, Store};
    let mut store = FileStore::open_fixed()?;
    if only_if_needed && store.load()?.is_none() {
        println!("No pending Mac DNS recovery record.");
        return Ok(());
    }
    let report = macos_recovery::recover(&mut store, &mut NativeSettings);
    let json = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    println!("{json}");
    if report.recovery_pending {
        Err("Mac DNS recovery needs attention; the protected record was retained".into())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
fn recover(_: bool) -> Result<(), String> {
    Err("Mac DNS recovery is available only on macOS".into())
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 1 && (args[0] == "recover" || args[0] == "recover-if-needed") {
        if let Err(error) = recover(args[0] == "recover-if-needed") {
            eprintln!("naab-dns-macos: {error}");
            std::process::exit(1);
        }
    } else if args.len() == 1 && (args[0] == "inspect" || args[0] == "preflight") {
        let result = if args[0] == "inspect" {
            macos::inspect().and_then(|services| {
                serde_json::to_string_pretty(&services).map_err(|error| error.to_string())
            })
        } else {
            macos_preflight::preflight().and_then(|report| {
                serde_json::to_string_pretty(&report).map_err(|error| error.to_string())
            })
        };
        match result {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("naab-dns-macos: {error}");
                std::process::exit(1);
            }
        }
    } else if args.is_empty() || args == ["--help"] {
        println!("macOS DNS preview\n  naab-dns-macos inspect\n  naab-dns-macos preflight\n  sudo naab-dns-macos recover\n  sudo naab-dns-macos recover-if-needed\n\nInspect and preflight are read-only. Recovery uses a protected, fixed local record and refuses to overwrite changed DNS or network context. No Mac trial/apply command is available yet.");
    } else {
        eprintln!("Unknown command; use --help");
        std::process::exit(2);
    }
}
