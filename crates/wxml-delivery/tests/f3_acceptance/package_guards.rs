//! F3-05: missing standard library / typings / engine-probe failure.
//! F3-10: manifest, per-file, license, extra and missing tampering.
//! F3-11: traversal, symlink, hardlink, PAX and decompression-budget archives.

use std::io::Write as _;

use flate2::Compression;
use flate2::write::GzEncoder;
use tar::{Builder, EntryType, Header};
use wxml_delivery::manifest::MAX_FILE_BYTES;
use wxml_delivery::manifest_v2::MINIPROGRAM_TYPINGS_ROOT;

use crate::support::{
    TestDir, Tripwires, VERSION_N1, VERSION_N2, add_extra_file, assert_contains, assert_failure,
    assert_no_activation, assert_previous_current, assert_success, combined, evidence,
    flip_payload_byte, install_n1, install_variant, installed_fingerprint, installer_bin,
    make_probe_failure, mutate_release, private_root, publish_archive_bytes, remove_required_file,
    remove_standard_lib, remove_typings, shared_channel, skip, state_bytes, tamper_manifest,
    workspace,
};

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
#[allow(clippy::too_many_lines)]
fn f3_05_missing_assets_and_probe_failure_fail_before_activation() {
    let Some(channel) = shared_channel("f3_05_missing_assets") else {
        skip(
            "F3-05",
            "pinned real assets are absent; set TSGO_NATIVE_TARBALL/TSGO_TYPINGS_TARBALL or WXML_REQUIRE_REAL_ENGINE=1",
        );
        return;
    };
    let base = TestDir::new("f3-05");
    let (work, _home, _tmp) = workspace(&base);
    let tripwires = Tripwires::install(base.path());
    let install_bin = installer_bin();
    let root = private_root(&base, "root");
    let install = install_n1(&install_bin, channel, &root, &tripwires, &work);
    assert_success(&install, "F3-05 baseline N1 install");
    assert_eq!(
        crate::support::active_version(&crate::support::open_root(&root)).as_deref(),
        Some(VERSION_N1)
    );
    let n1_fingerprint = installed_fingerprint(&root, VERSION_N1);
    let state_before = state_bytes(&root);
    println!(
        "F3-05 SCOPE: production package payload with one controlled mutation per case; \
         this is not a published defective release"
    );

    let missing_stdlib = mutate_release(
        base.path(),
        &channel.n2,
        "n2-missing-standard-lib",
        remove_standard_lib,
    );
    let output = install_variant(
        &install_bin,
        &missing_stdlib,
        VERSION_N2,
        &root,
        &tripwires,
        &work,
        "missing standard library",
    );
    assert_failure(&output, "missing standard library");
    assert_contains(
        &output,
        &["integrity", "type asset", "missing", "payload", "standard"],
    );
    assert_previous_current(&root, VERSION_N1, &n1_fingerprint, state_before.as_deref());
    evidence(
        "F3-05",
        "PASS",
        format!(
            "missing engine/lib/lib.es2020.d.ts rejected before activation; current={VERSION_N1}; \
             archive={}",
            missing_stdlib.sha256
        ),
    );

    let missing_typings = mutate_release(
        base.path(),
        &channel.n2,
        "n2-missing-typings-entry",
        remove_typings,
    );
    let output = install_variant(
        &install_bin,
        &missing_typings,
        VERSION_N2,
        &root,
        &tripwires,
        &work,
        "missing typings",
    );
    assert_failure(&output, "missing typings");
    assert_contains(&output, &["integrity", "typings", "missing", "payload"]);
    assert_previous_current(&root, VERSION_N1, &n1_fingerprint, state_before.as_deref());
    evidence(
        "F3-05",
        "PASS",
        format!(
            "missing {MINIPROGRAM_TYPINGS_ROOT}/index.d.ts rejected before activation; \
             current={VERSION_N1}; archive={}",
            missing_typings.sha256
        ),
    );

    let probe_failure = mutate_release(
        base.path(),
        &channel.n2,
        "n2-engine-probe-failure",
        |tree| make_probe_failure(tree, VERSION_N2),
    );
    let output = install_variant(
        &install_bin,
        &probe_failure,
        VERSION_N2,
        &root,
        &tripwires,
        &work,
        "engine probe failure",
    );
    assert_failure(&output, "engine probe failure");
    assert_contains(
        &output,
        &["engine version", "probe", "does not equal", "9.9.9"],
    );
    assert_previous_current(&root, VERSION_N1, &n1_fingerprint, state_before.as_deref());
    evidence(
        "F3-05",
        "PASS",
        format!(
            "tree-valid package with wrong engine probe output rejected before activation; \
             current={VERSION_N1}; archive={}",
            probe_failure.sha256
        ),
    );

    tripwires.assert_not_executed();
    println!(
        "F3-05: real-package-derived missing-stdlib, missing-typings and engine-probe failures \
         all left {VERSION_N1} current and byte-identical"
    );
}

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
#[allow(clippy::too_many_lines)]
fn f3_10_manifest_file_license_extra_missing_tamper_fails_before_activation() {
    let Some(channel) = shared_channel("f3_10_tamper_matrix") else {
        skip(
            "F3-10",
            "pinned real assets are absent; set TSGO_NATIVE_TARBALL/TSGO_TYPINGS_TARBALL or WXML_REQUIRE_REAL_ENGINE=1",
        );
        return;
    };
    let base = TestDir::new("f3-10");
    let (work, _home, _tmp) = workspace(&base);
    let tripwires = Tripwires::install(base.path());
    let install_bin = installer_bin();
    let root = private_root(&base, "root");
    println!(
        "F3-10 SCOPE: each archive starts from the production release and changes exactly one \
         manifest/payload/license/file-set property; the release index is recomputed so the \
         archive SRI itself passes"
    );

    let variants = [
        (
            "manifest",
            "manifest.json",
            mutate_release(
                base.path(),
                &channel.n1,
                "n1-tampered-manifest",
                tamper_manifest,
            ),
        ),
        (
            "payload",
            "typings/miniprogram-api-typings/index.d.ts",
            mutate_release(base.path(), &channel.n1, "n1-tampered-payload", |tree| {
                flip_payload_byte(tree, "typings/miniprogram-api-typings/index.d.ts");
            }),
        ),
        (
            "license",
            "engine/LICENSE",
            mutate_release(base.path(), &channel.n1, "n1-tampered-license", |tree| {
                flip_payload_byte(tree, "engine/LICENSE");
            }),
        ),
        (
            "extra",
            "engine/lib/extra-acceptance.d.ts",
            mutate_release(base.path(), &channel.n1, "n1-extra-file", add_extra_file),
        ),
        (
            "missing",
            "engine/lib/tsc.sig",
            mutate_release(base.path(), &channel.n1, "n1-missing-payload", |tree| {
                remove_required_file(tree, "engine/lib/tsc.sig");
            }),
        ),
        (
            "missing-license",
            "THIRD_PARTY_LICENSES.md",
            mutate_release(base.path(), &channel.n1, "n1-missing-license", |tree| {
                remove_required_file(tree, "THIRD_PARTY_LICENSES.md");
            }),
        ),
    ];

    for (case, changed, variant) in &variants {
        let output = install_variant(
            &install_bin,
            variant,
            VERSION_N1,
            &root,
            &tripwires,
            &work,
            &format!("tampered {case} install"),
        );
        assert_failure(&output, &format!("tampered {case}"));
        assert_contains(
            &output,
            &[
                "integrity",
                "sha-256",
                "manifest",
                "missing",
                "unexpected",
                "payload",
                "license",
            ],
        );
        assert_no_activation(&root);
        assert_eq!(
            crate::support::installed_versions(&crate::support::open_root(&root)),
            Vec::<String>::new(),
            "tampered {case} created an installed version"
        );
        evidence(
            "F3-10",
            "PASS",
            format!(
                "{case} tamper at `{changed}` rejected before activation; no current/versions/state; \
                 detail={}",
                combined(&output).trim().replace('\n', " | ")
            ),
        );
    }

    tripwires.assert_not_executed();
    println!(
        "F3-10: manifest/payload/license/extra/missing and missing-license tampering all failed \
         before any activation state existed"
    );
}

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
fn f3_11_malicious_real_archive_shapes_fail_before_activation() {
    let Some(channel) = shared_channel("f3_11_malicious_archives") else {
        skip(
            "F3-11",
            "pinned real assets are absent; set TSGO_NATIVE_TARBALL/TSGO_TYPINGS_TARBALL or WXML_REQUIRE_REAL_ENGINE=1",
        );
        return;
    };
    let base = TestDir::new("f3-11");
    let (work, _home, _tmp) = workspace(&base);
    let tripwires = Tripwires::install(base.path());
    let install_bin = installer_bin();
    let root = private_root(&base, "root");
    println!(
        "F3-11 SCOPE: the production installer fetches each archive through the real \
         `--from-dir` source path after its SRI/SHA-256/size record matches; only extraction safety \
         is under test"
    );

    let cases: [(&str, Vec<u8>, &[&str]); 5] = [
        (
            "traversal",
            traversal_archive(),
            &["unsafe", "traversal", "relative path", "path"],
        ),
        (
            "symlink",
            link_archive(EntryType::Symlink, "/etc/passwd"),
            &["unsafe", "symbolic link", "not a regular file", "link"],
        ),
        (
            "hardlink",
            link_archive(EntryType::Link, "engine/lib/tsc"),
            &["unsafe", "hard link", "not a regular file", "link"],
        ),
        (
            "pax",
            malformed_pax_archive(),
            &["pax", "unsafe", "archive"],
        ),
        (
            "budget",
            oversized_entry_archive(),
            &["size", "limit", "exceeds"],
        ),
    ];

    for (case, bytes, needles) in cases {
        let variant = publish_archive_bytes(
            base.path(),
            &channel.n1,
            &format!("malicious-{case}"),
            &bytes,
        );
        let output = install_variant(
            &install_bin,
            &variant,
            VERSION_N1,
            &root,
            &tripwires,
            &work,
            &format!("malicious {case} archive"),
        );
        assert_failure(&output, &format!("malicious {case} archive"));
        assert_contains(&output, needles);
        assert_no_activation(&root);
        evidence(
            "F3-11",
            "PASS",
            format!(
                "{case} archive rejected before activation; bytes={} sha256={}; detail={}",
                bytes.len(),
                variant.sha256,
                combined(&output).trim().replace('\n', " | ")
            ),
        );
    }

    tripwires.assert_not_executed();
    println!(
        "F3-11: traversal/symlink/hardlink/PAX/budget archives all used the real installer source \
         path and failed before activation"
    );
}

