# Wave 5 并行交付计划：补全、模板类型、事件链路与 Zed 实时同步

> **状态：历史归档（Superseded）**。P7 删除已完成；本文仅记录当时计划/证据，不再代表当前仓库状态。最终状态见 docs/plans/2026-11-17-rust-tsgo-migration.md §12.16。

- 状态：待确认（Draft）
- 日期：2026-11-16
- 基线：`master` @ `b57f4c8`（fix: answer hover across WXML expressions）
- 适用范围：功能 1–4，四个独立 worktree 并行实现，最后统一集成
- 评审方式：每个 worktree 完成后用 `rust-reviewer` 审核并自动修复，最多 2 轮
- 执行约束：主工作区中 ` tests/fixtures/page-basic/pages/index/index.wxml` 的未提交修改属于用户手工测试，不纳入任何提交；不在项目目录生成临时文件

## 0. 执行摘要

本计划把上一轮列出的四项优先功能拆成四个 worktree：

| 编号 | 功能 | worktree | 分支 |
| --- | --- | --- | --- |
| 1 | 补全体系 F-01–F-07 | `wave5-completion` | `feat/wave5-completion` |
| 2 | Wave 4 模板类型闭环 AC-005/006/008/012 | `wave5-templates` | `feat/wave5-template-types` |
| 3 | 事件处理函数诊断、定义、补全 | `wave5-events` | `feat/wave5-events` |
| 4 | Zed 多语言注册、未保存 JS/TS 同步、didSave、文件监听 | `wave5-zed-sync` | `feat/wave5-zed-sync` |

核心策略：

1. 先在 `integration/wave5` 上完成一次**接口冻结提交**（Phase 0），只落 DTO、协议 schema、Backend 默认方法和测试脚手架，不实现业务逻辑。
2. 四个 worktree 全部从该冻结提交分出，减少并行期间的接口漂移和合并冲突。
3. 每个 worktree 用 `/orchestrate` 启动 planner，并由其调度至少 2 个并行 subagent；实现与测试同步推进。
4. 每个 worktree 必须通过 Rust、Node、LSP 集成、fixture 和（适用时）Zed 全部门禁，再进入两轮 `rust-reviewer` 审核。
5. 合并顺序：模板类型 → 补全 → 事件 → Zed 同步；每合入一个 worktree 就重跑全量门禁。
6. 最终只在 `integration/wave5` 验收；是否合入 `master`、是否走 Codeup PR，等用户确认。

预计并行墙钟时间 7–10 个工作日；Phase 0 约 1–2 天，集成与返工 2–3 天。

---

## 1. 目标与边界

### 1.1 必须完成

1. **补全体系 F-01–F-07**
   - 标签名补全（基础组件 + 自定义组件）
   - 属性名补全
   - 枚举属性值补全
   - `wx:` 指令与事件属性补全
   - 上下文筛选（如 `picker mode` 只提示合法值）
   - `usingComponents` 自定义组件补全
   - Mustache 表达式成员补全、事件处理函数补全
2. **模板类型闭环 AC-005/006/008/012**
   - `wx:for` 元素类型、索引类型、嵌套作用域、自定义 item/index 名
   - `wx:if / wx:elif / wx:else` 条件窄化
   - 自定义组件属性绑定类型兼容性诊断
   - 多视图隔离与 `tsconfig` `extends/baseUrl/paths` 别名端到端验收
3. **事件处理函数链路**
   - 事件属性解析结果进入 PreparedView
   - 未知处理函数诊断
   - 处理函数 definition
   - 事件属性值补全
4. **Zed 实时同步**
   - Zed 扩展注册 WXML、JavaScript、TypeScript、TSX、JSON
   - 未保存 JS/TS/JSON 内容通过 overlay 影响 WXML 分析
   - `textDocument/didSave`、`workspace/didChangeWatchedFiles` 处理
   - 非 WXML 文档请求隔离，避免干扰其他语言服务器

### 1.2 非目标

以下不在本轮范围，避免工作树继续膨胀：

- F-08–F-18 中除上述必要部分外的能力：组件/标签 hover、class/WXSS 索引、资源路径链接、snippets、格式化、语义高亮、inlay hints、引用、重命名、成对重命名、code action
- 多根工作区 / 多 app 根（F-18）；本轮只做单项目内的多视图隔离
- 原生 TypeScript 后端替换、去 Node 决策
- Marketplace 发布、服务器自动下载安装、版本升级流水线
- 反向编辑（WXML → TS 自动修改）
- 组件 `required` 属性、复杂属性转换、插件组件属性校验

### 1.3 兼容性决策

- C1 Node 协议升级到 `v2`，Rust 与 Node 后端锁步更新；不再兼容 `v1` 新字段语义。
- C0 `Backend` trait 只做**增量默认方法**：旧后端不实现 `completion` 时仍可工作。
- LSP 新能力只在后端报告支持时声明；静态补全不依赖 Node，Node 不可用时仍可提供标签/属性/枚举补全。
- 现有 `textDocument/formatting` 等未支持方法继续保持 `-32601`。

---

## 2. 当前基线

### 2.1 已具备

- `wxml-text`：UTF-8 字节 ↔ UTF-16 位置、行索引、SourceRange。
- `wxml-syntax`：容错 lexer/parser、Mustache Pratt 表达式、WXS rawtext、资源上限、源码映射。
- `wxml-project`：app.json 页面/子包/组件发现、`usingComponents`、tsconfig `extends/baseUrl/paths`、overlay。
- `wxml-types`：C0/C1 契约、模板作用域分析、Backend trait、mock。
- `wxml-lsp`：JSON-RPC、ProjectHost、Node 后端、hover/definition/push diagnostics、版本屏障、缓存、超时、取消。
- Node 后端：Page data、Component data+properties、跨文件类型、JSDoc、部分 definition。
- Zed 扩展：仅注册 WXML，从绝对路径 `WXML_LSP_BIN` 启动。

### 2.2 关键缺口

