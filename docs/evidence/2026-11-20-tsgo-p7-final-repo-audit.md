# P7 删除后最终仓库/文档卫生审计（2026-11-20）

## 0. 结论先行

- **最高严重级别：HIGH**（对外权威状态与最终验收口径自相矛盾）。
- **是否阻塞最终交付：阻塞文档/验收口径签字，不阻塞代码构建、release、install 与 Zed smoke。** 若最终交付物包含 README、证据索引和合同/ADR 口径，必须先修 H-01～H-04。
- **硬条件 PASS**：`backend_node`/`backends/typescript` 运行树已删除；默认 `--stdio` 为 native，`--backend node` 被拒绝；生产路径无 Node 子进程；native gates **40/40**、F3 **16/0/0**、ledger **109/0/130/29/71** 均与脚本/JSON/合同一致。
- **主要残留**：README 仍写“P7 处于集成中”和“实际删除未完成”；证据索引同一节先写“删除已完成”再写“实际删除尚未全部落地 / 不声称 Node 已移除”； crates/wxml-lsp/README.md 整篇仍把已删除的 Node 后端当作当前用法；5 个 ADR 与 2 个合同仍写“P7 集成中”；P7 删除导致 18 个活跃文档相对链接 broken；Zed 仍转发两个已死的旧调优变量。
- 发现计数：**HIGH 4 / MEDIUM 8 / LOW 8**。

## 1. 审计身份与边界

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| 分支 | `tsgo/migration` |
| HEAD | `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b`（`b6c1ced`） |
| Tag | `p7-removed` 指向 HEAD |
| 审计时间（UTC） | 2026-11-20T01:45Z～01:49Z |
| 模式 | 只读；只写本文件；未改代码/测试/README/ledger/脚本；未 commit；未重跑测试 |
| 方法 | `git grep`/`git show HEAD:`/`git ls-files`、文件系统扫描、Python 解析 ledger JSON、Markdown 相对链接检查 |

**并发边界**：审计期间工作树持续变化。01:45Z 快照为 3 modified（`type_assets.rs`、`docs/evidence/README.md`、`docs/reference/tsgo-delivery-contract.md`）＋ 3 untracked；01:48Z 又出现 `crates/wxml-lsp/tests/q2_stability.rs` 修改、`q2_stability_support/` 与 `docs/evidence/2026-11-20-tsgo-p7-post-delete-revalidation.md`。这些是其他会话的工作，**未删除、未修改**，也不计入本审计的仓库残留。并发期间 `backends/typescript/` 空目录被外部清理，本审计未执行任何清理动作。

## 2. PASS 项

### 2.1 删除面与运行路径

| ID | 结论 | 证据 |
| --- | --- | --- |
| P-01 | `crates/wxml-lsp/src/backend_node/` 不存在；`git ls-files backends` 为 0；`backends/typescript/**` 无 tracked 文件 | `git ls-files backends`、`ls crates/wxml-lsp/src` |
| P-02 | `--backend node` 被显式拒绝，错误文案为“node backend was removed” |  crates/wxml-lsp/src/main.rs:137、 crates/wxml-lsp/tests/cli.rs:220 |
| P-03 | 默认 action 走 native（`Action::Default → serve_default → run_stdio_native_default`）；仅未安装引擎时 HostOnly |  crates/wxml-lsp/src/main.rs:151、 crates/wxml-lsp/src/main.rs:175 |
| P-04 | 生产源码无 Node 启动点；`Command::new` 仅指向引擎、安装器、探针和 shell harness | `git grep Command::new -- 'crates/**/src/**'` |
| P-05 | 构建/CI 无 npm/node 步骤；`build-release.sh` 与 CI 均显式拒绝 Node 工具链 |  scripts/build-release.sh:6、 scripts/build-release.sh:375、 .github/workflows/ci.yml |
| P-06 | v1 `NodeBackend`/`bin/wxml-type-backend/server.js` 只保留为 schema v1 只读识别、负向校验和测试 fixture；未发现 v1 写入口（`acquire.rs`/`cache.rs` 已不存在） |  crates/wxml-delivery/src/manifest.rs:320、 crates/wxml-delivery/src/manifest.rs:673、 crates/wxml-delivery/src/manifest_v2.rs |
| P-07 | 5 个 legacy Node 环境变量在 Zed 中不再用于选择后端（`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`/`WXML_NODE` 不转发） |  editors/zed/src/lib.rs:19、 editors/zed/src/lib.rs:705 |
| P-08 | ADR-0008 与 final-docs 已记录 Node removed / 删除完成 |  docs/adr/0008-rust-tsgo-no-node-backend.md:4、 docs/evidence/2026-11-20-tsgo-p7-final-docs.md:8 |

