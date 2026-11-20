# P7 最终验收声明交叉核对（2026-11-20）

## 0. 结论摘要

- **最高严重级别：HIGH**。不是构建或安装失败，而是**最终验收对象与 tag 内容不一致**：`p7-removed`/HEAD `b6c1ced` 的已提交 final-docs 仍写 **F-02 PARTIAL、2587 passed / 1 failed / 73 ignored**；`2592 / 0 / 73`、F-02 修复、stability 4/1 和文档修复都只存在于**未提交工作树**。
- **一致且有证据**：native gates 40/40 的 **23 + 3 + 9 + 1 + 1 + 3** 组成与六条 gate 日志一致；F3 **16/0/0**、ledger **109/0/130/29/71**、post-delete archive **7a51a3e9…/15,117,270 B**、ptrace **13 exec**、安全审计的 **0 HIGH/CRITICAL（漏洞/unsafe/exec 范围）** 均有对应文件或日志。
- **主要偏差**：Zed 证据写 **18/18**，实际日志只有 **16** 条 `F3-14-ZED-CHECK: PASS`（截图 9 张，不是 10 张）；stability 证据是 **4 implemented / 1 blocked**，但 README/ADR/plan/证据索引仍多处写 **5 项 BLOCKED**；迁移计划 §12.16 仍用删除前的 **b138d119…/15,117,268 B、Zed 16/16**，不是 post-delete 的 **7a51a3e9…/15,117,270 B**。
- **过宽 PASS**：`R-03` 明确“删除后未重跑”却标 PASS；`E-01` 写“无 Node 进程”，但宿主确有 Node，证据本身也声明不做全机零 Node 断言；`E-02` 写“无活跃引用”，但生产 schema v1 仍有 `NodeBackend`；`N-02` 的“移除 engine 复跑”没有 post-delete 证据。
- **并发新增证据（不在原始来源清单，但直接影响“已验收”）**：final install 功能冒烟报 **P1**（JSON didOpen 静默丢弃脚本首次诊断，20 秒不重试）和 **P2**（仅 `Page(...)` 的脚本 `documentSymbol` 返回 `-32803`）；发布许可审计报 **P1 HIGH、对外发布许可门禁 BLOCKED**。若这些属于最终交付，final-docs 的“已验收”应改为 **PARTIAL/BLOCKED**。

