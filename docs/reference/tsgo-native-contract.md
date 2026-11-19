# tsgo 原生后端 F0/F1 合同（草案）

**状态：draft / not accepted。** 维护者已**条件接受** `7.1.0-beta` 作为唯一候选基线（见 §2.1），但本文仍是 P2 草案；**只有在 P2 评审通过且 G1 关闭后才有规范效力**。不得据此声称迁移、实现、测试或验收已经完成，也不得视为默认后端切换已批准。本文不修改任何源码、测试、Cargo、计划、ADR 或其它文档；对 beta 的条件接受不等于对本文的接受。

更新日期：2026-11-17。适用基线：ADR-0008（proposed）、`docs/plans/2026-11-17-rust-tsgo-migration.md` 第 3–4、5、6 节、`docs/evidence/2026-11-17-tsgo-native-api-spike.md`（下称“P1-A 证据”；其 §15 为 2026-11-17 P2 追加探针 B-02/B-03/B-04/B-12）。

## 1. 状态、权威顺序与证据边界

### 1.1 本文件的性质

- 本文定义 **F0 原生兼容合同**与 **F1 Backend 语义合同**的草案文本，供 P2 评审、拆分测试与实现包。
- 规范词：**MUST / MUST NOT** 为强制项；**SHOULD** 为需要说明理由的默认项；**MAY** 为可选。中文“必须/不得”等同 MUST/MUST NOT。
- 本文中的 `F0-xx`、`F1-xx` 是合同项 ID；第 12 节把它们映射到**未来** Rust 测试文件和测试名。所有测试状态均为 `planned（未实现）`。
- 维护者的条件接受记录只针对**引擎候选与制品身份**（§2.1），不改变本文 draft 状态，也不关闭 G1；P2 评审通过前不得把任何 F0/F1 项标记为已实现或已验收。
- 2026-11-17 的 P2 追加探针已关闭 B-02/B-03/B-04/B-12 的原始未知项，其中 B-02 是**负结果**：`appendFile`/`chtimes` 没有可注册的回调；详见 §5.3 与证据 §15。
- 本文不定义交付/安装 schema（F3）、测试迁移账本（F2）或具体语义切片的代码布局（P3/P4）；这些由各自 owner 冻结。

### 1.2 权威顺序

冲突时按以下顺序判断：

1. 有效 ADR（当前 ADR-0008 仍为 proposed；版本接受与迁移方向以 ADR 为准）。
2. 实施计划（阶段、门禁、共享文件 owner、测试迁移规则）。
3. 本文件与既有 C0/C1 契约（`docs/reference/contracts.md`、`docs/reference/type-bridge-c1.md`）。
4. 证据记录（P1-A、P0/P1-C；只证明记录范围内的行为）。

代码事实优先于文档；发现不一致时修订对应文档，不保留两套说法。本文件与 ADR/计划冲突时，不得用本文件覆盖 ADR/计划。

### 1.3 已实测与未实测

**已由 P1-A 在 Linux x86-64、`7.1.0-beta`、无 Node 条件下实测：**

| 项 | 证据范围 |
| --- | --- |
| 原生进程启动、`--api --async`、stdio Content-Length JSON-RPC | initialize、虚拟快照、查询、退出均成功；无子进程 |
| 全虚拟 TS 项目 | `fileSystem.kind="full"` + `createPrograms` / `openFiles` / `openProjects`；业务文件不落盘 |
| 未保存内容更新 | 只有同一 `updateSnapshot` 的 `closeFiles` + `openFiles` 生效；仅 layer/full/notification 不刷新已打开文件 |
| 语义/语法/绑定诊断 | `getSemanticDiagnostics` 等返回 code/range/message |
| 类型、符号、文档、JSDoc、引用、成员补全 | `getTypeAtPosition` + `typeToString`、`getSymbolAtPosition`、`getDocumentationComment`、`getJsDocTags`、`getReferencedSymbolsForNode`、`getCompletionsAtPosition` |
| AST 关键字段 | protocol v9、header 64、字符串池 UTF-8、节点表 28 字节、identifier/string/numeric/SourceFile 扩展数据 |
| 坐标 | API position 与 AST pos/end 为 UTF-16；字符串池为 UTF-8/WTF-8 字节 |
| 句柄 | symbol descriptor 有内容校验；node handle 跨快照会静默指向新 AST 同序号节点 |
| 文件系统 | 读回调、`{kind:"missing"}`（仅 readFile/stat）、`{kind:"useOS"}`、未注册回调走 OS；`emit` 写宿主磁盘；`writeFile:noop` 阻断 |
| 回调 wire（P2 追加） | 已捕获 `readFile`/`fileExists`/`directoryExists`/`getAccessibleEntries`/`realpath`/`writeFile`/`removeFile` 的真实请求/响应帧；`fileExists`/`directoryExists`/`getAccessibleEntries`/`realpath` 不接受 `missing`；读回调 error/invalid kind 会使引擎 panic；写回调错误行为见 §5.3（证据 §15） |
| 宿主 FS + 完整回调串（P2 追加） | 完整读回调 + `writeFile:noop` + `removeFile:noop` 可接受；宿主文件读取确实经过回调；`useOS` 生效；虚拟快照可与之组合；`emit` 不落盘（证据 §15） |
| tsconfig 读边界（P2 追加） | `tsconfig`、`extends`、`package.json`、`src`/`deps`/项目外 import、`typeRoots`（显式 `types`）均观察到回调读取；未注册读回调会读宿主 FS（证据 §15） |
| append/chtimes 负结果（P2 追加） | `appendFile`/`chtimes` 不是合法回调名；任何形式都会启动 panic；源码显示二者始终走 base OS FS、无回调支持（证据 §15） |
| 进程控制 | stdin EOF exit 0；SIGKILL 后 BrokenPipe；SIGTERM 不能及时退出；坏帧 exit 1；短 body 无限等待 |
| 排除 7.0.2 | 无 `createSnapshot`/租约/`createSourceFile`；握手字段不同；AST v5/header 44；虚拟 FS 行为不同 |

**未实测，本文只提出要求、不得当成事实：**

- 完整 Page/Component 原位 shadow、Behavior/props/loops/guards 与原生 checker 的组合。
- 完整 AST 解码：structured data 的 msgpack、binder data、children 属性位掩码、完整树重建、WTF-8 孤立 surrogate 保真、畸形/越界/循环输入的性质测试。
- 保守 rename 的完整条件、`expected_text` 匹配与编辑后复检。
- hover 对旧 QuickInfo 的 display parts / MarkupContent / 重载分段的等价性。
- 补全详情、`prepareAutoImports` / `includeSymbol` / 自动导入的完整语义。
- CRLF、BOM、hashbang、组合字符、孤立 surrogate 的端到端坐标。
- 符号链接/宿主 `realpath` 逃逸的端到端负例；`typeRoots` 在未显式 `types` 时的自动发现语义。
- Windows/macOS/ARM64 资产与 ABI、`--pipe`、无 `--async` 的 MessagePack 路径。
- 多项目并发、长稳、LSP-connected `getCurrentLanguageServerSnapshot` 共享快照。
- `batchRequests` 分页/`continuationToken`、`getResolvedSignature` panic 是否在后续版本修复、PGP 验签。

**结论：本文的“已实测”只覆盖上表；其余都是待 F0/F1 测试证明的合同要求。**

## 2. F0：引擎固定、制品身份与 schema 探针

### 2.1 唯一候选发行物（维护者条件接受） [F0-01]

| 字段 | 固定值 |
| --- | --- |
| npm 包 | `@typescript/typescript-linux-x64` |
| 版本 | `7.1.0-beta`（dist-tag `beta`） |
| `package.json.gitHead` | `3e9236361fa97a9e6c685f4833ac457ae2d8c50a` |
| tarball URL | `https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz` |
| `dist.integrity`（SRI） | `sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==` |
| `dist.shasum`（SHA-1，仅元数据） | `5a8fbfcace1010b8c8a3b38df93c0444d3fef05b` |
| tarball 字节数 | `10,175,135` |
| tarball SHA-256 | `59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855` |
| unpacked 字节数 / 文件数 | `28,450,796` / `122` |
| 可执行文件 | `package/lib/tsc`（安装布局 `engine/lib/tsc`） |
| 可执行文件字节数 | `24,522,912` |
| 可执行文件 SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| `package.json` SHA-256 | `96557a8b3bdb4bd67f2781b3b217d935c063bb43b4791e3c0e0f1c04c6b1c0f9` |
| 许可证 | Apache-2.0；`LICENSE` SHA-256 `a7d00bfd54525bc694b6e32f64c7ebcf5e6b7ae3657be5cc12767bce74654a47`；`NOTICE.txt` SHA-256 `317cd4d9d8a4de4971222bd232bc45c010424bf0dae61be830e49532e62e4d33` |
| `lib/tsc.sig` | 存在，481 B，SHA-256 `e7daebc35e86ac9e30905031e6e83a3b5ea3d1b34b6618ab87c0ec0fc541f107`；**未验签**，不得声称已签名验证 |
| 平台 | 仅 `linux` / `x64`；静态链接 Go 二进制，无 glibc 动态依赖；ABI 结论仅限 x86-64 Linux |

上表是 F0 的**制品身份**。安装/构建必须在 `assets.lock`/manifest 中同时保存 SRI、tarball SHA-256、可执行文件 SHA-256、`gitHead` 和文件数；运行时必须重新校验 SRI、tarball SHA-256（若读取归档）和可执行文件 SHA-256。SHA-1 只用于与 registry 元数据核对，**不得**作为唯一安全摘要。

**维护者条件接受（2026-11-17）：** 维护者已条件接受 `7.1.0-beta`（`gitHead` 与 SRI 如上固定）作为唯一候选基线；不切换默认后端。该接受不使 ADR 自动变为 accepted，也不使本契约通过；F0 仍须 P2 评审与 G1 关闭后才能生效。若未来以 7.1 stable 替换，必须重新固定同一组制品字段并重跑 2.4/3.3/F0/F1 合同测试，不能只改版本字符串。

`engines.node >= 16.20.0` 只是包元数据；本项目运行时不得要求、启动或探测 Node。不得用 `node` 包装 `lib/tsc`。

### 2.2 明确排除 7.0.2 与浮动版本 [F0-02]

`7.0.2` **不是**回退候选，不得出现在启动、探测、测试 oracle 的默认路径中：

| 7.0.2 事实 | 后果 |
| --- | --- |
| 无 `createSnapshot`、`createSourceFile`、租约方法；初始快照只能走 `updateSnapshot` | 不满足本架构的快照/AST 要求 |
| `initialize` 返回 `useCaseSensitiveFileNames`，不是 `caseSensitivity` | 握手 schema 不兼容 |
| AST protocol v5、header 44 字节 | 与 7.1 的 v9/header 64 不兼容；用 v9 解码会得到错误值 |
| 虚拟 `fileSystem.kind="full"` + `openFiles` 不能得到虚拟文件内容 | 不满足未保存/不落盘要求 |
| `--useCaseSensitiveFileNames` 不被接受 | 启动参数不兼容 |

同样禁止 `main`、`next`、`7.1.0-dev.*` 或 registry dist-tag 浮动解析。`7.1.0-beta` 的条件接受已由维护者记录（§2.1），但 F0 仍须 P2 评审/G1 关闭后生效；若未来改用 7.1 stable，必须重新执行第 2.4、3.3 节探针并重跑 F0/F1 合同测试，不能只改版本字符串。

### 2.3 版本与制品预检 [F0-03]

启动前必须完成，任何一项失败都不得把项目内容送入引擎：

