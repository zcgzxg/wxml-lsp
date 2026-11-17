# W3 语义工具与安全编辑证据

更新日期：2026-11-17。

本记录保存 S 语义工具合并到集成分支后的验收快照。它只覆盖 S 的 LSP 语义工具、安全编辑和
相应回归测试，不把“工具已合入”扩大为 Q2 性能/恢复完成、完整 Linux UI 验收、marketplace
发布或跨平台验收。Q1 仍是 `f46dec9` 的本地交付结果，本记录不重新验收 Q1。W4 之后的最终状态
（Q2、release 和 Linux Zed 运行时冒烟）见 [W4 最终验收与发布证据](full-feature-final.md)；
本记录的 W3 数字保持原样。

## 1. 基线

| 项 | 值 |
| --- | --- |
| 分支 | `feat/full-integration` |
| W3 合并提交 | `379b097`（`merge: integrate semantic tools and safe workspace edits`） |
| S 最终提交 | `dd15bc1`（`fix: bound imported-template scans and close semantic gaps`） |
| S 功能提交 | `411e4c1`（初版）、`d08844f`（第 1 轮审核修复）、`dd15bc1`（第 2 轮审核残留修复） |
| S base | `a441de9`（W2） |
| Q1 基线 | `f46dec9`，本记录不改变其结论 |
| Q2 | 在另一个 worktree 实现；截至 W3 记录**未完成**（W4 最终状态见 [最终证据](full-feature-final.md)） |
| 环境 | Linux x86_64；Rust/Cargo 1.100.0；Node 24.22.2；npm 11.19.1 |

W3 之前，集成分支已包含 P、E、C、N、T、W2 和 Q1。W3 只新增 S 的语义工具与安全编辑；
没有新增 marketplace 上传、远端 release/tag、发布者签名或标准 Zed UI 重跑。

## 2. 交付内容

S 在 `initialize` 中只声明有真实 handler 和失败安全测试的能力。`WorkspaceEdit.documentChanges`
不被客户端声明时，`rename` 和 `codeAction` 不广告；客户端能力协商结果有测试覆盖。

| 能力 | 已实现范围 | 主要限制 |
| --- | --- | --- |
| `textDocument/references` | WXML 数据路径、事件处理函数、组件属性、template 名和 class 的静态引用；返回 LSP `Location[]` | 请求位置所在 WXML 必须打开；覆盖不完整时返回空列表，不把部分结果冒充完整 |
| `workspace/executeCommand` (`wxml.references`) | 同一 references 查询的只读命令包装，返回 `locations` + `coverage`（`supported`/`partial`/`unsupported`） | 只读；不写盘；不提供跨文件事务 |
| `textDocument/prepareRename` | 返回可编辑 token 的范围和 placeholder；不可证明安全时返回 `null` | 不猜测目标；不支持的 token 不返回编辑范围 |
| `textDocument/rename` | 安全 `EditPlan`：data、组件 `properties`、事件处理函数、`wx:for` 绑定、class、template import/name | 自定义组件 tag rename、静态 include 导致的 caller 覆盖不完整、动态访问、共享 importedData 跨脚本、快照截断/超限、跨视图冲突保守拒绝 |
| `textDocument/codeAction` | 字段拼写 quickfix（编辑距离 ≤ 2）和创建事件 handler；Component `methods` 插入带逗号 | 只有 `quickfix`；不提供 refactor、组织 import 或通用代码生成 |
| `textDocument/semanticTokens/full` | UTF-16 编码；WXML 结构、Mustache 和 WXS 静态 token；CRLF 与 emoji 覆盖 | 只处理可完整解析的模板；线性预算，异常输入返回空 token 流而不是部分伪造 |
| `workspace/symbol` | 聚合多 app 与静态 imported template 中的 template/组件符号；最多 512 项 | 索引不完整、预算耗尽或导入无法解析时返回空并写日志，不返回部分索引 |
| `textDocument/inlayHint` | 基于静态 WXS/表达式事实的类型提示 | 最多 1000 条；默认开启，`semanticTools.inlayHints=false` 可关闭并同时去掉能力广告 |
| `textDocument/signatureHelp` | WXS 静态子集的函数签名和 active parameter | 不执行 WXS；动态函数不推断 |
| `textDocument/codeLens` | template 调用计数和组件引用计数；include 的计数带当前文档锚点并区分 included 数量 | 模板/组件 lens 各最多 64 条；默认开启，`semanticTools.codeLens=false` 可关闭；不完整时标题带 `+` 或明确说明 |
| `textDocument/documentSymbol` | 复用 W2 已有实现，W3 未改变其语义 | 见 W2 基线 |

组件属性 rename 有静态 include markup 的覆盖测试；上述 include 拒绝适用于不能证明完整覆盖的
caller/data/event/class/template/loop 目标，不表示所有 include 场景一律拒绝。

