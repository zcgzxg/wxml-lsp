# Proposal

## Why

当前类型后端依赖外部 `engine/lib/tsc` 子进程：安装包需要额外分发约 28 MB 的引擎资产，启动时要校验可执行文件哈希、跑 `--version`、做 JSON-RPC 握手，并在运行期维护帧协议、回调泵、超时和重启逻辑。`tsrs` 已提供与 tsgo 同协议、同 AST schema 的 `tsrs_api::Session`，可以直接嵌入 LSP 进程，在保留现有查询语义的同时去掉子进程和整套传输层。

## What Changes

- 用 `tsrs_api::Session` 替换 `NativeSession` 的 `EngineSupervisor` + `RpcDispatch` + Content-Length 帧传输；查询、快照、AST 解码、handle/lease 逻辑保持不变，所有请求改为进程内 `Session::handle_request`。
- 新增 `tsrs_vfs::FS` 实现，直接对接现有 `VirtualFileSystem`：只读、路径限定、真实 `stat`/目录枚举/`realpath`，写操作一律拒绝，禁止回退宿主文件系统。
- 类型库改用 `tsrs_vfs` 内嵌的 `lib.*.d.ts`；安装树只需要 `typings`，不再需要 `engine/lib/tsc`、引擎 tarball 哈希和 `assets.lock.json` 的 engine 段。
- 启动校验改为：精确 pin 的 `tsrs_api` 版本、`tsrs_core::version()`、AST protocol v9/header 64/stride 28、`initialize` 和现有能力探针；删除可执行文件校验与 `--version` 探针。
- **BREAKING** 不再支持外部引擎覆盖：`TSGO_BIN`、`TSGO_ASSET_ROOT` 及子进程重启/崩溃隔离语义移除；`TSGO_TYPINGS_ROOT`/`WXML_REQUIRE_REAL_ENGINE` 的语义改为进程内真实类型门禁。
- **BREAKING** 安装布局变更：不再安装 `engine/` 与 `scripts/install-native.sh` 的引擎部分；升级后旧布局中的 `lib/tsc` 会被忽略。
- 引入 `tsrs_api`/`tsrs_core`/`tsrs_vfs` 精确版本依赖；workspace MSRV 从 1.85 提升到 1.100，并同步 `deny.toml`、合同文档与 ADR。
- 删除或收缩 `supervisor.rs`、`framing.rs`、`rpc.rs`、`sandbox.rs` 及对应测试；真实引擎测试改为进程内运行。

## Capabilities

### New Capabilities

- `type-engine-runtime`: 定义进程内类型引擎的生命周期、版本与 AST schema 固定、内嵌标准库、只读虚拟文件系统约束、启动能力探针、失败降级与关闭行为。

### Modified Capabilities

- `query-benchmarks`: 真实类型查询不再依赖外部 tsgo 引擎资产；基准的“引擎缺失跳过”条件改为进程内引擎初始化失败或未启用真实类型门禁。

## Impact

- **代码**：`crates/wxml-backend-tsgo` 的 `native/{session,vfs,mod}.rs`、新增 `native/inproc.rs`，删除 `native/{supervisor,framing,rpc}.rs` 及回调/传输相关测试；`crates/wxml-lsp` 的 `backend_tsgo/{engine,environment,sandbox,mod}.rs` 与安装发现逻辑。
- **依赖**：新增 `tsrs_api`、`tsrs_core`、`tsrs_vfs`（精确 prerelease pin），以及其 compiler/checker/project/ICU 依赖图；根 `Cargo.toml` MSRV 1.100，`deny.toml` 需复核重复版本与许可证。
- **资产与安装**：移除 engine tarball、`lib/tsc`、engine 段校验；保留并校验 `miniprogram-api-typings`。
- **文档与合同**：`docs/reference/tsgo-native-contract.md` 的进程/帧/回调/重启章节需重写或废弃，新增 ADR 记录进程内嵌入与故障隔离取舍；README/AGENTS 的安装说明同步。
- **测试与 CI**：真实引擎门禁、性能基准、CLI 安装发现用例、缺失资产用例均需改为进程内语义；默认 `cargo test` 仍不要求外部引擎。
