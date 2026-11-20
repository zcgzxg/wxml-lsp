//! F3-13 platform/ABI measurement on the real release package.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use wxml_delivery::manifest::sha256_file;
use wxml_delivery::manifest_v2::{ENGINE_BINARY_PATH, ENGINE_VERSION, read_manifest_v2};

use crate::real_asset_support::{TestDir, mode_of, run_with_timeout};
use crate::support::{
    VERSION_N1, build_release, expected, provision_real_assets, resolve_real_inputs,
    write_input_tree,
};

fn tool(program: &str, args: &[&str]) -> Output {
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    run_with_timeout(
        &mut command,
        std::time::Duration::from_secs(30),
        &format!("{program} {}", args.join(" ")),
    )
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_elf(path: &Path, expected_type: &[&str]) {
    let header = tool("readelf", &["-h", path.to_str().expect("UTF-8 path")]);
    assert!(
        header.status.success(),
        "readelf -h failed for {}",
        path.display()
    );
    let header = text(&header);
    assert!(
        header.contains("ELF64"),
        "{} is not ELF64: {header}",
        path.display()
    );
    assert!(
        header.contains("Advanced Micro Devices X86-64"),
        "{} is not x86-64: {header}",
        path.display()
    );
    assert!(
        expected_type.iter().any(|kind| header.contains(kind)),
        "{} has an unexpected ELF type: {header}",
        path.display()
    );
    println!(
        "ABI-ELF: {} type={} class=ELF64 machine=x86-64",
        path.display(),
        expected_type.join("|")
    );
}

fn dynamic_needed(path: &Path) -> String {
    let dynamic = tool("readelf", &["-d", path.to_str().expect("UTF-8 path")]);
    text(&dynamic)
}

fn max_glibc(path: &Path) -> Option<(u32, u32)> {
    let version_info = tool(
        "readelf",
        &["--version-info", path.to_str().expect("UTF-8 path")],
    );
    let text = text(&version_info);
    let mut maximum = None;
    for token in text.split(|character: char| {
        !(character.is_ascii_alphanumeric() || character == '_' || character == '.')
    }) {
        let Some(rest) = token.strip_prefix("GLIBC_") else {
            continue;
        };
        let mut parts = rest.split('.');
        let Some(major) = parts.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let Some(minor) = parts.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        if maximum.is_none_or(|current| (major, minor) > current) {
            maximum = Some((major, minor));
        }
    }
    maximum
}

fn runtime_glibc() -> (u32, u32) {
    let output = tool("ldd", &["--version"]);
    let text = text(&output);
    let first = text.lines().next().unwrap_or_default();
    assert!(
        first.contains("GNU libc") || first.contains("GLIBC"),
        "host is not a glibc userland: {first}"
    );
    let version = first
        .split_whitespace()
        .next_back()
        .expect("ldd version token");
    let mut parts = version.split('.');
    let major = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("cannot parse glibc version from `{first}`"));
    let minor = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("cannot parse glibc version from `{first}`"));
    (major, minor)
}

fn assert_dynamic_rust_binary(path: &Path) {
    let dynamic = dynamic_needed(path);
    assert!(
        dynamic.contains("libc.so.6"),
        "{} does not link glibc: {dynamic}",
        path.display()
    );
    let ldd = tool("ldd", &[path.to_str().expect("UTF-8 path")]);
    let text = text(&ldd);
    assert!(
        text.contains("libc.so.6"),
        "{} has no resolved libc: {text}",
        path.display()
    );
    assert!(
        !text.contains("not found"),
        "{} has unresolved libraries: {text}",
        path.display()
    );
    let required = max_glibc(path).unwrap_or_else(|| {
        panic!(
            "{} has no GLIBC_* version requirement; it may be statically linked",
            path.display()
        )
    });
    let runtime = runtime_glibc();
    assert!(
        required <= runtime,
        "{} requires GLIBC_{}.{} but the host provides {}.{}",
        path.display(),
        required.0,
        required.1,
        runtime.0,
        runtime.1
    );
    println!(
        "ABI-RUST: {} sha256={} size={} mode={:o} glibc_required={}.{} host_glibc={}.{}",
        path.display(),
        sha256_file(path).expect("binary sha256"),
        fs::metadata(path).expect("binary metadata").len(),
        mode_of(path),
        required.0,
        required.1,
        runtime.0,
        runtime.1
    );
}

fn assert_static_engine(path: &Path) {
    let dynamic = dynamic_needed(path);
    assert!(
        !dynamic.contains("NEEDED"),
        "the pinned engine unexpectedly has dynamic NEEDED entries: {dynamic}"
    );
    let ldd = tool("ldd", &[path.to_str().expect("UTF-8 path")]);
    let text = text(&ldd);
    assert!(
        text.contains("not a dynamic executable") || text.contains("statically linked"),
        "engine ldd did not report a static executable: {text}"
    );
    println!(
        "ABI-ENGINE: {} sha256={} size={} mode={:o} static=yes",
        path.display(),
        sha256_file(path).expect("engine sha256"),
        fs::metadata(path).expect("engine metadata").len(),
        mode_of(path)
    );
}

