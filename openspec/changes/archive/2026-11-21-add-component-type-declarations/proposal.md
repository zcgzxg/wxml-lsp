# Proposal

## Why

小程序第三方组件与本地自定义组件的 `props`、`events` 目前没有可靠的类型来源：本地组件只能靠脚本 `properties` 字面量静态提取，package/plugin 组件的 `property_facts` 永远为空且 `complete: false`，事件名则完全没有校验。Vue 生态用 `components.d.ts` 描述全局组件类型，小程序生态没有对应约定。本变更引入一套自包含的组件类型声明格式，用「与 `tsconfig.json` 同级的 `components.d.ts`」这一固定约定发现它，让属性绑定、事件名和 handler 签名都有判据。

## What Changes

- 声明文件采用固定约定：与项目 `tsconfig.json` 同级的 `components.d.ts`。不解析 tsconfig 的 `files`/`include`/`exclude`，不自研 glob，不遍历 `node_modules`；文件缺失时零副作用，也不产生降级噪声。
- 声明文件读取复用项目既有的 overlay 优先读取路径，只接受项目根内的普通 `.d.ts`，大小沿用既有单文件预算；不可读、路径逃逸或超限时给出可读的降级说明。
- 新增组件类型声明格式：`declare global` 内 `WxmlComponentMap` 映射到「每组件一个 interface」，interface 内 `props` 与 `events` 两段，`events` 携带 `detail` 类型。
- 新增静态声明解析器（项目内自包含子集，无引擎、无 Node），产出组件属性事实与事件事实；无法证明的类型标为未证明而不是猜测。
- 组件属性事实新增来源标记：命中声明文件时 **d.ts 覆盖本地脚本 `properties`**，并且不再让 overlay 刷新或引擎脚本推导覆盖它。
- 属性绑定检查扩展 TS 类型文本映射，使 d.ts 中的 `'a' | 'b'`、`boolean`、`T[]` 等能参与既有绑定校验。
- 事件能力：`bind:`/`catch:` 事件名补全、未声明事件名 **Warning**、事件属性 hover 展示 `(e: WechatMiniprogram.CustomEvent<T>) => void`、已标注 handler 与 `detail` 契约不匹配时 **Error**。
- 文档与验证：重写 `docs/reference/type-diagnostics.md` 组件属性/事件章节，新增「类型来源优先级」，新增 ADR-0011，新增真实小程序 fixture 工程。

非目标（本次不做）：shadow 给 handler 形参注入类型、未标注 handler 的 `e.detail` 补全、内置组件事件目录重构、`typeRoots`/`types`/`references`、npm 包内类型解析、quick fix。

## Capabilities

### New Capabilities

- `component-type-declarations`: 组件类型声明文件的发现规则（与 `tsconfig.json` 同级的固定约定）、声明格式与静态解析子集、类型来源优先级（d.ts 覆盖脚本），以及声明驱动的属性绑定类型检查。
- `component-event-checking`: 基于声明的自定义组件事件能力：事件名补全、未声明事件名诊断级别、事件 hover 的 detail 类型展示，以及已标注 handler 的签名一致性检查与降级语义。

### Modified Capabilities

无。现有 `template-expression-checking`、`template-member-completion`、`wxs-hover` 的行为不变。

## Impact

- `crates/wxml-project/src/typings.rs`（新）：固定路径发现与安全读取；`crates/wxml-project/src/typing.rs`（新）：声明解析。
- 不修改 `crates/wxml-project/src/config.rs`：tsconfig 只提供 `components.d.ts` 的定位锚点。
- `crates/wxml-project/src/{lib.rs,discovery.rs,script.rs}`：`Project` 暴露声明事实，`ComponentPropertyFacts` 增加来源。
- `crates/wxml-lsp/src/project_host.rs`：视图组件合并优先级、overlay 失效、降级原因与状态。
- `crates/wxml-lsp/src/backend_tsgo/component_bindings.rs`：类型文本映射与跳过脚本推导。
- `crates/wxml-lsp/src/backend_tsgo/event_completion.rs`、`crates/wxml-backend-tsgo/src/{script/events.rs,shadow/}`：事件候选、诊断与签名探针。
- 文档 `docs/reference/type-diagnostics.md`、`docs/adr/0011-*.md`、`tests/fixtures/components-typings/`。
- 无新增第三方依赖；不引入 Node 运行时；LSP 协议不变。
