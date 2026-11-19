#!/usr/bin/env bash
# F3-09 real legacy schema v1 release acceptance.
#
# Builds a full-fidelity schema v1 release with the production v1 packager
# (`wxml-lsp-package`, no `native` subcommand) from real artifacts:
#
#   - the release `wxml-lsp` / `wxml-lsp-install` Rust binaries;
#   - the real `backends/typescript` build output and locked node_modules;
#   - the real `editors/zed` wasm32-wasip2 component.
#
# The resulting package is installed with the production schema v1 installer
# path, then the opt-in `f3_legacy_real` test migrates it to the pinned real
# native channel and checks:
#
#   - successful migration, legacy tree byte-identical, previous=null;
#   - pre-switch failure against a real-package-derived bad native archive;
#   - post-switch interruption at the production AfterSwitch checkpoint;
#   - native rollback refusing the schema v1 target;
#   - `recover` never restoring or executing the legacy package;
#   - no Node/server.js process and no legacy entry execution.
#
# The package is NOT a historical published artifact. It is a full-fidelity
# build from a clean `git archive` revision (default HEAD) through the
# production v1 packager; the test output labels that limitation.
#
# Usage:
#   scripts/f3-legacy-real.sh [--package DIR] [--output DIR] [--source-rev REV]
#                             [--skip-build] [--node PATH] [--required] [--help]
#
# Options:
#   --package DIR   reuse an existing v1 release output directory (contains
#                   index.json); nothing is built
#   --output DIR    build/evidence root (default /tmp/opencode/f3-legacy-real)
#   --source-rev REV  build from `git archive REV` instead of the live worktree
#                   (default HEAD; use a commit for reproducible evidence)
#   --skip-build    reuse <output>/legacy-release instead of rebuilding
#   --node PATH     real Node executable used for `npm ci`/build and the
#                   legacy `--backend node` smoke
#   --required      fail closed if the v1 package or pinned native assets are
#                   missing (sets WXML_REQUIRE_LEGACY_PACKAGE=1 and
#                   WXML_REQUIRE_REAL_ENGINE=1)
#
# Environment:
#   WXML_F3_LEGACY_PACKAGE  release output directory (also accepts --package)
#   WXML_F3_LEGACY_SOURCE_REV  build revision (default HEAD; `worktree` builds
#                           the live dirty tree instead of `git archive`)
#   WXML_F3_LEGACY_BUILT_AT fixed --built-at timestamp
#   TSGO_NATIVE_TARBALL     7.1.0-beta typescript-linux-x64 tarball
#   TSGO_TYPINGS_TARBALL    miniprogram-api-typings-5.3.1 tarball
#   WXML_F3_ZED_WASM        prebuilt zed wasm component (built if absent)
#
# This script is evidence-only and is not wired into the default CI: it needs
# Node/npm and pinned native assets. Nothing is written inside the repository
# and nothing is committed. Logs stay under <output>/logs.

set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
OUTPUT=${WXML_F3_LEGACY_OUTPUT:-/tmp/opencode/f3-legacy-real}
PACKAGE=${WXML_F3_LEGACY_PACKAGE:-}
SOURCE_REV=${WXML_F3_LEGACY_SOURCE_REV:-HEAD}
SKIP_BUILD=0
NODE_BIN=${WXML_F3_REAL_NODE:-}
REQUIRED=0
BUILT_AT=${WXML_F3_LEGACY_BUILT_AT:-2026-11-20T00:00:00Z}

usage() {
    sed -n '2,54p' "$0" | sed 's/^# \{0,1\}//'
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

note() {
    printf 'F3-LEGACY: %s\n' "$*"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --package)
            [ "$#" -ge 2 ] || die "--package requires a directory"
            PACKAGE=$2
            shift 2
            ;;
        --package=*)
            PACKAGE=${1#*=}
            shift
            ;;
        --output)
            [ "$#" -ge 2 ] || die "--output requires a directory"
            OUTPUT=$2
            shift 2
            ;;
        --output=*)
            OUTPUT=${1#*=}
            shift
            ;;
        --source-rev)
            [ "$#" -ge 2 ] || die "--source-rev requires a git revision"
            SOURCE_REV=$2
            shift 2
            ;;
        --source-rev=*)
            SOURCE_REV=${1#*=}
            shift
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --node)
            [ "$#" -ge 2 ] || die "--node requires a path"
            NODE_BIN=$2
            shift 2
            ;;
        --node=*)
            NODE_BIN=${1#*=}
            shift
            ;;
        --required)
            REQUIRED=1
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

