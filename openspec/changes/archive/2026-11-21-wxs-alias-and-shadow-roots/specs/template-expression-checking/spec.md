# Spec Delta

## ADDED Requirements

### Requirement: WXS 模块根不参与 data 根检查

模板表达式中的静态 WXS 模块名（来自 `<wxs module="...">`）SHALL 作为独立值参与 shadow 检查，MUST NOT 被当成页面或组件 data 的属性。该模块名在表达式中 MUST 保持原文，不得被改写为 data 根访问；其成员访问不得产生“属性不存在”诊断。

#### Scenario: 调用 WXS 模块函数不误报

- **GIVEN** 页面声明 `<wxs module="m1" src="./index.wxs" />`，data 中没有 `m1`
- **WHEN** 模板写 `{{ m1.getLimitSettingText(formData.attendLimit, formData.userLimit) }}` 并执行类型检查
- **THEN** 不产生 `Property 'm1' does not exist` 或等价的缺失字段诊断
- **AND** 表达式中的 `formData` 等 data 字段仍按原规则检查

#### Scenario: 同一表达式混合 WXS 与 data

- **GIVEN** 表达式同时引用 WXS 模块根和页面 data 字段
- **WHEN** data 字段拼写错误
- **THEN** 仍报告该 data 字段的错误
- **AND** 不因 WXS 模块名存在而抑制其它诊断

#### Scenario: 仅引用 WXS 模块的表达式仍被检查

- **GIVEN** 表达式只有 `{{ m1.someValue }}`，没有 data 根
- **WHEN** 语言服务构建 shadow
- **THEN** 该表达式仍被挂载检查，而不是按“无自由根”直接跳过

#### Scenario: 局部变量优先于同名模块

- **GIVEN** `wx:for` 的 item/index 名与某个 WXS 模块名相同
- **WHEN** 循环体内引用该名字
- **THEN** 按局部变量解析
- **AND** 循环体外仍按 WXS 模块解析

#### Scenario: 模块声明不降级诊断状态

- **GIVEN** 模板只使用了合法声明的 WXS 模块
- **WHEN** 语言服务发布诊断
- **THEN** 不因该模块声明产生 unresolved/partial 提示
- **AND** 其它模块缺失或动态形式仍按既有规则报告

### Requirement: 非 WXS 表达式行为不变

本改动 MUST NOT 改变普通 mustache 数据表达式的诊断、hover 与补全结果，也 MUST NOT 改变事件绑定、组件属性绑定和 WXS 自身的 hover/definition 行为。

#### Scenario: 普通成员访问保持稳定

- **GIVEN** 模板写 `{{ user.name }}` 且 data 中存在 `user`
- **WHEN** 语言服务执行检查
- **THEN** 诊断与改动前一致

#### Scenario: 事件与组件绑定保持稳定

- **GIVEN** 模板包含事件绑定与自定义组件属性绑定
- **WHEN** 用户执行 hover、definition 或诊断请求
- **THEN** 结果与改动前一致
