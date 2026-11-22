# ADR-0001: 分层语言服务与模块依赖边界

**Date**: 2026-11-15  
**Status**: accepted（分层与依赖方向；运行时与类型后端形态由 ADR-0012 更新）
**Deciders**: 项目维护者、规划协作 agent

**2026-11-21 后续**：[ADR-0012](0012-in-process-tsrs-session.md)（2026-11-21 accepted）将类型后端改为进程内 `tsrs_api::Session`。本文的层图、职责隔离和依赖方向继续有效；原文中的 Node 首选运行时、每个服务一个 Node 子进程、私有 stdio 协议和“native 后端候选”均已删除，不再作为现行架构。

**2026-11-16 补充**：[ADR-0006](0006-explicit-mini-program-roots.md) 定义显式 `apps` 模式下的 app 根与类型项目根契约。**2026-11-20 补充**：[ADR-0010](0010-zero-config-lazy-discovery.md) 定义省略 `apps` 时的默认零配置惰性发现；显式模式仍是覆盖入口。

## Context

项目面向在 Zed 中开发原生微信小程序的个人使用场景，核心需求是 WXML 引用能获得实际 JS/TS 类型、字段说明和精确诊断，且不要求改写普通 `Page({...})`、`Component({...})`。工程已形成六个 crate 的 workspace：文本坐标、WXML/表达式语法、项目事实、类型契约与模板语义、进程内类型后端适配和 LSP 二进制。Zed 扩展是独立的 WASM workspace。

类型引擎不再通过外部进程接入，而是由 `wxml-backend-tsrs` 在语言服务器进程内驱动。因此本记录必须区分两类边界：仍然有效的 crate 分层与职责隔离，以及已由 ADR-0012 替换的 Node/子进程运行时。

## Decision

采用 Zed Rust/WASM 扩展、单一 Rust 语言服务器和进程内类型引擎。保留现有 crate 分层：WXML 解析、项目事实、模板语义、类型会话适配和协议适配各有单一所有者；类型层只通过精简 DTO 消费项目事实，不直接依赖项目数据库。

### 运行时结构

```text
Zed Dev Extension
  ├─ 语言配置、Tree-sitter grammar 与查询
  └─ 启动本地服务、配置辅助文档同步
       │ LSP over stdio
       ▼
Rust WXML Language Server（wxml-lsp）
  ├─ 文档协调器、项目宿主与调度
  ├─ WXML/表达式解析入口
  ├─ 模板语义、检查片段与源码映射
  └─ 类型后端适配
       │ 进程内调用 tsrs_api::Session
       ▼
wxml-backend-tsrs
  ├─ 只读 VFS、shadow 与查询
  └─ 把脚本结果映射回 WXML
```

语言服务器同一时刻只维护一个活动类型会话；没有运行时双后端，也没有 Node 回退。会话的启动、版本校验、有界重建、失败隔离和降级规则见 [ADR-0012](0012-in-process-tsrs-session.md)。

### 当前 crate 依赖

箭头表示允许的依赖，不表示每个模块都必须直接声明。当前 Cargo 依赖以此图为准。

```text
wxml-lsp ──→ wxml-backend-tsrs ──→ wxml-types ──→ wxml-syntax ──→ wxml-text
    │                                  └──────────────────────→ wxml-text
    ├──────→ wxml-project ────────────────────────────────────→ wxml-text
    ├──────→ wxml-types
    ├──────→ wxml-syntax
    └──────→ wxml-text
```

禁止 `types → project`、`syntax → types/project`、`project → syntax/types`、`backend-tsrs → lsp/project` 和任何底层 crate 依赖 LSP。依赖由实际消费需求驱动，不预先填充 workspace 依赖表；不新增收纳所有模型的通用 `common` crate。

### 模块职责与实现方式

下表路径均为当前实现位置；“规划名”不因为出现在历史 ADR 中就成为已实现 API。

