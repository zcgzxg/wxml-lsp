//! Real native session: handshake, virtual snapshots, callbacks and queries.
//!
//! [`NativeSession::start`] verifies the pinned executable hash, runs
//! `lib/tsc --version` in the executable directory with a cleared environment,
//! spawns `--api --async` with the frozen argv and cwd, performs the
//! `initialize` handshake, runs the full schema/capability probe (virtual
//! snapshot, `getSourceFileNames`, `getSourceFile` v9/header-64 validation,
//! `getTypeAtPosition`/`typeToString`, `getSemanticDiagnostics` +
//! `getSyntacticDiagnostics`, `getCompletionsAtPosition`,
//! `getSymbolAtPosition`/`getDocumentationComment`/`getJsDocTags`,
//! `getReferencedSymbolsForNode`, and a successful `release`) and only then
//! marks the supervisor ready.
//!
//! A session owns one current snapshot and a bounded ledger of older engine
//! snapshots that have not been confirmed released. [`NativeSession::open_project`]
//! creates a full virtual filesystem plus a synthesized `tsconfig.json` and
//! opens the project and its files. [`NativeSession::update_project`] sends the
//! required `closeFiles` + `openFiles` (and closes/reopens the project) in one
//! `updateSnapshot`, then releases the previous snapshot within a bounded
//! deadline and enforces the contract's live-snapshot budget. Every response is
//! checked against the epoch captured when the request was sent; an old
//! snapshot id produces `StaleSnapshot`.
//!
//! While waiting for a response the loop answers engine filesystem callbacks
//! from the current [`VirtualFileSystem`]. Handles are bound to the session
//! generation, engine snapshot id, file path, content hash, encoder index and
//! node kind before they may be sent back; an update makes every old binding
//! stale.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;

use crate::config::{MAX_LIVE_SNAPSHOTS, PinnedEngine};
use crate::input::{AnalysisInput, ProjectKey, SnapshotRevision};
use crate::native::ast::{AstHeader, decode_base64, find_identifier};
use crate::native::framing::{DEFAULT_FRAME_LIMITS, FrameLimits, FrameRole};
use crate::native::handles::{
    BoundNodeHandle, BoundSymbol, BoundSymbolReference, EngineSnapshotId, HandleBinding,
    NodeHandle, SessionGeneration, SymbolBinding, SymbolReferenceBinding,
};
use crate::native::protocol::{
    InitializeResult, NativeMethod, NativeRequest, SnapshotResult, SourceFileNames,
};
use crate::native::rpc::{
    CallbackId, CallbackRejected, JsonRpcError, RawPayload, RpcDispatch, decode_inbound,
    encode_callback_error, encode_callback_rejection, encode_callback_result, encode_request,
};
use crate::native::strings::{
    Utf16Offset, Utf16Position, position_to_offset, utf16_len, utf16_to_utf8,
};
use crate::native::supervisor::{
    Deadline, EngineSupervisor, HANDSHAKE_DEADLINE_MS, InboundEvent, ProcessSession,
    SHUTDOWN_DEADLINE_MS, ShutdownReport, SpawnError, SupervisorError, verify_engine_executable,
};
use crate::native::vfs::{CallbackError, VfsError, VirtualFileSystem};
use crate::native::wire::{
    RawCompletionList, RawDiagnostic, RawJsDocTag, RawReferencedSymbol, RawSourceFile, RawSymbol,
    RawSymbolReference, RawType, SnapshotChanges, SnapshotFileSystem, completion_params,
    create_snapshot_params, file_params, files_params, node_params, parse_optional, parse_response,
    project_params, release_params, source_file_params, symbol_params, type_params,
    update_snapshot_params,
};
use crate::native::{NativeError, UnavailableReason};

/// Static session options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOptions {
    /// Verified pinned executable (`lib/tsc`).
    pub executable: PathBuf,
    /// Backend-owned empty sandbox used as `--cwd`.
    pub sandbox_root: PathBuf,
}

impl SessionOptions {
    /// Creates options for an explicit verified executable.
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>, sandbox_root: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            sandbox_root: sandbox_root.into(),
        }
    }
}

/// Current native snapshot identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeEpoch {
    /// Monotonic session generation.
    pub generation: SessionGeneration,
    /// Engine snapshot id.
    pub snapshot: EngineSnapshotId,
    /// Rust host snapshot revision.
    pub revision: SnapshotRevision,
}

/// Raw result of a combined type query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeQuery {
    /// Type object returned by `getTypeAtPosition`, or `None` for no type.
    pub info: Option<RawType>,
    /// Text returned by `typeToString`, when the call succeeded.
    pub text: Option<String>,
}

/// Raw documentation evidence for one symbol reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentationQuery {
    /// Documentation comment, when returned.
    pub comment: Option<String>,
    /// `JSDoc` tags, when returned.
    pub tags: Vec<RawJsDocTag>,
    /// Whether `getDocumentationComment` failed.
    pub comment_failed: bool,
    /// Whether `getJsDocTags` failed.
    pub tags_failed: bool,
}

/// Raw referenced-symbol evidence for one position or handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferencesQuery {
    /// Symbol at the queried position, when one exists.
    pub symbol: Option<BoundSymbol>,
    /// Referenced-symbol groups returned by the engine.
    pub groups: Vec<RawReferencedSymbol>,
    /// Identifier handle actually sent to the engine, when one was found.
    pub identifier: Option<BoundNodeHandle>,
}

/// Both diagnostic sources for one file.
///
/// F0 §9.2 requires a clean result to prove semantic **and** syntactic checks;
/// a mapper that only sees one list must not report `Supported`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsEvidence {
    /// Semantic diagnostics from `getSemanticDiagnostics`.
    pub semantic: Vec<RawDiagnostic>,
    /// Syntactic diagnostics from `getSyntacticDiagnostics`.
    pub syntactic: Vec<RawDiagnostic>,
}

/// Source used by the startup schema probe.
const PROBE_SOURCE: &str = "/** probe */\nconst x = 1;\n";
/// Zero-based line/utf16 column of the probe identifier `x`.
const PROBE_POSITION: Utf16Position = Utf16Position {
    line: 1,
    character: 6,
};

/// Bounded ledger of engine snapshots that have not been confirmed released.
///
/// The current snapshot is tracked here too; `mark_released` is called only
/// after the engine acknowledged `release`, so a failed release stays visible
/// and counts against the contract's live-snapshot budget.
#[derive(Debug, Default)]
struct SnapshotLedger {
    live: VecDeque<EngineSnapshotId>,
}

impl SnapshotLedger {
    fn track(&mut self, snapshot: EngineSnapshotId) {
        if !self.live.contains(&snapshot) {
            self.live.push_back(snapshot);
        }
    }

    fn mark_released(&mut self, snapshot: EngineSnapshotId) {
        self.live.retain(|live| *live != snapshot);
    }

    fn len(&self) -> usize {
        self.live.len()
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.live.clear();
    }

    fn ids(&self) -> impl Iterator<Item = EngineSnapshotId> + '_ {
        self.live.iter().copied()
    }

    /// Returns the oldest snapshot that is not the current one.
    fn oldest_excluding(&self, current: Option<EngineSnapshotId>) -> Option<EngineSnapshotId> {
        self.live
            .iter()
            .copied()
            .find(|snapshot| Some(*snapshot) != current)
    }
}

