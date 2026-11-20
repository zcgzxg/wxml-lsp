# tsgo 测试台账 wave-6 回填（2026-11-20）

## 0. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 记录/验证 HEAD：`1d5222eef18904ee056d7da97de3b011feb65cc8`
- 记录日期：2026-11-20
- 范围：只改测试台账（`docs/plans/tsgo-test-ledger.json`）、Markdown 台账
  （`docs/plans/2026-11-17-tsgo-test-ledger.md`）、contract 计数
  （`docs/reference/tsgo-test-contract.md`）、ledger validator
  （`crates/wxml-lsp/tests/native_parity/ledger.rs`）、证据索引
  （`docs/evidence/README.md`）和本证据文档。未修改生产代码、测试源码、Cargo、
  delivery、Node 或默认后端；未提交。
- 记录期间工作树有并行会话的未提交改动（native edit/CLI 相关
  `crates/wxml-lsp/src/backend_tsgo/*`、`crates/wxml-lsp/tests/cli.rs`、
  `crates/wxml-lsp/tests/native_parity/*` 等）和未跟踪的
  `docs/evidence/2026-11-20-tsgo-f3-14-zed-smoke.md`、
  `scripts/f3-zed-smoke.sh`。本次没有修改它们，也不把它们的测试当作 wave-6
  证据；`native_parity` 的实际计数因此高于 HEAD-only 基线。
- `gate_green` 与 case `green` 保持两个状态轴：本轮没有因为任何 gate 变绿而自动
  提升 case，也没有修改 gate 状态。

**结论：没有新的 exact case。** case 级保持 **44 `green` / 259 `planned` /
36 retired**，339 总数不变。新增 9 个带证据的 planned（6 weaker、3 blocked），
`assessed_cases` 145 → 154，`weaker` 74 → 80，`blocked` 17 → 20，
`structural` 7，`near_exact` 3，无 Rust 证据的 planned 158 → 149。
另有 4 个既有用例追加 wave-6 测试（`TS-ANALYSIS-0297`、`TS-COMPLETION-0104`、
`TS-SYMBOLS-0116`、`TS-EVENTS-0108`），并修正 `TS-COMPLETION-0078` 的陈旧行号。

---

## 1. 盘点范围

本轮优先核对 wave-5 之后或 wave-5 未映射的能力：

| 提交 | 能力 | 本轮结论 |
| --- | --- | --- |
| `814ca5e` | script diagnostics routing、expression completion | 旧 TS 套件没有独立 `.ts/.js` diagnostics case；只作为 `TS-ANALYSIS-0297` 的 weaker 增量证据，不新增 case。 |
| `3220362` | event handler hover | `TS-EVENTS-0108` 已由 definition exact 证据为 green；追加 IT-15 hover E2E，不重复提升。 |
| `4fd45bd` | E2E-21 non-literal | `the_e2e21_non_literal_page_fixture_is_unsupported_not_unavailable` 是 template-level `Unsupported`，`TS-EVENTS-0176` 要 event-result `Partial + reason + no diagnostic`；不等价，保持 weaker。 |
| `304aa20`/`4140e9b` | native symbols、symbol query/this-scope 拆分 | `TS-SYMBOLS-0061/0150/0215/0226/0312` 已 green；`TS-SYMBOLS-0299/0425` 记录 blocked，其余强度不变。 |
| `8b6ca14` | PERF-01 / STABILITY-01 | `native_q2_performance_benchmark` 继续作为 `TS-PERF-0012` structural 证据；stability gate 没有 TS case。 |
| `04fa0fe` | E2E-20 component property bindings | 新增 component-binding pass 证据：6 weaker + 1 blocked，没有 exact。 |
| `2f072a6` | native gate wave-5 | 只更新 gate 证据；`gate_green` 不自动等同 case green。 |
| `28c5e18` | F3 kill window | delivery/进程证据，没有 TS oracle case。 |

新增 Rust 测试文件/区域：`crates/wxml-lsp/src/backend_tsgo/tests.rs`、
`crates/wxml-lsp/src/backend_tsgo/component_bindings.rs`、
`crates/wxml-lsp/src/handlers/diagnostics*`、
`crates/wxml-lsp/src/semantics/event_definition.rs`、
`crates/wxml-lsp/tests/native_parity/native_engine_real.rs`、
`crates/wxml-backend-tsgo/tests/script_symbol_facts.rs`、
`crates/wxml-lsp/tests/native_parity/native_measure.rs`。其中 document symbols
没有 TS oracle，不新增 case ID；`native_measure` 是 gate 支撑代码，不是 case test。

