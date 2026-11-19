#!/usr/bin/env bash
# F3 real-release acceptance runner.
#
# Builds production Rust binaries and the Zed wasm component outside the
# repository, then runs the opt-in `f3_real_release` test suite against the
# pinned real engine/typings tarballs. It never downloads, never writes inside
# the repository and never publishes anything.
#
# Usage:
#   scripts/f3-real-release.sh [--output DIR] [--network-isolate] [--skip-zed]
#                              [--skip-build] [--help]
#
# Environment (defaults point at the pinned local assets):
#   TSGO_NATIVE_TARBALL   7.1.0-beta typescript-linux-x64 tarball
#   TSGO_TYPINGS_TARBALL  miniprogram-api-typings-5.3.1 tarball
#   WXML_TEST_TMPDIR      temporary base outside the repository
#   WXML_F3_KEEP_ROOT=1   keep the installed root for the Zed smoke
#
# The final report lines are prefixed with `F3-` so they can be copied into
# docs/evidence/2026-11-19-tsgo-f3-real-release.md.

set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
OUTPUT=${WXML_F3_OUTPUT:-/tmp/opencode/f3-real-release}
NETWORK_ISOLATE=0
SKIP_ZED=0
SKIP_BUILD=0

usage() {
    sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'
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
        --network-isolate)
            NETWORK_ISOLATE=1
            shift
            ;;
        --skip-zed)
            SKIP_ZED=1
            shift
            ;;
        --skip-build)
            SKIP_BUILD=1
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
    /tmp/*) ;;
    *) die "--output must live under /tmp (got $OUTPUT)" ;;
esac

ENGINE_TARBALL=${TSGO_NATIVE_TARBALL:-/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz}
TYPINGS_TARBALL=${TSGO_TYPINGS_TARBALL:-/tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz}
[ -f "$ENGINE_TARBALL" ] || die "engine tarball is missing: $ENGINE_TARBALL"
[ -f "$TYPINGS_TARBALL" ] || die "typings tarball is missing: $TYPINGS_TARBALL"

TMP_BASE=${WXML_TEST_TMPDIR:-$OUTPUT/tmp}
case "$TMP_BASE" in
    /tmp/*) ;;
    *) die "WXML_TEST_TMPDIR must live under /tmp: $TMP_BASE" ;;
esac
mkdir -p -- "$OUTPUT/logs" "$OUTPUT/target" "$OUTPUT/zed-target" "$TMP_BASE" "$OUTPUT/keep"

if [ "$SKIP_BUILD" -eq 0 ]; then
    printf 'F3-BUILD: cargo build --release -p wxml-lsp -p wxml-delivery\n'
    CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$OUTPUT/target" \
        cargo build --release -p wxml-lsp -p wxml-delivery
    printf 'F3-BUILD: zed wasm32-wasip2\n'
    CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$OUTPUT/zed-target" \
        cargo build --manifest-path "$REPO_ROOT/editors/zed/Cargo.toml" \
        --release --target wasm32-wasip2 --locked
fi

LSP_BIN="$OUTPUT/target/release/wxml-lsp"
INSTALL_BIN="$OUTPUT/target/release/wxml-lsp-install"
ZED_WASM="$OUTPUT/zed-target/wasm32-wasip2/release/zed_wxml.wasm"
[ -x "$LSP_BIN" ] || die "missing release wxml-lsp: $LSP_BIN"
[ -x "$INSTALL_BIN" ] || die "missing release wxml-lsp-install: $INSTALL_BIN"
[ -f "$ZED_WASM" ] || die "missing Zed wasm: $ZED_WASM"

export TSGO_NATIVE_TARBALL=$ENGINE_TARBALL
export TSGO_TYPINGS_TARBALL=$TYPINGS_TARBALL
export WXML_F3_LSP_BIN=$LSP_BIN
export WXML_F3_INSTALL_BIN=$INSTALL_BIN
export WXML_F3_ZED_WASM=$ZED_WASM
export WXML_REQUIRE_REAL_ENGINE=1
export WXML_TEST_TMPDIR=$TMP_BASE
export WXML_F3_KEEP_ROOT=1
export CARGO_NET_OFFLINE=true
export CARGO_TARGET_DIR=$OUTPUT/target

if command -v strace >/dev/null 2>&1; then
    RUNNER=(strace -f -e trace=execve,execveat -o "$OUTPUT/logs/execve.log")
    printf 'F3-PROCESS-AUDIT: strace execve log enabled\n'
else
    RUNNER=()
    printf 'F3-PROCESS-AUDIT: BLOCKED (strace/ltrace/perf/auditctl absent); PATH tripwire and /proc observation are auxiliary only\n'
fi

if [ "$NETWORK_ISOLATE" -eq 1 ]; then
    command -v unshare >/dev/null 2>&1 || die "unshare is required for --network-isolate"
    unshare -rn true || die "unshare -rn is not permitted on this host"
    printf 'F3-NETWORK: running the suite in a new network namespace (lo DOWN)\n'
    TEST_RUNNER=(unshare -rn)
else
    printf 'F3-NETWORK: namespaces not requested; dead proxies and --offline only\n'
    TEST_RUNNER=()
fi

LOG="$OUTPUT/logs/f3-real-release.log"
set +e
"${TEST_RUNNER[@]}" "${RUNNER[@]}" cargo test -p wxml-delivery --test f3_real_release \
    -- --ignored --nocapture --test-threads=1 2>&1 | tee "$LOG"
STATUS=${PIPESTATUS[0]}
set -e
[ "$STATUS" -eq 0 ] || die "f3_real_release failed with exit $STATUS (log: $LOG)"

printf 'F3-TEST: real-release suite passed; log=%s\n' "$LOG"

if [ "$SKIP_ZED" -eq 1 ]; then
    printf 'F3-14: BLOCKED (Zed smoke skipped by request)\n'
    exit 0
fi
command -v zed >/dev/null 2>&1 || { printf 'F3-14: BLOCKED (zed binary not installed)\n'; exit 0; }
[ -n "${WAYLAND_DISPLAY:-${DISPLAY:-}}" ] || { printf 'F3-14: BLOCKED (no Wayland/X display)\n'; exit 0; }

INSTALLED_ROOT="$TMP_BASE/f3-real-release-keep/install-root"
if [ ! -x "$INSTALLED_ROOT/current/bin/wxml-lsp" ]; then
    printf 'F3-14: BLOCKED (no kept installed root at %s)\n' "$INSTALLED_ROOT"
    exit 0
fi

ZED_DATA="$OUTPUT/zed-data"
ZED_PROJECT="$OUTPUT/zed-project"
ZED_LOG="$OUTPUT/logs/zed-smoke.log"
rm -rf -- "$ZED_DATA" "$ZED_PROJECT"
mkdir -p -- "$ZED_DATA/config" "$ZED_DATA/home" "$ZED_PROJECT/pages/index"
cp -a "$HOME/.local/share/zed/extensions" "$ZED_DATA/extensions" 2>/dev/null || true
printf '{"session":{"trust_all_worktrees":true}}\n' > "$ZED_DATA/config/settings.json"
printf 'export WXML_LSP_BIN=%s\nunset WXML_TYPE_BACKEND WXML_BACKEND_DIR WXML_NODE\n' \
    "$INSTALLED_ROOT/current/bin/wxml-lsp" > "$ZED_DATA/home/.bashrc"
printf '{"pages":["pages/index/index"]}\n' > "$ZED_PROJECT/app.json"
printf '<view class="title">{{title}}</view>\n' > "$ZED_PROJECT/pages/index/index.wxml"
printf 'Page({ data: { title: "hello" } });\n' > "$ZED_PROJECT/pages/index/index.js"

printf 'F3-14: launching isolated Zed smoke (log: %s)\n' "$ZED_LOG"
HOME="$ZED_DATA/home" WXML_LSP_BIN="$INSTALLED_ROOT/current/bin/wxml-lsp" \
    setsid zed --foreground --user-data-dir "$ZED_DATA" -n \
    "$ZED_PROJECT/pages/index/index.wxml" >"$ZED_LOG" 2>&1 &
ZED_PID=$!
FOUND=0
for _ in $(seq 1 60); do
    if pgrep -af "$INSTALLED_ROOT/current/bin/wxml-lsp" >/dev/null 2>&1; then
        FOUND=1
        break
    fi
    sleep 0.5
done
sleep 2
{
    printf 'zed pid: %s\n' "$ZED_PID"
    printf 'lsp processes:\n'
    pgrep -af "$INSTALLED_ROOT/current/bin/wxml-lsp" || true
    printf 'node descendants of zed:\n'
    pstree -ap "$ZED_PID" 2>/dev/null || true
    ps -eo pid,ppid,comm,args | awk -v root="$ZED_PID" '$1==root || $2==root {print}'
    printf 'zed lsp log lines:\n'
    grep -i "starting language server\|wxml" "$ZED_DATA/logs/Zed.log" || true
} >>"$ZED_LOG"
if [ "$FOUND" -eq 1 ]; then
    printf 'F3-14: Zed started the packaged `wxml-lsp --stdio`; no Node child was observed\n'
else
    printf 'F3-14: BLOCKED (Zed did not start the packaged language server within 30s; see %s)\n' "$ZED_LOG"
fi
kill -TERM -- "-$ZED_PID" 2>/dev/null || true
sleep 1
kill -KILL -- "-$ZED_PID" 2>/dev/null || true
printf 'F3-14: BLOCKED until `wxml-lsp --stdio` defaults to the native backend and a UI/query smoke is recorded\n'
