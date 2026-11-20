# Design

## Context

`{{ m1. }}` 的静态补全已存在于 `crates/wxml-lsp/src/static_semantics.rs` 的 `wxs_member_completion`：它从 `unclosed_mustache` 取出光标前的表达式，按后缀识别 `module.` / `module?.`，再用 `StaticIndex.wxs` 中找到的 `WxsSummary` 生成候选。当前缺口只在空白处理：

- 点号识别用完整的 `before` 字符串做 `strip_suffix`，所以点号后出现空格时（`{{ m1. }}`）识别失败，返回 `None`，请求落到类型后端。
- 模块名分支要求 `before.is_empty()`，所以 `{{ m1` 的前导空格会让分支失效。

两条都属于前缀归一化问题，不需要改索引、解析或 LSP 协议。

## Goals / Non-Goals

**Goals:**

- `{{ m1. }}`、`{{ m1.  }}`、`{{ m1?. }}` 以及点号后换行的写法都返回导出成员。
- `{{ m1` 在前导空白存在时返回模块名候选。
- 候选内容保持现状：标签为导出名，detail 为 `summary.describe`，documentation 为非空 JSDoc，动态/截断时 `isIncomplete`。
- 真实二进制 LSP 测试证明 `src` 与内联两种来源都可用。

**Non-Goals:**

- 不改 `wxml-types` 的 WXS 解析或摘要。
- 不做 `.wxs` 源文件内补全、索引访问 `m1["x"]`、动态导出推断。
- 不改触发字符、补全协议、后端路由顺序。
- 不改变 `template is` / `template data` / 脚本成员补全。

## Decisions

### D1 只在识别阶段裁剪尾部空白

`wxs_member_completion` 中新增 `let trimmed = before.trim_end();`，点号识别改用 `trimmed.strip_suffix("?.")`，模块名分支改用 `before.trim().is_empty()`。

替代方案（在 `unclosed_mustache` 里全局 trim）被否：会改变其它静态站点拿到的表达式文本和 offset 语义，影响 hover/definition。

替代方案（按字符回退跳过空白后再找点号）被否：`trim_end` 已覆盖空格、制表符与换行，逻辑更直接。

### D2 替换范围仍只覆盖已输入前缀

`prefix` 与 `replace` 在裁剪之前计算，因此：

- `{{ m1.  }}` 光标在空格后时，`prefix` 为空，替换范围为光标处的零宽区间。
- `{{ m1.so }}` 光标在 `so` 后时，替换范围只覆盖 `so`。
- 空白不会进入替换范围，候选插入不会吞掉或重复空格。

### D3 候选内容与降级保持不变

导出成员过滤仍使用 `is_dot_member_name`（非法点号名不出现在 dot 补全中）与 `starts_with(prefix)`；detail/doc 直接来自静态摘要；`isIncomplete` 仍由 `index_incomplete` 统一决定。这样本次变更不会改变候选集合与文案，只扩大触发场景。

### D4 测试分层

- `static_semantics.rs` 单测：点号后空格、可选链后空格、多行空白、前导空格模块名、既有 `{{ u.` 回归。
- `crates/wxml-lsp/tests/semantic_surface.rs`：真实 `--stdio` 二进制 + HostOnly 模式，didOpen 含 `<wxs src>` 的 WXML，`{{ m1. }}` 光标在空格后请求补全，断言导出名与签名 detail。
- `crates/wxml-lsp/src/handlers/capability_gate_tests.rs` 或既有补全测试：确认非 WXS mustache 仍返回 `None`/走后端，不被静态层拦截。

## Risks / Trade-offs

- `trim_end` 让 `{{ m1.   }}` 这类"点号后空白"也进入 WXS 分支。空白后如果用户实际想写别的表达式（不合法语法），返回导出候选仍比空结果更有帮助；语法诊断由既有解析器负责。
- 前导空白模块名分支会把 `{{ ` 后只有空白的请求也当成模块名补全。该分支只在文档含 `<wxs>` 时启用，且候选为空时仍返回 `None`，不会吞掉其它补全。
- 真实二进制测试依赖 HostOnly 模式，不启动引擎；与既有 `semantic_surface.rs` 测试一致，不引入新的测试环境要求。

## Migration

无。纯行为扩展，无配置、协议或数据迁移。