1. 路径必须是安装树内的 `engine/lib/tsc` 常规文件、可执行、非符号链接；大小与 SHA-256 匹配 2.1。
2. `package.json` 的 SHA-256 与 `gitHead` 匹配 2.1；`LICENSE`/`NOTICE.txt` 存在且摘要匹配。
3. 以清空环境、引擎目录为 cwd、10 s 超时执行 `lib/tsc --version`；stdout 去尾随空白后必须**精确等于** `Version 7.1.0-beta`。`Version 7.0.2`、空输出、非 UTF-8、非零退出均失败。
4. 探针不得读取项目目录、`tsconfig`、插件或 `node_modules`；cwd 必须是引擎目录或专用空沙箱。
5. 失败返回 `Unavailable("engine artifact mismatch")` 并记录可审计摘要；不得静默回退到其它版本或 Node。

### 2.4 API 握手、AST schema 与能力探针 [F0-04][F0-05]

`initialize` 没有协议版本协商：P1-A 实测只返回 `caseSensitivity` 与 `currentDirectory`。因此必须在启动时做**运行时探针**，不能依赖握手版本号：

1. 发送 `initialize`（`params: null` 即可）；结果必须是对象，`caseSensitivity == 1`，`currentDirectory` 为字符串；若出现 `useCaseSensitiveFileNames` 字段，判定为 7.0.2 形状并失败。
2. 在专用空 cwd 上创建最小全虚拟快照（一个 `const x = 1;` 文件），至少验证以下方法存在且返回成功：`createSnapshot`、`getSourceFileNames`、`getTypeAtPosition`、`getSourceFile`、`getSemanticDiagnostics`、`getCompletionsAtPosition`、`getReferencedSymbolsForNode`、`getDocumentationComment`、`getJsDocTags`、`typeToString`、`release`。任一处 `unknown API method` 即使启动失败，不做“尽力兼容”。
3. 对已知源码请求 `getSourceFile`，解码 base64 并校验 AST header：
   - 头部长度 64 字节；metadata word 的高字节为 **9**（实测 bytes[0..3] = `00 00 00 09`）；
   - `bytes` 等于解码后 payload 长度；
   - `string_offsets == 64`，且 `string_offsets <= string_data <= extended <= structured <= nodes <= bytes`；
   - 节点表起点 `nodes` + `nnodes * 28 <= bytes`，且与前一 section 不重叠；
   - 字符串池条目数 `nstrings`、节点数 `nnodes` 在预算内（见 3.4），且各 section 长度不为负/不溢出；
   - `source_id`/`source_lease`/`binder` 只作为元数据，不作为安全令牌。
4. 版本不是 9、header 不是 64、字段不自洽、section 越界、`nstrings`/`nnodes` 超预算：返回 `Protocol("engine AST schema mismatch")`，终止会话，不得解码出“部分结果”。
5. `7.0.2` 的 v5/header 44 数据必须被上述检查拒绝；测试必须使用记录的 v5 字节或 v5 header 构造，而不是依赖再次下载 7.0.2。
6. 探针成功后关闭最小快照并 `release`；失败时关闭 stdin、等待、必要时 SIGKILL、reap，不能留下进程。

F0 冻结的 schema 标识：**AST protocol version = 9，AST header = 64 字节**。manifest 若另设 `protocolVersion`，它表示 RPC/传输协议，不得复用来表示 AST schema；AST schema 使用 `engine.apiSchemaVersion = 9` 与 `engine.astHeaderBytes = 64`（字段名由 F3 最终冻结；本文件只冻结值）。

## 3. 进程、帧与路由

### 3.1 启动参数与环境 [F0-06]

唯一允许的启动基线（7.1.0-beta 已实测接受完整回调串）：

```text
<engine>/lib/tsc
  --api
  --async
  --cwd <sandbox-root>
  --useCaseSensitiveFileNames=true
  --callbacks=readFile,fileExists,directoryExists,getAccessibleEntries,realpath,stat,writeFile:noop,removeFile:noop
```

- MUST 使用 `--api` 与 `--async`；不得使用无 `--async` 的 MessagePack 同步路径。
- `--callbacks` 只允许上串中的读回调与 `writeFile:noop`/`removeFile:noop`；**不得**追加 `appendFile`/`chtimes`（任何形式都会在启动时 panic，见 §5.3）。
- MUST NOT 传 `--runExternalCode`，也不得通过环境变量、tsconfig 插件或包装脚本间接启用外部代码。
- MUST NOT 传 `--pipe`、`--timing` 或未在本文冻结的额外 flag；`--timing` 会改变响应字段，未经探针不得启用。
- `--cwd` 必须是后端拥有的空沙箱根，不是用户项目根；项目文件只通过虚拟快照和资产根出现。探针阶段 cwd 不得含可读项目文件。
- `--useCaseSensitiveFileNames=true` 必须显式传递；Linux 实测 `caseSensitivity=1`。Windows/macOS 的大小写语义未验证，不得据字段名假设。
- MUST 直接 `exec` 可执行文件，不经过 shell；stdin/stdout/stderr 都是独立 pipe，不合并。
- 环境必须清空后只保留最小项（至少 `LANG=C`/`LC_ALL=C`）；移除 `NODE_OPTIONS`、`NODE_PATH`、`TS_NODE_PROJECT` 及项目/插件变量。不得依赖 `HOME`、`PATH` 或用户 profile 来定位引擎/资产。
- 启动/握手超时 10 s；失败必须 `close stdin → 等待 → SIGKILL → reap`，不能只发 SIGTERM（P1-A 实测 SIGTERM 不保证退出）。

### 3.2 握手顺序

1. 启动子进程，先挂 reader/stderr 线程，再发送 `initialize`（client id 从 1 开始）。
2. 收到成功结果后执行 2.4 探针；探针完成前不得发送任何项目内容。
3. 探针失败：不创建项目快照，终止进程，返回 `Unavailable`，不得进入“降级但继续”状态。

### 3.3 帧格式 [F0-07]

- 客户端请求/回调响应统一写：`Content-Length: <N>\r\n\r\n` + 恰好 `N` 字节 UTF-8 JSON-RPC 2.0 body。不得写 BOM，不得在 body 后追加换行。
- 解析响应时：读到空行结束 header；header 最大 8 KiB；必须恰好一个 `Content-Length`（重复/缺失/负值/非十进制/带符号均失败）；body 必须恰好 N 字节；body 必须是 UTF-8 JSON 对象。
- 允许 header 名按 ASCII 大小写不敏感匹配；未知 header 可忽略，但不得用它改变长度。写入固定使用 `\r\n`。
- 短 body 不得无限等待：每个帧读取受“请求 deadline + 5 s”约束。P1-A 实测短 body 会无限等待，客户端必须自己超时。
- 坏 JSON、坏长度、stdout 出现非帧字节：`Protocol` 错误，终止并重启；不得尝试重新同步，不得把 stderr 当 stdout。
- 引擎对坏帧会直接 exit 1；客户端必须同时处理“已退出”和“仍阻塞”。

### 3.4 帧、JSON 与 AST 预算 [F0-08]

以下是**客户端预算**（不是引擎已证明的限制），F0 冻结为初始值；调整必须重新评审：

| 预算 | 初值 | 超限行为 |
| --- | ---: | --- |
| 单个请求 body | 32 MiB | 本地拒绝，`Protocol`，不发送 |
| 单个响应 body | 64 MiB | 丢弃连接，终止/重启 |
| 回调请求/响应 body | 32 MiB | 拒绝回调并返回 JSON-RPC error |
| header | 8 KiB | `Protocol`，重启 |
| JSON 嵌套深度 | 128 | `Protocol`，丢弃消息；不得关闭递归保护 |
| 单消息数组/对象条目 | 100,000 | `Protocol` |
| 单字符串 | 32 MiB | `Protocol` |
| AST 节点数 `nnodes` | 1,000,000 | 拒绝解码，`Protocol` |
| AST 字符串池条目 `nstrings` | 1,000,000 | 拒绝解码，`Protocol` |
| 同时在途客户端请求 | 8 | 本地排队，队列有界（64） |
| 同时在途服务端回调 | 16 | 拒绝多余回调并记录错误 |
| 单请求默认 deadline | 15 s | `Timeout`，丢弃响应；见第 10 节 |
| 快照创建/更新 deadline | 30 s | `Timeout`，按 6.6 恢复 |

AST 解码必须使用有界分配、迭代或深度上限遍历；不得因恶意 `next`/`parent`/`data` 造成无限循环或栈溢出。

### 3.5 请求 ID、响应路由与服务端回调 [F0-09][F0-10]

- 客户端请求 ID 为 `u64`，从 1 单调递增，**同一连接内不得复用**；客户端维护 `client_id -> pending` 表。
- 服务端发来的 FS 回调也带 `id`，其值域与客户端 ID **无关**；必须使用独立的 `callback_id -> responder` 表。不得只按 `id` 路由，更不得假设服务端 ID 与客户端 ID 不冲突。
- 消息分类（严格）：
  - 同时含 `method` 与 `id`：服务端→客户端回调请求；
  - 不含 `method`、含 `id`、且恰含 `result` 或 `error` 之一：响应，按方向对应的表匹配；
  - 含 `method` 但无 `id`：通知；v1 视为协议违规（P1-A 正常会话未观察到通知）；
  - 同时含 `method` 与 `result`/`error`，或无 `id` 的响应：协议违规。
- 每个客户端请求必须恰好收到一个响应；未知 ID、重复响应、响应类型错误：`Protocol`，失败所有 pending，终止/重启。
- 回调 ID 必须以原始 JSON 类型（数字或字符串）原样回显，不能转成 u64 再序列化。
- 回调必须由独立 dispatcher 处理，**在等待任何查询响应时仍可服务**；不得持有全局请求锁去等待同一连接，否则会死锁。回调处理器只能读当前快照/资产，不得在回调内再向引擎发查询。
- 回调超时 2 s；超时/处理失败必须返回 JSON-RPC error，不能返回伪造成功。回调处理失败导致的上游请求错误按第 10 节分类。
- 未知回调方法：返回 JSON-RPC error（例如 `-32601`），记录并失败当前请求；不得回退 OS，不得返回空内容。
- 客户端 `initialize` 必须先于任何快照/查询；在 initialize 前收到的服务端消息视为协议违规。

### 3.6 stderr 与退出诊断 [F0-11]

- stderr 必须有独立 drain 线程/任务，持续读取直到 EOF；不得因未读导致 pipe 满而阻塞引擎。
- 保留有界环形缓冲（建议 64 KiB，最多保留最后 8 KiB 用于退出诊断）；单行超长截断，不得无界增长。
- 正常运行 stderr 可以为空；出现 stderr 不自动判定请求失败，但必须记录有界摘要。非零退出时把清洗后的尾部写入内部日志/`Internal`/`Unavailable` 原因。
- `-32603` 的 message 可能含 Go panic 栈和绝对路径；对用户可见的 `BackendError` 只能给“engine internal error”类清洗文本。原始栈仅在 debug 日志、且路径脱敏后保留。
- stdout 只允许帧；任何非帧字节、伪造 header、文本日志都按 `Protocol` 处理，不能与 stderr 混用。

## 4. 方法白名单、拒绝清单与 fail-closed

### 4.1 允许的方法（仅此清单） [F0-12]

Rust 侧必须使用**类型化请求枚举**构造 method 字符串，不提供任意 `call(method, params)` 入口。允许：

**生命周期**

| 方法 | 用途 |
| --- | --- |
| `initialize` | 握手/探针 |
| `createSnapshot` | 建立不可变输入快照 |
| `updateSnapshot` | 同 epoch 的文档更新 |
| `release` | 释放 snapshot |
| `createSourceFile` | 仅 parse-only 片段/探针；返回 lease |
| `retainSourceFile` | 固定缓存 AST lease |
| `releaseSourceFile` | 释放 lease |
| `getCachedSourceFile` | 读取已 retain 的 AST |

