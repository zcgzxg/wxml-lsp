# F1 查询缺口审计与下一步计划（tsgo-migration @ 576c231）

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD：`576c231`（`docs(plan): record native hover gate increment`）
- 审计日期：2026-11-19
- 范围：已提交的 definition 端到端、native hover evidence + adapter、completion 基础、document symbols 接入后的 F1 查询缺口；pinned TypeScript `7.1.0-beta` 的实际 API；现有 native session/query 与 LSP adapter。
- 边界：只读审计，只新增本文；未修改生产代码、测试、Cargo、合同、ADR、计划正文；未提交。工作树中未提交的 document-symbol scheduler/HIGH 与 asset-lock L1 不属于本文范围，本文不修改、不评估它们。
- 引用约定：以下路径均从 worktree 根开始，行号以 HEAD `576c231` 为准；`backend_tsgo/mod.rs`、`backend_tsgo/mapping.rs` 等工作树有并行改动，本文引用的是 HEAD 版本。路径前保留空白符，便于 VS Code 跳转。
- 结论先行：**F1/P6/G5 均未关闭。** 本文列出的查询层工作最多把若干缺口从“静默缺失”变成“可验证的 Partial/Unsupported”，不等于 F1 验收，也不等于默认后端切换。

---

## 0. 一页结论与排序

| 缺口 | 结论 | 能否产生用户可见功能 | 主要阻塞 |
| --- | --- | --- | --- |
| Completion auto-import | 请求参数与 `symbol` 可接；逐项来源/导入编辑不可得 | 否 | 无 origin/import edit；C0 `CompletionItem` 无 `additionalTextEdits`/`data` |
| Per-entry documentation | **BLOCKED** | 否 | 无 `getCompletionEntryDetails`；无 docs/displayParts 字段；合同禁止用 symbol docs 冒充 |
| Event-handler completion | 现有 Rust script semantics 已覆盖绑定、方法表、behaviors、docs/signature | **是**（建议优先） | 不需要 checker；`this`/方法体内的类型补全不在本切片 |
| Hover overloads/QuickInfo | 只能 type + merged docs + tags | 部分 | 无 QuickInfo/display parts；无逐 overload 文档关联 |
| Script diagnostics ownership | 当前静默丢弃 script-owned diagnostics | 否（先修真实性） | `RawDiagnostic` 无 `fileName`；`DiagnosticsResult` 无文件身份 |
| Rename/EditPlan | 无 native rename；recheck seam 可用但未接线 | 否 | 无 native rename/完整性 flag；`TypeCheckProvider` 非项目级 |

**依赖顺序（建议）：**

1. **先做 event-handler completion**：它不依赖 native rename/auto-import，且 Rust `script::events` 的方法表已经存在；可与 hover overload 测试并行。
2. **同时做 completion 真实性小修**：`isIncomplete`/`insertText`/`filterText` 的保真；这是 auto-import 和截断语义的前提，但本身不叫 auto-import。
3. **再做 diagnostics ownership**：先补 `fileName` 与 WXML/script 分类，把“静默丢弃”改成显式 `Partial`；这一步与 completion 的 wire/session 改动串行。
4. **hover overloads 做测试/状态文档**：不改 API；若旧断言要求 display parts/逐 overload 文档，保持 BLOCKED。
5. **completion auto-import 只做查询证据层**：wire/session/query + 显式 `Partial`；用户可见 auto-import 仍 BLOCKED。
6. **rename/edit gates 只做拒绝与 recheck 准备**：不要声称 rename 可实现；正向 rename 等 native symbol/roles。

**并行/串行点：**

- 可并行：event-handler completion（`backend_tsgo/completion.rs` + 新 event 模块）与 hover overload 测试（`query/hover.rs` + `backend_tsgo/hover.rs`）。
- 串行：`wxml-backend-tsgo/src/native/wire.rs` 同时承载 `prepareAutoImports`、`includeSymbol`、`RawDiagnostic.fileName`，必须一个 owner 一次改完。
- 串行：`wxml-backend-tsgo/src/native/session.rs` 同时承载 prepare-only snapshot、completion options、diagnostics；不要两个 PR 并行改。
- 串行：`wxml-lsp/src/backend_tsgo/session.rs` 的 `QuerySession` trait 是 adapter 与 fake 的公共缝；completion options、event completion、diagnostics 身份都从这里过。
- 串行：`wxml-lsp/src/backend_tsgo/mod.rs` 的分支与 capabilities 文本要和 `tests.rs` 同步；该文件当前有并行未提交改动，必须在它们落地后再改。
- 独立：`semantics/edit_plan.rs` 与其 tests 不与 native session 冲突，可单独推进 recheck 测试。

---

## 1. Completion auto-import

### 1.1 现状（HEAD 事实）

- adapter 只接受 `ExpressionMember`，其他 kind 直接 `Unsupported`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/completion.rs:36
- 该限制的文案在 backend 分支重复一次：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:471
- native 请求硬编码 `includeSymbol:false`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:231
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:245
- session 的 completion 方法没有 options，只传 path/position：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1977
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:104
- `RawCompletionEntry` 只保留 `name`/`kind`/`sortText`/`detail`/`labelDetails`；响应里的 `symbol`、`insertText`、`filterText` 被 serde 静默忽略：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:453
- query 映射只取 label/detail/kind/sortText，并给非空列表固定加 `DocumentationUnavailable` + `AutoImportsUnavailable`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/completion.rs:134
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/completion.rs:175
- adapter 用 `entry.label` 同时作为 `insert_text` 和 `filter_text`，`documentation` 恒为 `None`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/completion.rs:104
- C0 `CompletionItem` 没有 `additional_text_edits`、`data`、`text_edit`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs:262
- LSP 输出把 `isIncomplete` 硬编码为 `false`，即使 native 返回 `isIncomplete:true` 或 256 cap 截断：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs:141
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs:149

