# ADR-0003: 版本化文档同步、后端协议与 Zed 接入

**Date**: 2026-11-15  
**Status**: accepted  
**Deciders**: 项目维护者、规划协作 agent

**2026-11-17 后续**：[ADR-0008](0008-rust-tsgo-no-node-backend.md)（proposed）拟替代 Rust–Node NDJSON 协议；本文的版本化同步、快照一致性和发布屏障继续有效。

**2026-11-16 补充**：根配置来源、项目选择和配置生效时机以 [ADR-0006](0006-explicit-mini-program-roots.md) 为准；本记录的历史背景与版本化同步原则继续有效。

## Context

未保存的 WXML、页面/组件脚本、`data`/`properties` 字段引用的跨文件类型声明和字段说明都必须影响模板结果，只监听磁盘不足以满足要求。跨进程查询、编辑器事件、配置变化和后端重启可能让旧结果覆盖新结果，单个 WXML 的编辑器版本无法表示完整依赖状态。Zed 的多语言注册与辅助服务配置提供接入方向，但同一实例归属、更新顺序和多服务并存仍需本项目早期实测。项目层和 TS 宿主还必须避免各自解释一套不一致的项目与文件语义。

## Decision

采用 LSP 协调器唯一写入口、内存覆盖层优先、不可变分析快照和带版本 ACK 的 Rust–Node 协议。只有依赖、映射和后端世代仍有效的结果可以发布；类型后端失败应暴露不可用状态并恢复快照。Zed 辅助语言同步作为早期验收任务，与原 JS/TS 主服务并存。

### 文档与项目权威

- project 拥有文件身份、overlay、磁盘回退、配置候选与依赖图的数据结构；LSP 协调器顺序应用修改，分析任务不直接修改数据库。
- 已打开文档以内存内容为准；磁盘变化更新其磁盘基线，不覆盖尚未保存内容；关闭后回退磁盘并产生新的内部版本。
- Rust 发现小程序根、页面/组件和 TS 配置候选；Node 使用 TS 规则确定实际配置归属、选项、文件集合、路径别名、模块解析和项目引用，并反馈项目身份及依据。（根发现与配置选择部分由 [ADR-0006](0006-explicit-mini-program-roots.md) 收敛，见下文补充。）
- 多配置归属必须有确定策略，不能仅凭“最近 tsconfig”猜测。无适用配置时显式标记 inferred project 及生效选项。
- Node 反馈实际读取的依赖。先保守刷新相关 TS 项目的活跃模板，再按证据优化依赖粒度。
- Node 的文件缓存与查询依赖必须绑定版本；发现快照外依赖时登记其内容版本，扩充并验证快照，必要时重跑，不能用实时磁盘读取绕过一致性校验。

上述根发现与配置归属是最初的职责规划；[ADR-0006](0006-explicit-mini-program-roots.md) 将其收敛为 Rust 校验并消费 `initialize.initializationOptions.apps[]` 的显式 `root`/`projectRoot`，不扫描子 `app.json`、向上推断最近 tsconfig，或按生成目录名称自动忽略/发现 app。Node 消费 Rust 提供的项目快照与有效编译选项，不另行发现 app/tsconfig。根配置只在启动时消费，修改后重启 LSP，没有根热更新；配置非法以 `initialize -32602` 失败，合法目录中的 `app.json` 问题仍走 project issues。

同一 workspace 的多 app 按文档 URI 最长 app 目录匹配，以稳定 `projectId` 隔离；app 外而 `projectRoot` 内的共享文件保留 active 或唯一 owner，多个 owner 且无 active 时不猜测。`projectRoot` 是后端相对文件路径的基准。这一边界补充不把下文多 worktree/多配置验收目标改写成已支持的完整多 workspace 或跨 app 依赖图。

### 版本契约

以下为待实现的协议概念，不是现有公开 API。