---

## 2. 新增 exact → `green`（0）

候选逐条核对后都不满足“同一可观察语义、断言等价”的门槛，因此没有提升：

- **`TS-COMPONENT-0147`**：native 只断言单个 `title` mismatch 的表达式范围、错误
  严重度、消息和 Partial reason；TS 还要求 valid/valid-literal/valid-boolean/
  undeclared 全清洁，union/literal/boolean/array/object 分支，以及 8 个 skipped
  usage 和精确的 skipped reason（含最多三对 tag.attribute）。
- **`TS-COMPONENT-0193`**：native 有一个 clean 双属性样例，但没有 TS 的三属性
  fixture、`probeProblems=[]`、`components=[{tag, diagnostics: []}]` 和
  `skipped=0` 断言。
- **`TS-COMPONENT-0206`**：literal/boolean 属性范围和消息已精确，但 TS 的
  no-mustache fixture、Supported 状态和 `skipped=0` 未断言；native 测试还同时
  期望两个诊断。
- **`TS-COMPONENT-0229`**：Array/Object 是保守比较；`User | undefined`、
  `Item[] | undefined`、absent 绑定和 `skipped=0` 未覆盖。
- **`TS-COMPONENT-0251`**：missing component/incomplete properties 都 Partial，
  但没有 `Component({ data })` 无 properties 的 fixture，也没有
  `response.components=undefined`。
- **`TS-COMPONENT-0268`**：只断言 Partial reason path-free，没有遍历每条
  component diagnostic 验证不含 `/`。
- **`TS-SYMBOLS-0299/0425`**：native 行为与 TS 期望相反（见 §4），不是 exact。
- **`TS-PERF-0012`**：TS 覆盖两个 layout（含 symbol-bridge）和 wide-40；native
  只有原位 shadow 和 native gate 指标，继续 structural。

---

## 3. 新增 weaker 证据（6）

全部 `parity=weaker`、`green_eligible=false`、`status_applied=planned`、
`target_rust_test_status=proposed`。逐条 note 在 JSON
`cases[].migration_evidence.note`。

| Case | Rust 证据（path:line） | 缺口 |
| --- | --- | --- |
| `TS-COMPONENT-0147` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1233<br> crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1663<br> crates/wxml-lsp/src/backend_tsgo/tests.rs:6439 | 单个 mismatch 的范围/严重度/消息精确；per-tag 矩阵、union/literal/array/object 和 8 个 skipped pair 未覆盖。 |
| `TS-COMPONENT-0193` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1206 | 两个匹配绑定 clean；TS 的 `probeProblems`、`components` 数组和 `skipped=0` 未断言。 |
| `TS-COMPONENT-0206` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1289 | literal/boolean attribute range 和消息精确；no-mustache、Supported、`skipped=0` 未断言。 |
| `TS-COMPONENT-0229` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1612 | Array/Object 保守比较；optional `undefined` 和 absent 绑定未覆盖。 |
| `TS-COMPONENT-0251` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1365 | missing/incomplete properties 都是 Partial；无 `Component({ data })` 无 properties fixture。 |
| `TS-COMPONENT-0268` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:6476<br> crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1233 | Partial reason path-free；没有遍历所有 diagnostic 的 path-free 断言。 |

---

## 4. 新增 blocked 证据（3）

| Case | Rust 证据（path:line） | 阻塞原因 |
| --- | --- | --- |
| `TS-COMPONENT-0279` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:6476<br> crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1663 | native 只保留原位 shadow；symbol-bridge layout 已退役，无法复现该 layout 的 path-free 消息断言。 |
| `TS-SYMBOLS-0299` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:344<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:397 | native 把 `this.data` 解构视为 Partial 且不给 declaration；TS 要求 renamed destructuring `complete=true` + declaration。 |
| `TS-SYMBOLS-0425` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:438 | native 让另一个 script 的 setData 阻断 completeness，与 TS 的 per-page isolation 相反，无法断言 `complete=true`。 |

---

## 5. 既有用例追加 wave-6 证据（4）

状态/parity 不变，只在 `migration_evidence.rust_tests` 追加：

| Case | parity | wave-6 Rust 测试 |
| --- | --- | --- |
| `TS-ANALYSIS-0297` | weaker |  crates/wxml-lsp/src/backend_tsgo/tests.rs:4959<br> crates/wxml-lsp/src/backend_tsgo/tests.rs:4987 |
| `TS-COMPLETION-0104` | weaker |  crates/wxml-lsp/src/backend_tsgo/tests.rs:4418 |
| `TS-SYMBOLS-0116` | blocked |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:127 |
| `TS-EVENTS-0108` | exact/green |  crates/wxml-lsp/tests/full_feature_matrix_editing.rs:891 |

