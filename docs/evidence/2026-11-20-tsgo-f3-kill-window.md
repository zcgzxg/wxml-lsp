# F3-06/F3-09 生产 CLI 精确 SIGKILL 窗口（2026-11-20）

状态：**test-only 实现 + 真实运行完成**。生产 `wxml-lsp-install` 在真实升级与 legacy 迁移
的精确 `symlink`/`rename` 边界被**外部 SIGKILL**；杀前磁盘状态由父进程断言，杀后用真实
`recover`/`verify`（必要时显式重试）验证。

| 项 | 本次结论 | 说明 |
| --- | --- | --- |
| F3-06 进程级 SIGKILL（U1/U3/U5/U6/U5-durable） | **PASS（Linux x86_64）** | 5 个真实窗口 + never-match 控制项均通过 |
| F3-06 整体 | **PARTIAL** | “第二个真实 release”仍缺：N2 是 N1 生产二进制同长度版本常量重标，不是独立发布产物 |
| F3-09 进程级 SIGKILL（L3/L4） | **PASS（Linux x86_64）** | legacy 迁移切换前/后；旧树字节不变、legacy marker 未执行、`previous=null` |
| F3-09 整体 | **PARTIAL** | “真实 legacy 发布包”仍缺：legacy current 是生产 v1 packager 生成的 fixture |
| 合同 §6.3 第 975 行“切回”语义 | **未裁决，记录实测** | U5（`current=N2`、`state=N1`）被 SIGKILL 后 `recover` **前滚**到 N2，符合第 976 行；进程已死，无法执行切回 |

本切片不替评审做 provenance/语义裁决，也不把 `BLOCKED`/`SKIP` 写成 `PASS`。

## 1. 改动边界

新增（未提交）：

- ` crates/wxml-delivery/tests/f3_kill_window.rs`
- ` crates/wxml-delivery/tests/f3_kill_window/tracer.rs`
- ` crates/wxml-delivery/tests/f3_kill_window/gate.rs`
- ` crates/wxml-delivery/tests/f3_kill_window/support.rs`
- ` crates/wxml-delivery/tests/f3_kill_window/real.rs`
- ` scripts/f3-kill-window.sh`
- 本文件

未修改：`crates/wxml-delivery/src/**`、`crates/wxml-delivery/Cargo.toml`、workspace
`Cargo.toml`/`Cargo.lock`、`crates/wxml-delivery/tests/f3_exec_audit*`、其他会话文件。

工作树身份：

- 任务起始 HEAD `8ccebbf`；最终真实运行期间并发会话推进到 `04fa0fe`
  （`1102da7` build pipeline、`375cde2` native gates、`04fa0fe` component bindings）。
- `8ccebbf..04fa0fe` 对 `crates/wxml-delivery/` 只改了 `README.md`（18+/12-），
  **delivery 生产源码与合同审计基线一致**；release 二进制构建于起始 `8ccebbf`，
  之后三个提交未改 delivery 源码，因此运行二进制与最终 HEAD 的 delivery 源码同源。
- 运行结束后并发会话又推进到 `2f072a6`（仅 tsgo native gate 测试），
  `crates/wxml-delivery/` 与 `Cargo.toml`/`Cargo.lock` 仍未变化。
- 工作树中 `crates/wxml-lsp/**` 与 `docs/plans/*` 的未提交改动属于其他会话，本切片未编辑。

## 2. 方法

test-only `ptrace` syscall-stop tracer，无新增 Cargo 依赖：

1. fork；子进程 `setpgid(0,0)`、`PTRACE_TRACEME`、`raise(SIGSTOP)`，然后 `execve`
   真实 `wxml-lsp-install`（绝对路径、clean env）。
2. 父进程 `PTRACE_SETOPTIONS` 打开 `PTRACE_O_TRACESYSGOOD | PTRACE_O_EXITKILL`，
   以 `PTRACE_SYSCALL` 放行；**只跟踪根进程**，安装器探针子进程不被跟踪。
