//! Structured Text from standard input, in the one layout
//! (`helmsman_iec::tidy`), to standard output:
//! `cargo run -p helmsman-iec --example tidy < main.st`.

use std::io::{Read, Write};

fn main() -> std::io::Result<()> {
    let mut src = String::new();
    std::io::stdin().read_to_string(&mut src)?;
    std::io::stdout().write_all(helmsman_iec::tidy::format(&src).as_bytes())
}