/// One live native session with a bounded set of engine snapshots.
#[derive(Debug)]
pub struct NativeSession {
    engine: PinnedEngine,
    supervisor: EngineSupervisor,
    dispatch: RpcDispatch,
    limits: FrameLimits,
    generation: SessionGeneration,
    epoch: Option<NativeEpoch>,
    project_id: Option<String>,
    project_key: Option<ProjectKey>,
    project_root: Option<String>,
    vfs: Option<VirtualFileSystem>,
    initialize: Option<InitializeResult>,
    options: SessionOptions,
    ledger: SnapshotLedger,
}

impl NativeSession {
    /// Starts and validates a real engine session.
    ///
    /// # Errors
    ///
    /// Returns `Unavailable(ArtifactMismatch)` for a hash/version mismatch,
    /// `Unavailable(HandshakeFailed)` for an incompatible initialize shape,
    /// `Unavailable(ApiMismatch)` when a probe method is absent, and
    /// `Protocol` for an AST schema mismatch or a failed process.
    pub fn start(options: SessionOptions) -> Result<Self, NativeError> {
        let engine = PinnedEngine::pinned();
        verify_engine_executable(&options.executable, engine.executable_sha256())
            .map_err(|error| map_spawn_error(&error))?;
        probe_version(&options.executable, &engine)?;
        confinement_self_check(&options)?;

        let mut supervisor = EngineSupervisor::new(engine);
        supervisor
            .spawn_with_executable(&options.executable, &options.sandbox_root)
            .map_err(map_supervisor_error)?;

        let mut session = Self {
            engine,
            supervisor,
            dispatch: RpcDispatch::new(DEFAULT_FRAME_LIMITS),
            limits: DEFAULT_FRAME_LIMITS,
            generation: SessionGeneration::next(),
            epoch: None,
            project_id: None,
            project_key: None,
            project_root: None,
            vfs: None,
            initialize: None,
            options,
            ledger: SnapshotLedger::default(),
        };

        if let Err(error) = session.handshake() {
            drop(session);
            return Err(error);
        }
        Ok(session)
    }

    /// Returns the pinned engine identity.
    #[must_use]
    pub const fn engine(&self) -> &PinnedEngine {
        &self.engine
    }

    /// Returns the session generation.
    #[must_use]
    pub const fn generation(&self) -> SessionGeneration {
        self.generation
    }

    /// Returns the current epoch.
    #[must_use]
    pub const fn epoch(&self) -> Option<NativeEpoch> {
        self.epoch
    }

    /// Returns the engine project id used for queries.
    #[must_use]
    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    /// Returns the accepted initialize result.
    #[must_use]
    pub const fn initialize_result(&self) -> Option<&InitializeResult> {
        self.initialize.as_ref()
    }

    /// Returns the sandbox root passed as `--cwd`.
    #[must_use]
    pub fn sandbox_root(&self) -> &std::path::Path {
        &self.options.sandbox_root
    }

    /// Returns the control-character-sanitized last `stderr` tail.
    ///
    /// Engine paths and stack text may remain; this is diagnostic evidence,
    /// not a user-visible error message.
    #[must_use]
    pub fn stderr_tail(&self) -> String {
        self.supervisor.stderr_tail()
    }

    fn handshake(&mut self) -> Result<(), NativeError> {
        let deadline = Deadline::handshake();
        let value = self.call(NativeMethod::Initialize, Value::Null, deadline, None)?;
        let initialize = InitializeResult::from_value(&value).map_err(|_| handshake_error())?;
        if !initialize.is_case_sensitive() {
            return Err(handshake_error());
        }
        self.initialize = Some(initialize);
        self.probe_schema(deadline)?;
        self.supervisor.mark_ready().map_err(map_supervisor_error)?;
        Ok(())
    }

