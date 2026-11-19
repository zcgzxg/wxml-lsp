# F3-06/F3-09 切换后崩溃语义裁决（2026-11-20）

状态：**已裁决（docs-only）**。本文件只记录裁决与证据对照，不修改生产代码、测试、Cargo、
默认后端或 Node 链路。裁决基线 HEAD `b095258`；执行期间并发会话推进到 `899a89a`，但
`crates/wxml-delivery/src` 与 Cargo 清单未变。裁决针对 `28c5e18` 的进程级 SIGKILL 证据暴露的
合同冲突：§6.3“switch 后、state 写入前”原第 975 行要求“state 仍指 previous 则切回”，而
`recover` 在 `current` 已指向 target 时实际前滚。

## 0. 裁决摘要

| 窗口 | 失败/崩溃时的磁盘状态 | 裁决 | 禁止 |
| --- | --- | --- | --- |
| pre-switch（`current` 的 symlink/`rename`/`fsync` 前，含 tree rename 前后） | `current` 仍为 legacy 或 native previous；target 只在 `tmp/`（或已 rename 到 `versions/` 但未 switch）；journal `staged`/`committed` | `current` 与旧树**字节不变**；不自动激活；已有 state 字节不变；`staged` 删除 tmp，`committed` 保留 target 等显式命令 | 不新建/改写 state、不执行 Node、不修改旧树 |
| post-switch，legacy 迁移 | `current` 已指向已验证 native target；`state.json` 缺失或仍是迁移前状态 | **roll-forward**：重新校验 target，写 `state=active:target/previous:null`，清 journal | 不切回 legacy/Node；不把 legacy 记为 `previous`；不执行旧入口 |
| post-switch，native `N1→N2`，进程崩溃/`recover` | `current=N2`；`state` 仍为 `N1` 或缺失；journal `committed`/`activated` | **roll-forward**：重新校验 N2，写 `state=active:N2/previous:N1`，清 journal | 不切回 N1；不再写一次 `current` |
| post-switch，native `N1→N2`，进程存活且事务内失败 | `current=N2`；`state` 仍为 `N1` | 可切回**已验证 native v2** `N1`（不重写 state）；state 已指 N2 则前滚 | 不把 legacy 当 previous；不在 state 已写 N2 后切回 |
| post-switch，`state` 已写 target | `current=N2`、`state=N2` | 幂等确认或前滚；不改变 active | 不切回、不重写 state |

一句话：pre-switch 失败**什么都不动**；post-switch 失败以 `current` 为准**前滚到已验证 native
target**；只有进程仍存活、且 `state` 仍指向已验证 native v2 previous 时，才允许切回该 previous。
legacy/Node 永远不在可恢复候选内。

## 1. 实现与证据对照

### 1.1 原子切换

`InstallRoot::switch`（ `crates/wxml-delivery/src/install.rs:372` ）的顺序是：

1. 校验 `versions/<version>` 是真实目录；
2. 创建 `.current.tmp-<pid>-<nanos>` symlink；
3. `rename(temp, current)`；失败只删除 temp，`current` 不变；
4. `fsync(root)`；失败返回 `SwitchedButNotDurable`，但 `current` 已指向 target。

因此“switch 前”与“switch 后”的边界是第 3 步 `rename` 的退出点；第 4 步失败已经是 post-switch。

### 1.2 `recover` 只向 target 前滚，不切回 previous

`recover_committed`（ `crates/wxml-delivery/src/native_install.rs:1182` ）：

- `current == journal.target` → `complete_current_target`
  （ `crates/wxml-delivery/src/native_install.rs:1314` ）→ 重新校验 target，必要时写 state，
  清 journal；
- `current != target`，但 `state.active == target` 且 current 是另一个已验证 native →
  重新校验 current/target 后 `switch(target)`（向前修复 current/state 分裂）；
- 其余（target 存在但 current/state 不指向它）→ 保留 target、`not auto-activated`；
  不删除、不切换。

`complete_current_target` 的写 state 路径是 `write_state_for_current_target`
（ `crates/wxml-delivery/src/native_install.rs:1332` ）：整树 fingerprint、manifest/资产校验、
引擎探针、二进制探针、二次 fingerprint，然后 `State::write`、journal 删除。target 校验失败时
保留 journal 并报错，不会静默切到别的版本。

`reconcile_current_state`（ `crates/wxml-delivery/src/native_install.rs:1416` ）在 journal 处理
之后检查 current/state 一致性；native current 无 state 且没有可用 journal 修复时会报
`InstallState`，不会自动创建 state 或切换。

### 1.3 进程存活的失败处理

`handle_post_switch_failure`（ `crates/wxml-delivery/src/native_install.rs:1568` ）的分支：

