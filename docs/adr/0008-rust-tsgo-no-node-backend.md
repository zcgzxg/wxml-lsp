# ADR-0008: Rust + tsgo 类型后端，移除 Node 依赖

**Date**: 2026-11-17  
**Status**: proposed（用户已选择迁移方向；接口与能力验证完成前不升级为 accepted）  
**Deciders**: 项目维护者；planner、architect、Rust 测试及交付盘点参与设计

本文记录已采用的迁移方向；接口与能力验证完成前保持 proposed。实施计划见 `docs/plans/2026-11-17-rust-tsgo-migration.md`。本文件不表示迁移、测试或交付已经完成。

## Context

现有 Rust LSP 自行启动 Node 后端，后端自行创建 TypeScript LanguageService，并未复用 Zed 的 JavaScript/TypeScript 服务。小程序上下文、原位 shadow、Behavior、属性、事件、源码映射和安全符号规则已经形成项目能力，迁移必须保留其可观察行为及正在修改的修复。用户决定采用 Rust + tsgo，去除产品运行、构建、当前测试及发布流程对 Node/npm 的要求，并将后端 TypeScript 测试迁入 Rust。

原生 TypeScript 提供进程协议和类型查询，但不能将旧 LanguageService 的 API 与原生 API 当作相同接口。当前源码调研只支持将原生 API 作为候选；尚未通过本项目完整 shadow、查询呈现、读写边界及恢复验证。

## Decision

采用 **Rust 语义适配器 + 独立的原生 tsgo 子进程**。Rust 拥有 WXML/小程序语义、项目快照、shadow 生成、源码映射、查询编排和 C0 DTO；tsgo 拥有 JavaScript/TypeScript 解析、绑定、类型检查及类型关系计算。不自行实现 TypeScript 类型系统，不保留 Node 适配层，也不假设共享编辑器的类型服务。

新增 `wxml-backend-tsgo` crate，内部划分 native、script、shadow、query 模块；`wxml-lsp` 仅提供 ProjectHost 输入适配。依赖保持单向，底层后端不依赖 LSP，现有 `Backend` 为对上边界。常态每个 Rust LSP 最多管理一个原生子进程，多项目通过有界 session/snapshot 管理。

首选接口是同一个原生进程的 `--api --async` 双向 stdio。冻结版本后实现 Rust JSON-RPC 客户端、受限文件系统回调、必要的二进制 AST 解码、snapshot/lease/handle 生命周期与类型查询。保留现有 `wxml_types::Backend` 对上层的边界。若单进程 API 无法完整实现 hover 文档、重载、定义、引用、补全及 rename 安全语义，先阻塞迁移并修订本 ADR；只有验证同进程 LSP + API 连接确实共享快照且满足同一文件访问策略，才可作为明确批准的替代实现。不得默认改成两个独立类型图。

采用精确的原生发行版本、源码提交、API/AST schema 及平台资产摘要。先比较稳定版本与候选版本；不把调研用 `7.1.0-beta` 自动视为生产选择，不使用 `main`、`next` 或浮动版本。版本、协议与类型资产更新作为一个经过测试的变更交付。旧 Node 安装可在无 Node 条件下由受限旧 schema 读取器识别并迁入原生包；验证成功前保持旧 current 不变，原生版本之间支持校验后的回退，不自动回退激活 Node 包。

现有 TS 测试按源测试、展开用例、关键断言逐项登记并迁入 Rust。迁移期允许在隔离的历史基线环境运行旧 Node 后端生成 oracle；最终构建、测试、安装包及运行过程不得依赖它。删除旧实现前必须完成全量等价、恢复、安全、性能及无 Node 交付验收；删除后再验一次。

## Alternatives Considered

### 保留 TS 适配器，通过 Node 调用 tsgo

- **Pros**: 初期改动较少，可直接使用官方 TS 客户端。
- **Cons**: 同时需要 Node 和原生引擎，小程序语义及测试仍留在 TS。
- **Why not**: 不满足本次去 Node 和测试迁 Rust 的明确目标。

### Rust 只转发标准 LSP 请求

- **Pros**: 常规 hover、definition、completion 的接入较直观。
- **Cons**: 不足以直接表达现有 AST/checker、属性赋值关系、Behavior 合成及安全符号规则；还存在虚拟文件和配置归属问题。
- **Why not**: 在完整等价性证据缺失时，不能用普通 TS 文件验证代替小程序语义验收。

### Rust 直接链接或 fork Go 类型引擎

- **Pros**: 理论上可控制更深层接口或形成更紧的打包形式。
- **Cons**: 增加 FFI、ABI、Go runtime、跨平台链接和上游维护成本。
- **Why not**: 当前没有此需求；先使用进程边界隔离故障并复用上游原生发行物。未来若能力验证确实要求上游改动，单独决策。

### 用 Rust 重写 TypeScript 类型引擎