说明：

- `TS-ANALYSIS-0297` 的新增测试只覆盖 diagnostics 降级和 path-free reason；
  hover/definition 仍 Supported 这一点没有在同一测试中断言，所以保持 weaker。
- `TS-COMPLETION-0104` 的未闭合成员表达式补全断言精确 replace range，但状态仍是
  Partial（native completion API 没有 per-entry documentation）。
- `TS-SYMBOLS-0116` 新增的是组件 missing-handler 的 `methods` 包装插入；
  `observerPath` 仍未实现，blocked 不变。
- `TS-EVENTS-0108` 的新增 IT-15 是真实 LSP session 的 hover E2E，覆盖
  bindtap/catchtap/capture-bind/capture-catch/mut-bind 和 CJK handler 的精确
  WXML range；原 definition exact 证据仍是主证据。
- `TS-COMPLETION-0078` 仅修正既有测试行号到
   crates/wxml-lsp/src/backend_tsgo/tests.rs:4793 和
   crates/wxml-lsp/src/backend_tsgo/tests.rs:4418，不改变强度。

---

## 6. 计数变化

| 项 | wave-6 前 | wave-6 后 |
| --- | ---: | ---: |
| `green` | 44 | 44 |
| `planned` | 259 | 259 |
| `retired-with-decision` | 36 | 36 |
| 有 `migration_evidence` 的 case | 145 | 154 |
| `exact` | 44 | 44 |
| `near_exact` | 3 | 3 |
| `weaker` | 74 | 80 |
| `structural` | 7 | 7 |
| `blocked` | 17 | 20 |
| 无 Rust 证据的 planned | 158 | 149 |
| 339 总数 | 339 | 339 |

分区变化：

- `components`：有证据 9→16、planned 有证据 5→12、weaker 1→7、blocked 4→5、
  无证据 10→3。
- `symbols_edits`：有证据 19→21、planned 有证据 14→16、blocked 1→3、
  无证据 2→0。
- 其余分区不变。

---

## 7. 未映射的本轮原生面

- **script diagnostics routing**（`814ca5e`）：旧 TS 套件没有独立 `.ts/.js`
  diagnostics case；`script_diagnostics_*` 是 native-only 行为，不新增 case ID，
  也不把 template diagnostics 或 `TS-MATRIX-*` 当作同义证据。
- **event handler hover**（`3220362`）：`TS-EVENTS-0108` 已 green；IT-15 作为
  追加证据，不重复提升。
- **E2E-21 non-literal**（`4fd45bd`）：template-level `Unsupported` 与 event-result
  `Partial` 不等价；不新增 case。
- **native symbols**（`304aa20`/`4140e9b`）：只新增 0299/0425 blocked；
  document symbols 没有 TS oracle，不新增 case。
- **PERF-01 / STABILITY-01**（`8b6ca14`）：perf 继续 structural；stability 没有
  TS case，只记录在 gate 证据。
- **E2E-20 component bindings**（`04fa0fe`）：新增 6 weaker + 1 blocked；
  不把 gate 的 Supported 当作 `TS-COMPONENT-0193` 的 exact。
- **native gate wave-5**（`2f072a6`）：`gate_green` 不自动等同 case green。
- **F3 kill**（`28c5e18`）：delivery/进程证据，没有 TS oracle case。
- **Zed smoke**：不是测试迁移 case，不进入 ledger。

---

## 8. 验证

| 命令 | 结果 |
| --- | --- |
| `jq empty docs/plans/tsgo-test-ledger.json` | exit 0 |
| `cargo test -p wxml-lsp --test native_parity` | **37 passed / 0 failed / 26 ignored**（任务/wave-5 的 HEAD-only 期望 36 passed / 23 ignored；多出的测试来自并行会话未提交的 native edit 改动，不属于 wave-6） |
| `cargo fmt --all -- --check` | exit 0 |
| `git diff --check` | exit 0 |

限制：

1. 本轮没有新增 exact case；44 green 的既有结论未重新 review。
2. 9 个新证据都是 weaker 或 blocked，不改变 259 planned。
3. 工作树不是独占的；并行 native-edit 改动会让 `native_parity` 计数高于
   HEAD-only 基线，本文档不把它们的测试当作 case 证据。
4. 未做独立 review；所有 `green` 仍是 `reviewed` 之前的状态。
