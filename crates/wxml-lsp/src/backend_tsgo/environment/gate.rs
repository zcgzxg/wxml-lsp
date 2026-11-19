//! Real-install gate for the production trusted type environment.
//!
//! The gate is opt-in. `TSGO_BIN` must point at the pinned engine `lib/tsc`;
//! `TSGO_ASSET_ROOT` supplies the package root used for sandbox exclusion and
//! the standard library. `TSGO_TYPINGS_ROOT` must point at the
//! `miniprogram-api-typings` package directory; this gate also accepts the
//! containing directory and normalizes it. The `native_parity` fixture harness
//! copies the supplied directory as the package itself, so it requires the
//! package directory (`.../node_modules/miniprogram-api-typings`).
//!
//! `WXML_REQUIRE_REAL_ENGINE=1|true` turns a missing or unusable input into a
//! hard failure. Without it the gate skips and prints one line.
//!
//! # What this gate covers
//!
//! - the production `TypeAssets::load_pinned` pin through `load_environment`;
//! - `TsgoBackendAdapter::with_install` injecting `Environment::Trusted`, plus
//!   a real session start/stop through the managed sandbox factory;
//! - a real `with_install` adapter over a real on-disk project hovering an
//!   in-project symbol (`Known`/`string`/exact range), plus the same hover
//!   through `new` (legacy, no assets) and a trusted `new_with_environment`
//!   adapter for comparison;
//! - a real `NativeSession` opened from `AnalysisInput::with_environment` with
//!   the verified assets, and a `wx.request` hover that must resolve to the
//!   mini-program typings signature.
//!
//! No fake session participates. The `wx` global is queried through the real
//! session because WXML shadow fragments bind free roots to `this.data` and
//! therefore cannot address a script global; the in-project symbol hover above
//! exercises the adapter query path with assets.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Map;
use wxml_backend_tsgo::config::PinnedEngine;
use wxml_backend_tsgo::input::{
    AnalysisInput, CompilerOptions, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision,
    SourceFile, TypeAssets,
};
use wxml_backend_tsgo::native::session::{NativeSession, SessionOptions};
use wxml_backend_tsgo::native::strings::Utf16Position;
use wxml_backend_tsgo::query::type_info::TypeQuality as NativeTypeQuality;
use wxml_project::ProjectPath;
use wxml_types::{
    Backend, CapabilityStatus, HoverRequest, RequestContext, SnapshotId, SourceRevision,
    SourceSnapshot, TypeQuality, ViewRevision,
};

use super::fakes::{
    SCRIPT, SCRIPT_PATH, TestViews, WXML, context, offset_of, prepared_file,
    prepared_view_with_files,
};
use super::{Environment, load_environment};
use crate::backend_tsgo::TsgoBackendAdapter;
use crate::backend_tsgo::engine::DiscoveredInstall;
use crate::backend_tsgo::input::ViewProvider;
use crate::backend_tsgo::sandbox::SandboxSessionFactory;
use crate::backend_tsgo::session::NativeSessionFactory;
use crate::project_host::{PreparedView, ProjectHost, ProjectState};

fn required_engine() -> bool {
    matches!(
        std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
        Ok("1" | "true")
    )
}

