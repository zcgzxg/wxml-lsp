# P7 删除后最终 release/install/无 Node 独立复验（2026-11-20）

> **时点说明**：本文记录 `b6c1ced`（旧 tag `p7-removed`）时点的复验；后续工作树修复
> （F-02、functional P1、stability 4/1、许可生成）不在本文范围。最终验收对象是工作树，
> 见 [final-docs](2026-11-20-tsgo-p7-final-docs.md) 与
> [crosscheck resolution](2026-11-20-tsgo-p7-crosscheck-resolution.md)。

## 0. 结论摘要

在 P7 删除已提交、tag `p7-removed` 指向当时 HEAD `b6c1ced` 的基线上，
重新执行了 release 构建（两次指定的独立输出 + 一次额外的冷 target 构建）、
安装与 native 校验、PATH-only 无 Node 检查、安装后 `--stdio` LSP 冒烟和真实
Zed 冒烟。

| # | 检查 | 结果 |
| --- | --- | --- |
| 1 | `scripts/build-release.sh` 连续两次构建（A/B） | **PASS**：archive/`index.json`/`SHA256SUMS`/整棵 release 目录逐字节一致；archive SHA-256 `7a51a3e9…dcd81b7`（许可重建前；generator/2 之后为 `793a4f21…2e4077`，见 license 证据） |
| 1b | 额外冷 `CARGO_TARGET_DIR` 独立编译（C） | **PASS**：与 A/B 相同 SHA-256；证明不仅打包阶段、编译阶段也可复现 |
| 2 | `wxml-lsp-install` 安装 + `verify`/`current`/`env` | **PASS**：`0.1.0 (native)`，157 payload files / 41,899,646 bytes |
| 3 | `scripts/verify-no-node.sh`（PATH tripwire + 真实 engine） | **PASS**：positive control 生效，marker 未出现；`REAL_ENGINE: PASS` |
| 4 | 安装后 `wxml-lsp --stdio` LSP 冒烟 | **PASS**：initialize/didOpen/diagnostics/hover/documentSymbol/shutdown 全部通过；唯一子进程为 `engine/lib/tsc` |
| 5 | release/安装/归档禁用内容检查 | **PASS**：无 `backend_node`、`backends/typescript`、`server.js`、`node_modules`、业务外脚本 `.js` |
| 6 | Zed 五语言 + hover 冒烟（Zed 1.29.2 / Xvfb） | **PASS**：16/16 CHECK PASS（截图 9 张） |

本记录只覆盖上述 native 交付路径的最终复验；不代替 workspace 全量测试、
native gates、升级/回滚、跨平台或 F3-02 的 execve 审计。**本次 release/install/
LSP/Zed 范围无 BLOCKED 项**；F3-02 仍是脚本自带的 installer-scope residual
（网络 `connect` 层未审计）、非 Linux 和内核沙箱仍为残余，不是本次验证阻塞。