mkdir -p -- "$OUTPUT/logs" "$OUTPUT/tmp"
VERSIONS_LOG=$OUTPUT/logs/f3-legacy-real-versions.txt
TEST_LOG=$OUTPUT/logs/f3-legacy-real.log

if [ "$REQUIRED" -eq 1 ]; then
    export WXML_REQUIRE_LEGACY_PACKAGE=1
    export WXML_REQUIRE_REAL_ENGINE=1
fi

find_node() {
    if [ -n "$NODE_BIN" ]; then
        [ -x "$NODE_BIN" ] || die "--node is not executable: $NODE_BIN"
        printf '%s\n' "$NODE_BIN"
        return
    fi
    local candidate
    candidate=$(command -v node || true)
    [ -n "$candidate" ] || die "node is not on PATH; pass --node or set WXML_F3_REAL_NODE"
    printf '%s\n' "$candidate"
}

resolve_asset() {
    local variable=$1 fallback=$2
    local value=${!variable:-$fallback}
    if [ -n "$value" ] && [ -f "$value" ]; then
        printf '%s\n' "$value"
    fi
}

ENGINE_TARBALL=$(resolve_asset TSGO_NATIVE_TARBALL /tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz || true)
TYPINGS_TARBALL=$(resolve_asset TSGO_TYPINGS_TARBALL /tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz || true)
if [ -z "$ENGINE_TARBALL" ] || [ -z "$TYPINGS_TARBALL" ]; then
    if [ "$REQUIRED" -eq 1 ]; then
        die "pinned native tarballs are required (TSGO_NATIVE_TARBALL/TSGO_TYPINGS_TARBALL)"
    fi
    note "pinned native tarballs are absent; the migration half will print SKIP"
else
    export TSGO_NATIVE_TARBALL=$ENGINE_TARBALL
    export TSGO_TYPINGS_TARBALL=$TYPINGS_TARBALL
fi

