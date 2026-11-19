# tsgo F3-02 干净环境/进程审计证据（2026-11-18/19）

日期：2026-11-18（审计跨至 2026-11-19）。
状态：**BLOCKED**。本次审计执行了真实资产的安装与干净环境验证，但 **F3-02 不能关闭**。
审计工作区：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`。
审计起始 HEAD：`87fedc5`（任务指定）；审计过程中另有会话连续提交，观测到 `6ab1bc6`、`8490a92`。
最终观测 HEAD：`8490a92`（`test(delivery): verify real native asset installation`，2026-11-19 08:35:36 +0800）。
证据等级：**E1**。命令、退出码和关键输出内嵌本文；原始日志在仓库外
`/tmp/opencode/f3-clean-env/logs/`，但不是唯一证据来源。
写入边界：本次只新增本文件；未修改生产代码、现有测试、ADR/plan/contract、Cargo 文件；
未提交；未删除 Node；未切换默认 backend；未访问网络。
并发说明：审计期间另一会话在 worktree 内提交并把 `crates/wxml-lsp` 的后续改动写入工作区；
`crates/wxml-delivery` 的生产源码在 `87fedc5..8490a92` 内没有变化，新增的只有
`crates/wxml-delivery/tests/real_asset.rs` 与其 `support.rs`。本文所有安装器证据针对
未变化的 delivery 源码；LSP 相关结论针对 `8490a92` 可观测到的 `main.rs/lib.rs`。

## 0. 结论摘要

**F3-02：BLOCKED。** 真实引擎安装、干净 PATH、绝对路径 Node 掩蔽、网络命名空间隔离和
fixture 级 install/upgrade/rollback/recover 均有实测证据；但合同要求的进程级
`execve/execveat` 审计没有执行（主机没有 `strace` 或经认可的等价追踪器），并且当前
`wxml-lsp --stdio` 没有任何原生后端接线，无法产生“stdio LSP 会话只 exec
`wxml-lsp`/`engine/lib/tsc`”的证据。合同的 F3-02 验收文件和脚本也不存在。

| 检查项 | 判定 | 依据 |
| --- | --- | --- |
| 固定资产 SHA-256/SRI/大小 | **PASS** | §3.1，实测 SHA-256 与 manifest 常量一致 |
| 引擎身份/版本 `7.1.0-beta` | **PASS** | `lib/tsc --version` 输出 `Version 7.1.0-beta` |
| 干净 PATH 无 node/npm/npx | **PASS（辅助）** | §3.2，7 个名字全部 `ABSENT` |
| PATH tripwire 正控 | **PASS（辅助）** | shim 退出 99 并留下 marker |
| 绝对路径 `/usr/bin/node` 掩蔽 | **PASS（仅该路径）** | 命名空间内 `/usr/bin/node --version` 命中 shim，退出 99 |
| 真实引擎 install/verify/current/env | **PASS（辅助）** | §3.3，真实资产安装测试通过，无 PATH Node |
| 网络命名空间隔离 | **PASS（隔离级，非 syscall）** | `unshare -rmn`，`ip -brief addr` 仅 `lo DOWN` |
| 业务目录只读挂载 | **PASS（挂载级，非 syscall）** | 仓库与 `/home` 在 mount ns 内 `ro` |
| fixture install/upgrade/rollback/recover | **PASS（fixture engine）** | §3.4，CLI 流程与 recover 测试通过 |
| `execve/execveat` 原始进程审计 | **BLOCKED** | 无 `strace`；未用 PATH shim 代替 |
| 原生 stdio LSP 会话 | **BLOCKED** | `main.rs` 只构造 `NodeBackend`，`backend_tsgo` 未被接线 |
| 真实引擎 upgrade/rollback | **PARTIAL/BLOCKED** | 只有单版本真实资产包；双版本测试用 shell fixture |
| F3-02 验收测试/脚本 | **BLOCKED** | 两个 `new` 路径均不存在 |

## 1. 审计范围与环境

### 1.1 主机

```text
Linux deb 7.2.15+deb14-amd64 #1 SMP PREEMPT_DYNAMIC Debian 7.2.15-1 (2026-11-01) x86_64
ldd (Debian GLIBC 2.43-7) 2.43
```

### 1.2 追踪工具探测

```text
$ command -v strace
(空)                                  # exit 1
$ command -v ltrace perf sysdig fatrace bpftrace auditctl
(全部为空)
$ command -v gdb
/usr/bin/gdb
$ strace -V
bash: strace: 未找到命令               # exit 127
```

主机只有 `gdb`，没有 `strace`。本次**没有**把 gdb 脚本、PATH shim、`bwrap` 或
`LD_PRELOAD` 当作 F3-02 的进程审计替代品；合同要求 `strace -f -e
trace=execve,execveat` 或等价机制（` docs/reference/tsgo-delivery-contract.md:1143`），
并明确“PATH shim 只作辅助，不能单独证明”（` docs/reference/tsgo-delivery-contract.md:1145`）。
因此 §3 中的 PATH/绝对路径证据只能算辅助。

### 1.3 资产与引擎

使用任务给定的本地固定资产，未下载：

```text
/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz
sha256  59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855
size    10175135

