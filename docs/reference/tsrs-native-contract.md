# 原生类型后端合同（tsrs 进程内会话）

**状态：accepted（2026-11-22，进程内会话口径）。**

本文定义 native 后端的现行边界：固定版本与启动探针、方法白名单、只读虚拟文件系统、会话/快照/句柄身份、UTF 与坐标、基础契约查询组合、错误与安全验收。

**历史边界**：2026-11-20 的 tsgo 子进程合同（npm 发行物、tarball/可执行文件校验、`--api` argv、Content-Length 帧、JSON-RPC 回调、stdout/stderr 泵、进程重启与退避）已经废弃，不再表示当前实现；2026-11-17 的实测只作为方法形状、AST 布局和 FS 语义的历史来源。旧的 `F0-xx`/`F1-xx` 是迁移期编号，不再逐项对应现行条款。迁移归档见 [replace-tsgo-with-tsrs-session](../../openspec/changes/archive/2026-11-21-replace-tsgo-with-tsrs-session/)，历史版本见 tag `pre-slim-20261120`。权威顺序以[文档索引](../README.md#3-权威顺序)为准。

## 1. 性质、权威与证据边界

### 1.1 规范词

- **MUST / MUST NOT** 是强制项；**SHOULD** 是需要说明理由的默认项；**MAY** 是可选。
- 中文“必须/不得”等同 MUST/MUST NOT。
- 本文描述合同要求，不把“代码里存在某个测试”当作“当前行为已验证”。

### 1.2 权威顺序

冲突时按以下顺序判断：

1. 当前代码与默认运行的测试。
2. 已接受的 ADR（[ADR-0012](../adr/0012-in-process-tsrs-session.md)、[ADR-0009](../adr/0009-project-slimming.md)）。
3. 本文件与[基础契约](contracts.md)。
4. 项目 README 与 `docs/reference/` 其它参考。

代码事实优先于文档；发现不一致时修订文档，不保留两套说法。本文件与 ADR 冲突时，不得用本文件覆盖 ADR。

### 1.3 证据边界

- 默认 `cargo test` 不要求 native 真实类型环境：真实类型用例在缺少 typings 时跳过。
- `TSRS_TYPINGS_ROOT` 指定真实 typings 根；`WXML_REQUIRE_REAL_ENGINE=1` 把缺少 typings 的门禁变成硬失败。两者都是测试门禁，不是运行时配置。
- 相关测试在 [`crates/wxml-backend-tsrs/tests/real_engine.rs`](../../crates/wxml-backend-tsrs/tests/real_engine.rs)、[`crates/wxml-lsp/tests/`](../../crates/wxml-lsp/tests/) 等位置；测试存在或默认运行通过，都不等于真实类型行为已经验证。
- 本文不引用 2026-11-17 的 tsgo 测量数字作为当前性能或正确性结论。

## 2. 引擎固定、版本与启动基线

### 2.1 依赖与版本

| 字段 | 固定值 |
| --- | --- |
| crate | `tsrs_api`、`tsrs_core`、`tsrs_vfs` |
| 精确 pin | `=7.1.0-beta.tsrs4` |
| 引擎版本字符串 | `7.1.0-beta`（`tsrs_core::version()`） |
| AST protocol | `9` |
| AST header | `64` 字节 |
| AST node stride | `28` 字节 |
| 标准库 | `tsrs_vfs` 内嵌的 `lib.*.d.ts` |
| 平台 | 仅 `linux` / `x64` 有证据 |
| MSRV | `1.100`（workspace `rust-version`） |
| 供应链 | `Cargo.lock` + 精确版本 + `cargo deny`；typings 由 `assets.lock.json` 固定 |

- 依赖 MUST 使用精确版本；不得使用 `^`、`~`、`*` 或浮动 git 依赖。
- `Cargo.lock` MUST 提交；升级 tsrs MUST 作为独立变更，并重跑启动探针、AST 探针和真实类型用例。
- 许可证 MUST 随二进制分发；`cargo deny` 的重复版本族调整与依赖变更同一提交。
- **不得**读取 `TSRS_LIB_PATH` 选择标准库；**不得**在编译期设置 `TSRS_TS_VERSION`（crate 在编译期断言它未设置）。
- `TSRS_BIN`/`TSRS_ASSET_ROOT` 已移除；不存在外部引擎覆盖或 Node 回退路径。

固定值来自 [`config.rs`](../../crates/wxml-backend-tsrs/src/config.rs)；版本校验与启动失败映射来自 [`native/inproc.rs`](../../crates/wxml-backend-tsrs/src/native/inproc.rs)。

### 2.2 启动顺序

启动必须在提供任何类型能力之前完成：

1. 校验 `tsrs_core::version() == "7.1.0-beta"`。不匹配返回 `Unavailable(EngineVersionMismatch)`。
2. 在 `catch_unwind` 内构造 `Session`；构造没有 `Result`，panic 视为启动失败，返回 `Unavailable(EngineStartupPanic)`。
3. 用 `bundled::wrap_fs(VfsHandle)` 和 `bundled::lib_path()` 配置虚拟文件系统与标准库路径；不得回退宿主 FS 或环境变量。
4. 发送 `initialize`，要求 `caseSensitivity == 1` 且 `currentDirectory` 非空；带 `useCaseSensitiveFileNames` 的旧形状被拒绝。
5. 建立最小全虚拟探针快照并执行 §2.3 的探针；全部通过后才允许打开用户项目。
6. 任一步失败：版本/构造/缺少方法/资产缺失返回 `Unavailable`；AST 布局或响应形状不自洽返回 `Protocol`；不得加载项目内容，也不得部分启用类型能力。

### 2.3 AST 与能力探针

探针快照只包含一个 `probe.ts`（`const x = 1;`），按以下顺序验证：

- `createSnapshot` 返回快照与项目；`getSourceFileNames` 必须包含 `probe.ts`。
- `getSourceFile` 的 base64 payload 必须通过 `AstHeader::parse`：版本 9、header 64、section 顺序正确、字符串表与节点表对齐、`nstrings`/`nnodes` 各不超过 1,000,000、binder 范围合法。
- 在 UTF-16 偏移处找到 `x` 的 Identifier 节点，并用它调用 `getReferencedSymbolsForNode`。
- `getTypeAtPosition` 必须返回类型；`typeToString` 必须可调用。
- `getSymbolAtPosition` 必须返回符号；`getDocumentationComment` 与 `getJsDocTags` 必须可调用。
- `getSemanticDiagnostics`、`getSyntacticDiagnostics` 必须返回数组；`getCompletionsAtPosition` 以 `includeSymbol:false` 调用。
- 探针结束后释放快照；探针失败时释放/关闭会话，不留下可用状态。

缺少方法返回 `Unavailable(ApiMismatch)`；字段不自洽或 AST schema 不符返回 `Protocol`。实现见 [`native/session.rs`](../../crates/wxml-backend-tsrs/src/native/session.rs) 的 `handshake`/`probe_schema` 与 [`native/ast.rs`](../../crates/wxml-backend-tsrs/src/native/ast.rs) 的 `AstHeader::parse`。

## 3. 方法白名单、拒绝清单与 fail-closed

### 3.1 允许的方法

Rust 侧用**类型化请求枚举** `NativeMethod` 构造 method 字符串；允许的 wire 名称为：

- 生命周期：`initialize`、`createSnapshot`、`updateSnapshot`、`release`、`createSourceFile`、`retainSourceFile`、`releaseSourceFile`、`getCachedSourceFile`。
- 文件/诊断：`getSourceFile`、`getSourceFileNames`、`getSourceFileMetadata`、`getSemanticDiagnostics`、`getSyntacticDiagnostics`。
- 类型与符号：`getTypeAtPosition`、`getTypeAtLocation`、`getTypeOfSymbol`、`getDeclaredTypeOfSymbol`、`typeToString`、`getSymbolAtPosition`、`getSymbolAtLocation`、`getSymbolOfNode`、`getDocumentationComment`、`getJsDocTags`、`getReferencedSymbolsForNode`。
- 补全与项目：`getCompletionsAtPosition`、`getDefaultProjectForFile`。

`getSignaturesOfType`、`getResolvedSignature`、`getSignatureHelpItems` 不在当前白名单；`batchRequests` v1 也不允许。白名单数据在 [`config.rs`](../../crates/wxml-backend-tsrs/src/config.rs) 的 `ALLOWED_METHODS`，类型入口在 [`native/protocol.rs`](../../crates/wxml-backend-tsrs/src/native/protocol.rs) 的 `NativeMethod`。

### 3.2 生产边界与已知缺口

- 生产查询只通过 `NativeSession::call(NativeMethod, …)` 构造请求，因此只能产生白名单名称。
- 传输层 `InProcTransport::request(method: &'static str, …)` 接受字符串，**没有运行时白名单校验**。测试会发送非白名单的 `ping` 来验证传输/生命周期；生产调用方不得传任意字符串。
- `NativeError::MethodNotAllowed` 目前没有生产者；“请求未送入引擎”的保证来自类型化枚举与评审，不是传输层的运行时断言。这是一个已知边界缺口，本文不新增运行时防御。
- 引擎不得加载项目内插件或执行项目代码：不读取 `plugins`、不执行 `node_modules`、不提供外部代码开关。

### 3.3 拒绝清单

以下类别的方法**永远不得**由 Rust 发送；清单不是穷尽式，真正的控制仍是白名单：

| 类别 | 方法 |
| --- | --- |
| emit/生成/写路径 | `emit`、`emitToString`、`getJavaScriptEmit`、`getDeclarationEmit` |
| build/clean | `build`、`buildReferences`、`cleanBuild`、`cleanReferences`、`createBuildOrchestrator`、`disposeBuildOrchestrator` |
| profile/heap | `startCPUProfile`、`stopCPUProfile`、`saveHeapProfile` |
| 模块/配置解析 | `createModuleResolver`、`releaseModuleResolver`、`resolveModuleName`、`parseCommandLine`、`readConfigFile`、`parseConfigFile`、`parseJsonConfigFileContent` |
| 代码生成/格式化 | `transpileModule`、`transpileModuleFromFile`、`transpileDeclaration`、`transpileDeclarationFromFile`、`getImportAdderEdits`、`formatNodeForInsertion`、`printNode` |
| LSP 共享会话 | `getCurrentLanguageServerSnapshot` |
| 批量 | `batchRequests` v1 |
| 其它 | 白名单之外的全部方法 |

### 3.4 未知/拒绝方法

- 类型化枚举之外的 method 在调用前就不存在；生产代码没有 `call(method, params)` 逃生口。
- 引擎返回 `ErrorKind::Unsupported`：探针阶段映射为 `Unavailable(ApiMismatch)`；运行期映射为 `NativeError::Unsupported`，当前请求降级，不重试、不回退版本、不返回空成功。
- 引擎未来新增方法仍默认不可达；不得根据运行时返回动态扩充白名单。
- 拒绝方法/未知方法的测试必须断言无文件写入、无结果发布；不要求传输层另有字符串拒绝逻辑。

## 4. 文件系统合同

### 4.1 虚拟快照

- 每个项目快照 MUST 使用 `fileSystem.kind="full"`，`files` 包含该快照所有可读业务文件（TS/JS/`.d.ts`，以及确有需要的虚拟 `tsconfig`/`package.json`）。
- 业务文件 MUST NOT 依赖磁盘存在；所有内容通过快照进入引擎。
- 未保存文档通过 `openFiles` 打开；其内容同时出现在 `fileSystem.files`，`openFiles` 只能引用快照内路径。
- 受信任的 typings/标准库声明作为固定只读资产挂载；项目 `node_modules` 不扫描、不读取宿主的。
- 引擎生成的项目 id 是不透明标识；Rust 项目键由 `AnalysisInput` 的 `ProjectKey` + snapshot/epoch 组成，不把引擎 id 当稳定公共键。

### 4.2 `VfsHandle` 的 `tsrs_vfs::FS` 语义

`VfsHandle` 持有当前快照的虚拟文件表；读取只查虚拟表与受信任资产索引：

| FS 方法 | 当前行为 |
| --- | --- |
| `read_file` | 命中返回内容；缺失或越界返回“不存在”，不回退宿主 |
| `file_exists` / `directory_exists` | 返回布尔；越界返回 `false` |
| `get_accessible_entries` | 只枚举虚拟表中的直接子项；越界返回空集合，`symlinks` 为 `None` |
| `stat` | 文件返回 `0644`、目录返回 `0755`，`mod_time` 为 `UNIX_EPOCH`；不存在返回 `None` |
| `realpath` | 返回 `tsrs_core::tspath::normalize_path(path)` 的**词法**规范化结果；不调用宿主 realpath，不解析宿主符号链接 |
| `write_file` / `append_file` / `remove` / `chtimes` | 一律返回 `read-only virtual filesystem` 错误，宿主文件不变 |
| `use_case_sensitive_file_names` | 返回 `true`（仅 Linux 语义有证据） |

实现与测试见 [`native/inproc.rs`](../../crates/wxml-backend-tsrs/src/native/inproc.rs)；测试覆盖虚拟表命中、`/etc/passwd` 与穿越路径的拒绝、目录枚举、写入拒绝和清除快照后的 fail-closed。

### 4.3 读路径、写入与审计

- 只有两个允许的数据来源：当前快照虚拟表、固定只读资产索引。**不得**以任何方式回退宿主文件系统。
- 路径规范化之前不得查询虚拟表；越界按上表的否定结果返回，不得“修复路径后重试”。
- **当前不记录读取日志**。读路径的可审计性由测试保证（沙箱外 canary 不可见、穿越路径被拒绝、快照前后哈希不变），不是运行时日志。
- 写入在 FS 层就返回错误；这是权限边界，不是 noop 缓解。
- 引擎会探测项目根之外的 `tsconfig`/`jsconfig`、父目录、`package.json`、`/tmp`、`/` 等；这些一律返回不存在/`false`/空集合/identity，不得枚举宿主目录。

### 4.4 路径限制

以下是路径进入虚拟表或 handle 绑定前的规范化要求；`VfsHandle` 本身只按传入的绝对路径查表，未命中即 fail-closed。

1. 只接受 `file://` URI 或绝对路径；拒绝其它 scheme、userinfo、query、fragment。
2. 百分号解码一次为 UTF-8；无效 UTF-8、NUL/控制字符、解码后的路径分隔符：拒绝。
3. 必须是绝对路径；拒绝空组件、`.`、`..`（含编码形式）和相对路径。
4. 分隔符统一为 `/`；Windows 盘符/UNC 未验证，非 Linux 平台不得假设可用。
5. 大小写：Linux 上大小写敏感；不得做 Unicode case folding。
6. 不调用 OS canonicalize/realpath；快照内不允许符号链接项。
7. 允许根是各项目根与固定 typings 资产根；前缀匹配按**路径组件边界**，不得用字符串前缀。
8. 越界路径按 FS 方法 fail-closed：读取不存在、布尔为 `false`、枚举为空、`realpath` 为 identity。
9. 允许根之外的相对 import 也必须拒绝，并由诊断/`Partial` 表达，不得读 OS。

## 5. 会话、快照与生命周期

### 5.1 单会话与快照账本

- 同一时刻只使用一个进程内 `Session`；不按项目/文件创建会话。死会话或项目/根/环境变化可以重建。
- `NativeSession` 维护一个当前快照和一个有界账本 `SnapshotLedger`，记录尚未确认 `release` 的引擎快照；上限 `MAX_LIVE_SNAPSHOTS = 8`。
- 账本不是多项目 LRU 或 grace pool：正常只有当前一个快照；更新成功后立即释放旧快照，释放失败才继续留在账本并占用预算。超过上限时从最旧的、非当前快照开始释放。
- 账本项只有在引擎确认 `release` 后才移除；引擎已忘记该快照时按幂等清理处理。

### 5.2 打开与更新

- `open_project` 拒绝已打开的会话；创建 `fileSystem.kind="full"` 快照，同时保存引擎 snapshot id 与 Rust `SnapshotRevision`。
- 更新要求同一项目、同一根、revision 必须前进；`updateSnapshot` 在**同一次请求**中关闭并重新打开所有项目源文件与项目配置，并携带新的完整文件系统。
- 必须先提交新 epoch，再释放旧引擎快照；释放失败不影响新快照可用，但旧快照继续计入账本。
- 更新失败时恢复旧 VFS 并返回错误，不得假设部分生效；适配层可以把失败视为“原地更新不可修复”，关闭旧会话并按新输入重建。
- 项目、根或环境/配置/依赖/资产变化不是普通文档更新：停止旧会话并从头打开新输入。只有同项目、同根的源/视图 revision 变化才走原地 `updateSnapshot`。

### 5.3 死会话、重试与重建

- `hover`/`definition`/`completion`/`symbols`/`document_symbols` 等走统一会话查询路径的请求：一次查询的致命结果包括 `Unavailable`、`Protocol`、`Timeout`。只有致命结果且引擎会话已经 `!is_alive()` 时，适配层才对**当前请求**重建一次会话并重试。
- `diagnostics`/`script_diagnostics` 的致命失败只丢弃会话并返回降级/不可用结果，不在同一请求内重试；后续请求仍会按需重建。
- 重试使用新会话，但成功时不推进适配层 epoch：请求自身的 stamp 仍然有效，旧会话已不可能发布竞争结果。
- 每次请求最多一次重建/重试。第二次失败调用 `fail_session` 推进 epoch 并丢弃会话，只让当前请求失败；代码没有持久终态，后续请求仍会按需重建（同样每次最多一次）。
- 没有通用重试、退避或健康检查杀死。
- 查询期间的**外部** `invalidate` 不会被重试；请求返回 `Cancelled`，旧会话被丢弃。
- 项目/根/环境变化允许重建会话；这不等于对查询做不透明重试。
- 会话不可用且本次重建失败时，请求立即失败，不排队等待“可能恢复”。

### 5.4 关闭

- 关闭项目：停止新请求，释放快照，关闭会话；`Session::close` 不 join 全局线程，因此不得假设线程已经退出。
- LSP 关闭时，适配层先停止调度器并丢弃排队任务，再关闭会话；调度器最多等待 2 秒（[`scheduler.rs`](../../crates/wxml-lsp/src/scheduler.rs) 的 `SHUTDOWN_TIMEOUT`），超时后 detach worker。
- 没有 5 秒的 native shutdown 预算；native 关闭是同步调用，调度器的 2 秒是唯一的等待上限。
- 释放失败只记录，不阻塞进程退出；由进程回收资源。

### 5.5 结果身份

- 使用 handle/AST 证据的 native 查询在返回前重新检查 `(session_generation, engine snapshot id)` 和查询文件内容哈希；不匹配返回 `StaleSnapshot`/`StaleHandle`。
- LSP 发布层再检查 `AnalysisStamp`：文件、`SourceRevision`、`SnapshotId`、`ViewRevision`、`ProjectRevision` 与当前 `BackendEpoch`。
- 旧快照、旧 epoch、跨项目结果一律丢弃，不写入当前缓存，不转成空 `Supported`。

## 6. 原生句柄、符号描述符与租约

### 6.1 Handle 绑定

node handle 的 wire 形式是 `<encoder index>.<kind>.<path>`，不携带快照或内容身份；跨 `updateSnapshot` 可能静默指向新 AST 的同序号节点。因此每个原生 node handle 在 Rust 中 MUST 绑定：

```text
(session_generation, snapshot_id, normalized_file_path,
 content_hash, node_index, node_kind)
```

- `content_hash` 来自同一快照的源文件；`node_index` 是 handle 的 encoder index；`node_kind` 是 kind 段。
- 解析 handle 必须校验三段格式、十进制 index/kind、路径属于当前快照；畸形 handle 直接 `Protocol`/`InvalidHandle`，不发送。
- 跨 session/snapshot/内容哈希任一不匹配：在发送前拒绝，返回 `StaleHandle`；不得让引擎“再试一次”，不得缓存到下一 epoch。
- 每次使用前必须重新取得同一快照的 AST 或使用同 epoch 缓存；不得把旧 AST 节点用于新 program。
- 引擎自己拒绝 out-of-range handle 时映射为 `StaleHandle`/`Protocol`，不得当空结果。

### 6.2 符号描述符

- `SymbolResponse.reference`/declarations 中的 `SourceFileDescriptor` 含 `fileName`、`path`、`contentHash`、`parseOptionsKey`、`scriptKind`、`nodeId`；必须校验 `contentHash`/`parseOptionsKey` 与当前快照一致。
- 引擎对内容变化后的 symbol reference 可能报 `source file is not part of the requested program`；这是第二道校验，不能替代 Rust 的 epoch/内容绑定。
- 符号名、文档、tags 必须来自同一快照的 symbol；不得把旧文档拼到新类型上。

### 6.3 Source file lease

- `createSourceFile`、`retainSourceFile`、`releaseSourceFile`、`getCachedSourceFile` 在白名单内，但当前生产查询路径没有使用 lease：查询通过 snapshot + `getSourceFile` 解码 AST。
- 如果未来启用 lease，每个非零 lease 必须有 RAII guard 或显式释放路径；drop、查询结束、项目关闭、epoch 失效、会话关闭都要释放；重复释放不得 panic。
- 公共 DTO、LSP 层和持久缓存不得出现原生 handle/lease/snapshot id 作为稳定标识；它们只能存在于 `wxml-backend-tsrs` 内部并按 epoch 键控。

## 7. UTF、坐标与 URI

### 7.1 单位

| 边界 | 单位 | 要求 |
| --- | --- | --- |
| LSP `Position.character` | UTF-16 code unit | 现有 `Position`/`LineIndex` |
| 基础契约查询 `offset` | UTF-8 字节 | 必须是字符边界 |
| native API `position` | **文件内绝对 UTF-16 offset** | 由基础契约字节 offset 经同一快照转换 |
| AST `pos`/`end` | UTF-16 code unit | `pos` 是 full start，含 leading trivia；`end` 排他 |
| AST 字符串池偏移 | UTF-8 字节（孤立 surrogate 用 WTF-8） | `uint32` pos/end，相对 string data |
| 诊断 `startPosition`/`endPosition` | 零基行 + UTF-16 列 | 与 AST `pos`/`end` 交叉校验 |
| Rust `SourceRange` | UTF-8 字节 | 现有基础契约 |
| wire string index | `uint32` 字索引，恒为偶数 | ordinal = index / 2 |

- 查询位置进入 native 前必须由持有文本的 `LineIndex`/`SourceSnapshot` 转换；不得直接传基础契约字节偏移当 native `position`。
- 原生客户端 API 不出现裸 `u32` 位置参数；`Utf8Offset`、`Utf16Offset`、`Utf16Position` 是显式 newtype。
- 转换失败必须返回 typed error，不 clamp、不 panic：越过文本末尾、落在 `\r\n` 中间、落在代理对中间、UTF-8 非字符边界都拒绝。
- 查询位置落在代理对中间时拒绝请求，不猜测。

### 7.2 AST 范围与 trivia

- `pos` 包含 leading trivia；定义/引用/hover 范围必须取 identifier 子节点，或从 `pos` 向后裁剪空白/注释，得到 token start；不得把 declaration 的 `pos` 直接当 LSP range。
- 范围 `end` 排他；token 文本必须与字符串池内容一致；不一致时返回 `Partial` 并丢弃该位置。
- AST 节点查找可以扫描节点表（kind=Identifier，`pos <= offset < end`，取最小范围），但必须证明选中的节点文本与源码 token 一致。
- `next`/`parent` 索引、`data` 高位、字符串/扩展偏移必须按固定 encoder 布局（stride 28）校验；未知/越界返回 `Protocol`，不得读越界内存或循环遍历。

### 7.3 字符串池与 WTF-8

- 字符串池是 UTF-8/WTF-8 字节；wire index 必须为偶数且 `ordinal < nstrings`；`pos/end` 必须在 string data 范围内且 `pos <= end`。
- 有效 UTF-8 解码为 Rust `String`；含孤立 surrogate 的 WTF-8 不得替换为 U+FFFD。以原始字节保留并标记不可编辑；需要 Rust `String` 的展示/编辑路径返回 `Partial`/`Unsupported`。
- 非 BMP 字符（如 😀）在 UTF-16 中占 2 个 code unit；计数必须用 UTF-16，不得按 Unicode scalar 或 UTF-8 字节计数。
- 孤立 surrogate 的行为必须用原始 `Vec<u16>`/字节测试，断言不 panic、不损失、不伪造 range。

### 7.4 CRLF、BOM、hashbang

- 发送给引擎的源文本是原始字节的精确 UTF-8 表示；不得预先统一换行、去 BOM、去 hashbang 后再映射。
- 行索引沿用基础契约：`\n` 分行；`\r\n` 中 `\r` 属于行结束、不计入列；单独 `\r` 是普通字符。
- BOM 保留；BOM 是否被 AST 跳过、`pos` 从 0 还是 1 开始，必须用 fixture 校准并由映射层吸收。
- hashbang 保留；若 AST 从 shebang 之后开始，同样用 fixture 校准。
- 组合字符按 code unit 计数，不做 NFC/NFD 归一化。
- 每种组合至少覆盖 token 前/内/后/EOF、CRLF、BOM、非 BMP；先分层断言 AST/pool/Rust range，再做端到端。

### 7.5 URI 与 FileId

- LSP `file://` URI 与内部路径双向转换必须使用同一规范化函数；百分号编码的 UTF-8 文件名必须正确往返。
- `FileId` 是项目根相对、`/` 分隔、大小写敏感、无 `.`/`..`/空组件；项目外路径不得强行塞入 `FileId`。
- 标准库/typings 等资产路径不属于项目 `FileId`；无法表示时返回项目内可用位置并标 `Partial`，不得伪造项目相对路径。
- 客户端 URI 大小写/编码差异不得导致同一文件产生两个 key；统一规范化后比较。

## 8. 基础契约查询组合

### 8.1 总则

- 查询入口仍是基础契约 [`Backend`](contracts.md#8-后端接口与-dto)；completion/symbol/document-symbol 的内部 DTO 只在 `wxml-backend-tsrs` 与适配层使用，公共 DTO 变更仍按基础契约评审。
- 所有位置从基础契约的 UTF-8 `offset` 经当前 `SourceSnapshot::line_index()` 转 UTF-16，再转为 native 的绝对 UTF-16 `position`。
- 原生 `null` 是合法“无符号/无类型”，按查询语义映射；但原生调用失败、解码失败、超预算不得映射为“空且 `Supported`”。
- 状态沿用 `CapabilityStatus`：`Supported` 仅当必需调用成功、证据可解码且无截断；`Partial` 表示证据缺失/截断/不可映射；`Unsupported` 表示没有满足合同的路径；`Unavailable` 表示会话/资产不可用；`Disabled` 表示策略关闭。
- **QuickInfo、Definition、Rename 没有直接原生 API**：不得调用 `getQuickInfoAtPosition`、`getDefinitionAtPosition`、`getDefinitionAndBoundSpan`、`getRenameInfo`、`getEditsForRename`、`rename`；实现只能按本节组合。

### 8.2 查询矩阵

| 基础契约查询 | 原生证据 | Rust 组合 | 默认状态/拒绝 |
| --- | --- | --- | --- |
| diagnostics | `getSemanticDiagnostics` + `getSyntacticDiagnostics` | 转换 code/category/message/line-character；去重；映射回模板 | 两者都成功才 `Supported`；部分失败 `Partial`；全失败 `Unsupported`/`Unavailable` |
| hover/type | `getTypeAtPosition` + `typeToString` + symbol/docs/tags + declaration AST | 类型、文档、标签、声明范围 | 无类型 `Missing`；`Any` 可显示字面 `any`；解码失败 `Partial` |
| definition | `getSymbolAtPosition` + `getReferencedSymbolsForNode` + `getSourceFile` | handle→同快照 AST→token 范围→`SourceLocation` | 无直接 API；候选无法映射 `Partial`；不得空+`Supported` 掩盖缺证 |
| references | `getReferencedSymbolsForNode` + `getSourceFile` | 保留 handle/组；范围解码尚未接入 | 当前非空结果显式 `Partial`，不是可发布的编辑证据 |
| completion | `getCompletionsAtPosition` + 可选 `prepareAutoImports` | 条目 name/kind/sortText/detail/insert/filter；无 textEdit | 详情/自动导入降级；非空结果 `Partial`；截断 `Partial` |
| symbols | `getSymbolAtPosition`/`getSymbolOfNode` + declarations/reference | 名称、flags、声明、引用；绑定 epoch | `null` 为“无符号”；解码失败 `Partial` |
| document symbols | `getSourceFile` AST | 脚本文档大纲树 | 空 + `Supported` 才表示文档没有声明 |
| rename | 不调用原生 rename | LSP 语义层 + `EditPlan` | 只做可证明安全的 WXML 目标；见 §8.8 |

### 8.3 Diagnostics

- 请求当前文件的语义 + 语法诊断；`getBindDiagnostics`、`getSuggestionDiagnostics`、`getProgramDiagnostics` 不作为必需来源。
- 每条诊断取 `startPosition/endPosition`（零基行 + UTF-16 列）转换为 `SourceRange`；用 `pos/end` 交叉校验，不一致则丢弃该条并 `Partial`。
- category 映射：`Error -> Error`、`Warning -> Warning`、`Suggestion -> Hint`、`Message -> Information`；未知 category 标记 `Partial`，不得默认 Error。
- `code` 以十进制字符串保存；没有 code 时可省略。message 保留原文；**没有通用诊断消息长度预算**。本地组件绑定诊断的契约消息在展示前截断到 512 字节（UTF-8 边界），这是该路径的专门约束。
- 同一文件 semantic/syntactic 完全重复（同 range/code/message）去重；不同来源不得随意覆盖。
- 单文件诊断最多保留 `MAX_DIAGNOSTICS = 1000` 条；超限截断并 `Partial`，不得静默丢弃。
- 文件集合必须来自请求/当前项目；其他文件的诊断（如外部声明）排除并 `Partial`。shadow 诊断按同一映射规则回原 WXML/脚本，不重复发布基线脚本诊断。

### 8.4 Type / Symbol / Documentation

- `getTypeAtPosition` 返回 `null`：该位置无类型，映射为 `TypeQuality::Missing` + 空类型，不是错误，也不是 `error` 类型。
- `typeToString` 失败/空：`Unknown`；`intrinsicName == "error"` 或非法位置形态：`Unknown`。
- `intrinsicName == "any"` 或文本精确为 `any`：`Any`。LSP 可以显示字面 `any`，但不得把它当已知成员类型；事件处理函数路径对没有 symbol/声明证据的 `any` 不当作可渲染内容。
- 类型块只在 `Known` 或可证明的字面 `any` 时渲染；`Unknown`/`Missing` 不渲染具体类型块。
- 其余成功文本：`Known`；保留重载文本（`{ (...); (...) }`），不要自行拆分或只取一个签名。
- `getSymbolAtPosition` 的 name/flags/declarations/reference 组成内部 `NativeSymbolDto`；`declarations`/`reference` 必须是已绑定 epoch 的 handle；`null` 表示无符号。
- 文档：`getDocumentationComment` 保留换行；`getJsDocTags` 的每个 `{name, text?}` 渲染为 `@name text` 行；`@deprecated` 必须保留并可见。文档/标签为空时省略对应段落，不输出“无文档”。
- 文档调用失败但类型成功：hover `Partial`（或 type-only），不得伪造文档；符号查询仍可用。

### 8.5 Definition

组合顺序：

1. 对位置取 `getSymbolAtPosition`。`null` → 无符号，返回空 `Supported`。
2. 若存在 symbol，取同一快照的最小 Identifier 节点作为 `getReferencedSymbolsForNode` 的 handle；找不到 Identifier → `Partial`（有符号但不能证明无定义），不是空 `Supported`。
3. 收集每个 group 的 `definition` handle；import alias 场景保留 target 与 alias 候选，由 LSP 决定展示顺序。
4. 每个 handle 校验 §6.1 绑定 → 同一快照 `getSourceFile` → 按 index 取节点 → 取 identifier token 范围（裁剪 trivia）→ 映射为 `SourceLocation`。
5. 去重 `(file, range)`；最多 `MAX_DEFINITION_TARGETS = 16` 个，超限截断并 `Partial`。
6. 状态：
   - 所有候选成功映射且无截断 → `Supported`；
   - 任一候选因项目外、资产不可表示、节点缺失、handle stale、范围非法而丢弃 → `Partial`，保留已成功位置；
   - 组合调用失败且没有可用候选 → `Unsupported`/`Unavailable`，不得空+`Supported`；
   - 所有候选解码失败 → `Partial`/`Unsupported`，不得当作“无定义”。

限制：不得把 declaration 的 `pos` 直接发布；不得在 shadow 文件上返回未映射的范围；生成声明的导航必须走显式关系。

### 8.6 References 与 alias

- `getReferencedSymbolsForNode` 返回 handle，不是 source range。当前 `wxml-backend-tsrs` 的引用映射只保留 group/handle 证据，非空结果显式 `Partial`；它还不能作为可发布的 definition/rename 范围。
- 每个 handle 必须按 §6.1 校验；去重按 `(file, range)`，保留 alias/target 信息。
- 引用上限为 `MAX_REFERENCE_HANDLES = 1000`；超限截断并 `Partial`。截断结果不得用于重命名。
- 空引用 + symbol 非 null 不自动等于“零引用”；完整性未证明时保持 `Partial`。
- 不得把字符串/注释中的文本当引用；不得用文本搜索补足。

### 8.7 Completion

- 使用 `getCompletionsAtPosition`。适配层先尝试 `prepare_auto_imports(script_path)`：
  - 准备成功：发送 `includeSymbol:true`，得到 symbol 证据；
  - 准备不可用/不支持：不发送 `includeSymbol:true`，保持补全可用并把原因并入 `Partial`。
  - **不得**在未准备的快照上发送 `includeSymbol:true`，引擎会拒绝整个请求。
- 条目保留 `name`、原生 `kind`、`sortText`、可选 `insertText`/`filterText`/`detail`/`labelDetails.detail`。`insertText` 只在纯文本时发布；snippet 形状回退到 label 并标记 `InsertTextFormatUnknown`。
- 没有可靠的 auto-import `origin`/`source`、没有 `textEdit`/`additionalTextEdits`、没有 `data`；不得伪造 import 语句或自动导入标记。`getCompletionEntryDetails` 不存在；不得用 `getTypeOfSymbol`/文档拼出“详情”冒充原生详情。
- 原生 `null` → 空 `Partial`（`NoResponse`），不是“没有补全”；显式空数组 → 空 `Supported`；非空结果因缺少文档/自动导入来源保持 `Partial`。
- 最多 `MAX_COMPLETION_ITEMS = 256` 条；超限截断并 `Partial`；保留原生顺序和 `sortText`，不额外排序掩盖缺失。
- 位置在 `.`、EOF、非法边界可能返回全局补全或 `null`；无法确认成员上下文时不得把全局条目当成员补全。

### 8.8 Rename

- 原生没有 rename 方法；`getRenameInfo`/`getEditsForRename`/`rename` 都不在白名单。重命名由 `wxml-lsp` 的语义层构造，不依赖原生 references 的未解码范围。
- 只接受可证明安全的 WXML 目标：已打开模板中的数据路径、脚本组件属性、静态事件处理函数、`wx:for` item/index、`class` 值、静态 `template` 名称。
- 普通/自定义标签名拒绝；`.d.ts`/`.d.mts`/`.d.cts` 永不编辑；`components.d.ts` 可作为属性事实来源但不能重命名。
- 必须有完整的符号/引用或静态出现证据；alias/target 唯一；新名称与字段、循环局部、WXS 模块、组件模板作用域或类名冲突时拒绝。
- 编辑由 `EditPlan` 校验路径、版本、范围、`expected_text`、重叠和 UTF-16 转换；任一失败返回零编辑，不做部分修改。
- **没有“生成编辑后在隔离快照重新 parse/typecheck”这一步**；当前安全保证来自完整证据收集与 `EditPlan` 校验，不是完整重构式复检。该能力也不覆盖所有组件声明或所有跨视图引用。
- 最终只返回 `WorkspaceEdit.documentChanges`，不写磁盘；客户端不支持 `documentChanges` 时拒绝请求。

## 9. 错误、取消、deadline 与降级

### 9.1 错误映射

内部 `NativeError` 的类别（名称以代码为准）：

| 类别 | 示例 | 边界映射 |
| --- | --- | --- |
| `Unavailable { EngineVersionMismatch }` | `tsrs_core::version()` 与固定值不符 | `Unavailable` |
| `Unavailable { EngineStartupPanic }` | `Session::new` panic | `Unavailable` |
| `Unavailable { ApiMismatch }` | 探针缺少必需方法 | `Unavailable` |
| `Unavailable { MissingAsset }` | typings/内嵌标准库缺失 | `Unavailable` |
| `Unavailable { SessionPoisoned }` | 会话已被标记不可用或已关闭 | `Unavailable`；适配层丢弃会话并按 §5.3 重建 |
| `Unavailable { VfsConfinement }` | 启动期 VFS 围栏检查失败 | `Unavailable` |
| `Protocol` | AST schema 违规、响应形状非法 | `Protocol`（细节不入用户可见文本） |
| `Unsupported { method }` | 运行期方法不可用 | 当前请求降级；探针失败为 `Unavailable` |
| `InvalidParams`/`InvalidHandle` | 位置越界、URI 非法、handle 畸形 | `Protocol`/`Unavailable`，不重试 |
| `StaleHandle`/`StaleSnapshot`/`NoSnapshot` | 跨 epoch、已 release、未打开 | `Cancelled`/`Unavailable`；不发布旧结果 |
| `EngineError { code }` | 引擎返回其它 client error | `Internal`，只保留 code，消息清洗 |
| `InputTooLarge` | 快照请求超过 32 MiB 预算 | `Unavailable`（本地拒绝，不构建大请求） |
| `Timeout` | 调用前 deadline 已过 | `Timeout`；底层计算不可取消 |

`BackendError::Protocol`/`Internal` 的文本只用于日志和诊断，不得包含内部栈、绝对路径、环境变量或秘密。

### 9.2 deadline 的真实语义

- native 配置：普通请求 15 s、快照创建/更新 30 s、启动探针 10 s、lease/snapshot release 1 s（[`config.rs`](../../crates/wxml-backend-tsrs/src/config.rs)、[`native/inproc.rs`](../../crates/wxml-backend-tsrs/src/native/inproc.rs)）。
- `InProcTransport::request` 只在调用**前**检查 deadline 是否过期；`Session::handle_request` 是同步调用，启动后没有中断通道。
- 因此 deadline 不是等待上限，也不保证在 15 s/30 s 内返回；一次已经开始的同步调用可以超过配置值。不得把它描述成“超时后停止计算”。
- 调度器负责并发准入和排队；native 会话本身没有独立的 in-flight 上限声明。

### 9.3 取消

- **排队中**：从队列移除，结果标 `Cancelled`，不送入引擎。
- **运行中**：没有服务端取消能力；结果到达后丢弃并返回 `Cancelled`，不发布部分结果。
- 取消发生在快照更新期间：更新本身不可取消；完成后 epoch 提升，旧请求结果丢弃。
- 会话不可用时排队/在途请求以 `Cancelled`/`Unavailable` 结束。

### 9.4 会话降级

- 单请求的普通错误只让该请求失败；引擎 panic/编码失败映射为内部错误时，传输被 poison，适配层丢弃会话。
- 死会话按 §5.3 处理：统一会话查询路径对当前请求最多重建并重试一次；第二次失败通过 `fail_session` 推进 epoch 并丢弃会话，只让当前请求失败。`diagnostics`/`script_diagnostics` 不在同一请求内重试。
- 没有进程级重启、退避或健康检查杀死；代码没有持久终态，后续请求仍可按需重建（同样每次最多一次）。
- 不重放旧 handle/旧 symbol descriptor；不把旧结果映射到新 AST。

## 10. 安全验收

以下是不变量，评审必须逐项核对；任何一项不满足都阻塞变更：

| 编号 | 不变量 | 验收 |
| --- | --- | --- |
| S1 | 生产代码只发送白名单方法 | `NativeMethod` 枚举 + `ALLOWED_METHODS` 同步测试；传输层字符串入口没有运行时校验，属已知缺口 |
| S2 | 无宿主写：emit/build/clean/profile/heap 不可达；FS 写/追加/删除/chtimes 一律返回错误 | 沙箱树前后哈希、canary、拒绝方法测试、FS 写拒绝测试 |
| S3 | 读路径只查虚拟表与受信任资产根；越界返回否定结果；无宿主回退 | 越界/canary/symlink 负例；**当前没有读取日志**，审计靠测试 |
| S4 | 不执行外部代码 | 不加载 tsconfig `plugins`、不读宿主 `node_modules`、无 Node 子进程、无外部引擎覆盖变量 |
| S5 | 只读不是“noop 让写看起来成功” | 文档/测试显式断言写入口返回错误；即使 noEmit 也不暴露 emit 方法 |
| S6 | 路径限制：绝对、规范化、无 `..`/NUL、按组件前缀、资产根只读 | 路径表驱动负例 + 恶意 URI |
| S7 | 查询级上限明确且可观测 | AST nodes/strings 各 1,000,000；快照账本 8；诊断 1000；补全 256；请求快照 32 MiB；组件绑定消息 512 字节。**不承诺所有资源配置有界，也不承诺绝不 OOM** |
| S8 | 错误/日志不泄漏路径、栈、秘密 | panic/错误文本清洗测试；DTO 无绝对路径 |
| S9 | 不在探针前发送项目内容 | 启动顺序测试；失败不读项目 |
| S10 | 安全失败是显式失败，不是空成功/静默降级 | 所有拒绝路径返回 typed 状态/错误 |

补充说明：

- hover 的声明扫描只按“不同目标”封顶 16（`MAX_HOVER_DECLARATIONS`）。重复声明不会填满目标集合，因此可以扫描引擎返回的全部重复 handle；现有测试用 10,000 个重复项验证这一点。这不是 hover“扫描截断”预算。
- 快照账本 8 是未确认释放的引擎快照上限，不是多项目 LRU/grace pool。
- 请求快照 32 MiB 是序列化请求体的上限；`check_request_budget` 在构建 body 前用保守上界拒绝过大输入。

读路径审计至少覆盖：项目 `tsconfig`/`extends`/`paths`/`typeRoots`、`package.json`、模块解析、`node_modules`（仅快照）、内嵌标准库、typings、`realpath`、目录枚举、符号链接、`..`/编码路径、canary 文件。写路径审计至少覆盖 FS trait 的 `write_file`/`append_file`/`remove`/`chtimes` 以及方法白名单中的 emit/build/clean/profile 族。

## 11. 测试挂点与证据边界

- native 版本/探针/白名单/VFS：`crates/wxml-backend-tsrs/src/native/` 下 `inproc`/`vfs`/`ast`/`handles`/`session` 的单元测试，以及 `tests/asset_loader.rs`、`tests/type_assets.rs`。
- 会话/epoch/UTF/查询：`crates/wxml-backend-tsrs/tests/real_engine.rs`、`f1_hover.rs`、`shadow_*.rs`、`script_*.rs`。
- LSP 端到端：`crates/wxml-lsp/tests/lsp.rs`、`native_lsp_document_symbols.rs`、`project_host.rs`、`semantic_surface.rs`。
- 真实类型门禁：`TSRS_TYPINGS_ROOT` 提供 typings 根；`WXML_REQUIRE_REAL_ENGINE=1` 把缺失门禁变成硬失败。未启用时真实类型用例跳过。
- 测试通过不降低任何合同要求，也不证明未覆盖的平台/输入。平台结论仍只在 Linux x86_64。

## 12. 开放问题与已知限制

- **不可取消**：运行中的同步类型计算没有取消通道；deadline 只在调用前检查。
- **无进程级隔离**：OOM、abort、栈溢出会终止语言服务器；单请求 panic 按 §5.3 的会话重建路径处理。
- **会话降级/重建**：每次请求最多对死会话重建并重试一次；第二次失败调用 `fail_session` 推进 epoch 并丢弃会话，只让当前请求失败。代码没有持久终态，后续请求仍会按需重建；没有通用重试或退避。
- **全局 worker/arena 不回收**：同一时刻只有一个活动 Session；`Session::close` 不 join 全局线程，重建不会回收已创建的全局 worker/arena 资源，需持续测量 RSS 与重建成本。
- **prerelease API**：`7.1.0-beta.tsrs4` 是精确 pin 的预发布版本；升级必须重跑探针与真实类型用例。
- **字段级差异**：hover/补全/引用/文档仍可能与旧 tsgo 行为有可见差异；差异必须记录，不得静默。
- **引用范围未解码**：native references 目前只保留 handle/group，非空结果为 `Partial`，不能作为可发布的 definition/rename 范围。
- **重命名范围有限**：只覆盖可证明安全的 WXML 目标；标签名、`.d.ts` 声明和无法证明完整的跨视图引用不在范围内；没有隔离快照复检。
- **补全信息有限**：没有 per-entry 文档、没有可靠的 auto-import origin 或 import edit；非空 native 补全保持 `Partial`。
- **传输层白名单缺口**：`InProcTransport::request` 接受任意字符串；生产约束来自 `NativeMethod` 枚举，不是运行时检查。
- **平台**：只在 Linux x86_64 验证；大小写、路径分隔符与 ABI 结论不外推到其它平台。
- **性能**：没有大项目语料；冷启动与首查成本需要单独测量，不能引用旧基准数字。
