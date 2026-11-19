#!/usr/bin/env bash
# F3-02 syscall-level exec audit runner.
#
# Runs the `f3_exec_audit` integration test in both modes:
#
#   1. non-real: positive control (a binary named `node` must be recorded) and
#      the production installer against a shell fixture package (`FIXTURE`);
#   2. real: the pinned engine/typings tarballs are provisioned and packaged by
#      the production packager, installed by the production installer, and a
#      real `wxml-lsp --stdio --backend native` session is traced through
#      initialize/documentSymbol/hover/shutdown (`PASS`).
#
# The test replaces the missing `strace -f -e trace=execve,execveat` gate with
# an in-process ptrace tracer: it records `/proc/<pid>/exe`,
# `/proc/<pid>/cmdline`, basename, parent pid and environment keys for every
# exec in the traced process tree. A PATH shim cannot see an absolute-path
# `node` invocation; this tracer can.
#
# Usage:
#   scripts/f3-exec-audit.sh [--output DIR] [--non-real-only] [--keep-tmp] [--help]
#
# Environment:
#   TSGO_NATIVE_TARBALL   7.1.0-beta typescript-linux-x64 tarball
#   TSGO_TYPINGS_TARBALL  miniprogram-api-typings-5.3.1 tarball
#   WXML_F3_ZED_WASM      prebuilt zed wasm32-wasip2 extension (built if absent)
#   WXML_EXEC_AUDIT_OUTPUT  output root (default /tmp/opencode/f3-exec-audit)
#
# Nothing is written inside the repository and nothing is committed. The test
# logs are kept under <output>/logs.

set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
OUTPUT=${WXML_EXEC_AUDIT_OUTPUT:-/tmp/opencode/f3-exec-audit}
NON_REAL_ONLY=0
KEEP_TMP=0

usage() {
    sed -n '2,36p' "$0" | sed 's/^# \{0,1\}//'
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
        --non-real-only)
            NON_REAL_ONLY=1
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

ENGINE_TARBALL=${TSGO_NATIVE_TARBALL:-/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz}
TYPINGS_TARBALL=${TSGO_TYPINGS_TARBALL:-/tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz}
[ -f "$ENGINE_TARBALL" ] || die "engine tarball is missing: $ENGINE_TARBALL"
[ -f "$TYPINGS_TARBALL" ] || die "typings tarball is missing: $TYPINGS_TARBALL"

# A dedicated test base: cleanup must never touch a caller-owned directory.
TMP_BASE=$OUTPUT/tmp
mkdir -p -- "$OUTPUT/logs" "$OUTPUT/zed-target" "$TMP_BASE"
rm -rf -- "${TMP_BASE:?}"/*

cleanup() {
    if [ "$KEEP_TMP" -eq 0 ]; then
        rm -rf -- "${TMP_BASE:?}"/*
    fi
}
trap cleanup EXIT HUP INT TERM

export CARGO_NET_OFFLINE=${CARGO_NET_OFFLINE:-true}
export WXML_TEST_TMPDIR=$TMP_BASE
export TSGO_NATIVE_TARBALL=$ENGINE_TARBALL
export TSGO_TYPINGS_TARBALL=$TYPINGS_TARBALL

NON_REAL_LOG="$OUTPUT/logs/f3-exec-audit-nonreal.log"
printf 'F3-EXEC-AUDIT: non-real mode (positive control + fixture installer)\n'
set +e
cargo test -p wxml-delivery --test f3_exec_audit \
    -- --nocapture --test-threads=1 2>&1 | tee "$NON_REAL_LOG"
NON_REAL_STATUS=${PIPESTATUS[0]}
set -e
[ "$NON_REAL_STATUS" -eq 0 ] || die "non-real exec audit failed (log: $NON_REAL_LOG)"
grep -E '^EXEC-AUDIT: (PASS|FIXTURE|BLOCKED)' "$NON_REAL_LOG" || true
printf 'F3-EXEC-AUDIT: non-real mode passed (log: %s)\n' "$NON_REAL_LOG"

if [ "$NON_REAL_ONLY" -eq 1 ]; then
    exit 0
fi

if [ -z "${WXML_F3_LSP_BIN:-}" ]; then
    for candidate in "$REPO_ROOT/target/release/wxml-lsp" "$REPO_ROOT/target/debug/wxml-lsp"; do
        if [ -x "$candidate" ]; then
            export WXML_F3_LSP_BIN=$candidate
            break
        fi
    done
fi
[ -n "${WXML_F3_LSP_BIN:-}" ] && [ -x "$WXML_F3_LSP_BIN" ] || die \
    "no wxml-lsp binary: set WXML_F3_LSP_BIN or run \
     cargo build --release -p wxml-lsp -p wxml-delivery"
if [ -z "${WXML_F3_INSTALL_BIN:-}" ]; then
    sibling=$(dirname -- "$WXML_F3_LSP_BIN")/wxml-lsp-install
    if [ -x "$sibling" ]; then
        export WXML_F3_INSTALL_BIN=$sibling
    fi
fi
printf 'F3-EXEC-AUDIT: lsp=%s install=%s\n' \
    "$WXML_F3_LSP_BIN" "${WXML_F3_INSTALL_BIN:-<test-built>}"

ZED_WASM=${WXML_F3_ZED_WASM:-$OUTPUT/zed-target/wasm32-wasip2/release/zed_wxml.wasm}
if [ ! -f "$ZED_WASM" ]; then
    printf 'F3-EXEC-AUDIT: building the zed wasm component\n'
    CARGO_TARGET_DIR="$OUTPUT/zed-target" \
        cargo build --manifest-path "$REPO_ROOT/editors/zed/Cargo.toml" \
        --release --target wasm32-wasip2 --locked
fi
[ -f "$ZED_WASM" ] || die "missing zed wasm: $ZED_WASM"

export WXML_F3_ZED_WASM=$ZED_WASM
export WXML_REQUIRE_REAL_ENGINE=1
export WXML_REQUIRE_EXEC_AUDIT=1

REAL_LOG="$OUTPUT/logs/f3-exec-audit-real.log"
printf 'F3-EXEC-AUDIT: real mode (pinned assets + native stdio session)\n'
set +e
cargo test -p wxml-delivery --test f3_exec_audit \
    -- --ignored --nocapture --test-threads=1 2>&1 | tee "$REAL_LOG"
REAL_STATUS=${PIPESTATUS[0]}
set -e
[ "$REAL_STATUS" -eq 0 ] || die "real exec audit failed (log: $REAL_LOG)"

grep -E '^EXEC-AUDIT: (PASS|FIXTURE|BLOCKED)' "$REAL_LOG" || true
grep -E '^EXEC-AUDIT: real (installer|native LSP) executable:' "$REAL_LOG" || true
printf 'F3-EXEC-AUDIT: PASS (real mode; log: %s)\n' "$REAL_LOG"
printf 'F3-EXEC-AUDIT: no Node/Node-API executable was executed in either traced process tree\n'
