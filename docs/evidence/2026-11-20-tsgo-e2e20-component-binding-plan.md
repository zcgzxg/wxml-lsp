# P6-2 `G-P5-E2E-20` 自定义组件属性绑定诊断实现计划

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务给定 HEAD：`f47e4c0`（`docs: record F3 closure and native gate plans`）
- 读取/写入时实际 HEAD：`4140e9b`（`refactor(tsgo): split symbol query and this-scope helpers`）。并行会话在切片期间提交了 `304aa20`（native symbols 与 overlay replay）、`6466ff3`（F3-02 文档）、`3220362`（event handler hover）、`4fd45bd`（non-literal registrations）、`4140e9b`；HEAD 仍可能继续移动，本文引用的 gate 计数以实际工作树（`18 gate_green / 5 required_not_migrated`）为准，关键行号已在 `4140e9b` 复核。
- 日期：2026-11-20
- 范围：P6-2 剩余 gate `G-P5-E2E-20`（`ac012_component_property_bindings`）的 native 替代；`component-bindings` fixture 的 `title="{{ total }}"` 属性类型不匹配诊断。
- 性质：**read-only 设计与可行性切片**。本文之外不修改生产代码、测试、Cargo、台账或 gate 断言，不 commit。
- 工作树漂移：读取/写入时工作树仍有并行未提交改动（`crates/wxml-backend-tsgo/src/script/{mod,references,symbol_facts,symbol_query,this_scope}.rs`、`crates/wxml-lsp/tests/native_parity*`、`crates/wxml-lsp/tests/native_parity/native_measure.rs`、`docs/plans/tsgo-test-ledger.json`、`docs/evidence/2026-11-20-tsgo-p6-perf-stability.md` 等）。本文引用的是**当前工作树行号**；提交前必须按提交后的冻结代码重新核对。本文不把并行改动当作本切片的实现或通过证据。

## 0. 一页结论

| 问题 | 结论 |
| --- | --- |
| 为什么 Rust 现在没有该诊断 | 模板层已经解析出自定义组件 usage 和绑定表达式，且表达式已挂载到 shadow；缺的是“读取子组件 `properties` + 查询绑定表达式类型 + 比较并发布诊断”的检查 pass。不是缺少标签/属性解析，也不是 shadow 的 assignment check 漏了。 |
| 是否必须新增 native checker query | **不需要新增 wire method**。`getTypeAtPosition` + `typeToString` 已在 `NativeMethod` allow-list 和 `QuerySession::type_query` 中，hover 已在用。pinned API 没有 `isTypeAssignableTo`，比较必须在 Rust 侧完成。 |
| 能否纯 Rust 完成 | **对 fixture 可以**：`total: number` 和 `title: string` 都能从 `ScriptAnalysis` 的 `data`/`properties` overlay 静态取得。**对 AC-012 的一般语义不可以**：member chain、循环 locals、导入类型、联合、结构性类型都没有完整静态类型。推荐用“纯 Rust 声明提取 + 现有 native `type_query`”的混合方案。 |
| 是否要新增组件绑定解析器 | 不需要。复用 `TemplateAnalysis.components`（`ComponentUsage`）、`ScriptAnalysis.properties`（`PropertyOverlay`）、`ShadowLayout` 的 fragment mount，只新增检查和比较逻辑。 |
| gate 能否关闭 | **可以**，作为 opt-in native `gate_green`：实现后跑 required native job + fail-closed RED，再把 `G-P5-E2E-20` 从 `required_not_migrated` 移到 `gate_green`。不能先改计数。 |
| 全量 Node  parity | **不宣称**。Node 的 `checker.isTypeAssignableTo` 精确可赋值性无法在 pinned allow-list 上复现；Rust 侧只能做保守比较，无法证明时降 `Partial`，不伪造错误。 |

## 1. Gate 事实与当前基线

### 1.1 旧 gate 的冻结断言

旧 gate 在：

-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:772` `ac012_component_property_bindings`
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:774` `Runner::open("component-bindings", ...)`

它断言：

| 类别 | 断言 |
| --- | --- |
| hover | 2 条：`pageTitle` = `string` + `页面标题`；`count="{{ total }}"` 的 `total` = `number` + `卡片总数` |
| 属性诊断 | 非事件 marker 恰好 1 条；`target == "total"`；`message_contains` 含 `number` |
| 事件诊断 | 事件 marker 恰好 1 条；`target == "missingHandler"`；`severity == "error"` |
| 属性 definition | `expected/definition.json` 中没有属性 marker；旧 runner 断言 `property_definitions.is_empty()`（这是 expected 列表，不是对任意属性发起 definition） |
| 事件 definition | 2 条：`bind:tap="onCardTap"` 指向 `pages/index/index.ts` 的 `onCardTap`；`bindtap="missingHandler"` 为 null |

该测试必须保持 `#[ignore]`、保持 Node 路径、保持断言不变。P6 只新增 native replacement，不删除旧 gate。

### 1.2 fixture

`tests/fixtures/component-bindings/`：

```text
app.json                              # pages: pages/index/index；无 global usingComponents
pages/index/index.json                # usingComponents.card = ../../components/card/card
pages/index/index.wxml                # 两条 <card>，见下
pages/index/index.ts                  # Page({ data: { pageTitle: '首页', total: 3 }, onCardTap() {} })
components/card/card.json             # {"component":true}
components/card/card.ts               # Component({ properties: { title:{type:String}, count:{type:Number} } })
components/card/card.wxml
expected/{hover,diagnostics,definition}.json
```

WXML：

```xml
<view class="page">
  <card title="{{ pageTitle }}" count="{{ total }}" bind:tap="onCardTap" />
  <card title="{{ total }}" bindtap="missingHandler" />
</view>
```

期望诊断只有一条属性错误：

```text
component property "title" expects string, got number (bound expression "total")
range = pages/index/index.wxml 中第二条 <card> 的 title="{{ total }}" 里的 total
```

### 1.3 当前实测基线（非 gate 证据）

用已有 native 包对 fixture 做了手工 LSP 探针（包 `/tmp/opencode/tsgo-native-pkg`，二进制 mtime 2026-11-20 01:35；探针目录 `/tmp/opencode/e2e20-probe`，不在 repo 内）：

| 请求 | 当前结果 |
| --- | --- |
| `didOpen pages/index/index.wxml` + diagnostics | 只有 1 条 `Event handler "missingHandler" for "tap" is not defined on this page`；**没有** `title="{{ total }}"` 的属性错误；没有 Partial 状态 warning |
| hover `pageTitle` | `string` + `页面标题` |
| hover `count="{{ total }}"` 的 `total` | `number` + `卡片总数` |
| hover `title="{{ total }}"` 的 `total` | `number` + `卡片总数` |
| definition `onCardTap` | 精确 `pages/index/index.ts` 的 `onCardTap` |
| definition `missingHandler` | `result: null` |

