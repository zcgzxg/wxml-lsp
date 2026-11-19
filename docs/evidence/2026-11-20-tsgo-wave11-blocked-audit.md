# tsgo wave-11 blocked 用例审计（22 个 `parity=blocked`）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 审计基线：HEAD `51abc13`（`test(tsgo): close conditional parity and ledger backfill`）。
  任务输入快照的台账 `implementation_head` 是 `43625ee`；读取期间并行 session
  落地 wave-11 retire（`TS-COMPONENT-0774`、`TS-META-0027`），当前工作树 JSON 已变成
  `implementation_head=51abc13`。本审计以 HEAD 的实际源码为准，并在 §7 记录行号漂移；
  并行 retire 只改变 planned/retired、no-evidence 和 by_partition routed 计数，
  不改变 22 个 blocked 集合，也不改变 weaker/exact/near_exact/structural。
- 记录日期：2026-11-20
- 范围：只读读取台账、两份 contract、生产代码、Rust 测试和旧 TS oracle；
  **只新增本文件**。未修改代码、测试、 docs/plans/tsgo-test-ledger.json、台账
  Markdown、contract、README、CI 或脚本；未 commit；未 review；**未运行任何测试**。
- 台账快照：339 总 case。任务输入是 `green` 70 / `planned` 233 /
  `retired-with-decision` 36；并行 wave-11 retire 后当前工作树是
  `green` 70 / `planned` 231 / `retired-with-decision` 38。两种快照下
  `exact` 70、`near_exact` 2、`weaker` 96、`structural` 7、`blocked` 22、
  `assessed_cases` 197 都不变；无证据 planned 从 106 变为 104。
  本文件的 22 个 blocked 集合与 promotion delta 不受这 2 个 retire 影响。

**结论先行：**

1. **0/22 可以只靠现有证据直接转为 `exact`。** `exact` 要求 TS oracle 的每条可观察断言
   都能在同一 Rust 测试里找到等价断言；22 个 case 都至少缺一条这样的断言。
2. **1 个可以在补一个测试、且不新增 native API 的情况下转为 `exact`**：
   `TS-COMPONENT-0292`。生产 `component_bindings` 已经把 behavior properties 合并进
   `properties` 并运行绑定检查；缺的是把 TS 的 3 条诊断、1 个 skip 和 2 个
   “无结果”语义按 usage 逐个钉住的测试。`TS-COMPONENT-0147` 的 exact 先例已经接受
   “没有 per-tag 数组、按 usage 断言”的等价性。
3. **1 个已经具备 `near_exact` 语义**：`TS-EVENTS-0188`。
   生产 event 解析已经返回 imported behavior 的 `path`/`signature`/`documentation`，
   现有 `case_events_0425` 已经同时断言 status、definition path、文档和 signature；
   只差把 fixture 换成 TS 的 `src/behaviors/base.ts`/`fromBehavior`。
   `fileId` → `EventDefinition.path` 属于 near-exact 定义中的 renamed field；
   补一个完全对齐的 fixture 后可再评估 exact。
4. **4 个可以立即从 `blocked` 重新登记为 `weaker`**（不需要新测试）：
   `TS-COMPONENT-0193`、`TS-GUARDS-0201`、`TS-LOCALS-0343`、`TS-COMPONENT-0498`。
   它们都已经有 typed `Partial`/`Supported` 结果或同层事实断言，只是 TS oracle 的
   剩余断言没有 native 对应面（Node `probeProblems`、`unresolvedGuards` 计数、
   per-fragment diagnostics、hover 层）。
5. **16 个必须保持 `blocked`**：
   `TS-COMPONENT-0229`、`TS-COMPONENT-0279`、`TS-COMPONENT-0570`、
   `TS-COMPONENT-0676`、`TS-DEFINITIONS-0036`、`TS-GUARDS-0097`、
   `TS-GUARDS-0218`、`TS-GUARDS-0261`、`TS-GUARDS-0312`、
   `TS-LAYOUTS-0049`、`TS-LAYOUTS-0062`、`TS-LAYOUTS-0072`、
   `TS-LAYOUTS-0117`、`TS-SYMBOLS-0116`、`TS-SYMBOLS-0299`、
   `TS-SYMBOLS-0425`。
   其中 `TS-COMPONENT-0570`（未解析 behavior 仍可能伪造 2339）和
   `TS-COMPONENT-0676`（外部 behavior 未解析时整段跳过、不检查 own property）
   不是“缺证据”，而是**当前生产行为与 TS oracle 相反/有假阳性风险**；
   不能把 typed `Partial` 当作 weaker 来掩盖。
6. **需要 owner 裁决的候选**（不计入上面的 6 个）：
   `TS-COMPONENT-0229` 和 `TS-SYMBOLS-0116` 适合 split；
   `TS-COMPONENT-0279` 和 `TS-LAYOUTS-0062` 适合 retire/rewrite，
   因为 symbol-bridge / 旧探针放置已经被替换，不是 parity 提升问题。

### 0.1 计数

| 项 | 当前 | 建议应用后（保守：0188 记 near_exact） | 建议应用后（乐观：0188 记 exact） |
| --- | ---: | ---: | ---: |
| `green` | 70 | 71 | 72 |
| `planned` | 231 | 230 | 229 |
| `retired-with-decision` | 38 | 38 | 38 |
| `exact` | 70 | 71 | 72 |
| `near_exact` | 2 | 3 | 2 |
| `weaker` | 96 | 100 | 100 |
| `structural` | 7 | 7 | 7 |
| `blocked` | 22 | 16 | 16 |
| `assessed_cases` | 197 | 197 | 197 |
| 无证据 planned | 104 | 104 | 104 |

说明：`weaker`/`near_exact`/`structural`/`blocked` 都保持 `planned`，
只有 `exact` 才能 `green`。因此“blocked → weaker”不是 green 声明，而是证据分类的
诚实化；保守方案下本审计的 6 个建议里只有 `TS-COMPONENT-0292` 会改变
green 计数（若 0188 记 exact，则 green 再 +1）。

### 0.2 推荐提升（6 个）

| 顺序 | Case | 建议 | 是否需要新测试 | 是否需要新 native API/生产行为 | 风险 |
| ---: | --- | --- | --- | --- | --- |
| 1 | `TS-COMPONENT-0193` | `weaker` | 否（现有测试） | 否 | 低 |
| 2 | `TS-GUARDS-0201` | `weaker` | 否（现有测试） | 否 | 低 |
| 3 | `TS-LOCALS-0343` | `weaker` | 否（现有测试） | 否 | 低 |
| 4 | `TS-EVENTS-0188` | `near_exact`（现有 0425 测试）；对齐 fixture 后可评估 exact | 现有测试足够 near_exact；建议补 fixture 以评估 exact | 否 | 低 |
| 5 | `TS-COMPONENT-0292` | `exact`（补测后） | 是（binding 测试） | 否 | 中低 |
| 6 | `TS-COMPONENT-0498` | 先 `weaker`；hover 测试后 `near_exact` | 是（hover 测试） | 否（但建议把 behavior data 纳入 known roots） | 中 |

### 0.3 明确不可提升（16 个）

按阻塞根因分组（详细见 §4）：

| 类别 | 数量 | Case |
| --- | ---: | --- |
| 缺 native API / 契约 / DTO 字段 | 2 | `TS-COMPONENT-0229`、`TS-DEFINITIONS-0036` |
| 需要真实 checker 的 diagnostics / hover / narrowing | 7 | `TS-GUARDS-0097`、`TS-GUARDS-0218`、`TS-GUARDS-0261`、`TS-GUARDS-0312`、`TS-LAYOUTS-0049`、`TS-LAYOUTS-0072`、`TS-LAYOUTS-0117` |
| 需要新生产行为（API 已存在） | 5 | `TS-COMPONENT-0570`、`TS-COMPONENT-0676`、`TS-SYMBOLS-0116`、`TS-SYMBOLS-0299`、`TS-SYMBOLS-0425` |
| 旧布局/探针放置已退役或替换 | 2 | `TS-COMPONENT-0279`、`TS-LAYOUTS-0062` |

