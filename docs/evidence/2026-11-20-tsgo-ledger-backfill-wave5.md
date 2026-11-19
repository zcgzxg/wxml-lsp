# tsgo 测试台账 wave-5 回填（2026-11-20）

## 0. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务指定边界 HEAD：`375cde24a61141f2b122bdf6ca9c23fbfc0fb1a6`
- 记录/验证时 HEAD：`28c5e18eebfc1fe7c5df14e9a3655404185065a2`
- 记录日期：2026-11-20
- 范围：只改测试台账（`docs/plans/tsgo-test-ledger.json`）、Markdown 台账
  （`docs/plans/2026-11-17-tsgo-test-ledger.md`）、contract 计数
  （`docs/reference/tsgo-test-contract.md`）、ledger validator
  （`crates/wxml-lsp/tests/native_parity/ledger.rs`）和本证据文档。未修改生产代码、
  测试源码、Cargo、delivery、Node 或默认后端；未提交。
- 只映射 `375cde2` 及之前的 Rust 测试。任务给出之后，并行会话又提交了
  `04fa0fe`（E2E-20 component binding diagnostics）、`2f072a6`（final native gate）
  和 `28c5e18`（F3 kill window）。这些新提交不在本次边界内，没有映射；
  `E2E-20`、`f3_kill_window` 和 Zed smoke 均未进入 ledger。
- 记录/验证期间工作树还有并行会话的未提交改动
  （`crates/wxml-lsp/src/backend_tsgo/mod.rs`、`session.rs`）和未跟踪的
  `scripts/f3-zed-smoke.sh`；本次没有修改它们，也不把它们当作证据。
- `gate_green` 与 case `green` 保持两个状态轴：本轮没有因为任何 gate 变绿而自动
  提升 case，也没有修改 gate 状态。

**结论：case 级从 36 `green` / 267 `planned` / 36 retired 推进到 44 `green` /
259 `planned` / 36 retired。** 新增 8 个 exact green、26 个新证据用例
（17 weaker、1 blocked、1 structural，加上 7 个新 exact），另有 14 个既有
weaker/structural/blocked 用例追加本轮 Rust 测试。`near_exact` 仍为 3，
无 Rust 证据的 planned 从 184 降到 158。

---

## 1. 新增 exact → `green`（8）

全部 `parity=exact`、`green_eligible=true`、`status_applied=green`、
`target_rust_test_status=migrated_green`。逐条 note 在 JSON
`cases[].migration_evidence.note`。

| Case | 旧 TS 断言 | Rust 证据（全限定测试名 / 文件行） | 强度 |
| --- | --- | --- | --- |
| `TS-COMPLETION-0127` | typed-prefix root completion 列出 data roots，replace 覆盖整个前缀 | `wxml_lsp::backend_tsgo::tests::root_completion_with_a_cjk_prefix_replaces_only_the_prefix`<br> crates/wxml-lsp/src/backend_tsgo/tests.rs:4356 | **exact**：断言精确 item 列表、前缀 replace 范围、Supported 和 0 次 native session。 |
| `TS-COMPLETION-0153` | 空 mustache 列出 data keys | `wxml_lsp::backend_tsgo::tests::root_completion_uses_the_script_overlay_without_a_native_session`<br> crates/wxml-lsp/src/backend_tsgo/tests.rs:4315 | **exact**：空 mustache 的精确 root 列表、零宽 replace、Supported、0 次 session。 |
| `TS-EVENTS-0108` | 7 种绑定形式都映射到同一 handler 的精确 definition range | `wxml_lsp::semantics::event_definition::tests::every_event_binding_form_hovers_the_exact_handler_token`<br> crates/wxml-lsp/src/semantics/event_definition.rs:857 | **exact**：7 种形式加 `bind:my-event`/无引号形式，逐个断言 Supported 和精确 declaration token/file。 |
| `TS-SYMBOLS-0061` | dataPath 返回 declaration、`this.data` reference 和 setData segment，全部精确字节 | `script_symbol_facts::page_data_path_reads_references_and_setdata_segments`<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:68<br>`native_engine_real::native_symbols_smoke`<br> crates/wxml-lsp/tests/native_parity/native_engine_real.rs:2700 | **exact（组合）**：单元测试覆盖 Supported/complete、declaration/read 起点、setData path/segment；required-native smoke 补齐 declaration/read/setData 精确字节位置和 rename 编辑。 |
| `TS-SYMBOLS-0150` | 无 methods 的 Component 生成 `methods: {` + `onSave()` 插入 | `script_symbol_facts::component_missing_handler_wraps_a_new_methods_object`<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:127 | **exact**：complete=true、options 闭合括号上的零宽插入、文本含 `methods: {` 和 `onSave()`。 |
| `TS-SYMBOLS-0215` | 歧义/不完整 owner 表没有 insertion | `script_symbol_facts::opaque_or_dynamic_owner_tables_never_get_an_insertion`<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:480 | **exact**：opaque 和 spread owner 均 complete=false、无 declaration、无 insertion。 |
| `TS-SYMBOLS-0226` | UTF-8 offset 是字节范围，不是 UTF-16 index | `script_symbol_facts::alias_registration_and_cjk_emoji_ranges_are_exact`<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:161 | **exact**：CJK declaration 的 UTF-8 start/end 精确，并额外断言 UTF-16 start。 |
| `TS-SYMBOLS-0312` | `other.setData` receiver 不编辑、不声明 complete | `script_symbol_facts::setdata_computed_keys_are_partial_but_literal_keys_remain`<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:370 | **exact**：`other.setData` 分支 complete=false、occurrences 为空；同测试保留 literal key 对照。 |

