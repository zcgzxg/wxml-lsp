# tsgo wave-11 blocked followup：6 个可提升 case 的测试与证据（2026-11-20）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务给定起点 HEAD：`74669c3`（`feat(tsgo): add message sanitization and probe health`）。
  本轮工作期间并行 session 提交了 `1c89338`
  （`test(tsgo): retire legacy cases and backfill wave11 parity`）。测试与首轮证据在
  `1c89338` 工作树上完成，随后按 `1c89338` 台账回填；本轮未 commit。
- 第一轮只改测试/证据（生产行为未变）：
  - ` crates/wxml-lsp/src/backend_tsgo/component_bindings.rs`
    （只在 `#[cfg(test)] mod tests` 内新增/扩展测试）
  - ` crates/wxml-backend-tsgo/tests/script_events.rs`
  - ` crates/wxml-backend-tsgo/tests/script_behavior.rs`
  - ` crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs`
- 第二轮（本文件顶部计数对应的回填）只改台账/validator/台账 md/contract/证据：
  - `docs/plans/tsgo-test-ledger.json`
  - `crates/wxml-lsp/tests/native_parity/ledger.rs`（仅计数常量与注释）
  - `docs/plans/2026-11-17-tsgo-test-ledger.md`、`docs/reference/tsgo-test-contract.md`
  - 本文件
- 未修改 README、生产代码、`crates/wxml-lsp/tests/native_parity.rs`、矩阵文件、
  fixture 冻结树；未新增 fixture 文件；未 commit、未 review。

**结论先行（已回填台账）：**

| Case | 最终结果 | 测试 | 位置 |
| --- | --- | --- | --- |
| `TS-COMPONENT-0292` | **green / exact** | `case_component_0292_behavior_properties_and_kebab_camel_bindings_are_checked` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1918 |
| `TS-EVENTS-0188` | **planned / near_exact**（差 `fileId`→`path` 改名） | `case_events_0188_imported_behavior_handler_matches_the_oracle_path_and_signature` |  crates/wxml-backend-tsgo/tests/script_events.rs:583 |
| `TS-COMPONENT-0193` | **planned / weaker** | `fully_checkable_component_request_stays_supported`（扩展） |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1851 |
| `TS-GUARDS-0201` | **planned / weaker** | `case_guards_0201_an_unparseable_guard_does_not_fabricate_a_diagnostic_and_reports_partial`（扩展） |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:466 |
| `TS-LOCALS-0343` | **planned / weaker** | `case_locals_0343_a_locals_only_request_still_degrades_to_partial_with_a_typed_reason`（扩展） |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:317 |
| `TS-COMPONENT-0498` | **planned / weaker** | 脚本事实 `case_component_0498_spread_and_dynamic_data_members_stay_partial_and_any` + build 层 `case_component_0498_build_reports_a_behavior_reason_and_mounts_dynamic` |  crates/wxml-backend-tsgo/tests/script_behavior.rs:236 与 :617 |

- 最终计数：**green 83 / planned 218 / retired-with-decision 38**；
  `exact` 83、`near_exact` 3、`weaker` 100、`structural` 7、`blocked` 16、
  `assessed_cases` 209、无证据 planned 92。
- 本轮 **0 个 blocked**；`TS-COMPONENT-0292` 是唯一 green 增量（+1）、
  `TS-EVENTS-0188` 进入 near_exact（+1）、4 个 case 进入 weaker（+4）。
- 如果 owner/ledger 接受 `fileId` → `EventDefinition.path` 的改名等价，
  `TS-EVENTS-0188` 可再升 exact，green 再 +1；本文件按 P5 口径保守记 near_exact。
- 发现并已另派修复一个独立生产解析缺口：`behavior_unit` 原先不接受
  `Behavior({...})` 选项对象后的尾随逗号；后续 slice 已精确放行 `}` `,` `)`，
  fixture 恢复真实逗号并补回归（见 §3.2），不改变本 case 的 near-exact 判定。

---

## 1. 判定口径

沿用 `docs/evidence/2026-11-20-tsgo-wave11-blocked-audit.md` §1 与
`docs/evidence/2026-11-20-tsgo-near-exact-three.md` §1：

- **exact**：TS oracle 的每条可观察断言都能在同一 Rust 测试里找到等价断言，
  且来自同一可观察层。
