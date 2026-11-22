# ADR-0002: 模板类型检查、类型后端与源码映射

**Date**: 2026-11-15  
**Status**: accepted（模板语义与映射；类型宿主与查询归属由 ADR-0012 更新）
**Deciders**: 项目维护者、规划协作 agent

**2026-11-21 后续**：[ADR-0012](0012-in-process-tsrs-session.md)（2026-11-21 accepted）把 Node 类型宿主和跨进程插入协议替换为进程内 `tsrs_api::Session`；本文的原位 shadow、模板语义、检查状态和映射原则继续有效。

**2026-11-22 后续**：[ADR-0013](0013-engine-checked-component-bindings.md) 细化本文「组件属性」行的边界：只有纯 mustache 组件属性与静态事件处理函数使用引擎赋值探针；字面量、空属性、混合文本和文本节点仍按 WXML 值形态处理。不是所有属性都做目标类型赋值检查。

## Context

模板必须在不修改普通 Page/Component 源码的前提下保留真实类型、字段 JSDoc、声明位置和精确错误范围。WXML 的页面/组件数据根只能来自脚本中的 `data`（自定义组件还包括 `properties`），独立 `interface`/`type` 只有在被这些字段引用时才进入类型上下文。

WXML 外层解析、模板作用域和 TS 类型检查属于不同问题：某个后端能识别联合类型，不代表模板转换已经保留了窄化；JS 能返回 hover，也不代表启用了错误检查；过滤生成代码诊断后得到空结果更不能视为检查通过。因此本记录分别规定片段内容、shadow 映射、检查状态和类型质量。

## Decision

Rust 独占 WXML/表达式解析、名字绑定、`CheckFragment` 内容和模板映射；`wxml-backend-tsrs` 独占真实脚本类型上下文、shadow 根改写与布局、脚本映射；`wxml-lsp` 负责把项目事实和请求版本适配成片段、调度查询并发布结果。辅助代码只存在于内存虚拟文件系统，不写入业务项目，不执行工作区脚本。

### 当前模板语义管线

```text
PreparedView + RequestContext
  → CheckFragment（表达式、绑定、守卫、WXML 范围）
  → FragmentSpec / ShadowRequest
  → 根改写与 shadow 挂载
  → tsrs_api::Session 语义/语法诊断
  → 映射回原 WXML 范围
```

- `CheckFragment` 定义在 `crates/wxml-types/src/check.rs`，携带 `expr_text`、`target`、`bindings`、`guards`、`wxml_range`、`fragment_revision` 和 `root`。
- `wxml-lsp/src/backend_tsrs/input.rs` 构造 `FragmentSpec`；`wxml-backend-tsrs/src/shadow/layout.rs` 定义布局输入，`shadow/rewrite.rs`、`shadow/locals.rs` 和 `shadow/guards.rs` 完成根改写、局部变量和条件收窄。
- WXML parser 是手写的（`crates/wxml-syntax/src/parser.rs`），不是 Tree-sitter 集成。当前实现按快照重新解析，不存在 `InputEdit` 增量树契约；历史记录中的 Tree-sitter/增量解析规划不再作为现行要求。

| 语义 | 当前实现约定 | 检查或降级要求 |
| --- | --- | --- |
| 根数据 | 自由标识符改写为 `__wxml_root.*`；根由脚本 `data`（自定义组件还包括 `properties`）提供，`properties` 字段以顶层标识符出现 | 按小程序语义读取实际声明、类型和 JSDoc；`properties` 来自 `type`/`optionalTypes` 等声明，不只按默认值推断；独立 `interface`/`type` 仅通过字段引用参与 |
| 名字改写 | 区分局部变量、WXS 名字、成员名、对象键和简写属性；每个原 token 保留范围 | 不将 `user.name` 的 `name` 当作根变量；简写属性单独 lowering |
| 循环 | `LocalBinding` 显式记录 item/index、名字、`source_expr` 和 `source_range`；嵌套遮蔽按最内层生效，离开作用域后不泄漏 | 数组/对象遍历和循环条件用独立样本确认，不用通用 JS 迭代规则猜测 |
| 条件链 | `GuardChain` 保存 `wx:if`/`wx:elif`/`wx:else` 条件、顺序和选中分支；条件与正文使用一致访问路径 | 可空和判别联合分别验证，避免生成方式破坏窄化；`wx:else` 不生成额外条件 |
| 事件 | 独立事件通道携带属性、handler 范围和绑定形式；静态 handler 可挂载为 `this.<handler>` 成员片段 | 候选、hover、定义和存在性检查分别验收；动态绑定明确降级，不伪造方法 |
| 组件属性/事件契约 | 按 [ADR-0013](0013-engine-checked-component-bindings.md)：纯 mustache 组件属性与静态事件处理函数使用引擎赋值探针 | 字面量、空属性、混合文本、布尔真值转换不进入赋值路径；不把所有属性插值直接当作目标类型赋值 |
| 特殊属性 | `wx:key` 等采用专用语义 | 规则与运行时一致后再开放检查，不统一视为自由表达式 |
| 共享模板 | 调用实例及其上下文纳入片段和映射身份 | 不能只凭模板 URI 缓存不同调用的类型 |
| WXS、template/import/include、Behavior | 分别建立支持范围和样本 | 未覆盖情况返回 `Partial`/`Unsupported`，不靠生成 `any` 声称完整支持 |

