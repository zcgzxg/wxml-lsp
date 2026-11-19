# tsgo F3 真实 release 验收（2026-11-19）

## 0. 结论摘要

本次在真实 `assets.lock.json`、真实 7.1.0-beta 引擎 tarball、真实
`miniprogram-api-typings-5.3.1` tarball 和真实 Rust/Zed 产物上执行了 F3
包级/安装级验收。7 个 opt-in 测试全部通过；两次独立构建的
archive/manifest/index/SHA256SUMS 逐字节一致；真实安装、`verify`/`current`/
`env`、N1→N2→N1、`recover`、真实归档篡改拒绝、ABI 测量和 Zed 进程级 smoke
均有实测输出。

| ID | 本次结论 | 是否可关闭 | 依据 |
| --- | --- | --- | --- |
| F3-03 | **PASS（真实证据）** | 可按合同评审关闭 | 两次真实资产构建 archive/manifest/index/SHA256SUMS 相同；157 文件；无 Node/JS/`wxml-type-backend`；许可文件齐全 |
| F3-04 | **PASS（真实证据）** | 可按合同评审关闭 | 真实包空安装成功；精确树、原子 `current`、`state.json`、空 `tmp/`、`verify` 通过 |
| F3-05 | **PARTIAL** | 否 | 真实归档 SRI 篡改在激活前拒绝且无 `current`/`versions`；缺“真实包缺标准库/typings/引擎探针失败”三例 |
| F3-06 | **PARTIAL** | 否 | 真实资产 N1→N2 成功、N1 树不变；N2 的 Rust 二进制是 N1 生产二进制做同长度版本常量替换，且失败/中断窗口仍只有 fixture 注入证据 |
| F3-07 | **PASS（真实证据）** | 可按合同评审关闭 | 真实 `rollback --to 0.1.0 --offline` 重新校验/探针后切换；`current=0.1.0`、`state.previous=0.2.0`；网络命名空间内完成 |
| F3-10 | **PARTIAL** | 否 | 真实 archive SRI 篡改在激活前拒绝；逐文件/manifest/许可篡改仍只有 fixture 级覆盖 |
| F3-12 | **PASS（真实证据）** | 可按合同评审关闭 | 无源离线重装命中已验证 archive cache；损坏 cache 在激活前拒绝；空 cache 无源 fail closed |
| F3-13 | **PASS（测量完成）** | 可按合同评审关闭 | `readelf`/`ldd`/GLIBC 实测；Rust 需 GLIBC_2.39，宿主 2.43；引擎静态；`linux-aarch64` 在任何输出写入前拒绝 |
| F3-14 | **BLOCKED** | 否 | 真实 Zed 启动了打包的 `wxml-lsp --stdio` 且无 Node 子进程，但 `--stdio` 仍默认 Node 后端，且缺 UI/查询证据 |
| F3-02 | **BLOCKED** | 否 | 主机无 `strace`/等价追踪器；PATH tripwire 和 `/proc` 观察只能算辅助，不能替代 `execve/execveat` 原始日志 |

本次**不修改**任何合同状态或台账；上表的“可关闭”只表示本文件已经提供合同
要求的实测项，最终关闭仍需合同 owner 在证据评审后记录。

## 1. 身份、工作树与并发提交

- 任务指定工作区：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务指定 HEAD：`9a8b65c`
- 验收运行期间另一会话连续提交，实际运行 HEAD 已推进到：`561b00a`
  （`feat(tsgo): validate installed type assets`，2026-11-19 23:09:29 +0800）
- 运行期间出现的提交：`1dbeac7`、`6207c9e`、`e5f3893`、`561b00a`
- 本次新增（未提交）：
  - ` crates/wxml-delivery/tests/f3_real_release.rs`
  - ` crates/wxml-delivery/tests/f3_real_release/`（`support.rs`、`lsp.rs`、`package.rs`、`runtime.rs`、`abi.rs`）
  - ` scripts/f3-real-release.sh`
  - 本文件
- 本次未修改生产代码、CLI、manifest、provision、Cargo 文件或现有测试；未删除
  Node；未切换默认 backend；未访问网络；未 commit。

