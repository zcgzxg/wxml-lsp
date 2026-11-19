//! Real-engine integration gate for the P4-N vertical slice.
//!
//! Run explicitly with the pinned executable:
//!
//! ```text
//! TSGO_BIN=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc \
//!   cargo test -p wxml-backend-tsgo --test real_engine -- --nocapture
//! ```
//!
//! The test is opt-in through `TSGO_BIN` for a normal local run. When
//! `WXML_REQUIRE_REAL_ENGINE=1` is set (the CI/gate mode), a missing engine is a
//! hard failure rather than a skip.

#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use wxml_backend_tsgo::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::handles::NodeHandle;
use wxml_backend_tsgo::native::session::{AutoImportPreparation, NativeSession, SessionOptions};
use wxml_backend_tsgo::native::strings::{Utf16Position, position_to_offset};
use wxml_backend_tsgo::query::completion::{
    AutoImportContext, CompletionGap, CompletionQueryOptions, map_completions, map_completions_with,
};
use wxml_backend_tsgo::query::definition::map_definition;
use wxml_backend_tsgo::query::diagnostics::{DiagnosticSeverity, map_diagnostics};
use wxml_backend_tsgo::query::documentation::map_documentation;
use wxml_backend_tsgo::query::references::map_references;
use wxml_backend_tsgo::query::symbol::map_symbol;
use wxml_backend_tsgo::query::type_info::{TypeQuality, map_type_query};

const MAIN: &str = r#"import type { User } from "./model";
/** A documented user. */
const u: User = { name: "a", age: 1 };
u.missing;
u.
"#;

const MODEL: &str = r"export interface User {
    /** Display name. */
    name: string;
    age: number;
}
";

const MAIN_V2: &str = r#"import type { User } from "./model";
/** A documented user. */
const u: User = { name: "a", age: 1 };
const extra = 1;
u.name;
"#;

const MAIN_V3: &str = r#"import type { User } from "./model";
/** A documented user. */
const u: User = { name: "a", age: 1 };
const extra = 1;
const extra2 = 2;
u.name;
"#;