    fn probe_schema(&mut self, deadline: Deadline) -> Result<(), NativeError> {
        let root = self
            .options
            .sandbox_root
            .to_str()
            .ok_or_else(handshake_error)?
            .to_owned();
        let probe_path = format!("{root}/probe.ts");
        let vfs = VirtualFileSystem::from_root_and_files(
            &root,
            [(probe_path.clone(), PROBE_SOURCE.to_owned())],
        )
        .map_err(|error| map_vfs(&error))?;
        self.vfs = Some(vfs);

        let inner = self.probe_schema_inner(&probe_path, deadline);
        let release = self.release_current_snapshot(deadline);
        self.vfs = None;
        self.project_key = None;
        self.project_root = None;

        match (inner, release) {
            (Err(error), _) | (Ok(()), Err(error)) => Err(map_probe_error(error)),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn probe_schema_inner(
        &mut self,
        probe_path: &str,
        deadline: Deadline,
    ) -> Result<(), NativeError> {
        let vfs = self.vfs.as_ref().ok_or_else(handshake_error)?;
        let files = vfs.snapshot_files();
        let tsconfig = vfs.tsconfig_path().to_owned();
        let changes = SnapshotChanges::new()
            .open_file(probe_path)
            .with_file_system(SnapshotFileSystem::full(files));
        let value = self.call(
            NativeMethod::CreateSnapshot,
            create_snapshot_params(&changes),
            deadline,
            None,
        )?;
        let snapshot = parse_snapshot(&value)?;
        let project = select_project(&snapshot, &tsconfig).ok_or_else(|| {
            NativeError::Protocol("probe snapshot returned no project".to_owned())
        })?;
        let engine_snapshot = EngineSnapshotId::new(snapshot.snapshot());
        self.ledger.track(engine_snapshot);
        self.epoch = Some(NativeEpoch {
            generation: self.generation,
            snapshot: engine_snapshot,
            revision: SnapshotRevision::new(0),
        });
        self.project_id = Some(project.clone());

        let names = self.call(
            NativeMethod::GetSourceFileNames,
            project_params(snapshot.snapshot(), &project),
            deadline,
            Some(engine_snapshot),
        )?;
        let names = SourceFileNames::from_value(&names).map_err(|_| {
            NativeError::Protocol("getSourceFileNames response shape mismatch".to_owned())
        })?;
        if !names.files().iter().any(|name| name == probe_path) {
            return Err(NativeError::Protocol(
                "probe snapshot is missing the probe file".to_owned(),
            ));
        }

        let source = self.call(
            NativeMethod::GetSourceFile,
            source_file_params(snapshot.snapshot(), &project, probe_path),
            deadline,
            Some(engine_snapshot),
        )?;
        let source: RawSourceFile = parse_response(source)?;
        let payload = decode_base64(&source.data)
            .map_err(|_| NativeError::Protocol("engine AST payload is not base64".to_owned()))?;
        AstHeader::parse(&payload)
            .map_err(|_| NativeError::Protocol("engine AST schema mismatch".to_owned()))?;

        let probe_offset = position_to_offset(PROBE_SOURCE, PROBE_POSITION)
            .map_err(|_| handshake_error())?
            .get();
        let Some((index, kind)) =
            find_identifier(&payload, probe_offset, "x", utf16_len(PROBE_SOURCE))
                .map_err(|_| NativeError::Protocol("engine AST schema mismatch".to_owned()))?
        else {
            return Err(NativeError::Protocol(
                "probe AST is missing the probe identifier".to_owned(),
            ));
        };
        let raw_handle = format!("{index}.{kind}.{probe_path}");
        let bound = self.bind_handle(&raw_handle)?;
        let _: Vec<RawReferencedSymbol> = parse_response(self.call(
            NativeMethod::GetReferencedSymbolsForNode,
            node_params(
                snapshot.snapshot(),
                &project,
                &bound.as_wire(),
                probe_offset,
            ),
            deadline,
            Some(engine_snapshot),
        )?)?;

        let value = self.call(
            NativeMethod::GetTypeAtPosition,
            file_params(
                snapshot.snapshot(),
                &project,
                probe_path,
                Some(probe_offset),
            ),
            deadline,
            Some(engine_snapshot),
        )?;
        let info: Option<RawType> = parse_optional(value)?;
        let info = info.ok_or_else(|| NativeError::Protocol("probe type is missing".to_owned()))?;
        let _: Option<String> = parse_optional(self.call(
            NativeMethod::TypeToString,
            type_params(snapshot.snapshot(), &project, info.id),
            deadline,
            Some(engine_snapshot),
        )?)?;

        let value = self.call(
            NativeMethod::GetSymbolAtPosition,
            file_params(
                snapshot.snapshot(),
                &project,
                probe_path,
                Some(probe_offset),
            ),
            deadline,
            Some(engine_snapshot),
        )?;
        let symbol: Option<RawSymbol> = parse_optional(value)?;
        let symbol =
            symbol.ok_or_else(|| NativeError::Protocol("probe symbol is missing".to_owned()))?;
        let bound_symbol = self.bind_symbol(&symbol, "probe.ts")?;
        let reference = bound_symbol
            .reference()
            .ok_or_else(|| NativeError::Protocol("probe symbol has no reference".to_owned()))?;
        let reference_value = serde_json::to_value(reference.reference()).map_err(|_| {
            NativeError::Protocol("probe symbol reference is not serializable".to_owned())
        })?;
        let _: Option<String> = parse_optional(self.call(
            NativeMethod::GetDocumentationComment,
            symbol_params(snapshot.snapshot(), &project, &reference_value),
            deadline,
            Some(engine_snapshot),
        )?)?;
        let _: Vec<RawJsDocTag> = parse_response(self.call(
            NativeMethod::GetJsDocTags,
            symbol_params(snapshot.snapshot(), &project, &reference_value),
            deadline,
            Some(engine_snapshot),
        )?)?;

        let _: Vec<RawDiagnostic> = parse_response(self.call(
            NativeMethod::GetSemanticDiagnostics,
            files_params(snapshot.snapshot(), &project, &[probe_path]),
            deadline,
            Some(engine_snapshot),
        )?)?;
        let _: Vec<RawDiagnostic> = parse_response(self.call(
            NativeMethod::GetSyntacticDiagnostics,
            files_params(snapshot.snapshot(), &project, &[probe_path]),
            deadline,
            Some(engine_snapshot),
        )?)?;
        let _: Option<RawCompletionList> = parse_optional(self.call(
            NativeMethod::GetCompletionsAtPosition,
            completion_params(
                snapshot.snapshot(),
                &project,
                probe_path,
                probe_offset,
                None,
            ),
            deadline,
            Some(engine_snapshot),
        )?)?;
        Ok(())
    }

    /// Creates the snapshot for `input`.
    ///
    /// # Errors
    ///
    /// Rejects an already-open snapshot, invalid paths, or a protocol/session
    /// failure. The engine snapshot id and Rust revision are stored together.
    pub fn open_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError> {
        if self.epoch.is_some() {
            return Err(NativeError::InvalidParams {
                reason: "a snapshot is already open",
            });
        }

        let vfs = VirtualFileSystem::from_analysis_input(input).map_err(|error| map_vfs(&error))?;
        self.check_snapshot_budget(&vfs)?;
        let changes = Self::create_changes(&vfs);
        let tsconfig = vfs.tsconfig_path().to_owned();
        // Install the VFS before createSnapshot: the engine may resolve
        // modules while applying the request, and callbacks must see the same
        // table rather than the no-snapshot fallback.
        self.vfs = Some(vfs);

        let value = match self.call(
            NativeMethod::CreateSnapshot,
            create_snapshot_params(&changes),
            Deadline::snapshot(self.limits),
            None,
        ) {
            Ok(value) => value,
            Err(error) => {
                self.vfs = None;
                return Err(error);
            }
        };
        let snapshot = match parse_snapshot(&value) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.vfs = None;
                self.poison_session();
                return Err(error);
            }
        };
        let snapshot_id = EngineSnapshotId::new(snapshot.snapshot());
        self.ledger.track(snapshot_id);
        let Some(project) = select_project(&snapshot, &tsconfig) else {
            let _ = self.release_snapshot(snapshot_id, Deadline::release());
            self.vfs = None;
            self.poison_session();
            return Err(NativeError::Protocol(
                "createSnapshot returned no project".to_owned(),
            ));
        };

        self.epoch = Some(NativeEpoch {
            generation: self.generation,
            snapshot: snapshot_id,
            revision: input.revision(),
        });
        self.project_id = Some(project);
        self.project_key = Some(input.project().clone());
        self.project_root = Some(input.root().as_str().to_owned());
        self.dispatch.clear_abandoned();
        if let Err(error) = self.enforce_live_cap() {
            self.epoch = None;
            self.project_id = None;
            self.project_key = None;
            self.project_root = None;
            self.vfs = None;
            return Err(error);
        }
        Ok(())
    }

