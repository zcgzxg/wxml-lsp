# F3-06/F3-09 生产 CLI 精确 SIGKILL 窗口设计（2026-11-20）

状态：**设计 only**。不改生产代码、不改现有测试、不改 Cargo/workspace、不 commit。
基线：worktree `tsgo-migration` HEAD `3220362`；`crates/wxml-delivery/src` 在该工作树无改动，
F3 交付链与合同审计基线（`561b00a`/`0b3b3da`/`0ca4375`）一致。

## 0. 结论（先看）

- **推荐方案 A**：test-only `ptrace` 在目标系统调用边界暂停真实 `wxml-lsp-install` 进程，父进程读取
  `current`/`state.json`/`txn/*.json` 确认窗口后发送**外部** `SIGKILL`。它是唯一同时满足
  “精确窗口 + 真实 CLI + 不碰生产代码 + 不新增依赖”的方案。
- **方案 B（轮询 + `SIGSTOP`）只能采样，不能关闭**：`committed` 到 `switch`、`switch` 到
  `state` 的窗口是亚毫秒到低毫秒级，轮询延迟同量级；停下来的位置可能是 `switch` 之后，
  必须“分类 + 重试”，无法证明杀在指定边界。
- **方案 C（生产 failpoint）不应作为关闭机制**：env-only 版本最终用户可触发（可制造挂起/中断），
  不满足“不可被最终用户触发”；feature-only 版本虽然默认不编译，但被测二进制不再是发布产物，
  且要改生产源码/可能新增依赖。只能作为未来跨平台的**辅助**证据，不能替代方案 A。
- **方案 A 真正关闭的是“生产 CLI 进程级精确 SIGKILL”这一子条件**（Linux x86_64）。F3-06/F3-09
  是否整体关闭还取决于三个独立决定：F3-06 的第二真实 release（当前 N2 是 N1 二进制同长度版本
  常量重标）、F3-09 的真实 legacy 发布包（当前是生产 v1 packager 生成的 fixture）、以及合同
  §6.3 第 975/976 行在“进程崩溃”场景下的语义（切回 vs 前滚）。后三项不是 kill 机制能解决的。
- **关键合同冲突**：`switch` 后、`state` 写入前被杀，磁盘状态是 `current=N2`、`state=N1`。
  `recover_committed` 看到 `current==target` 会**前滚到 N2**；而合同 §6.3 第 975 行字面要求
  “state 仍指 previous 则切回”。进程已被 SIGKILL，无法自己切回；这是语义裁决，不是测试缺口。
- 工作量估计：test/script/doc 约 1200–1700 行，无生产/Cargo 改动；P0 六个窗口约 3–5 分钟，
  加 P1 约 6–10 分钟（不含真实包构建）。

## 1. 现状与精确缺口

### 1.1 生产事务顺序

`install_native_locked`（ `crates/wxml-delivery/src/native_install.rs:432` ）在锁内完成：

1. `verify_tree_v2`、引擎探针、Rust 二进制 `--version` 探针，全部在 staged tree 上（`:675-738`）。
2. 写 `txn/<id>.json`，状态 `staged`；`fsync(tmp)`、`fsync(root)`（`:510-518`）。
3. `fsync_tree(staged)`（仅目标目录不存在时），`rename(staged, versions/<target>)`，
   `fsync(versions)`（`:528-542`）。
4. 写 journal `committed`（`mark_committed`，`:1480-1483`），随后 `AfterCommittedJournal`、
   `BeforeSwitch` checkpoint（`:548-550`）。
5. `root.switch(target)`：`symlink(".current.tmp-<pid>-<nanos>", "versions/<target>")`，
   `rename(temp, current)`，`fsync(root)`（ `crates/wxml-delivery/src/install.rs:372-413` ）。
6. `journal.transition(Activated)`（仅内存），`InstallState::new`，`state.write(root)`
   （`atomic_write` + `fsync(root)`， `crates/wxml-delivery/src/state.rs:348-355` ）（`:600-634`）。
7. 写 journal `activated`（`:647`），`AfterActivatedJournal`，删除 journal，清理 `tmp`
   （`:659-663`）。

`rollback_native_locked`（`:875`）没有第 3 步的 tree rename：journal `staged` → `committed` →
`switch` → `state` → journal `activated`（`:936-1045`）。

legacy 迁移走同一 `install_native_locked`，只是 `previous=null`、目标为 N1、`current` 为 schema v1
（`plan_operation` 的 `CurrentInstall::Legacy` 分支，`:753-755`）。

### 1.2 原子写的真实形态

- journal 和 `state.json` 都走 `atomic_write`
  （ `crates/wxml-delivery/src/manifest.rs:727-764` ）：
  临时名 `.<name>.tmp-<pid>-<nanos>`，`create_new` → `write_all` → `fsync` → `chmod` →
  `rename(temp, final)`；调用方再 `fsync_dir(parent)`。
- 因此“文件出现/重命名”的精确边界是**临时名 rename 到最终名的 syscall 退出点**，不是 `write`。
  `write` 只能证明临时文件开始写，最终名还不可见。
- `switch` 的临时 symlink 名是 `.current.tmp-<pid>-<nanos>`；`rename` 到 `current` 是原子的。
- 本机实测（见 §3.2）：当前 Rust 1.100 / glibc 2.43 下，`std::fs::rename` 发 **`rename(82)`**，
  `std::os::unix::fs::symlink` 发 **`symlink(88)`**，`File::sync_all` 发 **`fsync(74)`**；
  不是 `renameat2`/`symlinkat`。匹配器必须同时支持 `rename`/`renameat`/`renameat2` 和
  `symlink`/`symlinkat`，并在证据里打印实际 syscall 号。

### 1.3 现有证据与缺口

- F3-06 的 library seam：`f3_06_real_upgrade_and_rollback_failpoint_windows`
  （ `crates/wxml-delivery/tests/f3_acceptance/transactions.rs:45-199` ）通过
  `InstallHooks` 在 `BeforeSwitch`/`AfterSwitch` 注入错误；真实包、真实探针，但进程没有死。
- F3-09 的 seam：`f3_08_09_15_legacy_migration_inert_env_and_v1_rollback`
  （ `crates/wxml-delivery/tests/f3_acceptance/legacy.rs:28-437` ）在 `AfterRename`/`AfterSwitch`
  注入；迁移成功与失败路径都有，但同样不是 CLI 进程被外部杀死。
- F3-16 的 journal 注入、并发、损坏 active rollback 已 closed
  （ `crates/wxml-delivery/tests/f3_acceptance/transactions.rs:204-570` ）。
- F3-02 的 ptrace exec 审计已证明仓库可以在 test-only `extern "C"` 下使用 ptrace，
  无 Cargo 依赖（ `crates/wxml-delivery/tests/f3_exec_audit/tracer.rs:33-87` ）。