| 身份 | 含义 |
| --- | --- |
| `ClientVersion` | 编辑器会话内的单文档版本，仅用于适用的文档事件校验 |
| `SourceRevision` | 内部文档内容版本；编辑、关闭回退和磁盘变化均可能产生新值 |
| `ProjectRevision` | 项目归属、配置、依赖环境或分析设置变化的版本 |
| `SnapshotId` | 一次分析的不可变输入集合，覆盖模板、脚本、导入依赖、配置、策略及 helper ABI |
| `ViewRevision` | 检查片段文本与映射的版本，生成规则或上下文改变也会更新 |
| `BackendFileVersion` | 后端实际安装文件的版本，包含注入变化，不能直接等同原脚本版本 |
| `BackendEpoch` | 后端进程/状态世代，重启或完整重建后变化 |

`AnalysisStamp` 汇集快照、项目、视图和后端世代信息，并关联依赖版本清单；每个请求、ACK、结果和缓存键均可校验所依赖的身份。片段、脚本和共享模板调用身份按 ADR-0002 隔离，不只用 URI 作为视图键。

### 公共交接数据

| 契约 | 提供方 | 内容与消费方式 |
| --- | --- | --- |
| `SyntaxSnapshot` | syntax | 文本、语法树、表达式 AST、源码版本；types 直接消费，不复制树 |
| `AnalysisContext` | types 定义，LSP 从 project 适配 | 所属脚本、注册候选、配置候选、组件目标及检查策略；不暴露可变项目数据库 |
| `CheckFragment` | types | 片段身份、JS/TS 方言、文本、模板映射、helper ABI；Node 原样插入 |
| `BackendResult` | Node | 实际项目身份、类型/文档/声明/诊断、后端范围及检查状态 |
| `MappedResult` | types | 原文范围、类型结果和覆盖状态；LSP 只转换协议并执行发布屏障 |

### 后端协议与发布屏障

Rust–Node 使用带长度边界的 stdio JSON 消息。stdout 只传协议，日志进入 stderr；消息大小、等待队列、超时和解析深度应有边界，不使用 shell 拼接启动命令，不自动执行工作区脚本或插件。

最小消息集合：

```text
initialize / capabilities
applySnapshot → ack
analyze
hover / completion / definition
cancel
disposeProject / shutdown
```

消息携带协议版本、请求 ID 和 `AnalysisStamp`，错误区分不支持、检查关闭、协议错误、后端失败等。取消、能力位和数据结构由 P0 冻结；协议不兼容应清晰失败，不静默解释未知消息。

典型查询流程：

```text
编辑事件 → 协调器更新 overlay 和依赖
         → 创建快照与检查片段
         → Node 安装快照并 ACK
         → 在已安装版本上查询
         → types 组合映射
         → LSP 校验发布屏障
         → 发布当前结果或调度新快照
```

发布与缓存至少校验：后端世代有效、所需快照已 ACK、依赖和项目版本有效、检查视图与映射一致、目标模板未被新分析替代。不能把旧范围套在新文本上；过时结果丢弃并调度当前快照。

### 调度、取消与恢复

- 主动 hover/补全优先于后台诊断，诊断适用防抖；等待后端时不持有阻塞数据库写入的锁。
- 首版可实现取消发布与超时处理；不能假定 Node 同线程同步检查能立即响应新取消消息。
- 需要更强取消能力时再评估 worker 或中断机制，不将协议中存在 `cancel` 当作已经可抢占。
- 后端超时/退出时报告类型能力不可用，保留可用的语法和静态语言功能；不将失败包装为空错误集合。
- 重启时递增 `BackendEpoch`，旧请求、ACK、缓存均失效；重放当前有效项目与 overlay，再恢复查询。
- 成功检查且错误修复后发布空诊断集合清除旧结果。关闭、项目移除等按生命周期清理；能力失效时不得继续把旧结果当作当前检查结果。

### Zed 接入验证

扩展负责语言注册、grammar/查询和本地启动，WXML 服务作为相关 JS/TS 文档的辅助接收者，不替代其主服务，不重复发布无关脚本诊断和补全。相关 JSON/JSONC 及后续样式未保存同步按能力接入，启用范围清晰可配置。

