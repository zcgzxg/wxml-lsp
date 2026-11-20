#!/usr/bin/env bash
# P7 native gate runner.
#
# Runs the 40 opt-in native gates (23 native engine + 3 symbols/edits + 9 stdio
# shells + 1 perf + 1 stability + 3 real_matrix scenarios) against the pinned
# TypeScript 7.1.0-beta engine and fails closed when any input, identity check,
# build or gate fails.
#
# Required:
#   --package DIR    installed v2 native package containing engine/lib/tsc and
#                    typings/ (an absolute, existing package root)
#
# Optional:
#   --engine FILE    pinned engine override (default <package>/engine/lib/tsc)
#   --typings DIR    typings override (default <package>/typings); accepts either
#                    the typings package directory or the directory containing
#                    miniprogram-api-typings
#   --output DIR     log directory (default
#                    /tmp/opencode/native-gates/<UTC timestamp>); must be
#                    outside the repository
#   --skip-build     reuse an existing release build
#   --verify-no-node run scripts/verify-no-node.sh after the 40 gates
#   --installer FILE wxml-lsp-install binary (required with --verify-no-node)
#   --install-root DIR install root (required with --verify-no-node)
#   -h, --help       show this help
#
# Environment written for the gate children:
#   WXML_TEST_NATIVE=1
#   WXML_REQUIRE_REAL_ENGINE=1
#   TSGO_BIN=<pinned engine>
#   TSGO_ASSET_ROOT=<package>/engine (contains lib/lib*.d.ts)
#   TSGO_TYPINGS_ROOT=<typings package directory>
#
# WXML_TEST_NATIVE_PACKAGE is deliberately NOT exported. The G-P5-E2E-13 gate
# deletes the engine from its package to prove the unavailable-backend path, so
# the gates must build one disposable package per test from TSGO_* instead of
# sharing an installed package. The package argument is only the source of the
# pinned engine and typings.
#
# Exit codes:
#   0  all 40 gates passed (and the optional no-Node check, when requested)
#   1  the build, a gate, or the optional no-Node check failed
#   2  a required input is missing, relative, unpinned, or unsafe

set -euo pipefail
umask 022

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

EXPECTED_ENGINE_VERSION='Version 7.1.0-beta'
EXPECTED_ENGINE_SHA256='a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70'
EXPECTED_ENGINE_SIZE=24522912
NATIVE_ENGINE_GATES=23
NATIVE_EDIT_GATES=3
NATIVE_STDIO_GATES=9
NATIVE_PERF_GATES=1
NATIVE_STABILITY_GATES=1
NATIVE_MATRIX_GATES=3
EXPECTED_NATIVE_GATES=$((NATIVE_ENGINE_GATES + NATIVE_EDIT_GATES + NATIVE_STDIO_GATES +
    NATIVE_PERF_GATES + NATIVE_STABILITY_GATES + NATIVE_MATRIX_GATES))

PACKAGE=''
ENGINE=''
TYPINGS=''
OUTPUT=''
SKIP_BUILD=0
VERIFY_NO_NODE=0
INSTALLER=''
INSTALL_ROOT=''

usage() {
    cat <<'EOF'
P7 native gate runner.

Runs the 23 native_engine_real gates, the 3 symbols_edits gates, the 9
stdio_type_loop shells, the G-P5-PERF-01 and G-P5-STABILITY-01 retargets and
the 3 real_matrix required-native tests, in that order, with --ignored
--test-threads=1. All gates use the release profile.

Required:
  --package DIR     installed v2 native package containing engine/lib/tsc and
                    typings/ (absolute, existing)

Optional:
  --engine FILE     pinned engine override (default <package>/engine/lib/tsc)
  --typings DIR     typings override (default <package>/typings); accepts the
                    typings package itself or its parent directory
  --output DIR      log directory (default /tmp/opencode/native-gates/<UTC
                    timestamp>); must be outside the repository
  --skip-build      reuse an existing release build
  --verify-no-node  also run scripts/verify-no-node.sh (installer scope)
  --installer FILE  wxml-lsp-install binary (required with --verify-no-node)
  --install-root DIR install root (required with --verify-no-node)
  -h, --help        show this help

Gate groups: 23 native engine + 3 symbols/edits + 9 stdio + 1 perf +
1 stability + 3 real_matrix = 40.

The runner exports WXML_TEST_NATIVE=1, WXML_REQUIRE_REAL_ENGINE=1, TSGO_BIN,
TSGO_ASSET_ROOT and TSGO_TYPINGS_ROOT. It does not export
WXML_TEST_NATIVE_PACKAGE: the unavailable-backend gate deletes its fixture
engine, so every gate builds a disposable package from TSGO_* instead.

The G-P5-STABILITY-01 shell keeps its strict clean-worktree contract. A local
evidence run on a dirty integration tree must export
WXML_STABILITY_ALLOW_DIRTY=1 explicitly; the runner neither sets nor documents
that as a skip.

Exit codes: 0 pass, 1 build/gate failure, 2 input or pinned-identity failure.
EOF
}

