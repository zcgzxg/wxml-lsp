#!/usr/bin/env bash
# F3-02 installer-scope no-Node guard.
#
# Verifies an existing schema v2 native installation (or, with --legacy, a
# schema v1 read-only installation) with `PATH` containing only Node tripwire
# shims. This is auxiliary evidence: a PATH shim cannot rule out an
# absolute-path Node invocation, so this script does NOT close F3-02.
#
# Required (environment or flags):
#   WXML_LSP_INSTALL / --installer   absolute wxml-lsp-install binary
#   WXML_INSTALL_ROOT / --root       absolute install root (outside repo)
#
# Optional:
#   --legacy             expect a schema v1 root; check current/verify/env only
#   TSGO_ASSET           pinned 7.1.0-beta tarball (SHA-256/size checked, then used;
#                        TSGO_NATIVE_TARBALL is accepted as an alias)
#   TSGO_BIN             extracted engine/lib/tsc (SHA-256/size checked)
#   WXML_REQUIRE_REAL_ENGINE=1|true  fail closed when no real engine is supplied
#   WXML_REQUIRE_LSP_NATIVE=1|true   fail closed unless main.rs has no Node backend
#   WXML_TIMEOUT_SECS    per-command timeout (default 60; must be > 0)
#   WXML_TEST_TMPDIR     temp base (default /tmp/opencode; must be under /tmp)
#
# No network is intentionally initiated, no repository file is written, and a
# cleanup trap removes every temporary file. The system Node is never placed on
# the child PATH. Commands run in their own process group with `timeout -k 5`;
# a broken tripwire fails the control before any PASS is printed.

set -euo pipefail
set -m
umask 077

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
INSTALLER=${WXML_LSP_INSTALL:-}
ROOT=${WXML_INSTALL_ROOT:-}
TIMEOUT_SECS=${WXML_TIMEOUT_SECS:-60}
LEGACY=0
EXPECTED_ENGINE_SHA256=a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70
EXPECTED_ENGINE_SIZE=24522912

usage() {
    cat <<'EOF'
F3-02 installer-scope no-Node guard.

Verifies an existing schema v2 native installation (or, with --legacy, a
schema v1 read-only installation) with PATH containing only Node tripwire
shims. This is auxiliary evidence: a PATH shim cannot rule out an absolute-path
Node invocation, so this script does NOT close F3-02.

Required (environment or flags):
  WXML_LSP_INSTALL / --installer   absolute wxml-lsp-install binary
  WXML_INSTALL_ROOT / --root       absolute install root (outside repo)

Optional:
  --legacy             expect a schema v1 root; check current/verify/env only
  TSGO_ASSET           pinned 7.1.0-beta tarball (checked, then used;
                       TSGO_NATIVE_TARBALL is accepted as an alias)
  TSGO_BIN             extracted engine/lib/tsc (SHA-256/size checked)
  WXML_REQUIRE_REAL_ENGINE=1|true  fail closed when no real engine is supplied
  WXML_REQUIRE_LSP_NATIVE=1|true   fail closed unless main.rs has no Node backend
  WXML_TIMEOUT_SECS    per-command timeout (default 60; must be > 0)
  WXML_TEST_TMPDIR     temp base (default /tmp/opencode; must be under /tmp)

No network is intentionally initiated, no repository file is written, and a
cleanup trap removes every temporary file. The system Node is never placed on
the child PATH.
EOF
}

die_usage() {
    printf 'ERROR: %s\n' "$*" >&2
    printf 'Run %s --help for usage.\n' "$0" >&2
    exit 2
}

fail_check() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 1
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail_check "$1 is required"
}

normalize_bool() {
    case "$(printf '%s' "${1:-0}" | tr '[:upper:]' '[:lower:]')" in
        1|true) printf '1' ;;
        *) printf '0' ;;
    esac
}

reject_control_chars() {
    local label=$1
    local value=$2
    local cleaned
    cleaned=$(printf '%s' "$value" | LC_ALL=C tr -d '\001-\037\177')
    if [ "$cleaned" != "$value" ]; then
        fail_check "$label contains control characters"
    fi
    case "$value" in
        *\'*) fail_check "$label contains a single quote" ;;
    esac
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --installer)
            [ "$#" -ge 2 ] || die_usage "--installer requires a value"
            INSTALLER=$2
            shift 2
            ;;
        --root)
            [ "$#" -ge 2 ] || die_usage "--root requires a value"
            ROOT=$2
            shift 2
            ;;
        --legacy)
            LEGACY=1
            shift
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