根配置写在 `.zed/settings.json` 的 `lsp.wxml-lsp.initialization_options` 内；两个路径均相对 LSP workspace root，不相对 `.zed`。配置示例及仅覆盖内层值的 Schema 见 [Zed 指南](../../editors/zed/README.md#mini-program-roots-initialization-options)。[Zed 官方文档](https://zed.dev/docs/configuring-languages#possible-configuration-options) 说明初始化选项修改后须重启服务；本补充不新增真实 Zed 运行验收结论。

在最小 LSP 存在后立即验证：

1. WXML、JS、TS、导入类型是否进入预期服务实例，记录 worktree/根目录和实例身份。
2. 打开、修改、关闭与查询的实际顺序；同一快照是否包含当前未保存内容。
3. 原 JS/TS 主服务是否继续工作，辅助服务是否避免重复结果。
4. 多 worktree、多小程序根、多 tsconfig 及重启后的归属是否隔离。
5. 类型和纯 JSDoc 修改是否无需保存或刷新 WXML 即可更新，修复后旧诊断是否清除。

验证日志以实例、事件和版本为依据，不默认记录业务源码。若未复用同一实例，先检查服务注册和根目录判定；仍不能满足时评估单一后台状态服务加多个前端连接，另行记录协议与生命周期调整。磁盘监听不能作为未保存同步的替代验收。

## Alternatives Considered

| 方案 | Pros | Cons | Why not |
| --- | --- | --- | --- |
| 只监听磁盘 | 实现简单 | 丢失未保存脚本、`data`/`properties` 类型声明和说明变化 | 不满足编辑一致性需求 |
| 每种语言独立状态且不共享 | 进程边界直观 | overlay 分裂，跨语言查询读取旧内容 | 先验证同实例，必要时使用共享状态服务 |
| 只校验 WXML 的客户端版本 | 成本较低 | `data`/`properties` 类型声明、配置、注入和后端重启无法表示 | 使用完整依赖快照和世代 |
| 多任务直接修改共享数据库 | 局部处理方便 | 读到混合版本，难以保证映射一致 | 单写入口加不可变读快照 |
| 无条件接收后端最新返回值 | 路由简单 | 完成顺序不等于输入新旧顺序 | 必须执行发布屏障 |

## Consequences

### Positive

- 可以复现并测试跨文件、跨语言和跨进程的一致性。
- 关闭回退、依赖变化及重启都通过显式版本处理。
- Zed 平台风险早暴露，避免服务完成后才发现未保存同步不可达。

### Negative

- ACK、版本清单、恢复和发布屏障增加实现与测试成本。
- 保守项目级失效可能触发较多分析，后续需要性能数据优化。
- 如果编辑器实例行为不符合预期，可能需要额外共享状态服务。

### Risks

- 未登记依赖造成假快照：以依赖读取反馈、版本确认及必要重跑处理。
- 同步 TS 检查阻塞取消：明确首版语义，设置超时，不承诺即时抢占。
- 多项目或共享脚本串扰：项目、所有者、视图及世代共同标识后端状态。

## Validation

使用可控延迟的 mock 和 stdio 集成测试覆盖乱序结果、过时 ACK、依赖变更、关闭回退、诊断清除、超时、重启及多项目隔离。真实 Zed 测试覆盖上述五组同步场景；只证明服务启动或收到 hover 请求不足以通过验收。相关验证均待实施。

## References

- [未保存文档需求](../../README.md#53-未保存文档同步)
- [证据索引](../evidence/README.md)
- [ADR-0001：运行时与模块边界](0001-layered-language-service-architecture.md)
- [ADR-0002：片段、映射与检查状态](0002-template-type-checking-and-source-maps.md)
- [ADR-0004：验证工作包和闸门](0004-parallel-delivery-and-validation-gates.md)
- [ADR-0006：显式根配置、归属与重启生效](0006-explicit-mini-program-roots.md)
