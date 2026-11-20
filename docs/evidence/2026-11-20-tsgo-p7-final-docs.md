# P7 删除后最终文档与验收清单（2026-11-20）

## 0. 结论先行

1. 本记录是 P7 删除后的文档口径与验收清单。**验收对象是当前工作树，不是旧 tag `p7-removed`。**
2. 身份：实际删除提交是 **`c4ddf32`**（`feat(p7): remove node backend and legacy typescript tree`）。`p7-removed` 指向 `b6c1ced`，该提交只改 final-docs 4 行，仍是 F-02 PARTIAL、2587 passed / 1 failed / 73 ignored，**不包含**本轮验收修复；`b6c1ced` 只是旧 tag，不是删除提交。工作树 HEAD 为 `180fa42`（functional P1 修复）。
3. 文档口径：ADR-0008 **accepted (native default; Node removed)**；验收 = **native engine + native gates + F3 + 合理行为测试**；owner waiver 原文保留。
4. Node 运行时、旧 TS 实现/测试树、12 个 delete gate 和 v1 写路径已删除，`--backend node` 已移除；最终状态为 **Node removed / P7 removed**。
5. 最终判定：**PASS-with-followup**，不是全 PASS。F-02（debug perf 假失败）的修复已落地（debug 只打印 p95，release 200 ms 预算未放宽）；`2592 passed / 0 failed / 73 ignored` 的最终验证**待 `p7-final-verification` 追加**，当前不是 tag 事实。functional smoke 为 **P1 fixed / P2 known / overall PASS-with-followup**：P1（JSON didOpen 静默丢弃脚本首次诊断）已由 `180fa42` 修复并真实复验；P2（仅 `Page(...)`/`Component(...)` 的脚本 `documentSymbol` 返回 `-32803`）保留为已知限制。
6. 发布许可：**第三方许可文本 P1 已解除**——155/155 个第三方 crate 有文本（144 crate 本地 + 11 个 Zed wasm 使用 vendored SPDX，逐行 `text_source`），0 missing / 0 unlicensed。**首方 LICENSE 仍 BLOCKED owner decision，对外发布仍 BLOCKED/需 owner 决策**；内部安装/校验/LSP/Zed 可用。

## 1. 身份与边界

| 项 | 值 |
| --- | --- |
| 基线 | `3e24c44` + P7 删除补丁；实际删除提交 `c4ddf32`；旧 tag `p7-removed` 指向 `b6c1ced`；工作树 HEAD `180fa42`（含 P1 修复；F-02/许可/stability 修复在工作树） |
| 文档切面 | `README.md`、`docs/README.md`、`docs/adr/**`、`docs/evidence/README.md`、`docs/plans/2026-11-17-rust-tsgo-migration.md`、`docs/reference/tsgo-*.md`、本文件 |
| 未改动 | 本 doc slice 未改生产代码、测试、`docs/plans/tsgo-test-ledger.json`、`legacy-p7-freeze.json`、脚本；最终验收依赖工作树中其它切片的未提交改动（`type_assets.rs`、`q2_stability.rs`、`asset_loader.rs`、`f1_hover.rs`、`tests/support/`、`editors/zed/src/lib.rs`、license 生成代码） |
| 删除动作 | 已执行（`c4ddf32`）：Node/旧 TS/v1 删除补丁已提交；后续验收修复与证据在 `c4ddf32..180fa42` 及其未提交工作树 |
| 平台边界 | 仅有 Linux x86_64 证据；其它平台不声明 |

## 2. 删除完成的前置条件

最终验收前确认以下删除面已落地：

| 删除面 | 完成标准 |
| --- | --- |
| Node 运行时 | `crates/wxml-lsp/src/backend_node/**` 不存在；`--backend node`、`serve_node()` 与 legacy Node env 已移除 |
| 旧 TS 树 | `backends/typescript/**` 不存在；构建、测试、安装、发布均不再引用它 |
| 12 个 delete gate | 已按冻结证据删除并标记 `old_test_removed=true`；不允许提前删除 |
| v1 写路径 | 15 个 delivery 写符号移除；仅保留 schema v1 只读识别、`verify_installed`、`read_legacy_manifest`、`build_native_package` |
| 脚本/CI/Zed | `verify-no-node.sh` 反转检查、`native-gates.sh` 计数同步、Zed 不再转发 legacy Node 变量 |