-  crates/wxml-lsp/src/handlers/lifecycle.rs:29 只声明 hover/definition；请求分派只有 `textDocument/hover`、`textDocument/definition`。
-  crates/wxml-types/src/backend.rs 的 `Backend` 没有 completion 方法，`BackendCapabilities.completion` 默认 Unsupported。
-  crates/wxml-types/src/template.rs 只把 `wx:for` 的 locals 名称传给后端；`wx:if/elif/else` 不改变作用域。
-  crates/wxml-lsp/src/backend_node/response.rs 的 `LOCALS_REASON` 明确说明 loop item/index 还是 `any`。
- `EventHandler` 已由模板分析产出，但除 re-export 外没有消费者。
-  crates/wxml-lsp/src/project_host.rs 已有 Script/Json overlay 能力，但 editors/zed/extension.toml 只注册 WXML，Zed 不会把 JS/TS/JSON 文档转给服务端。
- AC-005/006/008/012 仍未完成；F-01–F-07 全部未实现。

---

## 3. 分支与 worktree 布局

### 3.1 目录

所有 worktree 放在仓库外，避免项目目录出现临时内容：

```text
/home/zxg/code/rust/wxml-lsp-worktrees/
├── wave5-completion/
├── wave5-templates/
├── wave5-events/
└── wave5-zed-sync/
```

主仓库继续保留用户未提交的 fixture 修改，不切换主工作区分支，直到最终合并需要时再处理。

### 3.2 创建顺序

```bash
cd /home/zxg/code/rust/wxml-lsp
git switch -c integration/wave5
# Phase 0 接口冻结提交
git worktree add -b feat/wave5-completion \
  /home/zxg/code/rust/wxml-lsp-worktrees/wave5-completion integration/wave5
git worktree add -b feat/wave5-template-types \
  /home/zxg/code/rust/wxml-lsp-worktrees/wave5-templates integration/wave5
git worktree add -b feat/wave5-events \
  /home/zxg/code/rust/wxml-lsp-worktrees/wave5-events integration/wave5
git worktree add -b feat/wave5-zed-sync \
  /home/zxg/code/rust/wxml-lsp-worktrees/wave5-zed-sync integration/wave5
```

### 3.3 分支职责

| 分支 | 职责 | 不允许改动 |
| --- | --- | --- |
| `integration/wave5` | Phase 0 契约、合并、冲突解决、全量门禁 | 不直接实现四项业务功能 |
| `feat/wave5-template-types` | locals 类型、条件窄化、组件属性绑定、多视图/别名 | 不改补全静态目录、不改 Zed 注册 |
| `feat/wave5-completion` | 补全上下文引擎、静态目录、LSP 补全、Node 成员补全 | 不改 locals/guards 语义、不改事件诊断 |
| `feat/wave5-events` | 事件解析、诊断、definition、事件处理函数补全 | 不改组件属性类型诊断、不改 Zed 注册 |
| `feat/wave5-zed-sync` | Zed 多语言、overlay、didSave、watched files | 不改补全目录、不改模板类型算法 |

---

## 4. Phase 0：接口冻结

Phase 0 在 `integration/wave5` 上完成，必须单独提交并通过全量基线测试。目标是让四个 worktree 只实现各自逻辑，不反复改公共结构。

### 4.1 C0 DTO（wxml-types）

在  crates/wxml-types/src/backend.rs 增加：

```rust
pub enum CompletionKind {
    Tag,
    AttributeName,
    AttributeValue,
    ExpressionMember,
    EventHandler,
}

pub struct CompletionRequest {
    pub context: RequestContext,
    pub offset: u32,
    pub kind: CompletionKind,
}

pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionItemKind,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub insert_text: String,
    pub insert_text_format: InsertTextFormat,
    pub replace: SourceRange,
    pub sort_text: Option<String>,
    pub filter_text: Option<String>,
}

pub struct CompletionResult {
    pub items: Vec<CompletionItem>,
    pub status: CapabilityStatus,
    pub truncated: bool,
}
```

在 `Backend` trait 增加默认方法：

```rust
fn completion(&self, _request: CompletionRequest) -> Result<CompletionResult, BackendError> {
    Ok(CompletionResult {
        items: Vec::new(),
        status: CapabilityStatus::Unsupported {
            reason: "completion is not implemented".to_owned(),
        },
        truncated: false,
    })
}
```

约束：

- `MAX_COMPLETION_ITEMS = 256`，label/detail/documentation/insertText 都有长度上限。
- `replace` 必须是原文中的合法字符边界范围。
- 静态补全不经过 Backend，不要求 Node 在场。
- `BackendCapabilities.completion` 由 Node 后端在实现后报告 `Supported`。

### 4.2 模板契约（wxml-types/check.rs）

将“locals 名称列表”升级为可解析的绑定：

```rust
pub enum LocalRole {
    Item,   // wx:for 元素
    Index,  // 数组索引 / 对象键
}

pub struct LocalBinding {
    pub name: String,
    pub role: LocalRole,
    pub source_expr: String,
    pub source_range: SourceRange,
}
```

`CheckFragment.locals: Vec<String>` 改为 `bindings: Vec<LocalBinding>`；Phase 0 同步更新所有构造点和 Node 协议，先继续按 `any` 处理，保持现有行为。

条件链用结构化数据表示，避免把 if/elif/else 拼成不可追踪的字符串：

```rust
pub enum GuardBranch {
    If,
    Elif,
    Else,
}

pub struct GuardCondition {
    pub branch: GuardBranch,
    pub expr_text: String,
    pub range: SourceRange,
}

pub struct GuardChain {
    /// 同一 wx:if/elif/else 链上的条件，按源码顺序。
    pub conditions: Vec<GuardCondition>,
    /// 当前片段所在分支下标；等于 conditions.len() 表示 else。
    pub selected: usize,
}
```

`CheckFragment` 增加 `guards: Vec<GuardChain>`，默认空数组。

组件使用增加：