- `outcome == Migrated`（legacy）：只调用 `roll_forward_after_switch`
  （ `crates/wxml-delivery/src/native_install.rs:1659` ），`previous=null`，永不切回 legacy；
- 首次安装且无 previous：I/O 错误返回 typed `SwitchedButNotDurable`，其他中断保留 committed
  journal 等显式恢复；
- native upgrade/rollback：`current` 不是 target 时直接返回 typed 错误；`state.active == target`
  时前滚；`state.active == previous` 时调用
  `restore_previous`（ `crates/wxml-delivery/src/native_install.rs:1700` ），重新校验 previous
  后 `switch(previous)`，且不重写 state；previous 校验失败时 `current` 停在 target；切回失败时
  按实际 `current` 返回 typed `SwitchedButNotDurable`。

这里的关键区别：`restore_previous` 只在**进程仍存活**时执行；进程被 SIGKILL 后只能由
`recover` 按磁盘状态恢复，而 `recover` 的规则是前滚。

### 1.4 测试与实测窗口

| 证据 | 覆盖 |
| --- | --- |
| ` crates/wxml-delivery/tests/f3_kill_window/real.rs:318` `f3_06_real_cli_sigkill_upgrade_before_switch` | U3：`symlink` entry，pre-switch；杀后 `current=N1`、state 不变 |
| ` crates/wxml-delivery/tests/f3_kill_window/real.rs:427` `f3_06_real_cli_sigkill_upgrade_after_tree_rename` | U1：tree rename exit，pre-switch；`recover` 标 committed 但不激活 |
| ` crates/wxml-delivery/tests/f3_kill_window/real.rs:519` `f3_06_real_cli_sigkill_upgrade_after_switch` | U5：`rename(current)` exit，post-switch/state 未写；`recover` 前滚到 N2 |
| ` crates/wxml-delivery/tests/f3_kill_window/real.rs:609` `f3_06_real_cli_sigkill_upgrade_after_durable_switch` | U5-durable：switch 内 `fsync(root)` exit；同 U5 前滚 |
| ` crates/wxml-delivery/tests/f3_kill_window/real.rs:697` `f3_06_real_cli_sigkill_upgrade_after_state_write` | U6：state 已写；幂等确认、不重写 state |
| ` crates/wxml-delivery/tests/f3_kill_window/real.rs:842` `f3_09_real_cli_sigkill_legacy_before_switch` | L3：legacy pre-switch；旧 current/树不变、无 state |
| ` crates/wxml-delivery/tests/f3_kill_window/real.rs:942` `f3_09_real_cli_sigkill_legacy_after_switch` | L4：legacy post-switch；`recover` 前滚、`previous=null` |
| ` crates/wxml-delivery/tests/f3_kill_window/real.rs:1028` `f3_06_real_cli_kill_control_never_matches` | 控制项：不杀时完整升级成功，证明 gate 非空 |
| ` crates/wxml-delivery/tests/native_rollback.rs:529` `post_switch_failures_roll_forward_legacy_migration` | 库级：`AfterSwitch`/`BeforeStateWrite`/`AfterStateWrite` 全部前滚，legacy 不变 |
| ` crates/wxml-delivery/tests/native_rollback.rs:606` `recovery_rolls_forward_a_committed_legacy_target_idempotently` | 库级：legacy current=N1、无 state，recover 两次均前滚 |
| ` crates/wxml-delivery/tests/native_rollback.rs:785` `native_after_state_write_rolls_forward_without_switching_back` | 库级：state 已写 N2 后不切回 N1 |
| ` crates/wxml-delivery/tests/native_rollback.rs:842` `recovery_repairs_current_previous_state_target_mismatch_idempotently` | 库级：current=N1、state=N2、journal committed，recover 前滚到 N2 |
| ` crates/wxml-delivery/tests/native_rollback.rs:893` `restore_previous_verify_failure_is_typed_and_fails_closed` | 库级：previous 损坏时 fail closed，current 停在 target |
| ` crates/wxml-delivery/tests/f3_acceptance/transactions.rs:45` | 库级 `InstallHooks` 四窗口；`AfterSwitch` 覆盖进程存活的“切回 N1”分支 |

真实运行的杀前/杀后状态、`recover` 输出、fingerprint 和 syscall 计数见
[`2026-11-20-tsgo-f3-kill-window.md`](2026-11-20-tsgo-f3-kill-window.md) §5。

## 2. 为什么前滚更安全