探针命令与原始输出见附录 A。它不是 gate 证据：包二进制与 manifest 记录 size 不一致，且工作树有并行改动；它只用于证明“类型证据已经存在，缺的是检查 pass”。

### 1.4 当前 capability 文案

-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:980` `capabilities()`
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:989` 当前 diagnostics reason 明确写着 `event-handler and component diagnostics are not covered`

实现后必须把该文案改成“component property binding diagnostics are wired; composite expressions / unresolved declarations may degrade to Partial”，不能继续声称未覆盖。

## 2. 精确根因

### 2.1 Node 旧实现做了什么

`backends/typescript/src/componentBindings.ts`：

-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/componentBindings.ts:105` `collectComponentDiagnostics`：按请求里的 `components` 逐个 usage 检查。
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/componentBindings.ts:300` `declaredProperties`：从组件脚本的 `Component({ properties })` 读取 `scanDeclaredProperties`，并用 `resolveBehaviors` 合并 Behavior properties；组件自身声明优先。
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/componentBindings.ts:424` `boundType`：按 `exprRange` 找到 mounted fragment，取 mount 的 `startUtf16` / `targetStartUtf16` 处的最外层表达式，`checker.getTypeAtLocation` 得到真实类型。
- `accepts`：
  - mustache 绑定：`isTypeAssignableTo`；`undefined`/`null` constituent 视为“属性未设置”，允许；
  - literal：按 `string` 检查；boolean：按 `boolean` 检查；
  - `Array`/`Object`：结构性检查（`isArrayLikeType` / `Object` flag）；
  - `mixed`、`any`、`Function`、缺失 fragment 等：跳过并计入 `skipped`。
- 诊断 range：mustache 用 `exprRange`；literal/boolean 用 `attributeRange`；message 形如 `component property "title" expects string, got number (bound expression "total")`。
- 上限：64 个不同 tag、每个 tag 8 条诊断、表达式文本 80 字符、3 个 skipped 示例、message 512 字符。

Node 测试 `backends/typescript/test/componentBindings.test.ts`：

-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/test/componentBindings.test.ts:147` per-tag 诊断与 skipped 计数；
-  ` :206` 无 mustache 的 literal 检查；
-  ` :229` optional `undefined`/`null` 允许；
-  ` :292` Behavior properties / kebab-camel 映射 / `type:null` / `Function` 跳过。

### 2.2 Rust 已经有什么

| 事实 | 位置 | 对 E2E-20 的价值 |
| --- | --- | --- |
| 模板分析收集自定义组件 usage，且属性必须是配置过的 tag | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/template.rs:674` `collect_component` | 不需要重新解析标签 |
| 属性值分类 `literal` / `mustache` / `mixed` / `boolean`，并带 `expr_text`、`expr_range`、`attribute_range` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/check.rs:149` `ComponentAttribute`、` :123` `AttributeValueKind` | 直接可做比较和 range 发布 |
| 事件属性、`slot`、`wx:*` 从 `ComponentUsage.attributes` 排除 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/template.rs:829` `is_component_ignored_attribute` | 避免把 `bind:tap` / `bindtap` 当作属性 |
| 组件 attribute 的 mustache 仍然是 fragment（会被挂载） | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/template.rs:1524` 测试断言 | 不需要额外挂载表达式 |
| `PreparedView.components` 携带 `ComponentUsage`，并已解析本地组件脚本 `component_file` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:345`、` :357`、` :2527` `resolve_components` | 能定位子组件脚本 |
| `analyze_script` 返回 `PropertyOverlay`，含 `type` + `optionalTypes`、own/behavior 优先级、`type:null`→`any`、`Array`→`any[]`、`Object`→`Record<string, any>` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/properties.rs:72`、` :110`、` :241`；` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/values.rs:655`、` :721` | 直接得到 `title: string`、`count: number`；properties-only 组件的 `complete` 已由 ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/script_properties.rs:95` 固定 |
| `ShadowLayout` 保存每个 fragment 的 `MountRecord`，含 `target_start_utf16` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mapping.rs:27`、` :36`、` :365` | 能把 `expr_range` 映射到 derived UTF-16 查询位置 |
| `NativeSession::type_query` = `getTypeAtPosition` + `typeToString` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1125`；` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:81` | 能取得 `total` 的真实类型 `number` |
| `map_type_query` 区分 `Missing` / `Any` / `Unknown` / `Known` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/type_info.rs:51` | 决定是否跳过，不把 `any` 当错误 |
| 模板 owner 的 shadow 根类型来自 `Page`/`Component` 的 `data`/`properties` overlay | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:535` `collect_script_facts`（调用点 ` :720`） | hover 已经证明 `total` 是 `number` |

### 2.3 缺什么

1. **没有 component binding 检查 pass。** `ComponentUsage` 在 native 路径只被 `PreparedView` 保存；`backend_tsgo` 没有消费它。`grep ComponentUsage` 在 Rust 里只出现在 `project_host`、`backend_node` 和测试。
2. **没有子组件 `properties` 的声明表。** `RequestPlan.declared_properties` 是 owner script 的声明，不是被使用组件的。`PreparedView` 只有 `ComponentUsage.component_file`，没有已解析的 `PropertyOverlay`。
3. **没有对绑定表达式发类型查询。** `diagnostics()` 只调用 `semantic_diagnostics` + `syntactic_diagnostics`；它没有对 fragment mount 调用 `type_query`。
4. **shadow 本身不会产生这个错误。** 页面 shadow 只把模板表达式挂到 `__wxml_root`（页面 `data`）或 `__wxml_props_*`（owner 组件属性）上；子组件实例和它的 `properties` 类型从未进入 shadow。因此即使表达式 `total` 是 `number`，TypeScript 也不会知道 `card.title` 期望 `string`。
5. **诊断通道没有汇总入口。** `diagnostics()` 只映射 engine 返回的诊断；没有本地补充 pass 的 status/merge 逻辑。

### 2.4 这不是“没解析”

模板分析已经在 `analyze_element` 中对每个非事件属性调用 `analyze_attribute`，因此 `title="{{ total }}"` 既是 `ComponentUsage` 的属性，也是 `CheckFragment`；`wxml-types` 的 `component_attributes_are_classified_by_value_shape` 明确断言 “Component attribute mustaches are still fragments”。所以根因是**缺少检查**，不是缺少 WXML 解析或表达式挂载。

## 3. 最小实现路径

### 3.1 选择：复用现有事实 + 新增一个 LSP 侧 pass

推荐：

- **声明类型**：复用 `analyze_script(Component)` 的 `PropertyOverlay`（纯 Rust）；
- **绑定表达式类型**：复用现有 native `type_query`（不新增协议）；
- **range**：直接使用 `ComponentAttribute.expr_range` / `attribute_range`（WXML UTF-8 bytes）；
- **不新增**自定义组件/WXML 解析器，不修改 `wxml-types` 的 `ComponentUsage` DTO。

新增文件建议：

```text
crates/wxml-lsp/src/backend_tsgo/component_bindings.rs
```

这样 E2E-20 的检查逻辑与 `diagnostics.rs`、`input.rs` 解耦，减少与 E2E-01/E2E-05 的文本冲突。

### 3.2 M1：`RequestPlan` 保留 `PreparedView`

文件：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs`