const BROKEN: &str = "const broken = ;\n";

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Cleanup(std::path::PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unique(tag: &str) -> String {
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("wxml-tsgo-p4-{tag}-{}-{count}", std::process::id())
}

const fn position(line: u32, character: u32) -> Utf16Position {
    Utf16Position { line, character }
}

fn canary_source(specifier: &str) -> String {
    format!(
        "import type {{ HostCanary }} from {specifier:?};\n\
         const host: HostCanary = {{ secret: \"x\" }};\n"
    )
}

fn input(root: &str, revision: u64, main: &str, canary_specifier: &str) -> AnalysisInput {
    let files = vec![
        SourceFile::new("main.ts", main, DocumentRevision::new(1)).expect("main.ts"),
        SourceFile::new("model.ts", MODEL, DocumentRevision::new(1)).expect("model.ts"),
        SourceFile::new("broken.ts", BROKEN, DocumentRevision::new(1)).expect("broken.ts"),
        SourceFile::new(
            "canary.ts",
            canary_source(canary_specifier),
            DocumentRevision::new(1),
        )
        .expect("canary.ts"),
    ];
    AnalysisInput::new(
        ProjectKey::new("real-engine"),
        ProjectRoot::new(root).expect("project root"),
        SnapshotRevision::new(revision),
        files,
    )
    .expect("analysis input")
}

#[test]
fn real_engine_vertical_slice() {
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref() == Ok("1");
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1; \
             set it to the pinned 7.1.0-beta lib/tsc"
        );
        eprintln!(
            "real-engine gate skipped: set TSGO_BIN to the pinned 7.1.0-beta lib/tsc \
             (or WXML_REQUIRE_REAL_ENGINE=1 to make this a hard gate)"
        );
        return;
    };
    let executable = Path::new(&bin);
    assert!(
        executable.is_file(),
        "TSGO_BIN does not point at a file: {bin}"
    );

    let sandbox = std::env::temp_dir().join(unique("sandbox"));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _sandbox_guard = Cleanup(sandbox.clone());
    assert!(
        std::fs::read_dir(&sandbox)
            .expect("sandbox listing")
            .next()
            .is_none(),
        "sandbox must start empty"
    );

    let canary_dir = std::env::temp_dir().join(unique("canary"));
    std::fs::create_dir_all(&canary_dir).expect("canary dir");
    let _canary_guard = Cleanup(canary_dir.clone());
    let canary_file = canary_dir.join("host-canary.ts");
    std::fs::write(
        &canary_file,
        "export interface HostCanary { secret: string }\n",
    )
    .expect("canary file");
    let canary_text = std::fs::read_to_string(&canary_file).expect("canary read");
    let canary_specifier = canary_file
        .with_extension("")
        .to_str()
        .expect("utf8 canary specifier")
        .to_owned();

    let project_root = format!("/tmp/opencode/{}", unique("project"));
    assert!(
        !Path::new(&project_root).exists(),
        "the virtual project root must not exist on disk: {project_root}"
    );
    assert!(
        !canary_file
            .to_str()
            .expect("utf8 canary")
            .starts_with(&project_root),
        "the canary must be outside the virtual project root"
    );

    let mut session = NativeSession::start(SessionOptions::new(executable, sandbox.clone()))
        .expect("start real engine");
    let initialize = session
        .initialize_result()
        .expect("initialize result")
        .clone();
    assert_eq!(
        initialize.current_directory(),
        sandbox.to_str().expect("sandbox utf8")
    );
    println!(
        "initialize: caseSensitivity={} currentDirectory={}",
        initialize.case_sensitivity(),
        initialize.current_directory()
    );

    let first = input(&project_root, 1, MAIN, &canary_specifier);
    session.open_project(&first).expect("open project");
    let epoch = session.epoch().expect("epoch");
    assert_eq!(session.live_snapshot_count(), 1);
    println!(
        "snapshot={} project={:?} revision={}",
        epoch.snapshot.get(),
        session.project_id(),
        epoch.revision.get()
    );

    let evidence = session
        .diagnostics("main.ts")
        .expect("semantic + syntactic diagnostics");
    let mapped = map_diagnostics(&evidence.semantic, &evidence.syntactic, MAIN);
    println!(
        "diagnostics: semantic={} syntactic={} mapped={} status={:?}",
        evidence.semantic.len(),
        evidence.syntactic.len(),
        mapped.diagnostics.len(),
        mapped.status
    );
    let missing = mapped
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == 2339)
        .expect("property error 2339");
    assert_eq!(missing.severity, DiagnosticSeverity::Error);
    assert!(missing.message.contains("missing"));
    assert!(matches!(
        mapped.status,
        wxml_types::CapabilityStatus::Supported | wxml_types::CapabilityStatus::Partial { .. }
    ));

    let broken = session
        .diagnostics("broken.ts")
        .expect("broken diagnostics");
    assert!(
        !broken.syntactic.is_empty(),
        "a syntax-only error must come from getSyntacticDiagnostics"
    );
    let broken_mapped = map_diagnostics(&broken.semantic, &broken.syntactic, BROKEN);
    assert!(
        !broken_mapped.diagnostics.is_empty(),
        "a syntax-only file must not be reported as clean"
    );
    println!(
        "syntax: semantic={} syntactic={} mapped={}",
        broken.semantic.len(),
        broken.syntactic.len(),
        broken_mapped.diagnostics.len()
    );

    let canary_diagnostics = session
        .diagnostics("canary.ts")
        .expect("canary diagnostics");
    let canary_mapped = map_diagnostics(
        &canary_diagnostics.semantic,
        &canary_diagnostics.syntactic,
        &canary_source(&canary_specifier),
    );
    assert!(
        canary_mapped
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == 2307),
        "the out-of-root host canary must not resolve: {:?}",
        canary_mapped
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.code, diagnostic.message.as_str()))
            .collect::<Vec<_>>()
    );

    let type_query = session
        .type_query("main.ts", position(3, 0))
        .expect("type query");
    let type_result = map_type_query(&type_query);
    println!(
        "type: info={:?} text={:?} quality={:?}",
        type_result.info, type_query.text, type_result.quality
    );
    assert_eq!(type_result.quality, TypeQuality::Known);
    assert!(type_result.info.expect("type info").text.contains("User"));

    let symbol = session
        .symbol_at_position("main.ts", position(3, 0))
        .expect("symbol query")
        .expect("symbol at u");
    let symbol_result = map_symbol(Some(symbol.clone()));
    println!(
        "symbol: name={} declarations={} reference={} status={:?}",
        symbol.name(),
        symbol.declarations().len(),
        symbol.reference().is_some(),
        symbol_result.status
    );
    assert_eq!(symbol.name(), "u");
    assert_ne!(symbol.declarations().len(), 0);
    assert_eq!(
        symbol_result.status,
        wxml_types::CapabilityStatus::Supported
    );
    let reference = symbol.reference().cloned().expect("symbol reference");

    let documentation = session.documentation(&reference).expect("documentation");
    let documentation_result = map_documentation(&documentation);
    println!(
        "documentation: comment={:?} tags={} status={:?}",
        documentation_result.comment,
        documentation_result.tags.len(),
        documentation_result.status
    );
    assert!(
        documentation_result
            .comment
            .as_deref()
            .is_some_and(|comment| comment.contains("A documented user."))
    );

    let references = session
        .references("main.ts", position(3, 0))
        .expect("references");
    let references_result = map_references(&references);
    let identifier = references.identifier.as_ref().expect("identifier handle");
    assert_eq!(
        identifier.handle().kind,
        79,
        "references must use an Identifier node"
    );
    assert!(
        identifier.handle().path.ends_with("main.ts"),
        "identifier handle must belong to the queried file"
    );
    assert!(matches!(
        references_result.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert_ne!(references_result.groups.len(), 0);
    let mut saw_identifier = false;
    for group in &references_result.groups {
        for handle in &group.references {
            let handle = NodeHandle::parse(handle).expect("reference handle");
            if handle.encoder_index == identifier.handle().encoder_index && handle.kind == 79 {
                saw_identifier = true;
            }
        }
    }
    assert!(
        saw_identifier,
        "the reference list must include the queried Identifier node"
    );
    println!(
        "references: groups={} handles={} identifier={:?} truncated={} status={:?}",
        references_result.groups.len(),
        references_result
            .groups
            .iter()
            .map(|group| group.references.len())
            .sum::<usize>(),
        references
            .identifier
            .as_ref()
            .map(|handle| handle.handle().encoder_index),
        references_result.truncated,
        references_result.status
    );

    let identifier_definition = session
        .definition_for_identifier(identifier, position(3, 0))
        .expect("identifier definition");
    assert!(
        !identifier_definition.targets.is_empty(),
        "an Identifier handle must resolve a definition"
    );
    assert!(matches!(
        session.definition_for_identifier(&symbol.declarations()[0], position(3, 0)),
        Err(NativeError::InvalidHandle { .. })
    ));

    let completions = session
        .completions("main.ts", position(4, 2))
        .expect("completions");
    let completion_result = map_completions(completions.as_ref());
    let names: Vec<&str> = completion_result
        .entries
        .iter()
        .map(|entry| entry.label.as_str())
        .collect();
    println!(
        "completions: entries={} status={:?} names={:?}",
        completion_result.entries.len(),
        completion_result.status,
        names.iter().take(12).collect::<Vec<_>>()
    );
    assert!(
        names.contains(&"name") && names.contains(&"age"),
        "member completions must contain name and age: {names:?}"
    );

    let old_handle = symbol.declarations()[0].clone();
    assert!(session.validate_handle(&old_handle).is_ok());

    let second = input(&project_root, 2, MAIN_V2, &canary_specifier);
    session.update_project(&second).expect("update project");
    let updated_epoch = session.epoch().expect("updated epoch");
    println!(
        "updated snapshot={} revision={} live={}",
        updated_epoch.snapshot.get(),
        updated_epoch.revision.get(),
        session.live_snapshot_count()
    );
    assert_ne!(epoch.snapshot, updated_epoch.snapshot);
    assert_eq!(session.live_snapshot_count(), 1);
    assert!(matches!(
        session.validate_handle(&old_handle),
        Err(NativeError::StaleHandle)
    ));
    assert!(matches!(
        session.documentation(&reference),
        Err(NativeError::StaleHandle)
    ));
    assert!(
        matches!(
            session.definition_for_identifier(identifier, position(3, 0)),
            Err(NativeError::StaleHandle)
        ),
        "definition must reject an identifier bound to the previous snapshot"
    );

    let updated_diagnostics = session.diagnostics("main.ts").expect("updated diagnostics");
    println!(
        "updated diagnostics: semantic={} syntactic={}",
        updated_diagnostics.semantic.len(),
        updated_diagnostics.syntactic.len()
    );
    assert!(
        updated_diagnostics
            .semantic
            .iter()
            .all(|diagnostic| diagnostic.code != 2339),
        "the missing-property error must be gone after the update"
    );

    let third = input(&project_root, 3, MAIN_V3, &canary_specifier);
    session.update_project(&third).expect("second update");
    assert_eq!(session.live_snapshot_count(), 1);

    session.release().expect("release snapshot");
    assert_eq!(session.live_snapshot_count(), 0);
    assert!(session.epoch().is_none());
    assert!(matches!(
        session.diagnostics("main.ts"),
        Err(NativeError::NoSnapshot)
    ));

    let report = session.shutdown().expect("shutdown report");
    println!(
        "shutdown: forced_kill={} detached={} stderr_discarded={}",
        report.forced_kill(),
        report.threads_detached(),
        report.stderr_discarded_bytes()
    );

    assert!(
        !Path::new(&project_root).exists(),
        "the engine must not write the virtual project root to the host"
    );
    assert_eq!(
        std::fs::read_to_string(&canary_file).expect("canary read after run"),
        canary_text,
        "the engine must not write the host canary"
    );
}

/// Opt-in real-engine gate for the F1 definition composition.
///
/// The queried `u.name` token must resolve to the `name` property declaration
/// inside `interface User` in `model.ts`, with the exact identifier token range
/// after leading `JSDoc` trivia, not the declaration node's `pos`.
#[test]
fn real_engine_definition_decodes_declaration_identifier_ranges() {
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref() == Ok("1");
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1; \
             set it to the pinned 7.1.0-beta lib/tsc"
        );
        eprintln!(
            "real-engine definition gate skipped: set TSGO_BIN to the pinned 7.1.0-beta lib/tsc \
             (or WXML_REQUIRE_REAL_ENGINE=1 to make this a hard gate)"
        );
        return;
    };
    let executable = Path::new(&bin);
    assert!(
        executable.is_file(),
        "TSGO_BIN does not point at a file: {bin}"
    );

    let sandbox = std::env::temp_dir().join(unique("definition-sandbox"));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _sandbox_guard = Cleanup(sandbox.clone());

    let project_root = format!("/tmp/opencode/{}", unique("definition-project"));
    assert!(
        !Path::new(&project_root).exists(),
        "the virtual project root must not exist on disk: {project_root}"
    );
    let fixture = input(
        &project_root,
        1,
        MAIN_V2,
        "/tmp/opencode/definition-unused-canary",
    );

    let mut session =
        NativeSession::start(SessionOptions::new(executable, sandbox)).expect("start real engine");
    session.open_project(&fixture).expect("open project");

    let raw = session
        .definition("main.ts", position(4, 2))
        .expect("definition");
    let result = map_definition(&raw);
    println!(
        "definition: targets={:?} groups={} failures={} truncated={} status={:?}",
        result
            .targets
            .iter()
            .map(|target| {
                (
                    target.path.as_str(),
                    target.range.start.get(),
                    target.range.end.get(),
                    target.utf16.start.get(),
                    target.utf16.end.get(),
                )
            })
            .collect::<Vec<_>>(),
        raw.groups,
        raw.decode_failures,
        raw.truncated,
        result.status
    );
    assert_eq!(
        result.status,
        wxml_types::CapabilityStatus::Supported,
        "every definition declaration must decode"
    );

    let target = result
        .targets
        .iter()
        .find(|target| target.path == "model.ts")
        .expect("the interface property declaration in model.ts");
    let start = position_to_offset(MODEL, position(2, 4))
        .expect("name start")
        .get();
    let end = position_to_offset(MODEL, position(2, 8))
        .expect("name end")
        .get();
    assert_eq!(target.utf16.start.get(), start);
    assert_eq!(target.utf16.end.get(), end);
    assert_eq!(target.range.start.get(), start);
    assert_eq!(target.range.end.get(), end);
    assert_eq!(
        MODEL.get(
            usize::try_from(target.range.start.get()).expect("start")
                ..usize::try_from(target.range.end.get()).expect("end")
        ),
        Some("name"),
        "the published range must be the identifier token, not the \
         declaration node pos that includes leading JSDoc trivia"
    );

    // Cross-file `User` has both the interface target and the import alias.
    // Both candidates must survive decoding and deduplication.
    let imported = session
        .definition("main.ts", position(0, 14))
        .expect("import definition");
    let imported_result = map_definition(&imported);
    println!(
        "import definition: targets={:?} groups={} failures={} status={:?}",
        imported_result
            .targets
            .iter()
            .map(|target| (target.path.as_str(), target.utf16.start.get()))
            .collect::<Vec<_>>(),
        imported.groups,
        imported.decode_failures,
        imported_result.status
    );
    assert_eq!(
        imported_result.status,
        wxml_types::CapabilityStatus::Supported,
        "the target and the import alias must both decode"
    );
    assert_eq!(imported.groups, 2);
    let mut actual: Vec<(String, u32, u32, u32, u32)> = imported_result
        .targets
        .iter()
        .map(|target| {
            (
                target.path.clone(),
                target.range.start.get(),
                target.range.end.get(),
                target.utf16.start.get(),
                target.utf16.end.get(),
            )
        })
        .collect();
    actual.sort();
    let import_start = position_to_offset(MAIN_V2, position(0, 14))
        .expect("import User start")
        .get();
    let import_end = position_to_offset(MAIN_V2, position(0, 18))
        .expect("import User end")
        .get();
    let interface_start = position_to_offset(MODEL, position(0, 17))
        .expect("interface User start")
        .get();
    let interface_end = position_to_offset(MODEL, position(0, 21))
        .expect("interface User end")
        .get();
    assert_eq!(
        actual,
        vec![
            (
                "main.ts".to_owned(),
                import_start,
                import_end,
                import_start,
                import_end,
            ),
            (
                "model.ts".to_owned(),
                interface_start,
                interface_end,
                interface_start,
                interface_end,
            ),
        ],
        "both the import alias and the interface declaration must have exact ranges"
    );
    assert_eq!(
        MAIN_V2.get(
            usize::try_from(import_start).expect("start")
                ..usize::try_from(import_end).expect("end")
        ),
        Some("User")
    );
    assert_eq!(
        MODEL.get(
            usize::try_from(interface_start).expect("start")
                ..usize::try_from(interface_end).expect("end")
        ),
        Some("User")
    );

    session.release().expect("release snapshot");
    session.shutdown().expect("shutdown");
}

