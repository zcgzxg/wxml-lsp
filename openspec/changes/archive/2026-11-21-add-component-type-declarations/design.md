# Design

## Context

见 `proposal.md`。约束摘要：

- 分层固定：`wxml-project` 只依赖 `wxml-text`；`wxml-types` 不依赖 `wxml-project`；`wxml-lsp` 依赖全部。
- 无 Node 运行时，默认 `cargo test` 不启动真实引擎；凡是"必须问引擎"的行为都要能被伪造查询单测覆盖。
- 引擎 VFS 只挂固定库内 typings 与项目文件，npm 包内类型不可达（ADR-0010 不扫 `node_modules`）。
- `collect_sources` 已能按预算读取 `.d.ts`（含 overlay 优先、不跟随 symlink），声明读取复用同一套读取语义。
- 服务对用户工作区只读：不能改写脚本，因此不能给 handler 形参注入类型。

## Goals / Non-Goals

**Goals:**

- 一个可测试的纯 Rust 管线：约定路径 → 声明事实 → 视图合并 → 诊断/补全/hover。
- 事实对象带「可证明 / 未证明」与「不完整」标记，使降级原因可以透传到 LSP 状态。
- 事件签名校验不依赖引擎进程，可用普通单测覆盖。

**Non-Goals:**

- 不实现完整的 TypeScript 类型检查器；只解析本变更定义的自包含子集，其余一律 fail-closed。
- 不解析 `typeRoots`、`types`、`references`、`paths` 重写到 npm 包。
- 不做 `d.ts` 生成器（不从组件源码生成声明文件）。
- 不改 LSP 协议、不新增配置项以外的 IO。

## Decisions

### D1 固定约定路径，不做模式匹配

声明文件路径固定为项目 `tsconfig.json` 同级的 `components.d.ts`：

```rust
tsconfig.path.parent()?.join("components.d.ts")
```

- 没有 `tsconfig.json` → 没有声明，不做任何兜底猜测。
- 不读 tsconfig 的 `files`/`include`/`exclude`，不实现 glob，不遍历目录。
- 理由：小程序没有消费 tsconfig 类型语义的链路，为匹配声明文件再引入 glob、`extends` rebase 与默认排除是纯成本；固定路径让"哪个 app 用哪份声明"退化成"这个项目用哪份声明"，没有歧义。
- 替代方案被否：tsconfig 模式驱动（实现大、易误匹配）；扫描全部 `.d.ts`（每次加载多扫、还要处理同名冲突）。代价是同一 tsconfig 下的多个 app 共享一份声明，这符合"tsconfig 即项目边界"的既有模型。

### D2 安全读取与边界

- 读取走 `Project` 既有的 overlay 优先路径，未保存内容立即生效。
- 只接受项目根内、扩展名 `.d.ts` 的普通文件；不跟随 symlink（沿用 `sources` 的行为）。
- 大小上限复用 `sources.rs` 的单文件预算常量，不新增常量。
- 文件缺失 → `Ok(None)`，不产生 issue、不改变能力状态。
- 目录、路径逃逸、不可读、超限 → 返回一条稳定、不含绝对路径的中文 issue，能力状态 `Partial`；任何情况下都不 panic。

### D3 缓存与失效

- 事实按 `(declaration_path, revision)` 缓存；revision 由文件内容版本与 overlay 版本合成。
- overlay 改动、磁盘文件变化、`rediscover`、tsconfig 变化（锚点重算）都必须让缓存失效。
- 每个 `ProjectSlot` 独立持有，不跨 app 复用。

### D4 声明事实模型

`wxml-project/src/typing.rs`：

```rust
pub struct ComponentTypingFacts {
    pub components: BTreeMap<String, TypingComponent>, // key = WXML 标签
    pub path: ProjectPath,
    pub complete: bool,
    pub issues: Vec<TypingIssue>,
}
pub struct TypingComponent {
    pub props: Vec<TypingProperty>,   // name, type_text, optional, proven
    pub events: Vec<TypingEvent>,     // name, detail_text: Option<String>, proven
    pub complete: bool,
}
```

解析对象：`WxmlComponentMap`（`declare global` 内或顶层）+ 同文件 `interface`/`type` 别名。映射表内重复键 MUST 取第一个并记 issue（TypeScript 自身也会报重复属性）。

类型判定统一走一个 `TypeText` 迷你语法树（基础类型、字面量、联合、数组、`Record<string,T>`、嵌套对象）；解析不出的成分把该成员标 `proven = false`。`type_text` 原文保留用于 hover，不得重排用户的 union 顺序。

优先复用 `script.rs` 已有的对象字面量解析代码（它已能处理 `Component({ properties })`）；若其 AST 不适配，就在 `typing.rs` 内单独实现，不修改脚本路径的行为。

### D5 优先级在合并点一次决定

- `ComponentPropertyFacts` 增加 `source: PropertySource::{Script, Typing}`。
- `wxml-lsp/src/project_host.rs` 的视图组件构造处，先取脚本事实，再用声明事实整表替换并置 `source = Typing`。
- `ViewComponent::apply_live_facts`：`source == Typing` 时直接返回，overlay 不得覆盖。
- `component_bindings::analyze_component`（引擎脚本推导）：视图已带 `Typing` 事实时跳过，省一次查询。
- `ViewComponent` 增加 `typing: Option<Arc<TypingComponent>>`，事件链路直接读它。

