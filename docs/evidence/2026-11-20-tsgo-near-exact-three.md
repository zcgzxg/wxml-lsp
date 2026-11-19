# tsgo near-exact 三例补齐证据（2026-11-20）

## 0. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务给定起点 HEAD：`fb6047e`（`test(delivery): verify real native N2 upgrade`）
- 验证时工作树 HEAD：`8a56f27`（`docs: sync F3 closure and native default status`）。
  并行会话在本轮期间提交/修改了生产与台账文件；两个被改测试文件的 diff 只含本轮断言。
- 记录日期：2026-11-20
- 本轮只改 Rust 测试与本证据：
  - ` crates/wxml-backend-tsgo/tests/script_behavior.rs`
  - ` crates/wxml-backend-tsgo/tests/shadow_rewrite.rs`
  - 本文件
- 未修改生产代码、`docs/plans/tsgo-test-ledger.json`、`crates/wxml-lsp/tests/native_parity/ledger.rs`、
  README、`crates/wxml-lsp/tests/native_parity.rs`；未 commit、未 review。
- 工作树非独占：并行会话同时改动了 `crates/wxml-backend-tsgo/src/{input,shadow/layout,shadow/locals,shadow/rewrite}.rs`、
  `crates/wxml-lsp/src/server.rs`、`crates/wxml-lsp/tests/native_parity/ledger.rs`、
  `docs/plans/*` 等。本轮只读这些内容；验证在它们存在的工作树上运行并通过，不改写、不回滚。

**结论：`TS-REWRITE-0207` 达到 exact；`TS-COMPONENT-0521/0763` 已补齐脚本事实与
shadow build 断言，但仍缺 native checker hover 层，保持 near_exact（0 blocked）。
保守建议 green 数 58 → 59。**

---

## 1. 判定口径

exact 要求 TS oracle 的每条可观察断言都能在同一 Rust 测试里找到等价断言：

- 类型/状态/范围必须来自同一可观察层；脚本事实层不能替代 checker hover 层。
- 只补 oracle 原文要求的断言，不从实现输出反推期望。
- `near_exact` 的定义（`docs/plans/2026-11-19-tsgo-remaining-ledger-plan.md`）是
  “same semantics with a renamed field or a typed error replacing a TS error code”；
  本轮把改名/typed error 的映射写实，但不把脚本事实层冒充 checker hover 层。

---

## 2. `TS-REWRITE-0207` → exact

TS oracle：`wxmlRange must match the expression byte length`
（` backends/typescript/test/rewrite.test.ts:207`）。

Rust 证据：`case_rewrite_0207_wxmlrange_must_match_the_expression_byte_length`
（ ` crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:229` ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `run(request)` 抛 `RequestError` | `build_in_place_shadow(&invalid_request).expect_err(...)`（shadow_rewrite.rs:253） |
| `error.code === "bad-fragment"` | typed `ShadowError::Invalid { feature: "fragment.range", reason }`（shadow_rewrite.rs:255-260） |
| 触发条件：`exprText = "title"`、`wxmlRange = { start: 0, end: 99 }` | `FragmentSpec::new("f0", "title", SourceRange::new(0, 99))`（shadow_rewrite.rs:245） |
| （oracle 未要求，用于排除“任何 range 都失败”） | 同一表达式正确长度 `0..5` 构建成功（shadow_rewrite.rs:235-243） |

Exact 理由：

- TS 的 `bad-fragment` 是协议级错误码；native 用更细的 typed error
  `Invalid { feature: "fragment.range" }` 替代，属于 near-exact 定义明确允许的
  “typed error replacing a TS error code”。
- 本轮新增了 `feature` 与 `reason` 的精确断言，并加了正确长度的正向对照；
  拒绝原因被钉死为
  `wxmlRange length 99 does not match expression byte length 5`，不是泛化的
  `BadExpression` 或任意 layout 失败。
- 断言值与 oracle 的 `99`/`title`/`bad-fragment` 一一对应，没有从实现反推新期望。

建议 ledger：`status=green`、`parity=exact`、`green_eligible=true`、
`status_recommendation=green`、`status_applied=green`；
`rust_tests[0].line` 由 `209` 更新为 `229`。