```rust
pub struct ComponentAttribute {
    pub name: String,
    pub value_kind: AttributeValueKind, // Literal / Mustache / Mixed / Boolean
    pub expr_text: Option<String>,
    pub expr_range: Option<SourceRange>,
    pub attribute_range: SourceRange,
}

pub struct ComponentUsage {
    pub tag: String,
    pub component_file: Option<FileId>,
    pub attributes: Vec<ComponentAttribute>,
}
```

`EventHandler` 补充两个精确范围：

```rust
pub struct EventHandler {
    pub attribute: String,
    pub binding: EventBindingKind,
    pub event: String,
    pub handler: String,
    pub attribute_range: SourceRange,
    pub handler_range: SourceRange,
}
```

Phase 0 只补字段和构造点，不实现事件语义。

### 4.3 C1 协议 v2

`PROTOCOL_VERSION = 2`，Rust 与 Node 同步更新。`analyze` 请求增量字段：

```json
{
  "fragments": [
    {
      "fragmentId": "f0",
      "exprText": "item.name",
      "bindings": [
        {
          "name": "item",
          "role": "item",
          "sourceExpr": "groups",
          "sourceRange": { "start": 100, "end": 106 }
        }
      ],
      "guards": [
        {
          "conditions": [
            { "branch": "if", "exprText": "state.kind === 'ready'", "range": {} }
          ],
          "selected": 0
        }
      ]
    }
  ],
  "components": [
    {
      "tag": "card",
      "componentFileId": "components/card/card.ts",
      "attributes": []
    }
  ],
  "events": [
    {
      "eventId": "e0",
      "attribute": "bind:tap",
      "event": "tap",
      "handler": "onCardTap",
      "attributeRange": {},
      "handlerRange": {}
    }
  ]
}
```

`analyze` 响应增量字段：

```json
{
  "eventResults": [
    {
      "eventId": "e0",
      "state": "supported",
      "definition": { "fileId": "pages/index/index.ts", "range": {} },
      "signature": "(event: WechatMiniprogram.TouchEvent) => void",
      "diagnostic": null
    }
  ],
  "components": [
    {
      "tag": "card",
      "diagnostics": [
        {
          "range": {},
          "severity": "error",
          "message": "Property 'title' expects 'string' but got 'number'"
        }
      ]
    }
  ]
}
```

新增 `complete` 方法：

```json
{
  "method": "complete",
  "params": {
    "protocolVersion": 2,
    "requestId": "c1",
    "analysisStamp": {},
    "view": {},
    "project": {},
    "scriptFileId": "pages/index/index.ts",
    "files": [],
    "fragments": [],
    "offset": 123,
    "kind": "expressionMember",
    "prefix": "item."
  }
}
```

响应：

```json
{
  "protocolVersion": 2,
  "requestId": "c1",
  "state": "supported",
  "truncated": false,
  "items": [
    {
      "label": "name",
      "kind": "property",
      "detail": "string",
      "documentation": "成员说明",
      "insertText": "name",
      "replace": { "start": 120, "end": 123 },
      "sortText": "0"
    }
  ]
}
```

约束：

- `complete` 只处理 Mustache 表达式成员和事件处理函数；标签/属性/枚举值由 Rust 静态补全，不启动 Node。
- Node 侧复用现有虚拟文件布局和 source map，把 TS `replacementSpan` 映射回 WXML。
- 单次最多 256 项；按 label 去重；文档长度上限 4096 字节。
- 协议版本不匹配时返回结构化 `protocol` 错误，Rust 侧降级并记录日志。

### 4.4 LSP 能力与分派骨架

在  crates/wxml-lsp/src/handlers/lifecycle.rs 增加：

- `completionProvider`：`triggerCharacters = ["<", " ", ":", "\"", "'", ".", "{", "="]`，`resolveProvider = false`。
- 请求分派：`textDocument/completion`，参数解析失败返回 `-32602`，后端不可用返回空列表并记录状态，不向用户抛出协议错误。
- 通知分派：`textDocument/didSave`、`workspace/didChangeWatchedFiles`、`workspace/didChangeConfiguration`。
- 非 WXML 文档保护：对 Script/Json 的 hover/definition/completion 返回 `null`/空列表，不返回错误；诊断不推送；避免 Zed 把 JS/TS 请求转发给本服务后干扰其他 LSP。

Phase 0 只加骨架和测试，不实现业务逻辑；`completion` 在 Node 后端仍返回 Unsupported，直到 WT-A 合入。

### 4.5 测试脚手架

新增/扩展：

-  crates/wxml-lsp/tests/lsp.rs：`request_completion`、`did_save`、`did_change_watched_files` 辅助方法，沿用 Content-Length 帧和 10s 超时。
-  crates/wxml-lsp/tests/node_backend.rs：Fake transport 支持 `complete` 请求和 v2 `analyze` 响应；新增 `capabilities_are_four_supported_features`（在 WT-A 完成后启用）。
-  crates/wxml-lsp/src/backend_node/tests.rs：协议 v2 的请求/响应形状、版本不匹配、超限截断。
-  crates/wxml-types/src/template.rs：locals/guards 仍按旧行为生成的基线测试，确保 Phase 0 不改语义。
- fixture：新增 `tests/fixtures/completion-basic`、`completion-context`、`events-basic` 的目录骨架，具体断言由对应 worktree 填充。