die_usage() {
    printf 'ERROR: %s\n' "$*" >&2
    printf 'Run %s --help for usage.\n' "$0" >&2
    exit 2
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die_usage "$1 is required"
}

require_absolute() {
    local label=$1
    local path=$2
    case "$path" in
        /*) ;;
        *) die_usage "$label must be an absolute path: $path" ;;
    esac
}

require_directory() {
    local label=$1
    local path=$2
    require_absolute "$label" "$path"
    [ -d "$path" ] || die_usage "$label does not exist or is not a directory: $path"
    [ ! -L "$path" ] || die_usage "$label must not be a symlink: $path"
}

require_file() {
    local label=$1
    local path=$2
    require_absolute "$label" "$path"
    [ -f "$path" ] || die_usage "$label does not exist or is not a regular file: $path"
    [ ! -L "$path" ] || die_usage "$label must not be a symlink: $path"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --package)
            [ "$#" -ge 2 ] || die_usage "--package requires a value"
            PACKAGE=$2
            shift 2
            ;;
        --engine)
            [ "$#" -ge 2 ] || die_usage "--engine requires a value"
            ENGINE=$2
            shift 2
            ;;
        --typings)
            [ "$#" -ge 2 ] || die_usage "--typings requires a value"
            TYPINGS=$2
            shift 2
            ;;
        --output)
            [ "$#" -ge 2 ] || die_usage "--output requires a value"
            OUTPUT=$2
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --verify-no-node)
            VERIFY_NO_NODE=1
            shift
            ;;
        --installer)
            [ "$#" -ge 2 ] || die_usage "--installer requires a value"
            INSTALLER=$2
            shift 2
            ;;
        --install-root)
            [ "$#" -ge 2 ] || die_usage "--install-root requires a value"
            INSTALL_ROOT=$2
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die_usage "unknown argument: $1"
            ;;
    esac
done

[ -n "$PACKAGE" ] || die_usage "--package is required"

require_cmd cargo
require_cmd date
require_cmd realpath
require_cmd sha256sum
require_cmd stat
require_cmd timeout
require_cmd awk
require_cmd grep
require_cmd sed

REPO_REAL=$(realpath -e -- "$REPO_ROOT")
require_directory "--package" "$PACKAGE"
PACKAGE=$(realpath -e -- "$PACKAGE")

if [ -z "$ENGINE" ]; then
    ENGINE="$PACKAGE/engine/lib/tsc"
fi
require_file "--engine" "$ENGINE"
[ -x "$ENGINE" ] || die_usage "--engine is not executable: $ENGINE"
ENGINE=$(realpath -e -- "$ENGINE")

# The fixture builder copies <asset-root>/lib/lib*.d.ts, so the engine must be
# <asset-root>/lib/tsc (or a canonically equivalent path).
ASSET_ROOT=$(dirname -- "$(dirname -- "$ENGINE")")
[ ! -L "$ASSET_ROOT" ] || die_usage "engine asset root must not be a symlink: $ASSET_ROOT"
[ -d "$ASSET_ROOT/lib" ] || die_usage "engine must live at <asset-root>/lib/tsc; missing directory: $ASSET_ROOT/lib"
[ -f "$ASSET_ROOT/lib/lib.d.ts" ] || die_usage "engine asset root is missing lib/lib.d.ts: $ASSET_ROOT"

if [ -z "$TYPINGS" ]; then
    TYPINGS="$PACKAGE/typings"
fi
require_directory "--typings" "$TYPINGS"
TYPINGS=$(realpath -e -- "$TYPINGS")
if [ -e "$TYPINGS/miniprogram-api-typings" ]; then
    [ ! -L "$TYPINGS/miniprogram-api-typings" ] \
        || die_usage "--typings contains a symlinked miniprogram-api-typings: $TYPINGS"
    TYPINGS="$TYPINGS/miniprogram-api-typings"
fi
[ -f "$TYPINGS/index.d.ts" ] \
    || die_usage "--typings must contain index.d.ts or miniprogram-api-typings/index.d.ts: $TYPINGS"
[ -f "$TYPINGS/types/index.d.ts" ] \
    || die_usage "--typings is missing types/index.d.ts: $TYPINGS"
TYPINGS=$(realpath -e -- "$TYPINGS")

if [ "$VERIFY_NO_NODE" = "1" ]; then
    [ -n "$INSTALLER" ] || die_usage "--verify-no-node requires --installer"
    [ -n "$INSTALL_ROOT" ] || die_usage "--verify-no-node requires --install-root"
    require_file "--installer" "$INSTALLER"
    [ -x "$INSTALLER" ] || die_usage "--installer is not executable: $INSTALLER"
    require_directory "--install-root" "$INSTALL_ROOT"
    INSTALLER=$(realpath -e -- "$INSTALLER")
    INSTALL_ROOT=$(realpath -e -- "$INSTALL_ROOT")
    case "$INSTALL_ROOT" in
        "$REPO_REAL"|"$REPO_REAL"/*)
            die_usage "--install-root must be outside the repository: $INSTALL_ROOT"
            ;;
    esac
fi

if [ -z "$OUTPUT" ]; then
    OUTPUT="/tmp/opencode/native-gates/$(date -u +%Y%m%dT%H%M%SZ)"
    if [ -e "$OUTPUT" ]; then
        OUTPUT="$OUTPUT-$$"
    fi
fi
require_absolute "--output" "$OUTPUT"
OUTPUT=$(realpath -m -- "$OUTPUT")
case "$OUTPUT" in
    "$REPO_REAL"|"$REPO_REAL"/*)
        die_usage "--output must be outside the repository: $OUTPUT"
        ;;
esac
if [ -e "$OUTPUT" ] && [ ! -d "$OUTPUT" ]; then
    die_usage "--output exists but is not a directory: $OUTPUT"
fi
mkdir -p -- "$OUTPUT" || die_usage "cannot create output directory: $OUTPUT"

ENGINE_VERSION=''
if ! ENGINE_VERSION=$(timeout 30 "$ENGINE" --version 2>&1); then
    die_usage "engine --version failed: $ENGINE"
fi
[ "$ENGINE_VERSION" = "$EXPECTED_ENGINE_VERSION" ] \
    || die_usage "engine version mismatch: expected '$EXPECTED_ENGINE_VERSION', got '$ENGINE_VERSION'"

ENGINE_SHA256=$(sha256sum -- "$ENGINE" | awk '{print $1}') \
    || die_usage "cannot hash engine: $ENGINE"
[ "$ENGINE_SHA256" = "$EXPECTED_ENGINE_SHA256" ] \
    || die_usage "engine SHA-256 mismatch: expected $EXPECTED_ENGINE_SHA256, got $ENGINE_SHA256"

ENGINE_SIZE=$(stat -c '%s' -- "$ENGINE") \
    || die_usage "cannot stat engine: $ENGINE"
[ "$ENGINE_SIZE" = "$EXPECTED_ENGINE_SIZE" ] \
    || die_usage "engine size mismatch: expected $EXPECTED_ENGINE_SIZE, got $ENGINE_SIZE"

printf 'NATIVE_GATES runner\n'
printf 'repo: %s\n' "$REPO_REAL"
printf 'package: %s\n' "$PACKAGE"
printf 'engine: %s (%s, sha256=%s, size=%s)\n' \
    "$ENGINE" "$ENGINE_VERSION" "$ENGINE_SHA256" "$ENGINE_SIZE"
printf 'asset root: %s\n' "$ASSET_ROOT"
printf 'typings: %s\n' "$TYPINGS"
printf 'output: %s\n' "$OUTPUT"

BUILD_LOG="$OUTPUT/00-build.log"
BUILD_STATUS='SKIP'
BUILD_RC=0
if [ "$SKIP_BUILD" = "1" ]; then
    printf 'build: skipped (--skip-build)\n' >"$BUILD_LOG"
else
    printf 'build: cargo build --release -p wxml-lsp\n' >"$BUILD_LOG"
    (cd "$REPO_ROOT" && CARGO_TERM_COLOR=never \
        cargo build --release -p wxml-lsp) >>"$BUILD_LOG" 2>&1 || BUILD_RC=$?
    if [ "$BUILD_RC" -eq 0 ]; then
        BUILD_STATUS='PASS'
    else
        BUILD_STATUS="FAIL(rc=$BUILD_RC)"
    fi
fi
if [ "$BUILD_RC" -ne 0 ]; then
    printf 'NATIVE_GATES: FAIL 0/%s (build failed; see %s)\n' \
        "$EXPECTED_NATIVE_GATES" "$BUILD_LOG" >&2
    tail -n 60 "$BUILD_LOG" >&2
    exit 1
fi
printf 'build: %s\n' "$BUILD_STATUS"

ENV_ARGS=(
    env
    -u WXML_TEST_NATIVE_PACKAGE
    -u WXML_TYPE_BACKEND
    -u WXML_BACKEND_DIR
    -u WXML_NODE
    WXML_TEST_NATIVE=1
    WXML_REQUIRE_REAL_ENGINE=1
    CARGO_TERM_COLOR=never
    "TSGO_BIN=$ENGINE"
    "TSGO_ASSET_ROOT=$ASSET_ROOT"
    "TSGO_TYPINGS_ROOT=$TYPINGS"
)

FAILED_GATES=()
GATE_SUMMARY=()

run_gate() {
    local name=$1
    local target=$2
    local filter=$3
    local expected=$4
    local log=$5
    local rc=0
    local line=''
    local passed=''
    local -a filter_args=()
    if [ -n "$filter" ]; then
        filter_args=("$filter")
    fi

    (cd "$REPO_ROOT" && "${ENV_ARGS[@]}" cargo test --release -p wxml-lsp \
        --test "$target" "${filter_args[@]}" -- --ignored --test-threads=1) \
        >"$log" 2>&1 || rc=$?

    line=$(grep -E '^test result: (ok|FAILED)\.' "$log" | tail -n 1 || true)
    passed=$(printf '%s\n' "$line" \
        | sed -nE 's/^test result: [a-zA-Z]+\. ([0-9]+) passed.*/\1/p')
    [ -n "$passed" ] || passed=0

    if [ "$rc" -eq 0 ] && [ "$passed" -eq "$expected" ]; then
        printf 'GATE %s: PASS %s/%s\n' "$name" "$passed" "$expected"
        GATE_SUMMARY+=("PASS $name $passed/$expected")
        return 0
    fi

    printf 'GATE %s: FAIL rc=%s passed=%s/%s\n' \
        "$name" "$rc" "$passed" "$expected" >&2
    printf -- '--- %s (tail) ---\n' "$name" >&2
    tail -n 60 "$log" >&2 || true
    GATE_SUMMARY+=("FAIL $name rc=$rc passed=$passed/$expected")
    FAILED_GATES+=("$name")
    return 1
}

