# F3-14 默认路径打包 Zed native 冒烟（2026-11-20）

## 0. 结论先行

使用 HEAD `5020ae7` 的生产源码重新执行无 Node 的 `scripts/build-release.sh`，
得到真实 schema v2 native 包，并按 **默认路径** 在隔离 Xvfb + 真实 Zed 1.29.2
上执行 16 项检查：**16/16 PASS，exit 0**。

- Zed 直接启动包内 `bin/wxml-lsp --stdio`，**没有 wrapper，没有 `--backend`**；
- `ps` 精确显示 `.../bin/wxml-lsp --stdio`，进程树无 Node / `server.js` / `--backend node`；
- WXML hover 为 `string`，带脚本注释 `用户显示名`；
- WXML 语法诊断显示 `element is not closed before its parent closes (wxml-lsp)`；
- 未保存 `index.ts` 中 `name: 'Ada'` → `42` 后，WXML hover 变为 `number`，磁盘哈希不变；
- WXML / JavaScript / TypeScript / TSX / JSON 五语言状态栏均可见；
- 五语言阶段结束后恰好只有 **1 个** `wxml-lsp` 进程。

**F3-14 从 BLOCKED 提升为 closed。** 关闭依据是默认无 wrapper 的真实包路径，
不是之前的 `--backend native` bridge。剩余限制见 §6；Xvfb/TSX partial/definition/
completion/exec/网络审计不被当作 F3-14 的关闭条件，但明确记录。

## 1. 身份、边界与构建

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`。
- 构建目标源码：`5020ae7939f1a87212372fa8901349477d799dda`（P6-8 默认切换）。
- 为消除构建期间 HEAD 漂移，使用 `git archive 5020ae7 | tar -x -C
  /tmp/opencode/f3-14-exact/src` 从纯净源码树构建，而不是依赖当时工作树 HEAD。
- 另一次从工作树（HEAD 已推进到 `0bf666b`，仅测试/台账/证据提交）构建得到
  **相同** archive SHA-256 `4bb9a1e...`；`git diff 5020ae7..0bf666b -- crates/wxml-lsp/src
  crates/wxml-delivery/src Cargo.toml Cargo.lock editors/zed/src editors/zed/Cargo.toml
  assets.lock.json` 为空。构建后并行会话只在
  `crates/wxml-lsp/src/backend_tsgo/component_bindings.rs` 的 `#[cfg(test)]` 模块追加
  测试，不改变生产代码。
- 本切片只更新 `scripts/f3-zed-smoke.sh`、本文件、F3-14 合同状态与证据索引；
  未修改生产代码、默认 backend、Node 删除、`wxml-delivery`、Cargo 或 Zed 扩展。
- 未 commit，未启动 review；所有运行产物在 `/tmp/opencode/f3-14-exact`。

构建命令（`cd` 到解包后的 `5020ae7` 源码树；clean PATH 没有
node/npm/npx/pnpm/yarn/bun）：

```bash
cd /tmp/opencode/f3-14-exact/src
env PATH="/tmp/opencode/p6-7-release/clean-bin:$HOME/.cargo/bin" \
  SOURCE_DATE_EPOCH=1763683200 \
  WXML_BUILD_CACHE=/tmp/opencode/f3-14-exact/cache \
  WXML_ASSET_CACHE=/tmp/opencode/p6-7-release/cache/assets-cache \
  CARGO_TARGET_DIR=/tmp/opencode/f3-14-exact/target \
  ZED_CARGO_TARGET_DIR=/tmp/opencode/f3-14-exact/zed-target \
  ./scripts/build-release.sh \
    --output /tmp/opencode/f3-14-exact/release \
    --from-dir /tmp/opencode/p6-7-release/from-dir
```

构建结果：`BUILD_EXIT=0`；日志首行 `node: absent`，末行
`node: not executed (PATH tripwire clean)`。包由 `wxml-lsp-install verify-package`
验证，再安装到隔离 root：

