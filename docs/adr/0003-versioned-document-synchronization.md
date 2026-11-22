# ADR-0003: 版本化文档同步、后端协议与 Zed 接入

**Date**: 2026-11-15  
**Status**: accepted（同步与发布屏障；Rust–Node 后端协议由 ADR-0012 替代）
**Deciders**: 项目维护者、规划协作 agent

**2026-11-21 后续**：[ADR-0012](0012-in-process-tsrs-session.md)（2026-11-21 accepted）用进程内 `tsrs_api::Session` 替代 Rust–Node NDJSON 帧协议、ACK、消息集和跨进程取消。本文的版本化同步、快照一致性、发布屏障和失败降级原则继续有效；标题中的“后端协议”保留用于既有链接，但不再指 Node 消息集。

**2026-11-16 / 2026-11-20 补充**：显式 `apps` 的根选择与配置生效时机以 [ADR-0006](0006-explicit-mini-program-roots.md) 为准；省略 `apps` 时的默认零配置模式以 [ADR-0010](0010-zero-config-lazy-discovery.md) 为准。

## Context

未保存的 WXML、页面/组件脚本、`data`/`properties` 字段引用的跨文件类型声明和字段说明都必须影响模板结果，只监听磁盘不足以满足要求。查询、编辑器事件、配置变化和后端世代变化可能让旧结果覆盖新结果，单个 WXML 的客户端版本无法表示完整依赖状态。

项目层和类型后端还必须避免各自解释一套不一致的项目与文件语义。当前类型引擎在语言服务器进程内运行，没有外部进程握手，但“结果是否仍对应当前输入”的问题不变，发布前仍须校验完整依赖身份。

## Decision

采用 LSP 协调器唯一写入口、内存覆盖层优先、不可变分析快照和发布屏障。只有依赖、映射和后端世代仍有效的结果可以发布；类型后端失败必须暴露不可用状态，不能包装成空诊断。

### 文档与项目权威

- project 拥有文件身份、overlay、磁盘回退、配置候选与源码依赖路径的数据结构；LSP 协调器顺序应用修改，分析任务不直接修改数据库。
- 已打开文档以内存内容为准；磁盘变化更新其磁盘基线，不覆盖尚未保存内容；关闭后回退磁盘并产生新的内部版本。
- 根选择分两种模式：显式 `apps` 由 ADR-0006 定义，不扫描子 `app.json`、不向上寻找 tsconfig；省略 `apps` 时由 ADR-0010 从当前 WXML 向上发现最近的 `app.json` 和独立最近的 `tsconfig.json`。根配置只在启动时消费；默认模式在首次遇到文档时惰性建立项目。修改根配置后重启 LSP。
- 同一 workspace 的多 app 按文档 URI 最长 app 目录匹配，以稳定 `projectId` 隔离；app 外而 `projectRoot` 内的共享文件保留 active 或唯一 owner，多个 owner 且无 active 时不猜测。`projectRoot` 是后端相对文件路径的基准。
- LSP 从 project 构造 `PreparedView` 与不可变快照，把虚拟文件、脚本内容和有效编译选项交给 `wxml-backend-tsrs`；类型后端不另行发现 app/tsconfig，也不读取宿主文件系统。
- 脚本诊断走独立通道：`crates/wxml-lsp/src/handlers/diagnostics.rs` 把项目内 `.ts`/`.js`/`.mts`/`.cts`/`.mjs`/`.cjs` 文档路由到 `script_diagnostics`，发布到该脚本自己的 URI，不混入 WXML、样式或事件诊断。`Unsupported`/`Unavailable`/`Disabled`/`Unknown` 不发布空集合，以免看起来像“已检查且干净”。

### 版本契约

以下类型是当前实现中的公共身份，不是待实现协议概念。`AnalysisStamp` 已存在于 `crates/wxml-lsp/src/workspace.rs`，用于把一次结果绑定到它依赖的版本。

