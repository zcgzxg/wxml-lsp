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
use wxml_backend_tsgo::native::session::{NativeSession, SessionOptions};
use wxml_backend_tsgo::native::strings::Utf16Position;
use wxml_backend_tsgo::query::completion::map_completions;
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