Phase 0 门禁：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
(cd backends/typescript && npm ci && npm run check && npm test && npm run build)
(cd editors/zed && cargo fmt --check && cargo test)
```

---

## 5. 工作包

### 5.1 WT-B：模板类型闭环（最先合入）

**分支**：`feat/wave5-template-types`

**目标**：AC-005、AC-006、AC-008、AC-012 全部通过；删除 `LOCALS_REASON` 的降级路径。

#### B1. wx:for locals 类型化

- 模板分析器为每个 `wx:for` 生成 `LocalBinding`，记录 item/index 名称、角色、源表达式和源码范围。
- 默认绑定 `item`/`index`；`wx:for-item`、`wx:for-index` 覆盖默认名；只绑定实际生效的名称。
- 嵌套循环按外层到内层顺序携带 bindings；退出循环后绑定消失；同名内层绑定遮蔽外层。
- Node 侧按顺序解析每个 binding：
  - 数组/元组/`ReadonlyArray`：item = 元素类型，index = number
  - `Record<K, V>` / 对象：item = value 类型，index = 键类型（通常 string）
  - `Map`/`Set` 等非 WXML 常见迭代对象：保守降级 any
  - `any`/`unknown`/解析失败：绑定 any，片段状态 Partial，不产生误报
- `wx:for` 表达式本身只使用外层 bindings，不包含本次新绑定。
- `wx:key` 不按 TypeScript 表达式检查；只接受 `*this` 或属性名，非法形式保持 Unsupported。

**验收**：

- `loop-scope` fixture 所有 hover 期望通过：外层 `item.name/title` 为 string、`itemIndex` 为 number；内层 `index` number、`item.name` string；退出内层后 `item.title` 仍为 string；对象 `groupMap` 的 index 为 string；循环外 `{{ item }}` 报未知标识符。
- 不再出现 `template locals are bound as any` 的 Partial 原因。
- 新增 Node 单测：数组、元组、Record、嵌套、shadowing、any 降级。

#### B2. 条件窄化

- 模板分析器为每个片段收集所在分支的 `GuardChain`：
  - `wx:if`：selected = 0
  - `wx:elif`：selected = 当前 elif 下标；Node 生成 `if (c0) ... else if (c1) ...`
  - `wx:else`：selected = conditions.len()；Node 生成最后的 `else`
  - 支持多层嵌套链
- Node 在虚拟文件中为片段构造嵌套 `if/else` 块，把目标表达式放在对应分支，让 TypeScript 控制流分析完成窄化。
- 支持判别联合（`state.kind === 'ready'`）和真值窄化（`wx:if="{{ maybeUser }}"`）。
- 守卫表达式无法解析、包含副作用或超出限制时，片段状态降为 Partial，不伪造类型。

**验收**：

- `conditional-union` fixture：
  - `wx:if state.kind === 'loading'` 分支正常
  - `wx:elif state.kind === 'ready'` 下 `state.user.name` 为 string，`state.user.age` 报错
  - `wx:else` 下 `state.code` 为 number
  - `wx:if maybeUser` 内 `maybeUser.name` 为 string
  - 守卫外 `maybeUser.name` 报 null 错误
- 新增 Node 单测：if/elif/else、嵌套、真值窄化、不可解析守卫降级。

#### B3. 组件属性绑定类型诊断（AC-012）

- 从模板分析器产出 `ComponentUsage`：标签名、属性、字面量/Mustache/混合值、表达式范围。
- 通过 `usingComponents` 解析到本地组件脚本；插件/包组件或无法解析的组件跳过，不报错。
- Node 读取组件 `properties` 声明，按以下规则比较：
  - 字面量字符串 → string
  - 布尔简写 → boolean
  - `{{ expr }}` → expr 类型
  - 联合/可选属性按 TypeScript 可赋值性判断
  - `any`/`unknown`/无法解析 → 跳过
- 不兼容时在属性值范围上报 Error，消息包含属性名、期望类型、实际类型。
- 内置组件属性不纳入本轮；仅自定义组件。

**验收**：

- `component-bindings` fixture：
  - `title="{{ pageTitle }}"` hover 为 string
  - `count="{{ total }}"` hover 为 number
  - `title="{{ total }}"` 报 number 不兼容
  - 合法绑定不报错
- 新增 Node 单测：String/Number/Boolean、联合、any、缺失属性、插件组件跳过。

#### B4. 多视图与别名（AC-008）

- 审计 `ProjectState` / `PreparedView` 是否按 WXML 建立独立 view；修复跨页面 `usingComponents` 串味。
- 审计 Node 请求中的 `compilerOptions`、`baseUrl`、`paths`、`extends` 是否按 view 生效。
- 不引入多根工作区；只保证单项目多页面/多组件隔离。

**验收**：

- `multi-view` fixture：pages/a 与 pages/b 各自 hover 类型正确；`card.wxml` 的 `user.nmae` 报错只出现在对应文件。
- `alias-project` fixture：`@models/*` 在 WXML 中 hover/definition 正常。
- 新增 LSP 集成测试：同时打开两个 WXML 和两个脚本，互不污染。

#### WT-B 文件所有权

-  crates/wxml-types/src/check.rs
-  crates/wxml-types/src/template.rs
-  crates/wxml-lsp/src/project_host.rs
-  crates/wxml-lsp/src/backend_node/mod.rs、request.rs、response.rs
-  crates/wxml-lsp/src/backend_node/ 下新增 `bindings.rs`、`guards.rs`、`component_bindings.rs`
-  backends/typescript/src/layouts/locals.ts、新增 `guards.ts`、`componentBindings.ts`
-  backends/typescript/src/analysis.ts、fragments.ts、types.ts
-  tests/fixtures/loop-scope、conditional-union、component-bindings、multi-view、alias-project
- 相关测试文件

**与其他 worktree 的边界**：

- `component-bindings` fixture 中属性绑定归 WT-B，事件诊断归 WT-C；两者只改各自 marker 区块。
- 不改 `completion` 静态目录；不改 Zed 配置。

---

### 5.2 WT-A：补全体系（第二合入）

**分支**：`feat/wave5-completion`

**目标**：F-01–F-07 全部可用；静态补全不依赖 Node；Node 不可用时表达式补全安静降级。

#### A1. 补全上下文引擎

新增  crates/wxml-lsp/src/completion/：

- `context.rs`：根据 offset 和语法 AST 判断补全上下文
  - 标签名：`<` 后、闭合标签 `</` 后
  - 属性名：标签内、属性名前缀
  - 属性值：引号内、`=` 后
  - Mustache：`{{` 内、`.` 后、标识符前缀
  - 事件属性值：`bindtap="`、`bind:tap="`、`catchtap="` 等
  - 文本/WXS/注释内不提供补全
- `catalog.rs`：静态 WXML 目录
  - 基础组件标签及说明
  - 每个标签的常用属性
  - 枚举属性值及上下文约束
  - `wx:` 指令和事件属性
  - 数据来源和覆盖范围写入 docs/reference/wxml-language-feature-baseline.md 或 provenance
- `static_items.rs`：生成 LSP `CompletionItem`
- `expression.rs`：表达式/事件处理函数补全，调用 Backend
- `mod.rs`：总入口、去重、排序、上限

补全项必须带精确 `textEdit` 范围，不能用整行替换；处理：

- 已有前缀：只替换前缀 token
- 空值：插入到引号内
- 闭合标签：`</` 后补标签名
- Unicode：范围按 UTF-16 由 LSP 层转换
- 不完整/容错语法：parser 恢复后仍能定位上下文

#### A2. 静态补全规则

- **F-01 标签名**：基础组件 + 当前页面/组件 `usingComponents`；闭合标签只提示已打开标签。
- **F-02 属性名**：当前标签已知属性 + 全局 `wx:` 指令 + 事件属性；去重。
- **F-03 枚举值**：至少覆盖
  - `button type`: default/primary/warn
  - `picker mode`: selector/multiSelector/time/date/region
  - `input type`: text/number/idcard/digit/safe-password/nickname
  - `scroll-view` 布尔属性
  - `wx:for` 相关属性
- **F-04 指令与事件**：`wx:if/elif/else/for/for-item/for-index/key`；`bindtap`、`catchtap`、`bind:tap`、`capture-bind:tap`、`capture-catch:tap`、`mut-bind:tap`。
- **F-05 上下文筛选**：属性值只在所属标签/属性下出现；不合法组合不提示。
- **F-06 自定义组件**：从 `PageConfig` 取当前 view 的 `usingComponents`，local 覆盖 global；组件标签提示说明和脚本路径。
- **F-07 表达式成员**：Mustache 中 `foo.` 后请求 Node；事件属性值请求 Node 方法列表。

#### A3. Node `complete`

- 在  backends/typescript/src/ 新增 `completion.ts`，复用 `analysis.ts` 的虚拟文件和 source map。
- 表达式成员：
  - 将不完整表达式放进当前 view 的虚拟文件
  - 调用 TypeScript `getCompletionsAtPosition`
  - 过滤关键字/内部符号，映射 kind/detail/documentation
  - 用 `replacementSpan` 映射回 WXML 范围；无法映射时用当前 token 范围
- 事件处理函数：
  - Page：顶层方法
  - Component：`methods` 内方法
  - 返回名称、签名、JSDoc
- 限制：最多 256 项、单请求超时沿用现有配置、无结果返回空而不是错误。

#### A4. LSP 集成

- `textDocument/completion` 参数校验：position、textDocument、context 可选。
- 返回 `CompletionList { isIncomplete: false, items }` 或数组；统一用 `CompletionList` 便于将来分页。
- 排序：精确前缀 > 标签/属性 > 枚举 > 表达式成员；`sortText` 稳定。
- 后端 unavailable/timeout：静态结果照常返回；表达式部分为空并记录日志，不弹错误。
- `completionProvider` 在 Node 后端不支持表达式补全时仍可声明，因为静态补全独立可用。

#### A5. 测试

- LSP 集成：
  - `<vi`、`<view `、`<view cla`、`class="`、`wx:`、`bind`、`</vi`
  - `picker mode="` 只出现合法值；`input type="` 不出现 picker 值
  - 自定义组件标签和属性
  - Mustache `{{ user.`、`{{ item.`（配合 locals）
  - 事件 `bindtap="on` 出现方法
  - Unicode 前缀和替换范围
  - WXS/注释/文本中不返回补全
- Node 单测：表达式成员、replacementSpan、事件方法、无结果、超限。
- 属性测试：随机在合法 WXML 位置插入前缀，补全范围必须落在原文边界内，替换后不破坏引号/标签。
- fixture：`completion-basic`、`completion-context`、`completion-components`。

#### WT-A 文件所有权

-  crates/wxml-lsp/src/handlers/lifecycle.rs（completion 分派）
-  crates/wxml-lsp/src/completion/ 新目录
-  crates/wxml-lsp/src/backend_node/mod.rs（completion 实现）
-  crates/wxml-lsp/src/backend_node/completion.rs 新文件
-  crates/wxml-types/src/backend.rs（Phase 0 已冻结，WT-A 只实现）
-  backends/typescript/src/completion.ts、server.ts、protocol.ts、analysis.ts
-  tests/fixtures/completion-*
- 相关测试

**边界**：不改 `locals/guards` 语义；不改事件诊断；不改 Zed 配置。

---

### 5.3 WT-C：事件处理函数链路（第三合入）

**分支**：`feat/wave5-events`

**目标**：事件属性不再只是解析结果，而是可诊断、可跳转、可补全。

#### C1. 事件采集

- 模板分析器输出 `EventHandler` 列表到 `PreparedView`：
  - `bindtap`、`catchtap`、`bind:tap`、`catch:tap`
  - `capture-bind:tap`、`capture-catch:tap`
  - `mut-bind:tap`
  - 动态/空 handler 记录 Unsupported，不报未知方法
- 精确记录属性范围和处理函数 token 范围。
- 事件名解析：`bind:tap` → `tap`；`bindtap` → `tap`；保留原始属性名用于补全。

#### C2. Node 解析

新增  backends/typescript/src/events.ts：

- Page：在 `Page({...})` 顶层查找方法/函数属性。
- Component：在 `Component({ methods: {...} })` 内查找。
- 处理函数可以是方法简写、函数属性、箭头函数属性。
- 返回：
  - 是否存在
  - definition（脚本文件 + 方法名范围）
  - 签名/JSDoc（用于 hover 或补全 detail）
- 多注册/非字面量注册按现有策略只取首个，状态 Partial。
- `any`/`unknown` 根不报缺失。

#### C3. 诊断

- 未知处理函数：在 handler token 范围报 Error，消息包含事件名和处理函数名。
- 只对可解析的本地 Page/Component 生效；插件/外部脚本跳过。
- 动态绑定、空绑定、非标识符 handler 保持 Unsupported，不产生噪声。
- 诊断与现有类型诊断合并、排序、去重。

#### C4. Definition 与补全

- `textDocument/definition` 在事件 handler 范围命中时返回方法声明位置。
- 找不到时返回 `null`（Supported + 空），与“后端不可用”区分。
- 事件属性值补全由 WT-A 的 completion 入口调用 Node `complete`，kind = `eventHandler`；WT-C 负责 Node 侧方法枚举。
- 事件属性名补全（`bind`/`catch`/`capture` 等）归 WT-A 静态目录，WT-C 不重复实现。

#### C5. 测试

- `component-bindings` fixture：
  - `bind:tap="onCardTap"` definition 跳到 `index.ts`
  - `bindtap="missingHandler"` 报错，definition 为 null
- 新增 `events-basic` fixture：
  - Page 顶层方法、Component methods、箭头函数
  - capture/catch/mut 绑定
  - 动态/空 handler 不报错
  - 多注册降级
- Node 单测：方法查找、签名、JSDoc、缺失、any 根。
- LSP 集成：诊断、definition、补全三条链路。

#### WT-C 文件所有权

-  crates/wxml-types/src/check.rs、template.rs（事件字段）
-  crates/wxml-lsp/src/project_host.rs（事件列表进入 PreparedView）
-  crates/wxml-lsp/src/backend_node/request.rs、response.rs、events.rs
-  backends/typescript/src/events.ts、analysis.ts、types.ts
-  tests/fixtures/component-bindings（仅事件 marker）、events-basic
- 相关测试

**边界**：组件属性绑定归 WT-B；静态事件属性名补全归 WT-A。

---

### 5.4 WT-D：Zed 多语言与实时同步（最后合入）

**分支**：`feat/wave5-zed-sync`

**目标**：真实 Zed 中，未保存的 JS/TS/JSON 修改能影响 WXML 分析；保存和外部文件变化能刷新诊断。

#### D1. 可行性 spike（先做，失败则停止并报告）

确认三个事实，并记录到  docs/evidence/p8-zed.md：

1. Zed 对一个 `[language_servers.wxml-lsp]` 配置下的多个 `languages` 是否只启动一个服务进程。
2. Zed 是否把关联语言的 `didOpen/didChange/didSave/didClose` 发给该进程。
3. Zed 是否支持 `workspace/didChangeWatchedFiles` 动态注册；不支持时的降级路径。

如果 Zed 为每个语言启动独立进程，overlay 无法共享，需改用：

- 方案 A：在扩展中通过 Zed extension API 转发文档事件（若 API 可用）。
- 方案 B：只注册 WXML，另加一个轻量 companion server/命令，把 JS/TS 内容通过 `workspace/executeCommand` 注入。

方案 B 作为兜底；未验证前不进入 D2。

#### D2. Zed 注册

修改  editors/zed/extension.toml：

```toml
[language_servers.wxml-lsp]
languages = ["WXML", "JavaScript", "TypeScript", "TSX", "JSON"]
```

- 确认 Zed 语言名和 language id 映射；必要时补 `language_ids`。
- 不注册 JSX（本轮不需要），避免扩大干扰面。
- 保持 `WXML_LSP_BIN` 绝对路径校验和现有环境变量转发。

#### D3. 服务端文档隔离

- `DocumentRoute::Script/Json` 只对项目根内、且被页面/组件/app/tsconfig 引用的文件建立 overlay。
- 非 WXML 文档的 hover/definition/completion 返回 `null`/空；不推送诊断。
- 非项目文件直接忽略，不创建 view、不启动 Node。
- 防止 Zed 把其他项目的 JS/TS 发进来导致缓存污染。

#### D4. didSave

- WXML：
  - 更新 snapshot（若 didChange 已更新则复用）
  - 重新分析并推送诊断
- Script/Json：
  - 保留 overlay（保存后内容与磁盘一致）
  - 标记依赖 view dirty，重新分析并推送 WXML 诊断
  - `app.json`/页面 JSON/组件 JSON/tsconfig 保存后触发项目 rediscovery
- 失败时只记录日志，不向客户端发错误通知。

#### D5. 文件监听

- 处理 `workspace/didChangeWatchedFiles`：
  - Created/Changed/Deleted 分别失效 overlay、项目发现、tsconfig 缓存
  - 对受影响的打开中 WXML 重新分析
- 若客户端支持动态注册，注册：
  - `**/*.wxml`
  - `**/*.json`
  - `**/*.ts`
  - `**/*.js`
  - `**/*.wxss`（仅失效缓存，不新增功能）
- Zed 不支持时，依赖 didSave + 下一次请求被动读取；在 README/证据中明确限制。
- `workspace/didChangeConfiguration` 目前只记录，不实现配置项。

#### D6. 验收

- Rust 集成：
  - 打开 WXML + TS，TS didOpen/didChange 未保存，WXML hover 看到新类型
  - TS didSave 后诊断刷新
  - 修改 `usingComponents` JSON 后自定义组件补全/诊断刷新
  - 修改 tsconfig paths 后别名 hover 刷新
  - 非项目 JS 文档请求返回 null，不影响 WXML
- Zed 手动 smoke（记录到  docs/evidence/p8-zed.md）：
  - 打开页面 WXML 和脚本
  - 修改脚本中 data 字段类型但不保存
  - 回到 WXML hover，类型已更新
  - 保存脚本，诊断刷新
  - 打开无关 TS 文件，确认不出现 WXML 诊断/补全干扰
- AC-004 unsaved-overlay 与 AC-010 zed-multi-doc 的真实编辑器场景补齐证据。

#### WT-D 文件所有权

-  editors/zed/extension.toml、src/lib.rs、languages/wxml/config.toml
-  crates/wxml-lsp/src/handlers/lifecycle.rs、diagnostics.rs
-  crates/wxml-lsp/src/project_host.rs
-  crates/wxml-project/src/lib.rs、discovery.rs（若需失效接口）
-  docs/evidence/p8-zed.md、README §0.4/§10
- 相关测试

**边界**：不改补全目录、模板类型算法、事件解析。

---

## 6. 测试与门禁

### 6.1 每个 worktree 必跑

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

Node 后端：

```bash
cd backends/typescript
npm ci
npm run check
npm test
npm run build
```

真实 Node 门禁（有 Node 环境时必跑，不允许新增 ignored）：

```bash
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test stdio_type_loop -- --ignored
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test real_node -- --ignored
```

Zed（WT-D 必跑，其他 worktree 至少跑 check）：

```bash
cd editors/zed
cargo fmt --check
cargo check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --target wasm32-wasip2 --release
```

覆盖率：

```bash
cargo llvm-cov --workspace --fail-under-lines 80
```

### 6.2 测试分层

| 层级 | 要求 |
| --- | --- |
| 单元测试 | 每个新模块至少覆盖正常、边界、错误路径 |
| 属性测试 | 补全替换范围、locals 作用域 push/pop、守卫链组合 |
| LSP 集成 | 真实二进制 `--stdio --mock`，校验 capability、请求、通知、错误码 |
| Node fake | 协议 v2 请求/响应、超限、版本不匹配、超时 |
| Node real | 类型、locals、guards、组件绑定、事件、补全的端到端 |
| fixture/AC | AC-005/006/008/012 以及新增 completion/events fixture |
| Zed | 22 个现有单测 + extension.toml 解析/路径校验；真实编辑器 smoke 记录证据 |

### 6.3 质量门槛

- 不允许新增 `#[ignore]` 作为绕过手段。
- 不允许 `unwrap()`/`expect()` 进入生产路径；测试内可用。
- 所有新增 `unsafe` 必须带 `// SAFETY:`；本轮预期不需要 `unsafe`。
- 错误必须带上下文；协议错误不得泄漏路径/堆栈到用户可见消息。
- 补全/诊断数量、文本长度、文件大小全部受限。
- 不引入新依赖，除非在计划评审中单独说明理由和许可证。

