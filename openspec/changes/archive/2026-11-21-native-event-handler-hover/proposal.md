# Proposal

## Why

WXML 事件处理函数（如 `bind:click-left="onBackTap"`）的 hover 与 definition 目前由 Rust 静态脚本层提供。静态层只解析相对路径导入的 behavior；页面通过 tsconfig `paths` 别名（如 `@/behaviors/BasePage`）引入的 behavior 无法解析，行为图退化为 `Partial`，处理函数从方法表中消失，hover 只能返回空。tsgo 本身能解析这些别名并给出准确的类型与 JSDoc，仓库也已有 shadow 注入与位置映射基础设施，事件 handler 应当复用同一套原生查询，而不是继续在 Rust 侧重写模块解析。

## What Changes

- 事件 handler 的 hover/definition 改为原生（tsgo）优先：把 `this.<handler>` 作为 shadow 片段挂载到现有 probe 方法中，查询后把结果映射回 WXML 的 handler token。
- shadow 增加成员片段模式：挂载文本为 `this.<name>`，只有 `<name>` 段落映射回 WXML，`this.` 为合成文本。
- 该片段仅用于 hover/definition 的按点查询，不进入诊断 shadow，避免把 `this.<handler>` 的类型错误发布为模板诊断。
- 删除静态事件 handler hover/definition 路径及其只被它们使用的辅助函数、常量和测试；静态方法表继续服务处理函数补全与缺失处理函数诊断。
- 更新 backend capability 说明：事件 handler hover/definition 已由原生后端提供，不再声明“由静态层提供”。
- **BREAKING**：`Page({ behaviors: [...] })` 从 behavior 继承的 handler 不再由静态回退兜底（`Page.Instance` 不合并 behavior 方法，tsgo 无法解析 `this.<handler>`），hover/definition 返回空。无引擎（HostOnly）模式、纯 JS 或 behavior 被标注为 `any` 的项目同样不再返回静态签名。`Page` 自身定义的方法、`Component` 及带 behavior 的 `Component` 不受影响。
- 保留 `event_definition` 静态分析供诊断/补全使用，其中 tsconfig `paths` 别名解析继续覆盖这些路径。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `component-event-checking`: 修改处理函数 hover 的来源与覆盖范围：由原生类型引擎提供签名与 JSDoc，覆盖 tsconfig 别名引入的 behavior；definition 仍跳转到脚本声明；明确 `Page` behavior 继承 handler 与无引擎模式的降级行为；处理函数存在性诊断与补全保持不变。

## Impact

- `crates/wxml-lsp/src/handlers/hover.rs`：移除静态事件 hover/definition 路由与辅助代码。
- `crates/wxml-lsp/src/backend_tsgo/{mod.rs,input.rs,hover.rs,definition.rs}`：按点构建含事件片段的 shadow，并接入 hover/definition。
- `crates/wxml-backend-tsgo/src/shadow/{rewrite.rs,layout.rs}`：新增 `this.<member>` 片段模式与映射。
- `crates/wxml-lsp/src/semantics/event_definition.rs`：删除仅 hover/definition 使用的解析入口与测试，保留诊断/补全使用的分析与别名 resolver。
- 测试：新增真实引擎用例（Component + 别名 behavior、Page 自身方法、Page behavior 降级、shadow 映射），更新/删除依赖静态 hover 的用例。
- 无新增依赖；不改变 WXML 解析、补全或诊断契约。