**查询**

| 方法 | 用途 |
| --- | --- |
| `getSourceFile` | 解码 AST（definition/references/range） |
| `getSourceFileNames` | 校验快照文件集合 |
| `getSourceFileMetadata` | 区分默认库/外部库/项目文件 |
| `getSemanticDiagnostics` | C0 诊断 |
| `getSyntacticDiagnostics` | C0 诊断补充 |
| `getTypeAtPosition` | 位置类型 |
| `getTypeAtLocation` | 句柄类型 |
| `getTypeOfSymbol` | 符号类型（hover/补全详情内部） |
| `getDeclaredTypeOfSymbol` | 声明类型 |
| `typeToString` | 类型文本 |
| `getSymbolAtPosition` | 位置符号 |
| `getSymbolAtLocation` | 句柄/位置符号 |
| `getSymbolOfNode` | AST 节点符号 |
| `getDocumentationComment` | 文档字符串 |
| `getJsDocTags` | JSDoc 标签 |
| `getReferencedSymbolsForNode` | 引用/定义候选 |
| `getCompletionsAtPosition` | 成员/全局补全 |
| `getDefaultProjectForFile` | 文件所属项目路由 |

`getSignaturesOfType`、`getResolvedSignature`、`getSignatureHelpItems` **不在 C0 允许清单**；P1-A 已实测错误 node kind 会触发 panic。若未来需要签名帮助，必须单独增加 F1 合同、CallExpression 节点 kind 白名单和回归测试。

`batchRequests` 虽在 P1-A 中可用，v1 **不允许**：分页/`continuationToken`/大响应预算未验证，且必须递归校验内部 method。若未来启用，必须满足：内部 method 全部在白名单、禁止嵌套 batch、逐项预算、任一项被拒绝则整批不发送。

### 4.2 拒绝清单 [F0-13]

以下方法**永远不得**由 Rust 发送；清单不是穷尽式，真正的控制是 4.1 白名单：

| 类别 | 方法 |
| --- | --- |
| emit/生成/写路径 | `emit`、`emitToString`、`getJavaScriptEmit`、`getDeclarationEmit` |
| build/clean | `build`、`buildReferences`、`cleanBuild`、`cleanReferences`、`createBuildOrchestrator`、`disposeBuildOrchestrator` |
| profile/heap | `startCPUProfile`、`stopCPUProfile`、`saveHeapProfile` |
| 模块/配置解析（读边界未验证） | `createModuleResolver`、`releaseModuleResolver`、`resolveModuleName`、`parseCommandLine`、`readConfigFile`、`parseConfigFile`、`parseJsonConfigFileContent` |
| 代码生成/格式化（未评审） | `transpileModule`、`transpileModuleFromFile`、`transpileDeclaration`、`transpileDeclarationFromFile`、`getImportAdderEdits`、`formatNodeForInsertion`、`printNode` |
| LSP 共享会话 | `getCurrentLanguageServerSnapshot` |
| 批量 | `batchRequests`（v1） |
| 其它 | 4.1 未列出的全部 177 个方法 |

`emitToString` 在 P1-A 中实测不写磁盘，但它属于 emit 路径且未纳入生产评审，仍拒绝。`writeFile:noop` 不是“允许 emit”的理由。

### 4.3 未知/拒绝方法的 fail-closed [F0-14]

- 类型化枚举之外的 method 在**写入 stdin 之前**拒绝：`Protocol("method not allowlisted")`；不得发送、不得探测、不得重试。
- 引擎返回 `unknown API method`：
  - 探针阶段：启动失败，`Unavailable("engine API mismatch")`；
  - 运行阶段：当前请求返回 `Unsupported`/`Protocol`（按查询能力），不重试、不回退 7.0.2、不回退 Node、不返回空成功。
- 引擎未来新增方法仍默认拒绝；不得根据运行时返回动态扩充白名单。
- 任何拒绝方法/未知方法的测试都必须断言：stdin 未出现该方法、无文件写入、无结果发布。
- `--runExternalCode` 的禁止是启动不变量：argv 断言 + 进程树断言 + 插件 canary 测试，三者都要有。

## 5. 文件系统合同

### 5.1 虚拟快照 [F0-15]

- 每个项目快照 MUST 使用 `fileSystem.kind="full"`，`files` 字典包含该快照所有可读业务文件（TS/JS/`.d.ts`，以及确有需要的 `tsconfig`/`package.json` 的虚拟副本）。不得使用 `kind="layer"` 叠加宿主 FS。
- 业务文件 MUST NOT 依赖磁盘存在；P1-A 已证明 `createPrograms`、`openFiles`、`openProjects` + VFS tsconfig 均可工作。
- 未保存文档通过 `openFiles` 打开；其内容同时出现在 `fileSystem.files`。`openFiles` 只能引用快照内路径。
- `openProjects` 的 tsconfig、`extends`、`paths`、`typeRoots` 目标必须全部在 VFS 或固定资产根内；缺失时返回配置/诊断降级，不得读 OS。
- 合成程序、inferred 项目、配置项目的 project id（`/dev/null/synthetic/N`、`/dev/null/inferred`、tsconfig 路径）是引擎生成/不透明标识，Rust 不得把它们当作稳定项目键；Rust 项目键由 `AnalysisInput` 的 `ProjectId` + snapshot/epoch 组成。
- `parsedCommandLine.fileNames` 可能重复（P1-A 实测 main.ts 出现两次）；Rust MUST 去重，且不得按该数组长度推断文件数。

### 5.2 读回调注册与 OS 回退 [F0-15][F0-16]

- 启动参数必须注册所有可达读回调：至少 `readFile`、`fileExists`、`directoryExists`、`getAccessibleEntries`、`realpath`（P1-A 实测序列）。
- “可达”以固定提交 + 固定发行物的源码/二进制审计为准，并覆盖：项目/配置读取、`package.json`、模块解析、`typeRoots`、默认库、声明资产、`realpath`、目录枚举。审计必须列出**每个**读入口；未列入审计的回调视为不可达，需在 P2 重新审计。
- 每个读回调只允许两个数据来源：当前快照的虚拟文件表、固定只读资产索引；**不得**返回 `{kind:"useOS"}`（生产模式）。精确结果形状已由 2026-11-17 P2 追加探针捕获（证据 §15），必须按回调区分：

| 回调 | 引擎接受的结果（`useOS` 仅探针/显式批准时） | 非法/失败行为（实测） |
| --- | --- | --- |
| `readFile` | `{"kind":"value","value":"<text>"}`、`{"kind":"missing"}`、`{"kind":"useOS"}` | `missing` 是唯一合法“不存在”结果；`error`/未知 kind 会 panic |
| `fileExists` | `{"kind":"value","value":true/false}`、`{"kind":"useOS"}` | `missing` 会 panic：`invalid fileExists callback response kind: missing`；不存在必须返回 `value:false` |
| `directoryExists` | `{"kind":"value","value":true/false}`、`{"kind":"useOS"}` | `missing` 会 panic；不存在必须返回 `value:false` |
| `getAccessibleEntries` | `{"kind":"value","value":{"files":[],"directories":[],"symlinks":[]}}`、`{"kind":"useOS"}` | `missing` 会 panic；不存在/越界必须返回空 `value` 对象 |
| `realpath` | `{"kind":"value","value":"<path>"}`、`{"kind":"identity"}`、`{"kind":"useOS"}` | `missing` 会 panic；不得调用 OS realpath；虚拟路径优先 `identity` |
| `writeFile` / `removeFile` | 见 §5.3 | JSON-RPC error 的行为按操作不同：`writeFile` 在 `emit` 中变成 5033 诊断且不写；`removeFile` 在 `cleanBuild` 中被吞掉并仍列 `filesDeleted`；`kind:"error"`/未知 kind 走 panic 解码路径 |

- 服务端回调 ID 实测为字符串 `"api1"`、`"api2"`……，不是数字；`params` 在读回调中是路径字符串，`writeFile` 是 `{path,data}` 对象，`removeFile` 是路径字符串。客户端必须原样回显 ID。
- 读回调的 JSON-RPC error、`{"kind":"error"}` 或未知 kind **不能**作为“拒绝读取”的控制信号：实测会使引擎在请求处理 goroutine 中 panic，JSON-RPC 响应为 `-32603` 且 message 带回 Go 栈；不得依赖这种副作用做 fail-closed。写回调的错误行为见 §5.3（同样不可作为 deny）。
- 未注册回调会直接走 OS（P1-A 实测）。因此“漏注册”是安全失败，不是降级：启动自检必须证明无法读取沙箱外的 canary 文件（内容会改变类型结果），一旦读到即拒绝启动。
- 回调处理器必须无阻塞、无网络、无引擎递归调用；路径查找前先做 5.4 的规范化/限制。回调日志必须记录规范化后的请求路径与结果类别（命中/缺失/拒绝），用于读路径审计。
- 引擎的实测探测不只限于项目根：还会 `fileExists` 查询 `src/tsconfig.json`、`jsconfig.json`、父级目录、`/package.json`，`directoryExists` 查询 `/tmp`、`/tmp/opencode`、`/`，`readFile` 读取项目外相对 import、`typeRoots` 和默认库。所有这些必须由回调回答：`fileExists` 返回 `value:false`、`directoryExists` 返回 `value:false`、`getAccessibleEntries` 返回空数组、`readFile` 返回 `missing`；不得列宿主目录或读宿主文件。
- `realpath` 对项目根和 `typeRoots` 声明文件均会被调用；只能返回虚拟路径的 `value` 或 `identity`，不得解析宿主符号链接。

### 5.3 写回调与写入口 [F0-17]

- `writeFile:noop`、`removeFile:noop` 必须出现在启动回调串中；P2 追加探针确认完整串被接受，且 `emit` 返回 `emittedFiles` 但宿主目录不产生任何文件。这是第二道防线，不是权限边界。
- 显式启用 `writeFile`/`removeFile` 回调（不带 `:noop`）时，实测请求为：
  - `writeFile`：`{"path":"<绝对路径>","data":"<文本>"}`；成功返回 `{"kind":"value"}` 或 `{"kind":"noop"}`。
  - `removeFile`：`"<绝对路径>"`；成功返回 `{"kind":"value"}` 或 `{"kind":"noop"}`。
  - `useOS` 会写/删宿主，生产模式禁止。
- 回调错误不能当 deny 信号：
  - `writeFile` 返回 JSON-RPC error 时，`emit` 不写文件，但结果是 `emittedFiles: []` + 诊断 `5033 Could not write file ... ipc: remote error`；不得把它当成功。
  - `removeFile` 返回 JSON-RPC error 时，`cleanBuild` 仍可返回 `status: 0` 且 `filesDeleted` 列出该路径（错误被吞掉、文件实际仍在）；这会掩盖失败。
  - `{"kind":"error"}`/未知 kind 走 `decodeCallbackResponse` 的 panic 路径。生产上只能返回 `value`/`noop`（写）或依赖白名单不调用。
- **`appendFile`、`chtimes` 没有可注册的回调。** 已实测 `--callbacks=appendFile`、`appendFile:error`、`appendFile:noop` 均以 `panic: unknown callback name: ...` 在启动阶段失败（exit 2），`chtimes` 同样；源码 `callbackfs.go` 的 `AppendFile`/`Chtimes` 始终委托 base OS FS（证据 §15）。因此：
  - MUST NOT 把 `appendFile`/`chtimes` 加入 `--callbacks`；加入即无法启动。
  - 无法用回调 deny 它们；唯一保证是方法白名单（第 4 节）使任何写/append/chtimes 路径不可达，并在运行环境使用最小权限/只读目录作为纵深防御。
  - 任何允许清单内的方法若实际触发 AppendFile/Chtimes，视为 F0 阻塞缺陷，必须拒绝该会话并复审来源；不得把“没有观察到”当成已阻断。
  - 补偿控制：后端拥有的 cwd、项目根和资产根必须以最小权限运行（对引擎进程不可写，或位于只读挂载/只读目录）；测试必须在 append/chtimes 假设被触发时验证 OS 拒绝而不是写成功。该控制不能替代方法白名单，只能是第二层。
