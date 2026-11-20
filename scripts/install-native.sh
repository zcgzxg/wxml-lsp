#!/usr/bin/env bash
# Install the Node-free native wxml-lsp payload from assets.lock.json.
# Each tarball is pinned by URL, size and SHA-256; nothing is extracted until
# both checks pass. packageRoot is installed verbatim below --root and the
# resulting layout is verified. The engine is never discovered through PATH.
set -euo pipefail
umask 022

usage() { cat <<'USAGE'
Usage: scripts/install-native.sh [OPTIONS]

Downloads the tarballs pinned by assets.lock.json, verifies size and SHA-256,
and installs <root>/bin/wxml-lsp, <root>/engine/lib/tsc (TypeScript engine) and
<root>/typings/miniprogram-api-typings.

Options:
  --root DIR      install root (default: ${WXML_LSP_HOME:-$HOME/.local/share/wxml-lsp})
  --binary FILE   wxml-lsp release binary (default: <repo>/target/release/wxml-lsp)
  --assets FILE   asset lock (default: <repo>/assets.lock.json)
  -h, --help      print this message

Relative paths resolve against the current directory; the root must be outside
the repository. On success this script prints:
  export WXML_LSP_BIN="<root>/bin/wxml-lsp"
USAGE
}

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"
root=""
binary="$repo_root/target/release/wxml-lsp"
assets="$repo_root/assets.lock.json"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --root) root="${2:?--root requires a directory}"; shift 2 ;;
        --binary) binary="${2:?--binary requires a file}"; shift 2 ;;
        --assets) assets="${2:?--assets requires a file}"; shift 2 ;;
        --help|-h) usage; exit 0 ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done

[ -n "$root" ] || root="${WXML_LSP_HOME:-${HOME:?HOME must be set when --root is not supplied}/.local/share/wxml-lsp}"
root="$(realpath -m -- "$root")"
[ "$root" != "/" ] && [ "$root" != "$(realpath -m -- "${HOME:-/nonexistent}")" ] || die "refusing to use / or HOME as the install root"
case "$root/" in "$repo_root/"*) die "install root must be outside the repository: $root" ;; esac
case "$root" in *[[:cntrl:]]*|*'"'*|*'$'*|*'`'*|*\\*) die "install root has unsupported characters: $root" ;; esac

for tool in python3 curl tar sha256sum mktemp install realpath; do
    command -v "$tool" >/dev/null 2>&1 || die "required tool \`$tool\` was not found"
done
[ -f "$assets" ] || die "asset lock not found: $assets"
[ -f "$binary" ] && [ -x "$binary" ] || die "release binary missing or not executable: $binary (run: cargo build --release -p wxml-lsp)"

tmp="$(mktemp -d)"
cleanup() { local status=$?; trap - EXIT; rm -rf -- "$tmp"; exit "$status"; }
trap cleanup EXIT

manifest="$tmp/assets.tsv"
if ! python3 - "$assets" >"$manifest" <<'PY'
import json, re, sys

def die(msg):
    sys.exit("error: " + msg)
try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        lock = json.load(fh)
except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
    die("cannot parse %s: %s" % (sys.argv[1], exc))
if not isinstance(lock, dict) or lock.get("schemaVersion") != 1:
    die("unsupported assets lock schemaVersion (expected 1)")
assets = lock.get("assets")
if not isinstance(assets, list) or not assets:
    die("assets lock contains no assets")
NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]*$")
PKG = re.compile(r"^[A-Za-z0-9._+-]+(?:/[A-Za-z0-9._+-]+)*$")
SHA = re.compile(r"^[0-9a-f]{64}$")
STRIP = re.compile(r"^[A-Za-z0-9._+-]+/$")
def need(obj, key, label):
    value = obj.get(key) if isinstance(obj, dict) else None
    if not isinstance(value, str) or not value:
        die("%s: missing string field %s" % (label, key))
    return value
