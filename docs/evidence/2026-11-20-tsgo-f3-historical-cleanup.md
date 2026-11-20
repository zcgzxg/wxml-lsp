# F3 历史引用清理（2026-11-20）

## 0. 结论先行

1. ` docs/reference/tsgo-delivery-contract.md` §9.2 中 F3-03/F3-04/F3-06/F3-10/
   F3-11/F3-15/F3-16 行引用的 `packager_cli.rs`、`installer_flow.rs`、
   `installer_security.rs` 均为 **P7 删除前的历史证据**（tag `p7-pre-delete`
   / commit `4dd62b1`），当前 HEAD 已无这些文件。
2. 每处历史引用都保留原测试名/结论，并加注“历史证据（P7 删除前，tag
   `p7-pre-delete`/commit `4dd62b1`）”；其中 v1 覆盖明确标为
   **historical-only**。
3. 当前存在等价 native/归档证据的行已补充当前路径；没有等价证据的
   （`provisioning.rs` 等未采用计划名）明确标注“计划名未采用/当前无此文件”。
4. **F3 结论不变**：仍为 **16 closed / 0 partial / 0 blocked**；未新增
   acceptance、未改 ledger、未改任何代码/测试/脚本。
5. 本次只改两个既有文档并新增本记录：合同 §9.2 导语与 7 行、§9.4 一条
   2026-11-19 历史条目，以及 ` docs/evidence/README.md` 的两条索引行。

---

## 1. 身份与边界

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `b6c1ced`（`docs(p7): record clean native gate rerun`） |
| P7 删除前基线 | tag `p7-pre-delete` → `4dd62b1`（`test(p7): record pre-delete tag and remove stale gates`） |
| P7 删除提交 | `c4ddf32`（`feat(p7): remove node backend and legacy typescript tree`） |
| 文档切面 | ` docs/reference/tsgo-delivery-contract.md`、` docs/evidence/README.md`、本文件 |
| F3 状态 | 16 closed / 0 partial / 0 blocked（未改） |
| 未运行 | `cargo test`、`cargo clippy`、`cargo fmt`、任何 F3 脚本、任何 Node/npm 命令 |

工作树另有本次任务开始前已存在的 ` crates/wxml-backend-tsgo/tests/type_assets.rs`
修改和未跟踪的 ` docs/evidence/2026-11-20-tsgo-f02-debug-perf-fix.md`
（`git status` 可见）；本任务未 touch 这些文件，也未把它们计入本次改动。

---

## 2. 被清理的引用

### 2.1 已删除的 v1 文件

| 文件 | `p7-pre-delete`（`4dd62b1`）时 | 当前 HEAD | 处理 |
| --- | --- | --- | --- |
| ` crates/wxml-delivery/tests/packager_cli.rs` | 存在，v1 打包 CLI 测试 | 已删除（`c4ddf32`） | F3-03 现有覆盖标为历史证据；补 `native_package.rs` 与 `f3_real_release/package.rs` 作为当前等价 |
| ` crates/wxml-delivery/tests/installer_flow.rs` | 存在，v1 安装/升级/回滚/`WXML_LSP_BIN`/并发用例 | 已删除（`c4ddf32`） | F3-04/F3-06/F3-15/F3-16 标为历史证据；逐行补当前 native 等价 |
| ` crates/wxml-delivery/tests/installer_security.rs` | 存在，v1 归档哈希/PAX/下载拒绝用例 | 已删除（`c4ddf32`） | F3-10/F3-11 标为历史证据；补 `f3_acceptance/package_guards.rs` 与 `f3_real_release/runtime.rs` |
| ` crates/wxml-delivery/tests/f3_legacy_real.rs` | 存在，真实 v1 包验收 | 已删除（`c4ddf32`） | 之前已在 F3-09 行/§9.4/§9.5 标为历史证据；本次未重复改写 |

`git log --all -- crates/wxml-delivery/tests/packager_cli.rs`、
`git log --all -- crates/wxml-delivery/tests/installer_flow.rs` 均显示最后
一次修改为 `c4ddf32`（删除），确认它们不再存在于当前工作树。

