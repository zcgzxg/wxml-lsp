# P6-2 mandatory-gate conversion plan (2026-11-18 label; plan date 2026-11-19)

Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`  
HEAD inspected: `f6f175f` (`docs(evidence): plan document symbols DTO`)  
Concurrent HEAD while writing: `3fcfe18` (another session added `98e5c0f`
typings-payload decision and `3fcfe18` native upgrade/rollback test). This plan
is anchored to `f6f175f` and does not treat those commits as evidence.  
Plan nature: **read-only**. This document is the only file written. No test,
Cargo, contract, delivery, backend or Node file is changed by this plan; nothing
is committed.  
**2026-11-19 status update (append-only):** the trusted type-assets handoff made
ten more gates writable. The current gate ledger is **13 `gate_green` / 10
`required_not_migrated` / 23 total**; §2.1/§2.2 remain the original 2026-11-18
dispositions and are not rewritten. Current evidence and per-gate blockers are in
`docs/evidence/2026-11-19-tsgo-p6-native-gates-typed.md`.

**2026-11-20 edit-gate status update (append-only):** the three mandatory native
edit gates are now authored in
`crates/wxml-lsp/tests/native_parity/symbols_edits.rs`;
`MIGRATED_CASES` is exactly those three and `BLOCKED_NATIVE_GATES` is empty. The
old `semantic_tools` tests keep the default Node behavior and panic when a native
edit gate is requested without the package/engine or the post-edit typecheck. The
ledger `silent_skip_gates` remain `required_not_migrated`; 23/23 ignored gates are
unchanged. Evidence:
`docs/evidence/2026-11-20-tsgo-p6-edit-gates.md`.

Worktree concurrency: during this read-only inspection another session modified
`crates/wxml-backend-tsgo/**`, `crates/wxml-delivery/**`, `crates/wxml-lsp/**`
and added new tests. Those changes are not authored by this plan and are not
used as evidence; only this file is new from this plan.  
P6-4a status: the working tree contains an **uncommitted** P6-4a slice
(`crates/wxml-lsp/src/backend_tsgo/{engine,sandbox}.rs`, selector changes in
`main.rs`/`lib.rs`, `tests/cli.rs`). It is treated as WIP and **not** as evidence
of a landed capability. All committed-state claims below are anchored to
`f6f175f`; WIP anchors are marked `WIP`.

Line-number policy: files untouched by the WIP (`semantic_tools.rs`,
`native_parity/**`, `stdio_type_loop.rs`, `real_node.rs`,
`wave5_acceptance.rs`, `performance_bench.rs`, `q2_stability.rs`) use the same
lines at `f6f175f` and in the worktree. WIP-modified files (`main.rs`, `lib.rs`,
`backend_tsgo/mod.rs`, `backend_tsgo/input.rs`, `native/vfs.rs`) are cited at
**current working-tree** lines so Ctrl+click lands on the code; where the
`f6f175f` line differs, it is named in parentheses.

## 0. Verdict

- **0 of the 23 gates are green today.** No native backend is selectable at
  `f6f175f`: `main.rs:96` constructs `NodeBackend` and `lib.rs:46` keeps
  `backend_tsgo` private. The current WIP still keeps the default path Node
  (`main.rs:181-190`, `lib.rs:47`) and the adapter is inert. P6-4a is in
  progress, not landed.
- **3 tests are first-wave `enable-now-native`**: they need only the P6-4a
  selector/package, not P6-3 type assets. They cannot be removed from
  `#[ignore]` until the native gate job is required and a verified package is
  provisioned; their native replacements can be written and made green first.
- **8 tests stay `keep-gated-native`**: 5 type-hover/diagnostic tests need P6-3
  assets, plus the unavailable-engine decision, performance and stability.
- **12 tests are `delete/replace-at-P7`**: 5 `real_node.rs` and 7
  `wave5_acceptance.rs` Node-backend acceptance tests. The old tests are not
  deleted by this plan; deletion happens at P7 only after the recorded native
  replacement is `mandatory_green` and reviewed.
- **The 2 silent returns + optional `tsc` branch become 3 mandatory native
  gates**, not source-shape freezes. During the transition the old tests are
  marked `#[ignore]` (a skipped test is not a pass) and the native replacements
  carry `required_native_install()`, which panics when the gate is requested and
  the package/engine/assets are missing.
- **No false green**: gate status and ledger case status are separate axes. A
  green case does not make its E2E gate green; a deleted old test does not make
  a case green; an `Unsupported`/empty result is not a pass.
- **Still BLOCKED**: P6-3 asset handoff, native rename/symbol facts, native
  event-handler completion/definition, native session restart/epoch, P6-4b/c
  real package + no-Node process audit, native performance/stability baselines,
  cross-platform support, and the default backend switch / Node removal (P6-8 /
  P7). G5 and G7 are not claimed.

## 1. Baseline facts used by this plan

### 1.1 Inventory

The frozen inventory is exactly the P7 triage's 23 gates
(`docs/evidence/2026-11-18-tsgo-p7-gates-triage.md:82-106`). It is enforced by:

- `crates/wxml-lsp/tests/native_parity/native_gates.rs:131-190`
  (`all_ignored_rust_tests_are_mandatory_unmigrated_gates`): exactly 23 ledger
  gates, every one `status=required_not_migrated`, and the scanned `#[ignore]`
  set must equal the ledger set.
- `crates/wxml-lsp/tests/native_parity/native_gates.rs:193-304`
  (`semantic_tools_silent_returns_are_mandatory_unmigrated_gates`): exactly 2
  silent gates, 3 silent-skip entries, the two early returns, the optional
  `tsc` branch, and `symbols_edits::MIGRATED_CASES` must be empty.
- `crates/wxml-lsp/tests/native_parity.rs:153-197`: all 16 partition
  `MIGRATED_CASES` lists must be empty.
- `crates/wxml-lsp/tests/native_parity.rs:263-308`: declared `#[test]`
  functions must exactly equal `MIGRATED_CASES`.

Counts at `f6f175f` (static enumeration; the P7 triage recorded 25 passed at
`8490a92`):

| Surface | Count | Anchor |
| --- | ---: | --- |
| `#[ignore]` gates | 23 | `native_gates.rs:135,190` |
| `semantic_tools` silent returns | 2 | `semantic_tools.rs:2082-2085`, `:2569-2572` |
| Optional `tsc` branch | 1 | `semantic_tools.rs:2168-2195` |
| `native_parity` tests | 25 | `native_parity.rs` + `native_parity/{harness,ledger,native_gates,fixture_hash,source_scan}.rs` |
| `stdio_type_loop` old gates | 9 | `stdio_type_loop.rs:499,601,653,701,775,831,942,1008,1078` |
| `real_node` old gates | 5 | `real_node.rs:30,103,219,494,626` |
| `wave5_acceptance` old gates | 7 | `wave5_acceptance.rs:670,723,771,845,995,1029,1112` |
| `performance_bench` old gate | 1 | `performance_bench.rs:781` |
| `q2_stability` old gate | 1 | `q2_stability.rs:202` |
| `semantic_tools` tests | 41 | `semantic_tools.rs` (`#[test]`) |

### 1.2 Native backend reality (current worktree; `f6f175f` equivalents noted)

| Fact | Anchor |
| --- | --- |
| Default `--stdio` constructs `NodeBackend::new(NodeBackendConfig::from_env(), host)` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:181-190` (WIP; `f6f175f:93-101`) |
| `--backend` selector is WIP-only; `f6f175f` had no such flag | current WIP parser: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:101-160`; `f6f175f` parser: `main.rs:56-77` |
| `backend_tsgo` is `pub(crate)` and explicitly inert | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:47` (WIP; `f6f175f:44-46`); `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:60-64` |
| Adapter constructor is `with_host(host, SessionOptions)`; no package discovery | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:174-189` |
| Rename edits and script symbols are `Unsupported` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:29-43` and `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:145` |
| Completion supports only `ExpressionMember` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:447-474` |
| Definition only maps expression-member positions | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:433-445` |
| `symbols()` returns `SymbolResult::unsupported` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:476-481` |
| Adapter still builds legacy `AnalysisInput::new` (no assets/compiler options) | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:121-127` |
| `js_check` is hard-coded `Inherit` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:198-204` |
| Legacy VFS tsconfig keeps `noLib:true` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:928-939` |
| Environment tsconfig exists but is not wired from the adapter | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:941-1040` |

WIP P6-4a (uncommitted) adds `--backend native` (`main.rs:126-160`), `serve_native`
(`main.rs:196-198`), `run_stdio_native`, `EngineDiscoveryError`, `SandboxError`,
`engine.rs` and `sandbox.rs`. It still
does **not** wire P6-3 assets and does not change the default `--stdio` path.
Do not cite it as landed.

### 1.3 The two status axes

Do not conflate them:

1. **Gate status** (`ignored_rust_tests[].status`,
   `silent_skip_gates[].status`): whether the *old fail-open surface* has a
   mandatory native replacement. Today every entry is `required_not_migrated`.
2. **Case status** (`cases[].status`): whether a TS oracle case has exact Rust
   evidence. Today 33 are `green`, 270 `planned`, 36
   `retired-with-decision` (`docs/reference/tsgo-test-contract.md:10-24`).

A gate can be green while its related case stays `planned` (the native test is
weaker than the TS oracle). A related case can be green while its E2E gate is
still `required_not_migrated` (the exact evidence is a lower-level test, as with
`TS-EVENTS-0071`, `TS-EVENTS-0143`, `TS-MAPPING-0010`). The plan keeps both
honest.

## 2. Exact disposition of the 23 ignored tests

Definitions used in the tables:

- **ENABLE-NOW-NATIVE**: the native replacement needs only the P6-4a selector,
  discovery and a verified package. It needs **no P6-3 type assets**. Write the
  replacement now; remove `#[ignore]` only when the native gate job is required
  (S8). At HEAD no test can run because the selector is not committed.
- **KEEP-GATED-NATIVE**: the old test stays `#[ignore]` and its native
  replacement stays behind a named prerequisite. It is not deleted at P7 until
  the prerequisite lands and the replacement is `mandatory_green`.
- **DELETE/REPLACE-AT-P7**: the old Node-backend acceptance test is removed at
  P7 only after its native replacement is `mandatory_green` and reviewed. The
  behavior requirement is not dropped.

**No test is deleted by this plan.** “Delete” means P7, after the replacement
exists.

**Relation to the P7 triage.** The triage's `0 enable-now / 11 keep-gated /
12 delete` split is preserved at HEAD: 0 tests can actually run natively today.
Read strictly as “remove the `#[ignore]` attribute in this commit”, enable-now is
still **0**; the 3 are the first wave after S0/S1/S8, not a claim that they run
in the default suite today.
This plan refines the 11 keep-gated tests by separating the 3 static/degradation
tests (`ac011`, static WXS/template, unsaved WXS overlay) that need **no P6-3
assets** from the 8 that do (5 type tests + unavailable-engine decision +
performance + stability). The 3 are "enable-now" only in the sense that their
native replacement can be written and made green immediately after S0/S1; they
stay `#[ignore]` until the native job is required (S8). The 12 delete-at-P7
tests are unchanged.

### 2.1 ENABLE-NOW-NATIVE (3)

All three are public `--stdio` tests that do not need `wx`/standard-library
typings. They still need the P6-4a package to start `--backend native`; they do
not need P6-3.

| Gate | Old test | Native prerequisite | Native replacement / condition |
| --- | --- | --- | --- |
| G-P5-E2E-09 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:775` (`ac011_unknown_owner_degrades_honestly`) | `P6-4A_SELECTOR`: `--backend native` + verified package; adapter must return `Unavailable` for an ownerless template and the server must publish the existing degradation warning. No P6-3 assets. | Add `native_ac011_unknown_owner_degrades_honestly` to `native_engine_real`. Assert: one warning with `unsupported`/`unavailable`, empty hover, no error diagnostic, zero engine calls beyond startup. Green when the P6-4a package starts and the warning is emitted. |
| G-P5-E2E-12 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:831` (`static_wxs_and_template_semantics_work_over_stdio`) | `P6-4A_SELECTOR`: static WXS/template hover, definition and completion are server-side; no P6-3 assets. | Add `native_static_wxs_and_template_semantics_work_over_stdio`. Keep every exact assertion: `utils.format` hover + JSDoc, WXS definition line 1, `template \`card\`` hover, `shared.wxml` definition, `is=` completion, `utils.` member completion, template data completion. |
| G-P5-E2E-14 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:1008` (`unsaved_wxs_overlay_updates_static_hover_and_completion`) | `P6-4A_SELECTOR`: static WXS overlay path; no P6-3 assets. | Add `native_unsaved_wxs_overlay_updates_static_hover_and_completion`. Assert disk `string`, overlay `number`, `renamed` completion, close restores `string` + `label`; assert no file write. |

Why not more: `ac001`, `ac002`, `ac003`, `ac013` and
`component_behavior_data_and_properties_work_over_stdio` assert TypeScript
inference (`user.name: string`, `nmae` diagnostics, `sharedCount: number`,
`string | number`). Those cannot pass with the legacy `noLib:true` VFS. They
move to 2.2.

### 2.2 KEEP-GATED-NATIVE (8)

| Gate | Old test | Specific native prerequisite | Why it cannot be enabled now |
| --- | --- | --- | --- |
| G-P5-PERF-01 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/performance_bench.rs:781` (`q2_performance_benchmark`) | `native_q2_performance_benchmark`; `P6-4A_SELECTOR` + `P6-3_ASSETS` + `PERF_BASELINE`: native in-process adapter, release build, 200 samples, cold/hot p50/p95/p99, CPU/RSS, 10-round growth. | Today it constructs `NodeBackend`/`NodeTransport` directly. Retarget to a native adapter seam; a release baseline is impossible before P6-3/P6-4b. Keep Linux-only (`performance_bench.rs:1`). |
| G-P5-STABILITY-01 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/q2_stability.rs:202` (`ten_round_lifecycle_stability`) | `native_ten_round_lifecycle_stability`; `P6-4A_SELECTOR` + `P6-3_ASSETS` + `P6-5_RESTART` + `STABILITY_BASELINE`: 10 open/edit/close rounds, native session restart/epoch, no monotonic RSS/process growth, no stale diagnostics. | It directly constructs `NodeBackend` and uses `/proc`; native restart/epoch and native RSS accounting are not implemented. Keep Linux-only (`q2_stability.rs:1`). |
| G-P5-E2E-06 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:499` (`ac001_page_data_and_jsdoc_hover`) | `native_ac001_page_data_and_jsdoc_hover`; `P6-3_ASSETS`: `Page`/`wx` typings, compiler options, JSDoc composition, exact hover ranges; `P6-4A_SELECTOR`. | Asserts `user.name` is `string`, the Chinese JSDoc text, exact `name`/`user` ranges. Legacy native VFS has no `Page` typings (`vfs.rs:928-939`; `backend_tsgo/input.rs:121-127`). |
| G-P5-E2E-07 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:601` (`ac002_field_typo_diagnostic_and_fix_clear`) | `native_ac002_field_typo_diagnostic_and_fix_clear`; `P6-3_ASSETS` + adapter diagnostics + `didChange` overlay reanalysis; `P6-4A_SELECTOR`. | Asserts exactly one `nmae` error at an exact range and `[]` after the fix. Without `Page`/`wx` types the property does not exist for the right reason, and without adapter diagnostics the status/range mapping is unproven. |
| G-P5-E2E-08 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:653` (`ac003_unicode_and_crlf_positions`) | `native_ac003_unicode_and_crlf_positions`; `P6-3_ASSETS` + UTF-16/UTF-8 + CRLF mapping parity; `P6-4A_SELECTOR`. | Asserts two `nmae` errors at characters 13/17 and 12/16 across CJK + emoji + CRLF. This is a type-diagnostic test, not only a mapping unit test. |
| G-P5-E2E-10 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:701` (`ac013_component_properties_hover`) | `native_ac013_component_properties_hover`; `P6-3_ASSETS` + component property registration + Partial status parity; `P6-4A_SELECTOR`. | Asserts no error, the existing Partial warning (`registration has no \`data\` member`) and `title: string`. The Partial reason is a Node-path behavior that the native adapter must reproduce or explicitly replace; do not silently drop it. |
| G-P5-E2E-11 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:942` (`component_behavior_data_and_properties_work_over_stdio`) | `native_component_behavior_data_and_properties_work_over_stdio`; `P6-3_ASSETS` + Behavior/`optionalTypes` typings + definition mapping; `P6-4A_SELECTOR`. | Asserts `sharedCount: number`, JSDoc `共享计数`, definition in `components/card/card.ts`, and `cardTitle: string \| number`. Legacy VFS cannot produce those types. |
| G-P5-E2E-13 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:1078` (`static_wxs_answers_survive_an_unavailable_type_backend`) | `native_static_wxs_answers_survive_an_unavailable_type_backend`; `DECISION_STARTUP_FAILURE` + `P6-4A_SELECTOR`. Decision: is a missing native engine **startup-fatal** (P6-4a says yes) or a **started-but-unavailable session**? | The test intentionally starts the server with `/nonexistent/wxml-type-backend.js`. `run_stdio_native` currently fails before `initialize` when discovery fails, so the premise “static answers remain while the type backend is unavailable” has no native startup mode. If fail-closed is kept, rewrite the gate to assert startup failure and add a separate `static_answers_survive_a_started_unavailable_session` test using a test-only unavailable-session seam. Do not keep a green skip. |

### 2.3 DELETE/REPLACE-AT-P7 (12)

Old tests stay untouched and `#[ignore]` until P7. Each row names the native
replacement and the prerequisite that must be green before deletion.

| Gate | Old test | Native replacement | Native prerequisite |
| --- | --- | --- | --- |
| G-P5-E2E-04 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:30` (`real_node_hover_and_diagnostics_smoke`) | `native_hover_and_diagnostics_smoke` (`native_engine_real`) | `P6-3_ASSETS` + adapter hover/diagnostics. |
| G-P5-E2E-03 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:103` (`real_node_expression_and_event_completion_smoke`) | `native_expression_and_event_completion_smoke` | `P6-3_ASSETS` + `NATIVE_EVENT_COMPLETION` (completion currently only `ExpressionMember`). |
| G-P5-E2E-02 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:219` (`real_node_event_handler_diagnostics_definition_and_completion`) | `native_event_handler_diagnostics_definition_and_completion` | `P6-3_ASSETS` + `NATIVE_EVENT_COMPLETION`; the definition half needs event-handler definition, which the adapter does not implement. |
| G-P5-E2E-01 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:494` (`non_owner_script_overlay_propagates_and_survives_a_node_restart`) | `native_non_owner_script_overlay_survives_restart` | `P6-3_ASSETS` + `P6-5_RESTART`: non-owner script overlay propagation, restart, new epoch, no old response. |
| G-P5-E2E-05 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:626` (`real_node_symbols_smoke`) | `native_symbols_smoke` | `NATIVE_SYMBOLS`: `SymbolQuery::DataPath` + `EventHandler` with declaration/references/occurrences/insertion. The adapter returns `Unsupported` today. |
| G-P5-E2E-15 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:670` (`ac005_loop_scope_typed_locals`) | `native_ac005_loop_scope_typed_locals` | `P6-3_ASSETS` + loop-local `item`/`itemIndex` typing; native `DiagnosticsResult.status` must be `Supported` for this fixture, not the adapter's blanket `Partial`. |
| G-P5-E2E-16 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:723` (`ac006_conditional_union_narrowing`) | `native_ac006_conditional_union_narrowing` | `P6-3_ASSETS` + `wx:if`/`elif` narrowing; exact `age`/`maybeUser` diagnostics; native `DiagnosticsResult.status` must be `Supported`, not a blanket `Partial`. |
| G-P5-E2E-20 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:771` (`ac012_component_property_bindings`) | `native_ac012_component_property_bindings` | `P6-3_ASSETS` + component property diagnostics and `TypeQuality`/`CapabilityStatus` parity. |
| G-P5-E2E-21 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:845` (`ac012_event_handlers_basic`) | `native_ac012_event_handlers_basic` | `P6-3_ASSETS` + `NATIVE_EVENT_COMPLETION` + event-handler definition; exact missing-handler diagnostics. |
| G-P5-E2E-18 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:995` (`ac008_multi_view_fixture_isolation`) | `native_ac008_multi_view_fixture_isolation` | `P6-3_ASSETS` + project/view isolation: `card` typo must not leak into `pages/a`/`pages/b`. |
| G-P5-E2E-19 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:1029` (`ac008_simultaneous_views_do_not_pollute_each_other`) | `native_ac008_simultaneous_views_do_not_pollute_each_other` | `P6-3_ASSETS` + simultaneous-view isolation and interleaved result identity. |
| G-P5-E2E-17 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:1112` (`ac008_alias_project_hover_and_definition`) | `native_ac008_alias_project_hover_and_definition` | `P6-3_ASSETS` + `extends`/`baseUrl`/`paths` resolution and definition to `models/user.ts`. `project_host` resolves `extends` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:2030`, `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:2195`); the native VFS must forward the filtered options without dropping the alias (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:1005-1035`). |

### 2.4 What “green” means for a row

A row is not green because the replacement file exists. It is green only when:

1. the replacement test runs in the required native job;
2. it fails when the package/engine/assets are removed (fail-closed evidence);
3. it passes with the pinned engine identity and asset digest;
4. the old test is still `#[ignore]` (no early deletion);
5. the ledger gate status is `gate_green` or higher (`mandatory_green` for final G5/G7 credit).

## 3. The two silent returns and the optional `tsc` branch

### 3.1 Current behavior (exact)

`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/semantic_tools.rs:2069-2077`:

```rust
fn real_node_paths() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    // ...
    server.exists().then_some((server, backend_dir))
}
```

`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/semantic_tools.rs:2081-2085` and `:2568-2572`:

```rust
let Some((server, backend_dir)) = real_node_paths() else {
    eprintln!("skipping real Node LSP test: build backends/typescript first");
    return;
};
```

Both tests are **not** `#[ignore]`. The ordinary suite records two passes
without exercising rename, applied edits, reparse or typecheck. The optional
`tsc` branch is at `semantic_tools.rs:2168-2195`: if
`node_modules/typescript/bin/tsc` is absent it prints
`tsc not installed; skipped post-edit type check` and continues. The P7 triage
empirically confirmed both fail-open paths (`docs/evidence/2026-11-18-tsgo-p7-gates-triage.md:213-221`).

### 3.2 Target: 3 mandatory native gates

| Old surface | Native replacement | What it must prove |
| --- | --- | --- |
| `real_node_lsp_component_property_edit_applies_and_parses` silent return | `native_component_property_edit_applies_and_reparses` | A component property rename produces a `WorkspaceEdit` that applies to an isolated snapshot; the applied `card.wxml`, caller `.wxml` and `card.ts` reparse; the renamed symbol has the expected native type; a clean post-edit project has no type errors. |
| optional `tsc` branch | `assert_native_edit_reparses_and_typechecks` (standalone `#[test]`, not a helper-only registration) | Every applied edit is reparse/typechecked through the native engine. No `tsc`, no `node`, no “skipped” branch. Missing engine/assets is a hard failure. |
| `real_node_lsp_rejects_cross_namespace_and_dynamic_edits` silent return | `native_rejects_cross_namespace_and_dynamic_edits` | `-32803`, no `result`, source bytes unchanged for cross-namespace and dynamic rename attempts. |

The `tsc` replacement must be a real `#[test]` because the current
`BLOCKED_NATIVE_GATES` registration treats it as a gate but
`partition_modules_declare_only_listed_tests` only counts `#[test]` functions
(`crates/wxml-lsp/tests/native_parity.rs:263-308`). A helper-only name can never
satisfy `MIGRATED_CASES`; making it a standalone test removes that loophole.
It can land before the two rename tests because it needs only P6-3 assets and a
fixed edit fixture.

### 3.3 Required-mode / environment behavior

Use one test-only flag and one explicit package variable:

- `WXML_TEST_NATIVE=1` — the native gate is requested. If set and the package is
  missing, the test **panics** with the exact remediation. If unset, a native
  test is `#[ignore]` and libtest reports it as ignored; it is never a pass.
- `WXML_TEST_NATIVE_PACKAGE=<absolute path>` — test-only package root. It must be
  absolute, canonical, outside the repository, and contain
  `bin/wxml-lsp`, `manifest.json`, `engine/lib/tsc` and
  `typings/miniprogram-api-typings`. Production discovery must never read this
  variable; only `tests/common/native.rs` does.
- `WXML_REQUIRE_REAL_ENGINE=1` — existing convention (`crates/wxml-delivery/tests/real_asset/support.rs:119`,
  `crates/wxml-lsp/src/backend_tsgo/tests.rs:2498-2505`). The native package
  helper requires the pinned engine SHA-256
  `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70`,
  version `7.1.0-beta`, AST `9`, git head
  `3e9236361fa97a9e6c685f4833ac457ae2d8c50a`
  (`crates/wxml-lsp/tests/native_parity/harness.rs:15-23`).
- `WXML_REQUIRE_LSP_NATIVE_OPT_IN=1` — new: proves the opt-in `--backend native`
  path starts and does not spawn Node. This may pass before P6-8.
- `WXML_REQUIRE_LSP_NATIVE=1` — existing final-default check
  (`crates/wxml-delivery/tests/no_node_guard.rs:851-863`). It must stay failing
  while `NodeBackend::new` is the default; do not relax it to make the opt-in
  path look green.

Required-mode behavior:

1. `required_native_install()` is called **before** any project setup. No
   `Option`, no `return;`.
2. Missing package/engine/asset → panic naming the missing path/field and the
   exact env var to set. No Node fallback, no “skipped”.
3. Engine identity or asset digest mismatch → panic. Do not accept a different
   engine to make a test pass.
4. The post-edit typecheck has a **positive control**: a deliberately injected
   type error must produce an error diagnostic, and the clean file must produce
   none. This prevents “empty diagnostics” from being mistaken for a successful
   native typecheck.
5. A source-scan gate asserts the native replacement files contain no
   `WXML_TEST_NODE`, no `dist/server.js`, no `tsc.exists()`, no
   `Command::new("node")`, and no `eprintln!("skipping real Node LSP test`.

### 3.4 Transition (no green skip)

1. Add the three native tests first, with `#[ignore = "native gate: set
   WXML_TEST_NATIVE=1 and WXML_TEST_NATIVE_PACKAGE"]`. The required native job
   runs them with `--ignored` during P6-2. Do **not** simply replace the old
   `else` with `panic!` while leaving the old tests non-ignored: that makes
   `cargo test --workspace` red for every developer without a native package,
   which is a false red, not a fail-closed gate. Mark the old tests ignored and
   put the panic in the native replacement.
2. Immediately mark the two old `semantic_tools` tests
   `#[ignore = "superseded by G-P5-E2E-22/23; native replacement is the mandatory gate"]`
   and update the ledger/freeze deliberately (see §5). A skipped test is not a
   pass; this removes the false green without requiring native readiness.
3. Only after the native replacements are `mandatory_green` and reviewed, delete
   the old tests at P7.
4. A negative CI step runs the native test with `WXML_TEST_NATIVE=1` and a
   nonexistent `WXML_TEST_NATIVE_PACKAGE`; it must fail with the exact message.
   This is the runtime proof that `old_test_must_fail_when_engine_missing` is
   true.

### 3.5 Edit-safety vs native symbol extraction

The native adapter cannot supply `SymbolResult` today (`backend_tsgo/mod.rs:476-481`),
and the LSP rename path consumes `SymbolResult` in `node_rename_plan`
(`crates/wxml-lsp/src/semantics/handlers.rs:1428-1480`). Two options:

- **Recommended for P6-2:** give the edit tests a deterministic test
  symbol-fact provider (the existing fake-backend pattern in
  `semantic_tools.rs`) and a **real native typecheck** after applying edits.
  This proves edit safety + native post-edit checking without pretending the
  native `SymbolRequest` is implemented. The full native symbol parity remains
  a separate blocked case (`TS-SYMBOLS-0061/0116`).
- **Required for full G5:** implement `SymbolQuery::DataPath`,
  `Property` and `EventHandler` in the native adapter (or re-home the symbol
  facts to a Rust source extractor) and then remove the test provider. This is
  `NATIVE_SYMBOLS`, not part of P6-3.

The `native_rejects_cross_namespace_and_dynamic_edits` assertion itself does not
need the engine, but the gate must still require the native package so the whole
edit gate fails closed when the native environment is absent.

## 4. Test seam for native LSP without changing the default backend

### 4.1 Product selector

Use the explicit CLI flag, not an environment variable:

```text
wxml-lsp --stdio --backend native
```

- Default `--stdio` remains `NodeBackend`: current WIP
  `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:181-190`;
  at `f6f175f` this was `main.rs:93-101`.
- `--backend native` must fail closed before `initialize` when discovery,
  manifest, sandbox or engine verification fails. It must never construct
  `NodeBackend` or probe `server.js`.
- Do not add a `WXML_NATIVE_ENGINE`/`WXML_TYPE_BACKEND`-style override to
  production. The delivery contract forbids an engine override
  (`docs/reference/tsgo-delivery-contract.md:1110-1114`), and the existing
  `WXML_TYPE_BACKEND` hazard is enough.
- The P6-4a WIP already sketches this: `run_stdio_native` at
  `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:133`,
  the export at
  `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:69`,
  and the selector call at
  `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:197`.
  It must be committed and reviewed before any test depends on it.

### 4.2 Test package layout

Native tests must not use `current_exe` in `target/debug` (it is not
`<package>/bin/wxml-lsp`). Add `crates/wxml-lsp/tests/common/native.rs`:

1. `NativePackage::require()` reads `WXML_TEST_NATIVE_PACKAGE` (test-only),
   canonicalizes it, rejects symlinks and non-absolute paths, and validates
   `bin/wxml-lsp`, `manifest.json`, `engine/lib/tsc`,
   `typings/miniprogram-api-typings`, engine size/SHA and asset file count.
2. `NativePackage::fixture()` creates a package under
   `/tmp/opencode/<unique>`:
   - copy `env!("CARGO_BIN_EXE_wxml-lsp")` to `<pkg>/bin/wxml-lsp` and chmod
     `0755` (a copy, not a symlink; discovery rejects symlinks);
   - write a schema-v2 `manifest.json` whose `package_version` equals
     `env!("CARGO_PKG_VERSION")` and whose `engine` entry matches the pinned
     SHA/size;
   - place the real engine at `<pkg>/engine/lib/tsc` from `TSGO_BIN` or an
     extracted `TSGO_ASSET`;
   - place the standard library at `<pkg>/engine/lib/lib*.d.ts` and the
     mini-program typings at `<pkg>/typings/miniprogram-api-typings/`.
   This is a test fixture; it does not add a production lookup path.
3. `NativeSession::start(project)` spawns
   `<pkg>/bin/wxml-lsp --stdio --backend native` with `current_dir(project)`,
   `PATH` set to a tripwire-only directory, and
   `WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`/`WXML_NODE` removed. It asserts the
   process stays alive through `initialize` and captures stderr.
4. `assert_no_node_children(child)` scans `/proc/<pid>/task/*/children` and the
   tripwire marker; a `node`/`npm`/`npx` exec is a failure.

For in-process tests (perf/stability/parity), re-home the test to
`crates/wxml-lsp/src/backend_tsgo/tests.rs` (unit tests), where
`with_install`/`with_host` are accessible. Do **not** make `TsgoBackendAdapter`
public just for tests. The existing `ParityRunner`/`NativeParityBackend` trait
(`crates/wxml-lsp/tests/native_parity/harness.rs:341-347`) is the right seam for
parity cases; implement it in the unit-test module or a feature-gated test
module, not in a public API.

### 4.3 What must land first

In order:

1. **P6-4a** — engine discovery, sandbox, `--backend native` selector, CLI
   fail-closed test. Current WIP must be committed/reviewed; `f6f175f` does not
   have it.
2. **Test package builder** — `tests/common/native.rs` + copy-binary fixture;
   no product changes.
3. **P6-3 adapter** — `VerifiedInstall` → `TypeAssets` → `AnalysisInput::with_environment`;
   compiler options; `js_check`; adapter diagnostics/definition/completion.
   Without this, all type-behavior tests stay gated.
4. **P6-5 restart/epoch** — for the overlay restart gate.
5. **NATIVE_EVENT_COMPLETION / NATIVE_SYMBOLS** — for two real_node replacements,
   one wave5 replacement, and the two edit gates.
6. **P6-4c no-Node audit** — real package + `strace`/PATH tripwire for the final
   claim. The P6-2 plan can use an opt-in tripwire; it must not claim P6-4c.

### 4.4 No-Node process gate

- `crates/wxml-delivery/tests/no_node_guard.rs:473-489` currently derives
  `LSP_NATIVE: BLOCKED` from the presence of `NodeBackend::new` in `main.rs`.
  That is a source-shape check, not a runtime gate.
- Add `LSP_NATIVE_OPT_IN`: PASS only when a real package's
  `bin/wxml-lsp --stdio --backend native` answers `initialize`/`didOpen`/`hover`
  and the PATH tripwire/`/proc` scan shows no Node exec.
- Keep `LSP_NATIVE_DEFAULT` BLOCKED until the default path is native
  (P6-8/P7). Do not repurpose `WXML_REQUIRE_LSP_NATIVE=1`; add
  `WXML_REQUIRE_LSP_NATIVE_OPT_IN=1` for the opt-in claim.
- The final G7 claim still needs `strace -f -e trace=execve,execveat` on a real
  install (`scripts/verify-no-node.sh:1-20` documents the current limits; a PATH
  shim alone cannot rule out an absolute-path Node invocation).

## 5. Ledger updates and no-false-green criteria

### 5.1 Gate status ladder

Add these statuses to `ignored_rust_tests[]` and `silent_skip_gates[]`; keep
`must_migrate=true`:

```text
required_not_migrated
  -> replacement_authored
  -> replacement_red          (fails for the right reason: missing engine/wrong result)
  -> gate_green               (passes in the required native job; may still use --ignored)
  -> mandatory_green          (passes without --ignored in the required native job)
  -> replacement_reviewed     (independent reviewer + run on review head)
  -> old_test_removed         (P7 only)
  -> closed
