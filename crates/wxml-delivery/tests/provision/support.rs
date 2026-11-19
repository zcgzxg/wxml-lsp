//! Fixtures and oracles for the provisioning integration tests.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};
use tar::{Builder, EntryType, Header};

pub(crate) use crate::digests::{bundle_digest, sha256_hex, tree_digest_pairs};
pub(crate) use crate::lock_support::{TempDir, valid_value};

pub(crate) const PAYLOAD_FILES: &[(&str, &[u8])] = &[
    (
        "index.d.ts",
        b"export interface SyntheticApi { value: string }\n",
    ),
    (
        "types/synthetic.d.ts",
        b"export interface Nested { n: number }\n",
    ),
    ("LICENSE", b"Synthetic test license (MIT)\n"),
    ("README.md", b"# Synthetic asset\n"),
    ("CHANGELOG.md", b"## 1.0.0\n"),
    ("bin/tool", b"#!/bin/sh\nexit 0\n"),
];

pub(crate) const EXCLUDED_FILES: &[(&str, &[u8])] = &[
    ("package.json", b"{\"name\":\"synthetic-asset\"}\n"),
    ("README-en.md", b"# Synthetic asset (en)\n"),
    ("typings.json", b"{\"synthetic\":true}\n"),
];

pub(crate) const GOLDEN_PAYLOAD_TREE: &str =
    "50c849e14d62d01b3d1df86ec2226b95539ab37034bec20bc55bc80fd07cbbda";
pub(crate) const GOLDEN_UPSTREAM_TREE: &str =
    "cbba816f7e7d08bbe2d6111e0b8d1410a5dc913c883b635856d1acc9b8de73f7";
pub(crate) const GOLDEN_BUNDLE: &str =
    "7eeb17803f955bd9f44d9aa90d4a3730f975ee4e08270436c98d9a879cda3050";
pub(crate) const TEST_LOCK_SHA: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub(crate) const TEST_CACHE_KEY: &str =
    "50c849e14d62d01b3d1df86ec2226b95539ab37034bec20bc55bc80fd07cbbda.aaaaaaaaaaaaaaaa";