PASSED_GATE_TESTS=0
if run_gate 'native_parity::native_engine_real' 'native_parity' 'native_engine_real' \
    "$NATIVE_ENGINE_GATES" "$OUTPUT/01-native-engine-real.log"; then
    PASSED_GATE_TESTS=$((PASSED_GATE_TESTS + NATIVE_ENGINE_GATES))
fi
if run_gate 'native_parity::symbols_edits' 'native_parity' 'symbols_edits' \
    "$NATIVE_EDIT_GATES" "$OUTPUT/02-symbols-edits.log"; then
    PASSED_GATE_TESTS=$((PASSED_GATE_TESTS + NATIVE_EDIT_GATES))
fi
if run_gate 'stdio_type_loop' 'stdio_type_loop' '' \
    "$NATIVE_STDIO_GATES" "$OUTPUT/03-stdio-type-loop.log"; then
    PASSED_GATE_TESTS=$((PASSED_GATE_TESTS + NATIVE_STDIO_GATES))
fi
if run_gate 'performance_bench::G-P5-PERF-01' 'performance_bench' '' \
    "$NATIVE_PERF_GATES" "$OUTPUT/04-performance-bench.log"; then
    PASSED_GATE_TESTS=$((PASSED_GATE_TESTS + NATIVE_PERF_GATES))