补充事实：LSP 路由把所有 mustache 都标成 `ExpressionMember`，并不要求光标前有 `.`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/expression.rs:27
因此 `{{ Us }}` 这类 identifier/global 补全在现有 C0 kind 下也可能到达 native；真正阻断用户可见 auto-import 的不是 kind 名称，而是没有 origin 标记、没有 import edit、C0 也没有承载位置。

### 1.2 pinned 7.1.0-beta API 事实

下载的生成 API/引擎源码（不在仓库内，作为 pin 证据）：

- `createSnapshot` 的顶层参数有 `prepareAutoImports?: DocumentIdentifier`：
  `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:305`
- `updateSnapshot.changes` 复用 `CreateSnapshotParams`，因此也接受 `prepareAutoImports`：
  `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:331`
- `getCompletionsAtPosition` 接受 `includeSymbol?: boolean`：
  `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:992`
- `CompletionEntryResponse` 的字段只有 `name`、`kind?`、`sortText?`、`insertText?`、`filterText?`、`detail?`、`labelDetails?`、`symbol?`：
  `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:1678`
- `CompletionEntry.symbol` 仅在 `includeSymbol:true` 且条目有 symbol 时填充；keyword/literal 没有：
  `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_async_types.ts:395`
  `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_async_types.ts:403`
- `getCompletionEntryDetails` 在生成 API 中**不存在**（literal search 0 次）；`CompletionEntryResponse` 也没有 `documentation`、`displayParts`、`data`、`origin`、`source`、`hasAction`。
- 引擎行为：`includeSymbol:true` + 未 prepare 的 snapshot 会直接报 `snapshot is not prepared for auto-imports`；`includeSymbol:false` 才会尝试 clone/retry：
  `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:5509`
  `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:5515`
- `prepareAutoImports` 在 create/update 时都会校验目标文件可被 prepare；失败是 client error，不是静默 no-op：
  `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:1423`
  `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:1659`

### 1.3 哪些能保真，哪些必须 BLOCKED

| 能力 | 7.1.0-beta 是否有字段 | 现有 Rust 是否保留 | 结论 |
| --- | --- | --- | --- |
| `prepareAutoImports` create/update | 顶层/`changes` 有 | 否 | 可保真；需要 wire/session 支持 |
| `includeSymbol` | 有 | 否，硬编码 false | 可保真；需要 options |
| `insertText` | 可选 | 否，丢弃 | 可保真；有 snippet 歧义时保守回退 label 并标 Partial |
| `filterText` | 可选 | 否，丢弃 | 可保真 |
| `detail` / `labelDetails.detail` | 可选 | 是 | 已保真 |
| `symbol` | 仅 includeSymbol 时有 | 否，丢弃 | 可作为内部证据解析；**不能当 auto-import origin** |
| 逐项 documentation | 无 | 否 | **BLOCKED** |
| 逐项 auto-import origin | 无 | 否 | **BLOCKED**；`sortText`/`symbol`/模块名都不足以证明“未导入” |
| 可执行 import edit | 无 `textEdit`/`data`/import specifier | C0 也没有 `additionalTextEdits` | **BLOCKED** |
| `isIncomplete` | 有 | 映射成 status gap，但 LSP 输出丢成 false | 可修；是 auto-import 的前置真实性修复 |

### 1.4 可选方案

**方案 A：只做文档/状态，不做 auto-import。**
- 保持 `AutoImportsUnavailable`，把能力文案写清楚；不新增 wire 字段。
- 优点：零风险；缺点：F1-21 永远停在 planned。

**方案 B（推荐，若 F1-21 要继续）：只做“查询证据层”。**
- 接 `prepareAutoImports`、`includeSymbol`、`symbol`、`insertText`、`filterText`，新增 `AutoImportOriginUnavailable`，仍不发布任何 auto-import item edit。
- 用户可见 auto-import 仍 BLOCKED；但这个切片能证明 pinned 引擎在 prepared snapshot 下到底返回什么，并把“不可用”从猜测变成证据。

**方案 C：可执行 auto-import。**
- 需要 C0 `CompletionItem` 增加 `additional_text_edits`/`data`，需要从 symbol/reference 推导 import specifier 和插入位置，需要 `EditPlan` 校验+recheck。
- 7.1.0-beta 没有 origin 字段，也没有 import edit 字段；任何“这是 auto-import”或“应该插到哪一行”的判断都是宿主推断，不符合合同 §9.7 的“不得自行解析项目文件补全”。**BLOCKED**。

### 1.5 最小纵向切片（方案 B）

**切片 A0：completion 真实性前置（独立小 PR）**

- `wxml-types::CompletionResult` 增加 `incomplete: bool`（或等价字段），由 native `isIncomplete`/truncated 设置；所有构造点同步。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs:290
- `completion_list` 用该字段输出 `isIncomplete`，不再硬编码 false。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs:149
- 测试：
  - `completion_incomplete_list_sets_lsp_is_incomplete`
  - `completion_truncation_sets_lsp_is_incomplete`
  - `checked_empty_completion_keeps_is_incomplete_false`
- 风险：这是公共 DTO 改动，按合同 §9.1 需要集成 owner 评审；不要把它伪装成“只是 adapter 修复”。

**切片 A1：wire 参数与字段（无行为变化）**

- `SnapshotChanges` 增加 `prepare_auto_imports: Option<String>`，`to_value` 输出 `{"prepareAutoImports":{"uri":...}}`；`create_snapshot_params`/`update_snapshot_params` 自动继承。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:78
- `completion_params` 增加 `include_symbol: bool` 参数，删除硬编码 false。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:231
- `RawCompletionEntry` 增加 `insert_text: Option<String>`、`filter_text: Option<String>`、`symbol: Option<RawSymbol>`（`RawSymbol` 已有 reference/declarations 字段）。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:453
- 测试：精确 JSON 断言 create/update 的 `prepareAutoImports`；`includeSymbol:true/false`；带 `symbol`/`insertText`/`filterText` 的响应可反序列化；缺省不输出 `null`；无 `entries` 仍是 shape error。