3. 每个 stop 用 `PTRACE_GETREGS` 读 `orig_rax`；entry 以 x86_64 的 `rax == -ENOSYS`
   判定，exit 为同一 syscall 的返回 stop。路径用 `PTRACE_PEEKDATA` 读取
   （`rename`/`symlink` 直接参数；`renameat`/`renameat2`/`symlinkat` 解析
   `dirfd` + `/proc/<pid>/{cwd,fd}`）。
4. gate = syscall 家族 + entry/exit + 路径谓词 + 杀前磁盘谓词
   （`current`、`state.json`、`txn/*.json`、`versions/`、`.current.tmp-*`）。
   路径命中但磁盘谓词不符 → 记录 rejected candidate，测试最终失败；不杀、不假绿。
5. 命中后先记录 `pid/gate/paths/pre`，再 `kill(pid,SIGKILL)` + `kill(-pid,SIGKILL)`，
   最后断言 `WIFSIGNALED && WTERMSIG==9`。
6. 唯一路径 gate（`symlink`/`rename`）要求**恰好 1 个 candidate 且磁盘谓词成立**；
   `fsync(root)` gate 允许同一路径在事务不同阶段重复出现（磁盘谓词选窗口），但要求
   **恰好 1 个 hit**、0 个 rejected candidate。被杀进程 stdout/stderr 不得出现
   `upgraded to`/`migrated to`/`rolled back to`/`installed `。
7. preflight：用 `PTRACE_SYSCALL` 跑 `/bin/true`，必须观察到 ≥1 次 syscall stop。
8. 控制项：never-match gate 跑同一真实升级，CLI 必须成功、0 命中、无杀。

本机观察到的实际 syscall 号：`rename(82)`、`symlink(88)`、`fsync(74)`；fixture 中的
`mv` 使用 `renameat2(316)`，at-family 路径解析由该 fixture 覆盖。

## 3. 环境与输入

| 项 | 值 |
| --- | --- |
| host | Linux `7.2.15+deb14-amd64` x86_64 |
| `/proc/sys/kernel/yama/ptrace_scope` | `0` |
| rustc | `1.100.0 (0ef1bc386 2026-11-09)` |
| worktree HEAD（最终运行） | `04fa0fe`（起始 `8ccebbf`；delivery 源码自 `8ccebbf` 未变） |
| lock | `assets.lock.json` SHA-256 `ce33fa96…725456` |
| engine tarball | `/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz` SHA-256 `59128685…d51855` |
| typings tarball | `/tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz` SHA-256 `e5f8b50f…4b71` |
| LSP 二进制（真实包） | `/tmp/opencode/tsgo-native-pkg/bin/wxml-lsp` SHA-256 `ae510e06…d710e` |
| installer 二进制 | `/tmp/opencode/f3-kill-window/target/release/wxml-lsp-install` SHA-256 `ab74ddce…984f2` |
| Zed wasm | `/tmp/opencode/f3-kill-window/zed-target/wasm32-wasip2/release/zed_wxml.wasm` SHA-256 `488531ea…aeb1f` |
| N1 archive（本次构建） | SHA-256 `9c2d425f…6679d`，size 30,961,378 |
| N2 archive（本次构建） | SHA-256 `9a398d1d…aebc6d1`，size 30,961,381 |
| 真实/仿真 | N1/N2 由生产 packager 从 committed lock + 真实 tarball 构建；N2 仅重标同长度版本常量；legacy current 由生产 v1 packager 生成（`FIXTURE`） |

网络隔离：真实模式在 `unshare -rn`（新网络命名空间，lo DOWN）中执行；安装器仅用
`--from-dir`/`--offline`。

## 4. 运行命令与结果