/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc
sha256  a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70
file    ELF 64-bit LSB executable, x86-64, statically linked, Go BuildID...
$ env -i PATH=/usr/bin:/bin LANG=C LC_ALL=C .../lib/tsc --version
Version 7.1.0-beta                    # exit 0
```

manifest 固定值与实测一致：` crates/wxml-delivery/src/manifest_v2.rs:41`（版本）、
` crates/wxml-delivery/src/manifest_v2.rs:49`（SRI）、
` crates/wxml-delivery/src/manifest_v2.rs:51`（SHA-256）、
` crates/wxml-delivery/src/manifest_v2.rs:54`（大小）。

### 1.4 被测二进制

worktree 内已有 debug 二进制，未在 worktree 内重建：

```text
target/debug/wxml-lsp-install  sha256 b0db8e1a0aa5b8cf4b8ee2befdead1c7a8c06d6972ba7042b196788a51992837
                                mtime 2026-11-19 07:12:13 +0800, 66643040 B
target/debug/wxml-lsp          sha256 6b82c3bbe584e650246a666aff9ccb80fbc61a4128e620986228495bbd71872b
                                mtime 2026-11-19 08:32:00 +0800, 64923584 B
```

`target/debug/wxml-lsp-install` 的 mtime 晚于 `crates/wxml-delivery/src` 中最新源文件
（`state.rs` 07:11:50），仅用于核对 CLI 用法。安装器测试使用私有 target 重新构建的
二进制（同一 delivery 源码）：

```text
CARGO_TARGET_DIR=/tmp/opencode/f3-clean-env/target
/tmp/opencode/f3-clean-env/target/debug/wxml-lsp-install
sha256 9424487e0bf2a9a339e7633548099304b77426993b222f06c3502e63d161685c
mtime  2026-11-19 08:33:24 +0800, 15082408 B
```

构建命令（离线、2 并发、私有 target、去除 debug 信息以控制临时盘占用）：

```bash
CARGO_TARGET_DIR=/tmp/opencode/f3-clean-env/target CARGO_BUILD_JOBS=2 \
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_INCREMENTAL=false CARGO_NET_OFFLINE=true \
cargo test -p wxml-delivery --test real_asset --no-run
# exit 0, Finished in 29.32s
CARGO_TARGET_DIR=/tmp/opencode/f3-clean-env/target ... \
cargo test -p wxml-delivery --test native_cli --no-run
# exit 0, Finished in 0.51s
```

## 2. 合同要求与关闭条件

F3-02 的验收内容是：`install`/`upgrade`/`rollback` 与 stdio LSP 会话均无 node/npm 子进程，
只 exec `wxml-lsp-install`/`wxml-lsp`/`engine/lib/tsc`（及测试驱动），见
` docs/reference/tsgo-delivery-contract.md:1155`。证据要求明确包含
“进程审计原始日志（execve/execveat）与网络审计”，见
` docs/reference/tsgo-delivery-contract.md:1178`。合同还规定：
缺少 `strace`/原生资产/构建工具时，测试标记 **BLOCKED**，不得算通过或 skip
（` docs/reference/tsgo-delivery-contract.md:1145`）；当前 F3-01～F3-16 仍全部为
planned（` docs/reference/tsgo-delivery-contract.md:1185`）。

`real_asset.rs` 文件头也自述：它只证明没有 **PATH 解析**的 Node，不证明 F3-02，不做
`strace`/execve 审计，也不关闭任何 F3 ID（` crates/wxml-delivery/tests/real_asset.rs:21`）。

## 3. 已执行证据

所有命令从 worktree 根执行；`B=/tmp/opencode/f3-clean-env`，日志在该目录 `logs/`。

### 3.1 资产身份与引擎版本

```bash
sha256sum /tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz
stat -c '%s' /tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz
env -i PATH=/usr/bin:/bin LANG=C LC_ALL=C \
  /tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc --version
