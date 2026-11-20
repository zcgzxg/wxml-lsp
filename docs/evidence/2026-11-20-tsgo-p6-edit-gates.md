# P6-2 mandatory native edit gates (2026-11-20)

## 0. Identity and boundary

- Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- Slice start HEAD: `2f072a6` (`test(tsgo): promote final native gate`)
- Validation HEAD at batch time: `7f9116a` (`test(delivery): assert native default in no-node guard`).
  Concurrent commits landed while this slice was in progress; they are not part of it.
- Date: 2026-11-20
- Scope: `crates/wxml-lsp/tests/native_parity/{symbols_edits,native_lsp,
  native_gates,harness}.rs`, `crates/wxml-lsp/tests/native_parity.rs`,
  `crates/wxml-lsp/tests/semantic_tools.rs`, new
  `crates/wxml-lsp/tests/common/native_edit.rs`, this document, and the
  source-line anchors of the three silent-skip ledger entries.
- **No production `src/`, Cargo, delivery, default-backend or Node file is
  changed by this slice.** No gate ledger entry is added or removed:
  `ignored_rust_tests` stays **23 / 23**. The two silent-gate declarations and
  the optional `tsc` line moved with the test source, so their frozen anchors
  were updated deliberately: `2081 -> 2086`, `2169 -> 2184`,
  `2568 -> 2589`.
- Concurrent work: this worktree also contains uncommitted default-switch and
  wave-6 ledger changes by another worker. They are not authored here and are
  not treated as this slice's evidence.

## 1. What changed

### 1.1 Three authored native edit gates

`symbols_edits::MIGRATED_CASES` now names three real `#[test]` functions, and
`symbols_edits::BLOCKED_NATIVE_GATES` is empty:

| Gate | Test | What it proves |
| --- | --- | --- |
| `G-P5-E2E-22` | `native_component_property_edit_applies_and_reparses` | On the frozen `component-bindings` (E2E-20) fixture, renaming the `title` property at `title="{{ total }}"` produces exactly three `documentChanges`: one declaration edit in `card.ts`, one `{{ title }}` edit in `card.wxml`, and two `title=` edits in `pages/index/index.wxml`. Every range and `newText` is exact; the applied templates reparse; the applied script/component template publish no error; the applied page publishes exactly the frozen two errors, and the property error now names `heading`, proving the native check used the applied overlay rather than the original disk text. On-disk files are unchanged. |
| `G-P5-E2E-22` (post-edit check) | `assert_native_edit_reparses_and_typechecks` | A standalone gate, not a helper. It constructs exact `count -> total` TypeScript edits, applies them to an immutable snapshot, and requires the native script check to be clean. A positive control then injects a real `string`/`number` mismatch and requires a real error. No `tsc` branch and no Node subprocess exist in the gate. |
| `G-P5-E2E-23` | `native_rejects_cross_namespace_and_dynamic_edits` | Six rejection cases: component property -> component data, component data -> component property, a page data path referenced from another script's nested `setData`, a destructuring-shorthand overlay, a dynamic `bindtap="{{dynamicHandler}}"` binding, and a non-literal `Page(options)` registration handler. Each must be a `-32803` rejection or a `Partial` answer with zero edits; every source file is byte-identical after the session. A positive control first performs a real `alpha -> beta` page-data rename on the same project, so a completely unanalyzable project cannot pass by rejecting everything. |

All three are `#[ignore]` in the default job and require an opt-in native
package. Required mode (`WXML_REQUIRE_REAL_ENGINE=1`) is fail-closed: a missing
package/engine panics before any project setup.

### 1.2 The old Node edit tests no longer fail open

`crates/wxml-lsp/tests/semantic_tools.rs` keeps both Node tests and every
existing assertion. Default behavior is unchanged:

- no native flag: the existing `real_node_paths()` skip message and `return;`
  still run when `dist/server.js` is absent;
- no native flag: the existing optional `tsc` branch still prints
  `tsc not installed; skipped post-edit type check`.

When `WXML_TEST_NATIVE=1` or `WXML_REQUIRE_REAL_ENGINE=1` is set:

