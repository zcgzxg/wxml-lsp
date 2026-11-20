#!/usr/bin/env bash
# F3-14 Zed native smoke (default path or opt-in bridge).
#
# Default `--direct` mode starts the packaged `wxml-lsp` exactly as Zed ships
# it: `bin/wxml-lsp --stdio`, with no `--backend` argument. `--bridge` keeps
# the old test-only wrapper that appends `--backend native` for comparison.
# Both modes drive a real Zed process on an isolated Xvfb display and record:
#
#   - the server command line (direct: `--stdio`; bridge: `--stdio --backend
#     native`) and the absence of Node descendants in Zed's process tree;
#   - WXML hover and WXML syntax diagnostics, with screenshots and OCR;
#   - a hover that reflects an unsaved script edit, while the file on disk is
#     byte-for-byte unchanged;
#   - Zed's language indicator for WXML, JavaScript, TypeScript, TSX and JSON.
#
# It never writes inside the repository, never downloads anything and never
# touches the user's desktop: all state lives below /tmp.
#
# Usage:
#   scripts/f3-zed-smoke.sh --package DIR --grammar FILE [--direct|--bridge]
#                           [--output DIR]
#
#   --package DIR  Installed or unpacked native package root containing
#                  bin/wxml-lsp, manifest.json, engine/, typings/ and
#                  zed-extension/.
#   --grammar FILE Prebuilt tree-sitter-wxml wasm for the pinned revision. The
#                  release package carries the extension but not the grammar
#                  artifact; Zed normally builds it during install.
#   --direct       Default. Point WXML_LSP_BIN at the packaged binary; Zed
#                  passes only `--stdio` and the package selects native.
#   --bridge       Opt-in compatibility mode: wrapper appends `--backend native`.
#   --output DIR   Output directory under /tmp (default: /tmp/opencode/f3-zed-smoke).
#
# Exit status: 0 when the smoke passes or the environment is BLOCKED;
# 1 when the environment is present but a UI/process assertion fails.
#
# A direct-mode PASS is the evidence needed to close F3-14; a bridge-mode PASS
# is only a compatibility check and leaves F3-14 BLOCKED.

set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

PACKAGE=""
GRAMMAR=""
OUTPUT=${WXML_F3_ZED_OUTPUT:-/tmp/opencode/f3-zed-smoke}
MODE=direct

usage() {
    sed -n '2,38p' "$0" | sed 's/^# \{0,1\}//'
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

blocked() {
    printf 'F3-14-ZED: BLOCKED (%s)\n' "$*"
    exit 0
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --package)
            [ "$#" -ge 2 ] || die "--package requires a directory"
            PACKAGE=$2
            shift 2
            ;;
        --grammar)
            [ "$#" -ge 2 ] || die "--grammar requires a file"
            GRAMMAR=$2
            shift 2
            ;;
        --output)
            [ "$#" -ge 2 ] || die "--output requires a directory"
            OUTPUT=$2
            shift 2
            ;;
        --direct)
            MODE=direct
            shift
            ;;
        --bridge)
            MODE=bridge
            shift
            ;;
        --mode)
            [ "$#" -ge 2 ] || die "--mode requires direct or bridge"
            MODE=$2
            shift 2
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

[ -n "$PACKAGE" ] || die "--package is required"
[ -n "$GRAMMAR" ] || die "--grammar is required (pinned tree-sitter-wxml wasm)"
case "$MODE" in
    direct|bridge) ;;
    *) die "--mode must be direct or bridge (got $MODE)" ;;
esac
OUTPUT=$(realpath -m -- "$OUTPUT")
case "$OUTPUT/" in
    "$(realpath -m -- "$REPO_ROOT")/"*) die "--output must be outside the repository" ;;