/// Completion fixture with an unimported `User` from a second module.
const AUTO_IMPORT_MAIN: &str = "const value: Us\n";
const AUTO_IMPORT_MODEL: &str = "export interface User {\n    name: string;\n}\n";
/// Zero-based line/UTF-16 column directly after `Us`.
const AUTO_IMPORT_POSITION: Utf16Position = position(0, 15);

fn auto_import_input(root: &str, revision: u64) -> AnalysisInput {
    let files = vec![
        SourceFile::new("main.ts", AUTO_IMPORT_MAIN, DocumentRevision::new(1)).expect("main.ts"),
        SourceFile::new("model.ts", AUTO_IMPORT_MODEL, DocumentRevision::new(1)).expect("model.ts"),
    ];
    AnalysisInput::new(
        ProjectKey::new("real-engine-auto-import"),
        ProjectRoot::new(root).expect("project root"),
        SnapshotRevision::new(revision),
        files,
    )
    .expect("analysis input")
}

fn real_engine_bin() -> Option<std::path::PathBuf> {
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref() == Ok("1");
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1; \
             set it to the pinned 7.1.0-beta lib/tsc"
        );
        eprintln!(
            "real-engine auto-import gate skipped: set TSGO_BIN to the pinned 7.1.0-beta lib/tsc \
             (or WXML_REQUIRE_REAL_ENGINE=1 to make this a hard gate)"
        );
        return None;
    };
    let executable = std::path::PathBuf::from(bin);
    assert!(
        executable.is_file(),
        "TSGO_BIN does not point at a file: {}",
        executable.display()
    );
    Some(executable)
}