```text
installed 0.1.0 (native)
verified 0.1.0 (native)
files: 157  bytes: 42532286
```

安装时必须清除宿主已有的 `WXML_LSP_BIN`，否则安装器按设计优先使用外部二进制；
本次使用 `env -u WXML_LSP_BIN -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE`
后安装。

## 2. 包身份

| 项 | 值 |
| --- | --- |
| release 目录 | `/tmp/opencode/f3-14-exact/release/wxml-lsp-0.1.0-linux-x86_64` |
| 安装 root | `/tmp/opencode/f3-14-exact/install-root`，`current -> versions/0.1.0` |
| archive SHA-256 | `4bb9a1e2678348e3328dbfd394065f746c8e133e3a5a762e4223632bcf8532a5` |
| manifest SHA-256 | `22f846f0ba23c0ad0f1a1858e83da0e16b64d919b024f2be978feea78f1610ac` |
| `bin/wxml-lsp` SHA-256 | `70a02f22eb92fc6b94c7cf67b85af79a19fded9b43b659d782fc5fd81354e1ba` |
| `engine/lib/tsc` SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| `zed-extension/extension.wasm` SHA-256 | `488531eaec969281fa295803e2d3cd857493bd9e26f56adc1c21a71bad0aeb1f` |
| grammar wasm SHA-256 | `05bd00d173f465dc7597e961c0f480ae3b743a3a11cdf4b838a1d88906aab9ad` |
| 引擎 | TypeScript `7.1.0-beta`，gitHead `3e9236361fa97a9e6c685f4833ac457ae2d8c50a` |
| `--help` 默认后端 | `--backend <backend> type backend for --stdio: native (default) or node` |
| 文件数 | 157 |

`--help` 已不再写 `node (default)`，不存在文档漂移。包内不携带 tree-sitter
grammar；本次继续使用 pinned revision `1cc1761c8c11cacab5516dbe64e20ee689aa4428`
的预构建 wasm，不伪造引擎/类型资产。

## 3. 默认路径运行

`scripts/f3-zed-smoke.sh` 默认使用 `--direct`：

```bash
./scripts/f3-zed-smoke.sh \
  --package /tmp/opencode/f3-14-exact/install-root/current \
  --grammar /home/zxg/code/rust/wxml-lsp/editors/zed/grammars/wxml.wasm \
  --output /tmp/opencode/f3-14-exact/smoke
```

`WXML_LSP_BIN` 直接指向包内 `bin/wxml-lsp`；没有测试 wrapper。`--bridge` 仍保留，
用于复现旧的追加 `--backend native` 路径；本次也以该模式复跑 16/16，但 bridge 结果
不作为 F3-14 关闭依据。

Zed 日志：

```text
INFO [lsp] starting language server process. binary path:
  "/tmp/opencode/f3-14-exact/install-root/versions/0.1.0/bin/wxml-lsp",
  working directory: "/tmp/opencode/f3-14-exact/smoke/project",
  args: ["--stdio"]
```

即 Zed 只传 `--stdio`，包内二进制自行选择 native。

### 16/16 检查

```text
F3-14-ZED-NOTE: package --help documents native as default
PASS - Zed started the packaged default server (--stdio, no --backend)
PASS - ps shows wxml-lsp --stdio exactly (no --backend argument)
PASS - no Node process or server.js in Zed's process tree
PASS - WXML hover shows the inferred string type
PASS - WXML diagnostics show the parser error
PASS - Zed shows WXML for index.wxml
PASS - Zed shows JavaScript for util.js
PASS - Zed shows TypeScript for index.ts
PASS - Zed shows TSX for component.tsx
PASS - Zed shows JSON for config.json
PASS - WXML hover reflects the unsaved script edit (number)
PASS - unsaved buffer stayed in memory; index.ts hash is unchanged
PASS - exactly one wxml-lsp process remains after all five languages
PASS - Zed log records the language server start
PASS - Zed passed only --stdio; the package selected native
PASS - no vtsls/eslint/Node language server in the Zed log

F3-14-ZED-SMOKE: PASS (direct default path; real Zed UI on Xvfb)
F3-14: PASS (default packaged --stdio; evidence complete)
```

