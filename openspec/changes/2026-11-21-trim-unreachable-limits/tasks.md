# Tasks

## 1. wxml-types

- [x] 1.1 删除 `crates/wxml-types/src/wxs.rs` 的 `MAX_WXS_BYTES`/`MAX_WXS_MEMBERS`/`MAX_WXS_TOKENS`/`MAX_WXS_DEPTH`，以及字节切片、token 上限、成员上限、迭代扫描深度检查；`cargo check -p wxml-types` 通过。
- [x] 1.2 删除 `WxsSummary::truncated` 字段与 `describe` 对应分支，清理 `lib.rs` 的 re-export 与指针已删项的 rustdoc link；`cargo check -p wxml-types` 通过。
- [x] 1.3 删除 `template.rs` 的 `MAX_BINDINGS`/`MAX_GUARD_CONDITIONS`/`MAX_GUARD_CHAINS`/`MAX_COMPONENTS` 及其丢子树、变 Unresolvable 的分支，清掉对已删 `protocol.ts` 的注释引用；保留 `MAX_FRAGMENTS`。
- [x] 1.4 删除 `template_semantics.rs` 的 `MAX_TEMPLATE_DATA_KEYS`/`MAX_TEMPLATE_DATA_USES_PER_TEMPLATE` 与“超限标成 dynamic”的代码；`cargo test -p wxml-types` 通过。
- [x] 1.5 删除/改写只断言预算的测试（`byte_and_member_budgets_are_visible`、`byte_budget_cuts_only_on_character_boundaries` 等），保留畸形输入能正常结束的断言。
- [x] 1.6 第二轮穷举：删除 `MAX_TEMPLATE_DEFINITIONS`/`MAX_TEMPLATE_CALLS`/`MAX_STATIC_MODULES`、内联 `issues.len() < 4096`、`MAX_SYMBOL_PATH`、`MAX_SYMBOL_NAME_BYTES`、私有 `MAX_HANDLER_NAME_BYTES`；保留 `MAX_FRAGMENTS`（每个 fragment 是一次类型查询，16 MiB 下 ~3.35M 个）、`MAX_SYMBOL_ENTRIES`、`MAX_COMPLETION_ITEMS`、`MAX_DOCUMENT_SYMBOL*`。

## 2. wxml-syntax

- [x] 2.1 删除 `parse_with_limits` 的 `byte_limit` 参数及 `parse_inner` 内全部字节截断逻辑，更新本 crate 调用点；`cargo check -p wxml-syntax` 通过。
- [x] 2.2 删除只为 `byte_limit` 存在的测试与状态分支；`cargo test -p wxml-syntax` 通过。

## 3. wxml-project

- [x] 3.1 删除 `discovery.rs` 的 `MAX_PAGES`、`MAX_PAGE_LOCAL_COMPONENTS`、`MAX_REGISTRATION_SCRIPT_BYTES` 前缀截断；`cargo check -p wxml-project` 通过。
- [x] 3.2 删除 `resources.rs` 的 `MAX_RESOURCE_STATS`。
- [x] 3.3 删除 `styles.rs`/`styles/parse.rs` 的 `MAX_STYLE_FILE_BYTES`、`MAX_STYLE_CLASSES`、`MAX_VIEW_CLASSES`、`MAX_STYLE_ISSUES`（两处）及停止整表扫描的分支；保留 `MAX_CLASS_NAME_BYTES` 作为唯一定义。
- [x] 3.4 删除 `script.rs` 的 `MAX_COMPONENT_PROPERTIES` 与 `config.rs` 的 `MAX_MERGED_PATHS`。
- [x] 3.5 删除/改写引用被删常量的测试；`cargo test -p wxml-project` 通过。
- [x] 3.6 第二轮 discovery：删除 `MAX_APP_ENTRIES`、`MAX_COMPONENT_DEPTH`（后者守的是 BFS 队列，不是调用栈）；保留 `MAX_COMPONENTS`、`MAX_TOTAL_LOCAL_COMPONENTS`、`MAX_ISSUES`、三个 32 MiB 总字节预算（文件数量上游无界）。
- [x] 3.7 第二轮 config：删除 `MAX_CONFIG_EXTENDS_EDGES`、`MAX_EXTRA_OPTION_KEYS`、`MAX_OPTION_VALUE_BYTES`；保留 `MAX_EXTENDS_DEPTH`（真递归）、`MAX_CONFIG_LOADS`、`MAX_CONFIG_BYTES`、`MAX_MERGED_OPTION_BYTES`。
- [x] 3.8 第二轮 typing/script：删除 `MAX_TYPING_*`、`MAX_PROPERTY_*`、迭代栈版本 `MAX_SCAN_DEPTH`；保留真递归的 `typing::MAX_SCAN_DEPTH`、`MAX_RESOLVE_DEPTH`、`MAX_TEMPLATE_DEPTH`。
- [x] 3.9 第二轮 styles：删除 `MAX_SELECTOR_ITEMS`/`MAX_SELECTOR_BYTES`、`MAX_STYLE_IMPORTS_PER_SHEET`、`MAX_IMPORT_SPECIFIER_BYTES`、`MAX_DOC_COMMENT_BYTES`；保留 `MAX_SELECTOR_EXPANSIONS`（乘积放大）、import 链与缓存类预算。
- [x] 3.10 第二轮 resources/sources/files：删除 `MAX_RESOURCE_SPECIFIER_BYTES`、`SourceLimits::max_file_bytes`、`files::MAX_MESSAGE_BYTES`；保留 `MAX_FILE_BYTES`（系统唯一单文件 I/O 边界）。