**切片 A2：session 的 prepare 与 symbol 请求**

- `NativeSession::open_project_with_auto_imports(input, prepare_path)` 或 `NativeSession::prepare_auto_imports(path)`：
  - path 必须经 VFS 解析、在当前项目内、当前 snapshot 内；
  - `updateSnapshot` 只带 `changes.prepareAutoImports`，保持同一进程；
  - 成功后提交新 epoch、release 旧 snapshot、执行 live cap；
  - 失败返回 typed error/`Partial`，绝不静默继续。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:683
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:765
- `NativeSession::completions` 增加 options（`include_symbol`）；`create_changes` 支持初始 prepare。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1977
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:2091
- 缓存 `(engine epoch, prepare_path, include_symbol)`，禁止每次 keystroke 都 prepare/rotate snapshot。
- 测试：
  - real-engine `{{ Us }}` + `model.ts export interface User`：prepared + `includeSymbol:true` 返回 `User`，并带 symbol；
  - unprepared + `includeSymbol:true` 是硬错误，不是空 `Supported`；
  - prepare-only 后 engine snapshot id 变化、进程不重启、旧 snapshot 释放；
  - 路径越界/文件缺失/项目外 prepare 失败；
  - 更新 snapshot 后 prepare 是否仍有效（必须实测；若失效则重新 prepare，不能假设）。

**切片 A3：query 映射与 adapter 状态**

- `CompletionQuery` 增加 `include_symbol`/`prepared` 上下文；`CompletionEntry` 增加 `insert_text`/`filter_text`/`symbol` 证据；`CompletionGap` 增加 `AutoImportOriginUnavailable`。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/completion.rs:50
- 规则：
  - 未 prepared/未请求：非空列表仍是 `AutoImportsUnavailable`；
  - prepared + 请求：不再报 `AutoImportsUnavailable`，改报 `AutoImportOriginUnavailable`；
  - 两种情况都保留 `DocumentationUnavailable`；
  - **不得**从 `sortText`/`symbol`/模块名推断 per-entry auto-import 标记。
- adapter：`QuerySession::completions` 增加 options；`completion()` 只在启用路径传 `include_symbol:true`；用 `insert_text`（存在且非 snippet 形状）和 `filter_text`，否则回退 label；`documentation` 仍为 `None`。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:104
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/completion.rs:104
- 测试：
  - fake session 记录 `include_symbol`；
  - prepared 非空结果 status 含 `AutoImportOriginUnavailable`，不含 `AutoImportsUnavailable`；
  - 任何条目都没有 `auto_import: true`；
  - `u.` 回归仍返回 `name`/`age`，顺序不变，`documentation: None`；
  - 截断/`isIncomplete` 传到 LSP（依赖 A0）。

**切片 A4：用户可见 auto-import（BLOCKED）**

- 需要 C0 `CompletionItem` 新增 `additional_text_edits`/`data`（或等价），以及从 native 证据推导 import specifier/插入点；pinned API 没有这些字段。
- 即使只显示 label 也不安全：用户接受后只插入名字、不插 import，会制造新错误。
- 在合同 §9.7 修订、DTO 评审、真实引擎 origin 证据三者齐备前，**不要实现**。

### 1.6 风险与 BLOCKED 条件

- `prepareAutoImports` 会建立项目级导出索引，CPU/内存/延迟不受单个 path 字段约束；必须沿用 input/asset 上限、snapshot deadline、live-snapshot cap，并缓存。
- prepare-only `updateSnapshot` 会改变 epoch，adapter 的 `OpenProject.identity` 必须包含 prepare 状态；否则下一次请求会误判“同一个 snapshot”。
- `includeSymbol:true` 增加响应体积（最多 256 条 symbol）；只解析需要的字段，不保留完整 symbol。
- 不保证 prepare 在后续 `update_project` 后仍有效；必须真实引擎测试。
- BLOCKED：逐项 documentation、per-entry origin、可执行 import edit、snippet 格式推断、C0 DTO 未评审。

---

## 2. Per-entry documentation / display parts

### 2.1 结论

**不可行（按当前 pinned API 与合同）。** 保持 `CompletionItem.documentation = None`，非空 completion 继续 `Partial`，原因保持 `DocumentationUnavailable`。

### 2.2 事实

- 7.1.0-beta 没有 `getCompletionEntryDetails`；literal search 0 次。
- `CompletionEntryResponse` 没有 `documentation`/`displayParts`/`data`：
  `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:1678`
- `CompletionEntry` 也没有 docs/display parts：
  `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_async_types.ts:395`
- 合同明确禁止用 `getTypeOfSymbol`/docs 拼“详情”冒充原生详情：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:600
- 当前 adapter 已正确把 `documentation` 置空：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/completion.rs:113

### 2.3 能否用 `CompletionEntryResponse.symbol` + docs 组合

技术上可以做到一部分：
- `includeSymbol:true` + prepared snapshot 时，条目可能带 `symbol`（`SymbolResponse` 含 `reference`）；
- `getDocumentationComment`/`getJsDocTags` 接受 `CheckerSymbolParams`（symbol reference），所以理论上能取到该 symbol 的文档和标签。

但这不是 completion entry details：
- keyword/literal 条目没有 symbol；
- alias/target 的文档可能不同；auto-import 条目没有稳定 origin；
- 文档是“symbol 的文档”，不是“该补全条目在编辑器详情面板中应显示的内容”；
- 合同 §9.7 明确禁止这种冒充；
- `getSymbolAtPosition` 只能取光标处 symbol，不能给列表中的每个候选取文档。

