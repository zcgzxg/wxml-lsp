# P7 最终安全 / 供应链审计（2026-11-20）

> **范围说明（2026-11-20 后续 license 审计）**：本文件“供应链门禁 PASS”中的
> “许可”指 `cargo deny` 的 license expression 策略，不表示随包许可文本完整。
> 后续 [release 许可审计](2026-11-20-tsgo-p7-release-license-audit.md) 发现
> `THIRD_PARTY_LICENSES.md` 缺 Rust/Zed 依赖许可文本，当时对外发布为 **BLOCKED/P1**；
> [许可生成修复](2026-11-20-tsgo-p7-license-generation-fix.md)（generator/2）已把第三方文本补齐为
> **155/155**（144 本地 + 11 vendored SPDX；0 missing / 0 unlicensed），首方 LICENSE 仍 `BLOCKED owner decision`，对外发布仍需 owner 决策。
> 本文件的 0 HIGH/CRITICAL 只适用于漏洞/unsafe/exec 范围。

## 0. 结论先行

1. **总体结论：PASS（Linux x86_64，本审计范围）**。P7 删除 Node 之后未发现
   HIGH/CRITICAL 新增风险；P7 本身是净删除（`crates/*/src` **+152 / −10254**），
   新增行中没有 `Command::new`、`unsafe`、环境变量读取或文件写入口。
2. **最高残余风险：MEDIUM**。预编译 TypeScript `7.1.0-beta` 引擎以当前用户权限
   运行，没有 Landlock/seccomp/namespace 级隔离；缓解是 spawn 前 SHA-256 复验、
   固定 argv/env/cwd 和只读 VFS。该风险不是 P7 新引入的；P7 让 native 成为唯一
   后端，默认暴露面增加，但删除了 Node 运行时、npm 依赖树和 `server.js`。
3. **供应链门禁：PASS**。`cargo audit` 0 漏洞（225 crates；Zed 子工作区 89
   crates 另测 0 漏洞）；`cargo deny --offline check` 四项全 `ok`。
4. **无 Node 执行：PASS（exec 级）**。ptrace 真实审计记录 installer 10 个 exec、
   native LSP 3 个 exec，全部为绝对路径，**无** `node`/`npm`/`npx`/`yarn`/`pnpm`/
   `bun`、无 `server.js`；`verify-no-node.sh` 亦 PASS。该脚本自报 F3-02 的
   installer scope 为 **BLOCKED**（PATH shim 无法排除绝对路径），本审计用 ptrace
   补齐 exec 证据，但**不**声明网络 `connect` 审计。
5. 本文件只写安全证据；未改代码/测试/README/ledger/脚本，未 commit。

| 审计面 | 状态 |
| --- | --- |
| Rust 依赖漏洞 / 许可 / 来源 | **PASS** |
| 生产代码 `unsafe` / 进程 spawn | **PASS**（无生产 `unsafe`；spawn 均为 pin 二进制） |
| native engine 启动 / 沙箱 / VFS / 超时 | **PASS（有 MEDIUM 残余）** |
| release / 安装器无 Node、无项目写入 | **PASS** |
| 内核级沙箱 / `openat` 防 TOCTOU | **BLOCKED**（既有设计边界，非 P7 回归） |
| 非 Linux 平台 | **BLOCKED（未审计）** |

---