**显式保留**：业务 `.ts/.js/.d.ts` 输入与 JSDoc、WXS/WXML 语义、tree-sitter grammar、统一 fixtures、`assets.lock.json`/typings 资产、schema v1 只读边界、历史证据与台账。去 Node 不等于删除 TypeScript 语言支持。

## 3. 最终验收清单（工作树口径，已回填；F-02 最终计数待复验）

> 结果列只能由主线程在实际执行后填写。每项至少记录：命令、提交、日期、退出码、日志/产物路径。没有原始输出的“通过”一律无效；标 **self-reported** 的项只作为工作树自报证据（E0），不得当作完整 PASS。

crosscheck 的逐项处理见
[`2026-11-20-tsgo-p7-crosscheck-resolution.md`](2026-11-20-tsgo-p7-crosscheck-resolution.md)。

| ID | 检查项 | 命令/方法 | 通过标准 | 结果 | 证据/备注 |
| --- | --- | --- | --- | --- | --- |
| F-01 | workspace 静态检查 | `cargo fmt --all -- --check`；`cargo clippy --workspace --all-targets -- -D warnings` | 两个命令 exit 0，无 warning | PASS（self-reported） | HEAD `8ec0427`；fmt/clippy 均 exit 0；**原始日志未保留**（L-02） |
| F-02 | workspace 测试 | `cargo test --workspace --all-features`；`cargo test --workspace --doc` | 0 failed；ignored 仅限已登记 opt-in gate | **PARTIAL（pending final verification）** | F-02 修复已落地（debug perf 假失败已修复，release 200 ms 预算未放宽）；**2592 passed / 0 failed / 73 ignored 待 `p7-final-verification` 追加**；**不是 tag 事实**（tag `b6c1ced` 仍为 2587/1/73）；原始 stdout、`--doc` 结果和 73 ignored 清单未保留；见 [F-02 记录](2026-11-20-tsgo-f02-debug-perf-fix.md) |
| F-03 | workspace 文档 | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | exit 0，无 warning | PASS（self-reported） | 修复私有 intra-doc link 后 `DOC_EXIT=0`；**原始 rustdoc 输出未保留**（L-02） |
| N-01 | native gates 40/40 | `scripts/native-gates.sh --package /tmp/opencode/p7-post-delete-install/versions/0.1.0` | exit 0；23 engine + 3 edit + 9 stdio + 1 perf + 1 stability + 3 real_matrix = **40/40**；固定 `7.1.0-beta` | PASS | 组成与 [gate coverage](2026-11-20-tsgo-p7-native-gate-coverage.md)、[residual gates 复验](2026-11-20-tsgo-p7-residual-native-gates.md) 一致；运行在 dirty 工作树显式 `WXML_STABILITY_ALLOW_DIRTY=1`；**不声称 clean 复跑**；engine SHA-256 `a183b195…` |
| N-02 | native 默认与 HostOnly | 真实安装下启动 `wxml-lsp --stdio`；再移除 engine 复跑 | 有安装走 native；仅 `NotInstalled` → `HostOnly`；损坏安装 fail closed | PARTIAL | HostOnly 由单测覆盖；**真实 remove-engine 未在删除后重跑**；不把 HostOnly 当 native 通过 |
| R-01 | release 确定性 | 连续两次运行 `scripts/build-release.sh` | 两次产物 SHA-256 相同；包内无 `server.js`、无 `backends/typescript` | PASS（含构建次数边界） | 删除后（许可重建前）A/B/C 三次逐字节一致：archive `7a51a3e9…dcd81b7`、15,117,270 B；**许可重建后**（generator/2）E 构建为 `793a4f21…2e4077`、15,143,353 B，仅单次构建，逐字节确定性由生成器单测覆盖；以 [license 证据](2026-11-20-tsgo-p7-license-generation-fix.md) 为准 |
| R-02 | install/verify | `wxml-lsp-install` 安装、`verify`、`verify-package` | 全部 exit 0；schema v2 native 包可安装 | PASS | 删除后安装 157 files / 41,899,646 B；许可重建后（generator/2）安装 157 files / 42,310,099 B；见 post-delete 与 license 证据 |
| R-03 | upgrade/rollback | 两个 native 版本间升级、回退、失败保持 | 升级/回退成功；失败保持旧 current；不自动回退 Node | PARTIAL（pre-delete only） | F3-06 的 N1/N2 真实证据在删除前 `fb6047e`；**删除后未重跑**，post-delete §7 明确列为未运行；不作为最终 PASS |
| Z-01 | Zed smoke | 用最终 native 包设置 `WXML_LSP_BIN`，在 Linux Zed 执行 hover/definition/completion/diagnostics | 四项均通过；无 wrapper；无 Node 子进程 | PASS | 日志为 **16/16** `F3-14-ZED-CHECK: PASS`，截图 **9** 张；不写 18/18、不写 10 张；见 [post-delete](2026-11-20-tsgo-p7-post-delete-revalidation.md) §6 |
| E-01 | 无 Node 进程 | ptrace exec 审计 + `scripts/verify-no-node.sh` | **LSP/Zed 进程树内**无 Node/npm/`server.js`；脚本 PASS | PASS（范围化） | installer 10 + native LSP 3 = **13 exec**，0 Node/server.js；宿主本身有无关 Node 进程（OpenCode/CodeGraph/MCP），**不做全机零 Node 断言**；见 [exec 审计](2026-11-20-tsgo-p7-final-exec-audit.md) |
| E-02 | 源码无 Node 边界 | 搜索 `backend_node`、`NodeBackend`、`WXML_TEST_NODE`、`dist/server.js` | **无 Node 执行路径**；schema v1 只读识别与负向 guard 保留 | PASS（窄口径） | 生产 schema v1 仍保留 `ArtifactKind::NodeBackend` 只读识别（`manifest.rs`），native gate 仍用 `WXML_TEST_NODE` 作负向 guard；不声称“全仓库无 NodeBackend 字样” |
| D-01 | ADR 口径 | 检查 `docs/adr/0008-rust-tsgo-no-node-backend.md`、`docs/adr/README.md` | `accepted`；验收口径、owner 原文、`removed` 条件齐全 | PASS | 删除完成后才允许写 `removed` |
| D-02 | README/计划口径 | 检查 `README.md`、迁移计划、references | native 默认/HostOnly；Node 命令标注 P7 删除范围；保留项齐全 | PASS-with-followup | crosscheck H-01～H-03、M-01～M-07、L-01～L-03 中与本切面相关的口径修复已在本工作树完成（删除提交、F-02 待 `p7-final-verification`、functional P1/P2、Zed 16/16、stability 4/1、R-03/N-02/E-01/E-02 范围、许可 155/155 + 首方 BLOCKED）；followup 为随本轮一并提交后重跑口径 grep |
| D-03 | 文档一致性 | 运行仓库内 Markdown 链接检查；核对表格列数 | 相对链接全部存在；表格列数一致 | PASS-with-followup | 当前工作树仅剩 `.opencode/` 工具模板与 vendored typings fixture 的非产品链接；**tag `b6c1ced` 仍有 18 个 P7 删除造成的 broken link**，修复未提交；followup 为提交后重跑全仓检查 |
| F3 | 交付合同 | 检查 `docs/reference/tsgo-delivery-contract.md` §9.5 | 16 closed / 0 partial / 0 blocked | PASS | F3 交付合同单独成立；不等于 G7 parity；F3-09 是删除前历史验收，当前 HEAD 不可重跑 |
| L | 测试台账 | `docs/plans/tsgo-test-ledger.json` + closeout ledger | 109 green / 0 planned / 130 reviewed / 29 waived / 71 retired（339） | PASS | 非 green 终态未写成 green；stability 的 4 个已实现场景**未回填 ledger**，ledger 仍记 BLOCKED |
| S | stability | `q2_stability` native gate + scenario 证据 | 10 轮生命周期 + 4 implemented / 1 blocked | PASS-with-followup | 4 implemented（typing burst/cancel、资源删除恢复、app.json overlay、40 请求 burst）/ 1 blocked（外部 RSS baseline）；ledger 未回填；证据使用 dirty 工作树 |
| SEC | 安全 / exec | 安全审计 + ptrace exec 审计 | 漏洞/unsafe/exec 范围 0 HIGH/CRITICAL | PASS（范围化） | 最高残余 MEDIUM（预编译 beta 引擎无内核隔离）；内核沙箱、`openat` TOCTOU、非 Linux、网络 `connect` 仍 BLOCKED；**许可文本完整性不在本行 PASS 范围** |
| LIC | 发布许可 | 许可生成 + release 许可审计 | 对外发布许可材料完整 | **BLOCKED（对外发布；首方 LICENSE）** | 第三方文本 P1 已解除：155/155 有文本（144 本地 + 11 vendored SPDX，逐行 `text_source`），0 missing / 0 unlicensed；`THIRD_PARTY_LICENSES.md` 467,493 B（SHA `d7ef5513…`）；**首方 LICENSE 仍 `BLOCKED owner decision`**，对外发布需 owner 决策；内部安装可用 |

