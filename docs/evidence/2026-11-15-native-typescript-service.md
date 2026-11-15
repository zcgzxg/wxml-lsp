# TypeScript 原生语言服务实验（2026-11-15）

## 对象与环境

- TypeScript 原生服务稳定版：`7.0.2`
- TypeScript 原生服务开发版：`7.1.0-dev.20261114.1`，linux-x64
- 接入方式：`tsc --lsp --stdio`，LSP 请求与响应
- 业务项目：未修改

## 方法

在同一组虚拟文档上分别运行两个版本：

1. `included`：虚拟 TS 文件位于含 `tsconfig.json` 的目录，配置通过默认包含规则覆盖该文件。
2. `explicit-files`：`tsconfig.json` 的 `files` 只列出磁盘上的模型文件。
3. `custom-scheme`：虚拟文件使用自定义 URI scheme（`virtual://...`）并导入同目录相对模块。

每个场景检查：文件是否落盘、位置编码、项目归属、hover、诊断、修复后诊断，以及未保存依赖文档更新。两个版本结果一致。

## 结果

### `included`

- 虚拟文件未落盘。
- 位置编码为 `utf-16`。
- 项目信息返回实际 `tsconfig.json` 路径。
- hover 返回 `(property) User.Name: string` 和真实模型中的 JSDoc `Display name from the real model.`。
- 不存在的字段产生 `2339` 诊断，范围覆盖字段名。
- 修复表达式后诊断项为 0。
- 修改被导入模型的未保存 JSDoc 后，hover 显示 `Updated unsaved JSDoc.`。
- 结论：虚拟 `file://` 文档在默认包含规则下能进入真实项目并复用真实类型与文档。

### `explicit-files`

- hover 和诊断仍能工作。
- 项目信息中的 `configFilePath` 为空，虚拟文件没有归入预期的配置项目。
- 结论：仅依赖 `files` 列表时，虚拟文件的配置归属不可靠。

### `custom-scheme`

- hover 返回 `any`。
- 诊断码 `2307`：无法找到 `./model` 或其类型声明。
- 结论：自定义 URI scheme 不能按普通文件路径解析相对模块，不能直接作为检查视图的文件身份。

## 限制

- 只验证普通虚拟 TS 文件，不是完整小程序检查视图。
- 未覆盖全部 `tsconfig` 组合、项目引用、路径别名、多根和编辑器事件顺序。
- 未在 Zed 中验证。
- 两个版本结果相同，不代表未来版本保持相同行为。
- 实验只保留运行脚本、结果和开发版包清单；原生 TypeScript 二进制未入库，复跑需自行取得对应版本。

## 对项目的影响

- 原生 TypeScript 后端具备可用的 LSP 能力，虚拟文件能获得真实类型和 JSDoc。
- 检查视图应使用普通 `file://` 身份，避免自定义 scheme。
- 不能假定虚拟文件会自动归入显式 `files` 配置；项目归属需要显式验证和回退策略。
- 这支持[ADR-0001](../adr/0001-layered-language-service-architecture.md)中“原生后端作为后续候选、先通过同一验收矩阵”的判断，也为[ADR-0002](../adr/0002-template-type-checking-and-source-maps.md)的文件身份设计提供约束。

## 数据

- [probe.py](data/native-typescript-service/probe.py)：可复跑探针；通过 `PROBE_TSC` 指定 TypeScript 原生二进制，通过 `PROBE_ROOT` 指定仓库外的运行目录
- [probe-stable-results.json](data/native-typescript-service/probe-stable-results.json)：7.0.2 结果
- [probe-results.json](data/native-typescript-service/probe-results.json)：7.1 开发版结果
- [package.json](data/native-typescript-service/package.json)：7.1 开发版包清单

结果中的本机配置路径已替换为 `<probe-root>/fixtures/included/tsconfig.json`。
