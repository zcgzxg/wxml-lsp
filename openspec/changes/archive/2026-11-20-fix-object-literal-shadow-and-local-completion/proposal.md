# Proposal

## Why

WXML 的 mustache 允许对象字面量，但当前 shadow 生成把表达式当作裸语句挂载，`{{ { ... } }}` 会被 TypeScript 解析成语句块，导致合法对象 key（例如 `elementId`）被误报为 `Cannot find name`。用户无法从诊断判断是模板写错还是语言服务误报，而且同一问题会影响文件顶部的 `data-*` 对象参数。

同时，`wx:for` 局部变量只参与完整表达式的 shadow 检查，尾点补全路径只识别脚本 `data` 根。输入 `item.` 时没有候选，`user.name.` 这类链式前缀也不支持。这两个问题在日常小程序开发中高频出现，根因和修复路径均已明确，适合现在一起处理。

## What Changes

- **修复对象字面量 shadow 挂载**：表达式首个非空白字符为 `{` 时，在挂载文本两侧加括号，确保 TypeScript 按对象字面量解析；`mount.text`、UTF-16 映射和补全插入点保持不变。
- **新增模板局部变量查询**：在 `wxml-types` 提供按光标位置收集祖先 `wx:for` 绑定的纯函数，返回外层到内层的 item/index 绑定，支持自定义 `wx:for-item` / `wx:for-index`，并排除光标所在 `wx:for` 属性自身的绑定。
- **补全片段携带局部绑定**：`CompletionFragment` 增加 `bindings`，合成 shadow 时生成对应的 `const item = __wxml_item(...)` / `const index = __wxml_index(...)` 声明，复用现有 shadow 局部变量机制。
- **泛化尾点成员补全**：不再要求点号前是裸标识符；只要前缀表达式可解析，且首个根是脚本 `data` 根或模板局部变量，就挂载完整前缀。支持 `item.`、`item.Setting.`、`user.name.`。
- **明确遮蔽规则**：同名循环局部变量优先于脚本 `data` 根，内层循环优先于外层；未知根仍返回明确的 Unsupported 状态。
- **更新能力说明与验收基线**：同步 `backend_tsgo` 的状态文本和 `docs/reference/wxml-language-feature-baseline.md` 的 F-07。
- **明确不做**：根补全中列出循环变量、`wx:if` guard 收窄、`this.` / 方法体补全，留作后续独立改动。

## Capabilities

### New Capabilities

- `template-expression-checking`: mustache 表达式的 shadow 表示与诊断映射，确保对象字面量等合法表达式按书写语法检查，诊断位置能映射回 WXML。
- `template-member-completion`: mustache 点号后的成员补全，覆盖脚本 `data` 根、`wx:for` 局部变量、链式前缀与同名遮蔽规则。

### Modified Capabilities

无。项目当前没有已建立的 capability specs，本次新增的两个 capability 各自拥有独立的用户可见行为。

## Impact

- `crates/wxml-backend-tsgo/src/shadow/layout.rs`：fragment 挂载括号、偏移映射保持、对象字面量单元测试。
- `crates/wxml-types/src/template.rs`：新增 `loop_bindings_at` 及嵌套循环、自定义名称、属性位置等测试。
- `crates/wxml-lsp/src/backend_tsgo/input.rs`：`CompletionFragment.bindings` 与 `FragmentSpec` 填充。
- `crates/wxml-lsp/src/backend_tsgo/completion.rs`：尾点 site 识别、链式前缀挂载。
- `crates/wxml-lsp/src/backend_tsgo/mod.rs`：`completion_member` 的局部变量解析顺序、能力状态文本。
- `crates/wxml-lsp/src/backend_tsgo/tests.rs`、`crates/wxml-backend-tsgo/tests/real_engine.rs`：补全与诊断回归测试。
- `docs/reference/wxml-language-feature-baseline.md`：F-07 能力描述与验收样本。
- 依赖与协议：无新增依赖，LSP 协议不变；用户可见变化是误报消失、补全候选增加。
- 兼容性：无 **BREAKING**。示例项目的 `inlineFunctionComponents` 二维类型修正属于仓库外配套修改，不进入本仓库。