- 合同明确仍缺“生产 failpoint seam 下的精确进程级 SIGKILL”
  （ `docs/reference/tsgo-delivery-contract.md:1334`、`:1337` ）。

**精确缺口**：没有一个测试让**真实 `wxml-lsp-install` 进程**在 `current` 切换前/后、rollback
切换前/后、legacy 迁移切换前/后被**外部 SIGKILL**，并在不运行原进程的前提下用 `recover` 证明
恢复语义。现有 seam 是库函数在同一进程内返回错误，进程还活着，能执行 `restore_previous`；
SIGKILL 之后进程不能执行任何清理，二者不是同一类证据。

## 2. 需要杀的窗口（精确定义）

### 2.1 状态记号

- `C(v)`：`readlink(root/current) == versions/v`；`C(∅)`：无 `current`。
- `S(a,p)`：`state.json` 解析出的 active/previous；`S(∅)`：无 `state.json`。
- `J(s,t,p)`：`txn/` 中恰有一个 journal，状态 `s`、target `t`、previous `p`；`J(∅)`：无。
- `T(v)`：`root/versions/v` 是真实目录（非 symlink）。
- `F(v)`：`versions/v` 的整树 fingerprint（文件集合 + 每文件 SHA-256 + 模式）。
- `L`：legacy schema v1 的版本（fixture 为 `0.0.9`）；`Ltree` 永不执行、永不被记为 previous。

### 2.2 窗口表：upgrade N1→N2

| ID | 杀点（syscall/phase） | 杀前磁盘状态（必须在杀前断言） | 杀后立即状态 | `recover` 预期 |
| --- | --- | --- | --- | --- |
| U0 | `rename(staged, versions/N2)` 入口 | `C(N1)`、`S(N1,∅)`、`J(staged,N2,N1)`、`T(N2)=false`、staged 在 `tmp/` | tree 仍在 `tmp/`；`C(N1)`；`S(N1,∅)` | `staged N2 -> removed temporary state; current unchanged`；journal/tmp 清空 |
| U1 | 同上，退出 | `C(N1)`、`S(N1,∅)`、`J(staged,N2,N1)`、`T(N2)=true` | `T(N2)=true`；`C(N1)`；`S(N1,∅)` | `staged N2 -> target exists; marked committed; not auto-activated`；journal 变 committed |
| U2 | `rename(.<id>.json.tmp, txn/<id>.json)` 退出，解析 state=`committed` | `C(N1)`、`S(N1,∅)`、`J(committed,N2,N1)`、`T(N2)=true` | `C(N1)`；`S(N1,∅)`；journal committed | `committed N2 -> kept target; not auto-activated`；journal 保留 |
| U3 | `symlink(.current.tmp-*, versions/N2)` 入口 | `C(N1)`、`S(N1,∅)`、`J(committed,N2,N1)`、无 `.current.tmp-*` | `C(N1)`；无 temp；`S(N1,∅)` | 同 U2 |
| U4 | `rename(.current.tmp-*, current)` 入口 | `C(N1)`、`S(N1,∅)`、`J(committed,N2,N1)`、temp 存在 | `C(N1)`；temp 成为孤儿；`S(N1,∅)` | 同 U2 + 孤儿 `.current.tmp-*`（见 §10.2） |
| U5 | `rename(.current.tmp-*, current)` 退出 | `C(N2)`、`S(N1,∅)`、`J(committed,N2,N1)` | `C(N2)`；`S(N1,∅)`（state 仍是旧字节）；journal committed | `current==target` → `committed N2 -> verified and state written`；state 变 `S(N2,N1)`，journal 删除 |
| U6 | `rename(.state.json.tmp, state.json)` 退出 | `C(N2)`、`S(N2,N1)`、`J(committed,N2,N1)` | `C(N2)`；`S(N2,N1)`；journal committed | `committed N2 -> valid state already active; journal removed`；state 字节/inode 不变 |
| U7 | `rename(.<id>.json.tmp, txn/<id>.json)` 退出，解析 state=`activated` | `C(N2)`、`S(N2,N1)`、`J(activated,N2,N1)` | `C(N2)`；`S(N2,N1)`；journal activated | `activated N2 -> valid state already active; journal removed` |
| U8 | `unlink(txn/<id>.json)` 退出 | `C(N2)`、`S(N2,N1)`、`J(∅)`；`tmp/` 可能仍有 acquisition workspace | journal 已删除，`current`/state 指向 N2 | `no recovery actions were needed`；`recover` 顺带清理 `tmp/`（控制项） |

**P0（合同“switch 前/后”最小集）**：U3（前）和 U5（后）。
**P1（同一状态机的崩溃窗口）**：U0（rename 前）、U1/U2（rename 后、switch 前）、U6（state 已写）、U7（activated journal）。
**P2（卫生/控制）**：U4（孤儿 temp）、U8（完成态）。

> U3 是 `BeforeSwitch` hook 的精确等价物：hook 在 `root.switch` 之前，`switch` 的第一条
> 副作用就是 `symlink`。U5 是“`current` 已变、`state` 未写”的最早进程崩溃点（目录 fsync
> 尚未执行；这是进程崩溃，不是断电）。

### 2.3 窗口表：rollback N2→N1

| ID | 杀点 | 杀前状态 | 杀后立即状态 | `recover` 预期 |
| --- | --- | --- | --- | --- |
| R0 | journal `committed` 的 rename 退出 | `C(N2)`、`S(N2,N1)`、`J(committed,N1,N2)`、`T(N1)=true` | `C(N2)`；`S(N2,N1)`；journal committed | `committed N1 -> kept target; not auto-activated` |
| R1 | `symlink(.current.tmp-*, versions/N1)` 入口 | 同 R0，且无 temp | `C(N2)`；无 temp | 同 R0 |
| R2 | `rename(.current.tmp-*, current)` 退出 | `C(N1)`、`S(N2,N1)`、`J(committed,N1,N2)` | `C(N1)`；`S(N2,N1)`；journal committed | `current==target` → `committed N1 -> verified and state written`；state 变 `S(N1,N2)` |
| R3 | `rename(.state.json.tmp, state.json)` 退出 | `C(N1)`、`S(N1,N2)`、`J(committed,N1,N2)` | `C(N1)`；`S(N1,N2)`；journal committed | `committed N1 -> valid state already active; journal removed` |
| R4 | journal `activated` 的 rename 退出 | `C(N1)`、`S(N1,N2)`、`J(activated,N1,N2)` | `C(N1)`；`S(N1,N2)`；journal activated | `activated N1 -> valid state already active; journal removed` |

