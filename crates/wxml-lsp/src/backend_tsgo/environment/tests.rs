#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::items_after_statements
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Map, Value};
use wxml_backend_tsgo::config::PinnedEngine;
use wxml_backend_tsgo::input::{
    AnalysisInput, CompilerOptions, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision,
    SourceFile, TypeAssetError,
};
use wxml_backend_tsgo::native::session::SessionOptions;
use wxml_backend_tsgo::native::vfs::VirtualFileSystem;
use wxml_types::{
    Backend, BackendError, CapabilityStatus, CompletionKind, CompletionRequest, DefinitionRequest,
    DiagnosticsRequest, DocumentSymbolsRequest,
};

use super::fakes::*;
use super::fixtures::fake_assets;
use super::{Environment, EnvironmentLoadError, degradation_reason, load_environment};
use crate::backend_tsgo::engine::DiscoveredInstall;

use crate::backend_tsgo::input::ViewProvider;
use crate::backend_tsgo::runtime::virtual_cwd;
use crate::backend_tsgo::session::SessionFactory;
use crate::backend_tsgo::{NativeStartupError, TsgoBackendAdapter};
use crate::project_host::ProjectHost;

#[test]
fn fake_assets_match_their_pin_and_share_one_arc() {
    let assets = fake_assets("/typings");
    assert_eq!(assets.file_count(), 20);
    assert_eq!(assets.pin().total_file_count(), 20);
    assert_eq!(assets.pin().typings_version(), "5.3.1");
    assert!(assets.is_typings_only());
    assert_eq!(assets.pin().standard_lib_file_count(), 0);

    let views = TestViews::new(prepared_view(Map::new()));
    let (factory, _counts) = TestFactory::new(None);
    let adapter = TsgoBackendAdapter::new_with_environment(
        PinnedEngine::pinned(),
        Arc::clone(&views) as Arc<dyn ViewProvider>,
        Arc::new(factory),
        Environment::trusted(Arc::clone(&assets)),
    );
    let environment = Environment::trusted(Arc::clone(&assets));
    let plan = plan_with(&views, &adapter, &environment, 1);
    assert!(plan.identity.environment.is_some());
    let first = plan.input(SnapshotRevision::new(1)).expect("input");
    let second = plan.input(SnapshotRevision::new(2)).expect("input");
    assert!(first.has_type_environment());
    assert!(second.has_type_environment());
    assert!(std::ptr::eq(
        first.type_assets().expect("assets"),
        assets.as_ref()
    ));
    assert!(std::ptr::eq(
        second.type_assets().expect("assets"),
        assets.as_ref()
    ));
    assert_eq!(first.type_environment_identity(), plan.identity.environment);
}

#[test]
fn trusted_adapter_rejects_legacy_input_before_starting_a_session() {
    let views = TestViews::new(prepared_view(Map::new()));
    let assets = fake_assets("/typings");
    let (adapter, counts) = trusted_adapter(Arc::clone(&views), assets, None);
    let legacy = Environment::legacy();
    let plan = plan_with(&views, &adapter, &legacy, 1);
    assert!(plan.identity.environment.is_none());
    let error = adapter
        .with_session(&plan, |_session| Ok(()))
        .expect_err("legacy input must be rejected");
    assert!(matches!(error, BackendError::Unavailable(_)));
    assert_eq!(counts.starts.load(Ordering::SeqCst), 0);
    assert_eq!(counts.opens.load(Ordering::SeqCst), 0);
}

#[test]
fn trusted_adapter_rejects_legacy_input_with_an_open_session() {
    let views = TestViews::new(prepared_view(Map::new()));
    let assets = fake_assets("/typings");
    let (adapter, counts) = trusted_adapter(Arc::clone(&views), assets, None);
    adapter
        .hover(hover_request(&adapter, 1))
        .expect("trusted hover");
    let before = adapter.epoch();
    let legacy = Environment::legacy();
    let plan = plan_with(&views, &adapter, &legacy, 2);
    let error = adapter
        .with_session(&plan, |_session| Ok(()))
        .expect_err("legacy input must be rejected");
    assert!(matches!(error, BackendError::Unavailable(_)));
    assert_eq!(counts.starts.load(Ordering::SeqCst), 1);
    assert_eq!(counts.updates.load(Ordering::SeqCst), 0);
    assert_eq!(adapter.epoch(), before);
}

