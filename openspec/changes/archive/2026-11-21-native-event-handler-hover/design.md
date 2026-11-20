# Design

## Context

模板表达式的 hover/definition 已经通过 in-place shadow 工作：`build_in_place_shadow` 在 `Page`/`Component`/`Behavior` 的 options 里插入 `__wxml_check_0` probe 方法，把 mustache 表达式挂进 probe，`this` 因而拥有页面/组件实例类型；查询结果再经 `MountRecord`/`MountedSegment` 映射回 WXML。

事件 handler 当前走另一条路：`handlers/hover.rs` 的 `local_event_hover`/`local_event_definition` 直接使用 `semantics/event_definition.rs` 的静态方法表，从不构建 shadow。该方法表依赖 Rust 侧的 behavior 图与模块解析，tsconfig `paths` 别名未接入时无法解析 `@/behaviors/BasePage`，导致行为图 `Partial`、handler 不在表中、hover 为空。详见 `proposal.md`。

## Goals / Non-Goals

**Goals:**

- 事件 handler 的 hover/definition 复用现有 shadow probe 与位置映射，由 tsgo 给出类型、JSDoc 和声明位置。
- 同一份 shadow 输入只增加一个按点挂载的成员片段，不新建引擎会话、不复制查询实现。
- 删除静态 handler hover/definition 入口及其死代码，避免两套答案不一致。
- 静态分析与 tsconfig 别名 resolver 继续服务补全和缺失 handler 诊断。

**Non-Goals:**

- 不为 `Page` 的 behavior 方法合成类型探针；`Page.Instance` 不合并 behavior 方法，本改动接受该场景降级。
- 不把事件 handler 的虚拟表达式加入诊断 shadow，也不新增基于 `this.<handler>` 的诊断。
- 不改变事件名补全、事件 detail 契约、处理函数补全和缺失 handler 诊断。
- 不重构静态 method table，也不把 `analyze_script` 的其余六个调用点统一接入别名 resolver。

## Decisions

### 1. 新增 shadow 成员片段模式，而不是单独构造查询脚本

`FragmentSpec` 增加成员片段标记。成员片段不走 `rewrite_expression`（后者会给自由标识符加 `__wxml_root.`），而是直接构造：

- `text = "this.<handler>"`；
- `segments = [合成 "this."（source = None）, handler 名（source = 相对 handler token 的 0..len）]`；
- `target_offset_utf16 = "this.".len()`；`hover_targets` 只含 handler 名。

`layout` 现有的 `MountedSegment`、`target_start_utf16`、`target_length_utf16` 已支持“挂载文本比源范围长”，无需改映射算法。

替代方案是创建独立 shadow 文件或在 probe 中 `declare` 一个实例变量；前者要额外的会话/身份管理，后者仍要自己描述实例类型，都会重复现有 probe 的能力。

### 2. 事件片段只进入按点查询计划

`build_request_plan` 同时服务诊断、hover、definition、document symbols 等。新增 point plan：hover/definition 传入请求 offset，仅当 offset 落在 `PreparedView.events` 某个 `handler_range` 内时追加一个成员片段。

不把全部事件 handler 都挂进 shadow，也不进入诊断计划，原因：

- 诊断若包含 `this.<handler>`，`Page` behavior 或 `any` behavior 会新增 TS2339，把内部探针错误发布给用户；
- 全部挂载会让 shadow 随模板事件数量增长，且同一 WXML 的每次查询都改变 session 输入；
- 单片段足以回答当前 hover/definition，且与 mustache 的按点查询模型一致。

动态 handler（mustache/空值）不在 `PreparedView.events` 中，自然不会挂载。

### 3. hover/definition 直接交给原生后端，不保留静态回退

移除 `hover()` 中的 `local_event_hover` 和 `definition()` 中的 `local_event_definition`，让事件值落到 `schedule_point` → native backend。选择理由：

- 用户明确接受放弃 `Page` behavior 继承方法的支持，换取单一、类型准确的答案来源；
- 保留回退会让 `Component` 别名场景与 `Page` 场景给出不同质量的内容，且回退路径仍需维护模块解析子集；
- 无引擎时 `backend_available` 已返回空结果，不需要额外分支。

代价是 `Page` behavior 方法、纯 JS/`any` behavior、HostOnly 模式不再有事件 handler hover。该降级写入 spec，并在 capability 说明中体现。

### 4. 清理静态 hover/definition 专用入口

删除 `resolve`、`resolve_handler`、`EventValueSite`、`event_site`、`EventHandlerFacts`、`EventHandlerResolution` 及 `handlers/hover.rs` 的 `event_hover_contents`、相关常量与测试。保留：

- `analyze`、`ViewAnalysis`、`TsconfigPaths`：事件诊断与 handler 补全仍需要；
- `event_handler_sites`：诊断定位仍需要；
- 静态 method table：补全与存在性检查的事实来源。

删除前用 `grep`/编译器确认没有其他调用方；`argument`/`missing_status` 等仅供已删除入口使用的辅助函数一并移除。

### 5. capability 文案如实更新

`TsgoBackendAdapter::capabilities()` 的 hover/definition 说明改为“expression-member 与 event-handler 已接入原生后端”，不再声明事件语义由静态层提供。诊断说明保留“事件 handler 诊断由静态语义提供”。

## Risks / Trade-offs

- **`Page` + behavior 回退消失** → 在 spec 与 capability 中明确；`Page` 自身方法、`Component`（含 behavior）仍可用。
- **无引擎模式事件 hover 变空** → 与“原生优先、无回退”的决策一致；HostOnly 仍提供语法诊断与静态补全。
- **shadow 中 `this.<handler>` 报 TS2339 时 hover 可能为空** → 只影响该 handler 的查询，不发布诊断；返回空而非猜测。
- **每次 hover/definition 使用不同 shadow 文本，可能触发 session 更新** → 仅一个片段、仅按点查询；实测若出现抖动，再按 handler 名做小型计划缓存，不改变外部行为。
- **`this` 上下文依赖注册形态** → 复用现有 probe placement；无 `methods` 的组件由现有逻辑创建 `methods`。

## Migration Plan

无数据迁移。实现与测试在同一提交内完成；回滚即恢复静态 hover/definition 路由与 capability 文案。

## Open Questions

无。