> P-06 为抽查：`NodeBackend` 仍作为 v1 只读 schema 的枚举和测试数据出现是 P7 明确保留项，不是 Node 运行路径；未逐条复核 15 个 v1 写符号的全部历史清单。

### 2.2 状态与计数

| ID | 结论 | 证据 |
| --- | --- | --- |
| P-09 | ledger 终态精确为 **109 green / 0 planned / 130 reviewed / 29 waived / 71 retired-with-decision**（339 cases） | Python 解析 `docs/plans/tsgo-test-ledger.json` 的 `counts.statuses`、`migration_evidence.statuses` 与 `cases` 状态计数，三者一致 |
| P-10 | native gates 为 **40/40**（23 engine + 3 edit + 9 stdio + 1 perf + 1 stability + 3 real_matrix） |  scripts/native-gates.sh:54、 .github/workflows/ci.yml（native-gates job 注释与步骤名） |
| P-11 | F3 为 **16 closed / 0 partial / 0 blocked** | `docs/reference/tsgo-delivery-contract.md` §9.5、  docs/evidence/README.md:191 |
| P-12 | 默认后端为 native、Node removed 的权威口径已存在 |  README.md:25、 README.md:630、 README.md:659、 docs/adr/0008-rust-tsgo-no-node-backend.md:4、 docs/adr/README.md:18、 docs/plans/2026-11-17-rust-tsgo-migration.md:917 |
| P-13 | “35/35” 仅出现在删除前基线/stdio 切片的**历史证据**中，且 ADR 处明确标注“删除前 native 基线” |  docs/adr/0008-rust-tsgo-no-node-backend.md:166、 docs/evidence/2026-11-20-tsgo-p7-predeletion-baseline.md:8、 docs/evidence/2026-11-20-tsgo-p7-stdio-retarget.md:13 |
| P-14 | 未发现活跃文档声称“Node default”；未发现 P7 专属的当前“待验证”状态（仅有泛化标题/模板） | `git grep` 全仓 |

> P-09～P-11 为交叉核对，**未重跑**测试或 gates；本审计不替代真实执行证据。

### 2.3 仓库卫生

| ID | 结论 | 证据 |
| --- | --- | --- |
| P-15 | 无 `*.orig`、`*.rej`、`*.tmp`、`*.bak`、`*~`、`.DS_Store`、`node_modules`、`dist`、`.env` 等意外临时/构建文件；唯二命中的 `*.log` 是 tracked 的证据数据 | `find` 扫描（排除 `target/`、`.git/`）；`git ls-files docs/evidence/data/existing-wxml-lsp-audit` |
| P-16 | 构建产物仅 `target/` 与 `editors/zed/target/`，均被 ignore |  .gitignore:1、 editors/zed/.gitignore:1 |
| P-17 | `git status --short` 仅包含并发工作，不是本轮 P7 残留 | 01:48Z 快照见 §1；未删除/未 stage |

## 3. 发现清单

### 3.1 HIGH

| ID | 位置 | 发现 | 影响 |
| --- | --- | --- | --- |
| H-01 |  README.md:556 | 活跃“实施计划”段写“P7 处于集成中：……实际删除与删除后复验尚未完成”，与同文件 README.md:25/630/659 的“P7 完成、删除已完成”直接矛盾 | 对外主 README 自相矛盾，读者会认为 P7 未交付 |
| H-02 |  README.md:666 | “当前状态/未完成/限制”仍列“P7 实际删除与删除后全量重验、2 个 silent return + 可选 tsc 分支”，但删除与重验已在 HEAD 完成（gates 40/40、final-docs 已验收） | 最终状态页仍把已完成项列为未完成 |
| H-03 |  docs/evidence/README.md:236、237、245 | 同一 §3.4 表格（`:230`）写“删除状态 已完成”，正文却写“实际删除提交尚未全部落地……live backends/typescript/test\|samples 与 v1 写路径仍可达”，末尾又写“不声称 Node 已移除或 P7……已完成” | 证据索引是文档权威入口；同节自相矛盾，并否定自己的删除完成结论 |
| H-04 |  crates/wxml-lsp/README.md:5、10-15、22-23、34-45、51-52、60、80-94 | crate README 整篇仍以当前时态描述已删除的 Node 后端：`backend_node`、`WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`、`backends/typescript/dist/server.js`、`WXML_TEST_NODE`、`real_node`/`stdio_type_loop` 命令 | 用户按该 README 操作会进入不存在的树和已删除的测试；与 native 默认事实冲突。P7 删除面审计（`docs/evidence/2026-11-20-tsgo-p7-deletion-surface.md:473`）已把该文件列为“删除实施时必须改”，但未改 |