    /// Updates the open snapshot to `input`.
    ///
    /// The request closes and reopens every previously open file and the
    /// project configuration in the same `updateSnapshot`, and supplies the new
    /// full filesystem. After the new epoch is committed, the previous engine
    /// snapshot is released with a bounded deadline; a failed release remains
    /// tracked and counts against the live-snapshot budget.
    ///
    /// # Errors
    ///
    /// Rejects a missing snapshot, a different project/root, a non-advancing
    /// revision, a request-budget overflow, or a protocol/session failure.
    pub fn update_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError> {
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let key = self.project_key.as_ref().ok_or(NativeError::NoSnapshot)?;
        let root = self
            .project_root
            .as_deref()
            .ok_or(NativeError::NoSnapshot)?;
        if key != input.project() {
            return Err(NativeError::InvalidParams {
                reason: "update belongs to a different project",
            });
        }
        if root != input.root().as_str() {
            return Err(NativeError::InvalidParams {
                reason: "update root differs from the open project",
            });
        }
        if input.revision() <= epoch.revision {
            return Err(NativeError::InvalidParams {
                reason: "snapshot revision must advance",
            });
        }

        let vfs = VirtualFileSystem::from_analysis_input(input).map_err(|error| map_vfs(&error))?;
        self.check_snapshot_budget(&vfs)?;
        let old = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let old_sources: Vec<String> = old.source_paths().map(str::to_owned).collect();
        let new_sources: Vec<String> = vfs.source_paths().map(str::to_owned).collect();
        let tsconfig = vfs.tsconfig_path().to_owned();
        let mut changes = SnapshotChanges::new()
            .close_project(old.tsconfig_path())
            .open_project(vfs.tsconfig_path())
            .with_file_system(SnapshotFileSystem::full(vfs.snapshot_files()));
        for path in old_sources {
            changes = changes.close_file(path);
        }
        for path in new_sources {
            changes = changes.open_file(path);
        }

        // The engine applies the new full filesystem during this request;
        // answer callbacks from the new table and restore the old one on
        // failure so a rejected update never leaves a mixed VFS/epoch state.
        let old_vfs = self.vfs.replace(vfs).ok_or(NativeError::NoSnapshot)?;
        let value = match self.call(
            NativeMethod::UpdateSnapshot,
            update_snapshot_params(epoch.snapshot.get(), &changes),
            Deadline::snapshot(self.limits),
            None,
        ) {
            Ok(value) => value,
            Err(error) => {
                self.vfs = Some(old_vfs);
                return Err(error);
            }
        };
        let snapshot = match parse_snapshot(&value) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.vfs = Some(old_vfs);
                self.poison_session();
                return Err(error);
            }
        };
        let snapshot_id = EngineSnapshotId::new(snapshot.snapshot());
        if snapshot_id == epoch.snapshot {
            self.vfs = Some(old_vfs);
            self.poison_session();
            return Err(NativeError::Protocol(
                "updateSnapshot reused the current snapshot id".to_owned(),
            ));
        }
        self.ledger.track(snapshot_id);
        let Some(project) = select_project(&snapshot, &tsconfig) else {
            self.vfs = Some(old_vfs);
            let _ = self.enforce_live_cap();
            self.poison_session();
            return Err(NativeError::Protocol(
                "updateSnapshot returned no project".to_owned(),
            ));
        };

        self.epoch = Some(NativeEpoch {
            generation: self.generation,
            snapshot: snapshot_id,
            revision: input.revision(),
        });
        self.project_id = Some(project);
        self.dispatch.clear_abandoned();

        let release = self.release_snapshot(epoch.snapshot, Deadline::release());
        let cap = self.enforce_live_cap();
        release?;
        cap?;
        Ok(())
    }

    /// Releases the current snapshot and every tracked older snapshot.
    ///
    /// A snapshot the engine already forgot is treated as released for
    /// idempotent cleanup; any other release failure is returned after the
    /// local snapshot state is cleared so the session can still be shut down.
    ///
    /// # Errors
    ///
    /// Propagates the first native release failure, if any.
    pub fn release(&mut self) -> Result<(), NativeError> {
        let ids: Vec<EngineSnapshotId> = self.ledger.ids().collect();
        let mut first_error = None;
        for snapshot in ids {
            if let Err(error) = self.release_snapshot(snapshot, Deadline::release()) {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        self.epoch = None;
        self.project_id = None;
        self.project_key = None;
        self.project_root = None;
        self.vfs = None;
        self.dispatch.clear_abandoned();
        first_error.map_or(Ok(()), Err)
    }

    /// Returns how many engine snapshots are still believed to be live.
    ///
    /// A successful update releases the previous snapshot immediately, so this
    /// is normally `1` while a project is open and `0` after [`Self::release`].
    #[must_use]
    pub fn live_snapshot_count(&self) -> usize {
        self.ledger.len()
    }

    fn check_snapshot_budget(&self, vfs: &VirtualFileSystem) -> Result<(), NativeError> {
        check_request_budget(vfs, self.limits.request_bytes)
    }

    fn release_snapshot(
        &mut self,
        snapshot: EngineSnapshotId,
        deadline: Deadline,
    ) -> Result<(), NativeError> {
        let result = self.call(
            NativeMethod::Release,
            release_params(snapshot.get()),
            deadline,
            None,
        );
        match result {
            Ok(_) => {
                self.ledger.mark_released(snapshot);
                Ok(())
            }
            Err(NativeError::StaleSnapshot) => {
                // The engine already forgot this snapshot; cleanup stays
                // idempotent, while queries still surface StaleSnapshot.
                self.ledger.mark_released(snapshot);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn release_current_snapshot(&mut self, deadline: Deadline) -> Result<(), NativeError> {
        let Some(epoch) = self.epoch else {
            return Ok(());
        };
        let result = self.release_snapshot(epoch.snapshot, deadline);
        self.epoch = None;
        self.project_id = None;
        result
    }

    fn enforce_live_cap(&mut self) -> Result<(), NativeError> {
        while self.ledger.len() > MAX_LIVE_SNAPSHOTS {
            let Some(oldest) = self
                .ledger
                .oldest_excluding(self.epoch.map(|epoch| epoch.snapshot))
            else {
                break;
            };
            self.release_snapshot(oldest, Deadline::release())?;
        }
        Ok(())
    }

    /// Releases the snapshot and shuts the process down.
    #[must_use]
    pub fn shutdown(mut self) -> Option<ShutdownReport> {
        let _ = self.release();
        self.supervisor
            .shutdown_with_timeout(Duration::from_millis(SHUTDOWN_DEADLINE_MS))
    }

    /// Returns semantic diagnostics for one project-relative file.
    ///
    /// # Errors
    ///
    /// Requires an open snapshot and a file inside it.
    pub fn semantic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        let (epoch, project, absolute) = self.file_context(path)?;
        let value = self.call(
            NativeMethod::GetSemanticDiagnostics,
            files_params(epoch.snapshot.get(), &project, &[absolute.as_str()]),
            Deadline::request(self.limits),
            Some(epoch.snapshot),
        )?;
        parse_response(value)
    }

    /// Returns syntactic diagnostics for one project-relative file.
    ///
    /// # Errors
    ///
    /// Requires an open snapshot and a file inside it.
    pub fn syntactic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        let (epoch, project, absolute) = self.file_context(path)?;
        let value = self.call(
            NativeMethod::GetSyntacticDiagnostics,
            files_params(epoch.snapshot.get(), &project, &[absolute.as_str()]),
            Deadline::request(self.limits),
            Some(epoch.snapshot),
        )?;
        parse_response(value)
    }

    /// Returns both diagnostic sources required for a clean-file claim.
    ///
    /// Both calls must succeed; a hard failure is returned instead of being
    /// collapsed into a partial or empty result.
    ///
    /// # Errors
    ///
    /// Requires an open snapshot and a file inside it, and propagates either
    /// native call failure.
    pub fn diagnostics(&mut self, path: &str) -> Result<DiagnosticsEvidence, NativeError> {
        let semantic = self.semantic_diagnostics(path)?;
        let syntactic = self.syntactic_diagnostics(path)?;
        Ok(DiagnosticsEvidence {
            semantic,
            syntactic,
        })
    }

    /// Returns the type object and text at a UTF-16 position.
    ///
    /// # Errors
    ///
    /// Requires an open snapshot and a valid position. A hard engine or
    /// transport failure is propagated; only a genuinely absent
    /// `typeToString` capability is returned as `Unsupported`.
    pub fn type_query(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<TypeQuery, NativeError> {
        let (epoch, project, absolute) = self.file_context(path)?;
        let offset = self.position_offset(&absolute, position)?;
        let value = self.call(
            NativeMethod::GetTypeAtPosition,
            file_params(epoch.snapshot.get(), &project, &absolute, Some(offset)),
            Deadline::request(self.limits),
            Some(epoch.snapshot),
        )?;
        let info: Option<RawType> = parse_optional(value)?;
        let Some(info) = info else {
            return Ok(TypeQuery {
                info: None,
                text: None,
            });
        };

        let text = match self.call(
            NativeMethod::TypeToString,
            type_params(epoch.snapshot.get(), &project, info.id),
            Deadline::request(self.limits),
            Some(epoch.snapshot),
        ) {
            Ok(value) => parse_optional::<String>(value)?,
            Err(NativeError::Unsupported { .. }) => return Err(unsupported("typeToString")),
            Err(error) => return Err(error),
        };
        Ok(TypeQuery {
            info: Some(info),
            text,
        })
    }

    /// Returns the epoch-bound symbol at a UTF-16 position.
    ///
    /// # Errors
    ///
    /// Requires an open snapshot and a valid position.
    pub fn symbol_at_position(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<Option<BoundSymbol>, NativeError> {
        let (epoch, project, absolute) = self.file_context(path)?;
        let offset = self.position_offset(&absolute, position)?;
        let value = self.call(
            NativeMethod::GetSymbolAtPosition,
            file_params(epoch.snapshot.get(), &project, &absolute, Some(offset)),
            Deadline::request(self.limits),
            Some(epoch.snapshot),
        )?;
        let symbol: Option<RawSymbol> = parse_optional(value)?;
        symbol
            .map(|symbol| self.bind_symbol(&symbol, path))
            .transpose()
    }

    /// Binds a raw symbol and its handles to the current epoch.
    ///
    /// Declarations outside the current snapshot are omitted and recorded as a
    /// degradation; malformed handles and hard session failures propagate.
    ///
    /// # Errors
    ///
    /// Requires an open snapshot and rejects a malformed symbol reference.
    pub fn bind_symbol(&self, symbol: &RawSymbol, path: &str) -> Result<BoundSymbol, NativeError> {
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let absolute = vfs.absolute_path(path).map_err(|error| map_vfs(&error))?;
        let file = vfs.file(&absolute).ok_or(NativeError::InvalidParams {
            reason: "symbol file is not in the open snapshot",
        })?;
        let binding = SymbolBinding::new(
            self.generation,
            epoch.snapshot,
            absolute,
            file.hash().to_owned(),
        );

        let mut declarations = Vec::new();
        let mut dropped_declarations = 0usize;
        for raw in &symbol.declarations {
            match self.bind_handle(raw) {
                Ok(handle) => declarations.push(handle),
                Err(NativeError::InvalidHandle { .. }) => {
                    dropped_declarations = dropped_declarations.saturating_add(1);
                }
                Err(error) => return Err(error),
            }
        }
        let value_declaration = match symbol.value_declaration.as_deref() {
            Some(raw) => match self.bind_handle(raw) {
                Ok(handle) => Some(handle),
                Err(NativeError::InvalidHandle { .. }) => {
                    dropped_declarations = dropped_declarations.saturating_add(1);
                    None
                }
                Err(error) => return Err(error),
            },
            None => None,
        };
        let (reference, reference_dropped) = match symbol.reference.as_ref() {
            Some(raw) => match self.bind_symbol_reference(raw) {
                Ok(reference) => (Some(reference), false),
                Err(NativeError::InvalidHandle { .. }) => (None, true),
                Err(error) => return Err(error),
            },
            None => (None, false),
        };

        Ok(BoundSymbol::new(
            symbol.name.clone(),
            symbol.flags,
            declarations,
            value_declaration,
            reference,
            dropped_declarations,
            reference_dropped,
            binding,
        ))
    }

    /// Binds a content-addressed symbol reference to the current epoch.
    ///
    /// # Errors
    ///
    /// Rejects a reference whose path is not a normalized file inside the
    /// current snapshot, or whose descriptor fields are incomplete.
    pub fn bind_symbol_reference(
        &self,
        reference: &RawSymbolReference,
    ) -> Result<BoundSymbolReference, NativeError> {
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        crate::native::handles::validate_absolute_path(&reference.file.path).map_err(|_| {
            NativeError::InvalidHandle {
                reason: "symbol reference path is not normalized",
            }
        })?;
        if reference.file.file_name != reference.file.path {
            return Err(NativeError::InvalidHandle {
                reason: "symbol reference file names disagree",
            });
        }
        if reference.file.content_hash.is_empty()
            || reference.file.parse_options_key.is_empty()
            || reference.file.node_id.is_empty()
        {
            return Err(NativeError::InvalidHandle {
                reason: "symbol reference descriptor is incomplete",
            });
        }
        if !vfs.contains(&reference.file.path) {
            return Err(NativeError::InvalidHandle {
                reason: "symbol reference is outside the open snapshot",
            });
        }
        let file = vfs
            .file(&reference.file.path)
            .ok_or(NativeError::InvalidHandle {
                reason: "symbol reference is not in the open snapshot",
            })?;
        let binding = SymbolReferenceBinding::new(
            self.generation,
            epoch.snapshot,
            reference.file.path.clone(),
            file.hash().to_owned(),
            reference.file.content_hash.clone(),
            reference.file.parse_options_key.clone(),
            reference.file.node_id.clone(),
            reference.id,
            reference.kind,
        );
        Ok(BoundSymbolReference::new(reference.clone(), binding))
    }

    /// Rejects a symbol reference that no longer matches the current epoch,
    /// file content or descriptor identity.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::StaleHandle`] for any identity mismatch.
    pub fn validate_symbol_reference(
        &self,
        reference: &BoundSymbolReference,
    ) -> Result<(), NativeError> {
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let Some(file) = vfs.file(reference.binding().normalized_file_path()) else {
            return Err(NativeError::StaleHandle);
        };
        if reference.is_current(self.generation, epoch.snapshot, file.hash()) {
            Ok(())
        } else {
            Err(NativeError::StaleHandle)
        }
    }

    /// Returns documentation and `JSDoc` tags for a bound symbol reference.
    ///
    /// A genuinely absent method is recorded as an unsupported evidence field;
    /// a timeout, poisoned session or shape failure is propagated as a hard
    /// error instead of becoming a partial result.
    ///
    /// # Errors
    ///
    /// Requires an open snapshot and a current reference.
    pub fn documentation(
        &mut self,
        reference: &BoundSymbolReference,
    ) -> Result<DocumentationQuery, NativeError> {
        self.validate_symbol_reference(reference)?;
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let project = self.project_id.clone().ok_or(NativeError::NoSnapshot)?;
        let reference = serde_json::to_value(reference.reference()).map_err(|_| {
            NativeError::InvalidParams {
                reason: "symbol reference cannot be encoded",
            }
        })?;

        let comment = self.call(
            NativeMethod::GetDocumentationComment,
            symbol_params(epoch.snapshot.get(), &project, &reference),
            Deadline::request(self.limits),
            Some(epoch.snapshot),
        );
        let tags = self.call(
            NativeMethod::GetJsDocTags,
            symbol_params(epoch.snapshot.get(), &project, &reference),
            Deadline::request(self.limits),
            Some(epoch.snapshot),
        );

        let (comment, comment_failed) = match comment {
            Ok(value) => (parse_optional::<String>(value)?, false),
            Err(NativeError::Unsupported { .. }) => (None, true),
            Err(error) => return Err(error),
        };
        let (tags, tags_failed) = match tags {
            Ok(value) => (parse_response::<Vec<RawJsDocTag>>(value)?, false),
            Err(NativeError::Unsupported { .. }) => (Vec::new(), true),
            Err(error) => return Err(error),
        };
        Ok(DocumentationQuery {
            comment,
            tags,
            comment_failed,
            tags_failed,
        })
    }

    /// Returns referenced symbols for the symbol at a position.
    ///
    /// The node handle is derived from the current snapshot's AST by scanning
    /// the flat Identifier table. If no identifier is found this returns an
    /// explicit empty result; a declaration handle is never substituted,
    /// because `getReferencedSymbolsForNode` requires an Identifier node.
    ///
    /// # Errors
    ///
    /// Requires a symbol and a valid AST/position.
    pub fn references(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<ReferencesQuery, NativeError> {
        let (_, _, absolute) = self.file_context(path)?;
        let offset = self.position_offset(&absolute, position)?;
        let symbol = self.symbol_at_position(path, position)?;
        let Some(symbol) = symbol else {
            return Ok(ReferencesQuery {
                symbol: None,
                groups: Vec::new(),
                identifier: None,
            });
        };

        let identifier = self.identifier_handle(&absolute, offset, symbol.name())?;
        let Some(bound) = identifier else {
            return Ok(ReferencesQuery {
                symbol: Some(symbol),
                groups: Vec::new(),
                identifier: None,
            });
        };
        let groups = self.references_for_handle(&bound, position)?;
        Ok(ReferencesQuery {
            symbol: Some(symbol),
            groups,
            identifier: Some(bound),
        })
    }

    /// Returns the Identifier node handle at `offset` when its pool text is
    /// `name`.
    fn identifier_handle(
        &mut self,
        absolute: &str,
        offset: u32,
        name: &str,
    ) -> Result<Option<BoundNodeHandle>, NativeError> {
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let project = self.project_id.clone().ok_or(NativeError::NoSnapshot)?;
        let source_len = {
            let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
            let file = vfs.file(absolute).ok_or(NativeError::InvalidParams {
                reason: "file is not in the open snapshot",
            })?;
            utf16_len(file.text())
        };
        let value = self.call(
            NativeMethod::GetSourceFile,
            source_file_params(epoch.snapshot.get(), &project, absolute),
            Deadline::request(self.limits),
            Some(epoch.snapshot),
        )?;
        let source: RawSourceFile = parse_response(value)?;
        let payload = decode_base64(&source.data)
            .map_err(|_| NativeError::Protocol("engine AST payload is not base64".to_owned()))?;
        let found = find_identifier(&payload, offset, name, source_len)
            .map_err(|_| NativeError::Protocol("engine AST schema mismatch".to_owned()))?;
        let Some((index, kind)) = found else {
            return Ok(None);
        };
        let raw = format!("{index}.{kind}.{absolute}");
        self.bind_handle(&raw).map(Some)
    }

    /// Returns referenced symbols for an already bound node handle.
    ///
    /// # Errors
    ///
    /// Rejects a stale or malformed handle before sending anything.
    pub fn references_for_handle(
        &mut self,
        bound: &BoundNodeHandle,
        position: Utf16Position,
    ) -> Result<Vec<RawReferencedSymbol>, NativeError> {
        self.validate_handle(bound)?;
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let project = self.project_id.clone().ok_or(NativeError::NoSnapshot)?;
        let absolute = bound.binding().normalized_file_path.clone();
        let offset = self.position_offset(&absolute, position)?;
        let value = self.call(
            NativeMethod::GetReferencedSymbolsForNode,
            node_params(epoch.snapshot.get(), &project, &bound.as_wire(), offset),
            Deadline::request(self.limits),
            Some(epoch.snapshot),
        )?;
        parse_response(value)
    }

    /// Returns completions at a UTF-16 position with `includeSymbol: false`.
    ///
    /// # Errors
    ///
    /// Requires an open snapshot and a valid position.
    pub fn completions(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<Option<RawCompletionList>, NativeError> {
        let (epoch, project, absolute) = self.file_context(path)?;
        let offset = self.position_offset(&absolute, position)?;
        let trigger = self.trigger_character(&absolute, offset)?;
        let value = self.call(
            NativeMethod::GetCompletionsAtPosition,
            completion_params(
                epoch.snapshot.get(),
                &project,
                &absolute,
                offset,
                trigger.as_deref(),
            ),
            Deadline::request(self.limits),
            Some(epoch.snapshot),
        )?;
        parse_optional(value)
    }

    /// Binds a raw handle to the current epoch and file content.
    ///
    /// # Errors
    ///
    /// Rejects malformed handles, handles outside the snapshot, and a missing
    /// snapshot.
    pub fn bind_handle(&self, raw: &str) -> Result<BoundNodeHandle, NativeError> {
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let handle = NodeHandle::parse(raw).map_err(|_| NativeError::InvalidHandle {
            reason: "malformed node handle",
        })?;
        let Some(file) = vfs.file(&handle.path) else {
            return Err(NativeError::InvalidHandle {
                reason: "handle file is not in the snapshot",
            });
        };
        let normalized_file_path = handle.path.clone();
        let encoded_index = handle.encoder_index;
        let node_kind = handle.kind;
        let content_hash = file.hash().to_owned();
        Ok(BoundNodeHandle::new(
            handle,
            HandleBinding {
                session_generation: self.generation,
                engine_snapshot_id: epoch.snapshot,
                normalized_file_path,
                content_hash_or_node_id: content_hash,
                node_index: encoded_index,
                node_kind,
            },
        ))
    }

    /// Rejects a handle that no longer matches the current session, snapshot
    /// or file content.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::StaleHandle`] for any identity mismatch.
    pub fn validate_handle(&self, bound: &BoundNodeHandle) -> Result<(), NativeError> {
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let Some(file) = vfs.file(&bound.binding().normalized_file_path) else {
            return Err(NativeError::StaleHandle);
        };
        if bound.is_current(self.generation, epoch.snapshot, file.hash()) {
            Ok(())
        } else {
            Err(NativeError::StaleHandle)
        }
    }

    fn create_changes(vfs: &VirtualFileSystem) -> SnapshotChanges {
        let mut changes = SnapshotChanges::new()
            .open_project(vfs.tsconfig_path())
            .with_file_system(SnapshotFileSystem::full(vfs.snapshot_files()));
        for path in vfs.source_paths() {
            changes = changes.open_file(path);
        }
        changes
    }

    fn file_context(&self, path: &str) -> Result<(NativeEpoch, String, String), NativeError> {
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let project = self.project_id.clone().ok_or(NativeError::NoSnapshot)?;
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let absolute = vfs.absolute_path(path).map_err(|error| map_vfs(&error))?;
        if vfs.file(&absolute).is_none() {
            return Err(NativeError::InvalidParams {
                reason: "file is not in the open snapshot",
            });
        }
        Ok((epoch, project, absolute))
    }

    fn position_offset(&self, absolute: &str, position: Utf16Position) -> Result<u32, NativeError> {
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let file = vfs.file(absolute).ok_or(NativeError::InvalidParams {
            reason: "file is not in the open snapshot",
        })?;
        position_to_offset(file.text(), position)
            .map(Utf16Offset::get)
            .map_err(|_| NativeError::InvalidParams {
                reason: "position is outside the file or splits a surrogate pair",
            })
    }

    fn trigger_character(
        &self,
        absolute: &str,
        offset: u32,
    ) -> Result<Option<String>, NativeError> {
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let Some(file) = vfs.file(absolute) else {
            return Ok(None);
        };
        let byte = utf16_to_utf8(file.text(), Utf16Offset::new(offset)).map_err(|_| {
            NativeError::InvalidParams {
                reason: "position is outside the file",
            }
        })?;
        let prefix = file
            .text()
            .get(..usize::try_from(byte.get()).unwrap_or(usize::MAX));
        Ok(prefix
            .and_then(|prefix| prefix.chars().next_back())
            .filter(|character| *character == '.')
            .map(|character| character.to_string()))
    }

    fn call(
        &mut self,
        method: NativeMethod,
        params: Value,
        deadline: Deadline,
        expected_snapshot: Option<EngineSnapshotId>,
    ) -> Result<Value, NativeError> {
        let admission = self.dispatch.allocate_request()?;
        if admission.is_queued() {
            return Err(NativeError::Protocol(
                "native request queue is unexpectedly occupied".to_owned(),
            ));
        }
        let id = admission.id();
        let request = NativeRequest::new(method, RawPayload::new(params));
        let body = encode_request(id, &request)?;
        if let Err(error) = self
            .process()?
            .send_until(FrameRole::Request, &body, deadline)
        {
            let _ = self.dispatch.abandon_request(id);
            return Err(error);
        }

        loop {
            let event = match self.process()?.recv_until(deadline) {
                Ok(event) => event,
                Err(error) => {
                    let _ = self.dispatch.abandon_request(id);
                    return Err(error);
                }
            };
            let frame = match event {
                InboundEvent::Frame(frame) => frame,
                InboundEvent::Fault(fault) => {
                    let _ = self.dispatch.abandon_request(id);
                    return Err(NativeError::Protocol(fault.message().to_owned()));
                }
                InboundEvent::Closed => {
                    let _ = self.dispatch.abandon_request(id);
                    return Err(NativeError::Unavailable {
                        reason: UnavailableReason::ProcessExited,
                    });
                }
            };

            let decoded = decode_inbound(frame.body(), self.limits)?;
            match self.dispatch.route(decoded)? {
                crate::native::rpc::RoutedMessage::Response {
                    id: response_id,
                    result,
                } if response_id == id => {
                    self.ensure_snapshot(expected_snapshot)?;
                    return Ok(result);
                }
                crate::native::rpc::RoutedMessage::ErrorResponse {
                    id: response_id,
                    error,
                } if response_id == id => {
                    return Err(map_engine_error(method, &error));
                }
                crate::native::rpc::RoutedMessage::Callback {
                    id: callback_id,
                    method: callback_method,
                    params,
                } => {
                    self.serve_callback(&callback_id, &callback_method, &params)?;
                }
                crate::native::rpc::RoutedMessage::RejectedCallback(rejected) => {
                    self.send_rejected_callback(&rejected)?;
                }
                crate::native::rpc::RoutedMessage::LateResponse { .. } => {}
                crate::native::rpc::RoutedMessage::Response { .. }
                | crate::native::rpc::RoutedMessage::ErrorResponse { .. } => {
                    let _ = self.dispatch.abandon_request(id);
                    return Err(NativeError::Protocol("unexpected response id".to_owned()));
                }
            }
        }
    }

    fn ensure_snapshot(&self, expected: Option<EngineSnapshotId>) -> Result<(), NativeError> {
        if snapshot_is_current(self.epoch.map(|epoch| epoch.snapshot), expected) {
            Ok(())
        } else {
            Err(NativeError::StaleSnapshot)
        }
    }

    fn serve_callback(
        &mut self,
        id: &CallbackId,
        method: &str,
        params: &Value,
    ) -> Result<(), NativeError> {
        // Read callbacks must never be answered with a JSON-RPC error: F0
        // §5.2 records that the engine panics on that path. Before a VFS is
        // installed (initialize and the createSnapshot window) use the
        // per-callback fail-closed shape, and never use an error as a deny.
        let answer = self.vfs.as_ref().map_or_else(
            || {
                VirtualFileSystem::fail_closed_answer(method).ok_or_else(|| {
                    CallbackError::UnknownCallback {
                        method: method.to_owned(),
                    }
                })
            },
            |vfs| {
                vfs.answer(method, params)
                    .or_else(|error| VirtualFileSystem::fail_closed_answer(method).ok_or(error))
            },
        );
        let body = match answer {
            Ok(value) => encode_callback_result(id, &value)?,
            Err(error) => encode_callback_error(id, error.code(), error.reason())?,
        };
        self.process()?
            .send_until(FrameRole::CallbackResponse, &body, Deadline::callback())?;
        self.dispatch.finish_callback(id)?;
        Ok(())
    }

    fn send_rejected_callback(&mut self, rejected: &CallbackRejected) -> Result<(), NativeError> {
        let body = encode_callback_rejection(rejected)?;
        self.process()?
            .send_until(FrameRole::CallbackResponse, &body, Deadline::callback())?;
        Ok(())
    }

    fn process(&mut self) -> Result<&mut ProcessSession, NativeError> {
        self.supervisor.session().ok_or(NativeError::Unavailable {
            reason: UnavailableReason::ProcessExited,
        })
    }

    /// Marks the transport unusable after a protocol/shape failure.
    ///
    /// The process is not killed here; the caller must shut the session down.
    /// This is the F0 §6.6 recovery trigger: a malformed snapshot response is a
    /// protocol error, not a degraded-but-usable state.
    const fn poison_session(&mut self) {
        if let Some(session) = self.supervisor.session() {
            session.poison();
        }
    }
}

fn snapshot_is_current(
    current: Option<EngineSnapshotId>,
    expected: Option<EngineSnapshotId>,
) -> bool {
    expected.is_none() || current == expected
}

/// Rejects an input whose serialized snapshot request cannot fit the budget.
///
/// The estimate is a conservative upper bound (JSON escaping, paths, URI
/// lists), so a request that passes here always fits; rejecting before the
/// body is built avoids a large transient allocation.
fn check_request_budget(vfs: &VirtualFileSystem, limit: usize) -> Result<(), NativeError> {
    let bytes = vfs.request_size_upper_bound();
    if bytes > limit {
        Err(NativeError::InputTooLarge {
            bytes: u64::try_from(bytes).unwrap_or(u64::MAX),
            limit,
        })
    } else {
        Ok(())
    }
}

/// Selects the configured project (tsconfig path) or the first project.
fn select_project(snapshot: &SnapshotResult, tsconfig: &str) -> Option<String> {
    snapshot
        .projects()
        .iter()
        .find(|project| project.id() == tsconfig)
        .or_else(|| snapshot.projects().first())
        .map(|project| project.id().to_owned())
}

/// Proves the virtual dispatcher refuses a real host file outside the root.
///
/// The pinned executable is known to exist and cannot be inside the empty
/// sandbox, so it is a safe canary without creating a temporary host file.
fn confinement_self_check(options: &SessionOptions) -> Result<(), NativeError> {
    let root = options.sandbox_root.to_str().ok_or_else(handshake_error)?;
    let probe_path = format!("{root}/probe.ts");
    let vfs = VirtualFileSystem::from_root_and_files(root, [(probe_path, PROBE_SOURCE.to_owned())])
        .map_err(|error| map_vfs(&error))?;
    let canary = options.executable.to_str().ok_or_else(artifact_error)?;
    vfs.assert_confined(canary)
        .map_err(|_| NativeError::Unavailable {
            reason: UnavailableReason::VfsConfinement,
        })
}

fn probe_version(executable: &std::path::Path, engine: &PinnedEngine) -> Result<(), NativeError> {
    let directory = executable.parent().ok_or_else(artifact_error)?;
    let mut child = Command::new(executable)
        .arg("--version")
        .current_dir(directory)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| NativeError::Unavailable {
            reason: UnavailableReason::ProcessExited,
        })?;

    let deadline = Deadline::after(Duration::from_millis(HANDSHAKE_DEADLINE_MS));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(artifact_error());
                }
                break;
            }
            Ok(None) if !deadline.is_expired() => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(artifact_error());
            }
        }
    }

    let mut output = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read as _;
        stdout
            .read_to_string(&mut output)
            .map_err(|_| artifact_error())?;
    }
    if output.trim_end() != engine.version_line() {
        return Err(artifact_error());
    }
    Ok(())
}

