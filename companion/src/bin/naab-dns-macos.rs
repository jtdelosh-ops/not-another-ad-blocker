//! Read-only macOS DNS discovery preview.
use naab_companion::dns::system::macos;

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 1 && args[0] == "inspect" {
        match macos::inspect().and_then(|services| {
            serde_json::to_string_pretty(&services).map_err(|error| error.to_string())
        }) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("naab-dns-macos: {error}");
                std::process::exit(1);
            }
        }
    } else if args.is_empty() || args == ["--help"] {
        println!("Read-only macOS DNS discovery\n  naab-dns-macos inspect\n\nLists network services and explicitly configured DNS servers. Does not change DNS or identify the active resolver.");
    } else {
        eprintln!("Unknown command; use --help");
        std::process::exit(2);
    }
}