## 1. 审计快照与范围

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b` |
| Tag `p7-removed` | `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b`（与 HEAD 相同） |
| 实际删除提交 | `c4ddf32`（`feat(p7): remove node backend and legacy typescript tree`）；`b6c1ced` 只改 final-docs 4 行 |
| 工作树（快照） | 33 个 tracked 修改；untracked 至少 11 个（9 份 evidence 文档 + 2 个 support 目录，审计期间仍在增加） |
| 审计时间（UTC） | 2026-11-20T02:15Z 前后 |
| 方法 | 只读比对；读取文档、ledger JSON、gate 日志、release/install 产物、ptrace 日志、Zed 日志；未重跑全量测试或 native gates；未改代码/测试/README/ledger；未 commit |

> 说明：审计期间工作树仍在被其它会话修改；行号以本快照为准。`b6c1ced` 与 `8ec0427` 的差异只有 final-docs 4 行，因此 `8ec0427` 的 40/40 运行对最终**代码**等价，但对最终**文档/验收**不等价。

## 2. 一致项

| 检查 | 声明 | 证据 | 判定 |
| --- | --- | --- | --- |
| native gates 组成 | 23 engine + 3 edit + 9 stdio + 1 perf + 1 stability + 3 real_matrix = 40/40 |  scripts/native-gates.sh:54-61；`/tmp/opencode/native-gates/20261120T012446Z/summary.txt` 的 23/3/9/1/1/3 与各日志 `test result` 一致 | **组成一致**；命令与 clean 记录见 L-01 |
| F3 交付合同 | 16 closed / 0 partial / 0 blocked |  docs/reference/tsgo-delivery-contract.md:3、:12、:1398； docs/evidence/README.md:168、:199、:231；F3-06/09/14 证据 | **一致**；边界见 4.2（F3-09 历史、F3-02 仅 Linux x86_64） |
| ledger 终态 | 109 green / 0 planned / 130 reviewed / 29 waived / 71 retired（339） | `docs/plans/tsgo-test-ledger.json` 的 `counts.statuses` 与 cases 计数； docs/evidence/2026-11-20-tsgo-p7-closeout-ledger.md | **一致**；非 green 终态未被写成 green |
| post-delete release | archive 7a51a3e9…；size 15,117,270；157 payload files / 41,899,646 B |  docs/evidence/2026-11-20-tsgo-p7-post-delete-revalidation.md:94-95、:153；实测 A/B/C 三份 archive 同 SHA/同大小 | **一致**；迁移计划仍用旧值，见 M-04 |
| ptrace exec | installer 10 + native LSP 3 = 13 exec；0 Node/server.js；13/13 绝对路径 |  docs/evidence/2026-11-20-tsgo-p7-final-exec-audit.md:9-17、:177；`/tmp/opencode/p7-final-exec-audit.log` 中 13 条 `EXEC pid=` 行 | **一致** |
| 安全审计 0 HIGH | 无 HIGH/CRITICAL；最高残余 MEDIUM |  docs/evidence/2026-11-20-tsgo-p7-final-security-audit.md:5-11、:308-309；cargo audit 225 crates / 0 漏洞；cargo deny 四项 ok | **漏洞/unsafe/exec 范围一致**；发布许可文本另有 BLOCKED，见 H-03 |
| 当前工作树链接 | D-03：仅 `.opencode/` 模板与 vendored typings fixture 的非产品链接 | 本审计排除 `.opencode/` 后检查 `git ls-files '*.md'`：唯一 broken 是 `crates/wxml-delivery/tests/fixtures/miniprogram-api-typings-5.3.1/README.md` 的 `README-en.md` | **当前工作树一致**；tag HEAD 的 18 个 broken 见  docs/evidence/2026-11-20-tsgo-p7-final-repo-audit.md:127，修复未提交，见 H-01 |

## 3. 偏差项

### 3.1 HIGH

#### H-01 最终验收结论不在 tag `p7-removed` 内（验收对象不一致）

- **声明**： final-docs 写“F-02 已修复，全工作区 **2592 passed / 0 failed / 73 ignored**”，见  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:8、:41；README、ADR、计划同步为 2592，见  README.md:25、 docs/adr/0008-rust-tsgo-no-node-backend.md:12、 docs/plans/2026-11-17-rust-tsgo-migration.md:927。
- **反证**：`p7-removed` 指向的 HEAD `b6c1ced` 中，final-docs 仍是 **F-02 PARTIAL、2587 passed / 1 failed / 73 ignored**（`git show HEAD:docs/evidence/2026-11-20-tsgo-p7-final-docs.md:41`）；HEAD 的 ADR 与计划也仍是 2587/1（`git show HEAD:docs/adr/0008-rust-tsgo-no-node-backend.md:12`、`git show HEAD:docs/plans/2026-11-17-rust-tsgo-migration.md:927`）。2592 依赖的 `type_assets.rs` 修复是未提交改动：F-02 证据自己写“HEAD `b6c1ced` / 改动文件 `type_assets.rs`”，见  docs/evidence/2026-11-20-tsgo-f02-debug-perf-fix.md:5、:7；`git status` 当前为 33 M + 11 ??。
- **更严重的时序问题**：`c4ddf32`（09:16）提交 final-docs 时，post-delete revalidation（09:45）、exec audit（09:49）、security audit（09:51）、repo audit（09:54）、stability scenarios（09:56）尚未产生；但 `c4ddf32` 的 final-docs 已给 R-01/R-02/Z-01/E-01 等行填 PASS。`b6c1ced`（09:30）只改了 F-01/F-02/N-01 四行，也未包含这些证据。它违反了 final-docs 自己的规则“结果列只能在实际执行后填写”（ docs/evidence/2026-11-20-tsgo-p7-final-docs.md:36）。
- **判定**：若交付物是 tag `p7-removed`，则 2592/0/73、F-02 PASS、post-delete release/Zed/exec/security 结论**不成立**（至少是“未包含在 tag 中”）；若交付物是当前工作树，则 tag/commit 身份已过期，不能称 `p7-removed` 为最终验收 tag。
- **修正**：提交工作树中的 F-02、stability scenario、文档与证据改动；重新打 tag（或明确写“worktree-only, not tag”）；final-docs 在提交前不得把 2592/0/73 写成已验收。

#### H-02 final install 功能冒烟 P1：JSON didOpen 静默丢弃脚本首次诊断

- **声明**：final-docs 写“删除后验收完成 / 已验收”，验收口径包含“合理行为测试”，见  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:5-8、:70。
- **反证**：审计期间新增的  docs/evidence/2026-11-20-tsgo-p7-final-functional-smoke.md:12-17 记录：最终安装二进制（SHA `aa8aa03b…`，与 post-delete 安装一致）上 36 条操作只有 **32 PASS / 1 FAIL / 3 INFO**； 同文件:21-27 给出 **P1**：JSON 在脚本首次诊断进行中 `didOpen` 时，TS 首次 `publishDiagnostics` 被静默丢弃，等待 20 秒仍不重试；编辑脚本后才恢复。P2 是仅 `Page(...)`/`Component(...)` 的脚本 `documentSymbol` 返回 `-32803`，见 同文件:336-341。
- **判定**：这和“已验收/合理行为测试通过”直接冲突；40/40 与 F3 16/0/0 都不覆盖这个“JSON 与脚本并发 didOpen”的顺序。即使 P1 是并发会话新发现，final-docs 也不能在未处理/未声明的情况下写“已验收”。
- **修正**：修复后重建 final install 并重跑功能冒烟；或在 final-docs 明确写“交付合同 PASS，功能级 P1/P2 未关闭”，并把验收状态改为 PARTIAL。

#### H-03 发布许可审计 P1：对外发布许可门禁 BLOCKED

- **声明**：安全审计写“供应链门禁 PASS；cargo audit 0、cargo deny 四项 ok”，见  docs/evidence/2026-11-20-tsgo-p7-final-security-audit.md:12-13、:23；final-docs/README/ADR 写“release build/install/Zed smoke 5/5、已验收”。
- **反证**：审计期间新增的  docs/evidence/2026-11-20-tsgo-p7-release-license-audit.md:5-7、:14、:18-19 判定 **对外发布/发布候选 BLOCKED、最高 P1 HIGH**：`THIRD_PARTY_LICENSES.md` 只含 engine/typings 两份上游许可；两个 Rust bin 合计 109 个 runtime 外部 crate、Zed wasm 114 个 crate 均无许可文本/版权声明。 同文件:412、:468-478 给出完整风险与“内部验证需显式接受 B7/NF-09”的边界。
- **判定**：安全审计的“0 HIGH/CRITICAL”只覆盖漏洞/unsafe/exec，不等于“许可材料完整”；final-docs 的 release PASS 没有说明“内部安装可用、对外发布 BLOCKED”。若最终交付包含对外 release，这是 HIGH/BLOCKED，不是 PASS。
- **修正**：生成完整 Rust/Zed 依赖许可包并重新打包，或书面接受内部使用 waiver 并在 final-docs/README/ADR 明确写“release 仅内部验证，B7/NF-09 未关闭”。

### 3.2 MEDIUM

#### M-01 Zed 18/18 与日志 16/16 矛盾

- **声明**： post-delete revalidation 写 **18/18 CHECK PASS**，见  docs/evidence/2026-11-20-tsgo-p7-post-delete-revalidation.md:18、:325；证据索引也写 18/18，见  docs/evidence/README.md:92。final-docs 的 Z-01 只写“四项均通过”，没有写 18/18，见  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:48。
- **反证**：`/tmp/opencode/p7-post-delete-logs/zed-smoke.log` 只有 **16** 条 `F3-14-ZED-CHECK: PASS`；  scripts/f3-zed-smoke.sh 的 direct 模式也只有 16 个 pass 分支。截图目录实际 **9** 张，不是 revalidation 写的 10 张（ docs/evidence/2026-11-20-tsgo-p7-post-delete-revalidation.md:333）。
- **判定**：`16/16` 有日志支撑（迁移计划 §12.16 也写 16/16，见  docs/plans/2026-11-17-rust-tsgo-migration.md:926）；`18/18` 无法由该日志复现。若另有 2 项检查，必须补日志；否则改为 16/16，截图改为 9 张或补第 10 张。
- **修正**：修正 post-delete 与证据索引；保留 16/16 作为日志口径。

#### M-02 stability：证据是 4 implemented / 1 blocked，文档仍多处写 5 项 BLOCKED

- **声明**： stability 证据明确写 **4 scenarios IMPLEMENTED / 1 BLOCKED in the canonical run**，见  docs/evidence/2026-11-20-tsgo-stability-scenarios.md:20-26；逐项表见 同文件:71-77（typing burst/cancel、资源删除恢复、app.json overlay、40 请求 burst 为 IMPLEMENTED；外部 RSS baseline 为 BLOCKED）；证据索引也登记 4/1，见  docs/evidence/README.md:95。
- **反证**： README.md:662、 docs/adr/0008-rust-tsgo-no-node-backend.md:51、:163、 docs/plans/2026-11-17-rust-tsgo-migration.md:934、 docs/evidence/README.md:236、 docs/evidence/2026-11-20-tsgo-p7-retarget-native-tests.md:242、 docs/evidence/2026-11-20-tsgo-p7-native-gate-coverage.md:154-157、:170 仍写 **5 项 BLOCKED**。final-docs 只在残余风险写“stability BLOCKED”，没有写 4/1（ docs/evidence/2026-11-20-tsgo-p7-final-docs.md:60）。
- **判定**：最终状态把 4 个已实现场景误写成全 blocked；这是“保守但不准确”，会低估已完成覆盖，也与证据索引同文件自相矛盾。另：canonical stability 运行使用 `WXML_STABILITY_ALLOW_DIRTY=1`（同文件:63），且 `TSGO_TYPINGS_ROOT` 指向 `/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/...`（同文件:62），不是干净工作树/最终包内 typings；4/1 是脏树 + 外部 typings 的证据。ledger 仍把四项记为 BLOCKED（同文件:127），需要么回填，要么明确“ledger 未回填”。
- **修正**：README/ADR/plan/index/retarget/coverage 统一改为 **4 implemented / 1 blocked**；final-docs 加 stability 行；如坚持 ledger 5 blocked，明确“测试覆盖 4/1，ledger 尚未回填”并说明外部 RSS baseline 仍 blocked。

#### M-03 删除提交误写为 `b6c1ced`，实际是 `c4ddf32`

- **声明**： final-docs 写“删除提交 `b6c1ced`（tag `p7-removed`），含 `4035bb7`”，见  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:14；迁移计划 §12.16 写“删除提交为 `b6c1ced`”，见  docs/plans/2026-11-17-rust-tsgo-migration.md:919。
- **反证**：`b6c1ced` 的 diff 只有 final-docs 4 行，不是删除提交；实际删除提交是 `c4ddf32`，见  docs/evidence/2026-11-20-tsgo-p7-final-security-audit.md:38。`b6c1ced` 的父提交是 `8ec0427`，`4035bb7` 只是更早的祖先（“含”只能按 ancestry 解释，容易误导）。
- **判定**：provenance 错误；tag 指向 `b6c1ced` 与“删除提交 `b6c1ced`”是两件事。
- **修正**：改为“删除提交 `c4ddf32`；最终文档提交/tag `b6c1ced`（tag `p7-removed`）”。

#### M-04 迁移计划 §12.16 仍用删除前 release/Zed 数字

- **声明**：  docs/plans/2026-11-17-rust-tsgo-migration.md:926 写 archive `b138d119…`、15,117,268 B、Zed 16/16。
- **反证**：post-delete 复验是 archive `7a51a3e9…`、15,117,270 B，见  docs/evidence/2026-11-20-tsgo-p7-post-delete-revalidation.md:94-95；`b138d119…`/15,117,268 来自删除前的  docs/evidence/2026-11-20-tsgo-p7-release-validation.md:11、:63-64。后者还明确写“不代替 native gates/升级回滚”（同文件:18-19）。
- **判定**：§12.16 是“最终”章节，却引用旧的 pre-delete 构建；和 post-delete 证据、A/B/C 实测都不一致。Zed 16/16 恰好与日志一致，但与 post-delete 声称的 18/18 冲突（见 M-01）。
- **修正**：§12.16 改用 post-delete `7a51a3e9…/15,117,270`；Zed 用 16/16（除非补出 18 项日志）；删掉/标注旧的 release-validation 数字。

#### M-05 R-03 upgrade/rollback 标 PASS，但“删除后未重跑”

- **声明**： final-docs R-03 结果写“PASS（F3-06 真实 N1/N2；删除后未重跑）”，见  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:47；通过标准是“升级/回退成功；失败保持旧 current”。
- **反证**：post-delete revalidation 明确“未运行升级/回滚”，见  docs/evidence/2026-11-20-tsgo-p7-post-delete-revalidation.md:355-356；F3-06 证据是删除前 `fb6047e` 的真实 N1/N2。ADR 却写“删除后全量重验”已完成，见  docs/adr/0008-rust-tsgo-no-node-backend.md:53。
- **判定**：这是“pre-delete PASS”被写成最终 PASS；按 final-docs 自己的规则“没有真实命令输出不得标 PASS”，应至少是 PARTIAL 或“PASS (pre-delete only) / post-delete NOT RUN”。删除 Node/v1 后升级/回滚代码是否受影响没有 post-delete 证据。
- **修正**：在最终安装上重跑 N1/N2 upgrade/rollback；或把 R-03 改为 PARTIAL，并说明删除后未覆盖。

#### M-06 E-01/E-02/N-02 的 PASS 范围过宽

- **E-01（无 Node 进程）**： final-docs 写“执行 `pgrep -af 'node'` …… 无 Node/npm/server.js 进程；记录命令与空输出”，见  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:49。post-delete 证据自己写“宿主存在无关 Node 进程（OpenCode/CodeGraph/MCP），不做全机零 Node 断言”，见  docs/evidence/2026-11-20-tsgo-p7-post-delete-revalidation.md:349-350；本机 `pgrep -af node` 也确实非空。应改为“LSP/Zed 进程树内无 Node 子进程”，并以 ptrace/verify-no-node 为证据；不要写“无 Node 进程/空输出”。
- **E-02（源码无 Node 边界）**： final-docs 写“除历史文档/归档证据外，无活跃引用”，见  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:50。但生产 schema v1 仍有 `ArtifactKind::NodeBackend`，见  crates/wxml-delivery/src/manifest.rs:90、:320；native gate 测试仍用 `WXML_TEST_NODE` 作为负向 guard，见  crates/wxml-lsp/tests/native_parity/native_gates.rs:911、:1306。应改为“无 Node 执行路径；schema v1 只读识别与负向 guard 保留”，并把 `NodeBackend` 列为显式保留项。
- **N-02（HostOnly）**： final-docs 写“真实安装下启动 `wxml-lsp --stdio`；再移除 engine 复跑”，见  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:44；post-delete revalidation 没有运行“移除 engine”的真实复跑（同文件:355-356 列未运行项），只有 `cli.rs` 的 HostOnly 单测。应补真实复跑日志，或改为“单测覆盖 HostOnly；真实 remove-engine 未重跑”。

#### M-07 F-02 的 2592/0/73 只有自报表格，且只在未提交工作树成立

- **声明**： final-docs 写 F-02 PASS、2592/0/73，见  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:41；F-02 证据表格写 `cargo test --workspace --all-features` 0、2592/0/73，见  docs/evidence/2026-11-20-tsgo-f02-debug-perf-fix.md:59。
- **问题**：没有原始 stdout/log 路径；同表的 `--doc` 命令没有对应结果；`73 ignored` 的括注写成“73 个测试二进制/文档测试结果行”，这不是 cargo 的 ignored 计数含义；仓库实际有 119 个 `#[ignore]` 属性、ledger 的 `ignored_rust_tests` 只有 23 个旧 gate，不能仅凭一句“均为既有 opt-in native gate”证明 73 的组成。workspace 命令是 debug 构建，已不执行 200 ms 延迟断言；release 断言在单独的 `type_assets` 命令（同文件:60），不能把“workspace 0 failed”理解为 200 ms 预算在 workspace 内被强制执行。还有一个数字缺口：tag 的 2587 passed + 1 failed = 2588 个总用例，修复后却报 2592 passed（+4），单个 perf 测试修复不能解释；F-02 又说只改了 `type_assets.rs`，因此 2592 很可能包含其它未提交测试增加，需要逐项 delta 或 clean 重跑。
- **判定**：F-02 的修复设计与 release 断言有证据；但 2592/0/73 的原始输出、`--doc` 和 ignored 组成证据不足，且结论只对未提交工作树成立。
- **修正**：补 `cargo test --workspace --all-features` 原始日志、`cargo test --workspace --doc` 结果和 73 个 ignored 的清单；在 final-docs 明确“debug workspace 不含延迟门禁；release type_assets 才执行 200 ms”。