esac
case "$OUTPUT" in
    /tmp/*) ;;
    *) die "--output must live under /tmp (got $OUTPUT)" ;;
esac
[ "$OUTPUT" != "/tmp" ] || die "--output must name a subdirectory of /tmp"
PACKAGE=$(realpath -m -- "$PACKAGE")
GRAMMAR=$(realpath -m -- "$GRAMMAR")

for tool in zed Xvfb xdotool import tesseract sha256sum pstree; do
    command -v "$tool" >/dev/null 2>&1 \
        || blocked "required tool is missing: $tool"
done

[ -x "$PACKAGE/bin/wxml-lsp" ] || blocked "missing packaged binary: $PACKAGE/bin/wxml-lsp"
[ -f "$PACKAGE/manifest.json" ] || blocked "missing package manifest: $PACKAGE/manifest.json"
[ -f "$PACKAGE/zed-extension/extension.wasm" ] || blocked "missing packaged Zed extension"
[ -d "$PACKAGE/zed-extension/languages/wxml" ] || blocked "missing packaged WXML language files"
[ -x "$PACKAGE/engine/lib/tsc" ] || blocked "missing packaged native engine"
[ -f "$PACKAGE/typings/miniprogram-api-typings/index.d.ts" ] \
    || blocked "missing packaged mini-program typings"
[ -f "$GRAMMAR" ] || blocked "missing grammar wasm: $GRAMMAR"
grep -q '"schemaVersion"[[:space:]]*:[[:space:]]*2' "$PACKAGE/manifest.json" \
    || blocked "package manifest is not schema v2"

rm -rf -- "$OUTPUT"
mkdir -p -- "$OUTPUT"/{logs,shots,data/config,data/home,data/extensions/installed/wxml-lsp,project/pages/index,project/pages/broken}

XPID=""
ZPID=""
cleanup() {
    if [ -n "$ZPID" ]; then
        kill -TERM -- "-$ZPID" 2>/dev/null || true
        sleep 0.5
        kill -KILL -- "-$ZPID" 2>/dev/null || true
    fi
    if [ -n "$XPID" ]; then
        kill -TERM "$XPID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# --- package identity -------------------------------------------------------

{
    printf 'package=%s\n' "$PACKAGE"
    printf 'mode=%s\n' "$MODE"
    printf 'manifest_sha256='; sha256sum "$PACKAGE/manifest.json" | cut -d' ' -f1
    printf 'wxml_lsp_sha256='; sha256sum "$PACKAGE/bin/wxml-lsp" | cut -d' ' -f1
    printf 'engine_sha256='; sha256sum "$PACKAGE/engine/lib/tsc" | cut -d' ' -f1
    printf 'extension_wasm_sha256='; sha256sum "$PACKAGE/zed-extension/extension.wasm" | cut -d' ' -f1
    printf 'grammar_sha256='; sha256sum "$GRAMMAR" | cut -d' ' -f1
    printf 'package_version='; "$PACKAGE/bin/wxml-lsp" --version 2>&1 | head -1
    printf 'default_backend_help='; "$PACKAGE/bin/wxml-lsp" --help 2>&1 \
        | grep -m1 -- '--backend <backend>' || true
    printf 'zed_version='; zed --version 2>&1 | head -1
} >"$OUTPUT/logs/package-identity.txt"

if [ "$MODE" = "direct" ]; then
    LSP_BIN="$PACKAGE/bin/wxml-lsp"
    if grep -q 'node (default)' "$OUTPUT/logs/package-identity.txt"; then
        printf 'F3-14-ZED-NOTE: package --help still documents node as default (documentation drift; process evidence decides)\n'
    else
        printf 'F3-14-ZED-NOTE: package --help documents native as default\n'
    fi
else
    # --- test-only bridge wrapper ------------------------------------------
    WRAPPER="$OUTPUT/bin/wxml-lsp"
    mkdir -p -- "$OUTPUT/bin"
    {
        printf '#!/bin/sh\n'
        printf '# Test-only bridge: the packaged binary plus the explicit opt-in backend.\n'
        printf 'exec %q "$@" --backend native\n' "$PACKAGE/bin/wxml-lsp"
    } >"$WRAPPER"
    chmod 755 "$WRAPPER"
    grep -q -- '--backend native' "$WRAPPER" || die "bridge wrapper does not select native"
    if grep -Eqi 'node|server\.js|WXML_TYPE_BACKEND|WXML_BACKEND_DIR|WXML_NODE' "$WRAPPER"; then
        die "bridge wrapper must not reference Node or legacy environment variables"
    fi
    LSP_BIN="$WRAPPER"
fi

# --- isolated extension and settings ---------------------------------------

cp -a -- "$PACKAGE/zed-extension/." "$OUTPUT/data/extensions/installed/wxml-lsp/"
mkdir -p -- "$OUTPUT/data/extensions/installed/wxml-lsp/grammars"
cp -a -- "$GRAMMAR" "$OUTPUT/data/extensions/installed/wxml-lsp/grammars/wxml.wasm"

cat >"$OUTPUT/data/extensions/index.json" <<'JSON'
{
  "extensions": {
    "wxml-lsp": {
      "manifest": {
        "id": "wxml-lsp",
        "name": "WXML LSP",
        "version": "0.0.1",
        "schema_version": 1,
        "description": "WXML support backed by a locally built wxml-lsp binary.",
        "repository": null,
        "authors": ["wxml-lsp contributors"],
        "lib": {"kind": "Rust", "version": null},
        "themes": [],
        "icon_themes": [],
        "languages": ["languages/wxml"],
        "grammars": {
          "wxml": {
            "repository": "https://github.com/BlockLune/tree-sitter-wxml",
            "rev": "1cc1761c8c11cacab5516dbe64e20ee689aa4428",
            "path": null
          }
        },
        "language_servers": {
          "wxml-lsp": {
            "language": null,
            "languages": ["WXML", "JavaScript", "TypeScript", "TSX", "JSON"],
            "language_ids": {
              "WXML": "wxml",
              "JavaScript": "javascript",
              "TypeScript": "typescript",
              "TSX": "typescriptreact",
              "JSON": "json"
            },
            "code_action_kinds": null
          }
        },
        "context_servers": {},
        "slash_commands": {},
        "snippets": null,
        "capabilities": []
      },
      "dev": false
    }
  }
}
JSON

cat >"$OUTPUT/data/config/settings.json" <<'JSON'
{
  "session": {"trust_all_worktrees": true},
  "auto_install_extensions": {
    "vtsls": false,
    "eslint": false,
    "emmet": false,
    "html": false
  },
  "languages": {
    "JavaScript": {"language_servers": ["wxml-lsp"]},
    "TypeScript": {"language_servers": ["wxml-lsp"]},
    "TSX": {"language_servers": ["wxml-lsp"]},
    "JSON": {"language_servers": ["wxml-lsp"]}
  }
}
JSON

cat >"$OUTPUT/data/config/keymap.json" <<'JSON'
[
  {
    "bindings": {
      "ctrl-alt-h": "editor::Hover",
      "ctrl-alt-d": "diagnostics::DeployCurrentFile"
    }
  }
]
JSON

printf 'export WXML_LSP_BIN=%s\n' "$LSP_BIN" >"$OUTPUT/data/home/.bashrc"

# --- fixture project --------------------------------------------------------

PROJECT="$OUTPUT/project"
cat >"$PROJECT/app.json" <<'JSON'
{"pages":["pages/index/index","pages/broken/broken"],"usingComponents":{}}
JSON
cat >"$PROJECT/tsconfig.json" <<'JSON'
{"compilerOptions":{"strict":true}}
JSON
cat >"$PROJECT/pages/index/index.wxml" <<'WXML'
<view>{{user.name}}</view>
<view>{{title}}</view>
WXML
cat >"$PROJECT/pages/index/index.ts" <<'TS'
Page({
  data: {
    user: {
      /** 用户显示名 */
      name: 'Ada'
    },
    /** 页面标题 */
    title: 'Hello'
  }
});
TS
cat >"$PROJECT/pages/index/index.json" <<'JSON'
{"usingComponents":{}}
JSON
cat >"$PROJECT/pages/broken/broken.wxml" <<'WXML'
<view class="broken">
  <text>oops
