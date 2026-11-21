//! In-process `tsrs` transport and the read-only virtual filesystem bridge.
//!
//! [`InProcTransport`] owns the single [`tsrs_api::Session`] of one native
//! query session. Requests are dispatched synchronously on the calling thread;
//! the caller's [`Deadline`] is an upper bound on how long the caller waits, but
//! it cannot cancel work that already started (there is no way to preempt the
//! in-process checker).
//!
//! [`VfsHandle`] implements [`tsrs_vfs::FS`] over the current
//! [`VirtualFileSystem`]. Reads only consult the virtual tables, writes are
//! rejected, and `realpath` never touches the host filesystem.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde_json::Value;
use tsrs_api::{ApiError, ErrorKind, Handler, Response, Session, SessionOptions};
use tsrs_vfs::{Entries, FS, FileInfo, FileMode};

use crate::config::{DEFAULT_REQUEST_DEADLINE_MS, MAX_REQUEST_BODY_BYTES, SNAPSHOT_DEADLINE_MS};
use crate::input::BUNDLED_STANDARD_LIBRARY_ROOT;
use crate::native::vfs::VirtualFileSystem;
use crate::native::{NativeError, UnavailableReason};

/// Handshake/probe deadline (contract section 3.1).
pub const HANDSHAKE_DEADLINE_MS: u64 = 10_000;

/// Deadline for releasing one lease or snapshot.
pub const RELEASE_DEADLINE_MS: u64 = 1_000;

/// Budgets retained from the frozen F0-08 contract.
///
/// Only the request-size and deadline budgets the session enforces remain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLimits {
    /// Maximum outbound request body.
    pub request_bytes: usize,
    /// Default deadline for one native request, in milliseconds.
    pub request_deadline_ms: u64,
    /// Deadline for snapshot creation or update, in milliseconds.
    pub snapshot_deadline_ms: u64,
}

/// Budgets frozen by F0-08 (contract section 3.4).
pub const DEFAULT_FRAME_LIMITS: FrameLimits = FrameLimits {
    request_bytes: MAX_REQUEST_BODY_BYTES,
    request_deadline_ms: DEFAULT_REQUEST_DEADLINE_MS,
    snapshot_deadline_ms: SNAPSHOT_DEADLINE_MS,
};

/// Absolute point in time after which an operation must be abandoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    at: Instant,
    timeout: Duration,
}

impl Deadline {
    /// Handshake/probe deadline (10 s).
    #[must_use]
    pub fn handshake() -> Self {
        Self::after(Duration::from_millis(HANDSHAKE_DEADLINE_MS))
    }

    /// Default request deadline from the frozen budgets.
    #[must_use]
    pub fn request(limits: FrameLimits) -> Self {
        Self::after(Duration::from_millis(limits.request_deadline_ms))
    }

    /// Snapshot create/update deadline from the frozen budgets.
    #[must_use]
    pub fn snapshot(limits: FrameLimits) -> Self {
        Self::after(Duration::from_millis(limits.snapshot_deadline_ms))
    }

    /// Lease/snapshot release deadline (1 s).
    #[must_use]
    pub fn release() -> Self {
        Self::after(Duration::from_millis(RELEASE_DEADLINE_MS))
    }

    /// Starts a deadline of `timeout` from now.
    ///
    /// Overflow of `Instant` arithmetic is clamped to 24 hours instead of
    /// panicking; real deadlines are seconds.
    #[must_use]
    pub fn after(timeout: Duration) -> Self {
        let now = Instant::now();
        let at = now
            .checked_add(timeout)
            .or_else(|| now.checked_add(Duration::from_hours(24)))
            .unwrap_or(now);
        Self { at, timeout }
    }

    /// Returns the configured timeout used for error reporting.
    #[must_use]
    pub const fn timeout(self) -> Duration {
        self.timeout
    }

