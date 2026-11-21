# wxml-lsp

Language server for WeChat Mini Program WXML templates. This crate owns the LSP
transport, document state, request routing, project-host integration and the
in-process type backend adapter. There is no Node backend, no Node process and
no external engine process in the production path.

Removing Node did not remove TypeScript/JavaScript support: business `.ts`,
`.js`, `.mts`, `.cts`, `.mjs`, `.cjs` and `.d.ts` inputs (plus JS JSDoc and
TS/JSON overlays) are still analyzed by the pinned in-process type engine.

## Running

```bash
# Default: in-process type session with typings from the installed package.
# Falls back to HostOnly only when this executable is not installed as
# <package>/bin/wxml-lsp.
wxml-lsp --stdio

# Strict native: requires a verified installed layout and fails closed
# when discovery or the pinned typings are missing or mismatched.
wxml-lsp --stdio --backend native

# In-process mock backend, no project (used by the skeleton tests).
wxml-lsp --stdio --mock
```

`--backend node` is rejected with `the node backend was removed; expected
native`. The legacy `WXML_NODE`, `WXML_TYPE_BACKEND`, `WXML_BACKEND_DIR`,
`WXML_ANALYZE_TIMEOUT_MS` and `WXML_TEMPLATE_CHECK` variables are removed;
setting them has no effect.

The default `--stdio` mode opens the project from `initialize.rootUri` (falling
back to `workspaceFolders[0].uri`). Discovery starts at `current_exe()` and
requires the installed layout:

```text
<root>/bin/wxml-lsp
<root>/typings/                  # pinned miniprogram-api-typings
```

The TypeScript standard library is embedded in the binary; no `engine/`
directory, engine executable or `lib*.d.ts` tree is installed or read.

Discovery never reads `PATH`, the working directory or engine-override
variables, and never falls back to Node. A missing install degrades to
`HostOnly`; typings discovery or digest failures fail closed before the LSP
handshake. An in-process session that fails to initialise or becomes poisoned
degrades type features to `Unavailable` while the rest of the server keeps
serving.

## Degradation

- **Native**: expression hover/definition/diagnostics/completion, script
  diagnostics, script document symbols and event-handler facts come from the
  in-process pinned type session through an in-place shadow.
- **HostOnly**: template syntax, formatting, structure, event facts and script
  symbol facts keep working; session-backed type queries answer `Unavailable`
  with a path-free reason. This is not a Node fallback.
- **Degraded**: an uninitialised or poisoned type session answers type queries
  `Unavailable`; no type result is fabricated and no session is silently
  restarted.
- **Static limits** (for example a non-literal `Page(options)` registration)
  answer `Unsupported` with a path-free reason; they are never reported as an
  empty successful type result.

## Environment

No engine-override environment variable is read in the production path.
Test-only seams used by the opt-in real-type gates:

| Variable | Meaning |
| --- | --- |
| `TSGO_TYPINGS_ROOT` | Pinned `miniprogram-api-typings` root for real-type tests |
| `WXML_REQUIRE_REAL_ENGINE=1` | Turn a missing/unusable typings gate into a hard failure instead of a skip |
| `WXML_TEST_TMPDIR` | Absolute temporary root used by sandbox tests |

`TSGO_TYPINGS_ROOT` and `WXML_REQUIRE_REAL_ENGINE` keep their historical names;
the type engine itself is in-process. `TSGO_BIN` and `TSGO_ASSET_ROOT` are
removed and have no effect.

## Architecture

- `src/project_host.rs`: `ProjectHost`/`ProjectState`; real root, `FileId`,
  page/component views, script/JSON overlays, WXML snapshots, compiler-option
  filtering and `PreparedView` assembly.
- `src/backend_tsgo/`: in-process type adapter; typings discovery and layout
  checks, pinned type-asset loading, read-only virtual filesystem, session
  lifecycle, shadow/query mapping, hover/definition/diagnostics/completion/
  document symbols and event-handler facts.
- `src/server.rs`, `src/handlers/`, `src/workspace.rs`, `src/scheduler.rs`:
  protocol lifecycle, document state, bounded scheduling and publication
  barriers.
- `src/configuration.rs`, `src/styles.rs`, `src/structure.rs`,
  `src/formatting.rs`, `src/semantics/`: initialization options, style/resource
  indexes, outline, formatting and static template semantics.

`run_stdio(backend)` remains the generic in-process entry point used by mock and
skeleton tests. `run_stdio_with_host(backend, host)` and
`run_with_host(reader, writer, backend, host)` add real project integration;
`run_stdio_native`, `run_stdio_native_default` and `run_stdio_host_only` are the
production selectors described above.

## Tests

```bash
cargo fmt -p wxml-lsp -- --check
cargo clippy -p wxml-lsp --all-targets -- -D warnings
cargo test -p wxml-lsp
RUSTDOCFLAGS="-D warnings" cargo doc -p wxml-lsp --no-deps
```

`cargo test -p wxml-lsp` works without an installed layout; real-type tests in
`tests/native_lsp_document_symbols.rs` and the backend's
`crates/wxml-backend-tsgo/tests/real_engine.rs` skip unless
`TSGO_TYPINGS_ROOT` is provided, and `WXML_REQUIRE_REAL_ENGINE=1` turns a
missing typings gate into a failure instead of a green skip.

The former `scripts/native-gates.sh` 40/40 gate and the Q2/performance/stdio
suites were removed by ADR-0009; their historical versions are available at tag
`pre-slim-20261120`. Current default CI runs the normal `check` job only.

## Limits

- Requests are processed by one backend worker; a slow query serializes
  hover/diagnostics. The worker is not preemptible, and `shutdown` detaches
  after two seconds.
- `HostOnly` has no type queries; type sessions require the pinned typings
  layout. Only Linux x86-64 with POSIX absolute roots is supported.
- The type engine runs in-process: a single-request panic is returned as that
  request's failure and poisons the session; OOM, abort or stack overflow ends
  the whole process. Running type work cannot be cancelled, and a poisoned
  session is not restarted automatically.
- Definition is limited to expression-member targets the native referenced
  symbol path can decode and map back to original text.
- Template bodies are not fully type-checked per call instance; WXS is a static
  subset and is never executed.
- Single LSP workspace with explicit multiple apps: no multi-root workspaces,
  no root hot reload, no cross-app dependency graph.
- Incremental parsing is bounded by source/style/resource/WXS budgets;
  truncated results are marked incomplete rather than fabricated.

## Security notes

- Typings discovery is path-pinned to the running executable's package; `PATH`,
  cwd and environment cannot redirect it, and `TSGO_BIN`/`TSGO_ASSET_ROOT` are
  not recognised.
- Typings are re-verified against the crate pin and `assets.lock.json` before a
  session exists; symlinks, hard links, non-regular files and platform
  mismatches fail closed. The TypeScript standard library is embedded, so no
  engine artifact is downloaded or verified.
- The in-process session reads through a read-only virtual filesystem that never
  falls back to the host; request/AST/document-symbol budgets still bound sizes,
  and truncation is reported as `Partial`, never as a supported empty result.
- `tsconfig` `baseUrl` values that are absolute or contain `..` are dropped
  (reported as partial compiler options) so they cannot escape the project.
- Backend-supplied capability reasons and diagnostics are filtered for
  control/format characters and length at the protocol edge; native errors are
  path-free before logging.
