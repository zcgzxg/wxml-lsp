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
- 双层 query 呈现：`wxml-backend-tsgo::query` 与 `wxml-lsp::backend_tsgo` 之间多一跳 DTO，部分模块只被 native session 使用。

项目只面向本人在 Linux 上使用 Zed，不需要 Marketplace 发布、远端升级通道或多用户安装语义。

## Decision

1. **删除 `wxml-delivery`**。安装改由 `scripts/install-native.sh` 读取 `assets.lock.json`，下载并校验两个固定资产，安装到 `<root>/bin/wxml-lsp`、`<root>/engine/lib/tsc`、`<root>/typings/`。
2. **简化引擎发现**。运行时只校验上述布局、引擎 SHA-256/大小/权限和类型资产 pin；不再要求 `manifest.json`，删除 manifest schema v2 的包/平台/协议校验。
3. **删除迁移期门禁**：`native_parity`、`wxml-fixtures`、`q2_stability`、`performance_bench`、`stdio_type_loop` 及其辅助模块。保留的普通行为测试继续覆盖 LSP 和 WXML 语义。
4. **收敛 query 边界**：`bindings`、`message`、`probe_health` 移入 `wxml-lsp/src/backend_tsgo/query/`；`completion`、`definition`、`diagnostics`、`document_symbol`、`documentation`、`hover`、`references`、`symbol`、`type_info` 仍由 `wxml-backend-tsgo` 提供，因为 native session 直接组合它们。深度逻辑合并留待有测试窗口时再做。
5. **删除死代码**：移除未被使用的 `TsgoBackend`、`BackendState`、`ResultCache`，保留 `BackendError`。
6. **删除过程文档**：移除 `docs/evidence/`、`docs/plans/`、`tsgo-delivery-contract.md`、`tsgo-test-contract.md`。保留 ADR、reference、fixtures README；历史内容可从 tag `pre-slim-20261120` 取回。
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
- 真实引擎覆盖减少；保留的 opt-in 测试需要显式 `TSGO_BIN`/`TSGO_TYPINGS_ROOT`。
- query 深度合并未完成，`wxml-backend-tsgo` 仍保留 9 个呈现模块。

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

## References

- [ADR-0004 并行任务划分、交接契约与验收闸门](0004-parallel-delivery-and-validation-gates.md)
- [ADR-0007 本地交付与安装信任模型](0007-local-delivery-trust-model.md)
- [ADR-0008 Rust + tsgo 类型后端，移除 Node 依赖](0008-rust-tsgo-no-node-backend.md)
- [项目 README](../../README.md)
- [安装脚本](../../scripts/install-native.sh)
- [资产锁](../../assets.lock.json)