```

结果：SHA-256 `59128685...1855`，大小 `10175135`，输出 `Version 7.1.0-beta`，exit 0。
日志：`00-tarball-sha256.log`、`01-engine-version.log`。

### 3.2 干净 PATH

`$B/bin` 只放系统工具符号链接（`sh/bash/env/ls/cat/.../python3`），**不放** node、npm、
npx、pnpm、yarn、bun：

```bash
env -i PATH="$B/bin" LANG=C LC_ALL=C "$B/bin/sh" -c \
  'for n in node nodejs npm npx pnpm yarn bun; do
     command -v "$n" >/dev/null 2>&1 && echo "FOUND $n" || echo "ABSENT $n"; done'
```

结果：7 个名字全部 `ABSENT`。日志：`05-clean-path-check.log`。

Node tripwire（记录尝试并退出 99）正向控制：

```bash
PATH="$B/node-shims:$B/bin" "$B/bin/sh" -c 'node --version'
# exit 99; marker $B/logs/NODE_WAS_EXECUTED 创建; 记录 "node --version"
```

绝对路径绕过验证（证明 PATH shim 单独不足）：

```bash
/usr/bin/node --version
# v24.22.2, exit 0
```

日志：`06-tripwire-positive-control.log`、`07-absolute-node-bypass.log`。

绝对路径掩蔽：在 `unshare -rmn` 的 mount namespace 中把 tripwire 绑到 `/usr/bin/node`，
再执行 `/usr/bin/node --version`，命中 shim，exit 99；`/usr/bin/nodejs` 也因指向
`/usr/bin/node` 而命中。日志：`08-bwrap-absolute-mask.log`。

工具备注：先用过 `bwrap 0.13.1`，其文件掩蔽对 shell 生效，但 Rust
`std::process::Command::spawn` 在该 bwrap 沙箱内对 `/bin/sh` 也返回 `EACCES`（即使不做
Node 掩蔽）；改用 `unshare -rmn` + bind mount 后 Rust 测试正常。该失败保留在
`09-real-asset.log` 开头，不当作通过证据。

### 3.3 真实引擎安装（干净 PATH + 无网络 + 业务目录只读）

测试：`installs_pinned_real_engine_without_path_node`
（` crates/wxml-delivery/tests/real_asset.rs:182`）。测试自身用 tripwire PATH 启动
`wxml-lsp-install`，并在安装后校验完整树、`current`、manifest、引擎 `--version`、
`verify/current/env` 视图，且断言 tripwire 未被触发。

外层隔离命令（省略的长参数见日志）：

```bash
unshare -rmn sh -c '
  mount --bind /home /home; mount -o remount,ro,bind /home
  mount --bind "$R" "$R";  mount -o remount,ro,bind "$R"
  mount --bind "$B/node-shims/node" /usr/bin/node
  ip -brief addr
  env -i PATH="$B/bin" HOME="$B/home" TMPDIR="$B/tmp" LANG=C LC_ALL=C NO_COLOR=1 \
    WXML_TEST_TMPDIR="$B/tmp" \
    TSGO_NATIVE_TARBALL=/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz \
    WXML_REQUIRE_REAL_ENGINE=1 \
    "$TB" installs_pinned_real_engine_without_path_node --nocapture --test-threads=1
'
```

结果（exit 0）：

```text
NET:
lo               DOWN
MASK:
-rwxr-xr-x ... /usr/bin/node    # tripwire 脚本
REPO-MOUNT:
ro,relatime,errors=remount-ro
real asset verified: sha256=59128685...1855 ... extractedFiles=122 extractedBytes=28450796
upstream package: files=122 unpacked=28450796 engineSha256=a183b195...faaa70 engineSize=24522912
installed engine output: Version 7.1.0-beta
installed real engine without PATH-resolved Node: version=7.1.0-beta files=154 \
  engineSha256=a183b195...faaa70 installRootMode=700
