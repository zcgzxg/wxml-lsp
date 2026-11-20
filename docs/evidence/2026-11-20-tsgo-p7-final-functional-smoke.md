# P7 删除后最终安装功能级 LSP 冒烟（2026-11-20）

> **状态（crosscheck 后）**：P1 已由 [`180fa42`](2026-11-20-tsgo-p1-script-diagnostics-retry.md)
> 修复并真实复验；P2 保留为已知限制。整体为 **P1 fixed / P2 known / PASS-with-followup**，
> 不是全 PASS。原 P1/P2 描述保留为发现记录。

## 0. 结论先行

在 P7 删除后的最终安装
`/tmp/opencode/p7-post-delete-install/versions/0.1.0/bin/wxml-lsp` 上执行了
一次直接 `--stdio` 的功能级 LSP 冒烟（无 wrapper、无 `--backend`、无 Node），
覆盖 3 个真实 fixture 和 WXML/TS/JS/TSX/JSON 五种语言。

| 判定项 | 结果 |
| --- | --- |
| native 默认路径可用、无 Node | **PASS** |
| 功能级 LSP 覆盖 | **PASS-with-followup**（native 主路径可用；P1 已修复，P2 保留为已知限制） |
| 记录到的 LSP 操作 | 36 条：32 PASS、1 FAIL（P2）、3 INFO |
| 进程快照 | 4 条：全部 PASS，唯一后代可执行文件为 `engine/lib/tsc` |
| fixture 磁盘内容 | 4 个关键文件 SHA-256 前后一致，未写入项目目录 |
| 最高严重级别 | **P1（已修复）**；剩余已知限制为 P2 |

> **修复状态（2026-11-20）**：P1 已修复并有真实 LSP 回归，见
> [`2026-11-20-tsgo-p1-script-diagnostics-retry.md`](2026-11-20-tsgo-p1-script-diagnostics-retry.md)。
> 原 P1 描述保留为发现记录；P2 仍为已记录的已知限制。

两个发现（P1 已修复，P2 保留为已知限制；详见 §6）：

- **P1（高，已修复）**：当 JSON 文档在脚本首次诊断仍在进行时 `didOpen`，TS 脚本的
  首次 `publishDiagnostics` 被静默丢弃且不再重试；之后编辑脚本才恢复。
  受控 probe 可稳定复现，等待 20 秒仍不发布。
- **P2（中）**：只有 `Page(...)`/`Component(...)` 注册、没有顶层声明的脚本，
  `textDocument/documentSymbol` 返回 JSON-RPC 错误
  `-32803 the script outline is incomplete`，而不是空 outline；有顶层声明的
  脚本（B/C）正常返回符号。

TSX 的 native 限制是**显式 Unsupported**，不是伪造结果：WXML 发布
severity 2 的 “script `pages/index/App.tsx` was omitted because its language is
not supported”，TSX 自身的 documentSymbol 被拒绝、hover 为 `null`。

## 1. 身份、边界与环境

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b` |
| Tag `p7-removed` | `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b`（与 HEAD 相同） |
| 平台 | Linux x86_64 |
| 验证窗口（UTC） | 约 2026-11-20T01:50Z ～ 02:14Z |
| 安装 root | `/tmp/opencode/p7-post-delete-install` |
| `current` | `versions/0.1.0` |
| Binary | `/tmp/opencode/p7-post-delete-install/versions/0.1.0/bin/wxml-lsp` |
| Binary SHA-256 | `aa8aa03b40d3445a96405079cc6ab72c8a0a342ed5292306374d9e7540fe77e2` |
| Engine SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| Engine | TypeScript `7.1.0-beta`，`engine/lib/tsc` |
| 包构建时间 | `2025-11-21T00:00:00Z` |
| 本切片仓库改动 | 只新增本证据文档；未改代码、测试、README、ledger；未 commit（其他会话的并发改动见下） |

安装校验（在清空 `WXML_LSP_BIN` 后执行，只读）：

```text
$ env -u WXML_LSP_BIN -u WXML_TYPE_BACKEND -u WXML_NODE -u WXML_BACKEND_DIR \
    .../versions/0.1.0/bin/wxml-lsp-install verify \
    --root /tmp/opencode/p7-post-delete-install --version 0.1.0
