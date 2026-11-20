#!/usr/bin/env bash
# Build a reproducible, Node-free schema v2 WXML LSP release for Linux x86_64.
#
# The script never publishes, tags or uploads anything. It writes only below
# the user-selected --output directory and caches outside the repository. It
# never executes node/nodejs/npm/npx/pnpm/yarn/bun and never copies JavaScript
# from a legacy backend tree: the only inputs are the built Rust binaries, the
# wasm32-wasip2 Zed extension and an already verified asset cache (or the exact
# locked tarballs through `--from-dir`).
set -euo pipefail
umask 022

usage() {
    cat <<'USAGE'
Usage: scripts/build-release.sh --output DIR [OPTIONS]

Builds the Node-free schema v2 native package for linux-x86_64:

  1. cargo build --release -p wxml-lsp -p wxml-delivery
  2. cargo build --release --target wasm32-wasip2 (editors/zed)
  3. wxml-lsp-package provision --lock assets.lock.json --cache DIR
     (only the verified content-addressed cache or --from-dir tarballs)
  4. stage bin/wxml-lsp, bin/wxml-lsp-install, zed-extension/, and a generated
     THIRD_PARTY_LICENSES.md (engine/ and typings/ come from the cache; the
     Rust dependency inventory comes from `cargo metadata --locked --offline`)
  5. wxml-lsp-package native --input <stage> --output DIR
  6. wxml-lsp-install verify-package --from-dir DIR and sha256sum -c SHA256SUMS

Artifacts:

  <output>/wxml-lsp-<version>-linux-x86_64/
  <output>/wxml-lsp-<version>-linux-x86_64.tar.gz
  <output>/wxml-lsp-<version>-linux-x86_64.tar.gz.sha256
  <output>/index.json
  <output>/SHA256SUMS
  <output>/release-provenance.json  (channel-level; archive hash is known only
                                     after packaging, so it is not in the archive)

Options:
  --output DIR       artifact directory outside the repository (required)
  --version V        strict semver release version (default: the workspace
                     Cargo.toml version). When set, the build uses a clean
                     `git archive HEAD` source tree with Cargo.toml and
                     Cargo.lock retargeted to V before compiling; the
                     repository files are never modified.
  --cache DIR        content-addressed asset cache (default:
                     $WXML_BUILD_CACHE/assets-cache)
  --from-dir DIR     directory holding the exact locked tarball names; used
                     only when the cache is cold, never downloads anything
  --zed-wasm FILE    reuse a prebuilt zed_wxml.wasm instead of building it
  --skip-zed         fail instead of building the extension (requires --zed-wasm)
  --protocol-version N  native frame protocol version (default: source constant)
  --online           allow cargo to fetch missing Rust dependencies (default:
                     offline; assets are still never downloaded)
  --dry-run          print the plan and prerequisites; write nothing
  --help             print this message

Environment:
  CARGO_TARGET_DIR        Rust target directory (default: <cache>/cargo-target)
  ZED_CARGO_TARGET_DIR    Zed target directory (default: <cache>/zed-target)
  WXML_BUILD_CACHE        staging/cache root
                          (default: $XDG_CACHE_HOME/wxml-lsp-release)
  WXML_ASSET_CACHE        provisioned asset cache (default: <cache>/assets-cache)
  SOURCE_DATE_EPOCH       fixed build timestamp for reproducible archives

  ALLOW_UNLICENSED=1      local-only escape hatch: accept Rust crates with
                          neither a license expression nor a local license text.
                          Never use it for a publishable release.

Remote upload, tag creation and marketplace publishing are deliberately not
implemented. Any --publish/--tag/--release/--upload option fails before
building. The Zed grammar is bundled only when editors/zed/grammars/wxml.wasm
already exists; this script never downloads it.
USAGE
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

note() {
    printf '%s\n' "$*"
}

require_tool() {
    command -v "$1" >/dev/null 2>&1 || die "required tool \`$1\` was not found"
}

is_strict_semver() {
    [[ "$1" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$ ]]
}

shell_quote() {
    printf '%q' "$1"
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
output=""
assets_cache=""
from_dir=""
zed_wasm=""
skip_zed=0
dry_run=0
online=0
protocol_version=""
requested_version=""
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
        --version)
            [ "$#" -ge 2 ] || die "--version requires a value"
            requested_version="$2"
            shift 2
            ;;
        --version=*)
            requested_version="${1#*=}"
            shift
            ;;
        --cache|--assets-cache)
            [ "$#" -ge 2 ] || die "$1 requires a directory"
            assets_cache="$2"
            shift 2
            ;;
        --cache=*|--assets-cache=*)
            assets_cache="${1#*=}"
            shift
            ;;
        --from-dir)
            [ "$#" -ge 2 ] || die "--from-dir requires a directory"
            from_dir="$2"
            shift 2
            ;;
        --from-dir=*)
            from_dir="${1#*=}"
            shift
            ;;
        --zed-wasm)
            [ "$#" -ge 2 ] || die "--zed-wasm requires a file"
            zed_wasm="$2"
            shift 2
            ;;
        --zed-wasm=*)
            zed_wasm="${1#*=}"
            shift
            ;;
        --skip-zed)
            skip_zed=1
            shift
            ;;
        --protocol-version)
            [ "$#" -ge 2 ] || die "--protocol-version requires a number"
            protocol_version="$2"
            shift 2
            ;;
        --protocol-version=*)
            protocol_version="${1#*=}"
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