- `RequestPlan`（` :268`）增加一个字段：

```rust
/// Host-prepared view used by the component-property binding pass.
pub(crate) view: PreparedView,
```

- 在 `build_request_plan_with_completion`（` :658` 的 `let view = prepare_view(...)`）末尾把 `view` move 进 `RequestPlan`（` :902`）。不要在 plan 构造阶段额外跑 `analyze_script`；这样 hover/definition/completion 不承担组件属性检查的成本。
- 不改 `RequestIdentity`，不参与 `files_hash`；E2E-01 的 identity 语义不变。
- 只影响一个 `RequestPlan` 构造点；`PlanIdentity`/`OpenPlan` 不变。

替代方案（不推荐）：在 `input.rs` 构造时预计算 `ComponentBindingPlan`。它会让每次 hover 都跑组件脚本分析，且引入新的 identity/缓存问题；`view` 字段更小。

### 3.3 M2：新增 `component_bindings` pass

建议接口：

```rust
pub(crate) struct ComponentBindingDiagnostics {
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) status: Option<CapabilityStatus>,
}

pub(crate) fn diagnostics(
    session: &mut dyn QuerySession,
    layout: &ShadowLayout,
    view: &PreparedView,
    script_path: &str,
) -> ComponentBindingDiagnostics
```

内部数据流：

```text
view.components (ComponentUsage[])
  -> usage.component_file.path()
  -> find component script in view.files
  -> analyze_script(ScriptRequest { kind: Component })
  -> PropertyOverlay.properties -> { name -> DeclaredProperty { type_text, ... } }
  -> usage.attributes
       name -> exact / camelCase / kebabCase
       expr_range -> layout.mount_for_range(range) -> MountRecord.target_start_utf16
       -> offset_to_position(layout.derived(), target_start_utf16)
       -> session.type_query(script_path, position)
       -> map_type_query -> Known(text, flags)
       -> accepts(declared.type_text, actual.text, actual.flags)
       -> Diagnostic { range: expr_range, severity: Error, message: ... }
```

#### 3.3.1 子组件声明表的构造

- 用 `view.files` 构造一个 `AnalysisInput`（与 `semantics/event_definition.rs:325` 的 `analyze` 完全同构）：
  - `ProjectRoot::new(view.project_root.to_str()?)`；
  - 每个 `PreparedFile` → `SourceFile::new(path, text, revision)`，非 owner 的无效文件可丢弃；owner 无效则整体 `Partial`；
  - `AnalysisInput::new(ProjectKey::new("wxml-lsp-component-bindings"), root, SnapshotRevision::new(1), files)`。
- 对每个不同的 `component_file.path()` 调 `analyze_script(&ScriptRequest::new(&input, path, RegistrationKind::Component))`，按路径缓存结果（`BTreeMap<String, Option<ComponentProperties>>`）。
- `PropertyOverlay` 的语义直接复用：
  - `PropertyFact.type_text`：`string`、`number`、`boolean`、`any[]`、`Record<string, any>`、`string | number`；
  - `type: null` → `any`（`script_properties.rs:78` 已覆盖）；
  - `Function` → `(...args: any[]) => any`（按 unknown 跳过，匹配 Node 的 Function→any）；
  - `value` 默认值不参与类型检查（`default_text` 忽略）；
  - own properties 覆盖 Behavior properties；
  - `properties.complete == false`（spread/computed/非对象字面量/未解析 Behavior）时，整条 usage 保守跳过并 `Partial`，不伪造错误。

注意：

- 如果 `analysis.registration.complete == false`（多个 Component 注册/非顶层/有 rejected 注册），整条 usage 跳过并 `Partial`。
- 如果 `usage.component_file == None`（插件/远程/未发现脚本/无脚本），整条 usage 跳过并 `Partial`，但只在该 usage 有属性时计 skipped。
- 如果 `properties.is_empty()` 且 usage 有属性，按 Node 的“无 properties 声明”语义跳过并 `Partial`；不要静默当作 clean。
- 递归组件（子组件脚本就是当前 owner script）在 `view.files` 中是原始文本，不是 derived shadow；仍可分析，但建议加深度/数量上限。
- 不复用 `wxml_project::ComponentPropertyFacts` 作为声明表：它没有合并 Behavior properties，且把 `type:null` 保留为 `null` 而不是 `any`；只用它做 completion/metadata 的轻量场景，属性诊断必须以 `ScriptAnalysis.properties` 为准。

#### 3.3.2 表达式类型查询

- 只检查 **chain 表达式**：`Identifier` 或 `Member { object: chain, property: Identifier }`（不支持顶层括号、Index、Call）。这是 `wxml_syntax::parse_expression` 可以判定的；对 `Call`、`Binary`、`Logical`、`Conditional`、`Index`、`Parenthesized` 等一律 `skipped + Partial`，避免把右端标识符的类型误当整个表达式类型。
- 匹配 mount：新增 `ShadowLayout::mount_for_range(TemplateRange) -> Option<&MountRecord>`，按 `FragmentMap.wxml_range == ComponentAttribute.expr_range` 精确匹配。不要用“文本相同”或“第一个 total”匹配；fixture 里 `total` 出现两次。
- 查询位置：取 mount 的**最后一个 source segment** 的 `start_utf16`，再 `offset_to_position(layout.derived(), Utf16Offset::new(start))`。
  - 不要无条件使用 `mount.target_start_utf16`：`target_of` 只保留 identifier 文本，`resolve_target(Some(name))` 取第一次出现；`user.name.name` 这类重复成员名可能查到中间成员。
  - 不要查询 `mount.start_utf16`：那通常是合成前缀 `__wxml_root.` 的起点，会得到根对象类型。
  - `mount.target_start_utf16` 仅在它等于最后一个 source segment 起点时作为校验/回退。