## 1. 身份、时间与边界

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b` |
| Tag `p7-removed` | `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b`（与 HEAD 相同） |
| 平台 | Linux x86_64 |
| 固定构建时间 | `SOURCE_DATE_EPOCH=1763683200` → `2025-11-21T00:00:00Z` |
| 验证窗口（UTC） | 2026-11-20T01:34Z ～ 01:44Z |
| 仓库文件改动 | 只新增本证据文档；未改生产/测试/脚本/README/ledger；未 commit |
| 发布动作 | 无 upload / tag / marketplace / 远端请求 |

**并发工作树变化（非本切片）**：开始时 `git status` 干净；复验结束前另一会话
在工作树写入了仅涉及测试/文档的改动（`crates/wxml-backend-tsgo/tests/type_assets.rs`、
`crates/wxml-lsp/tests/q2_stability.rs`、`docs/evidence/README.md`、
`docs/reference/tsgo-delivery-contract.md` 及若干新证据文件）。这些文件不参与
`cargo build --release -p wxml-lsp -p wxml-delivery` 的 lib/bin 编译；且冷 target
构建 C 与构建 A/B 的 archive 逐字节相同，可确认它们未进入 release payload。
本切片未触碰这些改动。

`/tmp` 为 16G tmpfs，开始时可用约 1.4G，结束时约 1.1G；空间足够，未触发不足。
本次只新建 `p7-post-delete-*` 目录，未删除他人目录。

## 2. Release 构建与可复现性

### 2.1 命令

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration

# A：新 cache（冷 target），资产从锁定 tarball provision
env -u WXML_LSP_BIN -u WXML_ASSET_CACHE -u CARGO_TARGET_DIR -u ZED_CARGO_TARGET_DIR \
  SOURCE_DATE_EPOCH=1763683200 \
  WXML_BUILD_CACHE=/home/zxg/.cache/wxml-lsp-p7-post-delete \
  scripts/build-release.sh \
    --output /tmp/opencode/p7-post-delete-release-a \
    --from-dir /tmp/opencode/assets-lock-root/from-dir

# B：同一 cache/target，只走已验证 cache
env -u WXML_LSP_BIN -u WXML_ASSET_CACHE -u CARGO_TARGET_DIR -u ZED_CARGO_TARGET_DIR \
  SOURCE_DATE_EPOCH=1763683200 \
  WXML_BUILD_CACHE=/home/zxg/.cache/wxml-lsp-p7-post-delete \
  scripts/build-release.sh \
    --output /tmp/opencode/p7-post-delete-release-b

# C（额外）：完全新的 cache + 冷 target，重新编译
env -u WXML_LSP_BIN -u WXML_ASSET_CACHE -u CARGO_TARGET_DIR -u ZED_CARGO_TARGET_DIR \
  SOURCE_DATE_EPOCH=1763683200 \
  WXML_BUILD_CACHE=/home/zxg/.cache/wxml-lsp-p7-post-delete-c \
  scripts/build-release.sh \
    --output /tmp/opencode/p7-post-delete-release-c \
    --from-dir /tmp/opencode/assets-lock-root/from-dir
```

| 构建 | exit | 耗时 | 关键日志 | 说明 |
| --- | --- | --- | --- | --- |
| A | 0 | 92s | `/tmp/opencode/p7-post-delete-logs/build-a.log` | 冷 target；资产 `source from-dir` |
| B | 0 | 5s | `/tmp/opencode/p7-post-delete-logs/build-b.log` | 复用 A 的 cargo target；资产 `source cache` |
| C | 0 | 148s | `/tmp/opencode/p7-post-delete-logs/build-c-fresh-target.log` | 全新 cache/target；`wxml-lsp` 等重新编译 2m07s |

三次构建均以 `node: not executed (PATH tripwire clean)` 结束。构建机 PATH 本身
有 Node，脚本内置的 node/nodejs/npm/npx/pnpm/yarn/bun shim 未被触发。

### 2.2 产物身份（A 与 B、C 相同）

