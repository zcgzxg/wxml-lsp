# ADR-0012: 进程内 tsrs 类型引擎会话与故障隔离

**Date**: 2026-11-21

**Status**: accepted（架构约定；实现与验收进度以代码、测试和 openspec 变更记录为准）

**Deciders**: 项目维护者

**取代**：已删除的原 ADR-0008（Rust + tsgo 类型后端，移除 Node 依赖；内容并入本文，编号不复用）中「独立 tsgo 子进程、引擎发行物摘要校验、stdio JSON-RPC 传输、回调泵、进程重启与退避」部分。以下决定继续有效：Rust 侧拥有 WXML/小程序语义与源码映射；不自行实现 TypeScript 类型系统；生产路径无 Node 运行时；方法白名单、只读虚拟文件系统与保守 rename 规则。ADR-0009 的瘦身边界不变。

## Context

此前的方案采用的是 **Rust 语义适配器 + 独立 tsgo 子进程**：安装包分发约 28 MB 的 `engine/lib/tsc` 与标准库，启动时校验可执行文件/`package.json` 的摘要、执行 `--version`、跑 `initialize` 与能力探针，运行期维护 Content-Length 帧、服务端回调泵、stdout/stderr 排空、超时和重启退避。

`tsrs` 提供了与 tsgo 同协议、同 AST schema 的 `tsrs_api::Session`：查询方法、`initialize` 形状、AST protocol v9/header 64/node stride 28 与现有实现一致，并且通过 `tsrs_vfs` 内嵌 TypeScript 标准库。把 `Session` 直接嵌入语言服务器进程，可以在保留全部查询语义的同时删除整个子进程传输层和引擎资产分发。

同时有三条既定约束决定了本次形态：

- 生产路径仍然没有 Node 运行时，也不能把 TypeScript 包当作运行时依赖（无 Node 运行时是既有结论）。
- 语言服务对用户工作区只读：业务文件不落盘，类型引擎不得读取或写入宿主文件。
- 默认 `cargo test` 不要求真实类型引擎；真实类型行为必须由显式门禁证明，不能用默认绿代替。

## Decision

### 1. 唯一集成边界是 `tsrs_api::Session`

- 类型查询、快照、AST 解码、handle/lease 逻辑保留在 `NativeSession` 内，只把 `EngineSupervisor` + `RpcDispatch` + 帧传输替换为 `InProcTransport`；所有请求改为进程内 `Session::handle_request`。
- 不直接依赖 `tsrs_compiler`/`tsrs_checker` 的内部 API；升级或替换引擎时以 `Session` 的请求/响应结构为合同面。
- 首版保留 `binary_responses=false` 的 base64 AST 返回，`Response::Binary` 视为协议错误；切换到二进制 AST 需要单独变更和差分验证。

理由：查询编排与句柄逻辑是主要资产，重写会复制 hover/definition/completion/document symbol 的边界处理；只替换传输层能把失败面限制在启动、错误映射和 FS 语义上。

### 2. 标准库内嵌，安装树只保留 typings

- `tsrs_vfs` 自带的 `lib.*.d.ts` 作为唯一标准库来源；不读取 `TSRS_LIB_PATH`，不读取安装目录中的 `engine/lib`。旧布局里的 `lib/tsc` 与 `lib*.d.ts` 升级后一律忽略。
- 小程序类型声明仍来自受信任资产 `typings/miniprogram-api-typings`，继续固定版本、摘要与许可证。
- `scripts/install-native.sh` 只组装 `<root>/bin/wxml-lsp` 与 `<root>/typings/miniprogram-api-typings`；不再下载引擎 tarball，不再校验 `lib/tsc`。

因此安装总量下降，语言服务器仍是单一可执行文件，运行时没有需要回收的外部进程。

### 3. 只读 VFS 由 `VfsHandle` 实现 `tsrs_vfs::FS`

- `VfsHandle` 持有当前快照的虚拟文件表；`read_file`、`file_exists`、`directory_exists`、`get_accessible_entries`、`stat`、`realpath` 只查虚拟表和受信任资产根，禁止回退宿主文件系统。
- `stat` 必须返回真实的文件/目录 mode、大小和固定 mtime，供模块解析使用；目录枚举返回虚拟表内容，`realpath` 返回规范化后的虚拟路径。
- `write_file`、`append_file`、`remove`、`chtimes` 一律返回错误；`use_case_sensitive_file_names()` 返回 `true`（仅 Linux 语义有证据）。
- 方法白名单仍是第一道防线；VFS 写拒绝是权限边界，不是“noop 让 emit 看起来成功”的缓解。

