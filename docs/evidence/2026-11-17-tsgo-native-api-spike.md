# tsgo 原生 API 能力试验（P1-A）

日期：2026-11-17。状态：P1-A 原生能力 spike 已完成；本文只记录本机实测，不表示迁移或生产实现完成。  
执行工作区（只读基线）：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`（分支 `tsgo/migration`，基线 `b9616c5`）。  
实验目录（仓库外，临时）：`/tmp/opencode/tsgo-spike`。本轮未修改仓库源码、测试、计划、ADR、Cargo 文件或其它文档；仓库唯一写入为本文件。实验目录中的脚本与原始日志不作为仓库证据保存，关键原始输出已内嵌本文。

## 0. 结论摘要（供 P1 闸门 A）

**建议：有条件 PROCEED（推进 P2/P3），同时修订 ADR-0008 与 F0/F1 合同。** 不是 BLOCK，但不能按原 ADR 文字直接进入实现。

| 判定项 | 结果 | 依据 |
| --- | --- | --- |
| Rust 直接驱动原生 tsgo（`--api --async`，stdio + Content-Length JSON-RPC） | **PASS** | 无 Node，initialize/createSnapshot/查询均成功 |
| 原生进程可服务多个项目/快照 | **PASS** | 合成程序、inferred 项目、配置项目均可用 |
| 全虚拟 TypeScript 项目（未保存内容，不落盘） | **PASS** | `fileSystem.kind="full"` + `createPrograms`/`openFiles`；业务文件不在磁盘 |
| 未保存内容更新 | **PARTIAL** | 仅 `closeFiles`+`openFiles` 同一 `updateSnapshot` 生效；单纯 layer/full/notification 不刷新已打开文件 |
| 语义诊断 | **PASS** | `getSemanticDiagnostics`，code/range/message 完整 |
| 类型查询 | **PASS** | `getTypeAtPosition` + `typeToString`；重载签名可显示 |
| 符号/文档/JSDoc | **PASS** | `getSymbolAtPosition`、`getDocumentationComment`、`getJsDocTags` |
| 引用/定义 | **PARTIAL** | 无 `getDefinitionAtPosition`；用 `getReferencedSymbolsForNode` 的 node handle + AST 解码得到定义/引用范围 |
| 补全 | **PASS（带条件）** | `getCompletionsAtPosition` 成员补全成功；自动导入/`includeSymbol` 需要 `prepareAutoImports` |
| QuickInfo / hover 等价 | **PARTIAL** | 无 QuickInfo；可由类型字符串+文档+JSDoc 组合，但无 LSP MarkupContent、重载展示部分需自行实现 |
| 重命名/签名帮助/补全详情/代码修复 | **缺失** | 对应旧 LanguageService 方法全部返回 `unknown API method` |
| 二进制 AST | **PARTIAL（可解关键字段）** | 版本 9、header/字符串池/节点表/标识符/字符串字面量/SourceFile 扩展数据已实测解码；msgpack structured data、binder data、children 属性掩码未解码 |
| 坐标单位 | **PASS** | API 位置是 UTF-16 code unit；AST pos/end 也是 UTF-16；字符串池是 UTF-8/WTF-8 字节 |
| 句柄/租约 | **PARTIAL（有危险）** | 快照/符号句柄有内容校验；**node handle 跨快照可静默解析到错误节点**，必须由 Rust 做 epoch 绑定 |
| 安全/写入 | **FAIL（可缓解）** | `emit` 默认写宿主磁盘，甚至写到项目 cwd 之外；`writeFile:noop` 可阻断；默认 FS 读取无沙箱 |
| 稳定版 7.0.2 可替代 | **FAIL** | 无 `createSnapshot`/租约/`createSourceFile`；握手字段不同；AST protocol 版本 5（header 44），与 7.1 的版本 9（header 64）不兼容 |
| 性能/资源（微基准，非项目级） | **PASS（仅可行性）** | 冷启动约 3.5 ms，热查询 p50 0.06 ms / p95 0.13 ms，RSS 约 26.9 MB（noLib 小项目） |

**必须在 P2 之前修订/冻结的事项**

1. **版本线**：满足本试验需求的只有 `7.1.0-beta`（`7.0.2` 稳定版不满足）。ADR-0008 的“先比较稳定版本与候选版本，不默认 beta”必须改写为“稳定版 7.0.2 已实测不满足 snapshot/AST/租约要求；要么等待 7.1 stable，要么显式接受 7.1.0-beta 并固定 integrity”。F0 必须固定该事实与精确版本。
2. **查询组合**：QuickInfo/Definition/Rename/SignatureHelp 无直接 API，必须在 F1 明确组合规则、hover 内容模型、重载/alias/文档呈现，以及“不得伪造缺失结果”。
3. **句柄纪元**：原生 node handle 不携带快照版本，跨 `updateSnapshot` 会静默指向新 AST 的同序号节点。Rust 必须把每个 handle 绑定 `(snapshot id, file path, contentHash/nodeId)`，跨 epoch 一律拒绝；不得缓存到下一快照。
4. **写路径**：`emit`/`build`/`cleanBuild`/profile 方法存在且 `emit` 实测写磁盘（含项目外路径）。实现只允许白名单查询方法；启动时加 `writeFile:noop,removeFile:noop`，并对读回调 fail-closed；永不传 `--runExternalCode`。
5. **更新语义**：未保存文件内容更新必须 `closeFiles`+`openFiles` 同一请求；否则 layer/full FS 不生效。这要与现有 LSP `didChange` 的版本/取消模型对齐。
6. **AST 解码预算**：只证明了关键字段可解；完整 Rust 解码器（含 msgpack structured data、children 属性、binder 语义）和畸形输入性质测试仍是 P3/N 的明确工作量，不能写成“已解决”。

---

## 1. 环境

```text
$ uname -a
Linux deb 7.2.15+deb14-amd64 #1 SMP PREEMPT_DYNAMIC Debian 7.2.15-1 (2026-11-01) x86_64 GNU/Linux
$ python3 -V
Python 3.14.10
$ file <engine>
ELF 64-bit LSB executable, x86-64, version 1 (SYSV), statically linked, Go BuildID=..., stripped
$ ldd <engine>
不是动态可执行文件
```

- 全流程未安装、未调用 Node/npm/npx/pnpm/yarn；下载用 Python `urllib`，解包用 Python `tarfile`。
- 原生进程运行期间 `/proc/<pid>/children` 为空，`comm=tsc`；没有子进程/Node fallback。
- 引擎为静态链接 Go 二进制，不依赖 glibc 动态符号；ABI 结论只对 x86-64 Linux 成立，未测其它平台。

## 2. 制品来源与摘要

### 2.1 候选包与获取命令

主候选：`@typescript/typescript-linux-x64@7.1.0-beta`（npm `beta` dist-tag）。

```sh
# registry metadata（未使用 npm）
curl -fsSL 'https://registry.npmjs.org/@typescript%2ftypescript-linux-x64' -o registry.json
# 版本 metadata 中的 dist：
#   tarball   https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz
#   integrity sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==
#   shasum    5a8fbfcace1010b8c8a3b38df93c0444d3fef05b
```

验证与解包（实际执行脚本见 §14）：

```sh
python3 download/fetch.py            # 校验 sha512=dist.integrity、sha1=dist.shasum，记录 sha256
python3 download/safe_extract.py download/typescript-linux-x64-7.1.0-beta.tgz extract/7.1.0-beta
```

安全解包规则（Python 实现，实测 122 个条目全部为普通文件）：拒绝绝对路径、`./` 前缀、`..`、symlink/hardlink、其它特殊类型、realpath 逃逸。

### 2.2 7.1.0-beta 摘要（实测）

| 项 | 值 |
| --- | --- |
| registry metadata SHA-256 | `9bfa29f1c1b1c61930320c99a55e67d149a91adbca1277ac778054f942fdf21b`（347,029 B） |
| 包版本 | `7.1.0-beta`（dist-tag `beta`；`latest` 为 `7.0.2`，`next` 为 `7.1.0-dev.20261117.1`） |
| dist.integrity | `sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==` |
| SRI 校验 | **匹配**（base64(sha512(tarball)) 与 integrity 完全相等） |
| dist.shasum / sha1 | `5a8fbfcace1010b8c8a3b38df93c0444d3fef05b` / 匹配 |
| tarball 字节数 | 10,175,135 |
| tarball SHA-256 | `59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855` |
| `fileCount` / `unpackedSize` | 122 / 28,450,796（`du -sb` 与 unpackedSize 一致） |
| 可执行文件 | `package/lib/tsc`，24,522,912 B |
| 可执行文件 SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| `--version` | `Version 7.1.0-beta`（exit 0） |
| `package.json` SHA-256 | `96557a8b3bdb4bd67f2781b3b217d935c063bb43b4791e3c0e0f1c04c6b1c0f9` |
| `package.json.gitHead` | `3e9236361fa97a9e6c685f4833ac457ae2d8c50a`（与计划/ADR 的固定调研提交一致） |
| `LICENSE` | Apache-2.0，SHA-256 `a7d00bfd54525bc694b6e32f64c7ebcf5e6b7ae3657be5cc12767bce74654a47` |
| `NOTICE.txt` | SHA-256 `317cd4d9d8a4de4971222bd232bc45c010424bf0dae61be830e49532e62e4d33`（46,827 B） |
| `lib/tsc.sig` | PGP 签名（`-----BEGIN PGP SIGNATURE-----`，481 B），SHA-256 `e7daebc35e86ac9e30905031e6e83a3b5ea3d1b34b6618ab87c0ec0fc541f107`；本轮未验签 |
| `engines.node` | `>=16.20.0`（仅包元数据；运行时不需要 Node） |
| OS/CPU | linux / x64（无 `libc` 字段） |

### 2.3 稳定版对照（7.0.2，`latest`）

| 项 | 值 |
| --- | --- |
| dist.integrity | `sha512-EYdf2cNg7rgCWJnxCdJ+F3V39O8ihb37eHAu1LK8oAFizgTQbPOK7zHHXbPt8rX24COqODXeI3sIf0fCXG7H/A==`，**匹配** |
| tarball / SHA-256 | 9,669,334 B / `7ecad6f67377e831856367ab062ef394f21506a611405bf8ac0ff039348637d3` |
| 可执行文件 SHA-256 | `4f2de678286401759b3fb4475bafe35b8f32b4b3a07d92642bbf37eadc9b34a4` |
| `--version` | `Version 7.0.2` |
| gitHead | `2bd066d87f5bafd315be9f40889d0a60b9e58e0b`（不是 7.1 的固定提交） |
| LICENSE | 同 Apache-2.0 文件，SHA-256 相同 |
| 能力结论 | 见 §4.10/§10：**不满足**本试验所需的 snapshot/租约/虚拟 FS/AST 接口 |

---

## 3. 协议与启动

### 3.1 CLI（7.1.0-beta 实测）

```sh
extract/7.1.0-beta/package/lib/tsc --api --bogus
# Usage of api:
#   -async             use JSON-RPC protocol instead of MessagePack (for async API)
#   -callbacks string  comma-separated list of FS callbacks and defaults to enable
#   -cwd string        current working directory
#   -pipe string       use a named pipe (Windows) or create a private Unix socket at <path>/socket
#   -runExternalCode   allow projects to execute configured external plugins
#   -timing            collect per-request server processing time
#   -useCaseSensitiveFileNames  treat filesystem paths as case-sensitive (default true)
```

本轮启动参数（后续所有实验）：

```text
<tsc> --api --async --cwd <root> --useCaseSensitiveFileNames=true
# 回调实验额外：
--callbacks=readFile,fileExists,directoryExists,getAccessibleEntries,realpath
# 写阻断实验额外：
--callbacks=writeFile:noop,removeFile:noop
```

`--api`/`--async` 不显示在普通 `tsc --help` 中；`--async` 即 JSON-RPC；不加 `--async` 是 MessagePack 同步协议（本轮未测）。**不得**传 `--runExternalCode`。

### 3.2 帧格式

实测与固定提交源码 `tsc/internal/jsonrpc/baseproto.go` 一致：

```text
Content-Length: <N>\r\n
\r\n
<N bytes UTF-8 JSON-RPC 2.0>
```

- 客户端请求：`{"jsonrpc":"2.0","id":N,"method":"...","params":...}`。
- 服务端结果：`{"jsonrpc":"2.0","id":N,"result":...}`；错误：`{"jsonrpc":"2.0","id":N,"error":{"code":-32603,"message":"..."}}`。
- 服务端回调（FS）：反向 `method` 请求，客户端必须应答 `{"jsonrpc":"2.0","id":<server id>,"result":{...}}`。
- 缺失 `Content-Length`、负值、坏 JSON 会直接导致进程 exit 1（见 §4.8）；短 body 会一直等待（无超时）。

### 3.3 initialize 握手（7.1.0-beta）

```jsonc
// request params: null（{}、未知字段亦可；服务端不校验/不拒绝）
{"jsonrpc":"2.0","id":1,"method":"initialize","params":null}
// result
{"caseSensitivity":1,"currentDirectory":"/tmp/opencode/tsgo-spike/proj"}
```

- `params` 为 `null`、`{}`、含 `capabilities`、含未知字段 `foo` 均返回相同结果；无协议版本协商。
- **没有 protocol/version/schema 标识**：`initialize` 只返回 `caseSensitivity` + `currentDirectory`。唯一的 schema 版本在二进制 AST header 中（7.1 为 9，见 §4.5），且 7.0.2 的握手字段名不同（§4.10）。因此 F0 必须内置运行时探针，而不能依赖握手版本号。
- `caseSensitivity=1` 表示区分大小写（本机 Linux，`--useCaseSensitiveFileNames=true`）。

### 3.4 创建快照（全虚拟项目的最小请求）

```jsonc
{
  "openProjects": [],
  "createPrograms": [{
    "rootFiles": [{"uri": "file:///tmp/opencode/tsgo-spike/proj/main.ts"}],
    "compilerOptions": {"noLib": true, "strict": true}
  }],
  "fileSystem": {"kind": "full", "files": {
    "/tmp/opencode/tsgo-spike/proj/main.ts": "interface User { name: string; age: number }\n...\n"
  }}
}
```

```jsonc
// response（截断）
{
  "snapshot": 1,
  "projects": [{
    "id": "/dev/null/synthetic/1",
    "currentDirectory": "/tmp/opencode/tsgo-spike/proj",
    "dirty": false,
    "parsedCommandLine": {
      "fileNames": ["/tmp/opencode/tsgo-spike/proj/main.ts"],
      "options": {"noLib": true, "strict": true},
      "errors": []
    }
  }],
  "operation": {"createdPrograms": ["/dev/null/synthetic/1"]}
}
```

- 合成程序 id 形如 `/dev/null/synthetic/N`；`openFiles` 会创建 `/dev/null/inferred`；`openProjects`（tsconfig）会以 tsconfig 路径为 project id。
- `fileSystem.kind="full"` 是完整 VFS；`"layer"` 覆盖在上一层/宿主 FS 之上。
- 实验目录中 `proj*` 均**没有**对应磁盘文件；`getSourceFile`/诊断/类型查询仍工作，证明业务文件无需落盘。

---

## 4. 实验逐项结果与原始证据

### 4.1 E1：启动 + 分帧 + initialize — **PASS**

```text
spawn args: ['extract/7.1.0-beta/package/lib/tsc', '--api', '--async', '--cwd', '/tmp/opencode/tsgo-spike/vfs', '--useCaseSensitiveFileNames=true']
initialize result: {"caseSensitivity": 1, "currentDirectory": "/tmp/opencode/tsgo-spike/vfs"}
exit=0（close stdin 后）
```

无 stderr、无通知消息。子进程在 `initialize` 前不主动说话；客户端必须先发请求。

### 4.2 E2：握手参数/版本标识 — **PASS（无版本标识）**

见 §3.3。补充：`initialize` 对未知字段不报错；`getCurrentLanguageServerSnapshot` 在纯 stdio API 会话中返回：

```text
api: client error: getCurrentLanguageServerSnapshot requires an LSP-connected API session
```

即：**不要**把 stdio API 会话当成“共享 LSP 会话”；ADR 中“同进程 LSP+API”选项需要另做实验。

### 4.3 E3：虚拟项目创建与未保存更新 — **PASS（创建）/ PARTIAL（更新）**

创建：合成程序、inferred（`openFiles`）与配置项目（`openProjects`+VFS 中的 tsconfig）均 PASS；未落盘。

更新行为（同一进程，V1=`const a = 1;\nconst b = 2;\n`，V2 追加 `const changed = 3;`）：

| updateSnapshot changes | 读回的文件文本 |
| --- | --- |
| `fileSystem:{kind:"layer", files:{main:V2}}` | **仍是 V1** |
| `fileSystem:{kind:"full", files:{main:V2}}` | **仍是 V1** |
| `fileNotifications:{changed:[main]}` + layer | **仍是 V1** |
| `closeFiles:[main], openFiles:[main], fileSystem:{kind:"layer", files:{main:V2}}` | **V2** |
| `closeFiles:[main], openFiles:[main], fileSystem:{kind:"full", files:{main:V2}}` | **V2** |

结论：**已打开文件的未保存内容更新必须 close+reopen 同一 URI**（同一 `updateSnapshot` 请求内）。仅换 FS 或发 changed 通知不会刷新 open document 的内容。Rust 的 `didChange` 适配层必须按此实现，并在同一 snapshot 更新中重建 program handles。

补充：`updateSnapshot` 不带 `changes` 也会返回新的 snapshot id（项目列表为空增量）：

```json
{"snapshot":2,"projects":[]}
```

旧 snapshot 在未 `release` 前仍可用（引用计数）；`release` 后再用报 `snapshot 2 not found`。

现象：`openFiles` 的 inferred 项目 `parsedCommandLine.fileNames` 可能出现同一文件重复（实测 main.ts 两次）；`getSourceFileNames` 返回单条。Rust 侧必须去重，不能按 `fileNames` 长度推断文件数。

### 4.4 E4：语义查询 — **PASS（核心）/ PARTIAL / MISSING**

测试源（`proj/main.ts`，ASCII）：

```ts
interface User { name: string; age: number }
/** A documented user. */
const u: User = { name: "a", age: 1 };
u.missing;
```

UTF-16 行起点 `[0,45,71,110,121]`；`u` 使用位置 `110`，`missing` 位置 `112`，声明名位置 `78`。

**语义诊断**（`getSemanticDiagnostics`，只请求 main.ts）：

```json
[{
  "fileName": "/tmp/opencode/tsgo-spike/proj/main.ts",
  "pos": 112, "end": 119,
  "startPosition": {"line": 3, "character": 2},
  "endPosition": {"line": 3, "character": 9},
  "sourceLines": [{"line": 3, "text": "u.missing;\n"}],
  "code": 2339, "category": 1,
  "text": "Property 'missing' does not exist on type 'User'."
}]
```

- `pos/end` 与 `startPosition/endPosition` 同时存在；前者为 UTF-16 偏移，后者为 0-based line/UTF-16 character（§4.7 验证）。
- `getSyntacticDiagnostics`/`getBindDiagnostics`/`getSuggestionDiagnostics` 均存在并返回数组；`getProgramDiagnostics` 返回 `[]`。
- 诊断不含内部路径之外的敏感信息；但 panic 错误会带 Go 栈（§4.4.1/§4.8）。

**类型**：

```jsonc
// getTypeAtPosition(position=110)
{"id":65,"flags":1048576,"objectFlags":2,"isTupleType":false,"value":null,
 "isThisType":false,"symbol":{"file":"1","id":1}}
