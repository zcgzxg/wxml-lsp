# F1 event-handler completion 实施计划（tsgo-migration）

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 写作时 HEAD：`bff58d1`（`docs(evidence): verify real typings provenance`）
- 修订时 HEAD：`939831b`（S1 顶层注册选择已提交；本文的 S1 段已按该提交更新）
- 前置计划：`docs/evidence/2026-11-19-tsgo-f1-query-gap-plan.md`（提交 `1519a73`；结论：event-handler completion 是下一个可行 slice，auto-import/per-entry docs/quick-info display parts/script URI diagnostics/positive rename 仍 BLOCKED）
- 日期：2026-11-19
- 本文性质：**只读设计 + 实施计划**。写作期间没有修改生产代码、测试、`Cargo.toml`、合同、ADR、默认后端或 Node；只新增本文件。
- 行号基准：写作时工作树。`crates/wxml-lsp/src/backend_tsgo/{mod,input,tests}.rs` 存在未提交的 trusted type-assets 接线改动；这些改动的行号可能与本文明细略有偏移，但引用的符号名和判定点不变。
- 证据边界：本文所有“现状”均来自当前工作树代码；没有运行大规模测试，没有下载/安装，没有启动 native/Node 引擎。所有测试状态仍是计划，不得把本文当作已实现证据。
- 修订：2026-11-19 review BLOCK 修订版，处理 H1–H4、M1–M8、L1–L8；未改生产代码。
- 引用约定：绝对路径 + 符号名是权威锚点；`:line` 只是写作时辅助，会随并行改动漂移。

---

## 0. 结论先行

1. **event-handler completion 现在就能做，而且不需要 native session 或 checker。**
   候选来自 `ScriptAnalysis.methods`（Page/Component 方法表 + 静态 Behavior 合并），不是 `getCompletionsAtPosition`。现有 Rust script 语义已经覆盖注册、`methods`、Behavior、JSDoc、signature 和 `Partial` 降级。
2. **推荐架构：LSP 仍走 scheduler/backend，native adapter 内新增一个“纯脚本”快速分支。**
   `CompletionKind::EventHandler` 已经在 DTO 和 LSP 路由中存在；只需要在 `TsgoBackendAdapter::completion` 里先处理 EventHandler，再进入现有 ExpressionMember 路径。该分支**不调用** `self.prepare`、`ensure_open`、`NativeSession::completions`，因此不会启动 native 进程，也不会阻塞协议线程。
3. **不新增 DTO。** 复用 `CompletionKind::EventHandler`、`CompletionResult`、`CompletionItemKind::Method` 和现有 `CapabilityStatus`。`documentation`/`detail` 来自脚本声明（与事件 hover 同源），**不是** native `getCompletionEntryDetails`；不得把它写成 native completion 详情。
4. **状态规则必须是可证明的：**
   - 完整字面量表 → `Supported`，可返回候选；
   - spread/computed/多个注册/非字面量 `Component.methods`/Behavior 图不完整/opaque 成员 → `Partial`，只发布已证明的字面量方法；
   - 没有任何注册 → `Unsupported` 空列表，不能 `Supported` 空；
   - host view/输入构造失败 → `Err(BackendError::Unavailable)`；
   - 超过 256 条 → `Partial` + `truncated=true`。
5. **触发语法采用 H1 方案 2：保留宽松直接形式，只精确拒绝冲突形态。**
   `bind`/`catch`/`capture-bind`/`capture-catch`/`mut-bind` + 非空全小写后缀继续当作直接事件（因此 `bindgetphonenumber`/`bindchooseavatar`/`bindplay`/`bindpause` 不会回归）；冒号形式继续接受任意非空事件名。只显式拒绝裸前缀、空事件、`bindex` 等冲突形态。`EVENT_NAMES` 只用于属性名补全，不用于值触发判定。
6. **替换范围是“属性值内、光标前的连续 handler 标识符前缀”，不是 `Context::replace`。**
   `Context::replace` 在带引号值里从值起点开始，会把前导空格一起替换；event 分支必须自己按 token 计算。推荐 `replace = trimmed_token_start..cursor`，在值中间时只替换已输入前缀（与 Node 的 `eventReplace` 一致）。
7. **顺手修 `completion_list` 的 `isIncomplete:false` 和静默截断。**
   本切片只把它改成 `isIncomplete = result.truncated || result.items.len() > MAX_COMPLETION_ITEMS`；这是 event handler 截断真实性的必要条件。native `isIncomplete` 的完整 DTO 字段（A0）仍另开切片。
8. **不修 mustache 成员上下文 guard。** 现有 `Site::Mustache → ExpressionMember` 对 `{{ Us }}` 也会走 native；这是表达式补全问题，和事件 handler 无关，且与 bare-root/data completion 设计耦合。不要在本切片顺手改，也不得声称它已修。
9. **本切片不关闭 F1、P6 或 G5。**
   F1-21（native completion details/auto-import）、B-07、F1 hover/rename 等仍开放；默认后端仍是 Node；G-P5-E2E-21 仍是 Node gate；P6 切换和 G5 均未完成。

### 0.1 一页决策表

| 决策 | 选择 | 理由 |
| --- | --- | --- |
| 候选来源 | `ScriptAnalysis.methods` | 已有注册/方法表/Behavior/降级；不需要 checker |
| 直接事件触发 | 保留 `bind`/`catch` + 非空全小写后缀的宽松形式 | 兼容 `bindgetphonenumber` 等非 `EVENT_NAMES` 事件；`EVENT_NAMES` 只用于属性名补全 |
| 是否启动 native session | 否 | 事件补全是脚本语法事实；启动 session 只会增加延迟和失败面 |
| 是否新增 DTO | 否 | `CompletionKind::EventHandler`、`CompletionResult` 已存在 |
| 是否新增 `CompletionGap` | 否 | `CapabilityStatus::Partial { reason }` 足够；native gap 是另一条线 |
| 路由位置 | `completion/expression.rs` + `TsgoBackendAdapter::completion` | 保持现有 scheduler/stamp/取消语义 |
| 顶层注册选择 | `939831b` 已提交；S2 复用 `selection_integrity` | 唯一顶层完整表才 Supported |
| 替换范围 | 值内 handler 前缀 `token_start..cursor` | 不删除已有尾字符、不碰引号/空格 |
| 截断 | 256 + `Partial` + `truncated` | 合同 §9.7 要求截断可见 |
| 缓存键 | 项目根 + 脚本/owner + 文件 revisions + `source_complete` | 排除 `view_id`/模板 revision，避免每次 WXML 编辑必 miss |
| `isIncomplete` | `result.truncated` 或超 cap | 本切片只修本地截断；native `isIncomplete` 另议 |
| 默认后端/Node | 不改 | 本切片只接原生 opt-in 适配器 |

### 0.2 现状锚点

- LSP 已经把事件属性值路由到 `CompletionKind::EventHandler`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/expression.rs:20
- native adapter 目前只接受 `ExpressionMember`，EventHandler 直接 `Unsupported`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:556
- native completion 适配器同样只接受 `ExpressionMember`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/completion.rs:36
- native completion 的 pinned API 只有 `getCompletionsAtPosition`，没有 per-entry details/auto-import；本切片不依赖它：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/completion.rs:29

---

## 1. 语义边界（问题 1）

### 1.1 唯一事实源：注册根 + 方法表

事件 handler 补全只回答一个问题：**“这个 WXML 所属的 Page/Component 注册里，有哪些可以静态证明为函数的方法名？”**

- 注册根来自视图发现，不来自模板文本：
  - `PreparedView.target.owner: RootKind`（`Page`/`Component`） /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:198
  - `ViewTarget` 同时给出 `script`、`script_file`、`view_revision` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:198
- 注册事实来自 `ScriptAnalysis`：
  - `ScriptAnalysis.methods: MethodTable` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/mod.rs:140
  - `MethodTable { status, owner, methods, opaque, problems }` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:128
  - `build_table` 把 Page 顶层成员、Component `methods`、Behavior 方法合并，并对 spread/computed/非字面量降级 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:223
- `ScriptRequest` 的 `RegistrationKind` 决定注册根：
  - `RootKind::Page → RegistrationKind::Page`；`RootKind::Component → RegistrationKind::Component`
  - `select` 对 Page 请求允许 Page→Component fallback /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/registration.rs:489
- Behavior 方法来自 `BehaviorResolution.methods`，由 `build_table` 在自有方法之后合并，自有同名方法优先 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:223
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/behavior.rs:112

**不使用** `ScriptAnalysis.status` 作为事件补全状态。`status` 还会包含 `data`/`properties` 的问题；这些不影响 handler 枚举。事件补全使用 `methods.status` + 本文的 opaque/截断规则。

### 1.2 哪些成员是候选

| 注册事实 | 是否候选 | 依据 / 降级 |
| --- | --- | --- |
| `Page({ onTap() {} })` | 是 | `MemberKind::Method`，`add_member` 判为 literal /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:434 |
| `Page({ onLoad: function () {} })` | 是 | `function` 关键字字面量 |
| `Page({ onArrow: () => {} })` | 是 | 括号箭头；需要修正目前的“任意括号”误判，见 §1.4 |
| `Page({ onArrow: x => {} })` | 是 | 标识符 + `=>`；需要确认 `=` 后是 `>`，见 §1.4 |
| `Page({ onAsync: async () => {} })` | 是 | `async` + `(` + `=>` |
| `Page({ data: {...} })` | **否** | `PAGE_RESERVED` 排除 `data` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:328 |
| `Page({ properties: {...} })` | **否** | `PAGE_RESERVED` |
| `Page({ methods: {...} })` | **否** | `PAGE_RESERVED`（Page 没有 `methods` 容器语义） |
| `Page({ observers/lifetimes/pageLifetimes/behaviors/options/externalClasses/relations })` | **否** | `PAGE_RESERVED` |
| `Page({ __wxml_* })` | **否** | 生成 shadow 成员，`collect_page` 跳过 |
| `Component({ methods: { save() {} } })` | 是 | `collect_component` 只扫描 `methods` 对象 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:382 |
| `Component({ properties: { save: {...} } })` | **否** | 不是 `methods`；属性描述不是 handler |
| `Component({ data: {...} })` | **否** | 不是 `methods` |
| `Component({ attached() {} })` 等生命周期 | **否** | 在 `methods` 之外 |
| `Component({})` / `Component({ methods: {} })` | checked empty | 没有 `methods` 或空对象是“组件确实没有 handler”，`Supported` 空列表 |
| `Behavior({ methods: { onBack() {} } })`，且被当前组件静态引用 | 是 | 合并进方法表；自有同名覆盖 Behavior /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:223 |
| Behavior 的 `data`/`properties` | **否** | 不是方法表 |
| `Page({ onTap })` 简写 | **否**（opaque） | 不能证明值是函数；不发布，整体 `Partial` |
| `Page({ onTap: handlerRef })` | **否**（opaque） | 同上 |
| `Page({ onTap: this.onTap })` | **否**（opaque） | 成员表达式，不是字面量函数 |
| `Page({ onTap: makeHandler() })` | **否**（opaque） | 调用表达式 |
| `Page({ ...mixins })` | **否**，且 `Partial` | spread 可能隐藏任意 handler |
| `Page({ [name]: fn })` | **否**，且 `Partial` | computed key 可能隐藏任意 handler |
| `Component({ methods: someObject })` | **否**，且 `Partial` 空 | `methods` 不是字面量对象 |
| `Component({ methods: { ...mixin } })` | 已知字面量方法可选，`Partial` | spread 可能隐藏方法 |
| 多个同 kind 注册 | 用第一个/顶层表，`Partial` | 不做并集；见 §1.4 |
| `Page` + `Component` 同文件 | 由 `ViewTarget.owner` 决定；Page 请求可 fallback Component | 与现有 `select` 一致 |
| 注册别名 `const p = Page; p({...})` | 是 | `registration.rs` 的静态 alias 解析 |
| 直接自定义事件 `bindgetphonenumber`/`bindchooseavatar`/`bindplay`/`bindpause` | 是 | 宽松直接形式；不要求 `EVENT_NAMES`，见 §2.2 |
| 被遮蔽的 `Page`/非字面量参数 | 无候选，`Partial`/`Unsupported` | `scan.unsupported`，禁止猜 |
| 非标识符方法名（如 `'on-tap'`） | **否** | 不能写成 `bindtap="on-tap"`；用与 `resolve_one` 相同的 `is_identifier_name` 过滤 |
| 字符串 key `'onTap'` | 是（name 去引号） | 能写回 `bindtap="onTap"` |
| Unicode 标识符方法名 | 是 | `is_identifier_name` 接受 alphabetic/alphanumeric；见 §1.4 |

