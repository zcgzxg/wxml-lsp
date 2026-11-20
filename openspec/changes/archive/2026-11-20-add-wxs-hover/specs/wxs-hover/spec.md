# Spec Delta

## Purpose

让 WXS 模块与成员在 WXML mustache 和 `.wxs` 源文件中都有可用、可读、位置正确的 hover。内容来自 `wxml-types` 的静态摘要，不依赖 Node、真实 TypeScript 引擎或 WXS 运行时。

## ADDED Requirements

### Requirement: mustache 中的 WXS 模块 hover 定位到模块引用

当用户在 `{{ m1.someFunc() }}` 中 hover 模块根 `m1` 时，hover SHALL 返回被 hover 的 `m1` token 的范围，而不是 `<wxs module="m1">` 属性值的范围。definition 仍 MUST 指向对应的 WXS 源文件。

#### Scenario: hover src 引入的模块根

- **GIVEN** 页面含有 `<wxs module="m1" src="../../utils/fmt.wxs" />`
- **WHEN** 用户在 `{{ m1.someFunc(name) }}` 中 hover `m1`
- **THEN** hover range 覆盖 mustache 中的 `m1`
- **AND** 内容包含 WXS 模块名与来源文件
- **AND** definition 位置指向 `utils/fmt.wxs` 的声明

#### Scenario: hover inline 模块根

- **GIVEN** 页面含有内联 `<wxs module="m2">...</wxs>`
- **WHEN** 用户在同一页面的 mustache 中 hover `m2`
- **THEN** hover range 覆盖 mustache 中的 `m2`
- **AND** 不指向内联块的 `module` 属性

### Requirement: 模块 hover 展示导出成员的类型或签名

模块 hover SHALL 为每个可达导出成员展示名称与类型信息：函数展示签名，值展示类型，子模块展示模块说明。成员过多时 MUST 截断并明确说明，而不是静默省略。动态导出或解析不完整时 MUST 保留 Partial 提示，且不得伪造成员。

#### Scenario: 导出函数带签名

- **GIVEN** WXS 文件导出 `function someFunc(name: string): string`
- **WHEN** 用户 hover 模板中的 `m1`
- **THEN** 导出清单包含 `someFunc` 及其签名 `(name: string) => string`

#### Scenario: 导出值带类型

- **GIVEN** WXS 文件导出 `module.exports = { version: "1.0" }`
- **WHEN** 用户 hover 模块根
- **THEN** 导出清单中的 `version` 带上静态推断的类型

#### Scenario: 动态导出不伪造成员

- **GIVEN** WXS 使用 `module.exports[key] = value` 或无法静态枚举的导出形式
- **WHEN** 用户 hover 模块根
- **THEN** 内容保留 Partial 说明
- **AND** 不列出无法确认名字的成员

### Requirement: WXS 成员 hover 展示签名与可读 JSDoc

在模板 mustache 中 hover 导出的 WXS 成员时，hover SHALL 展示成员签名或类型，并把 JSDoc 按可读的多行结构呈现。`@param` / `@returns` 中的类型花括号 MUST 保持原样，不得转义为 `\{`。HTML、链接和控制字符仍 MUST 被安全处理。

#### Scenario: 多行 JSDoc 保持结构

- **GIVEN** WXS 函数带注释：
  ```js
  /**
   * Formats a name for display.
   * @param {string} name input name
   * @returns {string} formatted name
   */
  ```
- **WHEN** 用户在模板中 hover `m1.someFunc`
- **THEN** 注释行之间保留换行
- **AND** 内容包含 `@param {string} name input name`
- **AND** 不出现 `\{string\}`

#### Scenario: 文档超出预算时截断

- **GIVEN** WXS 成员的 JSDoc 超过 hover 文档预算
- **WHEN** 用户 hover 该成员
- **THEN** 文档以省略号截断
- **AND** 签名与成员名保持完整

### Requirement: .wxs 源文件内的 hover

在打开的 `.wxs` 文档中，hover 声明名、`module.exports` 键或文件内的成员引用时，语言服务 SHALL 返回该成员的签名/类型与 JSDoc。hover 范围 MUST 覆盖光标所在的词。非导出成员 MUST 标注未导出；无法解析为成员的词 MUST 返回 `null`，不得返回空 hover。

#### Scenario: hover 函数声明

- **GIVEN** `.wxs` 文件声明 `function someFunc(name) { ... }` 并带 JSDoc
- **WHEN** 用户 hover 函数名
- **THEN** 返回函数签名与 JSDoc
- **AND** hover range 覆盖函数名

#### Scenario: hover 导出键

- **GIVEN** `.wxs` 文件包含 `module.exports = { someFunc }`
- **WHEN** 用户 hover `someFunc` 导出键
- **THEN** 返回与声明一致的签名与文档

#### Scenario: hover 文件内调用

- **GIVEN** `.wxs` 文件在另一函数体内调用 `someFunc(name)`
- **WHEN** 用户 hover 调用处的 `someFunc`
- **THEN** 返回同一成员的签名与 JSDoc

#### Scenario: 未导出成员标注

- **GIVEN** `.wxs` 文件声明了未放入 `module.exports` 的局部函数
- **WHEN** 用户 hover 该局部函数名
- **THEN** hover 展示其签名
- **AND** 明确说明该成员未导出

#### Scenario: 非成员词返回 null

- **GIVEN** 光标位于 `.wxs` 文件中的关键字、字符串或普通注释文本
- **WHEN** 用户请求 hover
- **THEN** 返回 `null`

### Requirement: 既有 hover 行为不回退

本次改动 MUST NOT 改变 `<wxs>` 标签与属性 hover、WXS definition、WXS 补全、事件 hover、组件 hover，以及 `.ts` / `.json` 等非 WXML 文档的 hover 结果。

#### Scenario: 非 WXML 文档仍无静态 hover

- **GIVEN** 用户打开 `.ts` 或 `.json` 文档
- **WHEN** 请求 hover
- **THEN** 不经过 WXS 静态 hover 分支
- **AND** 结果与改动前一致

#### Scenario: WXML 事件 hover 保持单行文档

- **GIVEN** WXML 元素绑定事件处理器
- **WHEN** 用户 hover 事件属性值
- **THEN** 事件处理器的说明仍按既有单行格式展示
- **AND** 不因新的文档格式化逻辑改变
