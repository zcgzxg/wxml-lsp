# P7 删除后最终 native release/install/no-Node 验证（2026-11-20）

## 0. 结论先行

在 P7 删除补丁已应用（未提交）的工作树上，按最终 native 路径重跑了
release 构建、安装/校验、PATH-only 无 Node 检查、安装后 `--stdio` LSP
冒烟和真实 Zed 冒烟。**五项全部通过，无 BLOCKED。**

| # | 检查 | 结果 |
| --- | --- | --- |
| 1 | `scripts/build-release.sh` native release（离线、无 Node exec） | **PASS**，archive SHA-256 `b138d119…f30959` |
| 2 | `wxml-lsp-install` 安装 + `verify`/`current`/`env` | **PASS**，157 files / 41,899,646 bytes |
| 3 | PATH 仅 Node tripwire shim 下 `verify`/`current` | **PASS**，positive control 生效，marker 未出现 |
| 4 | 安装后 `bin/wxml-lsp --stdio` LSP 冒烟 | **PASS**，唯一子进程为包内 `engine/lib/tsc` |
| 5 | Zed smoke（真实 Zed 1.29.2，Xvfb） | **PASS**，16/16 检查 |

本记录只证明“删除补丁应用后的 native 交付路径”可构建、可安装、可运行且不拉起
Node；**不**代替整个 P7 的 `removed` 文档口径、workspace 全量测试、native
gates、升级/回滚或跨平台验收。

## 1. 身份与边界

| 项 | 值 |
| --- | --- |
| 主树 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `3e24c444e87de13a20d4a67367e4d8e99b940524` |
| P7 删除补丁 | 已应用、**未提交**（`git status` 显示删除/修改；无 commit） |
| 平台 | Linux x86_64 |
| 源码/测试/ledger/README | 未修改 |
| 新增 | 仅本证据文档 |
| 运行产物 | `/tmp/opencode/p7-final-release`、`/tmp/opencode/p7-final-install`、`/tmp/opencode/p7-logs`、`/tmp/opencode/p7-zed-smoke` |
| 发布 | 无 upload / tag / marketplace / 远端动作 |

构建直接使用工作树（未加 `--version`），因此删除后的源码树就是被测输入；
没有从 `git archive HEAD` 取已提交版本，避免绕过未提交的 P7 删除补丁。

## 2. Release 构建

命令：

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
scripts/build-release.sh \
  --output /tmp/opencode/p7-final-release \
  --cache /tmp/opencode/assets-lock-root/cache \
  --from-dir /tmp/opencode/assets-lock-root/from-dir
```

结果：`EXIT=0`。日志 `/tmp/opencode/p7-logs/build-release.log`。

- 构建机 PATH 上有 Node（`node: present on PATH`），但构建脚本自带 PATH
  tripwire 未被触发；日志末尾为 `node: not executed (PATH tripwire clean)`。
- 资产来自已验证 cache / `--from-dir`，未下载、未联网。
- `wxml-lsp-install verify-package` 与顶层 `sha256sum -c SHA256SUMS` 均在
  构建脚本内通过。

### 2.1 产物身份

| 项 | 值 |
| --- | --- |
| release 目录 | `/tmp/opencode/p7-final-release/wxml-lsp-0.1.0-linux-x86_64` |
| archive | `/tmp/opencode/p7-final-release/wxml-lsp-0.1.0-linux-x86_64.tar.gz` |
| archive SHA-256 | `b138d119929a5928811f49b943c6b79d4588b55926121b2e03a5509322f30959` |
| archive size | 15,117,268 bytes |
| 目录文件数 | 159（11 个目录） |
| `SHA256SUMS` 条目 | 158 |
| manifest payload 文件数 | 157（不含 `manifest.json` 与 `SHA256SUMS`） |
| 目录内容 SHA-256 | `cbb01d26e390ee5b5e2f4f6b1ca48ad3fd2a82dc2486181a32c318dd0242c776` |
| `manifest.json` SHA-256 | `a9d2fe2e9d3715465296a92567c5984661a382bf4394d1cd08bf2ddf99e00ac8` |
| `bin/wxml-lsp` SHA-256 | `aa8aa03b40d3445a96405079cc6ab72c8a0a342ed5292306374d9e7540fe77e2` |
| `bin/wxml-lsp-install` SHA-256 | `cc8070fad9a863fb6576a453d2db00af04657ca3a414c53f156bb62b574b45f5` |
| `zed-extension/extension.wasm` SHA-256 | `488531eaec969281fa295803e2d3cd857493bd9e26f56adc1c21a71bad0aeb1f` |

目录内容 SHA-256 的计算方式：按相对路径 `LC_ALL=C sort` 后，对每个文件执行
`sha256sum`，再对整段输出取 SHA-256。

### 2.2 Engine

| 项 | 值 |
| --- | --- |
| 引擎 | TypeScript `7.1.0-beta` |
| source commit | `3e9236361fa97a9e6c685f4833ac457ae2d8c50a` |
| `engine/lib/tsc --version` | `Version 7.1.0-beta` |
| `engine/lib/tsc` SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| size / mode | 24,522,912 bytes / `0755` |
| distribution | `@typescript/typescript-linux-x64` 7.1.0-beta |
| distribution SHA-256 | `59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855` |

### 2.3 无 Node 后端检查

对 release 目录、安装目录和归档清单同时检查 `server.js`、`backend_node`、
`backends/typescript`、`node_modules`、`*.js`、`*.mjs`、`*.cjs`：

```text
release 目录命中: 无
安装目录命中:     无
archive 清单命中: 无
```

归档中非 `.d.ts` 的脚本仅有 Rust 二进制、`engine/lib/tsc`、Zed wasm、
`.scm`、`.toml`、`.json`、`.md`、license 文本与 `tsc.sig`。日志：
`/tmp/opencode/p7-logs/release-forbidden-scan.log`、`archive-scan.log`。

## 3. 安装与 native 校验

宿主环境预置了 `WXML_LSP_BIN`（指向另一个 worktree 的 release 二进制），
安装器按设计会优先使用它；本次显式清除后安装，避免假阳性。

```bash
REL=/tmp/opencode/p7-final-release
INS=/tmp/opencode/p7-final-install
rm -rf "$INS"
env -u WXML_LSP_BIN "$REL/wxml-lsp-0.1.0-linux-x86_64/bin/wxml-lsp-install" \
  install --root "$INS" --from-dir "$REL" --version 0.1.0 --offline
