# P6-2 `G-P5-E2E-05` native symbols 最小实现设计

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD：`814ca5e`（`feat(tsgo): route script diagnostics and expression completion`）
- 日期：2026-11-20
- 范围：P6-2 剩余 gate `G-P5-E2E-05`（`real_node_symbols_smoke`）的 native 替代；`Backend::symbols()` 的 `DataRoots`/`DataPath`/`EventHandler` facts；与 documentSymbol、E2E-01 overlay、script diagnostics、completion/optionalTypes 的边界。
- 性质：**read-only 设计切片**。本文之外不修改生产代码、测试、Cargo、台账或 gate 断言，不 commit。
- 工作树漂移：读取时工作树已有并行未提交改动（`native_engine_real.rs`、`native_gates.rs`、F3/Zed/docs 等）。本文引用的 gate 计数以**工作树**为准；提交前必须按提交后的冻结列表重新核对。
- 结论先行：**`G-P5-E2E-05` 的旧 smoke 语义可以关闭为 opt-in native `gate_green`，但只能以“Rust script facts + 保守 Partial 规则”关闭。** 全量 Node `symbols.test.ts` 语义（语义 references/write roles、跨文件 rename、完整 setData/observer 覆盖）在当前 pinned `7.1.0-beta` 上只能 Partial，不能伪造 `complete=true`。

## 0. 一页结论

| 目标 | 结论 | 依据 |
| --- | --- | --- |
| `G-P5-E2E-05` / `native_symbols_smoke`（旧 smoke 的 DataPath + EventHandler + missing insertion） | **可关闭**（opt-in `gate_green`） | 固定 fixture 的 declaration、`this.data` read、setData segment、missing insertion 都可由 Rust 语法事实证明；见 §2、§3 |
| `TS-SYMBOLS-0061`（data path + this.data reference + setData segment） | **可关闭**（与 E2E-05 同一最小切片） | 同一组 scanner + exact byte ranges；见 §2.4 |
| `TS-SYMBOLS-0116`（component observerPath + missing handler insertion） | **只能 Partial**（除非同切片补 observer scanner） | `ScriptAnalysis` 目前完全没有 `observers`/`observer` 扫描；见 §1.3 |
| 全量 native symbols/rename parity | **BLOCKED** | pinned API 没有写角色、setData/observer、rename；见 §1.4 |
| 默认后端 / G5 / P6 完成 | **不由本文改变** | 本文只设计一个 opt-in gate；不删除 Node，不切换默认后端 |

**判定规则：** 如果验收把 E2E-05 定义为旧 `real_node_symbols_smoke` 的四个断言，本文方案可以关闭；如果验收要求 Node `symbols.test.ts` 的完整语义（任意 alias、动态 `this.data`、跨文件 behavior、写入角色、全量 observer），则只能 Partial，不能在本 pinned API 上关闭。

## 1. Gate 事实与精确需求

### 1.1 Gate 定义

- 台账 gate 行：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json:1295`。`target_gate` 是“symbols request over a real project with exact byte ranges”，`fail_closed` 是“complete=false or missing insertion must block edits; missing engine fails”。
- 旧测试：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:626`。四个实际断言是：
  1. `DataPath ["user","name"]`：`complete == true`；
  2. `declaration.is_some()`，且至少一个 `Read` reference 指向 `this.data.user.name`；
  3. 至少一个 `SetDataKey` occurrence，且 `path == ["user","name"]`；
  4. `EventHandler onTap`：`complete == true`、`declaration.is_some()`；`EventHandler onSave`（不存在）：`complete == true`、`insertion.is_some()`。
- 相关 case：`TS-SYMBOLS-0061`（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json:25104`）和 `TS-SYMBOLS-0116`（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json:25229`）。0061 与旧 smoke 基本重合；0116 额外要求 `observerPath` 和 component missing-handler insertion。
- 当前 native 侧仍是 `Unsupported`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:794`；原因常量在 ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:161`。现有单元测试 `symbols_are_explicitly_unsupported` 断言引擎启动次数为 0：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/tests.rs:2264`。
- 当前 gate 冻结：`GATE_GREEN_IDS` 16 个、`REQUIRED_NOT_MIGRATED_IDS 7` 个，E2E-05 仍在后者：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:33`、` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:55`。注意该文件有并行未提交改动；提交时不要把 16/7 写成历史 13/10。

### 1.2 `SymbolResult` 通道需求

`SymbolResult` 的字段定义在 ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/symbols.rs:176`。按 E2E-05 的消费者拆分：

| 通道 | 旧 smoke 是否直接断言 | 后续 LSP 消费者 | 最小实现需要什么 |
| --- | --- | --- | --- |
| `status` | 间接（`complete` 必须与 supported 一致） | `prepareRename`、`references`、`codeAction` | `Supported` 只在所有相关通道已证明时发布；否则 `Partial`/`Unsupported` |
| `complete` | 是（DataPath/EventHandler/missing 都为 true） | rename/code action 的硬闸门 | 任何未证明项 -> false；不能靠“fixture 能过”提升 |
| `reason` | 否 | `coverage`/日志/Partial 文案 | 有界、无绝对路径；与 status 原因一致 |
| `declaration` | 是（DataPath 和 onTap） | prepareRename、rename、references 去重 | 精确 token range；quoted key 排除引号 |
| `references` | 是（至少一个 `Read`） | references、rename、prepareRename 覆盖检查 | 至少 declaration + `this.data`/`this.<handler>` 可枚举 token；写角色未知时不能假报 |
| `occurrences` | 是（至少一个 `SetDataKey`） | rename 改 quoted path segment | literal `setData`/`observer` path 的精确 segment token |
| `insertion` | 是（missing onSave） | missing-handler quick fix | 只在字面量 owner 表完整时生成；格式安全 |
| `fields` | 否 | typo quick fix、rename 冲突 | 当前解析对象的 sibling 字段；DataRoots 的根字段 |
| `reserved` | 否 | rename 冲突（component data/properties/methods 共命名空间） | owner 已占用的 handler/data/property 名称 |

`SymbolToken` 的 `expected_text`（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/symbols.rs:91`）不是协议必填，但本设计应尽量填原 token 文本；`node_rename_plan` 在缺省时会从当前文件重新取文本（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/semantics/handlers.rs:1673`），填上可以更早发现 overlay/revision 漂移。

### 1.3 `ScriptAnalysis` 今天能诚实提供什么

现有 Rust script facts 在 ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/mod.rs:141`：