fn auto_import_cleanup(tag: &str) -> (std::path::PathBuf, Cleanup) {
    let sandbox = std::env::temp_dir().join(unique(tag));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let guard = Cleanup(sandbox.clone());
    (sandbox, guard)
}

#[test]
fn real_engine_completion_auto_import_needs_a_prepared_snapshot() {
    let Some(executable) = real_engine_bin() else {
        return;
    };
    let (sandbox, _sandbox_guard) = auto_import_cleanup("auto-import-sandbox");
    let project_root = format!("/tmp/opencode/{}", unique("auto-import-project"));

    let mut session =
        NativeSession::start(SessionOptions::new(executable, sandbox)).expect("start real engine");
    let first = auto_import_input(&project_root, 1);
    session
        .open_project_with_auto_imports(&first, "main.ts")
        .expect("open prepared project");
    assert!(session.is_prepared_for_auto_imports("main.ts"));
    assert_eq!(session.live_snapshot_count(), 1);

    let raw = session
        .completions_with_options(
            "main.ts",
            AUTO_IMPORT_POSITION,
            CompletionQueryOptions::SYMBOLS,
        )
        .expect("prepared symbol completion");
    let raw = raw.expect("prepared completion must return a list");
    let names: Vec<&str> = raw
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    eprintln!("prepared completion entries: {names:?}");
    let user = raw
        .entries
        .iter()
        .find(|entry| entry.name == "User")
        .expect("the prepared snapshot must offer the unimported User");
    eprintln!(
        "User entry: insertText={:?} filterText={:?} detail={:?} symbol={}",
        user.insert_text,
        user.filter_text,
        user.detail,
        user.symbol.is_some()
    );
    // API fact: `includeSymbol:true` is accepted on a prepared snapshot, but
    // the pinned engine does not attach a symbol to this auto-import candidate.
    // Symbol evidence is optional per entry and must never be treated as an
    // auto-import origin.
    let mapped = map_completions_with(Some(&raw), AutoImportContext::Prepared);
    assert!(
        mapped
            .gaps
            .contains(&CompletionGap::AutoImportOriginUnavailable),
        "a prepared list must report the origin gap: {:?}",
        mapped.gaps
    );
    assert!(!mapped.gaps.contains(&CompletionGap::AutoImportsUnavailable));
    let user_entry = mapped
        .entries
        .iter()
        .find(|entry| entry.label == "User")
        .expect("mapped User entry");
    if let Some(symbol) = &user_entry.symbol {
        let debug = format!("{symbol:?}");
        assert!(
            !debug.contains("/tmp/opencode") && !debug.contains("model.ts"),
            "symbol evidence must not publish engine paths: {debug}"
        );
    }

    // A second preparation for the same file/snapshot is a cache hit.
    let prepared_snapshot = session.epoch().expect("epoch").snapshot;
    assert_eq!(
        session.prepare_auto_imports("main.ts").expect("cache hit"),
        AutoImportPreparation::Prepared
    );
    assert_eq!(session.epoch().expect("epoch").snapshot, prepared_snapshot);
    assert_eq!(session.live_snapshot_count(), 1);

    // The preparation is carried into later snapshots without a new prepare.
    let second = auto_import_input(&project_root, 2);
    session.update_project(&second).expect("update project");
    assert!(session.is_prepared_for_auto_imports("main.ts"));
    assert_eq!(session.live_snapshot_count(), 1);
    let updated = session
        .completions_with_options(
            "main.ts",
            AUTO_IMPORT_POSITION,
            CompletionQueryOptions::SYMBOLS,
        )
        .expect("prepared completion after update")
        .expect("completion list after update");
    assert!(
        updated.entries.iter().any(|entry| entry.name == "User"),
        "the prepared index must survive update_project: {:?}",
        updated
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>()
    );

    session.release().expect("release snapshot");
    session.shutdown().expect("shutdown");
}