---

## 7. 每个 worktree 的 `/orchestrate` 流程

### 7.1 启动方式

控制器在主仓库外准备提示词和日志：

```text
/tmp/opencode/wave5/
├── prompts/{completion,templates,events,zed-sync}.md
├── logs/{...}.log
├── pids/{...}.pid
└── reports/{...}.md
```

启动命令示例：

```bash
cd /home/zxg/code/rust/wxml-lsp-worktrees/wave5-templates
opencode run --auto --agent planner \
  "/orchestrate 按 /tmp/opencode/wave5/prompts/templates.md 执行；完成后用 rust-reviewer 审核并自动修复，最多 2 轮"
```

每个 worktree 独立进程、独立日志；控制器不轮询等待，按完成事件收集报告。

### 7.2 orchestrate 内部步骤

1. **planner 复述**：范围、非目标、文件所有权、验收命令。
2. **并行 subagent**（至少 2 个，路径不重叠）：
   - `rust-tdd-guide`：核心 Rust 实现 + 单元/集成测试（RED→GREEN）
   - `rust-tdd-guide` 或 `rust-build-resolver`：Node 后端/独立模块/fixture；构建失败时切 build-resolver
   - 需要时再加 `rust-security-reviewer`（协议/路径/输入边界）和 `doc-updater`（文档）