seen = set()
for i, asset in enumerate(assets):
    name = need(asset, "name", "assets[%d]" % i)
    tarball = asset.get("tarball") if isinstance(asset, dict) else None
    url = need(tarball, "url", name)
    size = tarball.get("size") if isinstance(tarball, dict) else None
    sha = need(tarball, "sha256", name)
    pkg = need(asset, "packageRoot", name)
    if not NAME.match(name):
        die("assets[%d]: unsafe asset name %r" % (i, name))
    if not url.startswith("https://") or any(c.isspace() for c in url):
        die("%s: tarball url must be one https URL" % name)
    if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
        die("%s: tarball size must be a positive integer" % name)
    if not SHA.match(sha):
        die("%s: tarball sha256 must be 64 lowercase hex characters" % name)
    if not PKG.match(pkg) or pkg == "bin" or pkg.startswith("bin/"):
        die("%s: unsafe packageRoot %r" % (name, pkg))
    if pkg in seen:
        die("%s: duplicate packageRoot %r" % (name, pkg))
    seen.add(pkg)
    if not STRIP.match(need(asset.get("upstream"), "stripPrefix", name)):
        die("%s: stripPrefix must be exactly one directory" % name)
    print("\t".join((name, pkg, url, str(size), sha)))
PY
then
    die "could not parse assets lock (see python3 error above): $assets"
fi
[ -s "$manifest" ] || die "assets lock contains no installable assets: $assets"

installed=0
while IFS=$'\t' read -r name package_root url size sha256; do
    [ -n "$name" ] || continue
    dir="$tmp/$name"; tarball="$dir/$name.tgz"
    mkdir -p -- "$dir"
    printf 'downloading %s\n' "$name"
    curl -fL --retry 2 --retry-delay 2 --connect-timeout 20 -o "$tarball" "$url" \
        || die "$name: download failed: $url"
    actual_size="$(wc -c <"$tarball")"; actual_size="${actual_size//[[:space:]]/}"
    [ "$actual_size" = "$size" ] \
        || die "$name: size mismatch (expected $size bytes, got $actual_size)"
    sha_line="$(sha256sum -- "$tarball")"; actual_sha="${sha_line%% *}"
    [ "$actual_sha" = "$sha256" ] \
        || die "$name: sha256 mismatch (expected $sha256, got $actual_sha)"
    unpack="$dir/unpack"; mkdir -p -- "$unpack"
    # The Python parser rejects any stripPrefix that is not exactly one
    # directory, so --strip-components=1 matches every pinned tarball.
    tar --no-same-owner -xzf "$tarball" -C "$unpack" --strip-components=1 \
        || die "$name: failed to extract $tarball"
    [ -n "$(ls -A -- "$unpack")" ] || die "$name: tarball extracted no files"
    destination="$root/$package_root"; mkdir -p -- "$(dirname -- "$destination")"
    rm -rf -- "$destination"
    mv -- "$unpack" "$destination"
    installed=$((installed + 1))
done <"$manifest"
[ "$installed" -gt 0 ] || die "no assets were installed"

engine="$root/engine/lib/tsc"; typings="$root/typings/miniprogram-api-typings"
[ -f "$engine" ] && [ -x "$engine" ] || die "engine missing or not executable: $engine"
[ -f "$typings/index.d.ts" ] || die "typings missing after install: $typings/index.d.ts"
mkdir -p -- "$root/bin"
install -m 0755 -- "$binary" "$root/bin/wxml-lsp"
[ -x "$root/bin/wxml-lsp" ] || die "installed binary is not executable: $root/bin/wxml-lsp"

printf '\ninstalled native wxml-lsp\n  root:    %s\n  binary:  %s\n  typings: %s\n  engine:  %s\nexport WXML_LSP_BIN="%s"\n' \
    "$root" "$root/bin/wxml-lsp" "$typings" "$engine" "$root/bin/wxml-lsp"
