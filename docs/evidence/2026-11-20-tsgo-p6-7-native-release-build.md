# P6-7：无 Node/npm 的 v2 native release 构建（2026-11-20）

## 0. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`，HEAD `8ccebbf`；工作树同时有并行会话的未提交改动，本切片未触碰、也未纳入结论。
- 本切片只改构建/CI/README：`scripts/build-release.sh`、`.github/workflows/ci.yml`、`crates/wxml-delivery/README.md`、`README.md` 和本文件。
- 不改 Rust 生产代码、默认后端、Node 删除、Cargo workspace、测试或 v1 packager。
- 不 commit、不启动 review。下方结果是本机一次批量验证，不是 CI 运行结果。

## 1. 变更

### 1.1 `scripts/build-release.sh`

构建路径改为 schema v2 native，不再要求 Node/npm：

1. `cargo build --release --locked -p wxml-lsp -p wxml-delivery`；
2. `cargo build --manifest-path editors/zed/Cargo.toml --release --target wasm32-wasip2 --locked`（可用 `--zed-wasm` 复用）；
3. `wxml-lsp-package provision --lock assets.lock.json --cache DIR [--from-dir DIR] --offline`，只接受已校验的内容寻址 cache 或锁定的 tarball；
4. staging 只放 native allowlist：`bin/wxml-lsp`、`bin/wxml-lsp-install`、`zed-extension/extension.{wasm,toml}`、`zed-extension/languages/**`，以及脚本从已验证 payload 的 `LICENSE`/`NOTICE`/`COPYING` 生成的 `THIRD_PARTY_LICENSES.md`；`engine/` 和 `typings/` 只能来自 cache；
5. `wxml-lsp-package native --input <stage> --output DIR --assets-lock assets.lock.json --cache DIR`，生成版本化目录、manifest、`provenance.json`、archive、sidecar、`index.json`、`SHA256SUMS`；
6. `wxml-lsp-install verify-package --from-dir DIR --version V --offline` 与 `sha256sum -c SHA256SUMS`。

脚本在 `PATH` 最前面创建 node/nodejs/npm/npx/pnpm/yarn/bun shim；任一被执行即失败并打印记录。staging 不复制 `backends/typescript/**`，并显式拒绝 `.js`/`.mjs`/`.cjs`/`server.js`/`node_modules`。

平台固定 `linux-x86_64`；其他 OS/架构在参数检查阶段失败。native 协议版本从
 ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:180` 与
 ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/engine.rs:39` 交叉校验，不再读取 `backends/typescript/package.json`。

### 1.2 `.github/workflows/ci.yml`

保留原 `check` job 不变，新增 `native-package`：

- `ubuntu-latest` + `wasm32-wasip2`；
- 以 `assets.lock.json` 的哈希缓存 `~/.cache/wxml-lsp-release/assets-cache`；cache miss 时用 curl 取锁定的两个 tarball，并校验 SHA-256/size，再交给 `provision` 全量校验；
- 构建步骤把 PATH 收敛到 `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin` 加 `$HOME/.cargo/bin`，并断言 node/nodejs/npm/npx/pnpm/yarn/bun 均不可见；
- 同一 `SOURCE_DATE_EPOCH` 下构建两次：第一次在冷 cache 时用 `--from-dir`，第二次只用 cache；逐字节比较 archive、sidecar、`index.json`、`SHA256SUMS` 和整棵 release 目录；
- `--online` 只允许 cargo 取 Rust 依赖；两个锁定 asset 仍然只来自 cache 或 `--from-dir`；
- `install --from-dir` + `verify`，再运行 `scripts/verify-no-node.sh`（PATH shim + 真实 engine 探针）。

PR 只 restore 资产缓存，不 save；push 才写回。

## 2. 批量验证（本机）

所有输出在 `/tmp/opencode/p6-7-release`（仓库外）。`SOURCE_DATE_EPOCH=1763683200`。

复现命令（`<worktree>` 为仓库根，tarball 目录只放 `assets.lock.json` 中精确文件名的两个文件）：