### 1.3 降级矩阵（绝不伪造）

| 条件 | `CompletionResult.status` | items | 说明 |
| --- | --- | --- | --- |
| 完整字面量表，无 opaque，未截断 | `Supported` | 全部已证明方法 | 可以发布 `Supported` |
| spread / computed / 多注册 / 非字面量注册 | `Partial` | 已证明方法 | reason 来自 `methods.problems` |
| `Component.methods` 非字面量 | `Partial` | 空 | 不是 `Supported` 空 |
| Behavior 图不完整 | `Partial` | 已解析方法 | 不报 missing |
| `view.source_complete == false` | `Partial` | 已解析方法 | prepared source list incomplete |
| opaque 成员（合法 handler 名） | `Partial` | 不含该名 | 不能发布无法证明可调用的名字 |
| 非标识符成员名 | 不降级 | 过滤掉 | 不可绑定，不是“表不完整” |
| 候选 > 256 | `Partial` | 前 256 | `truncated=true`；reason 含截断 |
| 无任何注册 | `Unsupported` | 空 | 不得 `Supported` 空 |
| 注册形状被拒绝/遮蔽且无字面量选中 | `Partial` | 空 | 可能有动态注册 |
| 脚本扫描失败 | `Unsupported` | 空 | reason 用通用文案，不泄露源码 |
| 光标不在静态事件值 | `Unsupported` | 空 | 防御性；正常 LSP 路由不会到 |
| 无 target（模板没有 Page/Component 脚本） | `Unsupported` | 空 | 与 Node “template has no script” 对齐 |
| owning script 不在 `view.files` / 不可表示 | `Err(BackendError::Unavailable)` | — | M3；不是空 `Unsupported` |
| host `prepare` 失败 / 输入不可构造 | `Err(BackendError::Unavailable)` | — | 不是空 `Supported` |
| epoch 不匹配 | `Err(BackendError::Cancelled)` | — | 沿用现有 adapter 屏障 |

### 1.4 必须的前置小修（否则会伪造）

这些不是“顺手重构”，而是事件补全真实性的前置条件。每一项都要独立测试。

1. **箭头函数判定过宽（高优先级）。**
   当前 `add_member` 把“值以 `(` 开头”一律当函数，且把“标识符后跟 `=`”一律当箭头函数：
   /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs:434
   这会把 `Page({ onTap: (condition ? a : b) })`、`Page({ onTap: foo = 1 })` 发布成 handler。修法：
   - `(` 开头：用 `script.close_of(value_start)` 找匹配 `)`，从 `)` 之后向前扫描到 `=` + `>`；找不到就是 opaque。
   - 标识符开头：要求 `value_start` 是 Identifier，且 `value_start+1` 是 `=`、`value_start+2` 是 `>`。
   - `async` 开头：`async (` 走括号箭头；`async function` 是字面量；`async id =>` 要求 Identifier + `=` + `>`。
   - `function` 开头保持字面量。
   这也会让事件 hover/definition 更保守：无法证明的成员不再被当成方法。
2. **opaque 在完成层降级。**
   `MethodTable.status` 在只有 opaque 时仍是 `Supported`（`add_member` 只加入 `opaque`，不 push `problems`）。完成层必须：
   - 不发布 opaque 名字；
   - 如果 `opaque` 中存在合法 handler 名，则结果 `Partial`，reason “one or more handler-shaped members are not literal functions”。
   不要改 `MethodTable.status` 本身，否则会误伤 missing-handler 诊断（opaque 只影响同名，不影响其它名字的缺失证明）。
3. **非标识符候选过滤（L4）。**
   复用 `wxml_backend_tsgo::shadow::expression::is_identifier_name`（公开函数），不要新增 `is_handler_name`；`'on-tap'`、数字键、含 `.`/`-` 的字符串 key 都不发布。`script/events.rs` 的私有 `is_identifier_text` 应改为调用同一函数，保证解析与补全一致。
4. **多注册选择顶层（H3，S1 已落地：`939831b`）。**
   S1 之前 `select` 只优先“非 alias”，不优先 `top_level`；如果函数内先出现一个 `Page`，顶层再出现一个，会选中函数内的表，而 `top_level=1` 不 ambiguous，导致错误 `Supported`。该缺陷已由 `939831b` 修复；下面保留修法与测试，作为 S2 完成层必须复用的规则。
   /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/registration.rs — `fn select`（:489）
   同步：`script/registration.rs` 的 `selection_is_ambiguous` 与 `script/events.rs` 的 `fn build_table`（:223）。
   修法（必须同切片）：
   - `select` 顺序改为：同 kind 中 `top_level && alias.is_none()` → `top_level` → `alias.is_none()` → 第一个字面量。
   - `selection_is_ambiguous` 与 `build_table` 使用同一规则：有 `top_level` 时只按 `top_level` 数量判定；否则按同 kind 字面量数量判定。
   - 测试：函数内 `Page(...)` + 顶层 `Page(...)` → 选顶层、`Supported`；两个顶层 → `Partial`；只有函数内一个且无顶层 → 选它（保留现状），测试记录该语义。
   - 完成层加一个防御性检查：若存在 `top_level` 注册但 `selected` 不是 `top_level`，强制 `Partial`；S1 落地后该分支不应触发（测试可保留，防止回归）。

### 1.5 与 Node oracle 的差异（有意为之，必须写清）

| 行为 | Node `completeEventHandlers` | 本计划 | 处理 |
| --- | --- | --- | --- |
| Page 顶层成员 | 全部 property（含非字面量） | 只发布可证明字面量函数 | Rust 更保守；opaque 不发布 |
| Component | `methods` 成员 | 同 | 一致 |
| Behavior 方法 | **不包含**（只读 registration） | **包含**（方法表已合并） | 有意增强；与事件 hover/definition 一致 |
| 非标识符 key | 发布 label | 过滤 | 不发布不可绑定名 |
| detail | `member.getText()` 截 120 | `EventMethod.signature` 截 120 | 都是单行声明摘要；不是 native entry detail |
| documentation | `docTextOf` | `EventMethod.documentation` | 同一 JSDoc 事实；不是 native completion docs |
| 排序 | `localeCompare` | BTreeMap 字节序 + `sort_text=0_<name>` | 不承诺 locale 级 parity |
| 直接自定义事件 `bindmyevent`/`bindgetphonenumber` | 宽松直接形式接受 | 宽松直接形式接受 | 与 H1 一致；`EVENT_NAMES` 不参与值路由 |
| 替换范围 | 前缀 `offset-prefix..offset` | 同样的前缀 token 范围 | 中间光标会保留尾字符；这是冻结行为 |

结论：不要声称“与 Node 逐字节 parity”。本切片的目标是**可证明的 handler 候选**；差异要在证据和 README 中写明。

---

## 2. 触发上下文（问题 2）

### 2.1 判定链

LSP `textDocument/completion` 的现有链路：

1. `DocumentRoute::Wxml` 守卫：脚本/JSON/样式直接返回空列表。
   /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs:195
   /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/mod.rs:27
2. `workspace` 文档 + `offset_at` + `request_context`。
3. 静态 semantics（WXS/template/import）优先；事件值不会被它截走。
4. `context::detect(text, offset)` 得到 `Site`。
   /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/context.rs:24
5. `expression::route(&detected)` 分类：
   - `Site::Tag` / `Site::AttributeName` → `Static`
   - `Site::AttributeValue` + **精确事件属性分类器** → `Backend(EventHandler)`
   - `Site::AttributeValue` 非事件 → `Static`
   - `Site::Mustache` → `Backend(ExpressionMember)`（本切片不改）
   /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/expression.rs:15

### 2.2 精确词法规则：属性名（H1 方案 2）

新增 `catalog::is_event_value_attribute(attribute)`。它与现有 `is_event_prefix` 的宽松直接形式兼容，只增加**精确拒绝**，避免 `bindgetphonenumber`/`bindchooseavatar` 等非 `EVENT_NAMES` 事件回归。

**接受**

1. 冒号形式：`capture-bind:<event>`、`capture-catch:<event>`、`mut-bind:<event>`、`bind:<event>`、`catch:<event>`；`<event>` 非空，字符集 `[A-Za-z0-9_-]`。
2. 直接形式：`capture-bind<event>`、`capture-catch<event>`、`mut-bind<event>`、`bind<event>`、`catch<event>`；`<event>` 非空且全为 ASCII 小写字母（与当前 `is_event_prefix` 的直接形式一致）。

**显式拒绝表**

| 属性 | 拒绝原因 |
| --- | --- |
| `bind` / `catch` / `capture-bind` / `capture-catch` / `mut-bind` | 裸前缀，没有 event |
| `bind:` / `catch:` / `capture-bind:` / `capture-catch:` / `mut-bind:` | event 为空 |
| `bind::tap` / `bind:foo:bar` | event 以 `:` 开头或含 `:`，不合法 |
| `bindex` | 已知普通属性，与 `bind` + `ex` 冲突 |
| `binding-mode` / `bind-anything` | 直接形式后缀含 `-`，不满足“全小写字母” |
| `bind:` + 只有空白/非法字符 | 不满足冒号形式的 event 字符集 |

