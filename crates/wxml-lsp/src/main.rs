//! Entry point for the `wxml-lsp` binary.
//!
//! `--stdio` runs the language server over stdin/stdout. Every other mode
//! prints to the terminal and exits; nothing except JSON-RPC frames is ever
//! written to the server's stdout.

use std::process::ExitCode;
use std::sync::Arc;

use wxml_lsp::backend_node::{NodeBackend, NodeBackendConfig};
use wxml_lsp::project_host::ProjectState;
use wxml_lsp::{run_stdio, run_stdio_native, run_stdio_with_host, version};
use wxml_types::{CapabilityStatus, HoverResult, MarkupKind, MockBackend, TypeQuality};

const USAGE: &str = "\
Usage: wxml-lsp [--stdio [--backend node|native] [--mock]]

Without arguments the server prints this usage. `--version` prints the build
version and `--help` prints this message.

Options:
  --stdio              serve the Language Server Protocol over stdin/stdout
  --backend <backend>  type backend for --stdio: node (default) or native
  --mock               use the in-process mock backend (requires --stdio)
  --version            print the build version
  --help               print this message

`--backend native` is the opt-in native engine. It requires the running
executable to be installed as <package>/bin/wxml-lsp with a verified
manifest.json, engine/lib/tsc and the pinned standard-library and
mini-program typings. Discovery or asset verification failure fails closed
before the LSP handshake and never falls back to Node.

Environment:
  WXML_NODE                node executable name or absolute path (default: node)
  WXML_TYPE_BACKEND        absolute server.js path; relative values are rejected
  WXML_BACKEND_DIR         absolute backend directory; relative values are rejected
  WXML_ANALYZE_TIMEOUT_MS  per-request timeout (default: 15000)
  WXML_TEMPLATE_CHECK      off | pragma | inherit (default: pragma)

Without WXML_TYPE_BACKEND the server probes only executable-relative install
layouts (backends/typescript/dist/server.js or wxml-type-backend/server.js)
and never the process working directory. Set an absolute path for development.";

/// One parsed command-line action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Print the build version.
    Version,
    /// Print usage.
    Help,
    /// Serve with the default Node backend.
    Stdio,
    /// Serve with the in-process mock backend.
    Mock,
    /// Serve with the opt-in native backend.
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendChoice {
    Node,
    Native,
}

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(args.iter().map(String::as_str)) {
        Ok(Action::Version) => {
            println!("wxml-lsp {}", version());
            Ok(ExitCode::SUCCESS)
        }
        Ok(Action::Help) => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Ok(Action::Stdio) => serve_real(),
        Ok(Action::Mock) => serve_mock(),
        Ok(Action::Native) => serve_native(),
        Err(message) => {
            if args.is_empty() {
                eprintln!("wxml-lsp {}", version());
            }
            eprintln!("error: {message}\n\n{USAGE}");
            Ok(ExitCode::FAILURE)
        }
    }
}

fn parse_args<'a>(mut args: impl Iterator<Item = &'a str>) -> Result<Action, String> {
    let Some(first) = args.next() else {
        return Err("a mode is required".to_owned());
    };
    match first {
        "--version" | "-V" => {
            ensure_no_extra(args)?;
            Ok(Action::Version)
        }
        "--help" | "-h" => {
            ensure_no_extra(args)?;
            Ok(Action::Help)
        }
        "--stdio" => parse_stdio(args),
        other => Err(format!("unexpected argument `{other}`")),
    }
}

fn ensure_no_extra<'a>(mut args: impl Iterator<Item = &'a str>) -> Result<(), String> {
    if let Some(extra) = args.next() {
        return Err(format!("unexpected argument `{extra}`"));
    }
    Ok(())
}

fn parse_stdio<'a>(mut args: impl Iterator<Item = &'a str>) -> Result<Action, String> {
    let mut backend: Option<BackendChoice> = None;
    let mut mock = false;
    while let Some(argument) = args.next() {
        match argument {
            "--mock" => {
                if mock {
                    return Err("duplicate `--mock`".to_owned());
                }
                mock = true;
            }
            "--backend" => {
                if backend.is_some() {
                    return Err("duplicate `--backend`".to_owned());
                }
                let Some(value) = args.next() else {
                    return Err(
                        "`--backend` requires a value; expected `node` or `native`".to_owned()
                    );
                };
                match value {
                    "node" => backend = Some(BackendChoice::Node),
                    "native" => backend = Some(BackendChoice::Native),
                    other => {
                        return Err(format!(
                            "unsupported backend `{other}`; expected `node` or `native`"
                        ));
                    }
                }
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }

    match (backend, mock) {
        (Some(BackendChoice::Native), false) => Ok(Action::Native),
        (None | Some(BackendChoice::Node), false) => Ok(Action::Stdio),
        (None, true) => Ok(Action::Mock),
        (Some(_), true) => Err("`--mock` cannot be combined with `--backend`".to_owned()),
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

/// Runs the opt-in native backend over stdio.
///
/// Discovery and sandbox validation fail before `initialize`; this function
/// never constructs the Node backend.
fn serve_native() -> Result<ExitCode, String> {
    let outcome = run_stdio_native().map_err(|error| error.to_string())?;
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

#[cfg(test)]
mod tests {
    use super::{Action, parse_args};

    fn parse<'a>(args: impl IntoIterator<Item = &'a str>) -> Result<Action, String> {
        parse_args(args.into_iter())
    }

    #[test]
    fn stdio_defaults_to_node() {
        assert_eq!(parse(["--stdio"]), Ok(Action::Stdio));
    }

    #[test]
    fn parses_stdio_backend_native() {
        assert_eq!(
            parse(["--stdio", "--backend", "native"]),
            Ok(Action::Native)
        );
    }

    #[test]
    fn parses_explicit_node_backend() {
        assert_eq!(parse(["--stdio", "--backend", "node"]), Ok(Action::Stdio));
    }

    #[test]
    fn parses_mock_and_rejects_mixing_it_with_a_backend() {
        assert_eq!(parse(["--stdio", "--mock"]), Ok(Action::Mock));
        assert!(parse(["--stdio", "--mock", "--backend", "native"]).is_err());
        assert!(parse(["--stdio", "--backend", "node", "--mock"]).is_err());
    }

    #[test]
    fn rejects_unknown_backend() {
        let error = parse(["--stdio", "--backend", "tsgo"]).unwrap_err();
        assert!(error.contains("unsupported backend `tsgo`"), "{error}");
    }

    #[test]
    fn rejects_duplicate_and_incomplete_backend_flags() {
        assert!(parse(["--stdio", "--backend"]).is_err());
        assert!(parse(["--stdio", "--backend", "node", "--backend", "native"]).is_err());
    }

    #[test]
    fn version_and_help_accept_no_extra_arguments() {
        assert_eq!(parse(["--version"]), Ok(Action::Version));
        assert_eq!(parse(["--help"]), Ok(Action::Help));
        assert!(parse(["--version", "--stdio"]).is_err());
        assert!(parse(["--help", "extra"]).is_err());
    }
}