**交付侧源码与二进制的关系**：实际打包/安装使用的
`wxml-lsp-package`（22:52:53）和 `wxml-lsp-install`（22:52:49）构建于
2026-11-19 22:52（+0800）。当时 `crates/wxml-delivery/src` 的 mtime 均早于
22:47，已包含 `6207c9e` 的 delivery 改动；之后的 `e5f3893`/`561b00a` 只改
`crates/wxml-lsp`。因此 F3-03/05/06/07/10/12/13 的 delivery 证据对应当前
`561b00a` 的 delivery 源码。打包进安装包的 `bin/wxml-lsp` 构建于 23:21:19，
来自当时工作树（含并发会话未提交的 `crates/wxml-lsp` 改动），不是
`561b00a` 的逐字节重建；F3-04 的原生 LSP smoke 对该二进制有效，但不声明
“HEAD 561b00a 的 wxml-lsp 重建一致”。

## 2. 输入资产与 lock 身份

| 项 | 值 |
| --- | --- |
| 仓库 lock | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/assets.lock.json` |
| lock SHA-256（canonical 与文件字节一致） | `ce33fa960aa3eaf433419e2baeb6709587783ec11450642b6b1188c597725456` |
| 引擎 tarball | ` /tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz` |
| 引擎 tarball SHA-256 / 大小 | `59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855` / 10,175,135 B |
| typings tarball | ` /tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz` |
| typings tarball SHA-256 / 大小 | `e5f8b50fe25b8979432060b915eb1cdfd1c91852e7e3842f3bfca932bdbd4b71` / 449,429 B |
| 引擎版本/gitHead | `7.1.0-beta` / `3e9236361fa97a9e6c685f4833ac457ae2d8c50a` |
| 引擎二进制 | `engine/lib/tsc` SHA-256 `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70`，24,522,912 B，0755 |
| provisioning cache key（engine） | `5ee874f4f3310cc5c80c4ec4296dafbbb0fd067bc0cb16751f757f5d40fd196d.ce33fa960aa3eaf4` |
| provisioning cache key（typings） | `12a224f865880487f63b78d35f5e81835b1542c2bdb3a1e43bad0bc0eda7facb.ce33fa960aa3eaf4` |
| typings payload | 23 files / 2,548,075 B / tree `12a224f8...7facb` |
| engine payload | 122 files / 28,450,796 B / tree `5ee874f4...196d` |

`wxml-lsp-package provision` 两次运行：第一次 source `from-dir`，第二次删除
`--from-dir` 后 source `cache`；`--print-plan` 显示两个 `cache hit`。没有下载
路径，`--allow-download` 仍被拒绝。

## 3. F3-03：真实包、两次构建逐字节一致

运行命令（脚本内部，实际参数见 ` scripts/f3-real-release.sh`）：

```bash
TSGO_NATIVE_TARBALL=/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz \
TSGO_TYPINGS_TARBALL=/tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz \
WXML_F3_LSP_BIN=/tmp/opencode/f3-real-release/target/release/wxml-lsp \
WXML_F3_INSTALL_BIN=/tmp/opencode/f3-real-release/target/release/wxml-lsp-install \
WXML_F3_ZED_WASM=/tmp/opencode/f3-real-release/zed-target/wasm32-wasip2/release/zed_wxml.wasm \
WXML_REQUIRE_REAL_ENGINE=1 \
cargo test -p wxml-delivery --test f3_real_release -- --nocapture --test-threads=1
```

`wxml-lsp-package native` 的输入树只含 `bin/wxml-lsp`、`bin/wxml-lsp-install`、
`zed-extension/`（真实 wasm + `extension.toml` + `languages/`）和
`THIRD_PARTY_LICENSES.md`；`engine/`、`typings/` 全部来自验证后的 cache。

两次独立构建（不同 output 目录）结果：

| 项 | 值 |
| --- | --- |
| archive | `wxml-lsp-0.1.0-linux-x86_64.tar.gz` |
| archive SHA-256 | `3fa97f2e5c054ad9a0db6c7c5d5deb2985d516aaeaee5db128f737103c393c93` |
| archive SRI | `sha512-3/Rca9t7Qbm2r43AqjDQRcmLTgcC4s5FDCGRCtzFS7HobPnVqEWqKliXW4BOWK3FZF+A4Xf/b1ouS5hgePlIOA==` |
| archive size | 15,182,086 B |
| manifest SHA-256 | `cd560d5d798f7c17c67bbec66f4e709aa3168a803945efbc1f060214c0f7b53b` |
| output index SHA-256 | `f95ae1a15d714f80eab732aa21e2ef28407f0bc508439269530208c849771f46` |
| output SHA256SUMS SHA-256 | `935d01ec06628fc851074e8cf97965de9bc47fae81d0dafdfbc42093e93fa558` |
| tree SHA256SUMS SHA-256 | `23cc728ed4f7e5040ea6e5c1b05dc0a6e0b16759295fc769ff738f8bf0fb5972` |
| provenance.json SHA-256 | `b3001987c54845fd42f1c19484f78bdc4ccf9442f73bb25b78612764826addf7` |
| 包内文件数 / 解包字节 | 157 / 42,107,029 B |
| 包树指纹 | `0208bf5e43317ff517ca1a8558f036d4d423cd29698ff4605a10c6b13a8c1ee5` |

manifest 身份：engine `typescript 7.1.0-beta`，distribution SHA-256
`59128685...1855`，distribution file count 122，unpacked size 28,450,796；
`typeAssets` 为 `typescript-standard-lib` 116 文件和 `miniprogram-api-typings`
23 文件；`binaries` 4 项（两个 Rust binary、native engine、Zed wasm）。
`provenance.json` 记录 `assetsLockSha256=ce33fa96...5456` 和两个 asset 的
`payloadTreeDigest`。负向扫描：无 `node_modules/`、`wxml-type-backend`、
`.js/.cjs/.mjs/.node` 或 Node 锁文件；`SHA256SUMS` 等于 `files[] + manifest.json`。

可复现产物：

```text
/tmp/opencode/f3-real-release/tmp/f3-real-release-keep/artifacts/wxml-lsp-0.1.0-linux-x86_64/
/tmp/opencode/f3-real-release/tmp/f3-real-release-keep/artifacts/wxml-lsp-0.2.0-linux-x86_64/
```

`0.2.0` 目录是双版本 channel，含 0.1.0 和 0.2.0 两个 archive、index 与 SHA256SUMS。

## 4. F3-04：真实包安装与 CLI 视图（干净环境）

- PATH：只含 Node tripwire shim 目录（`node/nodejs/npm/npx/pnpm/yarn/bun`，
  执行即创建 marker 并退出 99）；shim 正向控制通过。
- env：`env_clear` 后只保留 `PATH/HOME/XDG_DATA_HOME/TMPDIR/LANG/LC_ALL/
  NO_COLOR/WXML_NODE/NODE/WXML_TYPE_BACKEND/WXML_BACKEND_DIR/HTTP(S)/ALL_PROXY`；
  `WXML_NODE`/`NODE` 指向 tripwire，代理指向 `127.0.0.1:9`。
- cwd：测试私有 `work/`，不在仓库内。
- 运行在 `unshare -rn` 网络命名空间（`lo DOWN`），`--offline`。
- `/proc` probe：子进程 `PATH` 只有 shim 目录，`PWD` 为私有 work 目录，
  `/proc/$$/exe` 为 `/usr/bin/dash`；没有 `NODE_PATH` 等注入变量。

实测：

```text
installed 0.1.0 (native)
engine: typescript 7.1.0-beta
verify: verified 0.1.0 (native), engine version: 7.1.0-beta
current: version 0.1.0, platform linux-x86_64, engine typescript 7.1.0-beta
env: export WXML_LSP_BIN=<root>/current/bin/wxml-lsp   （仅一行）
```

安装树：`verify_tree_v2` 通过，157 文件；`current` 原子链接指向
`versions/0.1.0`；`state.json` `active=0.1.0`、`previous=null`、
`lastTransaction.op=install`；`tmp/` 与 `txn/` 为空；root 与版本树权限保持
`0700/0755`。安装目录：

```text
/tmp/opencode/f3-real-release/tmp/f3-real-release-keep/install-root
```

额外原生 LSP smoke：用安装后的
`/tmp/.../install-root/current/bin/wxml-lsp --stdio --backend native`
完成 `initialize`、`textDocument/documentSymbol` 和 `textDocument/hover`；
`positionEncoding=utf-16`、`hoverProvider=true`。`documentSymbol` 返回
`["view", "button"]`，hover 返回非空 TypeScript Markdown；`/proc` 观察到
安装树内的 `wxml-lsp` 和 `engine/lib/tsc` 两个进程，没有 Node 子进程。
该 smoke 证明原生 LSP 入口可运行，但不替代 F3-02 的 `execve` 审计。

## 5. F3-06/F3-07：N1→N2→N1、离线回滚与 recover

双版本 channel（同一真实引擎/typings/输入树）安装结果：

| 步骤 | 结果 |
| --- | --- |
| install N1 | `installed 0.1.0 (native)`；`state.active=0.1.0`、`previous=null` |
| upgrade N2 | `upgraded to 0.2.0 (native)`；`state.active=0.2.0`、`previous=0.1.0`；N1 树指纹不变 |
| rollback N1 | `rolled back to 0.1.0`；`state.active=0.1.0`、`previous=0.2.0`；N1/N2 树指纹均不变 |
| recover（注入 staged 0.3.0 + 遗留 tmp） | `staged 0.3.0 -> removed temporary state; current unchanged`；`current: 0.1.0`；journal 清空、tmp 清空、state/current/两棵树不变 |
| recover（再次） | `no recovery actions were needed` |

N2 archive：`bb3c8332aa8524696b1ce3c6b9a2c48ca6077f70e0a70027cecb9f8c277dcdc8`，
15,182,080 B；manifest `65eb0f9df4bf2b0c208abb7b33f1e552b37e65e4f9b3c414116bb996a15cd2cb`。

**N2 限制（重要）**：本提交没有第二个真实 `0.2.0` 生产 release。N2 的
`bin/wxml-lsp`/`bin/wxml-lsp-install` 是 N1 生产二进制，仅把唯一的同长度
内嵌版本常量 `0.1.0` 替换为 `0.2.0`，以满足安装器激活前的
`--version` 探针；engine/typings 和其它字节全部来自真实包，archive/manifest
和全部校验都是真实重算的。因此 F3-06 只算 **PARTIAL**：真实资产的正常
upgrade 已跑通，失败/中断窗口仍只有 `native_delivery.rs`/`native_rollback.rs`
的 fixture 注入证据。

保留的安装根：

```text
/tmp/opencode/f3-real-release/tmp/f3-real-release-keep/transitions-root
```

## 6. F3-10：真实归档篡改在激活前拒绝

复制真实 N1 channel，翻转 archive 最后一个字节（保持 index 不变），执行
`install --from-dir <tampered> --version 0.1.0 --offline`：

```text
error: package integrity check failed: asset SHA-512 SRI does not match
```

安装根没有 `current`、`versions` 或 `state.json`；`tmp/` 为空；root 仍为
`0700`；tripwire 未触发。保留的失败根：

```text
/tmp/opencode/f3-real-release/tmp/f3-real-release-keep/tamper-root
```

逐文件哈希、manifest、额外/缺失文件、许可文件篡改在真实包上尚未覆盖，
因此 F3-10 仍为 **PARTIAL**。

## 6.1 F3-12：真实归档缓存与 fail-closed

真实 N1 包首次 `--from-dir` 安装后，安装根下的
`cache/v2/` 保存了已验证的 index 与 archive。随后删除 `current`、
`versions`、`state.json`、`txn`、`tmp`（保留 `cache/`），执行不带
`--from-dir` 的 `install --offline`：

```text
installed 0.1.0 (native)
```

证明无源离线重装只使用缓存中已验证的字节。再把缓存 archive 最后一字节
翻转后重复安装：

```text
error: package integrity check failed: asset SHA-512 SRI does not match
```

没有 `current`/`state.json`/激活版本。最后在完全没有 cache 和 source 的
空根上执行 `install --offline`，失败信息为 `no cached native index`，且不
创建 `current`/`state.json`/`versions`；代理仍是 `127.0.0.1:9`，tripwire
未触发。F3-12 的“有效缓存可安装、损坏缓存失败、无缓存 fail closed”在真实
包上均有证据；网络侧仍是命名空间/死代理级证据，不是 `connect` syscall 审计。

## 7. F3-13：平台/ABI 实测

`readelf`/`ldd` 输出（`LC_ALL=C`）：

| 文件 | 类型 | 动态/静态 | 模式 | 大小 | SHA-256 |
| --- | --- | --- | --- | --- | --- |
| `bin/wxml-lsp` | ELF64 x86-64 PIE | 动态，NEEDED+`libc.so.6` | 0755 | 5,613,712 | `0e2ee7ce9427c742bf53c5d07d16a9965b65bcfca7a655eac16ef52f2f5eca5b` |
| `bin/wxml-lsp-install` | ELF64 x86-64 PIE | 动态，NEEDED+`libc.so.6` | 0755 | 5,190,952 | `ab74ddcec917ea94e1dcd531182f1225607a50eee57d5044800064d3e22984f2` |
| `engine/lib/tsc` | ELF64 x86-64 EXEC | 静态（`ldd` 报 not a dynamic executable） | 0755 | 24,522,912 | `a183b195...faaa70` |

- Rust 二进制最高 `GLIBC_*` 符号版本：**GLIBC_2.39**；宿主 `ldd (Debian
  GLIBC 2.43-7) 2.43`。`required <= host` 成立。
- 引擎为静态 Go 二进制，无 `NEEDED` 项；不能据此推断 Rust 二进制也静态。
- `lib/tsc --version` 在空环境下输出 `Version 7.1.0-beta`。
- `wxml-lsp-package native --platform linux-aarch64` 在任何输出写入前失败，
  错误包含 `support only platform \`linux-x86_64\``；输出目录未创建/为空。
