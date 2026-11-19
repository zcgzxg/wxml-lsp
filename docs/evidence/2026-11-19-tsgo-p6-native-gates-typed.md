# P6-2 native gate promotion after trusted type assets (2026-11-19)

## 0. Identity and boundary

- Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD when the evidence was recorded: `561b00ab1aaa74447ff9fc1feb3f6f3e912cf954`
  (`feat(tsgo): validate installed type assets`)
- Date: 2026-11-19
- Scope: `crates/wxml-lsp/tests/native_parity/{native_engine_real,native_gates,native_lsp}.rs`,
  `docs/plans/tsgo-test-ledger.json` (gate status and gate evidence only), and this
  evidence document. No production code, Cargo file, delivery crate, backend crate,
  default backend or Node path is changed. Nothing is committed.
- Working-tree state: the verification run happened while another session had
  uncommitted changes under `crates/wxml-backend-tsgo/src/{native,query}` and
  `crates/wxml-lsp/src/{backend_tsgo,handlers,semantics}`. Those changes were not
  authored here and are not used as a prerequisite; §5 records a clean-HEAD replay.
- Case ledger is unchanged: **36 `green` / 267 `planned` / 36
  `retired-with-decision`**. This record only moves gate status.

**Result: 13 `gate_green` / 10 `required_not_migrated` / 23 total. Ten new gates
were promoted with real native tests: `G-P5-E2E-04/06/07/08/10/13/15/17/18/19`.
This is an opt-in required-native-job result (`--ignored`), not a default-CI pass,
not `mandatory_green`, and not a P6/G5/G6/F3 closure.**

---

## 1. Why the ten gates became writable

The committed trusted type-assets handoff (`caf045d`, refined by `561b00a`) makes
`wxml-lsp --stdio --backend native` load the pinned standard library and the
miniprogram typings before serving. The opt-in adapter now answers real typed
hover, semantic plus syntactic diagnostics, expression-member completion and
mapped definitions through the existing server dispatch. That is enough for the
ten gates below without any production edit.

| Gate | Old test | Native replacement | What the native test proves |
| --- | --- | --- | --- |
| `G-P5-E2E-04` | `real_node_hover_and_diagnostics_smoke` | `native_hover_and_diagnostics_smoke` | A clean page publishes no diagnostics; `user.name` hover is `string`. |
| `G-P5-E2E-06` | `ac001_page_data_and_jsdoc_hover` | `native_ac001_page_data_and_jsdoc_hover` | Empty diagnostics, `string` + Chinese JSDoc, exact `name`/`user`/`title` ranges, expression-end hover, `null` outside a mustache. |
| `G-P5-E2E-07` | `ac002_field_typo_diagnostic_and_fix_clear` | `native_ac002_field_typo_diagnostic_and_fix_clear` | Exactly one error on `nmae` at the exact range, message contains `does not exist`, `[]` after `didChange`. |
| `G-P5-E2E-08` | `ac003_unicode_and_crlf_positions` | `native_ac003_unicode_and_crlf_positions` | Two errors at UTF-16 characters `13..17` and `12..16` across CJK + emoji + CRLF. |
| `G-P5-E2E-10` | `ac013_component_properties_hover` | `native_ac013_component_properties_hover` | No error, the existing `registration has no \`data\` member` Partial warning, `title: string` hover. |
| `G-P5-E2E-13` | `static_wxs_answers_survive_an_unavailable_type_backend` | `native_static_wxs_answers_survive_an_unavailable_type_backend` | After the pinned engine is removed post-startup, static WXS hover/definition still answer, the unavailable backend is reported, typed hover is `null`, and no sandbox is left behind. |
| `G-P5-E2E-15` | `ac005_loop_scope_typed_locals` | `native_ac005_loop_scope_typed_locals` | Eight loop-scope hovers (`itemIndex`, nested `item`, map `index`) with exact types and JSDoc; only the outside-loop `item` errors; no status warning. |
| `G-P5-E2E-17` | `ac008_alias_project_hover_and_definition` | `native_ac008_alias_project_hover_and_definition` | `extends` + `baseUrl` + `paths` resolve `@models/user`; hover is `string` + `用户显示名称`; definition lands in `models/user.ts` covering `name`; one `nmae` error. |
| `G-P5-E2E-18` | `ac008_multi_view_fixture_isolation` | `native_ac008_multi_view_fixture_isolation` | All seven cross-file hovers keep their own type and JSDoc; the card typo stays the only diagnostic and does not leak into `pages/a`/`pages/b`. |
| `G-P5-E2E-19` | `ac008_simultaneous_views_do_not_pollute_each_other` | `native_ac008_simultaneous_views_do_not_pollute_each_other` | Interleaved A/card/B hovers keep the right documentation; card typo stays local; pages stay clean. |