#[test]
fn with_install_reports_asset_failure_before_starting_a_session() {
    let package_root = unique_temp("absent-install");
    let install = DiscoveredInstall {
        package_root: package_root.clone(),
        typings_root: package_root.join("typings"),
    };
    let host: Arc<dyn ProjectHost> = Arc::new(crate::project_host::ProjectState::new());
    let error = TsgoBackendAdapter::with_install(host, &install)
        .expect_err("a missing typings tree must fail startup");
    assert!(matches!(
        error,
        NativeStartupError::Environment(EnvironmentLoadError::RootUnavailable)
    ));
    assert_path_free(&error.to_string());
    assert!(!package_root.exists());
}

#[test]
fn with_install_reports_a_missing_typings_root() {
    let dir = TempDir::new("missing-typings");
    let package_root = dir.path().join("package");
    fs::create_dir_all(&package_root).expect("package root");
    let install = DiscoveredInstall {
        package_root: package_root.clone(),
        typings_root: package_root.join("typings"),
    };
    let host: Arc<dyn ProjectHost> = Arc::new(crate::project_host::ProjectState::new());
    let error = TsgoBackendAdapter::with_install(host, &install)
        .expect_err("a missing typings root must fail startup");
    assert!(matches!(
        error,
        NativeStartupError::Environment(EnvironmentLoadError::RootUnavailable)
    ));
    assert_path_free(&error.to_string());
}

#[test]
fn load_environment_rejects_an_incomplete_tree_without_exposing_paths() {
    let dir = TempDir::new("incomplete");
    let package_root = dir.path().join("package");
    let typings_root = package_root.join("typings");
    fs::create_dir_all(typings_root.join("miniprogram-api-typings")).expect("typings root");
    let install = DiscoveredInstall {
        package_root,
        typings_root,
    };
    let error = load_environment(&install).expect_err("empty trees are incomplete");
    assert_eq!(error, EnvironmentLoadError::MissingRequired);
    assert_path_free(&error.to_string());
    assert_path_free(&format!("{error:?}"));
}

