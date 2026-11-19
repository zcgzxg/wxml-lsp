# F3-02 ptrace exec 审计（2026-11-20）

状态：**F3-02 closed（Linux x86_64）**。实现与运行证据在
`crates/wxml-delivery/tests/f3_exec_audit.rs`、`crates/wxml-delivery/tests/f3_exec_audit/**`
和 `scripts/f3-exec-audit.sh`；本次提交 `f2952c2`。

## 方法

- test-only `extern "C"` ptrace，无新增 Cargo 依赖：fork + `PTRACE_TRACEME` + SIGSTOP，
  parent 使用 `PTRACE_O_TRACEEXEC|TRACEFORK|TRACEVFORK|TRACECLONE|EXITKILL`。
- 每个 pid 独立 `waitpid`；exec 时读取 `/proc/<pid>/{exe,cmdline,comm,status,environ}`；
  超时按进程组 SIGKILL；处理 clone/ECHILD 线程重新收养竞态。
- 真实 installer 和 native LSP 各一条；缺失 ptrace 权限/非 Linux x86_64 时显式 BLOCKED，
  `WXML_REQUIRE_EXEC_AUDIT=1` 时失败关闭。

## 真实运行

- 安装器：10 个 exec（installer、staged/installed `bin/wxml-lsp`/`bin/wxml-lsp-install`、
  `engine/lib/tsc`）。
- native LSP：3 个 exec（`wxml-lsp`、`tsc --version`、`tsc --api --async`）。
- 无 `node`/`npm`/`npx`/`yarn`/`pnpm`/`bun`，无 `server.js`，无 `WXML_TYPE_BACKEND`；
  所有路径为绝对路径。
- 日志：`/tmp/opencode/f3-exec-audit-evidence/`（`real-user-env.log`、`nonreal-final.log` 等）。

## 命令

```bash
cargo test -p wxml-delivery --test f3_exec_audit
WXML_F3_LSP_BIN=/tmp/opencode/tsgo-native-pkg/bin/wxml-lsp \
TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
TSGO_TYPINGS_ROOT=/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/miniprogram-api-typings \
WXML_REQUIRE_EXEC_AUDIT=1 WXML_REQUIRE_REAL_ENGINE=1 \
cargo test -p wxml-delivery --test f3_exec_audit -- --ignored --test-threads=1
```

结果：非真实 4 passed / 2 ignored；真实 2 passed / 0 failed（151.32s），主线程独立复跑
installer 137.60s、LSP 1.88s。该证据只证明 exec 集合，不证明网络 `connect` 级审计。