```

Here `gate_green` is the gate-level “green” (the required native job passes)
and `mandatory_green` is the no-`#[ignore]` green required for final G5/G7
credit. The case-level `green`/`reviewed` ladder is separate (§5.2).

Rules:

- No direct jump from `required_not_migrated` to `gate_green`; a RED run is
  required. The RED run proves the replacement actually fails when the
  engine/assets are absent.
- `gate_green` is the transition status while the required native job runs the
  replacement with `--ignored`. `mandatory_green` requires the same test to run
  **without** `--ignored`; only `mandatory_green` counts for final G5/G7 credit.
  Record the distinction explicitly to avoid counting an opt-in gate as
  mandatory.
- `replacement_reviewed` requires `mandatory_green`, `reviewer`, `reviewed_at`,
  `review_head`, `gate_run_command`, and `engine_identity`
  (version/gitHead/AST/artifact SHA). A review on a different head is not review
  evidence.
- `old_test_removed` requires the old `#[ignore]` and test body to be gone; the
  source scan must prove it. No gate may be `closed` while the old test exists.

### 5.2 Case `green` / `reviewed` criteria

For each related `TS-...` case, use the contract ladder
`planned -> red -> green -> reviewed -> closed`
(`docs/reference/tsgo-test-contract.md:43-52`):

