# ADR-0001: 分层语言服务与模块依赖边界

**Date**: 2026-11-15  
**Status**: accepted  
**Deciders**: 项目维护者、规划协作 agent

**2026-11-17 后续**：[ADR-0008](0008-rust-tsgo-no-node-backend.md)（proposed）拟替代 Node 首选运行时与进程模型；本文的分层、依赖边界和可替换后端原则继续有效。

**2026-11-16 补充**：[ADR-0006](0006-explicit-mini-program-roots.md) 明确 app 根与类型项目根的初始化配置和职责，替代根扫描及最近 tsconfig 推断；本记录的历史背景、分层和依赖边界继续有效。

## Context

项目面向在 Zed 中开发原生微信小程序的个人使用场景，核心需求是 WXML 引用能获得实际 JS/TS 类型、字段说明和精确诊断，且不要求改写普通 `Page({...})`、`Component({...})`。当前工程已有五 crate workspace、文本坐标模块和 CLI 脚手架，尚无完整语言服务、Node 类型后端或 Zed 扩展。有限的隔离实验支持继续验证 TypeScript Language Service 路线，但没有证明完整模板语义和编辑器行为。原 README 禁止兄弟 crate 横向依赖，而 Cargo 注释允许 `types → project/syntax`，需要统一成明确的无环依赖约定。

## Decision

采用轻量 Zed Rust/WASM 扩展、Rust WXML 语言服务和可替换的独立类型后端，首选 Node + TypeScript Language Service。保留现有五个 crate，允许 `wxml-types → wxml-syntax` 复用语法结构，项目事实通过类型层拥有的输入接口接入，类型层不直接依赖项目数据库。先完成类型体验与未保存同步，再扩充常用语言功能。

### 运行时结构

```text
Zed Dev Extension
  ├─ 语言配置、Tree-sitter grammar 与查询
  └─ 启动本地 Rust 服务、配置辅助文档同步
       │ LSP
       ▼
Rust WXML Language Server
  ├─ 文档协调器与项目数据库
  ├─ WXML/表达式解析
  ├─ 模板语义、检查片段与源码映射
  └─ 类型后端适配
       │ 私有 stdio 协议
       ▼
Node 类型后端
  ├─ TS 项目宿主、内存文件与脚本上下文桥
  └─ TypeScript Language Service
```

初期每个 Rust 服务管理一个 Node 子进程。Node 按 TS 项目组织状态，为活跃模板所有者维护隔离的派生检查视图；先证明隔离与正确性，再决定共享、缓存及淘汰策略。LSP 框架、依赖版本和 grammar 固定提交在接入时核实，本记录不为它们指定未经验证的版本。

### 目标 crate 依赖

箭头表示允许的依赖，不表示已添加对应 Cargo 声明。

```text
wxml-lsp ──→ wxml-types ──→ wxml-syntax ──→ wxml-text
    │             └──────────────────────→ wxml-text
    ├──────→ wxml-project ────────────────→ wxml-text
    ├──────→ wxml-syntax
    └──────→ wxml-text
```

禁止 `types → project`、`syntax → types/project`、`project → syntax/types` 和底层 crate 依赖 LSP。依赖由实际消费需求驱动，不预先填充 workspace 依赖表；不新增收纳所有模型的通用 `common` crate。

### 模块职责与实现方式

下列文件除现有入口和 `line_index.rs` 外均为规划路径，不代表已实现 API。

| 模块 | 规划文件 | 功能与实现 | 输出及边界 |
| --- | --- | --- | --- |
| `wxml-text` | `line_index.rs`、`range.rs`、`utf16.rs` | 行索引、类型化半开范围、UTF-8 字节与行内/全文 UTF-16 转换；明确 EOF、代理对和 CRLF 边界 | 坐标原语；不放 AST、项目状态或协议类型 |
| `wxml-syntax` | `parser.rs`、`ast.rs`、`expr/`、`source_map.rs` | 容错 WXML CST/AST、独立表达式解析、通用分段映射；复用经编辑的旧树进行增量解析 | 带源码范围的语法快照；不理解 TS 类型或访问项目文件系统 |
| `wxml-project` | `files.rs`、`overlay.rs`、`discovery.rs`、`components.rs`、`config.rs`、`graph.rs` | 文件身份、内存覆盖、页面/组件关联、`miniprogramRoot`、`usingComponents`、配置候选、依赖和逆向失效（根配置由 [ADR-0006](0006-explicit-mini-program-roots.md) 补充） | 不可变项目事实；不生成检查代码或重新实现 TS 模块解析 |
| `wxml-types` | `context.rs`、`semantic/`、`check_view/`、`mapping.rs`、`policy.rs`、`backend/` | 名字绑定、循环和条件语义、检查片段、后端查询、映射组合与诊断策略 | 拥有 `AnalysisContext`、`TemplateIR`、`CheckFragment` 和映射后的结果 |
| `wxml-lsp` | `server.rs`、`workspace.rs`、`scheduler.rs`、`handlers/`、`convert.rs` | stdio 生命周期、状态唯一写入口、快照调度、取消、请求路由和协议转换 | 将 project 结果适配为 types 输入；不复制 AST 或模板类型语义 |
| Node 后端 | `backends/typescript/src/{protocol,projects,host,registrations,context_bridge,shadow,queries}.ts` | 脚本 AST、注册识别、TS 配置解释、真实数据上下文、原样片段插入、类型/文档/诊断查询 | 独占脚本上下文和 shadow 映射；不重新改写模板表达式 |
| Zed 扩展 | `editors/zed/` 中的扩展清单、Rust 启动入口、语言配置与查询 | 注册 WXML、启动本地服务、辅助语言同步和主 JS/TS 服务并存 | 编辑器适配；不承担类型推断 |

