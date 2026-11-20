# F3-09 真实 legacy schema v1 发布包与迁移验收

**日期**：2026-11-20。**对象**：`wxml-lsp` schema v1 legacy 包（0.1.0）→ schema v2 native
（0.2.0）。**结论**：真实 v1 包的安装、入口可运行、legacy→native 一次迁移、迁移前失败、
切换后中断前滚、legacy 回滚拒绝和 `recover` 不恢复 legacy 全部 PASS；F3-09 的包身份子条件
（legacy current 不再是 fixture）已补齐。**限制**见 §7：本包是 `e4fed4a` clean archive 的
全保真构建，不是历史发布产物。

## 1. 方法

新增 opt-in 证据脚本 ` scripts/f3-legacy-real.sh` 和测试
` crates/wxml-delivery/tests/f3_legacy_real.rs`，只改 test/script/evidence/contract，不改生产
`src`、Cargo、Node 删除或默认 backend。脚本在仓库外构建：

1. 从 `git archive e4fed4a` 解出干净源码，再 `cargo build --release -p wxml-lsp -p wxml-delivery`；
2. 把 `backends/typescript` 复制到仓库外，执行 `npm ci` 和 `npm run build`（不修改仓库内
   `node_modules`/`dist`）；
3. `cargo build --manifest-path editors/zed/Cargo.toml --release --target wasm32-wasip2`；
4. 用真实产物组装 schema v1 输入树：`bin/wxml-lsp`、`bin/wxml-lsp-install`、完整的
   `backends/typescript/dist` + 锁定的 `node_modules`、`zed-extension/`、许可说明；
5. 用生产 v1 packager（`wxml-lsp-package`，不带 `native`）打包，再用生产安装器
   `wxml-lsp-install verify-package` 校验；
6. 安装器以生产 schema v1 路径安装；测试再用 pinned real-asset 通道的 native N2 执行迁移和
   全部负向/回滚/恢复断言。

运行命令：

```text
scripts/f3-legacy-real.sh --output <f3-legacy-real-run> \
  --source-rev e4fed4a --required
```

`--required` 同时设置 `WXML_REQUIRE_LEGACY_PACKAGE=1` 与 `WXML_REQUIRE_REAL_ENGINE=1`；缺少
v1 包或 pinned 资产时 fail-closed。本套件不依赖 ptrace。测试本身是 `#[ignore]`，默认 CI 不
运行；没有 `WXML_F3_LEGACY_PACKAGE` 时打印显式 `F3-EVIDENCE: F3-09 SKIP`，required 模式
panic。

## 2. 环境

| 项 | 值 |
| --- | --- |
| 平台 | Linux x86_64 |
| 源码 | `git archive e4fed4a`（session 指定 HEAD；脚本 `--source-rev e4fed4a`）；live worktree 后续并发提交/未提交改动不参与构建 |
| rustc / cargo | 1.100.0 (`0ef1bc386` / `3acc79c0c`) |
| Node / npm | v24.22.2 / 11.19.1 |
| Rust target | x86_64-unknown-linux-gnu + wasm32-wasip2 |
| native 目标 | pinned `typescript` 7.1.0-beta + `miniprogram-api-typings` 5.3.1；N2 由 N1 生产二进制同长度版本常量重标 |

## 3. 包身份

生产 v1 packager 输出：

| 字段 | 值 |
| --- | --- |
| package / version / platform | `wxml-lsp 0.1.0` / `0.1.0` / `linux-x86_64` |
| schema / protocol / node minimum | 1（读取时验证） / 2 / `>= 24.0.0` |
| backend version | 0.1.0 |
| builtAt | `2026-11-20T00:00:00Z` |
| manifest 文件数 | 333 |
| archive | `wxml-lsp-0.1.0-linux-x86_64.tar.gz` |
| archive SHA-256 / size | `b62d31127e130dc5c77e4a59c5a7078216031de6183b7ae9eb4d4c93d6d970ae` / 10,358,343 B |
| `bin/wxml-lsp` SHA-256 | `70a02f22eb92fc6b94c7cf67b85af79a19fded9b43b659d782fc5fd81354e1ba` |
| `bin/wxml-type-backend/server.js` SHA-256 | `7b1dfaafd94cc53d8cac805171ed0d1e4ba79b8215234da296b6ef54f56dd7da` |
| `wxml-lsp-install` SHA-256 | `ab74ddcec917ea94e1dcd531182f1225607a50eee57d5044800064d3e22984f2` |
| Zed wasm SHA-256 | `488531eaec969281fa295803e2d3cd857493bd9e26f56adc1c21a71bad0aeb1f` |

测试还断言：`bin/wxml-lsp` 是大于 512 KiB 的 ELF，不是 shell fixture；
`bin/wxml-type-backend/server.js` 是大于 4 KiB 的真实 ESM 后端；
`node_modules/typescript/package.json` 与 `node_modules/miniprogram-api-typings/package.json`
分别声明 `typescript` 和 `miniprogram-api-typings`。生产安装器的 `verify-package` 输出：

```text
verified package wxml-lsp 0.1.0 (linux-x86_64)
protocol: 2  node: >= 24.0.0  files: 333
archive sha256: b62d31127e130dc5c77e4a59c5a7078216031de6183b7ae9eb4d4c93d6d970ae
```

`e4fed4a` clean archive 构建与此前一次 live-worktree 构建产出相同的 archive、`bin/wxml-lsp`
和 `server.js` SHA-256；本记录采用 clean archive 的数值。live worktree 在运行期间有并发
提交/未提交改动，均未进入本次构建。