**保留与说明**

- `bindgetphonenumber`、`bindchooseavatar`、`bindplay`、`bindpause`、`bindended`、`bindwaiting`、`bindprogress`、`bindfullscreenchange`、`bindmarkertap`、`bindcallouttap`、`bindcontroltap`、`bindpoitap`、`bindanchorpointtap`、`binddragstart`、`binddragend` 全部按直接形式接受；它们不在 `EVENT_NAMES` 也不影响值路由。
- `EVENT_NAMES` 只用于**属性名补全**（生成 `bindtap`/`catchtap`/`bind:tap` 等）；docs/metadata 读取同一目录，但它**不用于**事件值触发判定。
- `bind:foo` / `bind:myevent` 是合法冒号事件，接受。Review H1 的 `bind:foo` 解释为 `bind:` 前缀形态：本设计拒绝 `bind:` 空事件与 `bind::...` 非法事件；`bind:foo` 本身作为完整合法事件保留。若产品确实要求字面拒绝 `bind:foo`，会与当前 Node/自定义冒号事件冲突，需要单独的产品/合同决策，不应由本补全切片暗改。
- `parse_event_attribute` 继续承担“已知事件属性的验证/归一化”，**不是**分类器；不要为了值路由改它的语义。分类器在 `catalog`，二者用交叉测试防漂移。
- `catalog::is_event_attribute` 用于 docs/metadata，保留现状；不要为了事件值补全改它的语义。
- `catalog::is_event_prefix` 继续服务属性名补全（它会接受 `bindex`，这是属性名场景的已知宽松行为）；值路由改用新分类器，不再直接使用它。

**测试**

- 正向：`bindgetphonenumber`、`bindchooseavatar`、`bindplay`、`bindpause`、`bindtap`、`catchtap`、`bind:tap`、`catch:tap`、`capture-bind:tap`、`capture-catch:tap`、`mut-bind:tap`、`bind:my-event`。
- 负向：`binding-mode`、`bindex`、裸 `bind`/`catch`、`bind:`/`catch:`、`bind::tap`。
- 交叉：新分类器接受的属性应能被 `parse_event_attribute` 接受（`bind:foo` 等冒号形式亦然）。

### 2.3 精确词法规则：属性值

在 `Site::AttributeValue { attribute, quoted, .. }` 且分类器为 true 后，event 模块在 worker 上重新执行一次 `context::detect` 并计算完整值范围（不把 LSP 私有 `Context` 放进 C0 DTO；检测是线性扫描，成本可接受）。推荐复用 `styles::value_bounds`（它已经处理引号、`{{...}}` 跳过、未加引号到空白/`>`）：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/styles.rs:273
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/styles.rs:421

规则（`value` 为 `value_bounds` 返回的完整值范围，`quote` 为它返回的引号字节）：

```text
1. cursor 必须在 value 范围内。
2. 引号权威来自 value_bounds 的 Option<u8>：
   - Some(quote) 才是“引号内”；
   - None 是未加引号值；
   - 与 Site::quoted 不一致时拒绝（防御）。
3. value 不得包含 "{{"、"}}"、'\n'、'\r'；否则拒绝（动态/跨行值）。
4. 去掉 value 两端 ASCII 空格/制表符，得到 [trim_start, trim_end)。
5. 若 trim_start == trim_end（空值或纯空白）：
   - 只允许 cursor == trim_start；
   - 返回零宽替换 cursor..cursor。
6. 非空：
   - 要求 value[trim_start..trim_end] 是单个合法 handler 名
     （复用 shadow::expression::is_identifier_name）；
   - 要求 trim_start < cursor <= trim_end；cursor 在 token 起点时拒绝（避免前插）；
   - 返回 replace = trim_start..cursor；prefix = value[trim_start..cursor]。
```

M5 的关键用例：`<view bindtap=|"onTap"` 中 `value_bounds` 返回 `quote=None`，`value` 是 `"onTap"`（含引号），不是单标识符 → 拒绝。只有 `value_bounds` 返回 `Some(quote)` 才认为光标在引号内；不能仅凭 `Site::quoted` 判断。

`is_identifier_name` 语义（`shadow::expression::is_identifier_name`，公开函数）：

- 首字符 `_`/`$`/`alphabetic`；
- 后续 `_`/`$`/`alphanumeric`；
- Unicode 字母/数字允许；`-`、`.`、空格、引号不允许。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/shadow/expression.rs:158

L4 说明：不要新增 `is_handler_name`；直接复用 `shadow::expression::is_identifier_name`。`script/events.rs` 的私有 `is_identifier_text` 可改为调用它（或保留但只作为内部别名），事件解析与补全必须共享同一判定。

L8 已知行为：`bindtap="on|Tap"` 会接受 `onTap` 后得到 `onTapTap`（只替换 `on`，保留 `Tap`）。这是 Node `eventReplace` 的前缀替换语义；若产品要求整 token 替换，另立合同/测试变更，不在本切片暗改。

L5 层级依赖：`event_completion` 虽然位于 `backend_tsgo`，但它在同一个 `wxml-lsp` crate 内使用 `completion::context::detect` 和 `styles::value_bounds`；这些是 LSP 私有模板事实，不是 C0 DTO。依赖方向是 `backend_tsgo -> completion/styles`，`styles`/`completion` 不反向依赖 `backend_tsgo`。若未来要把 `value_bounds` 提升为 `completion::context::attribute_value_range`，另做无行为变化重构，不在本切片。

为什么不用 `Context::replace`：

- 带引号值：`Context::replace = value_start..cursor`，`prefix` 是从值起点到光标的全部文本，包含前导空格。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/context.rs:446
- 未加引号值：`Context::replace` 会覆盖整个 token，包括光标后的尾字符。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/context.rs:489
  事件补全要保留“只替换已输入前缀”的语义，不能直接使用它。

### 2.4 测试矩阵

**正向（应到 `Backend(EventHandler)`）**

| 输入（`|` 为光标） | 期望 |
| --- | --- |
| `<view bindtap="|" />` | quoted AttributeValue；零宽替换 |
| `<view bindtap="on|" />` | EventHandler；replace `on` |
| `<view catchtap="on|" />` | EventHandler |
| `<view bind:tap="on|" />` | EventHandler |
| `<view catch:tap="on|" />` | EventHandler |
| `<view capture-bind:tap="on|" />` | EventHandler |
| `<view capture-catch:tap="on|" />` | EventHandler |
| `<view mut-bind:tap="on|" />` | EventHandler |
| `<view bind:my-event="on|" />` | EventHandler（冒号自定义） |
| `<view bindgetphonenumber="on|" />` | EventHandler（非 `EVENT_NAMES` 直接事件） |
| `<view bindchooseavatar="on|" />` | EventHandler（同上） |
| `<view bindplay="on|" />` | EventHandler（同上） |
| `<view bindpause="on|" />` | EventHandler（同上） |
| `<view bindcancel="on|" />` | EventHandler（直接事件；也在 `EVENT_NAMES`） |
| `<view bindtap='on|' />` | 单引号 EventHandler |
| `<view bindtap=on|>` | 未加引号 EventHandler；replace `on` |
| `<view bindtap=|>` | 未加引号空值；零宽 |
| `<view bindtap=|` EOF | 未加引号空值；零宽 |
| `<view bindtap="on` EOF | 未闭合引号；EventHandler；replace `on` |
| `<view bindtap="  on|" />` | EventHandler；replace 从 `on` 开始，不含空格 |
| `<view bindtap="  |" />` | EventHandler；空格后零宽 |
| `<view bindtap="onTap|" />` | EventHandler；替换整个 `onTap`（cursor=end） |
| `<view bindtap="on|Tap" />` | EventHandler；只替换 `on`，保留 `Tap`（冻结行为） |
| 中文/emoji 前缀后接事件值 | UTF-8 偏移正确；不切字符 |

**负向（不得进 EventHandler）**

| 输入 | 原因 |
| --- | --- |
| `<view binding-mode="on|" />` | 直接形式后缀含 `-`，不满足全小写字母 |
| `<view bindex="on|" />` | 显式拒绝：已知普通属性，与 `bind`+`ex` 冲突 |
| `<view bind="on|" />` | 裸前缀，无事件名 |
| `<view bind:="on|" />` | event 为空 |
| `<view bind::tap="on|" />` | event 以 `:` 开头，不合法 |
| `<view wx:if="on|" />` | 非事件属性 |
| `<view class="on|" />` | 非事件属性；class 路由优先 |
| `<view data-onTap="on|" />` | 非事件属性 |
| `<view bindt|` | AttributeName，不是值 |
| `<view bindtap=|"onTap"` | `value_bounds` quote=None，value 含引号，不是单标识符；仅靠 `Site::quoted` 会误判 |
| `<view bindtap="|onTap" />` | cursor 在 token 起点，拒绝前插 |
| `<view bindtap="onTap |" />` | cursor 在尾随空格，拒绝追加第二个值 |
| `<view bindtap="{{ onT| }}" />` | `Site::Mustache` → ExpressionMember，不是 EventHandler |
| `<view bindtap="pre{{onT}}|" />` | 混合动态值，值不是单标识符 |
| `<view bindtap="onT{{x}}|" />` | 同上 |
| `<view bindtap="onTap()|" />` | 不是单标识符 |
| `<view bindtap="item.onTap|" />` | 不是单标识符 |
| `<view bindtap="onTap extra|" />` | 两个 token |
| `<view bindtap="onTap"|` | 已闭合引号，光标不在值内 |
| `<view bindtap="on-|" />` | handler 名不合法（`-`） |
| `<view bindtap="123|" />` | handler 名不能以数字开头 |

补充（L2）：`bindtap="{{` 未闭合时，`scan_mustache` 返回 `None`，`scan_tag` 返回 `TagScan::Continue(offset)`，`detect` 的文本循环在 `index >= offset` 时返回 `None`；它不会落到 `Site::AttributeValue`，所以不会误触发事件补全。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/context.rs:428

---

## 3. 数据来源（问题 3）

### 3.1 推荐：`ScriptAnalysis`，不是 native completion

推荐直接调用 `wxml_backend_tsgo::script::analyze_script`，读取 `ScriptAnalysis.methods`。

理由：