verified 0.1.0 (native)
engine version: 7.1.0-beta
files: 157  bytes: 41899646
binary: /tmp/opencode/p7-post-delete-install/current/bin/wxml-lsp
```

`find -type f` 在安装目录看到 159 个文件（含 `bin/wxml-lsp-install` 和包元数据）；
`verify` 的 157 是 payload 计数。目录内没有 `node`、`node_modules`、
`server.js`、`backend_node`、`backends/typescript`，也没有非 `.d.ts` 的
`*.js/*.mjs/*.cjs`。

`--help` 的默认后端文案：

```text
--backend <backend>  type backend for --stdio: native (default)
```

同页明确写明 “There is no Node fallback and the `node` backend value is
rejected.”。

**边界与并发改动**：开始时工作树已有其他会话写入的文档/测试改动
（包括 ` tests/fixtures/README.md`），不属于本切片。本切片只读使用 fixture；
三个 fixture 的关键文件 mtime 仍是 2026-11-17，SHA-256 在冒烟前后完全一致
（见 §3.4）。所有 harness、日志和 probe 都在
`/tmp/opencode/p7-final-functional-smoke`，未在项目目录产生临时文件。

## 2. 方法

- Harness：仓库外 Python 脚本
  `/tmp/opencode/p7-final-functional-smoke/smoke.py`，直接读写 LSP
  `Content-Length` JSON-RPC，不依赖 LSP 客户端库。
- 启动参数：`[<install>/bin/wxml-lsp, "--stdio"]`；没有 wrapper，没有
  `--backend native`，没有 `--backend node`。
- 子进程环境：清空 `WXML_LSP_BIN`、`WXML_TYPE_BACKEND`、`WXML_NODE`、
  `WXML_BACKEND_DIR`；`PATH` 只包含 node/nodejs/npm/npx/pnpm/yarn/bun 的
  tripwire shim；隔离 `HOME` 和 `TMPDIR`；HTTP(S) 代理指向 `127.0.0.1:9`。
- 进程审计：每 25 ms 遍历 `/proc/<pid>/task/*/children` 的后代树，记录
  pid、cmdline、exe；对每个会话在关键阶段再做一次活动后代快照；匹配
  `node/nodejs/npm/npx/bun/server.js/backend_node/backends/typescript` 即失败。
- 磁盘审计：冒烟前后对 4 个关键 fixture 文件做 SHA-256；`git status` 检查
  fixture 目录没有新增/修改（除其他会话已存在的 README 改动）。
- 三个会话：

| 会话 | Fixture | 用途 |
| --- | --- | --- |
| A | ` tests/fixtures/component-bindings` | 组件属性绑定诊断、事件缺失、hover、definition、completion、WXML/脚本 documentSymbol、TS overlay、JSON/JS/TSX didOpen |
| B | ` tests/fixtures/cross-file-type` | 跨文件类型、表达式 hover/definition/completion、TS documentSymbol |
| C | ` tests/fixtures/js-jsdoc` | 真实 JS didOpen、JSDoc hover、JS documentSymbol、`// @ts-check` overlay 诊断、close/reopen |

受控 probe（`probe_diag.py`、`probe_diag2.py`、`probe_diag3.py`、
`probe_diag6.py`）单独复现 §6 F1，不计入上面的 36 条主流程操作。

## 3. 结果

### 3.1 Initialize（场景 1）

A 的 `initialize` 返回 `serverInfo={"name":"wxml-lsp","version":"0.1.0"}`，
`positionEncoding=utf-16`。能力摘要：