### Shadow、探针与检查策略

- 当前实现使用同一快照构造派生脚本文本，再把片段挂载进去；不向业务脚本插入 helper，也不维护 Node 时代的 `fragmentId`/`lengthUtf16` 安装协议。历史 Node 插入协议、A/B 上下文桥和 helper ABI 已删除。
- `FragmentSpec.wxml_range` 的长度必须等于表达式 UTF-8 字节长度；不满足时是片段错误，不做截断或近似映射。
- `ShadowRequest::new` 默认 `js_check: JsCheckPolicy::Inherit`，由项目 `checkJs`/`@ts-check` 决定是否检查 JS；`JsCheckPolicy::Off` 会在 shadow 中注入仅用于派生的 `@ts-nocheck`，关闭模板诊断，不修改源文件。源文件自身的前导 `@ts-nocheck` 同样关闭模板诊断；`@ts-check` 与 `@ts-nocheck` 的先后按前导注释的最后一个生效。
- 组件属性/事件契约由 `ContractSpec` 挂到既有片段上；生成 `const __wxml_bind_<id>: <expected> = <rewritten>; void __wxml_bind_<id>;`，诊断按契约锚点归属。该机制不新增引擎 API、不改写用户脚本，详见 ADR-0013。

### 检查状态与类型质量

检查状态使用 `wxml-types/src/status.rs` 的六态，不能把缺失检查包装成干净结果：

- `Supported`：样本覆盖的输入已检查。
- `Partial`：只覆盖部分输入，`reason` 说明缺口。
- `Unsupported`：该语言形式或查询不支持。
- `Disabled`：策略关闭了检查。
- `Unavailable`：后端或运行时不可用。
- `Unknown`：支持性尚未建立，不作声明。

只有 `Supported` 才可视为“已检查且干净”。类型质量另行描述：已知真实类型、`any`、缺少类型来源、合成声明失败等；合法存在的 `any` 也不代表严格校验。

### 源码映射与坐标

```text
原始 WXML / JS / TS / syntax AST：UTF-8 字节半开范围
        ↕ CheckFragment / FragmentSpec：UTF-8 字节半开范围
        ↕ shadow 脚本：绝对 UTF-16 范围
        ↕ LSP Position/Range：行号 + 行内 UTF-16
```

| 层次 | 坐标单位 | 绑定身份 |
| --- | --- | --- |
| 原始 WXML、JS/TS、syntax AST | 全文 UTF-8 字节半开范围 | `FileId` + `SourceRevision` |
| 检查片段、挂载片段 | 全文字节半开范围 | `FragmentId` + `ViewRevision` |
| shadow 查询和返回 span | 全文绝对 UTF-16 偏移与长度 | 实际文件 + `SnapshotId` + `BackendEpoch` |
| LSP Position/Range | 零基行号 + 行内 UTF-16 列 | 对应源文件版本 |

实现时用不同类型表示源文件字节范围、片段字节范围和后端 UTF-16 span；转换必须使用同一文本快照，定义目标使用目标文件自己的版本和索引。CRLF 不静默归一化；若引入转换则显式维护映射。

根访问前缀、`this.` 和契约变量名标为 synthetic，原标识符保留精确映射；允许原表达式映射到多处生成代码。跨来源或跨 synthetic 的诊断必须分类归因，不能整体平移或贴到最近字符。补全的替换范围及附加编辑无法安全反向映射时不返回；后端 helper 名称不能泄露为用户候选。

## Alternatives Considered