---

## 1. 判定口径

### 1.1 台账的四类证据

沿用  docs/plans/2026-11-19-tsgo-remaining-ledger-plan.md 的定义：

- **weaker**：存在 Rust 测试，但省略了 TS 断言的一部分（通常是 hover 类型、
  diagnostics、reason 文本或精确 range）。
- **structural**：只断言 overlay/layout 结构，或使用了不同的性能采样，不是同一可观察结果。
- **blocked**：TS 断言需要 checker/hover/definition/event-chain 查询，而 Rust 测试
  从不运行该查询，或依赖一个没有 `TSGO_BIN` 就会 skip 的 opt-in gate。
- **near-exact**：语义相同，只是字段改名，或用 typed error 替代 TS error code。

本次审计额外用两道门判断能否从 `blocked` 提升：

1. **同层门**：Rust 证据必须运行 TS 断言对应的查询/结果层。脚本声明事实（`PropertyFact`、
   `DataField`）只能支撑 `weaker`，不能支撑 `exact`；`BuildStatus::Partial` 是
   adapter `CapabilityStatus::Partial` 的上游，可以作为 `weaker` 的 typed 证据，
   但不能自动等同于 `states.diagnostics.status`。
2. **不矛盾门**：native 结果必须是 TS 断言的保守欠近似，不能是反例。
   如果 TS 期望 `complete=true` 而 native 明确返回 `complete=false`（`TS-SYMBOLS-0299/
   0425`），或 TS 期望 Object 清洁而 native 明确返回 `Partial`（`TS-COMPONENT-0229`），
   这不是 weaker，而是 blocked。**typed `Partial` 本身不是提升理由；它必须是
   对 oracle 的保守覆盖，而不是 oracle 的反面。**

### 1.2 为什么 `Partial` 不自动等于 `weaker`

`Partial` 在 native 里有两种含义：

- **保守欠近似**：native 不能证明 TS 期望的干净结果，但也没有伪造错误
  （`TS-COMPONENT-0229` 的 optional-object、`TS-SYMBOLS-0299` 的 renamed destructuring）。
  这种 Partial 是“未证明”，不是“已证明的更弱版本”；如果 TS 的核心断言正是
  “clean/complete”，把它记 weaker 会把反例包装成覆盖。
- **oracle 本身期望的降级**：`TS-GUARDS-0201`、`TS-LOCALS-0343` 的 TS 明确断言
  `partial` + typed reason；这种 Partial 才是可断言的同语义结果。

### 1.3 审计没有做的事

- 没有运行 `cargo test`、`cargo clippy` 或任何 native gate；所有结论来自静态阅读。
- 没有修改 ledger、contract、ledger.rs 常量或任何测试。
- §0.1 的计数是**投影**，不是当前 JSON 的值。
- 对 `TS-COMPONENT-0570/0676` 的“native 会发布 2339 / 不检查 own property”是
  代码路径推断；在把“known divergence”写进台账前，应先补一个最小回归测试确认。

---

## 2. 22 个 `parity=blocked` 的逐项矩阵

类别缩写：

- `TEST`：生产/typed 面已经存在，缺的是测试或只缺 harness-only 字段。
- `API`：缺 native API、contract 允许的方法或 DTO 字段。
- `CHK`：需要真实 checker 的 diagnostics/hover/narrowing 结果，现有测试从未运行该查询。
- `PROD`：API 已存在，但需要新的生产行为（包括把当前保守行为改成与 TS 一致）。
- `MECH`：旧布局/探针放置已退役或替换，需要 disposition 裁决。

### 2.1 组件、behavior、definition、event（9）

| # | Case | 分区 | TS 核心断言 | 现有 native typed 结果 | 类别 | 判定 | 风险 |
| ---: | --- | --- | --- | --- | --- | --- | --- |
| 1 | `TS-COMPONENT-0193` | components | Supported；单 tag clean；`probeProblems=[]`；`skipped=0` | component pass `status=None`（Supported）、0 诊断、3 个 fragment、3 次 query；无 `probeProblems` 数组 | TEST | **weaker（建议）** | 低（probeProblems / adapter 层未断言） |
| 2 | `TS-COMPONENT-0229` | components | optional-object clean；optional-array/absent clean；optional-mismatch 1 条诊断 | optional-array/absent clean；optional-object 与 optional-mismatch 均 Partial，无诊断 | API | blocked | 高（Object clean 被 Partial 反例） |
| 3 | `TS-COMPONENT-0279` | components | symbol-bridge 布局下恰好 1 条 path-free 消息 | 只有 in-place path-free；symbol-bridge 已退役 | MECH | blocked（需 disposition） | 中（需 disposition） |
| 4 | `TS-COMPONENT-0292` | behaviors | behavior properties + kebab/camel 的 3 条诊断、1 个 skip、2 个无结果 tag | 生产 binding pass 已支持 behavior properties；现有测试只断言 declaration facts | TEST | **exact（补测后）** | 中低（fixture/message 细节） |
| 5 | `TS-COMPONENT-0498` | behaviors | hover `typeOnly="any"`；hover reason `/behavior/i` | 脚本事实 `dynamic.type_text="any"`、`exact=false`、`data_incomplete`；无 hover 测试 | TEST | **weaker（建议）**；hover 测试后 near_exact | 中（hover any 来源未证实） |
| 6 | `TS-COMPONENT-0570` | behaviors | `behaviorIncomplete`；build/diagnostics Partial；无 2339 | 脚本事实 Partial；build 未测；无 unknown-behavior-root 抑制，可能发布 2339 | PROD | blocked（已知风险） | 高（可能 false 2339） |
| 7 | `TS-COMPONENT-0676` | behaviors | own `title` 诊断；`skipped>=1`；details 含 `bad.behavior` | `analyze_component` 在 `properties.complete=false` 时整段返回 None；tag 只 skip，不检查 own property | PROD | blocked（已知风险） | 高（可能漏报 own property） |
| 8 | `TS-DEFINITIONS-0036` | definitions | generated member 的 source range；definition `kind="generated"`；definition range 等于 source | layout 有 `generated_declarations` + `source`；definition DTO 只有 file/range，无 kind | API | blocked | 中（definition DTO 缺 kind） |
| 9 | `TS-EVENTS-0188` | events | Supported；definition fileId `src/behaviors/base.ts`；signature；文档 | event resolution 已返回 imported behavior 的 `path`/`signature`/`documentation`；`case_events_0425` 已同时覆盖四项 | TEST | **near_exact（建议）**；对齐 fixture 后可评估 exact | 低（fixture 路径/方法名） |

### 2.2 guards、layouts、locals、symbols（13）

