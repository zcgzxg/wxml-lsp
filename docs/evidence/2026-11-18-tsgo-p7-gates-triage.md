# P7 mandatory test gates triage (2026-11-18)

Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`  
HEAD at triage start: `8490a92` (`test(delivery): verify real native asset installation`)  
Triage date: 2026-11-19 (filename keeps the requested 2026-11-18 label)  
Scope: ignored-test inventory, individual execution evidence, gate classification,
`semantic_tools` fail-open analysis. **This triage changed no production code and
no test attribute. P7 is not done and G7 is not claimed.**

## 0. Verdict

- The workspace contains exactly **23 `#[ignore]` tests**, all under
  `crates/wxml-lsp/tests/**`; zero outside `tests/`.
- All 23 execute and pass individually once a worktree-local
  `backends/typescript/dist/server.js` exists and `WXML_TEST_NODE=1` is set.
- **0 `enable-now`**: every gate requires `WXML_TEST_NODE=1` and the Node 24
  `dist/server.js`; the P3-T freeze in `native_gates.rs` also asserts exactly 23
  ignores with `status=required_not_migrated`, so removing one attribute without a
  deliberate ledger rewrite would fail the ordinary suite.
- **11 `keep-gated-native`** (9 `stdio_type_loop.rs` public-boundary tests plus the
  performance and stability gates; each must stay opt-in and be retargeted to the
  native backend).
