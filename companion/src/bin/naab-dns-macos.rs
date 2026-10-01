//! Read-only macOS DNS discovery preview.
use naab_companion::dns::system::{macos, macos_preflight};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 1 && (args[0] == "inspect" || args[0] == "preflight") {
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
        println!("Read-only macOS DNS discovery\n  naab-dns-macos inspect\n  naab-dns-macos preflight\n\nPreflight reports the primary IPv4 route and macOS resolver entries. Neither command changes DNS or approves a trial.");
    } else {
        eprintln!("Unknown command; use --help");
        std::process::exit(2);
    }
}
