# Proposal

## Why

`wxml-types` 的 WXS 静态摘要已经能给出导出成员、函数签名和 JSDoc，但 LSP hover 层没有把这些信息完整暴露出来。当前在 `{{ m1.someFunc() }}` 上 hover 有三个问题：

- hover `m1` 时，返回的 range 指向 `<wxs module="m1">` 的属性值，而不是 mustache 中的 `m1`。编辑器因此把提示挂到错误位置，甚至可能高亮整个 WXS 声明。
- `m1` 的导出清单只有成员名，没有类型或签名，用户仍需再 hover 一次成员才能确认调用方式。
- JSDoc 被 `markdown_escape` 当作单行文本处理：换行被丢弃，`@param {string}` 中的花括号被转义成 `\{string\}`，多行注释挤成不可读的一行。

此外，打开的 `.wxs` 文件本身没有任何 hover。原因是 `.wxs` 被刻意路由为 `DocumentRoute::Script`（以便把未保存文本送进静态分析），而 `template_document` 只接受 WXML 文档。用户在 WXS 源码里 hover 声明或导出键时只能得到 `null`。

这些缺口都位于不依赖 Node、不依赖真实 TypeScript 引擎的静态路径上，修复成本低、用户感知明显，适合作为一次独立改动。

## What Changes

- **修复 mustache 模块 hover 的 range**：hover `m1` 时返回 mustache 内 `m1` token 的范围，而不是 `<wxs module>` 属性值；definition 仍指向 WXS 源文件。
- **导出清单带类型/签名**：`m1` 的 hover 内容由“成员名列表”升级为“成员名 + `describe()` 得到的签名或类型”，并保留来源、Partial 状态说明。
- **新增可读的 JSDoc 展示**：按行保留注释结构，`@param` / `@returns` 行以代码片段形式呈现，`{}` 不再被转义；仍禁止 HTML、链接和控制字符，超出预算时截断。
- **新增 `.wxs` 源文件 hover**：在打开的 WXS 文档中，hover 声明名、`module.exports` 键或文件内引用时展示签名与 JSDoc；非导出局部成员标注“未导出”，非成员词返回 `null`。
- **保持其他能力不变**：`<wxs>` 标签/属性的 hover、WXS definition、补全、事件 hover，以及 `.ts` / `.json` 等文档的 `null` 行为都不改变。
- **明确不做**：`.wxs` 的 definition/补全/rename、动态导出推断、WXS 执行语义、npm 包内 WXS 的额外读取策略。

## Capabilities

### New Capabilities

- `wxs-hover`: WXS 模块与成员在 WXML mustache 和 `.wxs` 源文件中的 hover 行为，包括 hover 范围、导出清单粒度、JSDoc 呈现与静态边界。

### Modified Capabilities

无。项目当前没有覆盖 WXS hover 的既有 capability。

## Impact

- `crates/wxml-lsp/src/static_semantics.rs`：mustache 模块 hover 范围覆盖、导出清单带签名、`member_markdown` 复用、新增 `wxs_source_hover`。
- `crates/wxml-lsp/src/handlers/mod.rs`：新增保留换行的文档格式化辅助函数；`markdown_escape` 的既有行为不变。
- `crates/wxml-lsp/src/handlers/hover.rs`：在 `Server::hover` 入口按 `.wxs` 后缀分流到静态 WXS hover。
- `crates/wxml-lsp/tests/semantic_surface.rs`、`crates/wxml-lsp/src/handlers/capability_gate_tests.rs`：新增与回归测试。
- `docs/reference/wxml-language-feature-baseline.md`：F-08 补充 WXS hover 覆盖范围与静态边界。
- 依赖与协议：无新增依赖，LSP 协议不变，不触碰 `DocumentRoute` 与 shadow 生成路径。
- 兼容性：无 **BREAKING**；hover 输出更完整，range 修正属于缺陷修复。
