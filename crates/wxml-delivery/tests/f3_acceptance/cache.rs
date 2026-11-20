//! F3-12: a real, verified native archive cache drives `upgrade` and
//! `rollback` with no source and no network; damaged cache state fails closed.
//!
//! The cache is the production content-addressed cache below
//! `<root>/cache/v2`. N1 is cached by a real `install --from-dir`; the N2 entry
//! is warmed through the production [`acquire_v2`] cache path without
//! activating it (the same on-disk state a failed install after caching leaves
//! behind). The `upgrade`/`rollback`/corruption commands are the real
//! `wxml-lsp-install` binary.

use std::fs;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use wxml_delivery::acquire_v2::{
    AcquireModeV2, AcquireOptionsV2, NATIVE_CACHE_ARCHIVE, NATIVE_CACHE_INDEX, acquire_v2,
    native_cache_dir, store_native_index,
};
use wxml_delivery::index_v2::parse_index_v2;
use wxml_delivery::source::Source;

use crate::support::{
    TestDir, Tripwires, VERSION_N1, VERSION_N2, active_version, assert_contains, assert_failure,
    assert_journal_empty, assert_state, assert_success, assert_tmp_empty, cached_entry, evidence,
    fixture, install_n1, installed_fingerprint, installed_versions, installer_bin, open_root,
    private_root, run_installer_env, shared_channel, skip, state_bytes, workspace,
};

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
#[allow(clippy::too_many_lines)]
fn f3_12_real_cache_driven_upgrade_rollback_and_corrupt_fail_closed() {
    let Some(channel) = shared_channel("f3_12_cache_upgrade") else {
        skip(
            "F3-12",
            "pinned real assets are absent; set TSGO_NATIVE_TARBALL/TSGO_TYPINGS_TARBALL or WXML_REQUIRE_REAL_ENGINE=1",
        );
        return;
    };
    let base = TestDir::new("f3-12");
    let (work, _home, _tmp) = workspace(&base);
    let tripwires = Tripwires::install(base.path());
    let install_bin = installer_bin();
    fixture(
        "F3-12",
        "N1 is cached by a real `install --from-dir`; the N2 cache entry is warmed through the \
         production `acquire_v2` cache path without activation; the cache-only upgrade, \
         rollback and corruption commands are the real CLI",
    );
    println!(
        "F3-12 SCOPE: every cache command passes `--offline` with no `--from-dir`/`--source`; the \
         source channel is never referenced after warm-up and all proxies point at a local sink"
    );

    let root_path = private_root(&base, "cache-only-root");
    let root_text = root_path.to_str().expect("UTF-8 root");
    let install = install_n1(&install_bin, channel, &root_path, &tripwires, &work);
    assert_success(&install, "F3-12 baseline N1 install");
    let root = open_root(&root_path);
    let n1_fingerprint = installed_fingerprint(&root_path, VERSION_N1);
    assert!(!root_path.join("versions").join(VERSION_N2).exists());
    assert_eq!(installed_versions(&root), vec![VERSION_N1]);
    assert!(
        cached_entry(&root, VERSION_N1).is_some(),
        "the real install did not cache N1"
    );
    assert!(
        cached_entry(&root, VERSION_N2).is_none(),
        "N2 must not be installed or cached yet"
    );

    // Warm the N2 cache entry through the production acquisition + cache path.
    // The staged tree is discarded: this mirrors an install that verified and
    // cached an archive, then failed before activation.
    let index_path = channel.channel.join("index.json");
    let index_bytes = fs::read(&index_path).expect("read channel index");
    let index = parse_index_v2(&index_bytes).expect("parse channel index");
    store_native_index(&root, &index_bytes).expect("store cached index");
    let source = Source::Local(channel.channel.clone());
    let acquired = acquire_v2(&AcquireOptionsV2 {
        root: &root,
        source: Some(&source),
        index: &index,
        mode: AcquireModeV2::Exact(VERSION_N2.to_owned()),
        allow_download: false,
        offline: true,
    })
    .expect("warm the verified N2 cache entry");
    assert_eq!(acquired.release.version, VERSION_N2);
    assert!(!acquired.cache_hit, "the warm-up must populate the cache");
    let staging = acquired
        .tree
        .parent()
        .expect("staging directory below tmp")
        .to_path_buf();
    fs::remove_dir_all(&staging).expect("drop the warm-up staging directory");
    assert_tmp_empty(&root);
    let (n2_entry, n2_sha) = cached_entry(&root, VERSION_N2).expect("N2 cache entry");
    let n2_archive = n2_entry.join(NATIVE_CACHE_ARCHIVE);
    let cached_n2 = fs::read(&n2_archive).expect("read cached N2 archive");
    assert_eq!(cached_n2.len() as u64, acquired.release.size);
    println!(
        "F3-12 CACHE: N1 + N2 verified archives cached below {}; N2 sha256={n2_sha}",
        native_cache_dir(&root).display()
    );
    assert_eq!(
        installed_versions(&root),
        vec![VERSION_N1],
        "cache warm-up must not install N2"
    );

    // A proxy sink proves that no offline command opens a network connection.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy sink");
    listener
        .set_nonblocking(true)
        .expect("non-blocking proxy sink");
    let proxy = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("proxy address").port()
    );
    let connections = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let sink = {
        let connections = Arc::clone(&connections);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok(_) => {
                        connections.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        })
    };
    let proxy_env = [
        ("HTTP_PROXY", proxy.as_str()),
        ("HTTPS_PROXY", proxy.as_str()),
        ("ALL_PROXY", proxy.as_str()),
        ("http_proxy", proxy.as_str()),
        ("https_proxy", proxy.as_str()),
        ("all_proxy", proxy.as_str()),
    ];

    // Cache-driven upgrade: no source, no network, only the cached index and
    // the verified N2 archive.
    let upgrade = run_installer_env(
        &install_bin,
        &["upgrade", "--root", root_text, "--offline"],
        &proxy_env,
        &tripwires,
        &work,
        "cache-only upgrade",
    );
    assert_success(&upgrade, "cache-only upgrade");
    assert_contains(&upgrade, &["upgraded to 0.2.0 (native)"]);
    assert_state(&root_path, VERSION_N2, Some(VERSION_N1));
    assert_eq!(
        active_version(&open_root(&root_path)).as_deref(),
        Some(VERSION_N2)
    );
    assert_eq!(
        installed_fingerprint(&root_path, VERSION_N1),
        n1_fingerprint
    );
    assert_eq!(
        fs::read(&n2_archive).expect("re-read cached N2 archive"),
        cached_n2,
        "the cache archive was rewritten instead of reused"
    );
    assert_journal_empty(&open_root(&root_path));
    assert_tmp_empty(&open_root(&root_path));
    evidence(
        "F3-12",
        "PASS",
        format!(
            "cache-only `upgrade --offline` without --from-dir/--source activated real N2 from \
             the verified cache (sha256 {n2_sha}); the cached archive was reused byte for byte"
        ),
    );

    // Cache-driven rollback: no source, no network; the verified N1 tree that
    // the cache-backed installs produced is the rollback target.
    let rollback = run_installer_env(
        &install_bin,
        &[
            "rollback",
            "--root",
            root_text,
            "--to",
            VERSION_N1,
            "--offline",
        ],
        &proxy_env,
        &tripwires,
        &work,
        "no-source rollback",
    );
    assert_success(&rollback, "no-source rollback");
    assert_contains(&rollback, &["rolled back to 0.1.0"]);
    assert_state(&root_path, VERSION_N1, Some(VERSION_N2));
    assert_eq!(
        active_version(&open_root(&root_path)).as_deref(),
        Some(VERSION_N1)
    );
    assert_eq!(
        installed_fingerprint(&root_path, VERSION_N1),
        n1_fingerprint
    );
    evidence(
        "F3-12",
        "PASS",
        "no-source/no-network `rollback --to 0.1.0 --offline` verified the installed N1 tree \
         and activated it",
    );

    // Corrupted cached archive: fail closed before touching current/state or
    // either installed tree.
    let state_before = state_bytes(&root_path);
    let n1_before = installed_fingerprint(&root_path, VERSION_N1);
    let n2_before = installed_fingerprint(&root_path, VERSION_N2);
    let mut corrupt_bytes = fs::read(&n2_archive).expect("read cached N2 archive");
    *corrupt_bytes.last_mut().expect("non-empty cached archive") ^= 0xff;
    fs::write(&n2_archive, &corrupt_bytes).expect("corrupt cached N2 archive");
    let corrupt = run_installer_env(
        &install_bin,
        &["upgrade", "--root", root_text, "--offline"],
        &proxy_env,
        &tripwires,
        &work,
        "corrupt-cache upgrade",
    );
    assert_failure(&corrupt, "corrupt-cache upgrade");
    assert_contains(&corrupt, &["integrity", "sri", "sha-256"]);
    assert_state(&root_path, VERSION_N1, Some(VERSION_N2));
    assert_eq!(state_bytes(&root_path), state_before);
    assert_eq!(installed_fingerprint(&root_path, VERSION_N1), n1_before);
    assert_eq!(installed_fingerprint(&root_path, VERSION_N2), n2_before);
    assert_journal_empty(&open_root(&root_path));
    assert_tmp_empty(&open_root(&root_path));
    evidence(
        "F3-12",
        "PASS",
        "a corrupted cached archive failed closed during `upgrade --offline`; current/state and \
         both installed trees stayed byte-identical",
    );

    // Corrupted cached index: fail closed as well.
    let cache_index = native_cache_dir(&open_root(&root_path)).join(NATIVE_CACHE_INDEX);
    let index_before = fs::read(&cache_index).expect("read cached index");
    let mut corrupt_index = index_before.clone();
    corrupt_index[0] ^= 0xff;
    fs::write(&cache_index, &corrupt_index).expect("corrupt cached index");
    let corrupt_index_output = run_installer_env(
        &install_bin,
        &["upgrade", "--root", root_text, "--offline"],
        &proxy_env,
        &tripwires,
        &work,
        "corrupt cached index upgrade",
    );
    assert_failure(&corrupt_index_output, "corrupt cached index upgrade");
    assert_contains(&corrupt_index_output, &["index", "malformed", "json"]);
    assert_eq!(state_bytes(&root_path), state_before);
    assert_eq!(installed_fingerprint(&root_path, VERSION_N1), n1_before);
    assert_eq!(installed_fingerprint(&root_path, VERSION_N2), n2_before);
    evidence(
        "F3-12",
        "PASS",
        "a corrupted cached index failed closed before acquisition; current/state/trees unchanged",
    );

    // Missing cached index with no source: fail closed instead of falling back
    // to the network.
    fs::remove_file(&cache_index).expect("remove cached index");
    let missing_index = run_installer_env(
        &install_bin,
        &["upgrade", "--root", root_text, "--offline"],
        &proxy_env,
        &tripwires,
        &work,
        "missing cached index upgrade",
    );
    assert_failure(&missing_index, "missing cached index upgrade");
    assert_contains(
        &missing_index,
        &[
            "no cached native index",
            "native release index is required",
            "--from-dir",
        ],
    );
    assert_eq!(state_bytes(&root_path), state_before);
    evidence(
        "F3-12",
        "PASS",
        "with no cached index and no source, `upgrade --offline` failed closed instead of \
         falling back to the network",
    );

    // Restoring the verified cache restores the cache-driven upgrade path.
    fs::write(&cache_index, &index_before).expect("restore cached index");
    fs::write(&n2_archive, &cached_n2).expect("restore cached archive");
    let recovered = run_installer_env(
        &install_bin,
        &["upgrade", "--root", root_text, "--offline"],
        &proxy_env,
        &tripwires,
        &work,
        "restored-cache upgrade",
    );
    assert_success(&recovered, "restored-cache upgrade");
    assert_state(&root_path, VERSION_N2, Some(VERSION_N1));
    evidence(
        "F3-12",
        "PASS",
        "restoring the exact verified cache bytes restored the cache-only upgrade path",
    );

    stop.store(true, Ordering::Relaxed);
    sink.join().expect("proxy sink thread");
    assert_eq!(
        connections.load(Ordering::Relaxed),
        0,
        "offline cache commands must never open a network connection"
    );
    println!("F3-12: proxy sink observed zero connections");

    tripwires.assert_not_executed();
    evidence(
        "F3-12",
        "BLOCKED",
        "connect-level egress auditing (for example `strace -e trace=connect`) is still absent; \
         this evidence uses `--offline` with no source, dead/local-sink proxies and a zero-connection \
         sink, not a kernel connect trace",
    );
    println!(
        "F3-12: real cache-driven upgrade and rollback, corrupt archive/index and missing index \
         all completed without a source, download or Node"
    );
}
