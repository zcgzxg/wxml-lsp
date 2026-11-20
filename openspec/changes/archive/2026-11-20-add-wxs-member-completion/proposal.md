# Proposal

## Why

在 `{{ m1. }}` 这类 mustache 中，WXS 模块的导出成员补全已经有静态实现，但只在点号紧贴光标时生效：`{{ m1. }}`（点号后有一个空格）或格式化后的多行写法会把请求落到类型后端，得到空结果或没有响应。表达式起始处即使有前导空格，也不会补全 WXS 模块名。用户在正常书写和格式化后的模板里因此看不到模块导出的内容。

## What Changes

- 点号成员补全在判断 `module.` / `module?.` 前缀前忽略点号与光标之间的空白，使 `{{ m1. }}`、`{{ m1.  }}` 和换行写法都能返回导出成员。
- WXS 模块名补全把"表达式尚未开始"的判断从严格空串改为忽略空白，使 `{{ m1` 在前导空格存在时也能得到模块候选。
- 导出成员候选继续使用静态摘要：标签为导出名，detail 为签名/类型，documentation 为非空 JSDoc；动态导出或索引截断时保持 `isIncomplete`。
- 补齐单元测试和真实二进制 LSP 集成测试，覆盖 `src` 引入与内联 `<wxs>` 两种来源。
- 更新能力基线文档中的 WXS 补全描述与证据。

非目标：不改 WXS 摘要解析、不加 `.wxs` 文件内补全、不推断动态导出、不做 `m1["..."]` 索引补全、不改 LSP 协议或触发字符。

## Capabilities

### New Capabilities

- `wxs-member-completion`: mustache 中 WXS 模块点号后的导出成员补全，包括候选内容、替换范围、空白容忍、模块名补全与降级语义。

### Modified Capabilities

无。`template-member-completion`（脚本 `data`/`wx:for`）与 `wxs-hover` 的行为不变。

## Impact

- `crates/wxml-lsp/src/static_semantics.rs`：`wxs_member_completion` 的前缀识别与模块名分支。
- `crates/wxml-lsp/src/static_semantics.rs`、`crates/wxml-lsp/tests/semantic_surface.rs`：新增单测与真实 LSP 集成测试。
- `docs/reference/wxml-language-feature-baseline.md`：WXS 补全行为与证据。
- 无新增依赖、无协议变更、不引入 Node 运行时。