## 4. 证据

### 4.1 进程

`/tmp/opencode/f3-14-exact/smoke/logs/zed-tree-final.txt`：

```text
1519526 1519457 zed             zed --foreground --user-data-dir /tmp/opencode/f3-14-exact/smoke/data -n /tmp/opencode/f3-14-exact/smoke/project
1519537 1519526 zed-editor      /home/zxg/.local/zed.app/libexec/zed-editor zed-cli:///tmp/.tmpqnVm5Z/socket --user-data-dir /tmp/opencode/f3-14-exact/smoke/data
1519603 1519537 zed-editor      /home/zxg/.local/zed.app/libexec/zed-editor --crash-handler ...
1519866 1519537 wxml-lsp        /tmp/opencode/f3-14-exact/install-root/versions/0.1.0/bin/wxml-lsp --stdio
1520711 1519866 tsc             /tmp/opencode/f3-14-exact/install-root/versions/0.1.0/engine/lib/tsc --api --async ...
```

没有 `--backend`、`--backend node`、`node` 或 `server.js`。`tsc` 是 native 引擎。
Zed 日志另有 `node_runtime ... using Node.js found on PATH`；本次没有 spawn Node
进程，判断以进程树为准。隔离配置把 JS/TS/TSX/JSON 的 `language_servers` 限制为
`wxml-lsp`，并禁用 vtsls/eslint 自动安装，否则 Zed 会为 TS 启动
`/usr/bin/node ... vtsls.js`，污染“无 Node”结论。

### 4.2 WXML hover 与诊断

截图：

| 截图 | SHA-256 | OCR 关键文本 |
| --- | --- | --- |
| ` /tmp/opencode/f3-14-exact/smoke/shots/20-wxml-hover.png` | `97b677108c5248dbdd4790fbbf9e046d04d3054c8c27505ec188cd1bbb0c8d3f` | `string`、`用户显示名` |
| ` /tmp/opencode/f3-14-exact/smoke/shots/21-wxml-diagnostics.png` | `b2c56aea282d13bee051043a578f36de6fe6b22b97348f2a0d4351f6ea545c47` | `element is not closed before its parent closes (wxml-lsp)` |
| ` /tmp/opencode/f3-14-exact/smoke/shots/41-unsaved-hover.png` | `4d3d51e7ff09ed6cc5f5c2bf3e8a83a014f05c49ef53f936c04f730a08a1623d` | `number`、`用户显示名` |
| ` /tmp/opencode/f3-14-exact/smoke/shots/40-unsaved-edit.png` | `5508feb79e6cd199c6a5a74dadd30bf76170e69b79291fff5beec9ffbdfa2d24` | `name: 42`、`index.ts` 未保存点 |

WXML hover 来自 `{{user.name}}`；诊断来自未闭合的 `<text>`。诊断面板同时显示
`component.tsx` 被类型分析省略的 partial 状态，这是现有 native 能力边界，不是
本次默认路径失败。

### 4.3 未保存脚本

1. 打开 `pages/index/index.ts`，把 `name: 'Ada'` 改为 `name: 42`，不保存；
2. 切回 `pages/index/index.wxml`，hover `{{user.name}}`；
3. hover 从 `string` 变为 `number`；
4. `index.ts` 的 SHA-256 前后一致：

```text
before=bb0f4353021f182ccdb54291dd1c1df078c1d28decfa361f00932965b6fd5cfe
after =bb0f4353021f182ccdb54291dd1c1df078c1d28decfa361f00932965b6fd5cfe
disk_line=5:      name: 'Ada'
```

### 4.4 五语言

