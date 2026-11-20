# P7 F3 历史引用核对与 scope waiver（2026-11-20）

## 0. 结论先行

1. **F3-09 的 v1 迁移验收是历史证据**：完成于 `p7-pre-delete` tag
   （`4dd62b1`）之前，真实 schema v1 包从 `e4fed4a` 的 clean `git archive`
   构建，记录在
   [`2026-11-20-tsgo-f3-legacy-real.md`](2026-11-20-tsgo-f3-legacy-real.md)。
2. **P7 删除已落地**：`scripts/f3-legacy-real.sh`、
   `crates/wxml-delivery/tests/f3_legacy_real.rs`、`backends/typescript/**`、
   `crates/wxml-lsp/src/backend_node/**` 和 v1 写路径均已不在当前工作树。
   **不得声称 F3-09 的 v1 验收仍可在当前 HEAD 重跑。**
3. **删除后仅保留 schema v1 只读识别**：`current`/`verify`/`env`/`list` 的
   legacy 只读投影、`rollback` 拒绝，以及 `verify_installed`/
   `read_legacy_manifest` 边界；不再有 v1 安装/升级/打包写路径。
4. **F3-02 的辅助 guard 仍存在但语义已反转**：
   `crates/wxml-delivery/tests/no_node_guard.rs` 与
   `scripts/verify-no-node.sh` 现在要求 `main.rs` 不得出现
   `NodeBackend::new`/`serve_node()`；`verify-no-node.sh --legacy` 仍只做
   schema v1 只读检查。`scripts/native-gates.sh` 仍存在，但本合同 F3 行
   未引用它。
5. **F3-06 不受影响**：其证据链（`scripts/build-release.sh`、
   `crates/wxml-delivery/tests/f3_kill_window.rs`、
   `crates/wxml-delivery/tests/f3_acceptance/transactions.rs`）在当前工作树
   仍存在，且未引用已删除的 v1/Node 文件。
6. 本次**只改文档**：更新 ` docs/reference/tsgo-delivery-contract.md` 的
   F3-09/F3-02 相关段落并新增本文件；未改生产代码、测试、脚本、ledger、
   `docs/evidence/README.md` 或根 `README.md`，未运行测试，未 commit，
   未启动 review。

---

## 1. 身份与边界

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `3e24c44`（`test(p7): retarget perf and stability gates to native`） |
| P7 删除基线 tag | `p7-pre-delete` → `4dd62b1`（`test(p7): record pre-delete tag and remove stale gates`） |
| F3-09 legacy 包源码 | `e4fed4a` clean `git archive`（历史脚本 `--source-rev e4fed4a`） |
| F3-09 历史包身份 | archive `wxml-lsp-0.1.0-linux-x86_64.tar.gz`，SHA-256 `b62d3112…d970ae`，333 文件 |
| 平台边界 | 仅 Linux x86_64 有证据；其它平台不声明 |
| 文档切面 | ` docs/reference/tsgo-delivery-contract.md`、本文件 |
| 未运行 | `cargo test`、`cargo clippy`、`cargo fmt`、任何 F3 脚本；本文件结论来自只读文件检查 |

工作树中 P7 删除补丁尚未提交（`git status` 显示删除/修改），本文件的
“当前工作树”指该未提交状态，不是 `p7-pre-delete` 或某个新 tag。

---

## 2. 修改项