根目录职责按 [ADR-0006](0006-explicit-mini-program-roots.md) 补充：LSP 在初始化时校验 `apps[].root`（`app.json` 目录）及可选 `projectRoot`（类型项目/`tsconfig.json` 目录，默认 `root`），project 层消费显式目录事实；不自动扫描小程序或向上选择最近 tsconfig。类型桥文件相对路径基于 `projectRoot`，app 资源仍按 app 根解析。单 workspace 内的显式多 app 不等同完整多 workspace 或跨 app 依赖图支持。

文档数据库由 project 提供数据结构及读写操作，LSP 协调器是唯一写入口；所有分析任务读取不可变快照。`LineIndex` 与文本在同一文档快照中封装使用，避免将旧索引用于新文本。

### 常用语言功能

| 功能 | 实现归属 | 验收关注点 |
| --- | --- | --- |
| 标签、属性、值、指令、事件补全 | project 的版本化 `metadata/` 与 LSP 上下文筛选 | 资料来源/版本/许可证、已有属性去重、模式相关候选 |
| 自定义组件 | project 解析本地/全局组件，Node 提供属性类型，LSP 合并展示 | 未保存配置、目标变化、重名与不支持的包/插件形式 |
| 表达式成员补全、hover、定义 | types 查询与映射，Node 提供实际类型和声明 | 不完整表达式、原字段文档、安全替换范围 |
| 事件函数 | 独立事件绑定通道 | 候选、文档、跳转、适用的存在性检查分别验收 |
| class 补全及导航 | project 的 `styles/` 索引 | 同名/全局样式、导入依赖、重复声明；解析器接入时另行核实 |
| 资源路径 | project 路径解析与 LSP 链接 | 相对路径、项目根路径、缺失文件 |
| 标签配对、闭合和 snippets | Zed 查询及必要的 LSP 编辑能力 | 输入过程中不重复插入，尊重客户端能力和用户配置 |
| 格式化 | 后置的 syntax/格式化实现 | 幂等；文本空白、插值和 WXS 保真；解析失败不修改 |

Vue/Pug、Emmet、创建组件命令、自定义变量装饰、市场发布和自动下载不进入当前范围。TypeScript 原生后端是后续候选，只有通过相同验收才考虑替换 Node。

## Alternatives Considered

| 方案 | Pros | Cons | Why not |
| --- | --- | --- | --- |
| Rust 静态提取字段作为完整类型引擎 | 进程和运行时依赖较少 | 需要重建 TS 类型关系、JSDoc 和项目语义 | 不能满足完整类型准确性目标，可用于基础元数据但不能成为类型真源 |
| 直接复用既有 WXML LSP 或只启动 analyzer Rust 二进制 | 复用解析和协议基础 | 现有核验不足以满足新增类型与编辑一致性需求 | 可借鉴，不能将原项目测试通过等同本项目验收 |
| 逐段翻译历史插件实现 | 保留熟悉的补全规则 | 历史兼容成本高，不能直接解决真实类型上下文 | 仅复用适用规则和资料 |
| 严格兄弟 crate，通过 LSP 复制一套语法输入树 | 保持原 README 的隔离形式 | 第二套树及穷尽转换增加遗漏和演进成本 | 类型分析直接消费唯一 syntax 结构更清晰 |
| types 同时直接依赖 project | 编排代码较少 | 类型层耦合项目数据库与文件系统实现 | 只传精简项目事实足够，保留项目层替换与独立测试能力 |

## Consequences

### Positive

- Rust 集中处理模板和编辑体验，成熟的 TS 引擎保留真实类型关系。
- 每个模块有单一职责与所有者，可用 mock 并行开发。
- 不复制表达式 AST，不将类型语义塞入 LSP 或文本基础层。

### Negative

- 增加 Node 运行时、私有协议和跨进程调试成本。
- 需要维护项目 DTO 适配、两段源码映射和快照一致性。
- 隔离派生检查视图可能消耗较多内存，需要实测后优化。

### Risks

- 普通注册写法的无损类型桥尚未证明完整可用，按 ADR-0002 先验证。
- Zed 跨语言文档同步和实例归属尚未通过端到端验证，按 ADR-0003 早期验证。
- 元数据、grammar 和类型依赖必须固定适用版本并核实许可证，不能将历史资料当作当前事实。

## Validation

- 检查 crate 依赖无环且符合目标图；新依赖由集成人统一接入。
- syntax、project、types 可在没有编辑器和真实后端的情况下独立测试。
- 最小类型闭环与 Zed 验收完成前，不宣称服务可用或完整支持 WXML。

## References

- [项目目标与功能需求](../../README.md#1-项目目标)
- [WXML 语言功能基线](../reference/wxml-language-feature-baseline.md)
- [已有实验与证据边界](../../README.md#6-已有验证与证据边界)
- [证据索引](../evidence/README.md)
- [来源与许可证记录](../reference/provenance.md)
- [ADR-0002：模板类型检查与映射](0002-template-type-checking-and-source-maps.md)
- [ADR-0003：版本化同步与接入](0003-versioned-document-synchronization.md)
- [ADR-0004：并行交付与验收](0004-parallel-delivery-and-validation-gates.md)
- [ADR-0006：显式小程序根与类型项目根](0006-explicit-mini-program-roots.md)