workspace_version="$(awk -F'"' '/^version = "/{print $2; exit}' "$repo_root/Cargo.toml")"
[ -n "$workspace_version" ] || die "could not read the workspace version"
[[ "$workspace_version" =~ ^[0-9A-Za-z][0-9A-Za-z.+_-]*$ ]] || die "unsafe workspace version"
version="$workspace_version"
if [ -n "$requested_version" ]; then
    is_strict_semver "$requested_version" \
        || die "--version must be a strict semantic version, got \`$requested_version\`"
    version="$requested_version"
fi

cache_root="${WXML_BUILD_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/wxml-lsp-release}"
case "$(realpath -m -- "$cache_root")/" in
    "$repo_real/"*) die "build and cache directories must be outside the repository: $cache_root" ;;
esac

# A version override is a real rebuild from a clean committed tree: never a
# binary patch and never an edit of the repository's Cargo files. The staged
# tree rewrites only the workspace package version, which Cargo compiles into
# `wxml-lsp --version` and the v2 manifest/index.
source_root="$repo_root"
source_stage=""
if [ -n "$requested_version" ] && [ "$dry_run" -eq 0 ]; then
    require_tool git
    require_tool tar
    mkdir -p "$cache_root"
    source_stage="$(mktemp -d "$cache_root/source-$version-XXXXXXXX")"
    trap 'rm -rf -- "${source_stage:-}"' EXIT
    git -C "$repo_root" archive --format=tar HEAD | tar -x -C "$source_stage"
    if [ "$version" != "$workspace_version" ]; then
        sed -i "0,/^version = \"$workspace_version\"\$/s//version = \"$version\"/" \
            "$source_stage/Cargo.toml"
        grep -qx "version = \"$version\"" "$source_stage/Cargo.toml" \
            || die "could not retarget the staged workspace version"
        workspace_members="$(grep -c '^name = "wxml-' "$source_stage/Cargo.lock")"
        awk -v old="$workspace_version" -v new="$version" -v expected="$workspace_members" '
            /^\[\[package\]\]$/ { in_workspace = 0 }
            /^name = "wxml-/ { in_workspace = 1 }
            in_workspace && $0 == "version = \"" old "\"" {
                print "version = \"" new "\""
                in_workspace = 0
                replaced++
                next
            }
            { print }
            END { if (replaced != expected) exit 1 }
        ' "$source_stage/Cargo.lock" > "$source_stage/Cargo.lock.release"
        mv "$source_stage/Cargo.lock.release" "$source_stage/Cargo.lock"
    fi
    source_root="$source_stage"
    note "source: clean git archive HEAD at $source_root (release version $version)"
fi