### 3.2 MEDIUM

| ID | 位置 | 发现 | 影响 |
| --- | --- | --- | --- |
| M-01 |  README.md:73、94、518 | 仍用未来时/现在时：“P7 删除完成后……将移除”“属于 P7 删除范围，删除完成后移除”“Node 后端位于 backends/typescript/，属于 P7 删除范围” | 与“已删除”矛盾；`backends/typescript` 已不存在 |
| M-02 |  docs/adr/0001:7、 docs/adr/0002:7、 docs/adr/0003:7、 docs/adr/0006:9、 docs/adr/0007:9 | 5 个 ADR 的首段“2026-11-17 后续”横幅仍写“ADR-0008 accepted，native 默认；**P7 集成中**” | ADR 索引虽已写 removed，交叉引用仍指向进行中状态 |
| M-03 |  docs/reference/tsgo-native-contract.md:5、 docs/reference/tsgo-test-contract.md:3、 docs/reference/tsgo-delivery-contract.md:3、 docs/reference/tsgo-delivery-contract.md:15、 docs/reference/tsgo-delivery-contract.md:58、 docs/reference/tsgo-delivery-contract.md:94 | F0/F1 与 F3 仍写“P7 集成中”；F2 仍写“draft；P2 评审通过前不是最终门禁”“不表示任何用例已经迁移”；F3 头部仍写“draft、未接受、未实现”，但同文 §9.5 已 16/0/0 且 B1 closed | 规范合同自述状态与最终验收事实冲突；`docs/README.md:33` 又据此写“四者当前均为 draft” |
| M-04 |  docs/reference/type-bridge-c1.md:6、 docs/reference/type-bridge-c1.md:426-427、 docs/reference/wxml-language-feature-baseline.md:48、 docs/reference/wxml-language-feature-baseline.md:49、 docs/reference/wxml-language-feature-baseline.md:50、 docs/reference/wxml-language-feature-baseline.md:52、 docs/reference/wxml-language-feature-baseline.md:54、 docs/reference/wxml-language-feature-baseline.md:56、 docs/reference/wxml-language-feature-baseline.md:59-61、 docs/reference/wxml-language-feature-baseline.md:74、 docs/reference/wxml-language-feature-baseline.md:92、 docs/reference/wxml-language-feature-baseline.md:94-96、 docs/reference/type-diagnostics.md:132、 docs/reference/type-diagnostics.md:212-218、 tests/fixtures/README.md:230-234 | 18 个相对 Markdown 链接指向 P7 已删除的测试/旧 TS 树（`backends/typescript/README.md`、`real_node.rs`、`semantic_tools.rs`、`multi_app_projects.rs`、`q2_recovery.rs`、`q2_scheduler.rs`、`full_feature_matrix.rs`）；另有 3 处仍给 `(cd backends/typescript && npm ci ...)` 和 `wave5_acceptance` 死命令；`type-diagnostics.md:132` 仍说事件补全“通过 Node complete” | 活跃参考文档链接/命令不可用；`D-03` 的“相对链接全部存在”结论不成立 |
| M-05 |  editors/zed/src/lib.rs:23、 editors/zed/src/lib.rs:231、 editors/zed/src/lib.rs:689-698、 editors/zed/README.md:47-56、 editors/zed/README.md:59-60、 editors/zed/README.md:172-173、 crates/wxml-lsp/src/main.rs:38-41 | Zed 仍转发 `WXML_ANALYZE_TIMEOUT_MS` 与 `WXML_TEMPLATE_CHECK`，并为其写测试和 README 表；但 `wxml-lsp` 已在 usage 中声明这两个 Node 时代变量“were removed; setting them has no effect” | 用户设置超时/模板检查无效果；与 P7 删除面第 7 项“FORWARDED_ENV 两个已死变量；README”未完成一致 |
| M-06 |  crates/wxml-backend-tsgo/tests/asset_loader.rs:16、 crates/wxml-backend-tsgo/tests/asset_loader.rs:821、 crates/wxml-backend-tsgo/tests/asset_loader.rs:875、 crates/wxml-backend-tsgo/tests/asset_loader.rs:926、 crates/wxml-backend-tsgo/tests/type_assets.rs:12、 crates/wxml-backend-tsgo/tests/type_assets.rs:719、 crates/wxml-lsp/tests/native_lsp_document_symbols/support.rs:145、 crates/wxml-lsp/tests/native_lsp_document_symbols/support.rs:390、 crates/wxml-lsp/tests/native_lsp_document_symbols/support.rs:429、 crates/wxml-backend-tsgo/tests/f1_hover.rs:427 | native opt-in 测试仍把已删除的 `backends/typescript/node_modules` 当“repository typings fixture”并提示 `npm ci --prefix backends/typescript`；该树已无 `package.json`，且 `backends/typescript/node_modules` 实际**不被 .gitignore 忽略**（`git check-ignore` 返回未忽略） | 默认测试因缺失而 skip，官方 `native-gates.sh` 通过 `TSGO_TYPINGS_ROOT` 规避；但开发者按提示无法修复，且误以为该目录被 ignore |
| M-07 |  docs/evidence/2026-11-20-tsgo-p7-final-docs.md:50、 docs/evidence/2026-11-20-tsgo-p7-final-docs.md:52、 docs/evidence/2026-11-20-tsgo-p7-final-docs.md:53、 docs/evidence/2026-11-20-tsgo-p7-final-docs.md:58 | E-02“源码无 Node 边界”、D-02“README/计划口径”、D-03“相对链接全部存在”均标 PASS；但本审计发现活跃 README/参考文档/测试注释仍有相关引用，且有 18 个 broken link | 最终验收清单的 PASS 结论过宽，削弱 D-01～D-03 的可信度 |
| M-08 |  docs/README.md:21、 docs/README.md:33、 docs/README.md:35、 docs/README.md:46、 docs/evidence/2026-11-20-tsgo-p7-closeout-ledger.md:21、 docs/evidence/2026-11-20-tsgo-p7-closeout-ledger.md:207、 docs/evidence/2026-11-20-tsgo-p7-closeout-ledger.md:218 | 文档索引仍把 `docs/evidence/p7-integration.md`（Node 时代证据）列为“集成证据”且不标历史；合同仍“均为 draft”；README 把 closeout ledger 当当前依据，但该 ledger 明写“不声称 Node/旧 TS 已删除 / Node 未删除 / deletion_allowed 仍为 false” | 读者按阅读顺序会得到“Node 仍存在”的结论；历史证据缺少 superseded 标识 |