### 3.3 LOW

#### L-01 N-01 的命令/包路径与 clean 状态记录不一致

- **声明**： final-docs N-01 命令写 `scripts/native-gates.sh --package "$HOME/.local/share/wxml-lsp/versions/<version>"`，见  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:43。
- **实际**：`/tmp/opencode/native-gates/20261120T012446Z/summary.txt` 记录的 package 是 `/tmp/opencode/p7-final-install/versions/0.1.0`；head 是 `8ec0427`；没有记录 `git_dirty`/`WXML_STABILITY_ALLOW_DIRTY`，也没有保存 stdout 的最终 `NATIVE_GATES: PASS 40/40` 行。原始 40/40 覆盖运行明确使用 `WXML_STABILITY_ALLOW_DIRTY=1`，见  docs/evidence/2026-11-20-tsgo-p7-native-gate-coverage.md:64-67；final-docs 的“clean 复跑”没有对应的 clean 记录。
- **判定**：40/40 的组成和逐 gate 日志一致；但“clean”与命令路径不能由该 summary 直接证明。改为实际 package 路径，补 `git status`/dirty 标记和完整 stdout。

#### L-02 F-01/F-03 只有自报，没有原始日志

- **声明**： final-docs F-01 写 HEAD `8ec0427`、fmt/clippy 均 exit 0；F-03 写“修复私有 intra-doc link 后 `DOC_EXIT=0`”，见  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:40、:42。
- **问题**：两行都没有日志/产物路径，未满足 final-docs 自己“每项记录命令、日期、退出码、日志/产物路径”的规则。F-03 也没有记录 rustdoc 的原始输出。建议补 `/tmp` 日志或改为“self-reported”。