fi
if run_gate 'q2_stability::G-P5-STABILITY-01' 'q2_stability' '' \
    "$NATIVE_STABILITY_GATES" "$OUTPUT/05-q2-stability.log"; then
    PASSED_GATE_TESTS=$((PASSED_GATE_TESTS + NATIVE_STABILITY_GATES))
fi
if run_gate 'native_parity::real_matrix' 'native_parity' 'real_matrix' \
    "$NATIVE_MATRIX_GATES" "$OUTPUT/06-real-matrix.log"; then
    PASSED_GATE_TESTS=$((PASSED_GATE_TESTS + NATIVE_MATRIX_GATES))
fi

NO_NODE_STATUS='NOT_REQUESTED'
NO_NODE_RC=0
if [ "$VERIFY_NO_NODE" = "1" ]; then
    (cd "$REPO_ROOT" && env -u WXML_TEST_NATIVE_PACKAGE \
        WXML_REQUIRE_REAL_ENGINE=1 "TSGO_BIN=$ENGINE" \
        scripts/verify-no-node.sh --installer "$INSTALLER" --root "$INSTALL_ROOT") \
        >"$OUTPUT/07-verify-no-node.log" 2>&1 || NO_NODE_RC=$?
    if [ "$NO_NODE_RC" -eq 0 ]; then
        NO_NODE_STATUS='PASS'
        printf 'NO_NODE_SCOPE: PASS (installer scope)\n'
    else
        NO_NODE_STATUS="FAIL(rc=$NO_NODE_RC)"
        printf 'NO_NODE_SCOPE: FAIL rc=%s\n' "$NO_NODE_RC" >&2
        tail -n 60 "$OUTPUT/04-verify-no-node.log" >&2 || true
    fi