### 1.1 为什么 `TS-SYMBOLS-0061` 是组合证据

非 ignored 单元测试 `page_data_path_reads_references_and_setdata_segments` 已经断言：

- `status=Supported`、`complete=true`；
- declaration 的 `expected_text` 与 UTF-8 start/end；
- `this.data.user.name` 的 read 起点；
- setData occurrence 的 `path=["user","name"]`、`segment=1`。

它没有断言 occurrence 的精确 start/end。`native_symbols_smoke` 在 required-native
job 中通过 LSP references/rename 断言 declaration、read、setData segment 的精确位置，
两者并集覆盖 TS case 的全部断言。该 smoke 仍是 `#[ignore]`，默认
`cargo test -p wxml-lsp --test native_parity` 不运行它；这是有意的透明限制，不是
用 `gate_green` 代替 case green。

---

## 2. 新增非 exact 证据（19 个新 case）

### 2.1 weaker（17）

| Case | Rust 证据 | 缺口 |
| --- | --- | --- |
| `TS-COMPLETION-0078` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:4452<br> crates/wxml-lsp/src/backend_tsgo/tests.rs:4276 | member 标签/range 匹配，但 native status 是 Partial，TS 是 Supported。 |
| `TS-SYMBOLS-0098` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:209 | root segment/text 匹配；缺 exact occurrence start/end 和 sibling fields。 |
| `TS-SYMBOLS-0164` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:546 | cross-file behavior declaration 有路径；TS 的 shared data object 场景未覆盖。 |
| `TS-SYMBOLS-0204` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:318<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:344<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:370 | dynamic data/setData 都是 Partial 且有 reason；没有单个测试覆盖 TS 的完整 fixture。 |
| `TS-SYMBOLS-0242` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:614<br> crates/wxml-lsp/src/backend_tsgo/tests.rs:2424 | malformed query 返回 Unsupported，不是 TS 的 RequestError 通道。 |
| `TS-SYMBOLS-0264` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:525 | declaration/read 存在，但 computed access 让结果变 Partial，且没有断言精确 read range。 |
| `TS-SYMBOLS-0286` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:397 | destructuring 是 Partial+reason；没有通过 edit plan 断言“never edited”。 |
| `TS-SYMBOLS-0324` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:344<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:318 | dynamic this.data/spread 是 Partial；TS 的 rest/Object.keys 形态未全部覆盖。 |
| `TS-SYMBOLS-0336` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:2446 | Partial/reason/0 session 匹配；没有断言 `fields` 为空。 |
| `TS-SYMBOLS-0346` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:108 | insertion 精确；缺 applied-text 的 TS1005 parse 断言。 |
| `TS-SYMBOLS-0367` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:504<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:242 | DataRoots 的 reserved 名称覆盖 data/property/method；没有覆盖两个 TS dataPath 查询。 |
| `TS-SYMBOLS-0391` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:438 | another-script setData 导致 incomplete+reason；缺 TS 的 status/zero-occurrence 断言。 |
| `TS-SYMBOLS-0406` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:397 | alias/destructuring 是 Partial+reason；没有断言 alias 上的动态访问。 |
| `TS-SYMBOLS-0439` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:209 | nested segment/text 匹配；缺 nested occurrence 的 exact start/end。 |
| `TS-OVERLAY-0023` |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:511 | unsaved model type 跨两个模板生效；没有断言 JSDoc 替换，fixture 使用 literal type。 |
| `TS-OVERLAY-0047` |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:511 | close 恢复 disk string；但只在 opt-in ignored job 中运行，fixture 与 TS 不同。 |
| `TS-OVERLAY-0055` |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:511 | imported model overlay 跨 view/restart；TS 的 number/JSDoc overlay 不完全相同。 |