| 模块 | 主要文件 | 职责 | 边界 |
| --- | --- | --- | --- |
| `wxml-text` | `line_index.rs`、`range.rs`、`source_range.rs`、`lib.rs` | 行索引、类型化半开范围、UTF-8 字节与行内/全文 UTF-16 转换；明确 EOF、代理对和 CRLF 边界 | 坐标原语；不放 AST、项目状态或协议类型 |
| `wxml-syntax` | `lexer.rs`、`parser.rs`、`ast.rs`、`expr/`、`source_map.rs` | 手写容错 WXML lexer/parser、独立表达式解析、源码分段映射；输入中间态保留为错误节点 | 带源码范围的语法快照；不理解 TS 类型或访问项目文件系统；不是 Tree-sitter 集成 |
| `wxml-project` | `files.rs`、`overlay.rs`、`discovery.rs`、`config.rs`、`typing.rs`、`sources.rs`、`styles.rs` | 文件身份、内存覆盖、页面/组件关联、`app.json` 发现、tsconfig 与源码依赖事实、样式/资源索引 | 不可变项目事实；不生成检查代码或重新实现 TS 模块解析 |
| `wxml-types` | `contract.rs`、`context.rs`、`check.rs`、`template.rs`、`template_semantics.rs`、`status.rs`、`backend.rs` | 公共身份与版本、`RequestContext`、名字绑定、循环和条件语义、`CheckFragment`、后端接口、检查状态 | 拥有模板片段 DTO 和映射后的结果形状；不依赖 `wxml-project`，不复制语法树。历史规划名 `AnalysisContext`/`TemplateIR` 不是现行 API |
| `wxml-backend-tsrs` | `native/`、`shadow/`、`query/`、`script/`、`input.rs`、`backend.rs` | 进程内类型会话、只读 VFS、shadow 布局与根改写、类型/文档/定义/诊断查询、脚本事实 | 消费 `wxml-types` 契约；不重新实现 WXML 解析或项目发现；不写用户工作区 |
| `wxml-lsp` | `server.rs`、`workspace.rs`、`scheduler.rs`、`project_host.rs`、`configuration.rs`、`workspace_packages.rs`、`handlers/`、`metadata.rs` | stdio 生命周期、状态唯一写入口、快照调度、取消、请求路由、工作区包发现、元数据与协议转换 | 将 project 结果适配为 types 输入；不复制 AST 或模板类型语义；`metadata` 属于 LSP，不回放到 project |
| Zed 扩展 | `editors/zed/` 中的扩展清单、Rust 启动入口、语言配置与查询 | 注册 WXML、启动本地服务、辅助语言同步和主 JS/TS 服务并存 | 编辑器适配；不承担类型推断 |

根选择按模式分开：显式 `apps` 的路径校验、canonical 包含关系、最长 app 匹配和共享文件归属由 [ADR-0006](0006-explicit-mini-program-roots.md) 定义；省略 `apps` 时的向上发现与工作区包挂载由 [ADR-0010](0010-zero-config-lazy-discovery.md) 定义。显式模式不扫描子 `app.json`，也不向上寻找 tsconfig；默认模式才从当前 WXML 独立寻找最近的 `app.json` 和 `tsconfig.json`。

文档数据库由 project 提供数据结构及读写操作，LSP 协调器是唯一写入口；所有分析任务读取不可变快照。`LineIndex` 与文本在同一文档快照中封装使用，避免将旧索引用于新文本。模板、脚本和配置的版本共同构成请求身份，结果必须通过发布屏障后才能进入 LSP。

### 常用语言功能

| 功能 | 实现归属 | 边界 |
| --- | --- | --- |
| 标签、属性、值、指令、事件补全 | LSP 的 `metadata/` 与上下文筛选，project 提供组件/页面事实 | 资料来源、版本、许可证；已有属性去重；模式相关候选 |
| 自定义组件 | project 解析本地/全局组件，`wxml-backend-tsrs` 提供属性类型，LSP 合并展示 | 未保存配置、目标变化、重名与不支持的包/插件形式 |
| 表达式成员补全、hover、定义 | `wxml-types` 片段与映射，`wxml-backend-tsrs` 提供实际类型和声明 | 不完整表达式、原字段文档、安全替换范围 |
| 事件函数 | 独立事件绑定通道 | 候选、文档、跳转、存在性检查分别验收 |
| class 补全及导航 | project 的 `styles/` 索引 | 同名/全局样式、导入依赖、重复声明 |
| 资源路径 | project 路径解析与 LSP 链接 | 相对路径、项目根路径、缺失文件 |
| 标签配对、闭合和 snippets | Zed 查询及必要的 LSP 编辑能力 | 输入过程中不重复插入，尊重客户端能力和用户配置 |
| 格式化 | `wxml-syntax` 与 LSP 格式化实现 | 幂等；文本空白、插值和 WXS 保真；解析失败不修改 |

