# 架构决策记录

更新日期：2026-11-21。

本目录记录 WXML LSP 的架构选择、替代方案、边界和后果。需求与实际实施进度见[项目 README](../../README.md)。

`accepted` 表示本项目采用该架构约定，不表示对应功能已经实现或通过验收。

| ADR | 标题 | 状态 | 日期 |
| --- | --- | --- | --- |
| [0001](0001-layered-language-service-architecture.md) | 分层语言服务与模块依赖边界 | accepted | 2026-11-15 |
| [0002](0002-template-type-checking-and-source-maps.md) | 模板类型检查、类型后端与源码映射 | accepted | 2026-11-15 |
| [0003](0003-versioned-document-synchronization.md) | 版本化文档同步、后端协议与 Zed 接入 | accepted | 2026-11-15 |
| [0004](0004-parallel-delivery-and-validation-gates.md) | 并行任务划分、交接契约与验收闸门 | accepted | 2026-11-15 |
| [0005](0005-self-contained-project-documentation.md) | 项目文档自包含与证据本地化 | accepted | 2026-11-15 |
| [0006](0006-explicit-mini-program-roots.md) | 显式小程序根与类型项目根 | accepted（显式 apps 模式；默认行为由 0010 替代） | 2026-11-16 |
| [0007](0007-local-delivery-trust-model.md) | 本地交付与安装信任模型 | accepted（安装机制由 0009 替代） | 2026-11-17 |
| [0009](0009-project-slimming.md) | 项目瘦身：移除交付 crate、迁移证据与 native parity 门禁 | accepted | 2026-11-20 |
| [0010](0010-zero-config-lazy-discovery.md) | 零配置惰性小程序发现与工作区包挂载 | accepted（已实现） | 2026-11-20 |
| [0011](0011-component-type-declarations.md) | 组件类型声明与 components.d.ts 固定发现约定 | accepted | 2026-11-20 |
| [0012](0012-in-process-tsrs-session.md) | 进程内 tsrs 类型引擎会话与故障隔离 | accepted | 2026-11-21 |

原 ADR-0008（Rust + tsgo 类型后端，移除 Node 依赖）已于 2026-11-21 删除，其仍有效的决定由 ADR-0012 承接，编号 0008 不复用。当前默认 `--stdio` 为 native，`NotInstalled` 时 HostOnly，损坏安装 fail closed；生产路径没有 Node 运行时。

ADR-0009 删除 `wxml-delivery`、迁移期 native parity/ledger/Q2/性能/stdio 门禁、`wxml-fixtures`、`docs/evidence`、`docs/plans` 和交付/测试合同。ADR-0009 当时记录为 **2,142 passed / 0 failed / 0 ignored**；ADR-0010 实现后最近一次统一验证为 **2,168 passed / 3 ignored / 0 failed**。历史计划与证据保留在 tag `pre-slim-20261120`。

ADR-0012 取代已删除的 ADR-0008：类型引擎改为进程内 `tsrs_api::Session`，标准库随 `tsrs_vfs` 内嵌，安装树只保留二进制与 typings；继续有效的是 Rust 侧语义所有权、无 Node 运行时、只读虚拟文件系统和方法白名单。实现进度以代码、测试和 `openspec/changes/archive/2026-11-21-replace-tsgo-with-tsrs-session/` 为准。

## 阅读与维护约定

1. 按 0001 → 0012 阅读（0008 已删除，内容并入 0012）；并行任务同时阅读其依赖的 ADR。
2. ADR-0001..0005 整理自 2026-11-15 的总体设计会话；ADR-0006 记录显式根配置；ADR-0007 记录本地交付与安装信任模型；ADR-0009 记录本次瘦身；ADR-0010 记录默认零配置惰性发现与工作区包挂载；ADR-0011 记录组件类型声明与 components.d.ts 固定发现约定；ADR-0012 记录进程内 tsrs 会话与故障隔离取舍，并承接已删除的 ADR-0008 关于 native 类型后端与 Node 删除的结论。当前实施进度见项目 README，不从历史背景推断。
3. 架构边界以有效的 ADR 为准；需求以项目 README 为依据；已支持能力必须以实际代码、测试及编辑器验证记录为依据。
4. ADR-0001 统一原 README 与 Cargo 注释中相互冲突的 crate 依赖描述。该决策没有自动添加 Cargo 依赖。
5. 工作包表是决策时的交付基线，日常实施进度维护在项目 README；不要通过改写 ADR 将待验证设计追记为既有事实。
6. 需求与功能基线维护在 `docs/reference/`；临时实验只在仓库外运行，结论写入 ADR/reference/提交说明，不再使用 `docs/evidence/`。
7. 后续改变已采用的关键决策时新增 ADR；整体替代时将原记录标为 `superseded by ADR-NNNN`，局部补充或替代时仅在相关条目链接新记录，保留原来的决策背景和其余有效决定。ADR-0006 仅补充/替代 0001、0003 的根配置及项目选择部分，ADR-0010 部分替代 ADR-0006 的默认根选择和默认扫描/向上查找行为；ADR-0009 替代 ADR-0007 的安装机制和 ADR-0004 的证据留存方式，保留运行时信任、类型 pin 和验收原则；ADR-0011 局部补充 ADR-0002、ADR-0010 的组件类型来源，不改变其分层、快照和发现边界；ADR-0012 取代已删除的 ADR-0008，固定进程内会话、Rust 侧语义所有权、无 Node、只读 VFS 与白名单结论。

新增记录使用下一个四位编号；手工编写可复制 [template.md](template.md)。可用状态为 `proposed`、`accepted`、`deprecated`、`superseded by ADR-NNNN`。