// typeToString(type=65)
"User"
```

- `getTypeAtPosition` 对非法位置返回 `null`（不是错误）。
- `typeToString` 对重载函数返回 `{ (a: number, b: number): number; (a: string, b: string): string; }`（§4.4.1）。

**符号/文档**：

```jsonc
// getSymbolAtPosition(position=110)
{
  "name": "u", "flags": 2, "checkFlags": 0,
  "declarations": ["15.262./tmp/opencode/tsgo-spike/proj/main.ts"],
  "valueDeclaration": "15.262./tmp/opencode/tsgo-spike/proj/main.ts",
  "reference": {
    "id": 281474976710662, "kind": 0,
    "file": {
      "fileName": "/tmp/opencode/tsgo-spike/proj/main.ts",
      "path": "/tmp/opencode/tsgo-spike/proj/main.ts",
      "contentHash": "ed07a28c52d39dfef9c12945c739f284",
      "parseOptionsKey": "0", "scriptKind": 3, "nodeId": "1"
    }
  }
}
// getDocumentationComment(symbol)
"A documented user."
// getJsDocTags(symbol)
[]
```

对带 `@deprecated` 的接口实测：

```json
// getDocumentationComment -> "A user."
// getJsDocTags -> [{"name":"deprecated","text":"use Person instead"}]
```

**补全**（`getCompletionsAtPosition`，位置在 `u.` 之后）：

```json
{"isIncomplete":false,"entries":[
  {"name":"name","kind":5,"sortText":"11"},
  {"name":"age","kind":5,"sortText":"11"}
]}
```

`includeSymbol:true` 时条目携带 `symbol`（含 file descriptor 与 declarations handle）。注意：位置在 `.` 上/文件 EOF 等边界会得到全局补全或 `null`；位置在 `.` 之后才是成员补全。另有 `completion list needs auto imports` 的失败路径，`includeSymbol:true` 要求快照 `prepareAutoImports`；`includeSymbol:false` 时服务端本应自动 clone 重试（源码），本试验在非法位置仍见该错误，合法位置未复现，标记 **PARTIAL/需合同测试**。

**引用**（`getReferencedSymbolsForNode`，`node` 必须是 node handle；`position` 同时给）：

```jsonc
// 本地 const u，node = 14.79.<main>，position = 42
[{
  "definition": "13.262./tmp/opencode/tsgo-spike/proj3/main.ts",
  "references": [
    "14.79./tmp/opencode/tsgo-spike/proj3/main.ts",
    "27.79./tmp/opencode/tsgo-spike/proj3/main.ts",
    "36.79./tmp/opencode/tsgo-spike/proj3/main.ts"
  ],
  "symbol": {"name":"u", "...":"..."}
}]
```

跨文件 `User`（model.ts 定义，main.ts import/使用）：

```jsonc
[
  {
    "definition": "3.266./tmp/opencode/tsgo-spike/proj3/model.ts",
    "references": ["6.79./tmp/opencode/tsgo-spike/proj3/model.ts"],
    "symbol": {"name":"User","flags":64,"declarations":["3.266...model.ts"],"...":"..."}
  },
  {
    "definition": "7.278./tmp/opencode/tsgo-spike/proj3/main.ts",
    "references": ["8.79...main.ts","16.79...main.ts","35.79...main.ts"],
    "symbol": {"name":"User","flags":2097152,"declarations":["7.278...main.ts"],"...":"..."}
  }
]
```

- 第二个条目是 import alias symbol；实现需去重/区分 alias 与 target。
- `definition`/`references` 都是 node handle，**不是位置**；必须用同快照的 `getSourceFile` AST 解码出 pos/end（§4.5）。这是 definition 等价路径的核心约束。
- `getSourceFileNames`、`getSourceFileMetadata`、`getDefaultProjectForFile`、`getSymbolsInScope`、`batchRequests` 均实测可用：

```jsonc
// getSourceFileNames
["/tmp/opencode/tsgo-spike/proj/main.ts"]
// getSourceFileMetadata
{"isDefaultLibrary":false,"isFromExternalLibrary":false,"packageJsonType":"",
 "packageJsonDirectory":"","impliedNodeFormat":1}