#### L-03 证据指针/措辞的小问题

- **40/40 指针错误**： docs/evidence/README.md:232 把 native gates 40/40 的证据指向 `2026-11-20-tsgo-p7-release-validation.md`；该文件明确写“不代替 native gates”，见  docs/evidence/2026-11-20-tsgo-p7-release-validation.md:18-19。应指向 `2026-11-20-tsgo-p7-native-gate-coverage.md` 或 `/tmp/opencode/native-gates/20261120T012446Z/summary.txt`。
- **“没有任何 BLOCKED”措辞**： post-delete revalidation 摘要写“没有任何 BLOCKED 项”，同文件:347-348 又写 F3-02 仍为 BLOCKED；应改为“本次 release/install/LSP/Zed 范围无 BLOCKED；F3-02 installer scope 仍为 residual”。
- **最终清单缺行**：final-docs 的验收表没有 F3、ledger、security/exec、stability 行（ docs/evidence/2026-11-20-tsgo-p7-final-docs.md:38-53），但 §0/ADR 把 F3、gates、合理行为测试列为验收口径。建议加行或明确“由合同/独立审计覆盖”，否则“F-01～D-03 全部通过”会被误读为覆盖了这些项。
- **“未改动测试”**：final-docs §1 写未改测试，见  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:16；但工作树的 `type_assets.rs`、`q2_stability.rs`、`asset_loader.rs`、`f1_hover.rs`、`support.rs` 都是未提交修改。应写“本 doc slice 未改；最终验收依赖未提交测试改动”。