### 3.3 LOW

| ID | 位置 | 发现 | 影响 |
| --- | --- | --- | --- |
| L-01 |  crates/wxml-backend-tsgo/src/input.rs:64、 crates/wxml-backend-tsgo/src/input.rs:160、 crates/wxml-backend-tsgo/src/input.rs:398、 crates/wxml-lsp/src/project_host.rs:45、 crates/wxml-lsp/src/backend_tsgo/tests.rs:86、 crates/wxml-lsp/src/backend_tsgo/tests.rs:94、 crates/wxml-lsp/src/backend_tsgo/tests.rs:114、 crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:64、 crates/wxml-types/src/template.rs:77、 crates/wxml-types/src/template.rs:82、 crates/wxml-types/src/template.rs:87、 crates/wxml-types/src/template.rs:900、 crates/wxml-types/src/template.rs:917、 crates/wxml-lsp/tests/native_parity/native_gates.rs:13、 crates/wxml-lsp/tests/native_parity/symbols_edits.rs:8、 crates/wxml-lsp/tests/native_parity/symbols_edits.rs:17 | 注释仍把已删除的 `backends/typescript/**` 当作活文件来源，或说“old Node tests can only be removed together with backend_node” | 仅注释误导；不影响编译/运行 |
| L-02 |  docs/plans/tsgo-test-ledger.json:102-103、 docs/plans/tsgo-test-ledger.json:693 | `support_files` 与 `support_file` 仍登记 `backends/typescript/test/support.ts` | ledger 是历史迁移账本，删除后该锚点无法重算；建议标注 archived |
| L-03 |  docs/evidence/p7-integration.md:1-15 | 标题“P7 真实集成证据”与正文全是 Node/`NodeBackend` 时代事实，文件头未标“历史/已被 P7 删除取代”，而 `docs/README.md` 仍把它当 P7 集成入口 | 命名与当前 P7 删除语义冲突，易误读 |
| L-04 |  docs/plans/2026-11-16-zed-standard-acceptance.md:13、 docs/plans/2026-11-16-zed-standard-acceptance.md:57、 docs/plans/2026-11-16-zed-standard-acceptance.md:74-76、 docs/plans/2026-11-16-zed-standard-acceptance.md:85、 docs/plans/2026-11-16-zed-standard-acceptance.md:183、 docs/plans/2026-11-16-wave5-parallel-delivery.md:26、 docs/plans/2026-11-16-wave5-parallel-delivery.md:75、 docs/plans/2026-11-16-wave5-parallel-delivery.md:445 | 历史计划仍给完整 Node 验收步骤/旧 TS 构建命令，只标“已完成/待确认”，未标“Node 路径已删除” | 历史计划可保留原事实，但应加 superseded 横幅；目前容易被当作可执行说明 |
| L-05 |  仓库根 `backends/` 空目录 | 无文件、无 tracked 条目、不被 git 记录；审计开始时其下还有空 `backends/typescript/`，随后被并发会话清理 | 纯本地 FS 残留；不会进入提交，但会让 `ls` 误以为旧树仍在 |
| L-06 |  docs/README.md:66-84 | 目录结构仍只列 2026-11-16 两个 plan，不列 `2026-11-17-rust-tsgo-migration.md`、`2026-11-17-tsgo-test-ledger.md` 与 `tsgo-*.md` 合同 | 文档结构图过时，但不影响链接 |
| L-07 |  docs/evidence/README.md:90（并发工作树） | 新增索引行指向未跟踪的 `docs/evidence/2026-11-20-tsgo-f3-historical-cleanup.md`；另有未跟踪的 `2026-11-20-tsgo-p7-post-delete-revalidation.md` 尚未入索引 | 若只提交索引、不提交被链接文档，会新增 broken link；提交时必须成对 |
| L-08 |  README.md:25、 README.md:630、 README.md:659、 docs/adr/0008-rust-tsgo-no-node-backend.md:12、 docs/evidence/2026-11-20-tsgo-p7-final-docs.md:41 | 并发工作树正在修 `type_assets.rs` 的 debug perf 假失败；修复合入后“2587 passed / 1 failed”与 F-02 PARTIAL 将过期，需要同步更新这些计数 | 当前 HEAD 口径正确；合并并发修复后需联动更新，避免新 stale |