// batchRequests（两条查询一次往返）
{"responses":[{"method":"getTypeAtPosition","result":{...}},
              {"method":"getSymbolAtPosition","result":null}]}
```

**明确缺失的旧 LanguageService API**（逐一实测返回 `api: invalid request: unknown API method "<name>"`）：

```text
getQuickInfoAtPosition, getDefinitionAtPosition, getDefinitionAndBoundSpan,
findReferences, getReferencesAtPosition, getRenameInfo, getEditsForRename, rename,
getCompletionEntryDetails, getSignatureHelpItems, getDocumentHighlights,
getNavigationBarItems, getOutliningSpans, getCodeFixesAtPosition, getCombinedCodeFix,
organizeImports, getFormattingEditsForDocument, getSemanticClassifications,
getEncodedSemanticClassifications, getImplementationAtPosition,
getTypeDefinitionAtPosition, getBreakpointStatementAtPosition,
getBraceMatchingAtPosition, getIndentationAtPosition, getSmartSelectionRange,
getApplicableRefactors, getEditsForRefactor
```

**等价路径实测**：

| 旧 API | 原生替代 | 结果 |
| --- | --- | --- |
| QuickInfo | `getTypeAtPosition` + `typeToString` + `getDocumentationComment` + `getJsDocTags` + `declarations` | PARTIAL；无 display parts/MarkupContent/重载分段；需 Rust 组合 |
| Definition | `getReferencedSymbolsForNode(...).definition` 或 `getSymbolAtPosition(...).declarations[]` + AST 解码 | PARTIAL；范围需从 node handle 解码，且要裁剪 leading trivia |
| Rename | `getReferencedSymbolsForNode` 的 references + AST 范围 | PARTIAL；无 rename 校验、无 WorkspaceEdit、无冲突/保守规则 |
| SignatureHelp | `getSignaturesOfType` / `getResolvedSignature`（必须传 CallExpression 节点）+ AST 解码 | PARTIAL；传函数声明名的 Identifier handle 会 panic（§4.4.1） |
| CompletionEntryDetails | 条目内 `symbol` + `getTypeOfSymbol`/`getDocumentationComment` | PARTIAL（`includeSymbol:true` 且 `prepareAutoImports`） |

### 4.4.1 hover/重载/文档原始证据 — **PARTIAL**

源：

```ts
/** Adds two numbers. */
function add(a: number, b: number): number;
/** Adds two strings. */
function add(a: string, b: string): string;
function add(a: any, b: any): any { return a + b; }
/** A user. @deprecated use Person instead */
interface User { name: string; age: number }
const u: User = { name: "a", age: 1 };
const r = add(1, 2);
```

```text
getDocumentationComment(add) -> "Adds two numbers.\nAdds two strings."
getJsDocTags(add) -> []
typeToString(type of add) -> "{ (a: number, b: number): number; (a: string, b: string): string; }"
getSignaturesOfType(type) -> 2 个 signature，declaration 分别指向两个 overload 的 FunctionDeclaration handle
```

- 重载文档合并为换行字符串；没有结构化 display parts（参数名/类型分段、代码块、链接）。
- `getResolvedSignature` 传 `add` 的 CallExpression（node handle `51.215.<path>`）成功返回所选 overload（declaration `3.264...`，2 参数）；传函数声明名的 Identifier handle（`4.79.<path>`）触发 `panic: Unhandled case in resolveSignature`（服务器 recover 并返回 -32603，进程继续存活）。**Rust 必须对 node kind 做白名单校验**。

### 4.5 E5：`getSourceFile` 二进制 AST — **PARTIAL（关键字段已解，完整解码未做）**

请求：

```jsonc
{"snapshot":1,"project":"/dev/null/synthetic/1",
 "file":{"uri":"file:///tmp/opencode/tsgo-spike/proj/main.ts"}}
```

响应：`{"data":"<base64>"}`；样本 1,469 字节 / 1,960 base64 字符（async 模式恒为 JSON base64；同步 MessagePack 模式未测）。

Header（按 `tsc/internal/api/encoder/encoder.go` 的 64 字节布局，实测）：

```jsonc
{
 "version": 9,                    // 实际位于 metadata word 0x09000000 的高字节（bytes[0..3]=00 00 00 09）
 "bytes": 1469,
 "hash": "84f239c74529c1f9fe9dd3528ca207ed", // xxh3 128-bit，LE 字节序
 "parse_options": 0,
 "string_offsets": 64,
 "string_data": 184,
 "extended": 397,
 "structured": 489,
 "nodes": 489,
 "source_id": 1,                  // 文件修订身份（见 §4.6/§9）
 "source_lease": 0,               // getSourceFile 不建立租约
 "binder": 0,
 "nstrings": 15, "nnodes": 35
}
```

字符串池（UTF-8 字节；wire 上的 string index 是 uint32 字索引，恒为偶数，解码需 `index/2`）：

```text
0 121 "interface User { name: string; age: number }\n/** A documented user. */\nconst u: User = { name: \"a\", age: 1 };\nu.missing;\n"
1  37 "/tmp/opencode/tsgo-spike/proj/main.ts"
2  37 "/tmp/opencode/tsgo-spike/proj/main.ts"
3   4 "User"
4   4 "name"
5   3 "age"
6   1 "u"
...
12 18 "A documented user."
13  1 "u"
14  7 "missing"
```

节点表（28 字节/节点，LE；kind/pos/end/next/parent/data/flags；pos/end 为 UTF-16）：

```text
i=0  kind=0          pos=0   end=0   （nil 哨兵）
i=1  kind=308        pos=0   end=121 data=0x80000000  （SourceFile，extended offset 0）
i=2  kind=0xFFFFFFFF pos=0   end=120 data=3            （NodeList；kind 全 1）
i=3  kind=266        pos=0   end=44  data=0x12         （InterfaceDeclaration）
i=4  kind=79         pos=9   end=14  data=0x40000006   -> "User"（string index 6 -> ordinal 3）
i=23 kind=10         pos=94  end=98  data=0x8000004c   -> extended off=76: a=18 -> "a"（StringLiteral）
i=26 kind=8          pos=104 end=106 data=0x80000054   -> extended off=84: a=22 -> "1"（NumericLiteral）
i=33 kind=79         pos=112 end=119 data=0x4000001c   -> "missing"
```

SourceFile 扩展数据（extended offset 0，19 个 uint32 中前 5 个）：

```text
(0, 2, 4, 0, 3, ...)
 text      = 字符串 ordinal 0（整段源码）
 fileName  = ordinal 1
 path      = ordinal 2
 languageVariant = 0 (Standard)
 scriptKind      = 3 (TS)
