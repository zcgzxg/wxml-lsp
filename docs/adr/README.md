# 架构决策记录

更新日期：2026-11-17。

本目录记录 WXML LSP 的架构选择、替代方案、边界和后果。需求与实际实施进度见[项目 README](../../README.md)。

`accepted` 表示本项目采用该架构约定，不表示对应功能已经实现或通过验收。类型桥、Zed 未保存文档同步等尚未完成的验证，在各 ADR 中单独列出。

| ADR | 标题 | 状态 | 日期 |
| --- | --- | --- | --- |
| [0001](0001-layered-language-service-architecture.md) | 分层语言服务与模块依赖边界 | accepted | 2026-11-15 |
| [0002](0002-template-type-checking-and-source-maps.md) | 模板类型检查、类型后端与源码映射 | accepted | 2026-11-15 |
| [0003](0003-versioned-document-synchronization.md) | 版本化文档同步、后端协议与 Zed 接入 | accepted | 2026-11-15 |
| [0004](0004-parallel-delivery-and-validation-gates.md) | 并行任务划分、交接契约与验收闸门 | accepted | 2026-11-15 |
| [0005](0005-self-contained-project-documentation.md) | 项目文档自包含与证据本地化 | accepted | 2026-11-15 |
| [0006](0006-explicit-mini-program-roots.md) | 显式小程序根与类型项目根 | accepted | 2026-11-16 |
| [0007](0007-local-delivery-trust-model.md) | 本地交付与安装信任模型 | accepted | 2026-11-17 |

## 阅读与维护约定

1. 按 0001 → 0007 阅读；并行任务同时阅读其依赖的 ADR。
2. ADR-0001..0005 整理自 2026-11-15 的总体设计会话，其历史实现基线是 Rust workspace、文本坐标模块和 CLI 脚手架；ADR-0006 记录 2026-11-16 的显式根配置决策；ADR-0007 记录 2026-11-17 的本地交付与安装信任模型。当前实施进度见项目 README，不从历史背景推断。
3. 架构边界以有效的 ADR 为准；需求以项目 README 为依据；已支持能力必须以实际代码、测试及编辑器验证记录为依据。
4. ADR-0001 统一原 README 与 Cargo 注释中相互冲突的 crate 依赖描述。该决策没有自动添加 Cargo 依赖。
5. 工作包表是决策时的交付基线，日常实施进度维护在项目 README；不要通过改写 ADR 将待验证设计追记为既有事实。
6. 需求与功能基线维护在 `docs/reference/`，实验证据维护在 `docs/evidence/`，外部来源和许可证集中记录在 `docs/reference/provenance.md`；外部项目或临时目录不作为需求依据。
7. 后续改变已采用的关键决策时新增 ADR；整体替代时将原记录标为 `superseded by ADR-NNNN`，局部补充或替代时仅在相关条目链接新记录，保留原来的决策背景和其余有效决定。ADR-0006 仅补充/替代 0001、0003 的根配置及项目选择部分。

新增记录使用下一个四位编号；手工编写可复制 [template.md](template.md)。可用状态为 `proposed`、`accepted`、`deprecated`、`superseded by ADR-NNNN`。