因此：
- **不能**把它作为 `CompletionItem.documentation` 发布；
- **不能**据此关闭 F1-21/B-07；
- 如果产品确实想要“symbol documentation fallback”，必须新增独立 DTO/状态（例如 `documentation_source: SymbolFallback`），并逐项标 `Partial`；这不是本 F1 切片，且当前合同不允许。

### 2.4 最小文件范围与测试

- 生产代码：无（保持现状）。
- 测试/文档：
  - `crates/wxml-backend-tsgo/src/query/completion.rs`：断言非空列表的 `DocumentationUnavailable` 与 `documentation=None` 语义；
  - `crates/wxml-lsp/src/backend_tsgo/tests.rs`：断言 completion 路径没有调用 `hover_documentation`/`getDocumentationComment`；
  - `crates/wxml-backend-tsgo/tests/real_engine.rs` 或新 `f1_query.rs`：真实引擎条目没有 docs/displayParts/data。
- BLOCKED 条件：pinned API 增加真正的 completion-entry details/docs 字段，且合同 §9.7 修订、C0 文档来源字段评审通过。

---

## 3. Event-handler completion

### 3.1 现状

- LSP 已把事件属性值路由到 `CompletionKind::EventHandler`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/expression.rs:20
- Node backend 的 `eventHandler` 语义：
  - Page：`options` 对象成员，过滤 `PAGE_RESERVED`；
  - Component：`methods` 对象成员；
  - label/name、`detail`（成员源码）、`documentation`（JSDoc）、`insertText`、`replace`、`sortText`；
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/completion.ts:72
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/completion.ts:115
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/completion.ts:150
- native adapter 目前只接受 `ExpressionMember`，EventHandler 直接 `Unsupported`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/completion.rs:36

### 3.2 Rust script semantics 已覆盖什么

`crates/wxml-backend-tsgo/src/script` 不是“只有数据根”，它已经有完整的事件方法表：

- `EventMethod`：name、signature、documentation、path、range：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:113
- `MethodTable`：status、owner、methods、opaque、problems：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:128
- `build_table`：Page 顶层成员、Component `methods`、behavior 方法合并、spread/computed/非字面量降级、reserved keys、处理上限：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:223
- `analyze_script` 已在 `ScriptAnalysis` 中暴露 `methods`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/mod.rs:208
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/mod.rs:269
- 模板侧 `EventHandler` 已有 attribute/event/handler/handler_range；绑定形式解析在 `script::events::parse_event_attribute`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:197
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/check.rs:242

覆盖矩阵：

| 语义 | Rust 现状 | 是否足够做事件补全 |
| --- | --- | --- |
| `bindtap`/`catchtap`/`capture-bind:` 等绑定 | `EventHandler` + `parse_event_attribute` 已覆盖 7 种形式 | 是 |
| Page 方法表 | `collect_page` + `PAGE_RESERVED` | 是 |
| Component `methods` | `collect_component` | 是 |
| behaviors 方法 | `build_table` 合并 `BehaviorMethod` | 是 |
| JSDoc/signature | `EventMethod.documentation`/`signature` | 是 |
| spread/computed/非字面量 | `opaque` + `problems` + `Partial` | 是（只能降级） |
| `this` 的类型化成员 | 没有；script 扫描器不解析 checker 类型 | 否，需 native checker/AST |
| 方法体内 `this.` 补全 | 没有 script 文档查询路径 | 否，需 native checker/AST |
| component 方法调用类型过滤 | 没有 | 否，需 checker |

关键结论：**事件属性值里的 handler 名补全不需要 checker，也不需要 native session。** 现有 Rust script 方法表已经是 Node `eventHandler` 语义的近似；需要的是把它接到 adapter，而不是先补 native 查询。

### 3.3 安全扩展的最小切片

**新增 `backend_tsgo/event_completion.rs`（新文件，约 150-250 行）：**

1. 输入 `ViewProvider` + `RequestContext` + `CompletionRequest`。
2. 调用 `provider.prepare(snapshot)`，取 `PreparedView`：
   - `target.owner: RootKind` -> `RegistrationKind::Page/Component`；
   - `target.script`/`target.script_file`；
   - `files` 的原始文本（不是 shadow）。
   /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:194
   /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:345
3. 用原始 `PreparedFile` 构造一个只读 `AnalysisInput`（revision 可用 0/1，不启动 session），调用 `analyze_script`；缓存键为 `view_id` + script revision + file set identity。
4. 从 `ScriptAnalysis.methods` 取候选：
   - `status` 原样保留；
   - `opaque` 不发布为候选，只把整体标 `Partial`（不要建议一个无法解析的 handler）；
   - 按 name 稳定排序（BTreeMap 已排序）；最多 `MAX_COMPLETION_ITEMS`。
5. 生成 `CompletionItem`：
   - label/name；
   - kind `Method`；
   - detail = `signature`（截断、单行）；
   - documentation = `documentation`（截断；这是原生 script 文档，不是 completion details）；
   - insert_text = name；
   - filter_text = name；
   - sort_text = `0_<name>`（与 Node 的 `sortText` 对齐，若冻结该行为）；
   - replace = 事件属性值中当前前缀的范围。
6. `replace` 计算：从 `request.context.source().text()` 在 `request.offset` 前扫描属性值（引号内/未加引号），只回退 identifier 字符；不依赖 shadow mount。不要用全文替换。
7. `TsgoBackendAdapter::completion` 在 `EventHandler` 分支直接调用该模块，**不要** `prepare()`/`ensure_open()`/启动 session。
   /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:467
8. Capabilities 文案从“event-handler completion not implemented”改为“event-handler completion uses static script method tables; `this`/method-body completion remains unsupported”。
   /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:571

**测试：**