| 事实 | 现状 | 对 symbols 的直接价值 |
| --- | --- | --- |
| registration 选择/别名/歧义 | `RegistrationSelection`（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/mod.rs:122`） | 决定是否能证明 owner；ambiguous/nested -> Partial/Unsupported |
| registration options 精确对象范围 | `RegistrationFact.options_range`（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/registration.rs:74`） | missing handler insertion 的容器/闭合括号 |
| data 顶层字段 | `DataOverlay.fields`（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/properties.rs:95`） | DataRoots fields、DataPath 第一段、typo quick fix |
| component properties | `PropertyOverlay.properties`（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/properties.rs:110`） | component property 的 DataPath declaration/fields |
| behavior data/properties/methods | `BehaviorResolution`（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/behavior.rs:114`） | 可列出声明，但跨文件 references/rename 不能证明 |
| handler 方法表 | `MethodTable`（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:128`） | EventHandler declaration、fields/reserved、missing 判定 |
| handler 非字面量成员 | `MethodTable.opaque` | 不能当作 missing，也不能生成 insertion |
| PAGE_RESERVED | 私有常量（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:323`） | 需要公开 helper 才能构造 reserved |
| 嵌套 data path | **没有**；`DataOverlay` 只保留顶层字段 | `user.name` 的 declaration/fields 需要新 resolver |
| `this.data` references | **没有** | 需要新 scanner |
| `setData` occurrences | **没有**；Rust crate 内没有 `setData` 扫描 | 需要新 scanner |
| component `observers`/`observer` occurrences | **没有** | 0116 只能 Partial，除非补 scanner |
| missing handler insertion | **没有** | 需要新 builder |
| 语义 write roles/跨文件 references | **没有** | 只能保守 Partial，或依赖 native checker（本切片不选） |

关键结论：**`ScriptAnalysis` 已经足够提供 DataRoots、顶层 declaration、handler declaration、opaque/missing 判定、options 容器；缺口集中在嵌套路径、references、occurrences 和 insertion 四件事。**

### 1.4 pinned native API 的边界

- `getReferencedSymbolsForNode` 的响应是 `ReferencedSymbolEntry[]`，每项只有 `definition`、`references: string[]`、可选 `symbol`：` /tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:965`。`references` 是 node handle 字符串，不是带 byte range/写角色的 `ReferenceEntry`；proto 中没有 `isWriteAccess`（该文件内无匹配）。
- 因此 native session 即使能返回 handles，也不能直接填 `SymbolReferenceRole::Read/Write`，还要逐 handle 解码 AST、映射 shadow→original、处理 template fragment/generated probe。`RawReferencedSymbol` 当前也只保留 handles（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:505`）。
- pinned API 没有 `getRenameInfo`/`getEditsForRename`，也没有 `setData`/`observer` 专用查询；这与既有 F1 审计一致：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-19-tsgo-f1-query-gap-plan.md:522`。
- 结论：**native session 不是本切片的最小实现路径。** 它能把 references 变成“更像 checker”的证据，但补不上写角色、setData/observer 和 insertion，反而引入 session/shadow 映射风险。

## 2. 最小实现方案

### 2.1 选择：复用 `ScriptAnalysis`，不打开 native session

**采用方案 M1：`ScriptAnalysis` + 纯 Rust query/scanner。** 理由：

1. Symbol facts 的所有权已经在 `wxml-backend-tsgo/src/script`；event-handler completion 已经用同一条 no-session 路径（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/event_completion.rs:124`）。复用它可以避免 event completion 与 symbols 对“同一个 handler 是否存在”给出不同答案。
2. 旧 smoke 需要的四个通道中，declaration/insertion/fields/reserved 都是语法事实；只有 references/occurrences 缺扫描器。native session 只能补 references，且没有写角色。
3. 不打开 session 就没有 `RequestIdentity`/restart/epoch 的第二套语义；E2E-01 的 overlay 问题不会进入 symbols 路径（见 §4.1）。
4. 不启动引擎的单元/适配器测试可以稳定覆盖 negative matrix；gate 仍通过 `--backend native` 启动，保留 fail-closed 包/引擎前置条件。

**明确不选：**

- **纯 native session**：references handles 无写角色，setData/observer/insertion 仍要 Rust 扫描；等于同时维护两套事实。
- **ScriptAnalysis + native references 混合**：可以作为后续“语义 references”增强，但不是 E2E-05 的最小路径；本切片不引入。
- **把 `data`/`methods` 解析搬到 adapter**：会让 adapter 变成第二个 script analyzer，违反单一事实源。

### 2.2 文件/函数切片