test result: ok. 1 passed; 0 failed
node-attempts: none
marker: absent
```

同一隔离方式完整跑 `real_asset` 四个测试（含资产身份负例、篡改负例、tripwire 正控）：

```text
test result: ok. 4 passed; 0 failed; 0 ignored
```

日志：`10-real-asset-install-clean.log`、`09-real-asset.log`。`/usr/bin/node` 被掩蔽期间
无任何绝对路径 Node 执行记录；测试内部 PATH tripwire 也未触发。

**边界**：这只证明该测试覆盖的安装路径没有 PATH 解析的 Node、没有执行被掩蔽的
`/usr/bin/node`。它不证明其他绝对路径（例如其他目录下的 node 副本）、库内部 `execve`
或 `fork/exec` 未发生；后者需要 syscall 追踪。

### 3.4 fixture 级 CLI 全流程与 recover

测试：`installs_native_verify_upgrade_rollback_env_without_node`
（` crates/wxml-delivery/tests/native_cli.rs:404`）与
`recover_keeps_a_committed_target_inactive_and_cleans_staged_state`
（` crates/wxml-delivery/tests/native_cli.rs:654`）。同样的 `unshare -rmn` + 仓库只读 +
`/usr/bin/node` 掩蔽 + 干净 PATH 下执行：

```text
installs_native_verify_upgrade_rollback_env_without_node ... ok
test result: ok. 1 passed; 0 failed   # exit 0
abs-node-attempts: none

