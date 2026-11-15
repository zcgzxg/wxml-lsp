# wxml-lsp

Language server for WeChat Mini Program WXML templates. This crate owns the LSP
transport, document state, request routing, project-host integration and the
C1.1 Node type backend adapter.

## Running

```bash
# Real project + Node type backend.
# For development, point at the absolute backend path:
WXML_TYPE_BACKEND="$PWD/backends/typescript/dist/server.js" wxml-lsp --stdio

# A packaged build may instead ship the backend under
# <exe-dir>/backends/typescript/dist/server.js.

# In-process mock backend, no project (used by the skeleton tests)
wxml-lsp --stdio --mock
```

The default `--stdio` mode opens the project from `initialize.rootUri` (falling
back to `workspaceFolders[0].uri`) and starts the Node backend lazily on the
first analysis. A missing project or Node backend degrades explicitly:

- WXML syntax diagnostics are still published;
- hover/definition return `null` (unavailable), never fabricated types;
- diagnostics carry a degraded `CapabilityStatus` warning instead of pretending
  the file was checked.

## Environment

| Variable | Default | Meaning |
| --- | --- | --- |
| `WXML_NODE` | `node` | Node executable name or absolute path (Node >= 24); relative paths with separators are rejected |
| `WXML_TYPE_BACKEND` | executable-relative install layout only | **Absolute** `server.js` path; relative values are rejected |
| `WXML_BACKEND_DIR` | parent of `dist/` | **Absolute** directory containing `node_modules`; relative values are rejected |
| `WXML_ANALYZE_TIMEOUT_MS` | `15000` | Per-request timeout (write + read) |
| `WXML_TEMPLATE_CHECK` | `pragma` | `off`, `pragma` or `inherit` |

The environment is trusted configuration. A relative `WXML_TYPE_BACKEND` or
`WXML_BACKEND_DIR` disables the type backend instead of resolving against the
editor's working directory, because a workspace may contain a malicious
`backends/typescript/dist/server.js`. The default probes only
`<exe-dir>/backends/typescript/dist/server.js` and
`<exe-dir>/wxml-type-backend/server.js`. An explicitly configured absolute path
is trusted even if it lives inside a workspace; that is the user's opt-in.

Build the backend before using the real mode:

```bash
(cd backends/typescript && npm ci && npm run build)
WXML_TYPE_BACKEND="$PWD/backends/typescript/dist/server.js" wxml-lsp --stdio
```

## Architecture

- `src/project_host.rs`: `ProjectHost`/`ProjectState`; real root, `FileId`,
  page/component views, script/JSON overlays, WXML snapshots, compiler-option
  filtering and `PreparedView` assembly (P3 + P6).
- `src/backend_node/`: `NodeBackend`, `NodeTransport`, NDJSON protocol,
  C1 request/response mapping, cache and epoch handling (P7-2/P7-3).
- `src/server.rs`, `src/handlers/`, `src/workspace.rs`, `src/scheduler.rs`:
  protocol lifecycle, document state, bounded scheduling and publication
  barriers.

The old `run_stdio(backend)` API is preserved and uses `NullHost` (synthetic
documents). `run_stdio_with_host(backend, host)` and `run_with_host(reader,
writer, backend, host)` add real project integration and an in-process entry
point for tests.

## Tests

```bash
cargo fmt -p wxml-lsp -- --check
cargo clippy -p wxml-lsp --all-targets -- -D warnings
cargo test -p wxml-lsp
RUSTDOCFLAGS="-D warnings" cargo doc -p wxml-lsp --no-deps
```

Real-Node tests are ignored by default so `cargo test` works without Node:

```bash
(cd backends/typescript && npm ci && npm run build)

# Full G1 acceptance loop (AC-001/002/003/011/013)
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test stdio_type_loop -- --ignored --nocapture

# Backend smoke test
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test real_node -- --ignored
```

Running either file with `--ignored` is an explicit request for the real gate.
Missing `WXML_TEST_NODE=1` or a missing built backend fails the test instead of
reporting a green skip. Node 24 and the built `backends/typescript/dist/server.js`
are required.

## Limits

- Requests are processed by one backend worker; a slow backend serializes
  hover/diagnostics. Each request has a 15 s write+read deadline and the
  process is killed on timeout, but the worker itself is not preemptible.
- `wx:for` locals are bound as `any` (C1.1); any fragment with locals reports
  `Partial`.
- Definition targets outside the project or without a client URI are dropped.
- Incremental parsing/LS, watchers, multi-root workspaces, completion,
  backspace/reverse edits and debouncing are deferred.

## Security notes

- Backend stdout is read through a bounded `sync_channel(4)` with an 8 MiB
  per-line cap; a flooding Node cannot grow the queue without bound. Responses
  are parsed directly into typed DTOs (no intermediate JSON value tree), and
  diagnostics/fragments are truncated while parsing; a truncated response is
  reported as `Partial`. Insertion overflow is rejected because revisions
  cannot be trusted. Oversized stderr lines are drained with bounded logging.
- Requests are written on a helper thread with the same deadline as the read,
  so a Node process that stops reading stdin cannot block the worker forever.
- Definition results are capped at 32 targets, deduplicated by `(file, range)`,
  and cross-file line indexes are built once per request. A request reads at
  most 8 distinct target files and at most 64 MiB of estimated text + line
  index; targets beyond that budget are skipped rather than cached without
  bound.
- `tsconfig` `baseUrl` values that are absolute or contain `..` are dropped
  (reported as partial compiler options) so they cannot escape the project.
- Node error messages are escaped/truncated before logging; hover documentation
  is Markdown-escaped and code fences grow past embedded backtick runs.
- Three consecutive failures open a cooldown so a broken backend is not
  restarted in a tight loop.
