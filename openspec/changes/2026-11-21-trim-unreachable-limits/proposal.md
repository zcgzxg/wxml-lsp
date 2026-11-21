# Proposal

## Why

代码里散布着大量防御性资源上限，多数在真实输入下不可能触发：单文件已被 `files::MAX_FILE_BYTES`（16 MiB）限制一次，stdio 入站已被 `MAX_MESSAGE_BYTES`（8 MiB）限制一次，但内层仍按字节、token、成员数、类名数、组件数再切一遍。这些上限本身只有几行，真正的成本在配套代码：`truncated`/`incomplete` 字段、能力状态、hover 的“Partial”文案、被截断后继续解析的分支，以及只为它们写的测试。结果是系统对“已经同意接收的文件”反过来声称结果不完整。

更糟的是，一部分上限虽然不可达，却会在触发时静默产出**错误**结果而非“不完整”结果：WXS 的迭代扫描深度超限后回到顶层继续解析、模板元素绑定数超限后丢掉整棵子树、注册脚本只读前 64 KiB 导致后半部的 `registration`/`isolation` 被漏掉。

本变更的判定是：**不可达的上限连同它的状态传播一起删除，不补降级、不补兜底、不新增日志。**

## What Changes

- **wxml-types**：删除 `wxs.rs` 的四个上限（`MAX_WXS_BYTES`/`MAX_WXS_MEMBERS`/`MAX_WXS_TOKENS`/`MAX_WXS_DEPTH`）以及 `WxsSummary::truncated`；删除 `template.rs` 的 `MAX_BINDINGS`/`MAX_GUARD_CONDITIONS`/`MAX_GUARD_CHAINS`/`MAX_COMPONENTS`；删除 `template_semantics.rs` 的两个 data 上限与“超限标成 dynamic”的误标。
- **wxml-syntax**：删除 `parse_with_limits` 的 `byte_limit` 死参数（生产路径恒为 `u32::MAX`）及其字节截断分支。
- **wxml-project**：删除不可达的 `MAX_PAGES`、`MAX_PAGE_LOCAL_COMPONENTS`、`MAX_RESOURCE_STATS`、`MAX_STYLE_FILE_BYTES`、`MAX_STYLE_CLASSES`、`MAX_VIEW_CLASSES`、`MAX_COMPONENT_PROPERTIES`、`MAX_MERGED_PATHS`；删除 `MAX_REGISTRATION_SCRIPT_BYTES` 的前缀截断（改为读完整文件，仍受 16 MiB 限制）；删除项目层的 `MAX_STYLE_ISSUES`，诊断数量由 LSP 统一的 1_000 负责。
- **wxml-backend-tsrs**：删除 `FrameLimits` 中 9 个只写不读的字段及其配置常量；删除不可达的 `MAX_INPUT_FILES`/`MAX_INPUT_BYTES`（48 MiB）；删除 `MAX_INSERTION_BYTES`；删除 `MAX_HOVER_TARGETS` 的静默截断；删除定义目标在 query 层的二次截断。
- **wxml-lsp**：删除 `MAX_WXS_SOURCE_BYTES` 的两处预检与 WXS 的“Partial”文案；合并三处 64 MiB 索引预算、两处包配置字节上限、四处补全上限、三处文档符号上限、四处定义目标上限；类名 128 改为引用 `wxml-project` 的单一定义。
- **测试**：删除或改写断言被删上限的用例，保留仍成立的行为断言。

## Capabilities

### New Capabilities

无。本变更是不可达分支与重复常量的清理，不改变任何已登记 capability 的需求或场景。

### Modified Capabilities

无。

> `skip_specs: true`：判定标准属实现约束（见 `design.md`），不是 capability 需求。

## Impact

- 代码：`crates/{wxml-types,wxml-syntax,wxml-project,wxml-backend-tsrs,wxml-lsp}/src` 及相关测试。
- 公开 API：删除的 `pub` 常量中，`MAX_COMPLETION_ITEMS`、`MAX_SYMBOL_*`、`MAX_DOCUMENT_SYMBOL*` 有跨 crate 引用，**保留**；`MAX_WXS_*`、`MAX_BINDINGS`、`MAX_GUARD_*`、`MAX_COMPONENTS`、`MAX_TEMPLATE_*` 仅 crate 内部使用，可删。`WxsSummary::truncated` 是 `pub` 字段，删除会改变 `wxml-types` 的公开形态。
- 行为：合法的超大/极端输入不再被静默截断，也不再产生“错误但不完整”的结果；真实资源上限（递归深度、文件字节、队列、诊断数）保持不变。
- 依赖与资产：无新增依赖，`assets.lock.json` 与引擎 pin 不变。
- 验证：各 crate `cargo check` + `cargo test`；最终 workspace 级 `cargo check` + `cargo test --workspace`。
