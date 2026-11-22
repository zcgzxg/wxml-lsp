# ADR-0009: 项目瘦身：移除交付 crate、迁移证据与 native parity 门禁

**Date**: 2026-11-20

**Status**: accepted

**Deciders**: 项目维护者

**2026-11-22 校准**：本文保留瘦身边界、删除范围和不恢复清单；早期版本中的逐文件删除表、源码/依赖/测试计数、临时日志路径和当时的安装数字已移除，避免把迁移期统计当作现状。历史内容见 tag `pre-slim-20261120`。

## Context

迁移期为了证明 Node → native 的等价性，项目同时维护了交付、迁移证据和门禁三类设施。它们不是 LSP 功能本身：

- `wxml-delivery` 交付 crate：带独立依赖树和包/平台/协议校验，但不被任何 LSP crate 使用；运行时早已独立校验类型资产。
- 迁移期 native parity/ledger/oracle/F3 测试与门禁：包含大量 opt-in/ignored 用例，主要证明迁移过程而不是当前功能。
- `wxml-fixtures`、Q2 稳定性与性能门禁、P7 stdio 门禁：依赖已删除的 parity 辅助代码。
- `docs/evidence`、`docs/plans`、交付/测试合同：过程证据和已完成的迁移计划。
- 双层 query 呈现：`wxml-backend-tsrs::query` 与 `wxml-lsp::backend_tsrs` 之间多一跳 DTO，部分模块只被 native session 使用。

项目只面向本人在 Linux 上使用 Zed，不需要 Marketplace 发布、远端升级通道或多用户安装语义。保留这些设施只会拖慢编译、扩大依赖面，并让文档把历史过程误当成现行能力。

## Decision

1. **删除 `wxml-delivery`**。安装改由 `scripts/install-native.sh` 读取 `assets.lock.json`，下载并校验 typings 资产，安装到 `<root>/bin/wxml-lsp` 和 `<root>/typings/miniprogram-api-typings`。早期版本同时安装的 `<root>/engine/lib/tsc` 已由 [ADR-0012](0012-in-process-tsrs-session.md) 替代：引擎进程内运行，不再有外部引擎或标准库文件。
2. **简化引擎发现**。运行时校验安装布局、typings pin，以及 tsrs 版本、AST 协议和能力探针；不再要求 `manifest.json`，删除 manifest schema v2 的包/平台/协议校验和引擎发行物摘要。校验失败在 LSP 握手前 fail closed。
3. **删除迁移期门禁**：`native_parity`、`wxml-fixtures`、`q2_stability`、`performance_bench`、`stdio_type_loop` 及其辅助模块。保留的普通行为测试继续覆盖 LSP 和 WXML 语义。
4. **收敛 query 边界**：`bindings`、`message`、`probe_health` 移入 `wxml-lsp/src/backend_tsrs/query/`；其余呈现模块仍由 `wxml-backend-tsrs` 提供，`wxml-lsp` 侧只做单一定义再导出，不复制源码。native session 实际只组合 `completion` 的 `CompletionQueryOptions`、`documentation`、`hover` 与 `type_info`；`definition`、`diagnostics`、`document_symbol` 只有 `wxml-lsp` 适配器消费。深度逻辑合并仍留待有测试窗口时再做。
5. **删除死代码**：移除未被使用的 `TsrsBackend`、`BackendState`、`ResultCache`，保留 `BackendError`。
6. **删除过程文档**：移除 `docs/evidence/`、`docs/plans/`、`tsrs-delivery-contract.md`、`tsrs-test-contract.md`。保留 ADR、reference、fixtures README；历史内容可从 tag `pre-slim-20261120` 取回。
7. **缩短 CI**：只保留 `check`（fmt、clippy、test、doc、cargo-deny），删除 `native-package` 与 `native-gates` job。
8. **重写 README**：以 native-only 的当前状态为准，不再维护交付 manifest 和 Node 历史操作步骤。

### 不恢复清单

以下内容属于已完成的迁移设施，不应以“兼容”或“完整性”为由恢复：