env -u WXML_LSP_BIN "$INS/current/bin/wxml-lsp-install" verify --root "$INS"
env -u WXML_LSP_BIN "$INS/current/bin/wxml-lsp-install" current --root "$INS"
env -u WXML_LSP_BIN "$INS/current/bin/wxml-lsp-install" env --root "$INS"
```

结果（日志 `/tmp/opencode/p7-logs/install.log`）：

```text
installed 0.1.0 (native)
engine: typescript 7.1.0-beta
binary: /tmp/opencode/p7-final-install/current/bin/wxml-lsp

verified 0.1.0 (native)
engine version: 7.1.0-beta
files: 157  bytes: 41899646
binary: /tmp/opencode/p7-final-install/current/bin/wxml-lsp

version: 0.1.0
platform: linux-x86_64
engine: typescript 7.1.0-beta
manifest protocol: 2

export WXML_LSP_BIN='/tmp/opencode/p7-final-install/current/bin/wxml-lsp'
```

`current -> versions/0.1.0`；`state.json` 的 active 为 `0.1.0`，最近事务
`install/activated`。安装目录 159 个文件，`verify` 统计的 payload 为 157
个文件（不含 `manifest.json`、`SHA256SUMS`）。

## 4. PATH-only Node tripwire

使用仓库现有 `scripts/verify-no-node.sh`（只读使用，未修改），PATH 只含
`node/nodejs/npm/npx/pnpm/yarn/bun` shim：

```bash
WXML_LSP_INSTALL="$INS/current/bin/wxml-lsp-install" \
WXML_INSTALL_ROOT="$INS" \
TSGO_ASSET=/tmp/opencode/assets-lock-root/from-dir/typescript-linux-x64-7.1.0-beta.tgz \
WXML_REQUIRE_REAL_ENGINE=1 \
WXML_REQUIRE_LSP_NATIVE=1 \
WXML_TEST_TMPDIR=/tmp/opencode \
scripts/verify-no-node.sh
```

结果：`EXIT=0`。日志 `/tmp/opencode/p7-logs/verify-no-node.log`。

```text
NO_NODE: positive control exercised (shim exit 99, marker created)
verify: verified 0.1.0 (native)
current: binary: /tmp/opencode/p7-final-install/current/bin/wxml-lsp
env: export WXML_LSP_BIN='/tmp/opencode/p7-final-install/current/bin/wxml-lsp'
NO_NODE: PASS (installer scope: verify/current/env)
LSP_NATIVE: PASS (main.rs has no NodeBackend or serve_node(); --stdio is native-only)
REAL_ENGINE: PASS (sha256=a183b195…faaa70, Version 7.1.0-beta)
F3-02: BLOCKED (installer scope only; no closure claim)
```

positive control 证明 shim 会置 marker；随后三个命令运行后 marker 与 attempt
log 均不存在。该脚本明确不把 PATH shim 当作 execve 审计，因此这里只下
“安装器作用域内无 Node 执行迹象”的结论。

说明：脚本尾部的 `F3-02: BLOCKED (installer scope only; no closure claim)`
是脚本自身对“installer-scope 不能关闭 F3-02”的既有 residual 标注，不是本次
release/install/LSP/Zed 验证被阻塞；本次五项执行均为 PASS。

## 5. 安装后 `--stdio` LSP 冒烟

客户端为 `/tmp/opencode/p7-lsp-smoke2.py`（临时 harness，不写入仓库）。环境：
`PATH` 只含上述 tripwire shim，另设隔离 `HOME`/`TMPDIR`，不设置
`WXML_LSP_BIN`、`WXML_TYPE_BACKEND`、`WXML_NODE`。fixture 从仓库只读复制到
`/tmp/opencode/p7-lsp-project2`（`tests/fixtures/cross-file-type`）。

命令：

```bash
python3 /tmp/opencode/p7-lsp-smoke2.py \
  /tmp/opencode/p7-lsp-project2 \
  pages/index/index.wxml \
  pages/index/index.ts