- **near_exact**：语义相同，只差字段改名或 typed error 替代 TS error code；
  是否把改名视为等价由 ledger/owner 裁决，测试 worker 不单方面升 exact。
- **weaker**：同层/相邻层可证明的核心结果（status/reason/range/text）已断言，
  但 oracle 的剩余断言没有 native 对应面。
- 本轮的 3-6 号 case 只补现有可证明的 weaker 断言，不升 exact。

---

## 2. 逐 case

### 2.1 `TS-COMPONENT-0292` → exact

**TS oracle**：`backends/typescript/test/componentBindings.test.ts:292`
（`behavior properties and kebab/camel bindings are checked`）。

**新测试**：
`case_component_0292_behavior_properties_and_kebab_camel_bindings_are_checked`
（ crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1918）。

fixture 对齐 TS：

- component：`behavior-card.ts`，`Component({ behaviors: [Behavior({ properties: { ... } })] })`；
- page：`pageTitle: 'hello'`、`total: 3`、`flag: true`、`list: string[]`；
- usages：`behavior-ok`、`behavior-bad`、`behavior-loose`、`behavior-missing-required`。

| TS 断言 | native 断言 | 结果 |
| --- | --- | --- |
| `results.get("behavior-ok") == []` | 0 诊断、`status.is_none()`、4 次真实 query、5 个属性中未声明的 `unknown` 不 query | 通过 |
| `bad.length == 3` | 诊断列表逐条等于 3 条（排序后整体相等） | 通过 |
| `card-title` 期望 string、got number | 逐字 message，range 为 `total` 表达式 range | 通过 |
| `score` 期望 `string \| number`、got boolean | 逐字 message，range 为 `flag` 表达式 range | 通过 |
| `defaulted` 期望 number、got string literal | 逐字 message，range 为 attribute range | 通过 |
| `results.has("behavior-loose") == false` | 0 诊断、`Partial`，不返回 clean 结果 | 通过 |
| `skipped == 1` 且 `skippedDetails == ["behavior-loose.loose"]` | reason 精确等于 `some component bindings could not be resolved and were not checked (behavior-loose.loose)` | 通过 |
| `results.has("behavior-missing-required") == false` | 0 属性、0 query、0 诊断、`status.is_none()` | 通过 |

**为什么可以 exact**：

- 生产 `build_properties` 已把 behavior properties 并入 `analysis.properties`，
  `analyze_component` 用同一 overlay 跑绑定检查（ crates/wxml-backend-tsgo/src/script/properties.rs:241、
    crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:620）。
- native 没有 TS 的 per-tag `results` 数组；按 usage 逐个断言与已 exact 的
  `TS-COMPONENT-0147` 先例一致（ crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1727）。
- 所有值都没有从实现反推：message 文本、属性名、skip detail 来自 TS oracle 原文。

**残余**：无。`required: true` 的缺失属性诊断不在本 oracle 内，测试没有额外发明。

**建议 ledger**：`parity=exact`、`green_eligible=true`、`status_recommendation=green`、
`status_applied=green`、`status=green`、`target_rust_test_status=migrated_green`；
`implementation_head` 必须是真正加入本测试的 commit（不能用 `74669c3`/`1c89338`），
`rust_tests` 指向
 crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1918
（test `case_component_0292_behavior_properties_and_kebab_camel_bindings_are_checked`）。

### 2.2 `TS-EVENTS-0188` → near_exact

**TS oracle**：`backends/typescript/test/events.test.ts:188`
（`component behaviors may define handlers and suppress missing diagnostics`）。

**新测试**：
`case_events_0188_imported_behavior_handler_matches_the_oracle_path_and_signature`
（ crates/wxml-backend-tsgo/tests/script_events.rs:583）。

fixture 已对齐：handler `fromBehavior`，behavior 文件 `src/behaviors/base.ts`，
JSDoc `来自 behavior`；`index.ts` 用相对 import `./src/behaviors/base`，解析结果与 TS 的
`fileId` 同值。

