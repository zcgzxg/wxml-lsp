#!/usr/bin/env bash
# Build a reproducible local WXML LSP release for Linux.
#
# The script never publishes, tags or uploads anything. It writes only below
# the user-selected --output directory and a cache outside the repository.
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: scripts/build-release.sh --output DIR [OPTIONS]

Builds the Rust language server + installer, the Node type backend with
production dependencies, and the wasm32-wasip2 Zed extension, then produces:

  <output>/wxml-lsp-<version>-<platform>/
  <output>/wxml-lsp-<version>-<platform>.tar.gz
  <output>/wxml-lsp-<version>-<platform>.tar.gz.sha256
  <output>/index.json
  <output>/SHA256SUMS

Options:
  --output DIR       artifact directory outside the repository (required)
  --dry-run          print the plan and prerequisites; write nothing
  --online           allow cargo/npm to fetch missing dependencies (default:
                     offline; the release script never contacts a server itself)
  --help             print this message

Environment:
  CARGO_TARGET_DIR   Rust target directory (default: <cache>/cargo-target)
  WXML_BUILD_CACHE   staging/cache root (default: $XDG_CACHE_HOME/wxml-lsp-release)
  SOURCE_DATE_EPOCH  fixed build timestamp for reproducible archives

Remote upload, tag creation and marketplace publishing are deliberately not
implemented. Any --publish/--tag/--release option fails before building.
The Zed grammar is bundled only when editors/zed/grammars/wxml.wasm already
exists; this script never downloads it.
USAGE
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

note() {
    printf '%s\n' "$*"
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
output=""
dry_run=0
online=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output)
            [ "$#" -ge 2 ] || die "--output requires a directory"
            output="$2"
            shift 2
            ;;
        --output=*)
            output="${1#*=}"
            shift
            ;;
        --dry-run)
            dry_run=1
            shift
            ;;
        --online)
            online=1
            shift
            ;;
        --publish|--tag|--release|--upload)
            die "remote publish/tag/release is disabled; this script only builds local artifacts"
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            die "unknown argument \`$1\`"
            ;;
    esac
done

[ -n "$output" ] || die "--output is required"
[ "$output" != "/" ] || die "refusing to use / as the output directory"

repo_real="$repo_root"
output_abs="$(realpath -m -- "$output")"
case "$output_abs/" in
    "$repo_real/"*) die "--output must be outside the repository" ;;
esac

version="$(awk -F'"' '/^version = "/{print $2; exit}' "$repo_root/Cargo.toml")"
[ -n "$version" ] || die "could not read the workspace version"
[[ "$version" =~ ^[0-9A-Za-z][0-9A-Za-z.+_-]*$ ]] || die "unsafe workspace version"

rust_protocol="$(sed -n 's/^pub const PROTOCOL_VERSION: u64 = \([0-9][0-9]*\);$/\1/p' \
    "$repo_root/crates/wxml-lsp/src/backend_node/protocol.rs" | head -n1)"
node_protocol="$(sed -n 's/^export const PROTOCOL_VERSION = \([0-9][0-9]*\);$/\1/p' \
    "$repo_root/backends/typescript/src/protocol.ts" | head -n1)"
[ -n "$rust_protocol" ] || die "could not read the Rust protocol version"
[ -n "$node_protocol" ] || die "could not read the Node protocol version"
[ "$rust_protocol" = "$node_protocol" ] || die "Rust and Node protocol versions differ"

package_json="$repo_root/backends/typescript/package.json"
node_minimum="$(sed -n 's/.*"node": *">=\([0-9][0-9.]*\)".*/\1/p' "$package_json" | head -n1)"
[ -n "$node_minimum" ] || die "could not read the Node minimum version"
backend_version="$(sed -n 's/^  "version": "\([^"]*\)",$/\1/p' "$package_json" | head -n1)"
[ -n "$backend_version" ] || die "could not read the backend version"
zed_version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$repo_root/editors/zed/extension.toml" | head -n1)"
[ -n "$zed_version" ] || die "could not read the Zed extension version"

case "$(uname -s)" in
    Linux) os=linux ;;
    *) die "only Linux release builds are verified by this project" ;;
esac
case "$(uname -m)" in
    x86_64|amd64) arch=x86_64 ;;
    aarch64|arm64) arch=aarch64 ;;
    *) die "unsupported architecture: $(uname -m)" ;;
esac
platform="$os-$arch"