fn skip_or_fail(message: &str, required: bool) {
    assert!(!required, "{message}");
    eprintln!("optional real trusted-asset test skipped: {message}");
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Returns `<typings>`, the directory that contains the typings package.
fn real_typings_root() -> Option<PathBuf> {
    let root = env_path("TSGO_TYPINGS_ROOT")
        .or_else(|| env_path("TSGO_ASSET_ROOT").map(|asset| asset.join("typings")))?;
    if root.file_name().and_then(|name| name.to_str()) == Some("miniprogram-api-typings") {
        return root
            .parent()
            .map(Path::to_path_buf)
            .filter(|parent| parent.join("miniprogram-api-typings").is_dir());
    }
    root.join("miniprogram-api-typings")
        .is_dir()
        .then_some(root)
}

/// Resolves the gate inputs without fail-open shortcuts.
///
/// Every rejection path goes through [`skip_or_fail`]: it skips in optional
/// mode and panics with the same message when the gate is required. `required`
/// is passed explicitly so the regression tests do not mutate process
/// environment variables.
fn load_real_install(
    bin: Option<PathBuf>,
    typings_root: Option<PathBuf>,
    asset_root: Option<PathBuf>,
    required: bool,
) -> Option<DiscoveredInstall> {
    let Some(bin) = bin else {
        skip_or_fail(
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1",
            required,
        );
        return None;
    };
    let Some(engine) = fs::canonicalize(&bin).ok() else {
        skip_or_fail("TSGO_BIN does not exist or cannot be resolved", required);
        return None;
    };
    let Some(standard_lib_root) = engine.parent().map(Path::to_path_buf) else {
        skip_or_fail("TSGO_BIN has no parent directory", required);
        return None;
    };
    if !standard_lib_root.join("lib.d.ts").is_file() {
        skip_or_fail(
            "TSGO_BIN must live in the engine lib directory with lib.d.ts",
            required,
        );
        return None;
    }
    let Some(typings_root) = typings_root else {
        skip_or_fail(
            "TSGO_TYPINGS_ROOT (or TSGO_ASSET_ROOT/typings) must provide miniprogram-api-typings",
            required,
        );
        return None;
    };
    let package_root = asset_root
        .and_then(|path| fs::canonicalize(path).ok())
        .or_else(|| standard_lib_root.parent().map(Path::to_path_buf));
    let Some(package_root) = package_root else {
        skip_or_fail("TSGO_ASSET_ROOT is not a resolvable package root", required);
        return None;
    };
    Some(DiscoveredInstall {
        package_root,
        engine,
        standard_lib_root,
        typings_root,
    })
}

fn real_install() -> Option<DiscoveredInstall> {
    load_real_install(
        env_path("TSGO_BIN"),
        real_typings_root(),
        env_path("TSGO_ASSET_ROOT"),
        required_engine(),
    )
}

/// Temporary directory used only by the gate and its regression tests.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "wxml-lsp-gate-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("gate temp dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_pinned_assets(assets: &TypeAssets) {
    assert_eq!(assets.file_count(), 136);
    assert_eq!(assets.total_bytes(), 6_392_948);
    assert_eq!(assets.pin().standard_lib_file_count(), 116);
    assert_eq!(assets.pin().typings_file_count(), 20);
    assert_eq!(
        assets.pin().standard_lib_bundle_digest(),
        "5e126e7206065b6fff9d651eff3ad66b9413804d3e23d7ecfb4257299af3e85e"
    );
    assert_eq!(
        assets.pin().typings_bundle_digest(),
        "821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9"
    );
}

/// Opens a real session on a virtual project with the verified assets.
fn open_trusted_session(
    install: &DiscoveredInstall,
    assets: &Arc<TypeAssets>,
    sandbox: &Path,
) -> NativeSession {
    let script = "const req = wx.request;\nPage({ data: {} });\n";
    let input = AnalysisInput::with_environment(
        ProjectKey::new("gate"),
        ProjectRoot::new("/tmp/opencode/wxml-tsgo-gate-project").expect("project root"),
        SnapshotRevision::new(1),
        vec![SourceFile::new("main.ts", script, DocumentRevision::new(1)).expect("source")],
        CompilerOptions::default(),
        Arc::clone(assets),
    )
    .expect("environment input");
    let mut session = NativeSession::start(SessionOptions::new(
        install.engine.clone(),
        sandbox.to_path_buf(),
    ))
    .expect("real native session");
    session.open_project(&input).expect("open trusted project");
    session
}