| TS 断言 | native 断言 | 结果 |
| --- | --- | --- |
| `state.status == "supported"` | `resolution.status.is_supported()` | 通过 |
| `definition.fileId == "src/behaviors/base.ts"` | `EventDefinition.path == "src/behaviors/base.ts"`（改名：`fileId` → `path`） | 通过 |
| `signature` 匹配 `/fromBehavior/` | `signature` 含 `fromBehavior` 与 `detail` | 通过 |
| `documentation` 匹配 `/来自 behavior/` | `documentation == Some("来自 behavior")` | 通过 |
| — | `definition.range` 精确等于 `fromBehavior` token 的 UTF-8 字节 range | 通过（附加 pin） |

**为什么不是 exact**：唯一差异是 TS 的 `definition.fileId` 在 native DTO 里叫
`EventDefinition.path`。按 near-exact 定义这是 renamed field；是否等价由 ledger 裁决。
如果 owner 接受，本测试已具备 exact 的全部同层断言，可以直接升 exact。

**fixture 说明（生产发现，已修复）**：TS 的 `behaviorText` 在 `Behavior({...})` 选项对象后带尾随逗号：

```
Behavior({
  methods: {
    /** 来自 behavior */
    fromBehavior(event: { detail: { x: number } }) {},
  },
})
```

原 `behavior_unit` 只接受 `close + 1 == end` 或 `close + 2 == end && ')'`
（ crates/wxml-backend-tsgo/src/script/behavior.rs:411）；带尾随逗号时
`close + 2 == end` 不成立，behavior 解析返回 None，事件表变成
`behavior methods are partially resolved`。后续生产 slice 已增加精确形态
`}` `,` `)`（ crates/wxml-backend-tsgo/src/script/behavior.rs:414），并补回归；
本 fixture 已恢复真实尾随逗号，四个可观察断言不变。剩余唯一差异仍是
`definition.fileId` 到 `EventDefinition.path` 的改名。

**建议 ledger**：`parity=near_exact`、`green_eligible=false`、
`status_recommendation=green`、`status_applied=planned`、`status=planned`、
`target_rust_test_status=proposed`；`rust_tests` 首选新测试
 crates/wxml-backend-tsgo/tests/script_events.rs:583
（test `case_events_0188_imported_behavior_handler_matches_the_oracle_path_and_signature`），
并保留旧 inline 测试
 crates/wxml-backend-tsgo/tests/script_events.rs:562
（test `case_events_0188_only_inline_behavior_methods_resolve`）作为辅证。
如果 ledger 接受 `fileId`→`path`，则改为 exact/green。

### 2.3 `TS-COMPONENT-0193` → weaker

**TS oracle**：`backends/typescript/test/componentBindings.test.ts:193`。

扩展后的测试：`fully_checkable_component_request_stays_supported`
（ crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1851）。

新增断言：

- 3 次 query 的 path 全部是 owning script；
- 每次 query 的 `Utf16Position` 精确等于对应 mount 的 `target_start_utf16`
  在 derived script 中的位置，并按模板顺序排列；
- 原有 3 个 fragment、0 诊断、`status.is_none()`（Supported）保持。

| TS 断言 | native 证据 | 状态 |
| --- | --- | --- |
| `result.ok` | 测试拿到结果，无错误 | 覆盖 |
| `states.diagnostics.status == "supported"` | component pass `status=None` | 覆盖（同层） |
| `components == [{tag:"valid",diagnostics:[]}]` | 0 诊断 + 3 次 checked query；无 per-tag 数组 | 按 0147 先例覆盖 |
| `skipped == 0` | `status=None`（Partial 只在 partial/skipped>0 时产生） | 覆盖 |
| `probeProblems == []` | **无 native 对应面** | 缺 |

**残余**：`probeProblems` 是 Node probe helper/root 健康数组；native 的
`query/probe_health.rs` 决策表尚未接入 hover/diagnostics/status，本轮没有伪造它。
adapter overall diagnostics status 也没有在这里单独断言（这是 component pass 层）。
**保持 weaker，不升 exact。**

**建议 ledger**：`parity=weaker`、`green_eligible=false`、
`status_recommendation=planned`、`status_applied=planned`；
`rust_tests[0].line` 1771 → 1851；`matching_root_binding_is_clean` 行号 1523 → 1603。

### 2.4 `TS-GUARDS-0201` → weaker

**TS oracle**：`backends/typescript/test/guards.test.ts:201`。

扩展后的测试：`case_guards_0201_an_unparseable_guard_does_not_fabricate_a_diagnostic_and_reports_partial`
（ crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:466）。

新增断言：