**P0**：R1（前）和 R2（后）。**P1**：R0/R3/R4。
合同 F3-06 只写 upgrade；rollback kill 是状态机加固（F3-07 已 closed，F3-16 覆盖 journal
注入），**不计入 F3-06 的合同关闭条件**，但推荐保留，因为它验证同一个 `switch`/state 机制。

### 2.4 窗口表：legacy L→N1 迁移

| ID | 杀点 | 杀前状态 | 杀后立即状态 | `recover` 预期 |
| --- | --- | --- | --- | --- |
| L0 | `rename(staged, versions/N1)` 入口 | `C(L)`、`S(∅)`、`J(staged,N1,∅)`、`T(N1)=false` | tree 在 `tmp/`；`C(L)`；无 state | `staged N1 -> removed temporary state; current unchanged` |
| L1 | 同上，退出 | `C(L)`、`S(∅)`、`J(staged,N1,∅)`、`T(N1)=true` | `T(N1)=true`；`C(L)`；无 state | `staged N1 -> target exists; marked committed; not auto-activated` |
| L2 | journal `committed` 的 rename 退出 | `C(L)`、`S(∅)`、`J(committed,N1,∅)` | `C(L)`；无 state；journal committed | `committed N1 -> kept target; not auto-activated` |
| L3 | `symlink(.current.tmp-*, versions/N1)` 入口 | 同 L2，且无 temp | `C(L)`；无 temp | 同 L2 |
| L4 | `rename(.current.tmp-*, current)` 退出 | `C(N1)`、`S(∅)`、`J(committed,N1,∅)` | `C(N1)`；无 state；journal committed | `current==target` → `committed N1 -> verified and state written`；state 变 `S(N1,∅)` |
| L5 | `rename(.state.json.tmp, state.json)` 退出 | `C(N1)`、`S(N1,∅)`、`J(committed,N1,∅)` | `C(N1)`；`S(N1,∅)`；journal committed | `committed N1 -> valid state already active; journal removed` |
| L6 | journal `activated` 的 rename 退出 | `C(N1)`、`S(N1,∅)`、`J(activated,N1,∅)` | `C(N1)`；`S(N1,∅)`；journal activated | `activated N1 -> valid state already active; journal removed` |

**P0**：L3（前）和 L4（后）。**P1**：L0/L1/L2/L5/L6。
所有 L 窗口都必须断言：`F(L)` 与杀前一致（旧树字节不变）、legacy marker 未执行、无 Node 进程、
`state.previous` 永不等于 `L`、`current` 永不在杀后回到 `L`（L4 之后）。

## 3. 方案 A：ptrace syscall-stop + 父进程检查 + 外部 SIGKILL

### 3.1 机制

1. 父进程（test harness）`fork`，子进程 `setpgid(0,0)`、`PTRACE_TRACEME`、`raise(SIGSTOP)`，
   父进程 `waitpid` 到初始 stop，设置
   `PTRACE_O_TRACESYSGOOD | PTRACE_O_EXITKILL`，用 `PTRACE_SYSCALL` 恢复。
2. 子进程 `execve` 真实 `wxml-lsp-install`（绝对路径、clean env、`--offline`、`--from-dir`）。
3. 每个 syscall 产生 entry/exit 两次 stop（`SIGTRAP|0x80`）。父进程用 `PTRACE_GETREGS`
   读 `orig_rax`（x86_64）和参数；用 `PTRACE_PEEKDATA` 读路径字符串（`PATH_MAX` 4096）。
4. **Gate** 由四部分组成：
   - `syscall ∈ {rename, renameat, renameat2, symlink, symlinkat}`；
   - `phase ∈ {entry, exit}`；
   - 路径谓词：newpath/linkpath 的父目录是 canonical root 下的 `versions/`、`txn/`、root 本身，
     basename 匹配 `current`、`state.json`、`*.json`、`.current.tmp-*`，或 `versions/<target>`；
   - 磁盘谓词：`C(v)`、`S(a,p)`、`J(s,t,p)`、`T(v)`、temp 是否存在。
5. 命中且磁盘谓词成立时，父进程**先记录** `syscall/pid/phase/path/C/S/J/T`，再
   `kill(pid, SIGKILL)`（同时 `kill(-pid, SIGKILL)` 清进程组），`waitpid` 断言
   `WIFSIGNALED && WTERMSIG == 9`。
6. 未命中则继续；整个 invocation 结束后，若 gate 命中次数 != 1，测试失败（禁止 false green）。

**为什么 entry 就是“之前”、exit 就是“之后”**：ptrace 的 syscall-entry-stop 在系统调用执行前，
exit-stop 在系统调用完成后、返回用户态前。tracee 在 stop 期间不执行任何用户代码；父进程杀死
它时，entry 的系统调用不会发生，exit 的效果已经发生。这不是时间竞争，而是内核提供的精确边界。

### 3.2 可行性实测（本机，合成程序，不是真实 CLI）

环境：Linux `7.2.15+deb14-amd64`、glibc 2.43、rustc 1.100、`/proc/sys/kernel/yama/ptrace_scope=0`。

用 `/tmp/opencode/f3-kill-probe` 下的合成程序（创建 `versions/0.1.0`、`current` → `versions/0.1.0`、
一个 committed journal 文本，然后 `symlink` + `rename` + `fsync`）做 PTRACE_SYSCALL：

- 观察到的 syscall：`symlink(88)`、`rename(82)`、`fsync(74)`；每个 syscall 都有 entry/exit 两次 stop。
- 在 `symlink` **entry** 杀：`current` 仍是 `versions/0.1.0`，没有 `.current.tmp-*`，目标目录已存在。
- 在 `rename` **exit** 杀：`current` 变成 `versions/0.2.0`。
- 在 `rename` **entry** 杀：`current` 仍是 `versions/0.1.0`，`.current.tmp-*` 成为孤儿。

结论：`PTRACE_TRACEME + PTRACE_SYSCALL + PTRACE_GETREGS + PTRACE_PEEKDATA + kill(SIGKILL)`
在本机可用；syscall 号与当前工具链一致。该实验只证明机制，不作为 F3 证据（合成程序不是
`wxml-lsp-install`，也没有真实包）。真实测试必须打印观察到的 syscall 号，并同时处理
`renameat/renameat2/symlinkat`，避免未来 libc/工具链变化造成静默漏杀。

### 3.3 精确性与 race