3. **集成**：主 orchestrate agent 合并子任务结果，解决同文件冲突，跑全量门禁。
4. **Review Round 1**：`rust-reviewer` 全量审核；修复 CRITICAL/HIGH，尽量修复 MEDIUM；重跑门禁。
5. **Review Round 2**：再次审核；只修复剩余 CRITICAL/HIGH/MEDIUM；重跑门禁。
6. **停止条件**：两轮后仍有 CRITICAL/HIGH，停止并输出报告，不合并。
7. **提交**：每个逻辑阶段一个 commit，消息遵循 Conventional Commits；不 push 远端，除非控制器明确要求。
8. **报告**：写入 `/tmp/opencode/wave5/reports/<name>.md`，包含：
   - 分支、HEAD SHA
   - 变更摘要
   - 测试命令与结果（通过数/失败数/ignored）
   - AC 覆盖
   - 两轮 review 发现与修复
   - 剩余风险/未完成项

### 7.3 并行度与资源

- 默认四路并行；启动前检查 `nproc`、内存和 Node 进程数。
- 若 `nproc < 8` 或可用内存 < 16 GB，改为两波：
  - 第一波：WT-B + WT-A
  - 第二波：WT-C + WT-D
- 每个 worktree 内最多 2 个写代码 subagent 同时运行；其余审核/构建任务串行。
- 任一 worktree 超过 30 分钟无有效提交或连续两次门禁失败，控制器介入，必要时停止并转为串行修复。

