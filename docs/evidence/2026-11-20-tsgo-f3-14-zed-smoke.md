# F3-14 真实打包 Zed native 冒烟（opt-in，2026-11-20）

## 0. 结论先行

本次新增 `scripts/f3-zed-smoke.sh`，用**已构建的 v2 native 安装包** + 测试专用
`WXML_LSP_BIN` wrapper（只追加 `--backend native`）启动真实 Zed 1.29.2，在隔离
Xvfb 显示上完成：

- `ps` 显示包内 `wxml-lsp --stdio --backend native`，Zed 进程树内无 Node；
- WXML hover 显示 `string`；
- WXML 语法诊断显示 `element is not closed before its parent closes (wxml-lsp)`；
- 未保存脚本修改后，WXML hover 从 `string` 变为 `number`，而磁盘文件哈希不变；
- WXML / JavaScript / TypeScript / TSX / JSON 五语言状态栏均可见，且五语言阶段
  始终只有同一个 `wxml-lsp` 进程。

脚本结果：**16 项检查全部 PASS，exit 0**。

但 **F3-14 仍是 BLOCKED**：本次是 opt-in wrapper 冒烟，不是默认路径冒烟。
本次使用的 P6-7 已构建包（`wxml-lsp 0.1.0`，SHA-256 `847f...`）的 `--help`
仍写 `node (default)`；它没有包含之后工作树上并行会话的 P6-8 改动。
不能用本证据把 F3-14 标为 closed。

## 1. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`，HEAD `1102da7`。
- 本次唯一新增：` scripts/f3-zed-smoke.sh` 与本文件。未修改生产代码、默认 backend、
  Node 删除、`wxml-delivery`、Cargo、现有测试或 Zed 扩展。
- 读取/运行期间，并行会话在工作树加入了未提交的 P6-8 默认选择器改动
  （`crates/wxml-lsp/src/main.rs`、`lib.rs`、`backend_tsgo/**`）；这些改动不是本切片
  产生的，也没有被构建或纳入本次包冒烟。本切片的结论只对应 P6-7 已构建包。
- 未 commit，未启动 review。
- 所有运行产物在 `/tmp/opencode/f3-zed-smoke`，仓库内没有临时文件。

## 2. 输入包与测试 wrapper

### 2.1 包身份

使用 P6-7 真实发布链构建并由 `wxml-lsp-install` 安装的 root：

```text
root=/tmp/opencode/p6-7-release/install-root
current -> versions/0.1.0
```

| 项 | 值 |
| --- | --- |
| 安装版本 | `0.1.0`，schema v2 |
| manifest SHA-256 | `f1f50e3629a8b354993275088a18d46768c8de8adbba4f81bb9fca98afb15939` |
| `bin/wxml-lsp` SHA-256 | `847f770a1c640a109d050c7aca244e903dec863a8b06c8def107861bca3adbc3` |
| `engine/lib/tsc` SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| 包内 `zed-extension/extension.wasm` SHA-256 | `488531eaec969281fa295803e2d3cd857493bd9e26f56adc1c21a71bad0aeb1f` |
| 引擎 | TypeScript `7.1.0-beta`，gitHead `3e92363...` |
| 原始 archive SHA-256 | `35a356589fb5aac4dad9de8d9edf6fb7ad82e9e0a82b1fb9f8c958a4bd42b488`（P6-7 out-a，四次一致） |
| grammar wasm SHA-256 | `05bd00d173f465dc7597e961c0f480ae3b743a3a11cdf4b838a1d88906aab9ad` |

包内 `--help` 的默认后端行（`logs/package-identity.txt`）：

```text
default_backend_help=  --backend <backend>  type backend for --stdio: node (default) or native
```

包来自 [2026-11-20-tsgo-p6-7-native-release-build.md](2026-11-20-tsgo-p6-7-native-release-build.md)
（HEAD `8ccebbf`），**不是**在 `1102da7` 上重新构建。本切片验证的是“已构建真实包”，
不声明二进制对应 `1102da7`。包内不包含 tree-sitter grammar 产物；脚本显式接收
pinned revision `1cc1761c8c11cacab5516dbe64e20ee689aa4428` 的预构建
`wxml.wasm`，不伪造引擎或类型资产。

### 2.2 wrapper

`/tmp/opencode/f3-zed-smoke/bin/wxml-lsp` 全文只有：

```sh
#!/bin/sh
# Test-only wrapper: the packaged binary plus the explicit opt-in backend.
exec /tmp/opencode/p6-7-release/install-root/versions/0.1.0/bin/wxml-lsp "$@" --backend native
```

wrapper 不创建、不替换 engine/typings/manifest，不设置 legacy Node 变量；只追加
`--backend native`。Zed 日志记录它实际传给 server 的参数仍是 `["--stdio"]`。

### 2.3 隔离配置