## 4. wxml-backend-tsrs

- [x] 4.1 删除 `native/inproc.rs` 中 9 个只写不读的 `FrameLimits` 字段及 `config.rs` 里只为它们存在的常量；保留 `request_bytes`、两个 deadline、`MAX_LIVE_SNAPSHOTS`；`cargo check -p wxml-backend-tsrs` 通过。
- [x] 4.2 删除 `input.rs` 的 `MAX_INPUT_FILES`/`MAX_INPUT_BYTES` 与对应校验。
- [x] 4.3 删除 `script/insertion.rs` 的 `MAX_INSERTION_BYTES`；删除 `shadow/rewrite.rs` 的 `MAX_HOVER_TARGETS` 静默截断。
- [x] 4.4 删除 `query/definition.rs` 对定义目标的二次截断，保留 `native/session.rs` 一处。
- [x] 4.5 删除/改写断言被删上限的测试（如 `tests/type_assets.rs` 的预算越界断言）；`cargo test -p wxml-backend-tsrs` 通过。
- [x] 4.6 第二轮穷举：删除 `MAX_MEMBERS`/`MAX_OWN_MEMBERS`/`MAX_BEHAVIORS`/`MAX_OWN_METHODS`、`MAX_SIGNATURE_LENGTH`、`MAX_SHADOW_NAMES` 与 `scanned < 256`（两者会把超限误报成 shadowed）、`MAX_IMPORT_SCAN_TOKENS`、`LocalBudget`/`DEFAULT_LOCAL_BUDGET`（收敛为 `MAX_LOCAL_DEPTH=32`）、`MAX_DOC_LOOKBACK_BYTES`、`MAX_REASON_BYTES`、`MAX_MESSAGE_BYTES`、`MAX_HOVER_DECLARATION_SCAN`、`MAX_EXPRESSION_NODES`、`MAX_SCRIPT_TOKENS`、三个 `MAX_OPTION_*`；document symbol 改用 `wxml-types` 定义；清除 `MAX_SYMBOL_PATH`/`MAX_SYMBOL_NAME_BYTES` 引用；`cargo test -p wxml-backend-tsrs` 通过。

## 5. wxml-lsp

- [x] 5.1 删除 `static_semantics.rs` 的 `MAX_WXS_SOURCE_BYTES` 两处预检，`load_wxs` 不再因大小返回 `None`。
- [x] 5.2 删除 WXS `truncated` 驱动的 `index_incomplete` 分支与 hover 的“Partial/truncated”文案。
- [x] 5.3 合并三处 64 MiB 索引预算为 `workspace::MAX_QUERY_INDEX_BYTES`；合并包配置字节上限到 `configuration.rs`。
- [x] 5.4 补全 256、文档符号 256/32、定义目标 16 各自收敛到唯一定义。
- [x] 5.5 类名 128 改为引用 `wxml_project::styles::MAX_CLASS_NAME_BYTES`，删除本 crate 副本。
- [x] 5.6 第二轮穷举：删除 `workspace_packages.rs` 8 个静默上限、`push_diagnostic` 内联 512、hints 的结果集上限、`MAX_FORMAT_EDITS`、`MAX_PLAN_EDITS`、`MAX_WATCHED_CHANGES`、`MAX_CONTENT_CHANGES`、`MAX_SEMANTIC_TOKENS`、折叠/选择深度等；`cargo check -p wxml-lsp`、`cargo test -p wxml-lsp` 通过。

## 6. 全量验证

- [ ] 6.1 `cargo check --workspace --all-targets` 通过。
- [ ] 6.2 `cargo test --workspace --all-features` 通过。
- [ ] 6.3 `cargo fmt --all -- --check` 通过。
- [ ] 6.4 更新 ADR/reference 中对已删常量的引用（`docs/adr/0011` 已改）。
