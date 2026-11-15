# C0 基础契约

更新日期：2026-11-15。

本文件记录第一轮实现（P0/C0）修订后的公共契约：坐标与范围类型、项目/文件/版本身份、请求上下文、能力状态、后端错误与接口、mock 行为、共享样本 schema，以及哪些内容**尚未**冻结。C0 之外的片段布局和线协议由后续工作包在 C1 冻结，见 [ADR-0002](../adr/0002-template-type-checking-and-source-maps.md) 与 [ADR-0004](../adr/0004-parallel-delivery-and-validation-gates.md)。

## 1. 权威顺序

发生冲突时按以下顺序判断：

1. 有效 ADR（架构边界）。
2. 本文件与[类型诊断策略](type-diagnostics.md)（契约与策略）。
3. 项目 README 与功能基线（需求与进度）。
4. 证据记录（历史结论及限制）。

代码事实优先于任何文档；发现不一致时更新对应文档，不保留两套说法。

## 2. 坐标与范围类型

| 类型 | 单位与用途 |
| --- | --- |
| [`TextRange`](../../crates/wxml-text/src/range.rs) | 全文 UTF-8 字节、半开区间 `[start, end)` 的原始范围；源文本与语法树共用 |
| [`SourceRange`](../../crates/wxml-text/src/source_range.rs) | 原始源文件范围；`Diagnostic`、`HoverResult`、`SourceLocation` 的唯一范围类型 |
| [`FragmentRange`](../../crates/wxml-text/src/source_range.rs) | 生成检查片段中的范围；必须经源码映射后才能变成 `SourceRange` |
| [`Position`](../../crates/wxml-text/src/line_index.rs) | 零基行号 + 行内 UTF-16 列，镜像 LSP `Position` |
| [`LineIndex`](../../crates/wxml-text/src/line_index.rs) | 拥有文本快照的行索引，字节 ↔ UTF-16 位置的唯一转换点 |

约定：

- 所有范围上限为 4 GiB（`u32`），超过的表达不在 C0 范围内。
- `TextRange` 不校验字符边界或文本长度；`SourceSnapshot::source_range` 负责校验。
- `SourceRange`/`FragmentRange` 在类型层面隔离，不能互相替换。把片段范围直接发布为诊断或 hover 范围属于错误。
- `LineIndex::new` 拥有 `Arc<str>` 文本；`position(offset)` 与 `offset(position)` 不再接受外部文本，因此旧索引无法套用到新文本。
- 行分隔符是 `\n`。`\r\n` 中的 `\r` 属于行结束、不计入列；单独 `\r` 是普通字符。
- 落在 `\r\n` 或 `\n` 内部的 offset 映射到该行内容末尾的位置；行起点映射到下一行第 0 列。
- 列等于行内容长度时映射到 `\r` 或行尾；列超出内容长度或落在代理对中间返回 `None`。
- 转换必须使用产生范围时的同一文本快照；范围自身不携带版本。
- LSP 层负责 UTF-16 ↔ 字节转换；后端请求使用字节 offset，后端可用 `context.source().line_index().position(offset)` 校验。

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
- `ClientVersion` 的单调性由 LSP 协调器执行：收到不大于当前值的版本时拒绝应用，并记录协议错误。

## 5. 源码快照与请求上下文

[`SourceSnapshot`](../../crates/wxml-types/src/context.rs)：

- 字段：`FileId`、`SourceRevision`、`SnapshotId`、`Arc<LineIndex>`。
- `new`（拥有文本）/`from_index`（共享已有索引）；`text()`、`line_index()` 返回同一快照的内容。
- `source_range(TextRange) -> Option<SourceRange>`：`end <= text.len()` 且 start/end 都在 UTF-8 字符边界时返回 `Some`；否则返回 `None`，**不 clamp、不 panic**。

[`RequestContext`](../../crates/wxml-types/src/context.rs)：

- 字段：`ProjectRevision`、`ViewRevision`、`BackendEpoch`、`Arc<SourceSnapshot>`。
- 构造时校验 `project_revision.project() == source.file().project()`，不一致返回 `ContextError::ProjectMismatch`。
- 所有 Backend 请求都携带 `context` 与字节 `offset`，不再有裸 `FileId`。
- offset 必须是 UTF-8 字符边界；后端可用行索引校验。

## 6. 能力状态

