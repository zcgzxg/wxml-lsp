# TypeScript Language Service 机制实验（2026-11-15）

## 对象与环境

- TypeScript：`6.0.2`
- `miniprogram-api-typings`：`5.3.1`
- 运行方式：Node + TypeScript Language Service，内存文档与受限 WXML 表达式样本
- 业务项目：未修改

## 方法

使用两组隔离样本，共 22 项断言。覆盖范围：

- 跨文件模型类型与字段 JSDoc
- 不存在的字段和非法类型操作
- 可空类型与条件分支窄化
- `wx:for` 元素类型
- 组件属性类型
- 带 JSDoc 的 JavaScript 页面/组件
- 未保存的类型修改
- 含中文和 emoji 的 WXML 位置映射

实验脚本在会话结束后未保留，只留下依赖清单。因此本记录为 E0：结论可读，但不能仅凭仓库内容逐步复跑。

## 结果

22 项断言全部通过。由此确认：TypeScript Language Service 能在受控内存视图中提供真实类型、字段说明、诊断和未保存依赖更新，适合作为模板类型体验的候选后端。

该结果支持[ADR-0001](../adr/0001-layered-language-service-architecture.md)中“先验证 Node 类型后端”的选择。

## 限制

- 样本是受限表达式，不是完整 WXML 转换器。
- 未覆盖完整项目结构、所有 Page/Component 写法、Behavior 合并和 Zed 端到端行为。
- 未证明检查视图注入方式、源码映射在复杂模板中的完整性。
- 原始运行脚本缺失，无法从本仓库复现当时的具体断言输出。

## 数据

- [package.json](data/typescript-language-service/package.json)
- [package-lock.json](data/typescript-language-service/package-lock.json)

依赖清单能证明实验环境和版本，不构成可运行样例。