- 校验：用 `layout.original_wxml()` 的字节切片确认 `expr_range` 处的文本等于 `ComponentAttribute.expr_text`；不一致则不发布诊断（stale/synthetic）。
- `type_query` 结果用 `map_type_query`：
  - `Any` / `Unknown` / `Missing` → `skipped + Partial`；
  - `Known` → 比较；
  - `NativeError::Unsupported` → 停止后续组件查询并 `Partial`；
  - 其他 hard error → `Partial`，不伪造错误；不把整个 diagnostics 结果设为 `fatal`，避免丢掉已成功的 backend 诊断。

#### 3.3.3 `accepts` 规则（Rust 侧保守比较）

声明目标从 `type_text` 按顶层 `|` 拆分：

| 声明片段 | 目标 |
| --- | --- |
| `string` | String |
| `number` | Number |
| `boolean` | Boolean |
| `any[]` | Array |
| `Record<string, any>` | Object |
| `any` / `null` / Function 文本 / 其他未知构造器 | Any/Unknown → 该属性跳过，`Partial` |

实际类型优先用 `TypeQuery.info.flags`，flags 不可用时退回 `typeToString` 文本：

| 实际类型 | 判定 |
| --- | --- |
| `String` / `StringLiteral` | string |
| `Number` / `NumberLiteral` | number |
| `Boolean` / `BooleanLiteral` | boolean |
| `Object` | object（数组也带 Object flag） |
| 文本以 `[]` 结尾、`Array<`/`ReadonlyArray<` 开头、或 tuple `[...]` | array |
| `Undefined` / `Null` | “属性未设置”，从 union 中剔除 |
| `Any` / `Unknown` / error | skip + `Partial` |
| `Union` | 顶层 `\|` 拆分，逐个 constituent；全部满足才算通过 |
| `Never` | 视为可赋值（Node `isTypeAssignableTo(never, T)` 为 true） |
| 其他/无法判定 | `Unknown` → skip + `Partial`，**不报错** |

`accepts` 返回三态 `Yes / No / Unknown`：

- 任一声明目标 `Yes` → 该实际 constituent 通过；
- 所有目标 `No` → 该 constituent 失败；
- 否则 `Unknown` → 整个属性跳过并 `Partial`；
- 实际 union 去掉 `undefined`/`null` 后为空 → 视为“未设置”，通过；
- 实际 union 中有一个 `No` → 整体失败（`No` 优先于 `Unknown`）。

结构性限制：

- `Array` 目标：已知数组文本 → Yes；primitive → No；其他 Object → Unknown（不因无法证明 array-like 而报错）。
- `Object` 目标：Object flag → Yes；primitive → No。
- 这比 Node 的 `checker.isTypeAssignableTo` 弱，但不会伪造错误；无法证明时只降 `Partial`。

#### 3.3.4 诊断、消息与上限

- mustache 不匹配：`range = expr_range`；
- literal 不匹配：`range = attribute_range`，message 后缀 `got string literal`；
- boolean 不匹配：`range = attribute_range`，message 后缀 `got boolean literal`；
- mustache message：

```text
component property "{name}" expects {declared}, got {actual} (bound expression "{expr}")
```

- 所有 message 不含路径/绝对路径/内部 helper 名；`expr` 截断到 80 字符；message 上限 512。
- 上限：64 个不同 tag、每个 tag 8 条诊断、最多 256 次 `type_query`、3 个 skipped 示例；超出即 `Partial`。
- 诊断按 `(start, end, message)` 去重，按 range 排序。

### 3.4 M3：`ShadowLayout` 增加 mount 查找

文件：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mapping.rs`

新增：

```rust
pub(crate) fn mount_for_range(&self, range: TemplateRange) -> Option<&MountRecord> {
    self.fragments
        .iter()
        .find(|fragment| fragment.wxml_range == range)
        .map(|fragment| &fragment.mount)
}
```

以及（用于 stale 校验）：

```rust
pub(crate) fn original_wxml(&self) -> &str {
    &self.original_wxml
}
```

不改 `map_native_range` / `ShadowMap` 的既有语义。

### 3.5 M4：接入 `diagnostics()`

文件：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/diagnostics.rs`

`diagnostics()`（` :46`）新增参数 `view: &PreparedView`（或 `component: &ComponentBindings`），在 native diagnostics 映射循环之后：

```rust
let component = crate::backend_tsgo::component_bindings::diagnostics(
    session,
    layout,
    view,
    script_path,
);
for diagnostic in component.diagnostics {
    let key = (
        diagnostic.code.clone(),
        diagnostic.range.start(),
        diagnostic.range.end(),
        diagnostic.message.clone(),
    );
    if seen.insert(key) {
        diagnostics.push(diagnostic);
    }
}
if let Some(component_status) = component.status {
    status = merge_status(status, component_status);
}
```

调用点：

-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:812` `diagnostics::diagnostics(...)` 增加 `&plan.view`。
- `script_diagnostics()` 不传 `view`，不会跑组件检查。
- `finish_diagnostics` / epoch / stale 逻辑不变。
- `DiagnosticsRun.fatal` 不因组件检查失败而置 true（除非未来决定让 transport 失败走现有重启路径）；建议保守 `Partial`。

### 3.6 M5：capability 与文档

- 更新 `capabilities()`（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:980`）的 diagnostics reason：说明 component property binding 已接线；复合表达式、Array/Object 结构判定、未解析 Behavior 可能 `Partial`。
- 更新 `docs/reference/type-diagnostics.md` §7.1 的实现状态（如果该文档当前声称不支持）。
- 不修改 Node capability、不切换默认 backend。

### 3.7 纯 Rust 方案对比

| 方案 | 能覆盖 fixture | 能覆盖一般 AC-012 | 主要风险 |
| --- | --- | --- | --- |
| 纯 Rust：`ScriptAnalysis.data.fields` + `properties` | 是（`total` exact number、`title` string） | 否 | `{{ total > 0 }}`、`{{ user.name }}`、`wx:for` locals、导入类型无法判定；可能把 root 类型误当表达式类型 |
| 纯 Rust：加表达式静态推导 | 部分 | 否 | 需要重写 TypeScript 类型系统，收益低 |
| **推荐：纯 Rust 声明 + native `type_query`** | 是 | 是（保守） | 每次诊断多若干 RPC；无 `isTypeAssignableTo`，只能保守比较 |
| shadow 合成 assignment + 让 engine 报 2322 | 是 | 理论上 | 需要新增合成代码/合成 mount/identity 变更，冲突面最大；不建议 |

