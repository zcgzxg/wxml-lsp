# Design

## Context

两个缺陷都在“组件事实的取数链路”上，不改协议、不改诊断语义：

- `component_bindings::analyze_component` 构造 `ScriptRequest` 时没有注入模块解析器，behavior 的 `@/...` 别名解析失败，`properties::build_properties` 把 `behavior.properties_incomplete` 折进 `complete`，`analyze_component` 因此返回 `None`，整个标签被 `skip`。事件 definition 路径已有可用的 `TsconfigPaths`，但它是 `event_definition.rs` 的私有类型。
- `wxml_project::typing` 只保留成员的类型文本与 proven 标记，丢弃成员前的 `/** ... */`；`script::merge_declared_properties` 又把 `documentation`/`default_text` 固定为 `None`。渲染层 `metadata::docs::property_document` 早已支持 `documentation`。

## Goals / Non-Goals

**Goals**

- 带 `behaviors` 的本地组件恢复属性检查；behavior 不可读时只降级受影响的部分。
- 声明文件里的 `JSDoc` 说明出现在属性与事件 hover 中。

**Non-Goals**

- 不改变“声明优先于脚本”的优先级，不改事件契约与诊断级别。
- 不引入新的配置项；不读取 `node_modules`；不支持 `interface ... extends`（解析器现状）。
- 不修复本仓库之外的 app 配置与 WXML 内容（`van-cell size`、拼写错误等），这些由应用侧单独处理。

## Decisions

### D1: 把 `TsconfigPaths` 提取为共用模块

新增 `crates/wxml-lsp/src/semantics/tsconfig_paths.rs`，把 `event_definition.rs` 中的 `TsconfigPaths`、`PathMapping`、`split_pattern`、`project_relative`、`join_project_path`、`candidate_paths` 原样迁入，`TsconfigPaths` 与 `from_view` 提升为 `pub(crate)`。`event_definition.rs` 改为 `use` 该模块。

理由：别名解析必须与事件 definition 完全一致（同样的 baseUrl 归一、候选扩展名与 `index` 探测、最具体 pattern 优先），复制一份会产生两套行为。迁移不改变任何解析结果，现有 event definition 测试即为回归保障。

被拒绝的方案：在 `component_bindings` 里手写一份简化 resolver——会在 `paths` 边界（越界、反斜杠、`*` 多目标）上分叉。

### D2: 绑定分析注入 resolver

`analyze_component(input, path)` 改为 `analyze_component(view, input, path)`，构造请求时：

```rust
let resolver = TsconfigPaths::from_view(view);
let request = ScriptRequest::new(input, path, RegistrationKind::Component);
let request = match resolver.as_ref() {
    Some(resolver) => request.with_resolver(resolver),
    None => request,
};
```

与 `event_definition::analyze` 的写法保持一致（先建 resolver，再按 `Option` 装饰）。`declarations` 缓存仍按组件路径缓存，resolver 来自同一 `PreparedView`，同一 view 内结果稳定。

### D3: behavior 不完整时回退到自身属性

`PropertyOverlay` 增加 `own_complete: bool`：只反映组件自身 `properties` 成员是否被完整枚举，**不**包含 behavior 的 `properties_incomplete` 与 `options` spread。

- `build_properties` 中把“自身枚举是否完整”与“behavior 是否完整”分开记录，`complete = own_complete && !behavior.properties_incomplete && !options_spread`。
- `analyze_component` 的判定改为：
  - `registration.complete == false` → `None`（未变）；
  - `properties.complete == false && properties.own_complete == false` → `None`（未变）；
  - 否则返回 `ComponentDeclarations { properties, complete: properties.complete }`。
- `Pass::run` 拿到 `declarations.complete == false` 时把 `self.partial = true`，继续按已知属性校验；未声明属性依旧不报错（脚本来源不把未知属性当错误，行为不变）。

这样 behavior 别名解析成功时行为与修复前完全一致（`complete == true`）；behavior 仍不可读时，组件自身的属性继续享有类型检查，只是不声称属性面完整。

### D4: 声明成员说明的解析

`TypingProperty`/`TypingEvent` 增加 `documentation: Option<String>`。解析器在读取成员键之前，从成员起点向前回看紧邻的 `/** ... */` 注释：

- 只接受块注释，不接受 `//`；
- 去掉 `/**`、`*/` 与每行行首的 `*`；
- 换行折叠为单个空格，去掉首尾空白；
- 空注释视为 `None`；
- 说明长度上限 512 字符（与既有成员预算同量级），超限截断而不是失败。

说明纯展示，不参与 `proven`、不参与类型检查、不影响 `complete`。

### D5: 合并与渲染

- `script::merge_declared_properties` 把 `property.documentation.clone()` 传入 `ComponentProperty.documentation`；`default_text` 仍为 `None`（声明格式没有默认值）。
- `metadata::docs::property_document` 已输出 `documentation`，无需改动。
- `metadata::docs::declared_event_document` 在签名与“（detail 类型未证明）”之后追加事件说明，复用同一套转义。

### D6: 降级原因措辞

回退路径的 `Partial` 原因沿用 `PropertyOverlay.problems`（behavior 解析失败、`options` spread 等原文），不新增笼统措辞。`docs/reference/type-diagnostics.md` §7.1 增加一段说明：behavior 经 `tsconfig` paths 解析；behavior 不可读时只校验组件自身属性并记 `Partial`，不把未知属性当错误。

### D7: 不新增 ADR

这属于 ADR-0008/0011 既定方向的缺陷修复，不改变分层与数据来源约定，因此只更新 `docs/reference/type-diagnostics.md` 与 fixture 说明，不新增 ADR。

## Risks / Trade-offs

| 风险 | 处理 |
| --- | --- |
| 迁移 `TsconfigPaths` 改变 event definition 行为 | 纯移动 + 可见性调整；既有 event definition 单测与集成测试全部保留并通过 |
| behavior 解析成功后组件属性面变大，暴露真实不一致诊断 | 这正是目标；先在 fixture 与真实工程抽样确认诊断合理，不做“按标签白名单”压制 |
| 自身属性回退把 behavior 贡献的已知属性当“未声明” | 脚本来源本就不报未声明属性，不会产生该误报 |
| 声明注释被解析进类型文本 | 文档解析在成员键之前独立进行，且 `documentation` 不进入 `type_text` |

## Migration

无迁移：行为只在“此前整体失效”的场景（behavior 别名、声明注释）变化；其余项目的诊断集合不变。

## Open Questions

无。应用侧 `components.d.ts` 补注释属于数据工作，按 `vant-jl` 各包 README 的 Props/Events 表格机械生成，失败项（README 未列出）留空即可。