    /// Returns the timeout in whole milliseconds, saturating at `u64::MAX`.
    #[must_use]
    pub fn timeout_ms(self) -> u64 {
        u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX)
    }

    /// Returns whether the deadline has passed.
    #[must_use]
    pub fn is_expired(self) -> bool {
        Instant::now() >= self.at
    }
}

/// Result of an explicit shutdown.
///
/// The in-process engine has no child process, so the process-specific numbers
/// are always `false`/`0`; they are kept for the existing reports and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownReport {
    forced_kill: bool,
    threads_detached: bool,
    stderr_tail: String,
    stderr_discarded_bytes: u64,
}

impl ShutdownReport {
    const fn in_process(stderr_tail: String) -> Self {
        Self {
            forced_kill: false,
            threads_detached: false,
            stderr_tail,
            stderr_discarded_bytes: 0,
        }
    }

    /// Returns whether a forced kill was required (never for in-process).
    #[must_use]
    pub const fn forced_kill(&self) -> bool {
        self.forced_kill
    }

    /// Returns whether reader/writer threads were detached (never in-process).
    #[must_use]
    pub const fn threads_detached(&self) -> bool {
        self.threads_detached
    }

    /// Returns the diagnostic `stderr` tail (empty in-process).
    #[must_use]
    pub fn stderr_tail(&self) -> &str {
        &self.stderr_tail
    }

    /// Returns how many `stderr` bytes were dropped (always zero in-process).
    #[must_use]
    pub const fn stderr_discarded_bytes(&self) -> u64 {
        self.stderr_discarded_bytes
    }
}

/// Handle to the virtual filesystem currently installed in the transport.
///
/// The transport holds one clone and the session holds another, so installing a
/// new snapshot is visible to every later engine read without restarting the
/// session.
#[derive(Clone, Default)]
pub struct VfsHandle {
    inner: Arc<RwLock<Option<VirtualFileSystem>>>,
}

impl VfsHandle {
    /// Creates a handle with no snapshot installed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs `vfs` as the table visible to engine reads.
    pub fn install(&self, vfs: VirtualFileSystem) {
        *self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(vfs);
    }

    /// Removes the current table; reads then fail closed.
    pub fn clear(&self) {
        *self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn read<R>(&self, inspect: impl FnOnce(Option<&VirtualFileSystem>) -> R) -> R {
        let guard = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inspect(guard.as_ref())
    }
}

impl fmt::Debug for VfsHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let installed = self.read(|vfs| vfs.is_some());
        formatter
            .debug_struct("VfsHandle")
            .field("installed", &installed)
            .finish()
    }
}

impl FS for VfsHandle {
    fn use_case_sensitive_file_names(&self) -> bool {
        true
    }

    fn file_exists(&self, path: &str) -> bool {
        self.read(|vfs| vfs.and_then(|vfs| vfs.file_or_asset(path)).is_some())
    }

    fn read_file(&self, path: &str) -> Option<String> {
        self.read(|vfs| {
            vfs.and_then(|vfs| vfs.file_or_asset(path))
                .map(|file| file.text().to_owned())
        })
    }

    fn write_file(&self, _path: &str, _data: &str) -> Result<(), String> {
        Err(READ_ONLY.to_owned())
    }

    fn append_file(&self, _path: &str, _data: &str) -> Result<(), String> {
        Err(READ_ONLY.to_owned())
    }

    fn remove(&self, _path: &str) -> Result<(), String> {
        Err(READ_ONLY.to_owned())
    }

    fn chtimes(
        &self,
        _path: &str,
        _a_time: std::time::SystemTime,
        _m_time: std::time::SystemTime,
    ) -> Result<(), String> {
        Err(READ_ONLY.to_owned())
    }