## 1. 身份、范围与方法

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b`（`b6c1ced`） |
| P7 删除提交 | `c4ddf32`（`feat(p7): remove node backend and legacy typescript tree`） |
| 平台 | Linux `7.2.15+deb14-amd64` x86_64 |
| 工具链 | rustc/cargo `1.100.0`；cargo-audit `0.23.0`；cargo-deny `0.21.0` |
| 审计日期 | 2026-11-20（本地 +08；UTC `2026-11-20T01:48Z`） |
| 方法 | 只读静态审查 + 供应链命令 + 真实二进制 tripwire/ptrace 运行 |
| 写入 | 仅本文件 |
| 工作树现状 | 另有其它会话的未提交测试/文档改动（`crates/*/tests/**`、`docs/**`）；**无 `crates/*/src/**` 改动**，故生产代码引用与 HEAD 一致 |

被测运行产物：`/tmp/opencode/p7-post-delete-install`（P7 之后构建，二进制时间
2026-11-20 09:38；schema v2，含 `engine/lib/tsc`，无 JS/node_modules）。
被测测试二进制：由 HEAD 工作树编译（`f3_exec_audit`、`no_node_guard`）。

---

## 2. 供应链门禁

### 2.1 Rust 依赖

| 命令 | 退出码 | 结果 |
| --- | --- | --- |
| `cargo audit` | `0` | 225 个 crate，**0 vulnerabilities / 0 warnings**；advisory DB 1550 条 |
| `cargo audit --file editors/zed/Cargo.lock` | `0` | Zed 扩展独立工作区 89 个 crate，**0 vulnerabilities** |
| `cargo deny --offline check` | `0` | `advisories ok, bans ok, licenses ok, sources ok` |

`deny.toml` 的有效约束：`yanked = "deny"`、`multiple-versions = "deny"`、
`wildcards = "deny"`、`unknown-registry/unknown-git = "deny"`、`allow-git = []`、
许可白名单仅 permissive（MIT/Apache-2.0/BSD-3-Clause/ISC/…），无 copyleft。

### 2.2 非 Rust 资产 pin

| 资产 | pin（`assets.lock.json`） | 实测 |
| --- | --- | --- |
| engine tarball `typescript-linux-x64-7.1.0-beta.tgz` | sha256 `59128685…d51855`，size `10175135` | 一致 |
| engine binary `engine/lib/tsc` | sha256 `a183b195…aaa70`，size `24522912` | 一致（PinnedEngine 同值） |
| typings tarball `miniprogram-api-typings-5.3.1.tgz` | sha256 `e5f8b50f…d4b71`，size `449429` | 一致 |

` assets.lock.json` 同时记录 SRI、上游 `distSha1`、逐文件 sha256 与 `treeDigest`；
` crates/wxml-backend-tsgo/src/config.rs:415` 的单元测试锁定上述 F0 常量，
` crates/wxml-backend-tsgo/src/native/supervisor.rs:1438` 在每次 spawn 前按
`engine.executable_sha256()` 复验可执行文件。

---

## 3. 活跃代码扫描

### 3.1 `unsafe`

- 生产代码 **0 个真实 `unsafe` 块**（`unsafe {` / `unsafe fn` / `unsafe extern`）。
- 根 ` Cargo.toml:38` 设置 `unsafe_code = "deny"`，8 个 workspace crate 全部
  `[lints] workspace = true`，编译期禁止生产 `unsafe`。
- `grep unsafe` 的 154 处文本命中里，真实块只出现在测试追踪器：
  ` crates/wxml-delivery/tests/f3_exec_audit/tracer.rs`、
  ` crates/wxml-delivery/tests/f3_kill_window/tracer.rs`（ptrace/fork）。生产文件的
  命中是错误信息/文档字符串（例如 “layout is unsafe”）与测试断言。测试目标显式
  `#![allow(unsafe_code)]`，不进入发布二进制。

### 3.2 `Command::new`（生产）

| 位置 | 用途 | 边界 |
| --- | --- | --- |
| ` crates/wxml-backend-tsgo/src/native/supervisor.rs:508` | spawn pinned engine | `env_clear` + `LANG/LC_ALL`，cwd=sandbox，SHA-256 前置复验 |
| ` crates/wxml-backend-tsgo/src/native/session.rs:2662` | `engine --version` 启动前探针 | `env_clear`，10 s deadline，超时 kill+wait |
| ` crates/wxml-delivery/src/native_install.rs:159` / `:253` | engine / LSP 激活版本探针 | manifest 锁定 `args == ["--version"]`，`env_clear`，超时/进程组 kill |
| ` crates/wxml-delivery/src/verify.rs:156` | 已安装 `wxml-lsp --version` | 10 s 超时，进程组 kill |
| ` crates/wxml-delivery/src/probe.rs:148` | 超时后 `kill` 辅助 | 仅进程组清理，不承载用户输入 |

没有 shell 拼接、没有 `sh -c`、没有 Node 运行时。

### 3.3 Node 相关标识符

| 标识符 | 生产命中 | 判定 |
| --- | --- | --- |
| `WXML_NODE` | 无读取 | ` main.rs:137` 仅拒绝 `--backend node`；` backend_tsgo/mod.rs:227` 明确“永不读取” |
| `WXML_TYPE_BACKEND` / `WXML_BACKEND_DIR` | 无读取 | 同上；`verify-no-node.sh` 把它们设为 poison 后仍 PASS |
| `server.js` / `bin/wxml-type-backend` | 仅 legacy 只读识别与禁止清单 | ` type_assets.rs:26` 在 v2 包中直接拒绝 |
| `backend_node` | 无 | 目录/模块已删除 |
| `NodeBackend` | 仅 schema v1 manifest 数据校验 | ` manifest.rs:320` 只校验路径/版本，不执行 |

补充扫描：仓库内 11 个受版本控制的 `.js` 全部是测试 fixture（`crates/**/tests/**`、
`tests/fixtures/**`），发布 stage 不含 JS（见 §5.1）。磁盘上残留的空目录
`backends/typescript/` 不是 git 跟踪内容、0 文件、不进包。

### 3.4 P7 删除本身

`git diff c4ddf32^ c4ddf32 -- 'crates/*/src/**'`：**+152 / −10254**，删除
`backend_node/**` 8 个文件与 v1 写路径，新增行中没有 `Command::new`、`unsafe`、
`std::env::var`、`fs::write`/`File::create` 或权限操作。删除是减少攻击面，不是
新增特权路径。

---

## 4. Native engine 启动与运行边界

### 4.1 启动参数（冻结 6 参数 argv）

` crates/wxml-backend-tsgo/src/config.rs:361` 生成且仅生成：

```text
--api --async --cwd <sandbox> --useCaseSensitiveFileNames=true --callbacks=<固定列表>
```

无 `--allow*`、无 `--runExternalCode`、无 shell、无用户可控参数。callback 列表
固定为 `readFile,fileExists,directoryExists,getAccessibleEntries,realpath,stat,
writeFile:noop,removeFile:noop`（` config.rs:128`）。

### 4.2 进程环境

` crates/wxml-backend-tsgo/src/native/supervisor.rs:507`：

- `env_clear()`；
- 只回填 `LANG=C`、`LC_ALL=C`（` supervisor.rs:108`）；
- `current_dir` 为已验证的空 sandbox；
- stdin/stdout/stderr 全 pipe，无继承终端。

### 4.3 sandbox

` crates/wxml-lsp/src/backend_tsgo/sandbox.rs:3`：

- base 必须是绝对、真实目录、非 symlink；属主为 euid，或 root-owned sticky `/tmp`；
- 禁止 group/world 可写（除非 sticky）；
- canonicalize 后必须位于安装包树、`versions/`、install root 之外
  （` sandbox.rs:249`）；
- 每次会话新建 `0700` 空目录，二次校验非 symlink、`0700`、空、未逃逸 base
  （` sandbox.rs:222`）；Drop 只 `remove_dir`（仅空目录，` sandbox.rs:290`）。

### 4.4 只读 VFS

` crates/wxml-backend-tsgo/src/native/vfs.rs:1`：所有 `readFile/fileExists/
directoryExists/getAccessibleEntries/realpath/stat` 只查内存表，**从不查询主机
文件系统，从不返回 `useOS`**；`writeFile`/`removeFile` 固定
`{"kind":"noop"}`（` vfs.rs:439`）。

路径边界：项目根与固定资产根之外一律 fail-closed（`vfs.rs:12` 的负向表）；
`..`、`.`、绝对路径逃逸、控制字符、反斜杠、空组件均在
` vfs.rs:1133` / ` vfs.rs:708` / ` input.rs:1536` 拒绝。会话启动时
` session.rs:2648` 用真实主机路径做 canary，证明 dispatcher 对根外文件返回
`missing/false`，失败则 `VfsConfinement` 不可用。

**限定**：这是“引擎遵守 host callback”的只读边界，不是内核强制；见 §6 R1/R2。

### 4.5 超时与杀进程

| 项 | 值/行为 |
| --- | --- |
| 请求 deadline | 15 s（` config.rs:105`） |
| 握手/启动探针 | 10 s（` supervisor.rs:56`） |
| 文件回调 | 2 s（` supervisor.rs:59`） |
| shutdown | close stdin → wait 5 s → SIGKILL → reap（` supervisor.rs:1113`） |
| 队列预算 | in-flight 8；inbound ≤ 96 MiB；frame header ≤ 8 KiB |
| 超时语义 | poison session，后续请求 `SessionPoisoned`，不静默重试 |
| 安装器探针 | 独立进程组 + 10 s + 64 KiB 输出上限（` probe.rs:25`） |

### 4.6 错误信息泄露

- engine JSON-RPC 错误只保留数值 code，**丢弃引擎 message**（可能含 Go panic
  栈或绝对路径）：` crates/wxml-backend-tsgo/src/native/mod.rs:143`。
- 其余错误使用 `&'static str` 原因，不带项目路径：` backend_tsgo/engine.rs:85`。
- 日志只写 stderr（` server.rs:662`），不写项目文件。
- 安装器错误对路径使用 `display_name`（仅 basename）或 `redact`。

---

## 5. 安装器 / release 边界

### 5.1 包内容

- ` scripts/build-release.sh:490`：stage 中出现 `*.js/*.mjs/*.cjs/server.js`
  直接失败；`:494` 出现 `node_modules` 直接失败。
- 构建期 PATH tripwire：`node/npm/npx/yarn/pnpm/bun` 被 shim 拦截，执行即失败
  （` build-release.sh:375`、`:384`）；仅辅助证据，脚本不冒充 execve 审计。
- v2 包逐路径拒绝 JS、`node_modules`、lockfile、`bin/wxml-type-backend/`
  （` type_assets.rs:26`；写入 manifest 校验 ` manifest_v2.rs:568`，打包
  ` package.rs:730`；解包树同规则）。
- 实测 `/tmp/opencode/p7-post-delete-install`：0 个 `*.js/*.cjs/*.mjs`、
  0 个 `node_modules`；engine 二进制 sha256/size 与 pin 一致。

### 5.2 归档处理

` crates/wxml-delivery/src/archive.rs:1` 将归档视为不可信输入：

- 只接受 regular file / directory，symlink、hardlink、FIFO、device 一律拒绝
  （` archive.rs:534`）；
- 路径先 `validate_relative_path`（拒绝绝对、`..`、drive prefix、NUL/控制字符、
  `\\`，` paths.rs:56`），再做大小写折叠冲突检测；
- 解压前 `prescan`，解压后核对允许清单与逐文件 size/digest；
- 上限：archive 512 MiB、单文件 128 MiB、解压 1 GiB、8192 文件；
- 解压目标必须不存在，失败递归清理。

### 5.3 安装根与写入范围

- 拒绝 `/`、`$HOME`、`$XDG_DATA_HOME` 本身作为安装根（` install.rs:51`）；
- 所有临时目录在 `<root>/tmp` 下创建（` install.rs:111`），状态、缓存、
  `versions/`、`current` 均在 root 内；
- `WXML_LSP_BIN` 只做校验/导出，不写入，也不作为安装目标（` install.rs:389`）；
- `verify`/`current`/`env` 为只读路径；schema v1 legacy 只做只读识别，
  ` verify.rs:1` 明确“never runs node --version，never resolves WXML_NODE”。

### 5.4 网络

- 仅 `install/upgrade` 的远程来源使用网络；`verify/current/env` 无网络。
- ` crates/wxml-delivery/src/source.rs:36`：HTTPS（HTTP 仅 loopback 测试）、
  拒绝 credentials/query/fragment、拒绝 `..`；
- 客户端 30 s 超时、**禁止重定向**、同源校验、响应 size cap
  （` source.rs:240`）；
- 归档安装前校验 size + SHA-256 + SRI，失败删除半成品（` source.rs:138`）。

### 5.5 无 Node 运行证据（本次实跑）

**A. `scripts/verify-no-node.sh`（真实 native 安装根 + 真实 engine）**

```text
NO_NODE: positive control exercised (shim exit 99, marker created)
verify: verified 0.1.0 (native)
current: binary: /tmp/opencode/p7-post-delete-install/current/bin/wxml-lsp
env: export WXML_LSP_BIN='/tmp/opencode/p7-post-delete-install/current/bin/wxml-lsp'
NO_NODE: PASS (installer scope: verify/current/env)
LSP_NATIVE: PASS (.../crates/wxml-lsp/src/main.rs has no NodeBackend or serve_node(); --stdio is native-only)
REAL_ENGINE: PASS (sha256=a183b195…aaa70, Version 7.1.0-beta)
RESIDUAL: execve/strace process audit is not performed; PATH shims cannot exclude absolute paths
F3-02: BLOCKED (installer scope only; no closure claim)
```

退出码 `0`。positive control 证明 tripwire 非空转；engine 只以 `--version` 运行。

**B. ptrace exec 审计（HEAD 测试二进制，真实 installer + native LSP）**

```text
EXEC-AUDIT: install:  execs=4 forks=16 exit=Code(0) survivors=[]
EXEC-AUDIT: verify:   execs=4 forks=15 exit=Code(0) survivors=[]
EXEC-AUDIT: current:  execs=1 forks=0  exit=Code(0) survivors=[]
EXEC-AUDIT: env:      execs=1 forks=0  exit=Code(0) survivors=[]
EXEC-AUDIT: PASS (real installer): install/verify/current/env exec'd 10 processes,
            7 distinct absolute executables
EXEC-AUDIT: real-native-lsp: execs=3 forks=36 exit=Code(0) survivors=[]
EXEC-AUDIT: PASS (real native LSP): initialize/documentSymbol/hover/shutdown completed;
            3 exec(s), 2 distinct absolute executables; no Node-family exec
test result: ok. 2 passed; 0 failed; 0 ignored
```

退出码 `0`。installer 的 exec 集合为 `wxml-lsp-install`、staged/installed
`wxml-lsp`、`wxml-lsp-install`、`engine/lib/tsc`；LSP 为 `wxml-lsp` +
`tsc --version` + `tsc --api --async`。**无 Node 家族、无 `server.js`**。

**C. `crates/wxml-delivery/tests/no_node_guard`（HEAD 测试二进制，直跑）**

```text
PROC_TREE: PASS (4 descendant processes observed, none Node)
NO_NODE: PASS (installer scope: install/verify/current/env)
LSP_NATIVE: PASS (... serve_default() is native and serve_node() is gone)
test result: ok. 4 passed; 0 failed
```

退出码 `0`。该测试内部同时打印 `EXEC_AUDIT: BLOCKED (strace not installed)`，
这是其自身 scope 说明；exec 证据以上述 ptrace 套件为准。

---

## 6. 残余风险（不夸大）

| ID | 等级 | 事实 | 现有缓解 | 状态 |
| --- | --- | --- | --- | --- |
| R1 | **MEDIUM** | 预编译 TypeScript `7.1.0-beta` 引擎以当前用户权限运行；无 Landlock/seccomp/namespace，恶意或投毒二进制可读写用户可及文件、访问网络 | 上游 tarball + 逐文件 treeDigest + 引擎 SHA-256 在 spawn 前复验；argv/env/cwd 固定；VFS 回调不暴露主机文件；包内无 JS | 接受（非 P7 新增） |
| R2 | LOW | “只读 VFS”由引擎遵守 host callback 实现；引擎若绕过 callback 直接 syscall，可读主机文件 | 与 R1 同一信任锚（pinned SHA-256）；`confinement_self_check` 证明 dispatcher 不读主机 | 接受 |
| R3 | LOW | sandbox 校验是 path-based，finalize→spawn 间存在父目录替换窗口；未实现 `openat`/目录句柄硬化 | spawn 前复验 engine hash（替换引擎会失败）；sandbox 为空且 `0700`；已在 ` sandbox.rs:14` 明示 BLOCKED | BLOCKED（既有设计边界） |
| R4 | LOW | LSP supervisor 只 `child.kill()` 直接子进程，未设 process group；若引擎派生后代可能存活 | 引擎是单一 pinned 二进制，正常不派生；shutdown 有界；ptrace 审计 `survivors=[]` | 接受 |
| R5 | LOW | 安装器 `upgrade`/远程 source 存在网络路径 | HTTPS、禁止重定向、30 s、size cap、SHA-256+SRI、同源校验 | 接受 |
| R6 | INFO | `verify-no-node.sh` 自报 F3-02 installer scope **BLOCKED**；网络 `connect` 未审计 | 本审计的 ptrace 套件补 exec 集合；脚本不冒充 closure | 证据边界 |
| R7 | INFO | schema v1 legacy 树仍可只读校验（manifest 解析） | ` verify.rs:1` 不执行 node、不解析 `WXML_NODE`；无写路径 | 保留 |
| R8 | INFO | `--mock` 内存后端在 release 二进制中仍可显式选择 | 纯内存假数据，无文件/网络/进程 | 保留 |

**最高风险等级：MEDIUM（R1）**。没有 HIGH/CRITICAL 发现；R1 的等级来自
“预编译第三方二进制 + 无内核隔离”的固有 blast radius，而不是 P7 引入的缺陷。

---

## 7. 证据命令与退出码

| # | 命令 | 退出码 | 日志/位置 |
| --- | --- | --- | --- |
| 1 | `cargo audit` | `0` | `/tmp/opencode/p7-final-audit-cargo-audit.log` |
| 2 | `cargo audit --file editors/zed/Cargo.lock` | `0` | 本文件 §2.1 |
| 3 | `cargo deny --offline check` | `0` | `/tmp/opencode/p7-final-audit-cargo-deny.log` |
| 4 | `target/.../no_node_guard --nocapture` | `0` | `/tmp/opencode/p7-final-audit-no-node-guard.log` |
| 5 | `scripts/verify-no-node.sh`（native + 真实 engine） | `0` | 本文件 §5.5-A |
| 6 | `cargo test -p wxml-delivery --test f3_exec_audit -- --ignored --test-threads=1` | `0` | `/tmp/opencode/p7-final-audit-exec-all.log` |

命令 6 的环境（真实 pin 资产 + post-delete 包）：
`TSGO_NATIVE_TARBALL`、`TSGO_TYPINGS_TARBALL`、`TSGO_BIN`、`TSGO_ASSET_ROOT`、
`TSGO_TYPINGS_ROOT`、`WXML_F3_LSP_BIN`、`WXML_F3_INSTALL_BIN`、
`WXML_F3_ZED_WASM`、`WXML_REQUIRE_REAL_ENGINE=1`、`WXML_REQUIRE_EXEC_AUDIT=1`。

---

## 8. 边界与未覆盖

1. 仅 **Linux x86_64**；macOS/Windows 的 sandbox、权限位与路径语义未审计。
2. 未做内核级 `connect` 审计；`source.rs` 的网络策略是代码审查结论。
3. 未重跑整个 workspace 测试套件（本任务只读审计，且其它会话正在改测试）；
   本文件不把“套件全绿”作为安全结论。
4. 未审计 Zed 运行时本身；仅审计 Zed 扩展 crate 的依赖锁（0 漏洞）与
   `extension.wasm` 作为包内容被安装器搬运。
5. `cargo audit` 覆盖根 workspace 与 Zed 子工作区锁文件；不覆盖 engine/typings
   上游 tarball 的内部代码，只覆盖其 pin 与逐文件摘要。
6. 历史证据（P7 删除前）未被当作本次通过依据；本文件 §5.5 的三项运行均为
   本次在 P7 之后重新执行。

---

## 9. 最终判定

- **PASS**：供应链依赖、许可与来源；生产 `unsafe` 禁用；Node 运行时与
  `server.js` 移除；installer/release 不执行 Node、不写项目；native engine
  启动参数、环境、cwd、VFS、超时与错误信息均符合最小暴露原则。
- **BLOCKED（如实记录）**：内核级沙箱、`openat` TOCTOU 硬化、非 Linux 平台、
  F3-02 的网络 `connect` 层审计。以上均为既有边界，不是 P7 删除引入的回归。
- **最高风险等级：MEDIUM**（R1，预编译 beta 引擎 + 无内核隔离）。

本文件为本次任务唯一写入；未修改代码、测试、README、ledger 或脚本，未 commit。
