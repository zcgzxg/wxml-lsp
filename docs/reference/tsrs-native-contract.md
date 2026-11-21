# 原生类型后端合同（tsrs 进程内会话）

**状态：accepted（2026-11-21，进程内会话口径）。** 本文替代 2026-11-20 冻结的 tsgo 子进程口径：F0/F1 合同项中仍然有效的是只读虚拟文件系统、路径限制、handle/lease、UTF-16 位置与 AST v9/header 64/node stride 28；引擎可执行文件、tarball 校验、`--version` 预检、`--api` argv、Content-Length 帧、JSON-RPC 回调、stdout/stderr 泵和进程重启条款**已废弃**，不再表示当前实现。

更新日期：2026-11-21（原 2026-11-20）。适用基线：[ADR-0012](../adr/0012-in-process-tsrs-session.md)（native 默认、Node 已移除、进程内 tsrs 会话）、[ADR-0009](../adr/0009-project-slimming.md)。实现与任务拆分见 `openspec/changes/archive/2026-11-21-replace-tsgo-with-tsrs-session/`；2026-11-17 的 tsgo 实测证据只作为协议/AST/FS 语义的历史来源，明细见 tag `pre-slim-20261120`。

## 1. 状态、权威顺序与证据边界

### 1.1 本文件的性质

- 本文定义 **F0 原生兼容合同**与 **F1 Backend 语义合同**在进程内会话下的现行口径，供实现、评审与测试对齐。
- 规范词：**MUST / MUST NOT** 为强制项；**SHOULD** 为需要说明理由的默认项；**MAY** 为可选。中文“必须/不得”等同 MUST/MUST NOT。
- 本文中的 `F0-xx`、`F1-xx` 是合同项 ID；第 12 节给出这些 ID 的当前覆盖位置。
- 历史条款（子进程、帧、回调、重启）保留在 §3 的废弃表中，只用于解释演进，不作为实现要求。
- 任何 F0/F1 项是否满足仍必须以真实代码、测试和门禁证据为准，不能仅凭条目存在推断。

### 1.2 权威顺序

冲突时按以下顺序判断：

1. 当前代码与默认运行的测试。
2. 有效的 ADR（ADR-0012 与 ADR-0009）。
3. 本文件与既有 C0 契约（`docs/reference/contracts.md`）。
4. 项目 README 与 `docs/reference/` 其它参考。
5. 来源与许可证记录（`docs/reference/provenance.md`），以及历史证据。

代码事实优先于文档；发现不一致时修订对应文档，不保留两套说法。本文件与 ADR 冲突时，不得用本文件覆盖 ADR。

### 1.3 证据边界

**仍然有效的实测结论（2026-11-17，Linux x86-64、`7.1.0-beta`、无 Node）：**

| 项 | 证据范围 |
| --- | --- |
| 方法语义 | `getTypeAtPosition`、`getSymbolAtPosition`、`getDocumentationComment`、`getJsDocTags`、`getReferencedSymbolsForNode`、`getCompletionsAtPosition`、诊断方法、`typeToString` 的返回形状 |
| `initialize` 形状 | 只返回大小写敏感信息与 `currentDirectory`，没有协议版本协商 |
| AST schema | protocol v9、header 64、字符串池 UTF-8、节点表 28 字节、identifier/string/numeric/SourceFile 扩展数据 |
| 坐标 | API position 与 AST pos/end 为 UTF-16；字符串池为 UTF-8/WTF-8 字节 |
| 句柄 | symbol descriptor 有内容校验；node handle 跨快照可能静默指向新 AST 同序号节点 |
| 快照与更新 | 全虚拟快照可完成类型查询；未保存内容必须在同一次更新中 `closeFiles` + `openFiles` |
| 虚拟 FS 语义 | 引擎会在项目根之外探测配置、`package.json`、`typeRoots`、目录和 `realpath`；读入口必须给出各自的合法否定结果 |
| 排除 7.0.2 | 无 `createSnapshot`/租约/`createSourceFile`；AST v5/header 44，与 v9 不兼容 |

**进程内路径专用、必须由当前测试证明（不得引用旧子进程证据替代）：**

- `tsrs_api::Session` 的构造、`handle_request`、`close` 生命周期与幂等性。
- 内嵌标准库加载、`bundled::wrap_fs` 与 `default_library_path` 的行为。
- `VfsHandle` 的 `stat`/目录枚举/`realpath` 与写拒绝。
- 版本/AST 探针失败时的 `Unavailable`、会话中毒时的 `SessionPoisoned`。
- 长会话 RSS/线程数，以及 `createSnapshot` 在调用线程上的栈深度。

**未实测，本文只提出要求、不得当成事实：**

- 完整 Page/Component 原位 shadow、Behavior/props/loops/guards 与进程内 checker 的组合。
- 完整 AST 解码：structured data、binder data、children 属性位掩码、完整树重建、WTF-8 孤立 surrogate 保真、畸形/越界/循环输入的性质测试。
- 保守 rename 的完整条件、`expected_text` 匹配与编辑后复检。
- hover 对旧 QuickInfo 的 display parts / 分段等价性。
- 补全详情、自动导入的完整语义。
- CRLF、BOM、hashbang、组合字符、孤立 surrogate 的端到端坐标。
- 符号链接/宿主 `realpath` 逃逸的端到端负例。
- Windows/macOS/ARM64 与多项目并发、长稳表现。
- `batchRequests` 分页、`getResolvedSignature` 的 panic 是否在上游修复。

**结论：本文的“已实测”只覆盖上表；其余都是待测试证明的合同要求。**

## 2. 引擎固定、版本与启动基线

### 2.1 依赖与版本固定 [F0-01]

| 字段 | 固定值 |
| --- | --- |
| crate | `tsrs_api`、`tsrs_core`、`tsrs_vfs` |
| 精确 pin | `=7.1.0-beta.tsrs4` |
| 引擎版本字符串 | `7.1.0-beta`（`tsrs_core::version()`） |
| AST protocol | `9` |
| AST header | `64` 字节 |
| AST node stride | `28` 字节 |
| 标准库 | `tsrs_vfs` 内嵌的 `lib.*.d.ts`（不读安装目录） |
| 平台 | 仅 `linux` / `x64` 有证据 |
| MSRV | `1.100`（workspace `rust-version`） |
| 供应链 | `Cargo.lock` + 精确版本 + `cargo deny`；引擎不再来自 npm tarball（typings 仍由 `assets.lock.json` 固定） |

