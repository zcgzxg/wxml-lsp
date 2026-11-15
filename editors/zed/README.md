# WXML LSP Zed extension (local development)

Minimal, local-only Zed extension for WXML. It provides:

- a `WXML` language (`languages/wxml/`) with highlighting, brackets, indentation,
  and JavaScript injection for `{{...}}` / `<wxs>` bodies;
- a `wxml-lsp` language server entry that launches a locally built
  `wxml-lsp --stdio` binary.

The extension id is `wxml-lsp` and its display name is `WXML LSP`. This avoids
colliding with the Marketplace `wxml` extension (grammar-only), which can be
auto-updated and overwrite a local dev extension.

It does **not** download or install the language server, and it is not
published to the Zed extension marketplace.

## Prerequisites

- Zed with extension API 0.7 support (stable Zed 1.28.1 supports 0.6.0–0.7.0;
  older Zed 0.205.x also supports 0.7.0).
- Rust toolchain with `wasm32-wasip2` available. Zed installs the target when it
  builds a dev extension; if Rust was installed outside rustup, provide the
  target yourself.
- `wasi-sdk` for compiling the tree-sitter grammar (tested with wasi-sdk 34).
  Zed usually downloads it automatically; this requires network access. To
  reuse an existing install, set `WASI_SDK_PATH` to the directory containing
  `bin/clang`.
- A built `wxml-lsp` binary from this repository.
- For type-aware features, the TypeScript backend and **Node.js >= 24**.

## 1. Build the language server

From the repository root:

```bash
cargo build -p wxml-lsp
# or, for a smaller/faster binary:
cargo build -p wxml-lsp --release
```

The binary is at `target/debug/wxml-lsp` or `target/release/wxml-lsp`.

Type-aware features additionally require the TypeScript backend:

```bash
cd backends/typescript
npm ci
npm run build
```

Then point the server at the built backend before launching Zed:

```bash
export WXML_TYPE_BACKEND=/absolute/path/to/wxml-lsp/backends/typescript/dist/server.js
# optional: override the backend working directory (contains node_modules)
# export WXML_BACKEND_DIR=/absolute/path/to/wxml-lsp/backends/typescript
```

`WXML_TYPE_BACKEND` must be absolute; the server rejects relative values.
`WXML_BACKEND_DIR` is not a replacement for the server path: it overrides the
backend working directory (the directory containing `node_modules`), defaults
to the parent of `dist/`, and must also be absolute. Without
`WXML_TYPE_BACKEND`, `wxml-lsp` only probes an executable-relative backend
layout (`backends/typescript/dist/server.js` or
`wxml-type-backend/server.js`) and otherwise degrades to syntax-only
diagnostics. It never probes the process working directory. Set `WXML_NODE` to
an executable name or an absolute path if `node` is not on Zed's `PATH`.
`WXML_ANALYZE_TIMEOUT_MS` (default 15000) and `WXML_TEMPLATE_CHECK`
(`off`/`pragma`/`inherit`, default `pragma`) are optional.

## 2. Install the dev extension

1. Open Zed.
2. Run the command palette action `zed: install dev extension`.
3. Select this directory: `editors/zed`.

This is the standard installation path for the extension id `wxml-lsp`
(display name `WXML LSP`). Zed builds the extension and registers
the `WXML` language. The first build needs network access to fetch the grammar
and Rust dependencies, and a `wasi-sdk` for the grammar. Use `zed: open log`
(or restart with `zed --foreground`) to see extension/server startup logs.

Expected result: the extension appears as an installed dev extension, and the
status bar shows `WXML` for `.wxml` files. If the build fails, check network
access to the grammar repository, the `wasm32-wasip2` target, and `wasi-sdk`
(or `WASI_SDK_PATH` pointing at the directory containing `bin/clang`).

The isolated user-data-dir smoke in
[docs/evidence/p8-zed.md](../../docs/evidence/p8-zed.md) only proves that Zed
can load the wasm, register WXML, and start the server; it is **not** a
substitute for this standard installation step.

### Manual dev install and grammar build

For local testing, the extension can also be placed as a dev symlink/index
instead of using the GUI flow:

```text
~/.local/share/zed/extensions/installed/wxml-lsp -> <repo>/editors/zed
```

with `index.json` marking `wxml-lsp` as `dev = true` and including the WXML
language entry. This is an equivalent manual dev install, not a substitute for
validating the standard UI flow. Restart Zed after placing it, and export
`WXML_LSP_BIN` (plus `WXML_TYPE_BACKEND` for type features) in the shell that
starts Zed; see section 3.

