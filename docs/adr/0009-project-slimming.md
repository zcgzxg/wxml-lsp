# ADR-0009: 项目瘦身：移除交付 crate、迁移证据与 native parity 门禁

**Date**: 2026-11-20

**Status**: accepted

**Deciders**: 项目维护者

## Context

项目在 2026-11-15 至 2026-11-20 的迁移期快速膨胀到 233,783 行 Rust（生产 115,228、测试 118,555，测试占 51%），另有 7.7MB 文档、4,317 行迁移脚本和 287 个依赖节点。

其中相当一部分不是 LSP 功能本身：

- `wxml-delivery` 交付 crate：生产 18,174 行、测试 29,201 行，带 220 个传递依赖，但不被任何 LSP crate 使用；运行时早已独立校验引擎与类型资产。
- 迁移期 native parity/ledger/oracle/F3 测试与门禁：约 17,700 行，包含大量 opt-in/ignored 用例，主要证明迁移过程而不是当前功能。
- `wxml-fixtures`、Q2 稳定性与性能门禁、P7 stdio 门禁：依赖已删除的 parity 辅助代码。
- `docs/evidence` 5.3MB、`docs/plans` 2.0MB、交付/测试合同 130KB：过程证据和已完成的迁移计划。
- 双层 query 呈现：`wxml-backend-tsrs::query` 与 `wxml-lsp::backend_tsrs` 之间多一跳 DTO，部分模块只被 native session 使用。

项目只面向本人在 Linux 上使用 Zed，不需要 Marketplace 发布、远端升级通道或多用户安装语义。

## Decision

1. **删除 `wxml-delivery`**。安装改由 `scripts/install-native.sh` 读取 `assets.lock.json`，下载并校验两个固定资产，安装到 `<root>/bin/wxml-lsp`、`<root>/engine/lib/tsc`、`<root>/typings/`。
2. **简化引擎发现**。运行时只校验上述布局、引擎 SHA-256/大小/权限和类型资产 pin；不再要求 `manifest.json`，删除 manifest schema v2 的包/平台/协议校验。
3. **删除迁移期门禁**：`native_parity`、`wxml-fixtures`、`q2_stability`、`performance_bench`、`stdio_type_loop` 及其辅助模块。保留的普通行为测试继续覆盖 LSP 和 WXML 语义。
4. **收敛 query 边界**：`bindings`、`message`、`probe_health` 移入 `wxml-lsp/src/backend_tsrs/query/`；其余呈现模块仍由 `wxml-backend-tsrs` 提供，`wxml-lsp` 侧只做单一定义再导出，不复制源码。深度逻辑合并留待有测试窗口时再做。

   > 后续核实（2026-11-20）：native session 实际只组合 `completion` 的
   > `CompletionQueryOptions`、`documentation`、`hover` 与 `type_info`。
   > `definition`、`diagnostics`、`document_symbol` 只有 `wxml-lsp` 适配器消费；
   > `references`、`symbol` 只被 opt-in 的 `real_engine` 测试使用，已随本轮清理删除。
   > 本条目原先把 `definition`/`diagnostics`/`references`/`symbol` 一并列为
   > native session 直接组合，与代码不符，现予更正。
5. **删除死代码**：移除未被使用的 `TsrsBackend`、`BackendState`、`ResultCache`，保留 `BackendError`。
6. **删除过程文档**：移除 `docs/evidence/`、`docs/plans/`、`tsrs-delivery-contract.md`、`tsrs-test-contract.md`。保留 ADR、reference、fixtures README；历史内容可从 tag `pre-slim-20261120` 取回。
7. **缩短 CI**：只保留 `check`（fmt、clippy、test、doc、cargo-deny），删除 `native-package` 与 `native-gates` job。
8. **重写 README**：以 native-only 的当前状态为准，不再维护交付 manifest 和 Node 历史操作步骤。

## Alternatives Considered

| 方案 | 优点 | 不采用原因 |
| --- | --- | --- |
| 保留 `wxml-delivery` | 原子升级/回滚、归档缓存、许可证清单 | 个人安装不需要；运行时已独立校验，维护成本远高于收益 |
| 保留 native parity/ledger 门禁 | 迁移证据完整，可逐条追溯旧 Node 行为 | 证据已完成使命；默认 CI 不再运行，保留只会拖慢编译和阅读 |
| 立即深度合并 query | 理论上再省 2–4k 行 | 无测试窗口时风险高；先收敛边界，保留 native session 需要的模块 |
| 保留 `docs/evidence`/`docs/plans` | 历史可读性 | 5.3MB+2.0MB 主要是过程记录；git tag 已保留完整历史 |

## Consequences

### Positive

- Rust 233,783 → 160,258 行（-31.4%），生产代码约 91.5k 行。
- 依赖树从 287 个节点降到 60 个；不再引入 reqwest/rustls/tar/flate2/fs2 链。
- 文档 7.7MB → 344KB，当前事实更容易定位。
- 测试 2,142 passed / 0 failed / 0 ignored，默认测试不再依赖 ignored 门禁。
- 安装从 Rust 二进制简化为一个可审计的 shell 脚本。

### Negative