build_package() {
    local node npm build_root source_mode commit
    node=$(find_node)
    npm=$(command -v npm || true)
    [ -n "$npm" ] || die "npm is not on PATH; a real backends/typescript build needs npm"

    if [ "$SOURCE_REV" = "worktree" ]; then
        build_root=$REPO_ROOT
        source_mode=worktree
        commit=$(git -C "$REPO_ROOT" rev-parse HEAD)
    else
        commit=$(git -C "$REPO_ROOT" rev-parse --verify "$SOURCE_REV^{commit}" 2>/dev/null) \
            || die "--source-rev is not a commit: $SOURCE_REV"
        build_root=$OUTPUT/source
        source_mode=git-archive
        rm -rf -- "$build_root"
        mkdir -p -- "$build_root"
        git -C "$REPO_ROOT" archive --format=tar "$commit" | tar -xf - -C "$build_root"
        [ -f "$build_root/Cargo.toml" ] || die "git archive did not produce a workspace"
    fi

    {
        printf 'source_mode=%s\n' "$source_mode"
        printf 'source_rev=%s\n' "$SOURCE_REV"
        printf 'source_commit=%s\n' "$commit"
        printf 'live_head=%s\n' "$(git -C "$REPO_ROOT" rev-parse HEAD)"
        printf 'live_status=%s\n' "$(git -C "$REPO_ROOT" status --porcelain | wc -l) modified paths"
        printf 'rustc=%s\n' "$(rustc --version)"
        printf 'cargo=%s\n' "$(cargo --version)"
        printf 'node=%s\n' "$("$node" --version)"
        printf 'npm=%s\n' "$("$npm" --version)"
        printf 'built_at=%s\n' "$BUILT_AT"
    } | tee "$VERSIONS_LOG"

    local cargo_target=$OUTPUT/target
    local zed_wasm=${WXML_F3_ZED_WASM:-}
    mkdir -p -- "$cargo_target" "$OUTPUT/npm-cache"

    note "building release wxml-lsp + wxml-lsp-install from $source_mode $commit"
    CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$cargo_target \
        cargo build --manifest-path "$build_root/Cargo.toml" \
        --release -p wxml-lsp -p wxml-delivery

    local lsp_bin=$cargo_target/release/wxml-lsp
    local install_bin=$cargo_target/release/wxml-lsp-install
    local packager=$cargo_target/release/wxml-lsp-package
    [ -x "$lsp_bin" ] || die "missing release wxml-lsp"
    [ -x "$install_bin" ] || die "missing release wxml-lsp-install"
    [ -x "$packager" ] || die "missing release wxml-lsp-package"

    local version
    version=$("$lsp_bin" --version | awk '{print $2}')
    [ -n "$version" ] || die "cannot read wxml-lsp version"

    if [ -z "$zed_wasm" ]; then
        note "building editors/zed wasm32-wasip2"
        CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$OUTPUT/zed-target \
            cargo build --manifest-path "$build_root/editors/zed/Cargo.toml" \
            --release --target wasm32-wasip2 --locked
        zed_wasm=$OUTPUT/zed-target/wasm32-wasip2/release/zed_wxml.wasm
    fi
    [ -f "$zed_wasm" ] || die "missing Zed wasm: $zed_wasm"
    export WXML_F3_ZED_WASM=$zed_wasm
    export WXML_F3_LSP_BIN=$lsp_bin
    export WXML_F3_INSTALL_BIN=$install_bin

    note "building backends/typescript in an isolated copy"
    rm -rf -- "$OUTPUT/backend"
    mkdir -p -- "$OUTPUT/backend"
    (
        cd "$build_root/backends/typescript"
        tar -cf - --exclude=node_modules --exclude=dist . | tar -xf - -C "$OUTPUT/backend"
    )
    (
        cd "$OUTPUT/backend"
        npm_config_cache=$OUTPUT/npm-cache "$npm" ci --no-audit --no-fund
        npm_config_cache=$OUTPUT/npm-cache "$npm" run build
    ) > "$OUTPUT/logs/node-build.log" 2>&1
    [ -f "$OUTPUT/backend/dist/server.js" ] || die "the real Node backend build produced no dist/server.js"

    local backend_version
    backend_version=$(cd "$OUTPUT/backend" && "$node" -e "process.stdout.write(require('./package.json').version)")
    [ -n "$backend_version" ] || backend_version=$version

    local stage=$OUTPUT/legacy-stage
    local release=$OUTPUT/legacy-release
    note "staging the schema v1 tree"
    rm -rf -- "$stage" "$release"
    mkdir -p -- "$stage/bin/wxml-type-backend" "$stage/zed-extension"
    install -m 0755 "$lsp_bin" "$stage/bin/wxml-lsp"
    install -m 0755 "$install_bin" "$stage/bin/wxml-lsp-install"
    cp -a "$OUTPUT/backend/dist/." "$stage/bin/wxml-type-backend/"
    cp -a "$OUTPUT/backend/node_modules" "$stage/bin/wxml-type-backend/node_modules"
    # npm's .bin entries are symlinks; the v1 packager requires a
    # self-contained tree and the release never needs them.
    rm -rf -- "$stage/bin/wxml-type-backend/node_modules/.bin"
    install -m 0644 "$zed_wasm" "$stage/zed-extension/extension.wasm"
    install -m 0644 "$build_root/editors/zed/extension.toml" "$stage/zed-extension/extension.toml"
    cp -a "$build_root/editors/zed/languages" "$stage/zed-extension/languages"
    install -m 0644 "$build_root/editors/zed/initialization-options.schema.json" \
        "$stage/zed-extension/initialization-options.schema.json"
    cat > "$stage/THIRD_PARTY_LICENSES.md" <<'LICENSES'
# Third-party licenses

This legacy schema v1 package payload contains:

- `wxml-lsp` and `wxml-lsp-install`: Rust binaries built from this workspace.
- `bin/wxml-type-backend/`: the `@wxml-lsp/type-bridge-probe` Node backend
  (`dist/`) plus its locked dependencies `typescript` 6.0.3 (Apache-2.0) and
  `miniprogram-api-typings` 5.3.1 (MIT). Their upstream license files are
  included in the corresponding `node_modules` directories.
- `zed-extension/`: the `zed_wxml.wasm` extension built from `editors/zed`.
LICENSES
    chmod 0644 "$stage/THIRD_PARTY_LICENSES.md"

    note "packaging the real schema v1 release with the production v1 packager"
    "$packager" --input "$stage" --output "$release" \
        --release-version "$version" \
        --platform linux-x86_64 \
        --protocol-version 2 \
        --node-minimum 24.0.0 \
        --backend-version "$backend_version" \
        --zed-version 0.0.1 \
        --built-at "$BUILT_AT" \
        --mtime 0 \
        --force | tee "$OUTPUT/logs/f3-legacy-package.log"

    note "verifying the archive with the production installer"
    "$install_bin" verify-package --from-dir "$release" --version "$version" --offline \
        | tee "$OUTPUT/logs/f3-legacy-verify-package.log"

    printf 'F3-EVIDENCE: F3-09 REAL :: real v1 package built by the production v1 packager: ' | tee -a "$VERSIONS_LOG"
    awk '/^package:|^protocol:|^sha256:|^size:/{printf "%s ", $0}' \
        "$OUTPUT/logs/f3-legacy-package.log" | tee -a "$VERSIONS_LOG"
    printf '\n' | tee -a "$VERSIONS_LOG"

    PACKAGE=$release
}

