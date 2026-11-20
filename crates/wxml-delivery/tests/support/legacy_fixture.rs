//! Test-only writer for frozen schema v1 (legacy Node) package fixtures.
//!
//! The production v1 packager was removed in P7. Delivery tests still need
//! byte-stable legacy trees to prove read-only inspection, migration refusal
//! and rollback boundaries, so this module rebuilds the exact fixture layout
//! without any Node runtime or production write path. It is test code only and
//! is deliberately not exported from the crate.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wxml_delivery::archive::create_tar_gz;
use wxml_delivery::manifest::{
    ArtifactKind, BinaryArtifact, FileEntry, Manifest, NodeInfo, PACKAGE_NAME, SCHEMA_VERSION,
    archive_name, atomic_write, format_sha256sums, set_mode, sha256_file,
};
use wxml_delivery::paths::host_platform;

/// One generated legacy release.
pub struct LegacyRelease {
    /// Release output root containing the versioned directory and index.
    pub output: PathBuf,
    /// Versioned release directory (`wxml-lsp-<version>-<platform>`).
    pub release_dir: PathBuf,
    /// Archive path.
    pub archive: PathBuf,
}

/// Builds a schema v1 package fixture at `output`.
///
/// `marker`, when supplied, is written by the fixture `wxml-lsp` entry point so
/// a test can prove the legacy binary was never executed. The generated files
/// live below `output` and are removed by the caller's temporary directory.
#[allow(clippy::too_many_lines)]
pub fn build_legacy_release(output: &Path, version: &str, marker: Option<&Path>) -> LegacyRelease {
    let platform = host_platform().expect("host platform");
    let input = output.join(format!(".legacy-input-{version}"));
    let _ = std::fs::remove_dir_all(&input);
    std::fs::create_dir_all(input.join("bin/wxml-type-backend/node_modules/typescript"))
        .expect("create legacy input");
    std::fs::create_dir_all(
        input.join("bin/wxml-type-backend/node_modules/miniprogram-api-typings"),
    )
    .expect("create legacy input");
    std::fs::create_dir_all(input.join("zed-extension")).expect("create legacy input");

    let marker_line = marker.map_or_else(
        || "true".to_owned(),
        |path| format!(": > {}", shell_quote(path)),
    );
    let script = format!(
        "#!/bin/sh\n{marker_line}\nif [ \"$1\" = \"--version\" ]; then printf 'wxml-lsp {version}\\n'; exit 0; fi\nexit 0\n"
    );
    write_file(&input.join("bin/wxml-lsp"), script.as_bytes(), 0o755);
    write_file(
        &input.join("bin/wxml-lsp-install"),
        format!("#!/bin/sh\nprintf 'wxml-lsp-install {version}\\n'\n").as_bytes(),
        0o755,
    );
    write_file(
        &input.join("bin/wxml-type-backend/server.js"),
        b"console.log('legacy fixture');\n",
        0o644,
    );
    write_file(
        &input.join("bin/wxml-type-backend/node_modules/typescript/package.json"),
        b"{}\n",
        0o644,
    );
    write_file(
        &input.join("bin/wxml-type-backend/node_modules/miniprogram-api-typings/package.json"),
        b"{}\n",
        0o644,
    );
    write_file(
        &input.join("zed-extension/extension.wasm"),
        b"\0asm-legacy-fixture",
        0o644,
    );
    write_file(
        &input.join("zed-extension/extension.toml"),
        b"id = \"wxml-lsp\"\nversion = \"0.0.1\"\n",
        0o644,
    );
    write_file(
        &input.join("THIRD_PARTY_LICENSES.md"),
        b"# licenses\n",
        0o644,
    );

    let paths = [
        "THIRD_PARTY_LICENSES.md",
        "bin/wxml-lsp",
        "bin/wxml-lsp-install",
        "bin/wxml-type-backend/node_modules/miniprogram-api-typings/package.json",
        "bin/wxml-type-backend/node_modules/typescript/package.json",
        "bin/wxml-type-backend/server.js",
        "zed-extension/extension.toml",
        "zed-extension/extension.wasm",
    ];
    let files: Vec<FileEntry> = paths
        .iter()
        .map(|path| {
            let absolute = input.join(path);
            let metadata = std::fs::metadata(&absolute).expect("fixture metadata");
            FileEntry {
                path: (*path).to_owned(),
                sha256: sha256_file(&absolute).expect("fixture hash"),
                size: metadata.len(),
                mode: mode_text(&absolute),
            }
        })
        .collect();
    let find = |path: &str| files.iter().find(|file| file.path == path).unwrap();
    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        package: PACKAGE_NAME.to_owned(),
        package_version: version.to_owned(),
        platform: platform.clone(),
        protocol_version: 2,
        node: NodeInfo {
            minimum_version: "24.0.0".to_owned(),
            backend_version: "0.1.0".to_owned(),
        },
        built_at: "2026-01-01T00:00:00Z".to_owned(),
        binaries: vec![
            BinaryArtifact {
                kind: ArtifactKind::Rust,
                name: "wxml-lsp".to_owned(),
                path: "bin/wxml-lsp".to_owned(),
                version: version.to_owned(),
                protocol_version: None,
                sha256: find("bin/wxml-lsp").sha256.clone(),
            },
            BinaryArtifact {
                kind: ArtifactKind::Rust,
                name: "wxml-lsp-install".to_owned(),
                path: "bin/wxml-lsp-install".to_owned(),
                version: version.to_owned(),
                protocol_version: None,
                sha256: find("bin/wxml-lsp-install").sha256.clone(),
            },
            BinaryArtifact {
                kind: ArtifactKind::NodeBackend,
                name: "node-backend".to_owned(),
                path: "bin/wxml-type-backend/server.js".to_owned(),
                version: "0.1.0".to_owned(),
                protocol_version: Some(2),
                sha256: find("bin/wxml-type-backend/server.js").sha256.clone(),
            },
            BinaryArtifact {
                kind: ArtifactKind::ZedExtension,
                name: "zed-extension".to_owned(),
                path: "zed-extension/extension.wasm".to_owned(),
                version: "0.0.1".to_owned(),
                protocol_version: None,
                sha256: find("zed-extension/extension.wasm").sha256.clone(),
            },
        ],
        files,
    };
    manifest
        .validate(wxml_delivery::manifest::ExpectedManifest::default())
        .expect("valid legacy fixture manifest");

    let release_name = format!("{PACKAGE_NAME}-{version}-{platform}");
    let release_dir = output.join(&release_name);
    let _ = std::fs::remove_dir_all(&release_dir);
    std::fs::create_dir_all(&release_dir).expect("create release directory");
    for file in &manifest.files {
        let source = input.join(&file.path);
        let destination = release_dir.join(&file.path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).expect("create release parent");
        }
        std::fs::copy(&source, &destination).expect("copy fixture payload");
        set_mode(&destination, file.mode_bits().expect("fixture mode")).expect("fixture mode");
    }
    atomic_write(
        &release_dir.join("manifest.json"),
        &manifest.to_json().expect("serialize fixture manifest"),
        0o644,
    )
    .expect("write fixture manifest");
    write_checksums(&release_dir, &manifest);

    let archive_name = archive_name(version, &platform).expect("fixture archive name");
    let archive = output.join(&archive_name);
    let _ = std::fs::remove_file(&archive);
    let mut entries = manifest.files;
    for name in ["manifest.json", "SHA256SUMS"] {
        let path = release_dir.join(name);
        entries.push(FileEntry {
            path: name.to_owned(),
            sha256: sha256_file(&path).expect("fixture metadata hash"),
            size: std::fs::metadata(&path).expect("fixture metadata").len(),
            mode: "0644".to_owned(),
        });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    create_tar_gz(&release_dir, &archive, &entries, 0).expect("create fixture archive");
    let archive_sha256 = sha256_file(&archive).expect("fixture archive hash");
    let archive_size = std::fs::metadata(&archive)
        .expect("fixture archive metadata")
        .len();

    let index = serde_json::json!({
        "schemaVersion": 1,
        "package": PACKAGE_NAME,
        "releases": [{
            "version": version,
            "platform": platform,
            "protocolVersion": 2,
            "nodeMinimumVersion": "24.0.0",
            "archive": archive_name,
            "sha256": archive_sha256,
            "size": archive_size,
        }],
    });
    atomic_write(
        &output.join("index.json"),
        format!("{index:#}\n").as_bytes(),
        0o644,
    )
    .expect("write fixture index");

    LegacyRelease {
        output: output.to_path_buf(),
        release_dir,
        archive,
    }
}

