//! Independent digest implementations used as test oracles.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::Value;
use sha2::{Digest, Sha256};

use wxml_delivery::manifest::FileEntry;

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn mode_octal(mode: &str) -> String {
    let bits = u32::from_str_radix(mode, 8).expect("octal mode");
    format!("{bits:o}")
}

pub(crate) fn tree_digest_pairs(rows: &[(String, String, String)], root: &str) -> String {
    let prefix = if root.is_empty() {
        String::new()
    } else {
        format!("{root}/")
    };
    let mut directories = BTreeSet::new();
    let mut files = Vec::new();
    for (path, mode, sha256) in rows {
        let relative = if prefix.is_empty() {
            path.as_str()
        } else {
            path.strip_prefix(&prefix)
                .unwrap_or_else(|| panic!("`{path}` is outside `{root}`"))
        };
        for (index, _) in relative.match_indices('/') {
            directories.insert(relative[..index].to_owned());
        }
        files.push((relative.to_owned(), mode_octal(mode), sha256.clone()));
    }
    let mut lines = String::new();
    let mut directories: Vec<String> = directories.into_iter().collect();
    directories.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    for directory in directories {
        lines.push_str("D 755 ");
        lines.push_str(&directory);
        lines.push('\n');
    }
    files.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    for (path, mode, sha256) in files {
        lines.push_str("F ");
        lines.push_str(&mode);
        lines.push(' ');
        lines.push_str(&sha256);
        lines.push(' ');
        lines.push_str(&path);
        lines.push('\n');
    }
    sha256_hex(lines.as_bytes())
}

pub(crate) fn tree_digest(files: &[Value], root: &str) -> String {
    let rows: Vec<(String, String, String)> = files
        .iter()
        .map(|file| {
            (
                file["path"].as_str().expect("path").to_owned(),
                file["mode"].as_str().expect("mode").to_owned(),
                file["sha256"].as_str().expect("sha256").to_owned(),
            )
        })
        .collect();
    tree_digest_pairs(&rows, root)
}

pub(crate) fn tree_digest_entries(files: &[FileEntry], root: &str) -> String {
    let rows: Vec<(String, String, String)> = files
        .iter()
        .map(|file| (file.path.clone(), file.mode.clone(), file.sha256.clone()))
        .collect();
    tree_digest_pairs(&rows, root)
}

fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    glob_components(&pattern, &path)
}

fn glob_components(pattern: &[&str], path: &[&str]) -> bool {
    let Some((first, rest)) = pattern.split_first() else {
        return path.is_empty();
    };
    if *first == "**" {
        return (0..=path.len()).any(|skip| glob_components(rest, &path[skip..]));
    }
    let Some((head, tail)) = path.split_first() else {
        return false;
    };
    wildcard(first, head) && glob_components(rest, tail)
}

fn wildcard(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let Some(first) = parts.next() else {
        return true;
    };
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    if parts.is_empty() {
        return rest.is_empty();
    }
    for (index, part) in parts.iter().enumerate() {
        if index + 1 == parts.len() {
            return rest.ends_with(part);
        }
        let Some(position) = rest.find(part) else {
            return false;
        };
        rest = &rest[position + part.len()..];
    }
    rest.is_empty()
}

pub(crate) fn bundle_digest(files: &[Value], root: &str, include: &[String]) -> (u64, u64, String) {
    let prefix = format!("{root}/");
    let mut matched = Vec::new();
    let mut total_bytes = 0_u64;
    for file in files {
        let path = file["path"].as_str().expect("path");
        let Some(relative) = path.strip_prefix(&prefix) else {
            continue;
        };
        if !include.iter().any(|pattern| glob_match(pattern, relative)) {
            continue;
        }
        total_bytes += file["size"].as_u64().expect("size");
        matched.push((path, relative, file["sha256"].as_str().expect("sha256")));
    }
    matched.sort_by(|left, right| left.0.cmp(right.0));
    let mut lines = String::new();
    for (_, relative, sha256) in &matched {
        lines.push_str(sha256);
        lines.push_str("  ");
        lines.push_str(relative);
        lines.push('\n');
    }
    let count = u64::try_from(matched.len()).expect("bundle count fits u64");
    (count, total_bytes, sha256_hex(lines.as_bytes()))
}

pub(crate) fn table_count(files: &[Value]) -> u64 {
    u64::try_from(files.len()).expect("table count fits u64")
}

pub(crate) fn table_bytes(files: &[Value]) -> u64 {
    files
        .iter()
        .map(|file| file["size"].as_u64().expect("size"))
        .sum()
}