manifest_v2="$source_root/crates/wxml-delivery/src/manifest_v2.rs"
engine_rs="$source_root/crates/wxml-lsp/src/backend_tsgo/engine.rs"
assets_lock="$source_root/assets.lock.json"
manifest_protocol="$(sed -n 's/^pub const SUPPORTED_NATIVE_PROTOCOL_VERSIONS: &\[u64\] = &\[\([0-9][0-9]*\)\];$/\1/p' \
    "$manifest_v2" | head -n1)"
engine_protocol="$(sed -n 's/^const NATIVE_PROTOCOL_VERSION: u64 = \([0-9][0-9]*\);$/\1/p' \
    "$engine_rs" | head -n1)"
[ -n "$manifest_protocol" ] || die "could not read the native manifest protocol version"
[ -n "$engine_protocol" ] || die "could not read the native engine protocol version"
[ "$manifest_protocol" = "$engine_protocol" ] \
    || die "native manifest and engine protocol versions differ"
if [ -z "$protocol_version" ]; then
    protocol_version="$manifest_protocol"
fi
[[ "$protocol_version" =~ ^[0-9]+$ ]] || die "--protocol-version must be a number"
[ "$protocol_version" = "$manifest_protocol" ] \
    || die "native protocol $protocol_version is not supported by this packager"

case "$(uname -s)" in
    Linux) ;;
    *) die "only Linux native release builds are verified by this project" ;;
esac
case "$(uname -m)" in
    x86_64|amd64) ;;
    *) die "native packages support only linux-x86_64 (host: $(uname -m))" ;;
esac
platform="linux-x86_64"

rust_target="${CARGO_TARGET_DIR:-$cache_root/cargo-target}"
zed_target="${ZED_CARGO_TARGET_DIR:-$cache_root/zed-target}"
if [ -z "$assets_cache" ]; then
    assets_cache="${WXML_ASSET_CACHE:-$cache_root/assets-cache}"
fi
for directory in "$rust_target" "$zed_target" "$cache_root" "$assets_cache"; do
    case "$(realpath -m -- "$directory")/" in
        "$repo_real/"*) die "build and cache directories must be outside the repository: $directory" ;;
    esac
done
if [ -n "$from_dir" ]; then
    from_dir="$(realpath -m -- "$from_dir")"
    [ -d "$from_dir" ] || die "--from-dir is not a directory: $from_dir"
fi
if [ -n "$zed_wasm" ]; then
    zed_wasm="$(realpath -m -- "$zed_wasm")"
    [ -f "$zed_wasm" ] || die "--zed-wasm is not a file: $zed_wasm"
fi
[ "$skip_zed" -eq 0 ] || [ -n "$zed_wasm" ] \
    || die "--skip-zed requires --zed-wasm FILE"

if [ -n "${SOURCE_DATE_EPOCH:-}" ]; then
    [[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] || die "SOURCE_DATE_EPOCH must be an integer"
    built_at="$(date -u -d "@$SOURCE_DATE_EPOCH" +%Y-%m-%dT%H:%M:%SZ)"
    build_epoch="$SOURCE_DATE_EPOCH"
else
    built_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    build_epoch="$(date -u -d "$built_at" +%s)"
fi
case "${ALLOW_UNLICENSED:-0}" in
    0|1) ;;
    *) die "ALLOW_UNLICENSED must be 0 or 1" ;;
esac

check_prerequisites() {
    require_tool cargo
    require_tool git
    require_tool realpath
    require_tool sha256sum
    require_tool awk
    require_tool sed
    require_tool find
    require_tool sort
    require_tool install
    require_tool stat
    require_tool tail
    [ -f "$assets_lock" ] || die "missing assets.lock.json"
    [ -f "$manifest_v2" ] || die "missing native manifest source"
    [ -f "$engine_rs" ] || die "missing native engine source"
    [ -f "$source_root/editors/zed/extension.toml" ] || die "missing Zed manifest"
    git -C "$repo_root" rev-parse --verify HEAD >/dev/null 2>&1 \
        || die "the release source is not a git worktree with a HEAD commit"
    if [ -z "$zed_wasm" ] && [ "$skip_zed" -eq 0 ] && command -v rustup >/dev/null 2>&1; then
        rustup target list --installed | grep -qx 'wasm32-wasip2' \
            || die "rustup target wasm32-wasip2 is not installed (or pass --zed-wasm)"
    fi
    if ! command -v node >/dev/null 2>&1; then
        note "node: absent (native release build does not require it)"
    else
        note "node: present on PATH; the build PATH tripwire fails closed if it is executed"
    fi
}