| # | Case | 分区 | TS 核心断言 | 现有 native typed 结果 | 类别 | 判定 | 风险 |
| ---: | --- | --- | --- | --- | --- | --- | --- |
| 10 | `TS-GUARDS-0097` | guards_js | if/elif/else 分支 narrowing 的 hover 类型 + 诊断 | 只固定渲染的 if/elif 结构和 mount 文本 | CHK | blocked | 高（checker narrowing 未运行） |
| 11 | `TS-GUARDS-0201` | guards_js | diagnostics Partial；reason `/could not be resolved and were not checked/`；空诊断；无 2339 | `BuildStatus::Partial` + 同 reason；mount 有 `@ts-ignore`；无 adapter diagnostics 测试 | TEST | **weaker（建议）** | 低（build/adapter 层差） |
| 12 | `TS-GUARDS-0218` | guards_js | `&&` / 逻辑或 narrowing；Partial；negated 诊断 | 渲染结构 + `BuildStatus::Partial`；无 hover/诊断 | CHK | blocked | 高（checker narrowing 未运行） |
| 13 | `TS-GUARDS-0261` | guards_js | 嵌套 loop/guard narrowing；hover `string`；无诊断 | 只固定 local capture 和 closer 顺序 | CHK | blocked | 高（checker narrowing 未运行） |
| 14 | `TS-GUARDS-0312` | guards_js | pragma 注入；typo diagnostic 被发布 | `JsCheckApplied::Pragma`、`// @ts-check` 前缀、note；无 typo 诊断 | CHK | blocked | 高（typo 诊断未运行） |
| 15 | `TS-LAYOUTS-0049` | components | page options 探针无诊断 | Supported 布局、probe anchor、byte-exact reconstruction；无 checker | CHK | blocked | 中（checker clean 未运行） |
| 16 | `TS-LAYOUTS-0062` | components | Component options 直接放探针应报 2353 | 生产始终把探针放进 `methods`（必要时包装）；只固定放置规则 | MECH | blocked（需 disposition） | 中（机制替换，需 disposition） |
| 17 | `TS-LAYOUTS-0072` | components | Component `methods` 探针无诊断 | 只固定探针在 methods 内 | CHK | blocked | 中（checker clean 未运行） |
| 18 | `TS-LAYOUTS-0117` | components | JS JSDoc overlay hover `string \| number` + 文档 + 无诊断 | 只固定 overlay 的 typedef/@property 文本 | CHK | blocked | 中（hover/checker 未运行） |
| 19 | `TS-LOCALS-0343` | loops | diagnostics Partial；reason `/could not be resolved and were not checked/`；空 fragment 诊断 | `BuildStatus::Partial` + 同 reason + `const item = undefined as any;` | TEST | **weaker（建议）** | 低（adapter/per-fragment 层差） |
| 20 | `TS-SYMBOLS-0116` | symbols_edits | property `complete=true` + declaration + `observerPath` occurrence；missing handler 安全插入 | 插入半段已覆盖 complete/declaration/insertion（status 未单独断言）；`observerPath` 未实现，dataPath 为 Partial | PROD | blocked（可 split） | 高（observerPath 未实现） |
| 21 | `TS-SYMBOLS-0299` | symbols_edits | renamed destructuring `complete=true` + declaration | 解构 `this.data` 为 Partial，无 declaration | PROD | blocked（相反结果） | 高（complete 结果相反） |
| 22 | `TS-SYMBOLS-0425` | symbols_edits | 另一个 page 的 `this.setData` 不阻断；`complete=true` + declaration | 另一个 script 的 setData 阻断，`complete=false` | PROD | blocked（相反结果） | 高（complete 结果相反） |

### 2.3 阻塞类别计数

| 类别 | 数量 | Case |
| --- | ---: | --- |
| TEST（可提升） | 6 | 0193、0292、0498、0188、0201、0343 |
| API | 2 | 0229、0036 |
| PROD | 5 | 0570、0676、0116、0299、0425 |
| MECH | 2 | 0279、0062 |
| CHK | 7 | 0097、0218、0261、0312、0049、0072、0117 |
| 合计 | 22 | — |

---

## 3. 推荐提升的 6 个

### 3.1 `TS-COMPONENT-0193` → `weaker`

**TS oracle**（ backends/typescript/test/componentBindings.test.ts:193）：

- `result.ok`
- `states.diagnostics.status == "supported"`
- `response.components == [{tag:"valid",diagnostics:[]}]`
- `probeProblems == []`
- `componentOutcome(request).skipped == 0`

**现有 native 证据**：

-   crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1771
  `fully_checkable_component_request_stays_supported`：
  3 个属性、3 个 fragment、3 次真实 type query、0 诊断、`result.status.is_none()`。
-   crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1523
  `matching_root_binding_is_clean`：两个匹配 root binding 无诊断、status None。
-   crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1647
  `component_binding_matrix_matches_the_typescript_oracle`：矩阵里 clean usage 的
  “status None 即没有 skip”规则已被 exact 的 `TS-COMPONENT-0147` 接受。

**映射**：

| TS 断言 | native 证据 |
| --- | --- |
| `states.diagnostics.status == supported` | component pass `status=None`；`finish` 在 `partial` 或 `skipped>0` 时必返回 Partial |
| `components == [{tag:"valid",diagnostics:[]}]` | 三个 usage 全部检查、无诊断；没有 per-tag 数组，但按 usage 断言与 0147 的 exact 先例一致 |
| `skipped == 0` | `status=None` 证明没有 skip |
| `probeProblems == []` | **无 native 对应**：native 有 `ProbeAnchors`（ crates/wxml-backend-tsgo/src/shadow/layout.rs:314），但没有 Node 的 probe health 数组 |

**残余与风险**：

- 唯一未覆盖的是 `probeProblems=[]`，它是 Node probe helper/root 健康检查；
  native 可以用 `probe_anchors.root_offset` + 原文字节重建做等价检查
  （ crates/wxml-backend-tsgo/tests/shadow_layout.rs:78 的 layout 049 已这么做），
  但这是**机制级**证据，不是同一个 `probeProblems` 字段。
- 现有测试是 component pass 层，不是 adapter overall status；adapter 层的
  Supported 还没有单独断言。
- 读取时工作树有未提交的  crates/wxml-backend-tsgo/src/query/probe_health.rs
  （`checkProbeHealth` 的决策表移植），但模块注释明确它**尚未接入**
  hover/diagnostics/capability status。它不改变当前的 0193 判定；若后续接入并
  在 adapter 结果中暴露 probe health，0193 可以重新评估 near_exact/exact。
- **不要**把 0193 升 exact：wave-6 的 exact 拒绝理由仍然成立。`weaker` 是诚实的
  分类，记录“4/5 断言覆盖，probeProblems 无 native 对应面”。

**建议 note**：

> Existing component-binding tests prove three clean bindings, zero diagnostics,
> three real type queries and status None (Supported), covering the clean per-tag
> result and skipped=0. The TS `probeProblems=[]` is a Node probe-health field with
> no native counterpart; the adapter-level overall status is not asserted. Kept
> weaker, not exact.

### 3.2 `TS-GUARDS-0201` → `weaker`

**TS oracle**（ backends/typescript/test/guards.test.ts:201）：

- `build.unresolvedGuards == 1`
- `states.diagnostics.status == "partial"`
- reason `/could not be resolved and were not checked/`
- `diagnostics == []`
- `fragments[0].diagnostics == []`

**现有 native 证据**：

-   crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:459
  `case_guards_0201_an_unparseable_guard_does_not_fabricate_a_diagnostic_and_reports_partial`：
  - `BuildStatus::Partial`
  - `built.reason` 含 `"could not be resolved"`
  - 不渲染 `if (`
  - mount 前有 `// @ts-ignore`
  - 仍挂载 `__wxml_root.state.user.name;`
-   crates/wxml-backend-tsgo/src/shadow/layout.rs:901：未解析 binding/guard 的
  reason 就是 `"some template bindings or guards could not be resolved and were not checked"`。
-   crates/wxml-backend-tsgo/src/shadow/layout.rs:1279：`ScopeBuilder::guard_text`
  在 `unresolved` 时注入 `// @ts-ignore`，这是“不伪造 2339”的 native 机制。