Zed normally builds the pinned grammar during `install dev extension`. To
reproduce the real grammar artifact with wasi-sdk 34, from a
`BlockLune/tree-sitter-wxml` checkout at rev
`1cc1761c8c11cacab5516dbe64e20ee689aa4428`:

```bash
$WASI_SDK_PATH/bin/clang --target=wasm32-wasip1 -Os -fPIC -shared \
  -I src src/parser.c src/scanner.c \
  -Wl,--export=tree_sitter_wxml -Wl,--allow-undefined \
  -o <repo>/editors/zed/grammars/wxml.wasm
```

The artifact is about 23,806 B; `editors/zed/grammars/` is gitignored. The
isolated smoke used this artifact. The standard flow still lets Zed build the
grammar itself.

## 3. Point the extension at the server

Export absolute paths in the environment Zed inherits, then restart Zed:

```bash
export WXML_LSP_BIN=/absolute/path/to/wxml-lsp/target/debug/wxml-lsp
# local development: point at the built TypeScript backend
export WXML_TYPE_BACKEND=/absolute/path/to/wxml-lsp/backends/typescript/dist/server.js
# optional: backend working directory (contains node_modules)
# export WXML_BACKEND_DIR=/absolute/path/to/wxml-lsp/backends/typescript
```

The extension reads the variable from Zed's shell environment
(`Worktree::shell_env`), not from the project. A value set in a terminal after
Zed started is not visible until Zed is restarted from that environment.

`WXML_LSP_BIN` is the **only** supported source. There is no worktree
`target/...` probing and no `PATH` lookup. Opening an untrusted project does
not by itself make the extension run a project-controlled executable.

`Worktree::shell_env` still includes Zed's shell environment. If direnv is
enabled and a project's `.envrc` has been explicitly allowed, that `.envrc` is
a shell-environment source and can set `WXML_LSP_BIN` (or backend variables).
Allowing `.envrc` already executes project code, so review the file before
running `direnv allow`; the extension only validates the resulting path.

Rules enforced by `src/lib.rs`:

- The value must be non-empty and absolute for the **host OS**:
  - Windows: `C:\tools\wxml-lsp.exe`, `C:/tools/wxml-lsp.exe`, or a UNC path
    such as `\\server\share\wxml-lsp.exe` / `//server/share/wxml-lsp.exe`.
    POSIX `/...` paths are rejected, including `/C:/...`; drive-relative
    `C:foo` and bare `C:` are rejected.
  - Linux/macOS: `/usr/local/bin/wxml-lsp`. Windows drive paths and UNC paths
    are rejected.