1. **native completion 不能枚举“注册的方法表”。** `getCompletionsAtPosition` 是“某个脚本位置的表达式/全局补全”，不是“这个 Page/Component 的 options 成员”。要得到后者，必须构造 `this.` 或 `Page({...}).` 之类的合成查询，既不可靠，也会把全局/导入/auto-import 条目混进来。
2. **Rust 方法表已经覆盖事件语义。** `build_table` 已处理 Page 顶层、Component `methods`、Behavior 合并、reserved key、spread/computed、opaque、预算和 `Partial`。
3. **不需要 native session。** 事件补全是纯语法事实；启动 session 会增加延迟、失败面和“类型资产不可用”耦合。
4. **不需要 checker 的方法可见性/成员列表。** Page/Component/Behavior 的对象字面量方法没有 TS class 的 private/protected 可见性；`methods` 的键就是可调用名。checker 只能帮助解析 alias/动态对象，而那些情况当前必须保守降级。
5. **`symbols` 不是候选源。** `ScriptAnalysis.symbols` 只包含已解析的稳定符号，且 `Method`/`BehaviorMethod` 混在 data/property/registration 中；它没有 `opaque`/`problems`，不能表达“表不完整”。候选源用 `methods`。
6. **`events` 不是候选源。** `events::resolve_events` 是“给定绑定名，解析一个 handler”，不是“枚举候选”。

### 3.2 输入构造：必须用原始脚本，不是 shadow

`PreparedView.files` 保存的是**原始有效脚本文本**（overlay 优先），而 `build_request_plan` 在构造 native 输入时会把 owning script 替换成 in-place shadow：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:298
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:304

事件补全不能使用 shadow 文本：

- shadow 会在脚本中注入 `__wxml_*` 和挂载片段，范围/成员语义都不是原文件；
- 事件候选要对应真实的脚本声明和 JSDoc；
- `PreparedView.files` 已经是 overlay-first 的有效文本，能反映未保存编辑。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:2877
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-project/src/sources.rs:312

推荐构造（H4：复用 `input.rs` 的路径无关映射，不复制私有 `host_error`/`input_error`）：

```text
let view = input::prepare_view(provider, context.source())?; // HostError -> BackendError::Unavailable
let target = view.target.as_ref() else { Unsupported };
if !view.files.iter().any(|f| f.path.as_str() == target.script.as_str()) {
    return Err(BackendError::Unavailable("owning script is missing from the prepared view"));
}
let kind = match target.owner { Page => RegistrationKind::Page, Component => RegistrationKind::Component };
let mut dropped = 0usize;
let mut files = Vec::new();
for f in view.files.iter() {
    match SourceFile::new(f.path.as_str(), f.text.to_string(), DocumentRevision::new(f.source_revision)) {
        Ok(file) => files.push(file),
        Err(_) if f.path.as_str() == target.script.as_str() => {
            return Err(BackendError::Unavailable("owning script cannot be represented for script analysis"));
        }
        Err(_) => dropped += 1,
    }
}
let input = AnalysisInput::new(
    ProjectKey::new(context.file().project().as_str()),
    ProjectRoot::new(view.project_root.to_str()?)?,
    SnapshotRevision::new(1),
    files,
).map_err(|error| input::input_error(&error))?;
let analysis = analyze_script(&ScriptRequest::new(&input, target.script.as_str(), kind));
```

M3 规则（替换旧 `filter_map(...).ok()`）：

- **不允许静默丢文件。** 任一 `SourceFile::new` 失败：
  - owning script → `Err(BackendError::Unavailable)`；
  - 其它文件 → `dropped += 1`，最终状态强制 `Partial`，reason 为 “N script files could not be represented”。
- `target.script` 不在 `view.files` → `Err(BackendError::Unavailable)`，不是 `Unsupported` 空。
- `AnalysisInput::new` 失败 → 一律 `Err(BackendError::Unavailable)`。**删除单文件 fallback**：`PreparedView` 的 `collect_sources(SourceLimits::default())` 已把文件数限制在 2048、总文本限制在 16 MiB，均低于 `AnalysisInput` 的 20k/48 MiB；`TooManyFiles`/`TooLarge` 在本路径不可达，保留 fallback 只会制造死代码。
- `view.source_complete == false` → `Partial`（prepared source list incomplete），不是 `Unavailable`。
- 这一步会复制文件文本（`SourceFile::new` 取 `String`）。现有 `build_request_plan` 在每次 native 查询时也复制所有文件文本，因此这不是本切片引入的新量级；缓存（§3.5）把复制限制在脚本/文件集变化时。未来若要消除复制，可把 `SourceFile.text` 改成 `Arc<str>` 或给 `script` 层引入借用快照，但那是独立重构，不在本切片。
- 不需要 `AnalysisInput::with_environment`；事件补全不挂载类型资产。

**H4 冲突面**：`input.rs` 的 `host_error`/`input_error` 是私有函数（`crates/wxml-lsp/src/backend_tsgo/input.rs:611` 与 `:615`）。本设计选择在 `input.rs` 暴露：

```rust
pub(crate) fn prepare_view(provider: &dyn ViewProvider, snapshot: &SourceSnapshot)
    -> Result<PreparedView, BackendError>;
pub(crate) fn input_error(error: &InputError) -> BackendError; // 或 pub(crate) map
```

因此 `input.rs` 进入冲突表，S2 必须排在 trusted type-assets handoff 之后；若集成 owner 不允许改 `input.rs`，退路是在 `event_completion.rs` 做本地、path-free 的 `HostError -> Unavailable` / `InputError -> Unavailable` 映射，并在 rebase 后复查。

### 3.3 不需要 NativeSession 的新方法

在本切片中**不要**给 `NativeSession`/`QuerySession` 增加：

- `completion_members_of_registration`
- `get_method_visibility`
- `get_completion_entry_details`
- `prepare_auto_imports` 相关方法

`QuerySession` 的现有 `completions` 只服务 ExpressionMember；EventHandler 分支在调用 `prepare`/`ensure_open` 之前返回。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:104
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:552

### 3.4 避免跨 namespace / 动态方法

- `analyze_script` 只读 `ScriptRequest.input` 中的文件；Behavior 解析用 `Snapshot::resolve`：
  - 相对 specifier 在输入快照内解析；
  - 非相对 specifier 需要 `ModuleResolver`；adapter 当前不提供，所以 `@/behaviors/x`、`tsconfig paths` 别名会解析失败并让 Behavior 图 `Partial`。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/snapshot.rs:38
- 不扫描项目里所有脚本的 `onTap`；不做 project-wide 方法索引；不把其它页面/组件的同名方法当候选。
- 不解析 `data`/`properties`/`observers`/`lifetimes` 里的名字。
- 不把 `import { onTap } from './handlers'; Page({ onTap })` 的 `onTap` 当候选；它是 shorthand/alias，`add_member` 会放 opaque。
- 不猜测 `Object.assign`、mixin 函数、类实例、`Page(new X())` 的成员；这些保持 `Partial`/空。

### 3.5 缓存（H2：键不能含 `view_id`）

`DependencyIndex` 只跟踪页面/组件脚本、JSON、tsconfig、组件关系，**不跟踪 Behavior 脚本的 import 图**：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/dependencies.rs:29

同时，`PreparedView.view_id` 含 `view_revision`，每次 WXML 编辑都会变并触发 `slot.prepared.clear()`；把 `view_id` 放进缓存键会让“打字时必然 miss”。脚本分析不依赖模板文本，因此：

- 缓存位置：`TsgoBackendAdapter` 内一个 `Mutex<Option<EventCandidateCache>>`，只缓存一个 entry（或 2-entry LRU）。
- 键 `EventCacheKey`：
  1. 项目根（`view.project_root`）；
  2. `target.script` 路径；
  3. `target.owner`（Page/Component）；
  4. `view.source_complete`；
  5. 对 `view.files` 的 `(path, source_revision)` 排序后做稳定哈希（FNV-1a 或等价）。
- **排除** `view_id`、`view_revision`、模板 `source_revision`、offset/prefix。模板文本变化但脚本文件集合/revisions 不变时，键相同、可以命中；这是产品允许的行为。
- 值：`Vec<EventMethod>`（或已过滤候选）+ `CapabilityStatus` + `source_complete` + 基础 `truncated` 信息。
- 命中：直接复用候选，只按当前 offset 计算 replace/prefix。
- 失效：任一脚本 revision 变化、文件集变化、项目根/owner 变化；不需要跟 native epoch 绑定（事件补全不依赖 session）。
- 不缓存 `analyze_script` 的 `Err`；错误每次重新尝试，避免把临时 I/O 失败固化。
- 缓存命中/未命中加 debug log（不含路径/源码）。

**测试（H2/M8）**：

- `cache_key_ignores_template_revision`：同一脚本文件集合/revisions、不同 `view_id`/`view_revision`/offset → key 相等。
- `cache_key_changes_with_behavior_revision`：Behavior 文件 `source_revision` 变化 → key 不等，重新分析后候选变化。
- `cache_key_changes_with_owner_or_project_root`。
- 不使用全局 `Atomic` 计数；用 `EventCacheKey` 等值单元测试 + “Behavior revision 变化导致候选变化”的行为测试。若确实要测 hit，用显式注入的 `Arc<dyn Fn>`/test-only counter，并随测试实例释放。

---

## 4. DTO 与状态（问题 4）

### 4.1 复用现有 DTO

- `CompletionKind::EventHandler` 已存在：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs:193
- `CompletionResult { items, status, truncated }` 已足够：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs:290
- `CompletionItemKind::Method` 已存在，LSP 映射为 `kind=2`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs:223
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs:99

为什么事件补全可以是 `Supported`：F1-21 的 `Partial` 约束针对 **native completion 条目**（没有 per-entry docs/auto-import）。事件候选是宿主侧脚本方法表事实，不依赖这些 native 字段；当方法表完整、未截断时，`Supported` 是诚实的。native adapter 的整体 `completion` capability 仍是 `Partial`，因为 expression-member 路径仍受 F1-21 约束。

不新增：

- 不新增 `CompletionKind::EventHandlerMember` 之类的 kind；
- 不新增 `CompletionGap` 到 C0；`CapabilityStatus::Partial { reason }` 足够表达本切片的降级；
- 不改 `CompletionItem` 字段；`additionalTextEdits`/`data` 仍不做（auto-import/详情是另一条 BLOCKED 线）。

### 4.2 Item 字段来源

| 字段 | 来源 | 规则 |
| --- | --- | --- |
| `label` | `EventMethod.name` | 去引号后的静态名 |
| `kind` | `CompletionItemKind::Method` | 只发布已证明函数 |
| `detail` | `EventMethod.signature` | 压成单行；最多 120 个字符；超出加 `…`；不用 Node 的 member source |
| `documentation` | `EventMethod.documentation` | JSDoc 文本；最多 2048 个字符；空则 `None`；这是脚本声明文档，不是 native completion details |
| `insert_text` | `EventMethod.name` | `PlainText` |
| `insert_text_format` | `PlainText` | 不生成 snippet |
| `replace` | 值内 handler 前缀 | `trimmed_token_start..cursor`；见 §2.3 |
| `sort_text` | `0_<name>` | 与 Node 的 `sortText` 形状对齐；不承诺 locale 排序 |
| `filter_text` | `name` | 客户端按名过滤 |