| 位置 | 改动 | 目的 |
| --- | --- | --- |
| ` docs/reference/tsgo-delivery-contract.md` §9.2 导语 | 真实 legacy 包证据改为“**历史证据**”，并写明 P7 删除后测试/脚本已移除、当前 HEAD 不可重跑 | 避免读者把历史验收当成可执行测试路径 |
| 同文件 §9.2 F3-02 行 | 给 `no_node_guard.rs`/`verify-no-node.sh` 加“P7 后为‘无 Node 后端’静态 guard”注 | 引用仍有效，但语义已随 P7 反转 |
| 同文件 §9.2 F3-09 行 | 从“未来测试路径”移除 `f3_legacy_real.rs`，改为“曾由……覆盖（P7 已删除，仅见历史证据）”；现有覆盖列指向历史证据与 schema v1 只读识别 | 不把已删除测试留在可执行路径中 |
| 同文件 §9.4 2026-11-20 历史条目 | 明确验收完成于 `p7-pre-delete`（`4dd62b1`）之前；补 P7 删除清单与“当前 HEAD 不能重跑” | 保留历史事实，同时记录删除边界 |
| 同文件 §9.5 F3-09 行 | 结果列改为 **closed（历史验收；P7 删除后不可重跑）**；关键证据改指历史证据文档；仍缺列增加 P7 scope waiver | 保持 F3-09 的历史 closed 结论，不伪造可重跑性 |
| 同文件 §9.5 结尾段落 | `f3-legacy-real.sh`/`f3_legacy_real.rs` 改为过去时并标注“二者已在 P7 删除”；补链接到本文件 | 与 §9.4/§9.5 行一致 |
| 同文件 §9.5 新增“P7 删除后的引用核对”段 | 汇总 F3-09 历史引用、F3-02 保留 guard、`native-gates.sh` 未引用、F3-06 证据链未受影响 | 给评审一个单点核对入口 |

未修改 `docs/evidence/2026-11-20-tsgo-f3-legacy-real.md` 与
`docs/evidence/2026-11-20-tsgo-f3-06-real-n2.md`：它们是历史证据，
原文事实保持不动；P7 后的状态只写在本文件与合同注记中。

---

## 3. 保留项

以下路径在当前工作树仍存在，未因 F3-09 的 v1 删除而失效：

| 路径 | 状态 | 说明 |
| --- | --- | --- |
| ` crates/wxml-delivery/tests/f3_kill_window.rs` + `tests/f3_kill_window/**` | 存在（P7 补丁有修改） | F3-06/F3-09 的 switch 前/后进程级 SIGKILL 窗口；legacy fixture 支撑 L3/L4 |
| ` crates/wxml-delivery/tests/native_rollback.rs` | 存在 | F3-09 legacy → native 回滚/前滚/恢复窗口（fixture） |
| ` crates/wxml-delivery/tests/native_delivery.rs` | 存在 | `legacy_migration_succeeds_once_and_keeps_legacy_read_only` 等迁移/只读断言 |
| ` crates/wxml-delivery/tests/f3_exec_audit.rs` | 存在（P7 补丁有修改） | F3-02 的 ptrace execve 关闭证据；不依赖 legacy guard |
| ` crates/wxml-delivery/tests/f3_acceptance/legacy.rs` | 存在 | F3-08/F3-15 的 legacy 只读、`env`、rollback 拒绝 |
| ` crates/wxml-delivery/tests/f3_acceptance/cache.rs` | 存在 | F3-12 的缓存升级/回滚/fail-closed |
| ` crates/wxml-delivery/tests/f3_acceptance/transactions.rs` | 存在 | F3-06/F3-16 的四窗口与 journal 恢复 |
| ` crates/wxml-delivery/tests/no_node_guard.rs` | 存在（P7 补丁有修改） | 静态 guard；现在要求无 `NodeBackend::new`/`serve_node()` |
| ` scripts/verify-no-node.sh` | 存在（P7 补丁有修改） | PATH shim + 安装器检查；`--legacy` 仅 schema v1 只读；LSP 部分为静态检查 |
| ` scripts/native-gates.sh` | 存在 | 35 个 native gate runner；本合同 F3 行未引用，只在迁移计划/ADR 中出现 |
| ` scripts/build-release.sh` | 存在（P7 补丁有修改） | F3-06 的 `--version` 真实 N1/N2 构建入口 |
| ` scripts/f3-kill-window.sh`、`scripts/f3-exec-audit.sh`、`scripts/f3-zed-smoke.sh` | 存在 | F3-06/F3-09、F3-02、F3-14 的历史/现行 opt-in 入口 |
| ` docs/evidence/2026-11-20-tsgo-f3-legacy-real.md`、`2026-11-20-tsgo-f3-06-real-n2.md`、`2026-11-20-tsgo-f3-kill-window.md` | 存在 | 历史证据原文，未修改 |

