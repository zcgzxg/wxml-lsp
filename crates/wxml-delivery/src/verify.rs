//! Read-only tree verification and version probing.
//!
//! Schema v1 (legacy Node) trees are hashed read-only: this module never runs
//! `node --version`, never resolves `WXML_NODE`, and never starts a Node
//! runtime. Native schema v2 activation lives in [`crate::native_install`].

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use crate::error::{DeliveryError, Result, display_name};
use crate::manifest::{
    ExpectedManifest, MAX_MANIFEST_BYTES, Manifest, parse_sha256sums, read_limited, read_manifest,
    sha256_file,
};

/// Verifies a complete package tree against its own manifest and checksums.
///
/// # Errors
///
/// Returns an error for missing, extra, linked or modified files, unexpected
/// modes, or a manifest that does not match `expected`.
pub fn verify_tree(root: &Path, expected: ExpectedManifest<'_>) -> Result<Manifest> {
    let manifest = read_manifest(root, expected)?;
    let checksums_path = root.join("SHA256SUMS");
    let checksum_bytes = read_limited(&checksums_path, MAX_MANIFEST_BYTES)?;
    let checksums = parse_sha256sums(&checksum_bytes)?;

    let mut required: BTreeSet<String> = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    required.insert("manifest.json".to_owned());
    let listed: BTreeSet<String> = checksums.keys().cloned().collect();
    if listed != required {
        return Err(DeliveryError::Integrity(
            "SHA256SUMS does not list exactly the manifest payload".to_owned(),
        ));
    }

    for file in &manifest.files {
        let path = root.join(&file.path);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                DeliveryError::Integrity(format!("payload `{}` is missing", file.path))
            } else {
                DeliveryError::io(&path, error)
            }
        })?;
        if !metadata.file_type().is_file() {
            return Err(DeliveryError::Integrity(format!(
                "payload `{}` is not a regular file",
                file.path
            )));
        }
        if metadata.len() != file.size {
            return Err(DeliveryError::Integrity(format!(
                "payload `{}` has the wrong size",
                file.path
            )));
        }
        check_mode(&path, &metadata, file.mode_bits()?)?;
        let digest = sha256_file(&path)?;
        if digest != file.sha256 || checksums.get(&file.path) != Some(&digest) {
            return Err(DeliveryError::Integrity(format!(
                "payload `{}` does not match its SHA-256",
                file.path
            )));
        }
    }

    let manifest_path = root.join("manifest.json");
    let manifest_digest = sha256_file(&manifest_path)?;
    if checksums.get("manifest.json") != Some(&manifest_digest) {
        return Err(DeliveryError::Integrity(
            "manifest.json does not match SHA256SUMS".to_owned(),
        ));
    }

    let mut actual = BTreeSet::new();
    collect_files(root, root, &mut actual)?;
    let mut expected_files = required;
    expected_files.insert("SHA256SUMS".to_owned());
    if actual != expected_files {
        return Err(DeliveryError::Integrity(
            "package tree contains missing or unexpected files".to_owned(),
        ));
    }

    Ok(manifest)
}

fn collect_files(root: &Path, directory: &Path, files: &mut BTreeSet<String>) -> Result<()> {
    let entries =
        std::fs::read_dir(directory).map_err(|error| DeliveryError::io(directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| DeliveryError::io(directory, error))?;
        let path = entry.path();
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(DeliveryError::Integrity(
                "package tree contains a symbolic link".to_owned(),
            ));
        }
        if metadata.file_type().is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.file_type().is_file() {
            let relative = path.strip_prefix(root).map_err(|_| {
                DeliveryError::Integrity("package path escaped its root".to_owned())
            })?;
            files.insert(relative.to_string_lossy().replace('\\', "/"));
        } else {
            return Err(DeliveryError::Integrity(
                "package tree contains a non-regular file".to_owned(),
            ));
        }
    }
    Ok(())
}

fn check_mode(path: &Path, metadata: &std::fs::Metadata, expected: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let actual = metadata.permissions().mode() & 0o777;
        if actual != expected {
            return Err(DeliveryError::Integrity(format!(
                "`{}` has mode {actual:o}, expected {expected:o}",
                crate::paths::redact(path)
            )));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, metadata, expected);
        Ok(())
    }
}

/// Runs a short version probe with a hard timeout.
pub(crate) fn output_with_timeout(command: &mut Command, timeout: Duration) -> Result<Output> {
    crate::probe::run_probe(command, timeout)
}

