//! In-memory fake typings assets for the adapter tests.
//!
//! The bundle is synthetic (`20` typings files) but the pin is computed with the
//! exact backend algorithm: each file's SHA-256 is rendered as
//! `<sha256>  <package-relative path>\n`, the lines are sorted by absolute path
//! and hashed once more. This lets
//! [`wxml_backend_tsrs::input::TypeAssets::new_typings_only`] accept the set
//! without weakening or reaching into the production `load_pinned_typings` pin.
//!
//! The TypeScript standard library is not part of the fake set: the in-process
//! engine supplies it. Tests that need the legacy full mode use the
//! `input::test_support` fixtures inside the backend crate.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use sha2::{Digest, Sha256};
use wxml_backend_tsrs::input::{AssetPath, TypeAssetFile, TypeAssets, TypeAssetsPin};

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

fn typings_files(root: &str) -> Vec<(String, String)> {
    (0..20_u32)
        .map(|index| {
            let relative = match index {
                0 => "index.d.ts".to_owned(),
                1 => "types/index.d.ts".to_owned(),
                _ => format!("types/fake{index}.d.ts"),
            };
            (
                format!("{root}/miniprogram-api-typings/{relative}"),
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

/// Builds a 20-file typings-only asset set whose pin matches exactly.
pub(super) fn fake_assets(typings_root: &str) -> Arc<TypeAssets> {
    let typings = typings_files(typings_root);
    let pin = TypeAssetsPin::typings_only(
        "5.3.1",
        20,
        byte_total(&typings),
        bundle_digest(&format!("{typings_root}/miniprogram-api-typings"), &typings),
    )
    .expect("fake typings-only pin");
    let files = typings
        .iter()
        .map(|(path, text)| asset_file(path, text))
        .collect();
    Arc::new(
        TypeAssets::new_typings_only(
            AssetPath::new(typings_root).expect("typings root"),
            files,
            pin,
        )
        .expect("fake typings assets"),
    )
}
