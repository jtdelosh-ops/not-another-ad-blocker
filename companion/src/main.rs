use std::io;

fn main() {
    // Rust's standard handles use byte I/O, including on Windows. Never print
    // status/logging to stdout: that handle belongs exclusively to the protocol.
    let result = naab_companion::serve(&mut io::stdin().lock(), &mut io::stdout().lock());
    if let Err(error) = result {
        eprintln!("naab-companion: {error}");
        std::process::exit(1);
    }
}