---

## 3. `TS-COMPONENT-0521` → near_exact（已补齐可补断言）

TS oracle：`named behavior imports and re-export aliases resolve statically`
（` backends/typescript/test/componentBindings.test.ts:521`）。

Rust 证据：`case_component_0521_named_imports_and_reexport_aliases_resolve_statically`
（ ` crates/wxml-backend-tsgo/tests/script_behavior.rs:254` ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `hoverOf(result, "count")?.hover?.typeOnly === "number"` | `count.type_text == "number"`（script_behavior.rs:269），并断言 `count.exact`、声明文件 `shared.ts`、原 token range 精确为 `namedCount`（script_behavior.rs:270-272） |
| `result.ok && result.states.hover.status === "supported"` | `behavior.status.is_supported()`（script_behavior.rs:284）、`behavior.data_incomplete/properties_incomplete == false`、`behavior.problems` 为空（script_behavior.rs:276-281），以及 `result.status.is_supported()`、`result.reasons` 为空（script_behavior.rs:296-297） |
| （解析身份）named import + re-export alias | 唯一 behavior unit 的 `path == "shared.ts"`、`alias == Some("shared")`（script_behavior.rs:273-275） |
| （TS build gate 的 native 输入） | 单 registration、object data root、data/properties 完整（script_behavior.rs:287-295） |
| （`states.hover.status` 的 build 侧等价物） | 同一 fixture 走 `build_in_place_shadow_with_behavior`：`BuildStatus::Supported`、`reason == None`、`registration.count == 1`、object data root、properties 完整、派生文本含 `__wxml_root.namedCount`（script_behavior.rs:301-335） |

未达到 exact 的原因（不伪造）：

- `hover.typeOnly` 是 native checker/quickinfo 的输出；Rust 证据是脚本事实
  `DataField.type_text`。二者在本 fixture（字面量 `4`）上语义相同，且 `exact`
  标志证明不是 `any` 退化，但它们不是同一可观察层。
- `states.hover.status` 还依赖 TS checker 的 `probeProblems`；native 测试没有启动
  checker，无法证明该 gate。`build_in_place_shadow_with_behavior` 只覆盖
  `buildStatus` 及其非 checker 输入。
- 现有真实引擎 gate `G-P5-E2E-11`
  （ ` crates/wxml-lsp/tests/native_parity/native_engine_real.rs:2867` ）能测 behavior
  data hover，但它是 `#[ignore]` + opt-in（`WXML_TEST_NATIVE=1`），不能作为 case
  green；本轮也没有可用的 `TSGO_BIN`/native package。
- 因此保持 near_exact。若 reviewer 接受“renamed field”口径，可把
  `typeOnly → type_text`、`states.hover.status → behavior.status + buildStatus`
  视为等价；但那应由 ledger 裁决，不应由测试 worker 单方面宣称 exact。

建议 ledger：保持 `status=planned`、`parity=near_exact`、`green_eligible=false`、
`status_recommendation=green`、`status_applied=planned`；
`rust_tests[0].line` 保持 `254`，note 建议改为：

> Same named re-export behavior resolution. Rust now pins the field type/exact flag,
> `namedCount` token range in `shared.ts`, the single behavior unit alias `shared`,
> all `behaviorIncomplete` inputs, and the shadow build gate. The native checker hover
> (`typeOnly`, `states.hover.status`) is still not run, so parity stays near_exact.

---

## 4. `TS-COMPONENT-0763` → near_exact（已补齐可补断言）

TS oracle：`behavior data names with a non-probe double underscore remain usable`
（` backends/typescript/test/componentBindings.test.ts:763`）。

Rust 证据：`case_component_0763_non_probe_double_underscore_data_names_remain_usable`
（ ` crates/wxml-backend-tsgo/tests/script_behavior.rs:430` ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `hover?.hover?.typeOnly === "number"` | `state.type_text == "number"`（script_behavior.rs:442），并断言 `state.exact`、`path == "index.ts"`、原 token range 精确为 `__state`（script_behavior.rs:443-445） |
| `result.ok && result.build.behaviorIncomplete === false` | `!behavior.data_incomplete`、`!behavior.properties_incomplete`、`behavior.problems.is_empty()`（script_behavior.rs:448-454）——这正是 TS `behaviorIncomplete` 的三个 OR 输入 |
| （完整解析身份） | 唯一 behavior unit 的 `path == "index.ts"`（script_behavior.rs:447） |
| （无其他降级） | 单 registration、object data root、data/properties 完整、`result.status.is_supported()`、`result.reasons` 为空（script_behavior.rs:460-469） |