</view>
WXML
cat >"$PROJECT/pages/broken/broken.ts" <<'TS'
Page({ data: {} });
TS
cat >"$PROJECT/pages/broken/broken.json" <<'JSON'
{"usingComponents":{}}
JSON
printf 'export function greet(name) { return `hi ${name}`; }\n' >"$PROJECT/util.js"
printf 'export const View = () => <view>hello</view>;\n' >"$PROJECT/component.tsx"
printf '{"name":"fixture","private":true}\n' >"$PROJECT/config.json"

# --- display and Zed --------------------------------------------------------

DISPLAY_NUM=${WXML_F3_ZED_DISPLAY:-98}
while [ -e "/tmp/.X${DISPLAY_NUM}-lock" ]; do
    DISPLAY_NUM=$((DISPLAY_NUM + 1))
done
DISPLAY=":$DISPLAY_NUM"
export DISPLAY

Xvfb "$DISPLAY" -screen 0 1600x1000x24 -nolisten tcp >"$OUTPUT/logs/xvfb.log" 2>&1 &
XPID=$!
for _ in $(seq 1 40); do
    xdpyinfo -display "$DISPLAY" >/dev/null 2>&1 && break
    sleep 0.25
done
xdpyinfo -display "$DISPLAY" >/dev/null 2>&1 || blocked "Xvfb did not start"

