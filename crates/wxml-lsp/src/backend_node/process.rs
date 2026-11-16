//! Node process transport for the C1 backend.
//!
//! The transport is deliberately isolated behind [`NodeTransport`] so the
//! `NodeBackend` can be tested with a fake transport and so a future process
//! supervisor can replace the stdio implementation.

use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

use super::config::NodeBackendConfig;
use super::protocol::{self, ProtocolError};

/// How long [`StdioNode::shutdown`] waits for a graceful exit.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

/// Poll interval for process exit.
const EXIT_POLL: Duration = Duration::from_millis(10);

/// Maximum bytes kept from one stderr line.
const STDERR_LINE_BYTES: usize = 64 * 1024;

/// Number of stdout response lines buffered between the reader and requests.
pub(crate) const STDOUT_QUEUE_CAPACITY: usize = 4;

/// A transport failure.
#[derive(Debug, Error)]
pub enum NodeTransportError {
    /// The Node process could not be started.
    #[error("node process could not be started")]
    Spawn(#[source] io::Error),
    /// Process I/O failed.
    #[error("node transport failed")]
    Io(#[source] io::Error),
    /// The request exceeded its deadline.
    #[error("node request timed out")]
    Timeout,
    /// The response violated the NDJSON/C1 protocol.
    #[error("node protocol error")]
    Protocol(#[source] ProtocolError),
    /// The process exited before answering.
    #[error("node process exited")]
    Exited,
    /// The trusted backend paths are missing or invalid.
    #[error("node type backend is not configured")]
    NotConfigured,
    /// One response line exceeded the configured cap.
    #[error("node response line is too large")]
    LineTooLarge,
}

impl From<ProtocolError> for NodeTransportError {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

/// A request/response transport to the Node backend.
pub trait NodeTransport: Send {
    /// Writes one NDJSON request line and returns the matching response line.
    fn request(&self, line: &str, timeout: Duration) -> Result<String, NodeTransportError>;

    /// Sends `shutdown`, waits briefly, then kills the process.
    fn shutdown(&self);
}

/// A real `node server.js` process speaking NDJSON on stdin/stdout.
pub struct StdioNode {
    child: Mutex<Child>,
    stdin: Arc<Mutex<ChildStdin>>,
    lines: Mutex<Receiver<Result<String, NodeTransportError>>>,
    next_id: AtomicU64,
}

impl fmt::Debug for StdioNode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StdioNode")
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

impl StdioNode {
    /// Starts the process and validates its `ping` response.
    ///
    /// # Errors
    ///
    /// Returns a spawn, I/O or protocol error when the process cannot be
    /// started or does not advertise protocol version 2.
    pub fn start(config: &NodeBackendConfig) -> Result<Self, NodeTransportError> {
        let mut child = spawn_command(config)?
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(NodeTransportError::Spawn)?;
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(NodeTransportError::Io(io::Error::other(
                "node stdin is unavailable",
            )));
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(NodeTransportError::Io(io::Error::other(
                "node stdout is unavailable",
            )));
        };
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(NodeTransportError::Io(io::Error::other(
                "node stderr is unavailable",
            )));
        };

        let (sender, receiver) = mpsc::sync_channel(STDOUT_QUEUE_CAPACITY);
        let max_line_bytes = config.max_line_bytes;
        if thread::Builder::new()
            .name("wxml-node-stdout".to_owned())
            .spawn(move || read_stdout(stdout, &sender, max_line_bytes))
            .is_err()
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(NodeTransportError::Io(io::Error::other(
                "node reader thread could not be started",
            )));
        }
        let _ = thread::Builder::new()
            .name("wxml-node-stderr".to_owned())
            .spawn(move || read_stderr(stderr));

        let node = Self {
            child: Mutex::new(child),
            stdin: Arc::new(Mutex::new(stdin)),
            lines: Mutex::new(receiver),
            next_id: AtomicU64::new(1),
        };
        let ping_id = "ping";
        let line = protocol::request_line(ping_id, "ping", None)?;
        let response = match node.request(&line, config.timeout) {
            Ok(response) => response,
            Err(error) => {
                node.shutdown();
                return Err(error);
            }
        };
        let result = match protocol::parse_response(&response, ping_id) {
            Ok(result) => result,
            Err(error) => {
                crate::server::log(format_args!(
                    "node ping rejected: {error}; line={}",
                    crate::handlers::log_safe_text(&response, 240)
                ));
                node.shutdown();
                return Err(NodeTransportError::Protocol(error));
            }
        };
        if let Err(error) = protocol::validate_ping(&result) {
            node.shutdown();
            return Err(NodeTransportError::Protocol(error));
        }
        Ok(node)
    }

    /// Kills the process if it is still running.
    fn kill(&self) {
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl NodeTransport for StdioNode {
    fn request(&self, line: &str, timeout: Duration) -> Result<String, NodeTransportError> {
        let deadline = Instant::now() + timeout;
        if let Err(error) =
            write_line_with_timeout(Arc::clone(&self.stdin), line.to_owned(), timeout)
        {
            if matches!(error, NodeTransportError::Timeout) {
                self.kill();
            }
            return Err(error);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            self.kill();
            return Err(NodeTransportError::Timeout);
        }
        let receiver = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        match receiver.recv_timeout(remaining) {
            Ok(Ok(line)) => Ok(line),
            Ok(Err(error)) => Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                drop(receiver);
                self.kill();
                Err(NodeTransportError::Timeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(NodeTransportError::Exited),
        }
    }

    fn shutdown(&self) {
        let id = format!("shutdown-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        if let Ok(line) = protocol::request_line(&id, "shutdown", None) {
            let _ =
                write_line_with_timeout(Arc::clone(&self.stdin), line, Duration::from_millis(100));
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        loop {
            let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            drop(child);
            thread::sleep(EXIT_POLL);
        }
    }
}

impl Drop for StdioNode {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Builds the Node command from trusted, absolute paths.
///
/// `--` terminates Node options so a path can never be interpreted as a flag.
fn spawn_command(config: &NodeBackendConfig) -> Result<Command, NodeTransportError> {
    let Some(server) = config.server.as_ref() else {
        return Err(NodeTransportError::NotConfigured);
    };
    let Some(backend_dir) = config.backend_dir.as_ref() else {
        return Err(NodeTransportError::NotConfigured);
    };
    if !server.is_absolute() || !backend_dir.is_absolute() {
        return Err(NodeTransportError::NotConfigured);
    }
    let mut command = Command::new(&config.node);
    command.arg("--").arg(server).current_dir(backend_dir);
    Ok(command)
}

/// Writes one NDJSON line on a helper thread so a Node process that stops
/// reading stdin cannot block the request worker forever.
#[allow(clippy::significant_drop_tightening)] // the guard covers the whole write
fn write_line_with_timeout<W>(
    writer: Arc<Mutex<W>>,
    line: String,
    timeout: Duration,
) -> Result<(), NodeTransportError>
where
    W: Write + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("wxml-node-stdin".to_owned())
        .spawn(move || {
            let mut writer = writer.lock().unwrap_or_else(PoisonError::into_inner);
            let result = writer
                .write_all(line.as_bytes())
                .and_then(|()| writer.write_all(b"\n"))
                .and_then(|()| writer.flush());
            let _ = sender.send(result);
        });
    if spawned.is_err() {
        return Err(NodeTransportError::Io(io::Error::other(
            "node writer thread could not be started",
        )));
    }
    match receiver.recv_timeout(timeout) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(NodeTransportError::Io(error)),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(NodeTransportError::Timeout),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(NodeTransportError::Exited),
    }
}

/// Reads stdout lines with a hard byte cap and forwards them.
fn read_stdout(
    stdout: impl io::Read,
    sender: &SyncSender<Result<String, NodeTransportError>>,
    max_line_bytes: usize,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = Vec::new();
        match read_line_bounded(&mut reader, &mut line, max_line_bytes) {
            Ok(0) => break,
            Ok(_) => {
                if line.is_empty() {
                    continue;
                }
                let Ok(text) = String::from_utf8(line) else {
                    let _ = sender.send(Err(NodeTransportError::Protocol(ProtocolError::Json)));
                    break;
                };
                let text = text.trim_end_matches(['\r', '\n']).to_owned();
                if text.is_empty() {
                    continue;
                }
                if sender.send(Ok(text)).is_err() {
                    break;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error));
                break;
            }
        }
    }
}

/// Reads stderr and logs it with a prefix; never writes to stdout.
fn read_stderr(stderr: std::process::ChildStderr) {
    let mut reader = BufReader::new(stderr);
    let mut logged_oversize = false;
    loop {
        let mut line = Vec::new();
        match read_line_bounded(&mut reader, &mut line, STDERR_LINE_BYTES) {
            Ok(0) => break,
            Ok(_) => {
                let safe = crate::handlers::log_safe_text(&String::from_utf8_lossy(&line), 200);
                if !safe.is_empty() {
                    crate::server::log(format_args!("[wxml-node] {safe}"));
                }
            }
            Err(NodeTransportError::LineTooLarge) => {
                // Keep draining so a chatty Node process cannot block on a full
                // stderr pipe; only the first oversized line is logged.
                if !logged_oversize {
                    crate::server::log(format_args!("[wxml-node] oversized stderr line truncated"));
                    logged_oversize = true;
                }
                if !finish_oversized_line(&mut reader, &line) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// Positions the reader at the next line after an oversized one.
///
/// [`read_line_bounded`] may have consumed the newline already when it was in
/// the same buffer chunk; only drain when the captured line is unterminated.
fn finish_oversized_line(reader: &mut impl BufRead, line: &[u8]) -> bool {
    line.ends_with(b"\n") || drain_to_newline(reader)
}

/// Discards bytes up to and including the next newline.
///
/// Returns `false` on EOF or a read error, so the caller stops draining.
fn drain_to_newline(reader: &mut impl BufRead) -> bool {
    loop {
        let Ok(available) = reader.fill_buf() else {
            return false;
        };
        if available.is_empty() {
            return false;
        }
        if let Some(index) = available.iter().position(|&byte| byte == b'\n') {
            reader.consume(index + 1);
            return true;
        }
        let length = available.len();
        reader.consume(length);
    }
}

/// Reads one line with a hard byte cap.
fn read_line_bounded(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    limit: usize,
) -> Result<usize, NodeTransportError> {
    let mut total = 0_usize;
    loop {
        let available = reader.fill_buf().map_err(NodeTransportError::Io)?;
        if available.is_empty() {
            return Ok(total);
        }
        let newline = available.iter().position(|&byte| byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        total += take;
        if total > limit {
            return Err(NodeTransportError::LineTooLarge);
        }
        if newline.is_some() {
            return Ok(total);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::io::Cursor;

    use super::*;

    #[test]
    fn default_line_cap_rejects_a_twelve_mebibyte_line() {
        let cap = crate::backend_node::config::DEFAULT_MAX_LINE_BYTES;
        assert_eq!(cap, 8 * 1024 * 1024);
        let mut data = vec![b'x'; 12 * 1024 * 1024];
        data.push(b'\n');
        let mut reader = Cursor::new(data);
        let mut line = Vec::new();
        assert!(matches!(
            read_line_bounded(&mut reader, &mut line, cap),
            Err(NodeTransportError::LineTooLarge)
        ));
    }

    #[test]
    fn bounded_lines_reject_oversized_input() {
        let mut reader = Cursor::new(b"123456789\n".to_vec());
        let mut line = Vec::new();
        assert!(matches!(
            read_line_bounded(&mut reader, &mut line, 5),
            Err(NodeTransportError::LineTooLarge)
        ));
        assert!(line.len() <= 10, "the error must stop further reads");
    }

    #[test]
    fn command_uses_end_of_options_and_absolute_paths() {
        let config = NodeBackendConfig {
            server: Some(std::path::PathBuf::from("/opt/wxml/dist/server.js")),
            backend_dir: Some(std::path::PathBuf::from("/opt/wxml")),
            ..NodeBackendConfig::default()
        };
        let command = spawn_command(&config).expect("command");
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["--", "/opt/wxml/dist/server.js"]);

        let relative = NodeBackendConfig {
            server: Some(std::path::PathBuf::from(
                "backends/typescript/dist/server.js",
            )),
            backend_dir: Some(std::path::PathBuf::from("/opt/wxml")),
            ..NodeBackendConfig::default()
        };
        assert!(matches!(
            spawn_command(&relative),
            Err(NodeTransportError::NotConfigured)
        ));
        let missing = NodeBackendConfig {
            server: None,
            backend_dir: None,
            ..NodeBackendConfig::default()
        };
        assert!(matches!(
            spawn_command(&missing),
            Err(NodeTransportError::NotConfigured)
        ));
    }

    #[test]
    fn stderr_drains_after_an_oversized_line() {
        // The newline was consumed together with the oversized line.
        let data = format!("{}\nnext\n", "x".repeat(64));
        let mut reader = Cursor::new(data.into_bytes());
        let mut line = Vec::new();
        assert!(matches!(
            read_line_bounded(&mut reader, &mut line, 8),
            Err(NodeTransportError::LineTooLarge)
        ));
        assert!(finish_oversized_line(&mut reader, &line));
        let mut next = Vec::new();
        assert_eq!(read_line_bounded(&mut reader, &mut next, 8).ok(), Some(5));
        assert_eq!(next, b"next\n");

        // The limit was reached before the newline; drain the remainder.
        let mut reader = Cursor::new(b"rest of line\nnext\n".to_vec());
        assert!(finish_oversized_line(&mut reader, b"partial"));
        let mut next = Vec::new();
        assert_eq!(read_line_bounded(&mut reader, &mut next, 8).ok(), Some(5));
        assert_eq!(next, b"next\n");
    }

    #[test]
    fn stdout_reader_backpressures_large_output() {
        let (sender, receiver) =
            mpsc::sync_channel::<Result<String, NodeTransportError>>(STDOUT_QUEUE_CAPACITY);
        let mut data = String::new();
        for index in 0..100 {
            if writeln!(data, "line{index}").is_err() {
                break;
            }
        }
        let handle = thread::spawn(move || read_stdout(Cursor::new(data), &sender, 1024));
        for index in 0..100 {
            let line = receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("line")
                .expect("utf-8 line");
            assert_eq!(line, format!("line{index}"));
        }
        handle.join().expect("reader");
    }

    #[cfg(unix)]
    #[test]
    fn write_timeout_does_not_block_on_a_full_pipe() {
        use std::os::unix::net::UnixStream;

        let (writer, reader) = UnixStream::pair().expect("pair");
        let writer = Arc::new(Mutex::new(writer));
        let line = "x".repeat(8 * 1024 * 1024);
        let started = Instant::now();
        let result = write_line_with_timeout(Arc::clone(&writer), line, Duration::from_millis(100));
        assert!(matches!(result, Err(NodeTransportError::Timeout)));
        assert!(started.elapsed() < Duration::from_secs(2));
        // Closing the read side unblocks the detached writer.
        drop(reader);
    }

    #[test]
    fn stdout_channel_is_bounded() {
        let (sender, _receiver) =
            mpsc::sync_channel::<Result<String, NodeTransportError>>(STDOUT_QUEUE_CAPACITY);
        for _ in 0..STDOUT_QUEUE_CAPACITY {
            sender.try_send(Ok("line".to_owned())).expect("capacity");
        }
        assert!(matches!(
            sender.try_send(Ok("extra".to_owned())),
            Err(mpsc::TrySendError::Full(_))
        ));
    }

    #[test]
    fn reads_complete_lines() {
        let mut reader = Cursor::new(b"one\ntwo\n".to_vec());
        let mut line = Vec::new();
        assert_eq!(
            read_line_bounded(&mut reader, &mut line, 8).expect("line"),
            4
        );
        assert_eq!(line, b"one\n".to_vec());
    }
}
