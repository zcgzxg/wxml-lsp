#!/usr/bin/env bash
# F3 remaining-acceptance runner (F3-05/06/08/09/10/11/15/16).
#
# Builds the production Rust binaries and Zed wasm component outside the
# repository, then runs the opt-in `f3_acceptance` suite against the pinned real
# engine/typings tarballs. It never downloads, never writes inside the
# repository and never publishes or commits anything.
#
# Usage:
#   scripts/f3-acceptance.sh [--output DIR] [--network-isolate] [--skip-build]
#                             [--skip-zed] [--help]
#
# Environment (defaults point at the pinned local assets):
#   TSGO_NATIVE_TARBALL   7.1.0-beta typescript-linux-x64 tarball
#   TSGO_TYPINGS_TARBALL  miniprogram-api-typings-5.3.1 tarball
#   WXML_TEST_TMPDIR      temporary base outside the repository
#
# Evidence lines are prefixed with `F3-EVIDENCE: <ID> <PASS|FIXTURE|BLOCKED|SKIP>`.

set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
OUTPUT=${WXML_F3_OUTPUT:-/tmp/opencode/f3-acceptance}
NETWORK_ISOLATE=0
SKIP_BUILD=0
SKIP_ZED=0

usage() {
    sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'
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
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --skip-zed)
            SKIP_ZED=1
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
mkdir -p -- "$OUTPUT/logs" "$OUTPUT/target" "$OUTPUT/zed-target" "$TMP_BASE"

if [ "$SKIP_BUILD" -eq 0 ]; then
    printf 'F3-BUILD: cargo build --release -p wxml-lsp -p wxml-delivery\n'
    CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$OUTPUT/target" \
        cargo build --release -p wxml-lsp -p wxml-delivery
fi

LSP_BIN="$OUTPUT/target/release/wxml-lsp"
INSTALL_BIN="$OUTPUT/target/release/wxml-lsp-install"
ZED_WASM="$OUTPUT/zed-target/wasm32-wasip2/release/zed_wxml.wasm"
if [ "$SKIP_ZED" -eq 0 ]; then
    if [ ! -f "$ZED_WASM" ]; then
        printf 'F3-BUILD: zed wasm32-wasip2\n'
        CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$OUTPUT/zed-target" \
            cargo build --manifest-path "$REPO_ROOT/editors/zed/Cargo.toml" \
            --release --target wasm32-wasip2 --locked
    fi
fi
[ -x "$LSP_BIN" ] || die "missing release wxml-lsp: $LSP_BIN (or pass --skip-build with populated output)"
[ -x "$INSTALL_BIN" ] || die "missing release wxml-lsp-install: $INSTALL_BIN"
[ -f "$ZED_WASM" ] || die "missing Zed wasm: $ZED_WASM"

export TSGO_NATIVE_TARBALL=$ENGINE_TARBALL
export TSGO_TYPINGS_TARBALL=$TYPINGS_TARBALL
export WXML_F3_LSP_BIN=$LSP_BIN
export WXML_F3_INSTALL_BIN=$INSTALL_BIN
export WXML_F3_ZED_WASM=$ZED_WASM
export WXML_REQUIRE_REAL_ENGINE=1
export WXML_TEST_TMPDIR=$TMP_BASE
export CARGO_NET_OFFLINE=true
export CARGO_TARGET_DIR=$OUTPUT/target

if [ "$NETWORK_ISOLATE" -eq 1 ]; then
    command -v unshare >/dev/null 2>&1 || die "unshare is required for --network-isolate"
    unshare -rn true || die "unshare -rn is not permitted on this host"
    printf 'F3-NETWORK: running the suite in a new network namespace (lo DOWN)\n'
    TEST_RUNNER=(unshare -rn)
else
    printf 'F3-NETWORK: namespaces not requested; dead proxies and --offline only\n'
    TEST_RUNNER=()
fi

LOG="$OUTPUT/logs/f3-acceptance.log"
set +e
"${TEST_RUNNER[@]}" cargo test -p wxml-delivery --test f3_acceptance \
    -- --ignored --nocapture --test-threads=1 2>&1 | tee "$LOG"
STATUS=${PIPESTATUS[0]}
set -e
[ "$STATUS" -eq 0 ] || die "f3_acceptance failed with exit $STATUS (log: $LOG)"

printf 'F3-TEST: f3_acceptance suite passed; log=%s\n' "$LOG"
grep -E '^F3-EVIDENCE: ' "$LOG" || true