- adapter fake view（Page）：`onLoad`/`onTap` 被建议，`data`/`methods` 被过滤；detail/docs 正确；replace 只覆盖前缀。
- Component：`methods` 成员被建议；无 `methods` 时 checked empty（`Supported` 空列表），不是 `Unsupported`。
- behavior：方法被合并；同名 component 方法覆盖 behavior 方法；未解析 behavior -> `Partial`，不报 missing。
- spread/computed/非字面量：`opaque` 不出现，整体 `Partial`，reason 含 script 降级。
- 256 cap：`truncated=true`，LSP `isIncomplete=true`（依赖 A0）。
- 无 session：FakeLog 的 `start_count`/`completion_calls` 为 0；EventHandler 分支不启动 native 进程。
- CRLF/Unicode：replace 范围是 UTF-8 字节且不切断字符。
- 真实 LSP gate：`native_engine_real` 的 event-handler completion 替换（G-P5-E2E-21 相关），断言无 Node、无 native session 也能返回 handler；若 engine 缺失，按 gate 语义失败而不是 skip。
- 回归：`u.` expression completion 不变。

### 3.4 风险与 BLOCKED

- 每次 keystroke 重新 `analyze_script` 会扫描整个 script + behavior 图；必须缓存，且缓存键要含 overlay 版本。
- Rust 与 Node 的差异：
  - Node 对 Page options 的所有成员（含非字面量）给 label；Rust 只给 literal-function methods；
  - Node 用 `localeCompare` 排序，Rust 用字节序。
  - 必须冻结一个 parity 规则；推荐“只建议可解析的 literal handler + `Partial`”，不要为了 label 对齐而建议不可解析项。
- `this`/方法体内补全、类型过滤、component 方法调用签名：需要 native checker/AST 与 script 文档查询路径，**BLOCKED**；不要在本切片声称覆盖。
- behavior 跨文件/未解析：只能 `Partial`，不能伪造 handler 文档。

---

## 4. Hover overloads / QuickInfo

### 4.1 现状

- hover 证据由 `getTypeAtPosition` + `typeToString` + `getSymbolAtPosition` + docs/tags + declarations 组合：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1333
- 类型质量与文本：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/type_info.rs:51
- 文档映射：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/documentation.rs:34
- Markdown 固定三段：` ```ts ` type fence、docs 原文、`@tag` 行：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/hover.rs:314
- adapter 原样发布 backend Markdown，只补 range/status：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/hover.rs:60

### 4.2 pinned API 事实

- 没有 `getQuickInfoAtPosition`；`getCompletionEntryDetails` 同样没有。
- `getSignaturesOfType` 在 API 中存在：
  `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:94`
  但 Rust allowlist 没有它：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/config.rs:135
- `SignatureResponse` 只有 id/flags/declaration/typeParameters/parameters/thisParameter/target，没有文档、没有 display parts：
  `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:709`
- `getResolvedSignature` 只对 CallExpression 正确；传错 node kind 会 panic（引擎 recover），不适合 hover：
  `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_async_api.ts:2476`
  P1-A 证据见 `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-17-tsgo-native-api-spike.md:444`
- P1-A 实测 overload：
  - `getDocumentationComment(add)` -> 合并字符串 `"Adds two numbers.\nAdds two strings."`
  - `typeToString(type of add)` -> `{ (a: number, b: number): number; (a: string, b: string): string; }`
  - 没有结构化 display parts
  `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-17-tsgo-native-api-spike.md:437`

### 4.3 保真上限

| 内容 | 能否由现有组合得到 | 结论 |
| --- | --- | --- |
| 重载签名列表 | `typeToString` 的合并文本已经包含 | 可保真（作为一个 fence） |
| 合并后的 JSDoc | `getDocumentationComment` 返回合并字符串 | 可保真 |
| JSDoc tags | `getJsDocTags` | 可保真 |
| 逐 overload 文档 | 无关联 API；合并字符串无法可靠拆分 | **Unsupported/Partial** |
| 参数名/类型 display parts | 无 | **Unsupported** |
| 代码块/链接/参数高亮 | 无 | **Unsupported** |
| “当前选中的 overload” | 无 `getResolvedSignature` 安全路径 | **Unsupported** |

补充：`getSignaturesOfType` 即使加入 allowlist，也只给 signature/declaration/parameters，不给 docs；要拿逐 overload 文档还得对每个 declaration handle 再取 symbol/docs，属于未验证组合，且需要合同/allowlist 变更。**不建议在 F1 做。**

### 4.4 推荐方案

- 保持现有三段式组合；不新增 API，不试图拆 overload。
- 把“无 QuickInfo/display parts/逐 overload 文档”写成 capability/合同可见降级；不要修改 `compose_hover_markdown` 去猜分段。
- 如果旧测试要求每 overload 独立段落，标 BLOCKED，按合同 §9.8 修订 ADR/合同，不得静默降级。
- 不把 `getSignaturesOfType` 当 hover 替代；`getResolvedSignature` 明确禁用。

### 4.5 最小文件范围与测试

- 生产：无（或仅文档/能力文案）。
- 测试：
  - `crates/wxml-backend-tsgo/tests/f1_query.rs` 新增 `hover_composes_type_docs_and_overloads`（合同 F1-22 计划名）：
    - 两个 overload 的 fixture；
    - 断言 Markdown 只有一个 ` ```ts ` fence，type 文本含两个签名；
    - docs 保留合并换行；
    - 不断言逐 overload 段落；
  - `crates/wxml-lsp/src/backend_tsgo/tests.rs`：fake evidence 的 overload 文本原样发布，status 不被错误提升；
  - `crates/wxml-backend-tsgo/tests/f1_hover.rs`：真实引擎 overload 组合；无 docs/displayParts 时不伪造。
- 风险：把“合并 docs”误判为 `Partial` 会产生噪音；合同允许合并文档，不应仅因重载就降级。只有旧断言要求 display parts 时才 BLOCKED。

---

## 5. Script diagnostics ownership

### 5.1 现状

- native 同时请求 semantic + syntactic：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:970
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:986
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1006
- `RawDiagnostic` 没有 `fileName`；pinned `DiagnosticResponse.fileName?` 被 serde 忽略：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:302
  `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:1023`
