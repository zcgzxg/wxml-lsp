# P6-2 fourth native gate wave: overlay replay, event hover, symbols and the non-literal class (2026-11-20)

## 0. Identity and boundary

- Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD when the evidence was recorded: `8ccebbfddcbddc20e3f42593880390ee0f3907e8`
  (`docs: index native gate and planning evidence`)
- Clean verification worktree: `/tmp/opencode/p6-wave4-clean` at the same
  commit, with only the three changed `native_parity` test files and the ledger
  copied in; separate `CARGO_TARGET_DIR=/tmp/opencode/p6-wave4-target`.
- Date: 2026-11-20
- Scope: `crates/wxml-lsp/tests/native_parity/{native_engine_real,native_gates,native_lsp}.rs`,
  `docs/plans/tsgo-test-ledger.json` (gate status and gate evidence only), this
  evidence document and the plan status. No production code, Cargo file, Node
  path, default backend or `#[ignore]` attribute is changed. Nothing is
  committed.
- Case ledger is unchanged: **36 `green` / 267 `planned` / 36
  `retired-with-decision`**. This record only moves gate status.

**Result: 22 `gate_green` / 1 `required_not_migrated` / 23 total.** Four gates
were promoted with real native stdio tests: `G-P5-E2E-01/02/05/21`. Only
`G-P5-E2E-20` stays required. This is an opt-in required-native-job result
(`--ignored`), not a default-CI pass, not `mandatory_green`, and not a
P6/G5/G6/F3 closure.

---

## 1. Why these four gates became writable

Three committed slices landed after the 2026-11-20 performance/stability run:

| Commit | Behavior |
| --- | --- |
| `304aa20` | Non-owner overlay replay is content-aware (`RequestIdentity.files_hash` covers path + revision + text) and a fatal query whose engine process is already dead is retried exactly once on a fresh session. `Backend::symbols` answers `DataRoots`/`DataPath`/`EventHandler` from the Rust script overlay without opening a session, including `setData` occurrences and a missing-handler insertion. |
| `3220362` | Event-handler hover from Rust script facts: a single identifier handler resolves to its exact one-line signature plus `JSDoc`, with the WXML token as the hover range; missing/opaque/ambiguous handlers answer `null`. |
| `4fd45bd` | A template owner the shadow deliberately cannot analyze (for example a non-literal `Page(options)` registration) is a static `Unsupported` limit, not `Unavailable`; hover, diagnostics, definition, completion and document symbols all follow that class. |

That is enough for these four gates without a production edit:

| Gate | Native replacement | What the native test proves |
| --- | --- | --- |
| `G-P5-E2E-01` | `native_non_owner_overlay_replay_survives_restart` | Disk models type `name` as `string` and `code` as `number`; one unsaved model overlay is visible on two templates; a second overlay is independent; a same-session text change (`OVERLAY` → `OVERLAY_TWO`) is visible immediately (content-aware `files_hash`); after `SIGKILL` of the engine child the **first** hover already replays `OVERLAY_TWO`, the second overlay still shows `EXTRA`, a new engine PID is used and exactly one engine remains; closing the overlay restores `string` while the other overlay stays. |
| `G-P5-E2E-02` | `native_event_handler_hover_diagnostics_definition_completion` | `onTap` hover carries `onTap(event: { detail: { x: number } })` plus `点击处理 😀` and covers exactly the handler token; `onShorthandTap` carries `onShorthandTap()` with the exact token range; the page publishes exactly one missing-handler error at `missingHandler` with the event name `tap`; `{{ dynamic }}`, empty and `this.invalid` bindings stay silent; `onTap`/`onArrow` definitions cover the exact declaration token, the missing handler is a checked `null`; `onA` completion offers `onTap`/`onArrow` with a whole-token `textEdit`, no data/synthetic labels; the component reports exactly one `missingWidget` error and resolves `onWidgetTap` in `widget.ts`. |
| `G-P5-E2E-05` | `native_symbols_smoke` | `textDocument/references` returns exactly four proven `name` tokens: the `data` declaration, the `this.data.user.name` read, the `"user.name"` `setData` segment and the WXML token. `prepareRename` returns the exact WXML range and `name`; `rename` produces exactly three script edits (declaration, read, `setData` segment) and one WXML edit, all `displayName` and none inside quotes. `textDocument/codeAction` on `onSave` returns one preferred `quickfix` titled `Create event handler 'onSave'`, a zero-width insertion on the Page options `}`, the inserted text contains `onSave()` and no Component `methods` wrapper. |
| `G-P5-E2E-21` | `native_ac012_event_handlers_basic` | The frozen `events-basic` fixture keeps its three typed hovers (`label`, `title`, `firstLabel`) with exact ranges and `JSDoc`; the page reports exactly one `missingHandler` error and stays silent over `{{dynamicHandler}}`, `""` and `this.onTap`; the component reports exactly one `missingWidgetHandler` error with a `Partial` status warning; `pages/multi` fabricates no error and reports `Partial`; `pages/nonliteral` fabricates no error and reports `Unsupported`; all 13 definitions are exact declaration tokens or checked `null`; page and component event completion return the owner methods, exclude reserved names and replace the whole handler token. |

