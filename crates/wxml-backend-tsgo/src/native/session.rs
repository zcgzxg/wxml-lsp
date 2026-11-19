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
//! opens the project and its files. An input created with
//! [`AnalysisInput::with_environment`](crate::input::AnalysisInput::with_environment)
//! also mounts the verified TypeScript standard library and mini-program
//! typings as virtual files; the legacy constructor keeps the historical
//! `noLib:true` probe configuration. An invalid or incomplete asset set is
//! rejected as `Unavailable(MissingAsset)` before any snapshot is created.
//! [`NativeSession::update_project`] sends the
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
//!
//! The engine has no definition method. [`NativeSession::definition`] composes
//! `getSymbolAtPosition`, a same-snapshot Identifier node,
//! `getReferencedSymbolsForNode` and declaration-handle AST decoding without
//! publishing a raw handle as a location.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::config::{MAX_LIVE_SNAPSHOTS, PinnedEngine};
use crate::input::{AnalysisInput, ProjectKey, SnapshotRevision, TypeEnvironmentIdentity};
use crate::native::ast::{
    AstError, AstHeader, AstRange, AstTree, IDENTIFIER_KIND, decode_base64, find_declaration_name,
    find_identifier, node_span,
};
use crate::native::document_symbol::DocumentSymbolsEvidence;
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
    Utf8Offset, Utf16Offset, Utf16Position, position_to_offset, utf16_len, utf16_to_utf8,
};
use crate::native::supervisor::{
    Deadline, EngineSupervisor, HANDSHAKE_DEADLINE_MS, InboundEvent, ProcessSession,
    SHUTDOWN_DEADLINE_MS, ShutdownReport, SpawnError, SupervisorError, verify_engine_executable,
};
use crate::native::vfs::{CallbackError, EnvironmentDegradation, VfsError, VirtualFileSystem};
use crate::native::wire::{
    RawCompletionList, RawDiagnostic, RawJsDocTag, RawReferencedSymbol, RawSourceFile, RawSymbol,
    RawSymbolReference, RawType, SnapshotChanges, SnapshotFileSystem, completion_params,
    create_snapshot_params, file_params, files_params, node_params, parse_optional, parse_response,
    project_params, release_params, source_file_params, symbol_params, type_params,
    update_snapshot_params,
};
use crate::native::{NativeError, UnavailableReason};
use crate::query::documentation::map_documentation;
use crate::query::hover::{
    DeclarationInsert, HoverDeclaration, HoverEvidence, HoverGap, HoverSymbolFacts,
    MAX_HOVER_DECLARATIONS, insert_declaration,
};
use crate::query::type_info::map_type_query;
use crate::script::symbols::Utf16Range;
use crate::shadow::mapping::SourceRange;

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

/// Maximum distinct definition targets retained for one query.
///
/// This matches the C0 definition publication cap in `wxml-lsp`
/// (`MAX_DEFINITION_LOCATIONS = 16`) so native evidence cannot exceed what the
/// server boundary accepts. Deduplication happens before this cap is applied.
pub const MAX_DEFINITION_TARGETS: usize = 16;

/// Maximum decoded AST bytes retained across declaration files for one query.
const MAX_DEFINITION_AST_CACHE_BYTES: usize = 32 * 1024 * 1024;

/// One declaration target decoded from a definition handle.
///
/// The path is project-relative and normalized. Both coordinate systems are
/// carried because the AST produces UTF-16 and the C0 boundary consumes UTF-8;
/// the pair comes from the same source snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionTargetEvidence {
    /// Normalized project-relative target path.
    pub path: String,
    /// Inclusive UTF-16 token start.
    pub start_utf16: Utf16Offset,
    /// Exclusive UTF-16 token end.
    pub end_utf16: Utf16Offset,
    /// Inclusive UTF-8 byte start.
    pub start_utf8: Utf8Offset,
    /// Exclusive UTF-8 byte end.
    pub end_utf8: Utf8Offset,
}

/// Raw definition evidence for one position.
///
/// `symbol: None` is a checked "no symbol at this position". `identifier:
/// None` with a symbol means the position has a symbol but no Identifier node
/// (for example a quoted member access); the presentation layer maps that to
/// `Partial`, because a checked "no definition" cannot be proven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionEvidence {
    /// Symbol at the queried position, when one exists.
    pub symbol: Option<BoundSymbol>,
    /// Identifier handle sent to `getReferencedSymbolsForNode`.
    pub identifier: Option<BoundNodeHandle>,
    /// Decoded declaration targets, deduplicated in engine order.
    pub targets: Vec<DefinitionTargetEvidence>,
    /// Referenced-symbol groups returned by the engine.
    pub groups: usize,
    /// Candidate declaration handles that could not be decoded.
    pub decode_failures: usize,
    /// Whether the target cap cut the answer short.
    pub truncated: bool,
}

impl DefinitionEvidence {
    const fn empty(symbol: Option<BoundSymbol>, identifier: Option<BoundNodeHandle>) -> Self {
        Self {
            symbol,
            identifier,
            targets: Vec::new(),
            groups: 0,
            decode_failures: 0,
            truncated: false,
        }
    }
}

/// One declaration-handle decode outcome for hover evidence.
#[derive(Debug)]
enum HoverDeclarationOutcome {
    /// The handle decoded into a verified declaration name token.
    Decoded(HoverDeclaration),
    /// The handle points outside the project snapshot.
    External(String),
    /// The handle could not be decoded into a verified token.
    Undecodable(String),
    /// The declaration's name token does not match the queried symbol.
    NameMismatch(String),
}

/// Fixed upper bound on declaration handles inspected for one hover.
///
/// Only the first [`MAX_HOVER_DECLARATION_SCAN`] symbol declarations are
/// decoded, so a hostile or synthetic symbol cannot trigger an unbounded number
/// of AST fetches or token scans. Hitting the cap is reported as
/// [`HoverGap::TruncatedDeclarationScan`].
const MAX_HOVER_DECLARATION_SCAN: usize = 64;

/// Cause of a symbol handle that could not be bound.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DroppedHandleCause {
    /// The raw handle names a file that is definitely outside the project
    /// snapshot (for example a standard-library or type-asset declaration).
    OutOfSnapshot(String),
    /// The handle was malformed or the cause cannot be proven.
    Unknown,
}

/// Dropped symbol handles with a truthful cause.
#[derive(Debug, Default)]
struct SymbolDrops {
    declarations: Vec<DroppedHandleCause>,
    value_declaration: Option<DroppedHandleCause>,
    reference: Option<DroppedHandleCause>,
}

/// Cached AST state for hover declaration decoding.
#[derive(Debug)]
enum HoverAstCacheEntry {
    /// The decoded v9 AST payload, shared cheaply with the current lookup.
    Ast(Arc<Vec<u8>>),
    /// The next payload would exceed the shared AST cache budget.
    BudgetExceeded,
    /// The AST could not be obtained for a local, non-structural reason.
    Unavailable,
}

/// One hover AST cache lookup outcome.
#[derive(Debug)]
enum HoverAstLookup {
    /// Payload shared from the per-query cache.
    Payload(Arc<Vec<u8>>),
    /// The payload is unavailable for a non-structural reason.
    Unavailable(String),
}

/// Result of the bounded hover declaration scan.
#[derive(Debug)]
struct HoverDeclarationScan {
    declarations: Vec<HoverDeclaration>,
    gaps: Vec<HoverGap>,
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
    type_environment: Option<TypeEnvironmentIdentity>,
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
            type_environment: None,
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

    /// Returns the identity of the open type environment.
    #[must_use]
    pub const fn type_environment_identity(&self) -> Option<&TypeEnvironmentIdentity> {
        self.type_environment.as_ref()
    }

