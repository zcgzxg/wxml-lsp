# P7 crosscheck 修正结果（2026-11-20）

## 0. 范围与结论

本文件逐项处理
[`2026-11-20-tsgo-p7-acceptance-crosscheck.md`](2026-11-20-tsgo-p7-acceptance-crosscheck.md)
的 HIGH / MEDIUM / LOW 项。只改文档口径（README、ADR、迁移计划、证据索引、证据状态与
reference 合同说明），不改代码、测试、Cargo、`docs/plans/tsgo-test-ledger.json`、
native gate 脚本，未 commit；未改并行 agent 的
[`2026-11-20-tsgo-p7-license-generation-fix.md`](2026-11-20-tsgo-p7-license-generation-fix.md)。

**结论**：H-01～H-03 已按当前事实改写为“工作树验收 / P1 fixed / P2 known /
第三方许可 155/155 文本、首方 LICENSE BLOCKED”；M-01/M-03/M-04 的数字与提交已纠正；
M-02 统一为 stability **4 implemented / 1 blocked** 并注明 ledger 未回填；
M-05/M-06 收紧为 PARTIAL 或范围化 PASS；M-07/L-01/L-02/L-03 已补证据边界与指针，
F-02 最终计数标为**待 `p7-final-verification` 追加**。
**仍未闭合**的是需要真实执行或 owner 决策的事项，不是本轮文档能替代的（见 §3）。

## 1. HIGH

| ID | 原问题 | 处理 | 结果 |
| --- | --- | --- | --- |
| H-01 | final-docs 的 2592/0/73、F-02 修复只存在于未提交工作树，tag `p7-removed` 内仍是 2587/1/73；删除提交误写 | [final-docs](2026-11-20-tsgo-p7-final-docs.md) 明确：实际删除提交 **`c4ddf32`**；`p7-removed` 指向 `b6c1ced`，只是旧 tag（只改 final-docs 4 行），**最终验收以工作树为准**；F-02 修复已落地，2592/0/73 **待 `p7-final-verification` 追加**；新增指针 `2026-11-20-tsgo-p7-final-verification.md`（文件尚不存在，标 **verified in worktree, evidence pending**） | **口径已修正**；tag 仍未重打，F-02 最终计数仍待复验 |
| H-02 | functional smoke 报 P1（JSON didOpen 丢弃脚本首次诊断）与 P2（Page-only `documentSymbol` `-32803`） | [functional smoke](2026-11-20-tsgo-p7-final-functional-smoke.md) 改为 **P1 fixed / P2 known / overall PASS-with-followup**；P1 状态链接 [`2026-11-20-tsgo-p1-script-diagnostics-retry.md`](2026-11-20-tsgo-p1-script-diagnostics-retry.md)（`180fa42`，真实 LSP 探针）；final-docs 增加 functional 行 | **P1 已修复并复验**；P2 保留为已知限制，需产品决策 |
| H-03 | 发布许可审计 P1 HIGH / 对外发布 BLOCKED | final-docs、README、ADR-0008、迁移计划 §12.16、证据索引与 [delivery contract](../reference/tsgo-delivery-contract.md) §8.4 统一写：generator/2 已覆盖 **155/155** 个第三方 crate（144 本地 + 11 vendored SPDX，逐行 `text_source`，0 missing / 0 unlicensed）；**11 个 wasm 文本缺口已解决**；`THIRD_PARTY_LICENSES.md` 467,493 B；首方 LICENSE **BLOCKED owner decision**，**对外发布仍 BLOCKED/需 owner 决策，内部安装可用**；security audit 顶部补“许可 PASS 仅指 expression 策略”的范围说明 | **第三方 P1 已解除**；对外发布仍 BLOCKED（首方 LICENSE） |

## 2. MEDIUM / LOW

### 2.1 MEDIUM