### 1.1 How status classes are observed over LSP

The old `Runner` read `CapabilityStatus` from the backend result and kept it
separate from the diagnostics array. LSP has no status field, so a degraded
status is published as exactly one zero-width warning at `0:0`
(`type checking is partial: …` / `type checking is unsupported: …`). The
`E2E-21` gate asserts that representation: zero errors **and** exactly one
status warning of the frozen class. It is not a relaxed assertion — `Partial`
and `Unsupported` remain distinct, and a missing warning fails the gate.

### 1.2 Test-harness capability

`Session::start` now advertises
`workspace.workspaceEdit.documentChanges: true`. Rename and the missing-handler
quick fix correctly refuse to answer without it, so the previous empty client
capabilities would have produced a false empty result. This is a test-client
capability, not a server behavior change.

---

## 2. Required command and result

Environment:

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  WXML_TEST_NATIVE=1 \
  WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
  TSGO_TYPINGS_ROOT=/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/miniprogram-api-typings \
  cargo test -p wxml-lsp --test native_parity --release native_engine_real \
    -- --ignored --test-threads=1
```

Result: **22 passed / 0 failed / 36 filtered**, exit 0, 57.88 s. `--release` is
required because `G-P5-PERF-01` rejects a debug build; the same command is used
for all gates so the evidence is one reproducible run.

Pinned identity:

| Field | Value |
| --- | --- |
| Engine | TypeScript `7.1.0-beta`, `@typescript/typescript-linux-x64` |
| Source commit | `3e9236361fa97a9e6c685f4833ac457ae2d8c50a` |
| AST protocol | `9` |
| `lib/tsc` SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| `lib/tsc` size | `24,522,912` bytes |
| Standard library | `engine/lib` (`116` `lib*.d.ts` copied into the disposable package) |
| Typings payload | `miniprogram-api-typings` `5.3.1` at `/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/miniprogram-api-typings` (26 files, 20 `.d.ts` mounted) |

Fail-closed RED run (same command without `WXML_TEST_NATIVE=1`):

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE -u WXML_TEST_NATIVE \
  WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
  TSGO_TYPINGS_ROOT=/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/miniprogram-api-typings \
  cargo test -p wxml-lsp --test native_parity --release native_engine_real \
    -- --ignored --test-threads=1
```

Result: **0 passed / 22 failed / 36 filtered**, exit 101. Every gate panics
with `WXML_TEST_NATIVE=1 is not set; the native gate job must request it
explicitly` (22 occurrences). A missing opt-in is a failure, never a skip.

---

## 3. Ledger and validator changes

- Four `ignored_rust_tests[]` rows move from `required_not_migrated` to
  `gate_green` and gain `replacement_test`, `replacement_kind`,
  `native_prerequisite`, `removal_condition` and `gate_evidence`.
- `native_gates.rs` freezes the new sets and counts (**22 `gate_green` / 1
  `required_not_migrated` / 23**), links every green gate to exactly one
  `native_engine_real` replacement, and scans the four new `#[ignore]` tests.