| 文件 | 类型 | 函数/职责 | 预计生产行数 |
| --- | --- | --- | --- |
| `crates/wxml-backend-tsgo/src/script/query.rs` | 新 | `query_symbols(...)` 编排；`SymbolFacts`/`Completeness` 内部 DTO；query 校验 | ~220 |
| `crates/wxml-backend-tsgo/src/script/mod.rs` | 改 | 声明 `pub mod query;` 及新子模块；不改变现有 `ScriptAnalysis` 字段 | ~5 |
| `crates/wxml-backend-tsgo/src/script/data_path.rs` | 新 | `resolve_data_path`：data/properties 根、递归对象字段、sibling fields | ~220 |
| `crates/wxml-backend-tsgo/src/script/references.rs` | 新 | `collect_data_references`、`collect_handler_references`、`this` alias/escape 拒绝 | ~260 |
| `crates/wxml-backend-tsgo/src/script/occurrences.rs` | 新 | `collect_set_data`、`collect_observer_paths`（observer 可 phase 2） | ~300 |
| `crates/wxml-backend-tsgo/src/script/insertion.rs` | 新 | `handler_insertion`：Page/Component、已有 `methods`/无 `methods`、CRLF/尾逗号/注释拒绝 | ~180 |
| `crates/wxml-backend-tsgo/src/script/events.rs` | 改 | 公开 `pub(crate)` reserved/owner-container helper；不改现有语义 | ~30 |
| `crates/wxml-backend-tsgo/src/script/properties.rs` | 改（可选） | 公开 data/property member/object bounds helper，供 data_path 复用 | ~20 |
| `crates/wxml-lsp/src/backend_tsgo/symbols.rs` | 新 | adapter 组合：view 校验、AnalysisInput 构造、ScriptRange→SourceRange、caps、status | ~240 |
| `crates/wxml-lsp/src/backend_tsgo/mod.rs` | 改 | 声明 `pub(crate) mod symbols;`；`symbols()` 改为调用新模块；删除 `SYMBOLS_UNSUPPORTED` 文档常量；更新模块文档 | ~20 |
| `crates/wxml-lsp/src/backend_tsgo/tests.rs` | 改 | 替换 `symbols_are_explicitly_unsupported`，新增 adapter 测试 | ~250 测试行 |
| `crates/wxml-lsp/tests/native_parity/native_engine_real.rs` | 改 | 新增 `native_symbols_smoke`（LSP references/prepareRename/codeAction） | ~180 测试行 |
| `crates/wxml-lsp/tests/native_parity/native_gates.rs` / ledger | 改 | E2E-05 -> `gate_green`、17/6、`GATE_REPLACEMENTS`/`IGNORED_GATE_TESTS` | ~20 |

M1 生产代码约 1,100-1,300 行；测试约 700-900 行。若只做 gate smoke（不接 observerPath、不接 shared data），可以压到约 800 行生产代码。

### 2.3 数据流

`TsgoBackendAdapter::symbols(request)` 的最小流程：

1. `epoch_matches(&request.context)` 检查；不匹配返回 `BackendError::Cancelled`。
2. `prepare_view(self.views.as_ref(), request.context.source())` 取 `PreparedView`（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:84`）。**不调用 `prepare`/`ensure_open`/`with_session`。**
3. `view.target` 缺失 -> `SymbolResult::unsupported("the template has no page or component script")`。
4. `request.subject` 必须出现在 `view.files` 的 `file` 中；否则 Unsupported。不要自行按路径解析。
5. `view.source_complete == false` -> 直接 `Partial`（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:369`）。
6. 用 `view.files` 构造只读 `AnalysisInput`；复用 event completion 的构造逻辑（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/event_completion.rs:171`），建议提取 `script_input_from_view` 到 `backend_tsgo/input.rs` 或新 helper，避免两份 file→SourceFile 逻辑。
7. `analyze_script(&ScriptRequest::new(&input, subject_path, target.owner))`。`target.owner` 是 WXML 的 root；非 owner component script 依赖 `registration::select` 的 Page→Component fallback（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/mod.rs:223`）。
8. `script::query::query_symbols(&analysis, &input, subject_path, &request.query)` 返回内部 facts。
9. adapter 映射：`ScriptRange.utf8` -> `SourceRange`；`PathBuf`/path -> `FileId`（用 `view.files` 的 `file`，不要用 engine 路径）；`expected_text` 用 `PreparedFile.text` 的原 token 文本；最后统一 caps/status。
10. 返回 `SymbolResult`；**不写任何文件/编辑**，只返回 facts。

### 2.4 各通道算法

#### A. DataRoots

- `registration.selected` 缺失：
  - 无 candidates 且无 unsupported -> `Unsupported`（checked absent）；
  - 有 candidates 但 ambiguous/nested -> `Partial`，可发布已证明字段，但 `complete=false`。
- `fields`：
  - data 根：`analysis.data.fields` 的 `name/range/path`，`quoted` 由原文本首字符是引号判断；
  - component properties：`analysis.properties.properties` 同样映射；
  - 若字段声明在 behavior 文件（`field.path != subject_path`），可以返回 declaration，但 `complete=false`（跨文件 references/rename 未证明）。
- `reserved`：
  - Page/Behavior：`analysis.methods.methods` + `opaque` 的名字；
  - Component：methods/opaque + data 字段 + property 名（模拟 Node `ownerReservedNames`，`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/symbols.ts:1069`）；
  - 名字只保留合法 symbol 名并做 `MAX_SYMBOL_NAME_BYTES` 截断/跳过；超限 -> `complete=false`。
- `complete`：`registration.complete && data.status.is_supported() && properties.status.is_supported()` 且没有 behavior 跨文件字段/截断；否则 Partial。

#### B. DataPath