/// Hovers `wx.request` in the real session and checks the typings signature.
fn assert_native_wx_hover(install: &DiscoveredInstall, assets: &Arc<TypeAssets>) {
    let sandbox = TempDir::new("native-session");
    let mut session = open_trusted_session(install, assets, &sandbox.0);
    let evidence = session
        .hover_evidence(
            "main.ts",
            Utf16Position {
                line: 0,
                character: 16,
            },
        )
        .expect("real wx.request hover evidence");
    let type_text = evidence
        .type_info
        .as_ref()
        .map(|info| info.text.as_str())
        .unwrap_or_default();
    let symbol = evidence
        .symbol
        .as_ref()
        .map(|symbol| symbol.name.as_str())
        .unwrap_or_default();
    assert_eq!(evidence.type_quality, NativeTypeQuality::Known);
    assert_eq!(symbol, "request");
    assert!(
        type_text.contains("RequestTask") && type_text.contains("RequestOption"),
        "wx.request must resolve to the pinned typings signature, got {type_text:?}"
    );
    assert!(
        !matches!(evidence.status, CapabilityStatus::Unsupported { .. }),
        "wx.request must resolve: {:?}",
        evidence.status
    );
    session.shutdown().expect("clean shutdown");
}

/// Host view that types `this.data` for both legacy and trusted sessions.
///
/// The explicit `Page` declaration keeps the legacy no-asset session at
/// `Known`/`string`; with assets the mini-program typings declare the same
/// global and the shadow fragment resolves identically.
fn project_symbol_view() -> PreparedView {
    let globals = "declare function Page(options: {\n  data: { user: { name: string } };\n  __wxml_check_0(this: { data: { user: { name: string } } }): void;\n}): void;\n";
    prepared_view_with_files(
        Map::new(),
        vec![
            prepared_file(SCRIPT_PATH, SCRIPT),
            prepared_file("globals.d.ts", globals),
        ],
    )
}

fn assert_adapter_project_symbol_hover(adapter: &TsgoBackendAdapter) {
    let result = adapter
        .hover(HoverRequest {
            context: context(adapter, 1),
            offset: offset_of(WXML, "name"),
        })
        .expect("real adapter hover");
    assert_eq!(result.quality, TypeQuality::Known);
    assert!(result.contents.contains("string"), "{:?}", result.contents);
    let range = result.range.expect("hover range");
    assert_eq!(range.start(), offset_of(WXML, "name"));
    assert_eq!(
        range.end(),
        offset_of(WXML, "name") + u32::try_from("name".len()).unwrap()
    );
}

#[test]
fn optional_real_adapter_hover_matches_legacy_without_assets() {
    let Some(install) = real_install() else {
        return;
    };
    let environment = load_environment(&install).expect("pinned real assets");
    let assets = environment.assets().expect("trusted assets");

    let legacy_views = TestViews::new(project_symbol_view());
    let legacy_sandbox = TempDir::new("legacy-adapter");
    let legacy = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        Arc::clone(&legacy_views) as Arc<dyn ViewProvider>,
        Arc::new(NativeSessionFactory::new(SessionOptions::new(
            install.engine.clone(),
            legacy_sandbox.0.clone(),
        ))),
    );
    assert_adapter_project_symbol_hover(&legacy);

    let views = TestViews::new(project_symbol_view());
    let sessions = SandboxSessionFactory::new(install.engine.clone(), &install.package_root)
        .expect("real sandbox factory");
    let trusted = TsgoBackendAdapter::new_with_environment(
        PinnedEngine::pinned(),
        Arc::clone(&views) as Arc<dyn ViewProvider>,
        Arc::new(sessions),
        Environment::trusted(Arc::clone(assets)),
    );
    assert_adapter_project_symbol_hover(&trusted);
}