if [ "$dry_run" -eq 1 ]; then
    check_prerequisites
    note "dry-run: no files will be written, no upload/tag/release"
    note "  repository: $repo_root"
    note "  output:     $output_abs"
    note "  version:    $version"
    note "  platform:   $platform"
    note "  protocol:   $protocol_version"
    note "  target dir: $rust_target"
    note "  zed target: $zed_target"
    note "  asset cache:$assets_cache"
    note "  from-dir:   ${from_dir:-<none>}"
    note "  cargo:      $([ "$online" -eq 1 ] && echo online || echo offline)"
    note "  built-at:   $built_at"
    note "planned steps:"
    note "  1. cargo build --release --locked -p wxml-lsp -p wxml-delivery"
    if [ -n "$zed_wasm" ]; then
        note "  2. reuse Zed wasm: $zed_wasm"
    else
        note "  2. cargo build --manifest-path editors/zed/Cargo.toml --release --target wasm32-wasip2 --locked"
    fi
    note "  3. wxml-lsp-package provision --lock assets.lock.json --cache $assets_cache [--from-dir DIR]"
    note "  4. cargo metadata + wxml-lsp-package licenses → THIRD_PARTY_LICENSES.md"
    note "  5. wxml-lsp-package native --input <stage> --output $output_abs"
    note "  6. wxml-lsp-install verify-package --from-dir $output_abs --version $version --offline"
    note "  7. sha256sum -c SHA256SUMS and release-provenance.json"
    exit 0
fi

check_prerequisites
mkdir -p "$output_abs" "$cache_root"
stage_root="$(mktemp -d "$cache_root/stage-XXXXXXXX")"
stage="$stage_root/root"
audit_dir="$stage_root/no-node-path"
marker="$stage_root/no-node-executed.log"
cleanup() {
    local status=$?
    trap - EXIT
    if [ -e "$marker" ]; then
        printf 'error: the release build executed a Node-family tool:\n' >&2
        sed 's/^/  /' "$marker" >&2
        [ "$status" -eq 0 ] && status=1
    fi
    rm -rf "$stage_root"
    [ -z "$source_stage" ] || rm -rf "$source_stage"
    exit "$status"
}
trap cleanup EXIT
mkdir -p "$stage/bin" "$stage/zed-extension" "$audit_dir"

# The build itself must never execute a Node-family tool. The tripwire only
# covers PATH-name invocations; it is auxiliary evidence, not an execve audit.
for name in node nodejs npm npx pnpm yarn bun; do
    {
        printf '#!/bin/sh\n'
        printf 'printf "%%s %%s\\n" "$0" "$*" >> %s\n' "$(printf '%q' "$marker")"
        printf 'exit 99\n'
    } > "$audit_dir/$name"
    chmod 755 "$audit_dir/$name"
done
export PATH="$audit_dir:$PATH"
check_no_node() {
    if [ -e "$marker" ]; then
        printf 'error: the release build executed a Node-family tool:\n' >&2
        sed 's/^/  /' "$marker" >&2
        exit 1
    fi
}

offline_flag=(--offline)
if [ "$online" -eq 1 ]; then
    offline_flag=()
    unset CARGO_NET_OFFLINE
else
    export CARGO_NET_OFFLINE=true
fi

note "building Rust release binaries (version $version, $platform)"
CARGO_TARGET_DIR="$rust_target" cargo build --release --locked "${offline_flag[@]}" \
    --manifest-path "$source_root/Cargo.toml" \
    -p wxml-lsp -p wxml-delivery
install -m 0755 "$rust_target/release/wxml-lsp" "$stage/bin/wxml-lsp"
install -m 0755 "$rust_target/release/wxml-lsp-install" "$stage/bin/wxml-lsp-install"
check_no_node

note "building Zed extension (wasm32-wasip2)"
if [ -n "$zed_wasm" ]; then
    install -m 0644 "$zed_wasm" "$stage/zed-extension/extension.wasm"
