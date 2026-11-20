# Design

## Context

当前 mustache 的检查流程是：`wxml-types` 分析模板并产出 fragment，`wxml-lsp` 把 fragment 交给 `wxml-backend-tsgo`，后者生成一段 shadow TypeScript 并交给 tsgo。问题集中在两处：

- `crates/wxml-backend-tsgo/src/shadow/layout.rs` 把 fragment 表达式作为**裸语句**挂载。表达式以 `{` 开头时，TypeScript 会把它解析成 block，而不是对象字面量。`crates/wxml-syntax` 本身能正确解析对象 key，因此修复点应在 shadow 文本生成，而不是表达式解析器。
- 尾点补全走一条独立的“合成 fragment”路径。`crates/wxml-lsp/src/backend_tsgo/completion.rs` 只接受点号前的裸标识符，`completion_member` 又只通过 `member_root` 检查脚本 `data` 根；`wx:for` 局部变量从未进入这条路径。完整表达式中的成员 hover/补全走普通 shadow，因此不受影响。

现有 `wxml-types` 的 `Engine::loop_bindings` 已经实现了 `wx:for` 的 item/index 解析和命名规则，但它是分析器内部方法，只在遍历到元素时可用；`PreparedView` 不保存语法树，补全请求需要按光标位置重新确定作用域。`wxml-types` 已经依赖 `wxml-syntax`，适合承载这个纯查询。

目标与需求见 `proposal.md` 和 `specs/`，这里只记录实现方式和取舍。

## Goals / Non-Goals

**Goals:**

- 让 `{{ { ... } }}` 在 shadow 中按对象字面量解析，同时保持现有 UTF-16 映射、补全插入点和 guard 折叠逻辑不变。
- 在 `wxml-types` 提供可按任意偏移查询的循环绑定，语义与 `Engine::loop_bindings` 完全一致。
- 让补全 fragment 复用现有 shadow 局部变量生成逻辑，而不是在补全层另写一套 `wx:for` 语义。
- 把尾点补全从“裸 data 根”泛化到“可解析的完整前缀”，并明确局部变量优先于 data 根、内层优先于外层。
- 用单元测试覆盖偏移映射和作用域规则；真实引擎行为用可选测试验证。

**Non-Goals:**

- 不在根补全中列出 `wx:for` 局部变量。
- 不把 `wx:if` guard 加入补全 fragment，因此不做基于条件的类型收窄。
- 不支持 `this.`、方法体或事件处理器参数中的成员补全。
- 不改 `wxml-syntax` 的表达式 AST，也不改 LSP 协议。
- 不处理 `<template>` 片段作用域；当前分析器本来就不支持。

## Decisions

### 1. 在挂载点按需加括号，而不是改 rewrite 或表达式 AST

在 `layout.rs` 挂载 fragment 时判断 `planned.rewritten.text.trim_start().starts_with('{')`，为真则在表达式前后各写一个 `(` / `)`：

```text
body.push(indent)
body.push("(")            // 仅对象字面量
expression_start = ...
body.push(rewritten.text)
body.push(")")            // 仅对象字面量
body.push(";\n")
```

`mount.text` 仍然只保存表达式本身，`expression_start` 仍指向表达式首字符，`prefix_len`、segment、target 和补全插入点都不需要改。补全时后缀会插在 `)` 之前，得到 `(item.__wxml_completion)`，仍是合法表达式。guard 折叠场景下 `prefix_len` 从 guard 文本开始计算，`(` 落在 `expression_start` 之前，guard segment 不受影响。

**备选方案：**

- 无条件给所有表达式加括号：会改变所有 shadow 文本、现有快照和偏移断言，收益为零。
- 在 `rewrite_expression` 中把对象包成某种辅助调用：会改变表达式结构和 target 映射，且把“TS 语句上下文”问题带进了重写层。
- 修改 TypeScript 解析方式：shadow 文本最终仍是 TS 源码，无法绕过语句开头的 `{` 歧义。