- `wxml-delivery`、manifest v2、`versions/<version>` + `current` 布局、release/index 生成器；
- `native_parity`、ledger/oracle/F3、`wxml-fixtures`、`q2_stability`、`performance_bench`、`stdio_type_loop`、`wave5_acceptance`；
- `docs/evidence/`、`docs/plans/`、交付/测试合同；
- `TSRS_BIN`、`TSRS_ASSET_ROOT` 或任何外部引擎覆盖变量。

## Alternatives Considered

| 方案 | 优点 | 不采用原因 |
| --- | --- | --- |
| 保留 `wxml-delivery` | 原子升级/回滚、归档缓存、许可证清单 | 个人安装不需要；运行时已独立校验，维护成本远高于收益 |
| 保留 native parity/ledger 门禁 | 迁移证据完整，可逐条追溯旧 Node 行为 | 证据已完成使命；默认 CI 不再运行，保留只会拖慢编译和阅读 |
| 立即深度合并 query | 理论上再省一些代码 | 无测试窗口时风险高；先收敛边界，保留 native session 需要的模块 |
| 保留 `docs/evidence`/`docs/plans` | 历史可读性 | 主要是过程记录；git tag 已保留完整历史 |

## Consequences

### Positive

- 依赖树不再包含为交付引入的 HTTP、归档和文件锁链；安装器只是可审计的 shell 脚本。
- 文档体积显著下降，当前事实更容易定位；ADR、reference 和 fixtures README 保留。
- 默认测试不再依赖 ignored 门禁，普通行为测试成为主路径。
- 安装树只保留二进制与 typings，运行时没有需要回收的外部引擎进程。

### Negative

- 安装器不再提供原子升级、回滚、归档缓存和许可证生成；需要旧版本时用 git tag 重建。
- 旧安装目录（`versions/<version>` + `current`）不能直接复用，需要重新安装。
- 真实类型覆盖仍是 opt-in：设置 `TSRS_TYPINGS_ROOT` 并按需设置 `WXML_REQUIRE_REAL_ENGINE=1`；默认测试通过不代表类型行为已验证。
- query 深度合并未完成，`wxml-backend-tsrs` 仍保留多个呈现模块。
- 编辑计划重检：`semantics::edit_plan::recheck` 已删除；rename 已经接线，但“生成编辑后在隔离快照上重新 parse/typecheck”仍未实现，属于后续工作，不能由本次瘦身宣称完成。

### Risks

- 安装脚本依赖 `curl`、`tar`、`python3` 和网络；没有离线 bundle 模式，离线环境需自行准备可访问的资产源。
- 删除过程文档后，历史引用只能通过 tag `pre-slim-20261120` 追溯。
- 若未来恢复发布/升级需求，应新增 ADR，重新设计签名、回滚和资产清单，不能直接复用旧交付 crate 的隐含假设。

## Validation

本次瘦身的当时验收以统一门禁和安装脚本 smoke 检查为准：`cargo fmt`、`cargo clippy -D warnings`、`cargo test --workspace`、`cargo doc`、`cargo deny --offline check`，以及 `scripts/install-native.sh` 的 `bash -n`、`--help`、真实 `assets.lock.json` 解析和错误路径检查（未做网络安装）。

逐文件删除台账、源码行数、依赖节点数、测试计数和临时日志不再作为现状记录；当前验证状态以 [项目 README](../../README.md) 和 CI（`.github/workflows/ci.yml`）为准。

## References

- [ADR-0004 并行任务划分、交接契约与验收闸门](0004-parallel-delivery-and-validation-gates.md)（历史交付计划，非现行规则来源）
- [ADR-0007 本地交付与安装信任模型](0007-local-delivery-trust-model.md)
- [ADR-0012 进程内 tsrs 类型引擎会话与故障隔离](0012-in-process-tsrs-session.md)
- [项目 README](../../README.md)
- [安装脚本](../../scripts/install-native.sh)
- [资产锁](../../assets.lock.json)
- [CI](../../.github/workflows/ci.yml)
