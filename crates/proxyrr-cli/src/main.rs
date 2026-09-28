//! `proxyrr`: interfaz de línea de comandos de ProxyRR.

use clap::Parser;

/// Proxy HTTP(S) de depuración multiplataforma.
#[derive(Debug, Parser)]
#[command(name = "proxyrr", version, about)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
    println!(
        "proxyrr {}: todavía no hay comandos. Ver `proxyrr --help`.",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