| 项 | 值 |
| --- | --- |
| release 目录 | `/tmp/opencode/p7-post-delete-release-a/wxml-lsp-0.1.0-linux-x86_64` |
| archive | `/tmp/opencode/p7-post-delete-release-a/wxml-lsp-0.1.0-linux-x86_64.tar.gz` |
| archive SHA-256 | `7a51a3e92b21b829c60f5a4dd5c911b3605c76a5cc09c57dfb7edf491dcd81b7` |
| archive size | 15,117,270 bytes |
| release 目录文件数 / 目录数 | 159 / 11 |
| archive 文件条目 | 159（与目录文件清单逐项一致） |
| manifest payload 文件数 | 157（不含 `manifest.json`、`SHA256SUMS`） |
| release 内 `SHA256SUMS` | 158 行 |
| 顶层 `SHA256SUMS` | 1 行（archive） |
| 目录内容 SHA-256 | `da145d6deffcaf830dd2e4b6db32b4a19d5cdbb16a0e493dfa7018e42d3d0bb4` |
| `manifest.json` SHA-256 | `2a6cddc2969df9177b13718d16a21809c0ce6d050f28a35f199ac91cd5555677` |
| `provenance.json` SHA-256 | `b3001987c54845fd42f1c19484f78bdc4ccf9442f73bb25b78612764826addf7` |
| `THIRD_PARTY_LICENSES.md` SHA-256 | `cad23bf2b8e5944a21db316673fbd7c90ddbb6a03be233b0d80c23c13c9bc3b7` |
| `index.json` SHA-256 | `9ee5620d12f5d6bda8ab87099d9a1d14b2b61756fc32cf63b4c34ecd070e850b` |
| `bin/wxml-lsp` SHA-256 | `aa8aa03b40d3445a96405079cc6ab72c8a0a342ed5292306374d9e7540fe77e2` |
| `bin/wxml-lsp-install` SHA-256 | `cc8070fad9a863fb6576a453d2db00af04657ca3a414c53f156bb62b574b45f5` |
| `zed-extension/extension.wasm` SHA-256 | `488531eaec969281fa295803e2d3cd857493bd9e26f56adc1c21a71bad0aeb1f` |
| `engine/lib/tsc` SHA-256 / size | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` / 24,522,912 B |

### 2.3 逐字节比较

```text
CMP_SAME wxml-lsp-0.1.0-linux-x86_64.tar.gz
CMP_SAME wxml-lsp-0.1.0-linux-x86_64.tar.gz.sha256
CMP_SAME index.json
CMP_SAME SHA256SUMS
DIFF_TREE_SAME
A_C_ARCHIVE_SAME
A_C_INDEX_SAME
A_C_TREE_SAME
ARCHIVE_FILE_LIST_MATCHES_DIR
```

日志：`/tmp/opencode/p7-post-delete-logs/repro-compare.log`、
`/tmp/opencode/p7-post-delete-logs/repro-compare-c.log`。

说明：

- A/B 的 `index.json` 中 `sha256`/`size` 与 archive 实测一致（`size: 15117270`）。
- `manifest.json` 的 `builtAt` 固定为 `2025-11-21T00:00:00Z`；若使用当前时间，
  manifest 与 archive 会变化。与 2026-11-20 早先未固定 epoch 的 P7 构建相比，
  本次 release 目录只有 `manifest.json`（仅 `builtAt`）和 `SHA256SUMS` 不同，
  其余文件逐字节相同，说明差异来源是时间戳而非代码/资产。
- C 使用全新 target 重新编译，仍与 A/B 相同，排除了“仅打包确定性”的弱解释。

## 3. 安装与 native 校验

```bash
REL=/tmp/opencode/p7-post-delete-release-a
INS=/tmp/opencode/p7-post-delete-install
rm -rf "$INS"
env -u WXML_LSP_BIN "$REL/wxml-lsp-0.1.0-linux-x86_64/bin/wxml-lsp-install" \
  install --root "$INS" --from-dir "$REL" --version 0.1.0 --offline
env -u WXML_LSP_BIN "$INS/current/bin/wxml-lsp-install" verify --root "$INS"
env -u WXML_LSP_BIN "$INS/current/bin/wxml-lsp-install" current --root "$INS"
env -u WXML_LSP_BIN "$INS/current/bin/wxml-lsp-install" env --root "$INS"
```

| 命令 | exit | 关键输出 |
| --- | --- | --- |
| `install` | 0 | `installed 0.1.0 (native)`；`engine: typescript 7.1.0-beta` |
| `verify` | 0 | `verified 0.1.0 (native)`；`files: 157  bytes: 41899646` |
| `current` | 0 | `version: 0.1.0`、`platform: linux-x86_64`、`manifest protocol: 2` |
| `env` | 0 | `export WXML_LSP_BIN='/tmp/opencode/p7-post-delete-install/current/bin/wxml-lsp'` |

安装结构：`current -> versions/0.1.0`；`state.json` 的 `active` 为 `0.1.0`，
最近事务 `install/activated`。安装树 164 个文件（含 `cache/`、`tmp/`、
`txn/`、`state.json`、`.lock`），payload 目录 159 个文件，与 release 目录
`diff -r` 完全一致（`RELEASE_INSTALL_TREE_IDENTICAL`）。

```text
wxml-lsp 0.1.0
Version 7.1.0-beta
```

日志：`/tmp/opencode/p7-post-delete-logs/install.log`、
`/tmp/opencode/p7-post-delete-logs/install-verify.log`、
`/tmp/opencode/p7-post-delete-logs/artifact-identity.log`。

## 4. 无 Node 证据

### 4.1 安装器作用域（`scripts/verify-no-node.sh`，只读调用）

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
env -u WXML_LSP_BIN \
  WXML_LSP_INSTALL=/tmp/opencode/p7-post-delete-install/current/bin/wxml-lsp-install \
  WXML_INSTALL_ROOT=/tmp/opencode/p7-post-delete-install \
  TSGO_ASSET=/tmp/opencode/assets-lock-root/from-dir/typescript-linux-x64-7.1.0-beta.tgz \
  WXML_REQUIRE_REAL_ENGINE=1 \
  WXML_REQUIRE_LSP_NATIVE=1 \
  WXML_TEST_TMPDIR=/tmp/opencode \
  scripts/verify-no-node.sh
```