/// Copies a generated release into `versions/<version>` and points `current` at
/// it, producing the exact read-only root shape left by a pre-P7 install.
pub fn install_legacy_root(root: &Path, release_dir: &Path, version: &str) {
    let versions = root.join("versions");
    std::fs::create_dir_all(&versions).expect("create versions directory");
    let target = versions.join(version);
    let _ = std::fs::remove_dir_all(&target);
    copy_tree(release_dir, &target);
    let current = root.join("current");
    let _ = std::fs::remove_file(&current);
    std::os::unix::fs::symlink(Path::new("versions").join(version), &current)
        .expect("create legacy current symlink");
}

fn write_checksums(root: &Path, manifest: &Manifest) {
    let mut entries = BTreeMap::new();
    for file in &manifest.files {
        entries.insert(
            file.path.clone(),
            sha256_file(&root.join(&file.path)).expect("fixture payload hash"),
        );
    }
    entries.insert(
        "manifest.json".to_owned(),
        sha256_file(&root.join("manifest.json")).expect("fixture manifest hash"),
    );
    atomic_write(
        &root.join("SHA256SUMS"),
        &format_sha256sums(&entries),
        0o644,
    )
    .expect("write fixture checksums");
}

fn write_file(path: &Path, bytes: &[u8], mode: u32) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create fixture parent");
    }
    std::fs::write(path, bytes).expect("write fixture file");
    set_mode(path, mode).expect("fixture file mode");
}

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).expect("create fixture tree");
    for entry in std::fs::read_dir(source).expect("read fixture tree") {
        let entry = entry.expect("fixture entry");
        let path = entry.path();
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("fixture file type").is_dir() {
            copy_tree(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("copy fixture tree file");
            let mode = std::fs::metadata(&path)
                .expect("fixture mode")
                .permissions();
            std::fs::set_permissions(&target, mode).expect("copy fixture mode");
        }
    }
}

fn mode_text(path: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .expect("fixture mode metadata")
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o111 != 0 {
        "0755".to_owned()
    } else {
        "0644".to_owned()
    }
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}