- The four promoted rows also correct `proposed_command` from the nonexistent
  `--test native_engine_real` target to
  `--test native_parity native_engine_real -- --ignored --test-threads=1`.
- Validator result: `cargo test -p wxml-lsp --test native_parity` is
  **36 passed / 0 failed / 22 ignored** in the default job.
- `MIGRATED_CASES`, partition counts and all `cases[]` statuses are untouched.
  The case ledger stays **36 / 267 / 36**.

## 4. Remaining gate and exact blocker

| Gate | Exact blocker |
| --- | --- |
| `G-P5-E2E-20` | Component property binding diagnostics are not implemented. On the `component-bindings` fixture, `title="{{ total }}"` produces no type diagnostic although both hovers (`pageTitle` = `string` + `页面标题`, `total` = `number` + `卡片总数`) and the `onCardTap` definition pass. The design needs a shadow expression range for component attribute bindings plus a `type_query` binding check; complex types stay `Partial` and event attributes are excluded. The plan is `docs/evidence/2026-11-20-tsgo-e2e20-component-binding-plan.md`; the other session owns it and this wave did not touch `src`. |

`G-P5-E2E-20` carries no `replacement_test`, no `replacement_kind` and no
`gate_evidence`; the validator rejects a half-written replacement.

## 5. Verification

| Command | Result |
| --- | --- |
| `cargo test -p wxml-lsp --test native_parity` | **36 passed / 0 failed / 22 ignored** |
| 22-gate required native run (§2) | **22 passed / 0 failed / 36 filtered** |
| Fail-closed RED run (§2) | **0 passed / 22 failed / 36 filtered** |
| `cargo fmt -p wxml-lsp -- --check` | exit 0 |
| `cargo clippy -p wxml-lsp --test native_parity -- -D warnings` | exit 0 |
| `git diff --check` | exit 0 |

The required, RED, fmt and clippy runs above were repeated in
`/tmp/opencode/p6-wave4-clean` so concurrent uncommitted `src/backend_tsgo`
edits from the `E2E-20` session cannot influence the result.

While this wave ran, another session committed `1102da7`
(`build(release): add native v2 release pipeline`) on top of the anchor. A
second clean worktree at `1102da7` with the same four files reproduced the
default validator (**36 passed / 0 failed / 22 ignored**) and the full required
job (**22 passed / 0 failed / 36 filtered**, 62.92 s). The frozen evidence
anchor remains `8ccebbf` because that is the HEAD the slice was written and
verified against.

## 6. Limitations and non-claims

1. **Not `mandatory_green`.** All twenty-two tests stay `#[ignore]`; the default
   job reports them ignored. Only the opt-in `--ignored` job runs them.
2. **Not default CI, not the default backend.** `--stdio` without
   `--backend native` still uses Node. Node is not deleted.
3. **No case promotion.** The case ledger stays 36 / 267 / 36. A green gate does
   not make its related `TS-*` cases green.
4. **E2E-05 is symbol-fact evidence, not checker semantics.** The gate proves
   the four LSP consumers of `Backend::symbols` for the frozen smoke fixture:
   DataPath declaration/read/`setData` segment, EventHandler declaration and
   missing insertion. It does not claim TypeScript checker reference roles,
   cross-file rename, `observerPath` or dynamic registration parity; those stay
   `Partial`/planned. The query itself intentionally does not open a native
   session; the pin/engine identity is still required because the package must
   start under `--backend native`.
5. **E2E-01 replay is engine-crash replay.** The gate kills the engine child
   (`SIGKILL`) and asserts the first request retries once. It does not add a
   public `restart` RPC; `Backend::invalidate()` remains covered by adapter
   tests.
6. **P6/G5/G6/F3 remain open.** Only `G-P5-E2E-20`, the two silent returns and
   the case backlog remain in P6-2.
7. **Linux x86-64 only.** The native path rejects Windows drive/UNC and macOS is
   untested.
8. **Uncommitted slice.** The run happened at `8ccebbf` with this test/ledger
   slice uncommitted; raw stdout is not committed (evidence level E0).