`EXIT=0`。日志 `/tmp/opencode/p7-post-delete-logs/verify-no-node.log`。

```text
NO_NODE: positive control exercised (shim exit 99, marker created)
verify: verified 0.1.0 (native)
current: binary: /tmp/opencode/p7-post-delete-install/current/bin/wxml-lsp
env: export WXML_LSP_BIN='/tmp/opencode/p7-post-delete-install/current/bin/wxml-lsp'
NO_NODE: PASS (installer scope: verify/current/env)
LSP_NATIVE: PASS (…/crates/wxml-lsp/src/main.rs has no NodeBackend or serve_node(); --stdio is native-only)
REAL_ENGINE: PASS (sha256=a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70, Version 7.1.0-beta)
```

脚本尾部保留 `F3-02: BLOCKED (installer scope only; no closure claim)`，表示
PATH shim 不能替代 execve 审计；这是既有 residual 标注，不是本次执行失败。

### 4.2 安装后 `--stdio` LSP 冒烟

临时 harness：`/tmp/opencode/p7-post-delete-lsp-smoke.py`（仓库外，未写入
仓库）。fixture 从 ` tests/fixtures/cross-file-type` 只读复制到
`/tmp/opencode/p7-post-delete-lsp-project`。子进程 PATH 只含 Node tripwire
shim；隔离 `HOME`/`TMPDIR`；不设置 `WXML_LSP_BIN`、`WXML_TYPE_BACKEND`、
`WXML_NODE`、`WXML_BACKEND_DIR`。

```bash
python3 /tmp/opencode/p7-post-delete-lsp-smoke.py \
  /tmp/opencode/p7-post-delete-lsp-project \
  pages/index/index.wxml \
  pages/index/index.ts
```

`EXIT=0`，`RESULT: PASS`。日志
`/tmp/opencode/p7-post-delete-logs/lsp-smoke.log`。

| 步骤 | 结果 |
| --- | --- |
| `initialize` | OK；`positionEncoding=utf-16`；`serverInfo={"name":"wxml-lsp","version":"0.1.0"}` |
| didOpen WXML + TS | 均收到 `publishDiagnostics` |
| WXML 诊断 | `code=2339`，`Property 'nmae' does not exist on type 'User'.` |
| TS 诊断 | 空（fixture 类型正确） |
| WXML hover `{{ user.name }}` | `User` + `当前登录用户，类型来自 types.ts` |
| WXML documentSymbol | `view` + 4×`text` |
| TS documentSymbol | `user`（kind 13） |
| `shutdown` | OK；进程 exit 0 |

进程采样（整个会话唯一后代）：

```text
/tmp/opencode/p7-post-delete-install/versions/0.1.0/engine/lib/tsc --api --async \
  --cwd /tmp/opencode/wxml-lsp-2406167-… --useCaseSensitiveFileNames=true \
  --callbacks=readFile,fileExists,directoryExists,getAccessibleEntries,realpath,stat,…
```

```text
forbidden_process_hits: []
node_marker_present: False
node_attempt_log_present: False
```