1. 校验 path：非空、`<= MAX_SYMBOL_PATH`（16）、每段非空且 `<= MAX_SYMBOL_NAME_BYTES`（256）、不含 `.`/`[`/`]`/空白/控制字符。非法 -> Unsupported（或 `BackendError::Protocol`，见 §4.8）。
2. 找根：在 selected registration 的 `data` 与（Component）`properties` 成员中查找第一段。两处同时声明 -> `Partial`（duplicate namespace）。Component property 的本体是 `{type, value, observer}` 选项对象，不是 data 值；因此 property 只承诺单段路径，`title.foo` 这类更深路径 -> `Partial`，不把 `type`/`observer` 当 data 字段。
3. 逐段解析：
   - `member_last`/`scan_members` 找唯一同名字段；重复 -> Partial；
   - `object_bounds` 要求值是单一对象字面量；
   - 值可能是 identifier/const/import：用 `values::resolve_binding_in`/`resolve_export` 解析（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/values.rs:268`）；跨文件解析成功可返回 declaration，但 `complete=false`；
   - spread/computed/shorthand/method -> `complete=false`，但保留已验证的前缀；
   - 数组/type reference/expression 根 -> `Unsupported` 或 `Partial`，不猜字段。
4. 末段：`declaration` = key token（string literal 去引号）；`fields` = 父对象的 sibling 字段（Node `resolveDataPath` 语义，`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/symbols.ts:370`）。
5. `references`：见 D；`occurrences`：见 E/F；`reserved`：见 A。

#### C. EventHandler

- 在 `analysis.methods.methods` 找 exact name：
  - 找到 -> `declaration` = `EventMethod.range`；`fields`/`reserved` = method table 名字 + opaque；
  - 找到但 `method.path != subject_path`（behavior 方法）-> declaration 可发布，`complete=false`（跨文件 refs 未证明）；
  - 在 `opaque` 中 -> 不是 missing；`complete=false`；不生成 insertion；
  - 没有且 `methods.status` 非 Supported（spread/computed/ambiguous/behavior incomplete）-> `complete=false`，不生成 insertion；
  - 没有且表完整 -> missing；生成 `insertion`（见 G）。
- `references`：见 D 的 handler 分支；`fields` 和 `reserved` 都填 `taken`（method+opaque），与 Node `handlerFacts` 一致（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/events.ts:335`）。

#### D. references 扫描（保守）

`references` 的诚实上限是“可枚举的语法引用”，不是 TypeScript checker 语义。规则：

- **data**：
  - 扫描 `this . data . <segments>`；只接受 identifier 链；`this[...]`/`this['data']` 等 computed 形式一律 `complete=false`；
  - `this.data[...]`、`this.data` 整体作为值、`this` alias、destructuring -> `complete=false`；
  - 链长度 `< query.len()`（祖先整体作为值）-> `complete=false`（比 Node 更严格，覆盖 `const u = this.data.user; u.name`）；
  - 链长度 `>= query.len()` 且前缀匹配 -> 第 `query.len()-1` 个 segment token 是一个 reference；若该 token 是赋值/复合赋值/`++`/`--` 的左侧 -> `Write`，否则 `Read`；
  - 声明 token 同时加入 `references`，role `Declaration`；与 `declaration` 去重；
  - `other.setData` / 其他文件 `.setData`：literal path 与 target 有交集 -> `complete=false`；无法判断 -> `complete=false`。
- **handler**：
  - 声明 token -> `Declaration`；
  - `this . <name>` / 已知 `this` alias 的 `.name` -> `Read`；
  - `this['name']`、destructuring、其他对象上的同名 key 不能证明 -> `complete=false`（不发布猜测 token）；
  - 任何 `this` alias（`const that = this`）-> 该 query `complete=false`。
- 所有 token 用 `ScriptRange`（UTF-8 bytes）；`expected_text` = 原文本精确切片。
- 上限 `MAX_SYMBOL_ENTRIES`（4096）；截断 -> `complete=false` + reason。
- 这套规则必然在复杂项目上返回 Partial；这是正确的降级，不是失败。

#### E. setData occurrences

- 只处理 `this.setData(<object literal>)`；`setData` 在其他 receiver 上 -> `complete=false`（不发布 occurrence）。
- 对象成员：
  - spread/computed/shorthand/method -> `complete=false`；
  - literal key 支持 dotted path（`"user.name"`）和嵌套对象（`{ user: { name } }`）；
  - key 中出现 `[`/`]`/空段 -> `complete=false`；
  - 对每个 path 与 target 取交集；目标 segment 在 literal key 内的 byte offset 必须精确可算；
  - 输出 `SymbolOccurrenceKind::SetDataKey`、完整 `path`、`segment`。
- 其他 project script 的 `.setData`：保守 `complete=false`（本切片不做跨文件 path token 发布）。
- `MAX_SYMBOL_PATH`/`MAX_SYMBOL_ENTRIES` 同 D。

#### F. observerPath occurrences（0116，phase 2）

`ScriptAnalysis` 完全没有 `observers` 扫描（Rust crate 中只有 Node `symbols.ts` 的实现）。因此：

- M1 若要保证 `complete` 诚实：只要 Component registration 含 `observers` 成员，或任一 property 的 options 含字符串 `observer`，且该 query 是 DataPath `title`，就必须 `complete=false`，reason 写明“component observer paths are not analyzed”。
- Phase 2 新增 scanner：
  - `observers` 的对象 key 或字符串字面量，按逗号/空白拆 path；
  - property `observer` 字符串；
  - wildcard `*`/`**` 不生成 token；
  - 只处理静态对象/字符串，动态 -> Partial。
- 0116 的 `observerPath` 断言在 phase 2 才可关闭；E2E-05 smoke 不依赖它。

#### G. missing handler insertion

- 只在 `EventHandler` missing、owner 表完整、`registration.fact.options_range` 可定位闭合 `}` 时生成。
- Page：容器 = options 对象；Component：优先 existing `methods` 对象，没有则插入 `methods: { ... }` 包装。
- 缩进/换行：
  - 从第一个成员行取 indent；无成员用 container indent + 2 spaces；
  - newline = 文本含 `\r\n` 则 `\r\n`，否则 `\n`（Node 规则，`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/symbols.ts:967`）；
  - 最后一个成员与 `}` 之间必须是空/仅逗号；含注释/其他 token -> 不生成 insertion，`complete=false`；
  - 需要时补 trailing comma；
  - text 长度 `<= 16 KiB`，不含 `\0`；offset 必须是 UTF-8 char boundary。