-   crates/wxml-lsp/src/backend_tsgo/input.rs:790：adapter 的 degradation 前缀是
  `"shadow coverage is partial: {reason}"`； crates/wxml-lsp/src/backend_tsgo/diagnostics.rs:170
  会把该 reason 合并进 diagnostics status。因此 adapter 层必然是 Partial 且 reason
  匹配 TS 正则。

**残余**：

- `unresolvedGuards == 1` 是 `layout.rs` 的局部计数（ crates/wxml-backend-tsgo/src/shadow/layout.rs:682），
  没有进入 `BuiltShadow`，因此不能精确断言“1”而不是“至少 1”。
- adapter 层的 `states.diagnostics.status`、top-level `diagnostics`、per-fragment
  diagnostics 没有在现有测试里断言；native 是 flat diagnostics list，没有 per-fragment DTO。
- 空诊断的最终来源是 checker 对 `@ts-ignore` mount 的处理；现有测试只证明机制，
  没有运行真实 checker。

**判定**：`weaker`。核心 typed 结果（Partial + 稳定 reason 子串 + no fabricated `if`）
已经同层证明；剩余的是计数/DTO 形状/真实 checker 空结果。风险低。

**建议 note**：

> Build-level test proves BuildStatus::Partial, the stable `could not be resolved
> and were not checked` reason, no rendered `if`, and the `@ts-ignore` unnarrowed mount.
> The TS diagnostics-level Partial status, empty top-level/fragment diagnostics and
> `unresolvedGuards=1` count are not asserted; the count is not exposed. Kept weaker.

### 3.3 `TS-LOCALS-0343` → `weaker`

**TS oracle**（ backends/typescript/test/locals.test.ts:343）：

- `states.diagnostics.status == "partial"`
- reason `/could not be resolved and were not checked/`
- `fragmentDiagnostics(response, "legacy") == []`

**现有 native 证据**：

-   crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:317
  `case_locals_0343_a_locals_only_request_still_degrades_to_partial_with_a_typed_reason`：
  - `BuildStatus::Partial`
  - reason 含 `"could not be resolved"`
  - mount 为 `const item = undefined as any;`
- 同 0201 的 reason/adapter 合并路径（ crates/wxml-backend-tsgo/src/shadow/layout.rs:901、
    crates/wxml-lsp/src/backend_tsgo/input.rs:790、
    crates/wxml-lsp/src/backend_tsgo/diagnostics.rs:170）。

**残余**：

- TS 的 `states.diagnostics.status` 是 adapter 层；现有测试是 build 层。
- `fragmentDiagnostics` 是 per-fragment；native 是 flat list，没有 per-fragment DTO。
- 与 0201 不同，0343 没有 `unresolvedBindings` 计数断言，TS 也不要求计数；
  因此 0343 比 0201 更接近 near_exact。

**判定**：`weaker`。如果补一个 adapter 测试，断言 diagnostics status Partial +
reason + flat diagnostics 为空，可以升 `near_exact`；要 exact 仍缺 per-fragment
DTO/计数，不建议。

**建议 note**：

> Build-level test proves Partial, the stable unresolved bindings/guards reason and
> the `undefined as any` fallback. The TS diagnostics-level Partial status and empty
> per-fragment diagnostics are not asserted; no per-fragment DTO exists. Kept weaker.

### 3.4 `TS-EVENTS-0188` → `near_exact`（现有 0425 测试；对齐 fixture 后可评估 exact）

**TS oracle**（ backends/typescript/test/events.test.ts:188）：

- `result.state.status == "supported"`
- `result.definition?.fileId == "src/behaviors/base.ts"`
- `result.signature` 匹配 `/fromBehavior/`
- `result.documentation` 匹配 `/来自 behavior/`

**现有 native 证据**：

-   crates/wxml-backend-tsgo/tests/script_events.rs:462
  `case_events_0425_aliased_behavior_handler_points_at_the_behavior_file`：
  imported behavior `behaviors/base.ts` 的 `onBack` 同时断言 status Supported、
  definition path、`"返回上一页"` 文档和 signature；这是 0188 四项语义的
  同层证据，只是 fixture 的路径/方法名与 TS 不同。
-   crates/wxml-backend-tsgo/tests/script_events.rs:562
  `case_events_0188_only_inline_behavior_methods_resolve`：
  inline `Behavior({methods:{fromBehavior(){}}})` 解析为 Supported、definition Some、
  documentation `"来自 behavior"`、无 missing diagnostic。
-   crates/wxml-lsp/src/semantics/event_definition.rs:910
  `component_behavior_alias_and_top_level_handlers_expose_hover_facts`：
  imported behavior `pages/index/base.ts` 的 `onBack` 定义位置被精确断言。
-   crates/wxml-lsp/src/semantics/event_diagnostics.rs:383
  `component_behavior_and_alias_handlers_are_known`：behavior handler 不产生 missing 诊断。
-   crates/wxml-backend-tsgo/src/script/events.rs:166
  `EventResolution` 已经带 `definition: Option<EventDefinition>`、`signature`、
  `documentation`； crates/wxml-backend-tsgo/src/script/events.rs:512 的 `resolve_one` 在 method 命中时同时填三者。
  `EventMethod`（ crates/wxml-backend-tsgo/src/script/events.rs:111 ）带 `path`、`signature`、`documentation`、`range`。

**为什么之前是 blocked、现在是什么状态**：

- 现有 inline 测试只覆盖 documentation，不同时覆盖 behavior fileId/signature；
  `case_events_0425` 已经覆盖四项，但 fixture 是 `behaviors/base.ts`/`onBack`，
  不是 TS 的 `src/behaviors/base.ts`/`fromBehavior`。
- 生产已经具备四项能力；按 near-exact 定义（同语义、字段改名），`case_events_0425`
  已经足够支撑 `near_exact`。要把 case 升 exact，需要把 fixture 对齐成
  `src/behaviors/base.ts`/`fromBehavior` 并断言 `path` 等于 TS 的 `fileId`。

**映射与分类**：

| TS 断言 | native 现有/合并测试可断言 | 备注 |
| --- | --- | --- |
| `state.status == supported` | `EventResolution.status.is_supported()` | 同层 |
| `definition.fileId == "src/behaviors/base.ts"` | `EventDefinition.path == "src/behaviors/base.ts"` | `fileId` → `path`，renamed field |
| `signature /fromBehavior/` | `EventResolution.signature` 含 `fromBehavior` | 同层 |
| `documentation /来自 behavior/` | `EventResolution.documentation == "来自 behavior"` | 同层 |

**判定**：记 `near_exact`（`case_events_0425` 已覆盖四项语义，但 fixture 路径/方法名
与 TS 不同）。补一个完全对齐的 fixture 后，如果 owner 接受 `fileId`→`path` 和
fixture 等价，可评估 exact；按当前 P5 口径，near_exact 更保守。风险低。

**建议 note**：

> `case_events_0425` already proves Supported, the imported behavior definition
> path, Chinese documentation and the handler signature in one event chain; its
> fixture uses `behaviors/base.ts`/`onBack` instead of the TS
> `src/behaviors/base.ts`/`fromBehavior`. The TS `fileId` maps to
> `EventDefinition.path`; kept near_exact until a fixture-aligned test pins the
> exact path and signature.

### 3.5 `TS-COMPONENT-0292` → `exact`（补测后）

**TS oracle**（ backends/typescript/test/componentBindings.test.ts:292）：

- `behavior-ok` diagnostics `[]`
- `behavior-bad` 3 条消息：
  - `component property "card-title" expects string, got number`
  - `component property "score" expects string | number, got boolean`
  - `component property "defaulted" expects number, got string literal`
- `results.has("behavior-loose") == false`
- `results.has("behavior-missing-required") == false`
- `outcome.skipped == 1`
- `outcome.skippedDetails == ["behavior-loose.loose"]`

**生产证据（不需要新 API）**：

