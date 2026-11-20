# P7 删除后最终产物 exec 审计（2026-11-20）

## 0. 结论先行

**PASS。** 在 `p7-removed`（`b6c1ced`，当时 tag）的安装
`/tmp/opencode/p7-post-delete-install/versions/0.1.0` 上重跑 F3-02 ptrace
执行审计，两个真实用例均通过：

- installer `install`/`verify`/`current`/`env`：**10 个 exec，7 个不同绝对可执行文件**；
- native LSP `--stdio --backend native`：**3 个 exec，2 个不同绝对可执行文件**
  （`wxml-lsp` 与 `engine/lib/tsc`）；
- 合计抓取 **13 条 exec**，**0** 条命中
  `node`/`nodejs`/`npm`/`npx`/`yarn`/`pnpm`/`bun`、`server.js` 或
  `WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`；**13/13** 的 `exe` 与 `argv[0]`
  都是绝对路径；
- 阳性对照在本次同一主机上抓到了字面名为 `node` 的 exec，证明 tracer 非空转；
- 测试退出码 **0**：`2 passed; 0 failed; ... 4 filtered out; finished in 53.44s`。

残余：F3-02 仍只在 **Linux x86_64 + ptrace** 上成立；审计范围是
`execve`/`execveat`，不覆盖网络 `connect`、文件访问、升级/回滚和 Zed 扩展。
本审计支持 final-docs **E-01 的 LSP/Zed 进程树范围**，不支持“全机零 Node 进程”断言。

---

## 1. 身份与输入

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b` |
| Tag | `p7-removed` |
| Branch | `tsgo/migration` |
| OS / arch | Linux 7.2.15+deb14-amd64 `x86_64` |
| rustc / cargo | `1.100.0` / `1.100.0` |
| `ptrace_scope` | `0` |
| 最终安装 | `/tmp/opencode/p7-post-delete-install/versions/0.1.0` |
| `bin/wxml-lsp` | sha256 `aa8aa03b40d3445a96405079cc6ab72c8a0a342ed5292306374d9e7540fe77e2`，5,539,136 B |
| `bin/wxml-lsp-install` | sha256 `cc8070fad9a863fb6576a453d2db00af04657ca3a414c53f156bb62b574b45f5`，5,058,560 B |
| `engine/lib/tsc` | sha256 `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70`，24,522,912 B；`Version 7.1.0-beta` |
| typings | `<install>/typings/miniprogram-api-typings`，23 个文件，2,548,075 B；`index.d.ts` sha256 `a32d71d6bbc9ae904afbc1ef595d14c6f8d7c43ef1226f6718a0a0ce5069efe7` |
| 主机 Node | `/usr/bin/node`，`v24.22.2`（存在，但本次追踪树内从未 exec） |

安装包中不存在 `server.js`/`tsc.js`（`find` 无结果）；`wxml-lsp --help`
明确“There is no Node fallback”，`--version` 为 `wxml-lsp 0.1.0`。

审计输入显式绑定到该安装：

```text
WXML_F3_LSP_BIN=<install>/bin/wxml-lsp
WXML_F3_INSTALL_BIN=<install>/bin/wxml-lsp-install
WXML_F3_ZED_WASM=<install>/zed-extension/extension.wasm
TSGO_BIN=<install>/engine/lib/tsc
TSGO_ASSET_ROOT=<install>/engine
TSGO_TYPINGS_ROOT=<install>/typings/miniprogram-api-typings
```

固定资产 tarball：

```text
TSGO_NATIVE_TARBALL=/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz
TSGO_TYPINGS_TARBALL=/tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz
```

---

## 2. 命令与退出码

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
I=/tmp/opencode/p7-post-delete-install/versions/0.1.0
env -u WXML_TEST_NODE -u WXML_TEST_NATIVE -u WXML_EXEC_AUDIT_FORCE_BLOCKED \
    -u WXML_F3_REAL_CHANNEL \
  WXML_F3_LSP_BIN="$I/bin/wxml-lsp" \
  WXML_F3_INSTALL_BIN="$I/bin/wxml-lsp-install" \
  WXML_F3_ZED_WASM="$I/zed-extension/extension.wasm" \
  TSGO_BIN="$I/engine/lib/tsc" \
  TSGO_ASSET_ROOT="$I/engine" \
  TSGO_TYPINGS_ROOT="$I/typings/miniprogram-api-typings" \
  TSGO_NATIVE_TARBALL=/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz \
  TSGO_TYPINGS_TARBALL=/tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz \
  WXML_REQUIRE_EXEC_AUDIT=1 WXML_REQUIRE_REAL_ENGINE=1 \
  CARGO_NET_OFFLINE=true \
  cargo test -p wxml-delivery --test f3_exec_audit -- \
    --ignored --test-threads=1 --nocapture
```

- 退出码：**0**（`PIPESTATUS[0]=0`；日志经 `tee` 保存）。
- 结果行：`test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 53.44s`。
- 日志：`/tmp/opencode/p7-final-exec-audit.log`（52 行，11,979 B）。
- `--ignored` 只运行两条真实用例；被过滤的 4 条是非 opt-in 的阳性对照、
  fixture installer、线程 helper 和线程压力用例。