包内 `zed-extension/extension.toml`：

```toml
languages = ["WXML", "JavaScript", "TypeScript", "TSX", "JSON"]
```

| 文件 | 截图 | 状态栏 OCR |
| --- | --- | --- |
| `index.wxml` | ` /tmp/opencode/f3-14-exact/smoke/shots/30-language-WXML.png` | `WXML` |
| `util.js` | ` /tmp/opencode/f3-14-exact/smoke/shots/30-language-JavaScript.png` | `JavaScript` |
| `index.ts` | ` /tmp/opencode/f3-14-exact/smoke/shots/30-language-TypeScript.png` | `TypeScript` |
| `component.tsx` | ` /tmp/opencode/f3-14-exact/smoke/shots/30-language-TSX.png` | `TSX` |
| `config.json` | ` /tmp/opencode/f3-14-exact/smoke/shots/30-language-JSON.png` | `JSON` |

五语言阶段结束后恰好 1 个 `wxml-lsp --stdio`，没有为 TS/TSX/JSON 启动 vtsls、
eslint 或 Node。

## 5. 关闭判断

F3-14 的关闭条件已满足：

- 默认路径：Zed 直接启动包内 `bin/wxml-lsp --stdio`，无 wrapper、无 `--backend`；
- 真实包：HEAD `5020ae7` 生产源码构建，157 文件，安装/verify 通过；
- 无 Node：进程树无 Node/server.js；构建在无 Node PATH 下完成；
- UI：WXML hover/诊断、五语言注册、未保存脚本 hover 均有截图/OCR/日志；
- 进程：结束时恰好 1 个 `wxml-lsp`。

因此合同 §9.5 的 F3-14 从 **blocked** 改为 **closed**，头部计数从
13 closed / 2 partial / 1 blocked 改为 **14 closed / 2 partial / 0 blocked**。
`docs/evidence/README.md` 的 F3-14 行与 §3.3 同步更新。

## 6. 剩余限制（不阻塞 F3-14）

1. **Xvfb**：真实 Zed 进程/UI/截图，但显示是 Xvfb + 软件渲染，不是物理显示器；
   自动化依赖 xdotool/ImageMagick/tesseract。
2. **TSX partial**：`component.tsx` 注册为 TSX，但 native project host 把它从
   WXML 类型分析中省略（诊断面板显示 partial）；不是注册失败。
3. **definition/completion**：本次未单独验证；F3-14 的“查询/未保存脚本”由
   hover、诊断和未保存脚本 hover 覆盖。
4. **exec/网络审计**：只有进程树，没有 execve 追踪或网络审计；属于 F3-02/其他项，
   不归 F3-14。
5. **grammar**：包不携带 tree-sitter artifact，本次用 pinned revision 的预构建
   wasm；Zed 标准安装流程通常自行构建。
6. **Node runtime 探测**：Zed 日志有 Node PATH 探测行，但没有 Node 子进程；
   vtsls/eslint 在测试配置中显式禁用。

## 7. 批量检查与产物

```text
bash -n scripts/f3-zed-smoke.sh                          PASS
scripts/f3-zed-smoke.sh（直接默认路径）                  16/16 PASS，exit 0
git diff --check                                         PASS
```

产物：

```text
/tmp/opencode/f3-14-exact/build.log
/tmp/opencode/f3-14-exact/install.log
/tmp/opencode/f3-14-exact/release/
/tmp/opencode/f3-14-exact/install-root/
/tmp/opencode/f3-14-exact/smoke/logs/package-identity.txt
/tmp/opencode/f3-14-exact/smoke/logs/zed.log
/tmp/opencode/f3-14-exact/smoke/logs/zed-tree-final.txt
/tmp/opencode/f3-14-exact/smoke/logs/unsaved-disk.txt
/tmp/opencode/f3-14-exact/smoke/shots/*.png
/tmp/opencode/f3-14-exact/smoke-run.log
```