- 方法层白名单（第 4 节）是第一道防线：即使写回调配置错误，也不得存在可达的 emit/build/clean/profile 方法。
- 测试必须对沙箱外 canary 目录/文件做快照前后哈希，断言：允许查询不改动树；误调用被拒绝方法不产生写；`emit` 即使被测试代码尝试也不可写；`appendFile`/`chtimes` 注册串启动失败。

### 5.4 路径规范化与限制 [F0-18]

所有 URI/路径在进入 VFS、回调、handle 绑定前必须规范化：

1. 只接受 `file://` URI；拒绝其它 scheme、userinfo、query、fragment；`file://localhost/path` 可映射为本机路径，其它 authority 拒绝。
2. 百分号解码一次为 UTF-8；无效 UTF-8、解码后出现 NUL/控制字符、解码后出现编码分隔符：拒绝。
3. 必须是绝对路径；拒绝空组件、`.`、`..`（包括 `%2e%2e` 解码后的形式）；拒绝相对路径。
4. 分隔符统一为 `/`；Windows 盘符/UNC 未验证，非 Linux 平台不得假设可用。
5. 大小写：Linux 上大小写敏感，`--useCaseSensitiveFileNames=true`；不得对路径做 Unicode case folding。大小写不敏感平台的语义未验证。
6. 不调用 OS canonicalize/realpath；Rust 层在 ProjectHost 边界解析宿主符号链接并生成 canonical 虚拟路径；快照内不允许符号链接项。
7. 允许根：各项目根、`engine/lib` 资产根、`typings/<fixed>` 资产根。前缀匹配必须按**路径组件边界**，不得用字符串前缀（防止 `/proj` 匹配 `/project-evil`）。
8. `getAccessibleEntries` 只返回允许根内的子项；不得枚举 `/`、用户 home、`node_modules` 或项目外目录。
9. 任何越界/非法路径必须按回调类型 fail-closed：`readFile -> {"kind":"missing"}`；`fileExists -> {"kind":"value","value":false}`；`directoryExists -> {"kind":"value","value":false}`；`getAccessibleEntries -> 空 value`；`realpath -> {"kind":"identity"}`。不得返回 `missing` 给后四种回调（会 panic），不得尝试“修复路径后重试”。
10. 已实测引擎会探测 `/package.json`、`/tmp`、`/tmp/opencode`、`/` 等根外路径；这些探测必须返回 false/空/identity，不能因为“看起来是根目录”而枚举宿主。允许根之外的真实文件（如项目外相对 import）也必须由回调拒绝，并由诊断/`Partial` 表达，不得读 OS。

### 5.5 标准库与类型资产 [F0-19]

- 默认库由引擎从**可执行文件同目录的 `lib/`** 读取（P1-A 实测请求 `<exe dir>/lib/lib.esnext.full.d.ts` 及 100+ 个 `.d.ts`）。安装包必须随带只读 `engine/lib/**`。
- 读回调启用后，`engine/lib/**` 的读取由客户端从**已校验资产索引**提供，不得直接回退 OS；资产索引的每个文件必须有 SHA-256/大小，并与 manifest/`assets.lock` 一致。
- 小程序声明资产（`typings/miniprogram-api-typings`）同样固定版本、摘要、许可证；版本由 F3 冻结，本文只要求“固定、校验、只读、不联网”。缺失必需资产：启动/探针失败；缺失可选资产：相关查询 `Partial`，不得静默返回“干净”。
- `typeRoots` 已被实测：配置 `typeRoots` 且显式 `types:["outer"]` 时，引擎通过回调查询 `typeRoots/outer.d.ts`、`typeRoots/outer/index.d.ts`、`outer/package.json`，并对 `index.d.ts` 调用 `realpath`；未显式 `types` 的同配置未观察到 `typeRoots` 读取（不得据此推断所有项目都不读）。因此必须同时支持“显式 `types` 的读取”和“未读取时按缺失/Partial 降级”，不得读 OS 补全。
- `extends` 可位于项目根之外（实测 `../b12_base/base.json` 通过回调读取）；只允许已批准的虚拟配置/资产，越界时返回配置错误/`Partial`，不得读 OS。
- 禁止运行时 npm/registry 解析、隐式下载、读取项目 `node_modules`；项目内依赖只能以虚拟快照内容出现。
- 许可证文件（Apache-2.0 LICENSE/NOTICE、typings LICENSE）必须随包，不得删除或改写。

## 6. 快照、更新与进程生命周期

### 6.1 创建与身份 [F1-01]

- `AnalysisInput` 是不可变源快照：项目/文件 revision、源文本、编译选项、固定声明资产、模板片段、依赖指纹。`DerivedView` 是同一输入的 shadow/映射/基线 facts，不得引用可变宿主状态。
- `createSnapshot` 请求由 `openProjects`（配置项目）、`createPrograms`（合成/原位 shadow）、`openFiles`（未保存/打开文档）、`fileSystem.kind="full"` 组成。
- 快照创建成功后记录：引擎 snapshot id、每个 project 的引擎 id、文件集合、内容哈希、配置/依赖指纹、session generation。Rust 自己的 `SnapshotId` 与引擎 snapshot id 必须同时保存；不得把引擎 id 直接暴露为稳定公共 ID。
- 快照创建失败/超时/进程死亡：不得发布半成品；旧快照若仍有效则保持，否则项目进入 `Unavailable`。

### 6.2 更新语义 [F1-02]

P1-A 实测：

| 更新方式 | 已打开文件内容 |
| --- | --- |
| 仅 `fileSystem.kind="layer"` 新内容 | 仍旧 |
| 仅 `fileSystem.kind="full"` 新内容 | 仍旧 |
| `fileNotifications.changed` + layer | 仍旧 |
| `closeFiles` + `openFiles`（同一请求） | 新内容 |

因此：

- 任何未保存内容变化 MUST 在**同一次** `updateSnapshot` 请求中同时发送 `closeFiles`（同 URI）与 `openFiles`（同 URI），并携带更新后的 `fileSystem` 内容；不得拆成两次请求，不得只发通知/layer。
- 新建文件：`openFiles` + files；删除/关闭文件：`closeFiles` 并从 files 移除；重命名按“关闭旧路径 + 打开新路径”处理。
- 一次 `updateSnapshot` 只对应一个 `AnalysisInput` revision；并发编辑必须串行化。不得在等待上一次更新响应时合并出第二个未确认的 epoch。
- 更新响应给出新 snapshot id；旧 snapshot 在未 `release` 前仍可查询（引用计数），但 Rust MUST 立即把旧 epoch 标记为非当前，且不再对其发布结果。
- 更新失败：若旧 snapshot 仍可用，保持旧 epoch 并返回失败；若发生协议/进程错误，按 6.6 重启并从 Rust 输入重放。不得假设更新已部分生效。

### 6.3 版本与 epoch [F1-03][F1-26]

一个可发布结果必须同时满足：

```text
native result.engine_session_generation == current.session_generation
native result.snapshot_id               == current.snapshot_id
result.project_revision                 == current.project_revision
result.document_version                 == current.document_version
result.dependency_fingerprint           == current.dependency_fingerprint
```

- epoch 至少由 `(session_generation, snapshot_id)` 组成；项目 cache key 还必须包含 `ProjectRevision`、文档版本、配置/依赖指纹、引擎/资产身份。
- LSP `didChange` 版本必须单调递增；小于等于当前值的编辑拒绝应用（沿用 C0 `ClientVersion` 规则）。
- 结果完成时再次校验 epoch；不匹配则丢弃，返回 `Cancelled`/内部 stale，不发布、不转成空结果。
- 旧 snapshot 在 release 后查询必须报错；Rust 不得缓存旧 snapshot 结果跨 epoch 复用。

### 6.4 一个 LSP 一个子进程、有限会话 [F1-08]

- 常态每个 Rust LSP **最多一个**原生子进程；懒启动，`initialize` 后复用。不得按项目/文件启动子进程。
- 多项目通过有界 session/snapshot 管理：初始预算为最多 **8 个存活 snapshot**、每个项目最多 **1 个 current snapshot**，旧 snapshot 在无 pending 请求后按 LRU/短 grace 释放；具体数值 P2 冻结，超限必须拒绝或回收，不得无界增长。
- 同一连接上的查询可并发，但同一项目的快照更新必须串行；全局最多 8 个在途请求，队列 64（见 3.4）。
- 切换项目/配置变化（tsconfig、依赖、typeRoots、资产版本）视为新快照，不是普通文档更新；旧项目 snapshot 释放。

### 6.5 关闭 [F0-20]

- 关闭项目：等待该 epoch pending 结束（有 deadline）→ 释放 lease → `release` snapshot → 删除 cache。
- 关闭 LSP：停止接受新请求 → 取消排队 → 对运行请求设 deadline → 释放 lease/snapshot（有界）→ close stdin → 等待 → SIGKILL → reap。
- 任何释放失败不得阻塞关闭；记录后强制终止会话，由下次启动重建。

### 6.6 重启与恢复 [F1-09]

- 触发器：stdout EOF、读/写 BrokenPipe、进程非零退出、协议违规、连续超时、显式健康检查失败。
- 恢复步骤：标记会话死亡 → 所有 pending 以 `Unavailable` 失败（回调返回错误）→ close stdin/等待/SIGKILL/reap → 递增 session generation → 启动新进程 → 探针（版本/握手/AST schema）→ 从当前 `AnalysisInput` 重放所有 current 项目 → 新 snapshot id/epoch → 清理旧 handles/leases/caches。
- 不得重放旧 handle/旧 symbol descriptor；不得把旧结果映射到新 AST。
- 重试必须有界：启动最多 3 次（初始 + 2 次），退避 100/500/2000 ms；协议/版本不匹配最多 1 次重启，短时间内重复失败则锁定 `Unavailable`，等待显式请求/项目变化再试。不得热循环。
- `updateSnapshot`、`createSourceFile`、`releaseSourceFile` 等非幂等请求不得自动重试；纯查询最多 1 次透明重试，前提是重启成功、输入/epoch 不变且没有副作用。超时/取消不自动重试非幂等请求。

## 7. 原生句柄、符号描述符与租约

### 7.1 Handle 绑定 [F1-04]

P1-A 已证明危险行为：node handle 格式为 `<encoder index>.<kind>.<path>`，不携带快照/内容身份；跨 `updateSnapshot` 可能静默指向新 AST 的同序号节点，甚至返回另一个符号且无错误。因此：

每个原生 node handle 在 Rust 中 MUST 绑定：

```text
(session_generation, snapshot_id, normalized_file_path, content_hash_or_node_id, node_index, node_kind)
```

- `content_hash`/`node_id` 来自同一快照的 `SourceFileDescriptor` 或 Rust 对源文本的哈希；`node_index` 是 handle 的 encoder index；`node_kind` 是 handle 的 kind 段。
- 解析 handle 必须校验三段格式、十进制 index/kind、路径规范化后属于当前快照；畸形 handle 直接 `Protocol`/`StaleHandle`，不发送。
- 跨 session/snapshot/内容哈希/node id 任一不匹配：**在发送前拒绝**，返回内部 stale 错误；不得让引擎“再试一次”，不得缓存到下一 epoch。
- 每次使用前必须重新取得该快照的 AST（或同 epoch 缓存）；不得把旧 AST 的节点用于新 program。
- 引擎自己也可能拒绝 out-of-range handle；把它映射为 `StaleHandle`/`Protocol`，不得当空结果。