/// Creates a real on-disk mini-program project for the host.
fn real_project() -> (TempDir, Arc<ProjectState>) {
    let dir = TempDir::new("real-project");
    fs::create_dir_all(dir.0.join("pages/index")).expect("project dirs");
    fs::write(dir.0.join("app.json"), r#"{"pages":["pages/index/index"]}"#).expect("app.json");
    fs::write(dir.0.join("pages/index/index.wxml"), WXML).expect("wxml");
    fs::write(dir.0.join("pages/index/index.ts"), SCRIPT).expect("script");
    let host = Arc::new(ProjectState::new());
    let uri = format!("file://{}", dir.0.display());
    host.open_root(Some(&uri)).expect("open real project");
    (dir, host)
}

#[test]
fn optional_real_with_install_adapter_hovers_a_project_symbol() {
    let Some(install) = real_install() else {
        return;
    };
    let (_dir, host) = real_project();
    assert!(host.is_project_open(), "real project must be loaded");
    let adapter =
        TsgoBackendAdapter::with_install(Arc::clone(&host) as Arc<dyn ProjectHost>, &install)
            .expect("with_install loads assets and builds the managed sandbox");
    assert!(adapter.environment.is_trusted());

    let path = ProjectPath::new("pages/index/index.wxml").expect("wxml path");
    let file = host.file_id(&path).expect("real file id");
    let context = RequestContext::new(
        host.project_revision(),
        ViewRevision::new(1),
        adapter.epoch(),
        Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(1),
            WXML,
        )),
    )
    .expect("context");
    let result = adapter
        .hover(HoverRequest {
            context,
            offset: offset_of(WXML, "name"),
        })
        .expect("real with_install adapter hover");
    assert_eq!(result.quality, TypeQuality::Known);
    assert!(result.contents.contains("string"), "{:?}", result.contents);
    let range = result.range.expect("hover range");
    assert_eq!(range.start(), offset_of(WXML, "name"));
    assert_eq!(
        range.end(),
        offset_of(WXML, "name") + u32::try_from("name".len()).unwrap()
    );
}

#[test]
fn optional_real_install_loads_assets_and_resolves_wx_in_a_native_session() {
    let Some(install) = real_install() else {
        return;
    };
    let environment = load_environment(&install).expect("pinned real assets");
    let assets = environment.assets().expect("trusted assets");
    assert_pinned_assets(assets);

    let host: Arc<dyn ProjectHost> = Arc::new(ProjectState::new());
    let managed = TsgoBackendAdapter::with_install(host, &install)
        .expect("production constructor loads assets before the sandbox");
    assert!(
        managed.environment.is_trusted(),
        "with_install must inject the trusted type environment"
    );
    managed
        .start_and_stop_session_for_test()
        .expect("real session start and shutdown");

    assert_native_wx_hover(&install, assets);
}

#[test]
fn optional_gate_skips_a_missing_binary() {
    assert!(
        load_real_install(
            Some(PathBuf::from("/nonexistent/wxml-tsgo-gate/tsc")),
            Some(PathBuf::from("/tmp")),
            None,
            false,
        )
        .is_none()
    );
}

#[test]
#[should_panic(expected = "TSGO_BIN does not exist")]
fn required_gate_fails_for_a_missing_binary() {
    let _ = load_real_install(
        Some(PathBuf::from("/nonexistent/wxml-tsgo-gate/tsc")),
        Some(PathBuf::from("/tmp")),
        None,
        true,
    );
}

#[test]
#[should_panic(expected = "no parent directory")]
fn required_gate_fails_for_a_binary_without_a_parent() {
    let _ = load_real_install(
        Some(PathBuf::from("/")),
        Some(PathBuf::from("/tmp")),
        None,
        true,
    );
}

#[test]
#[should_panic(expected = "lib.d.ts")]
fn required_gate_fails_without_the_standard_library() {
    let dir = TempDir::new("no-lib");
    let bin = dir.0.join("tsc");
    fs::write(&bin, b"engine").expect("bin");
    let _ = load_real_install(Some(bin), Some(PathBuf::from("/tmp")), None, true);
}

#[test]
#[should_panic(expected = "TSGO_TYPINGS_ROOT")]
fn required_gate_fails_without_typings() {
    let dir = TempDir::new("no-typings");
    let bin = dir.0.join("tsc");
    fs::write(&bin, b"engine").expect("bin");
    fs::write(dir.0.join("lib.d.ts"), b"// lib\n").expect("lib");
    let _ = load_real_install(Some(bin), None, None, true);
}