- 依赖 MUST 使用精确版本；不得使用 `^`、`~`、`*` 或浮动 git 依赖。
- `Cargo.lock` MUST 提交；升级 tsrs MUST 作为独立变更，重跑 §2.3/§2.4 探针与 §9 字段级差分。
- 许可证 MUST 随二进制分发 Apache-2.0/Unicode-3.0 NOTICE；`cargo deny` 的重复版本族调整必须与依赖变更同一提交。
- **不得**读取 `TSRS_LIB_PATH` 选择标准库；**不得**在编译期设置 `TSRS_TS_VERSION`（CI 必须断言它未设置）。

### 2.2 启动顺序 [F0-03][F0-04]

启动必须在提供任何类型能力之前完成，顺序固定：

1. 校验 `tsrs_core::version()` 与 §2.1 的固定值一致（crate 版本由 Cargo 精确 pin 保证）。
2. 在 `catch_unwind` 内构造 `InProcTransport` 与 `tsrs_api::Session`；构造没有 `Result`，panic 视为启动失败。
3. 用 `bundled::wrap_fs(VfsHandle)` 与 `bundled::lib_path()` 配置会话文件系统与默认库路径；不得回退到宿主 FS 或环境变量。
4. 发送 `initialize`，校验大小写敏感与 `currentDirectory` 形状。
5. 执行 §2.3 的 AST schema 探针与 §2.4 的能力探针；全部通过后才把类型能力标记为 `Supported`。
6. 任一步失败：返回 `Unavailable`（构造/版本/协议）或 `Protocol`（探针字段不自洽），类型能力不可用；不得加载项目内容、不得部分启用、不得自动降级到其它版本。

### 2.3 AST schema 探针 [F0-05]

对已知源码请求 `getSourceFile`，解码 base64 并校验：

- 头部长度 64 字节，metadata word 的高字节为 **9**；
- `bytes` 等于解码后 payload 长度；
- `string_offsets == 64`，且 `string_offsets <= string_data <= extended <= structured <= nodes <= bytes`；
- 节点表起点 `nodes` + `nnodes * 28 <= bytes`，且与前一 section 不重叠；
- 字符串池条目数 `nstrings`、节点数 `nnodes` 在预算内（各 1,000,000），section 长度不为负、不溢出；
- `source_id`/`source_lease`/`binder` 只作为元数据，不作为安全令牌。

版本不是 9、header 不是 64、stride 不是 28、字段不自洽、section 越界或超出预算：返回 `Protocol("engine AST schema mismatch")`，类型能力不可用，不得解码出“部分结果”。7.0.2 的 v5/header 44 数据必须被拒绝。

### 2.4 能力探针 [F0-04][F0-12]

在专用空虚拟 cwd 上创建最小全虚拟快照（一个 `const x = 1;` 文件），至少验证以下方法存在且返回成功：

`createSnapshot`、`getSourceFileNames`、`getTypeAtPosition`、`getSourceFile`、`getSemanticDiagnostics`、`getCompletionsAtPosition`、`getReferencedSymbolsForNode`、`getDocumentationComment`、`getJsDocTags`、`typeToString`、`release`。

任一处返回 `unknown API method` 即启动失败，不做“尽力兼容”；探针成功后释放最小快照与 lease，失败时关闭会话，不留下可用状态。

## 3. 已废弃的子进程与传输条款

以下 2026-11-20 口径的条款在进程内会话下**不再适用**。表中“现状”是唯一有效说法；旧条款只作为历史记录。

| 旧条款 | 现状 |
| --- | --- |
| 引擎发行物：npm `@typescript/typescript-linux-x64@7.1.0-beta`，tarball/SRI/`gitHead`/可执行文件摘要校验 [F0-01][F0-03] | 废弃。引擎由 Cargo 精确 pin 的 `tsrs_*` crate 提供，身份由 `Cargo.lock` 与 `cargo deny` 约束；不再有 tarball、`assets.lock.json` engine 段或 `lib/tsc`。 |
| 启动前执行 `lib/tsc --version` 并精确匹配 `Version 7.1.0-beta` | 废弃。改为进程内校验 `tsrs_core::version()`，不做可执行文件预检。 |
| `lib/tsc --api --async --cwd ... --callbacks=...` argv 基线 [F0-06] | 废弃。没有 argv；cwd 变成虚拟 cwd，回调能力由 `VfsHandle` 实现 `tsrs_vfs::FS` 代替。 |
| 独立 stdin/stdout/stderr 管道、清空环境、`LANG=C`、exec 不经 shell | 废弃。没有外部进程和管道。 |
| `Content-Length: N\r\n\r\n` 帧、header 上限、短 body 超时、坏帧退出 [F0-07] | 废弃。请求直接以结构化参数交给 `Session::handle_request`，没有帧、没有 stdout 字节流。 |
| 客户端请求 ID、服务端回调 ID 路由、双向 dispatcher、回调 2 s 超时 [F0-09][F0-10] | 废弃。没有 JSON-RPC 通道；文件访问由 FS trait 同步返回，不存在回调泵与死锁面。 |
| stderr 排空线程、环形缓冲、panic 栈清洗、退出诊断 [F0-11] | 废弃。没有子进程 stderr；错误文本仍必须脱敏，不得把绝对路径或内部栈暴露给用户。 |
| 进程死亡检测（EOF/BrokenPipe/非零退出）、`SIGKILL → reap`、最多 3 次启动、100/500/2000 ms 退避、协议不匹配 1 次重启 [F1-08][F1-09] | 废弃。没有进程可重启；不可恢复只降级为 `SessionPoisoned`，见 §6.6 与 §10。 |
| 通过 `TSRS_BIN`/`TSRS_ASSET_ROOT` 覆盖引擎位置 | 废弃。两个变量移除；外部引擎覆盖不再支持。 |

仍然有效的传输无关约束：用户不可见细节只进日志；本地拒绝的消息不得发送给引擎；未知方法默认拒绝；错误与日志不得泄漏路径、栈或秘密。

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

`getSignaturesOfType`、`getResolvedSignature`、`getSignatureHelpItems` **不在 C0 允许清单**；若未来需要签名帮助，必须单独增加 F1 合同、节点 kind 白名单和回归测试。

`batchRequests` v1 **不允许**：分页/`continuationToken`/大响应预算未验证。若未来启用，必须满足：内部 method 全部在白名单、禁止嵌套 batch、逐项预算、任一项被拒绝则整批不发送。

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
| 其它 | 4.1 未列出的全部方法 |

### 4.3 未知/拒绝方法的 fail-closed [F0-14]

