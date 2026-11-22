# 基础契约

更新日期：2026-11-22。

本文固定跨模块共享的基础契约边界：坐标与范围类型、项目/文件/版本身份、请求上下文、能力状态、后端错误与接口、mock 行为、共享样本的维护方式，以及重命名的安全边界。

**历史边界**：早期称 C0（P0 是首轮实现标签）；C1 类型桥与 Node 后端已随 native 进程内后端删除，Wave 4/5 的模板作用域、补全与事件链路已经落地。本文不再记录迁移批次或冻结清单，历史版本见 tag `pre-slim-20261120`。当前权威顺序与过程证据边界以[文档索引](../README.md#3-权威顺序)为准；组件内 `usingComponents`、属性/事件等 WXML 功能限制归[功能基线](wxml-language-feature-baseline.md)，本文只在需要时链接，不重复维护。

## 1. 权威顺序

发生冲突时按以下顺序判断：

1. 当前代码与默认运行的测试。
2. 已接受的 ADR（架构边界）。
3. 本文件与[类型诊断策略](type-diagnostics.md)。
4. 项目 README 与[功能基线](wxml-language-feature-baseline.md)。

代码事实优先于任何文档；发现不一致时更新对应文档，不保留两套说法。过程性证据、迁移计划、旧 Node 时代的验收记录和已删除的 C1 类型桥不作为现行事实来源；追溯方式见[文档索引](../README.md#3-权威顺序)。

## 2. 坐标与范围类型

| 类型 | 单位与用途 |
| --- | --- |
| [`TextRange`](../../crates/wxml-text/src/range.rs) | 全文 UTF-8 字节、半开区间 `[start, end)` 的原始范围；源文本与语法树共用 |
| [`SourceRange`](../../crates/wxml-text/src/source_range.rs) | 原始源文件范围；`Diagnostic`、`HoverResult`、`SourceLocation` 的唯一范围类型 |
| [`FragmentRange`](../../crates/wxml-text/src/source_range.rs) | 生成检查片段中的范围；必须经源码映射后才能变成 `SourceRange` |
| [`Position`](../../crates/wxml-text/src/line_index.rs) | 零基行号 + 行内 UTF-16 列，镜像 LSP `Position` |
| [`LineIndex`](../../crates/wxml-text/src/line_index.rs) | 拥有文本快照的行索引，字节 ↔ UTF-16 位置的唯一转换点 |

约定：

- 所有范围上限为 4 GiB（`u32`），超过的表达不在基础契约范围内。
- `TextRange` 不校验字符边界或文本长度；`SourceSnapshot::source_range` 负责校验。
- `SourceRange`/`FragmentRange` 在类型层面隔离，不能互相替换。把片段范围直接发布为诊断或 hover 范围属于错误。
- `LineIndex::new` 拥有 `Arc<str>` 文本；`position(offset)` 与 `offset(position)` 不再接受外部文本，因此旧索引无法套用到新文本。
- 行分隔符是 `\n`。`\r\n` 中的 `\r` 属于行结束、不计入列；单独 `\r` 是普通字符。
- 落在 `\r\n` 或 `\n` 内部的 offset 映射到该行内容末尾的位置；行起点映射到下一行第 0 列。
- 列等于行内容长度时映射到 `\r` 或行尾；列超出内容长度或落在代理对中间返回 `None`。
- 转换必须使用产生范围时的同一文本快照；范围自身不携带版本。
- LSP 层负责 UTF-16 ↔ 字节转换；查询类后端请求（hover、definition、completion）使用 UTF-8 字节 `offset`，后端可用 `context.source().line_index().position(offset)` 校验。

## 3. 身份

### 3.1 ProjectId / BackendId

- [`ProjectId`](../../crates/wxml-types/src/contract.rs)：不透明、稳定的项目根标识，字符串比较，serde 为普通字符串。
- [`BackendId`](../../crates/wxml-types/src/contract.rs)：不透明、稳定的后端实例标识，规则同上。
- 两者都不得为空；构造方由项目层/后端适配器负责，`wxml-types` 不解析路径。

### 3.2 FileId

[`FileId`](../../crates/wxml-types/src/contract.rs) 由 `ProjectId` + canonical 项目相对路径组成：

- 构造时把 `\` 统一为 `/`，去掉开头的 `./`。
- 拒绝空串、绝对路径（`/`、盘符、UNC）、`.`/`..` 组件、空组件（`//`、结尾 `/`）与 NUL。
- 保留大小写，路径大小写敏感。
- canonical 规则：项目根相对、`/` 分隔、无 `.`/`..`、无前导 `/`、大小写敏感。
- `project()`、`path()` 与结构化相等是身份依据；`as_str()`/`Display` 返回 `project:path`，仅用于展示和日志，不得作为稳定缓存键。
- 相等、哈希与排序包含 project，因此不同项目中的同名文件是不同文件。
- serde 形状为 `{"project":"example","path":"pages/index/index.wxml"}`；反序列化走同一校验，非法 JSON 被拒绝。

## 4. 版本与世代

| 类型 | 作用域与含义 |
| --- | --- |
| `SourceRevision` | 单文档内容版本；编辑、关闭回退和磁盘变化都会产生新值 |
| `SnapshotId` | 一次分析的不可变输入集合 |
| `ViewRevision` | 单个视图（检查片段文本与映射）的版本 |
| `ClientVersion` | 客户端 `didChange` 版本号；单调递增，服务端拒绝倒退或重复版本 |
| `ProjectRevision` | 单个项目的归属、配置、依赖或分析设置版本，携带 `ProjectId` |
| `BackendEpoch` | 单个后端实例的世代，携带 `BackendId` |

- 简单 newtype 提供 `new`/`get`/`next`；`next` 饱和到 `u64::MAX`。达到上限后必须停止接受新编辑/变更，而不是复用最大值。
- `ProjectRevision`/`BackendEpoch` 的 `next` 保留各自的 project/backend。
- 所有版本实现 `Display`；简单 newtype 序列化为数字，组合版本序列化为 `{ "project": …, "value": … }` / `{ "backend": …, "value": … }`。
- `ClientVersion` 的单调性由 LSP 层执行：收到不大于当前值的版本时拒绝应用，并记录协议错误。

## 5. 源码快照与请求上下文

[`SourceSnapshot`](../../crates/wxml-types/src/context.rs)：

- 字段：`FileId`、`SourceRevision`、`SnapshotId`、`Arc<LineIndex>`。
- `new`（拥有文本）/`from_index`（共享已有索引）；`text()`、`line_index()` 返回同一快照的内容。
- `source_range(TextRange) -> Option<SourceRange>`：`end <= text.len()` 且 start/end 都在 UTF-8 字符边界时返回 `Some`；否则返回 `None`，**不 clamp、不 panic**。

[`RequestContext`](../../crates/wxml-types/src/context.rs)：

- 字段：`ProjectRevision`、`ViewRevision`、`BackendEpoch`、`Arc<SourceSnapshot>`。
- 构造时校验 `project_revision.project() == source.file().project()`，不一致返回 `ContextError::ProjectMismatch`。
- 每个后端查询请求都携带 `context`，不存在脱离快照的查询；`hover`/`definition`/`completion` 另带字节 `offset`，`diagnostics`/`script_diagnostics` 只带 `context`，`symbols`/`document_symbols` 另带 `subject`。
- 偏移必须是 UTF-8 字符边界；后端可用行索引校验。`subject` 必须是当前已准备视图内的文件，后端不自行解析路径。

## 6. 能力状态

[`CapabilityStatus`](../../crates/wxml-types/src/status.rs) 为 `#[non_exhaustive]`，共六态：

| 变体 | `is_supported()` | `reason()` | 含义 |
| --- | --- | --- | --- |
| `Supported` | `true` | `None` | 样本覆盖的范围内已完成检查 |
| `Partial { reason }` | `false` | `Some` | 部分输入可用，`reason` 说明限制 |
| `Unsupported { reason }` | `false` | `Some` | 语言形式不支持 |
| `Disabled { reason }` | `false` | `Some` | 策略关闭了该检查 |
| `Unavailable { reason }` | `false` | `Some` | 运行时不可用（会话未就绪、已失效、未启动） |
| `Unknown { reason }` | `false` | `Some` / `None` | 尚未确认支持程度 |

JSON 使用自描述标签，例如 `{"status":"partial","reason":"…"}`；`Unknown` 无 reason 时省略字段。`Partial` 有意不算支持，调用方不能把部分覆盖当成完整检查。`Disabled` 只表示“策略关闭”，不表示语言形式不支持；没有实际策略时不得凭空产出该状态。

## 7. 失败与状态的区别

[`BackendError`](../../crates/wxml-types/src/backend.rs) 为 `#[non_exhaustive]`、`Clone + Debug + PartialEq + Error`：

| 变体 | 显示文本 | 用途 |
| --- | --- | --- |
| `Unavailable(String)` | `type backend unavailable: …` | 未启动、未就绪 |
| `Protocol(String)` | `type backend protocol error: …` | 私有协议不兼容或消息非法 |
| `Timeout` | `type backend request timed out` | 超过请求期限 |
| `Cancelled` | `type backend request cancelled` | 请求被取消或结果已过期 |
| `Internal(String)` | `type backend internal error: …` | 后端内部失败 |

`BackendError` 表示“请求失败”；`CapabilityStatus` 表示“检查质量/可用性”。两者不能互相替代。`Protocol`/`Internal` 的文本只用于日志和诊断，不直接展示给最终用户。`BackendError` 不实现 serde。

native 后端在进程内把引擎错误类别映射成 `NativeError`，再由适配层映射到上述变体；没有跨进程错误码表。映射入口是 [`native/inproc.rs`](../../crates/wxml-backend-tsrs/src/native/inproc.rs) 的 `map_engine_failure` 与 [`backend_tsrs/mod.rs`](../../crates/wxml-lsp/src/backend_tsrs/mod.rs) 的 `map_native_error`，具体类别见[原生合同](tsrs-native-contract.md)。

## 8. 后端接口与 DTO

[`Backend`](../../crates/wxml-types/src/backend.rs) 是 `Send + Sync` 的同步阻塞边界。下表按职责列出方法及默认行为，不复制完整 trait：

| 方法 | 职责 | 默认行为或约束 |
| --- | --- | --- |
| `epoch` | 返回当前后端世代 | 必须实现 |
| `hover` | 单模板位置 hover | 必须实现 |
| `diagnostics` | 单模板语义/语法诊断 | 必须实现 |
| `script_diagnostics` | 独立项目脚本诊断；无模板 shadow/语法/WXML facts，range 在脚本自身字节中 | 默认空 `Unsupported`，LSP 不发布任何诊断；与“检查过且干净”不同 |
| `definition` | 单模板位置定义 | 默认空 `Unsupported`，不是“检查通过且没有定义” |
| `completion` | 表达式成员/事件处理函数补全 | 默认空 `Unsupported`；静态模板补全不经此方法 |
| `symbols` | 脚本符号事实（数据路径/引用/事件） | 默认 `Unsupported`；空 `Supported` 必须证明没有声明/引用 |
| `document_symbols` | 脚本文档大纲 | 默认 `Unsupported`；空 `Supported` 必须证明文档没有声明 |
| `invalidate` | 项目/工作区标识变化后丢弃缓存 | 默认 no-op，仅适用于无状态后端；缓存型后端必须覆盖、使旧分析不可达并推进 epoch，迟到结果由发布屏障丢弃 |
| `capabilities` | 广告各功能支持程度 | 默认 `BackendCapabilities::none()`（四项 `Unsupported`） |

约束：

- 调用方必须在专用 worker 中调用；当前 native 后端同步执行且不可中断，没有读超时或取消，单次调用可能超过配置的 deadline。
- `symbols`、`document_symbols`、`script_diagnostics`、`completion` 等新增方法不存在“先有字段后有方法”的过渡态；未实现时统一由 trait 默认返回 `Unsupported`。
- `capabilities()` 仍只有 hover/diagnostics/definition/completion 四项，且都是 `CapabilityStatus`，不是 bool；脚本符号与文档大纲按请求状态表达，不进入这四个广告位。
- native 适配器当前对四项都报告 `Partial`（并给出具体原因），因为类型路径只覆盖 shadow 能表达的模板/脚本子集；不得把它说成 `Supported`。

主要 DTO：

| 类型 | 关键字段 |
| --- | --- |
| `HoverRequest` | `context`、`offset: u32`（UTF-8 字节） |
| `HoverResult` | `contents`、`markup: MarkupKind`、`range: Option<SourceRange>`、`status`、`quality` |
| `MarkupKind` | `Markdown`（默认）/ `PlainText` |
| `TypeQuality` | `Known` / `Any` / `Unknown` / `Missing`，`#[non_exhaustive]` |
| `DiagnosticsRequest` | `context` |
| `Diagnostic` | `range: SourceRange`、`severity`、`message`、`code: Option<String>` |
| `DiagnosticsResult` | `diagnostics: Vec<Diagnostic>`、`status` |
| `DefinitionRequest` | `context`、`offset: u32` |
| `SourceLocation` | `file: FileId`、`range: SourceRange` |
| `DefinitionResult` | `locations: Vec<SourceLocation>`、`status` |
| `CompletionRequest` | `context`、`offset`、`kind: CompletionKind` |
| `CompletionItem` | `label`、`kind`、`detail`、`documentation`、`insert_text`、`insert_text_format`、`replace`、`sort_text`、`filter_text` |
| `CompletionResult` | `items`、`status`、`truncated` |
| `SymbolRequest` / `SymbolResult` | `context`、`subject`、`query` / 脚本符号事实与 `status` |
| `DocumentSymbolsRequest` / `DocumentSymbolsResult` | `context`、`subject` / 大纲树、`status`、`truncated`、`skipped`、`gaps` |
| `BackendCapabilities` | 四项 `CapabilityStatus` |

关键语义：

- `DiagnosticsResult` 的空 `diagnostics` **只有**在 `status == Supported` 时表示“干净”；`Partial`/`Unsupported`/`Disabled`/`Unavailable`/`Unknown` 必须显示降级，不能当作通过。
- `Diagnostic.code` 是可选的稳定机器码：没有可靠来源时为 `None`，不得伪造；native 诊断保留引擎十进制 code，宿主样式/资源诊断使用 `style.*` 一类稳定前缀。
- `DefinitionResult` 的空 `locations` + `Supported` 表示“已检查，确实没有定义”；其余状态表示“没有能力查”，不得显示成“无定义”。
- `CompletionResult` 的空 `items` + `Supported` 表示“已检查，没有候选”；`truncated` 为 `true` 时已到达 `MAX_COMPLETION_ITEMS`（256），不得把截断结果当成完整列表。
- `HoverResult::status` 描述检查状态，`quality` 描述类型质量；`Any` 可以显示字面 `any`，但不得当作已知成员类型，`Unknown`/`Missing` 不得包装成具体类型文本。
- `SymbolResult`/`DocumentSymbolsResult` 的空结果只有在 `Supported` 时才表示“确实没有”；`Partial` 必须保留 `reason`/`gaps`。

## 9. Mock 行为

[`MockBackend`](../../crates/wxml-types/src/mock.rs)：

- 默认 capabilities：hover/diagnostics `Supported`，definition/completion `Unsupported`；默认 epoch 为 `BackendEpoch::new(BackendId::new("mock"), 1)`。
- 默认结果：diagnostics 为干净且 `Supported`；definition 为 `Unsupported`；未预置 hover 时返回 `Unavailable`。
- `set_hover`/`set_diagnostics`/`set_definition` 可在调用之间替换结果（AC-009）。
- `fail_hover`/`fail_diagnostics`/`fail_definition` 分别注入错误；`clear_errors` 清除全部；`with_error` 让三个方法都失败。错误路径也记录调用。
- `set_capabilities`/`set_epoch` 覆盖广告状态。
- `calls()` 按顺序返回 `BackendCall::Hover`/`Diagnostics`/`Definition` 三类记录。
- 新方法（`script_diagnostics`、`completion`、`symbols`、`document_symbols`）不扩展 mock：它们走 trait 默认的 `Unsupported`，不会被记录为上述三类调用；`invalidate` 仍是 no-op。
- 内部 `Mutex` 被毒化时通过 `PoisonError::into_inner` 恢复数据，不 panic。
- 不模拟延迟、取消或进程退出；时序测试使用可控延迟的集成后端（AC-009）。

## 10. 共享样本清单

完整 schema、marker/target 规则、目录结构和各 AC 的对应关系见[共享验收样本](../../tests/fixtures/README.md)。

- `tests/fixtures/manifest.json` 与各样本的 `expected/*.json` 是**人工维护的样本记录**：manifest 记录样本与 AC 对应关系，expected 记录 hover/diagnostics/definition 锚点。
- 原 `wxml-fixtures` crate 与 `wave5_acceptance` Node runner 已随 ADR-0009 删除；当前没有 runner 或校验器加载 manifest，crate 测试直接打开样本工程目录。
- 修改样本时必须人工同步 manifest 与 expected；`coverage` 只记录目标，不代表断言已被执行。
- 样本的换行约定（普通 `.wxml` 为 LF，`*-crlf.wxml` 为字节精确 CRLF）由仓库 `.gitattributes` 与样本字节维持，没有独立校验器。

## 11. 发布校验

发布结果前至少校验：

1. 结果的文件与 `SourceRevision` 匹配当前文档；
2. `SnapshotId` 是当前分析快照；
3. `ProjectRevision` 未失效；
4. `ViewRevision` 与当前检查片段和映射一致；
5. `BackendEpoch` 等于当前后端世代；
6. 目标模板没有被新分析替代。

[`AnalysisStamp`](../../crates/wxml-lsp/src/workspace.rs) 已存在，汇集 `file`、`revision`、`snapshot`、`view_revision`、`project_revision`、`epoch`。`Workspace::is_current` 检查文件、`SourceRevision`、`SnapshotId`、`ViewRevision` 与 `ProjectRevision`；LSP 的发布路径同时比较 `stamp.epoch` 与当前 `BackendEpoch`。任一不满足时丢弃结果并调度当前快照；不能把旧范围套到新文本上。

## 12. 当前边界

### 12.1 现行公共项

- `TextRange`/`SourceRange`/`FragmentRange`、`LineIndex`（拥有文本）、`Position`。
- `ProjectId`、`BackendId`、`FileId` 及 canonical 规则、`FileIdError`。
- `SourceRevision`、`SnapshotId`、`ViewRevision`、`ClientVersion`、`ProjectRevision`、`BackendEpoch`。
- `SourceSnapshot`、`RequestContext`、`ContextError`。
- `CapabilityStatus` 六态、`BackendError`、`Backend` trait、全部结果 DTO、`MockBackend`。
- 共享样本的记录方式（schema 细节归 `tests/fixtures/README.md`）。
- 本文件与[类型诊断策略](type-diagnostics.md)。

### 12.2 重命名（已实现，范围有限）

WXML 的 `textDocument/rename` 已经接入，只接受能证明安全的目标：

- 光标位于已打开的 WXML 模板；目标必须是数据路径、脚本组件属性、静态事件处理函数、`wx:for` 的 item/index、`class` 值或静态 `template` 名称之一。
- 普通标签名与自定义组件标签名不重命名：标签名缺少 owning `usingComponents` 别名和全部项目用法的证明，直接拒绝。
- 组件属性重命名会同时处理声明、脚本引用、调用方属性名（含 kebab/camel 拼写）和组件模板内引用；不能证明模板作用域无冲突时拒绝整次编辑。
- 所有目标都必须有完整的符号/引用或静态出现证据；别名与目标必须唯一；新名称与现有字段、循环局部、WXS 模块或类名冲突时拒绝。
- 编辑由 `EditPlan` 统一校验：路径可编辑、范围在字符边界、`expected_text` 匹配、编辑互不重叠、UTF-16 序列化有效。任一项失败返回错误与零编辑，不做部分修改，也不降级为文本查找替换。
- `.d.ts`/`.d.mts`/`.d.cts` 永远不可编辑。`components.d.ts` 可以作为组件属性事实来源，但不能被重命名。
- 只返回 `WorkspaceEdit.documentChanges`，不写磁盘；客户端不支持 `documentChanges` 时服务端拒绝请求。
- 该能力不等于“覆盖所有组件声明/所有跨视图引用”；未通过上述证明的目标没有编辑。

相关实现与测试：[`semantics/handlers.rs`](../../crates/wxml-lsp/src/semantics/handlers.rs)、[`semantics/edit_plan.rs`](../../crates/wxml-lsp/src/semantics/edit_plan.rs)、[`tests/semantic_surface.rs`](../../crates/wxml-lsp/tests/semantic_surface.rs)。

### 12.3 不属于本合同

- C1 类型桥与旧 Rust–Node 私有协议已删除；native 侧以[原生合同](tsrs-native-contract.md)为准。
- LSP 调度、防抖、队列和发布屏障的具体实现、编辑替换范围：属于实现细节，不属于基础契约公共 DTO。
- `AnalysisStamp` 已存在并携带发布校验所需的版本字段；不再维护单独的冻结清单或协调者工作流。

新增或修改基础契约项时：先更新契约测试与本文，再让依赖方接入；跨模块变更按[文档索引](../README.md#1-文档角色)的角色归属评审。