## 4. 关键词残留核对

| 关键词 | 结果 | 位置 |
| --- | --- | --- |
| “P7 集成中 / P7 处于集成中” | **FAIL** | README.md:556；docs/adr/0001:7；docs/adr/0002:7；docs/adr/0003:7；docs/adr/0006:9；docs/adr/0007:9；docs/reference/tsgo-native-contract.md:5；docs/reference/tsgo-delivery-contract.md:15、58、94 |
| “35/35” | **PASS（仅历史）** | 删除前基线：docs/evidence/2026-11-20-tsgo-p7-predeletion-baseline.md:8、129；stdio 切片：docs/evidence/2026-11-20-tsgo-p7-stdio-retarget.md:13、89、136；release 验证的“未验证”清单：docs/evidence/2026-11-20-tsgo-p7-release-validation.md:290；ADR-0008:166 明确标“删除前 native 基线”；证据索引行 85-86 明确标“P7 删除前” |
| “Node default / 默认 Node” | **PASS（无当前声明）** | 仅 ADR-0001/0003/0007 的“替代 Node 首选运行时”历史表述；无“Node 仍是默认” |
| “P7 删除未完成 / 未落地 / Node 未移除” | **FAIL** | README.md:666；docs/evidence/README.md:236-237、245；docs/evidence/2026-11-20-tsgo-p7-closeout-ledger.md:21、207、218；迁移计划 §12.14 的同类文字已明确标“历史，已被 §12.16 取代”（PASS） |
| “待验证” | **PASS（无 P7 当前状态）** | 仅 README.md:447 的泛化标题“推荐架构与待验证决策”、ADR 模板/维护规则等，不是 P7 当前状态 |