1. **`current` 是进程可见的已提交事实，`state.json` 是派生元数据**：`switch` 的 `rename`
   成功后，`current` 已指向完成 staged 校验/探针的 target；state 只记录 active/previous/
   lastTransaction，可以从已验证 target + journal 重建。前滚保留事实、修复元数据；切回则相反。
   本裁决只覆盖进程崩溃；断电/目录 fsync 持久性仍不在承诺内。
2. **避免第二个切换窗口**：崩溃后的切回需要再写一次 `current`，又引入 symlink/rename/fsync
   的二次崩溃窗口；前滚只写 state，不再改 `current`。
3. **legacy 没有合法 previous**：legacy 是 schema v1，没有 v2 manifest/资产身份；把它写成
   `previous` 或切回 legacy 会违反 I4/I5/I8，并可能执行旧 Node 入口。前滚是唯一能保持
   `previous=null` 且不执行旧入口的路径。
4. **失败关闭**：前滚前重新校验 target（整树 SHA-256、manifest、资产、引擎/二进制探针、二次
   fingerprint）；target 不一致时保留 journal 并报错，不会为了“有 current”而切到未验证版本。
5. **幂等**：重复 `recover` 在 state 已有效时只清 journal，不重写 state；不使用“上次操作是否
   成功”这类进程内状态。
6. **不制造 current/state 永久分裂**：要么 state 被补成 target，要么 typed 错误要求人工
   `recover`；不会静默把 current 改回 previous。

## 3. 合同修改

| 位置 | 修改 |
| --- | --- |
| §5.8 第 7 步后的失败说明（ `docs/reference/tsgo-delivery-contract.md:910` ） | 明确进程崩溃且 current 已指 target 时前滚；只有进程存活且 state 指 native previous 时才可切回 |
| §5.9 switch 后失败（ `docs/reference/tsgo-delivery-contract.md:922` ） | 同上，并保留“legacy 只前滚、永不恢复 Node” |
| §6.2 `S2` switch 后失败行（ `docs/reference/tsgo-delivery-contract.md:957` ） | 区分崩溃前滚与存活切回；不再写成“state 仍指 N1 就切回” |
| §6.3 第 975/976 行（ `docs/reference/tsgo-delivery-contract.md:977` ） | 拆成“进程存活”和“进程被杀/崩溃”两行；崩溃行只前滚，legacy 永不切回；新增前滚更安全说明与 `committed` 的 `current==target` 例外 |
| §6.4 I2/I3（ `docs/reference/tsgo-delivery-contract.md:1002` ） | 补充崩溃恢复的前滚分支；切回仍限已验证 native v2 previous |
| §6.6 legacy 专项（ `docs/reference/tsgo-delivery-contract.md:1040` ） | switch 后失败明确“含进程崩溃”，只前滚 |
| §9.2 F3-06/F3-09 行（ `docs/reference/tsgo-delivery-contract.md:1240`、`docs/reference/tsgo-delivery-contract.md:1243` ） | F3-06 验收语义改为“崩溃前滚、存活才切回”；两行补 `f3_kill_window` 覆盖 |
| §9.5 F3-06/F3-09 行（ `docs/reference/tsgo-delivery-contract.md:1346`、`docs/reference/tsgo-delivery-contract.md:1349` ） | 状态仍为 **partial**；进程级 SIGKILL 子条件标为已覆盖，剩余缺口分别归到 N2 provenance / 真实 legacy 发布包 |

原 §6.3 第 975 行的字面规则现在只属于“进程存活，事务内失败”行；进程崩溃行明确要求前滚。
这与 `recover_committed`、`handle_post_switch_failure`（Migrated 分支）和 I4/I5 一致。

## 4. F3-06/F3-09 验收状态

`28c5e18` 的 `f3_kill_window` 是 opt-in、Linux x86_64 的真实 CLI SIGKILL 证据；它补齐的是
“switch 前/后进程级中断”子条件，不改变下面两个 provenance 缺口。

| ID | 状态 | `28c5e18` 已覆盖 | 仍缺（归到本 ID 的子条件） |
| --- | --- | --- | --- |
| F3-06 | **partial** | U3（switch 前）、U1（tree rename 后、switch 前）、U5（switch 后、state 未写）、U5-durable（switch fsync 后）、U6（state 已写）；U5/U5-durable 实测前滚补 state，U6 幂等确认，U1/U3 不自动激活；never-match 控制项；库级 `AfterSwitch` 覆盖存活切回 | **第二个真实 release**：N2 是 N1 生产二进制的同长度版本常量重标，不是独立发布产物；kill 测试只证明崩溃恢复语义，不证明 N2 provenance |
| F3-09 | **partial** | L3（legacy switch 前）、L4（switch 后、state 未写）；旧树 fingerprint 不变、legacy marker 未执行、`previous=null`、`recover` 前滚 | **真实 legacy 发布包**：legacy current 由生产 v1 packager 生成（`0.0.9`，带 Node entry tripwire），不是历史发布包；L3/L4 不证明 legacy 包身份 |