```

**已证明**：header 字段、LE、版本 9、字符串池（UTF-8/WTF-8 字节）、节点表、identifier/string literal/numeric literal/SourceFile 扩展数据、node kind/pos/end/data 语义。  
**未证明/未实现**：structured data 的 msgpack（file references、span map、diagnostic directives、imports 索引）、binder data、children 属性位掩码到命名属性的完整树重建、WTF-8 孤立 surrogate 的 Rust 保真处理、畸形/越界/循环输入的性质测试。因此 **不得写成“AST 解码器已完成”**。

### 4.6 E6：句柄/租约/失效 — **PARTIAL（含危险行为）**

**租约**：

```text
createSourceFile {fileName:".../snippet.ts", sourceText:"const 甲 = \"乙\";\n", options:{scriptKind:3}}
  -> header source_id=1, source_lease=1
releaseSourceFile {lease:1} -> true
releaseSourceFile {lease:1} -> error "source file lease 1 not found"

retainSourceFile {file:<symbol.reference.file>} -> {"lease":2}
getCachedSourceFile {file:<同一 descriptor>} -> header source_lease=0, source_id=2, nodes=32
releaseSourceFile {lease:2} -> true
```

- `getSourceFile(snapshot, project, file)` **不建立租约**（header `source_lease=0`）；只要 snapshot 活着即可重复请求。
- `retainSourceFile` 用于固定缓存 AST；必须 `releaseSourceFile`，否则会话关闭前一直持有。`getCachedSourceFile` 返回的数据 header 不写回 lease（lease 0），不能靠数据判断租约。
- 租约/snapshot 都是引用计数：旧 snapshot 不 `release` 可继续查询；`release` 后报 `snapshot N not found`。

**符号句柄（content-addressed，安全）**：更新后旧 `symbol.reference` 对新快照查询会报：

```text
api: client error: source file is not part of the requested program
```

因为 `SourceFileDescriptor` 含 `contentHash`/`nodeId`/`parseOptionsKey`，服务器校验失败即拒绝。

**node handle（path+index，危险）**：格式 `"<encoder index>.<kind>.<path>"`。旧快照的 handle 对新快照**不会**因内容变化自动失效；实测：

```text
v1 handle 7.79./tmp/.../main.ts  = "a"（interface Outer 的属性名）
v2（顶部插入两条 const 后）同一 index 节点 = "inserted1"
getSymbolAtLocation(snapshot=2, location=该 handle) -> {"name":"inserted1", ...}   // 无错误
```

同一次对照中：

```text
out-of-range: "9999.79.<path>" -> api: client error: node handle ... could not be resolved (file may not be loaded or handle may be stale)
malformed "garbage" / "1.2" / "1.x.path" -> api: client error: invalid node handle ...
```

结论：**node handle 必须视为 epoch-scoped**。Rust 不得跨 snapshot 复用；每次使用前校验自己的 `(snapshot, path, contentHash/nodeId)` 绑定，并重新取得该快照的 AST。否则会静默返回错误符号/范围。

**AST 位置细节**：节点 `pos` 是 full start（包含 leading trivia），不是 token start。例如 CJK 源中 `值` 在行首，node pos=32/end=34，而实际 token 是 33..34（前导换行被计入 pos）。定义/引用范围必须由 Rust 裁剪 trivia（或取 identifier 子节点），不能把 `pos` 直接当 LSP range。

### 4.7 E7：UTF/坐标单位 — **PASS**

源（UTF-8 文件内容，实际字符）：

```ts
const 名字 = "汉字😀";
const 值 = 名字;
值;
```

```text
UTF-16 行起点: [0, 19, 33, 36]
UTF-8  行起点: [0, 29, 49, 54]
"值"（第 3 行）: UTF-16 pos = 33, UTF-8 pos = 49（差 16）
```

API 查询：

```jsonc
// getTypeAtPosition(position=33)  -> 正确
{"id":65,"flags":1024,"freshType":66,"regularType":65,
 "value":"汉字😀","isTupleType":false,"isThisType":false}
// getSymbolAtPosition(position=33)
{"name":"值","declarations":["12.262.....utf.ts"], "...":"..."}
// getSymbolAtPosition(position=49) -> null
// getTypeAtPosition(position=49)  -> {"flags":1,"intrinsicName":"error",...}
```

AST 解码（同一文件）：

```jsonc
{"version":9,"bytes":844,"parse_options":1,
 "string_offsets":64,"string_data":128,"extended":256,"structured":340,"nodes":340,
 "source_id":1,"source_lease":0,"binder":0,"nstrings":8,"nnodes":18}
```

字符串池偏移（UTF-8 字节）与内容：

```text
ordinal 0: utf8 54 B / utf16 36 units  "const 名字 = \"汉字😀\";\nconst 值 = 名字;\n值;\n"
ordinal 3: utf8  6 B / utf16  2 units  "名字"
ordinal 4: utf8 10 B / utf16  4 units  "汉字😀"
ordinal 5: utf8  3 B / utf16  1 unit   "值"
```

节点（identifier）：

```text
i=7  kind=79 "名字" pos=5  end=8    （`const` 后，pos 含前导空格）
i=13 kind=79 "值"   pos=24 end=26   （token 实际 25..26）
i=16 kind=79 "值"   pos=32 end=34   （token 实际 33..34）
```

结论：

- **API position = UTF-16 code unit**（不是 UTF-8 字节，不是 Unicode scalar）。
- **AST pos/end = UTF-16 code unit**（`utf16 := positionMap.UTF8ToUTF16(pos)`，源码与实测一致）；**pos 含 leading trivia**。
- **字符串池 = UTF-8 字节偏移**（WTF-8 用于孤立 surrogate，本轮未构造）。
- 现有 Rust `SourceMap` 是 UTF-8 字节；native 边界必须显式做 UTF-8↔UTF-16 转换，且转换应基于 `Vec<u16>`/原始字节处理孤立 surrogate（未验证）。

### 4.8 E8：进程失败/超时/畸形输入 — **PARTIAL（需要 Rust supervisor）**

**回调不回答 → 请求无限等待（无服务端超时）**：

```text
createSnapshot(打开 /vfs/main.ts) 发出后，服务端回调 fileExists("/vfs/main.ts")，
客户端故意不应答：
  -> 客户端 2.5 s 超时（TimeoutError）
  -> 期间另一个 initialize 请求仍成功返回 {"caseSensitivity":1,...}
```

结论：异步连接本身可并发，但单个请求可被 FS 回调永久阻塞；Rust 必须有 per-request deadline，超时后丢弃响应并按需重启。

**SIGKILL**：

```text
kill -9 后下一次 request -> BrokenPipeError(32, 'Broken pipe')
（读线程同时得到 stdout EOF；所有 pending 必须失败）
```

**SIGTERM（stdin 仍打开、阻塞在 Read）**：

```text
send SIGTERM；2 s 后 poll() 仍为 None（未退出）
close stdin 后立刻 exit=0
```

与上游 `client.ts` 注释一致：引擎阻塞在 stdin.Read()，SIGTERM 不能及时处理。**Supervisor 顺序：close stdin → 有界等待 → SIGKILL → reap**；不能只发 SIGTERM。

**stdin EOF（正常关闭）**：exit code 0。

**畸形帧**：

| 输入 | 结果 |
| --- | --- |
| `Content-Length: 5` + `notjs` | exit 1，stderr `jsontext: invalid character 'o' in literal null ...` |
| `Content-Length: -1` | exit 1，stderr `jsonrpc: invalid content length: negative value -1` |
| 无 `Content-Length` | exit 1，stderr `jsonrpc: no content length` |
| `Content-Length: 100` + 2 字节 body | 进程存活，继续等待（无读超时） |
| 正常帧后跟垃圾字节 | exit 1，stderr `jsontext: invalid character 'g' after top-level value ...` |

结论：Rust framing 必须有最大帧长、读超时、粘包/短读处理；引擎对坏帧直接退出，不能依赖引擎自身恢复。

**panic 恢复**：`getResolvedSignature` 错误 node kind 触发 Go panic，`AsyncConn.handleRequest` recover 后返回 -32603（含完整 Go 栈），进程继续服务；Rust 必须把 `-32603` 映射为内部错误并清洗栈信息，不能当空结果。

### 4.9 E9：性能/资源（仅可行性，不代表项目级） — **PASS（有限）**

```jsonc
{
 "spawn_to_initialize_ms": 3.5,
 "initialize_to_createSnapshot_ms": 1.5,
 "type_query_p50_ms": 0.06,
 "p95_ms": 0.13,
 "max_ms": 0.58,
 "rss_after_200_queries": "VmRSS: 26856 kB",
 "engine_binary_bytes": 24522912,
 "child_processes": []
}
```

测试为 noLib、单文件、单请求串行；不能外推到真实小程序项目、lib.dom/esnext 全量、跨文件或并发。P5 必须重新测量。

### 4.10 E10：稳定版 7.0.2 对照 — **FAIL（不满足）**

7.0.2 与 7.1 的**协议/ schema 不兼容**，实测：

1. CLI：`--useCaseSensitiveFileNames=true` 在 7.0.2 报 `flag provided but not defined` 并退出；7.1 支持。
2. 握手字段：7.0.2 `initialize` 返回 `{"useCaseSensitiveFileNames":true,"currentDirectory":...}`；7.1 返回 `{"caseSensitivity":1,...}`。
3. 方法缺失：7.0.2 无 `createSnapshot`、`createSourceFile`、`retainSourceFile`、`releaseSourceFile`、`getCachedSourceFile`、`parseCommandLine`、`readConfigFile`、`createProgram`。
4. 7.0.2 用 `updateSnapshot` 创建初始快照（`{"snapshot":1,"projects":[]}`），`ProjectResponse` 形状为 `rootFiles`/`compilerOptions`，不是 7.1 的 `parsedCommandLine`。
5. 虚拟 FS/文件标识：7.0.2 的 `fileSystem.kind="full"` + `openFiles` 不能得到虚拟文件内容；`file` 参数按 URI/路径处理与 7.1 不同（实测 `source file not found: file:///...`）。仅宿主磁盘文件（`openFiles:[path]`）能工作。
6. AST：7.0.2 `getSourceFile` 头部为 **protocol version 5**，header 44 字节（`string_offsets=44`），首个 u32 序列 `[0x05000000, ...]`；7.1 为 **version 9**、header 64。用 7.1 解码器读 7.0.2 数据会得到 `binder_off=134`、`nnodes=-10` 等明显错误值（印证不兼容，不是解码成功）。

因此：**当前 `latest` 稳定版不能作为本项目的原生引擎**；ADR “先验证稳定版” 的结论应记录为“7.0.2 已实测不满足”，而不是“未测”。