### 4. 版本、协议与启动探针在进程内固定

- `tsrs_api`、`tsrs_core`、`tsrs_vfs` 使用精确 pin `=7.1.0-beta.tsrs4`；升级必须作为独立变更，重跑能力探针与字段级差分。
- 启动时校验 `tsrs_core::version()` 等于 `7.1.0-beta`，AST protocol 为 v9、header 64 字节、node stride 28，且 `initialize` 返回大小写敏感与 `currentDirectory`；能力探针全部通过后才标记类型能力可用。任一不一致使类型能力不可用，不做“尽力兼容”。
- `Session::new` 不返回 `Result`，构造用 `catch_unwind` 包围；失败映射为 `Unavailable`，不得带着半初始化状态继续服务。
- CI 必须断言未设置编译期 `TSRS_TS_VERSION`，防止版本字符串被构建环境静默改写。

### 5. 失败模型：单请求隔离，不重启进程

- `Session::handle_request` 自带 panic 捕获，单次 panic 转为该请求的错误并返回；语言服务器继续处理其它请求。
- 若错误表明会话中毒或语言服务器状态不可恢复，`is_alive` 置 false，类型能力标记为 `SessionPoisoned`，LSP 继续提供非类型功能；不做静默重试，也不假装恢复。
- 进程内没有可靠的强制终止手段，因此**不实现超时取消、SIGKILL、进程重启与退避**。调用方 `Deadline` 只用于停止等待并返回 `Timeout`，底层计算不会被取消，迟到响应一律丢弃。
- LSP 适配层继续限制并发请求数；`Session` 自身是 `Send + Sync`，但一个语言服务器进程只创建一个会话，避免反复重建全局 worker/arena。
- OOM、abort、栈溢出属于进程级故障，无法隔离；用长会话 soak、RSS/线程数记录和 fixture 差分测试观察，未来若确需硬隔离再单独决策 helper 进程。

### 6. MSRV 与供应链

- workspace `rust-version` 从 `1.85` 提升到 `1.100`；`deny.toml` 随实际依赖复核重复版本族与许可证，二进制分发保留 Apache-2.0/Unicode-3.0 NOTICE。
- 供应链从“单个 npm tarball + 摘要校验”变为“Cargo 精确 pin 的多个 crate（含 compiler/checker/project/ICU 依赖图）”：依赖身份由 `Cargo.lock` 与精确版本锁定，`cargo deny` 是强制检查，`cargo vendor` 可选。
- 不再支持外部引擎覆盖：`TSRS_BIN`、`TSRS_ASSET_ROOT` 及子进程重启语义移除。`TSRS_TYPINGS_ROOT` 与 `WXML_REQUIRE_REAL_ENGINE=1` 名称保留，但语义改为“进程内真实类型门禁”：提供 typings 根并按需把缺失门禁升级为硬失败。

## Alternatives Considered

### 保留 tsgo 子进程

- **Pros**: 故障天然隔离，崩溃可重启；现有代码与证据链完整。
- **Cons**: 需要分发 28 MB 引擎与标准库，维护摘要校验、`--version` 探针、帧协议、回调泵、stderr 排空和退避重启；启动与 IPC 成本固定存在。
- **Why not**: 本次目标正是删除这层传输与资产；`tsrs` 已提供等价协议入口。

### 保留子进程，只把引擎换成 tsrs 可执行文件

- **Pros**: 保留硬隔离与重启能力，改动面小于进程内替换。
- **Cons**: 仍需定义帧、回调与版本探针，并额外维护一个自建可执行文件的分发与校验。
- **Why not**: 没有消除传输层，收益主要是资产体积；与“单可执行文件、无子进程”的目标冲突。

### 进程内会话 + 独立 helper 进程兜底

- **Pros**: 正常路径零 IPC，崩溃时可退回隔离执行。
- **Cons**: 两套生命周期、两套错误语义和双份测试；helper 仍需要帧协议。
- **Why not**: 本次不保留运行时双后端；先在单进程模型下把不可恢复显式降级，未来确有需要再单独引入。

### 直接链接 `tsrs_compiler`/`tsrs_checker`

- **Pros**: 可以绕开请求/响应编解码，理论上更省开销。
- **Cons**: 绑定内部 API 与类型结构，升级成本高，容易把引擎内部不变量泄漏进本项目。
- **Why not**: `Session` 已是稳定集成面，收益不足以抵消耦合。

### 继续用 Node/TypeScript 语言服务

- **Pros**: 生态成熟，QuickInfo/Definition/Rename 等接口齐全。
- **Cons**: 重新引入 Node 运行时与 npm 供应链，违背本 ADR 的无 Node 运行时结论。
- **Why not**: 已删除的 Node 路径不恢复。