## 4. 结果填写与判定规则

1. **结果**：以上为已记录结果；没有真实命令输出的事项不得标完整 PASS；标 self-reported 的项按 E0 工作树自报证据处理。
2. **总体判定**：**PASS-with-followup**。交付合同、native gates、F3、ledger、安全/exec 和内部安装可用；R-03/N-02 为 PARTIAL，functional P2、F-02 最终计数与首方 LICENSE 是已知 followup。
3. **失败/阻塞**：F-02 修复已落地，**2592/0/73 待 `p7-final-verification` 追加**（release 预算未放宽）；对外发布许可为 **BLOCKED（首方 LICENSE）**；stability 为 4 implemented / 1 blocked；不得把上述任一写成“全部通过”。
4. 删除已完成；工作树验收修复（F-02、stability、functional P1、license generator/2、文档）尚未提交。**旧 tag `p7-removed`（`b6c1ced`）不能作为这些结论的复现点**；若需要 tag 复现，必须提交后重新打 tag（当前不存在新 tag）。
5. 历史证据保持原事实；本清单只追加新结果，不改写旧记录。

## 5. 当前文档状态

| 文档 | 当前口径 |
| --- | --- |
| ADR-0008 | `accepted (native default; Node removed)`；验收 = native engine + native gates + F3 + 合理行为测试；owner 原文“不需要严格按照原来的功能写测试。功能合理即可” |
| README | 默认 `--stdio` native；仅 `NotInstalled` → HostOnly；损坏安装 fail closed；Node 命令标注 P7 已删除，当前不可运行 |
| 迁移计划 | §12.14/§12.15 为历史快照；§12.16 为 P7 删除完成终态；`removed` 已写入；§12.16 的 archive/Zed/许可数字已按 post-delete 与 license generator/2 证据修正 |
| 证据索引 | 已登记 retarget、matrix 源归档、F-02、post-delete revalidation、exec/security/repo/license 审计、functional smoke、P1 fix、stability 与 crosscheck resolution；本清单状态为 **PASS-with-followup（worktree：F-02 修复已落地、2592/0/73 待 p7-final-verification 追加；functional P1 fixed / P2 known；stability 4/1；第三方许可 155/155，首方 LICENSE BLOCKED）** |