- **12 `delete-at-P7`** (5 `real_node.rs` + 7 `wave5_acceptance.rs`; Node-backend
  acceptance tests replaced by the ledger's native parity gates).
- **0 `blocked` at run level**: no missing fixture or runtime feature prevented
  execution. All 23 are nevertheless **P7-blocked on native migration**.
- `semantic_tools` has **two fail-open early returns** and **one optional `tsc`
  branch**; both were empirically re-confirmed to pass when the engine is absent.
  Existing tests freeze the source/ledger shape but do not fail the fail-open path.

## 1. Method and environment

- Node: `v24.22.2`; rustc: `1.100.0`; cargo: `1.100.0`; Linux.
- Built the worktree-local Node backend once (gitignored build output):
  `(cd backends/typescript && npm run build)` → `backends/typescript/dist/server.js`.
- The primary run of every gate was in a **separate process**, one test at a time,
  with `--ignored --nocapture --exact`; no parallel batch.
- Environment was normalized with
  `env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR WXML_TEST_NODE=1`. This matters:
  the developer shell exported `WXML_TYPE_BACKEND` and `WXML_BACKEND_DIR` pointing
  at the main checkout, and `wave5_acceptance` intentionally rejects a backend
  outside its own worktree.
- Representative command:

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR WXML_TEST_NODE=1 \
  cargo test -p wxml-lsp --test <binary> <test_name> \
  -- --ignored --nocapture --exact
```

- Performance gate additionally used:
  `WXML_PERF_LABEL=triage WXML_PERF_OUT=/tmp/opencode/p7-gates/q2-perf-triage.json`.
  Default `WXML_PERF_SAMPLES=200` was left unchanged.
- Raw logs: `/tmp/opencode/p7-gates/` (not in the repository).
- Baseline gate ledger check after the runs:
  `cargo test -p wxml-lsp --test native_parity` → **25 passed; 0 failed**, including
  `native_gates::all_ignored_rust_tests_are_mandatory_unmigrated_gates` and
  `native_gates::semantic_tools_silent_returns_are_mandatory_unmigrated_gates`.

Concurrency note: starting at 08:39 local time, another worker began modifying
`crates/wxml-backend-tsgo/**`, `crates/wxml-lsp/src/backend_tsgo/**`,
`crates/wxml-delivery/tests/{no_node_guard,native_package}.rs`,
`docs/evidence/2026-11-18-tsgo-f3-clean-env.md`, and `scripts/verify-no-node.sh`.
Those changes are outside this triage and are not authored here. The 23 gate runs
completed 08:37:13–08:42:00 and used the pre-existing HEAD build. A later no-`dist`
probe briefly hit that worker's mid-edit `E0408` compile error; it was re-run with
the pre-edit test binaries directly, so the precondition evidence below is valid.

## 2. Inventory and individual execution

Fully-qualified name below is `<file>::<fn>`; the cargo filter is
`-p wxml-lsp --test <file stem> <fn>`. No ignored test exists outside
`crates/wxml-lsp/tests/`.

Deps legend: **N** = Node 24 runtime; **D** = built
`backends/typescript/dist/server.js`; **E** = `WXML_TEST_NODE=1`; **F** = external
fixture under `tests/fixtures/`; **L** = Linux-only (`cfg(target_os = "linux")`);
**T** = timing/performance sensitive; **W** = worktree-local backend enforced.
All rows also depend on **N, D, E**. **Real native engine: none yet**; all 23
currently require Node, and the ledger targets native replacements.

| # | Gate | Test (`#[ignore]` line → `fn` line) | Existing ignore reason | Extra deps | Run 1 | Class |
|---:|---|---|---|---|---|---|
| 1 | G-P5-PERF-01 | `crates/wxml-lsp/tests/performance_bench.rs:781` → `782 q2_performance_benchmark` | `performance gate: set WXML_TEST_NODE=1 and run with --ignored` | L,T; direct `NodeBackend`/`NodeTransport` | PASS 215.00 s | keep-gated-native |
| 2 | G-P5-STABILITY-01 | `crates/wxml-lsp/tests/q2_stability.rs:202` → `203 ten_round_lifecycle_stability` | `stability gate: set WXML_TEST_NODE=1 and run with --ignored` | L,T; `/proc`, direct `NodeBackend` | PASS 21.04 s | keep-gated-native |
| 3 | G-P5-E2E-04 | `crates/wxml-lsp/tests/real_node.rs:30` → `31 real_node_hover_and_diagnostics_smoke` | `requires Node 24 and a built P5 dist/server.js; set WXML_TEST_NODE=1` | direct `NodeBackend` | PASS 1.28 s | delete-at-P7 |
| 4 | G-P5-E2E-03 | `crates/wxml-lsp/tests/real_node.rs:103` → `105 real_node_expression_and_event_completion_smoke` | same | direct `NodeBackend` | PASS 1.33 s | delete-at-P7 |
| 5 | G-P5-E2E-02 | `crates/wxml-lsp/tests/real_node.rs:219` → `221 real_node_event_handler_diagnostics_definition_and_completion` | same | direct `NodeBackend` | PASS 1.31 s | delete-at-P7 |
| 6 | G-P5-E2E-01 | `crates/wxml-lsp/tests/real_node.rs:494` → `496 non_owner_script_overlay_propagates_and_survives_a_node_restart` | same | direct `NodeBackend`, restart timing | PASS 2.58 s | delete-at-P7 |
| 7 | G-P5-E2E-05 | `crates/wxml-lsp/tests/real_node.rs:626` → `627 real_node_symbols_smoke` | same | direct `NodeBackend` | PASS 1.28 s | delete-at-P7 |
| 8 | G-P5-E2E-06 | `crates/wxml-lsp/tests/stdio_type_loop.rs:499` → `500 ac001_page_data_and_jsdoc_hover` | `requires WXML_TEST_NODE=1 and a built backends/typescript/dist/server.js` | public `wxml-lsp --stdio` | PASS 1.27 s (3/3 runs) | keep-gated-native |
| 9 | G-P5-E2E-07 | `crates/wxml-lsp/tests/stdio_type_loop.rs:601` → `602 ac002_field_typo_diagnostic_and_fix_clear` | same | public `wxml-lsp --stdio` | PASS 1.44 s (3/3) | keep-gated-native |
| 10 | G-P5-E2E-08 | `crates/wxml-lsp/tests/stdio_type_loop.rs:653` → `654 ac003_unicode_and_crlf_positions` | same | public `wxml-lsp --stdio` | PASS 1.29 s (3/3) | keep-gated-native |
| 11 | G-P5-E2E-10 | `crates/wxml-lsp/tests/stdio_type_loop.rs:701` → `702 ac013_component_properties_hover` | same | public `wxml-lsp --stdio` | PASS 1.31 s (3/3) | keep-gated-native |
| 12 | G-P5-E2E-09 | `crates/wxml-lsp/tests/stdio_type_loop.rs:775` → `776 ac011_unknown_owner_degrades_honestly` | same | public `wxml-lsp --stdio` | PASS 0.01 s (3/3) | keep-gated-native |
| 13 | G-P5-E2E-12 | `crates/wxml-lsp/tests/stdio_type_loop.rs:831` → `832 static_wxs_and_template_semantics_work_over_stdio` | `requires the real server binary and Node fixture; set WXML_TEST_NODE=1` | public `wxml-lsp --stdio` | PASS 1.70 s (3/3) | keep-gated-native |
| 14 | G-P5-E2E-11 | `crates/wxml-lsp/tests/stdio_type_loop.rs:942` → `943 component_behavior_data_and_properties_work_over_stdio` | same | public `wxml-lsp --stdio` | PASS 1.33 s (3/3) | keep-gated-native |
| 15 | G-P5-E2E-14 | `crates/wxml-lsp/tests/stdio_type_loop.rs:1008` → `1009 unsaved_wxs_overlay_updates_static_hover_and_completion` | same | public `wxml-lsp --stdio` | PASS 1.29 s (3/3) | keep-gated-native |
| 16 | G-P5-E2E-13 | `crates/wxml-lsp/tests/stdio_type_loop.rs:1078` → `1079 static_wxs_answers_survive_an_unavailable_type_backend` | `requires the real server binary; set WXML_TEST_NODE=1` | public `wxml-lsp --stdio`; unavailable backend | PASS 0.05 s (3/3; also 0.20 s with `node` absent from `PATH`) | keep-gated-native |
| 17 | G-P5-E2E-15 | `crates/wxml-lsp/tests/wave5_acceptance.rs:670` → `671 ac005_loop_scope_typed_locals` | `requires Node 24 and a built dist/server.js; set WXML_TEST_NODE=1` | F `loop-scope`; W; direct `NodeBackend` | PASS 1.30 s | delete-at-P7 |
| 18 | G-P5-E2E-16 | `crates/wxml-lsp/tests/wave5_acceptance.rs:723` → `724 ac006_conditional_union_narrowing` | same | F `conditional-union`; W; direct `NodeBackend` | PASS 1.25 s | delete-at-P7 |
| 19 | G-P5-E2E-20 | `crates/wxml-lsp/tests/wave5_acceptance.rs:771` → `772 ac012_component_property_bindings` | same | F `component-bindings`; W; direct `NodeBackend` | PASS 1.21 s | delete-at-P7 |
| 20 | G-P5-E2E-21 | `crates/wxml-lsp/tests/wave5_acceptance.rs:845` → `847 ac012_event_handlers_basic` | same | F `events-basic`; W; direct `NodeBackend` | PASS 1.45 s | delete-at-P7 |
| 21 | G-P5-E2E-18 | `crates/wxml-lsp/tests/wave5_acceptance.rs:995` → `996 ac008_multi_view_fixture_isolation` | same | F `multi-view`; W; direct `NodeBackend` | PASS 1.53 s | delete-at-P7 |
| 22 | G-P5-E2E-19 | `crates/wxml-lsp/tests/wave5_acceptance.rs:1029` → `1030 ac008_simultaneous_views_do_not_pollute_each_other` | same | F `multi-view`; W; direct `NodeBackend` | PASS 1.42 s | delete-at-P7 |
| 23 | G-P5-E2E-17 | `crates/wxml-lsp/tests/wave5_acceptance.rs:1112` → `1113 ac008_alias_project_hover_and_definition` | same | F `alias-project`; W; direct `NodeBackend` | PASS 1.21 s | delete-at-P7 |

Run 1 totals: **23 passed, 0 failed, 0 skipped, 0 not run**. Sum of harness time:
≈ 262.9 s, dominated by the performance gate (215.00 s). The 9 `stdio_type_loop`
gates were additionally run two more times from the pre-edit test binary
(`target/debug/build/wxml-lsp/ed5e66f5267ad502/out/stdio_type_loop-*`, copied to
`/tmp/opencode/p7-gates/bin/`): 9/9 pass in round 2 and 9/9 in round 3.

### 2.1 Gate-request precondition failures (fail-closed evidence)

These probes intentionally omit the opt-in precondition; they are **expected
failures**, not test defects.

| Binary | Probe | Exit | Smallest exact reason |
|---|---|---:|---|
| `real_node` | no `WXML_TEST_NODE` | 101 | `set WXML_TEST_NODE=1 to run the real Node smoke test` |
| `stdio_type_loop` | no `WXML_TEST_NODE` | 101 | `set WXML_TEST_NODE=1 to run the real Node G1 gate` |
| `wave5_acceptance` | no `WXML_TEST_NODE` | 101 | `set WXML_TEST_NODE=1 to run the Wave 5 acceptance gate` |
| `performance_bench` | no `WXML_TEST_NODE` | 101 | `set WXML_TEST_NODE=1 to run the performance gate` |
| `q2_stability` | no `WXML_TEST_NODE` | 101 | `set WXML_TEST_NODE=1 to run the stability gate` |
| `real_node` | `WXML_TEST_NODE=1`, no `dist` | 101 | `WXML_TEST_NODE=1 but .../dist/server.js is missing; run \`npm ci && npm run build\` in backends/typescript` |
| `stdio_type_loop` | `WXML_TEST_NODE=1`, no `dist` | 101 | `WXML_TEST_NODE=1 but backend .../dist/server.js is missing; run \`npm ci && npm run build\` in backends/typescript` |
| `wave5_acceptance` | `WXML_TEST_NODE=1`, no `dist` | 101 | `WXML_TEST_NODE=1 but .../dist/server.js is missing; run \`npm ci && npm run build\` in backends/typescript` |
| `performance_bench` | `WXML_TEST_NODE=1`, no `dist` | 101 | `build the Node backend first: .../backends/typescript/dist/server.js` |
| `q2_stability` | `WXML_TEST_NODE=1`, no `dist` | 101 | `build the Node backend first` |

`wave5_acceptance` additionally fails **by design** when the shell exports an
external backend:

```text
WXML_TYPE_BACKEND=/home/zxg/code/rust/wxml-lsp/backends/typescript/dist/server.js
is outside this worktree (.../tsgo-migration); unset it so the gate tests this checkout
```

Observed on the first wave5 attempt: 7/7 failed in ~0.10 s; after
`env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR`, 7/7 passed. This is an environment
hazard for CI and developer shells, not a test regression.

## 3. Classification

Classification is by **disposition of the current test body at P7**:

- `keep-gated-native`: the test drives the public `wxml-lsp --stdio` boundary or is
  a product quality gate (performance/stability); it must survive as an opt-in
  native gate. Keep it `#[ignore]` until the native engine is the default and the
  `WXML_TEST_NODE`/`dist` guard is replaced.
- `delete-at-P7`: the test's purpose is Node-backend acceptance (it constructs
  `NodeBackend` directly); delete the old test at P7 after the ledger's native
  parity gate lands. This does **not** drop the behavior requirement.

Performance/stability also construct `NodeBackend`, but their purpose is the
product budget/recovery gate, not Node behavior; the ledger targets
`native_perf_gate`/`native_stability`, so they are treated as gates to retarget,
not as Node acceptance tests to delete.

Counts: `enable-now` **0**, `keep-gated-native` **11**, `delete-at-P7` **12**,
`blocked` **0**.

Why no `enable-now`:

1. Every gate asserts `WXML_TEST_NODE=1` and a built
   `backends/typescript/dist/server.js`; none is runnable in the normal no-env
   suite, and the no-Node target explicitly cannot provide `dist`.
2. `native_parity/native_gates.rs:131-190` asserts exactly 23 ignores, 23 unique
   ledger gates, and `status=required_not_migrated` for each. Removing one
   `#[ignore]` without a deliberate ledger/gate rewrite fails the ordinary suite;
   doing that rewrite in this slice would weaken the P7 freeze.
3. The 9 public-boundary tests passed three targeted runs each (one cargo run plus
   two direct-binary runs), but they still fail the "no Node/native env" criterion. `static_wxs_answers_survive_an_unavailable_type_backend`
   is the nearest candidate (it also passes with `node` removed from `PATH`), but it
   remains coupled to `backend_paths()` (`WXML_TEST_NODE` + `dist`) and to the frozen
   23-gate ledger, so it was left unchanged.
4. Performance/stability were run once each (the 215 s / 21 s gates); no attribute
   was removed, so the 3-run enablement rule was not invoked.

No fixture was missing: `loop-scope`, `conditional-union`, `component-bindings`,
`events-basic`, `multi-view`, and `alias-project` all exist under
`tests/fixtures/`. Hence `blocked` is 0 at run level.

## 4. `semantic_tools.rs` fail-open behavior

### 4.1 Exact behavior

- `real_node_paths()` (`crates/wxml-lsp/tests/semantic_tools.rs:2069-2077`) returns
  `Option<(PathBuf, PathBuf)>`; it returns `None` when
  `backends/typescript/dist/server.js` does not exist.
- `real_node_lsp_component_property_edit_applies_and_parses()`
  (`:2079-2085`) does:

```rust
let Some((server, backend_dir)) = real_node_paths() else {
    eprintln!("skipping real Node LSP test: build backends/typescript first");
    return;
};
```

  The test is **not** `#[ignore]`; the ordinary suite records it as passed without
  exercising rename, applied edits, reparse, or typecheck.
- `real_node_lsp_rejects_cross_namespace_and_dynamic_edits()` (`:2566-2572`) has the
  same `Option`/`return` shape. It can pass without exercising the four rejection
  cases or the `-32803` / no-edit / unchanged-source assertions.
- Post-edit check (`:2168-2195`): if
  `backend_dir/node_modules/typescript/bin/tsc` is absent, it prints
  `tsc not installed; skipped post-edit type check` and continues. Even with the
  Node backend present, the mandatory typecheck is optional, and it shells out to
  `node` when present.

With `dist` and `tsc` present, both tests also pass normally (`2.37 s` and
`1.81 s` harness time), so the happy path is green. Empirical fail-open proof
(worktree `dist` temporarily moved aside; restored by trap):

| Test | Engine | Exit | Observed |
|---|---|---:|---|
| `real_node_lsp_component_property_edit_applies_and_parses` | no `dist` | 0 | `skipping real Node LSP test: build backends/typescript first`; 1 passed |
| `real_node_lsp_rejects_cross_namespace_and_dynamic_edits` | no `dist` | 0 | same skip; 1 passed |
| `real_node_lsp_component_property_edit_applies_and_parses` | `dist` present, `tsc` hidden | 0 | `tsc not installed; skipped post-edit type check`; 1 passed |

### 4.2 What must fail closed in final no-Node mode

1. Missing native engine/asset must **panic/assert**, never `return` green. Replace
   `Option`/early-return with a required native-engine precondition.
2. Both edit tests must be replaced by the native gates recorded in the ledger:
   `native_component_property_edit_applies_and_reparses` and
   `native_rejects_cross_namespace_and_dynamic_edits`, or changed to panic when the
   engine is missing, before the old tests are deleted.
3. The post-edit reparse/typecheck must be unconditional through the native engine:
   `assert_native_edit_reparses_and_typechecks`. No `tsc.exists()`, no `node`
   subprocess, no "skipped" branch. Missing native type information is a failure.
4. The ledger already records `old_test_must_fail_when_engine_missing: true` for all
   three entries; the current behavior violates that requirement.

### 4.3 Existing tests covering it

- `crates/wxml-lsp/tests/native_parity/native_gates.rs:193`
  `semantic_tools_silent_returns_are_mandatory_unmigrated_gates`:
  freezes the two silent returns and the `tsc` branch, asserts
  `old_test_must_fail_when_engine_missing`, asserts the old tests are not
  `#[ignore]`, asserts the exact skip text and `return;`, and asserts the `tsc` else
  branch contains no `panic!`. It checks **source/ledger shape**, not runtime
  behavior.
- `crates/wxml-lsp/tests/native_parity.rs:154`
  `partition_skeleton_is_empty_and_fails_closed`: asserts all 16 `MIGRATED_CASES`
  lists are empty and `ParityRunner::without_backend` yields `Unavailable`, never a
  pass.
- `crates/wxml-lsp/tests/native_parity/symbols_edits.rs:18-34`
  `BLOCKED_NATIVE_GATES`: registers the three mandatory native replacements; the
  gate test above enforces their presence and non-empty reasons.
- `crates/wxml-lsp/tests/native_parity/ledger.rs` test
  `ledger_old_test_sources_and_lines_exist` verifies the frozen source/line mapping.

There is **no negative test** that makes the two `semantic_tools` tests fail when
the engine is absent. The current "coverage" is a freeze that *permits* the
fail-open path, not a behavioral gate.

## 5. Exact remaining P7 blockers

P7 is blocked; do not claim G7. In dependency order:

1. **23 `#[ignore]` gates are not native.** `native_gates.rs:131-190` freezes the
   count and `status=required_not_migrated`; `docs/plans/tsgo-test-ledger.json`
   targets new native commands. Split: 12 Node-backend acceptance tests to
   delete/replace, 11 gates to retarget (9 stdio + performance + stability).
2. **2 silent returns + 1 optional `tsc` branch** at
   `crates/wxml-lsp/tests/semantic_tools.rs:2082-2085`, `2169-2195`, `2569-2572`
   still pass without an engine. Required native replacements are the 3
   `BLOCKED_NATIVE_GATES` in
   `crates/wxml-lsp/tests/native_parity/symbols_edits.rs:18-34`;
   `MIGRATED_CASES` there is still empty (`:15`).
3. **Native backend is not selectable from the product entry point (at HEAD
   `8490a92`).** `crates/wxml-lsp/src/main.rs:96` hardcodes `NodeBackend::new`;
   `crates/wxml-lsp/src/lib.rs:46` exposes `backend_tsgo` only as `pub(crate)`;
   `crates/wxml-lsp/src/backend_tsgo/mod.rs:106` keeps
   `TsgoBackendAdapter` `pub(crate)`. There is no CLI/config path to select it, so
   the 9 stdio E2E tests cannot run no-Node.
4. **Native VFS lacks standard-library / wx typings (at HEAD `8490a92`).**
   `crates/wxml-backend-tsgo/src/native/vfs.rs:688` synthesizes
   `"noLib": true`; per `docs/evidence/2026-11-18-tsgo-p6-cutover-audit.md:359-362`
   the native engine has no stdlib/`wx` types, so AC-001..AC-013 type behavior
   cannot be reproduced natively yet.
5. **Parity ledger/fixture freeze must be updated deliberately.** `native_parity`
   reads `backends/typescript/test/**` and frozen hashes; deleting the TS tests or
   Node backend at P7 before the ledger rewrite fails
   `fixture_hash::fixture_tree_hashes_match_frozen_contract` and the ignored-gate
   tests. Current `native_parity` baseline is 25/25 green.
6. **No no-Node gate run exists.** All 23 require `WXML_TEST_NODE=1` and a built
   `dist/server.js`; removing `dist` produces 101, not a native pass. G7 requires a
   clean-PATH/no-Node execution of the native replacements.
7. **Environment hazard.** A globally exported `WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`
   outside the worktree makes `wave5_acceptance` fail by design. CI must unset or
   isolate these variables, or the gate will report false red.
8. **Platform scope.** `performance_bench.rs:1` and `q2_stability.rs:1` are
   `#![cfg(target_os = "linux")]`; P7 must either keep Linux-only claims or add
   equivalent native gates for other supported platforms.

## 6. Changes made by this triage

- Test attributes: **none**. No `#[ignore]` was removed or added; no assertion was
  weakened; no production code was edited.
- Added: this evidence file only.
- Build output created for execution: `backends/typescript/dist/**` (gitignored,
  `backends/typescript/.gitignore:2`).
- The worktree also contains concurrent uncommitted changes by another worker
  (`crates/wxml-backend-tsgo/**`, `crates/wxml-lsp/src/backend_tsgo/**`,
  `crates/wxml-delivery/tests/{no_node_guard,native_package}.rs`, F3
  evidence/script). They are outside this triage and must not be attributed to it.
