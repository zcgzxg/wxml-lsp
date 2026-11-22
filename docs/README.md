# 项目文档索引

更新日期：2026-11-22。

本文件说明当前文档的职责、权威来源和阅读顺序。每个事实只保留一个主要归属，其他文档通过链接引用，不重复维护。

## 1. 文档角色

| 角色 | 主要位置 | 负责内容 | 不负责 |
| --- | --- | --- | --- |
| 需求与状态 | [项目 README](../README.md) | 项目目标、范围、构建、安装和当前能力 | 架构取舍的完整论证 |
| 功能基线 | [WXML 语言功能基线](reference/wxml-language-feature-baseline.md) | 通用 WXML 语言功能的默认行为、数据来源和验收要求 | 当前是否已实现 |
| 基础契约 | [基础契约](reference/contracts.md) | 坐标单位、文件与版本身份、能力状态、后端接口、mock 行为和共享样本 schema | 类型桥与协议的具体实现（C1 是历史概念） |
| 原生合同 | [原生类型合同](reference/tsrs-native-contract.md) | 进程内 type session 固定、FS/查询协议、AST 解码和降级边界 | 迁移期 oracle/ledger 过程 |
| 诊断策略 | [模板类型诊断策略](reference/type-diagnostics.md) | 严格空值、运行时转换、属性绑定、`any`/`unknown` 和未支持形式的降级规则 | 当前实现进度或测试证据 |
| 查询基准 | [查询基准](reference/benchmarks.md) | 查询路径的测量边界、运行方式、冷热语义和结果解读规则 | CI 耗时门禁或固定延迟预算 |
| 验收样本 | [共享验收样本](../tests/fixtures/README.md) | AC-001..AC-013 的正例、反例、runtime 场景和 schema v2 记录 | 类型分析实现 |
| 架构决策 | [ADR 索引](adr/README.md) | 模块边界、类型检查与映射、文档同步、瘦身、根发现、安装信任、进程内类型会话和组件绑定检查决策 | 日常进度；已废弃的交付流程 |

## 2. 阅读顺序

首次进入项目：

1. 阅读[项目 README](../README.md)，确认目标、范围、构建和安装方式。
2. 阅读本索引，了解文档归属。
3. 需要理解架构时，按主题阅读 [ADR](adr/README.md)：分层与类型边界先看 0001/0002/0012，根发现看 0006/0010，安装信任看 0007，组件声明与检查看 0011/0013。ADR-0004 只是 Node 交付期的历史存根，不用于推断当前实现。
4. 实现或评审跨模块接口时，阅读[基础契约](reference/contracts.md)与[模板类型诊断策略](reference/type-diagnostics.md)。
5. 修改 native 后端时，阅读[原生类型合同](reference/tsrs-native-contract.md)。
6. 需要实现通用 WXML 功能时，阅读[功能基线](reference/wxml-language-feature-baseline.md)。

## 3. 权威顺序

发生冲突时按以下顺序判断：

1. 当前代码和默认运行的测试。
2. 已接受的 ADR。
3. `docs/reference/` 中的契约与诊断策略。
4. 项目 README 和功能基线。

默认 `cargo test` 会跳过真实类型用例；它证明的是默认测试通过，不是真实类型引擎行为。需要验证类型能力时使用 `TSRS_TYPINGS_ROOT` 与 `WXML_REQUIRE_REAL_ENGINE=1`，或运行已安装布局的真实引擎测试。

过程性证据、迁移计划、旧 Node 时代的验收记录和 C1 类型桥契约不再作为当前事实来源。需要追溯时使用 tag `pre-slim-20261120`。

## 4. 维护规则

- 需求变化先改项目 README 或功能基线；跨模块取舍新增 ADR。
- 临时实验只在仓库外运行，不在项目目录生成临时文件。
- 实验结论、环境、限制和最小可复查数据写入 ADR、reference 或提交说明；不再恢复 `docs/evidence/`。
- 已删除的交付 crate、迁移 parity/ledger、Q2/性能/stdio 门禁、C1 类型桥和过程文档不再重建；历史版本见 tag `pre-slim-20261120`。
- 新增 ADR 使用下一个四位编号，手写可复制 [template.md](adr/template.md)。

## 5. 目录结构

```text
docs/
├── README.md
├── adr/
│   ├── README.md
│   ├── template.md
│   └── 0001-*.md ... 0013-engine-checked-component-bindings.md
│       （0004 为 deprecated 历史存根，0008 已删除）
└── reference/
    ├── benchmarks.md
    ├── contracts.md
    ├── tsrs-native-contract.md
    ├── type-diagnostics.md
    └── wxml-language-feature-baseline.md

tests/fixtures/README.md
```