| 能力 | 值 |
| --- | --- |
| `textDocumentSync` | `openClose=true`，`change=2`（incremental），`save.includeText=false` |
| `hoverProvider` | `true` |
| `definitionProvider` | `true`（项目已打开） |
| `completionProvider` | `triggerCharacters=["<"," ",":","\"","'",".","{","="]`，`resolveProvider=false` |
| `documentSymbolProvider` | `true` |
| `documentFormattingProvider` / `documentRangeFormattingProvider` | `true` |
| `linkedEditingRangeProvider` / `foldingRangeProvider` / `selectionRangeProvider` | `true` |
| `documentLinkProvider` | `{resolveProvider:false}` |
| `referencesProvider` / `renameProvider` / `codeActionProvider` | `true` / `{prepareProvider:true}` / quickfix |
| `semanticTokensProvider` / `workspaceSymbolProvider` / `signatureHelpProvider` | 均存在 |
| `inlayHintProvider` / `codeLensProvider` | `{resolveProvider:false}` |
| `executeCommandProvider` | `["wxml.references"]` |

B 的 `initialize` 同样返回 `wxml-lsp 0.1.0`；C 的 `initialize` 成功。
三次会话都以 `shutdown` + `exit` 结束，进程 exit code 均为 0。

### 3.2 请求级结果（场景 2/3/4）

下表是主流程的 36 条 LSP 操作；4 条进程快照见 §5。为控制篇幅，各会话的初始
`didOpen`（A 的 WXML/TS、B 的 WXML/TS/types.ts、C 的 WXML/JS）与随后首条
diagnostics 合并到该会话的第一条 diagnostics 行；其余 `didOpen`/`didChange`/
`didClose` 均有独立行。“诊断” 行的 severity 1 为 error、2 为 warning。
判定口径：PASS = 观察到预期行为；FAIL/BLOCKED = 该请求/能力不可用；
INFO = 显式 Unsupported 或预期的空结果，不伪造成功。