### 7.2 符号描述符 [F1-05]

- `SymbolResponse.reference`/declarations 中的 `SourceFileDescriptor` 含 `fileName`、`path`、`contentHash`、`parseOptionsKey`、`scriptKind`、`nodeId`；必须校验 `contentHash`/`parseOptionsKey` 与当前快照一致。
- 引擎对内容变化后的 symbol reference 会报 `source file is not part of the requested program`；这是可用的第二道校验，但不能替代 Rust epoch 绑定。
- 符号名、文档、tags 必须来自同一快照的 symbol；不得把旧文档拼到新类型上。

### 7.3 Source file lease [F1-06]

- `createSourceFile` 返回带 `source_id`/`source_lease` 的 AST；`retainSourceFile` 返回 lease；`getSourceFile` **不建立租约**。
- 每个非零 lease 必须有 RAII guard/显式释放路径；drop、查询结束、项目关闭、epoch 失效、会话关闭都要释放。
- `releaseSourceFile` 对不存在 lease 返回错误（P1-A 实测）；重复释放只记录一次，不得 panic。
- `getCachedSourceFile` 返回的数据 header 不写回 lease；不得凭数据判断租约是否仍有效。
- 所有 lease 必须有上限和 deadline；泄漏检测测试在会话关闭后断言引擎侧 lease 全部释放或会话已终止。

### 7.4 清理 [F1-07]

- snapshot release 后不得再使用其 AST/handle/lease 查询；测试断言 `snapshot N not found` 被映射为 `StaleSnapshot`，不是空结果。
- 清理顺序：pending 请求失败/取消 → lease release → snapshot release → cache 删除。释放有 deadline（建议 1 s/项），失败则终止会话。
- 公共 DTO、LSP 层、持久缓存不得出现原生 handle/lease/snapshot id 作为稳定标识；它们只能存在于 `wxml-backend-tsgo` 内部并按 epoch 键控。

## 8. UTF、坐标与 URI

### 8.1 单位 [F1-10]

| 边界 | 单位 | 要求 |
| --- | --- | --- |
| LSP `Position.character` | UTF-16 code unit | 现有 `Position`/`LineIndex` |
| 原生 API position（type/symbol/completion/references） | UTF-16 code unit | 不得传 UTF-8 字节 |
| AST `pos`/`end` | UTF-16 code unit | `pos` 是 full start，含 leading trivia；`end` 排他 |
| AST 字符串池偏移 | UTF-8 字节（孤立 surrogate 用 WTF-8） | `uint32` pos/end，相对 string data |
| Rust `SourceMap`/`TextRange` | UTF-8 字节 | 现有 C0 契约 |
| wire string index | `uint32` 字索引，恒为偶数 | ordinal = index / 2 |

- 新增显式 newtype：`Utf8Offset(u32)`、`Utf16Offset(u32)`、`Utf16Position { line, character }`（可复用 `wxml_text::Position`）。原生客户端 API 不得出现裸 `u32` 位置参数；所有转换必须通过持有文本的 `LineIndex`/`SourceSnapshot`。
- `Utf8Offset -> Utf16Offset` 与反向转换必须使用产生范围时的同一文本快照；不得用全局/旧索引。落在 `\r\n`/`\n` 内部、越过文本末尾、落在代理对中间的转换返回 typed error，不 clamp、不 panic。
- 查询位置若落在代理对中间：拒绝请求（`InvalidParams`/`Partial`），不得猜测。

### 8.2 AST 范围与 trivia [F1-11]

- `pos` 包含 leading trivia（P1-A：CJK 行首 `值` 的 node pos=32/end=34，token 实际 33..34）。定义/引用/hover 范围必须取 identifier 子节点，或从 `pos` 向后裁剪空白/注释，得到 token start；不得把 declaration 的 `pos` 直接当 LSP range。
- 范围 `end` 排他；token 文本必须与字符串池内容一致；不一致时返回 `Partial` 并丢弃该位置，不得发布错位范围。
- AST 节点查找 v1 可以扫描节点表（kind=Identifier，`pos <= offset < end`，取最小范围），不要求先实现 children 属性/binder；但必须证明选中的节点文本与源码 token 一致。
- `next`/`parent` 索引、`data` 高 2 位、字符串/扩展偏移必须按固定 encoder 布局校验；未知/越界 → `Protocol`，不得读越界内存或循环遍历。

### 8.3 字符串池/WTF-8 [F1-14]

- 字符串池是 UTF-8/WTF-8 字节；wire index 必须为偶数且 `ordinal < nstrings`；`pos/end` 必须在 string data 范围内且 `pos <= end`。
- 有效 UTF-8 解码为 Rust `String`；含孤立 surrogate 的 WTF-8 **不得**替换为 U+FFFD。必须以原始字节/`Wtf8String` 保留，标记不可编辑；需要 Rust `String` 的展示/编辑路径返回 `Partial`/`Unsupported`。
- 非 BMP 字符（如 😀）在 UTF-16 中占 2 个 code unit；转换必须用 `encode_utf16` 计数，不得按 Unicode scalar 或 UTF-8 字节计数。
- 孤立 surrogate 的 API/AST 端到端行为 P1-A 未验证；F1 必须用原始 `Vec<u16>`/字节构造测试，断言不 panic、不损失、不伪造 range。

### 8.4 CRLF、BOM、hashbang [F1-13]

- 发送给引擎的源文本必须是**原始字节的精确 UTF-8 表示**；不得预先统一换行、去 BOM、去 hashbang 后再映射（否则 range 会偏移）。
- 行索引沿用 C0：`\n` 分行；`\r\n` 中 `\r` 属于行结束、不计入列；单独 `\r` 是普通字符。
- BOM 保留在文本中；BOM 是否被 TS AST 跳过、`pos` 是否从 0 还是 1 开始，必须用 fixture 校准并记录；映射层吸收该偏移，不得假设。
- hashbang 保留；若 AST 从 shebang 之后开始，同样用 fixture 校准；不得把 shebang 当注释删除。
- 组合字符按 code unit 计数，不做 NFC/NFD 归一化。
- 每种组合至少覆盖：token 前/内/后/EOF、CRLF、BOM、非 BMP；先用 AST/pool/Rust range 分层断言，再做端到端，防止两次错误换算相互抵消。

### 8.5 URI 与 FileId [F1-12]

- LSP `file://` URI 与内部路径双向转换必须使用同一规范化函数（5.4）；百分号编码的 UTF-8 文件名必须正确往返。
- `FileId` 是项目根相对、`/` 分隔、大小写敏感、无 `.`/`..`/空组件；项目外路径不得强行塞入 `FileId`。
- 标准库/typings 等资产路径不属于项目 `FileId`；若 C0 `SourceLocation` 无法表示，返回项目内可用位置并标记 `Partial("external declaration outside project")`，或按集成 owner 批准的 external location DTO 扩展；不得伪造项目相对路径。
- 客户端 URI 大小写/编码差异不得导致同一文件产生两个 key；统一规范化后比较。

## 9. C0 DTO 与查询组合

### 9.1 总则

- 查询入口仍使用 C0 `Backend` 边界（`hover`/`diagnostics`/`definition`/`capabilities`）；新增的 completion/references/symbol/internal type 查询先作为 `wxml-backend-tsgo` 内部 DTO，任何 `wxml-types` 公共 DTO 变更由集成 owner 评审后再接 LSP。
- 所有位置从 C0 的 UTF-8 `offset` 经当前 `SourceSnapshot::line_index().position(offset)` 转 UTF-16，再转原生位置；不得直接传字节偏移。
- 原生 `null` 是合法“无符号/无类型”，按查询语义映射，不自动当错误；但原生调用失败、解码失败、超预算不得映射为“空且 Supported”。
- 状态沿用 `CapabilityStatus`：`Supported` 仅当所有必需原生调用成功、所有证据可解码、无截断；`Partial` 表示证据缺失/截断/不可映射；`Unsupported` 表示没有满足合同的路径；`Unavailable` 表示进程/资产不可用；`Disabled` 表示策略关闭（如自动导入/编辑）。
- **QuickInfo、Definition、Rename 没有直接原生 API**：`getQuickInfoAtPosition`、`getDefinitionAtPosition`、`getDefinitionAndBoundSpan`、`getRenameInfo`、`getEditsForRename`、`rename` 在 P1-A 均返回 `unknown API method`。实现只能按本节组合；不得调用这些名字，不得伪造其结果。

### 9.2 查询矩阵

| C0 查询 | 原生方法 | Rust 组合 | 默认状态/拒绝 |
| --- | --- | --- | --- |
| diagnostics | `getSemanticDiagnostics` + `getSyntacticDiagnostics` | 转换 code/category/message/line-character；去重；裁剪预算 | 两者都成功才 `Supported`；部分失败 `Partial`；全失败 `Unsupported` |
| type | `getTypeAtPosition` + `typeToString` | 位置→type；`null` 为无类型；`intrinsicName`/文本判定质量 | 成功 `Supported`；无类型 `Missing` 但不伪造文本 |
| symbol | `getSymbolAtPosition`/`getSymbolOfNode` | 名称、flags、declarations、reference；绑定 epoch | `null` 为“无符号”；解码失败 `Partial` |
| documentation | `getDocumentationComment` + `getJsDocTags` | 文档字符串 + 标签数组 | 调用失败 `Partial`/omit；不伪造文档 |
| definition | `getSymbolAtPosition.declarations` / `getReferencedSymbolsForNode.definition` + `getSourceFile` AST | 句柄→同一快照 AST→token 范围→`SourceLocation` | 无直接 API；别名/越界/解码失败 `Partial`；不得空+`Supported` 掩盖缺证 |
| references | `getReferencedSymbolsForNode` + `getSourceFile` | 去重 alias/target；句柄→范围；shadow 反向映射 | 不完整/截断/无法映射 `Partial`；无证据不得当“零引用” |
| completion | `getCompletionsAtPosition` | 条目 name/kind/sortText；无 textEdit；详情降级 | 详情缺失 `Unsupported`/`Disabled`；自动导入关闭时 `Partial` |
| hover | `getTypeAtPosition` + `typeToString` + `getDocumentationComment` + `getJsDocTags` | 类型代码块 + 文档 + 标签 | 无 QuickInfo；组件缺失 `Partial`；不发明 display parts |
| rename | `getReferencedSymbolsForNode` + AST | 引用/符号证据 + `expected_text` + EditPlan 复检 | 证据不完整即拒绝；绝不部分/不安全编辑 |

### 9.3 Diagnostics [F1-15]

- 请求当前文件的语义 + 语法诊断；P1-A 的 `getBindDiagnostics`、`getSuggestionDiagnostics`、`getProgramDiagnostics` 不作为 C0 必需来源（`getProgramDiagnostics` 实测为 `[]`，不能当“干净”证明）。
- 每条诊断取 `startPosition/endPosition`（0-based line + UTF-16 character）转换为 `SourceRange`；用 `pos/end` 交叉校验，若不一致则丢弃该条并 `Partial`，不得发布可能错位的范围。
- category 映射（以固定发行物 enum 为准）：`Error -> Error`、`Warning -> Warning`、`Suggestion -> Hint`、`Message -> Information`；未知 category 不得默认 Error，标记 `Partial` 并保留原始 code。
- `code` 以十进制字符串保存；message 保留原文，仅做换行/长度预算处理。不得把内部绝对路径插入用户可见文本；若引擎消息含路径，按已知项目根替换为相对路径，不能确定时保留但不主动构造路径。
- 同一文件 semantic/syntactic 完全重复（同 range/code/message）去重；不同来源不得随意覆盖。单文件诊断上限（建议 1000 条）超限截断并 `Partial`，不得静默丢弃。
- 文件集合必须来自请求/当前项目，不得把多个项目的同名文件诊断混在一起；shadow 诊断按同一映射规则回原 WXML/脚本，不能重复发布基线脚本诊断。