| 风险 | 结论/缓解 |
| --- | --- |
| 杀点漂移 | tracee 在 syscall stop 中不运行；entry/exit 是内核定义的精确边界，无墙钟竞争 |
| 路径匹配错 | 先匹配路径，再匹配磁盘谓词；两者都成立才杀；命中次数必须为 1 |
| 目录项可见性 | `rename` 在同一内核中立即可见，父进程无需 `fsync`；但 exit 杀点可能在目录 fsync 之前，**只承诺进程崩溃，不承诺断电** |
| tracee 线程 | CLI 事务在主线程；未开 `TRACECLONE`。若未来移到 worker 线程，gate 不会命中，测试失败而不是假绿 |
| 子进程（引擎探针/`--version`） | 未开 `TRACEFORK`，子进程不被 trace，正常运行；杀根进程时用进程组 SIGKILL 兜底 |
| 待处理信号 | 非 syscall-stop（`SIGTRAP+0x80`）的 stop 按信号投递/忽略（`SIGCHLD` 可吞），与现有 tracer 的 `delivered_signal` 同策略 |
| 父进程崩溃 | `PTRACE_O_EXITKILL` 保证 tracee 一起死，不留孤儿 |
| `waitpid` 竞态 | 单 tracee + 进程组 kill + 有界 drain；不引入现有 tracer 的 clone/ECHILD 复杂度 |

### 3.4 权限与平台

- 同 uid 的父子关系即可；`PTRACE_TRACEME` 不需要 root，也不需要 `CAP_SYS_PTRACE`。
- `yama/ptrace_scope=0/1` 允许（子进程是后代）；`=2` 拒绝，归类 `Blocked`。
- 容器 seccomp/LSM 可能拒绝 ptrace；`EPERM/EACCES/ENOSYS` 归类 `Blocked`，不是 Failed。
- 需要 `/proc`（读 `fd`/`cwd` 解析 dirfd、读 `/proc/<pid>/exe` 可选）。没有 `/proc` 则 Blocked。
- 首版只支持 **Linux x86_64**（`PTRACE_GETREGS` + `user_regs_struct`）。aarch64 需要
  `PTRACE_GETREGSET/NT_PRSTATUS` 或 `PTRACE_GET_SYSCALL_INFO`，不是本项合同范围。
- 复用 `f3_exec_audit` 的 preflight 语义：`WXML_REQUIRE_EXEC_AUDIT=1` 或
  `WXML_REQUIRE_REAL_ENGINE=1` 时 Blocked 必须失败；新增 `WXML_REQUIRE_KILL_WINDOW=1` 同理
  （ `crates/wxml-delivery/tests/f3_exec_audit/support.rs:46-96` ）。

### 3.5 如何避免 false green

1. **preflight**：用 `PTRACE_SYSCALL` 跑 `/bin/true`，必须观察到至少一次 syscall stop 并正常退出；
   只证明 `TRACEME` 不够。
2. **gate 命中恰好 1 次**：0 次 = 测试失败（打印 CLI stdout/stderr、最后 syscall 摘要、gate 规格）；
   >1 次 = 失败（说明路径/谓词写错，或事务重复切换）。
3. **杀前磁盘谓词必须匹配**：不匹配就不能杀；记录“candidate 但状态不符”并最终失败。
4. **杀后断言 `WTERMSIG=9`**：不是普通错误退出，不是 `SIGABRT`/`SIGTERM`。
5. **被杀进程的 stdout 不得包含成功行**（`installed`/`upgraded to`/`migrated to`/`rolled back to`）。
6. **控制项**：用一个永不匹配的 gate 跑同一命令，CLI 必须成功、gate 0 命中；证明 tracer 不会
   随意杀，也证明 syscall 跟踪不会把 CLI 拖死。
7. **真实包缺失时 SKIP 而不是 PASS**；`WXML_REQUIRE_REAL_ENGINE=1` 时失败关闭。

### 3.6 代价与风险

- 新增 test-only 文件约 1200–1700 行（tracer/gate/real/script/doc），**不改生产/Cargo**。
- 每个 syscall 两次 stop；真实安装器估计 5 万–20 万次 stop，额外 0.1–2 秒；给 2× 超时预算。
- 需要 `unsafe extern "C"`（与 `f3_exec_audit/tracer.rs:33-87` 同模式），仅 test binary 编译。
- 主要风险：syscall 号/路径匹配漏（用实际号和 `renameat*` 兜底）、ptrace 被禁（Blocked）、
  真实测试时长（--test-threads=1）、未来事务移到线程（gate miss 会显式失败）。

### 3.7 能关闭什么

- 真实 `wxml-lsp-install` 在 U3/U5、R1/R2、L3/L4 被**外部** SIGKILL；杀前状态由父进程断言，
  杀后用真实 `recover`/`verify` 验证。这满足“生产 CLI 进程级精确 SIGKILL”。
- U1/U2 覆盖“rename 后、switch 前”的进程崩溃（现有 seam 的 `AfterRename` 的进程级版本）；
  U0 覆盖 rename 前的 staged 窗口。
- U6/U7/U8 覆盖“state 已写/activated journal/完成态”的幂等恢复。
- R2/R3/R4 覆盖 rollback 的同一机制。
- L1/L2/L4/L5/L6 覆盖 legacy 迁移的“旧树不变、无 native state、前滚补 state、永不恢复 Node”。

## 4. 方案 B：文件轮询 + SIGSTOP

### 4.1 机制

父进程正常 spawn 安装器，用 inotify（或紧循环 `stat/readlink/read_dir`）观察：

- `versions/` 出现 `<target>`（tree rename 完成）；
- `txn/` 出现/替换 `<id>.json`（journal staged/committed/activated）；
- root 的 `current` 被替换（IN_MOVED_TO）；
- `state.json` 被替换。

事件到达后读磁盘状态；若匹配目标窗口，`kill(pid, SIGSTOP)`；再读一次确认；然后 `SIGKILL`。
若不匹配（例如已经过了 `switch`），分类为“观察到的实际窗口”，重试或接受该样本。

### 4.2 窗口宽度（按代码顺序估算）

| 窗口 | 中间操作 | 估算宽度 |
| --- | --- | --- |
| committed journal 可见 → `symlink` | `fsync(txn)` + 2 个 no-op hook + `symlink_metadata` + `symlink` | ~0.1–1 ms |
| tree rename 可见 → `symlink` | `fsync(versions)` + `AfterRename` + journal committed 原子写/`fsync` + 2 hook | ~0.3–1.5 ms |
| `rename(current)` → `rename(state)` | `fsync(root)` + 2 hook + JSON 序列化 + state 原子写/`fsync` | ~0.3–1.5 ms |
| state 可见 → journal activated | `fsync(root)` + `AfterStateWrite` + journal 原子写/`fsync` | ~0.2–1 ms |

这是按系统调用和 fsync 数量做的数量级估算，不是真实 CLI 的实测。inotify 唤醒 + 读盘 +
`SIGSTOP` 的典型延迟 50–500 µs，负载下可到毫秒；与窗口同量级或更大。

### 4.3 为什么不可靠