- reason **精确等于**
  `some template bindings or guards could not be resolved and were not checked`
  （原来是 `contains`）；
- `notes` 含
  `unresolved `wx:for` bindings or `wx:if` guards are mounted as `any`/unnarrowed and not checked`；
- `// @ts-ignore` 在 unnarrowed mount 的上一行，二者之间只有空白；
- 原有 `!contains("if (")`、`BuildStatus::Partial` 保持。

| TS 断言 | native 证据 | 状态 |
| --- | --- | --- |
| `result.ok` | build 成功返回 | 覆盖 |
| `build.unresolvedGuards == 1` | 内部计数未进入 `BuiltShadow` | 缺（不能精确断言 1） |
| `states.diagnostics.status == "partial"` | `BuildStatus::Partial` | 相邻层，weaker |
| reason 匹配 `/could not be resolved and were not checked/` | 精确同串 | 覆盖 |
| `diagnostics == []` | `@ts-ignore` mount 机制证明不伪造诊断；未跑真实 checker | 机制级覆盖 |
| `fragments[0].diagnostics == []` | native 是 flat diagnostics，无 per-fragment DTO | 缺 |

**保持 weaker。** `unresolvedGuards` 计数和 adapter diagnostics/ per-fragment DTO 没有
native 对应面；不把机制级证据冒充 checker 空结果。

**建议 ledger**：`parity=weaker`、`green_eligible=false`、
`status_recommendation=planned`、`status_applied=planned`；
`rust_tests[0].line` 448 → 466。

### 2.5 `TS-LOCALS-0343` → weaker

**TS oracle**：`backends/typescript/test/locals.test.ts:343`。

扩展后的测试：`case_locals_0343_a_locals_only_request_still_degrades_to_partial_with_a_typed_reason`
（ crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:317）。

新增断言：

- reason 精确等于同一个稳定串；
- `notes` 含同一个 unresolved 说明；
- `const item = undefined as any;` fallback 保持。

| TS 断言 | native 证据 | 状态 |
| --- | --- | --- |
| `result.ok` | build 成功返回 | 覆盖 |
| `states.diagnostics.status == "partial"` | `BuildStatus::Partial` | 相邻层，weaker |
| reason 匹配 `/could not be resolved and were not checked/` | 精确同串 | 覆盖 |
| `fragmentDiagnostics(response, "legacy") == []` | flat diagnostics，无 per-fragment DTO | 缺 |

**保持 weaker。** 比 0201 更接近 near_exact（没有 unresolvedGuards 计数断言），
但 adapter diag status 与 per-fragment DTO 仍缺失，按任务要求不升。

**建议 ledger**：`parity=weaker`、`green_eligible=false`、
`status_recommendation=planned`、`status_applied=planned`；
`rust_tests[0].line` 307 → 317。

### 2.6 `TS-COMPONENT-0498` → weaker

**TS oracle**：`backends/typescript/test/componentBindings.test.ts:498`。

证据：

- 脚本事实：`case_component_0498_spread_and_dynamic_data_members_stay_partial_and_any`
  （ crates/wxml-backend-tsgo/tests/script_behavior.rs:236）：
  `dynamic.type_text == "any"`、`exact == false`、`behavior.data_incomplete`；
- 新增 build 层：`case_component_0498_build_reports_a_behavior_reason_and_mounts_dynamic`
  （ crates/wxml-backend-tsgo/tests/script_behavior.rs:617）：
  `BuildStatus::Partial`、reason 以
  `component behaviors are partially resolved: ` 开头且含 `behavior`、
  derived text 含 `__wxml_root.dynamic`。

| TS 断言 | native 证据 | 状态 |
| --- | --- | --- |
| `dynamic?.hover?.typeOnly == "any"` | 脚本事实 `type_text="any"` + build mount `__wxml_root.dynamic` | 不是 hover 层，weaker |
| `states.hover.reason` 匹配 `/behavior/i` | build reason `component behaviors are partially resolved: ...` | 相邻层，weaker |

**为什么保持 weaker**：

- TS 断言的是 hover 层；没有 hover adapter 测试；
- behavior data 没有并入 shadow 的 `this.data`；native 的 `any` 是未知属性访问的
  error recovery，不是“已建模的 behavior 字段 any”；