recover_keeps_a_committed_target_inactive_and_cleans_staged_state ... ok
test result: ok. 1 passed; 0 failed   # exit 0
abs-node-attempts: none
```

该流程覆盖 `install`、`verify`、`current`、`env`、`upgrade`、`rollback`、`versions`、
`verify-package`、`list`。**局限**：这套测试的 `engine/lib/tsc` 是测试自己生成的 shell
fixture，不是真实 7.1.0-beta 引擎；它证明 CLI 状态机在无 Node 下工作，不能替代真实引擎的
upgrade/rollback 进程审计。日志：`12-native-cli-flow-clean.log`、`13-native-recover-clean.log`。

真实引擎的 `verify/current/env` 由 §3.3 的 `assert_cli_views` 覆盖；真实引擎的
upgrade/rollback 未执行，原因见 §4（没有双版本真实 v2 包/生产 packager）。

### 3.5 网络与写入边界

- 网络：`unshare -rmn` 新 net namespace 中 `ip -brief addr` 仅 `lo DOWN`；`real_asset`
  测试还把 `HTTP_PROXY/HTTPS_PROXY/ALL_PROXY` 指向 `127.0.0.1:9`。没有外部网络能力。
- 写入：mount namespace 内 `/home` 与 worktree 递归 bind 后 remount 为 `ro`；测试只写
  `WXML_TEST_TMPDIR` 和 `$B/logs`。`git status --porcelain` 在审计前后没有本审计造成的
  变化（并发会话自己的改动除外）。

**边界**：这是命名空间/挂载级隔离，不是 `connect`/`openat(O_WRONLY)` syscall 审计；
不能证明“没有尝试连接”或“没有尝试写入”，只能证明这些尝试在隔离下不能成功。

### 3.6 stdio LSP 会话

当前 HEAD 的 `wxml-lsp` 只有 Node 后端接线：

- ` crates/wxml-lsp/src/main.rs:96`：`NodeBackend::new(...)`；
- ` crates/wxml-lsp/src/main.rs:100`：`run_stdio_with_host(backend, host)`；
- ` crates/wxml-lsp/src/lib.rs:46`：`pub(crate) mod backend_tsgo;`，除模块声明外
  `crates/wxml-lsp/src` 没有对 `backend_tsgo` 的引用；
- ` crates/wxml-lsp/src/backend_node/process.rs:258`：Node 后端按需
  `Command::new(&config.node)`。

因此不存在“stdio LSP 会话由原生引擎提供”的可执行路径；本次无法在 LSP 层执行
`engine/lib/tsc`，也无法生成 F3-02 所需的 LSP 进程树证据。直接调用
`wxml-backend-tsgo` 的测试驱动不属于 stdio LSP 会话，不能替代。

## 4. 残余缺口（F3-02 不能关闭的原因）

1. **B1 — 没有 execve 进程审计。** 主机缺 `strace`/`ltrace`/`perf`/`bpftrace`/
   `auditctl`；未用 PATH shim、bwrap、gdb 或自写 ptrace 追踪器替代。缺少
   `install`/`upgrade`/`rollback`/`recover` 的 `execve/execveat` 原始日志，无法证明
   “只 exec 允许列表”以及没有调用绝对路径 Node。
2. **B2 — stdio LSP 原生会话不存在。** `wxml-lsp --stdio` 仍构造 `NodeBackend`；
   `backend_tsgo` 是 `pub(crate)` 且未被 server/project_host/main 引用。F3-02 的 LSP
   一半在当前 HEAD 不可执行；需要先有原生后端接线或一个明确批准的原生 LSP 测试入口。
3. **B3 — 验收产物缺失。** `crates/wxml-delivery/tests/no_node_guard.rs` 和
   `scripts/verify-no-node.sh` 都不存在（F3-02 合同标注为 `new`）。合同 §9.4 仍把
   F3-01～F3-16 记为 planned。
4. **B4 — 真实引擎 upgrade/rollback 未审计。** 真实资产 harness 只构造并安装单版本
   `0.1.0`，测试结束即删除 fixture；双版本 upgrade/rollback 测试使用 shell fixture
   engine。缺少真实 `engine/lib/tsc` 的双版本安装包（或生产 v2 packager）来执行
   upgrade/rollback 的真实进程审计。
5. **B5 — 网络/写入只有隔离级证据。** `unshare -rmn` + 只读 bind mount 证明操作可在
   无网络、业务目录只读下成功，但没有 `connect`/`openat` syscall 日志，不能满足 §9.3
   的“网络审计”原始日志要求。
6. **B6 — HEAD 在审计中移动。** 起始 `87fedc5`，期间出现 `6ab1bc6`、`8490a92`。
   `crates/wxml-delivery` 生产源码未变，但真实资产测试在 08:35 被提交；若关闭门禁需要
   固定 commit，应在单一固定 HEAD 上重跑并记录。

## 5. 复跑命令与日志

关键日志（仓库外）：

```text
/tmp/opencode/f3-clean-env/logs/00-tarball-sha256.log
/tmp/opencode/f3-clean-env/logs/01-engine-version.log
/tmp/opencode/f3-clean-env/logs/02-binaries-sha256.log
/tmp/opencode/f3-clean-env/logs/03-build-real-asset.log
/tmp/opencode/f3-clean-env/logs/04-build-native-cli.log
/tmp/opencode/f3-clean-env/logs/05-clean-path-check.log
/tmp/opencode/f3-clean-env/logs/06-tripwire-positive-control.log
/tmp/opencode/f3-clean-env/logs/07-absolute-node-bypass.log
/tmp/opencode/f3-clean-env/logs/08-bwrap-absolute-mask.log
/tmp/opencode/f3-clean-env/logs/09-real-asset.log
/tmp/opencode/f3-clean-env/logs/10-real-asset-install-clean.log
/tmp/opencode/f3-clean-env/logs/11-tripwire-positive-control-clean.log
/tmp/opencode/f3-clean-env/logs/12-native-cli-flow-clean.log
/tmp/opencode/f3-clean-env/logs/13-native-recover-clean.log
/tmp/opencode/f3-clean-env/logs/14-static-checks.log
```

`14-static-checks.log` 记录 `strace`/`ltrace`/`perf` 等缺失、`backend_tsgo` 无接线、
两个验收文件缺失。所有操作未下载、未访问外部网络、未修改 Node 安装。

## 6. 边界声明

- 本文件是本次唯一的仓库写入；未提交。
- 未修改生产代码、现有测试、ADR、plan、contract、Cargo 文件。
- 未删除 Node，未切换默认 backend，未启用下载。
- 未关闭任何其他 F3 ID；F3-02 本身也未关闭。
- 本文不把 fixture 测试、PATH shim、隔离挂载或 `real_asset` 测试头自述的“不证明 F3-02”
  扩大为 F3-02 通过。

## 7. 最终判定

**F3-02 = BLOCKED。** 可复现的真实资产安装和干净环境证据已经取得，但关闭条件仍缺：

1. `strace -f -e trace=execve,execveat`（或经批准的等价机制）对
   `install`/`upgrade`/`rollback`/`recover` 与原生 stdio LSP 会话的原始进程日志；
2. 可执行的**原生** stdio LSP 接线（当前只有 `NodeBackend`）；
3. `crates/wxml-delivery/tests/no_node_guard.rs` 与 `scripts/verify-no-node.sh`；
4. 真实 7.1.0-beta 引擎的双版本 upgrade/rollback 证据；
5. 固定 commit 下的网络/写入 syscall 审计。

在上述证据补齐前，不能把 F3-02 标记为 closed/pass。
