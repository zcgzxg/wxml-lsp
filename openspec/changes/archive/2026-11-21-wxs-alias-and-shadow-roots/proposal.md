# Proposal

## Why

模板中的静态 WXS 模块名（如 `<wxs module="m1">`）在 shadow 表达式里被当成页面 data 根，挂载为 `__wxml_root.m1`，导致 tsgo 报 `Property 'm1' does not exist` 的误报；同时 `module.exports = { getLimitSettingText: getLimitSettingText }` 这类别名导出没有继承本地函数的分类信息，hover 只显示 `string`，丢失参数名与函数签名。

## What Changes

- shadow 表达式识别模板中的静态 WXS 模块名：保持原文（不加 `__wxml_root.` 前缀），在 probe 中以 `any` 声明一次，不再误报为缺失 data 字段。
- WXS 别名导出（`{ name: local }`）继承本地成员的 kind、类型与签名，hover 展示完整函数签名及参数名。
- 保持现有边界：WXS 仍只做静态摘要，不执行、不做完整类型检查；纯值别名仍按值展示。
- 清理并复用现有影子布局、表达式重写和 WXS 摘要逻辑，不引入新的查询通道。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `template-expression-checking`: 新增要求——模板中的静态 WXS 模块根不参与 data 根检查，不得产生“属性不存在”误报；普通 data 表达式检查不变。
- `wxs-hover`: 新增要求——别名导出的成员保留本地函数签名，hover 展示参数名。

## Impact

- `crates/wxml-types/src/wxs.rs`：别名导出分类与摘要。
- `crates/wxml-lsp/src/project_host.rs`：`PreparedView` 携带静态 WXS 模块名。
- `crates/wxml-lsp/src/backend_tsgo/input.rs`：把模块名传入 shadow 片段。
- `crates/wxml-backend-tsgo/src/shadow/layout.rs`：模块名声明与重写本地集合。
- 测试：`wxml-types` 单测、shadow 布局/映射测试、模板诊断集成测试。