```

结果：`EXIT=0`，`RESULT: PASS`。日志
`/tmp/opencode/p7-logs/lsp-smoke2.log`。

| 步骤 | 结果 |
| --- | --- |
| `initialize` | OK，`positionEncoding=utf-16`，`serverInfo={"name":"wxml-lsp","version":"0.1.0"}` |
| didOpen WXML | 诊断 `TS2339`：`Property 'nmae' does not exist on type 'User'.` |
| didOpen TS | 诊断为空（脚本本身类型正确） |
| hover `{{ user.name }}` | `User` + 注释 `当前登录用户，类型来自 types.ts` |
| documentSymbol WXML | `view` + 4×`text` |
| documentSymbol TS | `user`（kind 13） |
| `shutdown` | OK |
| 进程退出 | exit 0 |

进程采样：整个会话中 wxml-lsp 的唯一子进程是包内 native engine：

```text
/tmp/opencode/p7-final-install/versions/0.1.0/engine/lib/tsc --api --async \
  --cwd /tmp/opencode/wxml-lsp-<pid>-… \
  --useCaseSensitiveFileNames=true --callbacks=readFile,fileExists,…
```

无 `node`、无 `server.js`、无 `backend_node`、无 `backends/typescript`；
Node tripwire marker/attempt log 均不存在。stderr 只有 native partial 能力说明，
没有 Node 后端启动信息。

补充观察（非本次 release 失败）：对 `tests/fixtures/page-basic` 的纯
`Page({...})` TS 文件单独请求 `documentSymbol` 时，服务端返回
`-32803 the script outline is incomplete`。原因与仓库既有设计一致：native
outline 不把表达式语句 `Page(...)` 当作可下钻声明，该 fixture 因此没有可映射
符号；`cross-file-type` 的顶层 `const user` 则正常返回。日志：
`/tmp/opencode/p7-logs/lsp-smoke.log`。

## 6. Zed smoke

环境具备真实 Zed 1.29.2、Xvfb、xdotool、ImageMagick、tesseract；直接执行仓库
现有冒烟脚本（未修改）：

```bash
env -u WXML_LSP_BIN scripts/f3-zed-smoke.sh \
  --package /tmp/opencode/p7-final-install/current \
  --grammar /home/zxg/code/rust/wxml-lsp/editors/zed/grammars/wxml.wasm \
  --output /tmp/opencode/p7-zed-smoke