## 4. 迁移前 legacy 安装

| 项 | 观测 |
| --- | --- |
| 安装命令 | `wxml-lsp-install install --from-dir <v1-release> --version 0.1.0 --offline` |
| `current` | `versions/0.1.0`（schema 1，只读） |
| `state.json` | 不存在 |
| 安装树 | 335 个文件（含 `manifest.json`/`SHA256SUMS`） |
| tree fingerprint | `05557011c7f5b5e42f0c5688a1bd07ffe932629528b0ec00a77a2d5ef0f5ed62` |
| 入口 `--version` | `wxml-lsp 0.1.0` |
| Node 入口 | `--stdio --backend node` 完成 LSP `initialize`，响应 1,262 B，`serverInfo.name=wxml-lsp` |

入口烟测使用真实 Node v24.22.2 和包内 `wxml-type-backend/server.js`；迁移和安装命令本身在
PATH-only-Node-tripwire 环境中运行，未触发 `node/nodejs/npm/npx/pnpm/yarn/bun` shim。

## 5. 迁移与负向结果

| 场景 | 结果 |
| --- | --- |
| 成功迁移 | `install ... --version 0.2.0 --offline` 输出 `migrated to 0.2.0 (native)`；`current=0.2.0`；`state=active:0.2.0/previous:null`；无 journal/tmp 残留；legacy tree fingerprint 仍为 `0555701…5ed62`，`bin/wxml-lsp` 与 `server.js` 哈希不变；native current 无 `wxml-type-backend`/JS；无 Node/server.js 进程 |
| 迁移前失败 | 真实 N2 归档派生变体缺少一个 typings 文件：在激活前失败；legacy current/tree/state 不变；无 native state/journal；Node tripwire 未触发，未观察到 legacy 进程 |
| 切换后中断 | 生产 `AfterSwitch` checkpoint 中断：`SwitchedButNotDurable { version:0.2.0, previous:null, state_reconciled:true }`；已切换的 native target 前滚并写 state；legacy 树不变、legacy 永不成为 `previous`、无 journal、无 Node/legacy 进程 |
| `recover` | 迁移成功后 `recover` 输出 `no recovery actions were needed`，state 字节不变、current 仍 0.2.0；对仍为 legacy 的根执行 `recover` 不创建 state，Node tripwire 未触发且未观察到 legacy 进程；中断后的 `recover` 同样保持 native state 字节不变 |
| legacy 回滚拒绝 | `rollback --to 0.1.0 --offline` 失败并报 `legacy node package`；current/state/legacy 树不变 |
| 重复 install | 迁移后再次 `install --version 0.2.0` 报 already installed；state 与 legacy 树不变 |

测试输出的 `F3-EVIDENCE: F3-09 PASS` 行包含上述每条断言；完整日志在仓库外
`<f3-legacy-real-run>/logs/f3-legacy-real.log`，版本清单在
`<f3-legacy-real-run>/logs/f3-legacy-real-versions.txt`。

## 6. 与既有 switch 窗口证据的关系

本证据新增的是**包身份**：legacy current 不再是生产 v1 packager 生成的 shell-script
fixture，而是从 `e4fed4a` clean archive 构建的真实 Rust release 二进制、真实
`backends/typescript` 构建产物和真实 Zed wasm 组成、经生产 v1 packager 打包并由生产 v1
安装器安装的 schema v1 包。

进程级 `SIGKILL` 的 L3（switch 前）/L4（switch 后）仍由
[`2026-11-20-tsgo-f3-kill-window.md`](2026-11-20-tsgo-f3-kill-window.md) 在 `28c5e18` 的
`f3_kill_window` 中覆盖；本轮按任务允许使用生产 `AfterSwitch` failpoint 复验中断前滚语义，
不把 failpoint 冒充进程级 SIGKILL。`28c5e18` 的 L3/L4 使用 fixture legacy 包；本轮迁移
成功/失败/回滚/恢复使用真实 v1 包，两者互补。

## 7. 限制与边界

1. **不是历史发布产物**：本包从 `e4fed4a` 的 clean `git archive` 用生产 v1 packager 构建，
   没有历史 tag、发布记录或签名。合同子条件要求的是“真实包而非 fixture”；若维护者要求
   历史产物的 provenance，应把 F3-09 保持 partial 并另开 waiver。
2. **legacy 二进制默认后端**：`bin/wxml-lsp` 是 `e4fed4a` 的 release 二进制，默认 `--stdio`
   走 native/HostOnly；legacy Node 入口通过显式 `--backend node` 验证。历史 v1 发布包的默认
   后端可能是 Node，本证据不声明该默认行为等价。
3. **无 exec 审计的 legacy 入口不执行证明**：安装器和迁移命令在 Node tripwire 环境运行，
   能证明没有 Node 家族执行；未在本轮用 ptrace 记录 Rust `versions/0.1.0/bin/wxml-lsp` 的
   `execve`。安装的 post-install `--version` 探针是 schema v1 安装的预期执行；迁移阶段没有
   legacy 入口可观察副作用。
4. **native 目标 provenance**：N2 0.2.0 是 pinned real-asset 通道对 N1 生产二进制做同长度
   版本常量重标的结果，不是独立发布的第二版本；该限制属于 F3-06 的 provenance 缺口，不是
   F3-09 的包身份子条件。
5. **平台/CI**：只在 Linux x86_64、opt-in、有 Node/npm 和 pinned tarball 的环境运行；不是
   默认 CI，不是 clean-env/CI 矩阵，也不是签名/发布通道验收。