fn map_engine_error(method: NativeMethod, error: &JsonRpcError) -> NativeError {
    if error.is_method_not_found() {
        unsupported(method.as_str())
    } else if error.is_snapshot_not_found() {
        NativeError::StaleSnapshot
    } else if error.is_stale_handle() {
        NativeError::StaleHandle
    } else {
        error.to_native_error()
    }
}

fn map_probe_error(error: NativeError) -> NativeError {
    match error {
        NativeError::Unsupported { .. } => api_mismatch(),
        other => other,
    }
}

fn parse_snapshot(value: &Value) -> Result<SnapshotResult, NativeError> {
    SnapshotResult::from_value(value)
        .map_err(|_| NativeError::Protocol("snapshot response shape mismatch".to_owned()))
}

const fn unsupported(method: &'static str) -> NativeError {
    NativeError::Unsupported { method }
}

const fn handshake_error() -> NativeError {
    NativeError::Unavailable {
        reason: UnavailableReason::HandshakeFailed,
    }
}

const fn api_mismatch() -> NativeError {
    NativeError::Unavailable {
        reason: UnavailableReason::ApiMismatch,
    }
}

const fn artifact_error() -> NativeError {
    NativeError::Unavailable {
        reason: UnavailableReason::ArtifactMismatch,
    }
}