- `map_diagnostics` 只按 code/start/end/message 去重，没有文件身份：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/diagnostics.rs:80
- adapter 只在 `NativeRange::Template` 发布；`NativeRange::Script` 直接落入空 `else`，**静默丢弃且不改 status**：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/diagnostics.rs:109
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/diagnostics.rs:130
- `NativeRange::Script` 的定义：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mapping.rs:350
- WXML 诊断 publish 只带一个 document URI；script overlay 只调度受影响 WXML，不给 script 自己发 backend 诊断：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/diagnostics.rs:234
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/diagnostics.rs:289

### 5.2 为什么这是缺口

- 静默丢弃 script-owned diagnostics 可能把“有诊断但被丢弃”发布成 `Supported`，违反“空 + Supported 才是 checked clean”。
- 反过来，把 script range 直接塞进 WXML `publishDiagnostics` 会挂到错误文件，产生误报。
- 如果将来 script URI 也走 native，必须有稳定文件身份，否则同一 diagnostic 可能在 WXML 和 script 各发一次。
- `getSemanticDiagnostics` 虽然按单文件请求，但 pinned 响应带 `fileName?`；不要假设所有条目都属于请求文件。

### 5.3 推荐：先做“归属 + 真实性”，再做“script 发布”

**最小切片（WXML truthfulness，不新增 script URI 发布）：**

1. `RawDiagnostic` 增加 `#[serde(default)] file_name: Option<String>`。
   /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:302
2. 新增内部 `DiagnosticOwner`：
   - `Template { range }`：`map_native_range` 返回 `NativeRange::Template`；
   - `Script { path, range }`：返回 `NativeRange::Script`，且 `fileName` 与 owning script 的绝对路径一致；
   - `Opaque`：映射失败、`fileName` 缺失/不一致、或落在生成文本。
3. `map_diagnostics` 或 adapter 过滤：
   - 只发布 `Template`；
   - 存在 `Script`/`Opaque` 时 `status` 至少 `Partial`，reason 只写数量/类别，不写绝对路径；
   - 去重键改为 `(owner_file_identity, code, start, end, message)`。
4. 文件身份：
   - `RequestPlan.file_id(path)` 已有绝对路径 -> `FileId` 映射：
     /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:130
   - `PreparedView.target.script_file` 是 owning script 的 `FileId`：
     /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:201
   - 用 VFS 绝对路径比较 `fileName`，不要直接发布 engine 路径。
5. 不在 WXML 结果中伪造 script URI；script 诊断暂时只以 `Partial` 警告体现“被排除”。

**后续切片（真正的 script 发布，BLOCKED 到 DTO 决策）：**

- `DiagnosticsResult` 增加 per-file ownership（或新增 `DiagnosticsBatch`），server 能对 script URI 发 `publishDiagnostics`；
- 或者 adapter 增加显式的 `diagnostics_for_script`，server 在 `DocumentRoute::Script` 上调度；
- 需要处理 open/closed script 版本、overlay、stale epoch、重复发布。
- 在这些完成前，**不要**把 script 诊断塞进 WXML 数组。

### 5.4 测试

- wire：`fileName` 可解析；缺失仍可处理；未知字段忽略。
- query：semantic + syntactic 的去重包含文件身份；同一 code/range 但不同文件不合并。
- adapter：
  - 一个 template diagnostic + 一个 script diagnostic -> 只发布 template，status `Partial`，reason 含“script diagnostics excluded”；
  - 只有 script diagnostic -> WXML 空列表但 `Partial`，**不是** `Supported` 空；
  - 映射失败/未知文件名 -> `Partial`，不发布；
  - 同一 diagnostic 在 semantic/syntactic 各出现一次 -> 只发一次；
  - 文件名是 engine 绝对路径 -> 不出现在 message/reason。
- 回归：现有 `diagnostics_map_semantic_and_syntactic_ranges` 仍只发布 template；`a_fatal_diagnostic_source...` 不变。

### 5.5 风险与 BLOCKED

- 每次有 script 错误就把 WXML status 降为 `Partial`，可能让用户看到顶部 warning；这是真实性成本，需产品确认文案。
- `fileName` 可能是 root-relative 或绝对路径，必须按 VFS 规范化后比较；不要字符串前缀匹配。
- 诊断可能来自标准库/资产文件；这些属于 `Opaque`/外部，不能发布。
- BLOCKED：script URI 的诊断发布需要 per-file DTO/server 路由；在合同未冻结前不要实现。

---

## 6. Rename / EditPlan

### 6.1 现状

- `EditPlan` 做路径/版本/范围/文本/语法/UTF-16 校验并序列化 `documentChanges`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/semantics/edit_plan.rs:208
- `recheck` seam 已实现“应用编辑到内存 -> parse -> 可选 typecheck”，但 `#[allow(dead_code)]`，未接线：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/semantics/edit_plan.rs:406
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/semantics/edit_plan.rs:639
- `TypeCheckProvider` 是**单文件、text-only**，明确不解析 imports/sibling/project：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/semantics/edit_plan.rs:559
- 合同 §9.9 第 7 条要求“隔离快照上重新 parse/typecheck”，因此 text-only provider 不足以关闭该条：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:638
- native 没有 `getRenameInfo`/`getEditsForRename`/`rename`；adapter symbols 仍 `Unsupported`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:496
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:147
- 现有组件属性编辑靠 Node `SymbolResult` + `node_rename_plan`，会同时改 caller WXML、component WXML、script declaration：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/semantics/handlers.rs:1430
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/semantics/handlers.rs:1546
- WXML 侧已有 component property 冲突判断（loop/WXS/fragment bindings）：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/semantics/target.rs:1613
- native script symbols 已有 `RenameSafety`/`RenameBlock`，但只覆盖本文件、原 token、静态成员；`rename_edit` 只是单 token，不是完整引用集：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/symbols.rs:224
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/symbols.rs:337

