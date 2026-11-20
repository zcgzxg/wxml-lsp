# Design

## Context

见 `proposal.md`。当前组件成员名比较分散在多处，实现细节也不一致：

- `wxml-project::typing` 的声明事实（`TypingComponent::prop` / `event`）是精确匹配。
- `wxml-lsp::semantics::target` 的 `property_spelling` 只做声明 camel → 使用 kebab 的单向映射，`semantics::hints` 重复了同一规则。
- `wxml-lsp::backend_tsgo::component_bindings` 自己实现了 `camel_case` / `kebab_case`，能双向命中，但只服务属性诊断。
- `metadata.rs` 的 definition 与 `metadata::docs` 的 hover 用 `eq_ignore_ascii_case`，不处理连字符。
- 事件名在 `event_diagnostics`、`metadata::docs`、`event_completion` 中都是精确字符串比较；事件补全只认冒号前缀。

模板侧的事件解析本身已覆盖冒号与紧凑前缀（`wxml-types::template::event_attribute`、`wxml-backend-tsgo::script::events::parse_event_attribute`），缺的是声明侧匹配、补全前缀集合与展示层消费。

## Goals / Non-Goals

**Goals:**

- 一个权威的“声明名 ↔ 模板写法”等价规则，属性与事件共用。
- 所有读取组件声明的地方使用同一规则，行为不再因入口不同而分叉。
- 事件补全覆盖全部合法前缀，并按用户已输入风格插入。
- 保持无声明/不完整声明项目的既有诊断与状态语义。

**Non-Goals:**

- 不做大小写不敏感匹配，也不把下划线当作连字符；只承认 kebab↔camel。
- 不改变声明文件解析子集、事件 detail 类型校验算法或绑定前缀的解析归属。
- 不新增配置项或 LSP 协议能力。

## Decisions

### 1. 归一化函数放在 `wxml-project`，作为跨 crate 唯一权威

在 `crates/wxml-project/src/typing.rs` 增加：

- `canonical_component_name(name) -> String`：把 `-x` 折成 `X`（其余字符原样保留）。
- `component_name_eq(declared, written) -> bool`：`declared == written || canonical(declared) == canonical(written)`。
- `TypingComponent::prop_named` / `event_named`：在声明列表上按上述规则查找。

理由：声明事实由 `wxml-project` 拥有，`wxml-lsp` 依赖它是允许方向（ADR-0001）；放在 LSP 会导致 `backend_tsgo` 与 `semantics` 各自再实现一份。备选方案是把规则放到 `wxml-types`，但事件 `EventHandler` 与声明事实不共享类型，仍需要在 LSP 侧拼装，收益更小。

`component_bindings.rs` 现有的本地 `camel_case` / `kebab_case` 保留其声明目标解析逻辑，但成员查找改为调用共享函数，避免两套规则漂移。

### 2. 事件名在“查找声明”这一层归一化，不改解析结果

`EventHandler.event` 与 `EventBinding.event` 继续保持模板中的原始事件名（用于诊断范围、hover 展示与编辑），只在 `TypingComponent` 查找时归一化。这样既支持 `bind:some-event` 命中 `someEvent`，又不会把用户书写的名字改写成另一种拼写。

### 3. 补全按输入风格生成插入文本

事件补全：

- 前缀集合从“仅冒号形式”扩展为冒号 + 紧凑形式（`capture-bind:`/`capture-bind` 等），最长前缀优先，避免 `bind` 抢占 `bind:`。
- 事件名片段与声明名都先归一化再比较前缀，因此 `some-` 能匹配 `someEvent`、`someE` 也能匹配 `some-event`。
- 插入文本：输入片段含 `-` 时用声明的 kebab 形式（camel 声明转 kebab），否则用 camel 形式（kebab 声明转 camel）；只替换事件名片段，前缀保持原样。

属性补全同理：`push_component_properties` 的前缀过滤与“已存在”判断改用共享等价规则；插入文本按输入片段是否含 `-` 选择 kebab 或 camel。

### 4. 展示与导航统一走共享比较

`target.rs::property_spelling`、`hints.rs`、`metadata.rs::definition`、`metadata::docs` 的属性/事件查找、`component_bindings.rs::declared_property` / `declared_typing_property` 全部改为调用共享函数或 `*_named` 方法。`eq_ignore_ascii_case` 不再用于组件成员匹配。

## Risks / Trade-offs

- [归一化把 `some-Prop` 与 `someProp` 视为等价] → 这是 kebab 折叠的自然结果，放宽而非收紧，不会产生新的误报；仅影响此前完全匹配不到的名字。
- [补全插入风格与声明拼写不同] → 以用户已输入片段为准，保持编辑连续性；hover/detail 仍展示声明原文，不混淆来源。
- [多个入口改动导致行为不一致] → 通过共享函数收敛，并在事件/属性两条链路各加覆盖六种写法与双向声明的单元测试。
- [事件紧凑前缀与事件名歧义（如 `binder`）] → 保持既有“紧凑前缀字面匹配”语义，不新增推断；补全只在用户已输入合法前缀后触发。

## Migration Plan

无数据或配置迁移。实现完成后按仓库验收命令验证，随后用 OpenSpec 归档并提交。