### 9.4 Type / Symbol / Documentation [F1-16][F1-17][F1-18]

- `getTypeAtPosition` 返回 `null`：该位置无类型，映射为 `TypeQuality::Missing` + 空类型（不是错误，也不是 `error` 类型）。
- `typeToString` 失败/空：`Unknown`；`intrinsicName == "error"` 或 P1-A 的非法位置形态：`Unknown`，不得当具体类型展示。
- `intrinsicName == "any"` 或文本精确为 `any`：`Any`；LSP 可显示字面 `any`（这是准确文本），但不得把它当已知成员类型。
- 其余成功文本：`Known`；保留重载文本（`{ (...); (...) }`），不要自行拆分或只取一个签名。
- `getSymbolAtPosition` 的 name/flags/declarations/reference 组成内部 `NativeSymbolDto`；`declarations`/`reference` 必须是已绑定 epoch 的 handle；`null` 表示无符号。
- 文档：`getDocumentationComment` 字符串保留换行；`getJsDocTags` 的每个 `{name, text?}` 渲染为 `@name text` 行。`@deprecated` 必须保留并可见。标签/文档为空时省略对应段落，不输出“无文档”。
- 文档调用失败但类型成功：hover `Partial`（或 type-only），不得伪造文档；符号查询仍可用。

### 9.5 Definition（无直接 API） [F1-19]

组合顺序：

1. 对位置取 `getSymbolAtPosition`。`null` → 无符号，返回空 `Supported`（这是“位置没有符号”，不是“查询失败”）。
2. 若 `symbol.reference` 存在，用它作为 `getReferencedSymbolsForNode` 的 node handle；同时传同一 UTF-16 position。无 reference 时，可用同快照 AST 的最小 Identifier 节点；找不到 → 空 `Supported`。
3. `getReferencedSymbolsForNode` 返回多个条目（P1-A 中跨文件 `User` 同时有 target 和 import alias）。收集每个条目的 `definition` handle；对 import alias 场景，保留 target 和 alias 两个候选，由 LSP 决定展示顺序；不得因去重丢掉唯一可导航目标。
4. 对每个 handle：校验 7.1 绑定 → 同一快照 `getSourceFile` → 按 index 取节点 → 取 identifier token 范围（裁剪 trivia）→ 映射为 `SourceLocation`。
5. 去重 `(file, range)`；最多 32 个（沿用现有 `MAX_DEFINITION_LOCATIONS`）；超限截断并 `Partial`。
6. 状态：
   - 所有候选成功映射且无截断 → `Supported`；
   - 有任一候选因路径在项目外、资产不可表示、AST 节点缺失、句柄 stale、范围非法而丢弃 → `Partial`，保留已成功位置；
   - `getSymbolAtPosition` 成功为 null → `Supported` 空；
   - 组合调用本身失败且没有可用候选 → `Unsupported`，不得空+`Supported`；
   - 所有候选都解码失败 → `Partial`/`Unsupported`，不得当作“无定义”。

限制：不得把 declaration 的 `pos` 直接发布；不得在 shadow 文件上返回未映射的范围；生成声明的导航必须走显式关系，不冒充 verbatim copy。

### 9.6 References 与 alias [F1-20]

- 以 `getReferencedSymbolsForNode` 返回的每个 `references[]` 为候选；每个 handle 必须按 7.1 校验并在同一快照 AST 解码范围。
- 去重按 `(file, range)`，同时保留 alias/target 信息；对重命名，alias/target 必须解析为同一目标，否则拒绝（9.8）。
- 引用必须映射回原始源文件：脚本直接映射；shadow 位置经 `DerivedView`/`SourceMap` 反算到 WXML 表达式/脚本；映射不完整、落在 synthetic 文本、无法区分生成来源 → `Partial` 并从编辑证据中排除。
- 不得把字符串/注释中的文本当引用（AST 已排除）；不得用文本搜索补足。
- 引用上限（建议 1000）超限截断并 `Partial`；截断结果不得用于 rename。
- 空引用 + 所有调用成功且 symbol 非 null：可以表示“只有一个声明/无其它引用”，但状态是否 `Supported` 取决于是否证明完整性；在完整性未冻结前默认 `Partial`，不得把“只返回声明”当零引用。

### 9.7 Completion 与详情/自动导入限制 [F1-21]

- v1 使用 `getCompletionsAtPosition` 且 `includeSymbol:false`；条目只取 `name`、`kind`、`sortText`。不得自行执行代码/解析项目文件补全。
- 原生条目没有 textEdit；替换范围由 Rust 从当前 token 前缀计算。前缀无法确定、跨越语法边界、位置在 `.`/EOF 边界：返回 `Partial` 且不提供编辑；不得用全文替换或猜测。
- 条目 kind 映射必须使用固定发行物 enum；P1-A 只证明 `kind=5` 的成员条目。未知 kind 映射为 plain text 并标记 `Partial`（或内部保留原始 kind），不得按猜测分类。
- 详情：`getCompletionEntryDetails` **不存在**。v1 的 `details` 必须显式 `Unsupported`/`Disabled`（可见降级），不得用 `getTypeOfSymbol`/docs 拼出“详情”冒充原生详情。
- 自动导入：`includeSymbol:true` 需要快照 `prepareAutoImports`（P1-A 条件通过，精确参数未冻结）。v1 默认关闭；关闭时缺失自动导入条目，必须把结果标记为 `isIncomplete=true`/`Partial`，并在能力/日志中说明，不得静默假装完整。
- 位置在 `.` 上、EOF、非法边界可能返回全局补全或 `null`（P1-A）。`null` → 空 `Unsupported`/`Partial`，不得当“没有补全”发布；`.` 后若无法确认成员上下文，不得把全局条目当成员补全。
- 最多 `MAX_COMPLETION_ITEMS`（256）条；超限截断并 `Partial`；保留原生 `sortText`/顺序，不额外排序掩盖缺失。

### 9.8 Hover 内容模型 [F1-22]

Hover 由三部分组成，顺序固定：

1. 类型代码块：仅当 `typeToString` 成功且质量允许时输出；重载保留原生合并文本，不拆成多个“当前重载”。
2. 文档：`getDocumentationComment` 原文，保留换行；多个 overload 的文档可能已合并为换行字符串（P1-A），不得伪造分段。
3. JSDoc 标签：`@name text` 行；`@deprecated` 必须保留。

约束：

- 无 LSP `MarkupContent`/display parts；由 Rust 生成 Markdown 字符串，`markup=Markdown`。不得输出 HTML、伪造链接、参数高亮或未知结构。
- 范围：优先使用光标处 identifier token 的范围；不得用声明范围替代（否则 hover 高亮错误位置）。无可靠范围 → `range=None`。
- 质量：`Known` 才显示完整类型块；`Any`/`Unknown`/`Missing` 沿用 C0 规则，不得把 `error`/未知类型包装成具体类型。显式 `any` 可以显示字面 `any`，但 quality 仍为 `Any`。
- 文档可选：类型成功、文档缺失 → type-only hover，`Supported`；类型失败但文档可用 → `Partial`；两者都不可用 → `Unsupported`，不得返回空字符串 + `Supported`。
- 旧 QuickInfo 的 display parts、参数名/类型分段、每 overload 独立文档在原生 API 中不存在；F1 必须把旧断言映射到本模型，无法等价时标记为阻塞并修订 ADR/合同，不能悄悄降级。

状态合成（`HoverEvidence`）：

- `HoverEvidence` 的 `type_info`/`type_quality`、`documentation`、`symbol`、`declarations` 与 `gaps` 共同决定 `status`：没有可渲染的类型或文档 → `Unsupported`；声明目标落在项目外、声明句柄不可解码、声明列表/扫描被截断，或 `Missing` 类型仍有符号/声明证据（`Blocked`）→ 至少 `Partial`，且 `reason` 保留后端原文，不得省略或改写。
- 空 Markdown（`compose_hover_markdown` 返回空串）与 `Supported` 同时出现是契约违反：LSP adapter 必须发布 `Unsupported` 并保留可读原因，绝不能发布空内容 + `Supported` 或空内容 + `Partial`。
- `Unsupported` + 非空 Markdown 同样是契约违反：adapter 必须降级为 `Partial` 并明确说明矛盾，不得假 `Supported`，也不得丢弃已渲染内容。
- host degradation（shadow/源文件列表不完整等）与 hover range 映射失败都是追加原因：`Supported` → `Partial`；已有 `Partial`/`Unsupported` 保留原 `reason` 并追加（去重），不得覆盖后端原因；`Disabled`/`Unavailable`/`Unknown` 不因追加原因被提升。

### 9.9 Rename：保守编辑规则 [F1-23][F1-27]

`getRenameInfo`/`getEditsForRename`/`rename` 不存在；Rust 只能基于符号 + 引用 + AST 证据构造编辑。**只有同时满足以下全部条件才允许生成编辑：**

1. 光标位于原始源文件的 identifier token（不是 shadow/synthetic/generated 文本）；symbol 非 null，类型不是未知/`any`，不是外部资产符号。
2. `getReferencedSymbolsForNode` 成功，返回非空且未截断；所有 definition/reference handle 在同一 snapshot 内可解码。
3. 目标符号唯一：alias 与 target 解析到同一目标；出现多个目标、合并声明无法区分、或 alias 目标不唯一 → 拒绝。
4. 每个引用 token 的文本等于当前符号名/`expected_text`；`expected_text` 缺失或不匹配 → 拒绝。shorthand property、computed property、字符串/注释、导入 specifier 等必须逐项确认，不能只比较全文。
5. 所有引用都在当前 snapshot 的项目文件内，且能映射回原始 WXML/脚本；外部资产、生成声明、synthetic 文本、无法反算的 shadow 位置 → 拒绝。
6. 编辑范围通过当前文档版本/UTF 转换校验，互不重叠；任一范围落在代理对中间、CRLF/BOM 边界无法确定、或版本已变 → 拒绝。
7. 生成编辑后，在隔离快照上重新 parse/typecheck；出现新错误、目标数变化、或无法复检 → 拒绝整个编辑，不部分应用。
8. 最终仍由 LSP `EditPlan` 校验路径、版本、范围、重叠、语法和 UTF 转换；不得原样透传原生 rename edits（原生根本没有）。

不满足任一条：返回 `Unsupported`/`Partial` + **零编辑**，记录原因；不得降级为文本查找替换、不得只改部分引用、不得伪造 `WorkspaceEdit`。WXML 符号/`setData` key 的 rename 不在本原生合同内，沿用现有 Rust 符号层规则并另行测试。

### 9.10 拒绝/降级速查

| 情况 | 结果 |
| --- | --- |
| 原生无该方法/未在白名单 | `Unsupported`，不发送 |
| 原生返回 `unknown API method` | 探针：启动失败；运行：`Unsupported`/`Protocol`，不回退 |
| 句柄跨 epoch/stale | 内部 `StaleHandle`，不发布、不重试；请求可为 `Cancelled`/`Unsupported` |
| 某候选定义无法映射，但其它可映射 | `Partial` + 已映射位置 |
| 所有定义候选无法映射 | `Partial`/`Unsupported`，不是空+`Supported` |
| 补全详情缺失/自动导入关闭 | 条目可返回，`details`/自动导入显式降级，`isIncomplete`/`Partial` |
| hover 类型缺失但文档可用 | `Partial`，不伪造类型 |
| rename 任一安全条件不满足 | 零编辑 + `Unsupported`/`Partial` |