| # | 会话 | Method | 输入 / 目标 | 结果摘要 | 判定 |
| --- | --- | --- | --- | --- | --- |
| 1 | A | `initialize` | root=`component-bindings` | 见 §3.1；`positionEncoding=utf-16` | PASS |
| 2 | A | `publishDiagnostics` | `pages/index/index.wxml` | 2 条 error：组件属性 `title` 期望 string 得到 number（绑定 `total`）；事件 `tap` 的 `missingHandler` 未定义 | PASS |
| 3 | A | `publishDiagnostics` | `pages/index/index.ts` | `[]`（clean 脚本发布空集） | PASS |
| 4 | A | `hover` | `{{ pageTitle }}`（`title` 绑定） | `string` + `页面标题` | PASS |
| 5 | A | `hover` | `{{ total }}`（`count` 绑定） | `number` + `卡片总数` | PASS |
| 6 | A | `definition` | `bind:tap="onCardTap"` | `pages/index/index.ts`，0-based line 7（`onCardTap`） | PASS |
| 7 | A | `definition` | `bindtap="missingHandler"` | `null`（缺失 handler 不伪造位置） | PASS |
| 8 | A | `completion` | 事件值 `"missingHandler"`，光标在 `missing` 后 | `["onCardTap"]` | PASS |
| 9 | A | `completion` | `{{ pageTitle }}` 光标在 `page` 后 | `["pageTitle","total"]` | PASS |
| 10 | A | `documentSymbol` | WXML `index.wxml` | `view`(5) + `card`(5) + `card`(5) | PASS |
| 11 | A | `documentSymbol` | TS `index.ts`（仅 `Page(...)`，无顶层声明） | JSON-RPC error `-32803 the script outline is incomplete`，`result=null` | **FAIL/BLOCKED（P2）** |
| 12 | A | `didChange` | TS overlay：`pageTitle: '首页'` → `pageTitle: 42`（version 2） | WXML 新增 `title` 绑定 number 的 error；共 3 条 error | PASS |
| 13 | A | `hover` | 未保存 overlay 后的 `{{ pageTitle }}` | `number` + `页面标题` | PASS |
| 14 | A | `hover` | `didClose` 后的 `{{ pageTitle }}` | `string`（磁盘内容恢复） | PASS |
| 15 | A | `hover` | 重新 `didOpen` 磁盘 TS 后的 `{{ pageTitle }}` | `string` | PASS |
| 16 | A | `didOpen` | 真实 `pages/index/index.json` | 接受；无 JSON diagnostics；TS 保持 `[]`；WXML 诊断保持 2 条 error | PASS |
| 17 | A | `didOpen` | overlay `pages/index/helper.js`（磁盘不存在） | 接受且无错误；无 diagnostics（真实 JS 由 C 覆盖） | INFO |
| 18 | A | `documentSymbol` | overlay `pages/index/App.tsx` | error `-32803 the script is not an owning page or component script` | INFO（TSX 显式拒绝） |
| 19 | A | `hover` | TSX overlay | `null` | INFO（TSX Unsupported） |
| 20 | A | `shutdown` / `exit` | — | shutdown 成功，exit 0 | PASS |
| 21 | B | `initialize` | root=`cross-file-type` | `serverInfo={"name":"wxml-lsp","version":"0.1.0"}` | PASS |
| 22 | B | `publishDiagnostics` | `pages/index/index.wxml` | `code=2339`，`Property 'nmae' does not exist on type 'User'.`，line 4 | PASS |
| 23 | B | `hover` | `{{ user.name }}` | `string` + `用户显示名称` | PASS |
| 24 | B | `definition` | `{{ user.name }}` 的 `name` | ` types.ts`，0-based line 3（`name: string`） | PASS |
| 25 | B | `completion` | `{{ user.name }}`，光标在 `na` 后 | `["name","age"]` | PASS |
| 26 | B | `documentSymbol` | TS `pages/index/index.ts` | `[["user",13]]` | PASS |
| 27 | B | `shutdown` / `exit` | — | exit 0 | PASS |
| 28 | C | `initialize` | root=`js-jsdoc` | 成功 | PASS |
| 29 | C | `publishDiagnostics` | 真实 `pages/index/index.js` | `[]`（fixture `checkJs:false`，AC-007 期望不重复发布 JS 诊断） | PASS |
| 30 | C | `publishDiagnostics` | `pages/index/index.wxml` | `[]`（`js-jsdoc-off` 状态，诊断关闭） | PASS |
| 31 | C | `hover` | JS JSDoc `{{ user.name }}` | `string` + `用户显示名称` | PASS |
| 32 | C | `documentSymbol` | JS `pages/index/index.js` | `[["user",13],["unused",13]]` | PASS |
| 33 | C | `didChange` | JS overlay 顶部加 `// @ts-check`（version 2） | JS `code=2322 Type 'string' is not assignable to type 'number'.`；WXML `code=2339` `nmae` + severity 2 “1 native diagnostic(s) belong to script code and were excluded…” | PASS |
| 34 | C | `didClose` | 关闭 JS overlay | JS `[]`，WXML `[]`（磁盘状态恢复） | PASS |
| 35 | C | `didOpen` | 重新打开磁盘 JS（version 3） | JS `[]` | PASS |
| 36 | C | `shutdown` / `exit` | — | exit 0 | PASS |

> 第 11 行的 FAIL 是唯一主流程失败；第 18/19 行是 TSX 的显式静态限制，不按
> “伪造成功” 计。第 17 行的 JS overlay 不是真实磁盘文件，真实 JS 由 C 的
> 第 29–35 行覆盖。

### 3.3 关键响应原文

Hover 的实际 contents 是 Markdown 的 `ts` 代码围栏；下表摘要为围栏内的类型名
（`string`/`number`）加脚本注释原文。

组件属性/事件诊断（A，`index.wxml`）：

```text
severity 1  component property "title" expects string, got number
            (bound expression "total")
            range 2:18-2:23

severity 1  Event handler "missingHandler" for "tap" is not defined on this page
            range 2:37-2:51
```

跨文件成员错误（B，`index.wxml`）：

