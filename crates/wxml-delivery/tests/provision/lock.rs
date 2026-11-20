//! Lock, argument, platform, plan and no-subprocess tests.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use serde_json::json;
use wxml_delivery::assets_lock::MAX_ASSETS_LOCK_BYTES;
use wxml_delivery::error::DeliveryError;
use wxml_delivery::provision::{plan, provision, require_supported_host};

use crate::support::{TempDir, mode_of, provision_options, write_repo_lock};

fn write_lock(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    fs::create_dir_all(dir.join(".git")).unwrap();
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn valid_bytes() -> Vec<u8> {
    serde_json::to_vec_pretty(&crate::lock_support::valid_value()).unwrap()
}

#[test]
fn rejects_oversized_malformed_and_drifted_locks_before_cache_creation() {
    let temp = TempDir::new("bad-locks");
    let cache = temp.path().join("cache");

    let oversized_dir = temp.path().join("oversized");
    let oversized = write_lock(
        &oversized_dir,
        "assets.lock.json",
        &vec![b' '; usize::try_from(MAX_ASSETS_LOCK_BYTES).unwrap() + 1],
    );
    let error = provision(&provision_options(&oversized, &cache)).unwrap_err();
    assert!(matches!(error, DeliveryError::SizeLimit(_)), "{error:?}");
    assert!(!cache.exists(), "lock failure must not create the cache");

    let malformed = write_lock(&temp.path().join("malformed"), "assets.lock.json", b"{");
    let error = provision(&provision_options(&malformed, &cache)).unwrap_err();
    assert!(
        matches!(error, DeliveryError::InvalidPackage(_)),
        "{error:?}"
    );
    assert!(!cache.exists());

    let mut schema = crate::lock_support::valid_value();
    schema["schemaVersion"] = json!(2);
    let schema = write_lock(
        &temp.path().join("schema"),
        "assets.lock.json",
        &serde_json::to_vec(&schema).unwrap(),
    );
    let error = provision(&provision_options(&schema, &cache)).unwrap_err();
    assert!(
        error.to_string().contains("unsupported assets lock schema"),
        "{error}"
    );
    assert!(!cache.exists());

    let mut drift = crate::lock_support::valid_value();
    let assets = drift["assets"].as_array_mut().unwrap();
    let engine = assets
        .iter_mut()
        .find(|asset| asset["name"] == crate::fixtures::ENGINE_ASSET)
        .unwrap();
    engine["license"] = json!("MIT");
    let drift = write_lock(
        &temp.path().join("drift"),
        "assets.lock.json",
        &serde_json::to_vec(&drift).unwrap(),
    );
    let error = provision(&provision_options(&drift, &cache)).unwrap_err();
    assert!(
        error.to_string().contains("does not match the frozen"),
        "{error}"
    );
    assert!(!cache.exists());
}

#[test]
fn lock_must_be_assets_lock_json_at_a_repository_root() {
    let temp = TempDir::new("lock-path");
    let cache = temp.path().join("cache");
    let bytes = valid_bytes();

    let wrong_name = write_lock(&temp.path().join("renamed"), "lock.json", &bytes);
    let error = provision(&provision_options(&wrong_name, &cache)).unwrap_err();
    assert!(error.to_string().contains("assets.lock.json"), "{error}");

    let no_root = temp.path().join("plain");
    fs::create_dir_all(&no_root).unwrap();
    fs::write(no_root.join("assets.lock.json"), &bytes).unwrap();
    let error = provision(&provision_options(
        &no_root.join("assets.lock.json"),
        &cache,
    ))
    .unwrap_err();
    assert!(error.to_string().contains("repository root"), "{error}");

    let repo = temp.path().join("symlink-root");
    let real = write_lock(&repo, "assets.lock.json", &bytes);
    let link = temp.path().join("linked.lock.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let plan = plan(&provision_options(&link, &cache)).unwrap();
    assert_eq!(plan.lock_sha256, crate::fixtures::VALID_LOCK_SHA256);
    assert!(!cache.exists());

    let outside = temp.path().join("outside.lock.json");
    fs::write(&outside, &bytes).unwrap();
    let wrong_target = temp.path().join("wrong-target.json");
    std::os::unix::fs::symlink(&outside, &wrong_target).unwrap();
    let error = provision(&provision_options(&wrong_target, &cache)).unwrap_err();
    assert!(error.to_string().contains("assets.lock.json"), "{error}");

    let cargo_root = temp.path().join("cargo-root");
    fs::create_dir_all(&cargo_root).unwrap();
    fs::write(cargo_root.join("Cargo.toml"), b"[package]\n").unwrap();
    fs::write(cargo_root.join("assets.lock.json"), &bytes).unwrap();
    let cargo_plan = wxml_delivery::provision::plan(&provision_options(
        &cargo_root.join("assets.lock.json"),
        &cache,
    ))
    .unwrap();
    assert_eq!(cargo_plan.lock_sha256, crate::fixtures::VALID_LOCK_SHA256);
    assert!(!cache.exists());
}

#[test]
fn rejects_every_host_platform_except_linux_x64() {
    assert!(matches!(
        require_supported_host("macos", "x86_64"),
        Err(DeliveryError::UnsupportedPlatform(_))
    ));
    assert!(matches!(
        require_supported_host("linux", "aarch64"),
        Err(DeliveryError::UnsupportedPlatform(_))
    ));
    assert!(matches!(
        require_supported_host("windows", "x86_64"),
        Err(DeliveryError::UnsupportedPlatform(_))
    ));
    require_supported_host("linux", "x86_64").unwrap();
}

#[test]
fn allow_download_is_rejected_without_touching_the_cache() {
    let temp = TempDir::new("allow-download");
    let lock = write_repo_lock(temp.path());
    let cache = temp.path().join("cache");
    let mut options = provision_options(&lock, &cache);
    options.allow_download = true;
    options.offline = false;

    let error = plan(&options).unwrap_err();
    assert!(
        matches!(error, DeliveryError::InvalidArgument(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("not implemented"), "{error}");
    let error = provision(&options).unwrap_err();
    assert!(error.to_string().contains("not implemented"), "{error}");
    assert!(!cache.exists());
}

#[test]
fn print_plan_reports_missing_sources_and_writes_nothing() {
    let temp = TempDir::new("print-plan");
    let lock = write_repo_lock(temp.path());
    let cache = temp.path().join("cache");
    let options = provision_options(&lock, &cache);

    let plan = plan(&options).unwrap();
    assert_eq!(plan.lock_sha256, crate::fixtures::VALID_LOCK_SHA256);
    assert_eq!(plan.assets.len(), 2);
    assert_eq!(plan.assets[0].name, crate::fixtures::TYPINGS_ASSET);
    assert_eq!(plan.assets[1].name, crate::fixtures::ENGINE_ASSET);
    for asset in &plan.assets {
        assert!(!asset.cache_hit);
        assert_eq!(asset.source, None);
        assert!(
            Path::new(&asset.tarball_file_name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tgz")),
            "{}",
            asset.tarball_file_name
        );
    }
    assert!(!cache.exists(), "--print-plan must not create the cache");

    let from_dir = temp.path().join("from-dir");
    fs::create_dir_all(&from_dir).unwrap();
    for asset in &plan.assets {
        fs::write(
            from_dir.join(&asset.tarball_file_name),
            b"not verified by plan",
        )
        .unwrap();
    }
    let mut with_source = provision_options(&lock, &cache);
    with_source.from_dir = Some(from_dir);
    let source_plan = wxml_delivery::provision::plan(&with_source).unwrap();
    for asset in &source_plan.assets {
        assert!(!asset.cache_hit);
        assert_eq!(
            asset.source,
            Some(wxml_delivery::provision::SourceKind::FromDir)
        );
    }
    assert!(
        !cache.exists(),
        "--print-plan with --from-dir must not write"
    );

    let mut planned = options;
    planned.print_plan = true;
    let error = provision(&planned).unwrap_err();
    assert!(error.to_string().contains("--print-plan"), "{error}");
    assert!(!cache.exists());
}

#[test]
fn offline_without_cache_or_from_dir_fails_closed() {
    let temp = TempDir::new("offline");
    let lock = write_repo_lock(temp.path());
    let cache = temp.path().join("cache");
    let mut options = provision_options(&lock, &cache);
    options.offline = true;

    let error = provision(&options).unwrap_err();
    assert!(
        matches!(error, DeliveryError::InvalidArgument(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("--from-dir"), "{error}");
    assert_eq!(mode_of(&cache), 0o700, "new cache must be private");
    let assets = cache.join("assets");
    assert_eq!(mode_of(&assets), 0o700);
    assert_eq!(fs::read_dir(&assets).unwrap().count(), 0);
}

#[test]
fn from_dir_requires_the_exact_file_name_and_a_regular_file() {
    let temp = TempDir::new("from-dir");
    let lock = write_repo_lock(temp.path());
    let cache = temp.path().join("cache");

    let wrong = temp.path().join("wrong-name");
    fs::create_dir_all(&wrong).unwrap();
    fs::write(wrong.join("wrong.tgz"), b"not the locked tarball").unwrap();
    let mut options = provision_options(&lock, &cache);
    options.from_dir = Some(wrong);
    let error = provision(&options).unwrap_err();
    assert!(error.to_string().contains("--from-dir"), "{error}");
    assert!(
        error
            .to_string()
            .contains("miniprogram-api-typings-5.3.1.tgz"),
        "{error}"
    );

    let linked = temp.path().join("linked");
    fs::create_dir_all(&linked).unwrap();
    let target = temp.path().join("target.tgz");
    fs::write(&target, b"bytes").unwrap();
    std::os::unix::fs::symlink(&target, linked.join("miniprogram-api-typings-5.3.1.tgz")).unwrap();
    let mut options = provision_options(&lock, &cache);
    options.from_dir = Some(linked);
    let error = provision(&options).unwrap_err();
    assert!(error.to_string().contains("symlink"), "{error}");

    let mut options = provision_options(&lock, &cache);
    options.from_dir = Some(std::path::PathBuf::from("relative-dir"));
    let error = provision(&options).unwrap_err();
    assert!(error.to_string().contains("absolute"), "{error}");

    let hard = temp.path().join("hard-linked");
    fs::create_dir_all(&hard).unwrap();
    let real = hard.join("real.tgz");
    fs::write(&real, b"bytes").unwrap();
    fs::hard_link(&real, hard.join("miniprogram-api-typings-5.3.1.tgz")).unwrap();
    let mut options = provision_options(&lock, &cache);
    options.from_dir = Some(hard);
    let error = provision(&options).unwrap_err();
    assert!(error.to_string().contains("hard link"), "{error}");

    let file = temp.path().join("not-a-dir");
    fs::write(&file, b"x").unwrap();
    let mut options = provision_options(&lock, &cache);
    options.from_dir = Some(file);
    let error = provision(&options).unwrap_err();
    assert!(error.to_string().contains("directory"), "{error}");
}

#[test]
fn cache_must_be_absolute_and_outside_versions() {
    let temp = TempDir::new("cache-path");
    let lock = write_repo_lock(temp.path());

    let mut options = provision_options(&lock, &temp.path().join("cache"));
    options.cache = std::path::PathBuf::from("relative-cache");
    let error = provision(&options).unwrap_err();
    assert!(error.to_string().contains("absolute"), "{error}");

    let mut options = provision_options(&lock, &temp.path().join("cache"));
    options.cache = temp.path().join("versions").join("cache");
    let error = provision(&options).unwrap_err();
    assert!(error.to_string().contains("versions"), "{error}");

    let file = temp.path().join("cache-file");
    fs::write(&file, b"x").unwrap();
    let options = provision_options(&lock, &file);
    let error = provision(&options).unwrap_err();
    assert!(error.to_string().contains("directory"), "{error}");
}

#[test]
fn provisioning_sources_never_start_a_process() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/provision");
    let mut stack = vec![source_root];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let text = fs::read_to_string(&path).unwrap();
                assert!(
                    !text.contains("Command") && !text.contains("std::process"),
                    "{} must not reference process spawning",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn cli_plan_runs_without_node_or_npm_on_path() {
    let temp = TempDir::new("tripwire");
    let lock = write_repo_lock(temp.path());
    let cache = temp.path().join("cache");
    let shim = temp.path().join("shims");
    let marker = temp.path().join("spawn-marker");
    fs::create_dir_all(&shim).unwrap();
    for name in ["node", "npm", "npx", "pnpm", "yarn"] {
        let path = shim.join(name);
        fs::write(
            &path,
            format!("#!/bin/sh\necho {name} >> \"$WXML_TRIPWIRE_MARKER\"\n"),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"))
        .arg("provision")
        .arg("--lock")
        .arg(&lock)
        .arg("--cache")
        .arg(&cache)
        .arg("--print-plan")
        .env("PATH", &shim)
        .env("WXML_TRIPWIRE_MARKER", &marker)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("provision plan"),
        "unexpected stdout"
    );
    assert!(!marker.exists(), "provisioning must not execute node/npm");
    assert!(!cache.exists(), "print-plan must not create the cache");

    let rejected = Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"))
        .arg("provision")
        .arg("--lock")
        .arg(&lock)
        .arg("--cache")
        .arg(&cache)
        .arg("--allow-download")
        .env("PATH", &shim)
        .env("WXML_TRIPWIRE_MARKER", &marker)
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("not implemented"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(!marker.exists());
}

fn make_private_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn plan_rejects_invalid_cache_entries() {
    let temp = TempDir::new("plan-invalid");
    let lock = write_repo_lock(temp.path());
    let cache = temp.path().join("cache");
    let options = provision_options(&lock, &cache);
    let initial = plan(&options).unwrap();
    let first = &initial.assets[0];
    let entry = cache
        .join("assets")
        .join(&first.name)
        .join(&first.cache_key);

    make_private_dir(&cache);
    fs::create_dir_all(&entry).unwrap();
    make_private_dir(&cache.join("assets"));
    let error = plan(&options).unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
    assert!(
        error.to_string().contains("incomplete or unsafe"),
        "{error}"
    );
    fs::remove_dir_all(&entry).unwrap();

    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    let target = temp.path().join("entry-target");
    fs::create_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(&target, &entry).unwrap();
    let error = plan(&options).unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
    fs::remove_file(&entry).unwrap();

    fs::create_dir_all(&entry).unwrap();
    fs::write(entry.join("provenance.json"), b"{}").unwrap();
    let hit = plan(&options).unwrap();
    assert!(hit.assets[0].cache_hit);
    assert_eq!(
        hit.assets[0].source,
        Some(wxml_delivery::provision::SourceKind::Cache)
    );
    fs::remove_file(entry.join("provenance.json")).unwrap();
    fs::create_dir(entry.join("provenance.json")).unwrap();
    let error = plan(&options).unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
}

#[test]
fn plan_all_hits_ignores_an_unusable_from_dir() {
    let temp = TempDir::new("plan-all-hit");
    let lock = write_repo_lock(temp.path());
    let cache = temp.path().join("cache");
    make_private_dir(&cache);
    let options = provision_options(&lock, &cache);
    let initial = plan(&options).unwrap();
    for asset in &initial.assets {
        let entry = cache
            .join("assets")
            .join(&asset.name)
            .join(&asset.cache_key);
        fs::create_dir_all(&entry).unwrap();
        fs::write(entry.join("provenance.json"), b"plan only checks presence").unwrap();
    }
    make_private_dir(&cache.join("assets"));
    let mut all_hit = options;
    all_hit.from_dir = Some(temp.path().join("missing-from-dir"));
    let plan = plan(&all_hit).unwrap();
    assert!(plan.assets.iter().all(|asset| asset.cache_hit));
    assert!(
        plan.assets
            .iter()
            .all(|asset| asset.source == Some(wxml_delivery::provision::SourceKind::Cache))
    );
}

#[test]
fn preexisting_insecure_cache_directories_are_rejected() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new("cache-private");
    let lock = write_repo_lock(temp.path());

    let world = temp.path().join("cache-0777");
    fs::create_dir_all(&world).unwrap();
    fs::set_permissions(&world, fs::Permissions::from_mode(0o777)).unwrap();
    let error = plan(&provision_options(&lock, &world)).unwrap_err();
    assert!(
        matches!(error, DeliveryError::InvalidArgument(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("private"), "{error}");

    let cache = temp.path().join("cache");
    make_private_dir(&cache);
    let assets = cache.join("assets");
    fs::create_dir_all(&assets).unwrap();
    fs::set_permissions(&assets, fs::Permissions::from_mode(0o777)).unwrap();
    let error = provision(&provision_options(&lock, &cache)).unwrap_err();
    assert!(
        matches!(error, DeliveryError::InvalidArgument(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("private"), "{error}");
}
