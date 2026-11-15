# 官方模板分析器核验（2026-11-15）

## 对象与环境

- 被核验对象：官方模板分析器，版本 `1.0.0`，提交 `3aed66a785c334a5349907b79b955429ea6b725a`。来源与许可证见[来源记录](../reference/provenance.md)。
- TypeScript：`5.9.3`
- 小程序 backend：`miniprogram-config`
- 核验方式：客户端侧 TypeScript 集成与纯 Node WASM 两条路径；另做 LSP 握手兼容性测试。
- 业务项目：未修改。

## 方法

- 准备页面和组件的两种写法：导出式（模板能取得组件类型）与裸 `Page(...)`/`Component(...)`。
- 在虚拟 WXML/TS 样本上请求 hover 和诊断。
- 用同一导出式样本走纯 Node WASM 路径，比较结果。
- 修改被导入的 TS 文件但不刷新 WXML，检查类型是否即时更新；再刷新 WXML 观察结果。
- 发送缺少特定字段的 LSP 初始化请求，记录进程退出和错误信息。

## 结果

### 导出式写法

- 页面和组件样本的 `user.Name` hover 均为 `(property) User.Name: string`。
- 原始文档字段包含中文说明“用户姓名：用于展示。”，说明能够随类型返回。
- 诊断码 `2339`，覆盖：
  - 不存在的 `Missing`，第 0 行字符 30–37。
  - 数字上的 `Age.toUpperCase()`，第 0 行字符 53–64。
- 两条诊断范围与表达式位置一致。

### 裸写法与空配置

- 裸 `Page(...)` / `Component(...)` 的 hover 为 `any`，没有诊断。说明该路径没有把注册信息转换成可检查的数据上下文。
- 页面 JSON 为空时 hover 为 `null`。

### 纯 Node WASM

- 与客户端路径一致，导出式样本得到相同的 hover、中文说明和两条 `2339` 诊断。
- 说明核心分析能力可在 Node 侧运行，不必只依赖其 Rust 二进制。

### 未保存导入类型

- 只修改被导入的 TS、不刷新 WXML 时，hover 仍是旧类型 `(property) User.Name: string`。
- 刷新 WXML 后变为新类型 `(property) User.Name: number`。
- 即未保存依赖变化不会自动传播，需要模板侧重新请求。

### LSP 握手

| 请求变化 | 结果 |
| --- | --- |
| 缺少 `workDoneToken` | 退出码 1，stderr：`missing field \`workDoneToken\`` |
| 客户端不支持 `declarationLinkSupport` | 退出码 1，stderr：`unsupported client` |
| 只提供标准 `workspaceFolders` | 退出码 1，stderr：`missing field \`workspaceFolders\`` |

该实现对客户端能力要求较严格，Zed 接入前需要验证这些字段和能力声明。

### 直接 Rust LSP 探针

在同批样本中，直接连接其 Rust LSP 的 `rustHover` 全部为 `null`，而客户端 TypeScript 集成能返回完整结果。不能据此断言 Rust 二进制永远不提供 hover，但至少说明“只启动 Rust 服务”不等于类型能力可用。

## 限制

- 只有四个主要写法样本和一个空配置样本，不代表所有组件形态。
- 未在 Zed 中验证，也未覆盖多项目、路径别名、循环和条件窄化。
- 未保存类型测试只观察 WXML 刷新前后的 hover，不包含编辑器事件顺序和版本失效。
- 探针依赖未入库的解包 VSIX 和生成的类型声明文件，因此运行器未复制；本记录为 E1。
- 结果只代表上述版本、backend 配置和样本。

## 对项目的影响

- 模板检查视图、位置映射和组件资料有复用价值。
- 类型能力依赖客户端 TypeScript 集成，不能只连接 Rust 服务。
- 裸 `Page`/`Component` 支持、未保存依赖传播和 Zed 握手兼容性都需要本项目独立解决。
- 纯 Node WASM 路径支持“类型分析放在 Node 后端”的可行性。

这些结论支持[ADR-0002](../adr/0002-template-type-checking-and-source-maps.md)中“Rust 管模板转换、Node 管真实类型上下文”的边界，也提示[ADR-0003](../adr/0003-versioned-document-synchronization.md)中的未保存同步不能省略。

## 数据

- [result.json](data/official-template-analyzer-audit/result.json)：客户端路径结果
- [wasm-result.json](data/official-template-analyzer-audit/wasm-result.json)：纯 Node WASM 与未保存更新结果
- [handshake-results.json](data/official-template-analyzer-audit/handshake-results.json)：握手测试
- 虚拟样本：
  - [page-export.virtual.txt](data/official-template-analyzer-audit/page-export.virtual.txt)
  - [component-export.virtual.txt](data/official-template-analyzer-audit/component-export.virtual.txt)
  - [page-bare.virtual.txt](data/official-template-analyzer-audit/page-bare.virtual.txt)
  - [component-bare.virtual.txt](data/official-template-analyzer-audit/component-bare.virtual.txt)

样本中的生成类型声明路径已替换为 `<probe-root>/probe/generated-miniprogram-config.d.ts`；原始输出中的外部对象名称已替换为 `<analyzer>`。该生成文件、发布清单和解包后的扩展未入库。
