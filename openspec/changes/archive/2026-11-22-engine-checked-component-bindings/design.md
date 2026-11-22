# Design

## 目标

把组件属性与事件处理函数的**类型兼容判断**移交给现有类型引擎，删除 Rust 侧重复的受限类型比较。只复用已有 shadow、同一个类型会话和 `getSemanticDiagnostics`，不新增引擎 API，不引入第二个进程，不改变 LSP 之外的可观察接口。

必须保留的边界：

- 只处理**纯 mustache** 的组件属性表达式，以及**静态处理函数名**的事件绑定。
- 字面量、空属性、混合文本仍按 WXML 值形态规则在 Rust 侧判定；它们没有可挂载的表达式，且涉及字符串化/真值转换。
- `any` 保持“跳过且视为干净”，`unknown`/`never` 目标保持“跳过并 `Partial`”。
- 声明不完整、类型不可解析、引擎不可用时只降级为 `Partial`，不回退到手写结构比较。

## 现有管线与插入点

```text
ProjectHost::prepare
  → PreparedView { fragments, components, typing, events, files, ... }
  → input.rs: eligible_fragments → FragmentSpec → ShadowRequest
  → backend-tsrs: build_in_place_shadow → BuiltShadow { text, mounts, anchors }
  → LSP: session.semantic_diagnostics → ShadowLayout.map_native_range
  → diagnostics.rs 发布模板诊断
```

- `CheckFragment` 已有 `expr_text`、`wxml_range`、`bindings`、`guards`，是现成的表达式作用域描述。
- `ComponentAttribute` 对纯 mustache 记录 `expr_text` 与 `expr_range`，可与 `CheckFragment.wxml_range` 精确对应。
- `EventHandler` 记录事件名、处理函数名、`handler_range`，但没有所属元素标签；需要新增标签字段才能找到组件声明。
- `FragmentSpec` 已有 `member` 模式（`this.<handler>`）和源段映射，事件探针可直接复用。
- `diagnostics.rs` 已把落在 mount 内的引擎语义诊断映射回模板，并排除脚本/合成文本诊断。
- `component_bindings` 目前用 `type_query` 逐个查询表达式类型并按数组/对象/联合大类比较；`event_diagnostics` 目前用手写 `compatible` 比较。

## 数据前置

### 组件属性事实

抽取属性事实解析为共享步骤（放在 `backend_tsrs` 内，由 `component_bindings` 与 `input` 共用），输入是 `PreparedView` 与一个 `ComponentUsage`：

1. 若 `view.typing.component(tag)` 完整，使用声明成员：保留声明成员名、`optional`、原始声明文件路径。
2. 否则使用脚本 `properties`：只接受可完整规范化的 `type`/`optionalTypes`，得到 `type_text` 与 `type_complete`。
3. behaviors 不完整时仍检查已声明成员，标记 `Partial`，未声明属性不报错。

`PreparedView.components` 不需要改变形状；`input.rs` 和 `component_bindings` 都调用同一个解析函数，并在一次请求内按组件路径缓存，避免重复 `analyze_script`。

### 事件事实

- `wxml_types::EventHandler` 新增 `tag: String`，由模板分析器在生成事件时写入所属元素标签。
- `input.rs` 已有的脚本分析结果需要保留方法表（`MethodTable`）。当前 `collect_script_facts` 在没有 `properties`/`behaviors` 时提前返回；改为在存在待检查事件契约时也执行注册分析。
- 事件契约仅在以下条件同时满足时生成：处理函数在方法表中存在且是字面函数、`parameter` 为 `Some`（显式标注）、事件在声明中存在且 `detail` 可在项目类型上下文解析。`arity == 0` 仍生成探针（无参数函数应被接受）。

## 契约探针

### 描述

`wxml-backend-tsrs` 在 shadow 层新增：

```rust
pub struct ContractSpec {
    pub kind: ContractKind,        // Property | Event
    pub expected: String,          // TS 类型表达式
    pub display: String,           // 诊断消息中的期望类型
    pub anchor: SourceRange,       // WXML 归属范围
}
```

挂在 `FragmentSpec.contract: Option<ContractSpec>` 上。这样探针与 fragment 共享同一 block 作用域、`wx:for` 局部变量和 `wx:if` 收窄；不需要另一套表达式挂载。

### 生成形态

TypeScript：

```ts
const __wxml_bind_<id>: <expected> = <rewritten>;
void __wxml_bind_<id>;
```

JavaScript（`checkJs`/`@ts-check` 生效时）：

```js
/** @type {<expected>} */
const __wxml_bind_<id> = <rewritten>;
void __wxml_bind_<id>;
```

- `<rewritten>` 是该 fragment 现有的重写结果（`__wxml_root.`、`__wxml_props_0.`、局部变量、守卫均保留）；事件 member 模式为 `this.<handler>`。
- 直接赋值让表达式获得上下文类型，保留字面量窄化；不使用辅助泛型函数。
- 每条契约检查替换该 fragment 原有的裸表达式语句，不追加第二条，避免同一表达式重复检查。
- `void` 消费合成变量，避免项目开启 `noUnusedLocals` 时产生 6133 噪音。
- 探针变量名由 fragment id 派生并做标识符化，同一 block 内多个片段不冲突。

### 期望类型来源

| 来源 | 期望类型 |
| --- | --- |
| 声明 `props` | `WxmlComponentMap['<tag>']['props']['<declaredName>']`；`optional` 为真时追加 `\| undefined` |
| 脚本 `properties` | 规范化后的 `type_text`（已合并 `optionalTypes`），不引用 `this.properties` 实例类型 |
| 声明 `events` | `(event: WechatMiniprogram.CustomEvent<WxmlComponentMap['<tag>']['events']['<declaredName>']['detail']>) => void` |