#[test]
fn real_engine_unprepared_symbol_completion_fails_closed() {
    let Some(executable) = real_engine_bin() else {
        return;
    };
    let (sandbox, _sandbox_guard) = auto_import_cleanup("auto-import-unprepared-sandbox");
    let project_root = format!("/tmp/opencode/{}", unique("auto-import-unprepared-project"));

    let mut session =
        NativeSession::start(SessionOptions::new(executable, sandbox)).expect("start real engine");
    let input = auto_import_input(&project_root, 1);
    session
        .open_project(&input)
        .expect("open unprepared project");
    assert!(!session.is_prepared_for_auto_imports("main.ts"));

    let result = session.completions_with_options(
        "main.ts",
        AUTO_IMPORT_POSITION,
        CompletionQueryOptions::SYMBOLS,
    );
    match result {
        Err(error) => eprintln!("unprepared includeSymbol error: {error}"),
        Ok(list) => panic!(
            "an unprepared includeSymbol:true query must fail closed, got {:?}",
            list.map(|list| {
                list.entries
                    .into_iter()
                    .map(|entry| entry.name)
                    .collect::<Vec<_>>()
            })
        ),
    }

    // Prepare-only rotation keeps the process and releases the old snapshot.
    let before = session.epoch().expect("epoch").snapshot;
    assert_eq!(
        session.prepare_auto_imports("main.ts").expect("prepare"),
        AutoImportPreparation::Prepared
    );
    let after = session.epoch().expect("epoch").snapshot;
    assert_ne!(before, after, "prepare must rotate the engine snapshot");
    assert_eq!(session.live_snapshot_count(), 1);

    // The prepare-only `updateSnapshot` keeps the same project/file set: the
    // auto-import candidate must now be reachable with symbols requested.
    let raw = session
        .completions_with_options(
            "main.ts",
            AUTO_IMPORT_POSITION,
            CompletionQueryOptions::SYMBOLS,
        )
        .expect("completion after on-demand prepare")
        .expect("completion list after on-demand prepare");
    assert!(
        raw.entries.iter().any(|entry| entry.name == "User"),
        "the on-demand prepare must expose the unimported User: {:?}",
        raw.entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>()
    );

    session.release().expect("release snapshot");
    session.shutdown().expect("shutdown");
}