- `HandlerInsertion.file` = subject 的 `FileId`；`offset` 是闭合 `}` 的 UTF-8 byte offset；`text` 是完整格式化成员文本。
- 这里只产生 suggestion；`semantics/handlers.rs::handler_code_action` 仍会检查 `complete && declaration.is_none()` 和 `count_identifier_occurrences`（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/semantics/handlers.rs:1767`）。

### 2.5 status / complete 映射

必须满足以下不变式：

| 条件 | `status` | `complete` | 结果 |
| --- | --- | --- | --- |
| 无 target / subject 不在 view / 无 registration | `Unsupported` | false | 不发布任何字段/token |
| registration 有 literal 但 ambiguous/nested，或 data/methods 有 spread/computed/opaque | `Partial` | false | 发布已证明子集 + reason |
| behavior 跨文件 declaration | `Partial` | false | declaration 可发布；references/rename 不承诺 |
| `source_complete == false` | `Partial` | false | 空列表也不得是 supported empty |
| 扫描器遇到 `this` alias/dynamic `this.data`/other.setData/observer 未解析 | `Partial` | false | 已找到的 token 仍可发布，但 edit 必须拒绝 |
| 所有相关通道完整、无截断、无跨文件 | `Supported` | true | checked answer |
| 空列表 + `Supported` | `Supported` | true | 仅表示“已检查且确实没有”，不是 Unsupported |

硬规则：

1. `complete=true` 必须有 `status == Supported`；反之 `status != Supported` 时 `complete` 必须 false。
2. `reason` 只在 Partial/Unsupported 出现，长度 `<= 512` 字符/字节边界，去重，不写绝对路径。
3. `references`/`occurrences`/`fields`/`reserved` 每个数组 `<= MAX_SYMBOL_ENTRIES == 4096`；超限 -> 截断 + `complete=false`。
4. 任何 mapping 失败（未知 FileId、越界 range、不可映射 token）-> 丢弃该项 + `complete=false`；**绝不 clamp**（与 `document_symbol.rs` 的 no-clamp 规则一致：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/document_symbol.rs:147`）。
5. `SymbolField.quoted` 只在原 key 是 `"..."`/`'...'` 时为 true；`declaration.range` 排除引号。
6. 不得因为旧 smoke 断言 `complete=true` 就把未知通道标真；fixture 必须选在可证明子集内。

### 2.6 与 documentSymbol 的分工

- `Backend::symbols` 只服务 `SymbolQuery::{DataRoots, DataPath, EventHandler}`；**不要**新增 `SymbolQuery::DocumentSymbols`，也不要把 outline 塞进 `SymbolResult`。
- `textDocument/documentSymbol` 的 WXML 路由仍是静态 outline；script 路由单独走 `Backend::document_symbols`，由 `structure.rs` 分支调度：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/structure.rs:937`、` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/structure.rs:1034`。
- native document symbols 只接受 prepared view owning script：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:801`；symbols 可以接受 view 内非 owner script（component property），但两条路径不共享 result/缓存，也不互相当 fallback。
- scheduler 的 `JobKind::Symbols` 与 `JobKind::DocumentSymbols` 保持独立（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/scheduler.rs:456`）；symbols 不触发 outline，outline 不触发 symbols。
- 不允许把 script symbols 合并进 WXML `DocumentSymbol[]`（range 属于不同文档），这一点与既有 DTO 计划一致：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-document-symbols-dto-plan.md:13`。

### 2.7 缓存与 overlay 身份

- Symbols 不打开 session，所以不继承 `RequestIdentity` 的 non-owner 指纹缺口（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-20-tsgo-e2e01-overlay-replay-plan.md:212`）。但 `ScriptAnalysis` 读的是 `PreparedView.files` 的 effective text；每次请求都应使用当前 view 的 files。
- 若加分析缓存，复用 event cache 的 key 形态：project id、project root、script path、owner kind、`source_complete`、排序后的 `(path, source_revision)` hash（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/event_completion.rs:74`）。**不要把 WXML cursor/view revision 放进 key**，否则每次编辑 WXML 都重新分析 script；但必须把 script revision/overlay hash 放进去。
- 单条缓存足够 gate；若同时存在多 subject（page + component property），需要小 LRU；否则一次 non-owner 查询会替换 owner 缓存，造成抖动。
- symbols 与 event completion 可以共享 `ScriptAnalysis` helper，但不要共享“最后一次 cache”状态，除非 key 包含 owner kind/path 且 query 无状态。

## 3. 测试矩阵

### 3.1 `wxml-backend-tsgo` 单元测试（无引擎）