WXML 表达式语法中没有函数表达式或类表达式，只有对象字面量会以 `{` 开头，因此条件判断是完备的。

### 2. 在 `wxml-types` 新增 `loop_bindings_at`，并抽出共享的属性绑定逻辑

把 `Engine::loop_bindings` 中“从 `wx:for` 属性得到 item/index 绑定”的部分抽成纯函数，供分析器和按偏移查询共用。新增公开函数大致为：

```rust
pub fn loop_bindings_at(tree: &SyntaxTree, offset: u32) -> Vec<LocalBinding>
```

实现规则：

- 从包含 `offset` 的最内层元素向上走，收集所有祖先元素的 `wx:for` 绑定，最终按**外层到内层**返回。
- 每层先 Item 后 Index，名称来自 `wx:for-item` / `wx:for-index`，默认 `item` / `index`。
- 如果光标位于某个元素自己的 `wx:for` 属性范围内，排除该元素自身的绑定；这与 `analyze_element` 先分析 `wx:for`、再压入绑定的顺序一致。
- 绑定复用现有 `single_mustache_source`、`for_binding_name` 和 `valid_binding_name` 校验，避免两套命名规则漂移。
- `source_expr` / `source_range` 使用 `wx:for` mustache 的内层表达式，范围是 WXML 绝对偏移。

**备选方案：**

- 在 `wxml-lsp` 里重新实现一套：会复制命名、校验和 mustache 解析逻辑，后续容易漂移。
- 把循环作用域存进 `PreparedView`：需要扩大 `ViewProvider` 接口并处理每次编辑后的失效；补全路径本来就会解析当前请求文本，按需查询更小。
- 放到 `wxml-project`：`wxml-project` 只依赖 `wxml-text`，不应该为模板语义反向依赖 `wxml-types` 或 `wxml-syntax`。

`loop_bindings_at` 只做语法层的作用域查询，不判断绑定类型，也不依赖任何 LSP 类型。

### 3. `CompletionFragment` 携带完整 `LocalBinding`，复用 `render_locals`

`CompletionFragment` 增加 `bindings: Vec<wxml_types::LocalBinding>`。构造补全 `FragmentSpec` 时：

- `bindings` 通过现有 `local_binding` 映射为 `ShadowLocalBinding`；
- `locals` 同步填入绑定名，保持与普通 fragment 的约定一致。

这样 `render_locals` 会生成：

```ts
const item = __wxml_item(__wxml_root.inlineFunctionComponents);
const index = __wxml_index(__wxml_root.inlineFunctionComponents);
```

并复用现有的捕获、嵌套层级和 root 重写逻辑。补全 fragment 目前不携带 guards，因此只影响类型，不影响补全可用性。

**备选方案：**

- 只传绑定名、在补全层手写 `const item = ...`：会绕过 `__WxmlItem`、捕获变量和层级处理，容易与普通 shadow 不一致。
- 让补全直接用 `wxml-types` 推导类型：会把类型语义复制到 `wxml-lsp`，违背分层。

### 4. `completion_member` 先解析模板局部变量，再回退到 `data` 根

成员补全的根解析顺序改为：

1. 解析当前请求文本，调用 `loop_bindings_at` 得到光标处绑定。
2. 从内到外查找同名绑定；命中则视为局部变量，跳过 `member_root` 的 data 根检查。
3. 未命中时保留现有 `member_root` 流程；仍未知则返回现有的 Unsupported 状态。

局部变量优先于同名 `data` 字段，符合 WXML 的遮蔽语义。根名只用于判定作用域，真正的类型由 shadow 中生成的 `const item = __wxml_item(...)` 决定。

**备选方案：**

- 把局部变量和 data 根合并后交给 `member_root`：`member_root` 只理解脚本根，会把局部变量误判为未知。
- 在 `expression_site` 阶段就决定根：site 识别不应承担作用域解析，且 `completion_member` 已经持有请求上下文和 project host。