```text
code 2339   Property 'nmae' does not exist on type 'User'.
            range 4:29-4:33
```

JS `// @ts-check` overlay（C）：

```text
JS   code 2322  Type 'string' is not assignable to type 'number'.  range 11:6-11:12
WXML code 2339  Property 'nmae' does not exist on type 'User'.    range 4:16-4:20
WXML severity 2 type checking is partial: 1 native diagnostic(s) belong to
                script code and were excluded from this template result
```

TSX 显式 Unsupported（A，打开 `App.tsx` 后 `index.wxml` 的状态警告）：

```text
severity 2  type checking is partial: script `pages/index/App.tsx` was omitted
            because its language is not supported
```

### 3.4 磁盘不变

| 文件 | 冒烟前 SHA-256 | 冒烟后 SHA-256 |
| --- | --- | --- |
| ` tests/fixtures/component-bindings/pages/index/index.wxml` | `8f371c80…fea0d0` | 相同 |
| ` tests/fixtures/component-bindings/pages/index/index.ts` | `4e3630ba…eb5c8` | 相同 |
| ` tests/fixtures/cross-file-type/pages/index/index.wxml` | `e8041fa1…db33a3` | 相同 |
| ` tests/fixtures/js-jsdoc/pages/index/index.js` | `f6c691d5…61cf1` | 相同 |

完整 SHA-256（前后相同）：

```text
8f371c805fdf417fd5a37dc8d33d7fd1c539744d8bd925e5e7402329b1fea0d0  component-bindings/pages/index/index.wxml
4e3630ba4439bdc1821d233d7bb0d178f323c1de681e771592a8c635b69eb5c8  component-bindings/pages/index/index.ts
e8041fa188ecf8870a6abeb823710643be7b5cc6b9c30d8ac36062d9e3db33a3  cross-file-type/pages/index/index.wxml
f6c691d5660f9abd431adb84e6512d1570de57cd97329bd6a841308f7f061cf1  js-jsdoc/pages/index/index.js
```

未保存修改只通过 `didChange` overlay 完成；`didClose` 后 hover 立即回到磁盘
内容，重新 `didOpen` 后仍为磁盘内容。C 的 JS overlay 关闭后诊断也回到
`checkJs:false` 的空集。

## 4. 场景覆盖结论

| 场景 | 结论 | 证据 |
| --- | --- | --- |
| 1. 真实 fixture WXML+TS，initialize | PASS | §3.1，A 第 1–3 行 |
| 2. WXML hover | PASS | A 第 4/5/13–15 行；B 第 23 行；C 第 31 行 |
| 2. diagnostics（组件属性绑定 + 事件缺失） | PASS | A 第 2/12 行 |
| 2. diagnostics（跨文件成员 / JS `@ts-check`） | PASS | B 第 22 行；C 第 33 行 |
| 2. definition（事件 handler + 表达式成员） | PASS | A 第 6/7 行；B 第 24 行 |
| 2. completion（表达式 + 事件 handler） | PASS | A 第 8/9 行；B 第 25 行 |
| 2. documentSymbol（WXML 静态 + 脚本） | **部分 PASS / P2** | A 第 10 行 WXML PASS；A 第 11 行脚本 FAIL；B 第 26 行、C 第 32 行脚本 PASS |
| 3. 未保存脚本修改后 hover 更新 | PASS | A 第 12/13 行：string → number |
| 3. 关闭/重开恢复磁盘 | PASS | A 第 14/15 行；C 第 34/35 行 |
| 4. 多语言 didOpen WXML/JS/TS/TSX/JSON | PASS（TSX 显式 Unsupported） | A 第 2/3/16/17/18/19 行；B 第 22/23 行；C 第 29–33 行 |
| 5. 无 node/npm/server.js；唯一后代为 engine/lib/tsc | PASS | §5 |
| 6. 输出本证据文档 | PASS | 本文件 |

## 5. 进程与无 Node 审计（场景 5）

每个会话的根进程 `/proc/<pid>/cmdline` 都是：

