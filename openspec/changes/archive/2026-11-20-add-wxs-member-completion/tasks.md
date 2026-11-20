# Tasks

## 1. 空白容忍

- [x] 1.1 `wxs_member_completion` 识别 `module.` / `module?.` 前裁剪尾部空白，使 `{{ m1. }}`、`{{ m1.  }}` 与多行写法返回导出成员。验证：`cargo test -p wxml-lsp static_semantics` 新增用例通过。
- [x] 1.2 模块名补全改用忽略空白的空表达式判断，使 `{{ m1` 在前导空格存在时返回模块候选。验证：同一测试模块新增用例通过。
- [x] 1.3 保持替换范围只覆盖已输入前缀或光标零宽区间，空白不进入 textEdit。验证：断言 `replace` 边界。

## 2. 单测覆盖

- [x] 2.1 新增点号后空格、可选链后空格、多行空白、前导空格模块名用例。验证：`cargo test -p wxml-lsp wxs_member_completion`。
- [x] 2.2 回归既有用例：`wxs_dot_completion_omits_illegal_member_names`、`dynamic_wxs_summary_marks_completion_incomplete`、`optional_chain_wxs_member_completion_works`。验证：同上。

## 3. 真实 LSP 集成测试

- [x] 3.1 在 `crates/wxml-lsp/tests/semantic_surface.rs` 增加 `{{ m1. }}`（点号后空格）补全用例，断言导出名、签名 detail 与 textEdit 范围。验证：`cargo test -p wxml-lsp --test semantic_surface`。
- [x] 3.2 覆盖内联 `<wxs>` 的同类请求。验证：同一测试文件新增用例通过。

## 4. 文档

- [x] 4.1 更新 `docs/reference/wxml-language-feature-baseline.md` 的 WXS 补全行/章节：说明点号后空白容忍、模块名补全与真实 LSP 证据。验证：文档描述与测试一致。

## 5. 验收

- [x] 5.1 `cargo fmt --all -- --check`
- [x] 5.2 `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [x] 5.3 `cargo test --workspace --all-features`
- [x] 5.4 `cargo doc --workspace --no-deps`
- [x] 5.5 `cargo deny --offline check`
- [x] 5.6 `openspec validate add-wxs-member-completion --json` 通过；归档后 `openspec archive add-wxs-member-completion --yes` 并通过。
