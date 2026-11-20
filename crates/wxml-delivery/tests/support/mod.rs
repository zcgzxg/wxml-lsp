//! Shared fixtures for installer integration tests.

#![allow(dead_code)]
#![allow(unreachable_pub)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![allow(clippy::assert_is_empty)]

pub mod legacy_fixture;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A unique test directory below `/tmp/opencode` when available.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(name: &str) -> Self {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let path = base.join(format!(
            "wxml-it-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&path).expect("create test dir");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Builds one schema v1 legacy release fixture into `output` and returns the
/// versioned release directory. Test-only: the production v1 packager is gone.
pub fn pack(output: &Path, version: &str) -> PathBuf {
    legacy_fixture::build_legacy_release(output, version, None).release_dir
}

#[derive(Clone)]
struct Route {
    body: Vec<u8>,
    declared_len: Option<usize>,
}

/// A loopback HTTP fixture server. Never binds a public interface.
pub struct FixtureServer {
    address: std::net::SocketAddr,
    hits: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FixtureServer {
    /// Serves files from `directory` on each request.
    pub fn directory(directory: &Path) -> Self {
        let directory = directory.to_path_buf();
        Self::start(move |path| {
            let relative = path.trim_start_matches('/');
            if relative.contains("..") || relative.is_empty() {
                return None;
            }
            let file = directory.join(relative);
            std::fs::read(file).ok().map(|body| Route {
                body,
                declared_len: None,
            })
        })
    }

    /// Serves fixed routes; keys are request paths without a leading slash.
    pub fn routes(routes: BTreeMap<String, Vec<u8>>) -> Self {
        let routes: BTreeMap<String, Route> = routes
            .into_iter()
            .map(|(path, body)| {
                (
                    path,
                    Route {
                        body,
                        declared_len: None,
                    },
                )
            })
            .collect();
        Self::start(move |path| routes.get(path.trim_start_matches('/')).cloned())
    }

    /// Serves a valid index but truncates the archive response.
    pub fn truncated_archive(output: &Path) -> Self {
        let index = std::fs::read(output.join("index.json")).expect("read index");
        let parsed: serde_json::Value = serde_json::from_slice(&index).expect("parse index");
        let archive_name = parsed["releases"][0]["archive"]
            .as_str()
            .expect("archive name")
            .to_owned();
        let archive = std::fs::read(output.join(&archive_name)).expect("read archive");
        let declared = archive.len();
        let truncated = archive[..archive.len() / 2].to_vec();
        let mut routes = BTreeMap::new();
        routes.insert(
            "index.json".to_owned(),
            Route {
                body: index,
                declared_len: None,
            },
        );
        routes.insert(
            archive_name,
            Route {
                body: truncated,
                declared_len: Some(declared),
            },
        );
        Self::start(move |request| routes.get(request.trim_start_matches('/')).cloned())
    }

    /// Serves one route with a declared length larger than the bytes sent.
    pub fn truncated(path: &str, body: Vec<u8>, declared_len: usize) -> Self {
        let mut routes = BTreeMap::new();
        routes.insert(
            path.to_owned(),
            Route {
                body,
                declared_len: Some(declared_len),
            },
        );
        Self::start(move |request| routes.get(request.trim_start_matches('/')).cloned())
    }

    fn start<F>(lookup: F) -> Self
    where
        F: Fn(&str) -> Option<Route> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        listener.set_nonblocking(true).expect("nonblocking");
        let address = listener.local_addr().expect("address");
        let hits = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let hits_thread = Arc::clone(&hits);
        let stop_thread = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => handle_connection(stream, &lookup, &hits_thread),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            hits,
            stop,
            handle: Some(handle),
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}/", self.address)
    }

    pub fn hits(&self) -> Vec<String> {
        self.hits.lock().expect("hits lock").clone()
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_connection<F>(mut stream: TcpStream, lookup: &F, hits: &Arc<Mutex<Vec<String>>>)
where
    F: Fn(&str) -> Option<Route>,
{
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => request.extend_from_slice(&buffer[..read]),
        }
        if request.len() > 16 * 1024 {
            return;
        }
    }
    let text = String::from_utf8_lossy(&request);
    let path = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    hits.lock().expect("hits lock").push(path.to_owned());
    let Some(route) = lookup(path) else {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    };
    let length = route.declared_len.unwrap_or(route.body.len());
    let header =
        format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n");
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&route.body);
}

/// Returns the installer command with legacy/Node activation variables cleared.
pub fn installer() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wxml-lsp-install"));
    for name in [
        "WXML_LSP_BIN",
        "WXML_NODE",
        "WXML_TYPE_BACKEND",
        "WXML_BACKEND_DIR",
    ] {
        command.env_remove(name);
    }
    command
}

pub fn run(command: &mut Command) -> Output {
    command.output().expect("spawn installer")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed: status={:?}\nstdout={}\nstderr={}",
        output.status,
        stdout(output),
        stderr(output)
    );
}

pub fn assert_failure(output: &Output, needle: &str) {
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded: stdout={} stderr={}",
        stdout(output),
        stderr(output)
    );
    let combined = format!("{}{}", stdout(output), stderr(output));
    assert!(
        combined.to_lowercase().contains(&needle.to_lowercase()),
        "expected `{needle}` in output: {combined}"
    );
}