文档与详情边界：

- `documentation` 不是 `getCompletionEntryDetails`，也不是 native `symbol` 文档拼接；它是 `scripts/events` 已经为 hover/definition 使用的同一 JSDoc 事实。
- `detail` 不是 native entry detail；它是 `signature_of` 的一行摘要。
- 若以后有人要求“native per-entry documentation”，仍按 F1-21/B-07 BLOCKED 处理，不能把本切片的字段当证据。
- `detail`/`documentation` 的上限按 Rust `char` 计数，保证不切断 UTF-8/代理对；Node 按 UTF-16 code units，非 BMP 字符上可能有 1–2 个字符差异。这是安全优先的有意差异，不声称逐字节 parity。

### 4.3 状态与空列表的精确规则

```text
base = analysis.methods.status（Supported / Partial）
if no registration selected and no candidates/unsupported:
    return Unsupported empty
if no registration selected but registration-shaped call rejected:
    return Partial empty (reason from methods.problems)
if selected:
    candidates = methods.methods filtered by is_identifier_name, deduped (BTreeMap already unique)
    // 遍历时最多保留 MAX+1 个以判定 truncation，避免先 clone 全部
    if methods.opaque contains any legal handler name:
        base = Partial (reason: opaque handler-shaped members)
    if candidates.len() > MAX_COMPLETION_ITEMS:
        truncate to 256; base = Partial (reason: list truncated); truncated = true
    if !view.source_complete:
        base = Partial (reason: prepared source list incomplete)
    if dropped_script_count > 0:
        base = Partial (reason: N script files could not be represented)
    return { items, status: base, truncated }
```

**L6 精确顺序**（实现必须按此顺序，避免“先截断后过滤”导致假 truncated）：

1. 从 `methods.methods` 过滤 `is_identifier_name`（L4）。
2. 去重（`MethodTable` 已是 BTreeMap，理论上无重复；行为方法已由 `build_table` 去重）。
3. 遍历时最多保留 `MAX_COMPLETION_ITEMS + 1` 个，判定 `overflow`。
4. 取前 256；`truncated = overflow`；若 `overflow`，status 至少 `Partial`。
5. 再合并 opaque / `source_complete` / 基础 `methods.status` 的 reason。
6. 最后构造 items 和 replace；任何 item 构造失败见 §4.4 M7。

**L7 空集判定**：不要用 `methods.status` 判断“有没有注册”。`missing_registration` 也会返回 `methods.status = Partial`。空集必须由 `analysis.registration.selected` / `candidates` / `unsupported` 判定：

- `selected.is_none()` 且 `candidates.is_empty()` 且 `unsupported.is_empty()` → `Unsupported`；
- `selected.is_none()` 但存在 `candidates`/`unsupported` → `Partial` 空；
- `selected.is_some()` → 使用 `methods.status` + 本文规则。

补充：

- `Supported` + 空：完整表确实没有候选（`Page({data:{}})`、`Component({})`、`Component({methods:{}})`）。
- `Partial` + 空：表不完整（`Component({methods: someObject})`、spread-only 等），但没有任何可证明候选。不是 `Unsupported`。
- `Unsupported` + 空：没有注册/脚本扫描失败/光标不在事件值。
- `Unavailable`：host/输入不可用；推荐用 `Err(BackendError::Unavailable)`，由 scheduler 记录并以空列表响应，和现有 ExpressionMember 的 host 失败路径一致。
- 任何状态都不得携带“看起来可用”的伪造候选；`Unsupported`/`Unavailable` 必须 items 为空。
- `reason` 去重后限制在 512 字节内；不得包含文件路径、源码片段或 JSDoc 原文；脚本扫描失败用通用文案。

### 4.4 截断与 `isIncomplete`

- 事件模块：候选 > `MAX_COMPLETION_ITEMS`（256）时取前 256，`truncated=true`，status 至少 `Partial`。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs:185
- LSP `completion_list` 当前硬编码 `isIncomplete:false`，并且静默 `.take(MAX_COMPLETION_ITEMS)`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs:142
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs:150
- 本切片修（M7）：

```text
let mut items = Vec::new();
let mut dropped = false;
for item in result.items.iter().take(MAX_COMPLETION_ITEMS) {
    match item_value(document, item) {
        Some(value) => items.push(value),
        None => { dropped = true; log(...); } // invalid range / unrepresentable item
    }
}
let capped = result.items.len() > MAX_COMPLETION_ITEMS;
let truncated = result.truncated || capped || dropped;
json!({"isIncomplete": truncated, "items": items})
```

`item_value` 返回 `None`（例如 `source_range_value` 失败）不能静默丢：要么计入 `isIncomplete=true`，要么在错误路径返回空并记录；本设计选择“跳过该 item + `isIncomplete=true` + log”，保证 `truncated=false` 时不会悄悄少条目。补测试 `invalid_item_range_marks_is_incomplete`。

- 只修“本地截断”的 `isIncomplete`。native `isIncomplete` 目前只在 `query/completion.rs` 变成 gap，没有进入 C0 `CompletionResult`；要完整传递需要 A0 DTO 字段，另开切片。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/completion.rs:163
- `Partial` 但未截断时 `isIncomplete` 仍为 `false`：语义降级不是“再输入就会变完整”，不应触发客户端反复重查。

### 4.5 `completion_done` 的可见性

`completion_done` 只发布 `Supported` 和 `Partial`；`Unsupported`/`Unavailable`/`Disabled` 会被记录为日志并以空列表返回：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs:331

因此：

- 事件补全的 `Unsupported` 在客户端表现为空列表；这是现有 LSP 的可见性限制，不是本切片新增；
- 测试必须在 backend/adapter 层断言 `status=Unsupported`，不能只看 LSP 空列表；
- 不要在 `Unsupported` 时伪造 `Partial` 来“让状态可见”。

---

## 5. LSP 路由与并发（问题 5）

### 5.1 `DocumentRoute::Wxml` 与 kind 判定点

- `completion()` 的入口守卫是 `template_document(self, uri)`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs:195
  脚本/JSON/样式不会进入 WXML 补全。
- `detect` 返回 `Site::AttributeValue`；`expression::route` 是唯一把事件值变成 `CompletionKind::EventHandler` 的地方：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/expression.rs:15
- `Route::Backend(kind)` → `backend_available(&self.capabilities.completion)`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs:238
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/mod.rs:328
  native adapter 的 completion 是 `Partial`，所以 `backend_available` 为 true。
- `schedule_completion` 把 `kind` 放进 `JobKind::Completion(kind)`；worker 调 `Backend::completion`：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs:252
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/scheduler.rs:434

### 5.2 走 scheduler/backend，不做 host-local

Node 默认路径已经实现 `eventHandler`（`backends/typescript/src/completion.ts`），本切片**不改 Node 后端代码**；只补 native opt-in 适配器的同一 kind。共享的 LSP 路由分类器会收紧 `bindex` 等误判（`bindex` 不再走 eventHandler；`binding-mode` 本来就不走，继续 static），这是有意的正确性修复，必须跑 Node 回归。推荐保持 `Route::Backend(EventHandler)`，原因：

1. 协议线程只做 detect/schedule；脚本扫描 + Behavior 解析在 worker 上，避免阻塞 LSP 主循环。
2. 保留 stamp/epoch/取消语义；结果不会在文档 revision 变化后误发布。
3. 只改 native opt-in 适配器；Node backend 代码和它的 `Supported` capability 不变；共享路由只修正 `bindex` 误判，单独回归。
4. host-local 需要新的 `ProjectHost` 方法，并且会绕过 backend capability gating；还会让 Node/native 两条路径行为分裂，不是本切片该做的事。

### 5.3 adapter 的 EventHandler 快速分支

推荐 `TsgoBackendAdapter::completion` 结构：

```text
if !epoch_matches { Cancelled }
if kind == EventHandler {
    return event_completion::completion(self.views.as_ref(), &request, &self.event_cache);
}
if kind != ExpressionMember { Unsupported }
// 现有 expression 路径：prepare + with_session_environment
```

API 形状（建议）：

```rust
// backend_tsgo/event_completion.rs
pub(crate) fn completion(
    provider: &dyn ViewProvider,
    request: &CompletionRequest,
    cache: &Mutex<Option<EventCandidateCache>>,
) -> Result<CompletionResult, BackendError>;

pub(crate) struct EventCacheKey {
    project_root: String,        // 项目根
    script_path: String,         // target.script
    owner: RootKind,             // Page/Component
    source_complete: bool,
    files_revision_hash: u64,    // sorted (path, source_revision) over view.files
    // 明确不含 view_id / view_revision / 模板 revision / offset
}

pub(crate) struct EventCandidateCache {
    key: EventCacheKey,
    candidates: Vec<EventMethod>,
    methods_status: CapabilityStatus,
    source_complete: bool,
}
```

- 不调用 `self.prepare`：`prepare` 会构建 shadow，事件补全不需要。
- 不调用 `ensure_open`/`with_session_environment`：不启动/更新 native session。
- `event_completion::completion` 只依赖 `ViewProvider` + `RequestContext` + offset + 事件缓存。
- 视图准备复用 `input::prepare_view(provider, context.source())`（H4）；`host_error`/`input_error` 的映射集中在 `input.rs`，不在 event 模块复制私有函数。
- `views` 字段是 `Arc<dyn ViewProvider>`；测试用 `FakeViews` 就能覆盖。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:172
- `QuerySession::completions` 仍只服务 ExpressionMember；EventHandler 不会到达它。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:104

### 5.4 `isIncomplete` / truncated 列表 / 成员 guard 的取舍

| 问题 | 本切片是否修 | 原因 |
| --- | --- | --- |
| `isIncomplete:false` 硬编码 | **修** | 事件截断必须真实；改为 `result.truncated \|\| over_cap` |
| `completion_list` 静默 `.take(256)` | **修** | 即使 backend 没设 truncated，也不能悄悄丢条目 |
| static/expression 的本地截断 `isIncomplete` | **一并变真** | `completion_list` 是共享 helper；截断就是截断，不应按 kind 特判 |
| native `isIncomplete` 丢失 | **不修** | 需要 C0 `CompletionResult` 新字段（A0）；不在事件切片 |
| `Site::Mustache` 全部走 ExpressionMember | **不修** | `{{ Us }}`/bare-root 是表达式补全设计；与本切片无关 |
| `bindex` 误路由（`binding-mode` 本来 static） | **修** | 属于事件值判定，必须精确 |
| `Unsupported` 在 LSP 不可见 | **不修** | LSP 没有标准状态字段；保持空列表 + 日志 |