else
    CARGO_TARGET_DIR="$zed_target" cargo build --manifest-path "$source_root/editors/zed/Cargo.toml" \
        --release --target wasm32-wasip2 --locked "${offline_flag[@]}"
    wasm="$zed_target/wasm32-wasip2/release/zed_wxml.wasm"
    [ -f "$wasm" ] || die "Zed extension build produced no zed_wxml.wasm"
    install -m 0644 "$wasm" "$stage/zed-extension/extension.wasm"
fi
install -m 0644 "$source_root/editors/zed/extension.toml" "$stage/zed-extension/extension.toml"
cp -a "$source_root/editors/zed/languages" "$stage/zed-extension/languages"
find "$stage/zed-extension/languages" -type f -exec chmod 0644 {} +
find "$stage/zed-extension/languages" -type d -exec chmod 0755 {} +
if [ -f "$source_root/editors/zed/grammars/wxml.wasm" ]; then
    mkdir -p "$stage/zed-extension/grammars"
    install -m 0644 "$source_root/editors/zed/grammars/wxml.wasm" \
        "$stage/zed-extension/grammars/wxml.wasm"
else
    note "warning: editors/zed/grammars/wxml.wasm is absent; the package omits the grammar"
fi
check_no_node

note "provisioning locked assets from the verified cache or --from-dir"
packager="$rust_target/release/wxml-lsp-package"
provision_log="$stage_root/provision.log"
provision_args=(provision --lock "$assets_lock" --cache "$assets_cache" --offline)
if [ -n "$from_dir" ]; then
    provision_args+=(--from-dir "$from_dir")
fi
"$packager" "${provision_args[@]}" | tee "$provision_log"
check_no_node

note "generating Rust third-party license inventory from cargo metadata"
source_commit="$(git -C "$repo_root" rev-parse --verify HEAD)" \
    || die "could not read the source commit"
[[ "$source_commit" =~ ^[0-9a-f]{7,64}$ ]] || die "unexpected git commit id: $source_commit"
if [ -n "$(git -C "$repo_root" status --porcelain --untracked-files=no)" ]; then
    note "warning: the source worktree has uncommitted tracked changes; provenance records HEAD $source_commit"
fi
metadata_root="$stage_root/cargo-metadata-root.json"
metadata_zed="$stage_root/cargo-metadata-zed.json"
cargo metadata --format-version 1 --locked --offline \
    --manifest-path "$source_root/Cargo.toml" \
    --filter-platform x86_64-unknown-linux-gnu > "$metadata_root"
cargo metadata --format-version 1 --locked --offline \
    --manifest-path "$source_root/editors/zed/Cargo.toml" \
    --filter-platform wasm32-wasip2 > "$metadata_zed"
check_no_node
rust_licenses="$stage_root/rust-licenses.md"
license_args=(licenses
    --output "$rust_licenses"
    --metadata "$metadata_root"
    --label "root workspace"
    --target x86_64-unknown-linux-gnu
    --package wxml-lsp
    --package wxml-delivery
    --metadata "$metadata_zed"
    --label "editors/zed workspace"
    --target wasm32-wasip2
    --package zed-wxml
    --source-commit "$source_commit"
    --source-date-epoch "$build_epoch")
if [ "${ALLOW_UNLICENSED:-0}" -eq 1 ]; then
    note "warning: ALLOW_UNLICENSED=1 accepts crates without a license expression and without local license text; this inventory is local-only and must not be published"
    license_args+=(--allow-unlicensed)
fi
"$packager" "${license_args[@]}" | tee "$stage_root/licenses.log"
check_no_node
[ -s "$rust_licenses" ] || die "the license generator produced no inventory"
grep -qx '<!-- generated-by: wxml-lsp-license-generator/2 -->' "$rust_licenses" \
    || die "the license inventory is missing its generator marker"
grep -q '^<!-- text-coverage: ' "$rust_licenses" \
    || die "the license inventory is missing its text-coverage marker"
if [ "${ALLOW_UNLICENSED:-0}" -eq 0 ]; then
    grep -qx '<!-- text-coverage: .* missing=0 -->' "$rust_licenses" \
        || die "the license inventory has third-party crates without license text"
