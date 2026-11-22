# ADR-0004: 并行任务划分、交接契约与验收闸门

**Date**: 2026-11-15  
**Status**: deprecated（历史交付计划；不再作为现行规则）
**Deciders**: 项目维护者、规划协作 agent

## 状态

本记录保存的是 2026-11-15 Node 交付期的计划，已整体过时：P0–P9 工作包、C0/C1 接口冻结、G0–G4 闸门、AC-001–AC-013 验收矩阵、80%+ 覆盖率目标、任务派发模板，以及“格式化与替换 Node 后端后置”的未来计划，都不再是现行规则。文件名和标题保留，仅用于维持既有链接。

历史任务划分和验收矩阵不再维护；需要追溯时使用 git 历史或 tag `pre-slim-20261120`。当前实施范围、验证命令和能力状态以代码、测试、[项目 README](../../README.md) 与 [AGENTS 约定](../../AGENTS.md) 为准。

## 仍然有效的原则

- 交付前执行项目定义的测试与质量门禁；具体命令见 [AGENTS 约定](../../AGENTS.md) 和 [README 的检查与测试](../../README.md#检查与测试)。
- 临时实验在仓库外运行，不在项目目录生成临时文件；进入仓库的只有决定保留的实现、样本和测试。
- 文档自包含：需求和证据不依赖外部仓库、临时目录或已删除文档；规则见 [ADR-0005](0005-self-contained-project-documentation.md)。
- `accepted` 不等于 implemented；能力是否可用必须以实际代码和测试为依据，不能把计划或文档当作进度。

## 参考

- [项目 README](../../README.md)
- [AGENTS 约定](../../AGENTS.md)
- [文档索引](../README.md)
- [ADR-0005：项目文档自包含与证据本地化](0005-self-contained-project-documentation.md)
- [ADR-0009 项目瘦身](0009-project-slimming.md)（历史证据见 tag `pre-slim-20261120`）