- `common::native_edit::require` validates the test-only
  `WXML_TEST_NATIVE_PACKAGE` layout (binary, manifest, pinned engine, typings)
  or the `TSGO_ASSET_ROOT`/`TSGO_BIN` sources, including the pinned engine
  SHA-256, and panics with the exact remediation when anything is missing;
- a missing `dist/server.js` panics instead of returning green;
- a missing `tsc` panics instead of printing the optional skip.

### 1.3 Freeze and counters

- `native_parity` partition manifest: `symbols_edits` is exactly the three
  tests above; the other 15 partitions stay empty. The fail-closed
  `ParityRunner::without_backend` check is unchanged.
- `native_gates` now classifies `#[ignore]` functions as **23 old gates + 23
  native engine gates + 3 native edit gates = 49**. The edit-gate source scan
  rejects `WXML_TEST_NODE`, `dist/server.js`, `Command::new("node")`,
  `tsc.exists()` and the old skip text.
- `semantic_tools_silent_returns_are_mandatory_unmigrated_gates` still checks
  that the old tests are not `#[ignore]`, that the default early return remains,
  and now additionally checks the required-mode `panic!` and the conditional
  `tsc` panic. The replacements must exactly equal `MIGRATED_CASES`.
- The ledger `silent_skip_gates` remain `required_not_migrated`; this slice does
  not promote them, add gate evidence or claim `mandatory_green`.

## 2. Commands and results

Environment for the native runs:

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  WXML_TEST_NATIVE=1 \
  WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
  TSGO_TYPINGS_ROOT=/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/fixtures/miniprogram-api-typings-5.3.1 \
  cargo test -p wxml-lsp --test native_parity symbols_edits:: \
    -- --ignored --test-threads=1
```

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | exit 0 |
| `cargo check -p wxml-lsp --tests` | exit 0 |
| `cargo clippy -p wxml-lsp --all-targets -- -D warnings` | exit 0, no issues |
| `cargo test -p wxml-lsp --test native_parity` (default) | **37 passed, 0 failed, 26 ignored**, exit 0 |
| `cargo test -p wxml-lsp --test semantic_tools` (default) | **41 passed, 0 failed, 0 ignored**, exit 0 |
| Required native edit gates (`symbols_edits::`, `--ignored`) | **3 passed, 0 failed, 60 filtered**, exit 0, 12.83 s |
| `semantic_tools real_node_lsp_` with a valid native package | **2 passed, 0 failed, 39 filtered**, exit 0 |
| Negative: each edit gate with `WXML_TEST_NATIVE_PACKAGE=/tmp/opencode/does-not-exist` | three separate runs, each **0 passed, 1 failed, 62 filtered**, **exit 101**, reason names `WXML_TEST_NATIVE_PACKAGE` and the missing package |
| Negative: `semantic_tools real_node_lsp_` with `WXML_TEST_NATIVE=1` and no package/engine | **0 passed, 2 failed, 39 filtered**, exit 101, panic names the missing native package/engine |

Pinned identity used by the green run:

| Field | Value |
| --- | --- |
| Engine | TypeScript `7.1.0-beta` |
| `lib/tsc` SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| `lib/tsc` size | `24,522,912` bytes |
| Typings payload | in-repo `miniprogram-api-typings-5.3.1` fixture |

The required run is an opt-in `--ignored` gate run. It is **not**
`mandatory_green` and not a default-CI pass.

## 3. Remaining blockers and explicit non-claims

- The three edit gates are authored and green in the required native job, but
  the ledger `silent_skip_gates` stay `required_not_migrated` until the
  required job record, review and `mandatory_green` promotion are done.
- The old `semantic_tools` tests still use the Node backend by default and are
  not deleted. P7 deletion remains blocked on review and the P7 ledger rewrite.
- The required job must provision the pinned package/engine and typings. A
  missing engine or an unpinned engine is a hard failure; optional mode is an
  explicit printed skip, not a verified pass.
- This slice does not change production code, Cargo files, delivery, the
  default backend or the Node path. It does not add or remove a gate ledger
  entry; `ignored_rust_tests` remains 23/23.
- No G5, G6, G7 or P7 closure is claimed. The native package remains
  Linux-x86_64-only in this evidence.