保留项的共同边界：它们是**当前工作树**的文件状态；本文件不声称这些
opt-in gate 在本次文档改动中已经运行或通过。

---

## 4. 不可重跑项（scope waiver）

| 项 | 当前状态 | waiver / 残余边界 |
| --- | --- | --- |
| ` scripts/f3-legacy-real.sh` | **已删除**（P7） | 历史 v1 包的构建/验收脚本；当前 HEAD 无此入口，不能重跑 |
| ` crates/wxml-delivery/tests/f3_legacy_real.rs` | **已删除**（P7） | 真实 v1 包安装/迁移/失败/中断/回滚/恢复测试；当前 HEAD 无此测试 |
| ` backends/typescript/**` | **已删除**（P7） | 历史 v1 包的 Node 后端构建输入；包内不再有 `dist/`、`server.js` 或 `node_modules` |
| ` crates/wxml-lsp/src/backend_node/**` | **已删除**（P7） | `--backend node` 的进程/协议实现已移除；legacy Node 入口不能执行 |
| v1 packager/installer 写路径 | **已删除**（P7） | 不再能打包/安装/升级/回滚 schema v1 包；只保留只读识别 |
| `bin/wxml-lsp --stdio --backend node` LSP initialize | **不可执行** | 依赖已删除的 Node 后端与包内 `server.js`；历史证据中的 1,262 B initialize 响应不能复现 |
| 历史 archive `b62d3112…d970ae` | **不在仓库** | 只记录在证据中；当前 HEAD 无法重新生成同一包 |

**可重跑性的准确说法**：

- 当前 HEAD **没有** F3-09 v1 验收的可执行路径；不得把历史证据写成
  “仍可运行/仍可通过”。
- `p7-pre-delete` tag（`4dd62b1`）可作为历史回滚点；从该 tag 恢复脚本/测试
  属于**历史复原**，不是当前 HEAD 的验收，也不改变本文件的 waiver。
- 即使从 tag 恢复，仍需外部 Node/npm、`e4fed4a` clean archive、Zed wasm
  和 pinned 资产；这不是默认 CI 路径。

**删除后保留的 schema v1 行为**：

| 行为 | 保留形式 |
| --- | --- |
| `current` / `verify` / `env` / `list` | 读取 schema v1 manifest 的只读投影或字节哈希；不执行 Node/`server.js` |
| `rollback --to <legacy>` | 明确拒绝（`legacy Node package`），不改变 current |
| 安装树识别 | `install.rs::verify_installed`、`manifest_v2.rs::read_legacy_manifest` 边界 |

---

## 5. F3-02 / F3-06 核对

### 5.1 F3-02

- `crates/wxml-delivery/tests/no_node_guard.rs`：仍存在；P7 补丁删除了
  “`serve_node()` 必须保留 `NodeBackend`”的旧断言，改为要求
  `main.rs` 不含 `serve_node()`/`NodeBackend::new`。它是源码静态检查，
  **不是** F3-02 的关闭证据。
- `scripts/verify-no-node.sh`：仍存在；`--legacy` 继续校验 schema v1
  只读命令；LSP 部分同样是静态检查，并保留 “execve/strace 未执行”的
  residual 行。
- F3-02 的关闭证据仍是 `crates/wxml-delivery/tests/f3_exec_audit.rs` +
  `tests/f3_exec_audit/**` + `scripts/f3-exec-audit.sh` 的 ptrace execve
  审计；本合同未把上述辅助 guard 当作关闭依据。