- 本发布不产生 musl artifact；宿主是 glibc userland。F3-13 的
  `readelf`/`ldd`/`GLIBC_*`/glibc 下限/错误平台拒绝项已齐备。

## 8. F3-14：Zed 真实 smoke（进程级）

本机有 `zed 1.29.2`、Wayland `wayland-0` 和已安装的 dev extension
（`~/.local/share/zed/extensions/installed/wxml-lsp ->
/home/zxg/code/rust/wxml-lsp/editors/zed`）。使用隔离用户数据目录、隔离
`HOME/.bashrc`（`WXML_LSP_BIN` 指向真实安装根）、
`trust_all_worktrees=true`，打开 `pages/index/index.wxml`。

Zed 日志与进程：

```text
[wxml] starting language server binary `wxml-lsp`
INFO [lsp] starting language server process. binary path:
  "/tmp/opencode/f3-real-release/tmp/f3-real-release-keep/install-root/current/bin/wxml-lsp",
  working directory: ".../zed-project/pages/index", args: ["--stdio"]
965515 /tmp/.../install-root/current/bin/wxml-lsp --stdio
```

`pstree`/`ps` 未观察到 Node 子进程。**但 F3-14 仍 BLOCKED**：
`wxml-lsp --stdio` 当前默认仍是 Node 后端（原生需要 `--backend native`），
且本次没有 UI 查询/未保存脚本/五语言注册的截图或诊断证据。进程启动成功
不能替代合同要求的 UI smoke。