### 2.2 blocked（1）

| Case | Rust 证据 | 缺口 |
| --- | --- | --- |
| `TS-SYMBOLS-0116` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:108<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:573 | missing-handler insertion 精确；但 `observerPath` 未实现、component dataPath 仍是 Partial，TS 要求 complete=true 且有 observerPath occurrence。 |

### 2.3 structural（1）

| Case | Rust 证据 | 缺口 |
| --- | --- | --- |
| `TS-REWRITE-0413` |  crates/wxml-backend-tsgo/tests/shadow_guard_ranges.rs:112<br> crates/wxml-backend-tsgo/tests/shadow_guard_ranges.rs:196<br> crates/wxml-backend-tsgo/tests/shadow_guard_ranges.rs:353 | guard wrapper/value span 能映射回精确原始字节；但没有通过 checker 运行 hover/diagnostics。 |

---

## 3. 既有非 exact 用例追加 wave-5 证据（14）

状态/parity 不变，只在 `migration_evidence.rust_tests` 追加：

| Case | parity | wave-5 Rust 测试 |
| --- | --- | --- |
| `TS-COMPONENT-0357` | structural |  crates/wxml-backend-tsgo/tests/script_optional_types.rs:269 |
| `TS-COMPONENT-0403` | weaker |  crates/wxml-backend-tsgo/tests/script_optional_types.rs:176 |
| `TS-COMPONENT-0416` | weaker |  crates/wxml-backend-tsgo/tests/script_optional_types.rs:196 |
| `TS-COMPONENT-0607` | weaker |  crates/wxml-backend-tsgo/tests/script_optional_types.rs:221 |
| `TS-EVENTS-0093` | weaker |  crates/wxml-lsp/src/semantics/event_definition.rs:894 |
| `TS-EVENTS-0162` | weaker |  crates/wxml-lsp/src/semantics/event_diagnostics.rs:524<br> crates/wxml-lsp/src/semantics/event_definition.rs:712 |
| `TS-EVENTS-0176` | weaker |  crates/wxml-lsp/src/semantics/event_diagnostics.rs:540<br> crates/wxml-lsp/src/backend_tsgo/tests.rs:3807 |
| `TS-EVENTS-0188` | blocked |  crates/wxml-lsp/src/semantics/event_diagnostics.rs:383<br> crates/wxml-lsp/src/semantics/event_definition.rs:910 |
| `TS-EVENTS-0236` | weaker |  crates/wxml-lsp/src/semantics/event_diagnostics.rs:466 |
| `TS-EVENTS-0268` | weaker |  crates/wxml-lsp/src/semantics/event_diagnostics.rs:524<br> crates/wxml-lsp/src/semantics/event_diagnostics.rs:540 |
| `TS-EVENTS-0280` | weaker |  crates/wxml-lsp/src/semantics/event_diagnostics.rs:479<br> crates/wxml-lsp/src/semantics/event_definition.rs:1011 |
| `TS-EVENTS-0301` | weaker |  crates/wxml-lsp/src/semantics/event_diagnostics.rs:452 |
| `TS-EVENTS-0339` | weaker |  crates/wxml-lsp/src/semantics/event_diagnostics.rs:512<br> crates/wxml-lsp/src/semantics/event_diagnostics.rs:554 |
| `TS-PERF-0012` | structural |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:3131 |

---

## 4. 计数变化

| 项 | wave-5 前 | wave-5 后 |
| --- | ---: | ---: |
| `green` | 36 | 44 |
| `planned` | 267 | 259 |
| `retired-with-decision` | 36 | 36 |
| 有 `migration_evidence` 的 case | 119 | 145 |
| `exact` | 36 | 44 |
| `near_exact` | 3 | 3 |
| `weaker` | 57 | 74 |
| `structural` | 6 | 7 |
| `blocked` | 17 | 17 |
| 无 Rust 证据的 planned | 184 | 158 |
| 339 总数 | 339 | 339 |

分区变化：

- `completion`：green 3→5、有证据 5→8、无证据 4→1。
- `events`：green 9→10、exact 9→10、blocked 2→1。
- `symbols_edits`：有证据 0→19、green 0→5、weaker 0→13、blocked 0→1、
  无证据 21→2。
- `mapping_utf16`：有证据 14→15、structural 0→1、无证据 7→6。
- `project_overlay`：有证据 3→6、weaker 3→6、无证据 17→14。
- `perf_budget`：有证据 1→1（只追加测试，不改变计数）。