| 身份 | 含义 |
| --- | --- |
| `ClientVersion` | 编辑器会话内的单文档版本；`didChange` 拒绝更旧或重复版本 |
| `SourceRevision` | 内部文档内容版本；编辑、关闭回退和磁盘变化均可能产生新值 |
| `ProjectRevision` | 项目归属、配置、依赖环境或分析设置变化的版本 |
| `SnapshotId` | 一次分析的不可变输入集合，覆盖模板、脚本、导入依赖、配置和策略 |
| `ViewRevision` | 检查片段文本与映射的版本；生成规则或上下文改变也会更新 |
| `BackendEpoch` | 后端状态世代；显式失效或不可恢复失败后推进 |
| `AnalysisStamp` | 汇总文件、内容、快照、视图、项目和世代；结果必须匹配当前 stamp 才能发布 |

版本必须单调：同一文档的 `ClientVersion` 不接受旧值；接受的编辑推进 `SourceRevision` 和 `ViewRevision`；项目配置/依赖变化推进 `ProjectRevision`。片段、脚本和共享模板调用身份按 ADR-0002 隔离，不只用 URI 作为视图键。

### 发布屏障

发布与缓存至少校验：后端世代有效、文档内容与快照仍是当前版本、项目/视图版本有效、检查视图与映射一致、目标模板未被新分析替代。不能把旧范围套在新文本上；过时结果丢弃并调度当前快照。

- 成功检查且错误修复后，发布空诊断集合清除旧结果；空集合只有在 `Supported` 且 stamp 当前时才算干净。
- `Partial` 发布真实诊断并附带状态原因；`Unsupported`/`Unavailable`/`Disabled`/`Unknown` 不得表现为 `Supported` 的空结果。
- 关闭文档、项目移除或能力失效时按生命周期清理；不能继续把旧结果当作当前检查结果。
- 脚本诊断有独立 stamp 和发布通道，不能因为 WXML 结果更新而错误清除或覆盖脚本结果。

### 调度、取消与恢复

- 主动 hover/补全优先于后台诊断，诊断按配置防抖；等待后端时不持有阻塞状态写入的锁。
- 排队中的任务可直接取消并从队列移除；运行中的查询不能被强制抢占，取消标志只能在调用前后观察。类型计算在进程内同步运行；`Deadline` 只在送入引擎前检查是否过期，调用开始后不能停止等待或中断 checker，只能丢弃返回后的结果。
- 会话死亡只对当前请求做一次有界重试：首次致命结果若表明会话已死，丢弃旧会话并在同一请求上重建一次；重试成功不推进 `BackendEpoch`，因为请求 stamp 仍有效且旧会话已被移除。第二次失败仍为致命，推进 `BackendEpoch`、丢弃会话，当前请求返回不可用；不按每次重试推进 epoch，也不做退避循环。
- 项目、根或环境变化允许重建会话（`ensure_open` 停止旧会话并从新输入打开）；显式 `invalidate` 推进 `BackendEpoch`，旧请求、缓存和迟到结果随之失效。第二次失败后没有持久终态：后续请求仍可按同一规则重建，只是每个请求最多重试一次；没有服务器进程重启、取消或自动退避。
- 恢复时重放当前有效的项目、overlay 和快照，再恢复查询；用户源文件始终不被改写。

### Zed 接入

扩展负责语言注册、grammar/查询和本地启动，通过 `WXML_LSP_BIN` 指向安装好的二进制。WXML 服务作为相关 JS/TS 文档的辅助接收者，不替代其主服务，不重复发布无关脚本诊断和补全。扩展注册 WXML、JavaScript、TypeScript、TSX、JSON（不含 JSX），同一进程接收关联文档的打开、修改和保存。