Zed 日志：` /tmp/opencode/f3-real-release/logs/zed-smoke.log`。

## 9. 未关闭项与 BLOCKED

1. **F3-02 — BLOCKED（无进程审计）**：主机没有 `strace`/`ltrace`/`perf`/
   `bpftrace`/`auditctl`。本次 PATH tripwire、绝对路径 poison 变量和
   `/proc` 进程观察均为辅助证据，不能证明绝对路径 Node 未被 exec。
2. **F3-05 — PARTIAL**：真实包的“缺标准库/typings/引擎探针失败”三例未在
   真实包上执行；fixture 测试不能替代真实资产证据。
3. **F3-06 — PARTIAL**：N2 是版本常量重标的 N1 生产二进制；失败/中断窗口
   只有 fixture 注入证据。
4. **F3-10 — PARTIAL**：真实包只覆盖 archive SRI/SHA-256 篡改；逐文件/
   manifest/额外文件/许可篡改仍只有 fixture 级覆盖。
5. **F3-14 — BLOCKED**：真实 Zed 启动 `wxml-lsp --stdio` 成功且无 Node
   子进程，但默认后端仍是 Node，缺 UI/查询证据。
6. **干净环境**：`unshare -rn` + 只读/隔离挂载是命名空间级证据，不是
   `connect`/`openat` syscall 审计；`/proc` 观察不是 `execve` 追踪。