#[test]
#[rustfmt::skip]
fn every_asset_error_variant_maps_to_a_path_free_category() {
    let p = "/host/private/pkg/lib.d.ts";
    let cases = [
        (TypeAssetError::InvalidRoot { path: "/host/private/pkg".to_owned(), reason: "not absolute" }, EnvironmentLoadError::UnsafeTree { reason: "not absolute" }),
        (TypeAssetError::InvalidPath { path: p.to_owned(), reason: "outside fixed asset roots" }, EnvironmentLoadError::UnsafeTree { reason: "outside fixed asset roots" }),
        (TypeAssetError::UnsupportedFile { path: p.to_owned() }, EnvironmentLoadError::UnsafeTree { reason: "non-declaration asset path" }),
        (TypeAssetError::DuplicatePath { path: p.to_owned() }, EnvironmentLoadError::UnsafeTree { reason: "duplicate asset path" }),
        (TypeAssetError::RootsOverlap { first: "/host/a".to_owned(), second: "/host/b".to_owned() }, EnvironmentLoadError::UnsafeTree { reason: "overlapping asset roots" }),
        (TypeAssetError::TooManyFiles { count: 257, max: 256 }, EnvironmentLoadError::BudgetExceeded),
        (TypeAssetError::TooLarge { bytes: 1, max: 0 }, EnvironmentLoadError::BudgetExceeded),
        (TypeAssetError::InvalidDigest { path: p.to_owned() }, EnvironmentLoadError::AssetMismatch),
        (TypeAssetError::InvalidPinDigest { field: "typingsBundleDigest" }, EnvironmentLoadError::AssetMismatch),
        (TypeAssetError::PinMismatch { field: "typingsBytes", expected: "1".to_owned(), actual: "2".to_owned() }, EnvironmentLoadError::PinMismatch { field: "typingsBytes" }),
        (TypeAssetError::SizeMismatch { path: p.to_owned(), declared: 1, actual: 2 }, EnvironmentLoadError::AssetMismatch),
        (TypeAssetError::HashMismatch { path: p.to_owned() }, EnvironmentLoadError::AssetMismatch),
        (TypeAssetError::InvalidVersion { version: "not-a-version".to_owned() }, EnvironmentLoadError::PinMismatch { field: "typingsVersion" }),
        (TypeAssetError::MissingRequired { path: p.to_owned() }, EnvironmentLoadError::MissingRequired),
        (TypeAssetError::MissingRoot { root: "/host/private/pkg".to_owned() }, EnvironmentLoadError::RootUnavailable),
        (TypeAssetError::RootNotDirectory { root: "/host/private/pkg".to_owned() }, EnvironmentLoadError::RootUnavailable),
        (TypeAssetError::Symlink { path: p.to_owned() }, EnvironmentLoadError::UnsafeTree { reason: "symbolic link in the asset tree" }),
        (TypeAssetError::NonRegularFile { path: p.to_owned() }, EnvironmentLoadError::UnsafeTree { reason: "non-regular asset entry" }),
        (TypeAssetError::UnexpectedEntry { path: p.to_owned(), reason: "directory in a flat asset root" }, EnvironmentLoadError::UnsafeTree { reason: "directory in a flat asset root" }),
        (TypeAssetError::HardLink { path: p.to_owned() }, EnvironmentLoadError::UnsafeTree { reason: "hard-linked asset file" }),
        (TypeAssetError::UnsafeName { path: p.to_owned(), reason: "separator in name" }, EnvironmentLoadError::UnsafeTree { reason: "separator in name" }),
        (TypeAssetError::NonUtf8Path { path: p.to_owned() }, EnvironmentLoadError::UnsafeTree { reason: "non-UTF-8 asset path" }),
        (TypeAssetError::NonUtf8Content { path: p.to_owned() }, EnvironmentLoadError::AssetMismatch),
        (TypeAssetError::CaseCollision { first: "/host/a.d.ts".to_owned(), second: "/host/A.D.TS".to_owned() }, EnvironmentLoadError::UnsafeTree { reason: "case-insensitive asset path collision" }),
        (TypeAssetError::AssetIo { path: p.to_owned(), source: std::io::Error::other("/host/private/pkg/lib.d.ts") }, EnvironmentLoadError::Unreadable),
        (TypeAssetError::AssetChanged { path: p.to_owned() }, EnvironmentLoadError::ChangedWhileReading),
        (TypeAssetError::NestingTooDeep { path: p.to_owned() }, EnvironmentLoadError::UnsafeTree { reason: "asset directory nesting is too deep" }),
        (TypeAssetError::TooManyEntries { count: 1, max: 0 }, EnvironmentLoadError::BudgetExceeded),
    ];
    for (asset_error, expected) in cases {
        let mapped = super::map_asset_error(&asset_error);
        assert_eq!(mapped, expected);
        assert_path_free(&mapped.to_string());
        assert_path_free(&format!("{mapped:?}"));
    }
}

fn assert_path_free(text: &str) {
    assert!(!text.contains('/'), "path separator leaked: {text}");
    assert!(!text.contains('\\'), "path separator leaked: {text}");
}