- `--user-data-dir`、隔离 `HOME`、`trust_all_worktrees=true`；
- 包内 `zed-extension` 作为 **installed** 扩展加载，五语言 manifest 原样保留；
- 为隔离 WXML server，`languages` 中 JavaScript/TypeScript/TSX/JSON 的
  `language_servers` 仅保留 `wxml-lsp`，并关闭 vtsls/eslint 自动安装；否则 Zed
  会为 TS 启动 `/usr/bin/node ... vtsls.js`，污染“无 Node”结论；
- `ZED_ALLOW_EMULATED_GPU=1`，Xvfb `1600x1000x24`，软件渲染；不触碰用户当前
  Wayland/KDE 桌面。

## 3. 运行命令与结果

```bash
scripts/f3-zed-smoke.sh \
  --package /tmp/opencode/p6-7-release/install-root/current \
  --grammar /home/zxg/code/rust/wxml-lsp/editors/zed/grammars/wxml.wasm \
  --output /tmp/opencode/f3-zed-smoke
```

逐项结果：

```text
F3-14-ZED-CHECK: PASS - Zed started the packaged native server
F3-14-ZED-CHECK: PASS - ps shows the wrapper-resolved wxml-lsp --stdio --backend native
F3-14-ZED-CHECK: PASS - no Node process or server.js in Zed's process tree
F3-14-ZED-CHECK: PASS - WXML hover shows the inferred string type
F3-14-ZED-CHECK: PASS - WXML diagnostics show the parser error
F3-14-ZED-CHECK: PASS - Zed shows WXML for index.wxml
F3-14-ZED-CHECK: PASS - Zed shows JavaScript for util.js
F3-14-ZED-CHECK: PASS - Zed shows TypeScript for index.ts
F3-14-ZED-CHECK: PASS - Zed shows TSX for component.tsx
F3-14-ZED-CHECK: PASS - Zed shows JSON for config.json
F3-14-ZED-CHECK: PASS - WXML hover reflects the unsaved script edit (number)
F3-14-ZED-CHECK: PASS - unsaved buffer stayed in memory; index.ts hash is unchanged
F3-14-ZED-CHECK: PASS - native server still running after all five languages (1 process(es))
F3-14-ZED-CHECK: PASS - Zed log records the language server start
F3-14-ZED-CHECK: PASS - Zed passed only --stdio; the wrapper appended --backend native
F3-14-ZED-CHECK: PASS - no vtsls/eslint/Node language server in the Zed log

F3-14-ZED-SMOKE: PASS (opt-in wrapper; real Zed UI on Xvfb)
F3-14: BLOCKED (opt-in only; default-path packaged Zed smoke is not recorded)
```

`bash -n scripts/f3-zed-smoke.sh` 通过；本切片没有新增 Rust 测试，未运行
`cargo fmt/clippy`（工作树另有并行未提交 Rust 改动，不构成本切片的检查快照）。

## 4. 证据

### 4.1 进程

`/tmp/opencode/f3-zed-smoke/logs/zed-tree-final.txt`：

```text
1446919 1446849 zed             zed --foreground --user-data-dir /tmp/opencode/f3-zed-smoke/data -n /tmp/opencode/f3-zed-smoke/project
1446930 1446919 zed-editor      /home/zxg/.local/zed.app/libexec/zed-editor zed-cli:///tmp/.tmpP4npkj/socket --user-data-dir /tmp/opencode/f3-zed-smoke/data
1446996 1446930 zed-editor      /home/zxg/.local/zed.app/libexec/zed-editor --crash-handler ...
1447249 1446930 wxml-lsp        /tmp/opencode/p6-7-release/install-root/versions/0.1.0/bin/wxml-lsp --stdio --backend native
1448076 1447249 tsc             /tmp/opencode/p6-7-release/install-root/versions/0.1.0/engine/lib/tsc --api --async ...
```

没有 `node`、`server.js` 或 `--backend node`。`tsc` 是 native 引擎进程，不是 Node。
Zed 日志另有一行 `node_runtime ... using Node.js found on PATH`，但本次没有 spawn
Node 进程；判断以进程树为准。

Zed 日志 `/tmp/opencode/f3-zed-smoke/data/logs/Zed.log`：

```text
INFO [lsp] starting language server process. binary path:
  "/tmp/opencode/f3-zed-smoke/bin/wxml-lsp",
  working directory: "/tmp/opencode/f3-zed-smoke/project",
  args: ["--stdio"]
```

即 Zed 只传 `--stdio`；wrapper 追加 `--backend native`。

### 4.2 WXML hover 与诊断

截图：

| 截图 | SHA-256 | OCR 关键文本 |
| --- | --- | --- |
| ` /tmp/opencode/f3-zed-smoke/shots/20-wxml-hover.png` | `97b67710...0c8d3f` | `string`、`用户显示名` |
| ` /tmp/opencode/f3-zed-smoke/shots/21-wxml-diagnostics.png` | `b2c56aea...ea545c47` | `element is not closed before its parent closes (wxml-lsp)`、`type checking is partial ... component.tsx` |
| ` /tmp/opencode/f3-zed-smoke/shots/41-unsaved-hover.png` | `4d3d51e7...a08a1623d` | `number`、`用户显示名` |
| ` /tmp/opencode/f3-zed-smoke/shots/40-unsaved-edit.png` | `5508feb7...bdfa2d24` | `name: 42`、`index.ts` 标签有未保存点 |