### 2.2 未采用的计划名

以下名称在 `p7-pre-delete` 时也不存在，属于 2026-11-19 表格里的计划名，
不是被 P7 删除的当前证据：

| 名称 | 出现位置 | 当前处置 |
| --- | --- | --- |
| ` crates/wxml-delivery/tests/provisioning.rs` | §9.2 F3-10“未来测试路径” | 标注“计划名未采用，当前无此文件” |
| `archive_rejects_traversal_links_and_budget` | §9.2 F3-11 | 标注“计划名，未采用”；当前等价 `f3_11_...` |
| `crash_recovery_and_lock` | §9.2 F3-16 | 标注“计划名，未采用”；当前等价 `f3_acceptance/transactions.rs` 的 `f3_16_...` |
| `installs_native_package_atomically` | §9.2 F3-04“未来测试路径” | 保留为历史计划名；当前等价为 `native_delivery.rs` 的 `fresh_install_is_atomic_and_writes_state_and_no_journal`（已写入“现有覆盖”列） |

§9.2 导语新增一句总说明：这两列保留 2026-11-19 的计划与历史口径，
未逐一标注的早期 v1 用例名（`offline_rollback...`、`offline_install...` 等）
同属 P7 删除前的历史覆盖，当前关闭证据以 §9.5 为准。

---

## 3. 逐行改动

| 位置 | 原引用 | 现表述（摘要） | 当前等价证据（存在性已核对） |
| --- | --- | --- | --- |
| §9.2 F3-03 | `packager_cli.rs` 仅 v1 | 保留“仅 v1”，加历史证据 tag/commit；该 v1 覆盖 historical-only | ` crates/wxml-delivery/tests/native_package.rs`；§9.5 的 ` crates/wxml-delivery/tests/f3_real_release/package.rs` |
| §9.2 F3-04 | `installer_flow.rs` 为 v1/Node | 保留原结论，加历史证据；historical-only | ` crates/wxml-delivery/tests/native_delivery.rs` 的 `fresh_install_is_atomic_and_writes_state_and_no_journal`；` crates/wxml-delivery/tests/f3_real_release/runtime.rs` |
| §9.2 F3-06 | `installer_flow.rs` 部分 v1 | 保留原结论，加历史证据 | 同一 `f3_acceptance/transactions.rs` 四窗口；`f3_kill_window.rs` 的 U1/U3/U5/U5-durable/U6 |
| §9.2 F3-10 | `installer_security.rs`（extend）+ `provisioning.rs`（new） | 前者历史证据/historical-only；后者计划名未采用 | `f3_acceptance/package_guards.rs` 的 `f3_10_...`；`f3_real_release/runtime.rs` 的 `tampered_real_release_archive_fails_before_activation` |
| §9.2 F3-10 | `rejects_a_bad_archive_hash` 等仅 v1 | 保留原结论，补原文件已删除 | 同上 |
| §9.2 F3-11 | `installer_security.rs`（extend）`archive_rejects_traversal_links_and_budget` | 文件历史证据/historical-only；测试名为未采用计划名 | `f3_acceptance/package_guards.rs` 的 `f3_11_...` |
| §9.2 F3-15 | `installer_flow.rs`（extend）+ `editors/zed/src/lib.rs`（extend） | 前者历史证据/historical-only；Zed 测试标“当前存在” | `f3_acceptance/legacy.rs` 的 `f3_15_explicit_binary_precedence_for_install_upgrade_rollback`；`native_cli.rs`；` editors/zed/src/lib.rs` |
| §9.2 F3-15 | `explicit_binary_wins...` 仅部分 | 保留原结论，补原文件已删除 | 同上 |
| §9.2 F3-16 | `installer_flow.rs`（extend）+ `native_delivery.rs`（new）`crash_recovery_and_lock` | 前者历史证据/historical-only；后者计划名未采用 | `native_delivery.rs` 的 `recovery_of_a_committed_upgrade_does_not_auto_activate_it` 等 journal 用例；`f3_acceptance/transactions.rs` 的 `f3_16_...` |
| §9.2 F3-16 | `concurrent_installs...` 仅 v1 | 保留原结论，补原文件已删除 | 同上 |
| §9.4 2026-11-19 历史条目 | `installer_flow.rs` 的 legacy poison-env 用例 | 加“历史证据：P7 删除前……文件已删除” | 该段本身是历史快照，无当前等价断言 |

