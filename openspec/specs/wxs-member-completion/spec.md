# wxs-member-completion Specification

## Purpose

让 WXML mustache 中的 WXS 模块点号补全在真实书写与格式化后的文本里都可用：返回模块导出的成员、签名与 JSDoc，只替换用户已输入的片段，并在无法完整枚举导出时明确标记不完整。

## Requirements

### Requirement: 点号后返回模块导出成员

当光标位于 `{{ <module>.` 或 `{{ <module>?.` 之后，且 `<module>` 是当前文件中 `<wxs>` 声明的模块时，补全 SHALL 返回该模块 `module.exports` 可静态确定的成员。每个候选 MUST 使用导出名作为标签，并使用静态摘要中的签名或类型作为 detail；文档非空时 MUST 附带 JSDoc。动态导出、导出数量截断或 WXS 源不可用时 MUST 标记为不完整，而不是伪造成员。

#### Scenario: src 引入的模块补全导出成员

- **GIVEN** 页面含有 `<wxs module="m1" src="../../utils/fmt.wxs" />`，且 `fmt.wxs` 导出带 JSDoc 的 `someFunc`
- **WHEN** 用户在 `{{ m1. }}` 的点号后请求补全
- **THEN** 候选包含 `someFunc`
- **AND** detail 包含 `(name: string) => string`
- **AND** documentation 包含该函数的 JSDoc

#### Scenario: 内联模块补全导出成员

- **GIVEN** 页面含有内联 `<wxs module="m1">`，并 `module.exports = { fmt, count }`
- **WHEN** 用户在 `{{ m1. }}` 请求补全
- **THEN** 候选同时包含 `fmt` 与 `count`

#### Scenario: 只替换已输入片段

- **GIVEN** 用户已经输入 `{{ m1.so`
- **WHEN** 选择候选 `someFunc`
- **THEN** 替换范围只覆盖 `so`，`m1.` 与其余文本不变

#### Scenario: 动态导出标记不完整

- **GIVEN** 模块存在 `exports[k] = value` 这类无法静态枚举的导出
- **WHEN** 用户在点号后请求补全
- **THEN** 已确定的导出仍然返回
- **AND** 结果标记为不完整

### Requirement: 点号与光标之间的空白不影响补全

点号或可选链 `?.` 之后、光标之前存在空格、制表符或换行时，补全 SHALL 仍然按该模块的导出成员解析，不得退化为类型后端的空结果。替换范围 MUST 保持零宽或只覆盖光标前已输入的非空白片段。

#### Scenario: 点号后带空格

- **GIVEN** 文本为 `{{ m1.  }}`，光标位于两个空格之后、`}}` 之前
- **WHEN** 请求补全
- **THEN** 候选包含 `m1` 的导出成员
- **AND** 替换范围起点等于光标位置

#### Scenario: 可选链后带空格

- **GIVEN** 文本为 `{{ m1?. }}`，光标位于空格之后
- **WHEN** 请求补全
- **THEN** 候选包含 `m1` 的导出成员

### Requirement: 表达式起始处补全模块名

当 mustache 内光标前只有空白、或只有已输入的模块名前缀时，补全 SHALL 返回当前文件的 WXS 模块名候选。前导空格 MUST NOT 阻止该候选。

#### Scenario: 前导空格后补全模块名

- **GIVEN** 页面声明了 `<wxs module="m1">`，文本为 `{{ m1`
- **WHEN** 在 `m1` 之后请求补全
- **THEN** 候选包含模块名 `m1`

### Requirement: 既有补全行为不回归

本次改动 MUST NOT 改变 `template is` / `template data` 补全、脚本 `data`/`wx:for` 成员补全、WXS hover 与 definition 的行为；非 WXS 的 mustache 仍按原有顺序进入类型后端。

#### Scenario: 非 WXS 表达式不被静态补全拦截

- **GIVEN** 模板没有 `<wxs>` 声明
- **WHEN** 用户在 `{{ user. }}` 请求补全
- **THEN** 请求仍交给既有的脚本/后端补全路径
- **AND** 静态 WXS 补全不返回结果