| ID | 原问题 | 处理 | 结果 |
| --- | --- | --- | --- |
| M-01 | Zed 写 18/18，日志只有 16；截图写 10 张，实际 9 张 | [post-delete](2026-11-20-tsgo-p7-post-delete-revalidation.md) 改为 **16/16**、截图 **9**；证据索引、final-docs、functional smoke、plan、ADR、README 同步；不再出现 18/18 | **已统一为 16/16**；若存在额外 2 项检查，需另行补日志 |
| M-02 | stability 证据为 4 implemented / 1 blocked，文档多处写 5 项 BLOCKED | README、ADR-0008、迁移计划 §12.14/§12.15/§12.16、证据索引、retarget 证据、native gate coverage、final-docs 统一为 **4 implemented / 1 blocked**；每处注明 **ledger 未回填**（ledger 仍记 4 场景 BLOCKED） | **文档已统一**；ledger 回填不在本轮范围 |
| M-03 | 删除提交误写 `b6c1ced` | final-docs、迁移计划 §12.16 与当前进度行、证据索引 §3.4 全部改为 **`c4ddf32`**；`b6c1ced` 只作为旧 tag/最终文档提交出现 | **已修正** |
| M-04 | 迁移计划 §12.16 仍用删除前 archive `b138d119…`/15,117,268 B | §12.16 改为 post-delete `7a51a3e9…dcd81b7`/15,117,270 B；**许可重建后**（generator/2）另记 `793a4f21…2e4077`/15,143,353 B（以 license 证据为准）；Zed 16/16；[release-validation](2026-11-20-tsgo-p7-release-validation.md) 顶部标为删除前/许可重建前历史快照 | **已修正** |
| M-05 | R-03 标 PASS，但删除后未重跑 | final-docs R-03 改为 **PARTIAL（pre-delete only）**：F3-06 真实 N1/N2 在删除前 `fb6047e`，post-delete §7 明确未运行；ADR 与 plan 同步 | **已收紧**；删除后 upgrade/rollback 仍未执行 |
| M-06 | E-01/E-02/N-02 PASS 范围过宽 | E-01 限定为 **LSP/Zed 进程树**（ptrace 13 exec，0 Node；宿主有无关 Node，不做全机断言）；E-02 改为 **无 Node 执行路径**，显式保留 schema v1 `ArtifactKind::NodeBackend` 只读识别与 `WXML_TEST_NODE` 负向 guard；N-02 改为 **PARTIAL**：HostOnly 由单测覆盖，真实 remove-engine 未在删除后重跑 | **已收紧**；N-02 真实复跑仍未执行 |
| M-07 | F-02 的 2592/0/73 只有自报表格，且只在工作树成立；`--doc` 与 73 ignored 组成缺失 | final-docs F-02 标 **PARTIAL（pending final verification）**，明确**不是 tag 事实**；[F-02 记录](2026-11-20-tsgo-f02-debug-perf-fix.md) 保留修复设计与自报数据，并把最终计数标为 **待 `p7-final-verification` 追加**；`--doc` 行改为“无对应结果记录” | **修复已落地；最终计数仍待复验** |

### 2.2 LOW

| ID | 原问题 | 处理 | 结果 |
| --- | --- | --- | --- |
| L-01 | N-01 的 package 路径与 clean 记录不一致 | final-docs N-01 改为实际 package `/tmp/opencode/p7-post-delete-install/versions/0.1.0`；注明 dirty 工作树显式 `WXML_STABILITY_ALLOW_DIRTY=1`，**不声称 clean 复跑**；证据指向 [gate coverage](2026-11-20-tsgo-p7-native-gate-coverage.md) 与 [residual gates](2026-11-20-tsgo-p7-residual-native-gates.md) | **已修正**；clean 复跑仍未执行 |
| L-02 | F-01/F-03 只有自报，没有原始日志 | final-docs F-01/F-03 标 **PASS（self-reported）**，明确原始日志/rustdoc 输出未保留 | **已如实标注**；日志仍缺 |
| L-03 | 40/40 指针指向 release-validation；“没有任何 BLOCKED”过宽；final-docs 缺 F3/ledger/security/exec/stability 行；“未改动测试”措辞不准确 | 证据索引 §3.4 的 40/40 指针改为 gate coverage/residual gates；post-delete 改为“**本次 release/install/LSP/Zed 范围无 BLOCKED**，F3-02 等仍为残余”；final-docs 新增 F3、L、S、SEC、LIC、functional、交付边界行；final-docs §1 改为“本 doc slice 未改，最终验收依赖工作树其它切片的未提交测试改动” | **已修正**；全仓链接检查需在提交后重跑 |

## 3. 尚未闭合（需真实执行或 owner 决策）