- **`SIGSTOP` 在系统调用中不会立即生效**：信号在返回用户态时处理；如果进程正在 `rename`
  或 `fsync`，它会先完成该系统调用再停。也就是说，看到 `committed` 再 `SIGSTOP`，很可能
  已经过了 `switch`。
- **最好的目标也不是“committed → switch”**：那个窗口只有一次 `fsync(txn)` 和两个 no-op；
  更宽的是“tree rename → switch”，但仍然只有一次 journal 原子写 + fsync。
- **`switch` 后窗口**：看到 `current` 变化再停，可能 `state` 已经写完；样本会落到 U6 而不是 U5。
- **无法证明“杀在指定边界”**：只能证明“杀在某个观察到的磁盘状态”。必须分类 + 重试，
  并且报告实际样本分布。
- 轮询还会消耗 CPU；紧循环在多核上可行，但在 CI 的共享 runner 上抖动大。

### 4.4 能证明/不能证明

- 能：任意时刻的进程 kill 后，`current`/`state`/journal 至少有一个一致的可恢复状态；没有混合资产；
  某些样本能落入 pre/post-switch。
- 不能：确定性命中 U3/U5/R1/R2/L3/L4；不能替代“精确窗口”证据；不能关闭 F3-06/09 的进程级条件。
- 适用：ptrace 被禁、且评审接受“采样证据”的场合；此时只能标 `sampled`，不能标 `PASS` 关闭。

## 5. 方案 C：生产 CLI 加 test-only failpoint

### 5.1 env-only（默认关闭但编译进发布产物）

做法：在 `install_native_locked` 的 checkpoint 读 `WXML_INSTALL_FAILPOINT`；命中时
`raise(SIGSTOP)`/park 或 `raise(SIGKILL)`；测试父进程观察后杀。

支持理由：

- 确定性最高：源码位置固定，不受 syscall/路径匹配影响；
- 不需要 ptrace，跨平台；
- 可以复用现有 `FailPoint` 枚举，改动小。

反对理由（决定性）：

- **“不可被最终用户触发”不可能满足**：环境变量由最终用户/其 shell/CI 控制；任何用户都能让
  安装器停在半途或直接死掉。对无人值守安装这是 DoS/挂起风险。
- **发布产物被改变**：多一个隐藏分支/字符串；F3-03 的两次构建逐字节一致证据要重跑，安全评审
  要解释“为什么发布二进制里有一个能让进程停住的开关”。
- **自杀不是外部 SIGKILL**：`raise(SIGKILL)` 是进程自己选择死亡；评审可以合理拒绝它等价于
  “外部进程在精确边界杀死 CLI”。
- **需要新依赖或额外机制**：`wxml-delivery` 当前没有 `libc` 依赖；要 `raise` 就得加依赖，
  或者用 `abort()`（SIGABRT，不是 SIGKILL），或者 park + 父进程外部杀（仍然有 env 可触发性）。
- 当前切片明确“不改生产代码”；env-only 直接违反。

### 5.2 feature-only（`#[cfg(feature = "failpoints")]`，默认不编译）

做法：`wxml-delivery` 增加 `failpoints` feature，CLI 透传；checkpoint 在 feature 下读 env，
park 后由父进程 SIGKILL。

支持理由：

- 默认发布产物不含代码，满足“默认不可用、不可被最终用户触发”；
- 确定性、跨平台、无需 ptrace；
- 可作为未来非 Linux 平台的辅助证据。

反对理由：

- **被测二进制不再是发布产物**：即使源码相同，feature 组合不同；F3 合同要求“真实包/真实 CLI”，
  评审可能只接受为“instrumented build”辅助证据，不能单独关闭。
- **仍要改生产源码和 Cargo**；增加 feature 矩阵、CI 默认构建检查、`cargo test --all-features`
  的挂起风险（如果测试设置 env）。
- **代码漂移**：checkpoint 调用在事务函数里，未来重构可能移动；需要额外回归测试保证顺序。
- **park + 父进程 kill 仍然是协作式**：进程主动停在 checkpoint；不是异步外部杀。语义上比
  Plan A 弱。

### 5.3 结论

- **不作为 F3-06/09 的关闭机制**。env-only 不可接受；feature-only 只能作为未来跨平台的辅助
  证据，且必须：默认 feature 关闭、发布构建显式 `--no-default-features` 检查、用 `nm`/字符串
  检查发布二进制不含 failpoint、证据明确标注“instrumented build”。
- 当前最小方案不应引入任何生产 failpoint；Plan A 已经能在不改生产代码的前提下给出更强证据。

## 6. 推荐最小方案

### 6.1 文件

只新增 test/script/doc，不改 `crates/wxml-delivery/src/**`、`Cargo.toml`、现有测试：

- ` crates/wxml-delivery/tests/f3_kill_window.rs`：test binary 根，lint allow 与
  `f3_exec_audit.rs:48-77` 同风格，`#[path]` 挂载子模块。
- ` crates/wxml-delivery/tests/f3_kill_window/tracer.rs`：`PTRACE_TRACEME/SYSCALL/GETREGS/
  PEEKDATA/EXITKILL`、fork/exec、输出收集、超时、进程组 kill。
- ` crates/wxml-delivery/tests/f3_kill_window/gate.rs`：gate 规格、路径解析、`C/S/J/T` 谓词、
  杀前证据格式。
- ` crates/wxml-delivery/tests/f3_kill_window/real.rs`：P0/P1 测试，复用
  `f3_acceptance/support.rs` 的 `shared_channel`、`installer_bin`、`private_root`、
  `installed_fingerprint`、`legacy_fingerprint`、`Tripwires`。test 根还要像
  `f3_exec_audit.rs:106-116` 那样 `#[path]` 挂 `real_asset/support.rs` 与
  `f3_real_release/support.rs`，否则 `crate::release`/`real_asset_support` 解析不到。
- ` crates/wxml-delivery/tests/f3_kill_window/support.rs`：clean env、`TestDir`、证据行、
  `WXML_REQUIRE_KILL_WINDOW` 门禁。
- ` scripts/f3-kill-window.sh`：构建 release 二进制、要求 pinned tarball、跑
  `--ignored --nocapture --test-threads=1`、grep `F3-EVIDENCE`；输出目录必须在
  `/tmp/opencode` 且不在仓库内（复用 `f3-acceptance.sh:67-87` 的检查）。
- ` docs/evidence/2026-11-20-tsgo-f3-kill-window.md`：运行后写；本次只交设计。

Cargo 自动发现 `tests/*.rs`，不需要 `[[test]]`。不要改 `f3_exec_audit` 的 tracer，避免
F3-02 回归；fork/exec 低层代码可以复制（约 250 行），或后续再抽公共 test support。

### 6.2 实施顺序