Vue/Pug、Emmet、创建组件命令、自定义变量装饰、市场发布和自动下载不进入当前范围。当前没有运行时双后端或后端替换开关；类型后端形态由 ADR-0012 固定。

## Alternatives Considered

| 方案 | Pros | Cons | Why not |
| --- | --- | --- | --- |
| Rust 静态提取字段作为完整类型引擎 | 进程和运行时依赖较少 | 需要重建 TS 类型关系、JSDoc 和项目语义 | 不能满足完整类型准确性目标，可用于基础元数据但不能成为类型真源 |
| 直接复用既有 WXML LSP 或只启动 analyzer Rust 二进制 | 复用解析和协议基础 | 现有核验不足以满足新增类型与编辑一致性需求 | 可借鉴，不能将原项目测试通过等同本项目验收 |
| 逐段翻译历史插件实现 | 保留熟悉的补全规则 | 历史兼容成本高，不能直接解决真实类型上下文 | 仅复用适用规则和资料 |
| 严格兄弟 crate，通过 LSP 复制一套语法输入树 | 保持隔离形式 | 第二套树及穷尽转换增加遗漏和演进成本 | 类型分析直接消费唯一 syntax 结构更清晰 |
| types 同时直接依赖 project | 编排代码较少 | 类型层耦合项目数据库与文件系统实现 | 只传精简项目事实足够，保留项目层替换与独立测试能力 |
| 保留独立 Node 类型后端进程 | 类型生态成熟，故障隔离 | 引入 Node 运行时、私有协议和跨进程调试成本 | 生产路径已固定为进程内 tsrs，见 ADR-0012 |

## Consequences

### Positive

- 单一可执行文件承载模板语义和类型查询，没有外部类型进程需要启动、握手和回收。
- 每个模块有单一职责与所有者，可用 mock 并行测试。
- 不复制表达式 AST，不将类型语义塞入 LSP 或文本基础层。
- 只读 VFS 和 shadow 让类型引擎只能看到快照，业务文件不落盘。

### Negative

- 进程内引擎失去硬隔离：不可恢复的会话故障只能降级类型能力，OOM/abort 会影响整个语言服务器。
- `tsrs` 仍是 prerelease，精确 pin 使升级必须成组进行并重跑能力探针。
- 需要维护项目 DTO 适配、两段源码映射和快照一致性。

### Risks

- 普通注册写法的无损类型桥尚未证明覆盖所有写法，按 ADR-0002 的检查状态报告缺口。
- Zed 跨语言文档同步和实例归属仍未完全端到端验收，按 ADR-0003 验证。
- 元数据、grammar 和类型依赖必须固定适用版本并核实许可证，不能将历史资料当作当前事实。

## Validation

- 检查 crate 依赖无环且符合目标图；新依赖由集成人统一接入。
- syntax、project、types 可在没有编辑器和真实类型引擎的情况下独立测试。
- 默认 `cargo test` 不要求真实类型引擎；真实类型行为由 `TSRS_TYPINGS_ROOT` 与 `WXML_REQUIRE_REAL_ENGINE=1` 的显式门禁证明。
- 统一检查以根 `AGENTS.md` 和 README 为准；本记录不声称这些命令已在当前提交上执行。

## References

- [当前能力摘要](../../README.md#能力摘要)
- [WXML 语言功能基线](../reference/wxml-language-feature-baseline.md)
- [当前已知限制](../../README.md#已知限制)
- [ADR-0009 项目瘦身](0009-project-slimming.md)（历史证据见 tag `pre-slim-20261120`）
- [ADR-0002：模板类型检查与映射](0002-template-type-checking-and-source-maps.md)
- [ADR-0003：版本化同步与接入](0003-versioned-document-synchronization.md)
- [ADR-0006：显式小程序根与类型项目根](0006-explicit-mini-program-roots.md)
- [ADR-0010：零配置惰性发现](0010-zero-config-lazy-discovery.md)
- [ADR-0012：进程内 tsrs 类型引擎会话](0012-in-process-tsrs-session.md)