---

## 8. 合并与集成

### 8.1 合并顺序

1. `feat/wave5-template-types` → `integration/wave5`
2. `feat/wave5-completion` → `integration/wave5`
3. `feat/wave5-events` → `integration/wave5`
4. `feat/wave5-zed-sync` → `integration/wave5`

理由：

- WT-B 改动模板语义和 Node 布局，最早合入，后续补全/事件都建立在 typed locals 上。
- WT-A 依赖 Phase 0 completion 契约，但独立于事件。
- WT-C 与 WT-B 共享 component-bindings fixture，B 先合入可减少冲突。
- WT-D 改 lifecycle/通知，最后合入，避免前面三个反复 rebase 通知层。

### 8.2 每个合并步骤

```bash
cd /home/zxg/code/rust/wxml-lsp
git switch integration/wave5
git merge --no-ff feat/wave5-template-types
# 解决冲突，跑全量门禁
```

- 冲突解决后必须重跑该 worktree 的 AC 和全量门禁。
- 不合入任何带 failing test 的分支。
- 不合入任何新增 ignored 测试。
- 保留 worktree 分支直到最终验收，便于回滚。

### 8.3 最终验收

在 `integration/wave5` 上：

- 全量 Rust/Node/Zed 门禁
- 真实 Node 门禁
- AC-005/006/008/012
- 新增 completion/events fixture
- Zed 手动 smoke 证据
- `rust-reviewer` 对最终 diff 再审核一轮（可选，但建议）
- 更新 README §0.4/§10、docs/reference 中过时的“全部未实现”表述、docs/evidence/p8-zed.md
- 确认无项目目录临时文件、无用户 fixture 修改被提交