7. **HEAD 漂移**：任务指定 `9a8b65c`，实际运行在并发会话提交后的
   `561b00a` 工作树；delivery 二进制对应当前 delivery 源码，但
   `bin/wxml-lsp` 是 23:21 从含并发未提交改动的工作树构建的，不是
   `561b00a` 的逐字节重建。

## 10. 测试、命令与日志

测试文件：` crates/wxml-delivery/tests/f3_real_release.rs`（7 个测试：
2 个 package、4 个 runtime、1 个 ABI）。缺失资产时 opt-in skip；
`WXML_REQUIRE_REAL_ENGINE=1` 时缺失引擎/typings 或 Rust/Zed 产物直接 fail
closed。

最终运行：

```bash
./scripts/f3-real-release.sh --output /tmp/opencode/f3-real-release --network-isolate
```

结果：

```text
running 7 tests
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
F3-TEST: real-release suite passed
F3-14: Zed started the packaged `wxml-lsp --stdio`; no Node child was observed
F3-14: BLOCKED until `wxml-lsp --stdio` defaults to the native backend ...
```

### 10.1 最终批量检查

在同一工作树上执行一次批量验证，全部通过：

```text
cargo fmt --all -- --check                                  exit 0
cargo check --workspace --all-targets                       exit 0
cargo clippy --workspace --all-targets -- -D warnings       exit 0
cargo fmt -p wxml-delivery -- --check                       exit 0
cargo clippy -p wxml-delivery --all-targets -- -D warnings exit 0
cargo test -p wxml-delivery --test f3_real_release          exit 0
  -> 0 passed; 7 ignored（默认运行只报告 ignore，不执行真实资产套件）
```