**结论**：fixture 级别的 `total: number` / `title: string` 完全可以纯 Rust 取得；但“不要伪造、不要只对 fixture 写死”的实现应使用现有 `type_query`。这不是新增 native checker query，而是复用 hover 已经在用的方法。可选优化是：单根标识符且 owner `data` field `exact` 时直接用 `ScriptAnalysis.data.fields`，member chain 再走 native；本文不把这条混合路径作为基线，避免两套类型来源产生不一致。

### 3.8 是否新增 native checker query

| 需求 | 现有能力 | 是否新增 |
| --- | --- | --- |
| 子组件声明类型 | `ScriptAnalysis.properties`（纯 Rust） | 否 |
| 绑定表达式类型 | `NativeSession::type_query`（`getTypeAtPosition` + `typeToString`） | 否 |
| 精确可赋值性 `isTypeAssignableTo` | pinned allow-list 无此方法 | **不能在本切片新增**；Rust 侧保守比较 |
| 协议/引擎/manifest | 不需要 | 否 |

如果验收要求 Node `checker.isTypeAssignableTo` 的精确语义（包括任意结构类型、复杂 union、index signature），当前 pinned API 不足，只能 `Partial` 或走协议扩展；后者超出 E2E-20 的最小范围。

## 4. 文件、函数、数据流

### 4.1 修改点总表

| 文件 | 函数/位置 | 改动 |
| --- | --- | --- |
| `crates/wxml-lsp/src/backend_tsgo/input.rs` | `RequestPlan` ` :268`、`build_request_plan_with_completion` ` :658`/` :902` | 增加 `view: PreparedView` 并 move 进 plan |
| `crates/wxml-lsp/src/backend_tsgo/mapping.rs` | `ShadowLayout` ` :36` | 增加 `mount_for_range`、`original_wxml` |
| `crates/wxml-lsp/src/backend_tsgo/component_bindings.rs` | 新文件 | 声明提取、mount 查询、`type_query`、比较、诊断/状态 |
| `crates/wxml-lsp/src/backend_tsgo/diagnostics.rs` | `diagnostics` ` :46` | 新参数；追加组件诊断；merge status |
| `crates/wxml-lsp/src/backend_tsgo/mod.rs` | `diagnostics` ` :779`/` :812`、`capabilities` ` :980` | 传 `&plan.view`；更新 capability 文案 |
| `crates/wxml-lsp/src/backend_tsgo/tests.rs` | 现有 fake-session 测试 | 新增 wiring/回归测试 |
| `crates/wxml-lsp/tests/native_parity/native_engine_real.rs` | `GATE_REPLACEMENTS` ` :58`、`IGNORED_GATE_TESTS` ` :111` | 增加 E2E-20 映射和测试 |
| `crates/wxml-lsp/tests/native_parity/native_gates.rs` | `GATE_GREEN_IDS` ` :37`、`REQUIRED_NOT_MIGRATED_IDS` ` :63`、计数 ` :71` | 提升 E2E-20；18/5 → 19/4 |
| `docs/plans/tsgo-test-ledger.json` | `G-P5-E2E-20` 行 | 仅在证据齐全后改 `status`/证据字段 |

### 4.2 数据流（实现后）

```text
didOpen/didChange pages/index/index.wxml
  -> ProjectHost::analysis -> PreparedView
       components: [ComponentUsage { tag:"card", component_file:"components/card/card.ts",
                                      attributes:[title,count,...] }]
       fragments:  [f_pageTitle, f_total_1, f_total_2, ...]
       files:      [index.ts, card.ts, ...]
  -> build_request_plan
       view: PreparedView (new field)
       layout: ShadowLayout (fragment -> MountRecord)
       files:  [derived index.ts shadow, original card.ts, ...]
  -> diagnostics()
       1) semantic/syntactic diagnostics for derived index.ts
       2) component_bindings::diagnostics
            analyze_script(card.ts) -> properties { title:string, count:number }
            for title of card#2:
              expr_range(total#2) -> mount#2 -> last source segment start_utf16
              type_query(derived index.ts, position) -> "number"
              number vs string -> Error at expr_range(total#2)
       3) merge status; publish
```

### 4.3 状态 / Partial 语义

| 条件 | 诊断 | 状态 |
| --- | --- | --- |
| 所有 usage/属性都被证明 | 仅真实 mismatch 的 error | 不额外降级 |
| `component_file` 缺失/插件/远程/无脚本 | 无 | `Partial`（reason 列出 `tag.attr`，最多 3 个） |
| 组件脚本不在 `view.files` / `analyze_script` 失败 | 无 | `Partial` |
| `properties.complete == false`（spread/computed/非对象/未解析 Behavior） | 无（整条 usage 跳过） | `Partial` |
| `properties.is_empty()` 且 usage 有属性 | 无 | `Partial` |
| 某属性 `type_text` 为 `any`/Function/未知构造器 | 无 | `Partial` |
| `mixed` 绑定 | 无 | `Partial` |
| mustache 没有对应 mount / 表达式不是 chain | 无 | `Partial` |
| `type_query` 返回 `Any`/`Unknown`/`Missing` | 无 | `Partial` |
| `type_query` 硬失败 | 无（停止后续组件查询） | `Partial`，不置 `fatal` |
| `view.source_complete == false` | 视情况 | `Partial` |
| 实际 union 全部为 `undefined`/`null` | 无（属性未设置） | 不降级 |

`Partial` reason 复用 Node 的稳定前缀：

```text
some component bindings could not be resolved and were not checked (card.title, ...)
```

reason 必须 path-free、bounded；不拼接绝对路径。

### 4.4 Range 映射（UTF-8 / UTF-16）

- WXML 侧：`ComponentAttribute.expr_range` / `attribute_range` 是 `wxml_types::SourceRange`（UTF-8 bytes；BOM/CRLF 不归一化）。诊断直接使用这些 bytes；LSP 的 UTF-16 转换由现有 `handlers/diagnostics.rs` 的 `diagnostic_value` 完成。
- Native 侧：`ShadowLayout.derived()` 是 derived UTF-16；`MountRecord.start_utf16`/`target_start_utf16` 也是 UTF-16。查询位置必须用 `offset_to_position(layout.derived(), Utf16Offset::new(mount.target_start_utf16))`。
- 不要把 `expr_range` 的 UTF-8 byte 直接当 native position；也不要把 native 的 `TypeQuery` 当成有 range 的 evidence（它只有 type id/text）。
- 不做 native diagnostic 的反向 `map_native_range`；组件诊断的 range 完全来自原始 WXML，因此不会经过 `map_span_to_source` 的裁剪/近似规则。
- `type_query` 失败或表达式文本与 `original_wxml` 不一致时丢弃该条，不发布错位 range。
- CJK/emoji/CRLF 用例必须覆盖：WXML 前缀包含 `标题😀\r\n`，`expr_range` 仍精确到 `total`；derived position 是 UTF-16。
- 对同一表达式出现两次（fixture 的两个 `total`），必须按 `expr_range` 精确匹配 mount，不能用文本或第一次出现。