1. tracer + gate + preflight + 控制项（永不匹配的 gate，CLI 必须成功）。
2. U3/U5（upgrade P0），确认真实包、杀前谓词、`recover` 输出、N1/N2 指纹。
3. R1/R2（rollback P0），确认 state 前滚。
4. L3/L4（legacy P0），确认旧树不变、marker 未执行、无 Node、`state.previous=null`。
5. P1：U0/U1/U2/U6/U7、L0/L1/L2/L5/L6、R0/R3/R4；最后 P2 的 U4/U8（记录孤儿 temp）。
6. 脚本 + 网络命名空间 + 证据文档；更新合同 §9.5/README 放到评审通过之后。

### 6.3 验收标准

- 每个 P0/P1 窗口都打印 `F3-EVIDENCE: ... PASS :: gate=...; pre=...; sig=9; recover=...`。
- gate 0 命中、>1 命中、杀前谓词不匹配、`WTERMSIG != 9`、被杀进程打印成功行，任一都失败。
- 真实资产缺失 → `SKIP`；ptrace 被禁 → `BLOCKED`；硬门禁下二者都失败。
- 现有 F3-02/F3-16 测试不受影响（独立 test binary）。
- `git diff` 只有新增 test/script/doc；`crates/wxml-delivery/src` 与 `Cargo.toml` 无变化。

## 7. 测试矩阵

### 7.1 P0（关闭“精确 kill”必需，6 个）

| ID | F3 | 操作 | gate | 杀前断言 | 杀后/恢复断言 |
| --- | --- | --- | --- | --- | --- |
| K-U-B | F3-06 | N1 上 `upgrade` | `symlink(.current.tmp-*)` entry | `C(N1)`、`S(N1,∅)`、`J(committed,N2,N1)`、`T(N2)=true`、无 temp | 杀后 `C(N1)`/state 旧字节；`recover` = `kept target; not auto-activated`；显式 `upgrade` 后 `C(N2)`、`S(N2,N1)`、journal/tmp 空 |
| K-U-A | F3-06 | N1 上 `upgrade` | `rename(temp,current)` exit | `C(N1)`、`S(N1,∅)`、`J(committed,N2,N1)` | 杀后 `C(N2)`、`S(N1,∅)`；`recover` = `verified and state written`；`C(N2)`、`S(N2,N1)`；`verify` 通过 |
| K-R-B | F3-07/16 | N2 上 `rollback --to N1` | `symlink(.current.tmp-*)` entry | `C(N2)`、`S(N2,N1)`、`J(committed,N1,N2)` | 杀后 `C(N2)`、`S(N2,N1)`；`recover` = `kept target; not auto-activated`；显式 `rollback` 后 `C(N1)`、`S(N1,N2)` |
| K-R-A | F3-07/16 | N2 上 `rollback --to N1` | `rename(temp,current)` exit | `C(N2)`、`S(N2,N1)`、`J(committed,N1,N2)` | 杀后 `C(N1)`、`S(N2,N1)`；`recover` = `verified and state written`；`C(N1)`、`S(N1,N2)`；`verify` 通过 |
| K-L-B | F3-09 | legacy 上 `install --version N1 --from-dir ... --offline` | `symlink(.current.tmp-*)` entry | `C(L)`、`S(∅)`、`J(committed,N1,∅)`、`T(N1)=true` | 杀后 `C(L)`、无 state；`recover` = `kept target; not auto-activated`；`F(L)` 不变、marker 未执行；重试 = `migrated to 0.1.0 (native)`、`S(N1,∅)` |
| K-L-A | F3-09 | 同上 | `rename(temp,current)` exit | `C(L)`、`S(∅)`、`J(committed,N1,∅)` | 杀后 `C(N1)`、无 state；`recover` = `verified and state written`；`S(N1,∅)`；`F(L)` 不变、marker 未执行、`verify` 通过 |

### 7.2 P1（推荐，证明同一状态机的其他崩溃窗口）

| ID | F3 | gate | 关键断言 |
| --- | --- | --- | --- |
| K-U-S | F3-06 | journal `staged` rename exit | `C(N1)`、`T(N2)=false`；`recover` = `removed temporary state`；tmp 空 |
| K-U-R | F3-06 | `rename(staged, versions/N2)` exit | `C(N1)`、`T(N2)=true`、`J(staged,...)`；`recover` = `target exists; marked committed; not auto-activated` |
| K-U-C | F3-06 | journal `committed` rename exit | 同 K-U-B 的杀前；`recover` = `kept target; not auto-activated` |
| K-U-W | F3-06 | `rename(.state.json.tmp,state.json)` exit | `C(N2)`、`S(N2,N1)`、`J(committed,...)`；`recover` = `valid state already active; journal removed`；state 字节/inode 不变 |
| K-U-J | F3-06 | journal `activated` rename exit | `C(N2)`、`S(N2,N1)`、`J(activated,...)`；`recover` 幂等删除 journal |
| K-L-S | F3-09 | legacy journal `staged` rename exit | `C(L)`、`S(∅)`、`T(N1)=false`；`recover` = `removed temporary state`；`F(L)` 不变 |
| K-L-R | F3-09 | `rename(staged, versions/N1)` exit | `C(L)`、`T(N1)=true`、`J(staged,...)`；`recover` = `target exists; marked committed`；`F(L)` 不变 |
| K-L-C | F3-09 | legacy journal `committed` rename exit | `C(L)`、`S(∅)`、`J(committed,N1,∅)`；`recover` = `kept target; not auto-activated`；`F(L)` 不变 |
| K-L-W | F3-09 | `state.json` rename exit | `C(N1)`、`S(N1,∅)`、`J(committed,...)`；`recover` 幂等；`F(L)` 不变 |
| K-L-J | F3-09 | journal `activated` rename exit | `C(N1)`、`S(N1,∅)`、`J(activated,...)`；`recover` 幂等；`F(L)` 不变 |
| K-R-W | F3-07/16 | rollback `state.json` rename exit | `C(N1)`、`S(N1,N2)`、`J(committed,...)`；`recover` 幂等 |
| K-R-J | F3-07/16 | rollback journal `activated` rename exit | `C(N1)`、`S(N1,N2)`、`J(activated,...)`；`recover` 幂等 |

### 7.3 P2（可选，卫生/控制）

| ID | gate | 目的 |
| --- | --- | --- |
| K-U-T | `rename(temp,current)` entry | 证明 `.current.tmp-*` 孤儿的产生；记录 §10.2；测试后手工删除再重试 |
| K-C-N | 永不匹配的 gate | 控制项：CLI 必须成功、0 命中、无杀；证明 tracer 不误杀 |
| K-U-F | `rename(temp,current)` exit 后、`fsync(root)` exit | 可选的“durable 后”样本；证明与 U5 同一恢复语义 |