```text
/tmp/opencode/p7-post-delete-install/versions/0.1.0/bin/wxml-lsp --stdio
```

启动 argv 为 `["<install>/bin/wxml-lsp", "--stdio"]`；没有 `--backend`，
没有 wrapper。三个会话的后代审计：

| 会话 | 唯一后代可执行文件 | forbidden 命中 | 活动快照 |
| --- | --- | --- | --- |
| A component-bindings | `{tsc}` | 无 | 仅 `engine/lib/tsc` |
| B cross-file-type | `{tsc}` | 无 | 仅 `engine/lib/tsc` |
| C js-jsdoc | `{tsc}` | 无 | 仅 `engine/lib/tsc` |

代表性后代 cmdline：

```text
.../versions/0.1.0/engine/lib/tsc --api --async \
  --cwd /tmp/opencode/p7-final-functional-smoke/tmp/wxml-lsp-<pid>-...-1 \
  --useCaseSensitiveFileNames=true \
  --callbacks=readFile,fileExists,directoryExists,getAccessibleEntries,realpath,stat,writeFile:noop
```

B 还观察到启动时的 `engine/lib/tsc --version`（manifest 引擎探测）；A 在
会话中后段观察到第二个 `engine/lib/tsc --api` 实例（native session 重启）。两者都是同一个
engine 二进制；任一活动快照中只有 `engine/lib/tsc`，不存在 node、npm、
server.js、backend_node 或 backends/typescript。

PATH tripwire 结果：

```text
node_tripwire_marker: False
node_tripwire_log:    (empty)
forbidden_process_hits: []
```

即没有 Node 进程，也没有任何 Node 启动尝试。安装目录内容扫描也无 Node
产物（§1）。

## 6. 发现与严重级别

严重级别口径：P0 = 崩溃/数据损坏/安全；P1 = 主能力静默失效；P2 = 单一能力
可用性缺口；P3 = 信息性限制。

### F1（P1）JSON didOpen 静默丢弃脚本首次诊断，且不重试

**复现（最小）**：

```text
initialize
didOpen pages/index/index.ts   // overlay 末尾追加 const broken: number = 'x';
didOpen pages/index/index.json // 立即打开，不等待
wait 6s / 20s
→ 没有 index.ts 的 publishDiagnostics
```

受控 probe 结果（`probe_diag.log`、`probe_diag2.log`、`probe_diag3.log`、
`probe_diag6.log`）：

| 顺序 | 观察 |
| --- | --- |
| TS only | `index.ts v1 n=1 [2322]` → PASS |
| WXML + TS | `index.ts v1 n=1 [2322]` → PASS |
| WXML + TS + JSON | 只有 WXML 诊断；没有 TS publish → **BLOCKED** |
| TS + WXML + JSON | 同上 → **BLOCKED** |
| TS + JSON，等待 20 s | 仍无 TS publish → **BLOCKED** |
| TS 先等 2 s，再 JSON | TS `[2322]` 已先发布；JSON 打开后没有新 publish，已显示的错误保留 → PASS |
| 上述被丢弃后 `didChange` TS v2 | `index.ts v2 n=1 [2322]` → 恢复 |

服务端 stderr 同时出现：

```text
[wxml-lsp] dropping script diagnostics for an old revision
```

日志对应 ` crates/wxml-lsp/src/handlers/diagnostics.rs` 的 old revision 分支；
与同一函数中 old backend epoch 分支不同，该分支直接 `return`，没有
`mark_dirty` 重试。观察到的关联：JSON overlay 会触发项目 rediscovery /
project revision bump，使仍在进行中的脚本诊断 stamp 变成旧 revision；结果被
丢弃后没有重新调度。
这不是 JSON 自身诊断的问题（JSON 本来不发布诊断），而是脚本诊断的静默丢失。

