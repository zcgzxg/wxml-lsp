# Proposal

## Why

真实小程序工程（`jielong4/apps/miniprogram`）里两类组件的事实都不可用：

1. 带 `behaviors` 的自定义组件**属性完全不校验**。绑定分析构造脚本请求时没有传 tsconfig `paths` 解析器，behavior 里的 `@/behaviors/...` 别名解析失败，behavior 事实因此标记为不完整，整个组件被静默跳过（hover 却正常，加剧了“只有一半生效”的困惑）。同一工程 30 个业务组件里 17 个带 behavior，几乎覆盖全部业务组件。
2. `components.d.ts` 的声明属性 hover **没有说明文字**。声明解析器不读取成员前的 `/** ... */`，合并到组件属性事实时又把 `documentation` 写死为 `None`，渲染层本来能输出描述却拿不到数据。

两者都让“组件类型”在真实项目里只有一半可用，因此需要在本仓库修复数据链路。

## What Changes

- 组件绑定分析为脚本请求注入 tsconfig `paths` 解析器（与事件 definition 共用同一份实现），使 behavior 的路径别名可以解析。
- behavior 事实仍不完整时，用组件自身 `properties` 字面量的完整性做回退：自身属性照常校验，缺失的 behavior 贡献不伪造成错误，能力状态记 `Partial`。
- 声明解析读取 `props`/`events` 成员前的 `/** ... */`，写入 `documentation`；合并到组件属性事实与事件 hover 时透传。
- 事件 hover 在签名之外展示声明事件的 `JSDoc` 说明。
- 文档与 fixture 同步：`docs/reference/type-diagnostics.md`、`tests/fixtures/` 增加带 behavior 别名与声明注释的样例。

## Capabilities

### New Capabilities

- `component-script-typing`: 本地组件脚本属性事实的可用性——`behaviors`（含 tsconfig `paths` 别名）如何参与属性事实、behavior 不可读时如何降级而不是整体失效。

### Modified Capabilities

- `component-type-declarations`: 新增声明成员 `JSDoc` 说明进入属性 hover 的要求，并明确说明缺失时不伪造描述。
- `component-event-checking`: 事件 hover 在签名之外展示声明事件的 `JSDoc` 说明。

## Impact

- `crates/wxml-lsp/src/semantics/event_definition.rs`：`TsconfigPaths` 及路径工具提取为共用模块。
- `crates/wxml-lsp/src/backend_tsgo/component_bindings.rs`：`analyze_component` 注入 resolver；`PropertyOverlay` 增加自身完整性标记并接入回退。
- `crates/wxml-backend-tsgo/src/script/properties.rs`：`PropertyOverlay` 新增 `own_complete`。
- `crates/wxml-project/src/typing.rs`：`TypingProperty`/`TypingEvent` 增加 `documentation`。
- `crates/wxml-project/src/script.rs`、`crates/wxml-lsp/src/metadata/docs.rs`：透传与渲染。
- `docs/reference/type-diagnostics.md`、`tests/fixtures/`：行为说明与样例。
- 应用侧（不在本仓库）：`apps/miniprogram/components.d.ts` 补 `JSDoc` 说明，来源是 `vant-jl` 各包 README 的 Props/Events 表格。