### 2.1 安全编辑约束

`rename` 和 `codeAction` 使用同一套 `EditPlan` 校验：

- 只接受 `WorkspaceEdit.documentChanges`；客户端未声明时能力不广告，请求返回明确错误。
- 打开文件使用客户端 version；关闭文件在生成计划时重新读取内容，并以 `version: null` 返回，
  不把内部 revision 冒充客户端 version。
- 每个替换都校验 expected 原文、UTF-16 范围和文件身份；重复编辑去重，重叠或冲突整份拒绝。
- 目标路径必须可写、位于项目内；symlink 和生成目录被拒绝。
- 失败返回零编辑；语言服务只生成编辑，**不直接写盘**。
- 不提供跨文件事务、原子提交或 undo；客户端仍是编辑应用和撤销的唯一执行者。

### 2.2 保守拒绝

以下情况不做编辑，返回空、`null` 或 `-32803`，而不是产生部分/错误编辑：

- 自定义组件 tag rename（没有 owning `usingComponents` 别名和全部项目用法的完整覆盖）。
- 页面/组件 `data` 与 `properties` 的合并命名空间冲突，双向都拒绝。
- 共享 importedData 跨脚本、静态 include 隐藏 caller 侧 data/event/class/template/loop 引用、动态 template/`is`、动态 `wx:for`、动态 class 和
  动态 `setData`/`this` 别名访问。
- 文件含 `<template name>` 或 template import 时，页面 data rename 保守拒绝。
- 未落盘 WXML、源快照截断、单文件或总预算超限、`TooLarge`/`Unreadable` 源文件遗漏。
- 跨视图冲突、保留字、作用域捕获、对象 shorthand、重复 loop 绑定。
- 硬链接未检测；这是已知边界，不能把 `writable_target` 的 symlink 校验当作完整文件身份保证。

## 3. 评审与修复

S 经历了两轮正式 Rust 审核和两轮正式安全审核，结论都是 `CHANGES_REQUESTED`：

| 记录 | 对象 | 结论摘要 |
| --- | --- | --- |
| `reviews/full-semantic-tools-r1.md` | 初版 `411e4c1` | 8 BLOCKING / 9 MAJOR / 13 MINOR；组件属性 rename 不同步自身模板、shorthand/setData 误编辑、codeAction 逗号、截断快照不降级等 |
| `reviews/full-semantic-tools-r2.md` | `d08844f` | 3 BLOCKING（S-B1 组件 data/properties 冲突、S-B8 源快照不完整、S-M1 动态/跨脚本 setData）/ 3 MAJOR；最终正式轮，不提出第 3 轮流程 |
| `reviews/full-semantic-tools-security-r1.md` | 初版 `411e4c1` | 5 HIGH：2 项可复现主线程冻结、3 项静默不完整/错误编辑 |
| `reviews/full-semantic-tools-security-r2.md` | `d08844f` | 1 HIGH：`<import>` 模板在 codeLens/workspaceSymbol 中重复读盘和解析，实测 6.3–30.9s 冻结；1 MEDIUM 二次复杂度 |

`dd15bc1` 针对第 2 轮残余项做了修复，并补了对应回归测试：

- 请求级 `FileId` import cache，64 文档 / 4 MiB 预算；codeLens 和 workspaceSymbol 复用同一缓存，
  预算耗尽标记不完整而不是重复读盘。
- 预计算 template 定义边界和 loop 父链，scope 查询改为 O(log n + depth)。
- 组件 owner 的保留名包含 `data`、`properties`、`methods`；`data`/`properties` 冲突双向拒绝。
- `TooLarge`/`Unreadable` 源文件视为不完整，rename/prepareRename/references 拒绝或返回 partial。
- 扫描所有 receiver 的嵌套 `setData` 路径，拒绝带动态 data 访问的 `this` 别名，跳过其它页面的
  自有 `this.setData`。
- include-only 组件 codeLens 锚定当前文档，并如实报告 included 数量。

按用户设定的审核轮次上限，**没有开启第 3 轮正式审核**。残余项由集成侧探针复验；复验的回归
用例在 W3 固定日志中通过（见 §4）。这不构成 `APPROVED` 结论，也不把“探针通过”写成“正式审核
通过”。原始审核记录和完整日志没有复制进仓库，名称、SHA-256 和关键输出行见
[W3 最小原始数据](data/w3/test-summary.md)。

## 4. 测试结果

以下结果来自 W3 固定日志，**不与 W2 或 Q1 的计数混用**。

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| Rust workspace | `cargo test --workspace -- --test-threads=4` | `1251 passed / 0 failed / 21 ignored`（45 个套件，含 doctests） |
| S 语义工具 | `cargo test -p wxml-lsp --test semantic_tools` | `41 passed / 0 failed` |
| Node 后端（串行） | `cd backends/typescript && node --test --test-concurrency=1 test/*.test.ts` | `326 / 326` |
| Node 后端（并行对照） | `cd backends/typescript && npm test` | `324 / 326`；两处宿主负载墙钟超时，不作为功能失败 |
| 真实 Node ignored 门禁 | 默认 `cargo test` 不运行 | `21 ignored` = `stdio_type_loop` 9 + `real_node` 5 + `wave5_acceptance` 7 |