### 5.5 capability 文案

当前 native capability 文案明确说 event-handler completion 未实现：
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:658

实现后改成精确表述：

- expression-member completion 仍由 native `getCompletionsAtPosition` 提供，非空结果因无 per-entry docs/auto-import 仍是 `Partial`；
- event-handler completion 由 Rust script 方法表提供，不需要 native session，可以是 `Supported`；
- `this.`/方法体内补全、自定义组件事件定义、handler 签名帮助仍未覆盖。

不要改 `BackendCapabilities` 的结构；只改 reason 文本。

---

## 6. 测试（问题 6）

### 6.1 TS oracle 用例

`completion.test.ts` 的 9 个 case 中，与本切片直接相关：

| Case ID | 行 | 行为 | 本计划对应 |
| --- | ---: | --- | --- |
| `TS-COMPLETION-0175` | 175 | Page 事件补全列出 `onTap`/`onLoad`，kind method，replace 长度为前缀 bytes | Rust 候选 + replace 前缀 |
| `TS-COMPLETION-0193` | 193 | Component `methods` 只列 `save` | Component `methods` |
| `TS-COMPLETION-0207` | 207 | 不可用 layout → `unsupported` 空、不抛 | `Unavailable`/`Unsupported` 空 |
| `TS-COMPLETION-0220` | 220 | 300 方法 → 256 + `truncated:true` | 256 + `Partial` + truncated |

事件语义 oracle（`events.test.ts`）：

| Case ID | 行为 | 本计划对应 |
| --- | --- | --- |
| `TS-EVENTS-0071` | Page shorthand/function/arrow 解析 | 字面量判定（含前置修正） |
| `TS-EVENTS-0093` | Component shorthand/arrow | 同上 |
| `TS-EVENTS-0108` | 7 种绑定形式 | 触发矩阵 |
| `TS-EVENTS-0162` | 多 Page 注册 partial、用第一个 | 多注册 Partial |
| `TS-EVENTS-0176` | 非字面量注册 partial、无 missing diagnostic | 非字面量注册 |
| `TS-EVENTS-0188` | Behavior 方法 | Behavior 合并 |
| `TS-EVENTS-0236` | Behavior 图不完整 | Partial |
| `TS-EVENTS-0268` | 字面量 + 非字面量注册 | Partial |
| `TS-EVENTS-0280` | options/methods spread | Partial + 已知方法 |
| `TS-EVENTS-0301` | Component 无 methods 是真实缺失 | checked empty |
| `TS-EVENTS-0312` | 非字面量 methods | Partial 空 |
| `TS-EVENTS-0322` | reserved keys/`__wxml` | 排除 |
| `TS-EVENTS-0339` | 非标识符/非字面量函数不报诊断 | opaque/过滤 |
| `TS-EVENTS-0425` | alias Behavior handler | Behavior 路径 |
| `TS-EVENTS-0461` | 自有方法覆盖 Behavior | 合并顺序 |
| `TS-EVENTS-0498` | 未解析 Behavior alias | Partial |

说明：`TS-COMPLETION-0175/0193` 的 Node 行为包含 opaque/非字面量 label，而本计划不发布它们；因此这些 case 只能作为“weaker/语义证据”，不能把 ledger 状态直接提升为 exact `green`。`TS-EVENTS` 同理，保留 `planned` 或按现有规则登记 weaker 证据。

### 6.2 Rust 单元测试（新模块内）

在 `crates/wxml-lsp/src/backend_tsgo/event_completion.rs` 的 `#[cfg(test)]`：

- `page_literal_methods_are_suggested_and_data_is_not`
- `component_methods_only`
- `component_without_methods_is_checked_empty`
- `component_non_literal_methods_is_partial_empty`
- `behavior_methods_are_included_and_own_method_wins`
- `unresolved_behavior_is_partial_with_own_methods`
- `opaque_handler_is_not_suggested_and_marks_partial`
- `spread_and_computed_mark_partial_but_keep_known_methods`
- `multiple_registrations_use_selected_table_and_partial`
- `shadowed_or_non_literal_registration_is_partial_empty`
- `no_registration_is_unsupported_not_supported_empty`
- `script_scan_error_is_unsupported_and_empty`
- `truncation_caps_256_and_sets_partial_truncated`
- `method_budget_marks_partial`
- `non_identifier_method_names_are_not_suggested`
- `unicode_handler_names_are_suggested`
- `detail_and_documentation_are_bounded`
- `replace_range_is_the_handler_prefix_not_context_replace`
- `leading_and_trailing_whitespace_policy`
- `crlf_and_unicode_offsets_are_preserved`
- `cursor_outside_an_event_value_is_unsupported`
- `event_handler_completion_does_not_start_a_native_session`
- `source_complete_false_marks_partial`
- `target_none_is_unsupported`
- `owning_script_missing_is_unavailable`
- `page_root_falls_back_to_component_registration`
- `string_key_onTap_is_suggested`
- `only_opaque_is_partial_empty`
- `candidate_count_256_and_257_boundary`
- `non_catalog_direct_events_are_suggested`（`bindgetphonenumber`/`bindchooseavatar`/`bindplay`/`bindpause`）

### 6.3 Adapter 集成测试（`crates/wxml-lsp/src/backend_tsgo/tests.rs`）

- 用 `FakeViews` + `FakeFactory`，`FakeLog.starts == 0`、`completion_calls.is_empty()`、`hover_documentation_calls == 0`。
- `FailingViews` → `Err(BackendError::Unavailable)`，不是空 `Supported`。
- stale epoch → `Err(BackendError::Cancelled)`（EventHandler 也要走 epoch 检查）。
- 缓存（H2/M8）：第二次请求仍会调用 `provider.prepare`（host 有 prepared-view cache），但缓存键只含项目根/脚本/owner/`source_complete`/文件 revisions 哈希，**不含 `view_id`**；用 `EventCacheKey` 等值单元测试断言“不同模板 revision/offset 但同脚本 revisions → key 相等”，用“Behavior 文件 revision 变化 → 候选变化”验证失效。不要用全局 `Atomic` 计数；需要 hit 计数时用显式注入的 `Arc<dyn Fn>` 或 test-only counter。
- Page + Component 两种 owner。
- 事件值 `bind:myevent`/`bindtap`/`bindgetphonenumber`/`bindchooseavatar`/`bindplay`/`bindpause` 都到 EventHandler；`binding-mode`/`bindex`/裸 `bind`/`bind:` 不到。
- `target=None` → `Unsupported`；owning script 缺失 → `Unavailable`；`source_complete=false` → `Partial`。
- Page→Component fallback：Page view + 只有 `Component({methods...})` → Component 方法表；反向不 fallback。
- 字符串 key `'onTap'` 被建议；仅 opaque → `Partial` 空；256/257 边界。

### 6.4 LSP wire / e2e

- `completion/mod.rs` 单元：
  - `truncated_completion_sets_lsp_is_incomplete`
  - `over_cap_completion_truncates_and_sets_lsp_is_incomplete`
  - `semantic_partial_without_truncation_keeps_is_incomplete_false`
  - `event_handler_items_map_kind_detail_docs_sort_text_and_text_edit`
- `handlers/capability_gate_tests.rs` 或 `server.rs` 测试：
  - 构造 `CompletionResult`（EventHandler，`truncated=true`）走 `completion_done`，断言 `isIncomplete:true`；
  - `Unsupported` 结果仍返回空列表 + 不抛协议错误。
- `tests/lsp.rs`（mock backend）：
  - `bindtap` 仍然不 panic、返回空列表（mock 没有 completion）；
  - `bindex` 不再走 backend；`binding-mode` 仍 static（可通过 mock 调用计数或日志断言；若 harness 不暴露，至少断言空列表）。
- 现有 Node gate `wave5_acceptance::ac012_event_handlers_basic` 与 `real_node::real_node_expression_and_event_completion_smoke` 必须继续通过；它们只证明 **Node backend completion 未破坏**，**不证明 LSP 路由**。路由由 S0 的 `expression::route` 矩阵覆盖（含 `bindgetphonenumber`/`bindchooseavatar`/`bindplay`/`bindpause`/`bindex`/裸 `bind`/`bind:`）。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs:947
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:163

### 6.5 真实引擎 opt-in

事件补全不需要引擎；真正要证明的是“**即使 adapter 是真实引擎/真实资产构造的，EventHandler 也不启动 session**”。

推荐在 `crates/wxml-lsp/src/backend_tsgo/tests.rs` 增加一个和现有 `optional_real_engine_*` 同模式的测试：

- 无 `TSGO_BIN` 时显式打印 skip；`WXML_REQUIRE_REAL_ENGINE=1` 时硬失败；
- 用 `NativeSessionFactory` + 真实 `TSGO_BIN` 构造 adapter；
- 发 EventHandler 请求，断言返回候选；
- 不新增 `#[ignore]` 测试：`crates/wxml-lsp/tests` 下任何 `#[ignore]` 都会被 `native_gates.rs` 计入冻结的 23 个 gate，新增会破坏 gate 清单，不应在本切片做。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:29

### 6.6 负向测试清单

- 非事件属性：`wx:if`、`class`、`data-onTap`、`binding-mode`、`bindex`、裸 `bind`。
- 动态：`bindtap="{{ handler }}"`、`bindtap="pre{{x}}"`、`bindtap="onTap()"`、`bindtap="item.onTap"`。
- spread：`Page({ ...mixin, onTap() {} })` → 只有 `onTap`，`Partial`，不出现 spread key。
- opaque：`Page({ onTap: handlerRef })` → 不出现 `onTap`，`Partial`。
- 跨 namespace：`import { save } from './x'; Page({ save })` → 不出现 `save`。
- 非字面量 Component methods：`Component({ methods: obj })` → 空 + `Partial`，不是 `Supported` 空。
- 无注册：`Page(options)`（非字面量）→ `Partial` 空；完全没有注册 → `Unsupported` 空。
- `Unsupported` 不发布：LSP 空列表、无协议错误；adapter 层 status 断言。
- 截断：300 方法 → 256、`truncated=true`、`Partial`、LSP `isIncomplete=true`。
- 空格/引号/EOF：§2.4 矩阵，包括 `bindtap=|"onTap"` 必须以 `value_bounds` 的 `quote=None` 拒绝。
- H1 非目录直接事件：`bindgetphonenumber`/`bindchooseavatar`/`bindplay`/`bindpause` 必须仍到 EventHandler；`bindex`/裸 `bind`/`bind:` 必须不到。

### 6.7 README / ledger 更新（实现后，不在本文修改）