- `scripts/native-gates.sh`：仍存在，但本合同 F3 行没有引用；不需要为
  F3-09 删除改它的文档引用。

### 5.2 F3-06

- 证据文档
  [`2026-11-20-tsgo-f3-06-real-n2.md`](2026-11-20-tsgo-f3-06-real-n2.md)
  引用 `scripts/build-release.sh --version`、
  `crates/wxml-delivery/tests/f3_kill_window.rs`、
  `crates/wxml-delivery/tests/f3_acceptance/transactions.rs` 和真实 N1/N2
  包；这些路径/入口在当前工作树仍存在。
- 只读检查未在该证据文档中发现对 `backends/typescript`、`backend_node`、
  `f3_legacy_real`、`f3-legacy-real.sh`、`no_node_guard` 或
  `verify-no-node` 的引用，因此 F3-06 的文档口径不需要因 P7 删除而改写。
- F3-06 的 closed 结论与 F3-09 的 v1 scope waiver 相互独立；本文件不把
  F3-09 的历史限制外推到 F3-06。

---

## 6. 其他观察（本次未修改）

当前 P7 工作树还删除了 `crates/wxml-delivery/tests/packager_cli.rs`、
`crates/wxml-delivery/tests/installer_flow.rs`、
`crates/wxml-delivery/tests/installer_security.rs`。合同 §9.2 的
F3-03/F3-04/F3-10/F3-15/F3-16 行仍在“未来测试路径”或“现有覆盖”列
引用这些文件名。它们不在本次 F3-09/F3-02/F3-06 的修改范围内，故未改；
建议主线程在 P7 文档收口时统一处理（改成历史证据或现存替代测试）。

`crates/wxml-delivery/tests/provisioning.rs` 在合同中标为 `new`，当前也不
存在；它与上述删除项不同，属于未实现的计划路径。

---

## 7. 验证

| 检查 | 结果 |
| --- | --- |
| 合同内 `f3_legacy_real`/`f3-legacy-real` 引用 | 全部带“已删除/历史证据/不可重跑”限定 |
| 合同内 `no_node_guard`/`verify-no-node` 引用 | 文件仍存在；§9.2 F3-02 与 §9.5 注记已标明 P7 语义 |
| 合同内 `native-gates.sh` 引用 | 无（仅迁移计划/ADR 引用，未改） |
| 本文件相对链接 | `2026-11-20-tsgo-f3-legacy-real.md`、`2026-11-20-tsgo-f3-06-real-n2.md`、`2026-11-20-tsgo-f3-kill-window.md` 均存在 |
| 合同 §9.2/§9.5 表格 | 5 列 / 4 列保持不变，行数未增删 |
| `git diff --check` | 通过（本文件与合同改动无空白错误） |

未执行：任何 cargo 命令、任何 F3 脚本、任何 Node/npm 命令、任何
ledger/README 更新。验证仅为文档层面的链接/表格/空白检查。

---

## 8. 明确不声称

- 不声称 F3-09 的 v1 验收可在当前 HEAD 重跑或仍然通过。
- 不声称 `p7-pre-delete` 之后仍保留 v1 打包/安装/升级/回滚写路径。
- 不声称 `no_node_guard.rs`/`verify-no-node.sh` 是 F3-02 的关闭证据；
  关闭证据仍是 `f3_exec_audit` 的 ptrace execve 审计。
- 不声称 `scripts/native-gates.sh` 已在本轮运行，或它的 35 gate 计数
  已覆盖所有 retarget。
- 不声称 F3-06 因 F3-09 的 waiver 而受影响；F3-06 仍以其真实 N1/N2
  与 SIGKILL 证据为准。
- 不声称本文件更新了 `docs/evidence/README.md` 或根 `README.md`；索引
  由主线程统一处理。
- 不声称本次改动已通过 cargo 测试、clippy 或 fmt；未运行这些命令。
