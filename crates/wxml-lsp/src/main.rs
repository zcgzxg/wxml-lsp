//! Entry point for the `wxml-lsp` binary.
//!
//! `--stdio` runs the language server over stdin/stdout. Every other mode
//! prints to the terminal and exits; nothing except JSON-RPC frames is ever
//! written to the server's stdout.

use std::process::ExitCode;
use std::sync::Arc;

use wxml_lsp::backend_node::{NodeBackend, NodeBackendConfig};
use wxml_lsp::project_host::ProjectState;
use wxml_lsp::{run_stdio, run_stdio_with_host, version};
use wxml_types::{CapabilityStatus, HoverResult, MarkupKind, MockBackend, TypeQuality};

const USAGE: &str = "\
Usage: wxml-lsp [--stdio [--mock]]

Without arguments the server prints this usage. `--version` prints the build
version and `--help` prints this message.

Options:
  --stdio      serve the Language Server Protocol over stdin/stdout
  --mock       use the in-process mock backend (requires --stdio)
  --version    print the build version
  --help       print this message

Environment:
  WXML_NODE                node executable name or absolute path (default: node)
  WXML_TYPE_BACKEND        absolute server.js path; relative values are rejected
  WXML_BACKEND_DIR         absolute backend directory; relative values are rejected
  WXML_ANALYZE_TIMEOUT_MS  per-request timeout (default: 15000)
  WXML_TEMPLATE_CHECK      off | pragma | inherit (default: pragma)

Without WXML_TYPE_BACKEND the server probes only executable-relative install
layouts (backends/typescript/dist/server.js or wxml-type-backend/server.js)
and never the process working directory. Set an absolute path for development.";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Parses arguments and runs the requested mode.
fn run() -> Result<ExitCode, String> {
    let mut args = std::env::args().skip(1);
    let Some(argument) = args.next() else {
        eprintln!("wxml-lsp {}", version());
        eprintln!("error: a mode is required\n\n{USAGE}");
        return Ok(ExitCode::FAILURE);
    };
    let extra = args.next();
    match (argument.as_str(), extra.as_deref()) {
        ("--version" | "-V", None) => {
            println!("wxml-lsp {}", version());
            Ok(ExitCode::SUCCESS)
        }
        ("--help" | "-h", None) => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        ("--stdio", None) => serve_real(),
        ("--stdio", Some("--mock")) => serve_mock(),
        ("--stdio", Some(other)) => Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
        (other, None) => {
            eprintln!("error: unexpected argument `{other}`\n\n{USAGE}");
            Ok(ExitCode::FAILURE)
        }
        (other, Some(extra)) => {
            eprintln!("error: unexpected arguments `{other}` and `{extra}`\n\n{USAGE}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Runs the stdio server with the in-process mock backend (no project).
fn serve_mock() -> Result<ExitCode, String> {
    let backend = MockBackend::new().with_hover(HoverResult {
        contents: "string".to_owned(),
        markup: MarkupKind::Markdown,
        range: None,
        status: CapabilityStatus::Supported,
        quality: TypeQuality::Known,
    });
    let outcome = run_stdio(Arc::new(backend)).map_err(|error| error.to_string())?;
    Ok(exit_code(outcome))
}

/// Runs the real project host and Node type backend over stdio.
fn serve_real() -> Result<ExitCode, String> {
    let host = Arc::new(ProjectState::new());
    let backend = Arc::new(NodeBackend::new(
        NodeBackendConfig::from_env(),
        host.clone(),
    ));
    let outcome = run_stdio_with_host(backend, host).map_err(|error| error.to_string())?;
    Ok(exit_code(outcome))
}

/// Maps a clean shutdown to the LSP exit code.
const fn exit_code(outcome: wxml_lsp::ServerOutcome) -> ExitCode {
    if outcome.clean_shutdown {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