## 8. 真实包命令

### 8.1 构建（仓库外）

```bash
OUT=/tmp/opencode/f3-kill-window
mkdir -p "$OUT/logs" "$OUT/target" "$OUT/tmp"
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$OUT/target" \
  cargo build --release -p wxml-lsp -p wxml-delivery
```

### 8.2 运行

```bash
TSGO_NATIVE_TARBALL=/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz \
TSGO_TYPINGS_TARBALL=/tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz \
WXML_F3_INSTALL_BIN="$OUT/target/release/wxml-lsp-install" \
WXML_REQUIRE_REAL_ENGINE=1 WXML_REQUIRE_KILL_WINDOW=1 \
WXML_TEST_TMPDIR="$OUT/tmp" \
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$OUT/target" \
  cargo test -p wxml-delivery --test f3_kill_window -- \
    --ignored --nocapture --test-threads=1 2>&1 | tee "$OUT/logs/f3-kill-window.log"
```

网络隔离（可选但推荐）：

```bash
unshare -rn env \
  TSGO_NATIVE_TARBALL=... TSGO_TYPINGS_TARBALL=... \
  WXML_F3_INSTALL_BIN="$OUT/target/release/wxml-lsp-install" \
  WXML_REQUIRE_REAL_ENGINE=1 WXML_REQUIRE_KILL_WINDOW=1 WXML_TEST_TMPDIR="$OUT/tmp" \
  CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$OUT/target" \
  cargo test -p wxml-delivery --test f3_kill_window -- \
    --ignored --nocapture --test-threads=1
```

单窗口过滤（调试）：

```bash
cargo test -p wxml-delivery --test f3_kill_window -- \
  --ignored --nocapture --test-threads=1 f3_06_real_cli_sigkill_upgrade
```

### 8.3 期望证据行

```text
F3-EVIDENCE: F3-06 PASS :: real CLI pid=... sig=9 gate=symlink(.current.tmp-*)/entry;
  pre current=0.1.0 state=active:0.1.0/previous:null journal=committed(0.2.0<-0.1.0) target=0.2.0;
  post current=0.1.0 state-bytes=unchanged journal=committed; recover=kept target/not auto-activated;
  retry=upgraded to 0.2.0; N1 fingerprint unchanged; verify=N2
F3-EVIDENCE: F3-06 PASS :: real CLI pid=... sig=9 gate=rename(current)/exit;
  pre current=0.2.0 state=active:0.1.0/previous:null journal=committed(0.2.0<-0.1.0);
  post current=0.2.0 state=active:0.1.0; recover=verified and state written;
  state=active:0.2.0/previous:0.1.0; journal removed; verify=N2
F3-EVIDENCE: F3-09 PASS :: real CLI pid=... sig=9 gate=symlink(.current.tmp-*)/entry;
  pre current=0.0.9(schema 1) state=absent journal=committed(0.1.0<-null);
  post current=0.0.9 state=absent; recover=kept target/not auto-activated;
  legacy fingerprint unchanged; legacy marker absent; retry=migrated to 0.1.0 (native)
F3-EVIDENCE: F3-09 PASS :: real CLI pid=... sig=9 gate=rename(current)/exit;
  pre current=0.1.0 state=absent journal=committed(0.1.0<-null);
  post current=0.1.0 state=absent; recover=verified and state written;
  state=active:0.1.0/previous:null; legacy fingerprint unchanged; marker absent; verify=N1
F3-EVIDENCE: F3-06 BLOCKED :: ptrace syscall-stop unavailable (seccomp/Yama/ptrace_scope=2);
  WXML_REQUIRE_KILL_WINDOW=1 makes this a failure
```

### 8.4 主机前置

```bash
uname -r
cat /proc/sys/kernel/yama/ptrace_scope 2>/dev/null || echo "yama: n/a"
# 期望：Linux x86_64；ptrace_scope 为 0 或 1；容器允许 ptrace；/proc 已挂载
```

非 Linux x86_64、无 `/proc`、ptrace 被禁：显式 `BLOCKED`，不得算 PASS。与 F3-02 的平台口径一致。

## 9. 关闭判定

### 9.1 F3-06

| 子条件 | 方案 A 后的证据 | 能否关闭 |
| --- | --- | --- |
| switch 前 N1 digest/current 不变、N2 不激活 | K-U-B（U3）+ K-U-S/K-U-R/K-U-C（P1）；杀前 `C(N1)`、`S(N1,∅)`、`J(committed,N2,N1)`；`recover` kept target | **能**（Linux x86_64） |
| switch 后 state 已写则 N2 前滚 | K-U-A（U5）+ K-U-W（U6）+ K-U-J（U7）；`recover` 写 `S(N2,N1)`、journal 删除、`verify` 通过 | **能**（Linux x86_64） |
| switch 后 state 仍指 N1 则切回 N1 | 这是**进程存活时**的 `handle_post_switch_failure`/`restore_previous`（`native_install.rs:1640-1643`、`1700-1763`），现有 `AfterSwitch` seam 已 PASS；SIGKILL 后进程无法切回，`recover` 按 §6.3 第 976 行前滚 | 需要**合同语义裁决**；不能由 SIGKILL 测试产生“切回” |
| 无混合资产 | 每个窗口杀后 `F(N1)`/`F(N2)` 不变；恢复后 `assert_current_verified`/`verify` 通过 | **能** |
| 第二个真实 release | 当前 N2 是 N1 生产二进制同长度版本常量重标 | **仍 partial**，除非评审接受或产出真实 N2 |

**结论**：按“N2 retarget 已被接受、§6.3 第 975 行仅指进程存活失败”的口径，方案 A 可以关闭
F3-06 的进程级条件，F3-06 可判 **closed（Linux x86_64）**。按合同 §9.5 的字面“仍缺”列，
N2 provenance 与 §6.3 语义仍是独立缺口，F3-06 只能 **partial**。本设计不替评审做这个裁决。

### 9.2 F3-09

| 子条件 | 方案 A 后的证据 | 能否关闭 |
| --- | --- | --- |
| switch 前旧 current/旧树不变、无 native state | K-L-B（L3）+ K-L-S（staged journal）+ K-L-R（L1）+ K-L-C（L2）；`C(L)`、`S(∅)`、`F(L)` 不变、marker 未执行 | **能**（Linux x86_64） |
| switch 后对已验证 target 前滚补 state、永不恢复/执行 Node | K-L-A（L4）+ K-L-W/K-L-J（P1）；`C(N1)`、`S(N1,∅)`、`F(L)` 不变、marker 未执行、`verify` 通过 | **能**（Linux x86_64） |
| 真实 legacy 发布包 | 当前是生产 v1 packager 生成的 fixture（含 Node entry tripwire） | **仍 partial**，除非评审接受或产出真实 legacy 包 |