## 4. 按任务检查项逐条回答

### 4.1 native gates 40/40 的组成与 clean 运行记录

- 组成 **23 + 3 + 9 + 1 + 1 + 3 = 40** 正确；`scripts/native-gates.sh:54-61` 与 `/tmp/opencode/native-gates/20261120T012446Z/summary.txt` 的六行 gate 结果一致；日志计数分别为 23、3、9、1、1、3。
- `20261120T012446Z` 的 head 是 `8ec0427`，不是 tag `b6c1ced`；二者只差 final-docs 4 行，代码等价。summary 的 package 是 `/tmp/opencode/p7-final-install/versions/0.1.0`，不是 final-docs 写的 `$HOME/...`。summary 没有 dirty/clean 标记，无法证明“clean 复跑”；原始覆盖运行使用了 `WXML_STABILITY_ALLOW_DIRTY=1`。结论：**组成一致，clean 声明证据不足**。

### 4.2 F3 16/0/0、ledger 109/0/130/29/71、workspace 2592/0/73、release SHA/大小、Zed 18/18、ptrace 13、security 0 HIGH

- **F3 16/0/0**：有合同与证据索引（ docs/reference/tsgo-delivery-contract.md:3、:12、:1398； docs/evidence/README.md:168、:199、:231）。边界：F3-09 是删除前历史验收，当前 HEAD 不能重跑；F3-02 仅 Linux x86_64 + ptrace；F3 合同不等于 G7 parity。
- **ledger 109/0/130/29/71**：JSON 精确一致（109+130+29+71=339）；closeout ledger 记录逐组依据；未被写成 green。
- **workspace 2592/0/73**：有 F-02 证据表格（ docs/evidence/2026-11-20-tsgo-f02-debug-perf-fix.md:59），但**无原始日志**；tag `b6c1ced` 的已提交 final-docs/ADR/计划仍是 **2587/1/73**（`git show HEAD:...:41`、`:12`、`:927`）。结论：**worktree-only，不是 tag 事实**。
- **release SHA/大小**：post-delete 是 `7a51a3e9…`/15,117,270 B，实测 A/B/C 相同；迁移计划 §12.16 仍写删除前 `b138d119…`/15,117,268 B（M-04）。
- **Zed 18/18**：实际日志 **16** 条 CHECK PASS；18/18 只在 post-delete 与证据索引，16/16 在迁移计划与日志（M-01）。截图 9 张不是 10 张。
- **ptrace 13 exec**：证据一致（10 installer + 3 LSP，13 条 EXEC，0 forbidden）。
- **security 0 HIGH**：安全审计的漏洞/unsafe/exec 范围为 0 HIGH/CRITICAL；但发布许可文本另有 P1 HIGH/BLOCKED（H-03）。不要把“cargo deny licenses ok”当成“随包许可完整”。

