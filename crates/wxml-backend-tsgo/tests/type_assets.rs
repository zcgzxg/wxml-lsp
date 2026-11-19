//! P6-3 native type-environment slice tests.
//!
//! The fake tests do not need an engine. The real-engine tests are opt-in
//! through `TSGO_BIN`; `WXML_REQUIRE_REAL_ENGINE=1` turns a missing engine or a
//! missing asset root into a hard failure instead of a skip.
//!
//! The type assets are read from the pinned package layout:
//! `<package>/engine/lib/lib*.d.ts` (or `<package>/lib` for the extracted
//! upstream tarball) plus `<package>/typings/miniprogram-api-typings/**`. When
//! `TSGO_ASSET_ROOT` is unset the standard library is derived from
//! `TSGO_BIN`'s directory and the typings fall back to the repository copy at
//! `backends/typescript/node_modules/miniprogram-api-typings`. That fallback is
//! explicitly a repository fixture, not a packaged-chain claim (P6-3-4 blocked).

#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use wxml_backend_tsgo::config::MAX_REQUEST_BODY_BYTES;
use wxml_backend_tsgo::input::{
    AnalysisInput, AssetPath, COMPILER_OPTION_KEYS, CompilerOptions, DocumentRevision,
    MAX_ASSET_COMPONENT_BYTES, MAX_ASSET_PATH_BYTES, MAX_ASSET_PATH_COMPONENTS,
    MAX_TYPE_ASSET_BYTES, MAX_TYPE_ASSET_FILES, MINIPROGRAM_TYPINGS, PINNED_STANDARD_LIB_BYTES,
    PINNED_STANDARD_LIB_FILE_COUNT, PINNED_TYPE_ASSET_BYTES, PINNED_TYPE_ASSET_FILES,
    PINNED_TYPINGS_BYTES, PINNED_TYPINGS_FILE_COUNT, PINNED_TYPINGS_VERSION, ProjectKey,
    ProjectRoot, SnapshotRevision, SourceFile, TypeAssetError, TypeAssetFile, TypeAssets,
    TypeAssetsPin,
};
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::session::{NativeSession, SessionOptions};
use wxml_backend_tsgo::native::strings::Utf16Position;
use wxml_backend_tsgo::native::vfs::{VfsError, VirtualFileSystem};
use wxml_backend_tsgo::native::wire::{
    SnapshotChanges, SnapshotFileSystem, update_snapshot_params,
};

const MAIN: &str = r"const info = wx.getSystemInfoSync();
const bad = info.notAField;
const badType: number = info.model;
const page = Page({ data: { count: 1 } });
const component = Component({ data: { n: 1 } });
const app = getApp();
wx.
";