const fn map_spawn_error(error: &SpawnError) -> NativeError {
    match error {
        SpawnError::ExecutableMismatch
        | SpawnError::ExecutableMissing
        | SpawnError::ExecutableSymlink
        | SpawnError::ExecutableNotFile
        | SpawnError::ExecutableNotExecutable
        | SpawnError::ExecutableIo(_) => artifact_error(),
        SpawnError::InvalidExecutablePath => NativeError::InvalidHandle {
            reason: "engine executable path is not normalized",
        },
        _ => NativeError::Unavailable {
            reason: UnavailableReason::ProcessExited,
        },
    }
}

fn map_supervisor_error(error: SupervisorError) -> NativeError {
    match error {
        SupervisorError::Spawn(spawn) => map_spawn_error(&spawn),
        other => NativeError::from(other),
    }
}

fn map_vfs(error: &VfsError) -> NativeError {
    match error {
        VfsError::InvalidPath { .. } | VfsError::Root(_) => NativeError::InvalidParams {
            reason: "invalid project path",
        },
        VfsError::DuplicateFile { .. } => {
            NativeError::Protocol("duplicate virtual file".to_owned())
        }
        VfsError::Confinement { .. } => NativeError::Unavailable {
            reason: UnavailableReason::VfsConfinement,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_selection_prefers_the_configuration() {
        let snapshot = SnapshotResult::from_value(&serde_json::json!({
            "snapshot": 3,
            "projects": [
                {"id": "/dev/null/inferred", "currentDirectory": "/work"},
                {"id": "/work/tsconfig.json", "currentDirectory": "/work"},
            ],
        }))
        .expect("snapshot");
        assert_eq!(
            select_project(&snapshot, "/work/tsconfig.json").as_deref(),
            Some("/work/tsconfig.json")
        );
        assert_eq!(
            select_project(&snapshot, "/other/tsconfig.json").as_deref(),
            Some("/dev/null/inferred")
        );
    }

    #[test]
    fn stale_responses_are_detected_by_snapshot_identity() {
        let current = EngineSnapshotId::new(3);
        assert!(snapshot_is_current(Some(current), None));
        assert!(snapshot_is_current(Some(current), Some(current)));
        assert!(!snapshot_is_current(
            Some(current),
            Some(EngineSnapshotId::new(4))
        ));
        assert!(!snapshot_is_current(None, Some(current)));
    }

    #[test]
    fn spawn_errors_distinguish_artifact_from_process_failures() {
        assert!(matches!(
            map_spawn_error(&SpawnError::ExecutableMismatch),
            NativeError::Unavailable {
                reason: UnavailableReason::ArtifactMismatch
            }
        ));
        assert!(matches!(
            map_spawn_error(&SpawnError::SandboxNotEmpty),
            NativeError::Unavailable {
                reason: UnavailableReason::ProcessExited
            }
        ));
    }

    #[test]
    fn snapshot_ledger_tracks_only_confirmed_releases_and_enforces_the_cap() {
        let mut ledger = SnapshotLedger::default();
        for id in 1..=MAX_LIVE_SNAPSHOTS {
            ledger.track(EngineSnapshotId::new(
                u64::try_from(id).expect("snapshot id"),
            ));
        }
        assert_eq!(ledger.len(), MAX_LIVE_SNAPSHOTS);
        assert_eq!(
            ledger.oldest_excluding(Some(EngineSnapshotId::new(
                u64::try_from(MAX_LIVE_SNAPSHOTS).expect("cap")
            ))),
            Some(EngineSnapshotId::new(1))
        );
        ledger.track(EngineSnapshotId::new(1));
        assert_eq!(ledger.len(), MAX_LIVE_SNAPSHOTS);
        ledger.track(EngineSnapshotId::new(9));
        assert_eq!(ledger.len(), MAX_LIVE_SNAPSHOTS + 1);
        assert!(ledger.len() > MAX_LIVE_SNAPSHOTS);

        ledger.mark_released(EngineSnapshotId::new(1));
        assert_eq!(
            ledger.oldest_excluding(Some(EngineSnapshotId::new(9))),
            Some(EngineSnapshotId::new(2))
        );
        ledger.clear();
        assert_eq!(ledger.len(), 0);
    }

    #[test]
    fn snapshot_budget_is_rejected_before_serialization() {
        let vfs = VirtualFileSystem::from_root_and_files(
            "/tmp/opencode/budget",
            [(
                "/tmp/opencode/budget/main.ts".to_owned(),
                "\u{1}\u{2}\u{3}".to_owned(),
            )],
        )
        .expect("vfs");
        assert!(matches!(
            check_request_budget(&vfs, 8),
            Err(NativeError::InputTooLarge { .. })
        ));
        assert!(check_request_budget(&vfs, usize::MAX).is_ok());
    }

    #[test]
    fn probe_absent_method_is_api_mismatch() {
        assert!(matches!(
            map_probe_error(NativeError::Unsupported {
                method: "getSyntacticDiagnostics"
            }),
            NativeError::Unavailable {
                reason: UnavailableReason::ApiMismatch
            }
        ));
        assert!(matches!(
            map_probe_error(NativeError::Timeout(10)),
            NativeError::Timeout(10)
        ));
    }

    #[test]
    fn engine_errors_are_classified_for_stale_identity() {
        let snapshot = JsonRpcError::from_value(&serde_json::json!({
            "code": -32603,
            "message": "snapshot 2 not found",
        }))
        .expect("snapshot error");
        assert!(matches!(
            map_engine_error(NativeMethod::GetTypeAtPosition, &snapshot),
            NativeError::StaleSnapshot
        ));

        let handle = JsonRpcError::from_value(&serde_json::json!({
            "code": -32603,
            "message": "node handle 9.79./work/main.ts is stale",
        }))
        .expect("handle error");
        assert!(matches!(
            map_engine_error(NativeMethod::GetTypeAtPosition, &handle),
            NativeError::StaleHandle
        ));

        let missing = JsonRpcError::from_value(&serde_json::json!({
            "code": -32601,
            "message": "unknown API method",
        }))
        .expect("method error");
        assert!(matches!(
            map_engine_error(NativeMethod::GetTypeAtPosition, &missing),
            NativeError::Unsupported { .. }
        ));
    }
}