真实 opt-in 由 ` scripts/f3-real-release.sh --network-isolate` 执行，
`7 passed; 0 failed`。批量日志在 `/tmp/opencode/f3-real-release/batch-*.log`。

fail-closed 负向检查：清空 `TSGO_NATIVE_TARBALL`/`TSGO_TYPINGS_TARBALL` 等
输入但保留 `WXML_REQUIRE_REAL_ENGINE=1` 后，测试立即失败：

```text
TSGO_NATIVE_TARBALL is required when WXML_REQUIRE_REAL_ENGINE=1
(pinned engine tarball for real_assets_provision)
```

批量检查完成后并发会话继续修改 `crates/wxml-lsp`；上述静态检查结果只代表
该次运行快照，不声明当前工作树持续通过。

日志与产物：

```text
/tmp/opencode/f3-real-release/script-run-final.log
/tmp/opencode/f3-real-release/logs/f3-real-release.log
/tmp/opencode/f3-real-release/logs/zed-smoke.log
/tmp/opencode/f3-real-release/tmp/f3-real-release-keep/artifacts/
/tmp/opencode/f3-real-release/tmp/f3-real-release-keep/install-root/
/tmp/opencode/f3-real-release/tmp/f3-real-release-keep/transitions-root/
/tmp/opencode/f3-real-release/tmp/f3-real-release-keep/tamper-root/
/tmp/opencode/f3-real-release/target/release/wxml-lsp
/tmp/opencode/f3-real-release/target/release/wxml-lsp-install
/tmp/opencode/f3-real-release/zed-target/wasm32-wasip2/release/zed_wxml.wasm
```

## 11. 边界声明

- 本文件与三个新增路径是本次唯一写入；未提交。
- 未修改生产代码、CLI、manifest、provision、Cargo 文件或现有测试。
- 未删除 Node、未切换默认 backend、未启用下载、未访问外部网络。
- 本文件不把 fixture 测试、PATH shim、`/proc` 观察或隔离挂载扩大为
  F3-02/F3-14 的关闭证据。