- 硬门禁 `WXML_REQUIRE_EXEC_AUDIT=1` 与 `WXML_REQUIRE_REAL_ENGINE=1`
  同时打开，因此任何 ptrace/资产缺失都会失败关闭，而不是 skip。

### 2.1 阳性对照（证明 tracer 非空转）

```bash
env WXML_REQUIRE_EXEC_AUDIT=1 WXML_REQUIRE_REAL_ENGINE=1 \
  cargo test -p wxml-delivery --test f3_exec_audit -- \
    --exact guards::positive_control_records_a_node_named_exec --nocapture
```

退出码 **0**；3 个 exec 中包含一个字面名为 `node` 的绝对路径 exec
（`.../wxml-real-asset-f3-exec-audit-control-.../node`），且审计检查把它判为
forbidden evidence。日志：`/tmp/opencode/p7-final-exec-audit-control.log`。

---

## 3. 实际 exec 列表

每条记录均由 `PTRACE_O_TRACEEXEC|TRACEFORK|TRACEVFORK|TRACECLONE|EXITKILL`
追踪整棵 fork/clone 树，`exe` 取自 `/proc/<pid>/exe`。

### 3.1 installer 四命令（10 条）

| # | 阶段 | pid / ppid | root | exe | argv | env# |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | install | 2424235 / 2421878 | true | `/tmp/opencode/p7-post-delete-install/versions/0.1.0/bin/wxml-lsp-install` | `/tmp/opencode/p7-post-delete-install/versions/0.1.0/bin/wxml-lsp-install install --root /tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root --from-dir /tmp/opencode/wxml-real-asset-f3-acceptance-shared-2421878-0-1795139220369898911/channel --version 0.1.0 --offline` | 12 |
| 2 | install | 2424269 / 2424235 | false | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root/tmp/native-2424235-1795139271220336224/tree/engine/lib/tsc` | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root/tmp/native-2424235-1795139271220336224/tree/engine/lib/tsc --version` | 2 |
| 3 | install | 2424278 / 2424235 | false | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root/tmp/native-2424235-1795139271220336224/tree/bin/wxml-lsp` | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root/tmp/native-2424235-1795139271220336224/tree/bin/wxml-lsp --version` | 2 |
| 4 | install | 2424282 / 2424235 | false | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root/tmp/native-2424235-1795139271220336224/tree/bin/wxml-lsp-install` | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root/tmp/native-2424235-1795139271220336224/tree/bin/wxml-lsp-install --version` | 2 |
| 5 | verify | 2424286 / 2421878 | true | `/tmp/opencode/p7-post-delete-install/versions/0.1.0/bin/wxml-lsp-install` | `/tmp/opencode/p7-post-delete-install/versions/0.1.0/bin/wxml-lsp-install verify --root /tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root` | 12 |
| 6 | verify | 2424289 / 2424286 | false | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root/versions/0.1.0/engine/lib/tsc` | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root/versions/0.1.0/engine/lib/tsc --version` | 2 |
| 7 | verify | 2424298 / 2424286 | false | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root/versions/0.1.0/bin/wxml-lsp` | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root/versions/0.1.0/bin/wxml-lsp --version` | 2 |
| 8 | verify | 2424301 / 2424286 | false | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root/versions/0.1.0/bin/wxml-lsp-install` | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root/versions/0.1.0/bin/wxml-lsp-install --version` | 2 |
| 9 | current | 2424308 / 2421878 | true | `/tmp/opencode/p7-post-delete-install/versions/0.1.0/bin/wxml-lsp-install` | `/tmp/opencode/p7-post-delete-install/versions/0.1.0/bin/wxml-lsp-install current --root /tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root` | 12 |
| 10 | env | 2424312 / 2421878 | true | `/tmp/opencode/p7-post-delete-install/versions/0.1.0/bin/wxml-lsp-install` | `/tmp/opencode/p7-post-delete-install/versions/0.1.0/bin/wxml-lsp-install env --root /tmp/opencode/wxml-real-asset-f3-exec-audit-real-install-2421878-1-1795139271216288677/install-root` | 12 |

阶段统计：

```text
install  execs=4 forks=15 exit=0 timed_out=false survivors=[] 1.060s
verify   execs=4 forks=15 exit=0 timed_out=false survivors=[] 0.273s
current  execs=1 forks=0  exit=0 timed_out=false survivors=[] 0.004s
env      execs=1 forks=0  exit=0 timed_out=false survivors=[] 0.004s
```

`install` 的 engine exec 路径是该次安装的 staging tree；`verify` 的 engine exec
路径是安装后的 `versions/0.1.0/engine/lib/tsc`；测试断言安装树 engine 的 sha256
与 `TSGO_BIN` 完全一致。installer 的 root exec 是最终安装中的
`bin/wxml-lsp-install`（不是 cargo 测试临时构建的二进制）。

### 3.2 native LSP 会话（3 条）

包：`.../wxml-real-asset-f3-exec-audit-real-lsp-2421878-2-1795139273458016152/native-package`
（由最终安装 `bin/wxml-lsp`、`manifest.json`、`engine/`、`typings/` 逐字节复制）。