`G-P5-E2E-13` keeps the old behavioural contract rather than turning the gate into
a startup-failure check: discovery succeeds with the pinned package, then the
engine is removed before the first type query. The started server keeps its
static layer and reports the unavailable type backend, which is the native
equivalent of the old `/nonexistent/wxml-type-backend.js` case.

## 2. Required commands and results

Environment:

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  WXML_TEST_NATIVE=1 \
  WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
  TSGO_TYPINGS_ROOT=/tmp/opencode/assets-lock-root/typings \
  cargo test -p wxml-lsp --test native_parity native_engine_real \
    -- --ignored --test-threads=1
```

Result: **13 passed / 0 failed / 36 filtered**, exit 0. The thirteen tests are
the ten new gates plus the three first-wave gates (`G-P5-E2E-09/12/14`).

Pinned identity:

| Field | Value |
| --- | --- |
| Engine | TypeScript `7.1.0-beta`, `@typescript/typescript-linux-x64` |
| Source commit | `3e9236361fa97a9e6c685f4833ac457ae2d8c50a` |
| AST protocol | `9` |
| `lib/tsc` SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| `lib/tsc` size | `24,522,912` bytes |
| Standard library | `engine/lib` (`116` `lib*.d.ts` copied into the disposable package) |
| Typings payload | `miniprogram-api-typings` `5.3.1` at `/tmp/opencode/assets-lock-root/typings` (26 files, 20 `.d.ts` mounted) |

Fail-closed RED run (same command without `WXML_TEST_NATIVE=1`):

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE -u WXML_TEST_NATIVE \
  WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
  TSGO_TYPINGS_ROOT=/tmp/opencode/assets-lock-root/typings \
  cargo test -p wxml-lsp --test native_parity native_engine_real \
    -- --ignored --test-threads=1
```

Result: **0 passed / 13 failed / 36 filtered**, exit 101. Every gate panics with
`WXML_TEST_NATIVE=1 is not set; the native gate job must request it explicitly`.
A missing opt-in is therefore a failure, never a skip.

## 3. Ledger and validator changes

- Ten `ignored_rust_tests[]` rows move from `required_not_migrated` to
  `gate_green` and gain `replacement_test`, `replacement_kind`,
  `native_prerequisite`, `removal_condition` and `gate_evidence`.
- The three first-wave rows keep their status; their evidence gains explicit
  `passed`/`filtered`/`fail_closed.failed` counters.
- `native_gates.rs` freezes the new sets and counts
  (`13 gate_green`, `10 required_not_migrated`, `23` total), links every green
  gate to exactly one `native_engine_real` replacement, and derives the expected
  `N passed` / `N failed` / `N filtered` counts from the evidence instead of the
  old hard-coded `3`.
- Validator result: `cargo test -p wxml-lsp --test native_parity` is
  **36 passed / 0 failed / 13 ignored** in the default job.
- `MIGRATED_CASES`, partition counts and all `cases[]` statuses are untouched.
  The case ledger stays **36 / 267 / 36**; no case is promoted by a gate.

## 4. Remaining ten gates and exact blockers