## 5. 测试矩阵

### 5.1 Fixture 级（不改旧 gate）

- 保持 `tests/fixtures/component-bindings/**` 不变。
- 保持 ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:772` 的 Node gate 原样。
- 保持 `expected/{hover,diagnostics,definition}.json` 原样。
- 新增 native replacement（§5.4）只读 fixture，不改 marker/range。

### 5.2 `component_bindings` 单元测试

在 `crates/wxml-lsp/src/backend_tsgo/component_bindings.rs` 的 `#[cfg(test)]`：

1. `matching_root_binding_is_clean`：`title="{{ pageTitle }}"`、`count="{{ total }}"` 无诊断。
2. `mismatched_root_binding_is_an_error_at_the_expression`：`title="{{ total }}"` 一条 error，range 精确 `total`，message 含 `component property "title"`、`expects string`、`got number`。
3. `declared_union_accepts_each_member`：`string | number` 接受 string/number，拒绝 boolean。
4. `literal_and_boolean_checks_use_the_attribute_range`：literal number property + literal string → error；boolean property + bare attr → ok；number property + bare attr → error。
5. `undeclared_and_event_attributes_are_ignored`：`class`、`style`、`bind:tap`、`bindtap` 不产生属性诊断（事件由 `ComponentUsage` 层面排除）。
6. `mixed_binding_is_partial_not_an_error`。
7. `missing_component_or_incomplete_properties_is_partial`：`component_file=None`、spread properties、非对象字面量。
8. `any_unknown_and_missing_types_are_partial`：`type:null`/Function/`type_query` Any/Missing。
9. `two_components_with_the_same_property_keep_their_own_types`：card `title:string` 与 badge `title:number` 不串。
10. `cjk_crlf_expression_range_is_exact`：WXML 前缀 CJK+emoji+CRLF，诊断仍落在 `total`。
11. `duplicate_expression_uses_the_exact_expression_range`：两个 `total` 只诊断第二个。
12. `caps_and_dedup`：超过 8 条/tag 或 64 tag 时截断并 `Partial`；相同诊断去重。

测试可直接构造 `ComponentUsage`/`PropertyOverlay` 产物和一个小型 `ExpressionTypeSource`（闭包或 trait），避免实现完整 `QuerySession`。wiring 测试再用现有 `FakeSession`。

### 5.3 `backend_tsgo` wiring / 回归测试

在 ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/tests.rs`：

1. `component_binding_diagnostics_are_published_with_shadow_ranges`：构造带 `components` 的 `PreparedView`，fake session `type_query` 返回 `number`，断言 `DiagnosticsResult` 含一条 error，range 是 WXML `total`，status 仍 `Supported`。
2. `component_binding_any_type_degrades_to_partial`：fake 返回 `any`/`Unknown`，断言无 error、status `Partial`、reason path-free。
3. `component_binding_type_query_failure_keeps_backend_diagnostics`：语义诊断仍发布，组件检查 `Partial`，不 `fatal`。
4. `component_binding_pass_is_not_run_for_script_diagnostics`：`script_diagnostics()` 不调用 `type_query`（fake log `type_calls` 为空）。
5. `component_binding_pass_is_not_run_when_no_custom_components`：`view.components` 为空，`type_calls` 为空。
6. `completion_plan_does_not_query_component_types`：completion 路径不带 `view` 检查或至少不触发 `type_query`。
7. `capabilities_do_not_claim_component_diagnostics_uncovered`：更新后的 reason 断言（文案稳定前缀）。

### 5.4 真实 LSP native gate

新增：

```rust
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ac012_component_property_bindings() { ... }
```

位置：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_engine_real.rs` 末尾（避免插入到并行改动中间）。

流程：

1. `NativeGate::require("G-P5-E2E-20")`；`copy_fixture(&gate, "component-bindings")`。
2. `Session::start(&gate)`；读取 `pages/index/index.wxml` 原文并 `open_wxml`。
3. hover 两条（复用 `hover_target_end`）：
   - `title="{{ pageTitle }}"` 的 `pageTitle` → 含 `string` + `页面标题`；
   - `count="{{ total }}"` 的 `total` → 含 `number` + `卡片总数`。
4. 属性诊断：
   - `errors.len() == 2`（属性 1 + 事件 1）；
   - `assert_error_at(&diagnostics, &wxml, "title=\"{{ total }}\"", "total", "AC-012")`；
   - 该 error message 含 `component property "title"`、`number`；
   - `assert_error_at(..., "bindtap=\"missingHandler\"", "missingHandler", ...)`；
   - 断言 `pageTitle` / `count="{{ total }}"` 的 range 上没有 error。
5. definition：
   - `onCardTap` 用 `assert_definition_covers` 精确覆盖 `pages/index/index.ts` 的 `onCardTap`；
   - `missingHandler` 的 `result` 必须是 `null`（当前探针输出；若实现变化为 `[]`，gate 需要显式决策，不能接受非空）。
6. `assert_no_status_diagnostic`：当前 fixture 在全检查通过时不应出现 `type checking is` warning；如果实现选择对未证明项降级，必须确保 fixture 没有未证明项。
7. `assert_no_node_exec()`；`shutdown()`；`gate.tripwire().assert_absent()`。

该 gate 必须覆盖旧 gate 的五类断言（hover 2、属性诊断 1、事件诊断 1、expected 中没有属性 definition、事件 definition 2），不能只断言属性诊断。

### 5.5 回归矩阵

| gate | 场景 | 期望 |
| --- | --- | --- |
| E2E-10 `native_ac013_component_properties_hover` | `<card title="{{ user.name }}" />` | 仍无 error；hover string |
| E2E-11 `native_component_behavior_data_and_properties` | `<card />` + Behavior properties | diagnostics 仍 `[]`（无属性绑定，pass 不降级） |
| E2E-18 `native_ac008_multi_view_fixture_isolation` | multi-view：card `title:string` vs badge `title:number` | `pages/a`/`pages/b` 仍 `[]`；同名属性不串 |
| E2E-19 simultaneous views | 同上 + overlay | 仍无串扰 |
| E2E-02 events | `events-basic` 的 `widget` 无属性绑定 | 事件诊断/definition/completion 不变 |
| E2E-01 overlay replay | overlay 改任意 script（例如 `card.ts` properties） | 下一次 diagnostics 使用 overlay 文本；不丢检查 |
| E2E-07 typo | 无自定义组件 | 不触发 `type_query`，无新 status warning |