-   crates/wxml-backend-tsgo/src/script/properties.rs:241 `build_properties` 先用
  `behavior.properties` 初始化（`:253`），再合并 own properties；`complete` 由
  `!behavior.properties_incomplete` 决定（`:262`）。
-   crates/wxml-backend-tsgo/tests/script_properties.rs:72
  `case_component_0292_only_optional_types_and_defaults_are_preserved` 已经证明：
  - `cardTitle.type_text == "string | number"`
  - `score.type_text == "string | number"`
  - `loose.type_text == "any"`
  - `items.type_text == "any[]"`
  - `defaulted.type_text == "number"`
  - `result.properties.complete == true`
  并且 `property()` 读的就是 `analysis.properties.properties`，即 behavior 合并后的集合。
-   crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:620 `analyze_component`
  直接使用 `analysis.properties`；`:627` 的 `properties.complete` 为 true 时返回
  declarations。因此 behavior properties 会进入绑定检查。
-   crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:658 的 `declared_targets`
  支持 `string`、`number`、`boolean`、`any[]`、`Record<string, any>`，`string | number`
  会拆成两个 target；`:684` 的 `declared_property` 依次尝试 exact/camel/kebab，
  `card-title` 会命中 `cardTitle`。
-   crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1647 的 0147 矩阵已经
  接受“没有 per-tag 数组、按 usage 断言”作为 exact。

**补测计划（不改生产）**：

1. 复用 0292 的 fixture：component 在 `behavior-card.ts`，page 为 `PAGE`，
   fragments 为 `pageTitle/total/flag/list`，usages 为 `behavior-ok/behavior-bad/
   behavior-loose/behavior-missing-required`。
2. 对 `behavior-ok` 断言 0 诊断、status None、4 次 query（`card-title`、`score`、
   `items`、`defaulted`；`unknown` 是 undeclared，不 query）。
3. 对 `behavior-bad` 断言 3 条 exact message；`score="literal"` 是 string literal，
   被 `string | number` 接受，不应产生第 4 条。
4. 对 `behavior-loose` 断言 0 诊断、`Partial` reason 精确为
   `some component bindings could not be resolved and were not checked (behavior-loose.loose)`；
   这同时钉住 `skipped==1` 和 detail。
5. 对 `behavior-missing-required` 断言 0 诊断、status None；按 0147 先例，
   这等价于 `results.has(...) == false`（checked==0 且无诊断）。
6. 若以上全绿，`parity=exact`、`green_eligible=true`、`status_recommendation=green`、
   `status_applied=green`，并把 `rust_tests` 指向新测试。

**风险**：中低。生产路径已经由 declaration 测试证明；真正的风险是 fixture 细节
（`list` 的类型文本、`behavior-bad` 的 literal 属性、skip detail 的 tag 名）。
必须先跑新测试再改台账；如果任一 message/range 不符，退回 `weaker`。

**建议 note**：

> Production merges behavior properties into the property overlay and
> `analyze_component` runs the binding pass over them. The existing test only
> proves the declarations. A targeted test with the TS fixture should produce the
> three exact messages, the clean usages, the single `behavior-loose.loose` skip
> and no result for the zero-attribute usage; under the 0147 per-usage precedent
> this is exact.

### 3.6 `TS-COMPONENT-0498` → 先 `weaker`，hover 测试后 `near_exact`

**TS oracle**（ backends/typescript/test/componentBindings.test.ts:498）：

- `dynamic?.hover?.typeOnly == "any"`
- `states.hover.reason` 匹配 `/behavior/i`

**现有 native 证据**：

-   crates/wxml-backend-tsgo/tests/script_behavior.rs:236
  `case_component_0498_spread_and_dynamic_data_members_stay_partial_and_any`：
  - behavior data 的 `value` 为 `number`
  - `dynamic.type_text == "any"`、`dynamic.exact == false`
  - `behavior.data_incomplete == true`
-   crates/wxml-backend-tsgo/tests/script_properties.rs:203
  `case_component_0498_only_property_spread_keeps_known_properties`：
  property spread 使 `properties.complete == false`，status Partial。
-   crates/wxml-backend-tsgo/src/shadow/layout.rs:575：build reason 为
  `"component behaviors are partially resolved: ..."`；
   crates/wxml-lsp/src/backend_tsgo/input.rs:790 会把它加进 hover degradation，
  所以 hover status 至少是 Partial，且 reason 匹配 `/behavior/i`。

**为什么只能 weaker，不能直接 exact**：

- 现有测试是脚本事实层；TS 断言的是 hover 层。脚本事实层不能替代 hover 层
  （见  docs/evidence/2026-11-20-tsgo-near-exact-three.md 的判定口径）。
- behavior data **没有**被合并进 shadow 的 `this.data`；`dynamic` mount 是
  `__wxml_root.dynamic`，native 得到 `any` 的原因很可能是“未知属性访问的错误类型”，
  不是“已知 behavior data 字段的 any”。可观察文本相同，但来源不同。
- 如果真实 engine 在未知属性访问处返回 `Missing` 而不是 `Any`，hover 就不会发布
  `any`。因此必须先补 hover 测试（fake 或 real engine）确认。

**判定**：现在记 `weaker`（脚本事实 + behavior 不完整）；hover 测试确认
`quality=Any` + `status=Partial` + reason `/behavior/` 后记 `near_exact`。
不建议直接 exact，除非 owner 接受“错误恢复 any = behavior data any”的等价性。

**建议 note**：

> Script facts prove `dynamic` is `any`, not exact, and behavior data is incomplete;
> the TS case asserts hover typeOnly any plus a behavior reason. No hover adapter
> test exists and behavior data is not merged into the shadow, so the `any` is
> error recovery rather than a modeled behavior field. Kept weaker; a hover test
> could be near_exact.

---

## 4. 必须保持 blocked 的 16 个

### 4.1 缺 native API / 契约 / DTO 字段（2）

| Case | 现有 typed 结果 | 为什么不能提升 | 需要什么 |
| --- | --- | --- | --- |
| `TS-COMPONENT-0229` | optional-array/absent clean；optional-object/optional-mismatch 均 `Partial`，reason 精确列出两个 `tag.attribute` | pinned wire 只有整型 `flags` + `typeToString` 文本，没有 union 每个成员的结构 flag，也没有 `isTypeAssignableTo`/`isArrayLikeType`；名为 `User` 的成员无法证明是 Object 或不是 String。TS 要求 optional-object clean + 一条具体 mismatch 诊断；native 的 Partial 是**反面**，不是 weaker | 新的 native 类型关系 API 或 contract 扩展；不能靠 `component_bindings` 猜 |
| `TS-DEFINITIONS-0036` | layout 有 `generated_declarations`（ crates/wxml-backend-tsgo/src/shadow/layout.rs:301）和 `source`（`:309`）；definition 结果只有 file/range/status | `DefinitionResult` 没有 `kind`，`SourceLocation` 只有 file/range（ crates/wxml-types/src/backend.rs:160）；`definition.rs` 没有 generated 分类，也没有用 `generated_declarations.source`。contract §9.5 明确要求生成声明导航走显式关系（ docs/reference/tsgo-native-contract.md:584） | 新 DTO 字段 + definition 组合里的 generated relationship；可选先做 weaker 的 generated source 断言，但不是 exact |

补充：

- `TS-COMPONENT-0229` 的现有测试
    crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1843 已经把
  “不猜 Object”固定住，这是正确的保守行为；不要为了提升把它改写成 clean。
- `TS-DEFINITIONS-0036` 的 native `real_engine_definition_decodes_declaration_identifier_ranges`
  （ crates/wxml-backend-tsgo/tests/real_engine.rs:472）是 opt-in gate，
  且只覆盖真实 declaration range；不能替代 generated kind/range。