| # | pid / ppid | root | exe | argv | env# |
| --- | --- | --- | --- | --- | --- |
| 1 | 2424330 / 2421878 | true | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-lsp-2421878-2-1795139273458016152/native-package/bin/wxml-lsp` | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-lsp-2421878-2-1795139273458016152/native-package/bin/wxml-lsp --stdio --backend native` | 12 |
| 2 | 2424335 / 2424330 | false | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-lsp-2421878-2-1795139273458016152/native-package/engine/lib/tsc` | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-lsp-2421878-2-1795139273458016152/native-package/engine/lib/tsc --version` | 2 |
| 3 | 2424342 / 2424330 | false | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-lsp-2421878-2-1795139273458016152/native-package/engine/lib/tsc` | `/tmp/opencode/wxml-real-asset-f3-exec-audit-real-lsp-2421878-2-1795139273458016152/native-package/engine/lib/tsc --api --async --cwd /tmp/opencode/wxml-real-asset-f3-exec-audit-real-lsp-2421878-2-1795139273458016152/tmp/wxml-lsp-2424330-1795139273529553445-1 --useCaseSensitiveFileNames=true --callbacks=readFile,fileExists,directoryExists,getAccessibleEntries,realpath,stat,writeFile:noop,removeFile:noop` | 2 |

统计：`execs=3 forks=40 exit=0 timed_out=false survivors=[] 0.322s`。
会话完成 `initialize` → `documentSymbol`（`view`/`button`）→ `hover` → `shutdown`。

---

## 4. 断言与独立复核

测试内每个 traced 调用都执行
`crates/wxml-delivery/tests/f3_exec_audit/support.rs:290` 的
`assert_exec_audit`，其 `forbidden_evidence`（同文件 `:236`）检查：

- 可执行文件 basename ∈ `node`/`nodejs`/`npm`/`npx`/`yarn`/`pnpm`/`bun`；
- 任意 `argv` 的 basename 命中同一集合；
- 任意 `argv` 或 `exe` 包含 `server.js`；
- 任意 `argv` 包含 `WXML_TYPE_BACKEND`；
- exec'd 进程的 environment key 含 `WXML_TYPE_BACKEND` 或 `WXML_BACKEND_DIR`。

另外强制 `exe` 与 `argv[0]` 均为绝对路径；空日志直接失败。installer 的四个
调用还合并后再检查一次。

对日志的独立复核（只扫描 `EXEC` 记录）：

| 检查 | 结果 |
| --- | --- |
| EXEC 记录数 | 13 |
| Node-family / `server.js` 命中 | **0** |
| `exe` 非绝对路径 | **0** |
| `argv[0]` 非绝对路径 | **0** |
| installer root exec 是最终安装的 `wxml-lsp-install` | 通过 |
| LSP root exec 是 `bin/wxml-lsp` | 通过 |
| 安装树 engine sha256 == `TSGO_BIN` | 通过（`a183b195…`） |
| Node 环境键（`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`） | 测试断言通过 |

说明：日志只打印 `env_keys` 的数量（root 12、子进程 2），不打印键名；键名层面
的“无 Node 后端环境键”由上述 `forbidden_evidence` 断言保证，任何命中都会使
测试失败。独立扫描确认 13 条 `EXEC` 行中没有任何 forbidden basename。

---

## 5. 残余与边界

1. **平台：** F3-02 的 tracer 仅在 `target_os = "linux"` + `target_arch = "x86_64"`
   编译；其他平台会打印 `BLOCKED`，在 `WXML_REQUIRE_EXEC_AUDIT=1` 下失败关闭。
   本证据只覆盖 Linux x86_64。
2. **系统调用范围：** 只审计 `execve`/`execveat` 及其 fork/clone 子树；不审计
   `connect`、文件读取、`dlopen`、内存或内核模块。它证明的是“没有 Node 进程被
   exec”，不是“机器上没有 Node”。本次主机确实存在 `/usr/bin/node v24.22.2`。
3. **流程范围：** installer 只覆盖 `install`/`verify`/`current`/`env`；LSP 只覆盖
   一次 native stdio 会话。未覆盖升级/回滚、`--mock`、默认（非 `--backend native`）
   路径、zed wasm、release 脚本或 `cargo test --workspace` 的其他入口。
4. **测试输入的同一性：** LSP 用例把最终安装逐字节复制到 test-owned
   `native-package` 后再执行，日志中的 `PREBUILT` 行给出了 source；installer 用例
   用最终安装的 `wxml-lsp`/`wxml-lsp-install` 与固定 tarball 构建确定性 channel，
   并以 sha256 断言安装后的 engine 与 `TSGO_BIN` 相同。因此执行的是最终安装的
   字节等价副本，而非另一套构建。
5. **日志位置：** `/tmp/opencode/p7-final-exec-audit.log` 与
   `/tmp/opencode/p7-final-exec-audit-control.log`；测试临时目录由 `TestDir::drop`
   清理。本文件之外未修改任何代码、测试、README、ledger，也未 commit。