- A path ending in a separator is rejected (`/opt/wxml-lsp/`, `C:\tools\`,
  `\\server\share\`); it must name a file.
- It must not contain `..` path components.
- It must exist and be executable. The check is performed by Zed's host
  `Worktree::which`; the WASM sandbox never reads or executes the file itself.
  Its result is validated again with the same host-specific rules.
- If the value is missing, wrong for the host OS, or invalid, the extension
  returns an error and the language server does **not** start. It does **not**
  fall back to any other location.

The host OS comes from `zed::current_platform().0`. Path validation is
string-based on purpose: the extension is compiled to `wasm32-wasip2`, whose
path APIs have POSIX semantics and would misclassify Windows drive/UNC paths on
a Windows host.

## Backend environment forwarding

The extension adds these variables from the shell environment to the language
server command when present:

| Variable | Constraint |
| --- | --- |
| `WXML_TYPE_BACKEND` | Absolute `server.js` path; relative values are rejected |
| `WXML_BACKEND_DIR` | Absolute backend directory containing `node_modules`; defaults to the parent of `dist/`; does not select `server.js` |
| `WXML_NODE` | Executable name or absolute path; default `node` |
| `WXML_ANALYZE_TIMEOUT_MS` | Per-request timeout in milliseconds; default `15000` |
| `WXML_TEMPLATE_CHECK` | `off` / `pragma` / `inherit`; default `pragma` |

Their full semantics are documented in
[crates/wxml-lsp/src/main.rs](../../crates/wxml-lsp/src/main.rs).

`Command.env` is **not** a redaction boundary: it only adds these variables,
while Zed still merges the full shell environment when spawning the language
server. Other exported variables may therefore be visible to the process.

## Coexistence with the JavaScript/TypeScript language server

The extension registers only the `WXML` language and the `wxml-lsp` server. It
does not attach to JavaScript/TypeScript buffers and does not replace or
configure the existing JS/TS language server.

Spike S1 conclusion for Zed 1.28.1: Zed registers only WXML buffers with
`wxml-lsp`; JavaScript/TypeScript/JSON `didOpen`/`didChange` notifications are
not forwarded to it. As a result, unsaved JS edits are not reflected in WXML
analysis. After saving, the next analysis re-collects the script from disk and
sees the saved content.

To disable this server for WXML:

```json
{
  "languages": {
    "WXML": {
      "language_servers": ["!wxml-lsp"]
    }
  }
}
```

## Editor UX notes

- `line_comments = []` is intentional: WXML has no line-comment syntax, so Zed
  uses `block_comment` (`<!-- ... -->`) for `editor::ToggleComments`.
- `block_comment.prefix = " "` adds a leading space to continuation lines
  inside a block comment.
- The bracket list includes both `{{`/`}}` and `{`/`}`. Typing a single `{` may
  auto-close to `{}`; remove the `{` pair from `languages/wxml/config.toml` if
  that gets in the way.

## Known limitations

- Local extension only: no marketplace release, no server download, no auto
  backend discovery.
- The grammar is the third-party
  [BlockLune/tree-sitter-wxml](https://github.com/BlockLune/tree-sitter-wxml)
  pinned at revision `1cc1761c8c11cacab5516dbe64e20ee689aa4428` (ABI 15). The
  queries in `languages/wxml/` were validated against that revision with
  `tree-sitter 0.25.8`. A real wasi-sdk 34 build (~23,806 B) has been loaded in
  the isolated smoke, but Zed's automatic grammar build and visual highlighting
  are still unconfirmed.
- The 0.7.0 extension API has no `zed::log` module; the extension logs with
  `eprintln!`, which Zed forwards to its own stderr. Use `zed --foreground` for
  verbose logs.
- `Worktree::which` is documented as a `PATH` lookup; absolute-path support is
  an implementation detail verified in Zed 1.28.1. If a future Zed changes that
  behavior, the extension may fail to start; it never falls back to another
  location.
- Spike S1 was not re-run as a runtime experiment in this task; the conclusion
  above is based on Zed 1.28.1's language-buffer registration behavior and
  should be rechecked after Zed upgrades. See
  [docs/evidence/p8-zed.md](../../docs/evidence/p8-zed.md).

## Current runtime status (2026-11-16)

Verified in isolated user-data directories with Zed 1.28.1, but **not** through
the standard UI install:

- Early smoke used a temporary HTML grammar renamed to `tree_sitter_wxml`.
- The final isolated test loaded the real `tree-sitter-wxml` grammar built with
  wasi-sdk 34 (~23,806 B); the extension id `wxml-lsp` was not overwritten by
  the Marketplace `wxml` extension.
- Zed registered the `WXML` language and started the real
  `target/release/wxml-lsp --stdio` binary; the second launch log was stable,
  with no unload/restart.
- A `user.nmae` typo in a WXML sample produced an inline diagnostic.

A manual dev symlink/index is present in the main Zed user-data directory
(`~/.local/share/zed/extensions/installed/wxml-lsp -> <repo>/editors/zed`,
`dev = true`). The main Zed process was restarted on 2026-11-16; it loaded the
extension, started `wxml-lsp --stdio`, and spawned the Node backend for
`tests/fixtures/page-basic`. The `user.nmae` typo shows a red diagnostic and the
file tree shows an error badge; WXML syntax colours are visible.

Not confirmed: standard `zed: install dev extension`, hover/definition contents,
real grammar query quality, Windows/macOS hosts, multi-worktree/remote projects,
and long-running server lifecycle. KDE's EIS permission dialog blocked automated
mouse/keyboard input.

## Verification

From this directory (standalone workspace, no changes to the repository root):

```bash
cargo fmt --all -- --check
cargo check --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --target wasm32-wasip2 --locked
cargo build --target wasm32-wasip2 --release --locked
```

The last two commands require the `wasm32-wasip2` target. Zed performs the same
build during `install dev extension`.

Current snapshot (2026-11-16): all six commands pass; `cargo test --locked`
reports 22 passed (1 suite). The built extension wasm is 7,633,946 B (debug)
and 238,792 B (release); the real grammar artifact is about 23,806 B
(gitignored).
