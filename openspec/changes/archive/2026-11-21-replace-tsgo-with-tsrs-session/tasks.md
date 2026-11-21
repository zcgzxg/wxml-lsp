# Tasks

## 1. 依赖与工具链

- [x] 1.1 在 `crates/wxml-backend-tsgo/Cargo.toml` 添加 `tsrs_api`、`tsrs_core`、`tsrs_vfs` 的精确 pin，并确认 `cargo metadata` 解析到 `7.1.0-beta.tsrs4`；验证：`cargo check -p wxml-backend-tsgo` 通过。
- [x] 1.2 将根 `Cargo.toml` 的 `rust-version` 提升到 `1.100`，同步 CI/README 的工具链说明；验证：`cargo check --workspace` 在 1.100 工具链通过。
- [x] 1.3 运行 `cargo deny --offline check`，按实际依赖调整 `deny.toml` 的重复版本允许族与许可证说明；验证：`cargo deny --offline check` 通过且变更仅包含必要条目。
- [x] 1.4 在构建检查中断言未设置编译期 `TSRS_TS_VERSION`，并记录到 CI 或测试；验证：设置该变量时检查失败并给出明确信息。

## 2. 进程内传输与虚拟文件系统

- [x] 2.1 新增 `native/inproc.rs`，实现 `VfsHandle` 的 `tsrs_vfs::FS`：读取只查虚拟表、写操作返回错误、`stat`/目录枚举/`realpath` 与 VFS 一致；验证：单元测试覆盖命中、缺失、路径穿越、写拒绝和目录枚举。
- [x] 2.2 实现 `InProcTransport`：固定版本校验、`catch_unwind` 包裹 `Session::new`、`handle_request` 调用、死锁/中毒状态和 `close()`；验证：测试覆盖正常请求、版本不匹配、构造 panic、关闭幂等。
- [x] 2.3 在 `NativeSession` 中用 `transport`/`fs` 替换 `supervisor`/`dispatch`，新增 `set_vfs` 并替换所有 VFS 赋值点；验证：`cargo check -p wxml-backend-tsgo` 通过，且 open/update 测试能安装和切换 VFS。
- [x] 2.4 将 `call()` 改为进程内请求，移除 `serve_callback`/`send_rejected_callback`/`process()`，保留 `Deadline` 作为调用方等待上限；验证：initialize、探针、诊断、hover、补全、引用、文档查询的现有单测通过。
- [x] 2.5 映射 `ApiError` 到 `NativeError`，更新 `is_alive`/`stderr_tail`/`shutdown`/`poison_session`；验证：新增测试覆盖 unsupported、stale snapshot、stale handle、internal error 和 panic 后的能力降级。

## 3. LSP 安装层与运行环境

- [x] 3.1 让 `TypeAssets`/`Environment` 支持内嵌标准库模式，只校验并挂载 typings；验证：无 `engine/lib` 的安装目录能加载环境并通过现有 environment 测试。
- [x] 3.2 简化 `engine.rs` 发现逻辑为 typings-only，移除 engine 可执行文件校验；验证：现有发现测试更新后通过，缺少 typings 仍报明确错误。
- [x] 3.3 删除 `SandboxSessionFactory`/`sandbox.rs` 的进程沙箱职责，改用虚拟 cwd；验证：`cargo test -p wxml-lsp backend_tsgo` 通过。
- [x] 3.4 更新 `with_install`/`with_host`/CLI 启动路径，去掉 `TSGO_BIN`/`TSGO_ASSET_ROOT`；验证：CLI 测试覆盖旧布局被忽略、typings 缺失、正常启动三种情况。

## 4. 删除旧传输与引擎资产

- [x] 4.1 删除 `native/supervisor.rs`、`native/framing.rs`、`native/rpc.rs` 及对应测试/导出；验证：`cargo check --workspace --all-targets` 无残留引用。
- [x] 4.2 删除 `assets.lock.json` 的 engine 段、`scripts/install-native.sh` 的引擎下载/校验部分和 engine 资产测试；验证：安装脚本只依赖 `wxml-lsp` 与 typings 即可完成布局。
- [x] 4.3 清理 `UnavailableReason`/`NativeError` 中仅属于子进程的变体和错误映射；验证：`rg` 无 `ProcessExited|HandshakeFailed|ArtifactMismatch` 的生产引用，测试通过。

## 5. 测试、基准与文档

- [x] 5.1 将真实引擎门禁改为进程内语义：`TSGO_TYPINGS_ROOT` + `WXML_REQUIRE_REAL_ENGINE=1`，默认测试不要求外部引擎；验证：无门禁时真实类型用例跳过，有门禁时运行。
- [x] 5.2 以现有 tsgo golden 断言覆盖 Checker 查询（类型字符串、hover 文档、引用、补全、诊断、标准库成员）；没有保留可运行的旧 tsgo 后端，因此未做 live 双引擎差分；已知差异为零（`text.length` 曾因标准库未挂载为 `any`，已修复并加断言）。
- [x] 5.3 更新 `perf_budget`/基准用例，去掉帧/管道指标，保留启动、首次查询、预热查询和并发上限；验证：基准可过滤运行且不依赖外部引擎进程。
- [x] 5.4 更新 `docs/reference/tsgo-native-contract.md`：保留 VFS/handle/UTF-16 约束，重写或废弃进程、帧、回调、重启章节；新增 ADR 记录进程内嵌入与不可取消的取舍；验证：文档中的命令/路径与实现一致。
- [x] 5.5 更新 README/AGENTS 的安装、验证和故障说明，删除 engine tarball 与 Node 相关描述；验证：按文档从零安装后 `wxml-lsp --stdio` 可启动并返回类型查询。

## 6. 最终集成验证

- [x] 6.1 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`cargo doc --workspace --no-deps`、`cargo deny --offline check`；验证：全部命令一次通过。
- [x] 6.2 用真实 typings 跑一组端到端 LSP 会话：打开 fixture、hover、补全、引用、诊断、关闭；验证：无外部引擎进程、无宿主文件读取、关闭后无残留会话。
- [x] 6.3 运行 20 次 open-update-close soak（同一 `TypeAssets`），结束后 `session.is_alive()` 且类型环境仍有效（无 2304）；未引入 RSS/线程数采样工具，长期资源曲线仍需在真实编辑器会话中观察。

## Workflow follow-up

- 在项目评审通过后归档变更。
- 归档后运行 `openspec validate --specs` 并确认主 spec 的 Purpose 非 TBD。