| 项 | 状态 | 原因/下一步 |
| --- | --- | --- |
| tag 与工作树一致 | **OPEN** | 工作树修复未提交；没有新 tag。若要 tag 复现，需提交后重新打 tag；当前只能写“worktree-only, not tag” |
| F-02 最终验证 | **OPEN** | F-02 修复已落地；`cargo test --workspace --all-features` 的 2592/0/73 待 `p7-final-verification` 追加（原始 stdout、`cargo test --workspace --doc` 结果、73 ignored 逐条清单仍缺） |
| functional P2 | **OPEN（已知限制）** | Page-only / Component-only 脚本 `documentSymbol` 仍返回 `-32803`；需产品决策空 outline 或注册级 outline |
| 对外发布许可 | **BLOCKED（首方 LICENSE）** | 第三方文本已 155/155（144 本地 + 11 vendored SPDX）并解除 P1；首方 LICENSE/Cargo license 字段仍缺失，需 owner 声明或许可决策 |
| stability ledger | **OPEN** | 4 个已实现场景未回填 `tsgo-test-ledger.json`；ledger 仍记 BLOCKED |
| R-03 / N-02 | **PARTIAL** | 删除后 upgrade/rollback 与真实 remove-engine 未重跑 |
| 40/40 clean 复跑 | **OPEN** | 现有证据为 dirty 工作树 + `WXML_STABILITY_ALLOW_DIRTY=1`；无 clean summary |
| F-01/F-03 日志 | **OPEN** | self-reported，无原始日志/rustdoc 输出 |
| 全仓链接检查 | **OPEN** | 当前工作树仅剩非产品链接；旧 tag 仍有 18 个 broken；提交后需重跑 |

## 4. 修改文件

| 文件 | 变更 |
| --- | --- |
| ` README.md` | P7 状态：删除提交 `c4ddf32`、旧 tag、F-02 待 `p7-final-verification`、functional P1/P2、stability 4/1、第三方许可 155/155 + 首方 LICENSE BLOCKED |
| ` docs/README.md` | 最终验收指针改为 PASS-with-followup、F-02 待复验与许可边界 |
| ` docs/adr/README.md` | 同上，补 functional/stability/许可边界 |
| ` docs/adr/0008-rust-tsgo-no-node-backend.md` | 删除提交、验收 PASS-with-followup、stability 4/1、R-03/N-02 PARTIAL、许可 155/155 + 首方 BLOCKED、F-02 待 `p7-final-verification` |
| ` docs/plans/2026-11-17-rust-tsgo-migration.md` | §12.16 删除提交/archive/install/Zed/F-02/stability/许可；当前进度行；retarget 行 |
| ` docs/reference/tsgo-delivery-contract.md` | §8.4 许可状态：155/155 文本（144 本地 + 11 SPDX）、首方 LICENSE BLOCKED、对外发布 BLOCKED |
| ` docs/evidence/README.md` | 索引新增 crosscheck/license generation/functional smoke/P1 fix/security/repo/license audit/residual gates/stability/交叉结果行；§3.4 数字与指针修正 |
| ` docs/evidence/2026-11-20-tsgo-p7-final-docs.md` | 主验收清单重写为工作树 PASS-with-followup；H/M/L 口径与新增行 |
| ` docs/evidence/2026-11-20-tsgo-p7-post-delete-revalidation.md` | Zed 16/16、截图 9、BLOCKED 范围化、archive 许可重建说明 |
| ` docs/evidence/2026-11-20-tsgo-p7-final-functional-smoke.md` | P1 fixed / P2 known / PASS-with-followup；Zed 16/16 |
| ` docs/evidence/2026-11-20-tsgo-f02-debug-perf-fix.md` | F-02 修复设计保留；最终 2592/0/73 标为待 `p7-final-verification` 追加；`--doc` 无结果记录与 debug/release 边界 |
| ` docs/evidence/2026-11-20-tsgo-p7-retarget-native-tests.md` | stability 后续 4/1 与 ledger 未回填说明 |
| ` docs/evidence/2026-11-20-tsgo-p7-native-gate-coverage.md` | stability 5 blocked 的历史说明改为“本切片时 5，后续 4/1” |
| ` docs/evidence/2026-11-20-tsgo-p7-release-validation.md` | 顶部标注删除前/许可重建前历史快照与替代证据 |
| ` docs/evidence/2026-11-20-tsgo-p7-final-exec-audit.md` | 明确只支持 E-01 的 LSP/Zed 进程树范围，不支持全机零 Node 断言 |
| ` docs/evidence/2026-11-20-tsgo-p7-final-security-audit.md` | 顶部补“许可 PASS 仅指 expression 策略”的范围说明 |
| ` docs/evidence/2026-11-20-tsgo-p7-crosscheck-resolution.md` | 本文件（新增） |

**未修改**：代码、测试、`Cargo.toml`/`Cargo.lock`、`docs/plans/tsgo-test-ledger.json`、
`scripts/native-gates.sh`、`docs/evidence/2026-11-20-tsgo-p7-license-generation-fix.md`
（并行 agent 文件）。

## 5. 验证方式

本轮只做文档一致性，未执行 `cargo`/native gates/release/Zed/functional smoke，也未 commit。
核对命令限于只读的 `rg`、`git show`、`git log` 和文件读取；`cargo fmt` 不适用于本变更。