### 4.3 PARTIAL/BLOCKED 是否被误写成 PASS；stability 4/1

- **R-03**：PASS 但“删除后未重跑” → 应为 PARTIAL 或 pre-delete-only（M-05）。
- **E-01/E-02/N-02**：PASS 范围过宽（M-06）。
- **stability**：证据 4 implemented/1 blocked；README/ADR/plan/index/retarget/coverage 仍写 5 blocked；final-docs 只写“stability BLOCKED”，没有 4/1（M-02）。
- **security**：security audit 自己明确把内核沙箱、`openat` TOCTOU、非 Linux、F3-02 `connect` 审计列为 BLOCKED（ docs/evidence/2026-11-20-tsgo-p7-final-security-audit.md:26-28、:351-352），没有误写成 PASS；但 final-docs 的残余风险只列“stability、G7、跨平台”，没有列这些安全 BLOCKED，属于**漏报**而非误写。
- **D-02/D-03**：正确使用 PASS-with-followup，未误写成完全 PASS；当前工作树的链接检查确实只剩 vendored fixture 1 个 broken。但修复未提交，tag HEAD 仍有 18 个 broken（ repo audit）。

### 4.4 最终 tag/commit/HEAD

- HEAD 与 tag 都是 `b6c1ced`，这一点一致。
- 但“删除提交 `b6c1ced`”错误；实际删除是 `c4ddf32`（M-03）。
- tag 内 final-docs 仍是 F-02 PARTIAL/2587/1；tag 内 README/ADR/计划也仍是 2587/1；tag 内证据索引仍说“实际删除提交尚未全部落地 / Node 未删除”（`git show HEAD:docs/evidence/README.md:231、:236-237`）。因此 **tag 内容与 final-docs 的“已验收”不一致**（H-01）。
- 当前工作树的 2592/0/73、F-02 PASS、stability 4/1、D-02/D-03 修复、post-delete/exec/security 证据都在未提交区；不能把 tag 当作这些结论的复现点。