### 6.2 `recheck` 如何用于组件属性编辑

可用的部分：

1. 用 `EditPlan` 收集所有 WXML caller/component template 属性 token，按 `EditRule::AttributeName`/`Identifier` 校验，带 `expected_text`。
2. 对每个被修改的 WXML 文本调用 `recheck_edit_snapshot(original, edits, None)`：
   - 能证明编辑后 WXML 仍可 parse；
   - 不能证明 TypeScript 项目类型正确（`None` -> `CheckUnavailable`）。
3. 对 script declaration 的编辑，必须用项目级 typecheck；当前 `TypeCheckProvider` 只能看单文件文本，**不能**代替 native snapshot 诊断。

因此：
- 只改 WXML 属性名的组件属性编辑可以先得到 parse 级 recheck；
- 旧 gate 要求同时改 script declaration 并 typecheck，必须等 native script symbol facts + 项目级 recheck。

### 6.3 三个 blocked edit gates 的解锁顺序

| Gate | 能否先解 | 依赖 |
| --- | --- | --- |
| `native_component_property_edit_applies_and_reparses` | **部分可以，完整不行** | WXML 属性编辑可用 `EditPlan` + `recheck`；script declaration 需要 native script symbols/roles |
| `assert_native_edit_reparses_and_typechecks` | **可以优先做** | 需要项目级 native recheck provider（构造 modified `AnalysisInput`，开 snapshot，跑 semantic+syntactic diagnostics）；不需要 native rename API |
| `native_rejects_cross_namespace_and_dynamic_edits` | **拒绝路径可先做，动态 case 不行** | cross-namespace 可用 WXML `Target::Property`/`component_property_conflict`；`this.data[key]` 等动态脚本证据需要 native script symbols/roles |

推荐顺序：

1. **Gate 2（reparse/typecheck）**：先做一个项目级 `NativeTypeCheckProvider`（测试/内部 seam），用固定 edit fixture 证明“应用后 parse + semantic/syntactic 都干净”；失败必须 `CheckUnavailable`，不能 `Clean`。
2. **Gate 1 的 WXML 子集**：用 `EditPlan` + `recheck` 证明 caller/component template 属性编辑可应用、可 parse；script declaration 部分保持 BLOCKED，直到 native script symbols 接入。
3. **Gate 3 的拒绝路径**：先覆盖 property→data、component data→property、另一脚本 nested `setData` 等 WXML 可判定的冲突；`this.data[key]`/动态别名必须有 native script 角色事实，否则保持 BLOCKED。

**不要声称 rename 可实现。** native 没有 rename API，也没有完整性 flag；上述 gate 只证明“编辑安全/拒绝”，不证明“正向 rename 与旧行为等价”。

### 6.4 最小文件范围与测试

- `crates/wxml-lsp/src/semantics/edit_plan.rs`：接线 `recheck`（移除 staged dead_code），新增项目级 provider 的 trait/适配；不要改 `EditPlan` 校验语义。
- `crates/wxml-lsp/src/semantics/handlers.rs`：只在 gate/test 路径调用 recheck；生产 rename 路径保持现状/拒绝。
- `crates/wxml-lsp/tests/native_parity/symbols_edits.rs`：把 3 个 `BlockedGate` 变成真实测试；每个 gate 单独 fail-closed。
- 测试：
  - `native_component_property_edit_applies_and_reparses`：固定 caller/component WXML 编辑，应用后 `wxml_syntax::parse` 干净；script 部分不发编辑或标 BLOCKED；
  - `assert_native_edit_reparses_and_typechecks`：固定 WXML+script 编辑，构造 modified `AnalysisInput`，native diagnostics 为空；缺失 engine 必须 fail；
  - `native_rejects_cross_namespace_and_dynamic_edits`：四个旧 case 逐个断言 `-32803`、无 edit、源未变；动态 case 若证据不足应显式 BLOCKED，不得伪造拒绝理由；
  - recheck 负例：新增 type error、stale `expected_text`、重叠 range、CRLF/BOM 边界 -> 拒绝整个 plan。
- 风险：项目级 native snapshot 重建成本高；不能每个 rename 都重启进程，需复用 session + `update_project`/临时 snapshot，并释放旧 snapshot。
- BLOCKED：正向 rename、跨文件完整引用、`this`/computed/shorthand 语义、native roles；这些不是本切片能解决的。

---

## 7. 依赖排序与文件所有权

### 7.1 波次

| 波次 | 工作 | 可并行 | 串行点 |
| --- | --- | --- | --- |
| W0 | 决策/合同：F1-21 是否做查询证据层；diagnostics 是否接受 WXML `Partial` 警告；rename 只做拒绝 | 全部 | 无代码 |
| W1 | event-handler completion | 与 W2 并行 | `backend_tsgo/completion.rs`、新 event 模块、`backend_tsgo/mod.rs` |
| W1 | hover overload 测试/文案 | 与 W1 event 并行 | `query/hover.rs`、`backend_tsgo/hover.rs`、`f1_query.rs` |
| W2 | completion A0（isIncomplete） | 可与 W1 并行 | `wxml-types/backend.rs`、`completion/mod.rs` |
| W2 | completion A1 wire 字段 | 与 W3 串行 | `native/wire.rs` |
| W3 | completion A2 session prepare/options | 与 diagnostics 串行 | `native/session.rs`、`backend_tsgo/session.rs` |
| W3 | diagnostics ownership | 与 completion A2 串行 | `native/wire.rs`、`native/session.rs`、`backend_tsgo/diagnostics.rs` |
| W4 | completion A3 query/adapter | 依赖 A1/A2 | `query/completion.rs`、`backend_tsgo/completion.rs`、`tests.rs` |
| W4 | edit gates/recheck | 与 W4 completion 并行 | `semantics/edit_plan.rs`、`symbols_edits.rs` |