---

## 5. 支持方法表（7.1.0-beta 实测）

`proto.generated.ts` 的 `APIMethodInfo` 共 **177** 个方法（完整列表见附录 A）。与 WXML 后端相关的方法：

| 方法 | 状态 | 实测/说明 |
| --- | --- | --- |
| `initialize` | PASS | null/{} 均可；无版本 |
| `createSnapshot` / `updateSnapshot` | PASS | 虚拟项目；更新需 close+reopen |
| `release` | PASS | 释放 snapshot；释放后报 not found |
| `createSourceFile` | PASS | parse-only，返回带 lease 的 AST |
| `retainSourceFile` / `releaseSourceFile` / `getCachedSourceFile` | PASS | 租约可固定缓存 AST |
| `getSourceFile` | PASS | base64 AST；无租约 |
| `getSourceFileNames` / `getSourceFileMetadata` | PASS | 文件列表/元数据 |
| `getSemanticDiagnostics` / `getSyntacticDiagnostics` / `getBindDiagnostics` / `getSuggestionDiagnostics` | PASS | code/range/message 完整 |
| `getProgramDiagnostics` | PASS | 本轮为 `[]` |
| `getTypeAtPosition` / `getTypeAtLocation` / `getTypeOfSymbol` / `getDeclaredTypeOfSymbol` | PASS | 返回 TypeResponse |
| `typeToString` | PASS | 重载显示为 `{ (...); (...) }` |
| `getSymbolAtPosition` / `getSymbolAtLocation` / `getSymbolOfNode` | PASS | 返回 SymbolResponse |
| `getSymbolsInScope` | PASS | 实测返回 `u` 等 |
| `getDocumentationComment` / `getJsDocTags` | PASS | 纯字符串/标签数组 |
| `getReferencedSymbolsForNode` | PASS | definition/references 为 node handle |
| `getCompletionsAtPosition` | PASS（条件） | 成员补全；`includeSymbol` 需 `prepareAutoImports` |
| `getSignaturesOfType` | PASS | overload 列表 |
| `getResolvedSignature` | PARTIAL | 仅 CallExpression 节点正确；错误 kind 会 panic（recover） |
| `getDefaultProjectForFile` | PASS | 返回 inferred/配置项目 |
| `batchRequests` | PASS | 多查询单往返 |
| `getCurrentLanguageServerSnapshot` | PARTIAL | 纯 stdio API 会话拒绝，要求 LSP-connected session |
| `emit` / `emitToString` / `getJavaScriptEmit` / `getDeclarationEmit` | 存在 | `emit` 实测写磁盘；`emitToString` 实测不写；另两个未测，按危险方法禁用 |
| `build` / `buildReferences` / `cleanBuild` / `cleanReferences` | 存在（未调用） | 可能写/删；禁止暴露 |
| `startCPUProfile` / `stopCPUProfile` / `saveHeapProfile` | 存在（未调用） | profile 文件写入风险；禁止暴露 |
| `getDefinitionAtPosition` | **MISSING** | `unknown API method` |
| `getQuickInfoAtPosition` | **MISSING** | 同上 |
| `getCompletionEntryDetails` | **MISSING** | 同上 |
| `getSignatureHelpItems` | **MISSING** | 同上 |
| `getRenameInfo` / `getEditsForRename` / `rename` | **MISSING** | 同上 |
| `getCodeFixesAtPosition` / `getCombinedCodeFix` / `organizeImports` / `getApplicableRefactors` / `getEditsForRefactor` | **MISSING** | 同上 |
| `getDocumentHighlights` / `getNavigationBarItems` / `getOutliningSpans` | **MISSING** | 同上 |
| `getSemanticClassifications` / `getEncodedSemanticClassifications` | **MISSING** | 同上 |
| `getImplementationAtPosition` / `getTypeDefinitionAtPosition` | **MISSING** | 同上 |
| `getBraceMatchingAtPosition` / `getIndentationAtPosition` / `getSmartSelectionRange` | **MISSING** | 同上 |
| `formatNodeForInsertion` / `printNode` | 存在（未测） | 输入是 AST/base64，不是位置 |
| `parseCommandLine` / `readConfigFile` / `parseConfigFile` / `parseJsonConfigFileContent` | 存在（未测） | 配置文件解析，需另行验证 |
| `transpileModule` / `transpileDeclaration` / `resolveModuleName` | 存在（未测） | 代码生成/解析，不在最小查询路径 |
| 其余 checker 细粒度 API（约 100 个） | 存在（未逐个测） | 类型系统关系可用，但需按需验证 |

> “存在（未测）”不等于可用：P2 必须把它们按需求列入 F0 能力矩阵并逐项验证；未验证项不得在实现中当作保证。

---

## 6. 虚拟 FS 判定 — **PASS（可全虚拟）/ 更新 PARTIAL**

两条路径都实测：

1. **`fileSystem.kind="full"`**：`files` 字典即完整 VFS；业务文件不需要落盘。`createPrograms`（合成项目）、`openFiles`（inferred）与 `openProjects`+VFS tsconfig（配置项目）均可用。
2. **FS 回调**：`--callbacks=readFile,fileExists,directoryExists,getAccessibleEntries,realpath`，Python 回调只提供内存文件；服务端按回调读取并完成诊断。实测回调序列：

```text
fileExists /vfs/main.ts
directoryExists /vfs
fileExists /vfs/package.json
directoryExists /
fileExists /package.json
readFile /vfs/main.ts
fileExists /vfs/dep.ts
readFile /vfs/dep.ts
```

- 回调返回 `{kind:"missing"}` 即“不可读”，不会回退 OS（前提是该回调已注册）；`{kind:"useOS"}` 才显式回退。未注册的回调会直接走 OS FS。
- 标准库路径：服务端从**可执行文件同目录的 `lib/`** 读取（实测请求 `<exe dir>/lib/lib.esnext.full.d.ts` 及其依赖的 100+ 个 `.d.ts`）。回调可以提供这些内容；生产上直接随包放置只读 `lib/` 更简单。实测提供后诊断正确（只剩预期的 `Type 'number' is not assignable to type 'string'`）。
- 结论：Rust 可以用“快照 VFS（业务文件）+ 固定只读资产目录（lib）”实现读边界；**但回调不是沙箱**：漏注册或返回 `useOS` 就会读宿主磁盘。P2 安全评审必须逐项列出注册的回调与 `useOS` 白名单。

---

## 7. AST codec 判定 — **PARTIAL**

已实测可解并已用具体值证明：

- Header 64 字节，LE；版本在 metadata word 高字节（bytes[0..3] = `00 00 00 09`，version 9）；xxh3 128-bit 内容哈希；parse options；6 个 section 偏移；`source_id`/`source_lease`/`binder`。
- 字符串池：`uint32` pos/end 对（起点/终点相对 string data），string data 为 UTF-8/WTF-8；wire string index 是**字索引**（偶数值），需 `index/2` 得到 ordinal。
- 节点表：28 字节/节点；kind/pos/end/next/parent/data/flags；`pos/end` 为 UTF-16；NodeList kind=`0xFFFFFFFF`；data 高 2 位区分 children/string/extended。
- 具体解码：Identifier `"User"`（node 4）、StringLiteral `"a"`（node 23，extended offset 76）、NumericLiteral `"1"`（node 26）、SourceFile 扩展数据（text/fileName/path/scriptKind）。
- 畸形/越界/版本不匹配：未做性质测试；7.0.2 的 version 5 数据用 7.1 布局读取会得到明显错误值，已作为“必须校验版本”的反例。

未实现（不得声称完成）：structured data 的 msgpack 解码（referencedFiles、typeReferenceDirectives、spanMap、diagnosticDirectives、imports/moduleAugmentations 索引）、binder data、children 属性掩码到 TypeScript 命名属性、完整树重建、WTF-8 孤立 surrogate 保真、深度/分配/循环预算。P3 的 N 包必须把这些列为交付项和性质测试项。

---

## 8. UTF 判定 — **PASS**

见 §4.7。结论重述：

- API 位置：**UTF-16 code unit**。
- AST 节点 pos/end：**UTF-16 code unit**，且 `pos` 含 leading trivia。
- 字符串池：**UTF-8 字节**（WTF-8 用于孤立 surrogate）。
- LSP 边界同为 UTF-16；服务端内部即使在 `SessionOptions` 里设置 `PositionEncodingKindUTF8`，对外 API 位置仍是 UTF-16（实测确认）。不要按字段名推断 API 单位。
- Rust 内部若用 UTF-8 字节（现有 `SourceMap`），必须在每处 native 边界显式换算；禁止把 UTF-8 偏移直接传给 `getTypeAtPosition`/`getCompletionsAtPosition`。实测 UTF-8 位置 49 查询失败、UTF-16 位置 33 成功（差 16），是直接反例。

---

## 9. 句柄/租约判定 — **PARTIAL**

| 对象 | 生命周期 | 跨快照行为 | Rust 要求 |
| --- | --- | --- | --- |
| snapshot id | 引用计数；显式 `release` | 旧 snapshot 仍可查询，直到 release | 每个 epoch 只发布对应结果；release 有界 |
| node handle（`index.kind.path`） | 绑定文件路径+索引，**不绑定内容** | **可静默指向新 AST 的另一节点** | 必须绑定 `(snapshot, path, contentHash/nodeId)`；跨 epoch 拒绝；范围从同快照 AST 解码 |
| symbol reference | 含 `SourceFileDescriptor`（contentHash/nodeId/parseOptionsKey） | 内容变化后报 `source file is not part of the requested program` | 仍需绑定 epoch；不得跨快照复用 |
| source file lease | `createSourceFile` 返回；`retainSourceFile` 注册；显式 release | 与 snapshot 独立的引用计数 | 所有 lease 必须有 RAII/释放路径；会话关闭兜底 |
| `getSourceFile` 编码 | 按需编码，无租约 | 只要 snapshot 活着即可 | 不要在快照 release 后使用旧 AST |

补充：`source_id` 在内容不变时稳定（同一 revision 1），内容变化后递增（1→2）；可作为 Rust 侧 AST 身份的一部分，但它不是安全令牌，必须配合自己的 epoch。

---

## 10. 安全/写入判定 — **FAIL（可缓解，必须在实现中阻断）**