### D6 属性契约映射

`component_bindings.rs` 内新增 `PropertyContract`：

```rust
enum PropertyContract { Unknown, Any, Kind(Target), StringLiterals(Vec<String>), NumberLiterals(Vec<String>) }
```

- 由 `TypeText` 构建；`Unknown` → 不产生诊断（`Partial`）。
- 可空性：`optional` 或含 `undefined` 时允许缺省与 `undefined`。
- 复用现有"绑定表达式求类型"的查询与诊断落点；类型文本映射不到契约时降级为 `Unknown`，不报错。
- 保留既有 `Target` 枚举不变，避免扩散式改动。

### D7 事件签名校验走静态参数标注

不做 shadow 形参注入。`wxml-project/src/script.rs` 在提取方法时同时记录形参类型标注原文：

```rust
pub struct HandlerFact { pub name: String, pub parameter: Option<String> } // 原文，可能是 CustomEvent<...>
```

校验发生在事件解析处：`resolve_events` 增加可选 `EventContractTable`（事件名 → detail 文本 + 是否可证明），返回

```rust
enum HandlerTypeCheck { Skipped(Reason), Compatible, Incompatible }
```

`Incompatible` 才发布 `Error`；比较用 D4 的 `TypeText` 做规范化结构比较（空白、联合顺序、可选标记无关），不做结构化子类型。语义：

| 条件 | 结果 |
| --- | --- |
| handler 不存在 | 现有 `Error`，不叠加类型诊断 |
| 事件 detail 未证明 | `Skipped`，状态 `Partial` |
| handler 完全无参数（`arity == 0`） | `Compatible`，无诊断；调用方不进入 `check_handler` |
| handler 有参数但未标注 | `Skipped`，状态 `Partial`，原因可读 |
| 参数标注可解析且与 detail 不兼容 | `Incompatible` → handler token 上 `Error` |
| 其余 | `Compatible`，无诊断 |

未声明事件名诊断（`Warning`）在同一处产出，仅当 `TypingComponent.complete && events` 声明存在时启用，落在事件名 token。

### D8 状态与降级透传

- `PreparedView` 已能上报能力状态；新增原因文案（中文，稳定字符串）：声明文件不可读、声明解析不完整、类型未证明、handler 参数未标注、同名标签冲突。声明文件缺失不是降级原因，状态必须保持不变。
- 遵守 `type-diagnostics.md` 的"空诊断只有 `Supported` 才算干净"：任一降级原因存在时状态为 `Partial`。
- 多个同级原因共存时保留先到者（声明侧原因优先于 handler 参数未标注），原因文案稳定、不含绝对路径、不拼接事件名与 handler 名。

### D9 缓存与失效

- `Project` 持有 `Arc<ComponentTypingFacts>`，在 `rediscover` 时重算。
- 视图层用 digest 缓存：候选 `.d.ts` 的 `(path, revision)` 排序 + tsconfig 内容版本；digest 变化才重新解析。`ComponentTypingFacts` 不缓存跨 app（每个 `ProjectSlot` 独立）。
- overlay 修改被命中的 `.d.ts` 或 tsconfig MUST 触发 digest 变化。

### D10 文档与验证

- `docs/reference/type-diagnostics.md`：§7.1 增加声明来源与优先顺序，§7.2 重写为下表 + hover/补全契约，新增"组件类型声明"小节（格式、发现规则、子集、非目标）。
- `docs/adr/0011-component-type-declarations.md`：记录三个架构决策（tsconfig 同级固定路径发现、自包含声明子集、d.ts 优先于脚本）。
- fixture：`tests/fixtures/components-typings/`（真实小程序工程；app.json + usingComponents + `tsconfig.json` 与**同级** `components.d.ts`），并更新 `tests/fixtures/README.md`。
- 测试：`wxml-project` 单测覆盖路径发现/读取/解析/合并；`wxml-lsp` 用 HostOnly 覆盖属性与事件名诊断、补全、hover；表达式类型不匹配用既有可注入假查询的测试路径。

## Risks / Trade-offs

- [固定路径无法按 app 拆分声明] → 一个 `tsconfig.json` 对应一份 `components.d.ts`；需要不同声明时，把声明放在各自 app 的 tsconfig 目录（懒发现本来就以最近的 tsconfig 为归属）。
- [约定路径对用户是隐式的] → 文档、ADR 与 fixture 都写清"与 tsconfig 同级"；文件缺失时零噪声，未采用者完全无感。
- [事件名 `Warning` 误报] → 仅在声明完整时启用；文档明确"库可能 triggerEvent 未声明事件"；删除映射即可关闭。
- [声明解析器成为半个 TS 编译器] → 子集固定、成员级 fail-closed、设计文档列出明确非目标；新增语法必须伴随测试再扩展。
- [静态签名比较漏报] → 只承诺"可证明时报告"；未知类型与未标注参数一律 `Partial`，不声称检查通过。

## Migration Plan

无迁移：没有声明文件的项目行为完全不变；采用者只需把 `components.d.ts` 放到 `tsconfig.json` 同级。回滚即删除该文件。