- 类型化枚举之外的 method 必须在 `Session::handle_request` 之前拒绝：`Protocol("method not allowlisted")`；不得发送、探测或重试。
- 引擎返回 `unknown API method`：
  - 探针阶段：启动失败，`Unavailable("engine API mismatch")`；
  - 运行阶段：当前请求返回 `Unsupported`/`Protocol`，不重试、不回退版本、不返回空成功。
- 引擎未来新增方法仍默认拒绝；不得根据运行时返回动态扩充白名单。
- 任何拒绝方法/未知方法的测试都必须断言：请求未送入 `handle_request`、无文件写入、无结果发布。
- 引擎不得加载项目内插件或执行项目代码：不读取 `plugins`、不执行 `node_modules`、不提供外部代码开关。

## 5. 文件系统合同

### 5.1 虚拟快照 [F0-15]

- 每个项目快照 MUST 使用 `fileSystem.kind="full"`，`files` 字典包含该快照所有可读业务文件（TS/JS/`.d.ts`，以及确有需要的 `tsconfig`/`package.json` 的虚拟副本）。不得使用 `kind="layer"` 叠加宿主 FS。
- 业务文件 MUST NOT 依赖磁盘存在；所有内容通过快照进入引擎。
- 未保存文档通过 `openFiles` 打开；其内容同时出现在 `fileSystem.files`，`openFiles` 只能引用快照内路径。
- `openProjects` 的 tsconfig、`extends`、`paths`、`typeRoots` 目标必须全部在 VFS 或受信任资产根内；缺失时返回配置/诊断降级，不得读 OS。
- 合成程序、inferred 项目、配置项目的 project id 是引擎生成/不透明标识，Rust 不得把它们当作稳定项目键；Rust 项目键由 `AnalysisInput` 的 `ProjectId` + snapshot/epoch 组成。
- `parsedCommandLine.fileNames` 可能重复；Rust MUST 去重，且不得按该数组长度推断文件数。

### 5.2 `VfsHandle` 的 `tsrs_vfs::FS` 语义 [F0-15][F0-16]

`VfsHandle` 持有当前快照的虚拟文件表（打开/更新时安装），所有读取只查虚拟表与受信任资产索引：

| FS 方法 | 要求 |
| --- | --- |
| `read_file` | 命中返回内容；缺失或越界返回“不存在”，不得回退宿主 |
| `file_exists` | 返回布尔；越界返回 `false` |
| `directory_exists` | 返回布尔；越界返回 `false` |
| `get_accessible_entries` | 只枚举允许根内的直接子项；越界返回空集合 |
| `stat` | 返回真实文件/目录 mode、大小与固定 mtime；不存在返回“不存在” |
| `realpath` | 返回规范化后的虚拟路径；不得调用宿主 realpath，不得解析宿主符号链接 |
| `write_file` / `append_file` / `remove` / `chtimes` | 一律返回错误，宿主文件不变 |
| `use_case_sensitive_file_names` | 返回 `true`（仅 Linux 语义有证据） |

- 读入口 MUST 覆盖引擎的实际探测面：项目/配置、`package.json`、模块解析、`typeRoots`、默认库、typings、`realpath`、目录枚举。
- 只有两个允许的数据来源：当前快照的虚拟文件表、固定只读资产索引。**不得**以任何方式回退宿主文件系统。
- 路径规范化之前不得查询虚拟表；越界必须按 §5.4 的 fail-closed 结果返回，不得“修复路径后重试”。
- 读取结果必须可审计：日志记录规范化后的请求路径与结果类别（命中/缺失/拒绝），用于读路径审计。
- 引擎会探测项目根之外的配置、`src/tsconfig.json`、`jsconfig.json`、父级目录、`/package.json`、`/tmp`、`/` 等；这些一律返回不存在/`false`/空集合/identity，不得枚举宿主目录。

### 5.3 写入边界 [F0-17]

- `write_file`、`append_file`、`remove`、`chtimes` 在 FS 层就必须返回错误；这是权限边界，不是 noop 缓解。
- 方法层白名单（第 4 节）是第一道防线：即使 FS 实现写错，也不得存在可达的 emit/build/clean/profile 方法。
- 历史风险 B-02（`appendFile`/`chtimes` 无法注册回调）在进程内模型下消失：没有回调注册表，所有写入口都经过同一个 FS trait。
- 测试必须对沙箱外 canary 目录/文件做快照前后哈希，断言：允许查询不改动树；误调用被拒绝方法不产生写；任何写/追加/删除/chtimes 尝试都返回错误且宿主不变。

### 5.4 路径规范化与限制 [F0-18]

所有 URI/路径在进入 VFS、虚拟表或 handle 绑定前必须规范化：

1. 只接受 `file://` URI；拒绝其它 scheme、userinfo、query、fragment；`file://localhost/path` 可映射为本机路径，其它 authority 拒绝。
2. 百分号解码一次为 UTF-8；无效 UTF-8、解码后出现 NUL/控制字符、解码后出现编码分隔符：拒绝。
3. 必须是绝对路径；拒绝空组件、`.`、`..`（包括 `%2e%2e` 解码后的形式）；拒绝相对路径。
4. 分隔符统一为 `/`；Windows 盘符/UNC 未验证，非 Linux 平台不得假设可用。
5. 大小写：Linux 上大小写敏感；不得对路径做 Unicode case folding。大小写不敏感平台的语义未验证。
6. 不调用 OS canonicalize/realpath；Rust 层在 ProjectHost 边界解析宿主符号链接并生成 canonical 虚拟路径；快照内不允许符号链接项。
7. 允许根：各项目根、`typings/<fixed>` 资产根。前缀匹配必须按**路径组件边界**，不得用字符串前缀（防止 `/proj` 匹配 `/project-evil`）。
8. `get_accessible_entries` 只返回允许根内的子项；不得枚举 `/`、用户 home、`node_modules` 或项目外目录。
9. 任何越界/非法路径必须按 FS 方法 fail-closed：读取返回不存在，`file_exists`/`directory_exists` 返回 `false`，目录枚举返回空，`realpath` 返回 identity；不得尝试“修复路径后重试”。
10. 允许根之外的真实文件（如项目外相对 import）也必须被拒绝，并由诊断/`Partial` 表达，不得读 OS。

### 5.5 标准库与类型资产 [F0-19]