#[test]
fn unsupported_compiler_option_is_unavailable_before_start() {
    let views = TestViews::new(prepared_view(Map::from_iter([(
        "notAnOption".to_owned(),
        Value::Bool(true),
    )])));
    let (adapter, counts) = trusted_adapter(views, fake_assets("/typings"), None);
    let error = adapter
        .hover(hover_request(&adapter, 1))
        .expect_err("unsupported options must not be dropped silently");
    assert!(matches!(error, BackendError::Unavailable(_)));
    assert_eq!(counts.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn source_only_change_updates_without_reopening() {
    let views = TestViews::new(prepared_view(Map::new()));
    let (adapter, counts) = trusted_adapter(views, fake_assets("/typings"), None);
    let before = adapter.epoch();
    adapter
        .hover(hover_request(&adapter, 1))
        .expect("first hover");
    adapter
        .hover(hover_request(&adapter, 2))
        .expect("second hover");
    assert_eq!(counts.starts.load(Ordering::SeqCst), 1);
    assert_eq!(counts.opens.load(Ordering::SeqCst), 1);
    assert_eq!(counts.updates.load(Ordering::SeqCst), 1);
    assert_eq!(counts.shutdowns.load(Ordering::SeqCst), 0);
    assert_eq!(adapter.epoch(), before);
}

#[test]
fn compiler_option_change_restarts_and_shuts_down_the_old_session() {
    let first = prepared_view(Map::from_iter([("strict".to_owned(), Value::Bool(true))]));
    let second = prepared_view(Map::from_iter([("strict".to_owned(), Value::Bool(false))]));
    let views = TestViews::new(first);
    let (adapter, counts) = trusted_adapter(Arc::clone(&views), fake_assets("/typings"), None);
    adapter
        .hover(hover_request(&adapter, 1))
        .expect("first hover");
    views.set(second);
    adapter
        .hover(hover_request(&adapter, 2))
        .expect("second hover");
    assert_eq!(counts.starts.load(Ordering::SeqCst), 2);
    assert_eq!(counts.opens.load(Ordering::SeqCst), 2);
    assert_eq!(counts.updates.load(Ordering::SeqCst), 0);
    assert_eq!(counts.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn identity_tracks_typings_roots_and_options() {
    let first_assets = fake_assets("/typings");
    let second_assets = fake_assets("/typings-other");
    let first = Environment::trusted(first_assets);
    let second = Environment::trusted(second_assets);
    let defaults = CompilerOptions::default();
    assert_eq!(first.identity(&defaults), first.identity(&defaults));
    assert_ne!(first.identity(&defaults), second.identity(&defaults));
    let identity = first.identity(&defaults).expect("trusted identity");
    assert!(identity.standard_lib_embedded());
    let strict = CompilerOptions::new(BTreeMap::from_iter([(
        "strict".to_owned(),
        Value::Bool(true),
    )]))
    .expect("strict");
    assert_ne!(first.identity(&defaults), first.identity(&strict));
    assert_eq!(Environment::legacy().identity(&defaults), None);
}

#[test]
fn environment_degradation_is_merged_into_typed_queries_only() {
    let views = TestViews::new(prepared_view(Map::new()));
    let (adapter, _counts) =
        trusted_adapter(views, fake_assets("/typings"), Some(ENV_REASON.to_owned()));
    // E7b: the very first request already sees the degradation produced
    // while the session's VFS was built.
    let hover = adapter.hover(hover_request(&adapter, 1)).expect("hover");
    assert_partial(hover.status, ENV_REASON);

    let diagnostics = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, 1),
        })
        .expect("diagnostics");
    assert_partial(diagnostics.status, ENV_REASON);

    let completion = adapter
        .completion(CompletionRequest {
            context: context(&adapter, 1),
            offset: offset_of(WXML, "name"),
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion");
    assert_partial(completion.status, ENV_REASON);

    let definition = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, 1),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_partial(definition.status, ENV_REASON);

    let symbols = adapter
        .document_symbols(DocumentSymbolsRequest {
            context: context(&adapter, 1),
            subject: script_file(),
        })
        .expect("document symbols");
    assert_eq!(symbols.status, CapabilityStatus::Supported);
    assert!(
        !symbols
            .status
            .reason()
            .is_some_and(|reason| reason.contains("type environment is partial"))
    );
}

#[test]
fn first_request_reports_plan_environment_degradation_before_the_session_opens() {
    let options = Map::from_iter([
        (
            "types".to_owned(),
            Value::Array(vec![Value::String("node".to_owned())]),
        ),
        (
            "typeRoots".to_owned(),
            Value::Array(vec![Value::String("/host/types".to_owned())]),
        ),
        ("baseUrl".to_owned(), Value::String("/host/base".to_owned())),
        (
            "paths".to_owned(),
            Value::Object(Map::from_iter([(
                "@/*".to_owned(),
                Value::Array(vec![Value::String("/host/src/*".to_owned())]),
            )])),
        ),
    ]);
    let views = TestViews::new(prepared_view(options));
    // The fake session reports no degradation: the reason must come from the
    // request plan, not from the opened VFS.
    let (adapter, counts) = trusted_adapter(views, fake_assets("/typings"), None);
    let hover = adapter.hover(hover_request(&adapter, 1)).expect("hover");
    let CapabilityStatus::Partial { reason } = hover.status else {
        panic!("expected a partial first hover: {:?}", hover.status);
    };
    for field in ["types", "typeRoots", "baseUrl", "paths"] {
        assert!(reason.contains(&format!("{field} (")), "{field}: {reason}");
    }
    assert!(
        !reason.contains('/'),
        "plan-time environment reason leaked a path: {reason}"
    );
    assert_eq!(counts.starts.load(Ordering::SeqCst), 1);
    assert_eq!(counts.opens.load(Ordering::SeqCst), 1);

    let symbols = adapter
        .document_symbols(DocumentSymbolsRequest {
            context: context(&adapter, 1),
            subject: script_file(),
        })
        .expect("document symbols");
    assert!(
        !symbols
            .status
            .reason()
            .is_some_and(|reason| reason.contains("type environment")),
        "pure-syntax document symbols must not inherit the type environment: {:?}",
        symbols.status
    );
}

fn assert_partial(status: CapabilityStatus, needle: &str) {
    match status {
        CapabilityStatus::Partial { reason } => {
            assert!(reason.contains(needle), "missing {needle}: {reason}");
        }
        other => panic!("expected Partial with {needle}, got {other:?}"),
    }
}

#[test]
fn legacy_constructors_keep_the_legacy_environment() {
    let views = TestViews::new(prepared_view(Map::new()));
    let (factory, counts) = TestFactory::new(None);
    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        Arc::clone(&views) as Arc<dyn ViewProvider>,
        Arc::new(factory),
    );
    assert!(!adapter.environment.is_trusted());
    let legacy = Environment::legacy();
    let plan = plan_with(&views, &adapter, &legacy, 1);
    assert!(plan.identity.environment.is_none());
    assert!(
        !plan
            .input(SnapshotRevision::new(1))
            .expect("input")
            .has_type_environment()
    );

    let host: Arc<dyn ProjectHost> = Arc::new(crate::project_host::ProjectState::new());
    let with_host =
        TsgoBackendAdapter::with_host(host, SessionOptions::new(PathBuf::new(), virtual_cwd()));
    assert!(!with_host.environment.is_trusted());
    assert_eq!(counts.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn out_of_vfs_options_produce_a_path_free_environment_reason() {
    let assets = fake_assets("/typings");
    let options = CompilerOptions::new(BTreeMap::from_iter([
        (
            "typeRoots".to_owned(),
            Value::Array(vec![Value::String("/host/types".to_owned())]),
        ),
        ("baseUrl".to_owned(), Value::String("/host/base".to_owned())),
        (
            "paths".to_owned(),
            Value::Object(Map::from_iter([(
                "@/*".to_owned(),
                Value::Array(vec![Value::String("/host/src/*".to_owned())]),
            )])),
        ),
    ]))
    .expect("compiler options");
    let input = AnalysisInput::with_environment(
        ProjectKey::new("demo"),
        ProjectRoot::new("/work/demo").expect("root"),
        SnapshotRevision::new(1),
        vec![SourceFile::new(SCRIPT_PATH, SCRIPT, DocumentRevision::new(1)).expect("source file")],
        options,
        assets,
    )
    .expect("environment input");
    let vfs = VirtualFileSystem::from_analysis_input(&input).expect("vfs");
    let reason = degradation_reason(vfs.environment_degradations()).expect("non-empty reason");
    assert!(reason.contains("typeRoots"), "{reason}");
    assert!(reason.contains("baseUrl"), "{reason}");
    assert!(reason.contains("paths"), "{reason}");
    assert_path_free(&reason);
    assert_eq!(degradation_reason(&[]), None);
}

#[test]
fn test_session_reports_environment_reason_only_after_open() {
    let (factory, _counts) = TestFactory::new(Some(ENV_REASON.to_owned()));
    let mut session = factory.start().expect("session");
    assert_eq!(session.environment_degradation(), None);
    let input = AnalysisInput::new(
        ProjectKey::new("demo"),
        ProjectRoot::new("/work/demo").expect("root"),
        SnapshotRevision::new(1),
        vec![SourceFile::new(SCRIPT_PATH, SCRIPT, DocumentRevision::new(1)).expect("source file")],
    )
    .expect("legacy input");
    session.open_project(&input).expect("open");
    assert_eq!(
        session.environment_degradation(),
        Some(ENV_REASON.to_owned())
    );
}