| 用例 | Fixture | 断言 |
| --- | --- | --- |
| `page_data_path_smoke` | `Page({ data:{user:{name:"x"}}, onLoad(){const l=this.data.user.name; this.setData({"user.name":l});} })` | status Supported、complete true、declaration exact、Read ref exact、SetDataKey path `["user","name"]`、segment 1（对齐 TS-SYMBOLS-0061） |
| `root_data_path_setdata_first_segment` | `data:{user:{name:"x"}}` + setData `"user.name"`，query `["user"]` | occurrence token 是 `user` 段、`fields` 含 `user`（对齐 TS-SYMBOLS-0098） |
| `component_property_observer_path` | `Component({properties:{title:{type:String,observer:"title"}}, observers:{"title, open": fn}, methods:{onTap(){}}})` | M1 期望 Partial（observer 未扫描）；phase 2 期望 observerPath segment 0 |
| `component_missing_handler_existing_methods` | 上一个 fixture，query `onSave` | `complete=true`、declaration None、insertion 含 `onSave()` 且不含 `methods: {`（对齐 TS-SYMBOLS-0116） |
| `component_missing_handler_wraps_methods` | `Component({properties:{title:String}})` | insertion 含 `methods: {` + `onSave()` |
| `page_missing_handler_trailing_comma` | Page 最后成员后有/没有逗号 | 不重复逗号；offset 在 `}`；text 缩进正确 |
| `page_missing_handler_crlf` | CRLF script | insertion text 用 `\r\n`；offset byte exact |
| `missing_handler_ambiguous_or_spread_has_no_insertion` | `Page({...extra})` / 多 registration | complete false、insertion None |
| `opaque_handler_is_not_missing` | `Page({onSave: handlerRef})` | complete false、insertion None |
| `behavior_data_and_method_are_partial` | Component + 静态 behavior | declaration 可来自 behavior 文件；complete false；无跨文件 rename 承诺 |
| `registrar_alias_is_supported` | `const P=Page; P({data:{user:{name:"x"}}, onLoad(){this.data.user.name;}})` | declaration/read 与直接 Page 相同 |
| `static_spread_resolves_dynamic_spread_partial` | `const shared={...}; data:{...shared}` vs `data:{...get()}` | 前者可解析；后者 complete false |
| `dynamic_data_and_other_receiver_partial` | `this.data[key]`、`const {user}=this.data`、`other.setData({user:1})` | complete false，不发布猜测 occurrence |
| `cjk_and_emoji_byte_ranges` | 注释/字段含 `中文`、emoji；`名称: "值"` | declaration start/end 是 UTF-8 字节；quoted 正确 |
| `quoted_key_tokens_exclude_quotes` | `"user.name"` / `'title'` | token 不含引号；expected_text 不含引号 |
| `caps_and_reason_are_bounded` | >4096 refs/occurrences | 截断、complete false、reason 有界 |
| `invalid_query_is_unsupported` | 空 path、17 段、`a[b]`、handler `a.b` | 不 panic；Unsupported/Protocol；不查文件 |
| `subject_outside_view_is_unsupported` | subject 不在 `PreparedView.files` | Unsupported，不解析路径 |
| `no_registration_is_unsupported_not_empty_supported` | `const x=1;` | Unsupported + empty arrays |
| `source_truncated_is_partial` | `source_complete=false` | Partial，绝不 supported empty |

### 3.2 `backend_tsgo` adapter 测试（fake view，无引擎）

| 用例 | 断言 |
| --- | --- |
| `symbols_do_not_start_a_native_session` | `FakeLog.starts == 0`；无 completion/hover 调用；替换现有 `symbols_are_explicitly_unsupported` |
| `symbols_map_script_ranges_to_original_script` | UTF-8 `SourceRange` 与 prepared text 一致；`expected_text` 正确 |
| `symbols_target_owner_fallback_for_component_subject` | Page view + component subject -> `analyze_script` 走 Component fallback；property declaration 正确 |
| `symbols_use_overlay_text_not_disk` | `PreparedView.files` 的 overlay 文本改变 declaration/ref；不读 disk |
| `symbols_reject_subject_outside_view` | Unsupported；无 file path 推断 |
| `symbols_never_publish_partial_as_complete` | Partial + complete false；Supported + complete true |
| `symbols_stale_epoch_is_cancelled` | epoch mismatch -> `BackendError::Cancelled` |
| `symbols_caps_truncation_and_reason` | 超限数组截断、`complete=false`、reason 无绝对路径 |

### 3.3 LSP / native gate 测试

旧 `real_node_symbols_smoke` 是直接 `NodeBackend` 测试；native replacement 应走公开 `--backend native` LSP，因为 integration test 不能访问 `pub(crate)` adapter。

`native_symbols_smoke`（`native_engine_real`）建议 fixture：

```text
app.json
pages/index/index.wxml
  <view bindtap="onTap">{{ user.name }}</view>
  <view bindtap="onSave"></view>
pages/index/index.ts
  Page({
    data: { user: { name: 'Ada' } },
    onLoad() { const label = this.data.user.name; this.setData({ "user.name": label }); },
    onTap() {},
  });
tsconfig.json
```

断言：

1. `textDocument/references` 在 `name`：返回至少 2 个 location，declaration 与 `this.data.user.name` 的 byte range 精确。
2. `textDocument/prepareRename` 在 `name`：返回 `range` + `placeholder`，不是 `null`。
3. `textDocument/rename`：WorkspaceEdit 包含 declaration + read + `"user.name"` 的 `name` segment；没有编造的写入。
4. `textDocument/codeAction` 在 `onSave` 的 WXML range：返回 `Create event handler 'onSave'` quickfix；`EditPlan` 文本含 `onSave()`，offset 是 `}` 的 byte offset。
5. CJK/CRLF 变体：`名称`/emoji 前缀 + `\r\n` script，declaration/ref/insertion offset 仍正确。
6. `assert_no_node_exec`；**不要**在 typed page 上断言 `assert_no_engine_exec`：`open_wxml` 会等待 diagnostics，diagnostics 可能启动 engine；symbols 本身仍不打开 session，由 §3.2 的 `starts == 0` 单独证明。
7. 旧 `real_node_symbols_smoke` 继续 `#[ignore]`，P7 前不删除。
8. `native_lsp::Session::start` 当前 initialize 的 `capabilities` 为空（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_lsp.rs:112`）；`codeAction`/`rename` 需要 `workspace.workspaceEdit.documentChanges: true`（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/semantics/handlers.rs:630`）。测试 helper 必须先扩展初始化 capability，否则 codeAction 会直接返回 `[]`，形成假失败/假绿。

Gate 台账更新：

- `GATE_REPLACEMENTS` 增加 `("G-P5-E2E-05", "native_symbols_smoke")`。
- `IGNORED_GATE_TESTS` 增加对应 `(NATIVE_GATE_FILE, "native_symbols_smoke")`。
- `GATE_GREEN_IDS` 增加 E2E-05；`REQUIRED_NOT_MIGRATED_IDS` 删除 E2E-05；计数从工作树的 16/7 改为 17/6。
- ledger gate 行改为 `gate_green`，记录命令、pinned engine/asset identity、run head、passed/filtered；related cases 只记录已有证据，不自动提升 `TS-SYMBOLS-0061/0116`。
- `native_gates.rs` 的文档/冻结计数同步；当前工作树已有并行改动，提交前重新读源码确认，避免把 13/10 的历史值写进去。