F3-05/F3-07/F3-12 的“现有覆盖”列也含 v1 用例名（`failed_runtime_check...`、
`offline_rollback...`、`offline_install...`）。本次未逐行改写，由 §9.2 导语
统一说明它们同属 P7 删除前的历史覆盖，避免把表格改得不可读。

---

## 4. 验证

| 检查 | 命令/方法 | 结果 |
| --- | --- | --- |
| 已删除全路径引用 | `rg -n -e "crates/wxml-delivery/tests/packager_cli\.rs" -e "crates/wxml-delivery/tests/installer_flow\.rs" -e "crates/wxml-delivery/tests/installer_security\.rs" -e "crates/wxml-delivery/tests/provisioning\.rs" docs/reference/tsgo-delivery-contract.md` | 命中 F3-10/F3-11/F3-15/F3-16 四行，以及 F3-09/§9.4/§9.5 的历史段落；每处均带“历史证据/P7 删除前/tag `p7-pre-delete`/commit `4dd62b1`/historical-only”或“历史验收/不可重跑”限定 |
| 裸文件名引用 | 同文件 `rg -n -o -e "packager_cli\.rs" -e "installer_flow\.rs" -e "installer_security\.rs"` | 10 行、14 处：导语 3、F3-03 1、F3-04 1、F3-06 1、F3-10 2、F3-11 1、F3-15 2、F3-16 2、§9.4 1；均带历史/删除说明，无“当前存在”断言 |
| 当前等价路径存在 | 逐个 `test -e` | `native_package.rs`、`native_delivery.rs`、`f3_acceptance/package_guards.rs`、`f3_acceptance/legacy.rs`、`f3_acceptance/transactions.rs`、`f3_real_release/package.rs`、`f3_real_release/runtime.rs`、`f3_kill_window.rs`、`native_cli.rs`、`editors/zed/src/lib.rs` 全部存在 |
| F3 状态未变 | `rg -c "16 closed / 0 partial / 0 blocked"` | 合同 HEAD 4 次 = 工作树 4 次；README HEAD 2 次 = 工作树 2 次 |
| §9.2 表格列数 | `awk` 统计管道符 | 1241–1258 每行 6 个竖线（5 列），行数未增删 |
| 相对链接 | Python 正则抽取两文档的相对链接并 `os.path.exists` | 111 条，全部可解析（含本文件） |
| 空白错误 | `git diff --check` | 通过 |
| 改动面 | `git status --short` / `git diff --stat` | 仅 `docs/reference/tsgo-delivery-contract.md`、`docs/evidence/README.md` 与本文件；另有两个任务前已存在的条目（` type_assets.rs` 修改、` f02-debug-perf-fix.md` 未跟踪），未 touch |

---

## 5. 明确不声称

- 不声称本次重新执行或重新通过了任何 F3 测试/脚本；当前等价路径只做了
  存在性核对。
- 不声称 `packager_cli.rs`、`installer_flow.rs`、`installer_security.rs` 的
  历史结论仍可在当前 HEAD 复现；它们是 `p7-pre-delete`（`4dd62b1`）前的
  历史证据。
- 不声称 `provisioning.rs`、`archive_rejects_traversal_links_and_budget`、
  `crash_recovery_and_lock`、`installs_native_package_atomically` 曾实现或
  已存在；它们是未采用的计划名。
- 不改变 F3 16/0/0、B1/B6 或任何 ledger 状态，不新增 acceptance。
- 不声称改动了 `README.md` 主文档、迁移计划、代码、测试、脚本或合同
  状态字段。
- 不声称已 commit 或已发起 review；本次只产生工作树文档改动。