## 5. 相对 Markdown 链接检查

检查范围：`git ls-files '*.md'`，排除历史证据目录/文件与 `.opencode` 工具模板、vendored typings fixture。并发新增的 evidence 文档（F-02、F3 cleanup、post-delete revalidation 等）单独抽查，未发现 broken link。

| 源文件:行 | 目标 | 原因 |
| --- | --- | --- |
|  docs/reference/type-bridge-c1.md:6 | `../../backends/typescript/README.md` | P7 删除旧 TS 树 |
|  docs/reference/wxml-language-feature-baseline.md:48、 docs/reference/wxml-language-feature-baseline.md:49 | `../../crates/wxml-lsp/tests/full_feature_matrix.rs` | 测试已删除 |
|  docs/reference/wxml-language-feature-baseline.md:50、 docs/reference/wxml-language-feature-baseline.md:60 | `../../crates/wxml-lsp/tests/real_node.rs` | 测试已删除 |
|  docs/reference/wxml-language-feature-baseline.md:52、 docs/reference/wxml-language-feature-baseline.md:54、 docs/reference/wxml-language-feature-baseline.md:56、 docs/reference/wxml-language-feature-baseline.md:61、 docs/reference/wxml-language-feature-baseline.md:74 | `../../crates/wxml-lsp/tests/semantic_tools.rs` | 测试已删除 |
|  docs/reference/wxml-language-feature-baseline.md:59、 docs/reference/wxml-language-feature-baseline.md:61 | `../../crates/wxml-lsp/tests/multi_app_projects.rs` | 测试已删除 |
|  docs/reference/wxml-language-feature-baseline.md:61、 docs/reference/wxml-language-feature-baseline.md:94、 docs/reference/wxml-language-feature-baseline.md:95、 docs/reference/wxml-language-feature-baseline.md:96 | `../../crates/wxml-lsp/tests/q2_recovery.rs` | 测试已删除 |
|  docs/reference/wxml-language-feature-baseline.md:92、 docs/reference/wxml-language-feature-baseline.md:94 | `../../crates/wxml-lsp/tests/q2_scheduler.rs` | 测试已删除 |
|  tests/fixtures/README.md:230-234 | 非链接，但命令引用已删除的 `wave5_acceptance`/`backends/typescript` | P7 删除 |
|  docs/reference/type-diagnostics.md:212-218 | 非链接，但命令引用已删除的 `backends/typescript`/`real_node` | P7 删除 |
|  docs/reference/type-bridge-c1.md:426-427 | 非链接，但命令引用已删除的 `backends/typescript`/`wave5_acceptance` | P7 删除 |

统计：P7 直接造成的 broken 相对链接 **18 个**（1 + 17）；另有 3 处不可执行命令。

## 6. 交付判定

- **代码/构建/发布**：未发现阻塞。默认 native、无 Node 运行路径；`backends/typescript` 与 `backend_node` 均无 tracked 文件；CI/build 无 Node 步骤。
- **计数/门禁**：ledger、native gates、F3 三项与脚本/JSON/合同一致；未重跑，属于交叉核对。
- **文档验收**：**阻塞**。H-01～H-04 是权威状态自相矛盾；M-01～M-08 是活跃文档/配置/测试指引残留。修完 H 级并处理全部 M 级后，才可把 README 与证据索引作为最终交付口径。
- **历史证据**：`docs/evidence/**` 中的 35/35、Node 命令、删除前 baseline 不作为阻塞项；保留原事实即可，但应确保当前状态文档明确指向 §12.16 / final-docs。

## 7. 并发提醒

1. 工作树中的 `docs/reference/tsgo-delivery-contract.md` 正在补 F3 历史引用（P7 删除前的 `packager_cli.rs`/`installer_flow.rs`/`installer_security.rs`），本审计的 M-03/M-04 可能被该改动部分覆盖；引用行号以 HEAD 为准。
2. 工作树中的 `docs/evidence/README.md` 新增 `f3-historical-cleanup` 索引行，但该文档当时未跟踪；提交时必须连同文档一起提交，否则新增 broken link。
3. 工作树中的 F-02 修复会改变“2587 passed / 1 failed”和 F-02 PARTIAL 的结论；合并后需同步更新 README、ADR-0008、final-docs 与相关证据。
4. 本审计未执行删除、清理、reset、restore、stash 或 commit。