### 7.2 具体串行点

- `crates/wxml-backend-tsgo/src/native/wire.rs`：`prepareAutoImports`、`includeSymbol`、`RawDiagnostic.fileName` 都改同一个文件；一个 PR 完成 JSON 契约测试。
- `crates/wxml-backend-tsgo/src/native/session.rs`：`open_project`/`update_project`/`completions`/`diagnostics` 都改同一个 impl；一个 owner，避免 snapshot/epoch 语义冲突。
- `crates/wxml-lsp/src/backend_tsgo/session.rs`：`QuerySession` trait 的 `completions` options、event completion、diagnostics 身份都在这里；先冻结 trait，再改 fake。
- `crates/wxml-lsp/src/backend_tsgo/mod.rs`：分支/capabilities；当前有并行未提交改动，必须等其落地后再改，避免和你看到的 scheduler/HIGH 改动相互覆盖。
- `crates/wxml-lsp/src/backend_tsgo/tests.rs`：fake `FakeBehavior`/`FakeLog` 是共享测试缝；一个 owner 统一加字段，避免每个 PR 冲突。

---

## 8. 哪些只是文档/接口准备，不能算 F1 完成

以下工作只降低不确定性，**不关闭 F1**：

- 增加 `prepareAutoImports`/`includeSymbol` 参数、`RawCompletionEntry.symbol`/`insertText`/`filterText` 解析：只是 wire 接口准备；用户可见 auto-import 仍 BLOCKED。
- 增加 `AutoImportOriginUnavailable`/`DocumentationUnavailable` 文案：只是状态真实性；F1-21/B-07 仍开放。
- 增加 `RawDiagnostic.fileName` 和 script-owned `Partial`：只是避免静默丢弃；不证明 script 诊断已发布。
- hover overload 测试/文案：只是把保真上限写清楚；不提供 display parts/逐 overload 文档，F1-22 仍不能按旧 QuickInfo 断言关闭。
- `recheck` 接线/固定 gate fixture：只证明编辑安全/拒绝；不证明正向 rename，不关闭 B-09。
- 任何“只改测试名/ledger 状态”的改动：不是实现，不计 F1 完成。

**明确状态：**

- F1：B-06（hover display parts/逐 overload）、B-07（completion 详情/auto-import 参数）、B-08（外部 `.d.ts` definition）、B-09（rename 完整性）仍未关闭；F1-21/22/23 仍是 planned。
- P6：默认后端仍是 Node，native 只是 opt-in；P6-3/P6-4 未完成。
- G5：parity 的 `MIGRATED_CASES` 仍为空，不能声称 G5 关闭。
- 本文不修改默认后端、不删除 Node、不提交、不关闭任何 gate。

---

## 9. 最小验收清单（按缺口）

### Auto-import 查询证据

- [ ] create/update 的 `prepareAutoImports` 精确 JSON。
- [ ] `includeSymbol:true` 精确 JSON。
- [ ] entry 的 `symbol`/`insertText`/`filterText` 可反序列化。
- [ ] prepared + includeSymbol 的 real-engine `User` fixture。
- [ ] unprepared + includeSymbol 硬失败。
- [ ] 无 per-entry auto-import/origin 伪造。
- [ ] 无 `additionalTextEdits`/`data` 输出（因为 C0 没有）。
- [ ] `isIncomplete`/truncation 传到 LSP。
- [ ] 不声称 F1-21 完成。

### Per-entry documentation

- [ ] 非空 completion 的 `documentation=None`。
- [ ] completion 路径不调用 docs 查询。
- [ ] 真实引擎响应没有 docs/displayParts/data。
- [ ] 不声称 F1-21/B-07 完成。

### Event-handler completion

- [ ] Page/Component/behavior 方法表候选。
- [ ] reserved/opaque/spread 降级。
- [ ] replace 只覆盖前缀。
- [ ] EventHandler 分支不启动 native session。
- [ ] expression completion 回归。
- [ ] `this`/方法体仍是 Unsupported。

### Hover overloads

- [ ] 一个 type fence 含合并 overload 签名。
- [ ] 合并 docs 原文保留。
- [ ] 无 display parts/逐 overload 段落。
- [ ] 不声称 F1-22 关闭。

### Diagnostics ownership

- [ ] `fileName` 解析/规范化。
- [ ] template/script/opaque 分类。
- [ ] script-owned 被排除时 `Partial`，不是空 `Supported`。
- [ ] 去重含文件身份。
- [ ] 不把 engine 绝对路径发布给客户端。

### Rename/EditPlan

- [ ] `recheck` 接线并有负例。
- [ ] 项目级 native typecheck provider 失败即 `CheckUnavailable`。
- [ ] 3 个 gate 的拒绝/编辑安全测试；缺失 engine fail-closed。
- [ ] 不声称正向 rename 可实现。

---

## 10. 最终判断

**可以立即做、且不依赖 native rename/auto-import 的：**

1. Event-handler completion（Rust script method table 已存在，接 adapter 即可）。
2. Completion A0/A1 的真实性修复与 wire 字段（`isIncomplete`、`insertText`/`filterText`）。
3. Hover overload 的测试/文案，明确 display parts 上限。
4. Diagnostics `fileName` + script-owned `Partial`（先不发布 script URI）。
5. `recheck` 的项目级 typecheck provider 与 gate 2。

**必须等合同/DTO 决策的：**

- 用户可见 auto-import（C0 `additionalTextEdits`/`data`、origin、import spec）。
- 逐项 completion documentation（pinned API 不存在，合同禁止拼接）。
- QuickInfo/display parts/逐 overload 文档（pinned API 不存在）。
- script URI 的诊断发布（per-file DTO/server 路由）。

**不要声称：**

- F1、P6、G5 已关闭；
- native rename 已实现；
- completion auto-import 已可用；
- hover 已等价于旧 QuickInfo。