fi
grep -qx '## Rust dependency inventory' "$rust_licenses" \
    || die "the license inventory is missing the Rust dependency section"
grep -qx '## First-party packages' "$rust_licenses" \
    || die "the license inventory is missing the first-party section"

note "appending verified upstream asset licenses"
licenses="$stage/THIRD_PARTY_LICENSES.md"
asset_keys="$stage_root/asset-keys.txt"
awk '$1 == "asset:" {
    name = $2
    key = ""
    for (i = 1; i <= NF; i++) {
        if ($i == "cache-key") {
            key = $(i + 1)
        }
    }
    if (name != "" && key != "") {
        print name " " key
    }
}' "$provision_log" | LC_ALL=C sort > "$asset_keys"
[ -s "$asset_keys" ] || die "provision report did not list any verified assets"
{
    printf '# Third-party licenses\n\n'
    printf 'Generated by `scripts/build-release.sh`. This file has two parts: the\n'
    printf 'verified upstream assets from the committed `assets.lock.json` cache, and\n'
    printf 'the Rust dependency inventory from `cargo metadata` plus license texts in\n'
    printf 'the local Cargo cache. It is a build-time aggregation, not a legal review.\n\n'
    printf '## Upstream assets\n\n'
    while read -r name key; do
        payload="$assets_cache/assets/$name/$key/payload"
        [ -d "$payload" ] || die "missing verified payload entry: $payload"
        license_files="$stage_root/licenses-$name.txt"
        find "$payload" -type f \
            \( -iname 'LICENSE*' -o -iname 'NOTICE*' -o -iname 'COPYING*' \) \
            | LC_ALL=C sort > "$license_files"
        [ -s "$license_files" ] \
            || die "verified payload for \`$name\` contains no license file"
        printf '### %s\n\n' "$name"
        while IFS= read -r file; do
            relative="${file#"$payload"/}"
            printf '#### `%s`\n\n```text\n' "$relative"
            cat "$file"
            printf '\n```\n\n'
        done < "$license_files"
    done < "$asset_keys"
    printf '## Rust dependencies\n\n'
    tail -n +2 "$rust_licenses"
} > "$licenses"
chmod 0644 "$licenses"

[ ! -e "$stage/engine" ] || die "stage must not supply engine/; it comes from the cache"
[ ! -e "$stage/typings" ] || die "stage must not supply typings/; they come from the cache"
if find "$stage" -type f \( -name '*.js' -o -name '*.mjs' -o -name '*.cjs' \
    -o -name 'server.js' \) | grep -q .; then
    die "stage contains JavaScript; the native package must stay Node-free"
fi
if find "$stage" -type d -name node_modules | grep -q .; then
    die "stage contains node_modules; the native package must stay Node-free"
fi
check_no_node

note "packaging versioned directory, archive, checksums, index and provenance"
"$packager" native \
    --input "$stage" \
    --output "$output_abs" \
    --assets-lock "$assets_lock" \
    --cache "$assets_cache" \
    --release-version "$version" \
    --platform "$platform" \
    --protocol-version "$protocol_version" \
    --built-at "$built_at" \
    --source-commit "$source_commit" \
    --source-date-epoch "$build_epoch" \
    --force
check_no_node

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

release_dir="$output_abs/wxml-lsp-$version-$platform"
archive="$release_dir.tar.gz"
sidecar="$archive.sha256"
[ -d "$release_dir" ] || die "missing generated release directory"
[ -f "$archive" ] || die "missing generated archive"
[ -f "$sidecar" ] || die "missing archive sidecar checksum"
for required in manifest.json SHA256SUMS provenance.json THIRD_PARTY_LICENSES.md \
    bin/wxml-lsp bin/wxml-lsp-install zed-extension/extension.wasm \
    zed-extension/extension.toml; do
    [ -f "$release_dir/$required" ] || die "release is missing \`$required\`"
done
[ -f "$release_dir/engine/lib/tsc" ] || die "release is missing the native engine"
[ -x "$release_dir/engine/lib/tsc" ] || die "native engine is not executable"
grep -qx '## Rust dependency inventory' "$release_dir/THIRD_PARTY_LICENSES.md" \
    || die "release license inventory lost the Rust dependency section"