- `README.md` 补全章节（M2）：
  - 约 385 行：说明 native 事件补全走 Rust script 方法表，不启动 native session；表达式补全仍走 native completion 且仍是 `Partial`。
  - 约 388 行：`isIncomplete:false` 的说法改为“本地截断时 `isIncomplete:true`；native `isIncomplete` 的完整 DTO 传递另开切片”。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/README.md:385
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/README.md:388
- `docs/reference/wxml-language-feature-baseline.md`（M2）：
  - 约 220 行：更新事件值补全路由为“宽松直接形式（`bind`/`catch` + 非空全小写后缀）+ 任意冒号形式；拒绝裸前缀/空事件/`bindex`”；`EVENT_NAMES` 只用于属性名补全。
  - 约 227 行：同步 `isIncomplete` 的截断语义。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/wxml-language-feature-baseline.md:220
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/wxml-language-feature-baseline.md:227
- `docs/plans/2026-11-17-tsgo-test-ledger.md`：
  - `TS-COMPLETION-0175/0193/0207` 只能登记 weaker/partial 证据，不能直接标 exact `green`；
  - `TS-COMPLETION-0220` 已是 `green`，可补一条 Rust event truncation 证据，但不改状态；
  - `TS-EVENTS-*` 保持 `planned` 或按现有 weaker 规则登记；
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/2026-11-17-tsgo-test-ledger.md:318
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/2026-11-17-tsgo-test-ledger.md:375
- `docs/evidence/README.md`：实现/证据落地后加索引行；本文阶段不改。
- 合同/ADR：本切片不需要改合同 §9.7，因为事件补全不是 native completion details；`F1-21`/`B-07` 仍按原状态。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:595
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:776
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:800

### 6.8 建议的验证命令（实现后）

```bash
cargo fmt --all -- --check
cargo clippy -p wxml-lsp -p wxml-backend-tsgo --all-targets -- -D warnings

cargo test -p wxml-backend-tsgo --test script_events
cargo test -p wxml-backend-tsgo --test script_event_completion   # 若新建
cargo test -p wxml-lsp backend_tsgo::event_completion
cargo test -p wxml-lsp backend_tsgo::tests
cargo test -p wxml-lsp completion::
cargo test -p wxml-lsp --test lsp
cargo test -p wxml-lsp --test wave5_acceptance -- --ignored      # Node 回归，按环境
cargo test -p wxml-lsp --test real_node -- --ignored             # Node 回归，按环境
```

不要在本文阶段运行全量 workspace/Node 矩阵；实现阶段按切片跑定向测试，再由集成 owner 跑全量。

---

## 7. 文件、依赖与并行（问题 7）

### 7.1 文件清单

| 文件 | 变更 | Owner | 冲突面 |
| --- | --- | --- | --- |
| `crates/wxml-lsp/src/backend_tsgo/event_completion.rs` | **新增**：纯 builder、trigger/value 解析、缓存 entry、单元测试 | native adapter | 无现有文件冲突；不要改 `completion.rs` 的 expression 逻辑 |
| `crates/wxml-lsp/src/backend_tsgo/input.rs` | **H4**：暴露 `prepare_view` / `input_error`（或在 event 模块本地复制映射；设计选择前者） | native adapter | **与 trusted type-assets 未提交改动冲突**；S2 必须排在 handoff 之后 |
| `crates/wxml-lsp/src/backend_tsgo/mod.rs` | 声明新模块；EventHandler 快速分支；capability 文案 | native adapter / integrator | **与 trusted type-assets 未提交改动冲突**；与 auto-import 的 completion 分支串行 |
| `crates/wxml-lsp/src/completion/expression.rs` | 用精确事件分类器替换 `is_event_prefix` | LSP completion | 与属性补全/路由工作串行 |
| `crates/wxml-lsp/src/completion/catalog.rs` | 新增 `is_event_value_attribute` + 测试；保留旧函数职责 | LSP completion | 与静态目录/属性补全串行 |
| `crates/wxml-lsp/src/completion/mod.rs` | `isIncomplete`/over-cap 截断；wire 测试 | LSP completion | 与 A0 `CompletionResult.incomplete`、auto-import wire 改动串行 |
| `crates/wxml-lsp/src/completion/static_items.rs` | 值补全抑制改用精确分类器（可选） | LSP completion | 与静态补全测试 |
| `crates/wxml-backend-tsgo/src/script/events.rs` | 复用 `shadow::expression::is_identifier_name`（替换私有 `is_identifier_text`）；修正箭头字面量判定 | script semantics | **与其它 script 语义切片串行** |
| `crates/wxml-backend-tsgo/src/script/registration.rs` | 顶层注册优先（H3，S1 必做） | script semantics | 与注册/Behavior 工作串行 |
| `crates/wxml-backend-tsgo/tests/script_events.rs` 或新增 `script_event_completion.rs` | 分类器/箭头/opaque/多注册测试 | script semantics | 新增文件可降低冲突 |
| `crates/wxml-lsp/src/backend_tsgo/tests.rs` | fake adapter：无 session、失败、缓存、Page/Component | native adapter/QA | 该文件有未提交 trusted-assets 改动，需在其后追加 |
| `crates/wxml-lsp/src/handlers/capability_gate_tests.rs` / `src/completion/mod.rs` tests | LSP wire `isIncomplete`/textEdit | LSP completion | 与 completion wire 工作串行 |
| `crates/wxml-lsp/tests/lsp.rs` | mock 回归（可选） | QA | 低 |
| `README.md`、`docs/reference/wxml-language-feature-baseline.md`、`docs/plans/2026-11-17-tsgo-test-ledger.md`、`docs/evidence/README.md` | 实现后文档/台账 | docs | 最后做；不要先改状态 |

**不改**：

- `crates/wxml-types/**`（无 DTO 变更）
- `Cargo.toml` / `Cargo.lock`（现有依赖足够）
- `backends/typescript/**`（Node 默认路径不动）
- `docs/reference/tsgo-native-contract.md`、ADR、交付合同
- 默认后端选择、`server.rs` 的 Node 构造、`wxml-types` 的 `BackendCapabilities` 结构

### 7.2 依赖关系

- `wxml-lsp` 已经依赖 `wxml-backend-tsgo`，可直接调用 `script::analyze_script` / `script::events`。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/Cargo.toml:13
- 不需要新增 crate/feature。
- 事件模块使用 `wxml_backend_tsgo::input::{AnalysisInput, SourceFile, ...}`、`script::{ScriptRequest, analyze_script}`；这些都已公开。
- `ViewProvider` 是 `pub(crate)`，事件模块与 adapter 同 crate，可直接使用；H4 的 `input::{prepare_view,input_error}` 也由 event_completion 复用，保证 `HostError`/`InputError` 只有一处路径无关映射。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:39

### 7.3 实施顺序

| 阶段 | 内容 | 依赖 | 可并行 |
| --- | --- | --- | --- |
| S0 | 精确事件分类器 + 路由测试（`catalog.rs`/`expression.rs`） | 无 | 与 S1 并行 |
| S1 | script 前置修正：复用 `is_identifier_name`、箭头判定、**顶层注册优先（H3 必做）**、测试 | 无 | 与 S0 并行 |
| S2 | `event_completion.rs`：view→input→analysis→candidates→item/replace；单元测试；H4 `input::prepare_view` | S0/S1 + trusted handoff（input.rs） | 与 S3 并行，但 input.rs 单写者 |
| S3 | LSP `isIncomplete`/over-cap 截断 + wire 测试 | 无 | 与 S2 并行（不同文件） |
| S4 | `backend_tsgo/mod.rs` EventHandler 分支 + capability；缓存接线 | S2；trusted-assets `mod.rs` 落地后 | 与 S3 可并行，但 `mod.rs` 单写者 |
| S5 | adapter fake 测试（无 session）、失败/缓存/epoch；Node 回归 | S4/S3 | 串行 |
| S6 | README/baseline/ledger/evidence 索引；真实 opt-in 测试 | S5 | 最后 |

### 7.4 不可并行点

1. `crates/wxml-lsp/src/backend_tsgo/mod.rs`：trusted type-assets 未提交改动、EventHandler 分支、capability 文案必须一个 owner 顺序落地。写作期间并行会话还新增了 `backend_tsgo/environment*` 与 `tests/native_lsp_document_symbols*`；这些同样不要与 `mod.rs`/`tests.rs` 的改动并行。
2. `crates/wxml-lsp/src/completion/mod.rs`：本切片 `isIncomplete`/截断 与 A0 `CompletionResult.incomplete`/auto-import wire 改动不能并行。
3. `crates/wxml-backend-tsgo/src/script/{events,registration}.rs`：箭头/opaque/顶层注册修正与其它 script 语义切片不能并行。
4. `crates/wxml-lsp/src/completion/{catalog,expression}.rs`：精确事件分类器与属性名补全/静态目录不能并行。
5. `crates/wxml-lsp/src/backend_tsgo/tests.rs`：该文件已有未提交改动；追加测试前先确认落地。
6. `crates/wxml-lsp/src/backend_tsgo/input.rs`：H4 helper 与 trusted type-assets handoff 同一文件，必须一个 owner 顺序落地。
7. `docs/plans/2026-11-17-tsgo-test-ledger.md`：只能一个 owner 在证据确定后更新；不得提前标 green。

### 7.5 回滚

- 事件补全是加法：删除 `event_completion.rs`、回退 `mod.rs` 分支/capability 文本即可；不涉及数据迁移、DTO/wire 变更、session/snapshot 生命周期。
- `isIncomplete` 修复是独立小提交，可单独回退；回退后事件截断会重新变成 `isIncomplete:false`，属于回归，不建议保留。
- 精确分类器是独立提交，可单独回退；回退后 `bindex` 会重新误路由到 backend（`binding-mode` 本来 static），但 native 仍会返回空/防御性 Unsupported。
- 缓存可禁用（`event_cache` 不填）而不影响正确性，只影响延迟。

---

## 8. 范围诚实（问题 8）

### 8.1 本切片明确不做

- **auto-import**：不接 `prepareAutoImports`/`includeSymbol`；不发布未导入条目；C0 没有 `additionalTextEdits`/`data`。
- **per-entry native documentation/details**：不调用 `getCompletionEntryDetails`（pinned API 不存在）；不把脚本 JSDoc 冒充 native completion details。
- **`this.`/方法体内补全**：不做 checker 成员枚举；不做 `this.data`/`this.properties` 补全。
- **自定义组件事件定义/事件名索引**：不扫描组件 `triggerEvent`、不生成自定义事件 catalog；`bind:myevent` 以及宽松直接形式（如 `bindgetphonenumber`）的值补全仍是“当前 owner 方法”，不是组件事件签名/参数。
- **handler 签名帮助**：`detail` 是一行摘要，不是 `signatureHelp`。
- **跨文件方法索引**：只解析当前注册的静态 Behavior 图；不做 project-wide 方法搜索，不用 `tsconfig paths` 猜别名。
- **Node 删除/默认后端切换**：默认仍是 Node；native 仍是 opt-in。
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:184
  /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:45