- 标准库由 `tsrs_vfs` 内嵌提供，默认库路径取自 `bundled::lib_path()`；**不得**读取安装目录的 `lib*.d.ts`，**不得**读取 `TSRS_LIB_PATH`。
- 小程序声明资产（`typings/miniprogram-api-typings`）仍然固定版本、摘要、许可证；缺失必需资产：启动/探针失败；缺失可选资产：相关查询 `Partial`，不得静默返回“干净”。
- 项目声明资产只能以虚拟快照内容或受信任资产根出现；禁止运行时 npm/registry 解析、隐式下载、读取项目 `node_modules`。
- 许可证文件（typings LICENSE、内嵌标准库与依赖的 NOTICE）必须随包，不得删除或改写。
- `extends` 可位于项目根之外的受信任配置；越界时返回配置错误/`Partial`，不得读 OS。
- `typeRoots` 已实测会被读取；必须同时支持“显式 `types` 的读取”和“未读取时按缺失/`Partial` 降级”，不得读 OS 补全。

## 6. 会话、快照与生命周期

### 6.1 创建与身份 [F1-01]

- `AnalysisInput` 是不可变源快照：项目/文件 revision、源文本、编译选项、固定声明资产、模板片段、依赖指纹。`DerivedView` 是同一输入的 shadow/映射/基线 facts，不得引用可变宿主状态。
- 快照创建由 `openProjects`（配置项目）、`createPrograms`（合成/原位 shadow）、`openFiles`（未保存/打开文档）、`fileSystem.kind="full"` 组成。
- 快照创建成功后记录：引擎 snapshot id、每个 project 的引擎 id、文件集合、内容哈希、配置/依赖指纹、session generation。Rust 自己的 `SnapshotId` 与引擎 snapshot id 必须同时保存；不得把引擎 id 直接暴露为稳定公共 ID。
- 快照创建失败：不得发布半成品；旧快照若仍有效则保持，否则项目进入 `Unavailable`，并在诊断中给出可读原因。

### 6.2 更新语义 [F1-02]

下表是保留的更新语义要求（来源是 tsgo 实测，进程内会话必须由差分/回归测试保持）：

| 更新方式 | 已打开文件内容 |
| --- | --- |
| 仅 `fileSystem.kind="layer"` 新内容 | 仍旧 |
| 仅 `fileSystem.kind="full"` 新内容 | 仍旧 |
| `fileNotifications.changed` + layer | 仍旧 |
| `closeFiles` + `openFiles`（同一请求） | 新内容 |

因此：

- 任何未保存内容变化 MUST 在**同一次** `updateSnapshot` 请求中同时发送 `closeFiles`（同 URI）与 `openFiles`（同 URI），并携带更新后的 `fileSystem` 内容；不得拆成两次请求，不得只发通知/layer。
- 新建文件：`openFiles` + files；删除/关闭文件：`closeFiles` 并从 files 移除；重命名按“关闭旧路径 + 打开新路径”处理。
- 一次 `updateSnapshot` 只对应一个 `AnalysisInput` revision；并发编辑必须串行化。不得在等待上一次更新结果时合并出第二个未确认的 epoch。
- 更新后旧 snapshot 在未 `release` 前仍可查询（引用计数），但 Rust MUST 立即把旧 epoch 标记为非当前，且不再对其发布结果。
- 更新失败：若旧 snapshot 仍可用，保持旧 epoch 并返回失败；不得假设更新已部分生效。

### 6.3 版本与 epoch [F1-03][F1-26]

一个可发布结果必须同时满足：

```text
native result.session_generation   == current.session_generation
native result.snapshot_id          == current.snapshot_id
result.project_revision            == current.project_revision
result.document_version            == current.document_version
result.dependency_fingerprint      == current.dependency_fingerprint
```

- epoch 至少由 `(session_generation, snapshot_id)` 组成；项目 cache key 还必须包含 `ProjectRevision`、文档版本、配置/依赖指纹、引擎/资产身份。
- LSP `didChange` 版本必须单调递增；小于等于当前值的编辑拒绝应用（沿用 C0 `ClientVersion` 规则）。
- 结果完成时再次校验 epoch；不匹配则丢弃，返回 `Cancelled`/内部 stale，不发布、不转成空结果。
- 旧 snapshot 在 release 后查询必须报错；Rust 不得缓存旧 snapshot 结果跨 epoch 复用。

### 6.4 单进程单会话、有限快照 [F1-08]

- 每个 Rust LSP 进程只创建一个进程内 `Session`；不按项目/文件创建会话，也不保留“每项目一个引擎”的路径。
- 多项目通过有界 session/snapshot 管理：初始预算为最多 **8 个存活 snapshot**、每个项目最多 **1 个 current snapshot**，旧 snapshot 在无 pending 请求后按 LRU/短 grace 释放；超限必须拒绝或回收，不得无界增长。
- `Session` 是 `Send + Sync`，但 LSP 适配层仍限制同时在途请求（并发上限与 deadline 见 §10.3）；同一项目的快照更新必须串行。
- 切换项目/配置变化（tsconfig、依赖、typeRoots、资产版本）视为新快照，不是普通文档更新；旧项目 snapshot 释放。
- 不频繁重建会话：全局 worker/arena 不随 `close` 回收，重建会累积常驻资源。

### 6.5 关闭 [F0-20]

- 关闭项目：等待该 epoch pending 结束（有 deadline）→ 释放 lease → `release` snapshot → 删除 cache。
- 关闭 LSP：停止接受新请求 → 取消排队 → 对运行请求设 deadline → 释放 lease/snapshot（有界）→ 关闭会话。
- 没有外部进程需要等待或 reap；`Session::close` 不 join 全局线程是已知限制，不得据此假设线程已退出。
- 任何释放失败不得阻塞关闭；记录后放弃该会话，由进程退出回收。

### 6.6 不可恢复与能力降级（替代重启） [F1-09]

进程内没有可靠的强制终止手段，因此**不存在重启、退避或健康检查杀死**：

- 单请求的错误先以该请求失败返回；引擎 panic/编码失败映射为内部错误时，会话同时标记中毒（§6.6），类型能力降级。
- 会话中毒、锁中毒或语言服务器状态不可恢复：`is_alive` 置 false，类型能力标记 `SessionPoisoned`；非类型功能（语法、静态补全、格式化等）继续工作。
- `SessionPoisoned` 是终态：不静默重试、不自动重建会话、不把旧结果映射到新状态；恢复需要用户重启语言服务器。
- 所有 pending 请求在降级时以 `Unavailable`/`Cancelled` 结束，不得映射为“空诊断/无定义”。
- 不允许重放旧 handle/旧 symbol descriptor；不得把旧结果映射到新 AST。

## 7. 原生句柄、符号描述符与租约

### 7.1 Handle 绑定 [F1-04]