- **red**: a native replacement exists and fails with a captured command/exit/
  reason. This can be the fail-closed missing-engine run or a real mismatch.
- **green**: `migration_evidence.parity == "exact"`,
  `green_eligible == true`, `status_recommendation == "green"`, non-empty
  `rust_tests` that resolve to real `fn` declarations, `implementation_head` is
  40 hex, and the note is substantive. Additionally, every native gate linked to
  the case is `gate_green` or higher (`mandatory_green` for final G5/G7 credit).
- **reviewed**: a `review` block with reviewer identity, date, review head,
  gate run record and engine identity. Add `reviewed` to the ledger's
  `ALLOWED_STATUSES`/`counts.statuses` with a zero baseline first; update the
  frozen status-bucket count from 3 to 4 and add `reviewed: 0`; do not
  bulk-promote. The contract allows `reviewed`; the current ledger does not
  (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/ledger.rs:73`, `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/ledger.rs:99-100`).
- **closed**: P7 only, after the old test is removed and the gate is `closed`.

Do not change `GREEN_CASES`/`PLANNED_CASES` in bulk. Promote one case at a time
with its own evidence.

### 5.3 Gate-only entries

Three gates have no `related_ledger_cases`:
`G-P5-PERF-01`, `G-P5-STABILITY-01`, `G-P5-E2E-12`. Do not invent `TS-...`
case IDs. Add a gate-local `gate_evidence` block with `command`, `exit`,
`engine_identity`, `asset_digest`, `samples`/`rounds`, and `reviewer`. A gate
with empty `related_ledger_cases` and empty `gate_evidence` cannot be green.

### 5.4 `MIGRATED_CASES` / `BLOCKED_NATIVE_GATES`

Current:

- `crates/wxml-lsp/tests/native_parity/symbols_edits.rs:15` — empty.
- `:18-34` — 3 blocked gates.
- `crates/wxml-lsp/tests/native_parity.rs:164-168` — asserts all lists empty.
- `crates/wxml-lsp/tests/native_parity/native_gates.rs:301-304` — asserts
  `symbols_edits::MIGRATED_CASES.is_empty()`.

Required changes:

1. Add the 3 native edit tests to `symbols_edits.rs` as `#[test]` functions and
   list them in `MIGRATED_CASES`. Remove each name from
   `BLOCKED_NATIVE_GATES` only when the test is present and green.
2. Replace the blanket “must be empty” assertion with a per-partition expected
   manifest, and rename the test from
   `partition_skeleton_is_empty_and_fails_closed` to
   `partition_skeletons_match_manifest_and_fail_closed`. Before S6,
   `symbols_edits` is empty; after S6 it is exactly the 3 edit tests. The
   `MIGRATED_CASES` list is the same in the default and native builds; the
   difference is whether the 3 tests carry `#[ignore]` (default: yes; native
   feature/final: no). Every listed name must be a `#[test]` declaration (the
   existing `partition_modules_declare_only_listed_tests` check at
   `native_parity.rs:263-308` already enforces this).
3. `native_gates.rs:293` (`replacements.len() == 3`) becomes a ledger-driven
   check: the set of replacement names across silent gates is exactly the set
   of `MIGRATED_CASES` entries for `symbols_edits` plus the remaining
   `BLOCKED_NATIVE_GATES`.
4. `native_gates.rs:301-304` becomes: every `MIGRATED_CASES` entry is a
   `#[test]`; every `BLOCKED_NATIVE_GATES` name is not in `MIGRATED_CASES`;
   every green/reviewed gate names a replacement in `MIGRATED_CASES`.
5. `ignored_rust_tests[]` entries gain `replacement_test`, `replacement_kind`,
   `native_prerequisite`, `removal_condition` and `status`. `native_gates.rs`
   must fail if any of these is empty for a non-`required_not_migrated` gate.
   Also update `silent_skip_gates[].proposed_command` for G-P5-E2E-22/23 to
   `cargo test -p wxml-lsp --test native_parity -- --ignored`: their
   `replacement_test` path is `native_parity/symbols_edits.rs`, so the current
   `native_engine_real` command would not run them.
6. At `native_gates.rs:253-256`, flip the old-test assertion from “the old
   test must not be `#[ignore]`” to “the old test is `#[ignore]` and is listed
   in `superseded_rust_tests[]`; the native replacement is the gate.” The
   silent-return source text at `native_gates.rs:258-274` stays recorded as
   historical evidence, but it is no longer allowed to be a green pass path.
   Add two explicit lists beside `ignored_rust_tests[]`:
   - `superseded_rust_tests[]` for the two old `semantic_tools` tests when they
     are marked `#[ignore]`;
   - `native_gate_tests[]` for the new native replacements that remain
     `#[ignore]` during the transition.
   `ignored_functions()` must classify every `#[ignore]` under `tests/` into
   exactly one of the three lists; the union must equal the scan. Keep the old
   gate count at 23. At S2/S6 the scan is
   `23 old + 2 superseded + 26 native gate tests = 51`; the 26 are
   `21 native_engine_real + 1 native_perf_gate + 1 native_stability + 3
   native_parity edit tests`. At S8 the 26 native gate tests lose `#[ignore]`
   (or become feature-gated), leaving 25 ignored (23 old + 2 superseded); at P7
   the old/superseded tests are deleted, leaving 0. Do not rewrite the historical
   `baseline.rust_test_status` ignored count; add a current
   `counts.gate_conversion` block with `old_ignored_gates`,
   `superseded_old_tests`, `native_gate_tests_ignored`,
   `native_replacements_green`, `native_replacements_mandatory`,
   `ignored_scan_total`.

### 5.5 No false green checklist

- [ ] A green case has a runtime pass, not only a source-shape test.
- [ ] The pass records pinned engine identity and asset digest.
- [ ] A negative control fails when the engine/assets are absent.
- [ ] A positive typecheck control proves diagnostics are real.
- [ ] `Unsupported`/empty/`Partial` is not counted as the expected positive
      result unless the TS oracle expects that status.
- [ ] An old test deletion is not evidence of migration.
- [ ] An ignored test is not counted as passed.
- [ ] `MIGRATED_CASES` contains only real `#[test]` functions, not helpers.
- [ ] `BLOCKED_NATIVE_GATES` and `MIGRATED_CASES` are disjoint.
- [ ] Gate status and case status are both consistent with their evidence.

## 6. CI commands and expected counts

### 6.1 Default job (no native package)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo doc --workspace --no-deps
cargo deny --offline check
```

Expected at the end of P6-2 (transition state):

| Binary | Expected |
| --- | --- |
| `native_parity` | 25 passed, 0 failed; the 3 edit tests are `#[ignore]` (or feature-gated) |
| `semantic_tools` | 41 tests: 39 passed, 0 failed, 2 ignored (superseded old tests) |
| `stdio_type_loop` | 0 run, 9 ignored |
| `real_node` | 0 run, 5 ignored |
| `wave5_acceptance` | 0 run, 7 ignored |
| `performance_bench` | 0 run, 1 ignored |
| `q2_stability` | 0 run, 1 ignored |
| `native_engine_real` | 0 run, 21 ignored (during transition) |
| `native_perf_gate` | 0 run, 1 ignored |
| `native_stability` | 0 run, 1 ignored |
| `no_node_guard` | 3 passed |
| `#[ignore]` scan classification | 51 = 23 old gates + 2 superseded old tests + 26 native gate tests |

The default job must **not** require a native package, Node, `dist/server.js`
or network. Ignored is reported as ignored, never as passed.

### 6.2 Required native gate job

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  WXML_TEST_NATIVE=1 \
  WXML_REQUIRE_REAL_ENGINE=1 \
  WXML_TEST_NATIVE_PACKAGE=/tmp/opencode/tsgo-native-pkg \
  WXML_TEST_TMPDIR=/tmp/opencode/tsgo-gates \
  cargo test -p wxml-lsp --test native_engine_real \
    -- --ignored --test-threads=1 --nocapture
```

Expected at completion: **21 passed, 0 failed, 0 ignored** (all 21 are
`--ignored` by attribute, so the run executes them). Before completion, the
same command reports 21 run with the blocked replacements failing by design.

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
  WXML_TEST_NATIVE_PACKAGE=/tmp/opencode/tsgo-native-pkg \
  cargo test -p wxml-lsp --test native_perf_gate --release \
    -- --ignored --nocapture
```

Expected: **1 passed** at completion. It is Linux-only and runs alone (no
parallel test worker), 200 samples, p95 thresholds from
`performance_bench.rs:781-860`.

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
  WXML_TEST_NATIVE_PACKAGE=/tmp/opencode/tsgo-native-pkg \
  cargo test -p wxml-lsp --test native_stability \
    -- --ignored --nocapture
```

Expected: **1 passed** at completion. Linux-only, 10 rounds, no monotonic RSS.

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
  WXML_TEST_NATIVE_PACKAGE=/tmp/opencode/tsgo-native-pkg \
  cargo test -p wxml-lsp --test native_parity \
    -- --ignored --test-threads=1 --nocapture
```

Expected: **3 passed** (the edit gates) during transition; with the final
feature-gated target, `cargo test -p wxml-lsp --features native-e2e --test native_parity`
should be **28 passed**. Under that feature the 3 edit tests must not carry
`#[ignore]`; otherwise the result is 25 passed + 3 ignored, not 28.

Totals: 23 old ignored gates become 21 + 1 + 1 = **23 native replacements**;
the 2 silent returns + `tsc` become **3 edit gates**; `native_parity` grows from
25 to 28.

### 6.3 Negative fail-closed job (required)

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
  WXML_TEST_NATIVE_PACKAGE=/tmp/opencode/does-not-exist \
  cargo test -p wxml-lsp --test native_parity \
    -- --ignored --exact symbols_edits::native_component_property_edit_applies_and_reparses
```

Expected: **exit 101, 1 failed, 0 passed**, stderr contains
`WXML_TEST_NATIVE_PACKAGE` / `native package`. This is the proof that a missing
engine is not a skip. Add the same negative probe for `native_rejects...` and
`assert_native_edit_reparses_and_typechecks` in `native_parity`.

### 6.4 `WXML_TYPE_BACKEND` hazard

The triage recorded the exact failure mode: a shell-exported
`WXML_TYPE_BACKEND=/home/zxg/code/rust/wxml-lsp/backends/typescript/dist/server.js`
makes `wave5_acceptance` fail 7/7 because its helper rejects a backend outside
the worktree (`crates/wxml-lsp/tests/wave5_acceptance.rs:119-169`;
`docs/evidence/2026-11-18-tsgo-p7-gates-triage.md:132-142`). Handling:

1. **Never set** `WXML_TYPE_BACKEND` / `WXML_BACKEND_DIR` at CI job scope. Use
   `env -u` on every gate step.
2. For the old Node gate job, set them explicitly to this worktree's absolute
   `backends/typescript/dist/server.js` and `backends/typescript`; do not
   inherit.
3. For the native job, set **poison** values:
   `WXML_TYPE_BACKEND=/nonexistent/poison-server.js`,
   `WXML_BACKEND_DIR=/nonexistent`, `WXML_NODE=wxml-poison-node`. The native
   tests must still pass. This turns the hazard into a positive isolation test.
4. `PATH` for the native child is a tripwire-only directory with
   `node`/`npm`/`npx` shims that write a marker and exit 99. Assert the marker
   is absent and `/proc/<pid>/task/*/children` contains no Node.
5. Keep the final `WXML_REQUIRE_LSP_NATIVE=1` check red until the default path
   is native. Use `WXML_REQUIRE_LSP_NATIVE_OPT_IN=1` for the opt-in path.

## 7. Ordered prerequisites and what remains BLOCKED

### 7.1 Ordered plan

| Step | Work | Exit evidence |
| ---: | --- | --- |
| S0 | Commit/review P6-4a: engine discovery, sandbox, `--backend native`; keep default `--stdio` Node. | `cli.rs` `native_without_install_fails_closed` (WIP `crates/wxml-lsp/tests/cli.rs:63-88`) passes; `--stdio` still Node; no Node fallback. |
| S1 | Add `tests/common/native.rs` + package fixture + `WXML_TEST_NATIVE` semantics; add source-scan gate forbidding `WXML_TEST_NODE`/`dist`/`tsc`/`node` in native replacements. | One smoke test starts `bin/wxml-lsp --backend native`, answers `initialize`; missing package fails with the exact message. |
| S2 | Add `native_engine_real` with 21 tests: 3 enable-now + 18 blocked. Add `native_perf_gate`/`native_stability` skeletons. | 3 pass; 18 fail with `BLOCKED: ...`; negative fail-closed job passes. |
| S3 | P6-3 adapter assets: `VerifiedInstall` → `TypeAssets` → `AnalysisInput::with_environment`, compiler options, `js_check`, adapter diagnostics/hover. | 5 type stdio + hover/diag real_node replacements pass; environment tsconfig has no `noLib`; the asset bundle is pinned by `assets.lock.json` (currently missing — F3-03 BLOCKED; the P6-3 test split is 136 / 6,392,948 and is not a delivery count); positive/negative typecheck controls. |
| S4 | `NATIVE_EVENT_COMPLETION`: event-handler completion + definition. | `native_expression_and_event_completion_smoke`, `native_event_handler_diagnostics_definition_and_completion`, `native_ac012_event_handlers_basic` pass. |
| S5 | `P6-5_RESTART`: native session restart/epoch, overlay replay. | `native_non_owner_script_overlay_survives_restart` passes; old epoch responses dropped. |
| S6 | `NATIVE_SYMBOLS` or a deterministic test symbol provider + 3 edit gates. | `native_component_property_edit_applies_and_reparses`, `assert_native_edit_reparses_and_typechecks`, `native_rejects_cross_namespace_and_dynamic_edits` pass; `MIGRATED_CASES` has 3; `BLOCKED_NATIVE_GATES` shrinks. |
| S7 | Native perf/stability gates. | 1+1 pass in the required native job; Linux-only claims recorded. |
| S8 | Ledger/CI rewrite: gate statuses, case evidence, `MIGRATED_CASES`, required native job, default counts, poison-env test. | No gate is `required_not_migrated` for a tested replacement; no green skip; 0 false green; `WXML_TYPE_BACKEND` poison test passes. |
| S9 | P7 only, **not part of this plan and not executed here**: delete the 12 old Node acceptance tests, remove the superseded `semantic_tools` tests, update gate counts/hashes, remove Node. | Out of scope for this plan. |

### 7.2 BLOCKED

| Blocker | Why | What it blocks |
| --- | --- | --- |
| P6-3 type assets | `backend_tsgo/input.rs:121-127` still uses `AnalysisInput::new`; `vfs.rs:928-939` keeps `noLib:true`; `js_check` is `Inherit` at `input.rs:198-204`. | 5 keep-gated type tests, 11 of the 12 delete replacements, and the `tsc` edit gate. The symbols smoke needs `NATIVE_SYMBOLS` instead. |
| Native rename/edit | Adapter has no rename; `backend_tsgo/mod.rs:29-43`; rename consumes `SymbolResult` in `semantics/handlers.rs:1428-1480`; adapter symbols `Unsupported` at `mod.rs:476-481`. | The 2 silent-return edit gates. The `tsc` helper can land first with a fixed edit fixture; the two rename tests need `NATIVE_SYMBOLS` or a deterministic test symbol provider. |
| Native event-handler completion/definition | Completion is `ExpressionMember` only (`mod.rs:447-474`); definition only expression positions (`mod.rs:433-445`). | `G-P5-E2E-03`, `G-P5-E2E-02`, `G-P5-E2E-21`. |
| Native session restart/epoch | No restart in the production adapter; `run_stdio_native` creates one adapter/session. | `G-P5-E2E-01` and the stability gate. |
| P6-4b real startup with assets | `run_stdio_native` is WIP and still legacy no-asset; `VerifiedInstall` roots are informational. | All real E2E type tests and the real package gate. |
| F3-03 asset lock | `assets.lock.json` is still missing. The concurrent decision doc `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-typings-26-23-decision.md` recommends 26 upstream / 23 payload, but no lock freezes it and the tarball SRI/SHA-256 is not re-verified. | A reproducible real package and the P6-3/P6-4b asset handoff. |
| P6-4c real package/no-Node audit | No real v2 package is built in P6-2; `no_node_guard` is source-shape only; `scripts/verify-no-node.sh` is auxiliary and does not close F3-02. | Final G7 and the `LSP_NATIVE_DEFAULT` claim. |
| Performance/stability baselines | No native release baseline; perf test still constructs `NodeBackend`/`NodeTransport`; stability uses `/proc` and Node. | `G-P5-PERF-01`, `G-P5-STABILITY-01`. |
| Cross-platform | Native input rejects Windows drive/UNC; the contract is Linux-x86_64. | Windows/macOS/aarch64 claims. |
| Default backend switch / Node removal | Out of scope; P6-8/P7. | `--stdio`-without-flag native, Node deletion, G5/G7. |

### 7.3 Explicit non-claims

- This plan does not claim P6-2 is implemented, P6-4a is committed, or any of
  the 23 gates is green.
- It does not claim the native backend can run AC-001..AC-013; P6-3 is not
  wired.
- It does not claim the 2 silent returns are fixed; they are converted by S6.
- It does not claim G5/G7; 270 cases remain `planned` and the default backend is
  still Node.
- It does not delete any old test, remove Node, or modify `WXML_TYPE_BACKEND`
  behavior.

## Appendix A. Gate → related ledger cases

Statuses are from `docs/plans/tsgo-test-ledger.json` at `f6f175f`. Three cases
are already `green`; the rest are `planned`. The gate table is intentionally
separate from the case table: a green case does not make the gate green.

| Gate | Related case IDs (status) |
| --- | --- |
| G-P5-PERF-01 | none (gate-local evidence required) |
| G-P5-STABILITY-01 | none (gate-local evidence required) |
| G-P5-E2E-04 | `TS-ANALYSIS-0033` (planned), `TS-MATRIX-0102-PAGE_BASIC-IN_PLACE_SHADOW` (planned) |
| G-P5-E2E-03 | `TS-COMPLETION-0078` (planned), `TS-COMPLETION-0175` (planned) |
| G-P5-E2E-02 | `TS-EVENTS-0071` (green), `TS-EVENTS-0143` (green), `TS-COMPLETION-0175` (planned) |
| G-P5-E2E-01 | `TS-OVERLAY-0055` (planned), `TS-ANALYSIS-0161` (planned) |
| G-P5-E2E-05 | `TS-SYMBOLS-0061` (planned), `TS-SYMBOLS-0116` (planned) |
| G-P5-E2E-06 | `TS-ANALYSIS-0033` (planned), `TS-ANALYSIS-0054` (planned) |
| G-P5-E2E-07 | `TS-ANALYSIS-0033` (planned), `TS-MATRIX-0102-PAGE_BASIC-IN_PLACE_SHADOW` (planned) |
| G-P5-E2E-08 | `TS-MAPPING-0010` (green), `TS-MAPPING-0145` (planned), `TS-EVENTS-0130` (green) |
| G-P5-E2E-10 | `TS-COMPONENT-0147` (planned), `TS-MATRIX-0102-COMPONENT_PROPERTIES-IN_PLACE_SHADOW` (planned) |
| G-P5-E2E-09 | `TS-COMPLETION-0207` (planned), `TS-ANALYSIS-0297` (planned) |
| G-P5-E2E-12 | none (gate-local evidence required) |
| G-P5-E2E-11 | `TS-COMPONENT-0292` (planned), `TS-COMPONENT-0357` (planned), `TS-COMPONENT-0416` (planned) |
| G-P5-E2E-14 | `TS-OVERLAY-0023` (planned), `TS-ANALYSIS-0161` (planned) |
| G-P5-E2E-13 | `TS-COMPLETION-0207` (planned), `TS-ANALYSIS-0358` (planned) |
| G-P5-E2E-15 | `TS-LOCALS-0261` (planned), `TS-GUARDS-0174` (planned) |
| G-P5-E2E-16 | `TS-GUARDS-0097` (planned), `TS-GUARDS-0128` (planned) |
| G-P5-E2E-20 | `TS-COMPONENT-0147` (planned), `TS-COMPONENT-0206` (planned) |
| G-P5-E2E-21 | `TS-EVENTS-0071` (green), `TS-EVENTS-0143` (green) |
| G-P5-E2E-18 | `TS-ANALYSIS-0161` (planned), `TS-PROTOCOL-0450` (planned) |
| G-P5-E2E-19 | `TS-ANALYSIS-0161` (planned) |
| G-P5-E2E-17 | `TS-PROTOCOL-0450` (planned), `TS-MATRIX-0102-ALIAS_PROJECT-IN_PLACE_SHADOW` (planned) |

The 3 already-green cases have evidence in lower-level tests
(`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/script_events.rs:83`, `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/script_events.rs:183`;
`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/shadow_mapping.rs:93`). Do not re-promote them
when the E2E gate lands; add a gate link instead.

## Appendix B. Absolute anchors used in this plan

Each bullet is one clickable absolute path + line range. Multi-range files are
listed as separate bullets so the editor can resolve each one.

- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:181-190` (WIP; `f6f175f:93-101`)
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:47` (WIP; `f6f175f:44-46`)
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:29-43`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:95-100`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:174-189`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:433-481`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:121-127`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:198-204`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:928-1040`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:131-190`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:193-304`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity.rs:153-197`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity.rs:263-308`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/symbols_edits.rs:12-34`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/harness.rs:15-23`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/harness.rs:341-347`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/semantic_tools.rs:2069-2077`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/semantic_tools.rs:2081-2085`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/semantic_tools.rs:2168-2195`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/semantic_tools.rs:2568-2572`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:51-95`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:499`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:601`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:653`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:701`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:775`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:831`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:942`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:1008`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs:1078`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:30`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:103`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:219`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:458-489`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:494`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:626`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:119-169`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:670`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:723`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:771`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:845`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:995`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:1029`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:1112`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/performance_bench.rs:1`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/performance_bench.rs:781-860`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/q2_stability.rs:1`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/q2_stability.rs:202-208`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/no_node_guard.rs:473-489`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/no_node_guard.rs:851-863`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-test-contract.md:43-52`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-test-contract.md:73-81`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-test-contract.md:92-108`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-p7-gates-triage.md:82-106`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-p7-gates-triage.md:132-142`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-p7-gates-triage.md:213-221`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-p7-gates-triage.md:262-298`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-engine-discovery-plan.md:43`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-engine-discovery-plan.md:374-377`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-engine-discovery-plan.md:383`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-adapter-type-assets-plan.md:1-8`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-adapter-type-assets-plan.md:30-36`
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json`
