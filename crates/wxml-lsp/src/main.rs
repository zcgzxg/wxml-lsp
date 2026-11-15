//! Entry point for the `wxml-lsp` binary.

use std::process::ExitCode;

use wxml_lsp::version;

const USAGE: &str = "\
Usage: wxml-lsp

The WXML language server is under construction. Run `wxml-lsp --version` to
print the build version.";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--version" | "-V") => {
            println!("wxml-lsp {}", version());
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("error: unexpected argument `{other}`\n\n{USAGE}");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("wxml-lsp {}", version());
            eprintln!("error: the language server is not implemented yet");
            ExitCode::FAILURE
        }
    }
}