### 8.4 合入 master / 远端

默认**不自动合入 master、不 push、不创建 Codeup PR**。最终验收后向用户报告：

- 集成分支和 SHA
- 测试结果
- 剩余风险
- 建议的 PR 目标分支和评审人

如果用户要求按既有习惯提交 PR，再使用 Codeup 流程合并到目标分支（默认 `release/4.0`，评审人按用户指定）。

---

## 9. 风险与应对

| 风险 | 影响 | 概率 | 应对 |
| --- | --- | --- | --- |
| 条件窄化在虚拟文件中难以保持 TS 控制流 | AC-006 失败 | 中 | 先实现单一 if 链，再扩展嵌套；无法解析时 Partial，不伪造；WT-B 提前 spike |
| wx:for 复杂类型（union、tuple、Record、any）误报 | 用户体验差 | 中 | 只对确定类型检查；any/unknown 一律降级；fixture 覆盖 |
| 组件属性类型声明形式多样 | AC-012 漏报/误报 | 中 | 只处理可解析的本地组件；插件/包/复杂声明跳过并记录 Partial |
| 补全静态目录覆盖面不足 | F-01–F-05 验收不全 | 中 | 先覆盖 fixture 和常用基础组件；目录带 provenance；文档明确覆盖范围 |
| 补全替换范围破坏 WXML | 编辑事故 | 中 | 属性测试 + 所有位置用 textEdit；不整行替换；Unicode 测试 |
| Zed 多语言是否共享一个进程未证实 | WT-D 方案失效 | 中高 | D1 先 spike；失败走 companion/executeCommand 兜底；不通过不进入 D2 |
| Zed 把 JS/TS 请求发给本服务，干扰其他 LSP | 编辑器体验回退 | 中 | 非 WXML 请求返回 null/空；只处理项目内文件；真实 smoke 验证 |
| 四路并行资源不足 | 超时/质量下降 | 中 | 资源检查；必要时两波；每 worktree 限 2 个写代码 subagent |
| Phase 0 契约冻结不完整 | 四个分支反复改接口 | 中 | Phase 0 必须通过基线测试；新增字段先 wire 通；冻结后再开 worktree |
| 合并冲突集中在 lifecycle/response | 集成延迟 | 高 | Phase 0 分派骨架；文件所有权；固定合并顺序；每步全量门禁 |
| 协议 v2 与旧 Node 不兼容 | 运行时失败 | 低 | Rust/Node 锁步；版本不匹配结构化错误；启动时 ping 校验 |
| 用户未提交 fixture 被误提交 | 污染提交 | 低 | 所有 worktree 不含该改动；最终检查 diff；主工作区不切分支 |

---

## 10. 验收清单

### 10.1 功能

- [ ] F-01 标签名补全（基础 + 自定义 + 闭合标签）
- [ ] F-02 属性名补全
- [ ] F-03 枚举属性值补全
- [ ] F-04 `wx:` 指令与事件属性补全
- [ ] F-05 上下文筛选
- [ ] F-06 `usingComponents` 自定义组件补全
- [ ] F-07 Mustache 成员补全 + 事件处理函数补全
- [ ] AC-005 loop-scope 全部通过
- [ ] AC-006 conditional-union 全部通过
- [ ] AC-008 multi-view / alias-project 全部通过
- [ ] AC-012 组件属性绑定诊断通过
- [ ] 事件未知处理函数诊断
- [ ] 事件处理函数 definition
- [ ] Zed 未保存 JS/TS 同步（AC-004）
- [ ] Zed 多文档场景（AC-010）
- [ ] didSave / watched files 行为

### 10.2 工程

- [ ] Phase 0 契约提交独立、基线测试通过
- [ ] 四个 worktree 均通过 `/orchestrate` + 2 个并行 subagent
- [ ] 每个 worktree 两轮 `rust-reviewer`，无剩余 CRITICAL/HIGH
- [ ] 全量 Rust 门禁通过
- [ ] Node `check/test/build` 通过
- [ ] 真实 Node 门禁通过
- [ ] Zed fmt/check/clippy/test/wasm 构建通过
- [ ] 覆盖率 ≥ 80%
- [ ] 无新增 ignored 测试
- [ ] 无项目目录临时文件
- [ ] 用户 fixture 修改未被提交
- [ ] README / 功能基线 / 证据文档更新，删除过时表述

### 10.3 交付物

- [ ] `integration/wave5` 分支
- [ ] 四个 worktree 分支及提交历史
- [ ] `/tmp/opencode/wave5/reports/*.md` 审计报告
- [ ] 最终测试日志和 Zed smoke 证据
- [ ] 合并建议与剩余风险清单

---

## 11. 默认决策与待确认项

以下为本计划默认值；如无异议，批准后按此执行：

1. **协议**：C1 升级 v2，Rust/Node 锁步，不保留 v1 兼容。
2. **补全目录**：先覆盖常用基础组件和 fixture 所需属性，不追求一次性全量；目录来源写入 provenance。
3. **Zed 范围**：注册 WXML、JavaScript、TypeScript、TSX、JSON；不注册 JSX。
4. **多根**：F-18 不在本轮；只做单项目多视图。
5. **合并目标**：先 `integration/wave5`，不自动合 master；PR 目标分支和评审人按用户后续指令。
6. **并行度**：默认四路；资源不足时两波。
7. **评审**：`rust-reviewer` 两轮；两轮后仍有 CRITICAL/HIGH 则停止并报告。
8. **代码托管**：worktree 和日志全部在仓库外；项目目录只提交源码、测试、fixture 和文档。

需要用户确认的只有一件事：**是否按本计划创建四个 worktree 并启动并行实现**。确认后进入 Phase 0。
