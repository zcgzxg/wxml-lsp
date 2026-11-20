# Tasks

## 1. 对象字面量 shadow 修复

- [x] 1.1 在 `crates/wxml-backend-tsgo/src/shadow/layout.rs` 的 fragment 挂载逻辑中，对首个非空白字符为 `{` 的表达式在前后写入 `(` / `)`，保持 `mount.text`、`expression_start`、`prefix_len`、segment 和 target 偏移不变；验证 `cargo test -p wxml-backend-tsgo shadow::layout` 通过。
- [x] 1.2 在 `layout.rs` 测试模块新增多行对象字面量用例：断言 shadow 文本包含 `({` 且表达式后紧跟 `)`，`mount.text` 不含括号，对象 value 的 UTF-16 位置仍映射回原 WXML；验证该测试单独通过。
- [x] 1.3 增加带 guard 折叠的对象字面量用例，断言 guard segment 与表达式 segment 均未因括号偏移；验证该测试单独通过。

## 2. 模板循环绑定查询

- [x] 2.1 在 `crates/wxml-types/src/template.rs` 抽出 `Engine::loop_bindings` 中“从 `wx:for` 属性生成 item/index 绑定”的共享逻辑，保持现有命名、校验、item==index 去重行为；验证 `cargo test -p wxml-types` 通过且无行为变化。
- [x] 2.2 新增公开 `loop_bindings_at(tree: &SyntaxTree, offset: u32) -> Vec<LocalBinding>`，按外层到内层收集祖先 `wx:for` 绑定，光标位于某元素自己的 `wx:for` 属性内时排除该元素绑定；验证新增单元测试覆盖默认名称、自定义名称、嵌套循环和循环外场景。
- [x] 2.3 为 `loop_bindings_at` 增加边界测试：光标在普通属性、子节点、未闭合 mustache、同名 item/index；验证 `cargo test -p wxml-types loop_bindings_at` 通过。

## 3. 补全片段携带循环绑定

- [x] 3.1 在 `crates/wxml-lsp/src/backend_tsgo/input.rs` 为 `CompletionFragment` 增加 `bindings: Vec<wxml_types::LocalBinding>`，更新所有构造点和 `build_request_plan_with_completion` 的调用；验证 `cargo check -p wxml-lsp` 通过。
- [x] 3.2 构造补全 `FragmentSpec` 时通过现有 `local_binding` 填充 `bindings`，并同步填充 `locals`；验证新增单元测试能观察到 shadow 中出现 `const item = __wxml_item(...)` 与 `const index = __wxml_index(...)`。
- [x] 3.3 增加嵌套循环与自定义 `wx:for-item` 的 shadow 文本测试，断言局部变量层级和名称正确；验证相关测试通过。

## 4. 尾点成员补全

- [x] 4.1 在 `crates/wxml-lsp/src/backend_tsgo/mod.rs` 的 `completion_member` 中，先按光标偏移调用 `loop_bindings_at`，从内到外匹配根名；命中局部变量时跳过 `member_root`，未命中时保留现有 data 根检查；验证局部变量、同名遮蔽、未知根三类单元测试通过。
- [x] 4.2 在 `crates/wxml-lsp/src/backend_tsgo/completion.rs` 的 `expression_site` 和 `typed_member_site` 中移除“点号前必须是裸标识符”的限制，要求前缀可完整解析且首个根可解析；验证 `{{ item. }}`、`{{ item.Setting. }}`、`{{ user.name. }}` 均生成挂载完整前缀的补全请求。
- [x] 4.3 增加替换范围回归测试：`{{ item.Na }}` 选择 `Name` 后只替换 `Na`，前缀与点号不变；验证测试断言原始 WXML 片段为 `{{ item.Name }}`。
- [x] 4.4 增加 data 根回归测试，确认 `{{ user. }}` 和 `{{ users. }}` 的候选与替换范围未受局部变量逻辑影响；验证原有补全测试和新增测试同时通过。

## 5. 文档与真实引擎验证

- [x] 5.1 更新 `crates/wxml-lsp/src/backend_tsgo/mod.rs` 的能力状态文本，删除“template-local roots 不解析”和“链式前缀不解析”的过时说明；验证文本与实现一致且 `cargo test -p wxml-lsp` 通过。
- [x] 5.2 更新 `docs/reference/wxml-language-feature-baseline.md` 的 F-07，补充对象字面量诊断和循环局部变量尾点补全的验收样本；验证文档中的命令/示例与实现一致。
- [x] 5.3 在 `crates/wxml-backend-tsgo/tests/real_engine.rs` 增加真实引擎用例：对象字面量 mustache 不产生 key 误报；`item.` 返回 `FunctionComponentResult` 成员；按 `TSGO_BIN`/`TSGO_TYPINGS_ROOT` 条件跳过；验证有引擎时用例通过，无引擎时保持默认套件绿色。

## 6. 集成验收

- [x] 6.1 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`cargo doc --workspace --no-deps`，确认全部通过。
- [x] 6.2 运行 `cargo deny --offline check`，确认依赖许可和重复版本检查通过。
- [x] 6.3 构建并安装原生布局：`cargo build --release -p wxml-lsp`，再执行 `scripts/install-native.sh --root "$HOME/.local/share/wxml-lsp"`；在示例 WXML 中确认对象字面量无 key 误报，`item.` 能返回成员补全。

## Workflow follow-up

- 实现与验收完成后，按项目评审要求提交变更。
- 评审通过后执行 `openspec archive fix-object-literal-shadow-and-local-completion`，将 delta specs 同步到主 specs。