## 10. 错误、取消、超时与恢复

### 10.1 错误类别与映射

内部 `NativeError` 至少包含：

| 类别 | 示例 | 边界映射 |
| --- | --- | --- |
| `ArtifactMismatch` | 版本/摘要/gitHead 不符 | `BackendError::Unavailable` |
| `Spawn` | exec 失败、pipe 失败 | `Unavailable` |
| `Handshake` | initialize 形状不符、探针失败 | `Unavailable` |
| `Protocol` | 帧/JSON/ID/AST schema 违规 | `Protocol`（用户不可见细节） |
| `UnknownMethod`/`DeniedMethod` | 引擎不识别/合同拒绝 | `Unsupported` 或 `Protocol`；探针失败为 `Unavailable` |
| `InvalidParams` | 位置越界/URI 非法/参数形状错 | `Protocol` 或 `Unsupported`，不重试 |
| `StaleHandle`/`StaleSnapshot` | 跨 epoch/已 release | 内部 stale；请求层 `Cancelled`/`Unsupported`，不发布旧结果 |
| `EngineError { code }` | `-32603` 等 | `Internal`，消息清洗 |
| `EnginePanic` | Go recover 栈 | `Internal("engine internal error")`，栈只进脱敏日志 |
| `Timeout` | 请求/更新/回调超时 | `Timeout` |
| `Cancelled` | 排队跳过/运行丢弃 | `Cancelled` |
| `EpochInvalidated` | 结果到达时 epoch 已变 | 丢弃，不发布；调用方看到 `Cancelled` |
| `BudgetExceeded` | 帧/JSON/AST/诊断/引用超限 | 查询：`Partial`+截断；帧：`Protocol`+重启 |
| `Unavailable` | 进程死亡/锁定失败/资产缺失 | `Unavailable` |

`BackendError::Protocol`/`Internal` 的文本只用于日志和诊断，不得直接展示给最终用户；不得包含 Go 栈、绝对路径、环境变量或秘密。

### 10.2 取消 [F1-24]

- **排队中**：从队列移除，返回 `Cancelled`，**不发送**到引擎。
- **运行中**：标记 cancel token；响应到达后丢弃，返回 `Cancelled`，不发布任何部分结果。P1-A 未验证原生即时取消能力，因此不得声称有“服务端取消”；不得为一个请求发送未冻结的 cancel 方法。
- 取消发生在快照更新期间：更新本身不可取消；完成后 epoch 提升，旧请求结果丢弃；更新受 6.2 deadline 约束。
- 取消后仍可能收到 FS 回调：必须继续应答（从快照读），否则会阻塞引擎；回调应答不代表请求结果可发布。
- 运行中取消不会 kill 引擎；若响应永久不返回，由请求 deadline/健康检查处理。不得因单次取消重启进程。

### 10.3 超时与进程死亡 [F1-25]

- deadline 从请求入队或发送时开始（P2 冻结；默认从发送时算），默认 15 s；调用方更短的 deadline 优先。快照更新 30 s，握手 10 s，回调 2 s，lease/snapshot release 1 s，shutdown 5 s。
- 超时后：移除 pending、标记 ID 放弃、返回 `Timeout`；迟到响应丢弃。若同一会话连续超时/存在未完成 pending，判定不健康并按 6.6 重启。
- 进程死亡（EOF/BrokenPipe/exit）：所有 pending `Unavailable`；回调返回错误；epoch 递增；按 6.6 重启/重放。不得把死亡映射为“空诊断/无定义”。
- 重试：启动最多 3 次，退避 100/500/2000 ms；协议/版本不匹配最多 1 次重启；重复失败锁定 `Unavailable`，不得忙循环。
- 纯查询可在重启成功、输入/epoch 不变时透明重试 **1 次**；`updateSnapshot`、`createSourceFile`、lease/snapshot release 等非幂等操作不得自动重试。`DeniedMethod`/`UnknownMethod`/`InvalidParams`/`StaleHandle`/`EpochInvalidated` 一律不重试。

### 10.4 旧响应屏障 [F1-03]

- 每个请求携带入队时的 epoch；响应完成时比较当前 epoch。不匹配：丢弃并返回 `Cancelled`/内部 stale，不发布、不合并、不转空。
- LSP 发布层仍执行 C0 第 11 节校验（文件/SourceRevision/SnapshotId/ProjectRevision/ViewRevision/BackendEpoch）；双层校验缺一不可。
- `release` 后到达的响应、旧 session 的响应、跨项目响应均丢弃；不得写入当前缓存。
- 取消/超时/重启后不得把旧 diagnostics/WorkspaceEdit 写入新文本；测试必须注入“v1 查询→v2 更新→v1 迟到响应”。

## 11. 安全验收

以下是不变量，P2 安全评审必须逐项签字；任何一项不满足都阻塞迁移：

| 编号 | 不变量 | 验收 |
| --- | --- | --- |
| S1 | 只发送 4.1 白名单方法 | 类型化枚举 + 运行时断言 + fake peer 记录 method |
| S2 | 无 OS 写：emit/build/clean/profile/heap 不可达；writeFile/removeFile noop；append/chtimes 无回调且必须不可达 | 沙箱树前后哈希、canary、拒绝方法测试；append/chtimes 启动参数必须被拒绝 |
| S3 | 读路径可审计：所有可达回调注册；缺失回调不回落 OS；生产禁用 `useOS`；不存在必须用各回调的合法否定形状 | 回调日志覆盖、沙箱外 canary 不可见、symlink 不可逃逸、`fileExists`/`directoryExists`/`getAccessibleEntries`/`realpath` 的 `missing` 负例 |
| S4 | 不启用外部代码 | argv 无 `--runExternalCode`；tsconfig `plugins` canary 不执行；无 Node 子进程 |
| S5 | `noEmit`/`writeFile:noop` **不是** OS 沙箱 | 文档/测试显式断言；即使 noEmit 也不能暴露 emit 方法 |
| S6 | 路径限制：绝对、规范化、无 `..`/NUL、按组件前缀、资产根只读 | 路径表驱动负例 + 恶意 URI |
| S7 | 资源有界：帧/深度/AST/回调/队列/诊断/引用 | 超限 typed error、无 panic/无 OOM、会话可恢复 |
| S8 | 错误/日志不泄漏路径、栈、秘密 | panic/stderr 清洗测试；DTO 无绝对路径 |
| S9 | 不在探针前发送项目内容 | 启动顺序测试；失败不读项目 |
| S10 | 安全失败是显式失败，不是空成功/静默降级 | 所有拒绝路径返回 typed 状态/错误 |

读路径审计至少覆盖：项目 `tsconfig`/`extends`/`paths`/`typeRoots`、`package.json`、模块解析、`node_modules`（仅快照）、默认库、typings、`realpath`、目录枚举、符号链接、`..`/编码路径、canary 文件。写路径审计至少覆盖：`emit`/`emitToString`/`getJavaScriptEmit`/`getDeclarationEmit`、build/clean、profile/heap、writeFile/removeFile/appendFile/chtimes。P1-A 已证明 `emit` 默认可写到项目 cwd 之外，`writeFile:noop` 能阻断；这只是缓解，不是权限模型。2026-11-17 追加探针进一步确认：完整回调串 + 宿主 FS 下 `useOS` 生效、`emit` 不落盘、虚拟快照可组合；但 `appendFile`/`chtimes` 无法用回调 deny（启动 panic），因此 S2 的写阻断必须额外依赖方法白名单和运行目录最小权限，残余风险记为 B-02 PARTIAL。

## 12. 合同 ID 与测试挂点

以下测试文件和测试名均为**未来计划**；新 crate 尚未创建，所有状态为 `planned（未实现）`。实际 crate 路径/文件名在 P3 冻结时只能细化，不能删除 ID 或降低断言。

### 12.1 F0

| ID | 合同点 | 未来 Rust 测试文件 | 未来测试名 | 状态 |
| --- | --- | --- | --- | --- |
| F0-01 | 固定制品身份/SRI/sha256 | `crates/wxml-backend-tsgo/tests/f0_artifact.rs` | `accepts_pinned_linux_x64_artifact_identity` | planned |
| F0-02 | 排除 7.0.2/浮动版本 | `crates/wxml-backend-tsgo/tests/f0_artifact.rs` | `rejects_7_0_2_and_unpinned_versions` | planned |
| F0-03 | 版本/制品预检 | `crates/wxml-backend-tsgo/tests/f0_artifact.rs` | `version_probe_requires_exact_pinned_version` | planned |
| F0-04 | 握手/schema/方法探针 | `crates/wxml-backend-tsgo/tests/f0_protocol.rs` | `handshake_probe_requires_7_1_shape_and_methods` | planned |
| F0-05 | AST v9/header 64 校验 | `crates/wxml-backend-tsgo/src/native/ast.rs` | `tests::rejects_non_v9_or_wrong_header_size` | planned |
| F0-06 | 启动参数/环境不变量 | `crates/wxml-backend-tsgo/src/native/supervisor.rs` | `tests::builds_only_frozen_startup_args` | planned |
| F0-07 | Content-Length 分帧/坏帧 | `crates/wxml-backend-tsgo/src/native/framing.rs` | `tests::rejects_bad_content_length_and_short_body` | planned |
| F0-08 | 帧/JSON/AST 预算 | `crates/wxml-backend-tsgo/src/native/framing.rs` | `tests::enforces_frame_depth_and_size_budgets` | planned |
| F0-09 | 请求 ID/响应路由 | `crates/wxml-backend-tsgo/src/native/rpc.rs` | `tests::routes_server_callbacks_and_client_ids_separately` | planned |
| F0-10 | 回调并发/未知回调 | `crates/wxml-backend-tsgo/src/native/rpc.rs` | `tests::services_callbacks_while_request_is_pending` | planned |
| F0-11 | stderr/panic 清洗 | `crates/wxml-backend-tsgo/src/native/supervisor.rs` | `tests::drains_stderr_and_sanitizes_panic_stack` | planned |
| F0-12 | 方法白名单 | `crates/wxml-backend-tsgo/tests/f0_protocol.rs` | `allowlist_contains_only_reviewed_methods` | planned |
| F0-13 | 拒绝清单/写入口 | `crates/wxml-backend-tsgo/tests/f0_security.rs` | `denied_methods_never_reach_engine` | planned |
| F0-14 | 未知/拒绝方法 fail-closed | `crates/wxml-backend-tsgo/tests/f0_protocol.rs` | `unknown_method_fails_closed_without_fallback` | planned |
| F0-15 | 全虚拟快照/读回调注册 | `crates/wxml-backend-tsgo/src/native/vfs.rs` | `tests::serves_only_full_snapshot_and_registered_callbacks` | planned |
| F0-16 | 禁止 OS 读回退 | `crates/wxml-backend-tsgo/tests/f0_security.rs` | `unregistered_read_cannot_fall_back_to_os` | planned |
| F0-17 | write/remove noop；append/chtimes 无回调且必须不可达 | `crates/wxml-backend-tsgo/tests/f0_security.rs` | `write_callbacks_are_noops_and_append_chtimes_are_not_registrable` | planned |
| F0-18 | 路径规范化/限制 | `crates/wxml-backend-tsgo/src/native/vfs.rs` | `tests::normalizes_and_confines_paths` | planned |
| F0-19 | 标准库/类型资产读边界 | `crates/wxml-backend-tsgo/tests/f0_security.rs` | `stdlib_and_typings_reads_stay_in_asset_roots` | planned |
| F0-20 | 关闭/杀进程/回收 | `crates/wxml-backend-tsgo/src/native/supervisor.rs` | `tests::shutdown_closes_stdin_then_kills_and_reaps` | planned |

