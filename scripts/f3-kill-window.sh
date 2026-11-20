#!/usr/bin/env bash
# F3-06/F3-09 exact process-level SIGKILL window runner.
#
# Builds (or reuses) the production release binaries outside the repository,
# then runs the `f3_kill_window` integration test in two modes:
#
#   1. non-real: ptrace syscall-stop preflight plus a synthetic `mv`
#      rename kill/control (`FIXTURE`); no pinned assets needed;
#   2. real: the production packager builds two independent real releases
#      (0.1.0 and 0.2.0) with `scripts/build-release.sh --version`, the
#      production installer is traced with PTRACE_SYSCALL and killed with an
#      external SIGKILL at the upgrade U1/U3/U5/U6 and legacy L3/L4 windows,
#      followed by the real `recover`/`verify`/retry commands.
#
# Missing assets skip loudly; WXML_REQUIRE_KILL_WINDOW=1 (or
# WXML_REQUIRE_REAL_ENGINE=1) makes a blocked host or a missing asset fail.
#
# Usage:
#   scripts/f3-kill-window.sh [--output DIR] [--real-channel DIR]
#                             [--non-real-only] [--skip-build]
#                             [--network-isolate] [--keep-tmp] [--help]
#
# Environment:
#   TSGO_NATIVE_TARBALL   7.1.0-beta typescript-linux-x64 tarball
#   TSGO_TYPINGS_TARBALL  miniprogram-api-typings-5.3.1 tarball
#   WXML_F3_LSP_BIN       prebuilt production wxml-lsp (default: the real
#                         package at /tmp/opencode/tsgo-native-pkg, else the
#                         release build under <output>/target)
#   WXML_F3_ZED_WASM      prebuilt zed wasm32-wasip2 component (built if absent)
#   WXML_F3_REAL_CHANNEL  channel with independently built 0.1.0 and 0.2.0
#                         packages (default: <output>/channel, built here)
#   WXML_KILL_WINDOW_OUTPUT  output root (default /tmp/opencode/f3-kill-window)
#
# Nothing is written inside the repository and nothing is committed. Logs stay
# under <output>/logs.

set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
OUTPUT=${WXML_KILL_WINDOW_OUTPUT:-/tmp/opencode/f3-kill-window}
REAL_CHANNEL=${WXML_F3_REAL_CHANNEL:-}
NON_REAL_ONLY=0
SKIP_BUILD=0
NETWORK_ISOLATE=0
KEEP_TMP=0

usage() {
    sed -n '2,35p' "$0" | sed 's/^# \{0,1\}//'
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --output)
            [ "$#" -ge 2 ] || die "--output requires a directory"
            OUTPUT=$2
            shift 2
            ;;
        --real-channel)
            [ "$#" -ge 2 ] || die "--real-channel requires a directory"
            REAL_CHANNEL=$2
            shift 2
            ;;
        --non-real-only)
            NON_REAL_ONLY=1
            shift
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --network-isolate)
            NETWORK_ISOLATE=1
            shift
            ;;
        --keep-tmp)
            KEEP_TMP=1
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
done

[ -n "$OUTPUT" ] || die "--output must not be empty"
OUTPUT=$(realpath -m -- "$OUTPUT")
REPO_REAL=$(realpath -m -- "$REPO_ROOT")
case "$OUTPUT/" in
    "$REPO_REAL/"*) die "--output must be outside the repository: $OUTPUT" ;;