    fn directory_exists(&self, path: &str) -> bool {
        self.read(|vfs| vfs.is_some_and(|vfs| vfs.is_directory(path)))
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        self.read(|vfs| {
            vfs.and_then(|vfs| vfs.directory_entries(path)).map_or_else(
                Entries::default,
                |entries| Entries {
                    files: entries.files.clone(),
                    directories: entries.directories.clone(),
                    symlinks: None,
                },
            )
        })
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        self.read(|vfs| {
            let vfs = vfs?;
            if let Some(file) = vfs.file_or_asset(path) {
                return Some(FileInfo {
                    name: base_name(path),
                    size: i64::try_from(file.size()).unwrap_or(i64::MAX),
                    mode: FileMode::from_bits_truncate(0o644),
                    mod_time: Some(UNIX_EPOCH),
                });
            }
            vfs.is_directory(path).then(|| FileInfo {
                name: base_name(path),
                size: 0,
                mode: FileMode::Dir | FileMode::from_bits_truncate(0o755),
                mod_time: Some(UNIX_EPOCH),
            })
        })
    }

    fn realpath(&self, path: &str) -> String {
        tsrs_core::tspath::normalize_path(path)
    }
}

/// Error text shared by every rejected write operation.
const READ_ONLY: &str = "read-only virtual filesystem";

fn base_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

/// The in-process transport around one [`tsrs_api::Session`].
pub struct InProcTransport {
    session: Option<Arc<Session>>,
    poisoned: AtomicBool,
}

impl fmt::Debug for InProcTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InProcTransport")
            .field("started", &self.session.is_some())
            .field("poisoned", &self.poisoned.load(Ordering::SeqCst))
            .finish()
    }
}

impl InProcTransport {
    /// Creates the in-process session over `fs`.
    ///
    /// # Errors
    ///
    /// Returns `Unavailable(EngineVersionMismatch)` when the compiled
    /// `tsrs_core` TypeScript version is not `7.1.0-beta`, and
    /// `Unavailable(EngineStartupPanic)` when session construction panics.
    pub fn start(cwd: &str, fs: VfsHandle) -> Result<Self, NativeError> {
        Self::start_with(cwd, fs, tsrs_core::version(), Session::new)
    }

    fn start_with(
        cwd: &str,
        fs: VfsHandle,
        version: &str,
        create: impl FnOnce(SessionOptions) -> Arc<Session>,
    ) -> Result<Self, NativeError> {
        validate_version(version)?;
        let options = SessionOptions::new(
            cwd.to_owned(),
            BUNDLED_STANDARD_LIBRARY_ROOT.to_owned(),
            Arc::new(tsrs_vfs::bundled::wrap_fs(fs)),
            false,
        );
        let created = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| create(options)))
            .map_err(|_| NativeError::Unavailable {
                reason: UnavailableReason::EngineStartupPanic,
            })?;
        Ok(Self {
            session: Some(created),
            poisoned: AtomicBool::new(false),
        })
    }

    /// Dispatches one request and returns its JSON result.
    ///
    /// # Errors
    ///
    /// Maps dead/poisoned sessions, an expired deadline, malformed JSON results,
    /// unexpected binary responses and every [`ApiError`] kind to a
    /// [`NativeError`].
    pub fn request(
        &self,
        method: &'static str,
        params: Value,
        deadline: Deadline,
    ) -> Result<Value, NativeError> {
        let session = self.session.as_ref().ok_or_else(poisoned)?;
        if self.poisoned.load(Ordering::SeqCst) {
            return Err(poisoned());
        }
        if deadline.is_expired() {
            return Err(NativeError::Timeout(deadline.timeout_ms()));
        }
        let body = serde_json::to_vec(&params).map_err(|_| NativeError::InvalidParams {
            reason: "request params are not serializable",
        })?;
        // The engine only needs the serialized bytes; release the JSON tree
        // before the synchronous call.
        drop(params);
        match session.handle_request(method, &body) {
            Ok(Response::Json(text)) => serde_json::from_str(&text)
                .map_err(|_| NativeError::Protocol("engine response is not valid JSON".to_owned())),
            Ok(Response::Binary(_)) => Err(NativeError::Protocol(
                "engine returned an unexpected binary response".to_owned(),
            )),
            Err(error) => {
                let mapped = map_engine_error(method, &error);
                if error.kind == ErrorKind::Internal {
                    // A panic or encoder failure may have left the checker in
                    // an unusable state; stop using this session instead of
                    // silently retrying.
                    self.poison();
                }
                Err(mapped)
            }
        }
    }

    /// Returns whether the session exists and has not been poisoned.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.session.is_some() && !self.poisoned.load(Ordering::SeqCst)
    }

    /// Marks the session unusable without closing it.
    pub fn poison(&self) {
        self.poisoned.store(true, Ordering::SeqCst);
    }

    /// Returns the diagnostic `stderr` tail (always empty in-process).
    #[must_use]
    pub const fn stderr_tail(&self) -> String {
        String::new()
    }

    /// Releases the session; repeated shutdown is a no-op.
    pub fn shutdown(&mut self) -> Option<ShutdownReport> {
        if let Some(session) = self.session.take() {
            session.close();
        }
        Some(ShutdownReport::in_process(self.stderr_tail()))
    }
}

