# AGENTS.md

Native-only WXML language server in Rust. There is no Node runtime in the
production path. The Zed extension under `editors/zed/` is a separate WASM
workspace. Linux x86_64 is the only verified platform.

## Read first

- `README.md` is the current behavior/build/install baseline.
- `docs/README.md` defines doc ownership and authority: code/tests > accepted
  ADR > `docs/reference/` > README.
- The `.opencode/` harness config (agents, commands, instructions) was removed
  from the repo; keep guidance in this file. History still has the old files.
- `openspec/` holds OpenSpec change proposals and specs; `openspec/config.yaml`
  carries the project context. Its workflow skills live in `.opencode/skills/`
  (the `--scope project` default target `.agents/skills/` is moved there so
  OpenCode picks them up; re-move after `openspec update`). Architectural
  decisions still belong in `docs/adr/`.
- Architecture context: `docs/adr/0001` (layering), `0006` (explicit roots),
  `0009` (slimming), `0010` (zero-config lazy discovery), `0012` (in-process
  type session). ADR-0008 was deleted; its still-valid decisions live in
  `0012`, and the number is not reused.
- `tests/fixtures/README.md` documents the current layout: every sample is a
  real mini-program project read directly by crate tests. The `wxml-fixtures`
  crate and the Node `wave5_acceptance` runner are deleted; do not recreate them.

## Verification

Root workspace (matches CI):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo doc --workspace --no-deps
cargo deny --offline check
```

`cargo deny` needs `cargo deny fetch db` once first (CI does this).
The workspace MSRV is Rust 1.100 (`rust-version` in the root `Cargo.toml`);
`rust-toolchain.toml` tracks stable with clippy/rustfmt.
Focused checks: `cargo test -p wxml-text`, `cargo test -p wxml-syntax`, etc.;
a single test: `cargo test -p wxml-backend-tsrs --test shadow_mapping <name>`.

The Zed extension is **not** covered by root cargo commands; it has its own
lockfile and `[workspace]`:

```bash
cd editors/zed
cargo fmt --all -- --check
cargo check --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --target wasm32-wasip2 --locked
```

This needs the `wasm32-wasip2` target; grammar builds may need `wasi-sdk` or
`WASI_SDK_PATH` (see `editors/zed/README.md`).

## Backend and assets

- The type engine runs in-process (`tsrs_api::Session`). Installation only
  needs the binary and typings; there is no engine executable, no tarball and
  no `engine/lib` standard library. `wxml-lsp --stdio` is native by default,
  but running the binary from `target/debug` or `target/release` is not an
  installed package: it serves HostOnly and type queries return `Unavailable`.
  To exercise real types, install the typings layout:

  ```bash
  cargo build --release -p wxml-lsp
  scripts/install-native.sh --root "$HOME/.local/share/wxml-lsp"
  ```

  The installer needs network plus `python3`/`curl`/`tar` to fetch the typings
  tarball; use `--assets` for a local lock and keep the root outside the
  repository. Runtime discovery starts at `current_exe()` and requires
  `<root>/bin/wxml-lsp` and `<root>/typings/miniprogram-api-typings`; it does
  not use `PATH`, the cwd, or any engine-override variable.

- `tsrs_api`, `tsrs_core` and `tsrs_vfs` are pinned exactly
  (`=7.1.0-beta.tsrs4`), and `miniprogram-api-typings@5.3.1` is pinned in
  `assets.lock.json` with hardcoded digests in
  `crates/wxml-backend-tsrs/src/{config,input}.rs`. Keep pins and lockfile in
  sync; a mismatch fails closed before the LSP handshake. TypeScript standard
  library comes from `tsrs_vfs`; do not read `TSRS_LIB_PATH` or install
  `lib*.d.ts` into the package.

- Startup validates `tsrs_core::version()`, AST protocol v9/header 64/stride 28
  and the capability probes before enabling types. Compile-time
  `TSRS_TS_VERSION` must stay unset (CI asserts this).

- Failure model: a single request panic fails that request; an unrecoverable
  session becomes `SessionPoisoned` and type features are unavailable while the
  rest of the LSP keeps serving. There is no timeout cancellation, no process
  restart and no backoff. `TSRS_BIN`/`TSRS_ASSET_ROOT` are removed; do not
  reintroduce an external engine override.

- The Node backend is gone. `--backend node` and `WXML_NODE`,
  `WXML_TYPE_BACKEND`, `WXML_BACKEND_DIR`, `WXML_ANALYZE_TIMEOUT_MS`,
  `WXML_TEMPLATE_CHECK` are removed; do not reintroduce them.

## Architecture constraints

- Allowed dependencies (ADR-0001): `wxml-text` <- `wxml-syntax` <-
  `wxml-types`; `wxml-project` -> `wxml-text`; `wxml-backend-tsrs` ->
  `wxml-types`; `wxml-lsp` -> all. Never add reverse dependencies, and never
  make `wxml-types` depend on `wxml-project`.
- Keep `wxml-lsp` a protocol/adapter layer. Parsing belongs in `wxml-syntax`,
  project facts in `wxml-project`, template/type semantics in `wxml-types`, and
  the in-process type session/shadow/query work in `wxml-backend-tsrs`.
- Use `wxml-text` for byte/UTF-16/LSP position conversion. Do not hand-roll
  offsets; Unicode and CRLF fixtures assert exact byte behavior.
- The LSP server's stdout is JSON-RPC only. Logging and diagnostics must go
  through LSP messages or stderr.
- Default config is zero-config lazy discovery: from the current WXML find the
  nearest `app.json` and independently the nearest `tsconfig.json`, with the
  workspace root as `projectRoot` (ADR-0010). Explicit
  `initializationOptions.apps` pins roots and keeps ADR-0006 behavior: no
  upward scan. Neither mode scans `node_modules`. Lazy mode discovers
  candidates from root `package.json#workspaces` or `pnpm-workspace.yaml` and
  mounts only packages named in the app package dependencies; explicit mode
  mounts `apps[].dependencies`.
