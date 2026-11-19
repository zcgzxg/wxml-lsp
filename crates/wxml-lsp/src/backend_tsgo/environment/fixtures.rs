//! In-memory fake type assets for the adapter tests.
//!
//! The bundles are synthetic (`116` standard-library files and `20` typings
//! files), but the pin is computed with the exact backend algorithm: each
//! file's SHA-256 is rendered as `<sha256>  <root-relative path>\n`, the lines
//! are sorted by absolute path and hashed once more. This lets
//! [`wxml_backend_tsgo::input::TypeAssets::new`] accept the set without
//! weakening or reaching into the production `load_pinned` pin.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use sha2::{Digest, Sha256};
use wxml_backend_tsgo::input::{AssetPath, TypeAssetFile, TypeAssets, TypeAssetsPin};

/// Directory that contains the `miniprogram-api-typings` package.
pub(super) const TYPINGS_ROOT: &str = "/typings";

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        write!(text, "{byte:02x}").expect("hex formatting");
    }
    text
}

fn text_sha(text: &str) -> String {
    hex_lower(&Sha256::digest(text.as_bytes()))
}

/// Recomputes the backend bundle digest over `(absolute path, text)` pairs.
fn bundle_digest(root: &str, files: &[(String, String)]) -> String {
    let mut entries: Vec<(String, String)> = files
        .iter()
        .map(|(path, text)| {
            let relative = path
                .strip_prefix(root)
                .and_then(|rest| rest.strip_prefix('/'))
                .unwrap_or(path)
                .to_owned();
            (path.clone(), format!("{}  {relative}\n", text_sha(text)))
        })
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (_, line) in entries {
        hasher.update(line.as_bytes());
    }
    hex_lower(&hasher.finalize())
}

fn asset_file(path: &str, text: &str) -> TypeAssetFile {
    TypeAssetFile::new(
        AssetPath::new(path).expect("asset path"),
        text.to_owned(),
        text_sha(text),
        u64::try_from(text.len()).expect("text size"),
    )
    .expect("asset file")
}

/// Builds a 116 + 20 in-memory asset set whose pin matches exactly.
fn standard_lib_files(root: &str) -> Vec<(String, String)> {
    (0..116_u32)
        .map(|index| {
            let name = if index == 0 {
                "lib.d.ts".to_owned()
            } else {
                format!("lib.fake{index}.d.ts")
            };
            (
                format!("{root}/{name}"),
                format!("// fake standard library {index}\n"),
            )
        })
        .collect()
}

fn typings_files() -> Vec<(String, String)> {
    (0..20_u32)
        .map(|index| {
            let relative = match index {
                0 => "index.d.ts".to_owned(),
                1 => "types/index.d.ts".to_owned(),
                _ => format!("types/fake{index}.d.ts"),
            };
            (
                format!("{TYPINGS_ROOT}/miniprogram-api-typings/{relative}"),
                format!("// fake typings {index}\n"),
            )
        })
        .collect()
}

fn byte_total(files: &[(String, String)]) -> u64 {
    files
        .iter()
        .map(|(_, text)| u64::try_from(text.len()).expect("text size"))
        .sum()
}

pub(super) fn fake_assets(standard_lib_root: &str) -> Arc<TypeAssets> {
    let standard = standard_lib_files(standard_lib_root);
    let typings = typings_files();
    let pin = TypeAssetsPin::new(
        "5.3.1",
        116,
        byte_total(&standard),
        bundle_digest(standard_lib_root, &standard),
        20,
        byte_total(&typings),
        bundle_digest(&format!("{TYPINGS_ROOT}/miniprogram-api-typings"), &typings),
    )
    .expect("fake pin");
    let files = standard
        .iter()
        .chain(typings.iter())
        .map(|(path, text)| asset_file(path, text))
        .collect();
    Arc::new(
        TypeAssets::new(
            AssetPath::new(standard_lib_root).expect("standard root"),
            AssetPath::new(TYPINGS_ROOT).expect("typings root"),
            files,
            pin,
        )
        .expect("fake assets"),
    )
}