**影响**：工作区恢复/多标签启动时，如果 `.json` 与脚本几乎同时 `didOpen`，
脚本的首次类型错误可能不显示，直到用户编辑该脚本。WXML 模板诊断不受影响，
仍会发布并检查模板。**最高严重级别取 P1**，因为它是主诊断能力的静默丢失，
且存在常见触发顺序；不是 P0（无崩溃、无数据损坏，编辑可恢复）。

### F2（P2）无顶层声明的脚本 documentSymbol 返回错误

A 的 `pages/index/index.ts` 只有 `Page({...})`，没有顶层声明。请求：

```text
textDocument/documentSymbol → {"code":-32803,"message":"the script outline is incomplete"}
result = null
```

服务端 stderr：

```text
[wxml-lsp] script document symbols are partial with no symbols: 1 skipped, truncated=false
```

native 返回 `Partial` 且符号为空，LSP 层按 fail-closed 规则返回
`REQUEST_FAILED`，而不是空数组。对比：

- B 的 `index.ts` 有顶层 `const user` → `[["user",13]]`；
- C 的 `index.js` 有 `user`/`unused` → `[["user",13],["unused",13]]`；
- A 的 WXML 静态 outline → `view` + 2×`card`，正常。

**影响**：常见的“只有 Page/Component 注册”的脚本在编辑器里请求 outline 会
报错，而不是返回空 outline 或注册内部成员。它是单一能力缺口，且 fail-closed
避免了伪造空结果；**P2**，不是 native 路径阻塞。

### F3（INFO）TSX 显式 Unsupported

`pages/index/App.tsx` 的 overlay `didOpen` 被接受，但：

- WXML 诊断出现 severity 2：`script pages/index/App.tsx was omitted because
  its language is not supported`；
- TSX `documentSymbol` → `-32803 the script is not an owning page or component
  script`；
- TSX `hover` → `null`；
- 没有为 TSX 伪造符号、类型或诊断。

这是 native 无 JSX lexer 的既有静态限制，记录为 INFO，不作为回归。

### F4（INFO）JSON didOpen 没有诊断

`pages/index/index.json` 真实文件 `didOpen` 被接受，未发布 JSON 诊断；TS 保持
clean 空集，WXML 诊断不变。fixture 没有 JSON 语义检查需求；这里只确认多语言
生命周期不崩溃、不产生错误状态。

### F5（INFO）`js-jsdoc` 的空诊断是预期

`js-jsdoc/tsconfig.json` 是 `allowJs:true, checkJs:false`。因此初次打开
`index.js` 发布空集、WXML 也发布空集，符合 AC-007 “不重复发布 JS 诊断” 和
native matrix 的 `js-jsdoc-off` 状态。给 JS overlay 加 `// @ts-check` 后，
JS `2322` 和 WXML `2339` 都出现，说明 JS 诊断路径本身可用，不是能力缺失。

## 7. 能力矩阵

| 能力 | 结果 | 说明 |
| --- | --- | --- |
| initialize / capabilities | PASS | utf-16；hover/definition/completion/documentSymbol 等均声明 |
| WXML 语法 + 组件属性诊断 | PASS | `title` string/number |
| 事件缺失诊断 | PASS | `missingHandler` |
| WXML hover | PASS | 类型 + JSDoc |
| WXML definition（事件 handler） | PASS | `onCardTap` → `index.ts` |
| WXML definition（表达式成员） | PASS | B `user.name` → ` types.ts` |
| 事件 handler completion | PASS | `onCardTap` |
| 表达式/静态 completion | PASS | `pageTitle/total`；`user.name/age` |
| WXML documentSymbol | PASS | 静态 outline |
| 脚本 documentSymbol | **P2 部分** | B/C 有顶层声明 PASS；A 无顶层声明 FAIL |
| 未保存脚本 → WXML hover 更新 | PASS | string → number |
| 关闭/重开恢复磁盘 | PASS | string 恢复 |
| JS JSDoc hover / symbols | PASS | C |
| JS `@ts-check` 诊断 | PASS | 2322 |
| TSX 语义 | **显式 Unsupported** | 警告 + 拒绝，不伪造 |
| JSON didOpen | PASS（仅生命周期） | 无诊断 |
| 脚本诊断在 JSON 并发打开时 | **P1 已修复** | 原静默丢弃已修；真实 LSP 回归见 [P1 证据](2026-11-20-tsgo-p1-script-diagnostics-retry.md) |
| 无 Node / 进程树 | PASS | 仅 `engine/lib/tsc` |
| shutdown / exit | PASS | 三个会话都 exit 0 |