node handle 格式为 `<encoder index>.<kind>.<path>`，不携带快照/内容身份；跨 `updateSnapshot` 可能静默指向新 AST 的同序号节点，甚至返回另一个符号且无错误。因此：

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
- `releaseSourceFile` 对不存在 lease 返回错误；重复释放只记录一次，不得 panic。
- `getCachedSourceFile` 返回的数据 header 不写回 lease；不得凭数据判断租约是否仍有效。
- 所有 lease 必须有上限和 deadline；泄漏检测测试在会话关闭后断言引擎侧 lease 全部释放。

### 7.4 清理 [F1-07]

- snapshot release 后不得再使用其 AST/handle/lease 查询；测试断言 `snapshot N not found` 被映射为 `StaleSnapshot`，不是空结果。
- 清理顺序：pending 请求失败/取消 → lease release → snapshot release → cache 删除。释放有 deadline（建议 1 s/项）；失败只记录，不阻塞关闭。
- 公共 DTO、LSP 层、持久缓存不得出现原生 handle/lease/snapshot id 作为稳定标识；它们只能存在于 `wxml-backend-tsrs` 内部并按 epoch 键控。

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

- `pos` 包含 leading trivia；定义/引用/hover 范围必须取 identifier 子节点，或从 `pos` 向后裁剪空白/注释，得到 token start；不得把 declaration 的 `pos` 直接当 LSP range。
- 范围 `end` 排他；token 文本必须与字符串池内容一致；不一致时返回 `Partial` 并丢弃该位置，不得发布错位范围。
- AST 节点查找 v1 可以扫描节点表（kind=Identifier，`pos <= offset < end`，取最小范围），不要求先实现 children 属性/binder；但必须证明选中的节点文本与源码 token 一致。
- `next`/`parent` 索引、`data` 高 2 位、字符串/扩展偏移必须按固定 encoder 布局（stride 28）校验；未知/越界 → `Protocol`，不得读越界内存或循环遍历。

### 8.3 字符串池/WTF-8 [F1-14]

- 字符串池是 UTF-8/WTF-8 字节；wire index 必须为偶数且 `ordinal < nstrings`；`pos/end` 必须在 string data 范围内且 `pos <= end`。
- 有效 UTF-8 解码为 Rust `String`；含孤立 surrogate 的 WTF-8 **不得**替换为 U+FFFD。必须以原始字节/`Wtf8String` 保留，标记不可编辑；需要 Rust `String` 的展示/编辑路径返回 `Partial`/`Unsupported`。
- 非 BMP 字符（如 😀）在 UTF-16 中占 2 个 code unit；转换必须用 `encode_utf16` 计数，不得按 Unicode scalar 或 UTF-8 字节计数。
- 孤立 surrogate 的 API/AST 端到端行为必须用原始 `Vec<u16>`/字节构造测试，断言不 panic、不损失、不伪造 range。

### 8.4 CRLF、BOM、hashbang [F1-13]

- 发送给引擎的源文本必须是**原始字节的精确 UTF-8 表示**；不得预先统一换行、去 BOM、去 hashbang 后再映射（否则 range 会偏移）。
- 行索引沿用 C0：`\n` 分行；`\r\n` 中 `\r` 属于行结束、不计入列；单独 `\r` 是普通字符。
- BOM 保留在文本中；BOM 是否被 AST 跳过、`pos` 是否从 0 还是 1 开始，必须用 fixture 校准并记录；映射层吸收该偏移，不得假设。
- hashbang 保留；若 AST 从 shebang 之后开始，同样用 fixture 校准；不得把 shebang 当注释删除。
- 组合字符按 code unit 计数，不做 NFC/NFD 归一化。
- 每种组合至少覆盖：token 前/内/后/EOF、CRLF、BOM、非 BMP；先用 AST/pool/Rust range 分层断言，再做端到端。

### 8.5 URI 与 FileId [F1-12]

- LSP `file://` URI 与内部路径双向转换必须使用同一规范化函数（5.4）；百分号编码的 UTF-8 文件名必须正确往返。
- `FileId` 是项目根相对、`/` 分隔、大小写敏感、无 `.`/`..`/空组件；项目外路径不得强行塞入 `FileId`。
- 标准库/typings 等资产路径不属于项目 `FileId`；若 C0 `SourceLocation` 无法表示，返回项目内可用位置并标记 `Partial("external declaration outside project")`，或按集成 owner 批准的 external location DTO 扩展；不得伪造项目相对路径。
- 客户端 URI 大小写/编码差异不得导致同一文件产生两个 key；统一规范化后比较。

## 9. C0 DTO 与查询组合

### 9.1 总则

- 查询入口仍使用 C0 `Backend` 边界（`hover`/`diagnostics`/`definition`/`capabilities`）；completion/references/symbol/internal type 查询先作为 `wxml-backend-tsrs` 内部 DTO，任何 `wxml-types` 公共 DTO 变更由集成 owner 评审后再接 LSP。
- 所有位置从 C0 的 UTF-8 `offset` 经当前 `SourceSnapshot::line_index().position(offset)` 转 UTF-16，再转原生位置；不得直接传字节偏移。
- 原生 `null` 是合法“无符号/无类型”，按查询语义映射，不自动当错误；但原生调用失败、解码失败、超预算不得映射为“空且 Supported”。
- 状态沿用 `CapabilityStatus`：`Supported` 仅当所有必需调用成功、所有证据可解码、无截断；`Partial` 表示证据缺失/截断/不可映射；`Unsupported` 表示没有满足合同的路径；`Unavailable` 表示会话/资产不可用；`Disabled` 表示策略关闭（如自动导入/编辑）。
- **QuickInfo、Definition、Rename 没有直接原生 API**：`getQuickInfoAtPosition`、`getDefinitionAtPosition`、`getDefinitionAndBoundSpan`、`getRenameInfo`、`getEditsForRename`、`rename` 均返回 `unknown API method`。实现只能按本节组合；不得调用这些名字，不得伪造其结果。

### 9.2 查询矩阵