### 12.2 F1

| ID | 合同点 | 未来 Rust 测试文件 | 未来测试名 | 状态 |
| --- | --- | --- | --- | --- |
| F1-01 | 虚拟项目创建/身份 | `crates/wxml-backend-tsgo/tests/f1_session.rs` | `creates_virtual_projects_without_disk_files` | planned |
| F1-02 | closeFiles+openFiles 更新 | `crates/wxml-backend-tsgo/tests/f1_session.rs` | `unsaved_update_requires_close_and_open_in_same_snapshot` | planned |
| F1-03 | epoch/stale 结果屏障 | `crates/wxml-backend-tsgo/tests/f1_session.rs` | `drops_results_from_invalidated_epochs` | planned |
| F1-04 | node handle 绑定/跨 epoch 拒绝 | `crates/wxml-backend-tsgo/tests/f1_session.rs` | `rejects_node_handles_from_previous_snapshot` | planned |
| F1-05 | symbol descriptor 校验 | `crates/wxml-backend-tsgo/tests/f1_session.rs` | `rejects_symbol_descriptor_with_stale_content_hash` | planned |
| F1-06 | lease 生命周期 | `crates/wxml-backend-tsgo/tests/f1_session.rs` | `leases_are_released_on_drop_and_session_close` | planned |
| F1-07 | snapshot release/use-after-release | `crates/wxml-backend-tsgo/tests/f1_session.rs` | `released_snapshot_cannot_be_queried` | planned |
| F1-08 | 单子进程/有界会话 | `crates/wxml-backend-tsgo/tests/f1_session.rs` | `keeps_one_child_and_bounded_live_snapshots` | planned |
| F1-09 | 重启/重放/有界重试 | `crates/wxml-backend-tsgo/tests/f1_session.rs` | `restarts_with_bounded_retries_and_replays_inputs` | planned |
| F1-10 | UTF-8/UTF-16 newtype | `crates/wxml-backend-tsgo/tests/f1_utf.rs` | `distinguishes_utf8_and_utf16_offsets` | planned |
| F1-11 | AST trivia/范围 | `crates/wxml-backend-tsgo/tests/f1_utf.rs` | `trims_ast_leading_trivia_for_ranges` | planned |
| F1-12 | URI/FileId 规范化 | `crates/wxml-backend-tsgo/tests/f1_utf.rs` | `normalizes_file_uris_and_project_file_ids` | planned |
| F1-13 | CRLF/BOM/hashbang/非 BMP | `crates/wxml-backend-tsgo/tests/f1_utf.rs` | `preserves_crlf_bom_and_non_bmp_offsets` | planned |
| F1-14 | 孤立 surrogate/WTF-8 | `crates/wxml-backend-tsgo/tests/f1_utf.rs` | `preserves_lone_surrogates_without_replacement` | planned |
| F1-15 | diagnostics DTO | `crates/wxml-backend-tsgo/tests/f1_query.rs` | `diagnostics_maps_category_and_range` | planned |
| F1-16 | type DTO/quality | `crates/wxml-backend-tsgo/tests/f1_query.rs` | `type_query_distinguishes_any_error_and_known` | planned |
| F1-17 | symbol DTO/declarations | `crates/wxml-backend-tsgo/tests/f1_query.rs` | `symbol_query_returns_declarations_and_docs` | planned |
| F1-18 | 文档/JSDoc | `crates/wxml-backend-tsgo/tests/f1_query.rs` | `documentation_and_jsdoc_tags_are_composed` | planned |
| F1-19 | definition 组合 | `crates/wxml-backend-tsgo/tests/f1_query.rs` | `definition_composes_declarations_and_references` | planned |
| F1-20 | references/alias | `crates/wxml-backend-tsgo/tests/f1_query.rs` | `references_deduplicates_alias_and_target` | planned |
| F1-21 | completion/详情/自动导入 | `crates/wxml-backend-tsgo/tests/f1_query.rs` | `completion_without_details_is_visibly_degraded` | planned |
| F1-22 | hover 组合/重载 | `crates/wxml-backend-tsgo/tests/f1_query.rs` | `hover_composes_type_docs_and_overloads` | planned |
| F1-23 | rename 保守拒绝 | `crates/wxml-backend-tsgo/tests/f1_query.rs` | `rename_refuses_when_evidence_or_expected_text_mismatches` | planned |
| F1-24 | 取消语义 | `crates/wxml-backend-tsgo/tests/f1_session.rs` | `queued_cancel_skips_send_and_running_cancel_discards` | planned |
| F1-25 | deadline/进程死亡/错误分类 | `crates/wxml-backend-tsgo/tests/f1_session.rs` | `deadline_process_death_and_error_categories` | planned |
| F1-26 | cache invalidation key | `crates/wxml-backend-tsgo/tests/f1_session.rs` | `cache_keys_include_epoch_snapshot_and_dependency_fingerprint` | planned |
| F1-27 | rename 编辑复检 | `crates/wxml-backend-tsgo/tests/f1_query.rs` | `rename_edits_revalidate_before_publication` | planned |

LSP 端到端挂点（仍为计划）：`crates/wxml-lsp/tests/native_parity.rs` 覆盖 hover/diagnostics/definition/completion/references/rename 的原文件范围、版本屏障与安全拒绝；它不能替代上述真实原生合同测试，fake peer 也不能替代真实引擎门禁。

2026-11-17 的 P2 追加探针只关闭了 B-02/B-03/B-04/B-12 的**未知性**（并记录了 B-02 的负结果），**没有**把这些 ID 变成已实现测试；上述测试文件和测试名仍全部是 `planned`，不得因探针通过而删除或降级。

## 13. 开放问题与阻塞项

以下列出仍开放或已由 2026-11-17 P2 追加探针处理的事项；状态列以追加证据（证据 §15）为准。PASS 只表示该探针范围内的未知项已关闭，不表示合同/测试已验收。

| 编号 | 问题 | 影响 | 阻塞级别 |
| --- | --- | --- | --- |
| B-01 | 维护者已条件接受 `7.1.0-beta`，`gitHead`/SRI/sha256 已固定；不切换默认后端，契约仍为 draft，待 P2 评审/G1 | F0 生效条件 | **条件接受（CONDITIONAL）**；契约仍待 P2/G1 |
| B-02 | 已实测：`appendFile`/`chtimes` 不是合法回调名；`name`、`name:error`、`name:noop` 均启动 panic；源码注释明确“always delegates to base (no callback support)”，是当前 API 的有意能力边界而非临时缺陷。无法回调 deny，只能依赖方法白名单不可达 + 最小权限 | 回调级写阻断不可实现；残余纵深风险 | **PARTIAL（负结果）**；维护者 2026-11-18 接受补偿控制，P3 可启动；不得声称 write-callback deny 已实现 |
| B-03 | 已捕获 readFile/fileExists/directoryExists/getAccessibleEntries/realpath/writeFile/removeFile 的真实帧；`missing` 只对 readFile 合法；读回调 error/invalid kind 会 panic；writeFile error -> 5033/不写，removeFile error -> status 0/`filesDeleted` 仍列路径 | 回调可按精确形状实现，错误不可当 deny | **PASS**（形状/失败语义已冻结；测试仍 planned） |
| B-04 | 已实测宿主 FS + 完整回调串：回调确实被调用，`useOS` 生效，readFile `missing` 生效，writeFile/removeFile noop 下 emit 不落盘，虚拟快照可组合；非 read 回调的 `missing` 是非法形状 | F0 写阻断/组合验收 | **PASS**（范围内；非 read `missing` 属 B-03 负例） |
| B-05 | 完整 Page/Component 原位 shadow、Behavior/props/loops/guards 与原生 checker 未验证 | F1/P4 语义验收 | **F1/P4 阻塞** |
| B-06 | hover 无 display parts/每 overload 文档；旧 QuickInfo 断言如何映射未决 | F1 hover 保真 | **F1 阻塞**（需产品/测试决定） |
| B-07 | 补全详情/`prepareAutoImports`/`includeSymbol` 精确参数与可见降级未冻结 | F1 completion 验收 | **F1 阻塞**（若要求详情） |
| B-08 | 标准库/typings 等外部 `.d.ts` 目标无法用现有 `SourceLocation(FileId)` 表示；旧 definitions 断言含 `.d.ts` | F1 definition 等价 | **F1 阻塞**（DTO 决策） |
| B-09 | rename 完整性无原生 flag；旧 rename 行为/断言与保守拒绝的等价性未验证 | F1 rename 验收 | **F1 阻塞** |
| B-10 | 完整 AST msgpack/binder/children 未实现；C0 definition 可用节点表扫描，但其它功能可能依赖 | 后续语义/AST 功能 | 非 C0 阻塞；P3/N 交付 |
| B-11 | CRLF/BOM/hashbang/孤立 surrogate 的精确 AST 行为未校准 | F1 UTF 验收 | **F1 阻塞**（fixture） |
| B-12 | 已实测 `tsconfig`、`extends`（项目外）、`package.json`、源码/依赖/项目外 import、`typeRoots`（显式 `types`）的回调读取路径；未注册回调会读宿主 FS；合法缺失形状可阻止 OS 回退；符号链接/自动 typeRoots 仍有残余 | F0/F1 读安全与项目路由 | **PASS（已测配置）**；符号链接/自动发现另测 |
| B-13 | `updateSnapshot` 增量/project 列表/diff 语义、`openFiles` 重复 fileNames 的完整规则未验证 | F1 更新正确性 | F1 需测试；v1 用全量 VFS 规避 |
| B-14 | Windows/macOS/ARM64 资产、ABI、大小写、路径语义未验证 | 平台支持范围 | 非 Linux 平台阻塞 |
| B-15 | 项目级性能/RSS、长稳、并发预算未测 | P5 门禁 | P5 阻塞，非 F0 |
| B-16 | `batchRequests` 分页/嵌套方法校验未验证 | 优化项 | v1 已拒绝，非阻塞 |
| B-17 | SignatureHelp 无专用 API，错误 node kind 会 panic | 功能范围 | C0 明确排除；未来单独合同 |
| B-18 | LSP-connected 共享快照路径未验证 | ADR 替代架构 | 当前不采用，非阻塞 |
| B-19 | `lib/tsc.sig` PGP 验签未执行 | 供应链声明 | 不得声称已验签；非 F0 阻塞 |
| B-20 | 宿主机 `--cwd`/资产根在真实项目中的布局与权限未做端到端安装验证 | F3/安装 | F3 阻塞 |

**最低关闭顺序建议：** B-01 已条件接受，B-02 补偿控制已于 2026-11-18 接受，B-03/B-04/B-12 的探针未知项已关闭；B-02 仍为 PARTIAL（回调 deny 不可实现，依赖方法白名单 + 最小权限）。F1 仍须先关闭 B-05/B-06/B-07/B-08/B-09/B-11，B-13 按测试处理；B-10/B-14–B-20 按阶段处理。任何一项若无法关闭，必须修订 ADR/计划并显式记录，不得用“已降级”或“暂不测试”代替。

**P3 启动判断：** B-01 已条件接受，B-02 补偿控制已于 2026-11-18 接受，B-03/B-04/B-12 的探针未知项已关闭；自 2026-11-18 起可以启动 P3 的 native client/FS/AST 骨架（N）。但不得把当前文本称为“已冻结的 F0/F1”：B-02 仍为 PARTIAL，F1 的 B-05/B-06/B-07/B-08/B-09/B-11/B-13 仍未关闭，查询与语义切片继续阻塞；生产安装器属于 P3-D/F3。