fi

SUMMARY="$OUTPUT/summary.txt"
{
    printf 'NATIVE_GATES runner\n'
    printf 'repo: %s\n' "$REPO_REAL"
    printf 'head: %s\n' "$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || printf 'unknown')"
    printf 'package: %s\n' "$PACKAGE"
    printf 'engine: %s\n' "$ENGINE"
    printf 'engine_version: %s\n' "$ENGINE_VERSION"
    printf 'engine_sha256: %s\n' "$ENGINE_SHA256"
    printf 'engine_size: %s\n' "$ENGINE_SIZE"
    printf 'asset_root: %s\n' "$ASSET_ROOT"
    printf 'typings_root: %s\n' "$TYPINGS"
    printf 'build: %s\n' "$BUILD_STATUS"
    printf 'gate: %s\n' "${GATE_SUMMARY[@]}"
    printf 'no_node_scope: %s\n' "$NO_NODE_STATUS"
} >"$SUMMARY"

GATES_OK=0
if [ "${#FAILED_GATES[@]}" -eq 0 ]; then
    printf 'NATIVE_GATES: PASS %s/%s\n' \
        "$EXPECTED_NATIVE_GATES" "$EXPECTED_NATIVE_GATES"
    GATES_OK=1
else
    printf 'NATIVE_GATES: FAIL %s/%s\n' \
        "$PASSED_GATE_TESTS" "$EXPECTED_NATIVE_GATES" >&2
    for gate in "${FAILED_GATES[@]}"; do
        printf 'FAILED_GATE: %s\n' "$gate" >&2
    done
fi
if [ "$NO_NODE_RC" -ne 0 ]; then
    printf 'FAILED_SCOPE: no-node-installer (rc=%s)\n' "$NO_NODE_RC" >&2
fi
printf 'logs: %s\n' "$OUTPUT"
printf 'summary: %s\n' "$SUMMARY"

if [ "$GATES_OK" -eq 1 ] && [ "$NO_NODE_RC" -eq 0 ]; then
    exit 0
fi
exit 1