    /// Returns explicit type-environment degradations from the open VFS.
    #[must_use]
    pub fn environment_degradations(&self) -> &[EnvironmentDegradation] {
        self.vfs
            .as_ref()
            .map_or(&[], VirtualFileSystem::environment_degradations)
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
        let type_environment = input.type_environment_identity();
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
        self.type_environment = type_environment;
        self.dispatch.clear_abandoned();
        if let Err(error) = self.enforce_live_cap() {
            self.epoch = None;
            self.project_id = None;
            self.project_key = None;
            self.project_root = None;
            self.type_environment = None;
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
        let type_environment = input.type_environment_identity();
        validate_environment_transition(self.type_environment.as_ref(), type_environment.as_ref())?;

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
        self.type_environment = type_environment;
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
        self.type_environment = None;
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
        self.type_environment = None;
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
        let Some(symbol) = self.raw_symbol_at_position(path, position)? else {
            return Ok(None);
        };
        self.bind_symbol(&symbol, path).map(Some)
    }

    /// Returns the raw (unbound) symbol at a UTF-16 position.
    fn raw_symbol_at_position(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<Option<RawSymbol>, NativeError> {
        let (epoch, project, absolute) = self.file_context(path)?;
        let offset = self.position_offset(&absolute, position)?;
        let value = self.call(
            NativeMethod::GetSymbolAtPosition,
            file_params(epoch.snapshot.get(), &project, &absolute, Some(offset)),
            Deadline::request(self.limits),
            Some(epoch.snapshot),
        )?;
        parse_optional(value)
    }

    /// Binds a raw symbol and its handles to the current epoch.
    ///
    /// Declarations that cannot be bound (outside the current snapshot or
    /// malformed) are omitted and recorded as a degradation; hard session
    /// failures propagate.
    ///
    /// # Errors
    ///
    /// Requires an open snapshot and a project-relative query path. Malformed
    /// or out-of-snapshot handles are dropped and counted, not propagated.
    pub fn bind_symbol(&self, symbol: &RawSymbol, path: &str) -> Result<BoundSymbol, NativeError> {
        self.bind_symbol_with_drops(symbol, path)
            .map(|(symbol, _drops)| symbol)
    }

    /// Binds a raw symbol and returns the dropped-handle causes.
    fn bind_symbol_with_drops(
        &self,
        symbol: &RawSymbol,
        path: &str,
    ) -> Result<(BoundSymbol, SymbolDrops), NativeError> {
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

        let mut drops = SymbolDrops::default();
        let mut declarations = Vec::new();
        let mut dropped_declarations = 0usize;
        for raw in &symbol.declarations {
            match self.bind_handle(raw) {
                Ok(handle) => declarations.push(handle),
                Err(NativeError::InvalidHandle { .. }) => {
                    dropped_declarations = dropped_declarations.saturating_add(1);
                    drops.declarations.push(dropped_handle_cause(raw, vfs));
                }
                Err(error) => return Err(error),
            }
        }
        let value_declaration = match symbol.value_declaration.as_deref() {
            Some(raw) => match self.bind_handle(raw) {
                Ok(handle) => Some(handle),
                Err(NativeError::InvalidHandle { .. }) => {
                    dropped_declarations = dropped_declarations.saturating_add(1);
                    drops.value_declaration = Some(dropped_handle_cause(raw, vfs));
                    None
                }
                Err(error) => return Err(error),
            },
            None => None,
        };
        let (reference, reference_dropped) = match symbol.reference.as_ref() {
            Some(raw) => match self.bind_symbol_reference(raw) {
                Ok(reference) => (Some(reference), false),
                Err(NativeError::InvalidHandle { .. }) => {
                    drops.reference = Some(dropped_reference_cause(raw, vfs));
                    (None, true)
                }
                Err(error) => return Err(error),
            },
            None => (None, false),
        };

        let bound = BoundSymbol::new(
            symbol.name.clone(),
            symbol.flags,
            declarations,
            value_declaration,
            reference,
            dropped_declarations,
            reference_dropped,
            binding,
        );
        Ok((bound, drops))
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

    /// Composes truthful hover evidence for one position.
    ///
    /// The answer aggregates `getTypeAtPosition`/`typeToString`,
    /// `getSymbolAtPosition`, `getDocumentationComment`/`getJsDocTags` and
    /// declaration decoding from the same snapshot AST. No native method outside
    /// the frozen allowlist is used and no host file is read.
    ///
    /// Every declaration handle is decoded into its declaration's name token;
    /// targets are deduplicated by `(path, start, end)` and capped at
    /// [`MAX_HOVER_DECLARATIONS`]. At most `MAX_HOVER_DECLARATION_SCAN`
    /// declaration handles are inspected, so many declarations cannot trigger
    /// unbounded AST fetches or token scans. A declaration proven to be outside
    /// the project snapshot (a standard-library or mounted type-asset
    /// declaration) is recorded as an external gap. A stale, malformed or
    /// otherwise unbound handle, or a declaration whose name does not match the
    /// symbol, is recorded as its own degraded gap and counted; it is never
    /// relabeled as a proven external declaration.
    ///
    /// The session epoch, snapshot and query-file content hash captured before
    /// the calls are re-checked before returning. Only declaration handles carry
    /// epoch binding; the type and documentation values are plain query facts.
    /// The hover token range is intentionally absent; selecting it from the
    /// request is the adapter's responsibility.
    ///
    /// # Errors
    ///
    /// Requires an open snapshot and a valid position. Transport, deadline and
    /// session failures propagate; per-declaration decode failures do not.
    pub fn hover_evidence(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<HoverEvidence, NativeError> {
        let (epoch, _project, absolute) = self.file_context(path)?;
        let content_hash = {
            let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
            let file = vfs.file(&absolute).ok_or(NativeError::InvalidParams {
                reason: "file is not in the open snapshot",
            })?;
            file.hash().to_owned()
        };

        let type_result = map_type_query(&self.type_query(path, position)?);
        let mut gaps = Vec::new();
        if let Some(reason) = type_result.status.reason() {
            gaps.push(HoverGap::Unsupported {
                reason: format!("type query is incomplete: {reason}"),
            });
        }

        let raw_symbol = self.raw_symbol_at_position(path, position)?;
        let (symbol, drops) = match raw_symbol {
            Some(raw) => {
                let (symbol, drops) = self.bind_symbol_with_drops(&raw, path)?;
                (Some(symbol), Some(drops))
            }
            None => (None, None),
        };
        let mut documentation = None;
        if let Some(drops) = &drops {
            for cause in &drops.declarations {
                push_dropped_declaration_gap(cause, &mut gaps);
            }
            if let Some(cause) = &drops.value_declaration {
                push_dropped_declaration_gap(cause, &mut gaps);
            }
            if let Some(cause) = &drops.reference {
                gaps.push(match cause {
                    DroppedHandleCause::OutOfSnapshot(path) => HoverGap::Unsupported {
                        reason: format!(
                            "the symbol documentation reference `{path}` is outside the current snapshot"
                        ),
                    },
                    DroppedHandleCause::Unknown => HoverGap::Unsupported {
                        reason: "the symbol documentation reference could not be bound".to_owned(),
                    },
                });
            }
        }
        if let Some(facts) = &symbol {
            if let Some(reference) = facts.reference() {
                documentation = Some(map_documentation(&self.documentation(reference)?));
            }
        }

        let declarations = match &symbol {
            Some(facts) => self.decode_hover_declarations(facts, &mut gaps)?,
            None => Vec::new(),
        };

        self.verify_hover_epoch(epoch, &absolute, &content_hash, symbol.as_ref())?;

        let symbol = symbol.map(|facts| HoverSymbolFacts {
            name: facts.name().to_owned(),
            flags: facts.flags(),
            declarations: facts.declarations().to_vec(),
        });
        let mut evidence = HoverEvidence {
            type_info: type_result.info,
            type_quality: type_result.quality,
            symbol,
            documentation,
            declarations,
            status: wxml_types::CapabilityStatus::Supported,
            gaps,
        };
        evidence.finalize_status();
        Ok(evidence)
    }

    /// Decodes a symbol's declaration handles into verified project-relative
    /// name-token targets.
    fn decode_hover_declarations(
        &mut self,
        symbol: &BoundSymbol,
        gaps: &mut Vec<HoverGap>,
    ) -> Result<Vec<HoverDeclaration>, NativeError> {
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let query_file = vfs
            .file(symbol.binding().normalized_file_path())
            .ok_or(NativeError::StaleHandle)?;
        if !symbol.is_current(self.generation, epoch.snapshot, query_file.hash()) {
            return Err(NativeError::StaleHandle);
        }

        let mut cache = BTreeMap::new();
        let mut cache_bytes = 0usize;
        let handles = symbol.declarations();
        let scan = scan_hover_declarations(handles.len(), |index| {
            self.decode_hover_declaration(
                &handles[index],
                symbol.name(),
                &mut cache,
                &mut cache_bytes,
            )
        })?;
        gaps.extend(scan.gaps);
        Ok(scan.declarations)
    }

    /// Decodes one epoch-bound declaration handle into its name-token target.
    fn decode_hover_declaration(
        &mut self,
        declaration: &BoundNodeHandle,
        expected_name: &str,
        cache: &mut BTreeMap<String, HoverAstCacheEntry>,
        cache_bytes: &mut usize,
    ) -> Result<HoverDeclarationOutcome, NativeError> {
        match self.validate_handle(declaration) {
            Ok(()) => {}
            Err(NativeError::InvalidHandle { .. } | NativeError::StaleHandle) => {
                return Ok(HoverDeclarationOutcome::Undecodable(
                    "declaration handle is stale or invalid for the current snapshot".to_owned(),
                ));
            }
            Err(error) => return Err(error),
        }
        let absolute = declaration.binding().normalized_file_path.clone();
        {
            let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
            let Some(file) = vfs.file(&absolute) else {
                return Ok(HoverDeclarationOutcome::External(absolute));
            };
            if declaration.binding().content_hash_or_node_id.as_str() != file.hash() {
                return Err(NativeError::StaleSnapshot);
            }
        }

        let payload = match self.hover_ast_lookup(&absolute, cache, cache_bytes)? {
            HoverAstLookup::Payload(payload) => payload,
            HoverAstLookup::Unavailable(reason) => {
                return Ok(HoverDeclarationOutcome::Undecodable(reason));
            }
        };

        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let Some(file) = vfs.file(&absolute) else {
            return Ok(HoverDeclarationOutcome::External(absolute));
        };
        let source_len = utf16_len(file.text());
        let range = match find_declaration_name(
            payload.as_slice(),
            declaration.handle().encoder_index,
            declaration.handle().kind,
            expected_name,
            source_len,
        ) {
            Ok(Some(range)) => range,
            Err(error) if error.is_structural() => return Err(definition_ast_error(error)),
            Ok(None) => {
                return Ok(HoverDeclarationOutcome::NameMismatch(
                    expected_name.to_owned(),
                ));
            }
            Err(_) => {
                return Ok(HoverDeclarationOutcome::Undecodable(
                    "declaration AST node could not be decoded".to_owned(),
                ));
            }
        };
        let Ok(path) = self.project_relative_path(&absolute) else {
            return Ok(HoverDeclarationOutcome::External(absolute));
        };
        match definition_target_from_ast(path, file.text(), range, expected_name) {
            Some(target) => Ok(HoverDeclarationOutcome::Decoded(HoverDeclaration {
                path: target.path,
                range: SourceRange::new(target.start_utf8.get(), target.end_utf8.get()),
                utf16: Utf16Range::new(target.start_utf16.get(), target.end_utf16.get()),
            })),
            None => Ok(HoverDeclarationOutcome::NameMismatch(
                expected_name.to_owned(),
            )),
        }
    }

    /// Returns a cached hover AST payload or a truthful unavailability reason.
    fn hover_ast_lookup(
        &mut self,
        absolute: &str,
        cache: &mut BTreeMap<String, HoverAstCacheEntry>,
        cache_bytes: &mut usize,
    ) -> Result<HoverAstLookup, NativeError> {
        if let Some(entry) = cache.get(absolute) {
            match entry {
                HoverAstCacheEntry::Ast(payload) => {
                    return Ok(HoverAstLookup::Payload(Arc::clone(payload)));
                }
                HoverAstCacheEntry::BudgetExceeded => {
                    return Ok(HoverAstLookup::Unavailable(
                        "declaration AST exceeds the shared AST cache budget".to_owned(),
                    ));
                }
                HoverAstCacheEntry::Unavailable => {
                    return Ok(HoverAstLookup::Unavailable(
                        "declaration AST is unavailable".to_owned(),
                    ));
                }
            }
        }
        match self.get_source_ast(absolute) {
            Ok(payload) => {
                let total = cache_bytes.saturating_add(payload.len());
                if total > MAX_DEFINITION_AST_CACHE_BYTES {
                    cache.insert(absolute.to_owned(), HoverAstCacheEntry::BudgetExceeded);
                    return Ok(HoverAstLookup::Unavailable(
                        "declaration AST exceeds the shared AST cache budget".to_owned(),
                    ));
                }
                *cache_bytes = total;
                let payload = Arc::new(payload);
                cache.insert(
                    absolute.to_owned(),
                    HoverAstCacheEntry::Ast(Arc::clone(&payload)),
                );
                Ok(HoverAstLookup::Payload(payload))
            }
            Err(error) if definition_decode_error(&error) => {
                cache.insert(absolute.to_owned(), HoverAstCacheEntry::Unavailable);
                Ok(HoverAstLookup::Unavailable(
                    "declaration AST is unavailable".to_owned(),
                ))
            }
            Err(error) => Err(error),
        }
    }

    /// Rejects hover evidence assembled across a changed epoch or content hash.
    fn verify_hover_epoch(
        &self,
        epoch: NativeEpoch,
        absolute: &str,
        content_hash: &str,
        symbol: Option<&BoundSymbol>,
    ) -> Result<(), NativeError> {
        let current = self.epoch.ok_or(NativeError::NoSnapshot)?;
        if current != epoch {
            return Err(NativeError::StaleSnapshot);
        }
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let file = vfs.file(absolute).ok_or(NativeError::StaleSnapshot)?;
        if file.hash() != content_hash {
            return Err(NativeError::StaleSnapshot);
        }
        if let Some(symbol) = symbol {
            if !symbol.is_current(self.generation, current.snapshot, content_hash) {
                return Err(NativeError::StaleHandle);
            }
        }
        Ok(())
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

    /// Resolves the definition at a UTF-16 position from native evidence.
    ///
    /// The composition is: symbol at position, same-snapshot Identifier node,
    /// `getReferencedSymbolsForNode`, then decoding every group's declaration
    /// handle back into the current snapshot's AST. Declaration handles are
    /// never published as locations; they are decoded into the declaration's
    /// name Identifier token first.
    ///
    /// A missing symbol is a checked empty answer. A symbol without an
    /// Identifier and undecodable declaration candidates stay visible in
    /// [`DefinitionEvidence`] so the presentation layer returns `Partial`
    /// instead of a fake definition.
    ///
    /// # Errors
    ///
    /// Requires an open snapshot and a valid position. Transport, deadline and
    /// session failures propagate; individual declaration decode failures do
    /// not.
    pub fn definition(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<DefinitionEvidence, NativeError> {
        let (_, _, absolute) = self.file_context(path)?;
        let offset = self.position_offset(&absolute, position)?;
        let Some(symbol) = self.symbol_at_position(path, position)? else {
            return Ok(DefinitionEvidence::empty(None, None));
        };
        let Some(identifier) = self.identifier_handle(&absolute, offset, symbol.name())? else {
            return Ok(DefinitionEvidence::empty(Some(symbol), None));
        };
        require_identifier_kind(&identifier)?;
        // `identifier_handle` already proved that `offset` lies inside this
        // exact Identifier node, so the primary path does not refetch its AST.
        self.collect_definition_targets(&identifier, position, &symbol)
    }

    /// Resolves the definition for an already bound Identifier handle.
    ///
    /// The handle must belong to the current session generation, engine
    /// snapshot and file content; stale or foreign handles return
    /// [`NativeError::StaleHandle`] before any native request is sent. A
    /// non-Identifier handle returns [`NativeError::InvalidHandle`].
    ///
    /// # Errors
    ///
    /// Rejects a stale or non-Identifier handle, a position outside the
    /// identifier token, or a session without an open snapshot.
    pub fn definition_for_identifier(
        &mut self,
        identifier: &BoundNodeHandle,
        position: Utf16Position,
    ) -> Result<DefinitionEvidence, NativeError> {
        require_identifier_kind(identifier)?;
        self.validate_handle(identifier)?;
        let relative =
            self.project_relative_path(identifier.binding().normalized_file_path.as_str())?;
        let Some(symbol) = self.symbol_at_position(&relative, position)? else {
            return Ok(DefinitionEvidence::empty(None, Some(identifier.clone())));
        };
        self.verify_identifier_position(identifier, position, &symbol)?;
        self.collect_definition_targets(identifier, position, &symbol)
    }

    /// Verifies that a caller-supplied Identifier handle really contains the
    /// requested position.
    ///
    /// The AST token start is derived from the pool name. For an escaped
    /// identifier (`\u0061bc`) the source slice cannot equal the decoded name,
    /// so the full node span is used instead; a position inside the escape must
    /// not become `InvalidParams`.
    fn verify_identifier_position(
        &mut self,
        identifier: &BoundNodeHandle,
        position: Utf16Position,
        symbol: &BoundSymbol,
    ) -> Result<(), NativeError> {
        self.validate_handle(identifier)?;
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let absolute = identifier.binding().normalized_file_path.clone();
        let file = vfs.file(&absolute).ok_or(NativeError::StaleHandle)?;
        let content_hash = file.hash().to_owned();
        let source_len = utf16_len(file.text());
        if !symbol.is_current(self.generation, epoch.snapshot, &content_hash) {
            return Err(NativeError::StaleHandle);
        }
        let payload = self.get_source_ast(&absolute)?;
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let file = vfs.file(&absolute).ok_or(NativeError::StaleHandle)?;
        let token = match find_declaration_name(
            &payload,
            identifier.handle().encoder_index,
            identifier.handle().kind,
            symbol.name(),
            source_len,
        ) {
            Ok(Some(range)) => range,
            Ok(None) => {
                return Err(NativeError::InvalidHandle {
                    reason: "identifier handle does not resolve to an identifier token",
                });
            }
            Err(error) => return Err(definition_ast_error(error)),
        };
        let span = node_span(
            &payload,
            identifier.handle().encoder_index,
            identifier.handle().kind,
            source_len,
        )
        .map_err(definition_ast_error)?;
        let offset = self.position_offset(&absolute, position)?;
        let bounds = identifier_position_bounds(file.text(), token, span, symbol.name());
        if offset < bounds.start || offset >= bounds.end {
            return Err(NativeError::InvalidParams {
                reason: "position is not inside the definition identifier",
            });
        }
        Ok(())
    }

    fn collect_definition_targets(
        &mut self,
        identifier: &BoundNodeHandle,
        position: Utf16Position,
        symbol: &BoundSymbol,
    ) -> Result<DefinitionEvidence, NativeError> {
        require_identifier_kind(identifier)?;
        self.validate_handle(identifier)?;
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let absolute = identifier.binding().normalized_file_path.clone();
        let file = vfs.file(&absolute).ok_or(NativeError::StaleHandle)?;
        if !symbol.is_current(self.generation, epoch.snapshot, file.hash()) {
            return Err(NativeError::StaleHandle);
        }

        let groups = self.references_for_handle(identifier, position)?;
        let mut targets = Vec::new();
        let mut decode_failures = 0usize;
        let mut truncated = false;
        let mut cache = BTreeMap::new();
        let mut cache_bytes = 0usize;
        for group in &groups {
            let expected = group
                .symbol
                .as_ref()
                .map_or_else(|| symbol.name(), |group_symbol| group_symbol.name.as_str());
            let Some(target) = self.decode_definition_target(
                &group.definition,
                expected,
                &mut cache,
                &mut cache_bytes,
            )?
            else {
                decode_failures = decode_failures.saturating_add(1);
                continue;
            };
            if targets.contains(&target) {
                continue;
            }
            if targets.len() == MAX_DEFINITION_TARGETS {
                truncated = true;
                break;
            }
            targets.push(target);
        }

        Ok(DefinitionEvidence {
            symbol: Some(symbol.clone()),
            identifier: Some(identifier.clone()),
            targets,
            groups: groups.len(),
            decode_failures,
            truncated,
        })
    }

    fn decode_definition_target(
        &mut self,
        definition: &str,
        expected_name: &str,
        cache: &mut BTreeMap<String, Option<Vec<u8>>>,
        cache_bytes: &mut usize,
    ) -> Result<Option<DefinitionTargetEvidence>, NativeError> {
        let bound = match self.bind_handle(definition) {
            Ok(bound) => bound,
            Err(NativeError::InvalidHandle { .. } | NativeError::StaleHandle) => return Ok(None),
            Err(error) => return Err(error),
        };
        match self.validate_handle(&bound) {
            Ok(()) => {}
            Err(NativeError::InvalidHandle { .. } | NativeError::StaleHandle) => return Ok(None),
            Err(error) => return Err(error),
        }
        let absolute = bound.binding().normalized_file_path.clone();
        let payload = if let Some(entry) = cache.get(&absolute) {
            match entry {
                Some(payload) => payload,
                None => return Ok(None),
            }
        } else {
            match self.get_source_ast(&absolute) {
                Ok(payload) => {
                    let total = cache_bytes.saturating_add(payload.len());
                    if total > MAX_DEFINITION_AST_CACHE_BYTES {
                        cache.insert(absolute.clone(), None);
                        return Ok(None);
                    }
                    *cache_bytes = total;
                    cache.insert(absolute.clone(), Some(payload));
                    match cache.get(&absolute) {
                        Some(Some(payload)) => payload,
                        _ => return Ok(None),
                    }
                }
                Err(error) if definition_decode_error(&error) => return Ok(None),
                Err(error) => return Err(error),
            }
        };
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let Some(file) = vfs.file(&absolute) else {
            return Ok(None);
        };
        let source_len = utf16_len(file.text());
        let range = match find_declaration_name(
            payload.as_slice(),
            bound.handle().encoder_index,
            bound.handle().kind,
            expected_name,
            source_len,
        ) {
            Ok(Some(range)) => range,
            Err(error) if error.is_structural() => return Err(definition_ast_error(error)),
            Ok(None) | Err(_) => return Ok(None),
        };
        let Ok(path) = self.project_relative_path(&absolute) else {
            return Ok(None);
        };
        Ok(definition_target_from_ast(
            path,
            file.text(),
            range,
            expected_name,
        ))
    }

    /// Returns the Identifier node handle at `offset` when its pool text is
    /// `name`.
    fn identifier_handle(
        &mut self,
        absolute: &str,
        offset: u32,
        name: &str,
    ) -> Result<Option<BoundNodeHandle>, NativeError> {
        let source_len = {
            let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
            let file = vfs.file(absolute).ok_or(NativeError::InvalidParams {
                reason: "file is not in the open snapshot",
            })?;
            utf16_len(file.text())
        };
        let payload = self.get_source_ast(absolute)?;
        let found = find_identifier(&payload, offset, name, source_len)
            .map_err(|_| NativeError::Protocol("engine AST schema mismatch".to_owned()))?;
        let Some((index, kind)) = found else {
            return Ok(None);
        };
        let raw = format!("{index}.{kind}.{absolute}");
        self.bind_handle(&raw).map(Some)
    }

    /// Fetches and base64-decodes the current snapshot AST for one file.
    fn get_source_ast(&mut self, absolute: &str) -> Result<Vec<u8>, NativeError> {
        let epoch = self.epoch.ok_or(NativeError::NoSnapshot)?;
        let project = self.project_id.clone().ok_or(NativeError::NoSnapshot)?;
        let value = self.call(
            NativeMethod::GetSourceFile,
            source_file_params(epoch.snapshot.get(), &project, absolute),
            Deadline::request(self.limits),
            Some(epoch.snapshot),
        )?;
        let source: RawSourceFile = parse_response(value)?;
        decode_ast_payload(&source.data)
    }

    /// Converts an absolute snapshot path into its normalized project-relative
    /// form.
    fn project_relative_path(&self, absolute: &str) -> Result<String, NativeError> {
        let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
        let Some(rest) = absolute.strip_prefix(vfs.root()) else {
            return Err(NativeError::InvalidHandle {
                reason: "definition target is outside the project root",
            });
        };
        let Some(rest) = rest.strip_prefix('/') else {
            return Err(NativeError::InvalidHandle {
                reason: "definition target is not a project file",
            });
        };
        if rest.is_empty() {
            return Err(NativeError::InvalidHandle {
                reason: "definition target is the project root",
            });
        }
        Ok(rest.to_owned())
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

    /// Returns native document symbols for one project-relative file.
    ///
    /// The AST and the source text are captured from the same current epoch.
    /// A changed snapshot or file content is rejected as
    /// [`NativeError::StaleSnapshot`] instead of combining bytes from different
    /// revisions. The returned evidence is still raw: callers must map it with
    /// `query::document_symbol::map_document_symbols` and preserve `Partial`.
    ///
    /// # Errors
    ///
    /// Requires an open snapshot and a file inside it. Transport, deadline,
    /// stale-snapshot and structural AST failures propagate.
    pub fn document_symbols(&mut self, path: &str) -> Result<DocumentSymbolsEvidence, NativeError> {
        let (epoch, _project, absolute) = self.file_context(path)?;
        let (source, content_hash) = {
            let vfs = self.vfs.as_ref().ok_or(NativeError::NoSnapshot)?;
            let file = vfs.file(&absolute).ok_or(NativeError::InvalidParams {
                reason: "file is not in the open snapshot",
            })?;
            (file.text().to_owned(), file.hash().to_owned())
        };
        let payload = self.get_source_ast(&absolute)?;

        let current_hash = self
            .vfs
            .as_ref()
            .and_then(|vfs| vfs.file(&absolute))
            .map(crate::native::vfs::VirtualFile::hash);
        if !document_symbol_binding_is_current(self.epoch, current_hash, epoch, &content_hash) {
            return Err(NativeError::StaleSnapshot);
        }

        let source_utf16_len = utf16_len(&source);
        let ast = AstTree::parse(&payload, source_utf16_len).map_err(document_symbol_ast_error)?;
        crate::native::document_symbol::document_symbols(&ast, &source)
            .map_err(document_symbol_ast_error)
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

/// Returns whether a document-symbol query still sees the epoch and content it
/// captured before `getSourceFile`.
fn document_symbol_binding_is_current(
    current_epoch: Option<NativeEpoch>,
    current_hash: Option<&str>,
    captured_epoch: NativeEpoch,
    captured_hash: &str,
) -> bool {
    snapshot_is_current(
        current_epoch.map(|epoch| epoch.snapshot),
        Some(captured_epoch.snapshot),
    ) && current_hash == Some(captured_hash)
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

/// Decodes the base64 AST member into raw v9 bytes.
fn decode_ast_payload(data: &str) -> Result<Vec<u8>, NativeError> {
    decode_base64(data)
        .map_err(|_| NativeError::Protocol("engine AST payload is not base64".to_owned()))
}

/// Classifies an AST failure for a definition candidate.
///
/// Structural payload corruption is a protocol failure, never a partial
/// definition. Missing/mismatched nodes and out-of-source ranges are local to
/// one candidate and may degrade to `Partial`.
fn definition_ast_error(error: AstError) -> NativeError {
    if error.is_structural() {
        NativeError::Protocol("engine AST schema mismatch".to_owned())
    } else {
        NativeError::InvalidHandle {
            reason: "definition AST node is outside the candidate",
        }
    }
}

/// Document-symbol AST failures are protocol failures.
///
/// The walker records declaration-local problems in its evidence; an error that
/// escapes is a payload/source/header mismatch, not one bad candidate.
fn document_symbol_ast_error(_error: AstError) -> NativeError {
    NativeError::Protocol("engine AST schema mismatch".to_owned())
}

/// Returns whether a declaration decode failure is local and should degrade the
/// answer to `Partial` instead of aborting the whole definition query.
///
/// `Protocol` is deliberately absent: base64, header/schema, budget and string
/// corruption must abort instead of being reported as an incomplete answer.
const fn definition_decode_error(error: &NativeError) -> bool {
    matches!(
        error,
        NativeError::InvalidParams { .. }
            | NativeError::InvalidHandle { .. }
            | NativeError::StaleHandle
    )
}

/// Requires a definition handle to address an Identifier node.
const fn require_identifier_kind(identifier: &BoundNodeHandle) -> Result<(), NativeError> {
    if identifier.handle().kind == IDENTIFIER_KIND {
        Ok(())
    } else {
        Err(NativeError::InvalidHandle {
            reason: "definition handle is not an Identifier node",
        })
    }
}

/// Scans at most [`MAX_HOVER_DECLARATION_SCAN`] declaration handles.
///
/// The scan stops early when [`MAX_HOVER_DECLARATIONS`] distinct targets have
/// been collected. Both limits are reported as typed gaps so a bounded answer
/// is never mistaken for a complete one.
fn scan_hover_declarations<F>(
    declaration_count: usize,
    mut decode: F,
) -> Result<HoverDeclarationScan, NativeError>
where
    F: FnMut(usize) -> Result<HoverDeclarationOutcome, NativeError>,
{
    let scan_limit = declaration_count.min(MAX_HOVER_DECLARATION_SCAN);
    let mut declarations = Vec::new();
    let mut gaps = Vec::new();
    let mut target_truncated = false;
    for index in 0..scan_limit {
        if declarations.len() >= MAX_HOVER_DECLARATIONS {
            target_truncated = true;
            break;
        }
        match decode(index)? {
            HoverDeclarationOutcome::Decoded(target) => {
                match insert_declaration(&mut declarations, target) {
                    DeclarationInsert::Inserted | DeclarationInsert::Duplicate => {}
                    DeclarationInsert::Truncated => {
                        target_truncated = true;
                        break;
                    }
                }
            }
            HoverDeclarationOutcome::External(path) => {
                gaps.push(HoverGap::ExternalDeclaration { path: Some(path) });
            }
            HoverDeclarationOutcome::Undecodable(reason) => {
                gaps.push(HoverGap::UndecodableDeclaration { reason });
            }
            HoverDeclarationOutcome::NameMismatch(expected) => {
                gaps.push(HoverGap::NameMismatchDeclaration { expected });
            }
        }
    }
    if target_truncated {
        gaps.push(HoverGap::TruncatedDeclarations {
            limit: MAX_HOVER_DECLARATIONS,
        });
    } else if declaration_count > scan_limit {
        gaps.push(HoverGap::TruncatedDeclarationScan {
            limit: MAX_HOVER_DECLARATION_SCAN,
        });
    }
    Ok(HoverDeclarationScan { declarations, gaps })
}

/// Adds a typed gap for one unbound declaration handle.
fn push_dropped_declaration_gap(cause: &DroppedHandleCause, gaps: &mut Vec<HoverGap>) {
    gaps.push(match cause {
        DroppedHandleCause::OutOfSnapshot(path) => HoverGap::ExternalDeclaration {
            path: Some(path.clone()),
        },
        DroppedHandleCause::Unknown => HoverGap::UndecodableDeclaration {
            reason: "a symbol declaration handle could not be bound to the current snapshot"
                .to_owned(),
        },
    });
}

/// Classifies a declaration handle that [`NativeSession::bind_handle`] rejected.
///
/// A parsed handle whose file is definitely absent from the project VFS is a
/// proven out-of-snapshot declaration (standard library or mounted asset). A
/// malformed handle, or one whose path cannot be trusted, stays `Unknown`.
fn dropped_handle_cause(raw: &str, vfs: &VirtualFileSystem) -> DroppedHandleCause {
    match NodeHandle::parse(raw) {
        Ok(handle) if vfs.file(&handle.path).is_none() => {
            DroppedHandleCause::OutOfSnapshot(handle.path)
        }
        _ => DroppedHandleCause::Unknown,
    }
}

/// Classifies a symbol reference that [`NativeSession::bind_symbol_reference`]
/// rejected without claiming a cause that was not proven.
fn dropped_reference_cause(
    reference: &RawSymbolReference,
    vfs: &VirtualFileSystem,
) -> DroppedHandleCause {
    let file = &reference.file;
    if crate::native::handles::validate_absolute_path(&file.path).is_err()
        || file.file_name != file.path
        || file.content_hash.is_empty()
        || file.parse_options_key.is_empty()
        || file.node_id.is_empty()
    {
        return DroppedHandleCause::Unknown;
    }
    if vfs.file(&file.path).is_none() {
        DroppedHandleCause::OutOfSnapshot(file.path.clone())
    } else {
        DroppedHandleCause::Unknown
    }
}

/// Returns the UTF-8 slice for a UTF-16 AST range, if both ends are boundaries.
fn utf16_slice(text: &str, range: AstRange) -> Option<&str> {
    let start = utf16_to_utf8(text, Utf16Offset::new(range.start)).ok()?;
    let end = utf16_to_utf8(text, Utf16Offset::new(range.end)).ok()?;
    text.get(
        usize::try_from(start.get()).unwrap_or(usize::MAX)
            ..usize::try_from(end.get()).unwrap_or(usize::MAX),
    )
}

/// Chooses the position bounds for a caller-supplied identifier.
///
/// The strict token range is used only when the source slice equals the decoded
/// symbol name. Escaped identifiers cannot be validated that way, so their full
/// AST node span is used instead; this keeps a position inside the escape from
/// being rejected as `InvalidParams`.
fn identifier_position_bounds(
    text: &str,
    token: AstRange,
    span: AstRange,
    expected_name: &str,
) -> AstRange {
    if utf16_slice(text, token) == Some(expected_name) {
        token
    } else {
        span
    }
}

/// Maps an AST token range to a verified target, or `None` when the source text
/// does not equal the symbol name.
fn definition_target_from_ast(
    path: String,
    text: &str,
    range: AstRange,
    expected_name: &str,
) -> Option<DefinitionTargetEvidence> {
    if expected_name.is_empty() || range.start >= range.end {
        return None;
    }
    let start_utf8 = utf16_to_utf8(text, Utf16Offset::new(range.start)).ok()?;
    let end_utf8 = utf16_to_utf8(text, Utf16Offset::new(range.end)).ok()?;
    if utf16_slice(text, range)? != expected_name {
        return None;
    }
    Some(DefinitionTargetEvidence {
        path,
        start_utf16: Utf16Offset::new(range.start),
        end_utf16: Utf16Offset::new(range.end),
        start_utf8,
        end_utf8,
    })
}

/// Rejects an update that would silently replace the open type environment.
///
/// The engine program and every published handle belong to the environment
/// captured at open time. Adding, dropping or changing it requires a reopen so
/// handles cannot survive a different lib/typings closure.
fn validate_environment_transition(
    current: Option<&TypeEnvironmentIdentity>,
    next: Option<&TypeEnvironmentIdentity>,
) -> Result<(), NativeError> {
    match (current, next) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err(NativeError::InvalidParams {
            reason: "update would add a type environment; reopen the session",
        }),
        (Some(_), None) => Err(NativeError::InvalidParams {
            reason: "update would drop the type environment; reopen the session",
        }),
        (Some(current), Some(next)) if current == next => Ok(()),
        (Some(_), Some(_)) => Err(NativeError::InvalidParams {
            reason: "type environment changed; reopen the session",
        }),
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
        // An invalid or incomplete asset set must never degrade into a
        // no-library result; the caller sees an unavailable engine instead.
        VfsError::Asset(_) => NativeError::Unavailable {
            reason: UnavailableReason::MissingAsset,
        },
        VfsError::DuplicateAsset { .. } => NativeError::Protocol("duplicate type asset".to_owned()),
        VfsError::AssetCollision { .. } => NativeError::InvalidParams {
            reason: "type asset collides with a project file",
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::assert_is_empty)]

    use super::*;
    use crate::query::document_symbol::{DocumentSymbolKind, map_document_symbols};
    use wxml_types::CapabilityStatus;

    #[test]
    fn asset_failures_map_to_fail_closed_native_errors() {
        use crate::input::TypeAssetError;

        assert!(matches!(
            map_vfs(&VfsError::Asset(TypeAssetError::MissingRequired {
                path: "/pkg/lib.d.ts".to_owned(),
            })),
            NativeError::Unavailable {
                reason: UnavailableReason::MissingAsset
            }
        ));
        assert!(matches!(
            map_vfs(&VfsError::DuplicateAsset {
                path: "/pkg/lib.d.ts".to_owned(),
            }),
            NativeError::Protocol(_)
        ));
        assert!(matches!(
            map_vfs(&VfsError::AssetCollision {
                path: "/pkg/lib.d.ts".to_owned(),
            }),
            NativeError::InvalidParams { .. }
        ));
    }

    #[test]
    fn type_environment_transitions_require_reopen() {
        use crate::input::test_support::fixture_assets;
        use crate::input::{
            CompilerOptions, DocumentRevision, ProjectRoot, SourceFile as InputSourceFile,
        };

        let input = |tag: &str| {
            AnalysisInput::with_environment(
                ProjectKey::new("environment"),
                ProjectRoot::new("/work/environment").expect("root"),
                SnapshotRevision::new(1),
                vec![
                    InputSourceFile::new("main.ts", "", DocumentRevision::new(1)).expect("main.ts"),
                ],
                CompilerOptions::default(),
                Arc::new(fixture_assets(tag)),
            )
            .expect("environment input")
        };
        let first = input("first");
        let second = input("second");
        let first_identity = first.type_environment_identity();
        let second_identity = second.type_environment_identity();
        assert!(first_identity.is_some());
        assert_ne!(first_identity, second_identity);

        assert!(validate_environment_transition(None, None).is_ok());
        assert!(
            validate_environment_transition(first_identity.as_ref(), first_identity.as_ref())
                .is_ok()
        );

        for (current, next) in [
            (None, first_identity.as_ref()),
            (first_identity.as_ref(), None),
            (first_identity.as_ref(), second_identity.as_ref()),
        ] {
            assert!(matches!(
                validate_environment_transition(current, next),
                Err(NativeError::InvalidParams { .. })
            ));
        }

        let legacy = AnalysisInput::new(
            ProjectKey::new("environment"),
            ProjectRoot::new("/work/environment").expect("root"),
            SnapshotRevision::new(2),
            vec![InputSourceFile::new("main.ts", "", DocumentRevision::new(1)).expect("main.ts")],
        )
        .expect("legacy input");
        assert!(
            validate_environment_transition(
                first_identity.as_ref(),
                legacy.type_environment_identity().as_ref()
            )
            .is_err()
        );
    }

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
    fn document_symbols_require_the_captured_epoch_and_content_hash() {
        let epoch = NativeEpoch {
            generation: SessionGeneration::new(1),
            snapshot: EngineSnapshotId::new(3),
            revision: SnapshotRevision::new(1),
        };
        assert!(document_symbol_binding_is_current(
            Some(epoch),
            Some("hash"),
            epoch,
            "hash"
        ));
        assert!(!document_symbol_binding_is_current(
            Some(NativeEpoch {
                snapshot: EngineSnapshotId::new(4),
                ..epoch
            }),
            Some("hash"),
            epoch,
            "hash"
        ));
        assert!(!document_symbol_binding_is_current(
            Some(epoch),
            Some("new-hash"),
            epoch,
            "hash"
        ));
        assert!(!document_symbol_binding_is_current(
            None, None, epoch, "hash"
        ));
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

    fn bound_handle(kind: u32) -> BoundNodeHandle {
        BoundNodeHandle::new(
            NodeHandle::parse(&format!("7.{kind}./work/main.ts")).expect("handle"),
            HandleBinding {
                session_generation: SessionGeneration::new(1),
                engine_snapshot_id: EngineSnapshotId::new(3),
                normalized_file_path: "/work/main.ts".to_owned(),
                content_hash_or_node_id: "hash".to_owned(),
                node_index: 7,
                node_kind: kind,
            },
        )
    }

    fn header_with_version(version: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        bytes[0..4].copy_from_slice(&(version << 24).to_le_bytes());
        bytes[24..28].copy_from_slice(&64u32.to_le_bytes());
        bytes[28..32].copy_from_slice(&64u32.to_le_bytes());
        bytes[32..36].copy_from_slice(&64u32.to_le_bytes());
        bytes[36..40].copy_from_slice(&64u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&64u32.to_le_bytes());
        bytes
    }

    #[test]
    fn definition_handles_require_an_identifier_node() {
        assert!(require_identifier_kind(&bound_handle(IDENTIFIER_KIND)).is_ok());
        assert!(matches!(
            require_identifier_kind(&bound_handle(173)),
            Err(NativeError::InvalidHandle { .. })
        ));
    }

    #[test]
    fn malformed_base64_non_v9_and_truncated_payloads_are_protocol_failures() {
        assert!(matches!(
            decode_ast_payload("***"),
            Err(NativeError::Protocol(_))
        ));

        let truncated = [0u8; 10];
        let error = find_declaration_name(&truncated, 1, 300, "x", 1).expect_err("truncated");
        assert!(matches!(
            definition_ast_error(error),
            NativeError::Protocol(_)
        ));

        let non_v9 = header_with_version(5);
        let error = find_declaration_name(&non_v9, 1, 300, "x", 1).expect_err("non-v9");
        assert!(matches!(
            definition_ast_error(error),
            NativeError::Protocol(_)
        ));

        assert!(matches!(
            definition_ast_error(AstError::NodeIndexOutOfRange),
            NativeError::InvalidHandle { .. }
        ));
        assert!(definition_decode_error(&NativeError::InvalidHandle {
            reason: "missing node",
        }));
        assert!(!definition_decode_error(&NativeError::Protocol(
            "bad AST".to_owned()
        )));
    }

    #[test]
    fn definition_targets_convert_offsets_after_a_cjk_and_emoji_prefix() {
        let text = "// 😀 注释\nconst name = 1;\n";
        let start = position_to_offset(
            text,
            Utf16Position {
                line: 1,
                character: 6,
            },
        )
        .expect("start")
        .get();
        let end = position_to_offset(
            text,
            Utf16Position {
                line: 1,
                character: 10,
            },
        )
        .expect("end")
        .get();
        let target = definition_target_from_ast(
            "main.ts".to_owned(),
            text,
            AstRange::new(start, end),
            "name",
        )
        .expect("target");

        assert_ne!(target.start_utf8.get(), target.start_utf16.get());
        assert_eq!(
            target.start_utf8.get(),
            utf16_to_utf8(text, Utf16Offset::new(start))
                .expect("utf8 start")
                .get()
        );
        assert_eq!(
            text.get(
                usize::try_from(target.start_utf8.get()).expect("start")
                    ..usize::try_from(target.end_utf8.get()).expect("end")
            ),
            Some("name")
        );
    }

    #[test]
    fn escaped_identifiers_use_the_node_span_for_position_checks() {
        let text = "const \\u0061bc = 1;\n";
        let computed = AstRange::new(11, 14);
        let span = AstRange::new(5, 14);
        assert_eq!(
            definition_target_from_ast("main.ts".to_owned(), text, computed, "abc",),
            None,
            "an escaped token cannot be verified as literal source text"
        );
        let bounds = identifier_position_bounds(text, computed, span, "abc");
        assert_eq!(bounds, span);
        assert!(
            bounds.start <= 6 && 6 < bounds.end,
            "a position inside the escape must remain inside the accepted span"
        );
    }

    struct TempTree(PathBuf);

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn real_engine_doc_symbol_session(
        tag: &str,
        source: &str,
    ) -> Option<(NativeSession, TempTree)> {
        use crate::input::{DocumentRevision, ProjectRoot, SourceFile as InputSourceFile};
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let Ok(bin) = std::env::var("TSGO_BIN") else {
            let required = std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref() == Ok("1");
            assert!(
                !required,
                "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1; \
                 set it to the pinned 7.1.0-beta lib/tsc"
            );
            eprintln!(
                "real-engine document-symbol test `{tag}` skipped: set TSGO_BIN to the pinned \
                 7.1.0-beta lib/tsc (or WXML_REQUIRE_REAL_ENGINE=1 to make this a hard gate)"
            );
            return None;
        };
        let executable = std::path::Path::new(&bin);
        assert!(
            executable.is_file(),
            "TSGO_BIN does not point at a file: {bin}"
        );

        let suffix = COUNTER.fetch_add(1, Ordering::Relaxed);
        let sandbox = std::env::temp_dir().join(format!(
            "wxml-tsgo-doc-symbols-{tag}-sandbox-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&sandbox).expect("sandbox");
        let guard = TempTree(sandbox.clone());

        let project_root = format!(
            "/tmp/opencode/wxml-tsgo-doc-symbols-{tag}-project-{}-{suffix}",
            std::process::id()
        );
        let input = AnalysisInput::new(
            ProjectKey::new("doc-symbols"),
            ProjectRoot::new(&project_root).expect("project root"),
            SnapshotRevision::new(1),
            vec![
                InputSourceFile::new("main.ts", source, DocumentRevision::new(1)).expect("main.ts"),
            ],
        )
        .expect("analysis input");

        let mut session =
            NativeSession::start(SessionOptions::new(executable, sandbox)).expect("start engine");
        session.open_project(&input).expect("open project");
        Some((session, guard))
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn real_engine_document_symbols_are_exact_and_ordered() {
        let source = r#"// 😀 文档
export function greet(name: string): string { return name; }
export class User {
  name: string = "";
  greet(): string { return this.name; }
}
export interface Shape {
  area(): number;
}
const count = 1;
"#;
        let Some((mut session, _sandbox)) = real_engine_doc_symbol_session("exact", source) else {
            return;
        };
        let evidence = session
            .document_symbols("main.ts")
            .expect("native document symbols");
        let result = map_document_symbols(&evidence);
        println!(
            "document symbols: {:?} status={:?}",
            result
                .symbols
                .iter()
                .map(|symbol| (&symbol.name, symbol.kind))
                .collect::<Vec<_>>(),
            result.status
        );

        assert_eq!(result.status, CapabilityStatus::Supported);
        assert!(!result.truncated);
        assert_eq!(result.skipped, 0);
        assert!(result.gaps.is_empty());
        assert_eq!(
            result
                .symbols
                .iter()
                .map(|symbol| (symbol.name.as_str(), symbol.kind))
                .collect::<Vec<_>>(),
            vec![
                ("greet", DocumentSymbolKind::Function),
                ("User", DocumentSymbolKind::Class),
                ("Shape", DocumentSymbolKind::Interface),
                ("count", DocumentSymbolKind::Variable),
            ]
        );

        let greet = &result.symbols[0];
        assert_eq!(greet.range.start.get(), 0);
        assert_eq!(greet.range.end.get(), 75);
        assert_eq!(greet.utf16.start.get(), 0);
        assert_eq!(greet.utf16.end.get(), 69);
        assert_eq!(greet.selection_range.start.get(), 31);
        assert_eq!(greet.selection_range.end.get(), 36);
        assert_eq!(greet.selection_utf16.start.get(), 25);
        assert_eq!(greet.selection_utf16.end.get(), 30);

        let user = &result.symbols[1];
        assert_eq!(user.range.start.get(), 75);
        assert_eq!(user.range.end.get(), 158);
        assert_eq!(user.utf16.start.get(), 69);
        assert_eq!(user.utf16.end.get(), 152);
        assert_eq!(user.selection_range.start.get(), 89);
        assert_eq!(user.selection_range.end.get(), 93);
        assert_eq!(user.selection_utf16.start.get(), 83);
        assert_eq!(user.selection_utf16.end.get(), 87);
        assert_eq!(
            user.children
                .iter()
                .map(|symbol| (symbol.name.as_str(), symbol.kind))
                .collect::<Vec<_>>(),
            vec![
                ("name", DocumentSymbolKind::Property),
                ("greet", DocumentSymbolKind::Method),
            ]
        );
        assert_eq!(user.children[0].selection_range.start.get(), 98);
        assert_eq!(user.children[0].selection_range.end.get(), 102);
        assert_eq!(user.children[0].selection_utf16.start.get(), 92);
        assert_eq!(user.children[0].selection_utf16.end.get(), 96);
        assert_eq!(user.children[1].selection_range.start.get(), 119);
        assert_eq!(user.children[1].selection_range.end.get(), 124);
        assert_eq!(user.children[1].selection_utf16.start.get(), 113);
        assert_eq!(user.children[1].selection_utf16.end.get(), 118);

        let shape = &result.symbols[2];
        assert_eq!(shape.range.start.get(), 158);
        assert_eq!(shape.range.end.get(), 203);
        assert_eq!(shape.utf16.start.get(), 152);
        assert_eq!(shape.utf16.end.get(), 197);
        assert_eq!(shape.children.len(), 1);
        assert_eq!(shape.children[0].name, "area");
        assert_eq!(shape.children[0].kind, DocumentSymbolKind::Method);
        assert_eq!(shape.children[0].selection_range.start.get(), 186);
        assert_eq!(shape.children[0].selection_range.end.get(), 190);
        assert_eq!(shape.children[0].selection_utf16.start.get(), 180);
        assert_eq!(shape.children[0].selection_utf16.end.get(), 184);

        let count = &result.symbols[3];
        assert_eq!(count.range.start.get(), 209);
        assert_eq!(count.range.end.get(), 219);
        assert_eq!(count.utf16.start.get(), 203);
        assert_eq!(count.utf16.end.get(), 213);
        assert_eq!(count.selection_range.start.get(), 210);
        assert_eq!(count.selection_range.end.get(), 215);
        assert_eq!(count.selection_utf16.start.get(), 204);
        assert_eq!(count.selection_utf16.end.get(), 209);

        // The C0 conversion rechecks both coordinate systems against the
        // source and must keep the same tree without fabricating symbols.
        let backend = result.to_backend_result(source);
        assert_eq!(backend.status, CapabilityStatus::Supported);
        assert!(backend.gaps.is_empty());
        assert_eq!(backend.symbols.len(), result.symbols.len());
        assert_eq!(backend.symbols[0].name, "greet");
        assert_eq!(backend.symbols[0].selection_range.start(), 31);
        assert_eq!(backend.symbols[1].children.len(), 2);
        assert_eq!(
            backend.symbols[1].children[1].kind,
            wxml_types::DocumentSymbolKind::Method
        );

        let _ = session.shutdown();
    }

    #[test]
    fn real_engine_document_symbols_reject_destructuring_and_string_names() {
        use crate::native::ast::{
            OBJECT_BINDING_PATTERN_KIND, PROPERTY_DECLARATION_KIND, STRING_LITERAL_KIND,
            VARIABLE_DECLARATION_KIND,
        };

        let source = "const { a } = config;\nclass C { \"x\" = fallback; }\n";
        let Some((mut session, _sandbox)) = real_engine_doc_symbol_session("name-slots", source)
        else {
            return;
        };
        let evidence = session
            .document_symbols("main.ts")
            .expect("native document symbols");
        let result = map_document_symbols(&evidence);

        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
        assert_eq!(result.skipped, 2);
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "C");
        assert!(result.symbols[0].children.is_empty());
        assert!(
            !result
                .symbols
                .iter()
                .any(|symbol| matches!(symbol.name.as_str(), "config" | "fallback" | "x"))
        );
        assert!(result.gaps.iter().any(|gap| {
            gap.node_kind == VARIABLE_DECLARATION_KIND
                && gap.name_kind == Some(OBJECT_BINDING_PATTERN_KIND)
                && gap.reason == "name slot is not an Identifier"
        }));
        assert!(result.gaps.iter().any(|gap| {
            gap.node_kind == PROPERTY_DECLARATION_KIND
                && gap.name_kind == Some(STRING_LITERAL_KIND)
                && gap.reason == "name slot is not an Identifier"
        }));

        let _ = session.shutdown();
    }

    #[test]
    fn real_engine_document_symbols_keep_nested_namespace_children() {
        let source = "namespace A.B { const inner = 1; }\n";
        let Some((mut session, _sandbox)) =
            real_engine_doc_symbol_session("nested-namespace", source)
        else {
            return;
        };
        let evidence = session
            .document_symbols("main.ts")
            .expect("native document symbols");
        let result = map_document_symbols(&evidence);

        assert_eq!(result.status, CapabilityStatus::Supported);
        assert_eq!(result.skipped, 0);
        assert!(result.gaps.is_empty());
        assert_eq!(result.symbols.len(), 1);
        let module_a = &result.symbols[0];
        assert_eq!(module_a.name, "A");
        assert_eq!(module_a.kind, DocumentSymbolKind::Module);
        assert_eq!(module_a.range.start.get(), 0);
        assert_eq!(module_a.range.end.get(), 34);
        assert_eq!(module_a.selection_range.start.get(), 10);
        assert_eq!(module_a.selection_range.end.get(), 11);
        assert_eq!(module_a.children.len(), 1);
        let module_b = &module_a.children[0];
        assert_eq!(module_b.name, "B");
        assert_eq!(module_b.kind, DocumentSymbolKind::Module);
        assert_eq!(module_b.range.start.get(), 12);
        assert_eq!(module_b.range.end.get(), 34);
        assert_eq!(module_b.selection_range.start.get(), 12);
        assert_eq!(module_b.selection_range.end.get(), 13);
        assert_eq!(module_b.children.len(), 1);
        let inner = &module_b.children[0];
        assert_eq!(inner.name, "inner");
        assert_eq!(inner.kind, DocumentSymbolKind::Variable);
        assert_eq!(inner.range.start.get(), 21);
        assert_eq!(inner.range.end.get(), 31);
        assert_eq!(inner.selection_range.start.get(), 22);
        assert_eq!(inner.selection_range.end.get(), 27);

        let _ = session.shutdown();
    }

    #[test]
    fn real_engine_document_symbols_keep_nested_class_interface_enum_members() {
        let source = "namespace A { class C { x = 1 } interface I { y: number } enum E { Z } }\n";
        let Some((mut session, _sandbox)) =
            real_engine_doc_symbol_session("nested-members", source)
        else {
            return;
        };
        let evidence = session
            .document_symbols("main.ts")
            .expect("native document symbols");
        let result = map_document_symbols(&evidence);

        assert_eq!(result.status, CapabilityStatus::Supported);
        assert_eq!(result.skipped, 0);
        assert!(result.gaps.is_empty());
        assert_eq!(result.symbols.len(), 1);
        let module_a = &result.symbols[0];
        assert_eq!(module_a.name, "A");
        assert_eq!(module_a.range.start.get(), 0);
        assert_eq!(module_a.range.end.get(), 72);
        assert_eq!(module_a.selection_range.start.get(), 10);
        assert_eq!(module_a.selection_range.end.get(), 11);
        assert_eq!(
            module_a
                .children
                .iter()
                .map(|symbol| (symbol.name.as_str(), symbol.kind))
                .collect::<Vec<_>>(),
            vec![
                ("C", DocumentSymbolKind::Class),
                ("I", DocumentSymbolKind::Interface),
                ("E", DocumentSymbolKind::Enum),
            ]
        );

        let class_c = &module_a.children[0];
        assert_eq!(class_c.range.start.get(), 13);
        assert_eq!(class_c.range.end.get(), 31);
        assert_eq!(class_c.selection_range.start.get(), 20);
        assert_eq!(class_c.selection_range.end.get(), 21);
        assert_eq!(class_c.children.len(), 1);
        let member_x = &class_c.children[0];
        assert_eq!(member_x.name, "x");
        assert_eq!(member_x.kind, DocumentSymbolKind::Property);
        assert_eq!(member_x.range.start.get(), 23);
        assert_eq!(member_x.range.end.get(), 29);
        assert_eq!(member_x.selection_range.start.get(), 24);
        assert_eq!(member_x.selection_range.end.get(), 25);

        let interface_i = &module_a.children[1];
        assert_eq!(interface_i.range.start.get(), 31);
        assert_eq!(interface_i.range.end.get(), 57);
        assert_eq!(interface_i.selection_range.start.get(), 42);
        assert_eq!(interface_i.selection_range.end.get(), 43);
        assert_eq!(interface_i.children.len(), 1);
        let member_y = &interface_i.children[0];
        assert_eq!(member_y.name, "y");
        assert_eq!(member_y.kind, DocumentSymbolKind::Property);
        assert_eq!(member_y.range.start.get(), 45);
        assert_eq!(member_y.range.end.get(), 55);
        assert_eq!(member_y.selection_range.start.get(), 46);
        assert_eq!(member_y.selection_range.end.get(), 47);

        let enum_e = &module_a.children[2];
        assert_eq!(enum_e.range.start.get(), 57);
        assert_eq!(enum_e.range.end.get(), 70);
        assert_eq!(enum_e.selection_range.start.get(), 63);
        assert_eq!(enum_e.selection_range.end.get(), 64);
        assert_eq!(enum_e.children.len(), 1);
        let member_z = &enum_e.children[0];
        assert_eq!(member_z.name, "Z");
        assert_eq!(member_z.kind, DocumentSymbolKind::EnumMember);
        assert_eq!(member_z.range.start.get(), 66);
        assert_eq!(member_z.range.end.get(), 68);
        assert_eq!(member_z.selection_range.start.get(), 67);
        assert_eq!(member_z.selection_range.end.get(), 68);

        let _ = session.shutdown();
    }

    #[test]
    fn dropped_handle_cause_distinguishes_out_of_snapshot_from_malformed() {
        let vfs = VirtualFileSystem::from_root_and_files(
            "/work",
            [("/work/main.ts".to_owned(), "const x = 1;\n".to_owned())],
        )
        .expect("vfs");
        assert_eq!(
            dropped_handle_cause("15.174./pkg/lib.d.ts", &vfs),
            DroppedHandleCause::OutOfSnapshot("/pkg/lib.d.ts".to_owned())
        );
        assert_eq!(
            dropped_handle_cause("15.174./work/main.ts", &vfs),
            DroppedHandleCause::Unknown
        );
        assert_eq!(
            dropped_handle_cause("not-a-handle", &vfs),
            DroppedHandleCause::Unknown
        );
    }

    #[test]
    fn dropped_reference_cause_does_not_claim_out_of_snapshot_for_malformed_metadata() {
        use crate::native::wire::RawSourceFileDescriptor;

        let vfs = VirtualFileSystem::from_root_and_files(
            "/work",
            [("/work/main.ts".to_owned(), "const x = 1;\n".to_owned())],
        )
        .expect("vfs");
        let reference = |path: &str| RawSymbolReference {
            id: 7,
            kind: 0,
            file: RawSourceFileDescriptor {
                file_name: path.to_owned(),
                path: path.to_owned(),
                content_hash: "hash".to_owned(),
                parse_options_key: "0".to_owned(),
                script_kind: 3,
                node_id: "1".to_owned(),
            },
        };

        assert_eq!(
            dropped_reference_cause(&reference("/pkg/lib.d.ts"), &vfs),
            DroppedHandleCause::OutOfSnapshot("/pkg/lib.d.ts".to_owned())
        );

        let mut incomplete = reference("/pkg/lib.d.ts");
        incomplete.file.content_hash.clear();
        assert_eq!(
            dropped_reference_cause(&incomplete, &vfs),
            DroppedHandleCause::Unknown
        );

        let mut malformed = reference("relative/lib.d.ts");
        malformed.file.file_name = "relative/lib.d.ts".to_owned();
        assert_eq!(
            dropped_reference_cause(&malformed, &vfs),
            DroppedHandleCause::Unknown
        );
    }

    fn hover_declaration(index: u32) -> HoverDeclaration {
        let start = index * 8;
        HoverDeclaration {
            path: "model.ts".to_owned(),
            range: SourceRange::new(start, start + 4),
            utf16: Utf16Range::new(start, start + 4),
        }
    }

    #[test]
    fn hover_declaration_scan_is_bounded_by_scan_and_target_caps() {
        let mut calls = 0;
        let duplicates = scan_hover_declarations(10_000, |_| {
            calls += 1;
            Ok(HoverDeclarationOutcome::Decoded(hover_declaration(0)))
        })
        .expect("scan");
        assert_eq!(calls, MAX_HOVER_DECLARATION_SCAN);
        assert_eq!(duplicates.declarations.len(), 1);
        assert!(duplicates.gaps.iter().any(|gap| matches!(
            gap,
            HoverGap::TruncatedDeclarationScan { limit } if *limit == MAX_HOVER_DECLARATION_SCAN
        )));

        calls = 0;
        let unique = scan_hover_declarations(10_000, |index| {
            calls += 1;
            Ok(HoverDeclarationOutcome::Decoded(hover_declaration(
                u32::try_from(index).expect("index"),
            )))
        })
        .expect("scan");
        assert_eq!(calls, MAX_HOVER_DECLARATIONS);
        assert_eq!(unique.declarations.len(), MAX_HOVER_DECLARATIONS);
        assert!(unique.gaps.iter().any(|gap| matches!(
            gap,
            HoverGap::TruncatedDeclarations { limit } if *limit == MAX_HOVER_DECLARATIONS
        )));
        assert!(
            !unique
                .gaps
                .iter()
                .any(|gap| matches!(gap, HoverGap::TruncatedDeclarationScan { .. }))
        );
    }

    #[test]
    fn alias_name_mismatch_is_not_reported_as_undecodable() {
        let scan = scan_hover_declarations(2, |index| {
            if index == 0 {
                Ok(HoverDeclarationOutcome::Decoded(hover_declaration(0)))
            } else {
                Ok(HoverDeclarationOutcome::NameMismatch("Person".to_owned()))
            }
        })
        .expect("scan");
        assert_eq!(scan.declarations.len(), 1);
        assert!(scan
            .gaps
            .iter()
            .any(|gap| matches!(gap, HoverGap::NameMismatchDeclaration { expected } if expected == "Person")));
        assert!(
            !scan
                .gaps
                .iter()
                .any(|gap| matches!(gap, HoverGap::UndecodableDeclaration { .. }))
        );
    }
}
