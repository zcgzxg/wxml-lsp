# P6-2 fifth native gate wave: component property bindings (2026-11-20)

## 0. Identity and boundary

- Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD when the evidence was recorded: `04fa0feff7774356e08f4b0259d3d8d5220df524`
  (`feat(tsgo): diagnose component property bindings`)
- Date: 2026-11-20
- Scope: `crates/wxml-lsp/tests/native_parity/{native_engine_real,native_gates}.rs`,
  `docs/plans/tsgo-test-ledger.json` (gate status and gate evidence only), this
  evidence document and the plan status. No production code, Cargo file, Node
  path, default backend or `#[ignore]` attribute is changed. Nothing is
  committed.
- Case ledger is unchanged: **36 `green` / 267 `planned` / 36
  `retired-with-decision`**. This record only moves gate status.

**Result: 23 `gate_green` / 0 `required_not_migrated` / 23 total.** The last
native gate, `G-P5-E2E-20`, was promoted with a real native stdio test. This is
an opt-in required-native-job result (`--ignored`), not a default-CI pass, not
`mandatory_green`, and not a P6/G5/G6/F3 closure.

---

## 1. Why this gate became writable

`04fa0fe` added the Rust-side component binding pass:

| Change | Behavior |
| --- | --- |
| `src/backend_tsgo/component_bindings.rs` | For each custom-component usage, read the declared `properties` from prepared script facts, query the bound expression's type through the pinned native `type_query`, and publish an error only when the mismatch is provable. Complex expressions, unresolved declarations and structural questions degrade to `Partial`; event attributes are excluded upstream. |
| `src/backend_tsgo/mapping.rs` / `input.rs` | The shadow layout now exposes the exact expression range for mustache attribute values, so a property diagnostic lands on the bound token. |
| `src/backend_tsgo/diagnostics.rs` | Component binding diagnostics are merged after native fragment diagnostics with status merging. |

That is enough for the gate without any further production edit:

| Gate | Native replacement | What the native test proves |
| --- | --- | --- |
| `G-P5-E2E-20` | `native_ac012_component_property_bindings` | The frozen `component-bindings` fixture keeps its two typed hovers: `pageTitle` = `string` + `页面标题` and `total` = `number` + `卡片总数`, both covering exactly the bound token. The page publishes exactly two errors and no status warning: `title="{{ total }}"` reports the `title` property mismatch on the exact `total` range with a message naming `number`, and `bindtap="missingHandler"` reports the missing handler. The two event definitions stay exact: `onCardTap` covers the declaration token in `pages/index/index.ts`, and `missingHandler` is a checked `null`. |

The old gate's shape is unchanged: two hovers, one property error, one event
error and two event definitions. The native gate asserts the same counts at the
LSP surface, so a missing or extra diagnostic fails the run.

### 1.1 How the status class is observed over LSP

The old `Runner` read `CapabilityStatus` from the backend result. LSP has no
status field, so a degraded status is published as exactly one zero-width
warning at `0:0` (`type checking is partial: …` / `type checking is
unsupported: …`). This fixture resolves every binding, so the page must publish
**no** status diagnostic; the gate calls `assert_no_status_diagnostic` before
counting the two errors. A degradation warning would therefore fail the gate
instead of being silently accepted.

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

Result: **23 passed / 0 failed / 36 filtered**, exit 0, 65.84 s. `--release` is
required because `G-P5-PERF-01` rejects a debug build; the same command is used
for all gates so the evidence is one reproducible run. The new test
`native_engine_real::native_ac012_component_property_bindings ... ok` is part of
that run.

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

Result: **0 passed / 23 failed / 36 filtered**, exit 101. Every gate panics with
`WXML_TEST_NATIVE=1 is not set; the native gate job must request it
explicitly` (23 occurrences). The new gate's own panic is
`native gate G-P5-E2E-20 was invoked but WXML_TEST_NATIVE=1 is not set; the
native gate job must request it explicitly`. A missing opt-in is a failure,
never a skip.

---

## 3. Ledger and validator changes

- The `G-P5-E2E-20` `ignored_rust_tests[]` row moves from
  `required_not_migrated` to `gate_green` and gains `replacement_test`,
  `replacement_kind`, `native_prerequisite`, `removal_condition` and
  `gate_evidence`; `proposed_command` is corrected to the real
  `--test native_parity native_engine_real -- --ignored --test-threads=1`
  target.
- `native_gates.rs` freezes the new sets and counts (**23 `gate_green` / 0
  `required_not_migrated` / 23**), links every green gate to exactly one
  `native_engine_real` replacement, and scans the new `#[ignore]` test. The
  frozen required list is now empty, so the validator fails if any gate silently
  reverts to `required_not_migrated`.
- `IGNORED_GATE_TESTS` grows to 23 declared native gate tests; the old-gate
  inventory stays 23, so the `#[ignore]` scan must still find exactly 46
  attributes (23 old gates + 23 native gates).
- Validator result: `cargo test -p wxml-lsp --test native_parity` is
  **36 passed / 0 failed / 23 ignored** in the default job.
- `MIGRATED_CASES`, partition counts and all `cases[]` statuses are untouched.
  The case ledger stays **36 / 267 / 36**.

## 4. Remaining blockers

No P6-2 gate is left. The remaining P6-2/P6 boundaries are unchanged:

| Item | State |
| --- | --- |
| Two `semantic_tools` silent returns + the optional `tsc` branch | still `required_not_migrated` silent-skip gates; replacements stay registered in `symbols_edits::BLOCKED_NATIVE_GATES` |
| Case backlog | 267 `planned` cases stay planned; a green gate does not promote its `TS-*` cases |
| `mandatory_green` | no gate is `mandatory_green`; all 23 native tests remain `#[ignore]` and only the opt-in `--ignored` job runs them |
| Default backend | `--stdio` without `--backend native` still uses Node; Node is not deleted |
| P6/P7, G5, G6, F3 | remain open |

## 5. Verification

| Command | Result |
| --- | --- |
| `cargo test -p wxml-lsp --test native_parity` | **36 passed / 0 failed / 23 ignored** |
| 23-gate required native run (§2) | **23 passed / 0 failed / 36 filtered** |
| Fail-closed RED run (§2) | **0 passed / 23 failed / 36 filtered** |
| `cargo fmt -p wxml-lsp -- --check` | exit 0 |
| `cargo clippy -p wxml-lsp --test native_parity -- -D warnings` | exit 0 |
| `git diff --check` | exit 0 |

## 6. Limitations and non-claims

1. **Not `mandatory_green`.** All twenty-three tests stay `#[ignore]`; the
   default job reports them ignored. Only the opt-in `--ignored` job runs them.
2. **Not default CI, not the default backend.** `--stdio` without
   `--backend native` still uses Node. Node is not deleted.
3. **No case promotion.** The case ledger stays 36 / 267 / 36. A green gate does
   not make its related `TS-*` cases green.
4. **The property check is conservative.** The pinned API has no
   `isTypeAssignableTo`; complex expressions, unresolved component declarations
   and structural array/object questions degrade to `Partial` instead of
   producing an error. The gate proves the frozen fixture, not general AC-012
   parity.
5. **Event attributes are excluded from the property pass.** `bind*`/`catch*`/
   `capture-*`/`mut-bind`/`slot`/`wx:*` stay event semantics; the gate asserts
   the event error/definitions separately.
6. **Linux x86-64 only.** The native path rejects Windows drive/UNC and macOS is
   untested.
7. **Uncommitted slice.** The run happened at `04fa0fe` with this test/ledger
   slice uncommitted; raw stdout is not committed (evidence level E0).