```bash
scripts/f3-kill-window.sh --skip-build --network-isolate
# 等价手工命令：
TSGO_NATIVE_TARBALL=... TSGO_TYPINGS_TARBALL=... \
WXML_F3_LSP_BIN=/tmp/opencode/tsgo-native-pkg/bin/wxml-lsp \
WXML_F3_INSTALL_BIN=/tmp/opencode/f3-kill-window/target/release/wxml-lsp-install \
WXML_F3_ZED_WASM=.../zed_wxml.wasm \
WXML_REQUIRE_REAL_ENGINE=1 WXML_REQUIRE_KILL_WINDOW=1 \
WXML_TEST_TMPDIR=/tmp/opencode/f3-kill-window/tmp \
  cargo test -p wxml-delivery --test f3_kill_window -- \
    --ignored --nocapture --test-threads=1
```

- non-real（默认模式）：`3 passed; 0 failed; 8 ignored`（0.02s）
  - `TRACER-PASS`：preflight `/bin/true` 观察到 syscall stop；
  - `FIXTURE`：合成 `mv` 在 `renameat2(316)` exit 被杀，never-match 控制项完整跑完；
  - `SKIP`：真实窗口为 `#[ignore]` opt-in。
- real（`--ignored`，`unshare -rn`）：`8 passed; 0 failed; 0 ignored`，111.13s。
- `cargo fmt --all --check`：通过。
- `cargo clippy -p wxml-delivery --all-targets -- -D warnings`：通过。
- diff-check：`git status` 中本切片只有上述新增文件；`crates/wxml-delivery/src`、
  `Cargo.toml`、`Cargo.lock`、`f3_exec_audit*` 无改动。`git diff` 中的已跟踪改动全部是
  其他会话的 `crates/wxml-lsp/**` 与 `docs/plans/*`。

日志（仓库外）：

- `/tmp/opencode/f3-kill-window/logs/f3-kill-window-nonreal.log`
- `/tmp/opencode/f3-kill-window/logs/f3-kill-window-real.log`
- `/tmp/opencode/f3-kill-window/logs/fmt-check.log`、`clippy.log`

## 5. 真实窗口结果

所有窗口均记录到 `sig=9`，杀前 `pre` 由父进程在 tracee 停止时读取；杀后断言的
`current`/`state`/`journal`/`versions` 与下表一致。

| ID | F3 | gate（实际） | 杀前磁盘状态（父进程断言） | 杀后立即 | `recover` / 后续 | 结论 |
| --- | --- | --- | --- | --- | --- | --- |
| U1 | F3-06 | `rename(tmp/native-*/tree, versions/0.2.0)` exit | `current=0.1.0`、`state=active:0.1.0/previous:null`、`journal=staged(0.2.0<-0.1.0)` | `current=0.1.0`；state 字节不变；journal staged | `staged 0.2.0 -> target exists; marked committed; not auto-activated`；显式 `upgrade` → 0.2.0 | **PASS** |
| U3 | F3-06 | `symlink(versions/0.2.0, .current.tmp-*)` **entry** | `current=0.1.0`、`state=active:0.1.0/previous:null`、`journal=committed(0.2.0<-0.1.0)`、无 `.current.tmp-*` | `current=0.1.0`；state 字节不变；journal committed | `committed 0.2.0 -> kept target; not auto-activated`；显式 `upgrade` → 0.2.0 | **PASS** |
| U5 | F3-06 | `rename(.current.tmp-*, current)` **exit** | `current=0.2.0`、`state=active:0.1.0/previous:null`、`journal=committed(0.2.0<-0.1.0)` | `current=0.2.0`（已切换）；state 仍是 N1 字节 | `0.2.0 -> verified and state written`；state `active:0.2.0/previous:0.1.0`；journal 删除 | **PASS**（前滚） |
| U5-durable | F3-06 | `fsync(root)` **exit**（switch 内目录 fsync） | `current=0.2.0`、`state=active:0.1.0/previous:null`、`journal=committed(0.2.0<-0.1.0)` | 同 U5，且 link 已 durable | `0.2.0 -> verified and state written`；state `active:0.2.0/previous:0.1.0`；journal 删除 | **PASS** |
| U6 | F3-06 | `rename(.state.json.tmp-*, state.json)` exit | `current=0.2.0`、`state=active:0.2.0/previous:0.1.0`、`journal=committed(0.2.0<-0.1.0)` | 同左；state 已是 N2 | `committed 0.2.0 -> valid state already active; journal removed`；state 字节不变 | **PASS** |
| L3 | F3-09 | `symlink(versions/0.1.0, .current.tmp-*)` **entry** | `current=0.0.9`、`state=absent`、`journal=committed(0.1.0<-null)`、`versions=[0.0.9,0.1.0]` | `current=0.0.9`、无 state；journal committed | `committed 0.1.0 -> kept target; not auto-activated`；显式 `install` → `migrated to 0.1.0 (native)`；`state=active:0.1.0/previous:null` | **PASS** |
| L4 | F3-09 | `rename(.current.tmp-*, current)` **exit** | `current=0.1.0`、`state=absent`、`journal=committed(0.1.0<-null)` | `current=0.1.0`、无 state | `0.1.0 -> verified and state written`；`state=active:0.1.0/previous:null`；`verify=0.1.0` | **PASS** |
| CONTROL | F3-06 | never-match | 全新 N1 | 无杀；7 进程正常完成 | `state=active:0.2.0/previous:0.1.0`；`verify=0.2.0` | **PASS** |