const fn poisoned() -> NativeError {
    NativeError::Unavailable {
        reason: UnavailableReason::SessionPoisoned,
    }
}

fn validate_version(version: &str) -> Result<(), NativeError> {
    if version == "7.1.0-beta" {
        Ok(())
    } else {
        Err(NativeError::Unavailable {
            reason: UnavailableReason::EngineVersionMismatch,
        })
    }
}

/// Maps a typed engine error without leaking its message.
fn map_engine_error(method: &'static str, error: &ApiError) -> NativeError {
    map_engine_failure(method, &error.kind, &error.to_string())
}

fn map_engine_failure(method: &'static str, kind: &ErrorKind, message: &str) -> NativeError {
    match kind {
        ErrorKind::Unsupported => NativeError::Unsupported { method },
        ErrorKind::InvalidRequest => NativeError::InvalidParams {
            reason: "engine rejected the request",
        },
        ErrorKind::ClientError => {
            let lower = message.to_ascii_lowercase();
            if lower.contains("snapshot") && lower.contains("not found") {
                NativeError::StaleSnapshot
            } else if lower.contains("handle")
                && (lower.contains("stale") || lower.contains("could not be resolved"))
            {
                NativeError::StaleHandle
            } else {
                NativeError::EngineError { code: -32000 }
            }
        }
        // Internal errors may contain a panic stack or absolute paths; only the
        // category is retained.
        ErrorKind::Internal => NativeError::Protocol("engine internal error".to_owned()),
    }
}

