# Design

## Context

现有后端由 `NativeSession` 驱动一个 `lib/tsc --api --async` 子进程：`EngineSupervisor` 负责 spawn、stdout/stderr、超时与重启，`RpcDispatch`/`framing` 负责 JSON-RPC 帧与回调。查询、快照、AST 解码、handle/lease 逻辑都在 `NativeSession` 内，约 3,900 行。

约束保持不变：无 Node 运行时、Linux 优先、只读虚拟文件系统、AST protocol v9/header 64/node stride 28、API 位置 UTF-16、方法白名单、默认测试不要求真实引擎。`tsrs_api::Session` 提供同协议、同 AST schema 的进程内入口，并内嵌 TypeScript 标准库。

## Goals / Non-Goals

**Goals:**

- 用进程内 `Session` 替换子进程传输，保持现有查询结果与错误语义。
- 删除引擎可执行文件、tarball 校验、帧协议、回调泵和进程重启逻辑。
- 保持只读 VFS 边界，禁止类型引擎读取或写入宿主文件。
- 安装布局只保留 typings，语言服务器仍是单一可执行文件。

**Non-Goals:**

- 不直接链接 `tsrs_compiler`/`tsrs_checker` 内部 API；`Session` 是唯一集成边界。
- 不改变 WXML 查询功能、类型语义或 LSP 协议。
- 不承诺 tsrs 官方基准中的 2×/8× 数字；PGO/BOLT 不在本次范围。
- 不扩展到非 Linux 平台验证。

## Decisions

### 1. 复用 `NativeSession`，只替换传输层

新增 `native/inproc.rs`，提供 `InProcTransport` 和 `VfsHandle`；`NativeSession` 保留 `engine`、`vfs`、`epoch`、`ledger`、`auto_imports` 和全部查询方法，把 `supervisor`/`dispatch` 字段替换为 `transport`/`fs`。所有 `call()` 改为 `Session::handle_request`。

理由：查询与句柄逻辑是主要资产，重写会重复 hover/definition/completion/document symbol 的边界处理。替代方案“新写 `InProcSession` 实现 `QuerySession`”需要复制这些逻辑，放弃。

### 2. `binary_responses=false`，保留现有 base64 解码

首版让 `getSourceFile` 继续返回 `{"data":"<base64>"}`，避免同时改 AST 解码和空 Binary 语义；`Response::Binary` 视为协议错误。后续可单独切换二进制 AST，不影响本次结构。

理由：减少一次变更的失败面；`Session` 的 JSON 参数/结果仍与现有 wire 结构一致。

### 3. `VfsHandle` 实现 `tsrs_vfs::FS`，并包一层 bundled libs

`VfsHandle` 持有 `Arc<RwLock<Option<VirtualFileSystem>>>`，在 open/update 时安装快照 VFS。`Session` 的 FS 使用 `bundled::wrap_fs(handle)`，`default_library_path` 使用 `bundled::lib_path()`，不调用会读 `TSRS_LIB_PATH` 的 `default_lib_path()`。

FS 行为：

- `read_file`/`file_exists`/`directory_exists`/`get_accessible_entries`/`stat` 只查虚拟表。
- `stat` 返回真实的文件/目录 mode、大小和固定 mtime，供模块解析使用。
- `write_file`/`append_file`/`remove`/`chtimes` 一律返回错误。
- `realpath` 返回规范化后的虚拟路径，不回退宿主。
- `use_case_sensitive_file_names()` 返回 `true`。

`tsrs_api::Session::new` 无 `Result`，构造过程用 `catch_unwind` 包住；失败映射为 `Unavailable`。

### 4. 版本与协议固定

`tsrs_api`/`tsrs_core`/`tsrs_vfs` 使用 `=7.1.0-beta.tsrs4` 精确 pin。启动时校验：

- `tsrs_core::version() == "7.1.0-beta"`；
- AST protocol/header/stride 与 `PinnedEngine` 常量一致；
- `initialize` 返回大小写敏感和 `currentDirectory`；
- 现有能力探针全部通过后才标记可用。

CI 必须断言未设置编译期 `TSRS_TS_VERSION`，避免版本字符串被构建环境改写。

### 5. 失败模型：按请求隔离，不重启进程