**读**：

- 不传 `fileSystem` 且不注册回调时，服务端直接读宿主 OS；实测 `../hostoutside/secret.ts` 被读取并参与类型检查（项目 cwd 之外）。
- 回调注册后，`{kind:"missing"}` 可拒绝读取；`{kind:"useOS"}` 是显式回退。回调是“客户端代理”，不是权限沙箱；漏注册即回退 OS。

**写**：

```text
# 宿主 FS（no fileSystem），调用 emit：
before: ['main.ts']
emit -> emittedFiles: [hostoutside/secret.js.map, hostoutside/secret.js, hostproj/main.js.map, hostproj/main.js]
after:  ['main.js','main.js.map','main.ts']；hostoutside 也新增 secret.js/secret.js.map
```

- **`emit` 会写宿主磁盘，且会写到项目 cwd 之外**。这是禁止暴露的方法。
- `--callbacks=writeFile:noop,removeFile:noop` 实测：`emit` 返回相同的 `emittedFiles` 列表，但**没有任何文件写入**（before/after 相同）。这是必要的第二道防线。
- `emitToString` 实测不写磁盘（只返回 `outputFiles` 文本）；如未来确有代码生成需求，只允许该方法并仍需评审。
- `build`/`buildReferences`/`cleanBuild`/`cleanReferences`/`startCPUProfile`/`saveHeapProfile` 存在于方法表，本轮未调用；它们可能写/删文件，必须在 Rust 客户端方法白名单中排除。
- `--runExternalCode` 实测可用（CLI 列出），绝不能传；content mapper 的外部执行不在本试验范围。

**建议的启动基线**（P2 冻结）：

```text
--api --async --cwd <sandbox-root> --useCaseSensitiveFileNames=true
--callbacks=readFile,fileExists,directoryExists,getAccessibleEntries,realpath,writeFile:noop,removeFile:noop
```

该完整回调串已实测可被 7.1.0-beta 接受：`initialize` 正常，且在全虚拟 VFS 上调用 `emit` 只返回 `emittedFilesContents`，未创建 `projC` 目录或任何磁盘文件。宿主 FS 的 `writeFile:noop` 阻断另行实测（见本节前文）；两项组合的宿主 FS 场景未单独跑，P2 需补一条合同测试。

Rust 只实现白名单方法：`initialize, createSnapshot, updateSnapshot, release, createSourceFile, retainSourceFile, releaseSourceFile, getCachedSourceFile, getSourceFile, getSourceFileNames, getSourceFileMetadata, getSemanticDiagnostics, getSyntacticDiagnostics, getBindDiagnostics, getSuggestionDiagnostics, getProgramDiagnostics, getTypeAtPosition, getTypeAtLocation, getTypeOfSymbol, getDeclaredTypeOfSymbol, typeToString, getSymbolAtPosition, getSymbolAtLocation, getSymbolOfNode, getSymbolsInScope, getDocumentationComment, getJsDocTags, getReferencedSymbolsForNode, getCompletionsAtPosition, getSignaturesOfType, getResolvedSignature, getDefaultProjectForFile, batchRequests`。其余默认拒绝。

---

## 11. 阻塞项（P2 前必须解决）

1. **7.1.0-beta 版本线决策**（架构/ADR）：7.0.2 不满足；要么 7.1 stable 发布后升级并重跑本试验，要么显式接受 beta 并固定 integrity。必须写进 ADR-0008 与 F0。
2. **查询组合合同**（F1）：QuickInfo/Definition/Rename/SignatureHelp/CompletionDetails 的替代路径、hover 内容模型、重载/alias/文档分段、缺失结果策略。
3. **node handle epoch 绑定**（F0/F1 + N）：跨 snapshot 静默错误已实测，必须有拒绝测试。
4. **未保存更新协议**（F1）：close+reopen 语义、版本屏障、取消/旧响应丢弃。
5. **写路径封锁**（安全）：方法白名单 + `writeFile:noop,removeFile:noop` + 读回调 fail-closed + 禁止 `--runExternalCode`。
6. **AST 完整解码与性质测试**（N）：msgpack structured data、children 掩码、binder、WTF-8、畸形输入预算。
7. **协议探针**（F0）：启动时校验 `initialize` 字段形状 + AST header version 9 + 关键方法存在性；不匹配立即失败，不做“尽力兼容”。

## 12. 未验证项（明确列表）

- 7.1.0-beta 与 7.1 stable / 后续 dev 的 API/schema 兼容性（只测了 7.1.0-beta 与 7.0.2）。
- `--api`（无 `--async`）的 MessagePack 同步协议、二进制响应路径。
- `--pipe` Unix socket 传输。
- `batchRequests` 的分页/`continuationToken`/大响应预算。
- `parseCommandLine`/`readConfigFile`/`parseConfigFile`/`parseJsonConfigFileContent`、`transpileModule`、`resolveModuleName`、`getImportAdderEdits`、`formatNodeForInsertion`、`printNode`。
- 大部分 checker 细粒度 API（约 100 个）的语义与错误行为。
- `build`/`cleanBuild`/profile 方法的写入/删除行为（只确认存在）。
- 真实小程序/跨文件/大项目/多项目/并发/长稳性能与 RSS。
- Windows/macOS/ARM64 资产与 ABI。
- 7.1.0-beta 的 `tsc.sig` PGP 验签（只记录存在与 hash）。
- 孤立 surrogate、组合字符、CRLF/BOM/hashbang 的端到端坐标（本轮只测 CJK+emoji+LF）。
- LSP-connected `getCurrentLanguageServerSnapshot` 共享快照路径。
- 自动导入补全的完整语义（`prepareAutoImports` + `includeSymbol` + 服务端 clone 重试）。
- `getResolvedSignature` 的 panic 是否在后续版本修复。

## 13. 推荐（P1 gate A）

**PROCEED with conditions / revise ADR.** 具体：

1. 将 ADR-0008 的版本策略改为：**7.0.2 已实测不满足**；原生引擎候选固定为 `7.1.0-beta`（或待 7.1 stable 发布后以其替换并重跑本证据），固定 tarball SRI、binary SHA-256、gitHead。
2. F0 增加运行时探针（握手字段、AST header version=9、关键方法存在性），不兼容 fail-closed。
3. F1 冻结查询组合与 hover/definition/rename/signature 的降级规则；明确“无 QuickInfo/DefinitionAtPosition”是设计事实。
4. P3/N 把完整 AST 解码、UTF/WTF-8、畸形输入、epoch 句柄、supervisor 超时/重启列为合同测试；本文件的“已解码字段”不能替代这些工作。
5. 安全评审把“只读回调 + 写 noop + 方法白名单 + 禁 external code”作为启动不变量，并测试 `emit` 被拒绝/不可达。

门禁判断：核心路径（启动、虚拟项目、诊断、类型、符号、文档、补全、引用、AST 关键字段、UTF、进程控制）均有 PASS 证据；缺口属于**合同/实现工作量**而非“无法实现”，因此不 BLOCK 迁移；但 ADR 与 F0/F1 必须先修订，不能按原文字直接实现。

---

## 14. 复现方式与产物

仓库外临时目录：`/tmp/opencode/tsgo-spike`（本轮结束后不保证存在；关键原始输出已内嵌本文）。

关键脚本与 SHA-256：

```text
37240ed694df00f2ecb839154d03d4902c55d8d022e94c87c28f231f0a994bb0  client.py
0dabb933337e88ac5f76153e11e12b188db6be85be077bdd8c31b499d76ae097  decode_ast.py
04abb0eb08ac2ab15a4b540e1f758322c5c61be3acd4e39a11c1999667ec5a0e  decode_ast2.py
f0766544a77f5f3d43f85adecf8565b8a8534698b181b592ed1e90669713c816  download/fetch.py
2d9911b64610a2246115b06b920f657235122447450d4416d7ae9e4c40d2c4fb  download/safe_extract.py
```

复跑最小步骤：

```sh
mkdir -p /tmp/opencode/tsgo-spike && cd /tmp/opencode/tsgo-spike
curl -fsSL 'https://registry.npmjs.org/@typescript%2ftypescript-linux-x64' -o registry.json
# 用 §2.1 的 fetch 逻辑校验 integrity/shasum 并下载 tarball
python3 download/fetch.py
python3 download/safe_extract.py download/typescript-linux-x64-7.1.0-beta.tgz extract/7.1.0-beta
./extract/7.1.0-beta/package/lib/tsc --version
# 客户端骨架见附录 B；把 EXE 指向 extract/7.1.0-beta/package/lib/tsc
python3 exp01_initialize.py
python3 exp03_queries.py
python3 exp08_utf.py
```

原始输出（本轮实际保存，供复查）：

```text
logs/exp01.stderr                logs/exp02.snapshot.json
logs/exp03.results.json          logs/exp03.sourcefile.json
logs/exp04.results.json          logs/exp05_symbol_first.json    logs/exp05_file_first.json
logs/exp06.stdout                logs/exp06_*_incl*.json
logs/exp07.results.json          logs/exp08.results.json
logs/exp09.results.json          logs/exp09b.results.json        logs/exp11b.results.json
logs/exp10_*.stderr              logs/exp12_callbacks.json       logs/exp12d_*.stderr
logs/exp13*.stderr               logs/exp14.results.json
logs/exp15.results.json          logs/exp15b.results.json        logs/exp16.callbacks.json
logs/exp17.results.json          logs/exp18_stable.json          logs/exp19_stable_methods.json
logs/exp20_stable_flow.json      logs/exp22.stderr               logs/exp23.stderr
logs/exp24.stderr                logs/exp25.stderr               logs/exp26.stderr
logs/methods.txt                 download/registry-linux-x64.json
```

---

## 附录 A：7.1.0-beta 完整 API 方法表（177 个，按 proto 顺序每行 4 个）