WXML hover 来自 `{{user.name}}`：native 表达式成员 hover 返回 `string`，
并带脚本注释 `用户显示名`。诊断来自未闭合的 `<text>`，由 WXML 解析器发布。
同一诊断面板还显示 `component.tsx` 被类型分析省略的 partial 状态；这是现有
native 能力边界，不是本次新增失败。

### 4.3 未保存脚本

1. 打开 `pages/index/index.ts`，把 `name: 'Ada'` 改成 `name: 42`，**不保存**；
2. 切回 `pages/index/index.wxml`，hover `{{user.name}}`；
3. hover 从 `string` 变为 `number`；
4. `index.ts` 的 SHA-256 前后一致：

```text
before=bb0f4353021f182ccdb54291dd1c1df078c1d28decfa361f00932965b6fd5cfe
after =bb0f4353021f182ccdb54291dd1c1df078c1d28decfa361f00932965b6fd5cfe
disk_line=5:      name: 'Ada'
```

证明：未保存 buffer 参与了 WXML 类型查询，但没有落盘。

### 4.4 五语言

包内 `zed-extension/extension.toml`：

```toml
languages = ["WXML", "JavaScript", "TypeScript", "TSX", "JSON"]
```

Zed 状态栏截图与 OCR：

| 文件 | 截图 | 状态栏 OCR |
| --- | --- | --- |
| `index.wxml` | ` /tmp/opencode/f3-zed-smoke/shots/30-language-WXML.png` | `WXML` |
| `util.js` | ` /tmp/opencode/f3-zed-smoke/shots/30-language-JavaScript.png` | `JavaScript` |
| `index.ts` | ` /tmp/opencode/f3-zed-smoke/shots/30-language-TypeScript.png` | `TypeScript` |
| `component.tsx` | ` /tmp/opencode/f3-zed-smoke/shots/30-language-TSX.png` | `TSX` |
| `config.json` | ` /tmp/opencode/f3-zed-smoke/shots/30-language-JSON.png` | `JSON` |

五语言阶段结束后，进程树中仍只有 **1 个** `wxml-lsp --stdio --backend native`；
没有为 TS/TSX/JSON 启动 vtsls、eslint 或 Node。

## 5. 未覆盖与 BLOCKED 原因

1. **默认路径未用本包验证**：P6-7 包内 `wxml-lsp --help` 仍写
   `--backend <backend> ... node (default) or native`，即该二进制默认仍是 Node。
   读取期间并行会话已在工作树加入未提交的 P6-8 选择器
   （` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:200`
   `serve_default()` → `run_stdio_native_default()`），但本切片没有构建它，也没有
   用它录制默认 `--stdio` 的 Zed UI 证据；本次只是 wrapper opt-in，不是 F3-14 的
   最终形态。
2. **显示是 Xvfb 虚拟显示**：真实 Zed 进程、真实 UI 查询和截图，但不是物理显示器；
   使用软件渲染。自动化依赖 `xdotool` + ImageMagick + tesseract。
3. **未重新构建包**：使用 P6-7 在 HEAD `8ccebbf` 构建的包，不是 `1102da7` 的逐字节重建。
4. **五语言覆盖范围**：验证了语言注册/状态栏和 WXML 查询；没有对 JS/TS/TSX/JSON
   做各自语义查询，也没有验证 definition/completion。`component.tsx` 被 native
   project host 标为 partial（其脚本未进入 WXML 类型分析）。
5. **无 exec/网络审计**：本次只有 `ps`/`pstree` 进程树，不是 `execve` 追踪；不关闭
   F3-02，也不证明网络隔离。
6. **grammar 不在包内**：使用 pinned revision 的预构建 wasm；Zed 标准安装流程通常
   自行构建 grammar。
7. **测试配置关闭了无关 Node server**：vtsls/eslint 被显式禁用，以确保“无 Node”只
   反映 `wxml-lsp` 路径；这不等同于生产配置。

因此：**opt-in native Zed UI 冒烟 PASS；F3-14 仍 BLOCKED，直到 P6-8 后的包被重新
构建，并以默认 `--stdio`（无 wrapper）重新录制同样的 UI 证据。**

## 6. 产物索引

```text
/tmp/opencode/f3-zed-smoke/logs/package-identity.txt
/tmp/opencode/f3-zed-smoke/logs/zed.log
/tmp/opencode/f3-zed-smoke/logs/zed-tree-final.txt
/tmp/opencode/f3-zed-smoke/logs/zed-pstree-final.txt
/tmp/opencode/f3-zed-smoke/logs/unsaved-disk.txt
/tmp/opencode/f3-zed-smoke/logs/ocr-evidence.txt  # 本次运行的 OCR 摘录
/tmp/opencode/f3-zed-smoke/shots/*.png
/tmp/opencode/f3-zed-smoke-run.log
```