- 若真实 engine 在未知属性处返回 `Missing` 而不是 `Any`，hover 就不会发布 `any`。
  必须先补 hover 测试（fake 或 real engine）才可能 near_exact；本轮不宣称。

**建议 ledger**：`parity=weaker`、`green_eligible=false`、
`status_recommendation=planned`、`status_applied=planned`；
`rust_tests` 增加 build 测试
 crates/wxml-backend-tsgo/tests/script_behavior.rs:617；
同时把
 crates/wxml-backend-tsgo/tests/script_properties.rs:203
（test `case_component_0498_only_property_spread_keeps_known_properties`）的
旧行号 179 修正为 203。

---

## 3. 建议 ledger 字段（汇总）

### 3.1 逐 case

| Case | `parity` | `green_eligible` | `status_recommendation` | `status_applied` / `status` | `target_rust_test_status` |
| --- | --- | --- | --- | --- | --- |
| `TS-COMPONENT-0292` | `exact` | `true` | `green` | `green` | `migrated_green` |
| `TS-EVENTS-0188` | `near_exact` | `false` | `green` | `planned` | `proposed` |
| `TS-COMPONENT-0193` | `weaker` | `false` | `planned` | `planned` | `proposed` |
| `TS-GUARDS-0201` | `weaker` | `false` | `planned` | `planned` | `proposed` |
| `TS-LOCALS-0343` | `weaker` | `false` | `planned` | `planned` | `proposed` |
| `TS-COMPONENT-0498` | `weaker` | `false` | `planned` | `planned` | `proposed` |

### 3.2 计数（已回填）

| 项 | 回填前（HEAD `1c89338`） | 回填后（最终） |
| --- | ---: | ---: |
| `green` | 82 | 83 |
| `planned` | 219 | 218 |
| `retired-with-decision` | 38 | 38 |
| `exact` | 82 | 83 |
| `near_exact` | 2 | 3 |
| `weaker` | 96 | 100 |
| `structural` | 7 | 7 |
| `blocked` | 22 | 16 |
| `assessed_cases` | 209 | 209 |
| 无证据 planned | 92 | 92 |

如果 owner 接受 `fileId`→`path` 并把 0188 记 exact：`green` 84、
`planned` 217、`exact` 84、`near_exact` 2，其余相同。

### 3.3 `by_partition` 变化（已回填）

- `components`：`blocked` 7 → 6，`weaker` 1 → 2（0193）。
- `behaviors`：`blocked` 4 → 2，`weaker` 6 → 7，`exact` 1 → 2，
  `green_cases` 1 → 2，`planned_evidence_cases` 17 → 16（0292、0498）。
- `guards_js`：`blocked` 5 → 4，`weaker` 1 → 2（0201）。
- `loops`：`blocked` 1 → 0，`weaker` 2 → 3（0343）。
- `events`：`blocked` 1 → 0，`near_exact` 0 → 1（0188）。
- `definitions` / `symbols_edits`：不变。

### 3.4 台账行号（已回填）

| Case | 测试 | 回填前台账行 | 回填后实际行 |
| --- | --- | ---: | ---: |
| `TS-COMPONENT-0193` | `fully_checkable_component_request_stays_supported` | 1771 | 1851 |
| `TS-COMPONENT-0193` | `matching_root_binding_is_clean` | 1523 | 1603 |
| `TS-GUARDS-0201` | `case_guards_0201_...` | 448 | 466 |
| `TS-LOCALS-0343` | `case_locals_0343_...` | 307 | 317 |
| `TS-COMPONENT-0498` | `case_component_0498_only_property_spread_keeps_known_properties` | 179 | 203 |
| `TS-EVENTS-0188` | 旧 inline 测试 `case_events_0188_only_inline_behavior_methods_resolve` | 558 | 562 |
| `TS-EVENTS-0188` | 新测试 `case_events_0188_imported_behavior_handler_...` | — | 583 |

`migration_evidence.rust_tests[].line` 漂移不会让 `ledger.rs` 失败（它只检查
“文件存在 + 行号在范围内 + 文件里能搜到 `fn <test>(`”）；但退休 replacement 的
`line` 是严格断言。本轮把新增测试放在 `script_behavior.rs:430` 之后，未移动
`TS-COMPONENT-0774` replacement 指向的
`script_behavior.rs:340`（0570）和 `:430`（0763）。

---

## 4. 验证