根配置写在 `.zed/settings.json` 的 `lsp.wxml-lsp.initialization_options` 内；路径相对 LSP workspace root，不相对 `.zed`。配置只在启动时消费，修改后重启 LSP；示例见 [Zed 指南](../../editors/zed/README.md#mini-program-roots-initialization-options)。

Zed 同步仍应按以下场景验证：同一快照是否包含未保存内容；原 JS/TS 主服务是否继续工作且无重复诊断；多 worktree、多小程序根、多 tsconfig 及重启后的归属是否隔离；类型和纯 JSDoc 修改是否无需保存或刷新 WXML 即可更新，修复后旧诊断是否清除。

## Alternatives Considered

| 方案 | Pros | Cons | Why not |
| --- | --- | --- | --- |
| 只监听磁盘 | 实现简单 | 丢失未保存脚本、`data`/`properties` 类型声明和说明变化 | 不满足编辑一致性需求 |
| 每种语言独立状态且不共享 | 进程边界直观 | overlay 分裂，跨语言查询读取旧内容 | 使用同一协调器和快照 |
| 只校验 WXML 的客户端版本 | 成本较低 | `data`/`properties` 类型声明、配置、映射和后端世代无法表示 | 使用完整依赖快照和世代 |
| 多任务直接修改共享数据库 | 局部处理方便 | 读到混合版本，难以保证映射一致 | 单写入口加不可变读快照 |
| 无条件接收后端最新返回值 | 路由简单 | 完成顺序不等于输入新旧顺序 | 必须执行发布屏障 |
| 保留 Rust–Node ACK 协议 | 可显式确认安装版本 | 引入外部进程、帧协议和跨进程取消复杂度 | 已由进程内会话替代，见 ADR-0012 |

## Consequences

### Positive

- 可以复现并测试跨文件、跨语言和跨请求的一致性。
- 关闭回退、依赖变化和会话重建都通过显式版本处理。
- 脚本诊断和 WXML 诊断各有发布通道，不会互相伪装成干净结果。
- Zed 平台风险早暴露，避免服务完成后才发现未保存同步不可达。

### Negative

- 版本清单、恢复和发布屏障增加实现与测试成本。
- 保守项目级失效可能触发较多分析，后续需要性能数据优化。
- 进程内运行中没有可抢占的取消，长查询会占用 worker 直到返回。

### Risks

- **未登记依赖造成假快照**：以依赖读取反馈、版本确认及必要重跑处理。
- **同步检查阻塞取消**：明确运行中不可抢占；`Deadline` 只在送入引擎前检查，开始后不能停止等待，也不承诺即时中断 checker。
- **多项目或共享脚本串扰**：项目、所有者、视图及世代共同标识后端状态。
- **会话重建语义漂移**：一次重试成功不推进 epoch、第二次失败推进 epoch；测试必须固定这两条路径。

## Validation

使用可控延迟的 mock 和 stdio 集成测试覆盖乱序结果、过时 stamp、依赖变更、关闭回退、诊断清除、超时、会话重建及多项目隔离。真实 Zed 测试覆盖上述同步场景；只证明服务启动或收到 hover 请求不足以通过验收。相关验证按当前测试与门禁执行，本记录不声称新增运行结果。

## References

- [当前能力摘要与编辑一致性](../../README.md#能力摘要)
- [当前已知限制](../../README.md#已知限制)
- [ADR-0009 项目瘦身](0009-project-slimming.md)（历史证据见 tag `pre-slim-20261120`）
- [ADR-0001：运行时与模块边界](0001-layered-language-service-architecture.md)
- [ADR-0002：片段、映射与检查状态](0002-template-type-checking-and-source-maps.md)
- [ADR-0005：项目文档自包含](0005-self-contained-project-documentation.md)
- [ADR-0006：显式根配置、归属与重启生效](0006-explicit-mini-program-roots.md)
- [ADR-0010：零配置惰性发现](0010-zero-config-lazy-discovery.md)
- [ADR-0012：进程内 tsrs 类型引擎会话](0012-in-process-tsrs-session.md)
