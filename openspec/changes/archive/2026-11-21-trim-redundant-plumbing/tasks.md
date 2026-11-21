# Tasks

## 1. 类型层不可达兜底

- [x] 1.1 简化 `crates/wxml-types/src/wxs.rs` 的 `summarize_wxs` 截断：只保留一次字符边界截断，去掉 `(0..=limit).rev().find(...).unwrap_or(0)` 与 `text.get(..end).unwrap_or_default()` 两层默认值；用 `cargo test -p wxml-types` 验证，并新增/保留超长与多字节边界用例。
- [x] 1.2 复核 `crates/wxml-types` 内其余 `unwrap_or_default`/`unwrap_or(0)` 是否属于输入域内不可达；仅删除能证明不可达的分支，在提交说明中记录判定；`cargo test -p wxml-types` 通过。

## 2. 后端 helper 去重

- [x] 2.1 把 `utf16_len` 收敛到 `crates/wxml-backend-tsgo/src/native/strings.rs`，`shadow/mapping.rs` 改为 `pub use` 重导出；`cargo test -p wxml-backend-tsgo --test shadow_mapping` 通过，且 `utf16_len("\r\n😀名字")` 断言仍成立。
- [x] 2.2 把两处 `utf16_slice`（`native/session.rs`、`native/document_symbol.rs`）收敛到 `native/strings.rs`；`cargo test -p wxml-backend-tsgo` 通过。
- [x] 2.3 复用 `script/symbol_facts.rs` 的 `push_reason`/`dedupe` 于 `script/events.rs`、`script/behavior.rs`；`cargo test -p wxml-backend-tsgo --test script_events --test script_symbol_facts --test script_behavior` 通过。
- [x] 2.4 合并 `cap_signature` 与 `MAX_SIGNATURE_*` 为一处，保留 `events::MAX_SIGNATURE_LENGTH` 的 `pub use` 名称；`cargo test -p wxml-backend-tsgo --test script_events` 通过。
- [x] 2.5 清理已证不可达的默认值：`basename` 的 `rsplit(...).next().unwrap_or`、`symbols.rs` 的 `i32::try_from(delta).unwrap_or(0)`，改为可证明不溢出的写法或显式错误；`cargo test -p wxml-backend-tsgo --test script_symbol_facts` 通过。
- [x] 2.6 合并 `crates/wxml-project/src/resources.rs` 与 `files.rs` 的 `path_within`，保留 `files.rs`（含归一化分支）实现；`cargo test -p wxml-project` 通过。

## 3. 脚本单次扫描

- [x] 3.1 在 `crates/wxml-backend-tsgo/src/script/properties.rs` 的 `properties::build` 内只构造一次 `Script`，把引用传给 `build_data` 与 `build_properties`；保持 `Script::new` 失败时两个 overlay 各自的原有降级结果，`cargo test -p wxml-backend-tsgo --test script_properties` 通过。
- [x] 3.2 补一个同时声明 `data` 与 `properties` 的组件用例，断言重构前后字段、状态与 `problems` 完全一致；`cargo test -p wxml-backend-tsgo --test script_properties` 通过。

## 4. WXS 模块名提升到请求级

- [x] 4.1 让 shadow 构建入口接收一次模板级 WXS 模块名集合，`crates/wxml-lsp/src/backend_tsgo/input.rs` 的 `fragment_spec` 不再 `wxs_modules.to_vec()`；更新 `crates/wxml-backend-tsgo/src/shadow/layout.rs` 的 `probe_wxs_modules`/`effective_local_names` 只读取请求级集合。
- [x] 4.2 迁移 `FragmentSpec` 的全部构造点（普通片段、补全片段、事件片段、测试夹具），保留写入 shadow 前的非法名与 `__wxml_` 保留名过滤；`cargo test -p wxml-backend-tsgo --test shadow_locals_guards --test script_optional_types --test shadow_mapping` 通过。
- [x] 4.3 用既有 `prepared_view_collects_deduplicated_wxs_module_names` 与补全/事件 shadow 用例验证模块名仍不前缀化、仍去重；`cargo test -p wxml-lsp` 通过。

## 5. LSP 路由与样式诊断

- [x] 5.1 `crates/wxml-lsp/src/project_host.rs` 的 `style_diagnostics` 在已取得 `project` 后直接调用 `style_facts_in`，删除重复的 `style_facts_for`/`style_context_in` 解析；现有样式诊断测试通过。
- [x] 5.2 确认 `style_facts_in` 对 `~class`/`^class` 已置 `complete=false` 且输入为同一快照后，删除 `style_diagnostics` 末尾重复的 `has_external_class_reference` 条件；若不能证明则保留该条件，仅完成 5.1；`cargo test -p wxml-lsp` 通过。
- [x] 5.3 `ProjectState::component_properties` 改为按传入 `wxml` 解析所属项目（`ensure_view_project`），找不到返回 `None`，不使用活动项目；在现有多 app 夹具中补“同名模板、非活动 app”断言；`cargo test -p wxml-lsp --test lsp_stdio` 或对应集成测试通过。
- [x] 5.4 删除 `crates/wxml-project/src/styles.rs` 中零调用且语义不同的 `Project::is_style_path`；`cargo test -p wxml-project` 与 `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过。

## 6. 测试搭建收敛

- [x] 6.1 在 `crates/wxml-lsp` 测试内提供局部最小 `PreparedView`/`FragmentSpec` 构造，仅覆盖与断言相关的字段；迁移 `semantics/event_definition.rs`、`backend_tsgo/hover/tests.rs` 等受模板级字段牵连的用例。
- [x] 6.2 验证解耦标准：临时在请求级新增一个无用字段（不改测试）时，事件跳转与 hover 测试只需编译期默认值即可通过；恢复改动后 `cargo test -p wxml-lsp` 通过。

## 7. 全量验证与文档

- [x] 7.1 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`cargo doc --workspace --no-deps`，全部通过。
- [x] 7.2 若 `FragmentSpec`/shadow 请求字段形态变化影响 `docs/reference/contracts.md` 或 `docs/reference/tsgo-native-contract.md`，同步更新并在文档中说明“模板级 WXS 模块名由请求持有”。
- [x] 7.3 确认仓库内无临时文件或证据目录（`git status --short` 仅包含预期变更）。

## Workflow follow-up

- 归档本 change（`openspec archive trim-redundant-plumbing`）。
- 按 Conventional Commits 分组提交实现改动，并在提交说明中写明不可达分支的判定依据。
- 构建 release 二进制并安装到本地 Zed 使用的原生布局，验证编辑器实际启动。