### 4.2 需要真实 checker 的 diagnostics / hover / narrowing（7）

| Case | 现有 native 证据 | 为什么不能提升 |
| --- | --- | --- |
| `TS-GUARDS-0097` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:424 只固定 if/elif 渲染结构和 mount 文本 | TS 断言 `ready-name/error-code` 的 hover 类型、空诊断和 loading 分支的 `user` 诊断；这些是 checker narrowing 结果。native 没有运行 checker，`BuildStatus` 还是 Supported，没有 typed Partial 可借用 |
| `TS-GUARDS-0218` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:486 固定 `&&` 条件渲染、逻辑或不渲染、`@ts-ignore` 和 `BuildStatus::Partial` | TS 的核心是 `&&` / 逻辑或的 narrowing hover 和 negated 诊断；Partial 只是未解析 guard 的副作用，不是 narrowing 证据。把它记 weaker 会把“降级”冒充“narrowing 部分覆盖” |
| `TS-GUARDS-0261` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:525 固定 local capture、closer 顺序和 `BuildStatus::Supported` | TS 断言嵌套 loop/guard 的 hover `string`、`probeProblems=[]`、诊断为空；全部需要 checker。native 没有 Partial |
| `TS-GUARDS-0312` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:577 固定 `JsCheckApplied::Pragma`、`// @ts-check` 前缀和 note | TS 的核心是 typo diagnostic 被发布；native 只证明 pragma 注入，没有诊断。缺的是 checker 结果，不是 DTO 字段；把前三项记 weaker 会漏掉 case 的目的 |
| `TS-LAYOUTS-0049` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:78 固定 Supported 布局、`this.data` root、probe insertion 的 byte-exact reconstruction | TS 断言 `codesOf(project(file,text), file) == []`，即 checker 对注入探针无诊断。native 的 placement 正确不等于 checker clean；没有 Partial 可借用（BuildStatus Supported） |
| `TS-LAYOUTS-0072` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:118 固定探针在 Component `methods` 内 | 同 0049：TS 断言 checker 无诊断；native 只证明放置。没有 Partial |
| `TS-LAYOUTS-0117` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:442 固定 JSDoc typedef/@property 文本和 `__wxml_props_0` cast | TS 断言 hover `string \| number`、文档和 checker 无诊断；native 只证明 overlay 文本，没有 hover/diagnostics。没有 Partial |

共同点：这些 case 的 TS 断言是 checker 的**正向结果**（类型/诊断/clean），
native 的 typed `Partial` 要么不存在（0097/0261/0049/0072/0117），要么只是
“输入不完整”的副作用（0218），不能当作 weaker 覆盖。保持 blocked 是诚实的。

### 4.3 需要新生产行为（5）

| Case | 现有 typed 结果 | 当前生产行为 | 为什么不能提升 |
| --- | --- | --- | --- |
| `TS-COMPONENT-0570` | `BehaviorResolution.status=Partial`、`data_incomplete=true`、`fields=[]`（ crates/wxml-backend-tsgo/tests/script_behavior.rs:340） | build 会得到 `BuildStatus::Partial` + behavior reason；但 mount `__wxml_root.x` 没有 `@ts-ignore`（ crates/wxml-backend-tsgo/src/shadow/layout.rs:1279 只对 unresolved binding/guard 注入），engine 很可能发布 2339 | TS 的核心是“unresolved behavior 不伪造 2339”，并显式断言 `filter(code===2339)==[]`。native 当前很可能正好发布 2339；这是 false positive 风险，不是缺证据。需要像 TS `analysis.ts:1070` 的 `unknownBehaviorRoot` 抑制逻辑 |
| `TS-COMPONENT-0676` | 现有测试只证明 own `title` 的 declaration 和 `!behavior.status.is_supported()`（ crates/wxml-backend-tsgo/tests/script_properties.rs:180） | `analyze_component` 在 `registration.complete && properties.complete` 失败时整段返回 None（ crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:627）；external behavior unresolved 会让 `properties_incomplete=true`（ crates/wxml-backend-tsgo/src/script/behavior.rs:241），于是整个 tag 只 skip，不检查 own `title` | TS 要求 own property 诊断 + `skipped>=1` + details 含 `bad.behavior`（TS `componentBindings.ts:250`）。native 当前不检查 own property，是 false negative 风险；不能记 weaker |
| `TS-SYMBOLS-0116` | missing-handler 插入半段已覆盖 `complete=true`、`declaration=None`、`onSave` 文本、无 methods 包装、精确 close brace（status 未单独断言）（ crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:108）；observer 半段 `component_observers_make_data_paths_partial`（`:573`）断言 dataPath Partial 且无 observerPath | `SymbolOccurrenceKind::ObserverPath` 已在 DTO（ crates/wxml-types/src/symbols.rs:132），但生产不生成；observers 只让 dataPath Partial | TS 要求 `property.complete=true` + declaration + `observerPath` occurrence；native 的 observer 半段是反面。插入半段可以单独 split 成 near_exact（补 status 后 exact），但整个 case 不能 weaker |
| `TS-SYMBOLS-0299` | `dynamic_data_members_and_escaped_this_data_are_partial`（ crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:344）和 `aliases_destructuring_and_bare_setdata_are_partial`（`:397`）断言 destructuring 为 Partial、无 declaration | 生产把 `const { user: localUser } = this.data` 视为 Partial | TS 要求 renamed destructuring `complete=true` + declaration；native 明确相反。需要新的 destructuring 绑定跟踪，不是 Partial 断言 |
| `TS-SYMBOLS-0425` | `another_script_setdata_blocks_complete`（ crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:438）断言 complete=false、reason 含 “another script” | 另一个 script 的 `this.setData` 阻断当前 page | TS 要求 per-page isolation、complete=true + declaration；native 明确相反。需要生产行为变更 |

补充：

- `TS-COMPONENT-0570` 的现有测试甚至没有调用 shadow build；它只断言脚本事实。
  在把它记为 blocked 的“已知风险”前，建议补一个 `build_in_place_shadow` +
  fake diagnostics 的最小测试，确认 2339 是否真的被发布。但**不要**在没有
  suppression 的情况下记 weaker。
- `TS-COMPONENT-0676` 的 `bad.behavior` skip detail 是 TS `declaration.incomplete`
  的产物；native 当前只 skip `bad`。即使未来补了 own property 检查，也要补
  `tag.behavior` detail 才能 exact。

### 4.4 旧布局/探针放置已退役或替换（2）

| Case | 现状 | 为什么不能按 parity 提升 | 建议 |
| --- | --- | --- | --- |
| `TS-COMPONENT-0279` | TS 用 `layoutId:"symbol-bridge"` 断言恰好 1 条 path-free 消息；native 只有 in-place，symbol-bridge 已退役。in-place 的 path-free 已由 `component_diagnostics_never_leak_a_path` 和 `fixture_component_usages_publish_one_property_error` 覆盖（ crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1941 与 :2291） | `weaker` 需要把“symbol-bridge 的 path-free”映射到“in-place 的 path-free”，这是布局替换，不是同一可观察面；`exact` 更不可能。该 case 的 `disposition` 是 `rewrite`，应该由 owner 决定是改成 in-place 还是 retire | 建议 `retired-with-decision`（若接受 symbol-bridge 退役）或 `rewrite` 到 in-place 的 path-free 断言；在裁决前保持 blocked |
| `TS-LAYOUTS-0062` | TS 把探针直接放在 Component options，期望 2353；native 生产始终把探针放进 `methods`，必要时包装（ crates/wxml-backend-tsgo/tests/shadow_layout.rs:142） | TS 断言的是“非法放置会被 checker 拒绝”；native 的生产机制**避免**了这个非法放置，属于机制替换。native 没有 checker 2353 证据，也没有 Partial | 建议 `retired-with-decision`（mechanism_replaced）或改写成“探针必须放在 methods 内”的 placement 断言；在裁决前保持 blocked |

