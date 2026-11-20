//! Schema-v2 fixture package for the non-real exec-audit mode.
//!
//! This is a **FIXTURE**, never a release: `bin/wxml-lsp`, `bin/wxml-lsp-install`
//! and `engine/lib/tsc` are shell scripts and the typings/standard-library
//! payload is the frozen test fixture, not tarball provenance. It exists so the
//! production installer's `install`/`verify`/`current`/`env` flows can be run
//! under the exec tracer without any pinned asset. The real-asset audit is in
//! `real.rs`.
//!
//! Structure mirrors the already committed `no_node_guard.rs` fixture tree;
//! only the pieces needed by this suite are kept.

use std::fs;
use std::path::{Path, PathBuf};

use wxml_delivery::archive::create_tar_gz;
use wxml_delivery::index_v2::{IndexV2, ReleaseV2};
use wxml_delivery::manifest::{FileEntry, atomic_write, set_mode, sha256_bytes, sha256_file};
use wxml_delivery::manifest_v2::{
    ENGINE_DISTRIBUTION, ENGINE_DISTRIBUTION_FILE_COUNT, ENGINE_DISTRIBUTION_INTEGRITY,
    ENGINE_DISTRIBUTION_SHA256, ENGINE_DISTRIBUTION_SIZE, ENGINE_DISTRIBUTION_UNPACKED_SIZE,
    ENGINE_DISTRIBUTION_URL, ENGINE_LICENSE, ENGINE_PROBE_PREFIX, ENGINE_SOURCE_COMMIT,
    ENGINE_VERSION, EngineInfo, EngineProbe, MANIFEST_SCHEMA_V2, MINIPROGRAM_TYPINGS_ASSET,
    MINIPROGRAM_TYPINGS_FILE_COUNT, MINIPROGRAM_TYPINGS_LICENSE, MINIPROGRAM_TYPINGS_ROOT,
    MINIPROGRAM_TYPINGS_VERSION, ManifestV2, NATIVE_PACKAGE_PLATFORM, NativeArtifactKind,
    NativeBinaryArtifact, STANDARD_LIB_ASSET, STANDARD_LIB_FILE_COUNT, STANDARD_LIB_INCLUDE,
    TypeAsset, compute_type_asset_bundle,
};
use wxml_delivery::sri::sha512_sri_bytes;
use wxml_delivery::verify_v2::{checksums_for_tree_v2, verify_tree_v2};

#[path = "typings_payload.rs"]
mod typings_payload;

/// Fixture package version.
pub const VERSION: &str = "0.1.0";

/// A built fixture channel directory accepted by `install --from-dir`.
pub struct Release {
    /// Directory that holds `index.json` and the archive.
    pub channel: PathBuf,
}

struct FileBuilder {
    root: PathBuf,
    files: Vec<FileEntry>,
}

impl FileBuilder {
    fn new(root: &Path) -> Self {
        fs::create_dir_all(root).expect("create fixture root");
        Self {
            root: root.to_path_buf(),
            files: Vec::new(),
        }
    }

    fn file(&mut self, path: &str, bytes: &[u8], mode: u32) {
        let destination = self.root.join(path);
        fs::create_dir_all(destination.parent().expect("file parent")).expect("create parent");
        fs::write(&destination, bytes).expect("write fixture file");
        set_mode(&destination, mode).expect("set fixture mode");
        self.files.push(FileEntry {
            path: path.to_owned(),
            sha256: sha256_bytes(bytes),
            size: u64::try_from(bytes.len()).expect("fixture file size"),
            mode: format!("{mode:04o}"),
        });
    }

    fn hash(&self, path: &str) -> String {
        self.files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("fixture file `{path}`"))
            .sha256
            .clone()
    }
}