判定：

- 第二条 oracle 断言已按 TS 源码的 OR 定义完整落地：
  `behaviorIncomplete = dataIncomplete || propertiesIncomplete || problems.length > 0`。
- 第一条仍是 `hover.typeOnly` vs `DataField.type_text` 的层差异；同 0521，不能在
  没有真实 checker hover 的情况下宣称 exact。
- 本 fixture 的 `__state` 是普通非 probe 双下划线字段；测试同时钉住字段名、
  `number`、exact 标志和原 token range，能防止被 probe 过滤或改名。

建议 ledger：保持 `status=planned`、`parity=near_exact`、`green_eligible=false`、
`status_recommendation=green`、`status_applied=planned`；
`rust_tests[0].line` 由 `369` 更新为 `430`，note 建议改为：

> Same non-probe `__state` number type and complete behavior data. Rust now pins
> field name/exact flag/token range, all three `behaviorIncomplete` inputs, the single
> inline behavior unit, and the overall Supported status. The native checker hover
> (`typeOnly`) is still not run, so parity stays near_exact.

---

## 5. 验证记录

在 `8a56f27` + 本轮未提交测试改动上执行（工作树同时含有并行会话改动）：

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo check -p wxml-backend-tsgo --tests` | 通过 |
| `cargo clippy -p wxml-backend-tsgo --test script_behavior --test shadow_rewrite -- -D warnings` | 通过（No issues found） |
| `cargo clippy -p wxml-backend-tsgo --tests -- -D warnings` | 通过（No issues found） |
| `cargo test -p wxml-backend-tsgo --test script_behavior` | 20 passed |
| `cargo test -p wxml-backend-tsgo --test shadow_rewrite` | 22 passed |
| `cargo test -p wxml-backend-tsgo --test script_behavior case_component_0521` | 1 passed |
| `cargo test -p wxml-backend-tsgo --test script_behavior case_component_0763` | 1 passed |
| `cargo test -p wxml-backend-tsgo --test shadow_rewrite case_rewrite_0207` | 1 passed |
| `git diff --check` | clean |

说明：

- 本轮改动只在两个 integration test target；`script_behavior` 与 `shadow_rewrite`
  的全量套件均通过。
- 未 commit、未 review、未改 ledger/README/native_parity.rs。

---

## 6. 建议 ledger 汇总

| Case | 建议 parity | 建议 status | green_eligible | rust_tests line | 结论 |
| --- | --- | --- | --- | ---: | --- |
| `TS-REWRITE-0207` | `exact` | `green` | true | 229 | exact；typed error 替代 `bad-fragment`，reason + 正向对照已钉死 |
| `TS-COMPONENT-0521` | `near_exact` | `planned` | false | 254 | 脚本/布局/行为事实已补齐；checker hover 层未跑 |
| `TS-COMPONENT-0763` | `near_exact` | `planned` | false | 430 | `behaviorIncomplete` 已完整覆盖；checker hover 层未跑 |

若按上表应用：

- 当前工作树 ledger（并行 wave-10 回填后）：`green` 58、`planned` 245、
  `exact` 58、`near_exact` 3、`weaker` 82、`structural` 7、`blocked` 22、
  `assessed_cases` 172。
- 应用本表后：`green` 58 → **59**，`planned` 245 → **244**，
  `exact` 58 → **59**，`near_exact` 3 → **2**；
  `weaker` 82、`structural` 7、`blocked` 22、`retired` 36、总数 339、
  `assessed_cases` 172 不变。

如果 reviewer 接受 0521/0763 的 renamed-field 等价口径，可再把两者提升为
`exact/green`，此时 `green`/`exact` 为 **61**、`near_exact` 为 **0**、`planned`
为 **242**；本轮保守建议不这样做，避免把脚本事实层冒充 checker hover。