## 6. 明确不声称

- Node/旧 TS/v1 已删除；不再把 P7 描述为进行中。
- F-02 修复已落地（debug perf 假失败已修复，release 200 ms 预算未放宽）；**2592/0/73 待 `p7-final-verification` 追加**，且不是 tag `p7-removed` 的事实；不把修复写成“放宽预算”。
- 不声称 functional smoke 全 PASS：P1 已修复，P2（Page-only `documentSymbol` `-32803`）仍是已知限制，整体 PASS-with-followup。
- 不声称对外发布许可已通过：第三方文本已 155/155（144 本地 + 11 vendored SPDX），但首方 LICENSE 仍 BLOCKED；内部安装可用不等于对外发布合规。
- 不声称 stability 全部实现：4 implemented / 1 blocked，ledger 尚未回填。
- 不声称 R-03 在删除后通过：只有 pre-delete 的 F3-06 证据；N-02 的 remove-engine 真实复跑也未执行。
- 不声称全机零 Node 进程或无任何 `NodeBackend` 字样：只在 LSP/Zed 进程树与执行路径范围内成立。
- 不声称旧 TS/Node 测试逐条 parity；`reviewed`/`waived`/`retired` 不是 green。
- native gates 40/40 是 opt-in required job；不是默认 CI 绿或 `mandatory_green`。
- Linux x86_64 以外平台未验证；G7 逐条 parity 未声称。