```text
release | releaseSourceFile | retainSourceFile | getCachedSourceFile
getSymbolOfDeclaration | batchRequests | initialize | createSnapshot
updateSnapshot | getCurrentLanguageServerSnapshot | createBuildOrchestrator | disposeBuildOrchestrator
build | buildReferences | cleanBuild | cleanReferences
createModuleResolver | releaseModuleResolver | resolveModuleName | parseCommandLine
readConfigFile | parseJsonConfigFileContent | parseConfigFile | createSourceFile
createSourceFileFromFile | transpileModule | transpileModuleFromFile | transpileDeclaration
transpileDeclarationFromFile | getDefaultProjectForFile | getSymbolAtPosition | getSymbolsAtPositions
getSymbolAtLocation | getSymbolsAtLocations | getSymbolOfSourceFile | getSymbolsOfSourceFiles
getTypeOfSymbol | getTypesOfSymbols | getDeclaredTypeOfSymbol | getNonMissingTypeOfSymbol
getSourceFile | getSourceFileNames | getSourceFileMetadata | getModeForUsageLocation
getModeForResolutionAtIndex | getResolvedModule | getResolvedModuleFromModuleSpecifier | getResolvedTypeReferenceDirective
getResolvedTypeReferenceDirectiveFromTypeReferenceDirective | getConfigFileNames | getConfigSourceFile | resolveName
getSymbolsInScope | getSignaturesOfType | getResolvedSignature | getTypeAtLocation
getTypeAtLocations | getTypeAtPosition | getTypesAtPositions | getParentOfSymbol
getMembersOfSymbol | getExportsOfSymbol | getExportSymbolOfSymbol | getSymbolOfType
getTargetOfType | getFreshTypeOfType | getRegularTypeOfType | getTypesOfType
getTypeParametersOfType | getOuterTypeParametersOfType | getLocalTypeParametersOfType | getThisTypeOfType
getAliasTypeArgumentsOfType | getAliasSymbolOfType | getObjectTypeOfType | getIndexTypeOfType
getCheckTypeOfType | getExtendsTypeOfType | getBaseTypeOfType | getConstraintOfType
getTypeParameterOfMappedType | getConstraintTypeOfMappedType | getNameTypeOfMappedType | getTemplateTypeOfMappedType
getTypeParametersOfSignature | getParametersOfSignature | getThisParameterOfSignature | getTargetOfSignature
getContextualType | getContextualTypeForArgument | getAwaitedType | getBaseTypeOfLiteralType
getNonNullableType | getTypeFromTypeNode | getWidenedType | getParameterType
getTypeParameterAtPosition | isArrayLikeType | isTypeAssignableTo | getShorthandAssignmentValueSymbol
getTypeOfSymbolAtLocation | typeToTypeNode | signatureToSignatureDeclaration | typeToString
isContextSensitive | getReturnTypeOfSignature | getRestTypeOfSignature | getTypePredicateOfSignature
getBaseTypes | getPropertiesOfType | getApparentPropertiesOfType | getApparentType
getReducedType | getPropertyOfType | getTypeOfPropertyOfType | getIndexInfoOfType
getIndexInfosOfType | getConstraintOfTypeParameter | getDefaultFromTypeParameter | getBaseConstraintOfType
getTypeArguments | getImportAdderEdits | getTrueTypeOfConditionalType | getFalseTypeOfConditionalType
getConstantValue | getSignatureFromDeclaration | getExportSpecifierLocalTargetSymbol | getAliasedSymbol
getImmediateAliasedSymbol | getTargetSymbol | getMergedSymbol | getSymbolOfNode
getSymbolOfDeclarationForChecker | getParentOfSymbolForChecker | getExportSymbolOfSymbolForChecker | getFullyQualifiedName
getExportsOfModule | getMemberInModuleExports | getJsDocTags | getDocumentationComment
isArrayType | isReadonlySymbol | getReferencesToSymbolInFile | getReferencedSymbolsForNode
getSignatureUsages | getCompletionsAtPosition | formatNodeForInsertion | getSyntacticDiagnostics
getBindDiagnostics | getSemanticDiagnostics | getSuggestionDiagnostics | getDeclarationDiagnostics
getProgramDiagnostics | getGlobalDiagnostics | getConfigFileParsingDiagnostics | printNode
emit | emitToString | getJavaScriptEmit | getDeclarationEmit
getAnyType | getStringType | getNumberType | getBooleanType
getVoidType | getUndefinedType | getNullType | getNeverType
getUnknownType | getBigIntType | getESSymbolType | getNonPrimitiveType
getWellKnownSymbols | getWellKnownSignatures | startCPUProfile | stopCPUProfile
saveHeapProfile
```

## 附录 B：最小客户端骨架（无 Node，可直接复跑 initialize + 虚拟快照）

```python
import json, subprocess

EXE = "/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc"
CWD = "/tmp/opencode/tsgo-spike"
MAIN = CWD + "/proj/main.ts"
SRC = 'interface User { name: string; age: number }\nconst u: User = { name: "a", age: 1 };\nu.missing;\n'

p = subprocess.Popen([EXE, "--api", "--async", "--cwd", CWD,
                      "--useCaseSensitiveFileNames=true"],
                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
next_id = 0
def request(method, params=None):
    global next_id
    next_id += 1
    body = json.dumps({"jsonrpc": "2.0", "id": next_id, "method": method, "params": params},
                      separators=(",", ":")).encode()
    p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    p.stdin.flush()
    while True:
        headers = {}
        while True:
            line = p.stdout.readline()
            if line in (b"\r\n", b"\n"):
                break
            k, _, v = line.decode().partition(":")
            headers[k.strip()] = v.strip()
        msg = json.loads(p.stdout.read(int(headers["Content-Length"])))
        if msg.get("id") == next_id:        # 注意：真实客户端还要处理 server->client 回调
            if "error" in msg:
                raise RuntimeError(msg["error"])
            return msg.get("result")

print(request("initialize", None))
cs = request("createSnapshot", {
    "openFiles": [{"uri": "file://" + MAIN}],
    "fileSystem": {"kind": "full", "files": {MAIN: SRC}},
})
print(cs)
print(request("getSemanticDiagnostics", {
    "snapshot": cs["snapshot"], "project": cs["projects"][0]["id"],
    "files": [{"uri": "file://" + MAIN}],
}))
p.stdin.close(); p.wait()
```

> 该骨架省略了 FS 回调、超时、并发响应路由、AST 解码；生产实现不得照抄。

---

## 附：一句话结论

原生 tsgo 7.1.0-beta 的 `--api --async` **可以被 Rust 在无 Node 条件下驱动**，虚拟项目、诊断、类型、符号、文档、补全、引用、AST 关键字段、UTF-16 坐标和进程控制均已实测；但 **7.0.2 稳定版不满足**，且 QuickInfo/Definition/Rename 等旧 API 缺失、node handle 跨快照会静默错位、`emit` 默认可写磁盘。因此 P1 gate A 的结论是：**有条件推进，先修订 ADR 版本策略与 F0/F1 合同，再进入 P2/P3**。

---

## 15. P2 F0 追加探针（2026-11-17）：B-02/B-03/B-04/B-12

状态：P2 合同补充探针，只解决 F0 阻塞项的未知性；不表示合同接受、实现完成或 G1 关闭。
环境：同一台 Linux x86-64，已解包的 `7.1.0-beta`，路径 `/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc`；**未下载、未使用 npm/node**。所有脚本与帧记录保留在 `/tmp/opencode/tsgo-spike`，本轮未修改生产源码、测试或 Cargo。

### 15.1 B-02：appendFile / chtimes 回调名

命令：

```sh
cd /tmp/opencode/tsgo-spike
python3 probe_b02_cli.py
```

每次启动参数为：

```text
<engine> --api --async --cwd /tmp/opencode/tsgo-spike/b02_empty \
  --useCaseSensitiveFileNames=true --callbacks=<case>
```

| `--callbacks` | 启动行为 | stderr 首行 | 退出 |
| --- | --- | --- | --- |
| `appendFile` | 失败 | `panic: unknown callback name: appendFile` | 2 |
| `appendFile:error` | 失败 | `panic: unknown callback name: appendFile` | 2 |
| `appendFile:noop` | 失败 | `panic: unknown callback name: appendFile:noop` | 2 |
| `chtimes` | 失败 | `panic: unknown callback name: chtimes` | 2 |
| `chtimes:error` | 失败 | `panic: unknown callback name: chtimes` | 2 |
| `chtimes:noop` | 失败 | `panic: unknown callback name: chtimes:noop` | 2 |
| `readFile:error`（对照） | 接受 | 空 | 0（stdin close 后正常退出） |
| `writeFile:error`（对照） | 接受 | 空 | 0 |
| `removeFile:error`（对照） | 接受 | 空 | 0 |
| `foo:error`（对照） | 失败 | `panic: unknown callback name: foo` | 2 |

原始记录：`logs/b02_cli.jsonl`、`logs/b02_*.stderr`。注：append/chtimes 组在 3 s 采样时进程仍存活，关闭 stdin 后以 exit 2 结束；stderr 已出现上述 panic。源码对照（固定提交）：`tsc/internal/api/callbackfs.go` 第 40–49 行只列出 `readFile`、`fileExists`、`directoryExists`、`getAccessibleEntries`、`realpath`、`stat`、`writeFile`、`removeFile`；第 444–447 行 `AppendFile` 始终 `fs.base.AppendFile`，第 474–476 行 `Chtimes` 始终 `fs.base.Chtimes`，均无回调分支。

结论：**B-02 负结果，PARTIAL**。`appendFile`/`chtimes` 没有可注册的 deny 回调；任何 `--callbacks=appendFile*`/`chtimes*` 都会让引擎启动 panic，不能用来做写阻断。唯一可控边界是方法白名单使写路径不可达，加上运行目录最小权限/只读；不得声称“append/chtimes 已由回调 deny”。

### 15.2 B-03：回调 wire 与失败语义

命令：

```sh
python3 probe_b03.py
python3 probe_b03_remove.py
python3 probe_b03_missing.py
python3 probe_b03_cfg_missing.py
python3 probe_b03_identity.py
python3 probe_b03_write_error.py
python3 probe_b03_remove_error.py
```

成功帧示例（`logs/b03_read.frames.jsonl`、`logs/b03_write.frames.jsonl`、`logs/b03_remove_tsconfig.frames.jsonl`、`logs/b03_realpath_identity.frames.jsonl`）：