fn gzip_builder(build: impl FnOnce(&mut Builder<GzEncoder<Vec<u8>>>)) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = Builder::new(encoder);
    build(&mut builder);
    builder.finish().expect("finish malicious tar");
    let encoder = builder.into_inner().expect("finish malicious gzip");
    encoder.finish().expect("finish malicious gzip vector")
}

fn traversal_archive() -> Vec<u8> {
    gzip_builder(|builder| {
        let payload = b"escape\n";
        let mut header = Header::new_gnu();
        header.set_size(u64::try_from(payload.len()).expect("payload size"));
        header.set_mode(0o644);
        header.as_mut_bytes()[..b"../escape".len()].copy_from_slice(b"../escape");
        header.set_cksum();
        builder
            .append(&header, &payload[..])
            .expect("append traversal entry");
    })
}

fn link_archive(kind: EntryType, target: &str) -> Vec<u8> {
    gzip_builder(|builder| {
        let mut header = Header::new_gnu();
        header.set_entry_type(kind);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_path("link").expect("link path");
        header.set_link_name(target).expect("link target");
        header.set_cksum();
        builder.append(&header, &[][..]).expect("append link entry");
    })
}

fn malformed_pax_archive() -> Vec<u8> {
    gzip_builder(|builder| {
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::XHeader);
        header.set_size(4);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_path("pax").expect("pax path");
        header.set_cksum();
        builder
            .append(&header, &b"3 \xc3\xa9"[..])
            .expect("append malformed pax");
    })
}

fn oversized_entry_archive() -> Vec<u8> {
    let mut header = Header::new_gnu();
    header
        .set_path("engine/lib/oversized.d.ts")
        .expect("entry path");
    header.set_mode(0o644);
    header.set_size(MAX_FILE_BYTES + 1);
    header.set_cksum();
    let mut raw = header.as_bytes().to_vec();
    raw.extend_from_slice(&[0_u8; 1024]);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw).expect("write oversized header");
    encoder.finish().expect("finish oversized archive")
}
