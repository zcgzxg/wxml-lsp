# Tasks

## 1. 测试临时目录改用 tempfile

- [x] 1.1 在根 `Cargo.toml` 的 `[workspace.dependencies]` 加入 `tempfile`，并在 `wxml-project` 与 `wxml-lsp` 的 `[dev-dependencies]` 引用它（`wxml-backend-tsgo` 需要时同样处理）。
- [x] 1.2 在 `crates/wxml-lsp/tests/common/mod.rs` 把 `TempProject` 改为持有 `tempfile::TempDir`，保留 `root`/`write`/`uri`/`root_uri`，删除时间戳与 `AtomicU64` 命名逻辑。
- [x] 1.3 替换 `crates/wxml-project/src/{discovery,config,files,sources}.rs` 测试模块内的 `TempDir`：删除自定义结构与 `Drop`，改用 `tempdir_in`；保留各自的文件树搭建辅助。
- [x] 1.4 替换 `crates/wxml-project/tests/{project,p3b,styles}.rs` 的 `TempDir`/`TempProject`。
- [x] 1.5 替换 `crates/wxml-lsp` 内其余临时目录：`tests/{project_host,styles_resources,cli}.rs`、`tests/full_feature_matrix/support.rs`、`tests/native_lsp_document_symbols/support.rs`、`src/backend_tsgo/{engine,sandbox}.rs`、`src/backend_tsgo/environment/{gate,fakes}.rs`、`src/workspace_packages.rs`。
- [x] 1.6 替换 `crates/wxml-backend-tsgo/src/native/session.rs` 的 `TempTree`。
- [x] 1.7 验证：`cargo test -p wxml-project -p wxml-lsp -p wxml-backend-tsgo --all-features` 全绿；确认测试结束后 `/tmp/opencode` 无残留目录，仓库内无新增文件。

## 2. 收敛 LSP 集成测试的帧读取与子进程等待

- [x] 2.1 在 `crates/wxml-lsp/tests/common/mod.rs` 加入唯一的 `read_frame`（头部字节上限、消息字节上限、重复/缺失 `Content-Length` 校验）与 `wait_with_timeout`（超时 `kill` 后 `wait` 回收）。
- [x] 2.2 `crates/wxml-lsp/tests/lsp.rs` 删除本地副本并改用 `common`。
- [x] 2.3 `crates/wxml-lsp/tests/component_intelligence.rs` 同上。
- [x] 2.4 `crates/wxml-lsp/tests/structural_editing.rs` 同上。
- [x] 2.5 `crates/wxml-lsp/tests/semantic_surface.rs` 同上（该文件只有 `wait_with_timeout`）。
- [x] 2.6 验证：逐个跑 `cargo test -p wxml-lsp --test lsp`、`--test component_intelligence`、`--test structural_editing`、`--test semantic_surface`。

## 3. 合并逐字相同的生产函数

- [x] 3.1 `crates/wxml-lsp/src/workspace_packages.rs` 删除本地 `export_target`，改用 `crate::configuration` 的 `pub(crate)` 版本；补一条覆盖 `exports` 条件对象解析的测试（若现有覆盖已足够则复用）。
- [x] 3.2 `crates/wxml-backend-tsgo/src/native/{supervisor,vfs}.rs` 删除本地 `hex_lower`，改用 `crate::input::hex_lower`。
- [x] 3.3 验证：`cargo test -p wxml-lsp -p wxml-backend-tsgo --all-features`，并确认 `cargo clippy -p wxml-lsp -p wxml-backend-tsgo --all-targets --all-features -- -D warnings` 无 `dead_code` 残留。

## 4. 统一 TypeScript 对象成员扫描

- [x] 4.1 在 `crates/wxml-backend-tsgo/src/script/scanner.rs` 的测试模块内冻结一份 `shadow/layout.rs` 当前的 `parse_member` 副本，作为新旧对比基线。
- [x] 4.2 新增对比测试：对 `tests/fixtures/` 与 `script` 测试中的样本，分别用两套实现扫描同一对象字面量，断言成员列表（name/kind/key 范围/value token 范围）的差异集合符合预期。
- [x] 4.3 `shadow/layout.rs` 删除 `Member`/`MemberKind`/`scan_members`/`member_end`/`parse_member`/`function_keyword_before`/`method_name_before` 副本，改为 `use crate::script::scanner::{...}`；保留 layout 私有的 `object_bounds` 等仅 layout 使用的逻辑。
- [x] 4.4 处理差异：对 `async`/`*`/`get`/`set` 成员，确认 shadow 映射变化是"更准确"且受现有 shadow 测试接受；无法解释的变化退回"一份实现 + `Mode` 参数"，并在 design.md 记录。
- [x] 4.5 删除 4.1 的冻结副本与对比测试中不再需要的分支，保留一条防止再次分叉的等价性测试。
- [x] 4.6 验证：`cargo test -p wxml-backend-tsgo --all-features`（含 `shadow`、`script`、`real_engine` 中默认跳过的项保持不变），并记录该 crate 测试数量变化。

## 5. 全量验收

- [x] 5.1 `cargo fmt --all -- --check`
- [x] 5.2 `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [x] 5.3 `cargo test --workspace --all-features`（串行执行一次以排除既有的 `cli.rs` `ETXTBSY` 并行 flake）
- [x] 5.4 `cargo doc --workspace --no-deps`
- [x] 5.5 `cargo deny --offline check`（确认新增 dev-dependency `tempfile` 不触发 license / duplicate 问题；必要时更新 `deny.toml` 并在本任务记录原因）
- [x] 5.6 汇总净删行数与各 crate 测试数量变化，向用户报告