| Gate | Exact blocker |
| --- | --- |
| `G-P5-PERF-01` | No native release baseline exists. The old test constructs `NodeBackend`/`NodeTransport`; there is no reproducible native p50/p95/p99, CPU/RSS or growth dataset to freeze. |
| `G-P5-STABILITY-01` | No native restart/epoch accounting or RSS baseline; the old test uses the Node backend and `/proc`. |
| `G-P5-E2E-01` | Non-owner script overlay propagation needs native restart/epoch replay; the adapter has one lazy session and drops it on invalidation but does not replay a new epoch. |
| `G-P5-E2E-02` | Event-handler completion is native, but event-handler definition/diagnostics parity is not landed in committed code. The in-progress `semantics/event_definition.rs` is uncommitted and is not evidence. |
| `G-P5-E2E-03` | Native expression-member and event-handler completion pass, but root completion (`{{  }}`) and trailing-dot completion (`{{ user. }}`) return empty; the old test asserts both. |
| `G-P5-E2E-05` | `Backend::symbols()` still returns `SymbolResult::unsupported`; `DataPath`/`EventHandler` symbol facts are not wired. |
| `G-P5-E2E-11` | Behavior `sharedCount` hover/definition pass, but `optionalTypes: [Number]` renders `string`, not `string \| number`, and the diagnostics result is Partial (`behavior graph resolution is not implemented`) instead of empty. |
| `G-P5-E2E-16` | The two guard diagnostics are correct, but the `maybeUser` error range is reported as cropped at generated text, so the diagnostics status is `Partial` while the old gate requires `Supported`. |
| `G-P5-E2E-20` | Component binding property diagnostics need full `TypeQuality`/`CapabilityStatus` parity plus event-handler definitions; the fixture also asserts event definitions. |
| `G-P5-E2E-21` | AC-012 event handlers need committed event-handler definition plus exact missing-handler diagnostics. |

These rows carry no `replacement_test`, no `replacement_kind` and no
`gate_evidence`; the validator rejects a half-written replacement.

## 5. Verification

| Command | Result |
| --- | --- |
| `cargo test -p wxml-lsp --test native_parity` | **36 passed / 0 failed / 13 ignored** |
| 13-gate required native run (§2) | **13 passed / 0 failed / 36 filtered** |
| Fail-closed RED run (§2) | **0 passed / 13 failed / 36 filtered** |
| clean HEAD `561b00a` replay in `/tmp/opencode/p6-clean-561b00a` (same test files, same assets) | recorded in §5.1 |
| `cargo clippy -p wxml-lsp --test native_parity -- -D warnings` in the clean worktree | exit 0 |
| `rustfmt --check` on the three changed test files | exit 0 |
| `git diff --check` | exit 0 |

### 5.1 Clean-HEAD replay

The authoritative run uses a detached worktree at `561b00a` with only the changed
native-parity test files and the ledger copied in, a separate
`CARGO_TARGET_DIR`, and the same pinned engine/assets. This separates the slice
from the concurrent uncommitted backend work.

| Command | Result |
| --- | --- |
| 13-gate required native run in the clean worktree | **13 passed / 0 failed / 36 filtered**, exit 0, 47.07 s |
| Fail-closed RED run in the clean worktree | **0 passed / 13 failed / 36 filtered**, exit 101 |
| `cargo test -p wxml-lsp --test native_parity` in the clean worktree | **36 passed / 0 failed / 13 ignored** |
| `cargo clippy -p wxml-lsp --test native_parity -- -D warnings` in the clean worktree | exit 0 |

The clean worktree contains no uncommitted production change: the only copied
files are `native_engine_real.rs`, `native_gates.rs`, `native_lsp.rs` and
`docs/plans/tsgo-test-ledger.json`. The ten new gates therefore depend only on
committed `561b00a` behavior plus this test/ledger slice.

## 6. Limitations and non-claims

1. **Not `mandatory_green`.** All thirteen tests stay `#[ignore]`; the default
   job reports them ignored. Only the opt-in `--ignored` job runs them.
2. **Not default CI, not the default backend.** `--stdio` without
   `--backend native` still uses Node. Node is not deleted.
3. **No case promotion.** The case ledger stays 36 / 267 / 36. A green gate does
   not make its related `TS-*` cases green.
4. **P6/G5/G6/F3 remain open.** No perf, stability, restart/epoch, native symbol
   or cross-platform claim is made.
5. **Linux x86-64 only.** The native path rejects Windows drive/UNC and macOS is
   untested.
6. **Dirty-worktree caveat.** The primary run happened while another session had
   uncommitted backend changes; §5.1 is the clean-HEAD control. Raw stdout is not
   committed (evidence level E0).