if [ -n "$PACKAGE" ]; then
    PACKAGE=$(realpath -m -- "$PACKAGE")
    [ -d "$PACKAGE" ] || die "--package is not a directory: $PACKAGE"
    [ -f "$PACKAGE/index.json" ] || die "--package has no index.json: $PACKAGE"
    note "reusing real schema v1 release output: $PACKAGE"
elif [ "$SKIP_BUILD" -eq 1 ]; then
    PACKAGE=$OUTPUT/legacy-release
    [ -f "$PACKAGE/index.json" ] || die "--skip-build requires $PACKAGE/index.json"
    note "reusing $PACKAGE"
else
    build_package
fi

[ -n "$PACKAGE" ] || die "no schema v1 package was produced"
export WXML_F3_LEGACY_PACKAGE=$PACKAGE
export WXML_TEST_TMPDIR=$OUTPUT/tmp
export CARGO_NET_OFFLINE=true
export CARGO_TARGET_DIR=$OUTPUT/target
if [ -z "${WXML_F3_ZED_WASM:-}" ]; then
    for candidate in \
        "$OUTPUT/zed-target/wasm32-wasip2/release/zed_wxml.wasm" \
        "/tmp/opencode/f3-legacy-real/zed-target/wasm32-wasip2/release/zed_wxml.wasm"; do
        if [ -f "$candidate" ]; then
            export WXML_F3_ZED_WASM=$candidate
            break
        fi
    done
fi
if [ -z "${WXML_F3_LSP_BIN:-}" ] && [ -x "$OUTPUT/target/release/wxml-lsp" ]; then
    export WXML_F3_LSP_BIN=$OUTPUT/target/release/wxml-lsp
fi
if [ -z "${WXML_F3_INSTALL_BIN:-}" ] && [ -x "$OUTPUT/target/release/wxml-lsp-install" ]; then
    export WXML_F3_INSTALL_BIN=$OUTPUT/target/release/wxml-lsp-install
fi
[ -n "$NODE_BIN" ] && export WXML_F3_REAL_NODE=$NODE_BIN

note "running the opt-in F3-09 real-legacy acceptance test"
set +e
cargo test -p wxml-delivery --test f3_legacy_real \
    -- --ignored --nocapture --test-threads=1 2>&1 | tee "$TEST_LOG"
TEST_STATUS=${PIPESTATUS[0]}
set -e
[ "$TEST_STATUS" -eq 0 ] || die "F3-09 real-legacy acceptance failed (log: $TEST_LOG)"

if ! grep -q 'F3-EVIDENCE: F3-09 PASS' "$TEST_LOG"; then
    die "F3-09 real-legacy test produced no PASS evidence; pass --required or provide the pinned native assets"
fi
grep -E '^F3-EVIDENCE: ' "$TEST_LOG" || true
note "passed (log: $TEST_LOG)"