- **Pros**: 单语言实现。
- **Cons**: 需要重做跨文件解析、绑定、推断、JSDoc、泛型、条件类型和 TypeScript 兼容行为。
- **Why not**: 与迁移现有能力的目标不相称，回归面不可接受。

### 共享 Zed 已运行的 JavaScript/TypeScript 服务

- **Pros**: 可能降低重复项目状态开销。
- **Cons**: 当前项目没有共享会话协议；编辑器生命周期、未保存文档、shadow 所有权和快照隔离均需额外协作。
- **Why not**: 不把编辑器未提供的跨服务协作当作后端前提；本方案由 Rust 管理自己的原生进程。

## Consequences

### Positive

- 产品后端实现统一到 Rust，使用上游原生类型引擎；最终无 Node/npm 前置条件。
- 业务语义、缓存失效、路径策略和测试归属清晰，保留可替换 Backend 边界。
- 发布包可固定原生引擎和类型资产，沿用摘要校验、离线安装及原子升级。

### Negative

- 需要迁移现有 TS 语义实现与测试，不能仅改一个依赖或启动命令。
- Rust 需要维护原生协议/AST schema 的适配、查询呈现及远程对象生命周期。
- 多平台资产和原生协议兼容性成为发布职责；原生检查更快不等于端到端请求必然更快。

### Risks

- **查询保真度**：先验证文档、重载、alias、引用、rename 及范围映射；缺项阻塞并修订设计，不删除断言。
- **坐标与 AST**：逐字段区分 UTF-8 字节、UTF-16 code unit、字符串池偏移；校验大小、索引、版本、句柄和生命周期，加入性质测试及畸形输入测试。
- **文件访问**：未配置的回调可能使用 OS 文件系统；`noEmit` 和禁止 writeFile 不等于 OS 沙箱。验证所有可达读入口、配置/包/类型解析、符号链接及写入口，只开放查询方法，不启用 external code。任何超出现有权限的行为都阻塞切换。
- **旧响应污染**：请求绑定项目代次、文档版本、依赖指纹和进程 session；重启、失效、取消后不能发布旧 diagnostics 或 WorkspaceEdit。
- **未提交改动丢失**：冻结 HEAD + 工作区指纹及行为增量；旧目录内未提交修复未迁移并通过测试前禁止删除。

## Validation

详细门禁以实施计划为准，至少包含：

1. 固定版本的单进程能力试验：只读 VFS、原位 shadow、查询结果、AST/句柄及生命周期完整通过。
2. 每个旧测试和关键断言都有 Rust 落点及结果；差分比较不得抹去类型、文档、范围、诊断、编辑目标或安全标志。
3. 原有 Rust 全量测试、真实原生 E2E、状态恢复、UTF/CRLF、未保存跨文件更新及保守 rename 规则通过；缺少原生引擎必须使原生门禁失败，不能静默 skip。
4. 在 Node 不可用且进程调用受监测的环境，构建、测试、发布、安装、升级、回滚和 Linux Zed 冒烟通过。
5. 新 manifest/index schema、固定原生资产、类型声明许可证和摘要验证通过；离线模式不能隐式下载。
6. 旧 Node 默认入口、实现和当前测试删除后重新执行完整门禁；保留业务 TS/JS、`.d.ts`、fixture、字节规则和历史证据。

所有项目在本草案创建时均为**待执行**。现有原生实验和旧后端测试记录只能作为历史证据，不能据此声称本 ADR 已通过验证。

## 与既有 ADR 的关系

- ADR-0001：仅替代 Node 首选运行时/进程模型，保留 crate 分层和可替换后端。
- ADR-0002：仅替代类型桥实现归属；保留原位 shadow、模板语义及映射原则。
- ADR-0003：替代 Rust–Node NDJSON 协议；保留版本化同步、快照一致性和发布屏障。
- ADR-0006：更新后端资产路径配置说明，保留小程序根与类型项目根的区分。
- ADR-0007：替代 Node 安装前置条件和对应包字段；保留信任、离线、原子安装与回滚规则。
- ADR-0004/0005 继续有效。原 ADR 局部增加本记录链接，不整体标记失效，不改写历史证据。

## References

- 本轮用户明确选择 Rust + tsgo、去 Node、测试迁 Rust、通过 `/orchestrate` 并行迁移。
- `docs/adr/0001-*.md`、`0002-*.md`、`0003-*.md`、`0004-*.md`、`0005-*.md`、`0006-*.md`、`0007-*.md`。
- `docs/evidence/2026-11-15-native-typescript-service.md`：普通虚拟 TS 文件历史试验，不是完整小程序验收。
- Microsoft TypeScript 官方源码，调研提交 `3e9236361fa97a9e6c685f4833ac457ae2d8c50a`：`packages/typescript/src/api/async/client.ts`、`api/options.ts`、`api/proto.generated.ts`、`tsc/internal/ipc/protocol_jsonrpc.go`、`tsc/internal/api/encoder/encoder.go`、`tsc/internal/api/callbackfs.go`。源码调研不等于二进制实测。