无 `node`、`nodejs`、`npm`、`server.js`、`backend_node`、`backends/typescript`
子进程；stderr 只有 native partial 能力说明。进程发现方式为 50ms 间隔的
`/proc/<pid>/task/<tid>/children` 轮询；短于采样窗口的瞬时子进程不会被记录，
与 §7.1 的“无 execve 审计”限制一致。

### 4.3 Zed 进程树

`/tmp/opencode/p7-post-delete-zed-smoke/logs/zed-tree-final.txt`：

```text
zed
└─ zed-editor
   ├─ zed-editor --crash-handler …
   └─ wxml-lsp /tmp/opencode/p7-post-delete-install/versions/0.1.0/bin/wxml-lsp --stdio
      └─ tsc /tmp/opencode/p7-post-delete-install/versions/0.1.0/engine/lib/tsc --api --async …
```

无 `--backend`、`node`、`server.js`、vtsls 或 eslint。

## 5. Release 内容与业务资产检查

### 5.1 禁用内容

对 release 目录、archive 清单和安装目录分别扫描
`backend_node`、`backends/typescript`、`server.js`、`node_modules`：

```text
release 目录命中: 无
archive 清单命中: 无
安装目录命中:     无
非 .d.ts 的 *.js/*.mjs/*.cjs: 0
```

日志：`/tmp/opencode/p7-post-delete-logs/release-content-scan.log`。

### 5.2 文件构成（release，159 files）

| 类别 | 数量 |
| --- | --- |
| `engine/**` | 122（含 `lib/tsc`、`lib/tsc.sig`、116 个 `lib*.d.ts`、LICENSE/NOTICE/README/package.json） |
| `typings/miniprogram-api-typings/**` | 23（其中 20 个 `.d.ts`） |
| `bin/**` | 2 |
| `zed-extension/**` | 8（wasm、toml、5 个 `.scm`、`config.toml`） |
| 文档/元数据 | `manifest.json`、`provenance.json`、`THIRD_PARTY_LICENSES.md`、`SHA256SUMS` |

`.d.ts` 合计 136（116 + 20），脚本 `.js` 为 0。archive 文件清单与 release
目录文件清单逐项一致（159/159），没有隐藏的额外文件。

### 5.3 仓库侧业务输入保留

删除的是 Node 运行时/旧 TS 实现树，不是 TypeScript 语言支持：

| 项 | 计数 |
| --- | --- |
| `tests/fixtures/**` | 136 个文件 / 16 个用例目录 |
| `*.ts`（含 `.d.ts`） | 83（其中 `.d.ts` 20） |
| `*.js` | 11 |
| `*.wxml` | 41 |
| 活跃 `server.js` | 0 |
| `crates/wxml-lsp/src/backend_node` | 不存在 |

日志：`/tmp/opencode/p7-post-delete-logs/repo-content-scan.log`。

**观察（非失败）**：工作树存在一个空的未跟踪目录
` backends/typescript/`（0 文件，git 不跟踪空目录，未出现在 archive 或
安装包）。它不影响 release 内容；本切片未删除它，留给主线程判断是否清理。

## 6. Zed 冒烟

环境具备真实 Zed 1.29.2、Xvfb、xdotool、ImageMagick、tesseract。直接调用仓库
现有脚本（未修改）：

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
env -u WXML_LSP_BIN scripts/f3-zed-smoke.sh \
  --package /tmp/opencode/p7-post-delete-install/current \
  --grammar /home/zxg/code/rust/wxml-lsp/editors/zed/grammars/wxml.wasm \
  --output /tmp/opencode/p7-post-delete-zed-smoke