fn build_tree(root: &Path, version: &str) -> ManifestV2 {
    let mut builder = FileBuilder::new(root);
    builder.file(
        "bin/wxml-lsp",
        format!("#!/bin/sh\nprintf 'wxml-lsp {version}\\n'\n").as_bytes(),
        0o755,
    );
    builder.file(
        "bin/wxml-lsp-install",
        format!("#!/bin/sh\nprintf 'wxml-lsp-install {version}\\n'\n").as_bytes(),
        0o755,
    );
    builder.file(
        "engine/lib/tsc",
        format!("#!/bin/sh\nprintf '{ENGINE_PROBE_PREFIX}{ENGINE_VERSION}\\n'\n").as_bytes(),
        0o755,
    );
    builder.file("engine/lib/tsc.sig", b"signature-placeholder\n", 0o644);
    builder.file("engine/LICENSE", b"Apache License fixture\n", 0o644);
    builder.file("engine/NOTICE.txt", b"NOTICE fixture\n", 0o644);
    builder.file("engine/package.json", b"{\"name\":\"fixture\"}\n", 0o644);
    builder.file("engine/lib/lib.d.ts", b"// lib.d.ts\n", 0o644);
    builder.file("engine/lib/lib.es5.d.ts", b"// lib.es5.d.ts\n", 0o644);
    builder.file("engine/lib/lib.es2020.d.ts", b"// lib.es2020.d.ts\n", 0o644);
    for index in 0..113 {
        builder.file(
            &format!("engine/lib/lib.f{index:03}.d.ts"),
            format!("// filler {index}\n").as_bytes(),
            0o644,
        );
    }
    typings_payload::write_payload(|path, bytes| builder.file(path, bytes, 0o644));
    builder.file("zed-extension/extension.wasm", b"\0asm-fixture", 0o644);
    builder.file(
        "zed-extension/extension.toml",
        b"id = \"wxml-lsp\"\n",
        0o644,
    );
    builder.file("provenance.json", b"{\"fixture\":true}\n", 0o644);
    builder.file("THIRD_PARTY_LICENSES.md", b"# licenses\n", 0o644);

    let standard = compute_type_asset_bundle(
        &builder.files,
        "engine/lib",
        &[STANDARD_LIB_INCLUDE.to_owned()],
    )
    .expect("standard lib bundle");
    assert_eq!(standard.file_count, STANDARD_LIB_FILE_COUNT);
    let typings =
        compute_type_asset_bundle(&builder.files, MINIPROGRAM_TYPINGS_ROOT, &["**".to_owned()])
            .expect("typings bundle");
    assert_eq!(typings.file_count, MINIPROGRAM_TYPINGS_FILE_COUNT);

    let manifest = ManifestV2 {
        schema_version: MANIFEST_SCHEMA_V2,
        package: "wxml-lsp".to_owned(),
        package_version: version.to_owned(),
        platform: NATIVE_PACKAGE_PLATFORM.to_owned(),
        protocol_version: 2,
        built_at: "2026-11-20T00:00:00Z".to_owned(),
        engine: EngineInfo {
            name: "typescript".to_owned(),
            version: ENGINE_VERSION.to_owned(),
            source_commit: ENGINE_SOURCE_COMMIT.to_owned(),
            distribution: ENGINE_DISTRIBUTION.to_owned(),
            distribution_url: ENGINE_DISTRIBUTION_URL.to_owned(),
            distribution_integrity: ENGINE_DISTRIBUTION_INTEGRITY.to_owned(),
            distribution_sha256: ENGINE_DISTRIBUTION_SHA256.to_owned(),
            distribution_size: ENGINE_DISTRIBUTION_SIZE,
            distribution_file_count: ENGINE_DISTRIBUTION_FILE_COUNT,
            distribution_unpacked_size: ENGINE_DISTRIBUTION_UNPACKED_SIZE,
            api_schema_version: 9,
            ast_header_bytes: 64,
            license: ENGINE_LICENSE.to_owned(),
            license_files: vec!["engine/LICENSE".to_owned(), "engine/NOTICE.txt".to_owned()],
            binary: "engine/lib/tsc".to_owned(),
            probe: EngineProbe {
                args: vec!["--version".to_owned()],
                expect_prefix: ENGINE_PROBE_PREFIX.to_owned(),
            },
        },
        type_assets: vec![
            TypeAsset {
                name: STANDARD_LIB_ASSET.to_owned(),
                version: ENGINE_VERSION.to_owned(),
                license: ENGINE_LICENSE.to_owned(),
                root: "engine/lib".to_owned(),
                include: vec![STANDARD_LIB_INCLUDE.to_owned()],
                file_count: standard.file_count,
                sha256: standard.sha256,
                license_files: vec!["engine/LICENSE".to_owned(), "engine/NOTICE.txt".to_owned()],
            },
            TypeAsset {
                name: MINIPROGRAM_TYPINGS_ASSET.to_owned(),
                version: MINIPROGRAM_TYPINGS_VERSION.to_owned(),
                license: MINIPROGRAM_TYPINGS_LICENSE.to_owned(),
                root: MINIPROGRAM_TYPINGS_ROOT.to_owned(),
                include: vec!["**".to_owned()],
                file_count: typings.file_count,
                sha256: typings.sha256,
                license_files: vec![format!("{MINIPROGRAM_TYPINGS_ROOT}/LICENSE")],
            },
        ],
        binaries: vec![
            NativeBinaryArtifact {
                kind: NativeArtifactKind::Rust,
                name: "wxml-lsp".to_owned(),
                path: "bin/wxml-lsp".to_owned(),
                version: version.to_owned(),
                protocol_version: None,
                sha256: builder.hash("bin/wxml-lsp"),
            },
            NativeBinaryArtifact {
                kind: NativeArtifactKind::Rust,
                name: "wxml-lsp-install".to_owned(),
                path: "bin/wxml-lsp-install".to_owned(),
                version: version.to_owned(),
                protocol_version: None,
                sha256: builder.hash("bin/wxml-lsp-install"),
            },
            NativeBinaryArtifact {
                kind: NativeArtifactKind::NativeEngine,
                name: "tsgo".to_owned(),
                path: "engine/lib/tsc".to_owned(),
                version: ENGINE_VERSION.to_owned(),
                protocol_version: Some(2),
                sha256: builder.hash("engine/lib/tsc"),
            },
            NativeBinaryArtifact {
                kind: NativeArtifactKind::ZedExtension,
                name: "zed-extension".to_owned(),
                path: "zed-extension/extension.wasm".to_owned(),
                version: "0.0.1".to_owned(),
                protocol_version: None,
                sha256: builder.hash("zed-extension/extension.wasm"),
            },
        ],
        files: builder.files,
    };
    manifest
        .validate(wxml_delivery::manifest_v2::ExpectedManifestV2 {
            version: Some(version),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            protocol_version: Some(2),
        })
        .expect("fixture manifest validates");
    atomic_write(
        &root.join("manifest.json"),
        &manifest.to_json().expect("serialize manifest"),
        0o644,
    )
    .expect("write manifest");
    let checksums = checksums_for_tree_v2(root, &manifest).expect("fixture checksums");
    atomic_write(&root.join("SHA256SUMS"), &checksums, 0o644).expect("write checksums");
    verify_tree_v2(
        root,
        wxml_delivery::manifest_v2::ExpectedManifestV2 {
            version: Some(version),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            protocol_version: Some(2),
        },
    )
    .expect("fixture tree verifies");
    manifest
}

