# WXML 语言功能基线

更新日期：2026-11-17。

本文件是本项目 WXML 通用语言能力的自有需求基线。功能取舍、默认行为和验收条件以本文为准，不再依赖外部插件的 README、源码或本机实验目录。历史来源仅用于许可证与归属说明，见 [来源与许可证记录](provenance.md)。

当前状态：F-01–F-04 已完成；截至 W4（`9b804d4`，2026-11-17），W2 功能、S 语义工具和 Q2 调度/恢复均已合入，F-05–F-18 的逐项状态见 §2.1，S 工具见 §2.2，Q2 性能/恢复见 §2.3。`已实现（受限）` 表示主路径可用且有测试，但仍有明确降级或边界；`部分实现` 表示只覆盖了基线的一部分。最终 release 和 Linux Zed 运行时冒烟已完成，但完整多 workspace、大项目语料、Windows/macOS 实机和 marketplace/发布者签名仍未完成，不能从相邻功能推导为已完成。模板类型体验的优先级与验收见[项目 README](../../README.md#31-第一优先级模板类型体验)。

## 1. 优先级与范围

1. 先完成 T1–T9 模板类型闭环：真实类型、字段说明、精确诊断、模板作用域、条件窄化、项目类型上下文和未保存一致性。
2. 在类型闭环可验收后，按日常使用价值补齐 F-01–F-18 通用语言功能。
3. 每项功能必须说明数据来源、支持范围、降级行为和验收样本。不能只以“补全列表非空”或“hover 有返回”作为通过条件。

首阶段只面向原生微信小程序 `.wxml` 及其关联 JS/TS、JSON、样式文件。Vue/Pug 内嵌模板、Emmet、一键创建组件、JS 变量自定义装饰、自动修改编辑器配置、扩展市场发布和上游 PR 工作流不属于需求。

## 2. 功能清单

| 编号 | 功能 | 行为基线 | 主要数据来源 | 阶段 |
| --- | --- | --- | --- | --- |
| F-01 | 标签名补全 | 输入 `<` 后给出标签候选及说明；保留已输入前缀，不破坏后续文本 | 本地版本化基础组件元数据 | 5 |
| F-02 | 属性名补全 | 标签内输入空格或属性前缀后给出适用属性；按标签、组件和已有属性过滤 | 本地元数据、`usingComponents`、组件声明 | 5 |
| F-03 | 属性值补全 | 仅对枚举型属性给出值候选；替换完整属性值范围 | 本地元数据中的枚举定义 | 5 |
| F-04 | 指令与事件补全 | 补全 `wx:if/elif/else/for/key/for-item/for-index` 及常用事件绑定形式 | 本地元数据、模板语法 | 5 |
| F-05 | 上下文筛选 | 根据标签、组件和模式筛选候选，例如 `picker` 的 `mode` 影响可用属性 | 本地元数据中的条件规则 | 5 |
| F-06 | 自定义组件补全 | 从页面/组件 JSON 的 `usingComponents` 解析组件，补全标签和属性说明 | 项目 JSON、组件声明、类型后端 | 5 |
| F-07 | 表达式成员与事件处理补全 | 在 `{{ }}` 中按模板作用域补全成员；事件属性中补全处理函数 | 类型后端、模板作用域、脚本声明 | 4–5 |
| F-08 | Hover | 显示表达式类型、字段 JSDoc、声明归属；显示组件属性、标签和属性说明；WXS 模块/成员与打开的 `.wxs` 内声明、导出键和引用也可 hover | 类型后端、项目模型、本地元数据 | 2–5 |
| F-09 | 定义跳转 | 跳转到字段声明、组件文件、class、资源文件和事件处理函数 | 类型后端、项目索引、样式索引 | 4–5 |
| F-10 | 诊断 | 标记未知字段、非法类型操作和不兼容绑定；范围精确，修复后清除 | 类型后端、模板语义、诊断策略 | 2–4 |
| F-11 | Class 补全与导航 | 从同名样式文件和配置的全局样式补全 class，并展示注释说明 | 项目样式索引 | 5 |
| F-12 | 资源路径链接 | 对配置的路径属性提供可点击链接；只链接存在的文件 | 项目文件解析 | 5 |
| F-13 | 闭合标签与配对 | 自动闭合、标签配对高亮和成对重命名；不破坏 `wxs` 与表达式 | Zed 查询、必要的 LSP 编辑 | 5 |
| F-14 | Snippets | 提供内置片段和用户片段；占位符跳转与说明稳定 | 本地片段定义、用户配置 | 5 |
| F-15 | 格式化 | 保真格式化；解析失败时不修改文件 | 语法树、格式化器 | 6 |
| F-16 | 配置、依赖、缓存 | 解析项目根、页面/组件归属、配置变更和依赖失效；内存覆盖层优先 | 项目模型、文件监听、LSP 协调器 | 3–4 |
| F-17 | 类型与项目上下文 | 尊重适用 `tsconfig`、路径别名、模块解析和未保存文档 | 类型后端、项目模型 | 2–4 |
| F-18 | 多根与多项目 | 多工作区、多小程序根、多 `tsconfig` 和重启后状态隔离 | 项目模型、服务实例标识 | 3–6 |

### 2.1 实现状态（F-05–F-18，含 W3/W4 更新）

下表以 W2 验收为底；F-09、F-11、F-13、F-18 已按 W3 的 S 和 W4 的最终验收更新，其余行未在
W4 重跑。S 的逐项工具状态见 §2.2，Q2 调度/恢复见 §2.3，行为边界见 §3.9/§3.10。

> **2026-11-20 项目瘦身说明**：ADR-0009 删除了 native parity、`stdio_type_loop`、Q2 稳定性和性能门禁，
> 以及 `docs/evidence`。下表中这些文件名保留为历史标记，不再可执行；历史证据见 tag
> `pre-slim-20261120`。当前默认覆盖以 `component_intelligence.rs`、`styles_resources.rs`、
> `styles_diagnostics.rs`、`semantic_surface.rs`、`structural_editing.rs`、
> `full_feature_matrix_editing.rs`、`project_host.rs`、`lsp.rs` 和
> `native_lsp_document_symbols.rs` 为准。

| 编号 | 状态 | 已实现范围 | 主要证据 | 仍待/限制 |
| --- | --- | --- | --- | --- |
| F-05 | 部分实现 | 数据化条件规则至少覆盖 `picker` 各 mode 和 `input` type；补全与本地诊断共用规则；未知/动态 mode 保留合法候选 | [component_intelligence.rs](../../crates/wxml-lsp/tests/component_intelligence.rs)、`full_feature_matrix.rs`（历史，P7 删除；IT-03） | 未覆盖全部组件的条件属性；规则表需继续数据化扩展 |
| F-06 | 已实现（受限） | 页面/组件自身/app `usingComponents`、本地同名覆盖、`miniprogram_npm` 本地构建、组件标签/属性/事件补全、组件 definition | [component_intelligence.rs](../../crates/wxml-lsp/tests/component_intelligence.rs)、`full_feature_matrix.rs`（历史，P7 删除；IT-02） | 插件/远程组件仅在本地有可信元数据时建模；无远程下载或执行 |
| F-07 | 已实现 | `wx:for` 作用域、表达式成员补全、事件处理函数补全；后端离线时静态通道保留 | stdio_type_loop.rs（历史，ADR-0009 删除；见 tag pre-slim-20261120）、native_parity/real_matrix.rs（历史，ADR-0009 删除；见 tag pre-slim-20261120）、`real_node.rs`（历史，P7 删除） | 动态/不可解析类型保持降级 |
| F-08 | 已实现（受限） | 表达式类型/JSDoc/声明归属；内置标签/属性/事件静态文档；组件属性、事件与来源 hover；WXS 模块 hover 的 range 落在 mustache token，导出列表带签名/类型；WXS 成员展示签名与多行 JSDoc（`@param`/`@returns` 的 `{}` 保持可读）；打开的 `.wxs` 内声明、导出键与文件内引用可 hover；不依赖 Node 的静态 hover | [component_intelligence.rs](../../crates/wxml-lsp/tests/component_intelligence.rs)、[metadata.rs](../../crates/wxml-lsp/src/metadata.rs)、[semantic_surface.rs](../../crates/wxml-lsp/tests/semantic_surface.rs) | 目录仍是手写子集；插件/远程组件仅显示降级说明；WXS 静态子集不执行、不推断动态导出，也不提供跨文件 rename |
| F-09 | 已实现（受限） | 字段/事件/组件文件/组件属性/class/本地资源/template/import/WXS 导出 definition；S 增加 data、事件、组件属性、template 和 class 的 references，覆盖不完整时保守返回空 | [styles_resources.rs](../../crates/wxml-lsp/tests/styles_resources.rs)、[semantic_surface.rs](../../crates/wxml-lsp/tests/semantic_surface.rs)、ADR-0009（历史证据见 tag pre-slim-20261120）、ADR-0009（历史证据见 tag pre-slim-20261120） | 请求目标只接受已打开的 WXML；跨 app 不推断；插件/远程目标不猜测；partial coverage 不冒充完整 |
| F-10 | 部分实现 | 未知字段/成员、绑定不兼容、picker/input mode、资源不存在、可证明 class 缺失诊断；修复后清除 | [component_intelligence.rs](../../crates/wxml-lsp/tests/component_intelligence.rs)、[styles_diagnostics.rs](../../crates/wxml-lsp/tests/styles_diagnostics.rs) | 不是完整模板语义检查；动态/不完整索引抑制猜测 |
| F-11 | 已实现（受限） | WXSS/SCSS/CSS class 索引；同文件、`@import`/`@use`/`@forward`、app 样式、显式全局样式；`styleIsolation`；class 补全/hover/definition；S 增加 class references/rename | [styles_resources.rs](../../crates/wxml-lsp/tests/styles_resources.rs)、[styles.rs](../../crates/wxml-project/tests/styles.rs)、[semantic_surface.rs](../../crates/wxml-lsp/tests/semantic_surface.rs)、ADR-0009（历史证据见 tag pre-slim-20261120） | 动态插值、mixin/`@extend` 生成类不猜测；预算截断标记不完整 |
| F-12 | 已实现（受限） | 本地资源补全/documentLink/definition/存在性诊断；新增删除和 watched-file 更新；路径边界与 symlink 校验 | [styles_resources.rs](../../crates/wxml-lsp/tests/styles_resources.rs)、[full_feature_matrix_editing.rs](../../crates/wxml-lsp/tests/full_feature_matrix_editing.rs)（IT-10） | 只处理本地资源；不访问远程 URL，不处理插件包内未落盘资源 |
| F-13 | 部分实现 | 开始标签补全、`</` 栈补全、linkedEditingRange、Zed 括号/引号/Mustache 配对；S 增加 template、loop 绑定、class、组件属性、事件处理函数和 data 的安全 rename；自闭合与嵌套同名兄弟不误配 | [structural_editing.rs](../../crates/wxml-lsp/tests/structural_editing.rs)、[semantic_surface.rs](../../crates/wxml-lsp/tests/semantic_surface.rs)、ADR-0009（历史证据见 tag pre-slim-20261120）、ADR-0009（历史证据见 tag pre-slim-20261120） | 自定义组件 tag rename 和普通 tag rename 保守拒绝（没有完整 `usingComponents` 别名/项目用法覆盖）；没有 LSP on-type 自动闭合；跨文件事务/undo 不提供 |
| F-14 | 已实现（受限） | 内置标签/属性 snippets、工作区/用户 snippets、`enabled`/`builtin` 覆盖、占位符插入安全、watched-file 热更 | [structural_editing.rs](../../crates/wxml-lsp/tests/structural_editing.rs) | 只读取 `snippetsPath` 或 `.wxml-lsp/snippets.json`；不读取 Zed 私有配置；无 marketplace 片段包 |
| F-15 | 已实现（受限） | 保真 document/range formatting；只改开始标签内部 ASCII 空白；保留文本/注释/实体/Mustache/WXS/引号/重复属性；解析失败零编辑；CRLF/emoji/中文 | [structural_editing.rs](../../crates/wxml-lsp/tests/structural_editing.rs)、[full_feature_matrix_editing.rs](../../crates/wxml-lsp/tests/full_feature_matrix_editing.rs)（IT-11） | 不重排文本节点或整份文档缩进；多行标签中段无法安全推导时跳过 |
| F-16 | 已实现（受限） | 显式 app registry、依赖图、overlay、缓存、watched files、tsconfig/样式/snippet 失效；无效配置保留错误状态 | [project_host.rs](../../crates/wxml-lsp/tests/project_host.rs)（含多 app 隔离；原 `multi_app_projects.rs` 已随 P7 删除） | 根配置只在启动时消费；无完整多 workspace 或跨 app 自动依赖图 |
| F-17 | 已实现（受限） | Behavior/共享 data/props、复杂 props、guards/loops、JS/JSDoc、template/import/include、WXS 静态子集、未保存跨文件 overlay、native 引擎重启重放 | stdio_type_loop.rs（历史，ADR-0009 删除；见 tag pre-slim-20261120）、native_parity/real_matrix.rs（历史，ADR-0009 删除；见 tag pre-slim-20261120）、`real_node.rs`/`backends/typescript/test/`（历史，P7 删除） | 模板体不逐调用实例检查；WXS 不执行；动态 Behavior/WXS/template 明确降级 |
| F-18 | 部分实现 | 单 workspace 内显式多 app、独立 tsconfig/projectId/overlay/缓存、重启稳定身份、最长目录匹配；S 增加多 app + imported template 的 `workspace/symbol`；Q2 增加根移除/重加 overlay 重放、资源/页面增删恢复和关闭/重开 | [project_host.rs](../../crates/wxml-lsp/tests/project_host.rs)、[semantic_surface.rs](../../crates/wxml-lsp/tests/semantic_surface.rs)、q2_stability.rs（历史，ADR-0009 删除；见 tag pre-slim-20261120）、ADR-0009（历史证据见 tag pre-slim-20261120）（原 `multi_app_projects.rs`/`q2_recovery.rs` 已随 P7 删除） | 不是完整多 workspace；无多根热加/根热更新；无跨 app 依赖归属推断；多 app 重放受 active project 限制；索引不完整/预算耗尽时 workspaceSymbol 返回空并写日志 |

### 2.2 S 语义工具状态（W3 合入，W4 最终复验）

S 只广告有真实 handler 和失败安全测试的能力。客户端未声明 `WorkspaceEdit.documentChanges`
时，`rename` 和 `codeAction` 不广告。两轮正式 Rust/安全审核结论均为 `CHANGES_REQUESTED`；
`dd15bc1` 修复第 2 轮残留项并由集成探针复验，按用户上限未开第 3 轮正式审核，不标记为
`APPROVED`。W4 没有再改 S 语义，最终 release 冒烟覆盖了 hover/definition/completion/
diagnostics；证据见 ADR-0009（历史证据见 tag pre-slim-20261120） 与
ADR-0009（历史证据见 tag pre-slim-20261120）。

| 工具 | 状态 | 已实现范围 | 主要证据 | 仍待/限制 |
| --- | --- | --- | --- | --- |
| references | 已实现（受限） | `textDocument/references` 与只读 `wxml.references` 命令；data、事件、组件属性、template、class；命令返回 `locations` + `coverage` | [semantic_surface.rs](../../crates/wxml-lsp/tests/semantic_surface.rs)、ADR-0009（历史证据见 tag pre-slim-20261120） | 目标 WXML 必须打开；覆盖不完整时普通请求返回空，命令返回 `partial`/`unsupported` |
| prepareRename / rename | 已实现（受限） | data、组件 `properties`、事件 handler、loop 绑定、class、template name/import；安全 `EditPlan` | 同上 | tag rename、静态 include 导致的 caller 覆盖不完整、动态访问、共享 importedData、快照截断/超限、跨视图冲突保守拒绝 |
| codeAction | 已实现（受限） | 字段拼写 quickfix（编辑距离 ≤ 2）和创建事件 handler；Component `methods` 插入逗号安全 | 同上 | 只有 `quickfix`；不提供 refactor/组织 import |
| semanticTokens/full | 已实现（受限） | UTF-16；WXML 结构、Mustache、WXS 静态 token；CRLF/emoji | 同上 | 只处理可完整解析模板；异常/超限返回空 token 流 |
| workspace/symbol | 已实现（受限） | 多 app + static imported template 的 template/组件符号；最多 512 项 | 同上 | 索引不完整、预算耗尽或导入不可解析时返回空并写日志 |
| inlayHint | 已实现（受限） | 静态 WXS/表达式类型提示；最多 1000 条 | 同上 | 默认开启；`semanticTools.inlayHints=false` 关闭并去掉能力广告 |
| signatureHelp | 已实现（受限） | WXS 静态子集签名和 active parameter；触发字符 `(`、`,` | 同上 | 不执行 WXS；动态函数不推断 |
| codeLens | 已实现（受限） | template 调用计数、组件引用计数；include 计数带当前文档锚点并区分 included 数量 | 同上 | 默认开启；`semanticTools.codeLens=false` 关闭；模板/组件 lens 各最多 64 条，不完整时标题标记 |
| documentSymbol | 复用 W2 实现 | 既有 W2 实现，W3 未改语义 | [structural_editing.rs](../../crates/wxml-lsp/tests/structural_editing.rs) | 见 F-13/F-14 的 W2 边界 |

### 2.3 Q2 调度、性能与恢复状态（W4）

Q2 的 Rust 与安全 R2 均为 `APPROVE`；R1 的 BLOCKING/HIGH/MEDIUM 已关闭，R2 的两个证据/测试
脚手架 MAJOR 已在最终门禁前处理（`42a82ba`/`5990126` 和最终 manifest），没有第 3 轮正式审核。
Q2 验证的是协议、进程和调度行为，不包含真实编辑器 UI 或大项目语料。

| 项 | 状态 | 已实现/已验证 | 主要证据 | 仍待/限制 |
| --- | --- | --- | --- | --- |
| 诊断 debounce/coalescing | 已实现（受限） | 默认 120 ms；`wxml.diagnostics.debounceMs` 可设 0–5000；同一文档只保留最新 revision；打开/保存/配置立即调度 | q2_stability.rs（历史，ADR-0009 删除；见 tag pre-slim-20261120）、`server.rs`/`scheduler.rs` 单测（原 `q2_scheduler.rs` 已随 P7 删除） | 连续大流量下的 CPU 仍依赖宿主；大项目未单独测量 |
| 交互/诊断队列 | 已实现（受限） | 交互队列 128、诊断队列 8；每 8 个交互任务让诊断前进；队列满时明确拒绝 | [scheduler.rs](../../crates/wxml-lsp/src/scheduler.rs) | 单 worker；运行中的同步 TypeScript 调用不可中断 |
| 取消 | 已实现（受限） | 排队请求立即响应 `$/cancelRequest`；运行请求等后端返回/超时；attempt 防旧结果污染复用 id | q2_stability.rs（历史，ADR-0009 删除；见 tag pre-slim-20261120）（`typing burst + cancel`；原 `q2_scheduler.rs`/`q2_recovery.rs` 已随 P7 删除） | 运行中占用不作为取消延迟；late/duplicate cancel 仍有协议边界 |
| 后端失败恢复 | 已实现（受限） | timeout/crash/protocol 不匹配时降级并报告，静态能力保留；恢复后重新请求 | q2_stability.rs（历史，ADR-0009 删除；见 tag pre-slim-20261120）、stdio_type_loop.rs（历史，ADR-0009 删除；见 tag pre-slim-20261120）、`real_node`（历史，P7 删除） | 真实 15 s 读超时未新增端到端等待用例 |
| 工作区恢复 | 已实现（受限） | 根移除/重加重放 WXML snapshot 与 script/JSON/style overlay；资源/页面增删、关闭/重开；native 引擎重启重放 | q2_stability.rs（历史，ADR-0009 删除；见 tag pre-slim-20261120）、native_parity/native_engine_real.rs（历史，ADR-0009 删除；见 tag pre-slim-20261120）（原 `q2_recovery.rs` 已随 P7 删除） | 单 active root；多 app 重放受 active project 限制；配置/style 热更不在 Q2 组合测试 |
| 性能门禁 | 已通过（受限） | static p95 0.178 ms、semantic 39.659 ms、diagnostics stop 164.894 ms、queued cancel 0.674 ms、single TS 402.126 ms、parent RSS 10,460 KiB；`thresholds.pass=true` | `perf/final-rust-perf.json`、ADR-0009（历史证据见 tag pre-slim-20261120） | JSON `git_head=5990126`；两个后续 commit 只改 Node 发布入口；benchmark 为单个小 fixture |
| 稳定性门禁 | 已通过（受限） | strict 10 轮 + RSS baseline；`thresholds.pass=true`；Node/Rust RSS 比 1.0309/0.9928 | `test-logs/final-stability.json`、ADR-0009（历史证据见 tag pre-slim-20261120） | 同上；不能替代大项目/真实编辑器 UI 验收 |

## 3. 行为细则

### 3.1 标签、属性和值（F-01–F-05）

- 标签补全在 `<` 后触发，候选至少覆盖微信小程序基础组件。候选项带一句用途说明；选中后保留用户已输入的前缀，不自动添加未请求的属性。
- 属性补全在标签名后的空白处触发，按当前标签或自定义组件的声明过滤。已经出现的属性默认不再重复推荐；允许同一属性多次出现的场景需有显式规则。
- 属性名匹配同时支持源码中的写法和元数据中的规范名，例如连字符与驼峰之间的对应关系必须确定，不能依赖模糊字符串搜索。
- 属性值补全只在枚举值明确时提供，例如 `picker` 的 `mode` 及其相关属性。替换范围覆盖原值内容，不残留引号或半截值。
- 条件筛选规则必须数据化，不能把 `picker` 等特例硬编码在补全流程里。规则至少能表达“某属性取某值时，另一属性可用/不可用/改变候选”。
- 指令和事件补全使用独立分类：`wx:` 指令、`bind*`/`catch*`/`capture-bind*`/`mut-bind*` 事件、普通属性。事件名与处理函数补全走不同请求通道，避免把属性候选混入表达式成员。

### 3.2 自定义组件（F-06）

- 从最近的页面或组件 JSON 读取 `usingComponents`，解析本地路径、绝对路径和项目支持的包形式。
- 组件标签候选优先于同名基础组件；出现重名时保留两者并标注来源，不静默取第一个。
- 组件属性来自实际 `properties` 声明或类型后端。无法解析、插件组件、远程组件或声明不完整时，明确标记为降级，不伪造属性列表。
- 配置未保存时以内存内容为准；组件文件或属性声明变化后刷新引用它的模板。
- 重复声明、循环引用和缺失目标文件必须可诊断，不能让补全或诊断流程崩溃。

### 3.3 表达式、Hover 与定义（F-07–F-09）

- `{{ }}` 内按模板作用域解析：页面/组件数据、组件属性、`wx:for` 项与索引、自定义循环变量、模板调用上下文、`wxs` 模块。
- 页面/组件的数据根来自脚本声明：页面为 `data`，自定义组件为 `data` 与 `properties`；字段在 WXML 中直接作为顶层标识符访问。独立 `interface`/`type` 只有被 `data`/`properties` 字段引用时才参与，不能作为模板根。
- 成员补全使用真实类型，不用正则枚举字段。`any`、`unknown` 和缺失类型必须显示真实状态，不能生成看似确定的候选。
- 事件属性中的处理函数补全来自脚本声明：`Component` 读取 `methods`，`Page` 读取选项对象上的函数；方法引用、`data-*` 参数和绑定形式分别处理。
- Hover 至少包含类型、字段说明和声明归属。字段说明优先使用实际 `data`/`properties` 字段或其引用类型自身的 JSDoc；没有说明时明确省略，不复制其他字段的文档。
- 定义跳转优先返回原始声明，而不是内存检查视图中的辅助符号。无法安全反向映射的编辑或位置不返回。
- 资源路径和 class 的定义跳转见 F-11、F-12；组件标签跳转到组件文件或声明入口。

### 3.4 诊断（F-10）

- 诊断范围映射回原始 WXML，精确到出错字段或表达式片段；中文、emoji 和不同换行格式不能导致偏移。
- 未知字段、非法成员访问、非法操作数和可判定的绑定类型不兼容是首阶段重点。模板运行时转换、可空访问等规则必须有独立策略，不能直接照搬 TypeScript 诊断。
- 只发布可归因到 WXML 的问题；辅助代码、检查视图包装和探针声明产生的错误属于桥接失败或覆盖不足，不能过滤后当作“检查通过”。
- 修复后旧诊断必须清除。后端不可用、检查关闭或只完成部分分析时，显示对应能力状态，不把空诊断当作严格通过。
- 诊断去重以“源文件、范围、代码、消息类别”为键，避免同一问题因多个检查视图重复发布。

### 3.5 样式与资源（F-11–F-12）

- Class 补全读取同名 WXSS/SCSS/CSS、`app.wxss`/`app.scss`、显式 `wxml.globalStyles`，并按页面/组件的 `styleIsolation` 过滤；不假设固定文件名一定存在，也不自动扫描用户目录。
- class 说明来自样式声明前的块注释；注释掉的 selector 不算定义。W2 支持静态 `@import`/`@use`/`@forward` 链、逗号列表、复合 selector、可证明的 SCSS 嵌套/`&` 展开；动态插值、mixin/`@extend` 生成类和预算截断明确降级，不伪造完整结果。
- 资源链接只处理可证明的本地资源属性，并通过项目根和解析根计算目标；`http(s)`/`data:`/`wxfile:`/`cloud:` 等远程值与 mustache 不变成可点击链接。目标不存在时不生成链接，并按配置发布存在性诊断。
- 路径解析必须限制在项目可解析范围内，不因绝对路径、`..` 或符号链接越出预期工作区而不提示；W2 的资源扫描、class 数、导入深度/文件数/总字节均有独立预算。

### 3.6 编辑体验（F-13–F-15）

- 闭合标签、开始标签补全和 `</` 栈候选由 LSP/语法树提供；Zed 的括号、引号、Mustache 配对继续由语言配置负责。linkedEditingRange 只返回同名标签对；S 的 `rename` 只对已证明安全的目标生效，tag rename 仍保守拒绝，不能把 linked ranges 或局部 rename 当作标签重命名。
- 编辑必须保持原文空白、注释、表达式和 `<wxs>` 内容。W2 格式化只改写开始标签内部的 ASCII 空白，不重新打印整份文档；文本节点、实体、属性值、引号、属性顺序和重复属性逐字保留。
- Snippets 区分内置和用户定义。用户条目按 `id`/`label` 覆盖内置；占位符顺序、编辑后文档可重新解析和最终光标位置必须有测试。只有客户端声明 `snippetSupport` 且插入位置安全时才提供 snippet，否则回退普通候选。
- 格式化必须幂等、保留语义，解析失败、截断或存在未恢复语法错误时返回零编辑；rangeFormatting 只处理完全落在请求范围内的开始标签。多行标签中段无法安全推导缩进时跳过，不扩大到未请求文本。

### 3.7 项目与状态（F-16–F-18）

- 项目模型负责文件身份、内存覆盖层、磁盘回退、配置候选和依赖图。语言服务协调器是文档状态的唯一写入入口，分析只读不可变快照。
- 未保存的 WXML、脚本、导入类型和 JSON 配置优先于磁盘内容；关闭文档后回退到磁盘并产生新的内部版本。
- 项目根、页面/组件归属和 `tsconfig` 归属必须有确定规则。多候选时不能简单选择“最近的一个”；无法确定时显式标注推断状态。
- 依赖变化只刷新受影响的模板。旧请求、旧快照和旧后端世代的结果不得覆盖新文本。
- 多工作区、多小程序根、多 `tsconfig` 和服务重启必须隔离；日志至少包含实例、事件和版本标识。W2 已落实单 workspace 内的显式多 app、多 tsconfig 和重启身份；完整多 workspace 仍是后续需求，不能按已实现描述。
- 缓存键包含文档版本、项目版本、检查视图版本和后端世代。缓存失效条件必须可测试，不能只按文件路径缓存。

### 3.8 补全目录现状（W2）

W2 已实现 F-01–F-04 的目录驱动补全，并把 F-05 的条件规则数据化到 `picker`/`input`；F-06/F-07 继续使用同一目录和类型后端，F-11 的 class 补全改由项目样式索引提供。标签名、属性名、枚举值和指令/事件补全是**静态、同步**的，完全在 Rust 内完成，不启动 Node；Mustache 成员补全和事件处理函数值补全经过类型后端。以下目录仍是**有意限定的子集**，不宣称完整微信小程序组件覆盖：未收录的标签、属性照常可用于模板，只是没有静态候选。W2 新增的组件属性、class、资源、WXS/template 补全见 §2.1 对应行，不在本目录逐项重复。

#### 3.8.1 标签

| 标签 | 专属属性（全局属性、指令、事件同时可用） | 枚举值属性 |
| --- | --- | --- |
| `view` | `hover-class` `hover-stop-propagation` `hover-start-time` `hover-stay-time` | `hover-stop-propagation` |
| `text` | `selectable` `user-select` `space` `decode` | `space` |
| `button` | `type` `size` `plain` `disabled` `loading` `form-type` `open-type` `hover-class` | `type` `size` `form-type` `open-type` |
| `image` | `src` `mode` `lazy-load` `binderror` `bindload` | `mode` |
| `input` | `type` `value` `placeholder` `disabled` `maxlength` `focus` `confirm-type` `password` | `type` `confirm-type` |
| `picker` | `mode` `range` `range-key` `value` `start` `end` `bindchange` `bindcancel` | `mode` |
| `scroll-view` | `scroll-x` `scroll-y` `enable-flex` `scroll-top` `scroll-left` `enable-back-to-top` `bindscroll` `bindscrolltolower` | `scroll-y` |
| `swiper` | `indicator-dots` `autoplay` `interval` `duration` `circular` `vertical` `bindchange` | `indicator-dots` `autoplay` `circular` |
| `navigator` | `url` `open-type` `delta` `hover-class` | `open-type` |
| `textarea` | `value` `placeholder` `maxlength` `auto-height` `disabled` | `auto-height` |
| `form` | `bindsubmit` `bindreset` | — |
| `label` | `for` | — |
| `checkbox` | `value` `checked` `disabled` | `checked` |
| `radio` | `value` `checked` `disabled` | `checked` |
| `template` | `is` `data` | — |
| `block` `swiper-item` | — | — |
| `wxs` | `module` `src` | — |

全局属性：`id` `class` `style` `hidden`。补全候选按当前标签、全局属性、指令、事件分组排序；已输入前缀大小写不敏感匹配，不覆盖光标后的文本。

#### 3.8.2 指令与事件

- 指令：`wx:if` `wx:elif` `wx:else` `wx:for` `wx:for-item` `wx:for-index` `wx:key`。
- 固定事件属性：`bindtap` `catchtap` `bind:tap` `catch:tap` `capture-bind:tap` `capture-catch:tap` `mut-bind:tap` `bindinput` `bindchange` `bindconfirm` `bindsubmit` `bindreset` `bindscroll` `bindload` `binderror` `bindtouchstart` `bindtouchmove` `bindtouchend`。
- 由事件名生成的绑定属性：仅当前缀为空或已是事件前缀时，为下列事件名生成 `bind<name>`、`catch<name>`、`bind:<name>` 三种写法：空前缀下便于发现事件，输入事件前缀后则只保留匹配项。事件名：`tap` `longpress` `longtap` `touchstart` `touchmove` `touchend` `touchcancel` `input` `change` `confirm` `blur` `focus` `submit` `reset` `scroll` `scrolltoupper` `scrolltolower` `load` `error` `message` `animationstart` `animationiteration` `animationend` `transitionend`。
- 闭合标签补全（`</`）只来自当前未闭合元素栈：`<view><text></` 给出 `view`、`text`，不给出其他标签。

#### 3.8.3 枚举值

| 标签.属性 | 候选值 |
| --- | --- |
| `view.hover-stop-propagation` | `true` `false` |
| `text.space` | `ensp` `emsp` `nbsp` |
| `button.type` | `default` `primary` `warn` |
| `button.size` | `default` `mini` |
| `button.form-type` | `submit` `reset` |
| `button.open-type` | `contact` `share` `getPhoneNumber` `getUserInfo` `openSetting` `feedback` |
| `image.mode` | `scaleToFill` `aspectFit` `aspectFill` `widthFix` `heightFix` `center` `top` `bottom` `left` `right` |
| `input.type` | `text` `number` `idcard` `digit` `safe-password` `nickname` |
| `input.confirm-type` | `send` `search` `next` `go` `done` |
| `picker.mode` | `selector` `multiSelector` `time` `date` `region` |
| `scroll-view.scroll-y` | `true` `false` |
| `swiper.indicator-dots` `autoplay` `circular` | `true` `false` |
| `navigator.open-type` | `navigate` `redirect` `switchTab` `reLaunch` `navigateBack` `exit` |
| `textarea.auto-height` | `true` `false` |
| `checkbox.checked` `radio.checked` | `true` `false` |

值补全只对表内属性生效，候选按已输入前缀做 ASCII 大小写不敏感匹配；表外属性不产生值候选。

#### 3.8.4 Node 后端补全

- Mustache 成员补全：`{{ user.na }}` 经 C1 虚拟文件布局复用 TypeScript `getCompletionsAtPosition`，候选来自页面/组件真实类型。
- 事件处理函数值补全：`bindtap="on"` 枚举 Page 选项上的顶层函数，或 Component `methods` 的成员；注册容器名（`data`、`properties`、`methods` 等）不进入候选。路由规则：属性名以 `bind`、`catch`、`capture-bind`、`capture-catch`、`mut-bind` 开头且后缀为 `:` 或小写字母/事件名时按事件值处理，因此目录中未逐一列出的 `bindcancel`、`bindcolumnchange` 等也能得到方法补全；`binding-mode` 之类普通属性不受影响。
- 后端不可用、未实现、超时或不支持时返回空列表并记录日志，不返回协议错误，也不影响静态补全。

#### 3.8.5 替换范围、触发与能力声明

- 所有候选项都带精确 `textEdit`：标签名和属性名只替换已输入 token；引号内的值只替换值前缀（保留引号）；未加引号的值插入带引号的字面量。不整行替换。
- 在纯文本、HTML 注释、`<wxs>` 元素体和 Mustache 字符串字面量内不产生补全；非模板文档（`.ts`、`.json` 等）经项目路由判定后直接返回空列表。
- `initialize` 无条件声明 `completionProvider`，`resolveProvider: false`，触发字符为 `<`、空格、`:`、`"`、`'`、`.`、`{`、`=`；结果 `isIncomplete: false`，单次最多 256 项。参数缺失或结构非法返回 LSP `-32602`。

#### 3.8.6 已知边界

- 自定义组件标签来自页面/组件自身的 `usingComponents` 与 `app.json` 全局表；同名时页面/组件本地项生效，组件自身 WXML 也参与解析，组件属性名、类型、JSDoc 与事件参与补全/hover/definition。插件或远程组件缺少本地可信元数据时明确降级，不伪造属性。
- 静态目录仍是手写受限子集，不是官方组件表的自动镜像；新增条目需同时补测试与本文档。`picker`/`input` 之外的 mode 条件规则尚未数据化。
- class、资源和 WXS/template 的补全使用各自的索引/静态语义，不并入本节的标签目录；它们的降级边界见 §2.1 与 ADR-0009（历史证据见 tag pre-slim-20261120）。

目录溯源：条目定义在 `crates/wxml-lsp/src/completion/catalog.rs`，取值来源与许可证说明见[来源与许可证记录](provenance.md)；目录不下载、不执行外部元数据。

### 3.9 语义工具与安全编辑（S）

- references 和 rename 的请求目标只接受 WXML；编辑可以落到关联脚本。跨 app、插件/远程组件和无法解析的目标不推断。
- references 只有在覆盖完整时才返回 `Location[]`；`wxml.references` 命令额外返回 `coverage.status`，不完整返回 `partial` 或 `unsupported`，不把部分结果当完整。rename 的覆盖不完整时整份拒绝，不生成部分编辑。
- `EditPlan` 必须满足：只使用 `documentChanges`；打开文件带客户端 version，关闭文件重读并以 `version: null` 返回；校验 expected 原文、UTF-16 范围、文件身份、去重、不重叠和可写路径；失败零编辑；服务端不写盘。
- 保守拒绝包括：自定义组件 tag rename、静态 include 导致的 caller 覆盖不完整、动态访问、共享 importedData 跨脚本、文件含 `<template name>` 时的页面 data rename、快照截断/超限、跨视图冲突、保留字、作用域捕获和对象 shorthand。未落盘 WXML fail-closed。
- codeAction 只提供字段拼写 quickfix 和创建事件 handler；Component `methods` 插入必须保持逗号语法有效。不提供 refactor 或通用代码生成。
- semanticTokens 使用 UTF-16，覆盖 CRLF、emoji 和 WXS 静态 token；只对可完整解析的模板生成，异常/超限返回空 token 流。workspaceSymbol 聚合多 app 和 static imported template，索引不完整时返回空并写日志。
- inlayHint、signatureHelp 和 codeLens 都基于静态事实；inlayHint 与 codeLens 默认开启，可通过 `initializationOptions.semanticTools` 关闭。codeLens 的 template/组件计数有上限，include 计数带当前文档锚点并明确 included 数量。
- 不提供跨文件事务、原子提交或 undo；硬链接未检测。symlink 和生成目录已拒绝，但不等于完整文件身份安全。
- W4 没有再改 S 语义；最终 release 的 Linux Zed 运行时冒烟覆盖 hover、definition、成员补全和诊断，见 ADR-0009（历史证据见 tag pre-slim-20261120）。

### 3.10 性能与恢复边界（Q2/W4）

- 单后台 worker 串行执行后端请求；TypeScript 同步调用不可中断。取消只保证排队请求立即响应，
  运行中请求必须等后端返回或超时，运行占用单独测量，不能当作取消延迟。
- 诊断 debounce 默认 120 ms，可通过 `wxml.diagnostics.debounceMs` 设 0–5000 ms；打开、保存和
  配置变更绕过 debounce。同一文档只保留最新 revision，旧结果不得发布。
- 交互队列上限 128、诊断队列上限 8；连续 8 个交互任务后必须让诊断前进；队列满时明确报告
  busy，不无限排队。诊断重试最多 8 次。
- timeout、crash、协议不匹配时降级并保留静态能力；恢复后重新建立后端请求。根移除/重加
  重放已打开的 WXML 和 script/JSON/style overlay；单 active root，多 app 重放受 active project
  限制，不支持多根热加。
- 性能/稳定性门禁的最终数字是：static p95 0.178 ms、semantic 39.659 ms、diagnostics stop
  164.894 ms、queued cancel 0.674 ms、single TS 402.126 ms、parent RSS 10,460 KiB；strict
  10 轮 + RSS baseline `thresholds.pass=true`。这些数字来自单 fixture，不是大项目语料。
- 性能/稳定性 JSON 绑定 `5990126` 的 clean HEAD；之后只有 Node 发布入口/模块解析修复。
  最终 release 在 `9b804d4` 重建，两次构建 SHA-256 相同；install/verify/verify-package 通过。
- Linux Zed 运行时冒烟使用手工安装的最终发布包，不是标准 UI `zed: install dev extension`
  重跑；发布包只在 Linux x86_64 验证。没有 marketplace、远端 release 或发布者签名。

## 4. 数据来源与元数据规则

允许的数据来源：

1. 仓库内版本化的基础组件、属性、指令、事件和片段元数据。
2. 项目内的 `app.json`、页面/组件 JSON、`usingComponents`、`tsconfig`、样式文件和用户片段配置。
3. 通过类型后端获得的脚本类型、JSDoc 和声明位置；数据根限定为页面/组件 `data` 与组件 `properties` 及其引用类型。

约束：

- 运行时不依赖外部插件、外部仓库或临时目录。缺少本地元数据时功能降级并报告，不静默下载。
- 每条内置元数据记录应能追溯到来源、版本和许可证。来源标识只出现在[来源与许可证记录](provenance.md)，功能文档不把外部项目当作需求依据。
- 元数据结构至少包含：规范名称、别名、适用标签/组件、值类型、枚举值、条件规则、说明、来源版本。
- 不执行工作区脚本或 `new Function` 提取组件信息；需要脚本语义时交给类型后端做静态分析。
- 不把生成代码写入业务项目。检查视图、辅助符号和缓存均存在于内存或项目自有缓存目录。

## 5. 验收要求

- 每项功能至少有正例、反例和降级样本；补全类功能还要验证替换范围、触发字符和候选过滤。
- 页面/组件数据根必须来自 `data`/`properties`，不得以独立 `interface`/模型文件替代；跨文件类型只能作为字段类型来源。
- 位置相关功能覆盖中文、emoji、LF/CRLF 和文档边界。
- 项目相关功能覆盖多根、路径别名、未保存配置和依赖变化。
- 与已有 JS/TS 服务并存时，不重复发布无关脚本诊断，不抢占原服务职责。
- 文档和交付说明必须区分：已实现、部分实现、明确降级、未实现；逐项状态和证据见 §2.1。
  - 已实现（受限）：F-06/F-07 组件与表达式补全；F-08 表达式/标签/属性/组件 hover；F-09 字段/组件/class/资源/template/WXS definition 与 references；F-11 class 索引、导航与 references；F-12 本地资源；F-14 snippets；F-15 保真格式化；F-16 配置/依赖/缓存；F-17 类型与项目上下文。
  - 部分实现：F-05 条件规则（目前 `picker`/`input`）；F-10 诊断子集；F-13 无 tag rename/on-type，S 的 rename 只覆盖 data、组件属性、事件、loop、class 和 template；F-18 单 workspace 内显式多 app 和 workspaceSymbol，非完整多 workspace。
  - 明确降级：动态 template/WXS/Behavior、未知 spread、插件/远程组件、动态 class 插值/SCSS 生成类、索引/预算截断；模板体不逐调用实例检查，WXS 不执行。
  - S 语义工具：references、prepareRename/rename、codeAction、semanticTokens、workspaceSymbol、inlayHint、signatureHelp、codeLens 已合入，但都带 §2.2/§3.9 的保守拒绝和预算边界；不把它描述为无限制的完整 LSP。
  - Q2 调度/恢复：诊断 debounce/coalescing、交互/诊断有界队列、排队取消、timeout/crash/protocol 恢复、根移除/重加 overlay 重放和 Node 项目池稳定键已实现；Rust/安全 R2 均 APPROVE，最终性能/稳定性门禁通过，但边界见 §2.3/§3.10。
  - 最终 release：Linux x86_64、protocol 2、214 files、两次构建相同 SHA-256；install/verify/verify-package 和 Linux Zed 运行时冒烟通过。它是本地发布包，不是 marketplace/远端发布。
  - 未实现：完整多 workspace/多根热加、大项目语料、Windows/macOS 实机、真实主桌面 Wayland、标准 UI 在最终版本上的重跑、marketplace/远端发布/发布者签名。