| C0 查询 | 原生方法 | Rust 组合 | 默认状态/拒绝 |
| --- | --- | --- | --- |
| diagnostics | `getSemanticDiagnostics` + `getSyntacticDiagnostics` | 转换 code/category/message/line-character；去重；裁剪预算 | 两者都成功才 `Supported`；部分失败 `Partial`；全失败 `Unsupported` |
| type | `getTypeAtPosition` + `typeToString` | 位置→type；`null` 为无类型；`intrinsicName`/文本判定质量 | 成功 `Supported`；无类型 `Missing` 但不伪造文本 |
| symbol | `getSymbolAtPosition`/`getSymbolOfNode` | 名称、flags、declarations、reference；绑定 epoch | `null` 为“无符号”；解码失败 `Partial` |
| documentation | `getDocumentationComment` + `getJsDocTags` | 文档字符串 + 标签数组 | 调用失败 `Partial`/omit；不伪造文档 |
| definition | `getSymbolAtPosition.declarations` / `getReferencedSymbolsForNode.definition` + `getSourceFile` AST | handle→同一快照 AST→token 范围→`SourceLocation` | 无直接 API；别名/越界/解码失败 `Partial`；不得空+`Supported` 掩盖缺证 |
| references | `getReferencedSymbolsForNode` + `getSourceFile` | 去重 alias/target；handle→范围；shadow 反向映射 | 不完整/截断/无法映射 `Partial`；无证据不得当“零引用” |
| completion | `getCompletionsAtPosition` | 条目 name/kind/sortText；无 textEdit；详情降级 | 详情缺失 `Unsupported`/`Disabled`；自动导入关闭时 `Partial` |
| hover | `getTypeAtPosition` + `typeToString` + `getDocumentationComment` + `getJsDocTags` | 类型代码块 + 文档 + 标签 | 无 QuickInfo；组件缺失 `Partial`；不发明 display parts |
| rename | `getReferencedSymbolsForNode` + AST | 引用/符号证据 + `expected_text` + EditPlan 复检 | 证据不完整即拒绝；绝不部分/不安全编辑 |

### 9.3 Diagnostics [F1-15]

- 请求当前文件的语义 + 语法诊断；`getBindDiagnostics`、`getSuggestionDiagnostics`、`getProgramDiagnostics` 不作为 C0 必需来源（后者实测为 `[]`，不能当“干净”证明）。
- 每条诊断取 `startPosition/endPosition`（0-based line + UTF-16 character）转换为 `SourceRange`；用 `pos/end` 交叉校验，若不一致则丢弃该条并 `Partial`，不得发布可能错位的范围。
- category 映射：`Error -> Error`、`Warning -> Warning`、`Suggestion -> Hint`、`Message -> Information`；未知 category 不得默认 Error，标记 `Partial` 并保留原始 code。
- `code` 以十进制字符串保存；message 保留原文，仅做换行/长度预算处理。不得把内部绝对路径插入用户可见文本；若引擎消息含路径，按已知项目根替换为相对路径，不能确定时保留但不主动构造路径。
- 同一文件 semantic/syntactic 完全重复（同 range/code/message）去重；不同来源不得随意覆盖。单文件诊断超限截断并 `Partial`，不得静默丢弃。
- 文件集合必须来自请求/当前项目，不得把多个项目的同名文件诊断混在一起；shadow 诊断按同一映射规则回原 WXML/脚本，不能重复发布基线脚本诊断。

### 9.4 Type / Symbol / Documentation [F1-16][F1-17][F1-18]

- `getTypeAtPosition` 返回 `null`：该位置无类型，映射为 `TypeQuality::Missing` + 空类型（不是错误，也不是 `error` 类型）。
- `typeToString` 失败/空：`Unknown`；`intrinsicName == "error"` 或非法位置形态：`Unknown`，不得当具体类型展示。
- `intrinsicName == "any"` 或文本精确为 `any`：`Any`；LSP 可显示字面 `any`，但不得把它当已知成员类型。
- 其余成功文本：`Known`；保留重载文本（`{ (...); (...) }`），不要自行拆分或只取一个签名。
- `getSymbolAtPosition` 的 name/flags/declarations/reference 组成内部 `NativeSymbolDto`；`declarations`/`reference` 必须是已绑定 epoch 的 handle；`null` 表示无符号。
- 文档：`getDocumentationComment` 字符串保留换行；`getJsDocTags` 的每个 `{name, text?}` 渲染为 `@name text` 行。`@deprecated` 必须保留并可见。标签/文档为空时省略对应段落，不输出“无文档”。
- 文档调用失败但类型成功：hover `Partial`（或 type-only），不得伪造文档；符号查询仍可用。

### 9.5 Definition（无直接 API） [F1-19]

组合顺序：

1. 对位置取 `getSymbolAtPosition`。`null` → 无符号，返回空 `Supported`（这是“位置没有符号”，不是“查询失败”）。
2. 若 `symbol.reference` 存在，用它作为 `getReferencedSymbolsForNode` 的 node handle；同时传同一 UTF-16 position。无 reference 时，可用同快照 AST 的最小 Identifier 节点；找不到 → 空 `Supported`。
3. `getReferencedSymbolsForNode` 返回多个条目；收集每个条目的 `definition` handle；对 import alias 场景，保留 target 和 alias 两个候选，由 LSP 决定展示顺序；不得因去重丢掉唯一可导航目标。
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
- 空引用 + 所有调用成功且 symbol 非 null：可以表示“只有一个声明/无其它引用”，但状态在完整性未冻结前默认 `Partial`，不得把“只返回声明”当零引用。

### 9.7 Completion 与详情/自动导入限制 [F1-21]

- v1 使用 `getCompletionsAtPosition` 且 `includeSymbol:false`；条目只取 `name`、`kind`、`sortText`。不得自行执行代码/解析项目文件补全。
- 原生条目没有 textEdit；替换范围由 Rust 从当前 token 前缀计算。前缀无法确定、跨越语法边界、位置在 `.`/EOF 边界：返回 `Partial` 且不提供编辑；不得用全文替换或猜测。
- 条目 kind 映射必须使用固定引擎 enum；未知 kind 映射为 plain text 并标记 `Partial`，不得按猜测分类。
- 详情：`getCompletionEntryDetails` **不存在**。v1 的 `details` 必须显式 `Unsupported`/`Disabled`（可见降级），不得用 `getTypeOfSymbol`/docs 拼出“详情”冒充原生详情。
- 自动导入：`includeSymbol:true` 需要快照 `prepareAutoImports`。v1 默认关闭；关闭时缺失自动导入条目，必须把结果标记为 `isIncomplete=true`/`Partial`，并在能力/日志中说明，不得静默假装完整。
- 位置在 `.` 上、EOF、非法边界可能返回全局补全或 `null`。`null` → 空 `Unsupported`/`Partial`，不得当“没有补全”发布；`.` 后若无法确认成员上下文，不得把全局条目当成员补全。
- 最多 `MAX_COMPLETION_ITEMS`（256）条；超限截断并 `Partial`；保留原生 `sortText`/顺序，不额外排序掩盖缺失。