HOME="$OUTPUT/data/home" WXML_LSP_BIN="$LSP_BIN" DISPLAY="$DISPLAY" WAYLAND_DISPLAY= \
    ZED_ALLOW_EMULATED_GPU=1 setsid zed --foreground --user-data-dir "$OUTPUT/data" \
    -n "$PROJECT" >"$OUTPUT/logs/zed.log" 2>&1 &
ZPID=$!

WINDOW=""
for _ in $(seq 1 80); do
    WINDOW=$(xdotool search --onlyvisible --name "$(basename "$PROJECT")" 2>/dev/null | head -1 || true)
    [ -n "$WINDOW" ] && break
    sleep 0.5
done
if [ -z "$WINDOW" ]; then
    blocked "Zed did not create a window on $DISPLAY (see $OUTPUT/logs/zed.log)"
fi

xdotool windowsize "$WINDOW" 1440 900
xdotool windowmove "$WINDOW" 0 0
xdotool windowfocus --sync "$WINDOW"
sleep 1

window_pids() {
    local root=$1
    printf '%s\n' "$root"
    local child
    for child in $(pgrep -P "$root" 2>/dev/null || true); do
        window_pids "$child"
    done
}

tree_ps() {
    local pid
    for pid in $(window_pids "$1"); do
        ps -o pid=,ppid=,comm=,args= -p "$pid" 2>/dev/null || true
    done
}

lsp_started() {
    if [ "$MODE" = "direct" ]; then
        tree_ps "$ZPID" | grep -Eq -- '/bin/wxml-lsp --stdio[[:space:]]*$'
    else
        tree_ps "$ZPID" | grep -Eq -- '/bin/wxml-lsp --stdio --backend native[[:space:]]*$'
    fi
}