- The server is read-only for user workspaces: overlays live in memory and
  rename returns `documentChanges` without writing files.

## Tests and fixtures

- A default `cargo test` is green without the type engine: real-type suites
  skip unless `TSRS_TYPINGS_ROOT` is set. `WXML_REQUIRE_REAL_ENGINE=1` turns a
  missing engine/typings gate into a hard failure; the names are legacy, the
  engine itself is in-process. Do not claim native type behavior was verified
  from a green default run.
- Shared AC fixtures live in `tests/fixtures/`. `.gitattributes` requires LF
  for normal `.wxml` files and byte-exact CRLF for `*-crlf.wxml`; do not
  normalize line endings.
- `crates/wxml-lsp/tests/` holds stdio/LSP integration tests;
  `crates/wxml-backend-tsrs/tests/` holds native/shadow/script tests.
- Performance budgets are opt-in and timing-sensitive. Run them serially
  (`--test-threads=1`) on a quiet machine, not shared CI.

## Code and docs conventions

- Workspace lints: `unsafe_code = deny`; clippy `all = deny`;
  `unwrap_used`/`expect_used` warn (so `-D warnings` rejects them outside
  tests); `missing_docs` warns for public items. `cargo fmt` uses width 100.
- Adding a dependency can fail `cargo deny` license or duplicate-version
  checks; update `deny.toml` deliberately in the same change.
- Project docs are primarily Chinese; match the surrounding file. They are
  self-contained (ADR-0005): relative links only, no absolute local paths or
  external repositories as sole evidence. New architectural decisions go in
  `docs/adr/NNNN-*.md` and `docs/adr/README.md`; accepted does not mean
  implemented.
- Run experiments outside the repository; do not add temp files or evidence
  directories (ADR-0004/0005).
- Do not restore the ADR-0009 deletions (`wxml-delivery`, `wxml-fixtures`,
  parity/ledger/Q2 gates, `docs/evidence`, `docs/plans`); history is at tag
  `pre-slim-20261120`.
- Commit messages follow `git log`: Conventional Commits such as `feat:`,
  `fix(scope):`, `docs(scope):`.