- 声明路径始终引用原声明，不重新打印 `TypeText`；泛型、类型别名、导入类型因此保持真实身份。
- 索引使用声明的成员名（camel/kebab 归一化后），不使用模板拼写。
- 声明文件是模块顶层映射而非 `declare global` 时，用类型级引用指向该声明文件；无法解析则 `Partial`，不猜测类型。
- 声明文件必须进入 shadow 的 `AnalysisInput`；若当前 `files` 不包含 `components.d.ts`，在 input 收集阶段补入，仍不扫描 `node_modules`。

### 省略与降级

| 情况 | 行为 |
| --- | --- |
| 表达式有语法错误 | 不生成契约探针，沿用现有 `Malformed` 计数并按需 `Partial` |
| 表达式没有自由根（如 `{{ 1 }}`） | 契约探针仍挂载；普通检查是否挂载保持现状 |
| 字面量/空属性/混合文本 | 继续走现有 WXML 值规则，不生成探针 |
| 脚本 `properties` 不可完整规范化 | 不生成探针，`Partial` |
| 声明成员无法被引擎解析 | 探针可生成，但解析失败的错误不发布，`Partial` |
| 期望类型为 `any` | 不生成探针，视为干净 |
| 期望类型为 `unknown`/`never` | 不生成探针，`Partial` |
| 事件处理函数无显式参数标注 | 不生成探针，`Partial` |
| 事件处理函数缺失 | 不生成探针；缺失处理函数诊断仍由既有事件链发布 |
| 引擎不可用 | 走现有 HostOnly/Unavailable 路径，不发布契约诊断 |

## 诊断归属

赋值错误的引擎诊断范围通常落在生成的变量名 `__wxml_bind_<id>` 上，现有 source map 无法把它映射回 WXML。shadow 因此输出契约锚点：

```rust
pub struct ContractAnchor {
    pub probe: Utf16Range,      // 生成文本中的变量名范围
    pub anchor: SourceRange,    // 原 WXML 范围
    pub kind: ContractKind,
    pub display: String,
}
```

`diagnostics.rs` 按以下顺序处理一条引擎诊断：

1. 命中某个 `ContractAnchor.probe` 且 code ∈ `{2322, 2345, 2739, 2741}`：发布到 `anchor`，消息使用引擎消息（有界截断），同一绑定只保留一条。`2739`/`2741` 是缺少必需属性的赋值错误，与 `2322`/`2345` 一样落在合成变量名上。
2. code `2353`（对象字面量多余属性）：契约探针丢弃。WXML 允许结构兼容的额外字段；其余值错误和缺失必需属性仍由上面的赋值码报告。
3. code `6133`/`6196` 等未使用/合成声明噪音：丢弃。
4. 其他落在源表达式段内的诊断：沿用现有 `map_native_range`，例如属性不存在、表达式内部类型错误。
5. 诊断落在声明/类型注解等纯生成文本，或期望类型解析失败（如 `2304`/`2503`）：不发布模板诊断，把该组件/事件标为 `Partial` 并给出原因。

不按“最近 token”猜测位置；无法归属就不发布。旧的手写属性比较路径删除后，同一绑定不会同时出现两条诊断。

## 删除范围

删除：

- `component_bindings` 中 mustache 的 `type_query` 路径：`Target`/`Actual`/`Verdict`、`PropertyContract` 的数组/对象/联合分类、`chain_target`、`type_flags`、`check_declared_mustache`。
- `typing_contract` 中事件参数的结构相等比较：`compatible`、`union_eq`、`object_eq`、`is_permissive`；`event_diagnostics` 不再调用 `check_handler`。
- `component_bindings` 自己重复的组件脚本事实解析，改为共享解析结果。

保留：

- 未声明属性、保留属性、kebab/camel、组件解析、`Partial` 原因与上限。
- 字面量/空属性/混合文本的 WXML 值规则（可缩减为只服务字面量形态的小判定）。
- `TypeText` 的声明读取、hover 展示与可选性判断；它不再承担 mustache 可赋值性。
- 事件名检查、缺失处理函数、hover/definition/补全。

## 编译选项与 JS

- 契约探针不修改项目编译选项，结果按项目实际 `strict`/`strictFunctionTypes` 判定。
- JS 只有在项目 `checkJs` 为真或文件已有 `@ts-check` 时才可能产生诊断；否则不生成探针并标 `Partial`，不声称已检查。
- TS 的 `strictFunctionTypes` 关闭时，事件参数按项目规则放宽；实现不额外施加严格检查。

## 性能与上限

- 契约探针复用同一次 shadow 和同一次 `getSemanticDiagnostics`，不增加逐表达式查询。
- 每条组件属性绑定最多一个探针；事件绑定同理。现有 `MAX_FRAGMENTS` 与诊断 1000 条上限继续覆盖最坏情况。
- `MAX_TAGS` 继续限制参与属性检查的组件标签数；契约生成与未声明属性检查共享该上限。
- 不新增资源上限常量，不新增降级分支。

## 明确不做

- 不新增 `isTypeAssignableTo` 或直接 checker API。
- 不把声明 `TypeText` 重新打印成期望类型。
- 不把字面量、空属性、混合文本改成 TS 赋值。
- 不强制 `strict`、`checkJs` 或 `strictFunctionTypes`。
- 不在引擎失败时回退到手写结构比较。
- 不改变 hover、definition、completion、unknown-event 与 missing-handler 的现有范围语义。