### 5.6 命令、计数与 RED

required native job（沿用现有命令；包/资产路径按实际环境）：

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  WXML_TEST_NATIVE=1 \
  WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
  TSGO_TYPINGS_ROOT=/tmp/opencode/assets-lock-root/typings \
  cargo test -p wxml-lsp --test native_parity native_engine_real \
    -- --ignored --test-threads=1
```

预期（按当前工作树 18 个 green + 5 个 required 冻结）：

| 运行 | 结果 |
| --- | --- |
| 默认 `cargo test -p wxml-lsp --test native_parity` | 非 gate 测试全 passed / 0 failed / **19 ignored**（基线的 36 passed 可能已随并行提交变化） |
| required native job | **19 passed / 0 failed / 其余 filtered**（基线 36 filtered） |
| fail-closed RED（去掉 `WXML_TEST_NATIVE=1`） | **0 passed / 19 failed / 其余 filtered** |

计数必须在提交后的实际树上重新核对；如果 E2E-01/E2E-05 先合入，18/5 和 filtered 计数都会变，不能照抄本文数字。

台账提升顺序：

1. 先实现 + 单测 + 真实 LSP gate，required job 绿；
2. 再跑 RED，确认 19 个 gate 全部因缺少 `WXML_TEST_NATIVE=1` 失败而不是 skip；
3. 最后改 `GATE_REPLACEMENTS`、`IGNORED_GATE_TESTS`、`GATE_GREEN_IDS`、`REQUIRED_NOT_MIGRATED_IDS`、`GATE_GREEN_COUNT`/`REQUIRED_NOT_MIGRATED_COUNT` 和 ledger `status`；
4. 不能先改计数再补测试；不能删除旧 Node gate；不能把 `G-P5-E2E-20` 从 23 个旧 gate 集合中移除。

## 6. 风险

### 6.1 多组件 / 同名 property

- 按 `component_file` 路径缓存声明表，再按 usage 取；不能用 tag 作为唯一键。
- `multi-view` fixture 的 card `title:string` / badge `title:number` 是最小回归；必须验证两条页面都 clean。
- 同一 tag 多次出现时按 `expr_range` 匹配 mount，不能按出现顺序或文本。

### 6.2 Behavior / 默认值 / Union

- Behavior：复用 `analyze_script` 的 own/behavior 合并；`properties.complete == false` 时整条 usage 跳过并 `Partial`。这比 Node “仍检查 own property”更保守，但不会伪造错误；在 `Partial` reason 中说明。
- 默认值：`value` 不参与检查；`value: 1` + `type: String` 仍期望 string，不能把默认值类型当作允许类型。
- Union：顶层 `|` 拆分；`undefined`/`null` 剔除；实际 union 中任一无匹配即失败；无法分类的 constituent 降 `Unknown`/`Partial`。
- `optionalTypes: [Number]` → `string | number`；`type: null` → `any` 跳过；`Function` → 跳过。

### 6.3 `bind:` / 事件误判

- `ComponentUsage.attributes` 已排除 `bind*`/`catch*`/`capture-*`/`mut-bind`/`slot`/`wx:*`（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/template.rs:829`）；pass 只遍历 `ComponentUsage.attributes`，不要遍历原始 `Element.attributes()`。
- 已知边界：`binder` 会被 `event_attribute` 按 `bind` + `er` 解释，因此名为 `binder` 的组件属性不会进入 `ComponentUsage`；这是模板层既有分类，不是本 pass 引入。若要修，另开切片同时改 `metadata` 与 `ComponentUsage` 测试，不能在本 gate 里绕过。
- `bindtap` 缺失 handler 的诊断仍由 local `semantics/event_diagnostics.rs` 发布；组件 pass 不产生、不合并、不覆盖它。

### 6.4 Array / Object / 复合表达式

- 没有 `isArrayLikeType`；Array 目标只接受文本可证明的数组，其余 Object 标 `Unknown`，不报错。
- Object 目标用 Object flag，数组也接受（匹配 Node 注释的 `isObjectType` 语义）。
- 只检查 chain 表达式；`Call`/`Binary`/`Logical`/`Conditional`/`Index` 等 `Partial`。这不影响 fixture，但比 Node 覆盖窄。

### 6.5 与 E2E-01 / E2E-05 的共享文件冲突

| 文件 | E2E-20 改动 | 冲突方 | 处理 |
| --- | --- | --- | --- |
| `backend_tsgo/input.rs` | `RequestPlan.view` 字段、move | E2E-01 identity `files_hash`；E2E-05 symbol plan | 字段追加在 `RequestPlan` 末尾；不改 `RequestIdentity`；合并后重跑 plan 构造测试 |
| `backend_tsgo/mod.rs` | `diagnostics` 传 `&plan.view`；capability 文案 | E2E-01 `ensure_open`/retry；E2E-05 `symbols` | 只改 `diagnostics()` 调用和 diagnostics reason；不要重排 `ensure_open` |
| `backend_tsgo/diagnostics.rs` | 新参数 + 追加组件诊断 | E2E-01 可能改 degradation/stale | 组件诊断在 native map 后、degradation merge 前追加；不改 `diagnostics_done` |
| `backend_tsgo/tests.rs` | 新 wiring 测试 | E2E-05 symbols 测试 | 新测试放文件末尾；复用 FakeSession 字段 |
| `native_engine_real.rs` | 新测试 + 两个常量 | E2E-01/E2E-05 也加测试/常量 | 追加数组项；按提交后的 `IGNORED_GATE_COUNT`/计数重算 |
| `native_gates.rs` | E2E-20 迁移 + 18/5 → 19/4 | E2E-01/E2E-05 也迁移 | 最后改，必须以 required job + RED 证据为准 |
| `docs/plans/tsgo-test-ledger.json` | E2E-20 row | 并行 E2E 也改 row | 单独提交；不混在实现 commit |
| `wxml-types/template.rs` | 不改 | — | 不做 |
| `script/properties.rs` | 不改 | E2E-05 symbols 只读复用 | 不做 |

E2E-01 的 `RequestIdentity.files_hash` 已经存在；本 pass 不改变任何 session identity，不会引入额外的 `update_project`。E2E-05 的 `analyze_script` 只读复用，不共享可变缓存。

### 6.6 其他风险

