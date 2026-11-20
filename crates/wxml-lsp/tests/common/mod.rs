//! Shared fixtures for integration tests: a temporary project root plus the
//! JSON-RPC frame reader and child wait used by the stdio protocol tests.

#![allow(dead_code, clippy::redundant_pub_crate)]

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

/// Upper bound for the header block of a single frame.
const MAX_HEADER_BYTES: usize = 8 * 1024;
/// Upper bound for one `Content-Length` framed body.
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// How long [`wait_with_timeout`] waits before killing the child.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

/// A temporary project directory removed on drop.
pub(super) struct TempProject {
    dir: TempDir,
}

impl TempProject {
    /// Creates a unique project directory under `TMPDIR` (or `/tmp/opencode`).
    ///
    /// `prefix` only labels the directory for debugging.
    pub(super) fn new(prefix: &str) -> Self {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let dir = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(base)
            .expect("temp project");
        Self { dir }
    }

    /// Returns the project root.
    pub(super) fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Writes one project file, creating parent directories.
    pub(super) fn write(&self, path: &str, text: &str) {
        let target = self.root().join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(target, text).expect("fixture");
    }

    /// Returns the `file://` URI of the project root.
    pub(super) fn root_uri(&self) -> String {
        format!("file://{}", self.root().display())
    }

    /// Returns the `file://` URI of a project-relative path.
    pub(super) fn uri(&self, path: &str) -> String {
        format!("file://{}", self.root().join(path).display())
    }
}

/// Reads one `Content-Length` framed JSON-RPC message from `reader`.
///
/// Returns `None` on a clean end of stream. Panics when the stream breaks the
/// framing contract: a header block above [`MAX_HEADER_BYTES`], a missing or
/// duplicate `Content-Length`, a body above [`MAX_MESSAGE_BYTES`], a truncated
/// body, or a body that is not JSON.
pub(super) fn read_frame(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    let mut header_bytes = 0;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).expect("frame header read");
        if read == 0 {
            assert!(length.is_none(), "stream ended inside frame headers");
            return None;
        }
        header_bytes += read;
        assert!(
            header_bytes <= MAX_HEADER_BYTES,
            "frame header exceeds {MAX_HEADER_BYTES} bytes"
        );
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            assert!(length.is_none(), "duplicate Content-Length in frame header");
            let parsed = value
                .trim()
                .parse::<usize>()
                .expect("Content-Length is a byte count");
            assert!(
                parsed <= MAX_MESSAGE_BYTES,
                "frame body exceeds {MAX_MESSAGE_BYTES} bytes"
            );
            length = Some(parsed);
        }
    }
    let length = length.expect("Content-Length is required");
    let mut body = vec![0_u8; length];
    reader
        .read_exact(&mut body)
        .expect("frame body is complete");
    Some(serde_json::from_slice(&body).expect("frame body is JSON"))
}

/// Waits for `child` to exit, killing and reaping it once the deadline passes.
pub(super) fn wait_with_timeout(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("server did not exit before the {EXIT_TIMEOUT:?} timeout");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