const fn position(line: u32, character: u32) -> Utf16Position {
    Utf16Position { line, character }
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

fn sha(text: &str) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(text.as_bytes()) {
        encoded.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn asset(path: &str, text: &str) -> TypeAssetFile {
    TypeAssetFile::new(
        AssetPath::new(path).expect("asset path"),
        text,
        sha(text),
        u64::try_from(text.len()).expect("size"),
    )
    .expect("asset")
}

fn bundle_digest(root: &str, files: &mut Vec<&TypeAssetFile>) -> String {
    files.sort_by(|left, right| left.path().cmp(right.path()));
    let mut hasher = Sha256::new();
    for file in files {
        let path = file.path().as_str();
        let relative = path
            .strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(path);
        hasher.update(file.sha256().as_bytes());
        hasher.update(b"  ");
        hasher.update(relative.as_bytes());
        hasher.update(b"\n");
    }
    let mut encoded = String::with_capacity(64);
    for byte in hasher.finalize() {
        encoded.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn pin_for(standard_lib_root: &str, typings_root: &str, files: &[TypeAssetFile]) -> TypeAssetsPin {
    let typings_package = format!("{typings_root}/{MINIPROGRAM_TYPINGS}");
    let mut standard_lib: Vec<&TypeAssetFile> = files
        .iter()
        .filter(|file| {
            file.path().as_str() == standard_lib_root
                || file
                    .path()
                    .as_str()
                    .strip_prefix(standard_lib_root)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .collect();
    let mut typings: Vec<&TypeAssetFile> = files
        .iter()
        .filter(|file| {
            file.path()
                .as_str()
                .strip_prefix(&typings_package)
                .is_some_and(|rest| rest.starts_with('/'))
        })
        .collect();
    TypeAssetsPin::new(
        PINNED_TYPINGS_VERSION,
        PINNED_STANDARD_LIB_FILE_COUNT,
        standard_lib.iter().map(|file| file.size()).sum(),
        bundle_digest(standard_lib_root, &mut standard_lib),
        PINNED_TYPINGS_FILE_COUNT,
        typings.iter().map(|file| file.size()).sum(),
        bundle_digest(&typings_package, &mut typings),
    )
    .expect("matching pin")
}

fn fixture_files_at(
    standard_lib_root: &str,
    typings_root: &str,
    tag: &str,
    stdlib_size: usize,
    typings_size: usize,
) -> (String, String, Vec<TypeAssetFile>) {
    let standard_lib_root = standard_lib_root.to_owned();
    let typings_root = typings_root.to_owned();
    let typings_package = format!("{typings_root}/{MINIPROGRAM_TYPINGS}");
    let mut files = Vec::new();
    for index in 0..PINNED_STANDARD_LIB_FILE_COUNT {
        let name = match index {
            0 => "lib.d.ts".to_owned(),
            1 => "lib.es5.d.ts".to_owned(),
            _ => format!("lib.extra{index}.d.ts"),
        };
        let padding = stdlib_size.saturating_sub(32);
        files.push(asset(
            &format!("{standard_lib_root}/{name}"),
            &format!(
                "declare const stdlib{index}: number;\n{}",
                "x".repeat(padding)
            ),
        ));
    }
    for index in 0..PINNED_TYPINGS_FILE_COUNT {
        let name = match index {
            0 => "index.d.ts".to_owned(),
            1 => "types/index.d.ts".to_owned(),
            2 => "types/wx/index.d.ts".to_owned(),
            _ => format!("types/extra{index}.d.ts"),
        };
        let padding = typings_size.saturating_sub(32);
        files.push(asset(
            &format!("{typings_package}/{name}"),
            &format!(
                "declare const typings{index}: number;\n{}",
                "x".repeat(padding)
            ),
        ));
    }
    let _ = tag;
    (standard_lib_root, typings_root, files)
}

fn fake_assets_with_sizes(tag: &str, stdlib_size: usize, typings_size: usize) -> TypeAssets {
    let (standard_lib_root, typings_root, files) = fixture_files_at(
        "/pkg/engine/lib",
        "/pkg/typings",
        tag,
        stdlib_size,
        typings_size,
    );
    let pin = pin_for(&standard_lib_root, &typings_root, &files);
    TypeAssets::new(
        AssetPath::new(standard_lib_root).expect("stdlib root"),
        AssetPath::new(typings_root).expect("typings root"),
        files,
        pin,
    )
    .expect("fixture assets")
}

fn fake_assets() -> TypeAssets {
    fake_assets_with_sizes("fake", 64, 64)
}

fn environment_input(options: BTreeMap<String, serde_json::Value>) -> AnalysisInput {
    environment_input_with(fake_assets(), options, 1)
}

fn environment_input_with(
    assets: TypeAssets,
    options: BTreeMap<String, serde_json::Value>,
    revision: u64,
) -> AnalysisInput {
    AnalysisInput::with_environment(
        ProjectKey::new("type-assets"),
        ProjectRoot::new("/work/type-assets").expect("root"),
        SnapshotRevision::new(revision),
        vec![
            SourceFile::new("main.ts", "const x = 1;\n", DocumentRevision::new(1))
                .expect("main.ts"),
        ],
        CompilerOptions::new(options).expect("compiler options"),
        std::sync::Arc::new(assets),
    )
    .expect("environment input")
}

#[test]
fn compiler_options_accept_only_the_frozen_allowlist() {
    assert_eq!(COMPILER_OPTION_KEYS.len(), 33);
    let mut options = BTreeMap::new();
    for key in COMPILER_OPTION_KEYS {
        let value = match key {
            "lib" | "types" | "typeRoots" => serde_json::json!(["x"]),
            "paths" => serde_json::json!({"@/*": ["src/*"]}),
            "target" | "module" | "moduleResolution" | "jsx" | "moduleDetection" | "baseUrl"
            | "rootDir" | "outDir" => serde_json::json!("es2020"),
            _ => serde_json::json!(true),
        };
        options.insert(key.to_owned(), value);
    }
    let parsed = CompilerOptions::new(options).expect("all 33 keys are accepted");
    assert_eq!(parsed.digest().len(), 64);

    let unknown = CompilerOptions::new(BTreeMap::from([(
        "moduleSuffixes".to_owned(),
        serde_json::json!(["x"]),
    )]));
    assert!(matches!(
        unknown,
        Err(wxml_backend_tsgo::input::InputError::UnsupportedCompilerOption { .. })
    ));
}

#[test]
fn type_assets_reject_unsafe_duplicate_missing_and_pin_mismatch() {
    assert!(AssetPath::new("/pkg/../escape.d.ts").is_err());
    assert!(AssetPath::new("relative/lib.d.ts").is_err());
    assert!(AssetPath::new("/pkg\\lib.d.ts").is_err());
    assert!(AssetPath::new(format!("/pkg/{}", "a".repeat(MAX_ASSET_PATH_BYTES))).is_err());
    assert!(
        AssetPath::new(format!(
            "/{}",
            vec!["a"; MAX_ASSET_PATH_COMPONENTS + 1].join("/")
        ))
        .is_err()
    );
    assert!(
        AssetPath::new(format!(
            "/pkg/{}",
            "a".repeat(MAX_ASSET_COMPONENT_BYTES + 1)
        ))
        .is_err()
    );

    let (standard_lib_root, typings_root, files) =
        fixture_files_at("/pkg/engine/lib", "/pkg/typings", "invalid", 64, 64);
    let pin = pin_for(&standard_lib_root, &typings_root, &files);

    let outside = {
        let mut changed = files.clone();
        changed.push(asset("/pkg/engine/lib-evil/extra.d.ts", "export {};\n"));
        TypeAssets::new(
            AssetPath::new(&standard_lib_root).expect("root"),
            AssetPath::new(&typings_root).expect("root"),
            changed,
            pin.clone(),
        )
    };
    assert!(matches!(outside, Err(TypeAssetError::InvalidPath { .. })));

    let duplicate = {
        let mut changed = files.clone();
        changed.push(files[0].clone());
        TypeAssets::new(
            AssetPath::new(&standard_lib_root).expect("root"),
            AssetPath::new(&typings_root).expect("root"),
            changed,
            pin.clone(),
        )
    };
    assert!(matches!(
        duplicate,
        Err(TypeAssetError::DuplicatePath { .. })
    ));

    let missing_required = TypeAssets::new(
        AssetPath::new(&standard_lib_root).expect("root"),
        AssetPath::new(&typings_root).expect("root"),
        files
            .iter()
            .filter(|file| !file.path().as_str().ends_with("/types/index.d.ts"))
            .cloned()
            .collect(),
        pin.clone(),
    );
    assert!(matches!(
        missing_required,
        Err(TypeAssetError::MissingRequired { .. })
    ));

    let missing_transitive_lib = TypeAssets::new(
        AssetPath::new(&standard_lib_root).expect("root"),
        AssetPath::new(&typings_root).expect("root"),
        files
            .iter()
            .filter(|file| !file.path().as_str().ends_with("/lib.es5.d.ts"))
            .cloned()
            .collect(),
        pin.clone(),
    );
    assert!(matches!(
        missing_transitive_lib,
        Err(TypeAssetError::PinMismatch {
            field: "standardLibFileCount",
            ..
        })
    ));

    let missing_transitive_typings = TypeAssets::new(
        AssetPath::new(&standard_lib_root).expect("root"),
        AssetPath::new(&typings_root).expect("root"),
        files
            .iter()
            .filter(|file| !file.path().as_str().ends_with("/types/wx/index.d.ts"))
            .cloned()
            .collect(),
        pin,
    );
    assert!(matches!(
        missing_transitive_typings,
        Err(TypeAssetError::PinMismatch {
            field: "typingsFileCount",
            ..
        })
    ));

    let bad_hash = TypeAssetFile::new(
        AssetPath::new("/pkg/engine/lib/lib.d.ts").expect("path"),
        "declare const x: 1;\n",
        "0".repeat(64),
        20,
    );
    assert!(matches!(bad_hash, Err(TypeAssetError::HashMismatch { .. })));

    assert!(matches!(
        TypeAssetsPin::new(
            "5.3.1",
            PINNED_STANDARD_LIB_FILE_COUNT - 1,
            0,
            sha("pin-a"),
            PINNED_TYPINGS_FILE_COUNT,
            0,
            sha("pin-b")
        ),
        Err(TypeAssetError::PinMismatch { .. })
    ));
    assert!(matches!(
        TypeAssetsPin::new(
            "5.3.1",
            PINNED_STANDARD_LIB_FILE_COUNT,
            0,
            sha("pin-a"),
            PINNED_TYPINGS_FILE_COUNT,
            0,
            "not-a-digest"
        ),
        Err(TypeAssetError::InvalidPinDigest { .. })
    ));
}

#[test]
fn type_assets_enforce_file_byte_and_path_budgets() {
    let (standard_lib_root, typings_root, files) =
        fixture_files_at("/pkg/budget", "/pkg/typings", "budget", 64, 64);
    let mut oversized = files;
    while oversized.len() <= MAX_TYPE_ASSET_FILES {
        oversized.push(asset(
            &format!("/pkg/budget/lib.extra{}.d.ts", oversized.len()),
            "export {};\n",
        ));
    }
    assert!(matches!(
        TypeAssets::new(
            AssetPath::new(&standard_lib_root).expect("root"),
            AssetPath::new(&typings_root).expect("root"),
            oversized,
            TypeAssetsPin::pinned_runtime(),
        ),
        Err(TypeAssetError::TooManyFiles { .. })
    ));

    let long_component = "a".repeat(MAX_ASSET_COMPONENT_BYTES - 5);
    let mut huge = vec![asset(
        &format!("/pkg/budget/{long_component}.d.ts"),
        &"x".repeat(MAX_TYPE_ASSET_BYTES - 100),
    )];
    assert!(matches!(
        TypeAssets::new(
            AssetPath::new("/pkg/budget").expect("root"),
            AssetPath::new("/pkg/typings").expect("root"),
            std::mem::take(&mut huge),
            TypeAssetsPin::pinned_runtime(),
        ),
        Err(TypeAssetError::TooLarge { .. })
    ));
}

#[test]
fn environment_vfs_mounts_assets_without_exposing_host_files() {
    let vfs = VirtualFileSystem::from_analysis_input(&environment_input(BTreeMap::new()))
        .expect("environment vfs");
    let snapshot = vfs.snapshot_files();
    assert!(snapshot.contains_key("/pkg/engine/lib/lib.d.ts"));
    assert!(snapshot.contains_key("/pkg/typings/miniprogram-api-typings/index.d.ts"));
    assert!(vfs.source_paths().all(|path| !path.contains("/pkg/")));
    assert_eq!(
        vfs.asset_paths().count(),
        usize::try_from(PINNED_TYPE_ASSET_FILES).expect("count")
    );

    let answer = |method: &str, path: &str| {
        vfs.answer(method, &serde_json::Value::String(path.to_owned()))
            .expect("callback")
    };
    assert_eq!(
        answer("readFile", "/etc/passwd"),
        serde_json::json!({"kind": "missing"})
    );
    assert_eq!(
        answer("readFile", "/pkg/engine/lib/lib.d.ts"),
        serde_json::json!({"kind": "value", "value": snapshot["/pkg/engine/lib/lib.d.ts"]})
    );
    assert_eq!(
        answer("fileExists", "/pkg/engine/lib/lib.d.ts"),
        serde_json::json!({"kind": "value", "value": true})
    );
    assert_eq!(
        answer("getAccessibleEntries", "/pkg/engine/lib")["value"]["directories"],
        serde_json::json!([])
    );
}

#[test]
fn environment_vfs_rejects_asset_project_collisions() {
    let project = vec![
        SourceFile::new(
            "lib.d.ts",
            "declare const project: true;\n",
            DocumentRevision::new(1),
        )
        .expect("project file"),
    ];
    let (standard_lib_root, typings_root, files) = fixture_files_at(
        "/work/type-assets",
        "/pkg/typings-collision",
        "collision",
        64,
        64,
    );
    let pin = pin_for(&standard_lib_root, &typings_root, &files);
    let assets = TypeAssets::new(
        AssetPath::new(standard_lib_root).expect("stdlib root"),
        AssetPath::new(typings_root).expect("typings root"),
        files,
        pin,
    )
    .expect("assets");
    let input = AnalysisInput::with_environment(
        ProjectKey::new("type-assets"),
        ProjectRoot::new("/work/type-assets").expect("root"),
        SnapshotRevision::new(1),
        project,
        CompilerOptions::default(),
        std::sync::Arc::new(assets),
    )
    .expect("input");
    assert!(matches!(
        VirtualFileSystem::from_analysis_input(&input),
        Err(VfsError::AssetCollision { .. })
    ));
}

#[test]
fn environment_vfs_filters_out_of_vfs_resolution_options() {
    let options = BTreeMap::from([
        (
            "typeRoots".to_owned(),
            serde_json::json!(["/etc/types", "../outside-types", "local-types"]),
        ),
        ("baseUrl".to_owned(), serde_json::json!("/outside/base")),
        (
            "paths".to_owned(),
            serde_json::json!({"@/*": ["src/*"], "@escape/*": ["../../etc/*"]}),
        ),
    ]);
    let vfs = VirtualFileSystem::from_analysis_input(&environment_input(options))
        .expect("environment vfs");
    let tsconfig: serde_json::Value =
        serde_json::from_str(vfs.file(vfs.tsconfig_path()).expect("tsconfig").text())
            .expect("tsconfig json");
    let compiler = &tsconfig["compilerOptions"];
    assert_eq!(
        compiler["typeRoots"],
        serde_json::json!(["local-types", "/pkg/typings"])
    );
    assert!(compiler.get("baseUrl").is_none());
    assert!(compiler.get("paths").is_none());
    let degradations = vfs.environment_degradations();
    assert!(degradations.iter().any(|item| item.field() == "typeRoots"));
    assert!(degradations.iter().any(|item| item.field() == "baseUrl"));
    assert!(degradations.iter().any(|item| item.field() == "paths"));
}

#[test]
fn environment_request_budget_includes_assets() {
    let assets = fake_assets_with_sizes("budget", 64, 64);
    let mut files = vec![
        SourceFile::new("main.ts", "const x = 1;\n", DocumentRevision::new(1)).expect("main.ts"),
    ];
    files.push(
        SourceFile::new(
            "large.ts",
            format!("// {}\n", "x".repeat(40 * 1024 * 1024)),
            DocumentRevision::new(1),
        )
        .expect("large.ts"),
    );
    let input = AnalysisInput::with_environment(
        ProjectKey::new("type-assets"),
        ProjectRoot::new("/work/type-assets").expect("root"),
        SnapshotRevision::new(1),
        files,
        CompilerOptions::default(),
        std::sync::Arc::new(assets),
    )
    .expect("input");
    let vfs = VirtualFileSystem::from_analysis_input(&input).expect("vfs");
    assert!(
        vfs.request_size_upper_bound() > MAX_REQUEST_BODY_BYTES,
        "asset-aware request bound must exceed the frozen budget"
    );
}

#[test]
fn fake_full_resend_ten_updates_stays_within_budget() {
    // Faithful client-side measurement: 136 assets and ~6.39 MiB of text.
    // One warm-up plus 20 measured updates (the required 10 plus extra samples
    // for a stable p95). Engine apply time and process RSS are not measurable
    // here and remain BLOCKED (see evidence/report).
    let assets = fake_assets_with_sizes(
        "benchmark",
        usize::try_from(PINNED_STANDARD_LIB_BYTES)
            .expect("bytes")
            .saturating_div(usize::try_from(PINNED_STANDARD_LIB_FILE_COUNT).expect("count"))
            .saturating_add(2),
        usize::try_from(PINNED_TYPINGS_BYTES)
            .expect("bytes")
            .saturating_div(usize::try_from(PINNED_TYPINGS_FILE_COUNT).expect("count"))
            .saturating_add(2),
    );
    assert_eq!(assets.file_count(), PINNED_TYPE_ASSET_FILES);
    let mut samples = Vec::new();
    let mut lengths = Vec::new();
    for revision in 0..=20u64 {
        let input = environment_input_with(assets.clone(), BTreeMap::new(), revision);
        let started = Instant::now();
        let vfs = VirtualFileSystem::from_analysis_input(&input).expect("environment vfs");
        let mut changes = SnapshotChanges::new()
            .close_project(vfs.tsconfig_path())
            .open_project(vfs.tsconfig_path())
            .with_file_system(SnapshotFileSystem::full(vfs.snapshot_files()));
        for path in vfs.source_paths() {
            changes = changes.close_file(path).open_file(path);
        }
        let bytes =
            serde_json::to_vec(&update_snapshot_params(1, &changes)).expect("serialized update");
        samples.push(started.elapsed());
        lengths.push(bytes.len());
        assert!(bytes.len() <= MAX_REQUEST_BODY_BYTES);
    }
    samples.sort();
    let p95 = samples[samples
        .len()
        .saturating_mul(95)
        .div_ceil(100)
        .saturating_sub(1)];
    let min = *lengths.iter().min().expect("length");
    let max = *lengths.iter().max().expect("length");
    println!("full resend: p95={p95:?} bytes={min}..{max}");
    assert!(
        p95 <= Duration::from_millis(200),
        "p95 full-resend update exceeded 200 ms: {p95:?}"
    );
    assert!(
        max.saturating_sub(min) <= 8,
        "serialized bytes grew across updates: {min}..{max}"
    );
}

struct AssetRoots {
    standard_lib: PathBuf,
    typings: PathBuf,
}

fn normalize_typings_root(path: &Path) -> Option<PathBuf> {
    let root = if path.file_name().and_then(|name| name.to_str()) == Some(MINIPROGRAM_TYPINGS) {
        path.parent()?.to_path_buf()
    } else {
        path.to_path_buf()
    };
    root.join(MINIPROGRAM_TYPINGS)
        .join("index.d.ts")
        .is_file()
        .then_some(root)
}

fn repository_typings_root() -> Option<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let node_modules = manifest
        .parent()?
        .parent()?
        .join("backends/typescript/node_modules");
    normalize_typings_root(&node_modules)
}

fn resolve_asset_roots(executable: &Path) -> Option<AssetRoots> {
    let packaged = std::env::var_os("TSGO_ASSET_ROOT").map(PathBuf::from);
    let standard_lib = packaged
        .as_ref()
        .and_then(|root| {
            [root.join("engine/lib"), root.join("lib")]
                .into_iter()
                .find(|candidate| candidate.join("lib.d.ts").is_file())
        })
        .or_else(|| {
            executable
                .parent()
                .filter(|parent| parent.join("lib.d.ts").is_file())
                .map(Path::to_path_buf)
        })?;
    let typings = std::env::var_os("TSGO_TYPINGS_ROOT")
        .map(PathBuf::from)
        .and_then(|path| normalize_typings_root(&path))
        .or_else(|| {
            packaged
                .as_ref()
                .and_then(|root| normalize_typings_root(&root.join("typings")))
        })
        .or_else(repository_typings_root)?;
    Some(AssetRoots {
        standard_lib,
        typings,
    })
}

fn real_engine_context() -> Option<(PathBuf, AssetRoots)> {
    let required = std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref() == Ok("1");
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1; \
             set it to the pinned 7.1.0-beta lib/tsc"
        );
        eprintln!(
            "real-engine type-asset gate skipped: set TSGO_BIN to the pinned \
             7.1.0-beta lib/tsc (or WXML_REQUIRE_REAL_ENGINE=1 for a hard gate)"
        );
        return None;
    };
    let executable = PathBuf::from(bin);
    assert!(
        executable.is_file(),
        "TSGO_BIN does not point at a file: {}",
        executable.display()
    );
    let Some(roots) = resolve_asset_roots(&executable) else {
        assert!(
            !required,
            "type-asset roots are required when WXML_REQUIRE_REAL_ENGINE=1; \
             set TSGO_ASSET_ROOT or TSGO_TYPINGS_ROOT"
        );
        eprintln!(
            "real-engine type-asset gate skipped: no standard-library/typings roots \
             found near {}",
            executable.display()
        );
        return None;
    };
    Some((executable, roots))
}

fn declaration_files(root: &Path, recursive: bool, lib_only: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                files.extend(declaration_files(&path, recursive, lib_only));
            }
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.ends_with(".d.ts") || (lib_only && !name.starts_with("lib")) {
            continue;
        }
        files.push(path);
    }
    files.sort();
    files
}

fn load_type_assets(roots: &AssetRoots, omit: Option<&Path>) -> Result<TypeAssets, TypeAssetError> {
    let mut files = Vec::new();
    for path in declaration_files(&roots.standard_lib, false, true)
        .into_iter()
        .chain(declaration_files(
            &roots.typings.join(MINIPROGRAM_TYPINGS),
            true,
            false,
        ))
    {
        if omit.is_some_and(|omit| path.as_path() == omit) {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read declaration");
        let size = u64::try_from(text.len()).expect("size");
        let hash = sha(&text);
        files.push(
            TypeAssetFile::new(
                AssetPath::new(path.to_string_lossy().into_owned()).expect("asset path"),
                text,
                hash,
                size,
            )
            .expect("asset"),
        );
    }
    TypeAssets::new(
        AssetPath::new(roots.standard_lib.to_string_lossy().into_owned()).expect("stdlib root"),
        AssetPath::new(roots.typings.to_string_lossy().into_owned()).expect("typings root"),
        files,
        TypeAssetsPin::pinned_runtime(),
    )
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unique(tag: &str) -> String {
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("wxml-tsgo-type-assets-{tag}-{}-{count}", std::process::id())
}

fn real_engine_environment() -> Option<(PathBuf, AssetRoots, TypeAssets)> {
    let (executable, roots) = real_engine_context()?;
    let assets = load_type_assets(&roots, None).expect("verified real type assets");
    Some((executable, roots, assets))
}

#[test]
fn real_engine_resolves_wx_page_component_and_getapp() {
    let Some((executable, roots, assets)) = real_engine_environment() else {
        return;
    };
    println!(
        "type assets: files={} bytes={} stdlib={} typings={}",
        assets.file_count(),
        assets.total_bytes(),
        assets.standard_lib_root(),
        assets.typings_root()
    );
    assert_eq!(assets.file_count(), PINNED_TYPE_ASSET_FILES);
    assert_eq!(assets.total_bytes(), PINNED_TYPE_ASSET_BYTES);
    assert_eq!(assets.pin(), &TypeAssetsPin::pinned_runtime());
    println!("typings source: repository fixture (not a packaged-chain claim)");

    let sandbox = std::env::temp_dir().join(unique("sandbox"));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _sandbox_guard = Cleanup(sandbox.clone());
    let project_root = format!("/tmp/opencode/{}", unique("project"));
    assert!(
        !Path::new(&project_root).exists(),
        "the virtual project root must not exist on disk: {project_root}"
    );

    let files = vec![SourceFile::new("main.ts", MAIN, DocumentRevision::new(1)).expect("main.ts")];
    let input = AnalysisInput::with_environment(
        ProjectKey::new("type-assets"),
        ProjectRoot::new(&project_root).expect("project root"),
        SnapshotRevision::new(1),
        files,
        CompilerOptions::default(),
        std::sync::Arc::new(assets.clone()),
    )
    .expect("environment input");
    let mut session =
        NativeSession::start(SessionOptions::new(&executable, sandbox)).expect("start real engine");
    session.open_project(&input).expect("open project");

    let evidence = session.diagnostics("main.ts").expect("diagnostics");
    let semantic_codes: Vec<i64> = evidence.semantic.iter().map(|item| item.code).collect();
    println!("semantic diagnostics: {semantic_codes:?}");
    assert!(
        !semantic_codes.contains(&2304),
        "wx/Page/Component/getApp must not be missing: {semantic_codes:?}"
    );
    assert!(
        semantic_codes.contains(&2339),
        "the missing member must stay a 2339 error: {semantic_codes:?}"
    );
    assert!(
        semantic_codes.contains(&2322),
        "the type mismatch must stay a 2322 error: {semantic_codes:?}"
    );

    for (line, character, expected) in [
        (0, 6, "SystemInfo"),
        (0, 13, "Wx"),
        (3, 13, "Constructor"),
        (4, 18, "Constructor"),
        (5, 12, "GetApp"),
    ] {
        let text = session
            .type_query("main.ts", position(line, character))
            .expect("type query")
            .text
            .unwrap_or_default();
        println!("type at {line}:{character} = {text}");
        assert!(
            text.contains(expected),
            "position {line}:{character} must be {expected}, got {text:?}"
        );
    }

    let completions = session
        .completions("main.ts", position(6, 3))
        .expect("completions")
        .expect("completion list");
    let names: Vec<&str> = completions
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    println!("wx. completions: {} entries", names.len());
    assert!(!names.is_empty(), "wx. completion must not be empty");
    for expected in ["getSystemInfoSync", "getStorageSync", "getLocation"] {
        assert!(
            names.contains(&expected),
            "wx. completion must contain {expected}"
        );
    }

    // Open with assets, then reject a legacy update without downgrading.
    let legacy = AnalysisInput::new(
        ProjectKey::new("type-assets"),
        ProjectRoot::new(&project_root).expect("project root"),
        SnapshotRevision::new(2),
        vec![SourceFile::new("main.ts", MAIN, DocumentRevision::new(2)).expect("main.ts")],
    )
    .expect("legacy input");
    assert!(matches!(
        session.update_project(&legacy),
        Err(NativeError::InvalidParams { .. })
    ));
    assert!(session.type_environment_identity().is_some());
    let after = session
        .diagnostics("main.ts")
        .expect("diagnostics after rejection");
    assert!(
        !after.semantic.iter().any(|item| item.code == 2304),
        "a rejected downgrade must keep the type environment"
    );

    let changed_options = AnalysisInput::with_environment(
        ProjectKey::new("type-assets"),
        ProjectRoot::new(&project_root).expect("project root"),
        SnapshotRevision::new(3),
        vec![SourceFile::new("main.ts", MAIN, DocumentRevision::new(2)).expect("main.ts")],
        CompilerOptions::new(BTreeMap::from([(
            "strict".to_owned(),
            serde_json::json!(true),
        )]))
        .expect("options"),
        std::sync::Arc::new(assets),
    )
    .expect("changed environment");
    assert!(matches!(
        session.update_project(&changed_options),
        Err(NativeError::InvalidParams { .. })
    ));

    session.release().expect("release");
    session.shutdown().expect("shutdown");
    assert!(roots.typings.join(MINIPROGRAM_TYPINGS).is_dir());
}

#[test]
fn real_engine_missing_typings_declaration_fails_closed() {
    let Some((_executable, roots)) = real_engine_context() else {
        return;
    };
    for name in ["types/index.d.ts", "lib.es5.d.ts", "types/wx/index.d.ts"] {
        let missing = roots.typings.join(MINIPROGRAM_TYPINGS).join(name);
        let missing = if name.starts_with("lib") {
            roots.standard_lib.join(name)
        } else {
            missing
        };
        let error = load_type_assets(&roots, Some(&missing))
            .expect_err("a missing declaration must fail closed");
        println!("missing asset error: {error}");
        assert!(
            matches!(
                error,
                TypeAssetError::MissingRequired { .. } | TypeAssetError::PinMismatch { .. }
            ),
            "expected a typed missing/pin error, got {error}"
        );
    }
}