/// Executes `wxml-lsp --version` and returns the reported version.
///
/// # Errors
///
/// Returns an error when the process cannot start, exits unsuccessfully or
/// prints an unexpected version line.
pub fn binary_version(binary: &Path) -> Result<String> {
    let mut command = Command::new(binary);
    command.arg("--version");
    let output = output_with_timeout(&mut command, Duration::from_secs(10)).map_err(|error| {
        DeliveryError::Runtime(format!(
            "could not execute the installed wxml-lsp (`{}`); package may be corrupt: {error}",
            display_name(binary)
        ))
    })?;
    if !output.status.success() {
        return Err(DeliveryError::Runtime(
            "installed wxml-lsp --version exited unsuccessfully".to_owned(),
        ));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| {
        DeliveryError::Runtime("wxml-lsp --version output was not UTF-8".to_owned())
    })?;
    let reported = stdout
        .trim()
        .strip_prefix("wxml-lsp ")
        .ok_or_else(|| DeliveryError::Runtime("unexpected wxml-lsp --version output".to_owned()))?;
    Ok(reported.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::manifest::{
        ArtifactKind, BinaryArtifact, FileEntry, Manifest, NodeInfo, PACKAGE_NAME, SCHEMA_VERSION,
        atomic_write, set_mode, sha256_file,
    };
    use crate::paths::{host_platform, validate_relative_path};

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let path = base.join(format!(
            "wxml-verify-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_file(root: &Path, path: &str, bytes: &[u8], mode: u32) {
        validate_relative_path(path, false).unwrap();
        let destination = root.join(path);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::write(&destination, bytes).unwrap();
        set_mode(&destination, mode).unwrap();
    }

    fn write_checksums(root: &Path, manifest: &Manifest) {
        use std::fmt::Write as _;

        let mut entries = std::collections::BTreeMap::new();
        for file in &manifest.files {
            entries.insert(
                file.path.clone(),
                sha256_file(&root.join(&file.path)).unwrap(),
            );
        }
        entries.insert(
            "manifest.json".to_owned(),
            sha256_file(&root.join("manifest.json")).unwrap(),
        );
        let mut text = String::new();
        for (path, digest) in entries {
            writeln!(text, "{digest}  {path}").unwrap();
        }
        atomic_write(&root.join("SHA256SUMS"), text.as_bytes(), 0o644).unwrap();
    }

    fn valid_tree(root: &Path) -> Manifest {
        let script = b"#!/bin/sh\nprintf 'wxml-lsp 0.1.0\\n'\n";
        write_file(root, "bin/wxml-lsp", script, 0o755);
        write_file(
            root,
            "bin/wxml-type-backend/server.js",
            b"console.log('backend');\n",
            0o644,
        );
        write_file(
            root,
            "bin/wxml-type-backend/node_modules/typescript/package.json",
            b"{}\n",
            0o644,
        );
        write_file(
            root,
            "bin/wxml-type-backend/node_modules/miniprogram-api-typings/package.json",
            b"{}\n",
            0o644,
        );
        write_file(root, "zed-extension/extension.wasm", b"\0asm", 0o644);
        let mut files = Vec::new();
        for path in [
            "bin/wxml-lsp",
            "bin/wxml-type-backend/server.js",
            "bin/wxml-type-backend/node_modules/typescript/package.json",
            "bin/wxml-type-backend/node_modules/miniprogram-api-typings/package.json",
            "zed-extension/extension.wasm",
        ] {
            let absolute = root.join(path);
            files.push(FileEntry {
                path: path.to_owned(),
                sha256: sha256_file(&absolute).unwrap(),
                size: std::fs::metadata(&absolute).unwrap().len(),
                mode: if path == "bin/wxml-lsp" {
                    "0755".to_owned()
                } else {
                    "0644".to_owned()
                },
            });
        }
        let manifest = Manifest {
            schema_version: SCHEMA_VERSION,
            package: PACKAGE_NAME.to_owned(),
            package_version: "0.1.0".to_owned(),
            platform: host_platform().unwrap(),
            protocol_version: 2,
            node: NodeInfo {
                minimum_version: "0.0.0".to_owned(),
                backend_version: "0.1.0".to_owned(),
            },
            built_at: "2026-01-01T00:00:00Z".to_owned(),
            binaries: vec![
                BinaryArtifact {
                    kind: ArtifactKind::Rust,
                    name: "wxml-lsp".to_owned(),
                    path: "bin/wxml-lsp".to_owned(),
                    version: "0.1.0".to_owned(),
                    protocol_version: None,
                    sha256: files[0].sha256.clone(),
                },
                BinaryArtifact {
                    kind: ArtifactKind::NodeBackend,
                    name: "node-backend".to_owned(),
                    path: "bin/wxml-type-backend/server.js".to_owned(),
                    version: "0.1.0".to_owned(),
                    protocol_version: Some(2),
                    sha256: files[1].sha256.clone(),
                },
            ],
            files,
        };
        manifest.validate(ExpectedManifest::default()).unwrap();
        atomic_write(
            &root.join("manifest.json"),
            &manifest.to_json().unwrap(),
            0o644,
        )
        .unwrap();
        write_checksums(root, &manifest);
        manifest
    }

    #[test]
    fn verifies_a_complete_tree_and_rejects_tampering() {
        let base = scratch("tree");
        let manifest = valid_tree(&base);
        verify_tree(&base, ExpectedManifest::default()).unwrap();

        write_file(&base, "extra.txt", b"extra", 0o644);
        assert!(verify_tree(&base, ExpectedManifest::default()).is_err());
        std::fs::remove_file(base.join("extra.txt")).unwrap();

        let payload = base.join("bin/wxml-type-backend/server.js");
        std::fs::write(&payload, b"tampered").unwrap();
        assert!(verify_tree(&base, ExpectedManifest::default()).is_err());
        // Restore the original bytes so the following checks test other paths.
        write_file(
            &base,
            "bin/wxml-type-backend/server.js",
            b"console.log('backend');\n",
            0o644,
        );
        write_checksums(&base, &manifest);

        std::fs::remove_file(
            base.join("bin/wxml-type-backend/node_modules/typescript/package.json"),
        )
        .unwrap();
        assert!(verify_tree(&base, ExpectedManifest::default()).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_symlinks_and_wrong_modes() {
        let base = scratch("links");
        valid_tree(&base);
        let link = base.join("link");
        std::os::unix::fs::symlink("/etc/passwd", &link).unwrap();
        assert!(verify_tree(&base, ExpectedManifest::default()).is_err());
        std::fs::remove_file(&link).unwrap();
        set_mode(&base.join("bin/wxml-lsp"), 0o644).unwrap();
        assert!(verify_tree(&base, ExpectedManifest::default()).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_unexpected_binary_output() {
        let base = scratch("binary-output");
        let script = base.join("wrong");
        std::fs::write(&script, b"#!/bin/sh\nprintf 'not-wxml 9.9.9\\n'\n").unwrap();
        set_mode(&script, 0o755).unwrap();
        assert!(binary_version(&script).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
