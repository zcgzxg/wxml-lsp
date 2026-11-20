# Tasks

## 1. 共享成员名归一化

- [x] 1.1 在 `crates/wxml-project/src/typing.rs` 增加 `canonical_component_name`、`component_name_eq` 与 `TypingComponent::prop_named` / `event_named`，并用 `cargo test -p wxml-project` 验证双向 kebab/camel、非等价拼写与空名用例
- [x] 1.2 为共享规则补充文档注释（说明只折叠 `-x`、不做大小写或下划线归一化），并用 `cargo doc -p wxml-project --no-deps` 确认无缺失文档告警

## 2. 事件链路

- [x] 2.1 `semantics/event_diagnostics.rs` 的声明事件查找改用 `event_named`，覆盖六种绑定写法与声明 camel/kebab 双向匹配，`cargo test -p wxml-lsp semantics::event_diagnostics` 通过
- [x] 2.2 `metadata/docs.rs` 的事件 hover 与属性查找统一使用共享规则（该文件整体归本组），`cargo test -p wxml-lsp metadata` 通过
- [x] 2.3 `backend_tsgo/event_completion.rs` 支持冒号与紧凑前缀、按归一化前缀过滤并按输入风格生成插入文本，`cargo test -p wxml-lsp event_completion` 通过

## 3. 属性链路

- [x] 3.1 `semantics/target.rs` 与 `semantics/hints.rs` 改用共享等价规则，覆盖 definition、引用收集与 inlay hint，`cargo test -p wxml-lsp semantics` 通过
- [x] 3.2 `metadata.rs` 的属性 definition 与 `completion/static_items.rs` 的属性补全改用共享规则并按输入风格插入，`cargo test -p wxml-lsp metadata completion` 通过
- [x] 3.3 `backend_tsgo/component_bindings.rs` 的声明成员查找收敛到共享规则，补 kebab 声明 + camel 使用的诊断用例，`cargo test -p wxml-lsp component_bindings` 通过

## 4. 解析覆盖测试

- [x] 4.1 `wxml-types/src/template.rs` 测试覆盖 `bindxxx`、`bind:xxx`、`bindsome-event`、`bind:some-event`、`bindsomeEvent`、`bind:someEvent` 及 `catch`/`capture-*`/`mut-bind` 两种前缀，`cargo test -p wxml-types template::tests::event_attributes` 通过
- [x] 4.2 `wxml-backend-tsgo/src/script/events.rs` 测试覆盖同样的前缀矩阵与非法名，`cargo test -p wxml-backend-tsgo script_events` 通过

## 5. 集成验证

- [x] 5.1 在 `crates/wxml-lsp/tests/` 新增覆盖属性与事件全部拼写的集成测试（含无声明不回归），`cargo test -p wxml-lsp --test <新测试名>` 通过
- [x] 5.2 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`cargo doc --workspace --no-deps`，全部通过

## Workflow follow-up

- 用 `openspec validate normalize-component-spelling-variants --strict` 校验后归档该 change。
- 归档完成后按 Conventional Commits 提交（`feat(component): ...`），不包含临时文件。