[ -n "$INSTALLER" ] || die_usage "wxml-lsp-install is required (--installer or WXML_LSP_INSTALL)"
[ -n "$ROOT" ] || die_usage "install root is required (--root or WXML_INSTALL_ROOT)"
case "$INSTALLER" in
    /*) ;;
    *) die_usage "--installer must be an absolute path" ;;
esac
case "$ROOT" in
    /*) ;;
    *) die_usage "--root must be an absolute path" ;;
esac
[ -x "$INSTALLER" ] || fail_check "installer is not executable: $INSTALLER"
[ -d "$ROOT" ] || fail_check "install root does not exist: $ROOT"

case "$TIMEOUT_SECS" in
    ''|*[!0-9]*) fail_check "WXML_TIMEOUT_SECS must be a positive integer: $TIMEOUT_SECS" ;;
esac
[ "$TIMEOUT_SECS" -gt 0 ] || fail_check "WXML_TIMEOUT_SECS must be greater than zero"

REQUIRE_REAL=$(normalize_bool "${WXML_REQUIRE_REAL_ENGINE:-0}")
REQUIRE_LSP=$(normalize_bool "${WXML_REQUIRE_LSP_NATIVE:-0}")

require_cmd timeout
require_cmd tr
require_cmd grep
require_cmd awk
require_cmd head
require_cmd stat
require_cmd sha256sum
require_cmd realpath
require_cmd mktemp
require_cmd sed

REPO_REAL=$(realpath -m -- "$REPO_ROOT")
ROOT_REAL=$(realpath -m -- "$ROOT")
case "$ROOT_REAL" in
    "$REPO_REAL"|"$REPO_REAL"/*)
        fail_check "install root must be outside the repository: $ROOT_REAL"
        ;;
esac

TMP_BASE=${WXML_TEST_TMPDIR:-/tmp/opencode}
case "$TMP_BASE" in
    /*) ;;
    *) fail_check "WXML_TEST_TMPDIR must be absolute: $TMP_BASE" ;;
esac
TMP_REAL=$(realpath -m -- "$TMP_BASE")
reject_control_chars "WXML_TEST_TMPDIR" "$TMP_REAL"
case "$TMP_REAL" in
    "$REPO_REAL"|"$REPO_REAL"/*)
        fail_check "WXML_TEST_TMPDIR must be outside the repository: $TMP_REAL"
        ;;
esac
case "$TMP_REAL" in
    /tmp/*) ;;
    *) fail_check "WXML_TEST_TMPDIR must live under /tmp: $TMP_REAL" ;;
esac
mkdir -p -- "$TMP_REAL"

WORK=""
cleanup() {
    if [ -n "$WORK" ]; then
        rm -rf -- "$WORK"
    fi
}
trap cleanup EXIT HUP INT TERM

WORK=$(mktemp -d -- "$TMP_REAL/verify-no-node.XXXXXX")
reject_control_chars "temporary work directory" "$WORK"
SHIMS="$WORK/path"
TRIPWIRE="$WORK/tripwire"
MARKER="$TRIPWIRE/NODE_WAS_EXECUTED"
MARKER_LOG="$TRIPWIRE/node-exec-attempts.log"
HOME_DIR="$WORK/home"
TMP_DIR="$WORK/tmp"
mkdir -p -- "$SHIMS" "$TRIPWIRE" "$HOME_DIR" "$TMP_DIR"
chmod 700 -- "$TRIPWIRE"

for name in node nodejs npm npx pnpm yarn bun; do
    {
        printf '#!/bin/sh\n'
        printf 'printf "%%s %%s\\n" "$0" "$*" >> %s\n' "$(printf '%q' "$MARKER_LOG")"
        printf ': > %s\n' "$(printf '%q' "$MARKER")"
        printf 'exit 99\n'
    } > "$SHIMS/$name"
    chmod 755 -- "$SHIMS/$name"
done

clear_tripwire() {
    local path
    for path in "$MARKER" "$MARKER_LOG"; do
        if [ -e "$path" ]; then
            rm -f -- "$path" || fail_check "could not remove tripwire evidence: $path"
        fi
    done
}

assert_tripwire_absent() {
    if [ -e "$MARKER" ] || [ -e "$MARKER_LOG" ]; then
        fail_check "tripwire evidence is present after the positive control"
    fi
}

check_tripwire() {
    local label=$1
    if [ -e "$MARKER" ] || [ -e "$MARKER_LOG" ]; then
        printf 'NO_NODE: FAIL (%s executed a Node shim)\n' "$label" >&2
        if [ -f "$MARKER_LOG" ]; then
            sed 's/^/  attempt: /' "$MARKER_LOG" >&2
        fi
        if [ -f "$MARKER" ]; then
            printf '  marker: %s\n' "$MARKER" >&2
        fi
        exit 1
    fi
}

# Last command's redirected output and exit status from run_grouped.
LAST_STDOUT=""
LAST_STDERR=""
LAST_RC=0

run_grouped() {
    local label=$1
    shift
    LAST_STDOUT="$WORK/$label.stdout"
    LAST_STDERR="$WORK/$label.stderr"
    LAST_RC=0
    "$@" >"$LAST_STDOUT" 2>"$LAST_STDERR" </dev/null &
    local pid=$!
    wait "$pid" || LAST_RC=$?
    # `set -m` puts the background job in its own process group. Clean up any
    # grandchildren left behind by a timeout or a misbehaving child.
    if kill -0 -- "-$pid" 2>/dev/null; then
        kill -KILL -- "-$pid" || fail_check "could not kill process group $pid for $label"
    fi
}

assert_stdout_matches() {
    local label=$1
    local file=$2
    local pattern=$3
    if ! grep -Eq -- "$pattern" "$file"; then
        printf 'ERROR: %s output does not match /%s/\n' "$label" "$pattern" >&2
        cat "$file" >&2
        fail_check "$label output assertion failed"
    fi
}

assert_stdout_absent() {
    local label=$1
    local file=$2
    local pattern=$3
    if grep -Eq -- "$pattern" "$file"; then
        printf 'ERROR: %s output unexpectedly matches /%s/\n' "$label" "$pattern" >&2
        cat "$file" >&2
        fail_check "$label forbidden output assertion failed"
    fi
}

positive_control() {
    run_grouped control timeout -k 5 5 env -i \
        PATH="$SHIMS" LANG=C LC_ALL=C /bin/sh -c 'node --version'
    if [ "$LAST_RC" -ne 99 ]; then
        cat "$LAST_STDOUT" >&2
        cat "$LAST_STDERR" >&2
        fail_check "tripwire positive control exited $LAST_RC, expected 99"
    fi
    if [ ! -f "$MARKER" ]; then
        fail_check "tripwire positive control did not create $MARKER"
    fi
    if [ ! -f "$MARKER_LOG" ] || ! grep -q 'node --version' "$MARKER_LOG"; then
        fail_check "tripwire positive control did not record the attempt"
    fi
    clear_tripwire
    assert_tripwire_absent
    printf 'NO_NODE: positive control exercised (shim exit 99, marker created)\n'
}

run_installer() {
    local label=$1
    shift
    run_grouped "$label" timeout -k 5 "$TIMEOUT_SECS" env -i \
        PATH="$SHIMS" \
        HOME="$HOME_DIR" \
        XDG_DATA_HOME="$HOME_DIR" \
        TMPDIR="$TMP_DIR" \
        LANG=C \
        LC_ALL=C \
        NO_COLOR=1 \
        WXML_NODE="$SHIMS/node" \
        WXML_TYPE_BACKEND="$SHIMS/server.js" \
        WXML_BACKEND_DIR="$SHIMS" \
        HTTP_PROXY=http://127.0.0.1:9 \
        HTTPS_PROXY=http://127.0.0.1:9 \
        ALL_PROXY=http://127.0.0.1:9 \
        "$INSTALLER" "$@"
    if [ "$LAST_RC" -eq 124 ]; then
        printf 'ERROR: %s timed out\n' "$label" >&2
        cat "$LAST_STDOUT" >&2
        cat "$LAST_STDERR" >&2
        exit 1
    fi
    if [ "$LAST_RC" -ne 0 ]; then
        printf 'ERROR: %s failed with exit %s\n' "$label" "$LAST_RC" >&2
        cat "$LAST_STDOUT" >&2
        cat "$LAST_STDERR" >&2
        exit 1
    fi
    check_tripwire "$label"
}

printf 'REPO: %s\n' "$REPO_REAL"
printf 'INSTALLER: %s\n' "$INSTALLER"
printf 'INSTALL_ROOT: %s\n' "$ROOT_REAL"
printf 'PATH: %s\n' "$SHIMS"

positive_control

if [ "$LEGACY" = "1" ]; then
    printf 'MODE: legacy (schema v1 read-only)\n'

    run_installer verify verify --root "$ROOT"
    assert_stdout_matches verify "$LAST_STDOUT" '^verified .+ \(legacy, read-only\)$'
    assert_stdout_matches verify "$LAST_STDOUT" '^binary: /'
    printf 'verify: %s\n' "$(head -n 1 "$LAST_STDOUT")"

    run_installer current current --root "$ROOT"
    assert_stdout_matches current "$LAST_STDOUT" '^version: [^ ]'
    assert_stdout_matches current "$LAST_STDOUT" '^schema: 1 \(legacy, read-only'
    assert_stdout_matches current "$LAST_STDOUT" '^binary: /'
    printf 'current: %s\n' "$(head -n 1 "$LAST_STDOUT")"

    run_installer env env --root "$ROOT"
    assert_stdout_matches env "$LAST_STDOUT" '^export WXML_LSP_BIN='
    assert_stdout_absent env "$LAST_STDOUT" 'WXML_TYPE_BACKEND'
    assert_stdout_absent env "$LAST_STDOUT" 'WXML_NODE'
    assert_stdout_absent env "$LAST_STDOUT" 'WXML_BACKEND_DIR'
    printf 'env: %s\n' "$(head -n 1 "$LAST_STDOUT")"

    printf 'NO_NODE: PASS (legacy read-only scope: verify/current/env)\n'
    printf 'RESIDUAL: schema v1 install/upgrade/rollback were not exercised\n'
    printf 'RESIDUAL: execve/strace process audit is not performed; PATH shims cannot exclude absolute paths\n'
    printf 'F3-01/F3-15: PARTIAL (legacy read-only scope; no strace audit, no real legacy package)\n'
    exit 0
fi

run_installer verify verify --root "$ROOT"
assert_stdout_matches verify "$LAST_STDOUT" '^verified .+ \(native\)$'
assert_stdout_matches verify "$LAST_STDOUT" '^engine version: 7\.1\.0-beta$'
assert_stdout_matches verify "$LAST_STDOUT" '^binary: /'
printf 'verify: %s\n' "$(head -n 1 "$LAST_STDOUT")"

run_installer current current --root "$ROOT"
assert_stdout_matches current "$LAST_STDOUT" '^version: [^ ]'
assert_stdout_matches current "$LAST_STDOUT" '^engine: typescript 7\.1\.0-beta$'
assert_stdout_matches current "$LAST_STDOUT" '^binary: /'
printf 'current: %s\n' "$(head -n 1 "$LAST_STDOUT")"

run_installer env env --root "$ROOT"
assert_stdout_matches env "$LAST_STDOUT" '^export WXML_LSP_BIN='
assert_stdout_absent env "$LAST_STDOUT" 'WXML_TYPE_BACKEND'
printf 'env: %s\n' "$(head -n 1 "$LAST_STDOUT")"

printf 'NO_NODE: PASS (installer scope: verify/current/env)\n'

LSP_LINE=""
MAIN="$REPO_ROOT/crates/wxml-lsp/src/main.rs"
if [ -f "$MAIN" ]; then
    LSP_LINE=$(grep -nE 'NodeBackend::new|fn serve_node\(\)' "$MAIN" | head -n 1 || true)
    LSP_LINE=${LSP_LINE%%:*}
fi
if [ ! -f "$MAIN" ]; then
    LSP_STATUS=UNVERIFIED
    LSP_DETAIL="$MAIN does not exist"
elif [ -n "$LSP_LINE" ]; then
    LSP_STATUS=BLOCKED
    LSP_DETAIL="$MAIN:$LSP_LINE still wires the Node backend"
else
    LSP_STATUS=PASS
    LSP_DETAIL="$MAIN has no NodeBackend or serve_node(); --stdio is native-only"
fi
printf 'LSP_NATIVE: %s (%s)\n' "$LSP_STATUS" "$LSP_DETAIL"

REAL_STATUS=SKIP
REAL_EXIT=0
ASSET=${TSGO_ASSET:-${TSGO_NATIVE_TARBALL:-}}
if [ -n "$ASSET" ] || [ -n "${TSGO_BIN:-}" ]; then
    ENGINE_BIN=${TSGO_BIN:-}
    if [ -n "$ASSET" ]; then
        require_cmd tar
        [ -f "$ASSET" ] || fail_check "TSGO_ASSET is not a file: $ASSET"
        reject_control_chars "TSGO_ASSET" "$ASSET"
        asset_sha=$(sha256sum -- "$ASSET" | awk '{print $1}')
        [ "$asset_sha" = "59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855" ] \
            || fail_check "TSGO_ASSET SHA-256 mismatch: $asset_sha"
        asset_size=$(stat -c '%s' -- "$ASSET")
        [ "$asset_size" = "10175135" ] \
            || fail_check "TSGO_ASSET size mismatch: $asset_size"
        if [ -z "$ENGINE_BIN" ]; then
            mkdir -p -- "$WORK/asset"
            tar --no-same-owner -xzf "$ASSET" -C "$WORK/asset" \
                || fail_check "failed to extract TSGO_ASSET"
            ENGINE_BIN="$WORK/asset/package/lib/tsc"
        fi
    fi
    case "$ENGINE_BIN" in
        /*) ;;
        *) fail_check "TSGO_BIN must be an absolute path" ;;
    esac
    reject_control_chars "TSGO_BIN" "$ENGINE_BIN"
    [ -x "$ENGINE_BIN" ] || fail_check "real engine is not executable: $ENGINE_BIN"
    engine_sha=$(sha256sum -- "$ENGINE_BIN" | awk '{print $1}')
    [ "$engine_sha" = "$EXPECTED_ENGINE_SHA256" ] \
        || fail_check "real engine SHA-256 mismatch: $engine_sha"
    engine_size=$(stat -c '%s' -- "$ENGINE_BIN")
    [ "$engine_size" = "$EXPECTED_ENGINE_SIZE" ] \
        || fail_check "real engine size mismatch: $engine_size"

    run_grouped real-engine timeout -k 5 10 env -i \
        PATH="$SHIMS" LANG=C LC_ALL=C "$ENGINE_BIN" --version
    if [ "$LAST_RC" -eq 124 ]; then
        cat "$LAST_STDOUT" >&2
        cat "$LAST_STDERR" >&2
        fail_check "real engine --version timed out"
    fi
    if [ "$LAST_RC" -ne 0 ]; then
        cat "$LAST_STDOUT" >&2
        cat "$LAST_STDERR" >&2
        fail_check "real engine --version failed with exit $LAST_RC"
    fi
    check_tripwire "real-engine"
    engine_version=$(cat "$LAST_STDOUT")
    [ "$engine_version" = "Version 7.1.0-beta" ] \
        || fail_check "unexpected real engine version: $engine_version"
    REAL_STATUS="PASS (sha256=$engine_sha, $engine_version)"
elif [ "$REQUIRE_REAL" = "1" ]; then
    REAL_STATUS=BLOCKED
    REAL_EXIT=1
fi
printf 'REAL_ENGINE: %s\n' "$REAL_STATUS"
if [ "$REAL_STATUS" = "SKIP" ]; then
    printf 'REAL_ENGINE: SKIP (set TSGO_ASSET or TSGO_BIN; WXML_REQUIRE_REAL_ENGINE=1|true fails closed)\n'
fi

printf 'RESIDUAL: execve/strace process audit is not performed; PATH shims cannot exclude absolute paths\n'
printf 'RESIDUAL: native stdio LSP session is not exercised here; main.rs is statically checked for the removed Node backend\n'
printf 'RESIDUAL: two-version real-engine upgrade/rollback is not exercised\n'
printf 'F3-02: BLOCKED (installer scope only; no closure claim)\n'

if [ "$REQUIRE_LSP" = "1" ] && [ "$LSP_STATUS" != "PASS" ]; then
    printf 'ERROR: WXML_REQUIRE_LSP_NATIVE=1|true but native LSP is %s\n' "$LSP_STATUS" >&2
    exit 1
fi

exit "$REAL_EXIT"