- **性能**：每次 template diagnostics 会构造一次 `AnalysisInput`（同 `event_definition::analyze` 已有成本）+ 每个不同组件一次 `analyze_script` + 每个 chain 属性一次 `type_query`。只影响 diagnostics，不影响 hover/definition/completion；用上限保护。
- **内存**：`RequestPlan` 保留 `PreparedView` 会让 `Arc` 文本在整个请求期间存活，和 plan 已克隆的 `SourceFile` 文本形成双份保留。大项目若出现压力，可改为在构造 plan 时只抽取 `components` + 组件脚本文本 + 对应 mount 信息，或把 `PreparedFile.text` 改成可共享的 `Arc<str>`；本 gate 不需要这一步。
- **overlay**：`view.files` 是 host 当前 effective text（overlay 优先），所以未保存的 `card.ts` `properties` 修改会生效；需加 overlay 单元/集成测试。
- **stale range**：必须校验 `expr_range` 处的原始 WXML 文本等于 `expr_text`；否则不发布。
- **status 噪声**：fixture 全检查通过时不能新增 `Partial` warning；所有 skip 条件必须只对真正未证明的项触发。
- **属性名大小写**：按 exact → camelCase → kebabCase 匹配，不做全小写归一化；避免误匹配。
- **组件 WXML 自己**：`view.components` 为空时 pass 完全 no-op；不能对普通 `<view>` 属性做检查。

## 7. 明确不做

- 不新增 native wire method、不改 allow-list、不改引擎/manifest/typings pin。
- 不实现 `checker.isTypeAssignableTo` 的等价物；不做完整 TypeScript 类型系统。
- 不修改 shadow 合成代码（不插入 assignment assertion）。
- 不修改 `wxml-types::ComponentUsage` DTO，不新增 WXML 解析器。
- 不删除旧 Node gate、不切换默认 backend、不改 P6/P7 状态。
- 不为 `binder`、Index/Call/复合表达式做额外启发式；无法证明就 `Partial`。
- 不把属性 definition 新增到 E2E-20；旧 gate 明确要求属性 definition 为空。

## 8. 待决问题

1. **`properties.complete == false` 时是否仍检查 own properties？** 本文建议整条 usage 跳过（匹配 Node `scan.complete` 的保守面），代价是比 Node 少覆盖 own props；若要求更贴近 Node，需要在 `PropertyOverlay` 暴露 `own_structure_complete` 并单独判断。
2. **chain 表达式限制是否接受？** 本文建议接受，以保证不把右端标识符类型误当整个表达式；若要覆盖 `foo.bar + 1`，需要 shadow 合成或 `GetTypeAtLocation` node handle，成本高。
3. **`type_query` 硬失败是否置 `fatal`？** 本文建议 `Partial` 并停止后续查询；若希望触发 session 重启，需要同时保证不丢掉已成功的 backend 诊断，属于 E2E-01 的 retry 语义。
4. **Array/Object 的结构判定**：是否值得引入更精确的文本/index-signature 判定；本文只承诺“可证明则检查，否则 Partial”。
5. **组件 attribute 的 `class`/`style`/`id`**：当前按 undeclared 忽略；如果未来要区分“组件保留属性”，需要显式名单，不在本 gate。

## 附录 A：手工探针（非 gate 证据）

探针目录：`/tmp/opencode/e2e20-probe`（不在 repo 内）。fixture 从仓库复制到 `project/`；包使用 `/tmp/opencode/tsgo-native-pkg`，typings 放在 `typings/miniprogram-api-typings`。

命令：

```bash
cd /tmp/opencode/e2e20-probe
python3 probe.py
```

关键输出：

```text
DIAGNOSTICS: [
  {
    "message": "Event handler \"missingHandler\" for \"tap\" is not defined on this page",
    "range": {"end": {"character": 51, "line": 2},
              "start": {"character": 37, "line": 2}},
    "severity": 1,
    "source": "wxml-lsp"
  }
]
HOVER pageTitle: string + 页面标题
HOVER count="{{ total }}": number + 卡片总数
HOVER title="{{ total }}": number + 卡片总数
DEF onCardTap: file:///.../pages/index/index.ts, line 7, character 2..11
DEF missingHandler: null
STDERR: initialized with hover partial (... event-handler and component semantics are served by the static layer), diagnostics partial (... event-handler and component diagnostics are not covered) ...
```

结论：类型证据（`total` = number）和绑定 range 都已经可用；缺口只是没有检查 pass。探针不是 gate 证据：包二进制与 manifest size 不一致，且工作树有并行改动。

## 附录 B：关键代码索引

| 主题 | 位置 |
| --- | --- |
| 旧 Node 模块 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/componentBindings.ts:105`、` :300`、` :424` |
| 旧 Node 测试 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/test/componentBindings.test.ts:147`、` :206`、` :229`、` :292` |
| 旧 gate | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:772` |
| fixture | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/tests/fixtures/component-bindings` |
| 模板收集组件 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/template.rs:674`、` :698`、` :829` |
| `ComponentUsage` / `ComponentAttribute` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/check.rs:123`、` :149`、` :164` |
| PreparedView.components | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:345`、` :357` |
| 组件解析 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:2527`、` :2708` |
| 组件属性 overlay | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/properties.rs:72`、` :110`、` :241` |
| union/constructor | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/values.rs:655`、` :721` |
| ScriptAnalysis | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/mod.rs:149`、` :217` |
| type query | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1125`；` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:81` |
| type quality | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/type_info.rs:51` |
| shadow mapping | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mapping.rs:27`、` :36`、` :365` |
| diagnostics pipeline | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/diagnostics.rs:46`、` :76`、` :97` |
| adapter call | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:779`、` :812` |
| capability | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:980` |
| RequestPlan | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:268`、` :658`、` :902` |
| event diagnostics | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/semantics/event_diagnostics.rs:68` |
| publish merge | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/diagnostics.rs:169`、` :218`、` :241` |
| native gate lists | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_engine_real.rs:58`、` :111` |
| native gate counts | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:37`、` :63`、` :71` |

## 附录 C：一句话结论

Rust 已经拥有 E2E-20 所需的两块输入——`TemplateAnalysis.components` + 已挂载的表达式，以及 `ScriptAnalysis.properties` 的组件声明类型；缺的只是一个 `diagnostics()` 之后的 component-binding pass。最小实现是：`RequestPlan` 保留 `PreparedView`，新增一个 LSP 侧 `component_bindings` 模块，用现有 `type_query` 读取 chain 表达式类型，在 Rust 侧做保守比较，并把错误落在 `expr_range`。**不需要新增 native checker query，也不需要纯 Rust 重写类型系统；纯 Rust 只够关闭这个 fixture，不够覆盖一般 AC-012。**