### 9.8 Hover 内容模型 [F1-22]

Hover 由三部分组成，顺序固定：

1. 类型代码块：仅当 `typeToString` 成功且质量允许时输出；重载保留原生合并文本，不拆成多个“当前重载”。
2. 文档：`getDocumentationComment` 原文，保留换行；多个 overload 的文档可能已合并为换行字符串，不得伪造分段。
3. JSDoc 标签：`@name text` 行；`@deprecated` 必须保留。

约束：

- 无 LSP `MarkupContent`/display parts；由 Rust 生成 Markdown 字符串，`markup=Markdown`。不得输出 HTML、伪造链接、参数高亮或未知结构。
- 范围：优先使用光标处 identifier token 的范围；不得用声明范围替代。无可靠范围 → `range=None`。
- 质量：`Known` 才显示完整类型块；`Any`/`Unknown`/`Missing` 沿用 C0 规则，不得把 `error`/未知类型包装成具体类型。
- 文档可选：类型成功、文档缺失 → type-only hover，`Supported`；类型失败但文档可用 → `Partial`；两者都不可用 → `Unsupported`，不得返回空字符串 + `Supported`。
- 旧 QuickInfo 的 display parts、参数名/类型分段、每 overload 独立文档在原生 API 中不存在；无法等价时必须记录为已知差异并修订本合同，不能悄悄降级。

状态合成（`HoverEvidence`）：

- `HoverEvidence` 的 `type_info`/`type_quality`、`documentation`、`symbol`、`declarations` 与 `gaps` 共同决定 `status`：没有可渲染的类型或文档 → `Unsupported`；声明目标落在项目外、声明句柄不可解码、声明列表/扫描被截断，或 `Missing` 类型仍有符号/声明证据 → 至少 `Partial`，且 `reason` 保留后端原文，不得省略或改写。
- 空 Markdown 与 `Supported` 同时出现是契约违反：LSP adapter 必须发布 `Unsupported` 并保留可读原因。
- `Unsupported` + 非空 Markdown 同样是契约违反：adapter 必须降级为 `Partial` 并明确说明矛盾。
- host degradation（shadow/源文件列表不完整等）与 hover range 映射失败都是追加原因：`Supported` → `Partial`；已有 `Partial`/`Unsupported` 保留原 `reason` 并追加（去重），不得覆盖后端原因。

### 9.9 Rename：保守编辑规则 [F1-23][F1-27]

`getRenameInfo`/`getEditsForRename`/`rename` 不存在；Rust 只能基于符号 + 引用 + AST 证据构造编辑。**只有同时满足以下全部条件才允许生成编辑：**

1. 光标位于原始源文件的 identifier token（不是 shadow/synthetic/generated 文本）；symbol 非 null，类型不是未知/`any`，不是外部资产符号。
2. `getReferencedSymbolsForNode` 成功，返回非空且未截断；所有 definition/reference handle 在同一 snapshot 内可解码。
3. 目标符号唯一：alias 与 target 解析到同一目标；出现多个目标、合并声明无法区分、或 alias 目标不唯一 → 拒绝。
4. 每个引用 token 的文本等于当前符号名/`expected_text`；`expected_text` 缺失或不匹配 → 拒绝。shorthand property、computed property、字符串/注释、导入 specifier 等必须逐项确认，不能只比较全文。
5. 所有引用都在当前 snapshot 的项目文件内，且能映射回原始 WXML/脚本；外部资产、生成声明、synthetic 文本、无法反算的 shadow 位置 → 拒绝。
6. 编辑范围通过当前文档版本/UTF 转换校验，互不重叠；任一范围落在代理对中间、CRLF/BOM 边界无法确定、或版本已变 → 拒绝。
7. 生成编辑后，在隔离快照上重新 parse/typecheck；出现新错误、目标数变化、或无法复检 → 拒绝整个编辑，不部分应用。
8. 最终仍由 LSP `EditPlan` 校验路径、版本、范围、重叠、语法和 UTF 转换。

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
| 会话中毒 | `SessionPoisoned`，类型能力不可用，非类型功能继续 |

## 10. 错误、取消、超时与降级

### 10.1 错误类别与映射

内部 `NativeError`/`UnavailableReason` 的类别与边界映射（名称以代码为准）：

| 类别 | 示例 | 边界映射 |
| --- | --- | --- |
| `Unavailable { EngineVersionMismatch }` | crate/`tsrs_core::version()` 与固定值不符 | `BackendError::Unavailable` |
| `Unavailable { EngineStartupPanic }` | `Session::new` panic | `Unavailable` |
| `Unavailable { ApiMismatch }` | 能力探针缺少必需方法 | `Unavailable` |
| `Unavailable { MissingAsset }` | typings/内嵌标准库缺失 | `Unavailable` |
| `Unavailable { VfsConfinement }` | 启动期 VFS 围栏 canary 失败 | `Unavailable` |
| `Protocol` | AST schema 违规、响应形状非法 | `Protocol`（用户不可见细节） |
| `Unsupported { method }` | 运行期方法不可用 | `Unsupported` 或 `Protocol`；探针失败为 `Unavailable` |
| `MethodNotAllowed` | 白名单外方法 | `Protocol`，不送入引擎 |
| `InvalidParams`/`InvalidHandle` | 位置越界/URI 非法/handle 畸形 | `Protocol` 或 `Unsupported`，不重试 |
| `StaleHandle`/`StaleSnapshot`/`NoSnapshot` | 跨 epoch/已 release/未打开 | 内部 stale；请求层 `Cancelled`/`Unsupported`，不发布旧结果 |
| `EngineError { code }` | 引擎返回错误码 | `Internal`，只保留 code，消息清洗 |
| `InputTooLarge` | 序列化请求超预算 | 本地拒绝；查询可 `Partial`+截断 |
| `Unimplemented { feature }` | 未实现能力 | `Unsupported`/`Disabled` |
| `Timeout` | 调用方 deadline 到期 | `Timeout`；底层计算不可取消 |
| `Unavailable { SessionPoisoned }` | 会话锁中毒/内部状态不可恢复 | `Unavailable`；类型能力降级，进程继续 |

`BackendError::Protocol`/`Internal` 的文本只用于日志和诊断，不得直接展示给最终用户；不得包含内部栈、绝对路径、环境变量或秘密。

### 10.2 取消 [F1-24]

