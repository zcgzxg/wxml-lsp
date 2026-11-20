# 项目文档索引

更新日期：2026-11-20。

本文件说明当前文档的职责、权威来源和阅读顺序。每个事实只保留一个主要归属，其他文档通过链接引用，不重复维护。

## 1. 文档角色

| 角色 | 主要位置 | 负责内容 | 不负责 |
| --- | --- | --- | --- |
| 需求与状态 | [项目 README](../README.md) | 项目目标、范围、构建、安装和当前能力 | 架构取舍的完整论证 |
| 功能基线 | [WXML 语言功能基线](reference/wxml-language-feature-baseline.md) | 通用 WXML 语言功能的默认行为、数据来源和验收要求 | 当前是否已实现 |
| 基础契约 | [C0 基础契约](reference/contracts.md) | 坐标单位、文件与版本身份、能力状态、后端接口、mock 行为和共享样本 schema | 类型桥与协议的具体实现 |
| 类型桥契约 | [C1 类型桥契约](reference/type-bridge-c1.md) | 布局、数据根与 `properties` 覆盖、源码映射、C1 请求/响应和状态 | C0 坐标/身份/后端接口 |
| 原生合同 | [F0/F1 原生合同](reference/tsgo-native-contract.md) | 引擎固定、进程/FS/查询协议、AST 解码和回退边界 | 迁移期 oracle/ledger 过程 |
| 诊断策略 | [模板类型诊断策略](reference/type-diagnostics.md) | 严格空值、运行时转换、属性绑定、`any`/`unknown` 和未支持形式的降级规则 | 当前实现进度或测试证据 |
| 查询基准 | [查询基准](reference/benchmarks.md) | 查询路径的测量边界、运行方式、冷热语义和结果解读规则 | CI 耗时门禁或固定延迟预算 |
| 验收样本 | [共享验收样本](../tests/fixtures/README.md) | AC-001..AC-013 的正例、反例、runtime 场景和 schema v2 记录 | 类型分析实现 |
| 架构决策 | [ADR 索引](adr/README.md) | 模块边界、类型检查与映射、文档同步、交付闸门、瘦身和零配置惰性发现决策 | 日常进度 |
| 来源与许可证 | [来源记录](reference/provenance.md) | 外部调研对象的版本、提交、许可证和归属 | 功能需求 |

## 2. 阅读顺序

首次进入项目：

1. 阅读[项目 README](../README.md)，确认目标、范围、构建和安装方式。
2. 阅读本索引，了解文档归属。
3. 需要理解架构时，按 0001 → 0010 阅读 [ADR](adr/README.md)。
4. 实现或评审跨模块接口时，阅读 [C0 基础契约](reference/contracts.md)、[C1 类型桥契约](reference/type-bridge-c1.md) 与[模板类型诊断策略](reference/type-diagnostics.md)。
5. 修改 native 后端时，阅读[F0/F1 原生合同](reference/tsgo-native-contract.md)。
6. 需要实现通用 WXML 功能时，阅读[功能基线](reference/wxml-language-feature-baseline.md)。
7. 需要复用外部代码或数据时，先查[来源记录](reference/provenance.md)，再复核许可证。

## 3. 权威顺序

发生冲突时按以下顺序判断：

1. 当前代码和默认运行的测试。
2. 已接受的 ADR。
3. `docs/reference/` 中的契约与诊断策略。
4. 项目 README 和功能基线。
5. 来源记录。

过程性证据、迁移计划和旧 Node 时代的验收记录不再作为当前事实来源。需要追溯时使用 tag `pre-slim-20261120`。

## 4. 维护规则

- 需求变化先改项目 README 或功能基线；跨模块取舍新增 ADR。
- 临时实验只在仓库外运行，不在项目目录生成临时文件。
- 实验结论、环境、限制和最小可复查数据写入 ADR、reference 或提交说明；不再恢复 `docs/evidence/`。
- 外部来源和许可证变化同步到 `docs/reference/provenance.md`。
- 已删除的交付 crate、迁移 parity/ledger、Q2/性能/stdio 门禁和过程文档不再重建；历史版本见 tag `pre-slim-20261120`。
- 新增 ADR 使用下一个四位编号，手写可复制 [template.md](adr/template.md)。

## 5. 目录结构

```text
docs/
├── README.md
├── adr/
│   ├── README.md
│   ├── template.md
│   └── 0001-*.md ... 0010-zero-config-lazy-discovery.md
└── reference/
    ├── benchmarks.md
    ├── contracts.md
    ├── provenance.md
    ├── tsgo-native-contract.md
    ├── type-bridge-c1.md
    ├── type-diagnostics.md
    └── wxml-language-feature-baseline.md

tests/fixtures/README.md
```