```bash
export SOURCE_DATE_EPOCH=1763683200
export WXML_BUILD_CACHE=/tmp/opencode/p6-7-release/cache
export CARGO_TARGET_DIR=/tmp/opencode/p6-7-release/target
export ZED_CARGO_TARGET_DIR=/tmp/opencode/p6-7-release/zed-target

# A：冷 cache，从锁定 tarball provision
scripts/build-release.sh --output /tmp/opencode/p6-7-release/out-a \
  --from-dir /tmp/opencode/p6-7-release/from-dir
# B/C/D：只用已验证 cache（source cache）；D 是最终脚本状态
scripts/build-release.sh --output /tmp/opencode/p6-7-release/out-b
scripts/build-release.sh --output /tmp/opencode/p6-7-release/out-c
scripts/build-release.sh --output /tmp/opencode/p6-7-release/out-d

cmp out-a/wxml-lsp-0.1.0-linux-x86_64.tar.gz out-b/wxml-lsp-0.1.0-linux-x86_64.tar.gz
cmp out-a/SHA256SUMS out-b/SHA256SUMS
diff -r out-a/wxml-lsp-0.1.0-linux-x86_64 out-b/wxml-lsp-0.1.0-linux-x86_64

release=/tmp/opencode/p6-7-release/out-a/wxml-lsp-0.1.0-linux-x86_64
root=/tmp/opencode/p6-7-release/install-root
"$release/bin/wxml-lsp-install" install --root "$root" \
  --from-dir /tmp/opencode/p6-7-release/out-a --version 0.1.0 --offline
"$release/bin/wxml-lsp-install" verify --root "$root"
TSGO_BIN="$root/current/engine/lib/tsc" WXML_REQUIRE_REAL_ENGINE=1 \
  scripts/verify-no-node.sh --installer "$release/bin/wxml-lsp-install" --root "$root"
```

| 命令/检查 | 结果 |
| --- | --- |
| `bash -n scripts/build-release.sh` | PASS |
| `scripts/build-release.sh --dry-run` | PASS（不写文件；协议 2；linux-x86_64） |
| `--skip-zed` 无 `--zed-wasm`、`--publish`、仓库内 `--output`、`--protocol-version 3` | 全部按预期 fail closed |
| build A：`--from-dir`，冷 cache，`source from-dir`（两个 asset） | PASS |
| build B/C/D：无 `--from-dir`，`source cache`；D 为最终脚本状态 | PASS |
| A/B/C/D archive SHA-256 | `35a356589fb5aac4dad9de8d9edf6fb7ad82e9e0a82b1fb9f8c958a4bd42b488`（四次一致） |
| `cmp` archive/sidecar/`index.json`/`SHA256SUMS`；`diff -r` release tree | 全部逐字节一致 |
| manifest/provenance/licenses 的 SHA-256 | A/B 一致：`f1f50e36...`、`b3001987...`、`cad23bf2...` |
| `wxml-lsp-install install --from-dir` + `verify` | PASS；157 files；engine `7.1.0-beta` |
| install 在 node/npm/... shim PATH 下执行（`env -u WXML_LSP_BIN`） | PASS；shim marker 未创建 |
| `scripts/verify-no-node.sh`（PATH shim，`TSGO_BIN`，`WXML_REQUIRE_REAL_ENGINE=1`） | `NO_NODE: PASS`（installer scope）；`REAL_ENGINE: PASS` |
| clean PATH（仅 symlink 到非 node 工具，`node: absent`）构建 E/F | PASS；provision/package/verify/SHA256SUMS 全过 |
| `.github/workflows/ci.yml` PyYAML 解析 + 每个 `run:` 的 `bash -n` | PASS（`check` 10 步不变，`native-package` 9 步） |

**并行会话说明**：验证期间另一个会话持续修改
 ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs` 和
 ` .../component_bindings.rs`（02:29、02:31 两次）。A/B/C/D 构建时 cargo 未重编译（`Finished in 0.0xs`），所以四者逐字节一致；clean PATH 的 E、F 分别触发了 `wxml-lsp` 重编译，archive 各自不同，只用于证明“无 Node 环境可完整构建/验证”，不纳入 determinism 结论。本切片没有修改任何 Rust 生产代码，也没有把并行会话的改动纳入结论。

产物：159 个文件；含 `engine/lib/tsc`、`typings/miniprogram-api-typings/**`、两个 Rust bin、Zed wasm/toml/languages、`THIRD_PARTY_LICENSES.md`、`provenance.json`、`manifest.json`、`SHA256SUMS`；无 `.js`/`server.js`/`node_modules`。

## 3. 未关闭项

| 项 | 状态 |
| --- | --- |
| F3-14 Zed UI（五语言、hover/definition/completion、无 Node 子进程） | **BLOCKED**，不在本切片 |
| P6-8 默认 `--stdio` 切 native/HostOnly | 未做；` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:184` 仍构造 `NodeBackend` |
| F3-02 关闭 | 未关闭；本次只复用 `scripts/verify-no-node.sh` 的 PATH shim + 真实 engine，未运行 `f3-exec-audit.sh` 的 ptrace exec 审计 |
| v1/Node packager | Rust v1 路径保留（供 legacy 证据），`build-release.sh` 不再调用 |
| 其他平台 | native 只支持 `linux-x86_64`；aarch64/macOS/Windows 明确失败 |
| CI 实际运行 | 本机只做 YAML 语法/结构检查；尚未在 GitHub runner 上执行 |