```jsonc
// readFile：params 是路径字符串
{"jsonrpc":"2.0","id":"api5","method":"readFile","params":"/tmp/opencode/tsgo-spike/b03_host/main.ts"}
{"jsonrpc":"2.0","id":"api5","result":{"kind":"value","value":"import { Dep } from \"./dep\";\n..."}}
// 否定：{"kind":"missing"}；回退：{"kind":"useOS"}

// fileExists / directoryExists：params 是路径字符串
{"jsonrpc":"2.0","id":"api6","method":"fileExists","params":"/tmp/opencode/tsgo-spike/b03_host/dep.ts"}
{"jsonrpc":"2.0","id":"api6","result":{"kind":"value","value":true}}

// getAccessibleEntries
{"jsonrpc":"2.0","id":"api8","method":"getAccessibleEntries","params":"/tmp/opencode/tsgo-spike/b12_host"}
{"jsonrpc":"2.0","id":"api8","result":{"kind":"value","value":{"files":["package.json","tsconfig.json"],"directories":["deps","src"],"symlinks":[]}}}

// realpath：value 与 identity 都是成功形状
{"jsonrpc":"2.0","id":"api7","method":"realpath","params":"/tmp/opencode/tsgo-spike/b12_host"}
{"jsonrpc":"2.0","id":"api7","result":{"kind":"identity"}}
// 另一次：{"kind":"value","value":"/tmp/opencode/tsgo-spike/b12_host"}

// writeFile：params 是 {path,data}
{"jsonrpc":"2.0","id":"api8","method":"writeFile","params":{"path":"/tmp/opencode/tsgo-spike/b03_host/out/dep.js","data":"export {};\n"}}
{"jsonrpc":"2.0","id":"api8","result":{"kind":"noop"}}

// removeFile：params 是路径字符串
{"jsonrpc":"2.0","id":"api17","method":"removeFile","params":"/tmp/opencode/tsgo-spike/b03_build/out/main.js"}
{"jsonrpc":"2.0","id":"api17","result":{"kind":"noop"}}
```

服务端回调 ID 实测是字符串 `api1`、`api2`……，不是数字。`writeFile`/`removeFile` 的 `value`/`noop` 都表示成功；`useOS` 会执行真实写/删，生产禁止。

否定/错误语义（`logs/b03_missing_*.result.json`、`logs/b03_cfg_missing_*.stderr`、`logs/b03_error.frames.jsonl`、`logs/b03_invalid.frames.jsonl`）：

| 回调 | 返回 | 结果 |
| --- | --- | --- |
| `readFile` | `{"kind":"missing"}` | 合法，表示不存在 |
| `fileExists` | `{"kind":"missing"}` | **panic**：`invalid fileExists callback response kind: missing`；创建快照请求返回 `-32603` |
| `directoryExists` | `{"kind":"missing"}` | **panic**：`invalid directoryExists callback response kind: missing` |
| `getAccessibleEntries` | `{"kind":"missing"}` | **panic**：`invalid getAccessibleEntries callback response kind: missing` |
| `realpath` | `{"kind":"missing"}` | **panic**：`invalid realpath callback response kind: missing` |
| `readFile` | JSON-RPC error `{"code":-32000,...}` | **panic**：`ipc: remote error [-32000]...`，进程/请求失败 |
| `readFile` | `{"kind":"bogus"}` | **panic**：`invalid readFile callback response kind: bogus` |

写回调的错误行为另有不同（`logs/b03_write_error.frames.jsonl`、`logs/b03_remove_error.frames.jsonl`）：

```jsonc
// writeFile 返回 JSON-RPC error：不写，emit 结果出现 5033 诊断，emittedFiles 为空
{"jsonrpc":"2.0","id":"api8","method":"writeFile","params":{"path":".../dep.js","data":"export {};\n"}}
{"jsonrpc":"2.0","id":"api8","error":{"code":-32000,"message":"probe writeFile error"}}
// emit => {"emitSkipped":false,"diagnostics":[{code:5033,text:"Could not write file ... ipc: remote error ..."}],"emittedFiles":[]}

// removeFile 返回 JSON-RPC error：cleanBuild 仍 status 0、filesDeleted 仍列出路径，文件实际仍在
{"jsonrpc":"2.0","id":"api17","method":"removeFile","params":".../out/main.js"}
{"jsonrpc":"2.0","id":"api17","error":{"code":-32000,"message":"probe removeFile error"}}
// cleanBuild => {"status":0,"filesDeleted":[".../out/main.js"]}; out/main.js 仍存在
```

因此回调 error 不是可靠的 deny：写路径会给出失败诊断/被吞掉的结果，删除路径甚至可能报告已删除；生产必须用 `writeFile:noop`/`removeFile:noop` 并且不暴露写方法，不能依赖 error 返回值。

结论：**B-03 PASS**（所列七种回调的成功/否定形状与失败语义均已实测）。重要修正：不存在路径对 `readFile` 用 `missing`，对 `fileExists`/`directoryExists` 必须用 `value:false`，对 `getAccessibleEntries` 用空 `value`，对 `realpath` 用 `identity`；不得把这些回调的 `missing` 当普通失败，也不得用 JSON-RPC error 做拒绝。

### 15.3 B-04：宿主 FS + 完整回调串

命令：

```sh
python3 probe_b04.py
```

完整串：

```text
--callbacks=readFile,fileExists,directoryExists,getAccessibleEntries,realpath,writeFile:noop,removeFile:noop
```

结果（`logs/b04_*.result.json`、`logs/b04_*.frames.jsonl`）：

| 场景 | 结果 |
| --- | --- |
| 宿主项目 + 全部读回调用 `useOS` | 创建快照成功；诊断 `2322 Type 'string' is not assignable to type 'number'`；回调计数 `fileExists=13, directoryExists=1, readFile=93`，证明宿主文件与默认库读取都经过回调 |
| 宿主项目 + 回调提供内容 | 创建快照成功；诊断 `2322`；回调计数 `fileExists=13, readFile=5, directoryExists=1` |
| 宿主项目 + 全部读回调用 `missing` | `fileExists` 返回 `missing` 导致 `-32603` panic；**非 read 回调不能用 missing** |
| 虚拟快照 + 完整串 | `createPrograms` 成功；诊断 `2322`；回调计数 `fileExists=4, directoryExists=2, readFile=2` |
| 宿主项目 + `emit`（完整串含 write/remove noop） | 返回 `emittedFiles`，但宿主目录仍只有 `main.ts/dep.ts/package.json`，未生成 `.js/.js.map` |

结论：**B-04 PASS**（范围内）。宿主 FS 下读回调确实被调用，`useOS`/`readFile missing` 生效，完整串可与虚拟快照组合，`writeFile:noop`/`removeFile:noop` 阻止 `emit` 落盘；但 `fileExists`/`directoryExists`/`getAccessibleEntries`/`realpath` 的 `missing` 是非法形状（见 B-03）。

### 15.4 B-12：tsconfig/extends/typeRoots/package.json 读边界

命令：

```sh
python3 probe_b12.py
python3 probe_b12_typeroots.py
```

已注册读回调时的实际读取路径（`logs/b12_registered.frames.jsonl`）：

```text
fileExists  <proj>/src/tsconfig.json
fileExists  <proj>/src/jsconfig.json
fileExists  <proj>/tsconfig.json
readFile    <proj>/tsconfig.json
fileExists  <proj>/../b12_base/base.json
readFile    <proj>/../b12_base/base.json
realpath    <proj>
getAccessibleEntries <proj>
getAccessibleEntries <proj>/src
fileExists  <proj>/src/index.ts
directoryExists <proj>/src
fileExists  <proj>/src/package.json
directoryExists <proj>
fileExists  <proj>/package.json
readFile    <proj>/package.json
readFile    <proj>/src/index.ts
directoryExists <proj>/../b12_outside
fileExists  <proj>/../b12_outside/secret.ts
directoryExists <proj>/deps
fileExists  <proj>/deps/foo.ts
fileExists  <proj>/../b12_outside/package.json
fileExists  <proj>/deps/package.json
directoryExists /tmp/opencode/tsgo-spike
directoryExists /tmp/opencode
directoryExists /tmp
directoryExists /
fileExists  /package.json
readFile    <proj>/../b12_outside/secret.ts
```

`typeRoots` 追加探针（`logs/b12_typeroots.frames.jsonl`）：显式 `types:["outer"]` 时观察到：

```text
directoryExists <typeRoots>
fileExists  <typeRoots>/outer.d.ts
directoryExists <typeRoots>/outer
fileExists  <typeRoots>/outer/package.json
fileExists  <typeRoots>/outer/index.d.ts
realpath    <typeRoots>/outer/index.d.ts
readFile    <typeRoots>/outer/index.d.ts
```

诊断为空，说明回调提供的 `Outer.X` 被采用。未显式 `types` 的同配置未观察到 typeRoots 读取，仍报 `2503 Cannot find namespace 'Outer'`；不能据此推断所有项目都不读 typeRoots。

边界行为：

| 场景 | 结果 | 证据 |
| --- | --- | --- |
| 项目外 import 的 `readFile/fileExists` 返回 `missing`/`false` | `2307 Cannot find module ...`，没有读取宿主同名文件的内容 | `logs/b12_outside_missing.result.json` |
| 所有读回调返回合法缺失（fileExists false、dir false、entries 空、realpath value） | `createSnapshot` 返回 `-32603 failed to create snapshot: no project found for opened file`，未回退 OS | `logs/b12_all_missing.result.json` |
| **不注册读回调** | 无任何回调；诊断包含 `2322`（项目外 `secret.ts` 的 `number` 被读入），证明读宿主 FS | `logs/b12_unregistered.result.json` |

结论：**B-12 PASS（已测配置）**。`tsconfig`、项目外 `extends`、`package.json`、源码/依赖/项目外相对 import、显式 `types` 的 `typeRoots` 都会通过已注册回调读取；合法缺失形状能阻止 OS 回退；未注册读回调确实读到宿主 FS。残余：符号链接/宿主 realpath 逃逸、未显式 `types` 的自动发现语义仍需单独测试。

### 15.5 与合同的关系

- B-03/B-04/B-12 的未知项已关闭；B-02 是负结果（PARTIAL），不能把 append/chtimes 写成“回调 deny”。
- 合同仍为 draft；维护者对 `7.1.0-beta` 的条件接受与固定 integrity 不改变 P2/G1 状态。
- 本节所有脚本/帧只在 `/tmp/opencode/tsgo-spike`，未触碰生产源码、测试、Cargo、计划或 ADR。