## 5. 建议修正（按优先级）

1. **先决定验收对象**：如果最终交付是 tag，提交 F-02/stability/文档/证据改动并重新打 tag；如果最终交付是工作树，把 tag 标记为“非最终”或改成 `p7-removed-worktree`，避免双口径。
2. **把 final-docs 的 2592/0/73 与 tag 对齐**：要么提交 `type_assets.rs` 修复并重跑，要么把 F-02 行改回 PARTIAL/2587/1；不要同时存在“tag 2587/1”和“final 2592/0/73”。
3. **处理 functional P1/P2**：修复 JSON didOpen 丢弃脚本首次诊断；修复/明确 Page-only `documentSymbol`。重建 final install 后重跑功能冒烟；在 final-docs 记录结果。
4. **处理 license P1**：补 Rust/Zed 依赖许可文本并重新打包，或书面接受内部使用 waiver，保持 B7/NF-09 打开；final-docs/README/ADR 明确“对外发布 BLOCKED / 内部可用”。
5. **修正数字**：Zed 18/18 → 16/16（或补 2 项日志）；截图 10 → 9（或补图）；stability 5 blocked → 4 implemented/1 blocked；§12.16 archive → 7a51a3e9/15,117,270；删除提交 → c4ddf32。
6. **收紧 PASS**：R-03 → PARTIAL/pre-delete-only；E-01 → LSP/Zed 进程树无 Node；E-02 → 无 Node 执行路径（保留 schema v1 `NodeBackend`）；N-02 → 补 remove-engine 真实日志或标 PARTIAL。
7. **补证据**：F-02 原始日志、`--doc` 结果、73 ignored 清单；F-01/F-03 日志；N-01 实际 package/clean 标记；40/40 证据索引指向 gate coverage 而不是 release validation。
8. **收尾文档**：final-docs 加 F3/ledger/security/exec/stability 行或明确外部覆盖；把“没有任何 BLOCKED”改成范围化表述；把“未改动测试”改成“doc slice 未改，验收依赖未提交测试改动”。