```

结果：`EXIT=0`，**16/16 CHECK PASS**。日志
`/tmp/opencode/p7-logs/zed-smoke.log`、`/tmp/opencode/p7-zed-smoke/logs/zed.log`。

```text
F3-14-ZED-CHECK: PASS - Zed started the packaged default server (--stdio, no --backend)
F3-14-ZED-CHECK: PASS - ps shows wxml-lsp --stdio exactly (no --backend argument)
F3-14-ZED-CHECK: PASS - no Node process or server.js in Zed's process tree
F3-14-ZED-CHECK: PASS - WXML hover shows the inferred string type
F3-14-ZED-CHECK: PASS - WXML diagnostics show the parser error
F3-14-ZED-CHECK: PASS - Zed shows WXML / JavaScript / TypeScript / TSX / JSON
F3-14-ZED-CHECK: PASS - WXML hover reflects the unsaved script edit (number)
F3-14-ZED-CHECK: PASS - unsaved buffer stayed in memory; index.ts hash is unchanged
F3-14-ZED-CHECK: PASS - exactly one wxml-lsp process remains after all five languages
F3-14-ZED-CHECK: PASS - Zed passed only --stdio; the package selected native
F3-14-ZED-CHECK: PASS - no vtsls/eslint/Node language server in the Zed log
F3-14-ZED-SMOKE: PASS (direct default path; real Zed UI on Xvfb)
```

进程树（`/tmp/opencode/p7-zed-smoke/logs/zed-tree-final.txt`）：

```text
zed
└─ zed-editor
   ├─ zed-editor --crash-handler …
   └─ wxml-lsp /tmp/opencode/p7-final-install/versions/0.1.0/bin/wxml-lsp --stdio
      └─ tsc /tmp/opencode/p7-final-install/versions/0.1.0/engine/lib/tsc --api --async …
```

未出现 `--backend`、`node`、`server.js`、vtsls 或 eslint。截图与 OCR 断言通过；
`index.ts` 未保存编辑前后 SHA-256 均为
`bb0f4353021f182ccdb54291dd1c1df078c1d28decfa361f00932965b6fd5cfe`，磁盘
第 5 行仍为 `name: 'Ada'`。

grammar 说明：release 包按设计不携带 `editors/zed/grammars/wxml.wasm`
（构建日志有 warning），本次冒烟沿用主检出中 pinned revision 的预构建
`wxml.wasm`（SHA-256 `05bd00d1…aab9ad`），不伪造 grammar 产物。

## 7. 限制与未声称

1. **无 execve/strace 审计**：PATH shim + `/proc` 进程树采样只能证明本次运行
   的 PATH 调用和可见子进程；绝对路径 Node 调用未被内核级追踪。
2. **宿主存在无关 Node 进程**：本机 OpenCode/CodeGraph/MCP 自身运行 Node，
   因此没有做全局 `pgrep node` 的“零进程”断言；无 Node 结论限定在 release
   构建、安装器命令、LSP 会话和 Zed 进程树。
3. **page-basic 纯 `Page(...)` 脚本的 outline 为空**：这是既有 native 能力
   边界，不是本包引入；本次以 `cross-file-type` 的顶层声明验证
   `documentSymbol` 可用。
4. **Zed 运行在 Xvfb 虚拟显示**：真实 Zed 进程与 UI 自动化，不是物理显示器；
   grammar 为外部 pinned wasm。
5. **未验证**：workspace 全量测试、native gates 35/35、升级/回滚、双次
   release 确定性、跨平台。本记录不声称这些项通过，也不声称 P7 文档口径已可
   写 `removed`。

## 8. 产物与日志索引

```text
/tmp/opencode/p7-final-release/
/tmp/opencode/p7-final-release/wxml-lsp-0.1.0-linux-x86_64.tar.gz
/tmp/opencode/p7-final-install/
/tmp/opencode/p7-zed-smoke/
/tmp/opencode/p7-logs/build-release.log
/tmp/opencode/p7-logs/release-inventory.log
/tmp/opencode/p7-logs/release-meta.log
/tmp/opencode/p7-logs/release-forbidden-scan.log
/tmp/opencode/p7-logs/archive-scan.log
/tmp/opencode/p7-logs/install.log
/tmp/opencode/p7-logs/verify-no-node.log
/tmp/opencode/p7-logs/lsp-smoke.log
/tmp/opencode/p7-logs/lsp-smoke2.log
/tmp/opencode/p7-logs/zed-smoke.log
/tmp/opencode/p7-logs/package-identity.txt
/tmp/opencode/p7-zed-smoke/logs/zed-tree-final.txt
/tmp/opencode/p7-zed-smoke/logs/unsaved-disk.txt
/tmp/opencode/p7-zed-smoke/shots/*.png
```

关键截图 SHA-256：

| 截图 | SHA-256 |
| --- | --- |
| `20-wxml-hover.png` | `97b677108c5248dbdd4790fbbf9e046d04d3054c8c27505ec188cd1bbb0c8d3f` |
| `21-wxml-diagnostics.png` | `b2c56aea282d13bee051043a578f36de6fe6b22b97348f2a0d4351f6ea545c47` |
| `40-unsaved-edit.png` | `5508feb79e6cd199c6a5a74dadd30bf76170e69b79291fff5beec9ffbdfa2d24` |
| `41-unsaved-hover.png` | `4d3d51e7ff09ed6cc5f5c2bf3e8a83a014f05c49ef53f936c04f730a08a1623d` |