### 3.4 CJK / CRLF / 边界专项

- 所有 token 都是 UTF-8 byte range；UTF-16 只在 LSP 输出边缘由 `LineIndex` 转换。测试 `中文`、emoji（4 UTF-8 bytes / 2 UTF-16 units）、BOM。
- CRLF：`insertion.text` 用 `\r\n`；declaration/ref 的 byte offset 不受 `\r\n` 与 `\n` 换算影响（因为只存 UTF-8 bytes）。
- `expected_text` 必须精确等于原 token；CRLF/BOM 不得归一化。
- 0 字节/越界/非 char boundary 的 range 直接丢弃并 `complete=false`；不 clamp。

### 3.5 真实引擎是否必要

- **unit/adapter**：不需要引擎；fake view + `FakeLog.starts == 0` 是主要证据。
- **gate**：需要 native package/engine 身份，因为 `--backend native` 启动会做 discovery/verification；但 symbols 查询本身不打开 session。
- 如果团队要求“symbols 必须由引擎计算”，本设计不满足：pinned API 没有 symbol facts 查询，必须 BLOCKED。不要为了 gate 绿而伪造一次无意义的 engine 调用。

## 4. 风险与冲突

### 4.1 与 E2E-01 overlay 的交叉

- Symbols 必须从 `PreparedView.files` 的 effective text 读取；这是 overlay-first 的同一份数据（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-20-tsgo-e2e01-overlay-replay-plan.md:197`）。
- 不要为 symbols 引入 `RequestIdentity`/session 更新；否则会继承 E2E-01 的 non-owner 指纹/restart 首请求问题。若未来加缓存，key 必须含 `(path, source_revision)` 全量 hash。
- E2E-01 的 `invalidate`/restart 不影响 no-session symbols；但 symbols 的 `epoch_matches` 仍要保留，避免旧 epoch 的结果覆盖。

### 4.2 与 script diagnostics routing 的交叉

- `script_diagnostics` 走 `prepare_script` + `ScriptRequestPlan`，会打开/复用 session（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:309`、` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:507`）。
- Symbols 走 `prepare_view`，不碰 session；两者可以并发，不共享 mutable state。不要为了“统一”把 symbols 塞进 script session，否则一个 script diagnostics 的 plan 切换会让 symbols 的 subject/range 失去上下文。
- 若共享 `ScriptAnalysis` 缓存，key 必须包含 subject path/owner kind；script diagnostics 的 view 与 template view 的 files 可能不同，不能混用。

### 4.3 与 completion / optionalTypes 的共享文件

- `event_completion.rs` 已经构造 `AnalysisInput`、缓存、status 映射（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/event_completion.rs:171`）。建议提取 `script_input_from_view` / `analysis_cache_key` 到共享 helper，但这是一次**跨切片重构**，需要与 completion/optionalTypes 的 owner 串行。
- `script/properties.rs` 与 `script/events.rs` 同时被 `optionalTypes`、event completion、symbols 使用。M1 只加 `pub(crate)` helper，不改 `DataField`/`PropertyFact`/`MethodTable` 字段；若必须加字段，所有构造点/测试一起改。
- `backend_tsgo/mod.rs` 同时承载 hover/diagnostics/definition/completion/document_symbols；symbols 分支只加一行调用，避免和并行分支冲突。
- `backend_tsgo/tests.rs` 的 `FakeViews`/`FakeBehavior`/`FakeLog` 是共享测试缝；先冻结形状再加 symbols 测试。

### 4.4 与 documentSymbol 的重复风险

- 不要把 `DataPath` 的 `fields` 转成 `DocumentSymbol`，也不要把 script outline 合并进 WXML outline。两条 DTO 的 range 坐标系/complete 语义不同。
- `SymbolResult.complete` 是 rename/reference 合同，不是 outline 合同；document symbols 用 `truncated/skipped/gaps`。不要互相借用状态字段。
- 若未来需要“script data field outline”，应新增独立 query/DTO，不复用 `SymbolQuery::DataRoots`。

### 4.5 不能伪造 `complete`

以下任一条件必须 `complete=false`，即使旧 gate 断言 true：

- registration ambiguous/non-top-level/spread/non-literal；
- data/properties/methods 有 spread/computed/shorthand/opaque；
- declaration 在 behavior/其他文件，references 未证明；
- `this` alias、`this.data[...]`、`this.data` 整体值、destructuring；
- 其他 script 的 `.setData` 可能命中 target；
- component `observers`/`observer` 未扫描（M1 的 0116 必须 Partial）；
- 任何 token range 不可映射、数组截断、`source_complete=false`；
- native/engine 语义 references 不可得（本设计本来就不声称）。

这条规则优先于“gate 变绿”；如果为了绿而提升 complete，`node_rename_plan` 会生成不完整 edit，违反安全性。

### 4.6 Node parity 差异

- Node 使用 TypeScript checker 的 `findReferences`/`getReferencesAtPosition`（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/symbols.ts:468`），Rust scanner 没有类型信息；复杂 alias/泛型/type-only 声明会 Partial。
- Node `resolveObjectLiteral` 支持同文件/paths/imports 的 checker 解析（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/symbols.ts:228`）；Rust 只能做 relative import + 简单 const/export 解析，`baseUrl`/`paths` 需要 `ModuleResolver`，本切片不接。
- Node `scanDataEscapes` 只检查 `this.data` 整体值（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/symbols.ts:582`）；本设计更严格（祖先整体值也拒绝），所以某些 Node `complete=true` 的 case 在 Rust 会 Partial。这是可接受的保守差异，但需在 case 证据里写明。
- Node 对 Page data 的 `reserved` 只含 handler 名；Component 含 data/properties/methods。Rust 必须照此实现，不能把 Page 的所有 data 字段塞进 reserved（会误报重名）。
- `ScriptAnalysis.methods` 会合并 behavior 方法（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/mod.rs:280`），而 Node `handlerFacts` 的直接查询不带 behavior 方法（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/events.ts:335`）。M1 应对 behavior 贡献的 `methods/fields/reserved` 标记 `Partial`，否则 `reserved` 会比 Node 多出 behavior 名字；不要因此误报冲突。