```

`EXIT=0`，16/16 `F3-14-ZED-CHECK: PASS`。日志
`/tmp/opencode/p7-post-delete-logs/zed-smoke.log`。

覆盖项：默认 `--stdio`（无 `--backend`）；进程树无 Node/`server.js`；WXML
hover 与语法诊断；WXML/JavaScript/TypeScript/TSX/JSON 五种语言识别；未保存
编辑反映到 hover 且磁盘文件不变；五种语言后仅剩一个 `wxml-lsp`；Zed 日志无
vtsls/eslint/Node language server。

截图目录：`/tmp/opencode/p7-post-delete-zed-smoke/shots/`（9 张，不是 10 张）。关键截图
SHA-256 与早先 P7 记录一致：

| 截图 | SHA-256 |
| --- | --- |
| `20-wxml-hover.png` | `97b677108c5248dbdd4790fbbf9e046d04d3054c8c27505ec188cd1bbb0c8d3f` |
| `21-wxml-diagnostics.png` | `b2c56aea282d13bee051043a578f36de6fe6b22b97348f2a0d4351f6ea545c47` |
| `40-unsaved-edit.png` | `5508feb79e6cd199c6a5a74dadd30bf76170e69b79291fff5beec9ffbdfa2d24` |

`--grammar` 为外部 pinned 预构建 `wxml.wasm`；release 包按设计不携带 grammar
（构建日志有 warning），本次不伪造包内 grammar。

## 7. 限制与未声称

1. **无 execve/strace 审计**：PATH shim + `/proc` 采样只能证明本次 PATH 调用
   与可见子进程；绝对路径 Node 调用未被内核级跟踪。F3-02 仍为 BLOCKED。
2. **宿主存在无关 Node 进程**：OpenCode/CodeGraph/MCP 自身使用 Node，因此无
   “全机零 Node 进程”断言；结论限定在构建、安装器命令、LSP 会话和 Zed 进程树。
3. **并发工作树改动**：仅测试/文档文件，不参与 release lib/bin 编译；冷 target
   构建 C 与 A/B 逐字节相同。未把并发改动纳入 release 结论。
4. **平台**：仅 Linux x86_64；其它平台不声明。
5. **Zed**：运行在 Xvfb 虚拟显示；真实 Zed 进程/UI 自动化，不是物理显示器。
6. **未运行**：workspace 全量测试、native gates、升级/回滚、schema v1 兼容
   路径；本记录不声称这些项通过。
7. 空的未跟踪 ` backends/typescript/` 目录保留未动，见 §5.3。

## 8. 产物与日志索引

```text
/tmp/opencode/p7-post-delete-release-a/          # 构建 A
/tmp/opencode/p7-post-delete-release-b/          # 构建 B
/tmp/opencode/p7-post-delete-release-c/          # 构建 C（冷 target）
/tmp/opencode/p7-post-delete-install/            # 安装根
/tmp/opencode/p7-post-delete-zed-smoke/          # Zed 冒烟产物与截图
/tmp/opencode/p7-post-delete-lsp-project/        # 只读复制的 fixture
/tmp/opencode/p7-post-delete-lsp-smoke.py        # LSP 冒烟 harness（仓库外）

/tmp/opencode/p7-post-delete-logs/build-a-dryrun.log
/tmp/opencode/p7-post-delete-logs/build-a.log
/tmp/opencode/p7-post-delete-logs/build-b.log
/tmp/opencode/p7-post-delete-logs/build-c-fresh-target.log
/tmp/opencode/p7-post-delete-logs/repro-compare.log
/tmp/opencode/p7-post-delete-logs/repro-compare-c.log
/tmp/opencode/p7-post-delete-logs/artifact-identity.log
/tmp/opencode/p7-post-delete-logs/install.log
/tmp/opencode/p7-post-delete-logs/install-verify.log
/tmp/opencode/p7-post-delete-logs/verify-no-node.log
/tmp/opencode/p7-post-delete-logs/lsp-smoke.log
/tmp/opencode/p7-post-delete-logs/release-content-scan.log
/tmp/opencode/p7-post-delete-logs/repo-content-scan.log
/tmp/opencode/p7-post-delete-logs/zed-smoke.log
/tmp/opencode/p7-post-delete-logs/archive-files.txt
/tmp/opencode/p7-post-delete-logs/dir-files.txt
/tmp/opencode/p7-post-delete-zed-smoke/logs/zed-tree-final.txt
```

构建 cache（仓库外，可丢弃）：`/home/zxg/.cache/wxml-lsp-p7-post-delete`、
`/home/zxg/.cache/wxml-lsp-p7-post-delete-c`。