---

## 5. 需要 owner 裁决的拆分/退役候选

这些不计入 §0.2 的 6 个提升；它们需要新的 case ID 或 disposition，不是简单的
parity 重新登记。

### 5.1 `TS-COMPONENT-0229`：split 成 Array/absent 与 optional-object

- 现有测试已经证明：
  - `Item[] | undefined` 对 `Array` clean；
  - 纯 `undefined` 对 `Number` 视为 absent，clean；
  - 两个 `User | undefined` usage 都保守 Partial，reason 精确。
- 可拆成：
  - **Array/absent half**：`weaker` 或 `exact`（如果 owner 接受 per-usage 断言）；
  - **optional-object half**：blocked，等 union 成员结构 flag / assignability API。
- 风险：拆分会改变 ledger case 数和 `TOTAL_CASES`/validator 常量；必须由 owner 裁决，
  不能在 parity 回填里顺手做。

### 5.2 `TS-SYMBOLS-0116`：split 成 observerPath 与 missing-handler insertion

- 插入半段已经覆盖 exact 所需的大部分断言（status 未单独断言）：
  -  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:108
    `component_missing_handler_uses_existing_methods_without_wrapping`
    （complete=true、onSave 文本、无 methods 包装、精确 close brace）；
  - `:127` `component_missing_handler_wraps_a_new_methods_object`（新 methods 包装）。
- observer 半段是 blocked：
  - `:573` `component_observers_make_data_paths_partial` 证明 observer 让 dataPath
    Partial 且不发布 `observerPath`。
- 建议：split 后分别登记；插入半段先记 near_exact（补 `status=supported` 后 exact），不要用插入半段给整个 case 记 weaker。

### 5.3 `TS-COMPONENT-0279` / `TS-LAYOUTS-0062`：retire/rewrite

- 0279：symbol-bridge 已退役；in-place path-free 已覆盖。建议 retire 或 rewrite。
- 0062：生产探针放置已经避免 Component options；建议 retire 或 rewrite 成 placement
  不变量。
- 这两个不是“等 native API”的问题，而是**旧机制是否还需要保留覆盖**的裁决。

---

## 6. 如果应用建议：ledger/validator 同步清单

本节只是清单，**没有执行**。任何一项都需要与代码/测试同一个 commit，并重新跑
`cargo test -p wxml-lsp --test native_parity ledger`。

### 6.1 逐 case 字段

| Case | `parity` | `green_eligible` | `status_recommendation` | `status_applied` / `status` | `target_rust_test_status` |
| --- | --- | --- | --- | --- | --- |
| `TS-COMPONENT-0193` | `weaker` | `false` | `planned` | `planned` | `proposed` |
| `TS-GUARDS-0201` | `weaker` | `false` | `planned` | `planned` | `proposed` |
| `TS-LOCALS-0343` | `weaker` | `false` | `planned` | `planned` | `proposed` |
| `TS-COMPONENT-0498` | `weaker`（或 hover 后 `near_exact`） | `false` | `planned`（near_exact 时 `green`） | `planned` | `proposed` |
| `TS-EVENTS-0188` | `near_exact` | `false` | `green` | `planned` | `proposed` |
| `TS-COMPONENT-0292` | `exact` | `true` | `green` | `green` | `migrated_green` |

`migration_evidence.implementation_head` 必须指向**真正加入新测试/证据的 commit**，
不能复用 `43625ee` 或 `51abc13`；`assessed_at` 更新为实际日期。
`rust_tests` 必须非空，且每条 `path` 是仓库相对 `.rs`、`line` 在文件内、
`test` 名在文件中存在、`note` 长度 > 20；`fully_qualified` 以 `test` 结尾。

### 6.2 计数

保守方案（0188 记 near_exact，0498 记 weaker）：

| 项 | 当前 | 应用后 |
| --- | ---: | ---: |
| `counts.statuses.green` | 70 | 71 |
| `counts.statuses.planned` | 231 | 230 |
| `counts.statuses.retired-with-decision` | 38 | 38 |
| `counts.migration_evidence.exact` | 70 | 71 |
| `counts.migration_evidence.near_exact` | 2 | 3 |
| `counts.migration_evidence.weaker` | 96 | 100 |
| `counts.migration_evidence.structural` | 7 | 7 |
| `counts.migration_evidence.blocked` | 22 | 16 |
| `counts.migration_evidence.assessed_cases` | 197 | 197 |
| `counts.migration_evidence.remaining_planned_without_evidence` | 104 | 104 |

如果 0188 记 exact：`green` 72、`planned` 229、`exact` 72、`near_exact` 2；
其余相同。

### 6.3 Validator 常量

-   crates/wxml-lsp/tests/native_parity/ledger.rs:26 `GREEN_CASES` 70 → 71（或 72）。
-   crates/wxml-lsp/tests/native_parity/ledger.rs:28 `PLANNED_CASES` 231 → 230（或 229）。
-   crates/wxml-lsp/tests/native_parity/ledger.rs:33 `NEAR_EXACT_CASES` 2 → 3（0188 exact 时保持 2）。
-   crates/wxml-lsp/tests/native_parity/ledger.rs:38 `WEAKER_CASES` 96 → 100。
-   crates/wxml-lsp/tests/native_parity/ledger.rs:44 `BLOCKED_CASES` 22 → 16。
-   `RETIRED_CASES` 保持 38；wave-11 retire 的常量已经由并行 session 更新。

`green` case 还会触发 `non_obsolete == PLANNED_CASES + GREEN_CASES`、
`targets.len()`、`decision_ids` 等断言；只改常量不够，必须同步 JSON 的 per-case
`status`/`target_rust_test_status` 和 `counts`。

### 6.4 `by_partition`

保守方案下的分区变化：

| 分区 | 变化 |
| --- | --- |
| `components` | `blocked` 7 → 6（0193）；`weaker` 1 → 2 |
| `behaviors` | `blocked` 4 → 2（0292 exact、0498 weaker）；`weaker` 6 → 7；`exact` 0 → 1 |
| `guards_js` | `blocked` 5 → 4（0201）；`weaker` 1 → 2 |
| `loops` | `blocked` 1 → 0（0343）；`weaker` 2 → 3 |
| `events` | `blocked` 1 → 0（0188 near_exact）；`near_exact` 0 → 1 |
| `definitions` / `symbols_edits` | 不变 |

同时更新 `counts.migration_evidence.by_partition` 里的 `evidence_cases`、
`planned_evidence_cases`、`green_cases`、`exact`/`near_exact`/`weaker`/`blocked`。

### 6.5 文档

-   docs/reference/tsgo-test-contract.md §1 的计数表（`green`/`planned`/
  `exact`/`near_exact`/`weaker`/`structural`/`blocked`/无证据）。
-   docs/plans/2026-11-17-tsgo-test-ledger.md 的对应摘要。
-   docs/evidence/README.md 的 wave-11 条目。
- 本文件只记录建议，不是 ledger 的第二事实源；ledger JSON 仍是唯一事实源。

---

## 7. 台账行号漂移与 `target_rust_test` 观察

### 7.1 行号漂移

HEAD `51abc13` 相对任务输入快照的 `implementation_head` `43625ee` 增加了
 crates/wxml-lsp/src/backend_tsgo/tests.rs 等改动；部分 `migration_evidence.rust_tests[].line`
已不指向测试声明。`ledger.rs` 只校验“文件名存在 + 行号在文件范围内 + 文件里能搜到
`fn <test>(`”，不校验行号就是该测试，因此漂移不会被门禁发现。