cache_root="${WXML_BUILD_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/wxml-lsp-release}"
rust_target="${CARGO_TARGET_DIR:-$cache_root/cargo-target}"
zed_target="${ZED_CARGO_TARGET_DIR:-$cache_root/zed-target}"
case "$(realpath -m -- "$rust_target")/" in
    "$repo_real/"*) die "CARGO_TARGET_DIR must be outside the repository" ;;
esac
case "$(realpath -m -- "$zed_target")/" in
    "$repo_real/"*) die "ZED_CARGO_TARGET_DIR must be outside the repository" ;;
esac
case "$(realpath -m -- "$cache_root")/" in
    "$repo_real/"*) die "WXML_BUILD_CACHE must be outside the repository" ;;
esac

if [ -n "${SOURCE_DATE_EPOCH:-}" ]; then
    [[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] || die "SOURCE_DATE_EPOCH must be an integer"
    built_at="$(date -u -d "@$SOURCE_DATE_EPOCH" +%Y-%m-%dT%H:%M:%SZ)"
    mtime="$SOURCE_DATE_EPOCH"
else
    built_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    mtime=0
fi

require_tool() {
    command -v "$1" >/dev/null 2>&1 || die "required tool \`$1\` was not found"
}

check_prerequisites() {
    require_tool cargo
    require_tool node
    require_tool npm
    require_tool realpath
    require_tool sha256sum
    [ -f "$package_json" ] || die "missing backend package.json"
    [ -f "$repo_root/crates/wxml-lsp/src/backend_node/protocol.rs" ] || die "missing Rust protocol source"
    [ -f "$repo_root/backends/typescript/src/protocol.ts" ] || die "missing Node protocol source"
    [ -f "$repo_root/editors/zed/extension.toml" ] || die "missing Zed manifest"
    if command -v rustup >/dev/null 2>&1; then
        rustup target list --installed | grep -qx 'wasm32-wasip2' \
            || die "rustup target wasm32-wasip2 is not installed"
    fi
    node -e 'process.exit(Number(process.versions.node.split(".")[0]) >= 24 ? 0 : 1)' \
        || die "Node.js >= $node_minimum is required to build the backend"
}

if [ "$dry_run" -eq 1 ]; then
    check_prerequisites
    note "dry-run: no files will be written, no network access, no upload/tag/release"
    note "  repository: $repo_root"
    note "  output:     $output_abs"
    note "  version:    $version"
    note "  platform:   $platform"
    note "  protocol:   $rust_protocol"
    note "  node:       >= $node_minimum"
    note "  target dir: $rust_target"
    note "  zed target: $zed_target"
    note "  npm:        $([ "$online" -eq 1 ] && echo online || echo offline)"
    note "  built-at:   $built_at"
    note "planned steps:"
    note "  1. cargo build --release --locked -p wxml-lsp -p wxml-delivery"
    note "  2. copy backend sources; npm ci, npm run check, npm run build, npm prune --omit=dev"
    note "  3. cargo build --manifest-path editors/zed/Cargo.toml --release --target wasm32-wasip2 --locked"
    note "  4. stage bin/wxml-lsp, bin/wxml-lsp-install, bin/wxml-type-backend/, zed-extension/"
    note "  5. wxml-lsp-package --input <stage> --output $output_abs"
    note "  6. wxml-lsp-install verify-package --from-dir $output_abs --version $version"
    note "  7. sha256sum -c SHA256SUMS"
    exit 0
fi

check_prerequisites
mkdir -p "$output_abs"
mkdir -p "$cache_root"
stage_root="$(mktemp -d "$cache_root/stage-XXXXXXXX")"
trap 'rm -rf "$stage_root"' EXIT
stage="$stage_root/root"
mkdir -p "$stage/bin" "$stage/zed-extension"

offline_flag=()
if [ "$online" -eq 0 ]; then
    offline_flag=(--offline)
    export CARGO_NET_OFFLINE=true
else
    unset CARGO_NET_OFFLINE
fi
npm_offline=(--offline)
if [ "$online" -eq 1 ]; then
    npm_offline=()
fi

note "building Rust release binaries (version $version, $platform)"
CARGO_TARGET_DIR="$rust_target" cargo build --release --locked "${offline_flag[@]}" \
    -p wxml-lsp -p wxml-delivery
install -m 0755 "$rust_target/release/wxml-lsp" "$stage/bin/wxml-lsp"
install -m 0755 "$rust_target/release/wxml-lsp-install" "$stage/bin/wxml-lsp-install"

note "building Node type backend"
node_work="$stage_root/node-work"
mkdir -p "$node_work"
cp -a "$repo_root/backends/typescript/src" "$node_work/src"
cp "$package_json" "$node_work/package.json"
cp "$repo_root/backends/typescript/package-lock.json" "$node_work/package-lock.json"
cp "$repo_root/backends/typescript/tsconfig.json" "$node_work/tsconfig.json"
cp "$repo_root/backends/typescript/tsconfig.build.json" "$node_work/tsconfig.build.json"
(
    cd "$node_work"
    npm ci --no-audit --no-fund "${npm_offline[@]}"
    npm run check
    npm run build
    npm prune --omit=dev --no-audit --no-fund "${npm_offline[@]}"
)
find "$node_work/node_modules" -type l -delete
mkdir -p "$stage/bin/wxml-type-backend"
cp -a "$node_work/dist/." "$stage/bin/wxml-type-backend/"
cp "$node_work/package.json" "$stage/bin/wxml-type-backend/package.json"
cp -a "$node_work/node_modules" "$stage/bin/wxml-type-backend/node_modules"
[ -f "$stage/bin/wxml-type-backend/server.js" ] || die "Node backend build produced no server.js"

note "building Zed extension (wasm32-wasip2)"
CARGO_TARGET_DIR="$zed_target" cargo build --manifest-path "$repo_root/editors/zed/Cargo.toml" \
    --release --target wasm32-wasip2 --locked "${offline_flag[@]}"
wasm="$zed_target/wasm32-wasip2/release/zed_wxml.wasm"
[ -f "$wasm" ] || die "Zed extension build produced no zed_wxml.wasm"
install -m 0644 "$wasm" "$stage/zed-extension/extension.wasm"
cp "$repo_root/editors/zed/extension.toml" "$stage/zed-extension/extension.toml"
cp -a "$repo_root/editors/zed/languages" "$stage/zed-extension/languages"
if [ -f "$repo_root/editors/zed/initialization-options.schema.json" ]; then
    cp "$repo_root/editors/zed/initialization-options.schema.json" \
        "$stage/zed-extension/initialization-options.schema.json"
fi
if [ -f "$repo_root/editors/zed/grammars/wxml.wasm" ]; then
    mkdir -p "$stage/zed-extension/grammars"
    cp "$repo_root/editors/zed/grammars/wxml.wasm" "$stage/zed-extension/grammars/wxml.wasm"
else
    note "warning: editors/zed/grammars/wxml.wasm is absent; the package omits the grammar"
fi

note "packaging versioned directory, archive, checksums and index"
"$rust_target/release/wxml-lsp-package" \
    --input "$stage" \
    --output "$output_abs" \
    --release-version "$version" \
    --platform "$platform" \
    --protocol-version "$rust_protocol" \
    --node-minimum "$node_minimum" \
    --backend-version "$backend_version" \
    --zed-version "$zed_version" \
    --built-at "$built_at" \
    --mtime "$mtime" \
    --force

note "verifying generated package with the installer"
"$rust_target/release/wxml-lsp-install" verify-package \
    --from-dir "$output_abs" \
    --version "$version" \
    --offline

note "verifying top-level SHA256SUMS"
(
    cd "$output_abs"
    sha256sum -c SHA256SUMS
)

archive="$output_abs/wxml-lsp-$version-$platform.tar.gz"
sidecar="$archive.sha256"
[ -f "$archive" ] || die "missing generated archive"
[ -f "$sidecar" ] || die "missing archive sidecar checksum"

note ""
note "release build complete"
note "  directory: $output_abs/wxml-lsp-$version-$platform"
note "  archive:   $archive"
note "  sha256:    $(awk '{print $1}' "$sidecar")"
note "  manifest:  $output_abs/wxml-lsp-$version-$platform/manifest.json"
note "  index:     $output_abs/index.json"
note "  installer: $output_abs/wxml-lsp-$version-$platform/bin/wxml-lsp-install"
note ""
note "to use the staged release without installing:"
note "  export WXML_LSP_BIN='$output_abs/wxml-lsp-$version-$platform/bin/wxml-lsp'"
note "to install it:"
note "  '$output_abs/wxml-lsp-$version-$platform/bin/wxml-lsp-install' install --from-dir '$output_abs' --version '$version'"
note "no upload, tag or remote release was created."
