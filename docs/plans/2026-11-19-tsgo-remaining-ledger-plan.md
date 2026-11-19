# Implementation Plan: tsgo Remaining Ledger Backlog (270 planned)

Date: 2026-11-19  
Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`  
Base HEAD: `60f4c6c` (ledger evidence head: `6ba407b`)  
Scope: docs-only backlog. No production, Cargo, delivery, ledger, ADR, or commit changes.
No case is marked `green`, `reviewed`, or `closed` by this plan.

> **2026-11-20 后续状态（wave-5 回填）**：8 个 exact 用例已提升为 `green`
> （`TS-COMPLETION-0127`、`TS-COMPLETION-0153`、`TS-EVENTS-0108`、
> `TS-SYMBOLS-0061/0150/0215/0226/0312`）。其中 7 个由无证据转为 exact，
> `TS-EVENTS-0108` 由 blocked 证据转为 exact；另新增 17 weaker、1 blocked、
> 1 structural 证据，并对 14 个既有证据用例追加 wave-5 测试。当前台账为
> 44 `green` / 259 `planned` / 36 retired，其中 158 个 planned 无证据。本文下面的
> 270/189 表是计划制定时的基线快照，不随回填改写；见
> `docs/evidence/2026-11-20-tsgo-ledger-backfill-wave5.md`。
>
> **2026-11-19 后续状态（P6 回填）**：3 个 exact 用例已提升为 `green`
> （`TS-COMPLETION-0175`、`TS-COMPLETION-0193`、`TS-PROBES-0169`），另有
> `TS-META-0009`、`TS-SERVER-0237` 登记 weaker 证据。P6 回填后的台账为 36 `green` /
> 267 `planned` / 36 retired，其中 184 个 planned 无证据。本文下面的 270/189
> 表是计划制定时的基线快照，不随回填改写；见
> `docs/evidence/2026-11-19-tsgo-ledger-backfill.md`。

## 0. Summary

- 270 ledger cases remain `planned`: 189 with no Rust evidence, 55 weaker, 6 structural,
  17 blocked, 3 near-exact.
- They are assigned to 10 work packages, each summing to the 270 cases exactly once.
- 5 implementation workers can run concurrently on disjoint scopes. WP1, WP2 and WP4
  are a single serialized adapter lane because they share
  `crates/wxml-lsp/src/backend_tsgo/{mod,session,tests}.rs`.
- WP9 (scenario matrix) is integration-only and starts after the semantic packages.
  WP10 (perf) runs exclusively, never beside another timing-sensitive worker.
- Two Phase-0 prerequisites must land before any worker starts:
  1. the native-parity skeleton gate currently asserts every `MIGRATED_CASES` list is
     empty, so a real case cannot be added without changing
     `crates/wxml-lsp/tests/native_parity.rs`;
  2. `backend_tsgo` is `pub(crate)` and no server/config path selects it, so LSP-level
     parity needs a public test seam or an integrator-approved re-home of the case to
     the adapter's unit tests.

## 1. Baseline facts used by this plan

| Fact | Value |
| --- | --- |
| Ledger schema | `wxml-tsgo-test-ledger/1` |
| Cases | 339 (`green` 33, `planned` 270, `retired-with-decision` 36) |
| Partitions | 16, all routed from `cases[].target_rust_test` |
| Partition skeletons | `MIGRATED_CASES` empty in all 16 files |
| Skeleton gate | `native_parity.rs::partition_skeleton_is_empty_and_fails_closed` asserts emptiness |
| Ledger validator | `native_parity/ledger.rs` freezes 33/270/36 and target status `proposed` for planned |
| Frozen fixture trees | `backends/typescript/test`, `backends/typescript/samples`, `tests/fixtures`, `crates/wxml-lsp/tests/fixtures` |
| Engine pin | `7.1.0-beta`, gitHead `3e923636…`, AST 9, artifact SHA-256 `a183b195…` |
| Native query reality | hover = type + documentation composition; definition = symbol + referenced-symbols + AST; completion has no native `textEdit`/details; rename has no direct native API; script symbol extraction is not wired |
| Adapter reality | `backend_tsgo::TsgoBackendAdapter` is `pub(crate)`; rename/symbols are `Unsupported`; Windows drive/UNC roots are rejected |

Evidence-class definitions used below:

- **none**: no `migration_evidence` entry; no same-semantics Rust test.
- **weaker**: a Rust test exists but omits part of the TS assertion (usually hover type,
  diagnostics, reason text, or exact range).
- **structural**: only overlay/layout structure or a different performance sampling is
  asserted; not the same observable result.
- **blocked**: the TS assertion needs a checker/hover/definition/event-chain query that
  the Rust test never runs, or an opt-in gate that skips without `TSGO_BIN`.
- **near-exact**: same semantics with a renamed field or a typed error replacing a TS
  error code. P5 policy keeps these `planned`; they must not be promoted by workers.

## 2. Counts by partition and evidence class

| Partition | planned | none | weaker | structural | blocked | near-exact | Package |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `hover_docs` | 10 | 7 | 3 | 0 | 0 | 0 | WP1 |
| `definitions` | 5 | 0 | 4 | 0 | 1 | 0 | WP1 |
| `completion` | 8 | 6 | 2 | 0 | 0 | 0 | WP2 |
| `symbols_edits` | 21 | 21 | 0 | 0 | 0 | 0 | WP3 |
| `project_overlay` | 20 | 17 | 3 | 0 | 0 | 0 | WP4 |
| `shadow_rewrite` | 16 | 1 | 14 | 0 | 0 | 1 | WP5 |
| `mapping_utf16` | 15 | 7 | 8 | 0 | 0 | 0 | WP5 |
| `loops` | 11 | 7 | 3 | 0 | 1 | 0 | WP6 |
| `guards_js` | 21 | 15 | 1 | 0 | 5 | 0 | WP6 |
| `components` | 15 | 10 | 1 | 0 | 4 | 0 | WP7 |
| `behaviors` | 21 | 4 | 6 | 5 | 4 | 2 | WP7 |
| `events` | 15 | 3 | 10 | 0 | 2 | 0 | WP7 |
| `protocol_limits` | 40 | 40 | 0 | 0 | 0 | 0 | WP8 |
| `messages_evidence` | 8 | 8 | 0 | 0 | 0 | 0 | WP8 |
| `scenario_matrix` | 43 | 43 | 0 | 0 | 0 | 0 | WP9 |
| `perf_budget` | 1 | 0 | 0 | 1 | 0 | 0 | WP10 |
| **Total** | **270** | **189** | **55** | **6** | **17** | **3** | |

### 2.1 Exact case-ID index

`hover_docs` (10)
- none (7): `TS-ANALYSIS-0078`, `TS-ANALYSIS-0093`, `TS-ANALYSIS-0139`,
  `TS-ANALYSIS-0161`, `TS-ANALYSIS-0174`, `TS-ANALYSIS-0184`, `TS-ANALYSIS-0196`
- weaker (3): `TS-ANALYSIS-0033`, `TS-ANALYSIS-0054`, `TS-ANALYSIS-0123`

`definitions` (5)
- weaker (4): `TS-DEFINITIONS-0013`, `TS-DEFINITIONS-0052`, `TS-DEFINITIONS-0074`,
  `TS-DEFINITIONS-0093`
- blocked (1): `TS-DEFINITIONS-0036`

`completion` (8)
- none (6): `TS-COMPLETION-0066`, `TS-COMPLETION-0078`, `TS-COMPLETION-0127`,
  `TS-COMPLETION-0153`, `TS-COMPLETION-0175`, `TS-COMPLETION-0193`
- weaker (2): `TS-COMPLETION-0104`, `TS-COMPLETION-0207`

`symbols_edits` (21)
- none (21): `TS-SYMBOLS-0061`, `TS-SYMBOLS-0098`, `TS-SYMBOLS-0116`,
  `TS-SYMBOLS-0150`, `TS-SYMBOLS-0164`, `TS-SYMBOLS-0204`, `TS-SYMBOLS-0215`,
  `TS-SYMBOLS-0226`, `TS-SYMBOLS-0242`, `TS-SYMBOLS-0264`, `TS-SYMBOLS-0286`,
  `TS-SYMBOLS-0299`, `TS-SYMBOLS-0312`, `TS-SYMBOLS-0324`, `TS-SYMBOLS-0336`,
  `TS-SYMBOLS-0346`, `TS-SYMBOLS-0367`, `TS-SYMBOLS-0391`, `TS-SYMBOLS-0406`,
  `TS-SYMBOLS-0425`, `TS-SYMBOLS-0439`

`project_overlay` (20)
- none (17): `TS-ANALYSIS-0272`, `TS-ANALYSIS-0287`, `TS-ANALYSIS-0315`,
  `TS-ANALYSIS-0332`, `TS-ANALYSIS-0349`, `TS-ANALYSIS-0371`,
  `TS-COMPILER-0050`, `TS-COMPILER-0063`, `TS-COMPILER-0086`,
  `TS-COMPILER-0095`, `TS-COMPILER-0132`, `TS-OVERLAY-0023`,
  `TS-OVERLAY-0036`, `TS-OVERLAY-0047`, `TS-OVERLAY-0055`, `TS-OVERLAY-0076`,
  `TS-PROBES-0176`
- weaker (3): `TS-ANALYSIS-0297`, `TS-ANALYSIS-0358`, `TS-COMPILER-0035`

`shadow_rewrite` (16)
- none (1): `TS-REWRITE-0197`
- weaker (14): `TS-REWRITE-0051`, `TS-REWRITE-0078`, `TS-REWRITE-0088`,
  `TS-REWRITE-0101`, `TS-REWRITE-0118`, `TS-REWRITE-0127`, `TS-REWRITE-0156`,
  `TS-REWRITE-0183`, `TS-REWRITE-0190`, `TS-REWRITE-0224`, `TS-REWRITE-0239`,
  `TS-REWRITE-0250`, `TS-REWRITE-0260`, `TS-REWRITE-0285`
- near-exact (1): `TS-REWRITE-0207`

`mapping_utf16` (15)
- none (7): `TS-MAPPING-0188`, `TS-MAPPING-0208`, `TS-MAPPING-0306`,
  `TS-REWRITE-0349`, `TS-REWRITE-0368`, `TS-REWRITE-0378`, `TS-REWRITE-0413`
- weaker (8): `TS-MAPPING-0078`, `TS-MAPPING-0091`, `TS-MAPPING-0110`,
  `TS-MAPPING-0145`, `TS-MAPPING-0223`, `TS-MAPPING-0239`, `TS-MAPPING-0261`,
  `TS-REWRITE-0309`

`loops` (11)
- none (7): `TS-ANALYSIS-0205`, `TS-ANALYSIS-0226`, `TS-ANALYSIS-0243`,
  `TS-ANALYSIS-0255`, `TS-LOCALS-0261`, `TS-LOCALS-0288`, `TS-LOCALS-0312`
- weaker (3): `TS-LOCALS-0063`, `TS-LOCALS-0089`, `TS-LOCALS-0144`
- blocked (1): `TS-LOCALS-0343`

`guards_js` (21)
- none (15): `TS-GUARDS-0128`, `TS-GUARDS-0152`, `TS-GUARDS-0174`,
  `TS-GUARDS-0302`, `TS-GUARDS-0321`, `TS-GUARDS-0329`,
  `TS-PROBES-0027`, `TS-PROBES-0048`, `TS-PROBES-0059`, `TS-PROBES-0064`,
  `TS-PROBES-0080`, `TS-PROBES-0098`, `TS-PROBES-0135`, `TS-PROBES-0153`,
  `TS-PROBES-0169`
- weaker (1): `TS-GUARDS-0075`
- blocked (5): `TS-GUARDS-0097`, `TS-GUARDS-0201`, `TS-GUARDS-0218`,
  `TS-GUARDS-0261`, `TS-GUARDS-0312`

`components` (15)
- none (10): `TS-COMPONENT-0147`, `TS-COMPONENT-0193`, `TS-COMPONENT-0206`,
  `TS-COMPONENT-0229`, `TS-COMPONENT-0251`, `TS-COMPONENT-0268`,
  `TS-COMPONENT-0279`, `TS-LAYOUTS-0084`, `TS-LAYOUTS-0101`, `TS-LAYOUTS-0147`
- weaker (1): `TS-LAYOUTS-0216`
- blocked (4): `TS-LAYOUTS-0049`, `TS-LAYOUTS-0062`, `TS-LAYOUTS-0072`,
  `TS-LAYOUTS-0117`

`behaviors` (21)
- none (4): `TS-COMPONENT-0550`, `TS-COMPONENT-0628`, `TS-COMPONENT-0697`,
  `TS-COMPONENT-0774`
- weaker (6): `TS-COMPONENT-0403`, `TS-COMPONENT-0416`, `TS-COMPONENT-0591`,
  `TS-COMPONENT-0607`, `TS-COMPONENT-0654`, `TS-COMPONENT-0724`
- structural (5): `TS-ANALYSIS-0380`, `TS-COMPONENT-0357`,
  `TS-COMPONENT-0476`, `TS-COMPONENT-0514`, `TS-COMPONENT-0539`
- blocked (4): `TS-COMPONENT-0292`, `TS-COMPONENT-0498`,
  `TS-COMPONENT-0570`, `TS-COMPONENT-0676`
- near-exact (2): `TS-COMPONENT-0521`, `TS-COMPONENT-0763`

`events` (15)
- none (3): `TS-EVENTS-0375`, `TS-EVENTS-0387`, `TS-EVENTS-0399`
- weaker (10): `TS-EVENTS-0093`, `TS-EVENTS-0162`, `TS-EVENTS-0176`,
  `TS-EVENTS-0236`, `TS-EVENTS-0268`, `TS-EVENTS-0280`, `TS-EVENTS-0301`,
  `TS-EVENTS-0322`, `TS-EVENTS-0339`, `TS-EVENTS-0461`
- blocked (2): `TS-EVENTS-0108`, `TS-EVENTS-0188`

`protocol_limits` (40)
- none (40): `TS-META-0009`, `TS-META-0027`, `TS-PROTOCOL-0021`,
  `TS-PROTOCOL-0053`, `TS-PROTOCOL-0098`, `TS-PROTOCOL-0126`,
  `TS-PROTOCOL-0141`, `TS-PROTOCOL-0154`, `TS-PROTOCOL-0178`,
  `TS-PROTOCOL-0234`, `TS-PROTOCOL-0250`, `TS-PROTOCOL-0280`,
  `TS-PROTOCOL-0306`, `TS-PROTOCOL-0319`, `TS-PROTOCOL-0329`,
  `TS-PROTOCOL-0348`, `TS-PROTOCOL-0368`, `TS-PROTOCOL-0395`,
  `TS-PROTOCOL-0421`, `TS-PROTOCOL-0438`, `TS-PROTOCOL-0450`,
  `TS-PROTOCOL-0479`, `TS-PROTOCOL-0500`, `TS-PROTOCOL-0505`,
  `TS-PROTOCOL-0525`, `TS-PROTOCOL-0549`, `TS-PROTOCOL-0561`,
  `TS-PROTOCOL-0600`, `TS-PROTOCOL-0606`, `TS-PROTOCOL-0644`,
  `TS-PROTOCOL-0653`, `TS-SERVER-0094`, `TS-SERVER-0109`,
  `TS-SERVER-0129`, `TS-SERVER-0150`, `TS-SERVER-0171`,
  `TS-SERVER-0203`, `TS-SERVER-0216`, `TS-SERVER-0237`, `TS-SERVER-0267`

`messages_evidence` (8)
- none (8): `TS-EVIDENCE-0011`, `TS-EVIDENCE-0025`, `TS-EVIDENCE-0050`,
  `TS-MESSAGES-0005`, `TS-MESSAGES-0016`, `TS-MESSAGES-0025`,
  `TS-MESSAGES-0032`, `TS-MESSAGES-0039`

`scenario_matrix` (43)
- none (43): `TS-MATRIX-0092`; 33 `TS-MATRIX-0102-<scenario>-IN_PLACE_SHADOW`
  (`PAGE_BASIC`, `PAGE_EXPORT_EMPTY`, `PAGE_CJS`, `PAGE_ESM`, `PAGE_CLOSURE`,
  `COMPONENT_PROPERTIES`, `COMPONENT_MODULE`, `COMPONENT_JS`, `PROPERTIES_SPREAD`,
  `MULTI_REGISTRATION`, `JS_JSDOC_PRAGMA`, `JS_JSDOC_OFF`, `JS_CHECKJS`,
  `TS_NOCHECK`, `JS_NOCHECK`, `JS_SHEBANG_BOM`, `INTERFACE_ROOT`,
  `INTERFACE_ROOT_IMPORT`, `NULLABLE`, `CROSS_FILE_OVERLAY`, `ALIAS_PROJECT`,
  `COMPONENT_DATA_CROSS_FILE`, `CRLF_SCRIPT`, `DATA_SHORTHAND`,
  `JS_SOURCE_CHECK_OFF`, `JS_SOURCE_CHECK_INHERIT`, `METHODS_NONLITERAL`,
  `UNCHECKED_INDEX`, `NULLABLE_NO_STRICT_NULL`, `NO_IMPLICIT_THIS_OFF`,
  `JS_BOM_CHECK_OFF`, `JS_BOM_CHECK_INHERIT`, `UNICODE_CRLF`);
  `TS-MATRIX-0215`, `TS-MATRIX-0228`, `TS-MATRIX-0239`, `TS-MATRIX-0252`,
  `TS-MATRIX-0271`, `TS-MATRIX-0283`, `TS-MATRIX-0301`, `TS-MATRIX-0319`,
  `TS-MATRIX-0335`

`perf_budget` (1)
- structural (1): `TS-PERF-0012`

## 3. Work packages

| WP | Risk tier | Cases | Partitions | Primary test files | Production modules | Conflict set |
| --- | --- | ---: | --- | --- | --- | --- |
| WP1 | 1 | 15 | `definitions`, `hover_docs` | `native_parity/{definitions,hover_docs}.rs`; adapter `backend_tsgo/tests.rs` | `query/{definition,hover,documentation,type_info,diagnostics}.rs`; `native/session.rs`; `backend_tsgo/{hover,definition,diagnostics,mod,tests}.rs` | WP2, WP4, WP9 |
| WP2 | 1 | 8 | `completion` | `native_parity/completion.rs`; adapter `backend_tsgo/tests.rs` | `query/completion.rs`; `native/session.rs`; `backend_tsgo/{completion,mod,tests}.rs` | WP1, WP4, WP9 |
| WP3 | 2 | 21 | `symbols_edits` | `native_parity/symbols_edits.rs`; `semantics/{target,edit_plan}.rs` unit tests | `wxml-lsp/src/semantics/{target,edit_plan,handlers,facts}.rs`; `handlers/lifecycle.rs`; `server.rs` | WP9 (symbol scenarios only) |
| WP4 | 3 | 20 | `project_overlay` | `native_parity/project_overlay.rs`; adapter `backend_tsgo/tests.rs` | `backend_tsgo/{session,input,mod}.rs`; `native/{session,vfs}.rs`; `wxml-project/src/discovery.rs` | WP1, WP2, WP9 |
| WP5 | 4 | 31 | `shadow_rewrite`, `mapping_utf16` | `native_parity/{shadow_rewrite,mapping_utf16}.rs`; `wxml-backend-tsgo` shadow unit tests | `shadow/{rewrite,expression,mapping}.rs`; `native/strings.rs` | WP6 (shadow seam), WP9 (mapping scenarios) |
| WP6 | 4 | 32 | `loops`, `guards_js` | `native_parity/{loops,guards_js}.rs`; shadow unit tests | `shadow/{locals,guards}.rs`; read `shadow/layout.rs` | WP5 (shadow seam), WP1 (diagnostics), WP9 |
| WP7 | 3 | 51 | `components`, `behaviors`, `events` | `native_parity/{components,behaviors,events}.rs`; `wxml-backend-tsgo` script tests | `script/{behavior,properties,registration,events,symbols,scanner,snapshot}.rs`; `shadow/layout.rs` | WP1/WP2/WP4 on `native/session.rs`; WP9 |
| WP8 | 4 | 48 | `protocol_limits`, `messages_evidence` | `native_parity/{protocol_limits,messages_evidence}.rs` | `native/{framing,protocol,rpc,wire,handles,supervisor}.rs`; message sanitizer in `handlers/{diagnostics,hover}.rs` | WP4 on `backend_tsgo/input.rs`; WP9 |
| WP9 | 5 | 43 | `scenario_matrix` | `native_parity/scenario_matrix.rs`; `native_parity/harness.rs` | test-only: scenario table + frozen `backends/typescript/samples` | all (runs last) |
| WP10 | 5 | 1 | `perf_budget` | `native_parity/perf_budget.rs`; `wxml-backend-tsgo/tests/perf_budget.rs` | native measurement only | all (exclusive) |

### WP1 — Definition, hover and diagnostics contract (15 cases)

- Cases: `TS-DEFINITIONS-0013/0036/0052/0074/0093`;
  `TS-ANALYSIS-0033/0054/0078/0093/0123/0139/0161/0174/0184/0196`.
- Scope: WP1 also owns the cross-cutting diagnostic mapping contract. Domain diagnostic
  assertions stay in WP5/WP6/WP7 (mapping/components/guards/events) to keep file scopes
  disjoint.
- Test files: `crates/wxml-lsp/tests/native_parity/definitions.rs`,
  `crates/wxml-lsp/tests/native_parity/hover_docs.rs`;
  engine-level assertions in `crates/wxml-backend-tsgo/tests/real_engine.rs`
  or a new `f1_query.rs`; adapter-level assertions in
  `crates/wxml-lsp/src/backend_tsgo/tests.rs`.
- Production: `crates/wxml-backend-tsgo/src/query/{definition,hover,documentation,type_info,diagnostics}.rs`,
  `crates/wxml-backend-tsgo/src/native/session.rs` (type/documentation/definition/diagnostics rows),
  `crates/wxml-lsp/src/backend_tsgo/{hover,definition,diagnostics,mod,tests}.rs`.
- Prerequisites: Phase 0 harness change; pinned engine and `WXML_REQUIRE_REAL_ENGINE=1`;
  exact range helper for UTF-8/UTF-16; default-position and target-token fixtures;
  WP4 for `TS-ANALYSIS-0161` and `TS-ANALYSIS-0174` (read-policy/overlay).
- Exact-strength acceptance: assert the same observable fields as the TS case —
  hover type string (including `any`/`error` quality distinction), documentation bytes,
  `@deprecated`/JSDoc tag lines, diagnostic code/severity/range/message, definition target
  path plus UTF-8 range plus UTF-16 range, and `Supported`/`Partial`/`Unsupported`.
  `empty + Supported` may only mean a checked no-symbol; `Partial`/`Unsupported` may never
  be accepted where TS expects a value.
- Known blocked: `TS-DEFINITIONS-0013` and `TS-DEFINITIONS-0093` external `.d.ts`
  targets (C0 `SourceLocation` cannot represent an external asset; contract §9.1);
  `TS-DEFINITIONS-0036` generated members (no generated-member classification or overlay
  source mapping); no native QuickInfo (hover must remain a type + docs composition, so
  any case requiring display parts or links is blocked); Windows drive/UNC project roots
  are rejected.
- Fail-closed: missing engine, asset hash mismatch, protocol mismatch, or an empty
  evidence list is a hard failure. No skip, no `return`, no Node fallback. The comparator
  must have reverse self-tests for missing result, shifted range, lost documentation,
  out-of-bounds edit, and double-empty.

### WP2 — Completion correctness (8 cases)

- Cases: `TS-COMPLETION-0066/0078/0104/0127/0153/0175/0193/0207`.
- Test files: `crates/wxml-lsp/tests/native_parity/completion.rs`;
  engine-level `crates/wxml-backend-tsgo/tests/real_engine.rs` or `f1_query.rs`;
  adapter `crates/wxml-lsp/src/backend_tsgo/tests.rs`.
- Production: `crates/wxml-backend-tsgo/src/query/completion.rs`,
  `crates/wxml-backend-tsgo/src/native/session.rs` (completions row),
  `crates/wxml-lsp/src/backend_tsgo/{completion,mod,tests}.rs`.
- Prerequisites: WP1's engine seam or the existing completion row; script-fact data for
  `TS-COMPLETION-0078/0175/0193` from WP7.
- Exact-strength acceptance: exact item labels, kinds, `sortText`/order, prefix replacement
  range in UTF-16 and UTF-8, 256-item truncation, `isIncomplete`/`Partial`, malformed query
  rejection; `details` must be explicitly `Unsupported`/`Disabled` (no native
  `getCompletionEntryDetails`); `.`/EOF/illegal-boundary behavior must match the TS case.
- Known blocked: no native `textEdit` and no completion details; auto-import disabled by
  default; `TS-COMPLETION-0078/0175/0193` need page/component data and methods;
  `TS-COMPLETION-0207` needs an unavailable-layout degradation path.
- Fail-closed: an unsupported native completion is `Unsupported`, never an empty
  `Supported` list; missing engine is a hard failure; no fabricated edit range.

### WP3 — Symbols, rename and edit safety (21 cases)

- Cases: `TS-SYMBOLS-0061/0098/0116/0150/0164/0204/0215/0226/0242/0264/0286/0299/0312/0324/0336/0346/0367/0391/0406/0425/0439`.
- Test files: `crates/wxml-lsp/tests/native_parity/symbols_edits.rs`;
  pure target/grammar assertions in `crates/wxml-lsp/src/semantics/target.rs` and
  `edit_plan.rs` unit tests (contract §10.6 allows re-homing while keeping case IDs);
  port the fake-backend pattern from `crates/wxml-lsp/tests/semantic_tools.rs`
  (`symbols_marker`, `symbol_response`, `apply_edits`) into `tests/common/` or the
  partition file; integration-test binaries do not share private helpers.
- Production: `crates/wxml-lsp/src/semantics/{target,edit_plan,handlers,facts,tokens}.rs`,
  `crates/wxml-lsp/src/handlers/lifecycle.rs` (rename capability), and
  `crates/wxml-lsp/src/server.rs` (`documentChanges` capability).
  Do not route these cases through `backend_tsgo` symbols; that adapter path is
  explicitly `Unsupported`.
- Prerequisites: the WXML symbol/edit-plan contract; client `WorkspaceEdit.documentChanges`;
  closed-file version checks; the two mandatory native edit gates
  `native_component_property_edit_applies_and_reparses` and
  `native_rejects_cross_namespace_and_dynamic_edits`.
- Exact-strength acceptance: exact byte ranges and `expected_text`; `complete=false`
  excludes edits; unsafe shapes (shorthand, computed, spread, dynamic `this.data`,
  other receiver) produce zero edits with a typed reason; cross-namespace/dynamic rename
  returns `-32803`, no edit, unchanged sources; applied edits reparse and typecheck;
  nested `setData` edits only the intended segment; UTF-8 offsets are bytes, not UTF-16.
- Known blocked: native rename has no `getRenameInfo`/`getEditsForRename`/`rename`
  (contract §9.9, F1 blocker B-09); conservative rename equivalence is unverified;
  missing-handler insertion and cross-file edit composition remain contract work;
  Windows edit paths are unverified. `TS-SYMBOLS-0116/0150/0346` (insertions) and
  `TS-SYMBOLS-0098/0312/0439` (nested/renamed `setData`) are the highest-risk cases.
- Fail-closed: any incomplete evidence, stale handle, version mismatch, unmappable
  shadow position, or failed reparse returns zero edits. `Unsupported`/`Partial` plus an
  edit is a test failure. No text-search fallback and no partial `WorkspaceEdit`.

### WP4 — Project overlay, unsaved text and session identity (20 cases)

- Cases: `TS-ANALYSIS-0272/0287/0297/0315/0332/0349/0358/0371`;
  `TS-COMPILER-0035/0050/0063/0086/0095/0132`;
  `TS-OVERLAY-0023/0036/0047/0055/0076`; `TS-PROBES-0176`.
- Test files: `crates/wxml-lsp/tests/native_parity/project_overlay.rs`;
  adapter `crates/wxml-lsp/src/backend_tsgo/tests.rs`;
  real-engine `crates/wxml-backend-tsgo/tests/real_engine.rs`.
- Production: `crates/wxml-lsp/src/backend_tsgo/{session,input,mod}.rs`,
  `crates/wxml-backend-tsgo/src/native/{session,vfs}.rs`,
  `crates/wxml-project/src/discovery.rs`.
- Prerequisites: WP1's session seam; VFS read logging; revision/epoch barriers;
  `TS-ANALYSIS-0161/0174` and `TS-OVERLAY-0055` share the same session code.
- Exact-strength acceptance: exact `viewRevision`, `backendFileVersion`, insertion ids,
  mount/source split and module kind; unsaved text wins over disk; closing the overlay
  restores original content; an imported model file propagates; project key reuse only
  on the same file set/options, fresh project on any key change; no project-file disk read
  outside the snapshot; missing snapshot is `Unavailable`, never read from disk; stale
  results are dropped.
- Known blocked: `TS-COMPILER-0132` is the D3 implementation gate (custom `typeRoots`
  must still load pinned mini-program typings; no evidence yet, not a contract change);
  Windows roots; `TS-OVERLAY-0055` needs the native session/overlay equivalent of the old
  pool path (assert propagation, not the old transport).
- Fail-closed: a missing project/view/snapshot is `Unavailable` and starts no session;
  a read-policy violation is a hard failure; an empty success is not accepted for a
  degraded overlay.

### WP5 — Shadow rewrite, mapping and UTF-16 (31 cases)

- Cases: `TS-REWRITE-0051/0078/0088/0101/0118/0127/0156/0183/0190/0197/0207/0224/0239/0250/0260/0285`;
  `TS-MAPPING-0078/0091/0110/0145/0188/0208/0223/0239/0261/0306`;
  `TS-REWRITE-0309/0349/0368/0378/0413`.
- Test files: `crates/wxml-lsp/tests/native_parity/shadow_rewrite.rs`,
  `crates/wxml-lsp/tests/native_parity/mapping_utf16.rs`;
  unit tests next to `shadow/{rewrite,mapping,expression}.rs`;
  real-engine mapping cases in `crates/wxml-backend-tsgo/tests/real_engine.rs`.
- Production: `crates/wxml-backend-tsgo/src/shadow/{rewrite,expression,mapping}.rs`,
  `crates/wxml-backend-tsgo/src/native/strings.rs`.
- Prerequisites: the shadow rewrite/mapping unit contracts; a diagnostic publish path for
  `TS-MAPPING-0188/0208/0306` and `TS-REWRITE-0197` (WP1/WP8); frozen LF checkout.
- Exact-strength acceptance: exact rewritten text for every expression shape; template
  locals never prefixed; string/template text verbatim; object keys/numbers not default
  targets; rightmost target offsets; comments/strings rejected; depth and 1/4 MiB
  linearity; byte/UTF-16 boundaries, mid-surrogate and mid-codepoint rejection, BOM and
  CRLF handling; unattributable spans; rewritten roots keep hover/diagnostic mapping.
- Known blocked: `TS-REWRITE-0197` exact WXML byte range and
  `TS-MAPPING-0188/0208/0306` published-diagnostic mapping need a frozen WXML shadow
  reverse-mapping contract for diagnostics (only references/definition reverse mapping is
  frozen); `TS-REWRITE-0378` is perf-adjacent and may need the WP10 measurement style.
- Fail-closed: any unmappable span is `Partial` and excluded from edits; a fabricated
  range or a text-search substitute is a failure; missing engine is a hard failure.

### WP6 — Shadow guards, locals and loops (32 cases)

- Cases: `TS-ANALYSIS-0205/0226/0243/0255`;
  `TS-LOCALS-0063/0089/0144/0261/0288/0312/0343`;
  `TS-GUARDS-0075/0097/0128/0152/0174/0201/0218/0261/0302/0312/0321/0329`;
  `TS-PROBES-0027/0048/0059/0064/0080/0098/0135/0153/0169`.
- Test files: `crates/wxml-lsp/tests/native_parity/loops.rs`,
  `crates/wxml-lsp/tests/native_parity/guards_js.rs`;
  unit tests next to `shadow/{locals,guards}.rs`.
- Production: `crates/wxml-backend-tsgo/src/shadow/{locals,guards}.rs`;
  read `shadow/layout.rs`; pragma scanning in `shadow/guards.rs` or the existing scanner.
- Prerequisites: WP1 diagnostic mapping for the blocked guard cases; a probe-health table;
  locale/indent fixtures.
- Exact-strength acceptance: exact open/close text, closer order (innermost first),
  array-order level grouping, TDZ/self-shadow behavior, typed element/key end-to-end,
  JavaScript locals as `any` + `Partial`; pragma plain/BOM/hashbang handling, last pragma
  wins, out-of-leading-block `@ts-nocheck` ignored, checkJs off/inherit, probe health
  rejects `any` roots and intersecting diagnostics, duplicate helper and module
  classification flip.
- Known blocked: `TS-GUARDS-0097/0218/0261` need checker narrowing per branch;
  `TS-GUARDS-0201` needs diagnostics `Partial` + reason + no fabricated 2339;
  `TS-GUARDS-0312` needs a published typo diagnostic; `TS-LOCALS-0343` needs diagnostics
  `Partial` through a locals-only request.
- Fail-closed: `Partial` must carry a typed reason; no fabricated 2339; missing engine or
  missing probe asset is a hard failure; no skip for JS pragma cases.

### WP7 — Components, behaviors and events (51 cases)

- Cases: `TS-COMPONENT-0147/0193/0206/0229/0251/0268/0279/0292/0357/0403/0416/0476/0498/0514/0521/0539/0550/0570/0591/0607/0628/0654/0676/0697/0724/0763/0774`;
  `TS-ANALYSIS-0380`;
  `TS-LAYOUTS-0049/0062/0072/0084/0101/0117/0147/0216`;
  `TS-EVENTS-0093/0108/0162/0176/0188/0236/0268/0280/0301/0322/0339/0375/0387/0399/0461`.
- Test files: `crates/wxml-lsp/tests/native_parity/components.rs`,
  `behaviors.rs`, `events.rs`;
  unit/integration tests in `crates/wxml-backend-tsgo/tests/script_{behavior,properties,registration,events}.rs`.
- Production: `crates/wxml-backend-tsgo/src/script/{behavior,properties,registration,events,symbols,scanner,snapshot}.rs`;
  `crates/wxml-backend-tsgo/src/shadow/layout.rs`.
- Prerequisites: a complete behavior graph (inline, imported, re-exported, CommonJS,
  spread, unresolved); component binding facts; event binding classifier for all seven
  forms; WP1 diagnostics for published messages.
- Exact-strength acceptance: per-tag component binding diagnostics and skipped-check
  counts; no path leakage; behavior data/properties precedence; behavior cycle/factory
  degradation with reason; literal const types; `any` for spread/dynamic members with a
  reason; all event binding forms map to the same handler; handler file/signature/docs
  bytes; missing-handler suppression only where proven; reserved keys/`__wxml` ignored;
  event strings count against the request byte budget; early-return and full analyze
  paths return `eventResults` without changing fragment results.
- Known blocked: `TS-COMPONENT-0292/0498/0570/0676` binding checks;
  `TS-EVENTS-0108/0188` full template chain/behavior file id;
  `TS-LAYOUTS-0049/0062/0072/0117` checker probe diagnostics;
  `TS-ANALYSIS-0380` checker spread; `TS-COMPONENT-0357/0476/0514/0539` structural.
  These are implementation-blocked, not contract-blocked.
- Fail-closed: no fabricated missing-handler diagnostic; unresolved behavior is `Partial`
  with no invented 2339; missing engine is a hard failure; no path in user-visible text.

### WP8 — Protocol, codec, recovery, security and evidence (48 cases)

- Cases: all `TS-META-0009/0027`; `TS-PROTOCOL-0021/0053/0098/0126/0141/0154/0178/0234/0250/0280/0306/0319/0329/0348/0368/0395/0421/0438/0450/0479/0500/0505/0525/0549/0561/0600/0606/0644/0653`;
  `TS-SERVER-0094/0109/0129/0150/0171/0203/0216/0237/0267`;
  `TS-EVIDENCE-0011/0025/0050`; `TS-MESSAGES-0005/0016/0025/0032/0039`.
- Test files: `crates/wxml-lsp/tests/native_parity/protocol_limits.rs`,
  `crates/wxml-lsp/tests/native_parity/messages_evidence.rs`;
  unit tests in `native/{framing,protocol,rpc,wire,handles,supervisor}.rs`.
- Production: `crates/wxml-backend-tsgo/src/native/{framing,protocol,rpc,wire,handles,supervisor}.rs`;
  message sanitizer at the protocol edge (`server.rs`,
  `handlers/{diagnostics,hover}.rs`; locate the existing filter, do not add a new layer;
  coordinate with WP3 on `server.rs`);
  `crates/wxml-lsp/src/backend_tsgo/input.rs` (coordinate with WP4).
- Prerequisites: native protocol whitelist and frame limits; JSON-RPC server harness;
  path sanitizer; pinned asset/license metadata for `TS-META`. The message-edge
  sanitizer in `server.rs`/`handlers` starts only after WP3 releases `server.rs`, or the
  integrator applies it.
- Exact-strength acceptance: exact JSON-RPC codes/messages, unknown-method/bad-JSON/bad-params
  behavior, notification no-response, oversized-line rejection without killing the server,
  ten concurrent requests on one stream, frame length/unknown kind/cyclic ref/oversize
  rejection, request byte budget, limits merge, enum/protocol version validation, option
  digest independent of checkout path, path escape/symlink/read-policy denial, write/emit
  refusal, sanitizer POSIX/Windows path forms, duplicated-intersection collapse, long
  message truncation, no absolute paths in committed evidence.
- Known blocked: `TS-META-0027` is a `rewrite` to an F0 version-pin test;
  `TS-SERVER-0237/0267` packaged/symlink layouts need the delivery package;
  `TS-EVIDENCE-0025/0050` need the evidence tree; `TS-PROTOCOL-0450` needs WP4's alias
  project; Windows drive/UNC paths are unverified; `TS-PROTOCOL-0500` needs the
  compiler-target spelling table (not a file-path case).
- Fail-closed: malformed/oversized/unknown input is rejected, never silently dropped;
  path escape is `missing/false/empty/identity` per callback, never a host read; missing
  engine/asset is a hard failure; no Node fallback.

### WP9 — Scenario matrix (43 cases)

- Cases: `TS-MATRIX-0092`; 33 `TS-MATRIX-0102-<scenario>-IN_PLACE_SHADOW`;
  `TS-MATRIX-0215/0228/0239/0252/0271/0283/0301/0319/0335`.
- Test files: `crates/wxml-lsp/tests/native_parity/scenario_matrix.rs`;
  runner extension in `crates/wxml-lsp/tests/native_parity/harness.rs`.
- Production: test-only. Scenario table ported from
  `backends/typescript/src/scenarios.ts`; read the frozen
  `backends/typescript/samples` tree directly. Do not add files to `tests/fixtures` or
  `crates/wxml-lsp/tests/fixtures` (both are hash-pinned).
- Prerequisites: WP1, WP2, WP4, WP5, WP6, WP7; in-place-shadow expectations only
  (Candidate B is retired); a scenario table with `id`, sample dir, script file,
  expected status, expected introduced diagnostics and reason.
- Exact-strength acceptance: per scenario, exact hover/diagnostics/definition/completion
  DTOs and statuses, plus introduced-diagnostic codes and reason text; a failure names the
  scenario and field. `TS-MATRIX-0092` must be re-scoped to in-place only, because the
  symbol-bridge candidate is retired.
- Known blocked: all 43 until the semantic packages land; `TS-MATRIX-0271` component
  properties union needs WP7; `TS-MATRIX-0335` `noImplicitThis:false` needs the probe
  degradation path; `TS-MATRIX-0102-ALIAS_PROJECT` and
  `TS-MATRIX-0102-COMPONENT_DATA_CROSS_FILE` need WP4.
- Fail-closed: no scenario may pass with `Unavailable`/`Blocked`; a missing sample is a
  hard failure; do not copy samples into a pinned tree.

### WP10 — Performance budget (1 case)

- Case: `TS-PERF-0012` (structural, `mechanism_replaced`).
- Test files: `crates/wxml-lsp/tests/native_parity/perf_budget.rs`;
  `crates/wxml-backend-tsgo/tests/perf_budget.rs`.
- Production: native measurement only; no production behavior change.
- Prerequisites: exclusive quiet machine, `--test-threads=1`, pinned engine,
  `WXML_REQUIRE_REAL_ENGINE=1`, 200 samples and 5 warmups.
- Exact-strength acceptance: reproduce the TS observable budgets that remain meaningful —
  cold start `<8000 ms`, hover p50 `<500 ms`, wide-40 workload — or record an explicit
  integrator decision that the mechanism is replaced and the native p95 rows
  (type 200 ms, diagnostics 500 ms, completion 200 ms, queued cancel 100 ms,
  snapshot-cycle growth) are the accepted substitute. The serial `perf.test.ts` remains
  the authoritative old gate until that decision is recorded.
- Known blocked: native gate is opt-in and currently excludes cold start from samples;
  static completion p95 is `BLOCKED` (no static path); running-cancellation is `BLOCKED`
  (no cancel token). This case cannot be exact without a native cold-start/hover-p50
  contract.
- Fail-closed: missing engine is a hard failure in gate mode; a `--test-threads` > 1 run
  is invalid evidence; do not mark green from a single sample or a skipped run.

## 4. Conflict matrix and seam rules

`backend_tsgo/{mod,session,tests}.rs` is the adapter seam. WP1, WP2 and WP4 all need it
and **must not run together**. WP9 also needs adapter e2e and runs only after them.

| Shared file | Owner | Other users | Rule |
| --- | --- | --- | --- |
| `crates/wxml-lsp/tests/native_parity.rs` | integrator | all | Phase 0 only; workers never edit |
| `crates/wxml-lsp/tests/native_parity/ledger.rs` | integrator | all | no worker edits; status counts frozen |
| `crates/wxml-lsp/tests/native_parity/harness.rs` | integrator | WP9 | comparator/runner changes only by integrator |
| `crates/wxml-lsp/src/backend_tsgo/mod.rs` | adapter token | WP1, WP2, WP4, WP9 | one worker at a time |
| `crates/wxml-lsp/src/backend_tsgo/session.rs` | WP4 | WP1, WP2 | serialize behind adapter token |
| `crates/wxml-lsp/src/backend_tsgo/tests.rs` | adapter token | WP1, WP2, WP4 | one worker at a time |
| `crates/wxml-backend-tsgo/src/native/session.rs` | engine token | WP1, WP2, WP4, WP7, WP9 | one worker may edit; others request |
| `crates/wxml-lsp/src/backend_tsgo/input.rs` | WP4 | WP8 | WP8 waits or uses native-only changes |
| `crates/wxml-lsp/src/server.rs` | WP3 | WP8 (message edge) | serialize WP3 and WP8; no concurrent edit |
| `crates/wxml-backend-tsgo/src/shadow/mod.rs` | integrator | WP5, WP6 | export-only changes by integrator |
| `crates/wxml-backend-tsgo/src/input.rs` | WP4 | WP5, WP6 | read-only for WP5/WP6 |
| `crates/wxml-backend-tsgo/src/script/snapshot.rs` | WP7 | WP4 (read) | no concurrent edits |
| `Cargo.toml` / lockfile | integrator | all | no worker edits |
| `tests/fixtures`, `crates/wxml-lsp/tests/fixtures` | frozen | all | no new files; hash gate fails |

Additional rules:

- Only one worker holds the **adapter token** (`backend_tsgo/*`) at a time.
- Only one worker holds the **engine token** (`native/session.rs`) at a time.
- WP5 and WP6 share the shadow seam: they may run in parallel only if neither edits
  `shadow/mod.rs` or `input.rs`; otherwise serialize.
- Any package that needs a new fixture must use in-memory text or a new unpinned
  directory; it must not write to a frozen tree. If a frozen fixture must change, the
  integrator updates the ledger hash and records the content reason in the same change.

## 5. Priority by risk

| Tier | Packages | Why |
| --- | --- | --- |
| 1 | WP1, WP2 | Definition/hover/diagnostics/completion correctness; most reused by other packages |
| 2 | WP3 | Symbols/rename/edit safety; highest data-loss risk if edits are unsafe |
| 3 | WP4, WP7 | Overlay/unsaved and component/event/behavior correctness; both feed WP9 |
| 4 | WP5, WP6, WP8 | UTF-16/mapping/recovery/security; broad but lower semantic coupling |
| 5 | WP9, WP10 | Integration matrix and perf; depend on everything above |

## 6. Subagent scheduling

Phase 0 (integrator, one change series, before any worker):

1. De-skeletonize the parity gate in `native_parity.rs`: remove the "all
   `MIGRATED_CASES` must be empty" assertion. Keep the synthetic `P3-T-SKELETON` case
   `Unavailable` for every partition, add a check that each `MIGRATED_CASES` name is a
   planned `target_rust_test` function for that partition, and keep
   `PARTITIONS.len() == 16`, routed counts, and the exact `MIGRATED_CASES` ↔ `#[test]`
   equality. A real case must never be able to turn `Unavailable` into a pass.
2. Add a shared comparator/evidence helper (new `native_parity/compare.rs` or equivalent)
   with the five reverse self-tests.
3. Decide and document the LSP test seam for `backend_tsgo` (public `#[doc(hidden)]`
   constructor, test feature, or re-home to `backend_tsgo/tests.rs`). Do not expose
   production behavior just for tests.
4. Re-run the existing `native_parity` suite (25 tests at the evidence head) and the
   ledger validator at `60f4c6c`; record the result, not a green claim.

| Slot | Start | Replacement order | Owns (disjoint) | Cannot overlap with |
| --- | --- | --- | --- | --- |
| W1 (adapter) | WP1 | WP2 → WP4 → WP9 integration | `backend_tsgo/{mod,session,tests}.rs`, `query/*`, `native/session.rs`; `input.rs` only after WP4 starts | WP2, WP4, WP9 at the same time |
| W2 (shadow) | WP5 | WP6 → WP10 | `shadow/{rewrite,expression,mapping}`, `native/strings.rs` | WP6, WP10 |
| W3 (semantics) | WP3 | WP9 symbol-scenario support; WP1 review | `semantics/*`, `handlers/lifecycle.rs`, `server.rs` | WP9 code edits |
| W4 (script) | WP7 | WP9 script-scenario support; WP3 review | `script/*`, `shadow/layout.rs` | WP1/WP2/WP4 if `native/session.rs` must change |
| W5 (protocol) | WP8 (native protocol first; message-edge sanitizer only after WP3 releases `server.rs`) | WP10 support; WP9 protocol-scenario support | `native/{framing,protocol,rpc,wire,handles,supervisor}` | WP4 on `input.rs`; WP3 on `server.rs` |

- **Max concurrent implementation workers: 5.** At most one holds the adapter token and
  at most one holds the engine token; W4/W5 are read-only on `native/session.rs` until
  the token is free.
- When a worker finishes and its next package is blocked by a conflict or dependency, it
  stops and reports; it must not pick a conflicting package. It may instead take a review
  or fixture-inventory task with no production edits.
- W1 is the critical path (WP1 → WP2 → WP4 → WP9). Keep its queue full; do not assign
  W1 review work that delays the adapter lane.
- W2 and W4/W5 may run in parallel with W1 only while they do not edit the adapter or
  engine seam.
- Build contention: give each worker its own `CARGO_TARGET_DIR` or use a build lock; do
  not run workspace-wide `cargo fmt` while another worker is editing. Format only owned
  files, then run the full formatter at handoff.
- WP10 runs alone after all measurement-affecting packages; never beside W2 or W5.

Handoff per case:

- One worker owns one package file at a time; at handoff, the integrator reruns the exact
  gate and checks the ledger target name.
- The worker supplies the evidence bundle (case ID, TS source line, Rust fully-qualified
  test, fixture, engine identity, expected/actual fields, RED and GREEN commands,
  comparator ID). The integrator records it in `migration_evidence` but **does not**
  change `status` from `planned` unless the case is exact and independently reviewed.
- No worker edits `docs/plans/tsgo-test-ledger.json`, `tsgo-test-ledger.md`, the test
  contract, or the ADR.

## 7. Cases that need a native capability or contract change first

| Case(s) | Exact blocker |
| --- | --- |
| `TS-DEFINITIONS-0013`, `TS-DEFINITIONS-0093` | External `.d.ts` target: C0 `SourceLocation` cannot represent an external asset; contract §9.1 requires `Partial("external declaration outside project")` or an approved external-location DTO. No approved external-location DTO is recorded. |
| `TS-DEFINITIONS-0036` | Generated member mapping back to `properties.<name>`: native definition evidence has no generated-member classification and no overlay source reverse mapping. |
| `TS-DEFINITIONS-0052` | "Every query definition is classified" needs external/generated/dropped classification. Same gap as 0013/0036. |
| `TS-SYMBOLS-0098`, `TS-SYMBOLS-0116`, `TS-SYMBOLS-0150`, `TS-SYMBOLS-0346`, `TS-SYMBOLS-0439` | Conservative rename/insertion edits: native has no `getRenameInfo`/`getEditsForRename`/`rename`; F1 blocker B-09 says rename completeness has no native flag and old/new equivalence is unverified. Needs the frozen rename + `EditPlan` composition (including missing-handler insertion) before these can be exact. |
| `TS-REWRITE-0197`, `TS-MAPPING-0188`, `TS-MAPPING-0208`, `TS-MAPPING-0306` | WXML shadow reverse mapping for published diagnostics is not a frozen contract. The contract covers references/definition reverse mapping, not diagnostic ranges, suppressed sets, or one expression mounted twice. |
| `TS-PERF-0012` | TS cold start `<8000 ms`, hover p50 `<500 ms` and wide-40 are not in the native performance contract; the native gate measures different p95 rows and is opt-in. Needs a native cold-start/hover-p50 contract or an integrator replacement decision. |
| `TS-MATRIX-0092` | "Every scenario runs both candidates" includes the retired symbol-bridge candidate. The integrator retired 36 cases overall (33 matrix + 2 matrix top-level + 1 protocol), so this assertion cannot be exact until re-scoped to in-place shadow only. |
| No case ID (platform) | Windows drive-letter/UNC project roots: native input rejects them and the contract says they are unverified. No ledger case may claim Windows-root support until the native path contract and assets change. |

Not contract-blocked, but currently evidence-blocked (unblock by implementation):
`TS-COMPILER-0132` (custom `typeRoots` must keep the pinned mini-program declarations;
D3 is an implementation gate), `TS-COMPONENT-0292/0498/0570/0676`,
`TS-LAYOUTS-0049/0062/0072/0117`,
`TS-EVENTS-0108/0188`,
`TS-GUARDS-0097/0201/0218/0261/0312`,
`TS-LOCALS-0343`, and the 5 structural behavior cases.
These become exact through WP1/WP7/WP6 work; they do not need a new native API.

## 8. Global fail-closed gate and definition of done

Every package runs under the same gate:

```sh
# engine-required mode; a missing engine is a failure, not a skip
TSGO_BIN=/path/to/pinned/7.1.0-beta/lib/tsc \
  WXML_REQUIRE_REAL_ENGINE=1 \
  cargo test -p wxml-lsp --test native_parity \
    hover_docs::case_analysis_0033_analyze_returns_hover_diagnostic_and_definition_for_a_page \
    -- --exact
```

The exact package and target come from the case's `target_rust_test`; pure shadow/codec
cases may run under `wxml-backend-tsgo` instead.

- No `#[ignore]`, no `return` on a missing `dist/server.js`, no optional `tsc` branch,
  no Node fallback.
- `ParityRunner` must receive non-empty `AssertionEvidence` and the pinned identity.
  `Unavailable`/`Blocked` is never a pass; an empty `Passed` is a failure.
- The comparator must pass its reverse self-tests: missing result, shifted range, lost
  documentation, out-of-bounds edit, and double-empty all fail.
- A case is not "migrated" until the same observable fields as the TS case are asserted
  at the lowest layer that can observe them, and the adapter/e2e layer passes.
- No status promotion by workers. The integrator may record evidence while the case stays
  `planned`; `green` requires exact parity plus independent review, and this plan
  produces no green claims.

Definition of done for the backlog:

- [ ] Every one of the 270 cases belongs to exactly one WP (the table sums to 270).
- [ ] Phase 0 gate change is landed and the 16 partition skeletons are no longer
      hard-coded empty.
- [ ] WP1/WP2/WP4 never ran concurrently; the conflict matrix is respected.
- [ ] Each completed case has an evidence bundle and a fail-closed command.
- [ ] No frozen fixture hash changed without an integrator reason and ledger update.
- [ ] No `green`/`reviewed`/`closed` was written by a worker.