| Case | 测试 | 台账行 | HEAD 实际行 | 偏差 |
| --- | --- | ---: | ---: | ---: |
| `TS-COMPONENT-0279` | `component_binding_any_type_degrades_to_partial` | 6476 | 6966 | +490 |
| `TS-COMPONENT-0279` | `fixture_component_usages_publish_one_property_error` | 1663 | 2291 | +628 |
| `TS-COMPONENT-0498` | `case_component_0498_only_property_spread_keeps_known_properties` | 179 | 203 | +24 |
| `TS-COMPONENT-0570` | `case_component_0570_only_unresolved_behavior_is_partial_and_invents_no_field` | 279 | 340 | +61 |
| `TS-COMPONENT-0676` | `case_component_0676_only_unresolved_external_behavior_keeps_own_property` | 156 | 180 | +24 |
| `TS-DEFINITIONS-0036` | `real_engine_definition_decodes_declaration_identifier_ranges` | 470 | 472 | +2 |
| `TS-EVENTS-0188` | `case_events_0188_only_inline_behavior_methods_resolve` | 558 | 562 | +4 |
| `TS-GUARDS-0097` | `guards_0097_rendered_structure_only_narrowing_blocked` | 413 | 424 | +11 |
| `TS-GUARDS-0201` | `case_guards_0201_an_unparseable_guard_does_not_fabricate_a_diagnostic_and_reports_partial` | 448 | 459 | +11 |
| `TS-GUARDS-0218` | `guards_0218_rendered_structure_only_narrowing_blocked` | 475 | 486 | +11 |
| `TS-GUARDS-0261` | `guards_0261_rendered_structure_only_narrowing_blocked` | 514 | 525 | +11 |
| `TS-GUARDS-0312` | `case_guards_0312_pragma_injects_a_shadow_only_ts_check` | 566 | 577 | +11 |
| `TS-LOCALS-0343` | `case_locals_0343_a_locals_only_request_still_degrades_to_partial_with_a_typed_reason` | 307 | 317 | +10 |

（另：exact 的 `TS-COMPONENT-0147` 的  crates/wxml-lsp/src/backend_tsgo/tests.rs:6439 也已漂移到 6929；
这说明 `tests.rs` 在 wave-10b 之后整体后移，属于同一类问题。）

建议在真正改这些 case 的证据时顺便把行号重新锚定；不要只改 parity 不改 line。

### 7.2 `target_rust_test` 不是证据存在性的证明

已提升 exact 的 `TS-COMPONENT-0147/0206/0251/0268` 的 `target_rust_test` 指向
 crates/wxml-lsp/tests/native_parity/components.rs::case_...，但该文件目前是空的
`MIGRATED_CASES` skeleton；真实证据在 `component_bindings.rs` 的 unit tests，
由 `migration_evidence.rust_tests` 指向。`ledger.rs` 只校验 target module 的
routing，不校验 target test 是否存在。

对本审计的 6 个建议：

- `weaker`/`near_exact` 继续把 `rust_tests` 指向现有/新增的 unit test；
- `TS-COMPONENT-0292` 若要 exact，按 0147 的模式在
   crates/wxml-lsp/src/backend_tsgo/component_bindings.rs 或
   crates/wxml-backend-tsgo/tests/script_properties.rs 增加真实测试，
  并把 `rust_tests` 指向它；不要用 skeleton 的名字冒充证据。

---

## 8. 验证与限制

- 本审计没有运行任何测试；所有“生产支持/不支持”的判断来自静态阅读
  production + Rust test + TS oracle。
- `TS-COMPONENT-0570` 的“可能发布 2339”和 `TS-COMPONENT-0676` 的“整段跳过、
  不检查 own property”是代码路径推断，不是运行结果。建议在把它们写成
  ledger 的 known divergence 前，各补一个最小回归测试确认；但即使不确认，
  也不应把这两个 case 记 weaker。
- `TS-COMPONENT-0292` 的 exact、`TS-EVENTS-0188` 的 near_exact、
  `TS-COMPONENT-0498` 的 near_exact 都需要先补测试；本文件只给判定和测试计划。
- §0.1/§6.2 的计数是投影；当前工作树 JSON 是 70/231/38、`blocked` 22、
  `weaker` 96。任务输入快照是 70/233/36；差别只来自并行 retire 的 2 个 case。
- 没有 commit、没有 review；本文件不修改任何 ledger/README/contract。

---

## 附录 A：`TS-COMPONENT-0292` exact 断言映射

| TS 断言 | native 补测断言 | 生产依据 |
| --- | --- | --- |
| `behavior-ok` diagnostics `[]` | 0 诊断 + `status=None` + 4 次 query |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1647 的 per-usage clean 规则；`declared_targets` 的 `any[]`/`string \| number` |
| `card-title` 期望 string、got number | 逐字 message，range 为 `total` 表达式 | `declared_property` camel 查找 `cardTitle`；`check_attribute` 的 mustache 诊断 |
| `score` 期望 string \| number、got boolean | 逐字 message，range 为 `flag` 表达式 | `declared_targets` 拆 union；`Target::String` / `Number` |
| `defaulted` 期望 number、got string literal | 逐字 message，range 为 attribute | literal 分支使用 stringType；`Target::Number` 拒绝 |
| `behavior-loose` 无结果 | 0 诊断 + 精确 Partial reason `(behavior-loose.loose)` | `loose` 的 `any` → `declared_binding` None → `Unproven` → `skip`；`finish` 把 skipped 变成 Partial |
| `behavior-missing-required` 无结果 | 0 诊断 + `status=None` | 0 attribute → 0 checked/0 diagnostic → 无 skip；按 0147 per-usage 等价 |
| `skipped == 1` | Partial reason 只列 `behavior-loose.loose` | `Pass::finish` 的 details 汇总 |

## 附录 B：`TS-EVENTS-0188` near_exact 断言映射

| TS 断言 | native 合并测试断言 | 生产依据 |
| --- | --- | --- |
| `state.status == supported` | `EventResolution.status.is_supported()` |  crates/wxml-backend-tsgo/src/script/events.rs:166  |
| `definition.fileId == src/behaviors/base.ts` | `EventDefinition.path == "src/behaviors/base.ts"` | `EventMethod.path`（ crates/wxml-backend-tsgo/src/script/events.rs:111 ）、`resolve_one`（ crates/wxml-backend-tsgo/src/script/events.rs:512 ） |
| `signature /fromBehavior/` | `EventResolution.signature` 含 `fromBehavior` | `EventMethod.signature` + `cap_signature` |
| `documentation /来自 behavior/` | `EventResolution.documentation == "来自 behavior"` | `doc_before`（ crates/wxml-backend-tsgo/src/script/events.rs:467 ） |

## 附录 C：`TS-GUARDS-0201` / `TS-LOCALS-0343` weaker 映射

| TS 断言 | native 现有证据 | 残余 |
| --- | --- | --- |
| `states.diagnostics.status == partial` | `BuildStatus::Partial`（ crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:459 与 :317 ） | adapter status 未断言 |
| reason `/could not be resolved and were not checked/` | build reason 精确同串（ crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:459 与 :317 ） | adapter 前缀是 `"shadow coverage is partial: "`，仍匹配 |
| `diagnostics == []` / fragment 空 | 0201 的 `@ts-ignore` mount；0343 的 `undefined as any` mount | 真实 checker 空结果 / per-fragment DTO 未断言 |
| `unresolvedGuards == 1`（仅 0201） | 内部计数  crates/wxml-backend-tsgo/src/shadow/layout.rs:682  | 未进入 `BuiltShadow`，无法精确断言 1 |