/// Build-time guard for proposal task 1.4.
const _: () = assert!(
    option_env!("TSRS_TS_VERSION").is_none(),
    "TSRS_TS_VERSION must not be set: the pinned engine version is part of the contract"
);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicU64;
    use std::time::SystemTime;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn vfs(files: &[(&str, &str)]) -> VirtualFileSystem {
        let tag = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = format!("/tmp/opencode/inproc-{}-{tag}", std::process::id());
        let mut entries = Vec::new();
        for (name, text) in files {
            entries.push((format!("{root}/{name}"), (*text).to_owned()));
        }
        VirtualFileSystem::from_root_and_files(&root, entries).expect("virtual filesystem")
    }

    #[test]
    fn fs_reads_only_the_virtual_table_and_rejects_writes() {
        let fs = VfsHandle::new();
        let table = vfs(&[
            ("main.ts", "const x = 1;\n"),
            ("lib/nested.ts", "export {};"),
        ]);
        let root = table.root().to_owned();
        fs.install(table);

        assert_eq!(
            fs.read_file(&format!("{root}/main.ts")).as_deref(),
            Some("const x = 1;\n")
        );
        assert!(fs.file_exists(&format!("{root}/main.ts")));
        assert!(!fs.file_exists("/etc/passwd"));
        assert!(fs.read_file("/etc/passwd").is_none());
        // Traversal never falls back to a host file.
        assert!(fs.read_file("/tmp/../etc/passwd").is_none());

        for result in [
            fs.write_file(&format!("{root}/main.ts"), "x"),
            fs.append_file(&format!("{root}/main.ts"), "x"),
            fs.remove(&format!("{root}/main.ts")),
            fs.chtimes(
                &format!("{root}/main.ts"),
                SystemTime::now(),
                SystemTime::now(),
            ),
        ] {
            assert_eq!(result.err().as_deref(), Some(READ_ONLY));
        }
    }

    #[test]
    fn fs_enumerates_directories_and_stats_files() {
        let fs = VfsHandle::new();
        let table = vfs(&[
            ("main.ts", "const x = 1;\n"),
            ("lib/nested.ts", "export {};"),
        ]);
        let root = table.root().to_owned();
        fs.install(table);

        let root_entries = fs.get_accessible_entries(&root);
        assert!(root_entries.files.contains(&"main.ts".to_owned()));
        assert!(root_entries.directories.contains(&"lib".to_owned()));
        assert!(root_entries.symlinks.is_none());

        let nested = fs.get_accessible_entries(&format!("{root}/lib"));
        assert_eq!(nested.files, vec!["nested.ts".to_owned()]);
        assert!(fs.directory_exists(&format!("{root}/lib")));
        assert!(!fs.directory_exists(&format!("{root}/missing")));

        let file = fs.stat(&format!("{root}/main.ts")).expect("file stat");
        assert_eq!(file.name, "main.ts");
        assert_eq!(file.size, 13);
        assert_eq!(file.mode.bits(), 0o644);
        assert_eq!(file.mod_time, Some(UNIX_EPOCH));
        assert!(!file.mode.is_dir());

        let directory = fs.stat(&format!("{root}/lib")).expect("directory stat");
        assert!(directory.mode.is_dir());
        assert_eq!(directory.mode.bits() & 0o777, 0o755);
        assert_eq!(directory.mod_time, Some(UNIX_EPOCH));

        assert!(fs.stat("/etc/passwd").is_none());
    }

    #[test]
    fn fs_without_a_snapshot_is_fail_closed() {
        let fs = VfsHandle::new();
        assert!(fs.read_file("/work/main.ts").is_none());
        assert!(!fs.file_exists("/work/main.ts"));
        assert!(fs.read_file("/etc/passwd").is_none());
        assert!(!fs.file_exists("/etc/passwd"));
        assert!(!fs.directory_exists("/work"));
        assert!(fs.stat("/work/main.ts").is_none());
        let entries = fs.get_accessible_entries("/work");
        assert!(entries.files.is_empty() && entries.directories.is_empty());
        assert_eq!(
            fs.write_file("/work/main.ts", "").err().as_deref(),
            Some(READ_ONLY)
        );
        assert_eq!(fs.realpath("/work/./a/../main.ts"), "/work/main.ts");
    }

    #[test]
    fn bundled_standard_library_is_visible() {
        let fs = tsrs_vfs::bundled::wrap_fs(VfsHandle::new());
        assert!(fs.file_exists("bundled:///libs/lib.es5.d.ts"));
        assert!(
            fs.read_file("bundled:///libs/lib.es5.d.ts")
                .is_some_and(|text| text.contains("interface String"))
        );
    }

    #[test]
    fn installing_and_clearing_switches_the_visible_table() {
        let fs = VfsHandle::new();
        let first = vfs(&[("main.ts", "one")]);
        let first_root = first.root().to_owned();
        fs.install(first);
        assert!(fs.file_exists(&format!("{first_root}/main.ts")));

        let second = vfs(&[("other.ts", "two")]);
        let second_root = second.root().to_owned();
        fs.install(second);
        assert!(!fs.file_exists(&format!("{first_root}/main.ts")));
        assert!(fs.file_exists(&format!("{second_root}/other.ts")));

        fs.clear();
        assert!(!fs.file_exists(&format!("{second_root}/other.ts")));
    }

    #[test]
    fn pinned_version_is_required() {
        assert!(validate_version("7.1.0-beta").is_ok());
        assert!(matches!(
            validate_version("7.0.2"),
            Err(NativeError::Unavailable {
                reason: UnavailableReason::EngineVersionMismatch
            })
        ));
    }

    #[test]
    fn build_did_not_override_the_engine_version() {
        assert!(
            option_env!("TSRS_TS_VERSION").is_none(),
            "TSRS_TS_VERSION must not be set at build time"
        );
    }

    #[test]
    fn constructor_panics_become_unavailable() {
        let outcome = InProcTransport::start_with(
            "/tmp/opencode/inproc-panic",
            VfsHandle::new(),
            "7.1.0-beta",
            |_| panic!("synthetic constructor panic"),
        );
        assert!(matches!(
            outcome,
            Err(NativeError::Unavailable {
                reason: UnavailableReason::EngineStartupPanic
            })
        ));
    }

    #[test]
    fn engine_errors_map_without_leaking_messages() {
        assert!(matches!(
            map_engine_failure(
                "getTypeAtPosition",
                &ErrorKind::Unsupported,
                "api: unsupported: not implemented"
            ),
            NativeError::Unsupported {
                method: "getTypeAtPosition"
            }
        ));
        assert!(matches!(
            map_engine_failure("initialize", &ErrorKind::InvalidRequest, "bad params"),
            NativeError::InvalidParams { .. }
        ));
        assert!(matches!(
            map_engine_failure(
                "release",
                &ErrorKind::ClientError,
                "api: client error: snapshot 2 not found"
            ),
            NativeError::StaleSnapshot
        ));
        assert!(matches!(
            map_engine_failure(
                "getTypeAtPosition",
                &ErrorKind::ClientError,
                "api: client error: node handle is stale"
            ),
            NativeError::StaleHandle
        ));
        assert!(matches!(
            map_engine_failure(
                "getTypeAtPosition",
                &ErrorKind::ClientError,
                "api: client error: project missing"
            ),
            NativeError::EngineError { code: -32000 }
        ));
        let internal = map_engine_failure(
            "getTypeAtPosition",
            &ErrorKind::Internal,
            "panic: /secret/path/main.ts",
        );
        assert!(
            matches!(&internal, NativeError::Protocol(message) if !message.contains("/secret"))
        );
    }

    #[test]
    fn transport_serves_requests_and_closes_idempotently() {
        let mut transport =
            InProcTransport::start("/tmp/opencode/inproc-transport", VfsHandle::new())
                .expect("start in-process session");
        assert!(transport.is_alive());

        let pong = transport
            .request("ping", Value::Null, Deadline::handshake())
            .expect("ping");
        assert_eq!(pong, json!("pong"));

        assert!(matches!(
            transport.request("ping", Value::Null, Deadline::after(Duration::ZERO)),
            Err(NativeError::Timeout(_))
        ));

        let _ = transport.shutdown().expect("shutdown report");
        let _ = transport.shutdown().expect("idempotent shutdown");
        assert!(!transport.is_alive());
        assert!(matches!(
            transport.request("ping", Value::Null, Deadline::handshake()),
            Err(NativeError::Unavailable {
                reason: UnavailableReason::SessionPoisoned
            })
        ));
    }

    #[test]
    fn poisoned_session_degrades_instead_of_panicking() {
        let transport = InProcTransport::start("/tmp/opencode/inproc-poison", VfsHandle::new())
            .expect("start in-process session");
        assert!(transport.is_alive());
        transport.poison();
        assert!(!transport.is_alive());
        assert!(matches!(
            transport.request("ping", Value::Null, Deadline::handshake()),
            Err(NativeError::Unavailable {
                reason: UnavailableReason::SessionPoisoned
            })
        ));
    }
}