其余分区不变。

---

## 5. 仍 BLOCKED 的 17 个 case

| Case | 阻塞原因 |
| --- | --- |
| `TS-COMPONENT-0292` | component-binding diagnostics 未对 behavior properties 运行；Rust 只断言 declaration facts。 |
| `TS-COMPONENT-0498` | TS 断言 hover any + behavior reason；Rust 只断言 spread completeness facts。 |
| `TS-COMPONENT-0570` | TS 断言 behaviorIncomplete/Partial 且不伪造 2339；Rust 只断言 Partial 和不发明字段。 |
| `TS-COMPONENT-0676` | TS 断言 component-binding diagnostic + skipped details；Rust 只断言 own-property overlay。 |
| `TS-DEFINITIONS-0036` | generated-member kind/source range 未实现；native definition 没有 overlay source mapping。 |
| `TS-EVENTS-0188` | behavior definition 的 fileId/range 已覆盖，但 signature/docs 仍未断言。 |
| `TS-GUARDS-0097` | 需要 checker 的 if/elif/else union narrowing；Rust 只固定渲染结构。 |
| `TS-GUARDS-0201` | 需要 diagnostics Partial + reason + 不伪造 2339；Rust 只固定结构和 Partial reason。 |
| `TS-GUARDS-0218` | 需要 checker 的 `&&`/`||`/`!` truthiness narrowing；Rust 只固定渲染结构。 |
| `TS-GUARDS-0261` | 需要嵌套 loop/guard narrowing；Rust 只固定 local capture 和 closer 顺序。 |
| `TS-GUARDS-0312` | pragma/typo diagnostic 未发布；Rust 只检查 pragma 应用和 note。 |
| `TS-LAYOUTS-0049` | 需要 checker 对 probe member 的 diagnostics；Rust 只固定 layout/probe 位置。 |
| `TS-LAYOUTS-0062` | 需要 2353 diagnostic；Rust 固定的是 methods 内 probe 位置。 |
| `TS-LAYOUTS-0072` | 需要 checker 对 probe 的 diagnostics；Rust 只固定 Component methods 内位置。 |
| `TS-LAYOUTS-0117` | 需要 JS JSDoc overlay 的 hover type/documentation + 空 diagnostics；Rust 只固定 pragma/JSDoc layout。 |
| `TS-LOCALS-0343` | 需要 locals-only request 的 diagnostics Partial + typed reason；Rust 只固定 layout status/reason。 |
| `TS-SYMBOLS-0116` | `observerPath` occurrence 未实现；component dataPath 仍为 Partial。 |

---

## 6. 未映射的本轮原生面

- `8014739` standalone script diagnostics：旧 TS 套件没有独立的 `.ts/.js` diagnostics
  case；`script_diagnostics_*` 是 native-only 行为，不新增 case ID，也不把
  `TS-MATRIX-*`/`TS-ANALYSIS-*` 当作同义证据。
- `8b6ca14` perf/stability：`native_q2_performance_benchmark` 作为 structural 证据追加到
  `TS-PERF-0012`；`native_ten_round_lifecycle_stability` 没有对应 TS case，只保留 gate
  证据。
- `c5ee251`/`375cde2` gate wave 3/4：只使用其中可独立核对的
  `native_symbols_smoke` 和 `native_non_owner_overlay_replay_survives_restart` 断言；
  没有把 `gate_green` 自动等同为 case green。
- `04fa0fe` E2E-20、`28c5e18` f3_kill_window、Zed smoke：不在任务边界内，未映射。

---

## 7. 验证

| 命令 | 结果 |
| --- | --- |
| `jq empty docs/plans/tsgo-test-ledger.json` | exit 0 |
| `cargo test -p wxml-lsp --test native_parity` | **36 passed / 0 failed / 23 ignored** |
| `cargo fmt --all -- --check` | exit 0 |
| `git diff --check` | exit 0 |

限制：

1. 8 个 exact 中有 1 个（`TS-SYMBOLS-0061`）依赖 `#[ignore]` 的 required-native
   symbols smoke 补齐精确字节位置；默认 validator 不运行该 smoke。
2. 26 个新证据中的 overlay 3 个和 1 个 symbols smoke 也只在 opt-in required-native
   job 中运行；它们保持 planned，不影响默认测试。
3. 本轮不是 G5/P6 关闭：仍有 259 个 planned、17 个 blocked、158 个无证据。
4. 未做独立 review；所有 `green` 仍是 `reviewed` 之前的状态。