`Session::handle_request` 自带 `catch_unwind`；单次 panic 转为 `NativeError` 并返回给该请求。若错误表明会话已中毒或语言服务器状态不可恢复，`is_alive` 置 false、类型能力标记为 `SessionPoisoned`，LSP 继续提供非类型功能。没有 SIGKILL/重启；调用方 deadline 仍然保留，只用于停止等待，不能取消底层计算。

理由：进程内没有可靠的强制终止手段。与其假装能 kill，不如把“不可恢复”显式暴露为能力降级。

### 6. 安装层改为 typings-only

`Environment`/`TypeAssets` 增加“内嵌标准库”模式：不再从 `engine/lib` 加载 `lib.*.d.ts`，只校验并挂载 `typings`。`engine.rs` 的发现结果只保留 `typings_root`；`SandboxSessionFactory` 删除，`SessionOptions` 的 `sandbox_root` 继续作为虚拟 cwd 使用，`executable` 保留字段但不再参与启动校验（避免一次改动波及所有测试构造）。

`scripts/install-native.sh` 删除引擎下载、解包和 `lib/tsc` 校验，只保留 `wxml-lsp` 与 typings 布局。

### 7. 并发与栈

`NativeSession` 由单个查询会话独占，`Session` 自身是 `Send + Sync`；LSP 适配层继续限制并发请求数。tsrs 的重型解析/checker 工作在其 512 MiB 栈的后台 worker 上运行；`createSnapshot` 仍在调用线程，需在真实项目 fixture 上验证栈深度，必要时把 open/update 放到专用大栈线程。

理由：先保持现有调用模型，避免把查询接口整体改成消息通道；栈风险用测试和后续隔离兜底。

### 8. 依赖与策略

根 `Cargo.toml` 的 `rust-version` 提升到 `1.100`。新增 tsrs crate 后运行 `cargo deny`，若 `multiple-versions` 因 `hashbrown`/ICU 等触发，按实际依赖在 `deny.toml` 增加最小允许族；许可证继续使用现有 allowlist，并随二进制分发 Apache-2.0/Unicode-3.0 NOTICE。

## Risks / Trade-offs

- **无超时取消**：请求超时只能放弃等待，计算仍占用 checker → 在 LSP 层限制并发，文档明确不保证可取消。
- **OOM/abort/栈溢出不可恢复**：进程内没有隔离 → 长会话 soak 测试；必要时未来改回 helper 进程，本次不做。
- **Mutex 中毒**：持锁 panic 后会话可能持续失败 → 映射为 `SessionPoisoned`，停止使用该会话，不静默重试。
- **全局 worker/arena 不回收**：`Session::close` 不 join 全局线程 → 单进程只创建一个 Session，避免频繁重建；测量 RSS。
- **FS 语义漂移**：现有回调故意答 missing/空目录，进程内必须真实 stat/entries → 差分测试覆盖模块解析、typings、路径穿越和写拒绝。
- **Checker 方法为 Partial**：hover/补全/引用/文档格式可能与 tsgo 有差异 → 用现有 fixtures 做字段级差分，用户可见差异必须记录。
- **供应链变化**：从单个 npm tarball 变成约 20 个 crate + ICU → `Cargo.lock`、精确 pin、`cargo deny`、`cargo vendor`（可选）与 NOTICE。
- **二进制体积/构建时间**：内嵌 4 MB libs 与 ICU → 测量 release 体积和干净构建时间；同时删除 28 MB engine，安装总量预计下降。
- **tsrs prerelease API 不稳定**：精确 pin，升级必须重新跑能力探针和差分。

## Migration Plan

1. 添加精确依赖与 MSRV，新增 `native/inproc.rs`、`VfsHandle`、`InProcTransport`。
2. 改造 `NativeSession`：替换传输字段、接入 `set_vfs`、错误映射和生命周期方法。
3. 改造 LSP 安装层：typings-only 环境、移除 engine 发现与 sandbox 工厂。
4. 删除 `supervisor`/`framing`/`rpc`/`sandbox` 及引擎资产、安装脚本中的引擎部分。
5. 更新合同、ADR、README/AGENTS、测试与基准门禁。
6. 全部完成后一次性运行 fmt/clippy/test/doc/deny；真实类型测试需要 `TSGO_TYPINGS_ROOT` 与 `WXML_REQUIRE_REAL_ENGINE=1`。

回滚策略：不保留运行时双后端；本次变更以单个提交/revert 回退到子进程实现。

## Open Questions

无。类型查询的格式差异按差分结果记录为已知差异，不阻塞本次结构替换。