U5-durable 的 gate 在 switch 之外还会看到一次 `fsync(root)`（staged journal 之后的
`fsync_dir(root)`）；该 stop 因 `current=0.1.0`/`journal=staged` 不满足磁盘谓词被跳过，
只有 switch 内目录 fsync（`current=0.2.0`、state 仍 N1、journal committed）命中并被杀。

每个窗口额外断言：

- 被杀进程 `WTERMSIG=9`，且 stdout/stderr 无成功行；
- gate 恰好 1 次命中，0 个 rejected candidate；
- 杀后 `current` 仍可 `verify_tree_v2`（native）或按 schema v1 只读打开（legacy）；
- U1/U3/U5/U5-durable/U6 的 N1 树 fingerprint 不变；U5/U5-durable/U6 的 N2 树切换后仍可验证；
- L3/L4 的 legacy 树 fingerprint 不变、legacy marker 未执行、journal/state 的
  `previous` 从不等于 `0.0.9`；
- U1/U3/L3 重试成功后 journal 与 `tmp/` 清空；U5/U5-durable/U6/L4 recover 后 journal 清空；
- 控制项 92,823 次 stop、`fsync=376,rename=16,symlink=2`，证明 tracer 不误杀。

真实模式观测规模（单窗口）：

| 窗口 | stops | 观察到的 syscall | 耗时 |
| --- | --- | --- | --- |
| U1 | 92,648 | `fsync=358,rename=8` | 1.64s |
| U3 | 92,691 | `fsync=364,rename=10,symlink=1` | 1.38s |
| U5 | 92,694 | `fsync=364,rename=12,symlink=2` | 2.07s |
| U5-durable | 92,652 | `fsync=366,rename=12,symlink=2` | 1.33s |
| U6 | 92,718 | `fsync=368,rename=14,symlink=2` | 1.35s |
| L3 | 86,221 | `fsync=364,rename=10,symlink=1` | 1.58s |
| L4 | 86,224 | `fsync=364,rename=12,symlink=2` | 1.87s |
| CONTROL | 92,823 | `fsync=376,rename=16,symlink=2` | 1.32s |

## 6. 仍未关闭 / PARTIAL 的原因

1. **F3-06 第二个真实 release 缺失**：N2 是 N1 生产二进制的同长度版本常量重标
   （`f3_real_release/runtime.rs`），不是独立发布产物。本证据只证明崩溃恢复语义，
   不证明 N2 provenance。除非评审接受 retarget 或产出真实 N2，F3-06 仍 **PARTIAL**。