## Consequences

### Positive

- 删除引擎 tarball、`lib/tsc`、摘要校验、`--version` 预检、Content-Length 帧、服务端回调泵和进程重启逻辑，安装树只剩二进制与 typings。
- 类型查询不再有子进程启动、握手和管道 IPC 成本；单可执行文件即可承载类型能力。
- 只读 VFS 变成引擎直接调用的 `FS` 实现：写入、追加、删除、修改时间在类型层面就没有成功路径，`appendFile`/`chtimes` 的历史残余风险随之消失。
- 标准库随 crate 内嵌，安装目录不可能出现“标准库版本与引擎不匹配”的组合。

### Negative

- 失去进程级隔离：panic 可以按请求捕获，但 OOM、abort、栈溢出会带走整个语言服务器。
- 无法取消正在运行的类型计算；超时只是放弃等待，计算仍占用 checker。
- 会话中毒后只能降级为“类型能力不可用”，恢复需要用户重启语言服务器。
- 依赖树从单个 tarball 变成约二十个 crate 加 ICU，`Cargo.lock`、`cargo deny` 和 NOTICE 的维护成本上升；MSRV 提升到 1.100。
- `tsrs` 仍是 prerelease：精确 pin 意味着升级必须成组进行并重跑探针。

### Risks

- **栈深度**：重型解析/checker 在 tsrs 的 512 MiB 栈后台 worker 上运行，`createSnapshot` 仍在调用线程；需在真实项目 fixture 上验证，必要时把 open/update 放到专用大栈线程。
- **FS 语义漂移**：旧回调故意回答“缺失/空目录”，进程内必须返回真实 `stat`/目录项；用差分测试覆盖模块解析、typings、路径穿越和写拒绝。
- **Checker 结果差异**：hover/补全/引用/文档格式化在 `Partial` 方法上可能与 tsgo 有字段级差异；差异必须记录为已知差异，不能静默改变用户可见行为。
- **Mutex 中毒与全局状态**：持锁 panic 后会话可能持续失败，映射为 `SessionPoisoned` 并停止使用；全局 worker/arena 不回收，因此不频繁重建会话，需测量 RSS。
- **二进制体积与构建时间**：内嵌标准库与 ICU 增大 release 体积和干净构建时间，需要实测并与删除的 28 MB 引擎对比。
- **回滚**：不保留运行时双后端；本次变更以单个提交/revert 回退到子进程实现。

## Validation

验收口径（实现完成后按此核对，本文不声称已通过）：

- `cargo fmt`、`cargo clippy -D warnings`、`cargo test --workspace --all-features`、`cargo doc`、`cargo deny --offline check` 一次通过。
- 进程内引擎单测覆盖：正常请求、版本不匹配、构造 panic、关闭幂等、`SessionPoisoned` 降级、stale snapshot/handle、unsupported 方法与内部错误映射。
- VFS 单测覆盖：命中、缺失、路径穿越、写/追加/删除/chtimes 拒绝、目录枚举、真实 `stat`、`realpath` 不回退宿主。
- 真实类型门禁：设置 `TSRS_TYPINGS_ROOT` 与 `WXML_REQUIRE_REAL_ENGINE=1` 时运行端到端类型用例；未设置时跳过且不得据此声称类型行为已验证。
- 差分测试：类型字符串、hover 文档、JSDoc、引用、补全、诊断与 tsgo 历史结果对比，差异为零或记录为已知差异。
- 长会话 soak：重复 open-update-close，记录 RSS、线程数和失败请求，确认没有持续增长或不可恢复的中毒状态。
- 按文档从零安装后 `wxml-lsp --stdio` 可启动并返回类型查询，安装根只需 `bin/wxml-lsp` 与 `typings/miniprogram-api-typings`。

实现与任务拆分见 `openspec/changes/archive/2026-11-21-replace-tsgo-with-tsrs-session/`（归档后见 `openspec/changes/archive/`）。

## References

- [ADR-0001：分层语言服务与模块依赖边界](0001-layered-language-service-architecture.md)
- [ADR-0002：模板类型检查、类型后端与源码映射](0002-template-type-checking-and-source-maps.md)
- 原 ADR-0008（Rust + tsgo 类型后端，移除 Node 依赖）：已删除，仍有效的决定并入本文，编号不复用
- [ADR-0009：项目瘦身](0009-project-slimming.md)
- [原生类型后端合同](../reference/tsrs-native-contract.md)
- [共享验收样本说明](../../tests/fixtures/README.md)