esac
case "$OUTPUT" in
    /tmp/*|/var/tmp/*) ;;
    *) die "--output must live under /tmp or /var/tmp (got $OUTPUT)" ;;
esac
[ -n "$REAL_CHANNEL" ] || REAL_CHANNEL="$OUTPUT/channel"
REAL_CHANNEL=$(realpath -m -- "$REAL_CHANNEL")
case "$REAL_CHANNEL/" in
    "$REPO_REAL/"*) die "--real-channel must be outside the repository: $REAL_CHANNEL" ;;
esac

ENGINE_TARBALL=${TSGO_NATIVE_TARBALL:-/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz}
TYPINGS_TARBALL=${TSGO_TYPINGS_TARBALL:-/tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz}
[ -f "$ENGINE_TARBALL" ] || die "engine tarball is missing: $ENGINE_TARBALL"
[ -f "$TYPINGS_TARBALL" ] || die "typings tarball is missing: $TYPINGS_TARBALL"

TMP_BASE=$OUTPUT/tmp
mkdir -p -- "$OUTPUT/logs" "$OUTPUT/target" "$TMP_BASE"
rm -rf -- "${TMP_BASE:?}"/*

cleanup() {
    if [ "$KEEP_TMP" -eq 0 ]; then
        rm -rf -- "${TMP_BASE:?}"/*
    fi
}
trap cleanup EXIT HUP INT TERM

LSP_BIN=${WXML_F3_LSP_BIN:-}
if [ -z "$LSP_BIN" ]; then
    if [ -x /tmp/opencode/tsgo-native-pkg/bin/wxml-lsp ]; then
        LSP_BIN=/tmp/opencode/tsgo-native-pkg/bin/wxml-lsp
    else
        LSP_BIN=$OUTPUT/target/release/wxml-lsp
    fi
fi
INSTALL_BIN=${WXML_F3_INSTALL_BIN:-$OUTPUT/target/release/wxml-lsp-install}
ZED_WASM=${WXML_F3_ZED_WASM:-$OUTPUT/zed-target/wasm32-wasip2/release/zed_wxml.wasm}

if [ "$SKIP_BUILD" -eq 0 ]; then
    if [ ! -x "$INSTALL_BIN" ] || [ ! -x "$OUTPUT/target/release/wxml-lsp" ]; then
        printf 'F3-KILL: building release wxml-lsp + wxml-lsp-install\n'
        CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$OUTPUT/target" \
            cargo build --release -p wxml-lsp -p wxml-delivery
    fi
    if [ ! -f "$ZED_WASM" ]; then
        printf 'F3-KILL: building zed wasm32-wasip2\n'
        CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$OUTPUT/zed-target" \
            cargo build --manifest-path "$REPO_ROOT/editors/zed/Cargo.toml" \
            --release --target wasm32-wasip2 --locked
    fi
fi
[ -x "$LSP_BIN" ] || die "missing production wxml-lsp: $LSP_BIN"
[ -x "$INSTALL_BIN" ] || die "missing release wxml-lsp-install: $INSTALL_BIN (or pass --skip-build)"
[ -f "$ZED_WASM" ] || die "missing Zed wasm: $ZED_WASM"

if [ "$NON_REAL_ONLY" -eq 0 ]; then
    if [ ! -f "$REAL_CHANNEL/wxml-lsp-0.1.0-linux-x86_64/manifest.json" ] \
        || [ ! -f "$REAL_CHANNEL/wxml-lsp-0.2.0-linux-x86_64/manifest.json" ]; then
        [ "$SKIP_BUILD" -eq 0 ] \
            || die "real channel is incomplete and --skip-build was given: $REAL_CHANNEL"
        FROM_DIR="$OUTPUT/release-from-dir"
        mkdir -p -- "$FROM_DIR" "$REAL_CHANNEL"
        cp -f -- "$ENGINE_TARBALL" "$FROM_DIR/typescript-linux-x64-7.1.0-beta.tgz"
        cp -f -- "$TYPINGS_TARBALL" "$FROM_DIR/miniprogram-api-typings-5.3.1.tgz"
        for REAL_VERSION in 0.1.0 0.2.0; do
            printf 'F3-KILL: building independent real %s release\n' "$REAL_VERSION"
            SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-1795046400} \
            WXML_BUILD_CACHE="$OUTPUT/release-cache" \
            WXML_ASSET_CACHE="${WXML_ASSET_CACHE:-$OUTPUT/release-cache/assets-cache}" \
            CARGO_TARGET_DIR="$OUTPUT/release-target" \
            ZED_CARGO_TARGET_DIR="$OUTPUT/zed-target" \
            "$REPO_ROOT/scripts/build-release.sh" \
                --output "$REAL_CHANNEL" \
                --version "$REAL_VERSION" \
                --zed-wasm "$ZED_WASM" \
                --from-dir "$FROM_DIR" \
                2>&1 | tee "$OUTPUT/logs/build-release-$REAL_VERSION.log"
        done
    else
        printf 'F3-KILL: reusing independent real channel %s\n' "$REAL_CHANNEL"
    fi
    REAL_INSTALL_BIN="$REAL_CHANNEL/wxml-lsp-0.2.0-linux-x86_64/bin/wxml-lsp-install"
    REAL_LSP_BIN="$REAL_CHANNEL/wxml-lsp-0.2.0-linux-x86_64/bin/wxml-lsp"
    [ -x "$REAL_INSTALL_BIN" ] || die "real channel has no installer: $REAL_INSTALL_BIN"
    [ -x "$REAL_LSP_BIN" ] || die "real channel has no wxml-lsp: $REAL_LSP_BIN"
    INSTALL_BIN="$REAL_INSTALL_BIN"
    LSP_BIN="$REAL_LSP_BIN"
    export WXML_F3_REAL_CHANNEL="$REAL_CHANNEL"
    export WXML_REQUIRE_REAL_N2=1
fi

export CARGO_NET_OFFLINE=true
export CARGO_TARGET_DIR=$OUTPUT/target
export WXML_TEST_TMPDIR=$TMP_BASE
export TSGO_NATIVE_TARBALL=$ENGINE_TARBALL
export TSGO_TYPINGS_TARBALL=$TYPINGS_TARBALL
export WXML_F3_LSP_BIN=$LSP_BIN
export WXML_F3_INSTALL_BIN=$INSTALL_BIN
export WXML_F3_ZED_WASM=$ZED_WASM

NON_REAL_LOG="$OUTPUT/logs/f3-kill-window-nonreal.log"
printf 'F3-KILL: non-real mode (ptrace preflight + fixture rename kill/control)\n'
set +e
cargo test -p wxml-delivery --test f3_kill_window \
    -- --nocapture --test-threads=1 2>&1 | tee "$NON_REAL_LOG"
NON_REAL_STATUS=${PIPESTATUS[0]}
set -e
[ "$NON_REAL_STATUS" -eq 0 ] || die "non-real kill-window suite failed (log: $NON_REAL_LOG)"
grep -E '^F3-EVIDENCE: ' "$NON_REAL_LOG" || true

if [ "$NON_REAL_ONLY" -eq 1 ]; then
    printf 'F3-KILL: non-real mode passed (log: %s)\n' "$NON_REAL_LOG"
    exit 0
fi

export WXML_REQUIRE_REAL_ENGINE=1
export WXML_REQUIRE_KILL_WINDOW=1

TEST_RUNNER=()
if [ "$NETWORK_ISOLATE" -eq 1 ]; then
    command -v unshare >/dev/null 2>&1 || die "unshare is required for --network-isolate"
    unshare -rn true || die "unshare -rn is not permitted on this host"
    printf 'F3-KILL: running the real windows in a new network namespace (lo DOWN)\n'
    TEST_RUNNER=(unshare -rn)
else
    printf 'F3-KILL: namespaces not requested; dead proxies and --offline only\n'
fi

REAL_LOG="$OUTPUT/logs/f3-kill-window-real.log"
printf 'F3-KILL: real mode (pinned assets; %s)\n' "$INSTALL_BIN"
set +e
"${TEST_RUNNER[@]}" cargo test -p wxml-delivery --test f3_kill_window \
    -- --ignored --nocapture --test-threads=1 2>&1 | tee "$REAL_LOG"
REAL_STATUS=${PIPESTATUS[0]}
set -e
[ "$REAL_STATUS" -eq 0 ] || die "real kill-window suite failed (log: $REAL_LOG)"

grep -E '^F3-EVIDENCE: ' "$REAL_LOG" || true
if grep -q 'F3-EVIDENCE: .* SKIP ::\|F3-EVIDENCE: .* BLOCKED ::' "$REAL_LOG"; then
    printf 'F3-KILL: PARTIAL (a real window skipped or the host blocked ptrace; see %s)\n' "$REAL_LOG"
    exit 0
fi
printf 'F3-KILL: PASS (real kill windows; log: %s)\n' "$REAL_LOG"