说明：

- W3 固定日志中的 `semantic_tools` 有 41 个测试；其中两个真实 Node 用例在
  `backends/typescript/dist/server.js` 存在时实际执行，日志中没有 `skipping real Node LSP test`。
- W3 **没有重新执行** `WXML_TEST_NODE=1` 的显式真实 Node 门禁；`stdio_type_loop` 9、
  `real_node` 5、`wave5_acceptance` 7 仍是 ignored 计数，不写成 W3 通过。W2 的显式真实 Node
  结果继续只属于 W2 记录。
- 并行 Node 运行的两处失败是 `cold start 11959.45 ms > 8000 ms` 和 15s server timeout；
  同一套件串行 `326/326`。W3 采用串行结果，并行计数只作负载敏感性对照。
- `semantic_tools` 的 8000 条 `wx:for` 重命名断言（1 秒上限）在并行套件中曾 `1.10s > 1s`，
  单独运行 `0.68–0.85s` 通过；阈值未改。该断言只覆盖线性预算，不是 Q2 大项目性能验收。
- `editors/zed` 的 `25 passed` 来自 W2 记录；W3 没有重跑 Zed host 测试，也没有重跑标准
  Zed UI 安装/交互。
- Q1 `cargo test -p wxml-delivery` 的通过结果仍来自 W2/Q1 记录；W3 workspace 运行包含 delivery
  测试，但不把这次合并当作 Q1 的新验收。

## 5. 能力边界

- references/rename 的请求目标只接受 WXML；编辑可以落到关联脚本，但不接受直接在 TS/JS 文档上
  发起这些语义请求，也不替代用户已有的 JS/TS 语言服务。
- workspaceSymbol 的“空结果”是保守行为：索引不完整、预算耗尽或导入无法解析时返回空并写日志，
  不返回部分索引。
- codeLens/inlayHint 默认开启，但可被 `initializationOptions.semanticTools` 关闭；关闭后不广告
  对应能力。template/组件 lens 有独立上限，超出时标题明确标记不完整。
- 仍然没有跨文件事务、原子多文件提交或 undo；服务端不写盘，客户端负责应用和撤销。
- 硬链接未检测；symlink 和生成目录已拒绝，但不能把这两者等同于完整文件身份安全。
- 没有跨 app 引用推断；多 app 的 references/rename 仍遵守显式 app 边界。
- 没有把 `semantic_tools` 的回归测试扩大为所有历史 WXML 写法的验收。

## 6. 未完成与风险

- **Q2 在 W3 时未完成**：性能/取消/恢复完善、大项目预算和最终 Linux UI 验收仍在另一个 worktree 实现，
  不在 W3 交付内。W4 最终状态见 [最终证据](full-feature-final.md)；W3 的 8000-loop 断言只说明线性预算回归，不是 Q2 结论。
- 没有 marketplace 发布、远端 release/tag、自动更新或发布者签名。Q1 仍是本地打包/验证安装器；
  信任链仍是 TLS + checksum，ADR-0007 的残留风险描述不变。
- 没有在最终版本上重跑标准 Zed UI 安装；P8 第 13 节的安装/hover/definition/completion/
  diagnostics/重启恢复是既有版本证据，不能写成 W3 复跑结果。
- 没有 macOS/Windows 实机验收；没有完整多 workspace、根配置热更新或反向编辑。
- S 没有第 3 轮正式审核；第 2 轮残留项由集成探针和回归测试复验，正式审核记录仍停留在
  `CHANGES_REQUESTED`。

## 7. 相关记录

- [W3 最小原始数据](data/w3/test-summary.md)
- [W2 集成与 Q1 交付](full-feature-w2.md)
- [WXML 语言功能基线](../reference/wxml-language-feature-baseline.md)
- [项目 README](../../README.md)
- [ADR-0006：显式小程序根与类型项目根](../adr/0006-explicit-mini-program-roots.md)
- [ADR-0007：本地交付与安装信任模型](../adr/0007-local-delivery-trust-model.md)
- [P8 Zed 扩展证据](p8-zed.md)
- 外部审核与日志（未入库，校验值见 §1）：`reviews/full-semantic-tools-r1.md`、
  `reviews/full-semantic-tools-r2.md`、`reviews/full-semantic-tools-security-r1.md`、
  `reviews/full-semantic-tools-security-r2.md`、`test-logs/integration-s-rust.log`、
  `test-logs/integration-s-node-test-serial.log`