2. **F3-09 真实 legacy 发布包缺失**：legacy current 由生产 v1 packager 生成
   （`0.0.9`，带 Node entry tripwire），不是历史发布包。L3/L4 证明迁移状态机与
   “永不执行/恢复 Node”，不证明 legacy 包身份；除非评审接受或产出真实 legacy 包，
   F3-09 仍 **PARTIAL**。
3. **合同 §6.3 第 975/976 行语义**：U5 的实测是 `current=N2`、`state=N1` 被 SIGKILL 后
   `recover` 前滚到 N2（第 976 行），不是“state 仍指 previous 则切回”（第 975 行）。
   进程已死，无法执行 `handle_post_switch_failure`/`restore_previous`。若评审要求
   §6.3 第 975 行也覆盖进程崩溃，则必须修改 `recover` 或承认该子条件在 SIGKILL 场景
   不可满足；本切片不替评审裁决。
4. **断电/目录 fsync**：exit-stop 可能在目录 `fsync` 之前；本证据只证明进程崩溃，
   不证明掉电持久性（与合同 §10 一致）。
5. **孤儿 temp 卫生**：本次 7 个杀点杀前 `root-temp=[]`，杀后也未依赖清理
   `.current.tmp-*`/`.state.json.tmp-*`；U4（rename entry）未覆盖，孤儿 temp 的
   累积仍是卫生缺口，不是正确性缺口。
6. **平台**：首版只支持 Linux x86_64；aarch64/非 Linux 无 syscall-stop 证据，
   只能 `BLOCKED`。
7. **rollback kill（R1/R2）**：F3-07 已 closed；本次按任务范围只做 F3-06 upgrade 与
   F3-09 legacy，未把 rollback kill 计入关闭条件。

## 7. 复现与失败关闭

```bash
# 仅非真实（fixture/preflight）
scripts/f3-kill-window.sh --skip-build --non-real-only

# 单窗口调试
WXML_KILL_WINDOW_TRACE=1 \
  cargo test -p wxml-delivery --test f3_kill_window -- \
  --ignored --nocapture --test-threads=1 f3_06_real_cli_sigkill_upgrade_before_switch
```

失败关闭：

- `WXML_REQUIRE_KILL_WINDOW=1`、`WXML_REQUIRE_REAL_ENGINE=1` 或
  `WXML_REQUIRE_EXEC_AUDIT=1` 时，ptrace 被禁（`BLOCKED`）或真实资产缺失（`SKIP`）
  都按失败处理；
- `WXML_KILL_WINDOW_FORCE_BLOCKED=1` 可人工演练 blocked 分支；
- gate 0 命中、>1 命中、rejected candidate、`WTERMSIG != 9`、被杀进程打印成功行，
  任一都失败；脚本在出现 `SKIP`/`BLOCKED` 时输出 `PARTIAL` 而不是 `PASS`。

## 8. 参考

- 设计：` docs/evidence/2026-11-20-tsgo-f3-kill-window-plan.md`
- 生产事务：` crates/wxml-delivery/src/native_install.rs:432`、`:528`、`:548`、
  `:556`、`:575`、`:600`、`:635`、`:647`；`switch`：` crates/wxml-delivery/src/install.rs:372-413`；
  `recover_committed`：` crates/wxml-delivery/src/native_install.rs:1195`；
  `atomic_write`：` crates/wxml-delivery/src/manifest.rs:727-764`。
- 现有 seam 证据：` crates/wxml-delivery/tests/f3_acceptance/transactions.rs:45-199`、
  ` crates/wxml-delivery/tests/f3_acceptance/legacy.rs:28-437`。
- F3-02 ptrace 模式：` crates/wxml-delivery/tests/f3_exec_audit/tracer.rs:33-87`；
  ` docs/evidence/2026-11-20-tsgo-f3-exec-audit.md`。
- 合同：` docs/reference/tsgo-delivery-contract.md:967-988`（§6.3）、`:1229`（F3-06）、
  `:1232`（F3-09）、`:1334`/`:1337`（仍缺）、`:1350`（13 closed / 2 partial / 1 blocked）。