[`CapabilityStatus`](../../crates/wxml-types/src/status.rs) 为 `#[non_exhaustive]`，共六态：

| 变体 | `is_supported()` | `reason()` | 含义 |
| --- | --- | --- | --- |
| `Supported` | `true` | `None` | 样本覆盖的范围内已完成检查 |
| `Partial { reason }` | `false` | `Some` | 部分输入可用，`reason` 说明限制 |
| `Unsupported { reason }` | `false` | `Some` | 语言形式不支持 |
| `Disabled { reason }` | `false` | `Some` | 策略关闭（如 `templateCheckPolicy`） |
| `Unavailable { reason }` | `false` | `Some` | 运行时不可用（后端退出、超时、未启动） |
| `Unknown { reason }` | `false` | `Some` / `None` | 尚未确认支持程度 |

JSON 使用自描述标签，例如 `{"status":"partial","reason":"…"}`；`Unknown` 无 reason 时省略字段。`Partial` 有意不算支持，调用方不能把部分覆盖当成完整检查。

## 7. 失败与状态的区别

[`BackendError`](../../crates/wxml-types/src/backend.rs) 为 `#[non_exhaustive]`、`Clone + Debug + PartialEq + Error`：

| 变体 | 显示文本 | 用途 |
| --- | --- | --- |
| `Unavailable(String)` | `type backend unavailable: …` | 未启动、未就绪 |
| `Protocol(String)` | `type backend protocol error: …` | 私有协议不兼容或消息非法 |
| `Timeout` | `type backend request timed out` | 超过请求期限 |
| `Cancelled` | `type backend request cancelled` | 请求被取消 |
| `Internal(String)` | `type backend internal error: …` | 后端内部失败 |

`BackendError` 表示“请求失败”；`CapabilityStatus` 表示“检查质量/可用性”。两者不能互相替代。`Protocol`/`Internal` 的文本只用于日志和诊断，不直接展示给最终用户。`BackendError` 不实现 serde；跨进程错误码在 C1 协议中定义。

## 8. 后端接口与 DTO

[`Backend`](../../crates/wxml-types/src/backend.rs) 是 `Send + Sync` 的同步阻塞边界：

```rust
pub trait Backend: Send + Sync {
    fn epoch(&self) -> BackendEpoch;
    fn hover(&self, request: HoverRequest) -> Result<HoverResult, BackendError>;
    fn diagnostics(&self, request: DiagnosticsRequest) -> Result<DiagnosticsResult, BackendError>;
    fn definition(&self, request: DefinitionRequest) -> Result<DefinitionResult, BackendError>;
    fn capabilities(&self) -> BackendCapabilities;
}
```

- 调用方必须在专用线程或 `spawn_blocking` 中调用，并设置读超时；后端不得无限阻塞。
- `definition` 默认返回 `Unsupported` 与空 locations，而不是“检查通过且没有定义”。
- `capabilities()` 默认返回四项 `Unsupported { reason: "not implemented" }`；`completion` 字段先于方法存在，方法在 C1 接入。

| 类型 | 关键字段 |
| --- | --- |
| `HoverRequest` | `context: RequestContext`、`offset: u32` |
| `HoverResult` | `contents`、`markup: MarkupKind`、`range: Option<SourceRange>`、`status`、`quality` |
| `MarkupKind` | `Markdown`（默认）/ `PlainText` |
| `TypeQuality` | `Known` / `Any` / `Unknown` / `Missing`，`#[non_exhaustive]` |
| `DiagnosticsRequest` | `context` |
| `Diagnostic` | `range: SourceRange`、`severity`、`message` |
| `DiagnosticsResult` | `diagnostics: Vec<Diagnostic>`、`status` |
| `DefinitionRequest` | `context`、`offset` |
| `SourceLocation` | `file: FileId`、`range: SourceRange` |
| `DefinitionResult` | `locations: Vec<SourceLocation>`、`status` |
| `BackendCapabilities` | 四项 `CapabilityStatus`（不再使用 bool） |

关键语义：

- `DiagnosticsResult` 的空 `diagnostics` **只有**在 `status == Supported` 时表示“干净”；`Partial`/`Unsupported`/`Disabled`/`Unavailable`/`Unknown` 必须显示降级，不能当作通过。
- `DefinitionResult` 的空 `locations` + `Supported` 表示“已检查，确实没有定义”；`Unsupported`/`Unavailable` 表示“没有能力查”，不得显示成“无定义”。
- `HoverResult::status` 描述检查状态，`quality` 描述类型质量；`Any`/`Unknown`/`Missing` 不得显示成具体类型文本。