/// Writes the pinned 23-file typings payload below `root`.
pub fn write_typings_payload(root: &Path) {
    fs::create_dir_all(root).expect("create typings root");
    for (relative, bytes) in typings_payload::payload_files() {
        let destination = root.join(relative);
        fs::create_dir_all(destination.parent().expect("typings file parent"))
            .expect("create typings directory");
        fs::write(&destination, &bytes)
            .unwrap_or_else(|error| panic!("write {}: {error}", destination.display()));
        set_mode(&destination, 0o644)
            .unwrap_or_else(|error| panic!("chmod {}: {error}", destination.display()));
    }
}

/// Builds the fixture release channel under `base`.
pub fn build_release(base: &Path) -> Release {
    let channel = base.join("fixture-release");
    fs::create_dir_all(&channel).expect("create fixture release dir");
    let tree = base.join("fixture-tree");
    let manifest = build_tree(&tree, VERSION);
    let archive_name = format!("wxml-lsp-{VERSION}-{NATIVE_PACKAGE_PLATFORM}.tar.gz");
    let archive = channel.join(&archive_name);
    let mut entries = manifest.files;
    for name in ["manifest.json", "SHA256SUMS"] {
        let path = tree.join(name);
        entries.push(FileEntry {
            path: name.to_owned(),
            sha256: sha256_file(&path).expect("hash fixture metadata"),
            size: fs::metadata(&path).expect("fixture metadata size").len(),
            mode: "0644".to_owned(),
        });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    create_tar_gz(&tree, &archive, &entries, 0).expect("create release archive");
    let bytes = fs::read(&archive).expect("read release archive");
    let mut index = IndexV2::empty();
    index.upsert(ReleaseV2 {
        version: VERSION.to_owned(),
        platform: NATIVE_PACKAGE_PLATFORM.to_owned(),
        protocol_version: 2,
        engine_version: ENGINE_VERSION.to_owned(),
        engine_source_commit: ENGINE_SOURCE_COMMIT.to_owned(),
        engine_protocol_version: 2,
        engine_api_schema_version: 9,
        engine_ast_header_bytes: 64,
        engine_distribution: ENGINE_DISTRIBUTION.to_owned(),
        engine_integrity: ENGINE_DISTRIBUTION_INTEGRITY.to_owned(),
        min_installer_schema: 2,
        archive: archive_name,
        archive_integrity: sha512_sri_bytes(&bytes).to_sri(),
        sha256: sha256_bytes(&bytes),
        size: u64::try_from(bytes.len()).expect("archive size"),
    });
    atomic_write(
        &channel.join("index.json"),
        &index.to_json().expect("serialize index"),
        0o644,
    )
    .expect("write index");
    println!(
        "EXEC-AUDIT: FIXTURE package built at {} (shell fixture engine, not a release)",
        channel.display()
    );
    Release { channel }
}