wait_for_lsp() {
    local _ pid
    for _ in $(seq 1 80); do
        if lsp_started; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

open_file() {
    local name=$1
    xdotool key --clearmodifiers ctrl+p
    sleep 0.8
    xdotool type --clearmodifiers --delay 20 "$name"
    sleep 0.8
    xdotool key --clearmodifiers Return
    sleep 3
    # Emmet/onboarding popups can cover the status bar; Escape closes them.
    xdotool key --clearmodifiers Escape
    sleep 0.3
}

search_select() {
    local needle=$1
    xdotool key --clearmodifiers ctrl+f
    sleep 0.4
    xdotool type --clearmodifiers --delay 20 "$needle"
    sleep 0.8
    xdotool key --clearmodifiers Escape
    sleep 0.4
}

shot() {
    import -window "$WINDOW" "$OUTPUT/shots/$1.png"
}

# OCR is deliberately conservative: try several layouts and both the full
# window and the top-left editor region. A missing token fails the assertion.
ocr_contains() {
    local image=$1 token=$2
    local psm text
    for psm in 3 4 6 11 12; do
        text=$(tesseract "$image" - --psm "$psm" 2>/dev/null || true)
        if printf '%s' "$text" | grep -qi -- "$token"; then
            return 0
        fi
    done
    local crop="$OUTPUT/logs/ocr-crop.png"
    convert "$image" -crop 1000x420+0+60 +repage -resize 300% -colorspace Gray \
        -contrast-stretch 0.5%x0.5% "$crop" 2>/dev/null || return 1
    for psm in 3 4 6 11 12; do
        text=$(tesseract "$crop" - --psm "$psm" 2>/dev/null || true)
        if printf '%s' "$text" | grep -qi -- "$token"; then
            return 0
        fi
    done
    return 1
}

status_ocr() {
    local image=$1 width height
    width=$(identify -format '%w' "$image")
    height=$(identify -format '%h' "$image")
    convert "$image" -crop "340x34+$((width - 460))+$((height - 34))" +repage \
        -resize 600% -colorspace Gray -contrast-stretch 1%x1% \
        "$OUTPUT/logs/status-crop.png" 2>/dev/null || return 1
    tesseract "$OUTPUT/logs/status-crop.png" - --psm 6 2>/dev/null || true
    tesseract "$OUTPUT/logs/status-crop.png" - --psm 7 2>/dev/null || true
}

status_contains() {
    local image=$1 token=$2
    printf '%s' "$(status_ocr "$image")" | grep -qi -- "$token"
}

FAILURES=0
fail() {
    printf 'F3-14-ZED-CHECK: FAIL - %s\n' "$*"
    FAILURES=$((FAILURES + 1))
}
pass() {
    printf 'F3-14-ZED-CHECK: PASS - %s\n' "$*"
}

# --- WXML hover and diagnostics --------------------------------------------

open_file index.wxml
if wait_for_lsp; then
    if [ "$MODE" = "direct" ]; then
        pass "Zed started the packaged default server (--stdio, no --backend)"
    else
        pass "Zed started the packaged bridge server (--stdio --backend native)"
    fi
else
    fail "expected wxml-lsp process did not appear below Zed"
fi

tree_ps "$ZPID" >"$OUTPUT/logs/zed-tree.txt"
pstree -ap "$ZPID" >"$OUTPUT/logs/zed-pstree.txt" 2>/dev/null || true

if lsp_started; then
    if [ "$MODE" = "direct" ]; then
        pass "ps shows wxml-lsp --stdio exactly (no --backend argument)"
    else
        pass "ps shows wxml-lsp --stdio --backend native"
    fi
else
    fail "ps does not show the expected wxml-lsp command line"
fi
if [ "$MODE" = "direct" ] && grep -Eq -- '/bin/wxml-lsp .*--backend' "$OUTPUT/logs/zed-tree.txt"; then
    fail "direct mode passed a --backend argument"
fi
if grep -Eq -- '--backend node|server\.js|(^|[ /])node( |$)' "$OUTPUT/logs/zed-tree.txt"; then
    fail "Node or the legacy Node backend appears in Zed's process tree"
else
    pass "no Node process or server.js in Zed's process tree"
fi

search_select name
xdotool key --clearmodifiers ctrl+alt+h
sleep 2
shot 20-wxml-hover
if ocr_contains "$OUTPUT/shots/20-wxml-hover.png" 'string'; then
    pass "WXML hover shows the inferred string type"
else
    fail "WXML hover text did not contain 'string'"
fi

open_file broken.wxml
xdotool key --clearmodifiers ctrl+alt+d
sleep 1.5
shot 21-wxml-diagnostics
if ocr_contains "$OUTPUT/shots/21-wxml-diagnostics.png" 'element is not closed'; then
    pass "WXML diagnostics show the parser error"
else
    fail "WXML diagnostics did not show the parser error"
fi
xdotool key --clearmodifiers ctrl+alt+d
sleep 0.5

# --- five language indicators ----------------------------------------------

declare -A LANGUAGE_FILES=(
    [WXML]=index.wxml
    [JavaScript]=util.js
    [TypeScript]=index.ts
    [TSX]=component.tsx
    [JSON]=config.json
)
for language in WXML JavaScript TypeScript TSX JSON; do
    open_file "${LANGUAGE_FILES[$language]}"
    shot "30-language-${language}"
    if status_contains "$OUTPUT/shots/30-language-${language}.png" "$language"; then
        pass "Zed shows ${language} for ${LANGUAGE_FILES[$language]}"
    else
        fail "Zed did not show ${language} in the status bar"
    fi
done

# --- unsaved script hover and disk immutability ----------------------------

TS_FILE="$PROJECT/pages/index/index.ts"
BEFORE=$(sha256sum "$TS_FILE" | cut -d' ' -f1)
open_file index.ts
search_select "'Ada'"
xdotool type --clearmodifiers --delay 20 '42'
sleep 2
shot 40-unsaved-edit
open_file index.wxml
search_select name
xdotool key --clearmodifiers ctrl+alt+h
sleep 2
shot 41-unsaved-hover
if ocr_contains "$OUTPUT/shots/41-unsaved-hover.png" 'number'; then
    pass "WXML hover reflects the unsaved script edit (number)"
else
    fail "unsaved script hover did not show 'number'"
fi
AFTER=$(sha256sum "$TS_FILE" | cut -d' ' -f1)
if [ "$BEFORE" = "$AFTER" ] && grep -q "name: 'Ada'" "$TS_FILE" \
    && ! grep -q 'name: 42' "$TS_FILE"; then
    pass "unsaved buffer stayed in memory; index.ts hash is unchanged"
else
    fail "unsaved script was written to disk"
fi
{
    printf 'before=%s\nafter=%s\n' "$BEFORE" "$AFTER"
    printf 'disk_line='; grep -n "name:" "$TS_FILE" | head -1
} >"$OUTPUT/logs/unsaved-disk.txt"

# --- final process evidence -------------------------------------------------

tree_ps "$ZPID" >"$OUTPUT/logs/zed-tree-final.txt"
pstree -ap "$ZPID" >"$OUTPUT/logs/zed-pstree-final.txt" 2>/dev/null || true
if [ "$MODE" = "direct" ]; then
    LSP_COUNT=$(grep -Ec -- '/bin/wxml-lsp --stdio[[:space:]]*$' "$OUTPUT/logs/zed-tree-final.txt" || true)
else
    LSP_COUNT=$(grep -Ec -- '/bin/wxml-lsp --stdio --backend native[[:space:]]*$' "$OUTPUT/logs/zed-tree-final.txt" || true)
fi
if [ "${LSP_COUNT:-0}" -eq 1 ]; then
    pass "exactly one wxml-lsp process remains after all five languages"
else
    fail "expected exactly one wxml-lsp process, found ${LSP_COUNT:-0}"
fi

if grep -Eqi 'starting language server process.*--stdio' "$OUTPUT/data/logs/Zed.log"; then
    pass "Zed log records the language server start"
else
    fail "Zed log has no language server start line"
fi
if grep -q 'args: \["--stdio"\]' "$OUTPUT/data/logs/Zed.log"; then
    if [ "$MODE" = "direct" ]; then
        pass "Zed passed only --stdio; the package selected native"
    else
        pass "Zed passed only --stdio; the bridge appended --backend native"
    fi
else
    fail "Zed log does not show the --stdio launch"
fi
if grep -Eqi 'starting language server process.*(vtsls|eslint|server\.js|/usr/bin/node)' "$OUTPUT/data/logs/Zed.log"; then
    fail "a Node-based language server was selected"
else
    pass "no vtsls/eslint/Node language server in the Zed log"
fi

printf '\n'
printf 'F3-14-ZED-ARTIFACTS: %s\n' "$OUTPUT"
printf 'F3-14-ZED-PACKAGE: %s\n' "$PACKAGE"
printf 'F3-14-ZED-LOG: %s\n' "$OUTPUT/logs/zed.log"

if [ "$FAILURES" -ne 0 ]; then
    printf 'F3-14-ZED-SMOKE: FAIL (%s checks failed; see %s)\n' "$FAILURES" "$OUTPUT"
    exit 1
fi

if [ "$MODE" = "direct" ]; then
    printf 'F3-14-ZED-SMOKE: PASS (direct default path; real Zed UI on Xvfb)\n'
    printf 'F3-14: PASS (default packaged --stdio; evidence complete)\n'
else
    printf 'F3-14-ZED-SMOKE: PASS (bridge wrapper; real Zed UI on Xvfb)\n'
    printf 'F3-14: BLOCKED (bridge only; default-path packaged Zed smoke is not recorded)\n'
fi
