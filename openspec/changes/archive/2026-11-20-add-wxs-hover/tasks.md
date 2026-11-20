# Tasks

## 1. 静态 hover 修复

- [x] 1.1 在 `crates/wxml-lsp/src/static_semantics.rs` 的 `wxs_hover` mustache 根分支中，用 `chain.range` 覆盖 `module_hover` 返回的 `range`，保持 `positions` 不变；新增单测断言 hover `m1` 的 range 等于 mustache 中 `m1` 的 token 范围，且不等于 `<wxs module>` 属性范围；验证 `cargo test -p wxml-lsp static_semantics` 通过。
- [x] 1.2 修改 `module_hover`，用 `WxsSummary::describe()` 输出“成员名 + 签名/类型”的列表，新增导出条数上限常量并在超出时追加剩余数量说明，保留 `dynamic` / `truncated` 的 Partial 行；验证单测覆盖函数签名、值类型、子模块与截断。
- [x] 1.3 增加 inline `<wxs>` 与 `src` 引入两种来源的模块 hover 测试，断言两者的 range 都落在 mustache token 上；验证对应测试单独通过。

## 2. JSDoc 可读化

- [x] 2.1 在 `crates/wxml-lsp/src/handlers/mod.rs` 新增保留换行的文档格式化辅助函数，逐行复用现有安全过滤，保留 `\n` 并跳过空行；不得修改 `markdown_escape` 的现有行为；验证新增单测覆盖多行、控制字符、HTML/链接过滤。
- [x] 2.2 在 `static_semantics.rs` 的 `member_markdown` 中改用新辅助函数渲染 `doc`，将 `@param` / `@returns` 行输出为行内代码，保持 `{}` 原样；验证单测断言输出包含 `@param {string} name input name`、不包含 `\{string\}` 且保留换行。
- [x] 2.3 增加超预算文档测试，断言以 `…` 截断且成员签名完整；验证 `MAX_HOVER_DOC_CHARS` 行为不变。

## 3. .wxs 源文件 hover

- [x] 3.1 在 `static_semantics.rs` 抽出可复用的 `member_markdown(prefix: Option<&str>, ...)`，新增 `pub(crate) fn wxs_source_hover(text: &str, offset: u32) -> Option<StaticHover>`：按 `name_range` 覆盖偏移优先选成员，否则按词匹配；range 用 `word_range`，`positions` 为空；验证单测覆盖声明、导出键、别名、文件内调用。
- [x] 3.2 在 `wxs_source_hover` 中对未导出成员追加“未导出”说明，对非成员词返回 `None`；验证单测覆盖局部函数与关键字/字符串/注释。
- [x] 3.3 在 `crates/wxml-lsp/src/handlers/hover.rs` 的 `Server::hover` 入口增加 `.wxs` 后缀分流：URI 去掉 query/fragment 后小写以 `.wxs` 结尾且文档已打开时调用 `wxs_source_hover`，命中经 `hover_value` 返回，未命中返回 `null`；不修改 `template_document` 与 `DocumentRoute`；验证 handler 测试覆盖 `.wxs` 命中与 `.ts` / `.json` 不进入分支。

## 4. 集成与回归测试

- [x] 4.1 在 `crates/wxml-lsp/tests/semantic_surface.rs` 增加 HostOnly 集成测试：didOpen `.wxs` 文件后 hover 导出成员，断言返回签名与 JSDoc；hover 普通词断言 `null`；验证 `cargo test -p wxml-lsp --test semantic_surface` 通过。
- [x] 4.2 在 `crates/wxml-lsp/src/handlers/capability_gate_tests.rs` 复跑并补充 partial 模式下的 WXS hover/definition 回归断言，确保没有真实引擎时行为不变；验证对应测试通过。

## 5. 文档

- [x] 5.1 更新 `docs/reference/wxml-language-feature-baseline.md` 的 F-08，补充 WXS 模块 hover 带导出签名、成员 hover 的 JSDoc 呈现、`.wxs` 源文件 hover，并注明静态子集边界（不做执行、动态导出与跨文件 rename）；验证文档描述与实现一致。

## 6. 集成验收

- [x] 6.1 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`cargo doc --workspace --no-deps`，确认全部通过。
- [x] 6.2 运行 `cargo deny --offline check`，确认依赖许可和重复版本检查通过。
- [x] 6.3 构建并安装原生布局：`cargo build --release -p wxml-lsp`，执行 `scripts/install-native.sh --root "$HOME/.local/share/wxml-lsp"`；在仓库外示例工程中确认 `m1` hover 位置正确、导出带签名、`.wxs` 内 hover 返回 JSDoc。