## 6. 并发新增证据（不在原始来源清单，但影响结论）

| 文档 | 关键结论 | 对 final-docs 的影响 |
| --- | --- | --- |
|  docs/evidence/2026-11-20-tsgo-p7-final-functional-smoke.md | final install 36 条操作：32 PASS / 1 FAIL / 3 INFO；**P1** JSON didOpen 静默丢弃 TS 首次诊断（20 s 不重试）；**P2** Page-only `documentSymbol` `-32803`；二进制 SHA 与 post-delete 安装一致（同文件:45） | “已验收/合理行为测试通过”不成立；至少 PARTIAL，P1 应先修 |
|  docs/evidence/2026-11-20-tsgo-p7-release-license-audit.md | 归档机械完整性/上游许可/无 Node PASS；`THIRD_PARTY_LICENSES.md` 不含 Rust/Zed 依赖许可 → **对外发布 BLOCKED，P1 HIGH**（同文件:14、:18-19、:412、:468-478） | “release 5/5/已验收”只适用于内部安装；对外发布需补许可或显式 waiver |

> 这两份文档在本次审计期间出现且未被 final-docs/证据索引收录；若它们属于最终交付，必须先被主线程吸收并更新验收状态。若不属于，final-docs 应明确“功能级/许可级审计不在本次验收范围”，否则读者会误以为已覆盖。

## 7. 审计限制

- 本审计未重跑 `cargo test --workspace`、native gates、release build、Zed smoke 或功能冒烟；所有“通过/失败”均引用现有证据文件/日志/产物。未重跑意味着 2592、40/40、Zed 等只做了证据一致性核对，不替代真实执行。
- 工作树在审计期间持续变化（final-docs 10:07、functional smoke 10:18、license audit 10:14 等）；行号与 `git status` 以 2026-11-20T02:15Z 快照为准。并发修改可能使部分行号在提交时移动；审计期间还出现未跟踪的 `crates/wxml-delivery/src/licenses.rs`，可能是许可修复的并发工作，本审计未评估其完成度。
- 未修改代码、测试、README、ledger、脚本或其他文档；本文件是唯一写入。