**结论**：方案 A 可以关闭 F3-09 的进程级条件；若“真实 legacy 发布包”不被接受，F3-09 仍
**partial**，原因与 kill 机制无关。若评审接受 fixture 为等价 legacy current，F3-09 可判
**closed（Linux x86_64）**。

### 9.3 仍只能 partial / 不能声称的项

- **非 Linux x86_64**：无 syscall-stop tracer；只能 `BLOCKED`。
- **断电/目录 fsync 持久性**：exit 杀点可能在 `fsync` 之前；本设计只证明进程崩溃，不证明断电
  （与合同 §10.2 一致）。
- **孤儿 temp 文件**：U4（rename entry）的 `.current.tmp-*`、state/journal 的 `.*.tmp-*` 不被 `recover`
  清理；不影响正确性，但属于卫生缺口（§10.2）。
- **N2 / legacy provenance**：见 §9.1/§9.2。
- **合同 §6.3 第 975 行**：见 §10.1。
- **rollback kill**：F3-07 已 closed；本项是加固证据，不改变 F3-07 状态。

## 10. 风险与未决问题

### 10.1 合同 §6.3：切回 vs 前滚（必须先裁决）

- §6.3 第 975 行：“switch 后、state 写入前：若 state 仍指 previous 且 previous 已验证：切回且不重写 state；
  若 state 已指 target：不切回，重新校验后前滚。”
- §6.3 第 976 行：“current 为已验证 native，state 指向另一版本：有 committed/activated journal
  且 target 可验证时，重新校验 current/target 后前滚到 target；否则报错；恢复幂等。”
- `recover_committed`（`native_install.rs:1195-1199`）先看 `current == journal.target`，成立就
  “complete target”（前滚），不看 state 是否仍指 previous。因此 U5（`current=N2`、`state=N1`）
  的 `recover` 结果是 **N2 前滚**，不是 N1 切回。
- `handle_post_switch_failure`（`:1622-1643`）在进程存活、`state.active == previous` 时调用
  `restore_previous`，才是“切回”。现有 `AfterSwitch` seam 已覆盖。
- **建议**：在合同/证据里明确“第 975 行只描述进程存活时的失败处理；进程被 SIGKILL 后按第 976 行
  前滚”。如果不接受这个解释，就必须改 `recover` 让它在 state 仍指 previous 时切回，或者承认
  F3-06 的该子条件在崩溃场景下不可满足。**这是关闭 F3-06 的前置决定，不是测试实现细节。**

### 10.2 孤儿 temp 文件（卫生，不是正确性）

- `switch` 在 `symlink` 后、`rename` 前被杀：`.current.tmp-<pid>-<nanos>` 留下。
- `atomic_write` 在 `create_new` 后、`rename` 前被杀：`.state.json.tmp-*` 或 `.<id>.json.tmp-*` 留下。
- `recover` 只清 `root/tmp`；`Journal::list` 只读 `*.json`；`InstallState::read` 只读 `state.json`；
  `ensure_safe_layout` 不检查这些点文件。它们不影响 `current`/state/journal 的正确性，但会累积。
- 建议后续在持锁的 `recover`/`switch` 里清理“属于已结束事务”的 `.*.tmp-*`；本设计不把它作为
  F3-06/09 的阻塞项。测试遇到时记录为 `FIXTURE` 并在 detail 里标注 `hygiene`，不要误判为状态损坏。

### 10.3 provenance

- N2 是 N1 生产二进制替换同长度版本常量（`f3_real_release/runtime.rs:334-347`）；kill 测试可以
  用它，但不能证明“第二个真实 release”的来源。
- legacy 是生产 v1 packager 的 fixture（`f3_acceptance/support.rs:907` 起），不是历史发布包；
  kill 测试可以用它证明迁移状态机，但不能证明真实 legacy 包的身份。
- 这两项要么产出真实产物重跑，要么在合同/证据里记录评审 waiver；方案 A 不改变它们。

### 10.4 平台/时间

- 首版 Linux x86_64；aarch64 需要另一套寄存器读取。
- syscall 号可能随 libc/工具链变化；测试必须打印实际号，并覆盖 `renameat/renameat2/symlinkat`。
- 真实测试 `--test-threads=1`；共享 channel 构建 + 10 个 kill 约 6–10 分钟；给 CI 30 分钟超时。
- ptrace 被禁时 `BLOCKED`；不要把 `BLOCKED` 写成 `PASS`，也不要用 Plan B 的采样替代。

## 11. 参考

- 事务顺序与 hooks： `crates/wxml-delivery/src/native_install.rs:432`、`:510`、`:528`、`:548`、
  `:556`、`:575`、`:587`、`:600`、`:623`、`:635`、`:647`、`:659`；rollback `:875`、`:936`；
  恢复 `:1125`、`:1182`、`:1288`、`:1332`；失败处理 `:1568`、`:1659`、`:1700`。
- `switch`/lock： `crates/wxml-delivery/src/install.rs:146`、`:372-413`。
- journal/state： `crates/wxml-delivery/src/state.rs:348-355`、`:494-502`、`:532-554`。
- `atomic_write`： `crates/wxml-delivery/src/manifest.rs:727-764`。
- CLI `NoHooks`： `crates/wxml-delivery/src/bin/wxml-lsp-install.rs:397-401`、`:519-525`；
  `recover`/`verify`/`current`/`env`：`:469`、`:759`、`:715`、`:814`。
- 现有 seam 证据： `crates/wxml-delivery/tests/f3_acceptance/transactions.rs:45-199`；
  ` crates/wxml-delivery/tests/f3_acceptance/legacy.rs:28-437`；真实 N1→N2→N1：
  ` crates/wxml-delivery/tests/f3_real_release/runtime.rs:325-514`。
- ptrace 能力： `crates/wxml-delivery/tests/f3_exec_audit/tracer.rs:33-87`、`:434-468`、`:586-601`；
  ` crates/wxml-delivery/tests/f3_exec_audit/support.rs:46-96`；F3-02 证据：
  ` docs/evidence/2026-11-20-tsgo-f3-exec-audit.md:7-15`。
- 合同： `docs/reference/tsgo-delivery-contract.md:896-912`（§5.8）、`:967-988`（§6.3）、
  `:992-999`（I2/I3）、`:1229`（F3-06）、`:1232`（F3-09）、`:1334`/`:1337`（仍缺）、
  `:1350`（13 closed / 2 partial / 1 blocked）。
- 现有 acceptance： `docs/evidence/2026-11-19-tsgo-f3-acceptance.md:112-128`、`:139-154`、
  `:249-254`；构建/运行： `scripts/f3-acceptance.sh:90-140`、`scripts/f3-real-release.sh:120-145`。
