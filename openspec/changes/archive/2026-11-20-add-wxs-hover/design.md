# Design

## Context

WXS 的静态语义已经在 `crates/wxml-types/src/wxs.rs` 完成：解析 `<wxs>` 块与 `.wxs` 文件，产出 `WxsSummary`（成员、导出集合、`dynamic` / `truncated` 状态），并通过 `describe()` 给出函数签名或值类型。`crates/wxml-lsp/src/static_semantics.rs` 在此基础上实现模板内的静态 hover，`crates/wxml-lsp/src/handlers/hover.rs` 负责 LSP 入口和 markdown 输出。

三个缺口都在 adapter 层，而不是解析层：

- `wxs_hover` 的 mustache 根分支直接返回 `module_hover(...)`，后者把 `range` 固定成 `<wxs module>` 的属性范围。
- `module_hover` 用 `summary.exported()` 只拼成员名，没有调用已有的 `describe()`。
- `member_markdown` 通过 `handlers::markdown_escape` 输出文档，而该函数按设计丢弃所有 `is_control()` 字符（包括 `\n`），并转义 `{}`。事件 hover 依赖这种扁平化，因此不能直接改全局行为。

`.wxs` 文档之所以没有 hover，是因为 `DocumentRoute::Script` 是有意选择（未保存文本要进入静态分析），而 `template_document` 只承认 WXML。修复点应在 hover 入口按后缀分流，不改变路由本身。

## Goals / Non-Goals

**Goals:**

- hover `m1` 返回 mustache 内 token 的 range，并列出带类型/签名的导出。
- WXS JSDoc 以多行、可读、安全的形式展示，同时保持 512 字符预算。
- 打开的 `.wxs` 文件支持声明、导出键和文件内引用的 hover。
- 复用 `wxml-types` 的现有摘要与 `describe()`，不在 adapter 层重新推断类型。
- 用单元测试 + HostOnly 集成测试覆盖行为，不要求真实引擎。

**Non-Goals:**

- `.wxs` 的 definition、补全、rename。
- 动态导出、`require` 运行时求值、WXS 执行语义。
- 改变 `DocumentRoute`、`template_document` 对非 WXML 文档的既有判定。
- 改变 `markdown_escape` 的全局行为。

## Decisions

### 1. 在 mustache 分支覆盖 range，而不是给 `module_hover` 增加位置参数

`wxs_hover` 已经通过 `mustache_chain` 得到 `chain.range`（即 `m1` token 的范围）。保持 `module_hover` 只负责内容，在 mustache 根分支返回前覆盖 `range`：

```rust
let mut hover = module_hover(wxs, path, first_block(index, &wxs.module)?)?;
hover.range = chain.range;
```

`positions` 保持不变，definition 继续指向 WXS 源文件；`<wxs module="m1">` 属性自身的 hover 仍走原有分支，range 仍是属性值范围。

**备选方案**：给 `module_hover` 传入可选 range。缺点是调用点多一个与内容无关的参数，且属性 hover 与 mustache hover 的差异会更隐蔽。

### 2. 导出清单用 `describe()` 渲染，并设置条数上限

`module_hover` 改为遍历 `summary.exported()`，每项输出：

```text
- `someFunc`: `(name: string) => string`
```

函数走 `describe()` 的签名，值走类型文本，子模块走 `module <specifier>`。新增常量（例如 `MAX_HOVER_EXPORTS = 64`）限制条数，超出时追加 `… and N more`；`dynamic` / `truncated` 的 Partial 行保持原样。上限避免 512 个导出把 tooltip 撑爆，也避免一次 hover 生成过大 payload。

### 3. 文档格式化新增“保留换行”的入口，不改全局转义

在 `crates/wxml-lsp/src/handlers/mod.rs` 增加行级辅助函数（例如 `markdown_doc_text`）：按 `\n` 拆分，逐行执行现有安全过滤与转义，跳过纯空行，再用 `\n` 连接。JSDoc 标签行（`@param` / `@returns` 等）用行内代码呈现 `{type}`，使 `{}` 不被转义为 `\{`。

约束：

- 仍然拒绝 HTML/链接/控制字符，`is_display_unsafe` 的检查不放松。
- 总长度仍受 `MAX_HOVER_DOC_CHARS = 512` 限制，截断补 `…`。
- `markdown_escape` 保持现状，事件 hover 输出不变。

**备选方案**：直接让 `markdown_escape` 保留 `\n`。这会改变事件 hover 和其他调用方的单行输出，回归面不可控。

### 4. `.wxs` hover 作为独立纯函数，入口只做后缀分流

在 `static_semantics.rs` 新增：

```rust
pub(crate) fn wxs_source_hover(text: &str, offset: u32) -> Option<StaticHover>
```

流程：

1. 用 `word_range(text, offset)` 得到光标所在词及范围。
2. `summarize_wxs(text)` 得到摘要。
3. 优先选择 `name_range` 覆盖 `offset` 的成员（声明或导出键本身）；否则按词调用 `summary.member(word)`。
4. 内容复用抽出的 `member_markdown(prefix: Option<&str>, ...)`，`.wxs` 文件不传 `m1.` 前缀。
5. `range` 用 `word_range`，`positions` 为空，避免在源文件里追加“Definition: 自己”的页脚。
6. 未导出成员追加“未导出”说明；找不到成员返回 `None`。

入口处（`Server::hover`）在现有 `static_hover` 之前判断：文档 URI 去掉 query/fragment 后小写以 `.wxs` 结尾，且文档已打开，则调用 `wxs_source_hover`，命中后经 `hover_value` 生成 `Hover`；未命中返回 `null`。

为什么按 `name_range` 优先：`WxsSummary::member()` 对同名导出成员会优先返回导出项，其 `name_range` 指向 `module.exports` 里的键。若只按名字查，hover 声明处会得到导出键的 range；按光标所在 `name_range` 选择才能让 range 与光标一致。

## Risks / Trade-offs

- **文档格式变化影响快照测试**：现有测试没有断言精确的 `Exports:` 文本，但 `wxs_member_hover_and_definition_use_the_export` 与 `capability_gate_tests` 会检查内容片段。实现时保持签名行与 definition 文本不变，只替换文档渲染。
- **`.wxs` 分流范围**：必须严格按后缀与“文档已打开”判断，不能放宽 `template_document`，否则 `.ts` / `.json` 会意外进入静态路径。
- **大文件 hover 成本**：每次 `.wxs` hover 都重新 `summarize_wxs`。WXS 文件通常较小，且与现有模板内 `load_wxs` 行为一致；暂不引入缓存，避免新的失效逻辑。
- **别名导出**：`alias_of` 存在时 `describe()` 仍给出目标签名；由 `name_range` 优先规则保证 range 落在光标处。