| 方案 | Pros | Cons | Why not |
| --- | --- | --- | --- |
| 正则提取并改写表达式 | 原型代码少 | 无法可靠识别嵌套、作用域、对象键和输入中间态 | 不满足类型语义与精确映射要求 |
| Rust 与类型后端各自重写模板 | 单端局部实现方便 | 双份语义、位置信息漂移、难以定位责任 | Rust 独占片段，后端只做 shadow 与查询 |
| 将推断类型转成字符串再生成声明 | 容易建立独立检查文件 | 丢失类型身份、私有名、声明及文档关系 | 不作为准确类型来源 |
| 要求导出或改写用户 Page/Component | 上下文更易引用 | 侵入业务源码，偏离普通写法零改动需求 | 仅允许内存视图 |
| 将独立 `interface`/模型文件作为模板数据根 | 检查视图可直接引用类型，构造简单 | WXML 运行时只绑定 `data`/`properties`；独立类型不是数据源，会产生虚假根和漏检 | 仅让 `data`/`properties` 字段引用该类型 |
| 把所有属性插值都改成目标类型赋值 | 检查路径统一 | 与运行时字符串化、布尔真值转换和文本节点渲染冲突，会产生误报 | 由 ADR-0013 区分纯 mustache/静态事件与字面量/混合文本 |

## Consequences

### Positive

- 模板语义与真实类型检查边界明确，两端不会重复维护表达式改写。
- 字段文档、定义及泛型等沿真实声明关系查询。
- 精确映射与覆盖状态可以独立测试，能识别“没有检查”的空结果。
- 进程内 shadow 不修改用户文件，也不需要跨进程安装协议。

### Negative

- shadow 布局、契约锚点和双向映射是核心实现成本。
- 默认 `cargo test` 不要求真实类型引擎，完整行为矩阵必须由 opt-in 门禁证明。
- 输入中间态、特殊属性、共享模板和组件契约增加语义与映射分支。

### Risks

- **映射落入 synthetic 文本**：标记 `Partial`，不发布猜测位置，不返回不安全编辑。
- **`@ts-nocheck` 或 `checkJs` 意外关闭检查**：把实际生效策略和状态随结果返回，错误字段不能靠 hover 证明。
- **`any`/`unknown` 被误读为通过**：类型质量与检查状态分开，`any` 不作为严格校验证据。
- **契约诊断外溢**：只发布命中锚点且属于赋值类的诊断，其余丢弃或降级，见 ADR-0013。

## Validation

以下为验收口径，不声称已在当前提交上全部执行：

1. 裸 Page/Component（含其 `data`/`properties` 字段）、局部 JSDoc typedef 和闭包内注册不要求改源码；未覆盖写法明确记录。
2. 跨文件类型、泛型、递归、联合和带私有成员的类保持预期类型关系，不只比较 hover 字符串。
3. `user.name` 展示 `data.user.name` 的实际类型和该字段文档，`user.nmae` 只标记错误字段；`title` 展示 `properties.title` 声明处的类型和文档。
4. JS 的 `checkJs` 开关、文件 pragma、`allowJs` 和 `noImplicitThis` 的适用组合逐项记录支持或降级；无关 JS 错误不重复发布。
5. 未保存的 `data`/`properties` 字段类型及纯 JSDoc 更新可见，定义跳转回到原声明。
6. 中文、emoji、CRLF、多个片段、一对多映射和不完整表达式位置正确。
7. 原始脚本模块分类、类型和诊断对比通过；synthetic/helper 错误不能被计作模板检查成功。
8. 独立 `interface`/模型文件不被当作模板根；只有被 `data`/`properties` 字段引用时才生效；未声明的 `properties` 不被伪造。
9. 组件属性与事件按 ADR-0013 的矩阵验证：纯 mustache、静态 handler、字面量、空属性、混合文本、`any`/`unknown` 和 JS `checkJs` 两侧。
10. 六态与类型质量随结果返回；`Partial`/`Unsupported`/`Disabled`/`Unavailable`/`Unknown` 均不得表现为干净的 `Supported`。

## References

- [当前能力摘要](../../README.md#能力摘要)
- [当前已知限制](../../README.md#已知限制)
- [WXML 语言功能基线](../reference/wxml-language-feature-baseline.md)
- [ADR-0009 项目瘦身](0009-project-slimming.md)（历史证据见 tag `pre-slim-20261120`）
- [ADR-0001：分层与职责](0001-layered-language-service-architecture.md)
- [ADR-0003：版本和后端协议](0003-versioned-document-synchronization.md)
- [ADR-0011：组件类型声明](0011-component-type-declarations.md)
- [ADR-0012：进程内 tsrs 类型引擎会话](0012-in-process-tsrs-session.md)
- [ADR-0013：组件属性与事件的引擎判定绑定检查](0013-engine-checked-component-bindings.md)