## 9. Mock 行为

[`MockBackend`](../../crates/wxml-types/src/mock.rs)：

- 默认 capabilities：hover/diagnostics `Supported`，definition/completion `Unsupported`；默认 epoch 为 `BackendEpoch::new(BackendId::new("mock"), 1)`。
- 默认结果：diagnostics 为干净且 `Supported`；definition 为 `Unsupported`；未预置 hover 时返回 `Unavailable`。
- `set_hover`/`set_diagnostics`/`set_definition` 可在调用之间替换结果（AC-009）。
- `fail_hover`/`fail_diagnostics`/`fail_definition` 分别注入错误；`clear_errors` 清除全部；`with_error` 让三个方法都失败。
- `set_capabilities`/`set_epoch` 覆盖广告状态。
- `calls()` 按顺序返回 `BackendCall::Hover`/`Diagnostics`/`Definition`；错误路径也记录调用。
- 内部 `Mutex` 被毒化时通过 `PoisonError::into_inner` 恢复数据，不 panic。
- 不模拟延迟、取消或进程退出；时序测试使用可控延迟的集成后端（AC-009）。

## 10. 共享样本清单

`tests/fixtures/manifest.json` 为 schema v2，完整规则见[共享验收样本](../../tests/fixtures/README.md)。C0 冻结：

- `version = 2`。
- `id` 唯一；`kind` 为 `fixture`/`runtime`；`acs` 只使用 AC-001..AC-013 且全覆盖。
- fixture 必须给出 `root`、`positive`、`negative`、`expected.hover`、`expected.diagnostics`；可选 `expectClean`、`expected.definition`、`coverage`。
- runtime 必须有 `scenario` 与至少两个 `steps`。
- marker 唯一；`target` 是 marker 内唯一的 token；`position` 由 `wxml_text::LineIndex` 计算并精确匹配；`positive`/`negative` 按 marker 出现次数比较（跨文件重复需重复列出），`requirePositions` 强制每条 hover 带 `position`。
- 校验入口：`cargo test -p wxml-fixtures`。

## 11. 发布校验

发布结果前至少校验：

1. 结果的文件与 `SourceRevision` 匹配当前文档；
2. `SnapshotId` 是当前分析快照；
3. `ProjectRevision` 未失效；
4. `ViewRevision` 与当前检查片段和映射一致；
5. `BackendEpoch` 等于当前后端世代；
6. 目标模板没有被新分析替代。

任一不满足时丢弃结果并调度当前快照；不能把旧范围套到新文本上。

## 12. C0 冻结范围

已冻结：

- `TextRange`/`SourceRange`/`FragmentRange`、`LineIndex`（拥有文本）、`Position`。
- `ProjectId`、`BackendId`、`FileId` 及 canonical 规则、`FileIdError`。
- `SourceRevision`、`SnapshotId`、`ViewRevision`、`ClientVersion`、`ProjectRevision`、`BackendEpoch`。
- `SourceSnapshot`、`RequestContext`、`ContextError`。
- `CapabilityStatus` 六态、`BackendError`、`Backend` trait、全部结果 DTO、`MockBackend`。
- 共享 fixtures schema v2 与校验器。
- 本文件与[类型诊断策略](type-diagnostics.md)。

未冻结（C1 及后续）：

- `CheckFragment` 的 JS/TS 方言、helper ABI、shadow 布局和片段身份。
- Rust–Node 私有协议帧、消息集合、请求 ID、ACK、超时与错误码。
- `AnalysisStamp`、`BackendFileVersion` 的具体结构与组合方式。
- LSP 调度、防抖、发布屏障实现和编辑替换范围。
- AC-012 的反向编辑/重命名结果：manifest 以 `coverage.edits = "deferred-to-C1"` 记录，G4 前补齐。
- runner 断言：AC-007 的“不重复发布 JS/TS 主服务诊断”与 AC-004 的 `properties` overlay 变体在 P6/P7 补齐。

新增或修改 C0 项时：先更新契约测试与本文，再让依赖方接入；影响多个工作包的变更需要协调者确认。