fn assert_manifest_bytes(release: &Path, manifest: &wxml_delivery::manifest_v2::ManifestV2) {
    for path in ["bin/wxml-lsp", "bin/wxml-lsp-install", ENGINE_BINARY_PATH] {
        let entry = manifest
            .files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("manifest is missing `{path}`"));
        let absolute = release.join(path);
        assert_eq!(
            sha256_file(&absolute).expect("hash release file"),
            entry.sha256,
            "manifest hash mismatch for {path}"
        );
        assert_eq!(
            fs::metadata(&absolute)
                .expect("release file metadata")
                .len(),
            entry.size
        );
        assert_eq!(entry.mode, "0755", "{path} is not executable");
        assert_eq!(mode_of(&absolute), 0o755, "{path} on-disk mode");
    }
}

fn packager_command(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"));
    command
        .args(args)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null());
    command
}

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
fn real_release_abi_and_platform_evidence() {
    let Some(inputs) = resolve_real_inputs("real_release_abi") else {
        return;
    };
    assert_eq!(
        wxml_delivery::paths::host_platform().expect("host platform"),
        "linux-x86_64"
    );
    let base = TestDir::new("f3-abi");
    let provisioned = provision_real_assets(&base, &inputs);
    let tree = write_input_tree(&base, &inputs, &provisioned);
    let release = build_release(
        &base.path().join("release"),
        &tree,
        &provisioned,
        VERSION_N1,
        true,
    );
    let manifest =
        read_manifest_v2(&release.release_dir, expected(VERSION_N1)).expect("read manifest");
    assert_manifest_bytes(&release.release_dir, &manifest);

    let lsp = release.release_dir.join("bin/wxml-lsp");
    let install = release.release_dir.join("bin/wxml-lsp-install");
    let engine = release.release_dir.join(ENGINE_BINARY_PATH);
    assert_elf(
        &lsp,
        &[
            "DYN (Position-Independent Executable",
            "DYN (Shared object",
            "EXEC",
        ],
    );
    assert_elf(
        &install,
        &[
            "DYN (Position-Independent Executable",
            "DYN (Shared object",
            "EXEC",
        ],
    );
    assert_elf(&engine, &["EXEC"]);
    assert_dynamic_rust_binary(&lsp);
    assert_dynamic_rust_binary(&install);
    assert_static_engine(&engine);

    let file = tool(
        "file",
        &[
            lsp.to_str().expect("UTF-8 lsp"),
            install.to_str().expect("UTF-8 install"),
            engine.to_str().expect("UTF-8 engine"),
        ],
    );
    println!("ABI-FILE: {}", text(&file).replace('\n', " | "));

    let mut version = Command::new(&engine);
    version
        .arg("--version")
        .current_dir(engine.parent().expect("engine parent"))
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    let output = run_with_timeout(
        &mut version,
        std::time::Duration::from_secs(10),
        "engine --version",
    );
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim_end(),
        format!("Version {ENGINE_VERSION}")
    );
    println!(
        "ABI-VERSION: engine `lib/tsc --version` = Version {ENGINE_VERSION}; host glibc = {}.{}",
        runtime_glibc().0,
        runtime_glibc().1
    );

    let wrong = base.path().join("wrong-platform");
    let mut command = packager_command(&[
        "native",
        "--input",
        tree.to_str().expect("UTF-8 tree"),
        "--output",
        wrong.to_str().expect("UTF-8 wrong"),
        "--assets-lock",
        provisioned.lock_path.to_str().expect("UTF-8 lock"),
        "--cache",
        provisioned.cache.to_str().expect("UTF-8 cache"),
        "--release-version",
        VERSION_N1,
        "--platform",
        "linux-aarch64",
        "--built-at",
        "2026-11-19T00:00:00Z",
    ]);
    let output = run_with_timeout(
        &mut command,
        std::time::Duration::from_secs(60),
        "wrong platform build",
    );
    assert!(
        !output.status.success(),
        "packager accepted linux-aarch64: status={:?}",
        output.status
    );
    let combined = text(&output);
    assert!(
        combined.contains("support only platform `linux-x86_64`")
            || combined.contains("linux-x86_64"),
        "wrong-platform rejection did not name the supported platform: {combined}"
    );
    assert!(
        !wrong.exists() || fs::read_dir(&wrong).expect("wrong dir").next().is_none(),
        "wrong-platform build wrote output"
    );
    println!(
        "F3-13: readelf/ldd/GLIBC measured; linux-aarch64 rejected before any output write; \
         musl is not a supported artifact"
    );
}