### 4.7 安全/资源

- `reason` 不含绝对路径、engine 路径、文件系统错误原文；有界 512。
- 所有数组/路径/名字按 `wxml_types` 常量截断；不分配超过输入大小的内存。
- 不执行 shell/Node；不读 prepared view 之外的文件。
- `SymbolError::Invalid` 目前没有被 `Backend::symbols` 使用（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/symbols.rs:79`）。建议 invalid query 返回 `SymbolResult::unsupported`（不 panic、不启动任何进程），不要为此改 trait 签名。

### 4.8 gate 冻结列表冲突

- 当前工作树的 `native_gates.rs` 已经是 16 green / 7 required，且 `native_engine_real.rs` 有并行未提交的 E2E-03/11/16 改动。E2E-05 的实现必须基于这些改动之后的文件，不要按历史 `13/10` 或旧行号写断言。
- `IGNORED_GATE_TESTS` 目前含重复的 `native_hover_and_diagnostics_smoke` 条目（工作树并行改动）；这不是本文引入的，但提交 E2E-05 前应确认 `native_gates.rs` 的去重/计数测试通过，否则 gate 冻结列表会先失败。
- ledger 的 `proposed_command` 仍是 `cargo test -p wxml-lsp --test native_engine_real -- --ignored`；实际应使用 `--test native_parity native_engine_real -- --ignored`（与 E2E-06 记录一致）。改 gate 行时一并修正，避免证据命令不可复现。

## 5. 结论与实现顺序

### 5.1 结论

1. **E2E-05 smoke：可关闭。** 用 `ScriptAnalysis` + `DataPath` 嵌套 resolver + `this.data`/`setData` scanner + handler insertion，固定 Page fixture 可以达到旧 smoke 的四个断言，且不伪造 `complete`。
2. **E2E-05 全量语义：只能 Partial。** component observerPath、跨文件 behavior/shared data、写角色、动态 alias/type-only 解析无法用当前 Rust 语法层证明；pinned native API 也没有 `isWriteAccess`/setData/rename。
3. **全量 native symbols/rename：BLOCKED。** 不要删除 Node 的 `real_node_symbols_smoke`，不要声称 `TS-SYMBOLS-0061/0116` 全部完成；只记录 gate 级证据。
4. **不改变默认后端。** symbols 是 opt-in native adapter 的一部分；默认仍是 Node，P6/G5 未关闭。

### 5.2 下一步实现顺序

1. **P0：建立 query 骨架（无行为变化）**
   - `script/query.rs` 定义内部 `SymbolFacts`/`Completeness`；实现 `DataRoots` + `EventHandler` declaration/fields/reserved。
   - `backend_tsgo/symbols.rs` 接线，替换 `symbols_are_explicitly_unsupported`；断言 `FakeLog.starts == 0`。
   - 先让 `DataRoots`/`EventHandler` 的 Partial/Unsupported 语义正确；此阶段不关闭 gate。

2. **P1：DataPath 嵌套 + setData + data references**
   - `script/data_path.rs` 递归解析 `data`/`properties` 对象。
   - `script/occurrences.rs` 实现 `SetDataKey`。
   - `script/references.rs` 实现 `this.data.<path>` 保守读取/写入；negative matrix（alias/destructuring/computed/other receiver）。
   - 添加 TS-SYMBOLS-0061 等价单元测试；此时 E2E-05 smoke 的数据半边可绿。

3. **P2：handler references + insertion**
   - `script/insertion.rs` 实现 Page/Component/methods 包装/CRLF/尾逗号/注释拒绝。
   - `script/references.rs` 实现 `this.<handler>`；negative matrix（opaque/behavior/ambiguous）。
   - 添加 TS-SYMBOLS-0116 的 missing-handler 部分；observer 部分保持 Partial。

4. **P3：adapter/gate 接线**
   - 加 `native_symbols_smoke` LSP 测试：references/prepareRename/rename/codeAction。
   - 更新 `GATE_REPLACEMENTS`/`IGNORED_GATE_TESTS`/`GATE_GREEN_IDS`/`REQUIRED_NOT_MIGRATED_IDS`/ledger；当前 16/7 -> 17/6。
   - 跑 opt-in native job（pinned engine/assets），记录真实证据；不删除旧 Node 测试。

5. **P4：observerPath / shared data（case parity）**
   - `script/occurrences.rs` 加 component `observers` + property `observer`。
   - `script/data_path.rs` 加 shared data/export 解析；跨文件仍 `complete=false`。
   - 通过 TS-SYMBOLS-0116/共享数据 case 后，才把相关 case 从 `planned` 提升。

6. **P5（可选/后续）**
   - 若要“语义 references”，再评估 native `getReferencedSymbolsForNode` + AST handle 解码；仍需解决写角色缺失与 shadow 映射，不应阻塞 E2E-05。
   - 全量 rename 仍需要 native rename API 或项目级 recheck，不在本切片。

### 5.3 验收命令（实现后）

- `cargo fmt`
- `cargo clippy -p wxml-backend-tsgo -p wxml-lsp --all-targets -- -D warnings`
- `cargo test -p wxml-backend-tsgo script`
- `cargo test -p wxml-lsp backend_tsgo`
- opt-in native gate：按 ledger 的 pinned engine/typings 环境，运行 `cargo test -p wxml-lsp --test native_parity native_engine_real -- --ignored --test-threads=1`，至少 `native_symbols_smoke` 通过。
- 证据记录必须写实际 HEAD、run date、engine SHA-256、asset/typings root、passed/filtered 数；不能把 plan 行号或旧计数当作证据。