for field in sourceCommit sourceDateEpoch archivePayloadDigest; do
    grep -q "\"$field\"" "$release_dir/provenance.json" \
        || die "provenance.json is missing $field"
done
grep -q "\"sourceCommit\": \"$source_commit\"" "$release_dir/provenance.json" \
    || die "provenance.json sourceCommit does not match HEAD"
grep -q "\"sourceDateEpoch\": $build_epoch" "$release_dir/provenance.json" \
    || die "provenance.json sourceDateEpoch does not match the build epoch"

note "writing channel-level release-provenance.json (archive identity is known after packaging)"
release_provenance="$output_abs/release-provenance.json"
manifest_sha256="$(sha256sum "$release_dir/manifest.json" | awk '{print $1}')"
provenance_sha256="$(sha256sum "$release_dir/provenance.json" | awk '{print $1}')"
archive_sha256="$(awk '{print $1}' "$sidecar")"
archive_size="$(stat -c '%s' "$archive")"
archive_integrity="$(sed -n 's/^ *"archiveIntegrity": "\([^"]*\)",*$/\1/p' \
    "$output_abs/index.json" | head -n1)"
payload_digest="$(sed -n 's/^ *"archivePayloadDigest": "\([0-9a-f]*\)",*$/\1/p' \
    "$release_dir/provenance.json" | head -n1)"
[[ "$archive_sha256" =~ ^[0-9a-f]{64}$ ]] || die "invalid archive SHA-256"
[[ "$payload_digest" =~ ^[0-9a-f]{64}$ ]] || die "invalid archivePayloadDigest"
[[ "$archive_integrity" == sha512-* ]] || die "index.json is missing archiveIntegrity"
release_provenance_stage="$stage_root/release-provenance.json"
{
    printf '{\n'
    printf '  "schemaVersion": 1,\n'
    printf '  "scope": "release-channel",\n'
    printf '  "generator": "scripts/build-release.sh",\n'
    printf '  "generatorVersion": "wxml-lsp-release-provenance/1",\n'
    printf '  "package": "wxml-lsp",\n'
    printf '  "packageVersion": "%s",\n' "$version"
    printf '  "platform": "%s",\n' "$platform"
    printf '  "sourceCommit": "%s",\n' "$source_commit"
    printf '  "sourceDateEpoch": %s,\n' "$build_epoch"
    printf '  "builtAt": "%s",\n' "$built_at"
    printf '  "archive": {\n'
    printf '    "name": "wxml-lsp-%s-%s.tar.gz",\n' "$version" "$platform"
    printf '    "sha256": "%s",\n' "$archive_sha256"
    printf '    "size": %s,\n' "$archive_size"
    printf '    "integrity": "%s"\n' "$archive_integrity"
    printf '  },\n'
    printf '  "payload": {\n'
    printf '    "archivePayloadDigest": "%s",\n' "$payload_digest"
    printf '    "manifestSha256": "%s",\n' "$manifest_sha256"
    printf '    "provenanceSha256": "%s"\n' "$provenance_sha256"
    printf '  }\n'
    printf '}\n'
} > "$release_provenance_stage"
install -m 0644 "$release_provenance_stage" "$release_provenance"
check_no_node

note ""
note "native release build complete"
note "  directory: $release_dir"
note "  archive:   $archive"
note "  sha256:    $(awk '{print $1}' "$sidecar")"
note "  manifest:  $release_dir/manifest.json"
note "  provenance:$release_dir/provenance.json"
note "  channel:   $release_provenance"
note "  licenses:  $release_dir/THIRD_PARTY_LICENSES.md"
note "  index:     $output_abs/index.json"
note "  installer: $release_dir/bin/wxml-lsp-install"
note "  node:      not executed (PATH tripwire clean)"
note ""
note "to use the staged release without installing:"
note "  export WXML_LSP_BIN=$(shell_quote "$release_dir/bin/wxml-lsp")"
note "to install it:"
note "  $(shell_quote "$release_dir/bin/wxml-lsp-install") install --from-dir $(shell_quote "$output_abs") --version $(shell_quote "$version") --offline"
note "no upload, tag or remote release was created."