**总体判定**：native 默认路径可用且无 Node，**PASS-with-followup**；功能级主路径
32/36 PASS，1 FAIL（P2），3 INFO；原 P1 竞态已由
[`180fa42` 的修复](2026-11-20-tsgo-p1-script-diagnostics-retry.md) 解决并真实复验。
不宣称“所有能力无缺口”：P2（仅 `Page(...)`/`Component(...)` 的脚本
`documentSymbol` 返回 `-32803`）保留为已知限制，需要产品决策
（空 outline 还是注册级 outline）。

## 8. 复现命令

```bash
cd /tmp/opencode/p7-final-functional-smoke

# 主流程（36 条 LSP 操作 + 4 条进程快照）
env -u WXML_LSP_BIN -u WXML_TYPE_BACKEND -u WXML_NODE -u WXML_BACKEND_DIR \
  python3 smoke.py

# F1 受控 probe
env -u WXML_LSP_BIN -u WXML_TYPE_BACKEND -u WXML_NODE -u WXML_BACKEND_DIR \
  python3 probe_diag.py
env -u WXML_LSP_BIN -u WXML_TYPE_BACKEND -u WXML_NODE -u WXML_BACKEND_DIR \
  python3 probe_diag2.py
env -u WXML_LSP_BIN -u WXML_TYPE_BACKEND -u WXML_NODE -u WXML_BACKEND_DIR \
  python3 probe_diag3.py
env -u WXML_LSP_BIN -u WXML_TYPE_BACKEND -u WXML_NODE -u WXML_BACKEND_DIR \
  python3 probe_diag6.py
```

证据产物（全部在仓库外）：

| 文件 | 内容 |
| --- | --- |
| `/tmp/opencode/p7-final-functional-smoke/report.json` | 36 条 LSP 操作 + 4 条快照、完整诊断、磁盘哈希、进程树 |
| `/tmp/opencode/p7-final-functional-smoke/run.log` | 主流程 stdout |
| `/tmp/opencode/p7-final-functional-smoke/logs/*.stderr.log` | 三个会话与 probe 的服务端 stderr |
| `/tmp/opencode/p7-final-functional-smoke/probe_diag*.log` | F1 的受控顺序与 didChange 恢复观察 |
| `/tmp/opencode/p7-final-functional-smoke/probe_diag*.py` | probe 源码 |

## 9. 限制与未覆盖

- 只验证 Linux x86_64 安装包；没有 Windows/macOS、升级/回滚、跨机器复现。
- 只通过 raw LSP 验证；没有 Zed GUI。真实 Zed 的 16/16 冒烟是另一份证据
  （` docs/evidence/2026-11-20-tsgo-p7-post-delete-revalidation.md` §6），
  本文件不重复。
- TSX 是 overlay（磁盘不存在的 `pages/index/App.tsx`）；这足以验证显式
  Unsupported，但不是真实 TSX 项目的完整验收。
- 未覆盖 rename、references、codeAction、semanticTokens、formatting、
  linkedEditingRange、documentLink、workspaceSymbol、signatureHelp 的行为；
  这些在 initialize 中声明，但不在本次功能级主路径。
- F1 的“脚本诊断”在 probe 中用的是 TS overlay + 故意 `2322`；WXML 诊断仍正常。
  JS 可能走同一调度路径，但本次只在 TS 上做了受控复现，未扩大断言。
- JSON 只有 didOpen 生命周期和“不发布诊断/不崩溃”的观察；没有 JSON 语义
  （例如 `usingComponents`）诊断验收。
