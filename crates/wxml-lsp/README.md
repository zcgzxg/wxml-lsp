# wxml-lsp

Language server for WeChat Mini Program WXML templates. This crate owns the LSP
transport, document state, request routing, project-host integration and the
native `tsgo` backend adapter. There is no Node backend and no Node process in
the production path.

Removing Node did not remove TypeScript/JavaScript support: business `.ts`,
`.js`, `.mts`, `.cts`, `.mjs`, `.cjs` and `.d.ts` inputs (plus JS JSDoc and
TS/JSON overlays) are still analyzed by the pinned native engine.

## Running

```bash
# Default: native engine from the installed package.
# Falls back to HostOnly only when this executable is not installed as
# <package>/bin/wxml-lsp.
wxml-lsp --stdio

# Strict native: requires a verified installed package and fails closed
# when discovery, manifest, engine or type assets are missing or mismatched.
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
<package>/bin/wxml-lsp
<package>/manifest.json          # schema v2, package/platform/protocol/pins
<package>/engine/lib/tsc         # pinned 7.1.0-beta
<package>/typings/               # pinned miniprogram-api-typings
```

Discovery never reads `PATH`, the working directory or backend environment
variables, and never falls back to Node. A missing install degrades to
`HostOnly`; every other discovery, asset, sandbox or transport failure fails
closed before the LSP handshake.

## Degradation

- **Native**: expression hover/definition/diagnostics/completion, script
  diagnostics, script document symbols and event-handler facts come from the
  pinned engine through an in-place shadow.
- **HostOnly**: template syntax, formatting, structure, event facts and script
  symbol facts keep working; session-backed type queries answer `Unavailable`
  with `native engine is not installed`. This is not a Node fallback.
- **Static limits** (for example a non-literal `Page(options)` registration)
  answer `Unsupported` with a path-free reason; they are never reported as an
  empty successful type result.

## Environment

No backend environment variable is read in the production path. Test-only
seams used by the opt-in native gates:

| Variable | Meaning |
| --- | --- |
| `TSGO_BIN` | Absolute engine `lib/tsc` path for real-engine tests |
| `TSGO_TYPINGS_ROOT` / `TSGO_ASSET_ROOT` | Pinned type-asset roots for real-engine tests |
| `WXML_REQUIRE_REAL_ENGINE=1` | Turn a missing/unusable real engine into a hard failure instead of a skip |
| `WXML_TEST_TMPDIR` | Absolute temporary root used by sandbox tests |

## Architecture

- `src/project_host.rs`: `ProjectHost`/`ProjectState`; real root, `FileId`,
  page/component views, script/JSON overlays, WXML snapshots, compiler-option
  filtering and `PreparedView` assembly.
- `src/backend_tsgo/`: native adapter; engine discovery and manifest checks,
  pinned type-asset loading, `0700` sandbox, native session lifecycle,
  shadow/query mapping, hover/definition/diagnostics/completion/document
  symbols and event-handler facts.
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

`cargo test -p wxml-lsp` works without an installed engine; real-engine tests
skip unless `TSGO_BIN`/`TSGO_TYPINGS_ROOT` are provided, and
`WXML_REQUIRE_REAL_ENGINE=1` turns a missing engine into a failure instead of a
green skip. The repository-level opt-in gate is:

```bash
scripts/native-gates.sh --package "$HOME/.local/share/wxml-lsp/versions/<version>"
```

Native gates are an opt-in required job, not default CI. They currently cover
40/40 cases (23 engine + 3 edit + 9 stdio + 1 perf + 1 stability + 3
real_matrix). `q2_stability` and `performance_bench` keep their measured
scenarios on the native path.

## Limits

- Requests are processed by one backend worker; a slow query serializes
  hover/diagnostics. The worker is not preemptible, and `shutdown` detaches
  after two seconds.
- `HostOnly` has no type queries; native sessions require the pinned install
  layout. Only Linux x86-64 with POSIX absolute roots is supported.
- Definition is limited to expression-member targets the native referenced
  symbol path can decode and map back to original text.
- Template bodies are not fully type-checked per call instance; WXS is a static
  subset and is never executed.
- Single LSP workspace with explicit multiple apps: no multi-root workspaces,
  no root hot reload, no cross-app dependency graph.
- Incremental parsing is bounded by source/style/resource/WXS budgets;
  truncated results are marked incomplete rather than fabricated.

## Security notes

- Engine discovery and manifest verification are path-pinned to the running
  executable's package; `PATH`, cwd and environment cannot redirect the engine.
- Engine and type assets are re-verified against the crate pin before a sandbox
  or session exists; symlinks, hard links, non-regular files and platform
  mismatches fail closed.
- Each session runs in a fresh `0700` sandbox with pinned argv/env/cwd; the
  native framing layer bounds frame headers, body sizes, in-flight requests and
  callbacks, pending queues, AST nodes/strings and document symbols. Truncation
  is reported as `Partial`, never as a supported empty result.
- `tsconfig` `baseUrl` values that are absolute or contain `..` are dropped
  (reported as partial compiler options) so they cannot escape the project.
- Backend-supplied capability reasons and diagnostics are filtered for
  control/format characters and length at the protocol edge; native errors are
  path-free before logging.