### 4.1 第一轮（测试/证据，HEAD `1c89338` + 未提交改动）

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过（当轮工作树） |
| `cargo check --all-targets` | 通过 |
| `cargo clippy -p wxml-lsp --lib -- -D warnings` | 通过 |
| `cargo clippy -p wxml-backend-tsgo --all-targets -- -D warnings` | 通过 |
| `cargo clippy --all-targets -- -D warnings -A clippy::single_match_else` | 通过 |
| `cargo test -p wxml-lsp --lib` | 952 passed |
| `cargo test -p wxml-backend-tsgo --test script_properties` | 14 passed |
| `cargo test -p wxml-backend-tsgo --test script_events` | 27 passed |
| `cargo test -p wxml-backend-tsgo --test script_behavior` | 21 passed |
| `cargo test -p wxml-backend-tsgo --test shadow_locals_guards` | 21 passed |
| `cargo test -p wxml-lsp --test native_parity` | 76 passed, 26 ignored |

**已知非本轮问题**：`cargo clippy --all-targets -- -D warnings` 在
 crates/wxml-lsp/tests/native_parity/ledger.rs:1170
报 `clippy::single_match_else`。该代码在 HEAD `74669c3` 和 `1c89338` 中都存在
（`match retirement_scope { Some(..) => .., None => .. }`），第一轮没有改 `ledger.rs`；
允许该单条 lint 后全 workspace `--all-targets -D warnings` 干净。

### 4.2 第二轮（台账/validator/文档回填）

| 命令 | 结果 |
| --- | --- |
| `jq empty docs/plans/tsgo-test-ledger.json` | 通过 |
| `cargo test -p wxml-lsp --test native_parity ledger` | 14 passed |
| `rustfmt --check --edition 2024`（本轮 5 个 Rust 文件） | 通过 |
| `git diff --check` | 通过 |

回填后计数器与 JSON 实际 case 一致（`native_parity` 的 ledger 子集全绿）。
验证时工作树有并行 session 正在新增 `real_matrix.rs`/`native_gates`；全量
`native_parity` 的一次运行出现 `native_gates` 的 `#[ignore]` 计数 52 vs 49
和 `real_matrix` 冻结 manifest 失败，二者都来自并行文件而非本回填的 6 个 case；
并行 session 随后修掉了 real_matrix 一项，`native_gates` 仍在进行中。
本回填的 `ledger` 子集不受影响。

**diff-check**：

- 第一轮只改 4 个文件：
  `crates/wxml-backend-tsgo/tests/script_behavior.rs`、
  `crates/wxml-backend-tsgo/tests/script_events.rs`、
  `crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs`、
  `crates/wxml-lsp/src/backend_tsgo/component_bindings.rs`（仅 `#[cfg(test)]` 模块）。
- 第二轮改 4 个文件：
  `docs/plans/tsgo-test-ledger.json`、
  `crates/wxml-lsp/tests/native_parity/ledger.rs`（仅计数常量/注释）、
  `docs/plans/2026-11-17-tsgo-test-ledger.md`、
  `docs/reference/tsgo-test-contract.md`，以及本文件。
- README、生产代码、`native_parity.rs`、矩阵文件、`tests/fixtures` 均未改；
  没有新增 fixture 文件。
- 未 commit、未 review。

---

## 5. 残余与后续

1. `TS-EVENTS-0188` 的 exact 只差 `fileId`→`path` 的命名裁决；若 ledger 接受，
   本测试已经是 exact 证据。
2. `Behavior({...})` 选项对象后的尾随逗号生产缺口已修复
   （ crates/wxml-backend-tsgo/src/script/behavior.rs:414，精确放行 `}` `,` `)`），
   并补了 7 个回归；0188 fixture 已恢复真实尾随逗号。
3. `TS-COMPONENT-0193` 的 `probeProblems`、`TS-GUARDS-0201` 的
   `unresolvedGuards` 计数、`TS-LOCALS-0343` 的 per-fragment diagnostics、
   `TS-COMPONENT-0498` 的 hover `typeOnly` 仍无 native 对应面；本轮没有为它们
   伪造数组/计数/hover 结果。
4. `TS-COMPONENT-0292` 的 exact 依赖 ledger 接受 0147 的 per-usage 先例；
   如果 owner 不接受，应退回 weaker，而不是改测试去伪造 per-tag 数组。