附加窗口（不属于本行合同子条件，未计入关闭）：F3-06 的 U0/U2/U4/U7/U8，F3-09 的
L0/L1/L2/L5/L6。它们属于 kill-window plan 的 P1/P2；其中 U4 的孤儿 `.current.tmp-*` 是卫生
缺口，不是 current/state 正确性缺口。不得因这些窗口未跑而把两行从 partial 改 blocked，也不得
因 U3/U5/L3/L4 通过而把两行写成 closed。

## 5. 剩余 BLOCKED

| 项 | 状态 | 退出条件 |
| --- | --- | --- |
| F3-06 第二个真实 release | **BLOCKED** | 产出与 N1 内容不同的真实 N2 发布产物并重跑 N1→N2 崩溃窗口；或评审书面 waiver（本文件不代替 waiver） |
| F3-09 真实 legacy 发布包 | **BLOCKED** | 取得历史 legacy 发布包（schema v1，真实入口）并重跑 L3/L4；或评审书面 waiver |
| 断电/目录 fsync 持久性 | **未证明** | 需要掉电/故障注入，不是进程 SIGKILL；合同 §10 已排除 |
| 非 Linux x86_64 | **BLOCKED** | ptrace 寄存器/路径解析需移植；无证据时保持 `BLOCKED` |
| 孤儿 `.current.tmp-*`/`.*.tmp-*` | **卫生缺口** | 后续在持锁 `recover`/`switch` 清理已结束事务的临时文件；不影响本次语义裁决 |

## 6. 复现

```bash
# 非真实 fixture/preflight（默认）
cargo test -p wxml-delivery --test f3_kill_window -- --nocapture

# 真实 CLI SIGKILL（需要 pinned assets + ptrace）
scripts/f3-kill-window.sh --skip-build --network-isolate
```

失败关闭：`WXML_REQUIRE_KILL_WINDOW=1`、`WXML_REQUIRE_REAL_ENGINE=1` 或
`WXML_REQUIRE_EXEC_AUDIT=1` 时，ptrace 不可用或真实资产缺失按失败处理；`SKIP`/`BLOCKED` 不得
写成 `PASS`。

## 7. 参考

- 合同：` docs/reference/tsgo-delivery-contract.md:945`（§6.2）、
  ` docs/reference/tsgo-delivery-contract.md:969`（§6.3）、
  ` docs/reference/tsgo-delivery-contract.md:999`（§6.4）、
  ` docs/reference/tsgo-delivery-contract.md:1040`（§6.6）、
  ` docs/reference/tsgo-delivery-contract.md:1323`（§9.5）。
- 执行证据：` docs/evidence/2026-11-20-tsgo-f3-kill-window.md` §5/§6；
  设计：` docs/evidence/2026-11-20-tsgo-f3-kill-window-plan.md` §10.1。
- 切换：` crates/wxml-delivery/src/install.rs:372`；
  恢复：` crates/wxml-delivery/src/native_install.rs:1182`、
  ` crates/wxml-delivery/src/native_install.rs:1314`、
  ` crates/wxml-delivery/src/native_install.rs:1332`、
  ` crates/wxml-delivery/src/native_install.rs:1568`、
  ` crates/wxml-delivery/src/native_install.rs:1659`、
  ` crates/wxml-delivery/src/native_install.rs:1700`。
- 测试：` crates/wxml-delivery/tests/f3_kill_window/real.rs:318`、
  ` crates/wxml-delivery/tests/f3_kill_window/real.rs:427`、
  ` crates/wxml-delivery/tests/f3_kill_window/real.rs:519`、
  ` crates/wxml-delivery/tests/f3_kill_window/real.rs:609`、
  ` crates/wxml-delivery/tests/f3_kill_window/real.rs:697`、
  ` crates/wxml-delivery/tests/f3_kill_window/real.rs:842`、
  ` crates/wxml-delivery/tests/f3_kill_window/real.rs:942`、
  ` crates/wxml-delivery/tests/f3_kill_window/real.rs:1028`、
  ` crates/wxml-delivery/tests/native_rollback.rs:529`、
  ` crates/wxml-delivery/tests/native_rollback.rs:606`、
  ` crates/wxml-delivery/tests/native_rollback.rs:785`、
  ` crates/wxml-delivery/tests/native_rollback.rs:842`、
  ` crates/wxml-delivery/tests/native_rollback.rs:893`、
  ` crates/wxml-delivery/tests/f3_acceptance/transactions.rs:45`。