- **F1/P6/G5 关闭**：不声称。

### 8.2 不得声称的结论

- 不得说 “F1 event-handler completion 完成” 等于 “F1 完成”；F1-21/B-07、hover display parts、rename 等仍开放。
- 不得说 “G-P5-E2E-21 已迁移”；Node gate 仍是原 gate；本切片新增的是 Rust adapter/单元证据。
- 不得说 “P6 默认后端已切换”或“Node 已移除”。
- 不得说 “completion details/auto-import 已支持”；本文的 `documentation`/`detail` 是脚本声明事实，不是 native 条目详情。
- 不得说 “Behavior 跨别名/跨 tsconfig path 已完整支持”；没有 `ModuleResolver` 时只能 `Partial`。

### 8.3 残留风险

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| `DependencyIndex` 不跟踪 Behavior import | 缓存可能过期 | 缓存键用全部 `PreparedView.files` revisions 哈希（§3.5） |
| 每次 cache miss 复制全部文件文本 | 一次补全延迟 | 与现有 native 查询同量级；缓存；未来可 `Arc<str>`/借用快照 |
| 箭头函数启发式仍有边缘误判 | 可能 opaque 过多或漏判 | 前置修正 + 保守 opaque；不发布无法证明的名字 |
| 顶层注册选择 | 可能用函数内注册表 | **S1/H3 必做**：top_level 优先 + 回归测试；完成层对未修复情况强制 Partial |
| 自定义事件定义/参数不解析 | 只补 handler 名，不补事件签名 | 宽松直接形式 + 冒号形式均接受；组件事件定义仍不做 |
| Node 与本计划候选集不同 | parity 断言不能 exact | 证据登记 weaker；README 写清差异 |
| `isIncomplete` 只修本地截断 | native `isIncomplete` 仍丢 | A0 DTO 另开切片；不得声称完整 |

### 8.4 验收清单

- [ ] 事件值只从 `ScriptAnalysis.methods` 取候选，不启动 native session。
- [ ] `binding-mode`/`bindex`/`wx:if`/`class`/`data-*` 不触发 handler 补全。
- [ ] `bindtap`/`catchtap`/`bind:<event>`/`catch:<event>`/`capture-bind:<event>`/`capture-catch:<event>`/`mut-bind:<event>` 均触发。
- [ ] `bindgetphonenumber`/`bindchooseavatar`/`bindplay`/`bindpause` 等非 `EVENT_NAMES` 直接事件仍触发（H1）。
- [ ] `bindtap="{{x}}"` 走 ExpressionMember，不走 EventHandler。
- [ ] `replace` 只覆盖值内 handler 前缀，不含引号/前导空格/尾字符。
- [ ] `Page.data`/`properties`/`methods`/`observers` 等不出现；Component 只取 `methods`。
- [ ] spread/computed/opaque/非字面量/多注册 → `Partial`，不发布伪造候选。
- [ ] 无注册 → `Unsupported` 空，不是 `Supported` 空。
- [ ] >256 → 256 + `truncated` + `Partial`；LSP `isIncomplete:true`。
- [ ] `documentation`/`detail` 来自脚本声明，未调用 native completion details/auto-import。
- [ ] 默认 Node 路径回归通过；没有改 `backends/typescript`。
- [ ] 没有声称 F1/P6/G5 关闭。

---

## 9. Review 修订摘要

| Review 项 | 修订落点 |
| --- | --- |
| H1 直接事件白名单回归（方案 2） | §0 第 5 条、§1.2、§1.5、§2.2、§2.4、§6.4/§6.6、§8.4 |
| H2 缓存键不含 `view_id` | §3.5、§5.3 `EventCacheKey`、§6.3 |
| H3 顶层注册选择升为 S1 必做 | §1.4 第 4 条、§7.1、§7.3、§8.3 |
| H4 `backend_tsgo/input.rs` 冲突面 | §3.2、§5.3、§7.1、§7.3、§7.4 |
| M1 `bind:`/`catch:` 表述 | §8.4 |
| M2 baseline 227 / README 388 | §6.7 |
| M3 删除静默丢文件/不可达 fallback | §3.2 |
| M4 Node gate 只证明 backend，不证明路由 | §6.4 |
| M5 `value_bounds` 判定引号 | §2.3、§2.4、§6.6 |
| M6 缺失测试矩阵 | §6.2、§6.3、§6.6 |
| M7 `item_value` 失败计入 `isIncomplete` | §4.4 |
| M8 缓存计数改为显式 seam/行为测试 | §3.5、§6.3 |
| L1 引用改符号名 | 顶部引用约定、附录 A |
| L2 `bindtap="{{` 说明 | §2.4 |
| L3 `parse_event_attribute` 只是 helper | §2.2 |
| L4 复用 `is_identifier_name` | §1.2、§1.4、§2.3、§6.2、§7.1 |
| L5 跨层依赖说明 | §2.3 |
| L6 截断顺序 | §4.3 |
| L7 空集判定不用 `methods.status` | §4.3 |
| L8 `on|Tap` 已知行为 | §2.3 |

**仍 BLOCKED（本切片不解）**：auto-import、per-entry native details/docs、`this.`/方法体内补全、自定义组件事件定义/参数、handler 签名帮助、跨文件方法索引、native `isIncomplete` 的完整 DTO 传递（A0）、默认后端切换、F1/P6/G5 关闭。

---

## 附录 A：关键代码锚点（L1：符号名为准）

> 下列行号是写作时辅助；审查/实现以路径 + 符号名为准。并行 trusted-assets 改动可能使行号漂移。

| 事实 | 绝对路径 + 符号（写作时行号仅参考） |
| --- | --- |
| `CompletionKind::EventHandler` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs — `enum CompletionKind`（:193） |
| `CompletionResult` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs — `struct CompletionResult`（:290） |
| `CompletionItem` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs — `struct CompletionItem`（:263） |
| `MAX_COMPLETION_ITEMS` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs — `const MAX_COMPLETION_ITEMS`（:185） |
| `Site::AttributeValue` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/context.rs — `enum Site`（:24） |
| 带引号值 replace/prefix | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/context.rs — `fn scan_tag` 引号分支（:446） |
| 未加引号值整 token replace | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/context.rs — `fn scan_tag` 未加引号分支（:489） |
| `Route` 分类 | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/expression.rs — `fn route`（:15） |
| `is_event_prefix` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/catalog.rs — `fn is_event_prefix`（:1309） |
| `is_event_attribute` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/catalog.rs — `fn is_event_attribute`（:1291） |
| `completion_list` / `isIncomplete` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs — `fn completion_list`（:142） |
| `completion_done` 状态过滤 | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/mod.rs — `fn completion_done`（:331） |
| `value_bounds` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/styles.rs — `fn value_bounds`（:273） |
| `attribute_value_end` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/styles.rs — `fn attribute_value_end`（:421） |
| native adapter completion | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs — `impl Backend for TsgoBackendAdapter::completion`（:552） |
| native capability 文案 | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs — `fn capabilities`（:658） |
| native expression completion | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/completion.rs — `fn completion`（:29） |
| `QuerySession::completions` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs — `trait QuerySession::completions`（:104） |
| `PreparedView` / `files` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs — `struct PreparedView`（:345） |
| `build_prepared` 文件来源 | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs — `fn build_prepared`（:2877） |
| `build_request_plan` target shadow | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs — `fn build_request_plan`（:304） |
| `host_error` / `input_error` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs — `fn host_error`（:611） / `fn input_error`（:615） |
| `AnalysisInput::new` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs — `impl AnalysisInput::new`（:1279） |
| `MAX_INPUT_FILES/BYTES` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs — `MAX_INPUT_FILES` / `MAX_INPUT_BYTES`（:50） |
| `ScriptAnalysis` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/mod.rs — `struct ScriptAnalysis`（:140） |
| `MethodTable` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs — `struct MethodTable`（:128） |
| `EventMethod` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs — `struct EventMethod`（:113） |
| `build_table` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs — `fn build_table`（:223） |
| `collect_page` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs — `fn collect_page`（:349） |
| `collect_component` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs — `fn collect_component`（:382） |
| `add_member` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs — `fn add_member`（:434） |
| `is_identifier_name` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/shadow/expression.rs — `fn is_identifier_name`（:158） |
| `is_identifier_text`（待收敛） | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs — `fn is_identifier_text`（:614） |
| `parse_event_attribute` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/events.rs — `fn parse_event_attribute`（:197） |
| `registration::select` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/registration.rs — `fn select`（:489） |
| Behavior methods | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/script/behavior.rs — `struct BehaviorResolution`（:112） |
| DependencyIndex 不含 Behavior import | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/dependencies.rs — `impl DependencyIndex`（:29） |
| Node `collectEventHandlers` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/completion.ts — `function collectEventHandlers`（:115） |
| Node `eventMembers` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/completion.ts — `function eventMembers`（:150） |
| Node `eventReplace` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/completion.ts — `function eventReplace`（:168） |
| Node `PAGE_RESERVED` | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/completion.ts — `const PAGE_RESERVED`（:40） |
| TS completion cases | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/2026-11-17-tsgo-test-ledger.md — `completion.test.ts` 表（:318） |
| TS events cases | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/2026-11-17-tsgo-test-ledger.md — `events.test.ts` 表（:375） |
| 合同 §9.7 | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md — `### 9.7`（:595） |
| F1-21 / B-07 | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md — `F1-21`（:776） / `B-07`（:800） |
| 事件值补全基线 | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/wxml-language-feature-baseline.md — 事件值补全条（:220） |
| `isIncomplete` 基线 | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/wxml-language-feature-baseline.md — 能力声明（:227） |
| README 补全/能力声明 | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/README.md — 补全（:385） / 能力声明（:388） |

## 附录 B：审阅检查表

- [ ] 本文没有修改任何生产代码/测试/Cargo/合同/ADR/默认后端/Node。
- [ ] 所有代码引用是绝对路径 + 行号，且路径前有空白符。
- [ ] “结论先行”明确 event-handler completion 可行，但 F1/P6/G5 未关闭。
- [ ] 精确事件分类器与 `is_event_prefix`/`parse_event_attribute` 的职责写清。
- [ ] 触发矩阵覆盖引号/未加引号/空格/已有值/mustache/非事件属性/EOF。
- [ ] 状态规则覆盖 Supported/Partial/Unsupported/Unavailable 和空列表。
- [ ] 明确不新增 DTO、不启动 native session、不做 auto-import/per-entry docs。
- [ ] 文件/并行/回滚/验收清单可直接执行。
