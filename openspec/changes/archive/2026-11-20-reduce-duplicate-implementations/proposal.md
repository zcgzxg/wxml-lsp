# Proposal

## Why

同一套逻辑在多处以复制粘贴的形式并存：测试临时目录、LSP 集成测试的帧读取与子进程等待、
TypeScript 对象成员扫描，以及若干逐字相同的生产小函数。重复已经开始漂移——`script/scanner.rs`
与 `shadow/layout.rs` 的两套成员扫描对 `async`、生成器方法和访问器的处理并不一致，改一处容易漏掉另一处。
上一轮瘦身后测试基线干净（2126 通过、clippy 零告警），此时收敛的验证成本最低。

## What Changes

- **测试临时目录改用 `tempfile`**：以 dev-dependency 引入 `tempfile`，删除各 crate 内自定义的
  `TempDir`/`TempProject`/`TestDir`/`TestRoot`/`TempTree` 的创建与 `Drop` 实现，保留每个测试自己的
  项目结构搭建辅助。目录位置统一通过 `tempdir_in` 落在既有测试工作区，不进入仓库目录。
- **收敛 LSP 集成测试工具**：`crates/wxml-lsp/tests/` 中 4 份重复的 `read_frame` 与 4 份
  `wait_with_timeout` 合并到现有 `tests/common/mod.rs`；超时后的终止与回收策略只保留一处。
- **统一 TypeScript 对象成员扫描**：`script/scanner.rs` 与 `shadow/layout.rs` 两套
  `scan_members`/`member_end`/`parse_member`/`function_keyword_before`/`method_name_before`
  合并为一份，shadow 侧只保留确实不同的策略参数，不再各自维护一份拷贝。
- **合并逐字相同的生产小函数**：`export_target`（`configuration.rs` 与 `workspace_packages.rs`）、
  `hex_lower`（`wxml-backend-tsgo` 内三处）。
- **明确不做的**：不引入完整 TypeScript 解析器替换手写扫描；不引入 `cssparser` 重写样式索引
  （评估结论：仍需为 SCSS 插值、嵌套和"不完整"判定另写一层，收益不足）。
- **BREAKING**：无。协议输出、诊断文本、shadow 文本与用户可见行为均不变。

## Capabilities

### New Capabilities

无。这是纯重构：没有新增或改变任何用户可见行为，改动范围限于实现去重与测试基础设施。

### Modified Capabilities

无。

## Impact

- `crates/wxml-project`：测试模块（`discovery.rs`、`config.rs`、`files.rs`、`sources.rs`）与两个集成测试；
  `Cargo.toml` 新增 dev-dependency `tempfile`。
- `crates/wxml-lsp`：`tests/common/mod.rs` 与使用它的集成测试；`configuration.rs`、`workspace_packages.rs`
  的 `export_target`；测试模块内的临时目录辅助。
- `crates/wxml-backend-tsgo`：`script/scanner.rs`、`script/registration.rs`、`shadow/layout.rs`、
  `input.rs`、`native/{supervisor,vfs}.rs`。
- 依赖：仅新增 **dev** 依赖 `tempfile`（已在 `Cargo.lock` 中作为传递依赖存在，不引入新的树）；发布依赖不变。
- 风险：成员扫描合并触及 shadow 生成路径，需要两侧等价性对比测试；其余三项是删除与搬移。