- 安装器不再提供原子升级、回滚、归档缓存和许可证生成；需要时用 git tag 重建旧版本。
- 旧安装目录（`versions/<version>` + `current`）不能直接复用，需要重新安装。
- 真实引擎覆盖减少；保留的 opt-in 测试需要显式 `TSRS_BIN`/`TSRS_TYPINGS_ROOT`。
- query 深度合并未完成，`wxml-backend-tsrs` 仍保留 9 个呈现模块。

### Risks

- 安装脚本依赖 `curl`、`tar`、`python3` 和网络；离线安装需自行准备资产。
- 删除过程文档后，历史引用只能通过 tag `pre-slim-20261120` 追溯。
- 若未来恢复发布/升级需求，应新增 ADR，重新设计签名、回滚和资产清单，不能直接复用旧交付 crate 的隐含假设。

## Validation

- `cargo check --workspace --all-targets` 通过。
- `cargo clippy --workspace --all-targets -- -D warnings` 通过。
- `cargo test --workspace`：2,142 passed / 0 failed / 0 ignored（46 个 test result）。
- `scripts/install-native.sh` 通过 `bash -n`、`--help`、真实 `assets.lock.json` 解析和错误路径检查；未做网络安装。
- 文件与依赖计数见上；日志位于 `/tmp/opencode/wxml-test.log`、`/tmp/opencode/wxml-clippy-all.log`。

## 后续清理（2026-11-20，同一轮）

ADR 落地后复查又删除了以下无人消费的代码，均不改变运行时行为：

| 删除项 | 位置 | 行数 | 依据 |
| --- | --- | --- | --- |
| `QuerySession::hover_documentation` 遗留通道、`HoverDocumentation` 类型及测试替身管线 | `crates/wxml-lsp/src/backend_tsrs/` | −120 | 生产路径只有 `hover_evidence`；单查询通道无调用者 |
| `query::bindings` 占位 DTO | `crates/wxml-lsp/src/backend_tsrs/query/bindings.rs` | −47 | 零引用；真实的 `RegistrationKind` 等来自 `wxml_backend_tsrs::script` 与 `wxml_project::script` |
| `query::message` 消息清洗模块 | `crates/wxml-lsp/src/backend_tsrs/query/message.rs` | −571 | 零引用；协议边界实际走 `handlers::sanitized_message` → `sanitize_display` |
| `query::probe_health` 探测健康决定表 | `crates/wxml-lsp/src/backend_tsrs/query/probe_health.rs` | −702 | 零引用；模块自述「deliberately does not wire the result into hover, diagnostics or capability status」 |
| 零调用 query DTO/enum（两个 `QueryError`、`DefinitionQuery`、`DiagnosticsQuery`、`CompletionQuery`、`completion_limit`、`CompletionSymbolEvidence` 及其 `symbol` 字段、`DocumentSymbolKind::as_u32`） | `crates/wxml-backend-tsrs/src/query/`、`crates/wxml-lsp/src/backend_tsrs/query/mod.rs`、`crates/wxml-backend-tsrs/tests/real_engine.rs` | −152 | 零调用；`CompletionSymbolEvidence` 只在映射后写出、无读者，删除不影响 `includeSymbol` 门控与 gap/status 计算 |
| `semantics::edit_plan::recheck` 模块及其测试（原为「生成编辑后在隔离快照上重新 parse/typecheck」预埋） | `crates/wxml-lsp/src/semantics/edit_plan/` | −1075 | 零引用；契约 §9.9 第 7 条仍是未来接线 rename 时必须满足的要求，只是不再预置实现 |
| `structure.rs` 中仅测试使用的 4 个 free wrapper（`folding_ranges`/`linked_editing_range`/`selection_chain`/`document_symbols`）与 9 处过期 `allow(dead_code)` | `crates/wxml-lsp/src/structure.rs` | −88 | 5 个 `StructureFacts` 方法经 handler 在生产路径上，属性过期；free wrapper 无生产读者，测试改写为调用方法，覆盖由 `tests/structural_editing.rs` 端到端保留 |

`crates/wxml-lsp/src/backend_tsrs/` 仍保留模块级 `#![allow(dead_code)]`
（`backend_tsrs/mod.rs`），但它不再掩盖上面这批 DTO：剩下的零调用项是
`CompletionQueryOptions::SYMBOLS`、`AutoImportContext`、`AutoImportPreparation`
等 forward-compatibility seam，能否删除取决于 auto-import 是否还要保留扩展位。

`tests/fixtures/README.md` 同步改写：原文引用的 `wxml-fixtures` crate 与
Node `wave5_acceptance` runner 已删除，样本现在由 crate 测试直接作为项目根读取。

## References

- [ADR-0004 并行任务划分、交接契约与验收闸门](0004-parallel-delivery-and-validation-gates.md)
- [ADR-0007 本地交付与安装信任模型](0007-local-delivery-trust-model.md)
- [ADR-0012 进程内 tsrs 类型引擎会话与故障隔离](0012-in-process-tsrs-session.md)
- [项目 README](../../README.md)
- [安装脚本](../../scripts/install-native.sh)
- [资产锁](../../assets.lock.json)