pub(crate) enum Spec<'a> {
    Dir(&'a str),
    File(&'a str, u32, &'a [u8]),
    Symlink(&'a str, &'a str),
    Hardlink(&'a str, &'a str),
    Fifo(&'a str),
    Char(&'a str),
}

/// Builds a deterministic npm tarball; every path is prefixed with `package/`.
pub(crate) fn npm_tar_gz(specs: &[Spec<'_>]) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = Builder::new(encoder);
    for spec in specs {
        let mut header = Header::new_gnu();
        match spec {
            Spec::Dir(name) => {
                header.set_entry_type(EntryType::Directory);
                header.set_size(0);
                header.set_mode(0o755);
                header.set_mtime(0);
                header.set_uid(0);
                header.set_gid(0);
                let path = format!("package/{name}");
                header.set_path(&path).unwrap();
                header.set_cksum();
                builder.append(&header, std::io::empty()).unwrap();
            }
            Spec::File(name, mode, body) => {
                header.set_entry_type(EntryType::Regular);
                header.set_size(u64::try_from(body.len()).unwrap());
                header.set_mode(*mode);
                header.set_mtime(0);
                header.set_uid(0);
                header.set_gid(0);
                let path = format!("package/{name}");
                header.set_path(&path).unwrap();
                header.set_cksum();
                builder.append(&header, *body).unwrap();
            }
            Spec::Symlink(name, target) => {
                header.set_entry_type(EntryType::Symlink);
                header.set_size(0);
                header.set_mode(0o777);
                header.set_mtime(0);
                let path = format!("package/{name}");
                header.set_path(&path).unwrap();
                header.set_link_name(target).unwrap();
                header.set_cksum();
                builder.append(&header, std::io::empty()).unwrap();
            }
            Spec::Hardlink(name, target) => {
                header.set_entry_type(EntryType::Link);
                header.set_size(0);
                header.set_mode(0o644);
                header.set_mtime(0);
                let path = format!("package/{name}");
                header.set_path(&path).unwrap();
                header.set_link_name(target).unwrap();
                header.set_cksum();
                builder.append(&header, std::io::empty()).unwrap();
            }
            Spec::Fifo(name) => {
                header.set_entry_type(EntryType::Fifo);
                header.set_size(0);
                header.set_mode(0o644);
                header.set_mtime(0);
                let path = format!("package/{name}");
                header.set_path(&path).unwrap();
                header.set_cksum();
                builder.append(&header, std::io::empty()).unwrap();
            }
            Spec::Char(name) => {
                header.set_entry_type(EntryType::Char);
                header.set_size(0);
                header.set_mode(0o644);
                header.set_mtime(0);
                let path = format!("package/{name}");
                header.set_path(&path).unwrap();
                header.set_cksum();
                builder.append(&header, std::io::empty()).unwrap();
            }
        }
    }
    builder.finish().unwrap();
    let encoder = builder.into_inner().unwrap();
    encoder.finish().unwrap()
}

pub(crate) fn synthetic_tarball() -> Vec<u8> {
    let mut specs = Vec::new();
    for (path, bytes) in PAYLOAD_FILES.iter().chain(EXCLUDED_FILES) {
        let mode = if *path == "bin/tool" { 0o755 } else { 0o644 };
        specs.push(Spec::File(path, mode, bytes));
    }
    npm_tar_gz(&specs)
}

pub(crate) fn sha512_sri(bytes: &[u8]) -> String {
    format!("sha512-{}", base64(&Sha512::digest(bytes)))
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(char::from(ALPHABET[usize::from(first >> 2)]));
        output.push(char::from(
            ALPHABET[usize::from((first & 0b0000_0011) << 4 | second >> 4)],
        ));
        output.push(if chunk.len() > 1 {
            char::from(ALPHABET[usize::from((second & 0b0000_1111) << 2 | third >> 6)])
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            char::from(ALPHABET[usize::from(third & 0b0011_1111)])
        } else {
            '='
        });
    }
    output
}

pub(crate) fn file_value(path: &str, mode: &str, bytes: &[u8]) -> Value {
    json!({
        "path": path,
        "sha256": sha256_hex(bytes),
        "size": bytes.len(),
        "mode": mode,
    })
}

fn payload_values() -> Vec<Value> {
    PAYLOAD_FILES
        .iter()
        .map(|(path, bytes)| {
            file_value(
                &format!("assets/synthetic/{path}"),
                if *path == "bin/tool" { "0755" } else { "0644" },
                bytes,
            )
        })
        .collect()
}

fn upstream_values() -> Vec<Value> {
    PAYLOAD_FILES
        .iter()
        .chain(EXCLUDED_FILES)
        .map(|(path, bytes)| {
            file_value(
                path,
                if *path == "bin/tool" { "0755" } else { "0644" },
                bytes,
            )
        })
        .collect()
}

/// Builds the synthetic asset JSON; digests are computed by the test oracles.
pub(crate) fn synthetic_asset_value(tarball: &[u8]) -> Value {
    let mut upstream = upstream_values();
    upstream.sort_by(|left, right| {
        left["path"]
            .as_str()
            .unwrap()
            .cmp(right["path"].as_str().unwrap())
    });
    let mut payload = payload_values();
    payload.sort_by(|left, right| {
        left["path"]
            .as_str()
            .unwrap()
            .cmp(right["path"].as_str().unwrap())
    });
    let upstream_tree = tree_digest_pairs(
        &upstream
            .iter()
            .map(|file| {
                (
                    file["path"].as_str().unwrap().to_owned(),
                    file["mode"].as_str().unwrap().to_owned(),
                    file["sha256"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<Vec<_>>(),
        "",
    );
    let payload_tree = tree_digest_pairs(
        &payload
            .iter()
            .map(|file| {
                (
                    file["path"].as_str().unwrap().to_owned(),
                    file["mode"].as_str().unwrap().to_owned(),
                    file["sha256"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<Vec<_>>(),
        "assets/synthetic",
    );
    let (bundle_count, bundle_bytes, bundle_sha) =
        bundle_digest(&payload, "assets/synthetic", &["**".to_owned()]);
    let total_bytes: u64 = payload
        .iter()
        .map(|file| file["size"].as_u64().unwrap())
        .sum();
    let unpacked: u64 = upstream
        .iter()
        .map(|file| file["size"].as_u64().unwrap())
        .sum();
    json!({
        "name": "synthetic-asset",
        "kind": "npm-tarball",
        "npmPackage": "synthetic-asset",
        "version": "1.0.0",
        "os": ["any"],
        "cpu": ["any"],
        "provenance": {
            "registry": "https://registry.npmjs.org",
            "distSha1": "0123456789abcdef0123456789abcdef01234567",
            "publishedAt": "2025-01-01T00:00:00Z",
        },
        "license": "MIT",
        "licenseFiles": ["assets/synthetic/LICENSE"],
        "packageRoot": "assets/synthetic",
        "tarball": {
            "url": "https://registry.npmjs.org/synthetic-asset/-/synthetic-asset-1.0.0.tgz",
            "size": tarball.len(),
            "integrity": sha512_sri(tarball),
            "sha256": sha256_hex(tarball),
        },
        "upstream": {
            "fileCount": upstream.len(),
            "unpackedSize": unpacked,
            "stripPrefix": "package/",
            "treeDigest": upstream_tree,
            "files": upstream,
        },
        "extract": {
            "include": [
                "index.d.ts",
                "types/**",
                "LICENSE",
                "README.md",
                "CHANGELOG.md",
                "bin/**",
            ],
            "exclude": [
                {"path": "package.json", "reason": "synthetic metadata"},
                {"path": "README-en.md", "reason": "synthetic translation"},
                {"path": "typings.json", "reason": "synthetic legacy metadata"},
            ],
            "executables": ["bin/tool"],
        },
        "payload": {
            "fileCount": payload.len(),
            "totalBytes": total_bytes,
            "treeDigest": payload_tree,
            "files": payload,
            "bundles": [{
                "name": "synthetic-bundle",
                "root": "assets/synthetic",
                "include": ["**"],
                "fileCount": bundle_count,
                "totalBytes": bundle_bytes,
                "sha256": bundle_sha,
            }],
        },
    })
}

pub(crate) fn synthetic_asset(tarball: &[u8]) -> wxml_delivery::assets_lock::Asset {
    let value = synthetic_asset_value(tarball);
    serde_json::from_value(value).unwrap()
}

pub(crate) fn write_file(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

pub(crate) fn write_tarball(directory: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = directory.join(name);
    write_file(&path, bytes);
    path
}

pub(crate) fn synthetic_asset_dir(tarball: &[u8]) -> (TempDir, PathBuf) {
    let temp = TempDir::new("synthetic-source");
    write_tarball(temp.path(), "synthetic-asset-1.0.0.tgz", tarball);
    let tarball_path = temp.path().join("synthetic-asset-1.0.0.tgz");
    (temp, tarball_path)
}

pub(crate) fn synthetic_asset_and_tarball() -> (TempDir, Vec<u8>) {
    let temp = TempDir::new("synthetic-fixture");
    let tarball = synthetic_tarball();
    (temp, tarball)
}

/// Writes a valid frozen lock into a temporary repository root.
pub(crate) fn write_repo_lock(root: &Path) -> PathBuf {
    write_repo_lock_value(root, &valid_value())
}

/// Writes an arbitrary lock value into a temporary repository root.
pub(crate) fn write_repo_lock_value(root: &Path, value: &Value) -> PathBuf {
    let repo = root.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let bytes = serde_json::to_vec_pretty(value).unwrap();
    let path = repo.join("assets.lock.json");
    fs::write(&path, &bytes).unwrap();
    path
}

pub(crate) fn provision_options(
    lock: &Path,
    cache: &Path,
) -> wxml_delivery::provision::ProvisionOptions {
    wxml_delivery::provision::ProvisionOptions {
        lock: lock.to_path_buf(),
        cache: cache.to_path_buf(),
        from_dir: None,
        offline: true,
        print_plan: false,
        allow_download: false,
    }
}

pub(crate) fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

pub(crate) fn mode_of_full(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

/// Minimal raw tar header for paths that `tar::Header::set_path` rejects.
pub(crate) fn raw_header(name: &[u8], size: u64, typeflag: u8) -> [u8; 512] {
    raw_header_mode(name, size, typeflag, 0o644)
}

pub(crate) fn raw_header_mode(name: &[u8], size: u64, typeflag: u8, mode: u32) -> [u8; 512] {
    let mut block = [0_u8; 512];
    block[..name.len()].copy_from_slice(name);
    let mode_field = format!("{mode:07o}\0");
    block[100..108].copy_from_slice(mode_field.as_bytes());
    block[108..116].copy_from_slice(b"0000000\0");
    block[116..124].copy_from_slice(b"0000000\0");
    let size_field = format!("{size:011o}\0");
    block[124..136].copy_from_slice(size_field.as_bytes());
    block[136..148].copy_from_slice(b"00000000000\0");
    block[148..156].fill(b' ');
    block[156] = typeflag;
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    let sum: u32 = block.iter().map(|byte| u32::from(*byte)).sum();
    let checksum = format!("{sum:06o}\0 ");
    block[148..156].copy_from_slice(checksum.as_bytes());
    block
}

pub(crate) fn raw_tar_gz(blocks: &[u8]) -> Vec<u8> {
    let file = Vec::new();
    let mut encoder = GzEncoder::new(file, Compression::default());
    encoder.write_all(blocks).unwrap();
    encoder.finish().unwrap()
}

pub(crate) fn pax_record(key: &str, value: &str) -> String {
    let payload = format!(" {key}={value}\n");
    let mut length = payload.len() + 1;
    loop {
        let record = format!("{length}{payload}");
        if record.len() == length {
            return record;
        }
        length = record.len();
    }
}

pub(crate) fn empty_extract() -> wxml_delivery::assets_lock::AssetExtract {
    wxml_delivery::assets_lock::AssetExtract {
        include: vec!["**".to_owned()],
        exclude: Vec::new(),
        executables: Vec::new(),
    }
}

pub(crate) fn digest_of(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