### 5. 尾点补全接受完整前缀，但保留“根必须可解析”的约束

`expression_site` 和 `typed_member_site` 删除“点号前必须是裸标识符”的限制，改为：

- 前缀必须能被 `wxml-syntax` 完整解析，且没有 missing 节点；
- 前缀的首个根必须是脚本 data 根或当前局部变量；
- 挂载完整前缀，替换范围仍只覆盖点号后已输入的标识符。

这样 `item.`、`item.Setting.`、`user.name.` 都走同一条路径。`expression_has_free_root` 继续作为“是否存在可解析根”的快速检查。

**备选方案：**

- 只允许标识符和成员访问链：能覆盖当前场景，但会把 `foo().` 这类合法表达式排除在外；挂载完整前缀本身是合法 TS，限制没有收益。
- 保留裸根限制，只给局部变量开特例：`item.Setting.` 仍然不可用，需求没有真正满足。

### 6. 测试分层

- `wxml-types`：`loop_bindings_at` 的嵌套循环、自定义名称、光标在 `wx:for` 属性内、循环外、同名 item/index 去重。
- `wxml-backend-tsgo/src/shadow/layout.rs`：多行对象字面量 fragment 的 shadow 文本包含 `({`、`mount.text` 不含括号、值位置映射回原 WXML 字节。
- `wxml-lsp/src/backend_tsgo/tests.rs`：用 fake adapter 检查补全 shadow 中出现局部变量声明和 `item.__wxml_completion`；覆盖自定义 `wx:for-item`、局部遮蔽 data 根、未知根仍 Unsupported、链式前缀。
- `wxml-backend-tsgo/tests/real_engine.rs`：在 `TSGO_BIN` 可用时验证真实成员候选；默认跳过，不把默认 `cargo test` 当成类型行为已验证。
- 文档：同步 `mod.rs` 的状态文本和 `docs/reference/wxml-language-feature-baseline.md` 的 F-07。

## Risks / Trade-offs

- **括号改变 shadow 文本，可能影响现有快照或偏移断言** → 只对 `{` 开头表达式加括号；`mount.text` 保持表达式原文；新增对象字面量的映射测试，并跑完整 layout 测试。
- **guard 折叠时括号位置影响 guard segment** → `(` 写在 `expression_start` 之前，guard segment 仍相对于 guard 起点；补一个带 guard 的对象字面量测试。
- **每次补全都解析整份 WXML 有成本** → 只在成员补全路径解析，且 `expression_site` 已经按请求解析；不触发项目级分析，成本与现有补全同量级。
- **光标位于 `wx:for` 属性内部时错误地包含自身绑定** → 明确用属性范围排除自身，并加针对性测试。
- **嵌套同名变量顺序错误** → `loop_bindings_at` 返回外层到内层，查找时从后向前取第一个；用内层遮蔽测试锁定。
- **补全 fragment 没有 guards，无法收窄** → 明确列为 non-goal；只生成局部变量声明，不引入错误的窄化假设。
- **链式前缀包含不完整调用或缺失节点** → 复用 `parse` 的完整性检查，失败时回退 Unsupported，不挂载半截表达式。
- **真实引擎测试默认跳过，容易误判已验证** → 单元测试覆盖 shadow 文本和映射；真实引擎测试显式标注需要 `TSGO_BIN`，交付说明中写清验证条件。

## Migration Plan

无数据迁移。实现完成后：

1. 运行仓库验收命令；
2. `cargo build --release -p wxml-lsp`；
3. `scripts/install-native.sh --root "$HOME/.local/share/wxml-lsp"`；
4. 重启编辑器的 wxml-lsp 会话，使新二进制生效。

回滚只需恢复代码；shadow 文本和 LSP 协议均无持久化状态。