- **排队中**：从队列移除，返回 `Cancelled`，**不送入**引擎。
- **运行中**：没有服务端取消能力。标记 cancel token；结果到达后丢弃，返回 `Cancelled`，不发布任何部分结果。不得声称底层计算已停止。
- 取消发生在快照更新期间：更新本身不可取消；完成后 epoch 提升，旧请求结果丢弃。
- 运行中取消不会结束会话，也不会触发重启；若结果永久不返回，由调用方 deadline 处理。
- 会话降级后，所有排队与在途请求以 `Cancelled`/`Unavailable` 结束。

### 10.3 超时与不可恢复状态 [F1-25]

- deadline 从请求入队或发送时开始，默认 15 s；调用方更短的 deadline 优先。快照更新 30 s，启动探针 10 s，lease/snapshot release 1 s，shutdown 5 s。
- 超时后：移除 pending、标记该请求放弃、返回 `Timeout`；迟到结果丢弃。**不重启**，也不因连续超时杀死会话。
- `Deadline` 只限制等待，不取消底层检查；不得把“已超时”描述成“已停止计算”。
- 单请求 panic 以内核内部错误返回，并使会话中毒（§6.6）；其它错误类别不改变会话可用性。
- 重试：没有进程重启，因此没有启动重试与退避，也不对纯查询做不透明重试；`MethodNotAllowed`/`Unsupported`/`InvalidParams`/`StaleHandle` 一律不重试。
- 类型能力不可用（`Unavailable`/`SessionPoisoned`）时，请求立即失败，不得排队等待“可能恢复”。

### 10.4 旧响应屏障 [F1-03]

- 每个请求携带入队时的 epoch；结果完成时比较当前 epoch。不匹配：丢弃并返回 `Cancelled`/内部 stale，不发布、不合并、不转空。
- LSP 发布层仍执行 C0 校验（文件/SourceRevision/SnapshotId/ProjectRevision/ViewRevision/BackendEpoch）；双层校验缺一不可。
- `release` 后到达的结果、旧 epoch 的结果、跨项目结果均丢弃；不得写入当前缓存。
- 取消/超时/降级后不得把旧 diagnostics/WorkspaceEdit 写入新文本；测试必须注入“v1 查询→v2 更新→v1 迟到响应”。

## 11. 安全验收

以下是不变量，评审必须逐项核对；任何一项不满足都阻塞变更：

| 编号 | 不变量 | 验收 |
| --- | --- | --- |
| S1 | 只发送 4.1 白名单方法 | 类型化枚举 + 运行时断言 + fake 记录 method |
| S2 | 无宿主写：emit/build/clean/profile/heap 不可达；FS 写/追加/删除/chtimes 一律返回错误 | 沙箱树前后哈希、canary、拒绝方法测试、FS 写拒绝测试 |
| S3 | 读路径可审计：只查虚拟表与受信任资产根；越界返回各自否定结果；无宿主回退 | 读路径日志、沙箱外 canary 不可见、symlink 不可逃逸、越界负例 |
| S4 | 不执行外部代码 | 不加载 tsconfig `plugins`、不读 `node_modules`、无 Node 子进程、无外部引擎覆盖变量 |
| S5 | 只读不是靠“noop 让写看起来成功” | 文档/测试显式断言写入口返回错误；即使 noEmit 也不能暴露 emit 方法 |
| S6 | 路径限制：绝对、规范化、无 `..`/NUL、按组件前缀、资产根只读 | 路径表驱动负例 + 恶意 URI |
| S7 | 资源有界：JSON/AST/队列/诊断/引用 | 超限 typed error、无 panic/无 OOM、会话仍可用或显式降级 |
| S8 | 错误/日志不泄漏路径、栈、秘密 | panic/错误文本清洗测试；DTO 无绝对路径 |
| S9 | 不在探针前发送项目内容 | 启动顺序测试；失败不读项目 |
| S10 | 安全失败是显式失败，不是空成功/静默降级 | 所有拒绝路径返回 typed 状态/错误 |

读路径审计至少覆盖：项目 `tsconfig`/`extends`/`paths`/`typeRoots`、`package.json`、模块解析、`node_modules`（仅快照）、内嵌标准库、typings、`realpath`、目录枚举、符号链接、`..`/编码路径、canary 文件。写路径审计至少覆盖 FS trait 的 `write_file`/`append_file`/`remove`/`chtimes` 以及方法白名单中的 emit/build/clean/profile 族。

## 12. 合同 ID 与测试挂点

F0-01..F0-20 与 F1-01..F1-27 是 2026-11-17 迁移期的合同 ID；其中进程/帧/回调/重启相关条目已在 §3 废弃，其余 ID 语义不变。当前覆盖以仓库测试为准：

- F0（版本/探针、白名单、VFS 与回收）：`crates/wxml-backend-tsrs/tests/asset_loader.rs`、`type_assets.rs`（typings 闭包与内嵌标准库探针），以及 `src/native/` 下 `inproc`/`vfs`/`ast`/`handles` 的单元测试。
- F1（会话/epoch、UTF 与范围、查询 DTO）：`crates/wxml-backend-tsrs/tests/real_engine.rs`、`f1_hover.rs`、`shadow_*.rs`、`script_*.rs`。
- LSP 端到端：`crates/wxml-lsp/tests/lsp.rs`、`native_lsp_document_symbols.rs`、`project_host.rs`。

这些测试不能替代真实类型探针与差分，也不得因测试通过而降低任何 F0/F1 合同点的断言。真实类型用例由 `TSRS_TYPINGS_ROOT` 与 `WXML_REQUIRE_REAL_ENGINE=1` 门禁启用；未启用时跳过，不得据此声称类型行为已验证。

## 13. 开放问题与已知限制

- **不可取消**：运行中的类型计算没有取消通道，超时只停止等待；LSP 层靠限制并发控制资源。
- **无进程级隔离**：OOM、abort、栈溢出会终止语言服务器；单请求 panic 之外的故障不可恢复。
- **中毒即降级**：`SessionPoisoned` 后类型能力持续不可用，只能由用户重启恢复；不自动重建会话。
- **全局 worker/arena 不回收**：`Session::close` 不 join 全局线程；单进程只创建一个会话，需持续测量 RSS。
- **prerelease API**：`7.1.0-beta.tsrs4` 为精确 pin 的预发布版本，升级必须重跑探针与差分。
- **字段级差异**：`Partial` 方法（hover/补全/引用/文档）可能与 tsgo 有可见差异，差异必须记录，不阻塞结构替换但不得静默。
- **平台**：只在 Linux x86_64 验证；大小写、路径分隔符与 ABI 结论不外推到其它平台。
- **性能**：没有大项目语料；冷启动与首查成本需要单独测量，不能引用上游基准数字。
