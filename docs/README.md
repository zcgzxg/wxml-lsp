# 项目文档索引

更新日期：2026-11-16。

本文件说明各类文档的职责、权威来源和阅读顺序。每个事实只保留一个主要归属，其他文档通过链接引用，不重复维护。

## 1. 文档角色

| 角色 | 主要位置 | 负责内容 | 不负责 |
| --- | --- | --- | --- |
| 需求与状态 | [项目 README](../README.md) | 项目目标、范围、T1–T9 类型需求、实施计划、当前进度和第一阶段验收 | 架构取舍的完整论证 |
| 功能基线 | [WXML 语言功能基线](reference/wxml-language-feature-baseline.md) | 通用 WXML 语言功能的默认行为、数据来源和验收要求 | 当前是否已实现 |
| 基础契约 | [C0 基础契约](reference/contracts.md) | 坐标单位、文件与版本身份、能力状态、后端接口、mock 行为和共享样本 schema | 类型桥与协议的具体实现 |
| 类型桥契约 | [C1 类型桥契约](reference/type-bridge-c1.md) | 候选 A 布局、数据根与 `properties` 覆盖、源码映射、C1 请求/响应和状态 | C0 坐标/身份/后端接口 |
| 诊断策略 | [模板类型诊断策略](reference/type-diagnostics.md) | 严格空值、运行时转换、属性绑定、`any`/`unknown` 和未支持形式的降级规则 | 当前实现进度或测试证据 |
| 验收样本 | [共享验收样本](../tests/fixtures/README.md) | AC-001..AC-013 的正例、反例、runtime 场景、schema v2 与校验入口 | 类型分析实现 |
| 执行计划 | [Wave 5 并行交付计划](plans/2026-11-16-wave5-parallel-delivery.md)、[Zed 标准安装与真实项目验收计划](plans/2026-11-16-zed-standard-acceptance.md) | 并行 worktree、接口冻结、测试门禁与评审流程；Zed 标准安装和真实编辑器验收步骤 | 需求定义和最终状态 |
| 架构决策 | [ADR 索引](adr/README.md) | 模块边界、类型检查与映射、文档同步、交付闸门等长期决策 | 日常进度 |
| 证据 | [证据索引](evidence/README.md) | 实验环境、方法、结果、限制和最小原始数据 | 把实验结论当作已实现功能 |
| 集成与编辑器证据 | [P7 真实集成](evidence/p7-integration.md)、[P8 Zed 扩展](evidence/p8-zed.md) | 真实 Node 端到端与 G1 门禁；Zed wasm 构建、隔离 smoke 和未验证项 | 把部分证据当作完整验收 |
| 来源与许可证 | [来源记录](reference/provenance.md) | 外部调研对象的版本、提交、许可证和归属 | 功能需求 |
| 工具链说明 | [.opencode/README.md](../.opencode/README.md) | 本项目 OpenCode 配置的来源、安装、更新和卸载 | 产品架构或 WXML 需求 |

## 2. 阅读顺序

首次进入项目：

1. 阅读[项目 README](../README.md)，确认目标、范围和当前状态。
2. 阅读本索引，了解文档归属。
3. 需要理解架构时，按 0001 → 0005 阅读 [ADR](adr/README.md)。
4. 实现或评审跨模块接口时，阅读 [C0 基础契约](reference/contracts.md)、[C1 类型桥契约](reference/type-bridge-c1.md) 与[模板类型诊断策略](reference/type-diagnostics.md)。
5. 需要实现通用 WXML 功能时，阅读[功能基线](reference/wxml-language-feature-baseline.md)。
6. 需要引用实验或集成依据时，阅读[证据索引](evidence/README.md)及 [P7 真实集成](evidence/p7-integration.md)、[P8 Zed 扩展](evidence/p8-zed.md) 记录，同时检查其限制。
7. 需要复用外部代码或数据时，先查[来源记录](reference/provenance.md)，再复核许可证。

## 3. 权威顺序

发生冲突时按以下顺序判断：

1. 当前代码、测试和可复现的编辑器行为，说明“现在实际是什么”。
2. 有效的 ADR，说明“架构上决定怎么做”。
3. C0/C1 契约与类型诊断策略（[contracts.md](reference/contracts.md)、[type-bridge-c1.md](reference/type-bridge-c1.md)、[type-diagnostics.md](reference/type-diagnostics.md)），说明“接口和检查规则是什么”。
4. 项目 README 和功能基线，说明“需求是什么”。
5. 证据记录（包括 [P7](evidence/p7-integration.md)、[P8](evidence/p8-zed.md)），说明“过去验证过什么、边界在哪里”。
6. 来源记录，只说明“历史来源和许可证是什么”。

C1 与 C0 冲突时，类型桥部分以 C1 为准；C0 未覆盖的坐标、身份和后端接口仍以 `contracts.md` 为准。文档不能覆盖代码事实。发现文档与实现不一致时，记录差异并更新对应文档，不能静默选择一个来源。

## 4. 维护规则

- 需求或验收条件变化：更新项目 README 或功能基线，不把新要求只写在 ADR 里。
- 架构决策变化：新增 ADR，将原记录标为 `superseded by ADR-NNNN`，保留背景。
- 新增实验：在 `docs/evidence/` 增加记录和最小数据；不在功能文档中直接链接临时目录。
- 外部来源变化：更新来源记录中的版本、提交和许可证；不复用许可证不明确的内容。
- 普通修复和提交：依赖 Git 历史，不额外写治理记录。
- 文档中的路径使用仓库相对链接；代码位置写成可在编辑器中跳转的路径。
- 不在项目目录生成临时文件。实验在仓库外运行，结论和最小数据转写到证据目录。

## 5. 目录结构

```text
README.md                          # 需求、计划、状态
docs/
├── README.md                      # 本索引
├── plans/
│   ├── 2026-11-16-wave5-parallel-delivery.md
│   └── 2026-11-16-zed-standard-acceptance.md
├── reference/
│   ├── wxml-language-feature-baseline.md
│   ├── contracts.md
│   ├── type-bridge-c1.md
│   ├── type-diagnostics.md
│   └── provenance.md
├── evidence/
│   ├── README.md
│   ├── 2026-11-15-*.md
│   ├── type-bridge-c1.md
│   ├── p7-integration.md
│   ├── p8-zed.md
│   └── data/
└── adr/
    ├── README.md
    ├── 0001-*.md ... 0005-*.md
    └── template.md
```
