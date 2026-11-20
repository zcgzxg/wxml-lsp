# 架构决策记录

更新日期：2026-11-20。

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
| [0008](0008-rust-tsgo-no-node-backend.md) | Rust + tsgo 类型后端与移除 Node 依赖 | accepted (native default; Node removed) | 2026-11-17；2026-11-20 |
| [0009](0009-project-slimming.md) | 项目瘦身：移除交付 crate、迁移证据与 native parity 门禁 | accepted | 2026-11-20 |
| [0010](0010-zero-config-lazy-discovery.md) | 零配置惰性小程序发现与工作区包挂载 | accepted（已实现） | 2026-11-20 |

ADR-0008 于 2026-11-20 在 owner scope waiver（C-lite）下最终关闭为 `accepted`，并随 P7 删除完成写入 **Node removed**：接受 native 默认后端和合理行为测试的验收口径，不声称旧 TS 逐条 parity。当前默认 `--stdio` 为 native，`NotInstalled` 时 HostOnly，损坏安装 fail closed。

ADR-0009 删除 `wxml-delivery`、迁移期 native parity/ledger/Q2/性能/stdio 门禁、`wxml-fixtures`、`docs/evidence`、`docs/plans` 和交付/测试合同。ADR-0009 当时记录为 **2,142 passed / 0 failed / 0 ignored**；ADR-0010 实现后最近一次统一验证为 **2,164 passed / 3 ignored / 0 failed**。历史计划与证据保留在 tag `pre-slim-20261120`。

## 阅读与维护约定

1. 按 0001 → 0010 阅读；并行任务同时阅读其依赖的 ADR。
2. ADR-0001..0005 整理自 2026-11-15 的总体设计会话；ADR-0006 记录显式根配置；ADR-0007 记录本地交付与安装信任模型；ADR-0008 记录 native 类型后端和 Node 删除；ADR-0009 记录本次瘦身；ADR-0010 记录默认零配置惰性发现与工作区包挂载。当前实施进度见项目 README，不从历史背景推断。
3. 架构边界以有效的 ADR 为准；需求以项目 README 为依据；已支持能力必须以实际代码、测试及编辑器验证记录为依据。
4. ADR-0001 统一原 README 与 Cargo 注释中相互冲突的 crate 依赖描述。该决策没有自动添加 Cargo 依赖。
5. 工作包表是决策时的交付基线，日常实施进度维护在项目 README；不要通过改写 ADR 将待验证设计追记为既有事实。
6. 需求与功能基线维护在 `docs/reference/`，外部来源和许可证集中记录在 `docs/reference/provenance.md`；临时实验只在仓库外运行，结论写入 ADR/reference/提交说明，不再使用 `docs/evidence/`。
7. 后续改变已采用的关键决策时新增 ADR；整体替代时将原记录标为 `superseded by ADR-NNNN`，局部补充或替代时仅在相关条目链接新记录，保留原来的决策背景和其余有效决定。ADR-0006 仅补充/替代 0001、0003 的根配置及项目选择部分，ADR-0010 部分替代 ADR-0006 的默认根选择和默认扫描/向上查找行为；ADR-0008 仅替代 0001/0002/0003/0006/0007 中与 Node 运行时、类型桥实现、协议和安装前置相关的部分；ADR-0009 替代 ADR-0007 的安装机制和 ADR-0004 的证据留存方式，保留运行时信任、类型 pin 和验收原则。

新增记录使用下一个四位编号；手工编写可复制 [template.md](template.md)。可用状态为 `proposed`、`accepted`、`deprecated`、`superseded by ADR-NNNN`。
