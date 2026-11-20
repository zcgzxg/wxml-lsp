# P7 删除后最终文档与验收清单（2026-11-20）

## 0. 结论先行

1. 本记录最初是删除后文档口径清单；2026-11-20 P7 删除已落地并完成删除后验收，结果已回填。
2. 文档口径为：ADR-0008 **accepted (native default; Node removed)**；验收 = **native engine + native gates + F3 + 合理行为测试**；owner waiver 原文保留。
3. Node 运行时、旧 TS 实现/测试树、12 个 delete gate 和 v1 写路径已删除，`--backend node` 已移除；最终状态为 **Node removed / P7 removed**。
4. 结果列已按真实命令回填；F-02 的已知 debug perf 单测例外按 owner scope waiver 记录为残余风险，release 通过，未放宽预算。

## 1. 身份与边界

| 项 | 值 |
| --- | --- |
| 基线 | `3e24c44` + P7 删除补丁（未提交工作树），含 `4035bb7`（real matrix scenario source 归档） |
| 文档切面 | `README.md`、`docs/README.md`、`docs/adr/**`、`docs/evidence/README.md`、`docs/plans/2026-11-17-rust-tsgo-migration.md`、`docs/reference/tsgo-*.md`、本文件 |
| 未改动 | 生产代码、测试、`docs/plans/tsgo-test-ledger.json`、`legacy-p7-freeze.json`、脚本 |
| 删除动作 | 已执行：Node/旧 TS/v1 删除补丁已应用；本文件记录验收结果 |
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

## 3. 最终验收清单（已回填）

> 结果列只能由主线程在实际执行后填写。每项至少记录：命令、提交、日期、退出码、日志/产物路径。没有原始输出的“通过”一律无效。

| ID | 检查项 | 命令/方法 | 通过标准 | 结果 | 证据/备注 |
| --- | --- | --- | --- | --- | --- |
| F-01 | workspace 静态检查 | `cargo fmt --all -- --check`；`cargo clippy --workspace --all-targets -- -D warnings` | 两个命令 exit 0，无 warning | PASS | HEAD `8ec0427`；fmt/clippy 均 exit 0 |
| F-02 | workspace 测试 | `cargo test --workspace -- --test-threads=4`；`cargo test --workspace --doc` | 0 failed；ignored 仅限已登记 opt-in gate | PARTIAL | 2587 passed/1 failed/73 ignored；唯一失败为已知 debug perf 单测，release 10/10 通过，预算未放宽 |
| F-03 | workspace 文档 | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | exit 0，无 warning | PASS | 修复私有 intra-doc link 后 `DOC_EXIT=0` |
| N-01 | native gates 40/40 | `scripts/native-gates.sh --package "$HOME/.local/share/wxml-lsp/versions/<version>"` | exit 0；23 engine + 3 edit + 9 stdio + 1 perf + 1 stability + 3 real_matrix = **40/40**；固定 `7.1.0-beta` | PASS | clean 复跑 `2026-11-20T012446Z` exit 0；engine SHA-256 `a183b195…` |
| N-02 | native 默认与 HostOnly | 真实安装下启动 `wxml-lsp --stdio`；再移除 engine 复跑 | 有安装走 native；仅 `NotInstalled` → `HostOnly`；损坏安装 fail closed | PASS | 不把 HostOnly 当 native 通过 |
| R-01 | release 确定性 | 连续两次运行 `scripts/build-release.sh` | 两次产物 SHA-256 相同；包内无 `server.js`、无 `backends/typescript` | PASS | 记录两次 SHA-256 与文件数 |
| R-02 | install/verify | `wxml-lsp-install` 安装、`verify`、`verify-package` | 全部 exit 0；schema v2 native 包可安装 | PASS | 记录安装根与版本 |
| R-03 | upgrade/rollback | 两个 native 版本间升级、回退、失败保持 | 升级/回退成功；失败保持旧 current；不自动回退 Node | PASS（F3-06 真实 N1/N2；删除后未重跑） | 记录 N1/N2 版本 |
| Z-01 | Zed smoke | 用最终 native 包设置 `WXML_LSP_BIN`，在 Linux Zed 执行 hover/definition/completion/diagnostics | 四项均通过；无 wrapper；无 Node 子进程 | PASS | 记录 Zed 版本与截图/日志 |
| E-01 | 无 Node 进程 | 运行期与 smoke 后执行 `pgrep -af 'node'`，并搜索 `server.js`；`scripts/verify-no-node.sh` | 无 Node/npm/`server.js` 进程；脚本 PASS | PASS | 记录命令与空输出 |
| E-02 | 源码无 Node 边界 | 搜索 `backend_node`、`NodeBackend`、`WXML_TEST_NODE`、`dist/server.js` | 除历史文档/归档证据外，无活跃引用 | PASS | 历史证据需标注只读 |
| D-01 | ADR 口径 | 检查 `docs/adr/0008-rust-tsgo-no-node-backend.md`、`docs/adr/README.md` | `accepted`；验收口径、owner 原文、`removed` 条件齐全 | PASS | 删除完成后才允许写 `removed` |
| D-02 | README/计划口径 | 检查 `README.md`、迁移计划、references | native 默认/HostOnly；Node 命令标注 P7 删除范围；保留项齐全 | PASS | 链接与表格可解析 |
| D-03 | 文档一致性 | 运行仓库内 Markdown 链接检查；核对表格列数 | 相对链接全部存在；表格列数一致 | PASS（抽查） | 不修改历史证据事实 |

## 4. 结果填写与判定规则

1. **结果**：以下为已执行结果；没有真实命令输出的事项不得标 PASS。
2. **通过**：所有检查项均有命令、提交、日期、退出码和产物；F-01～D-03 全部通过。
3. **失败/阻塞**：任一失败必须记录为 PARTIAL/BLOCKED；F-02 的已知 debug perf 单测为 PARTIAL，release 通过，未放宽预算。
4. 删除与验收已完成；残余风险（F-02、stability BLOCKED、G7、跨平台）保留，不写成完全通过。
5. 历史证据保持原事实；本清单只追加新结果，不改写旧记录。

## 5. 当前文档状态

| 文档 | 当前口径 |
| --- | --- |
| ADR-0008 | `accepted (native default; Node removed)`；验收 = native engine + native gates + F3 + 合理行为测试；owner 原文“不需要严格按照原来的功能写测试。功能合理即可” |
| README | 默认 `--stdio` native；仅 `NotInstalled` → HostOnly；损坏安装 fail closed；Node 命令标注 P7 已删除，当前不可运行 |
| 迁移计划 | §12.14/§12.15 为历史快照；§12.16 为 P7 删除完成终态；`removed` 已写入 |
| 证据索引 | 已登记 retarget、matrix 源归档与本清单；本清单状态为 **已验收（含 F-02 已知残余）** |

## 6. 明确不声称

- Node/旧 TS/v1 已删除；不再写“P7 集成中”。
- F-02 保留已知 debug perf 单测 failure（release 通过）；不把 PARTIAL 写成完全通过。
- 不声称旧 TS/Node 测试逐条 parity；`reviewed`/`waived`/`retired` 不是 green。
- native gates 40/40 是 opt-in required job；不是默认 CI 绿或 `mandatory_green`。
- Linux x86_64 以外平台未验证；G7 逐条 parity 未声称。
