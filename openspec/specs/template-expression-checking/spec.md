# template-expression-checking Specification

## Purpose

保证 mustache 中的表达式按 WXML 的书写语义参与类型检查，尤其是对象字面量不会被误解析为语句块。表达式产生的诊断必须能准确映射回模板中的原始位置，帮助用户区分模板错误与语言服务误报。

## Requirements

### Requirement: 对象字面量按对象字面量检查

mustache 中以 `{` 开头的表达式 SHALL 作为一个完整的对象字面量参与类型检查。对象 key MUST NOT 被当作变量引用，key 对应的 value MUST 按正常表达式解析并检查类型。

#### Scenario: 多行对象字面量不误报 key

- **GIVEN** 模板中存在 `{{ { elementType: SomeEnum.Value, elementId: SomeConfig.Value } }}`
- **WHEN** 语言服务对该 mustache 执行类型检查
- **THEN** `elementType` 与 `elementId` 不产生“找不到名称”或“缺少分号”诊断
- **AND** 两个 value 的类型按正常规则检查

#### Scenario: 对象字面量中的局部变量正常解析

- **GIVEN** 模板位于 `wx:for` 内，存在 `{{ { elementType: item.ActionType, elementId: item.ElementId } }}`
- **WHEN** 语言服务执行类型检查
- **THEN** 对象 key 不产生诊断
- **AND** `item.ActionType` 与 `item.ElementId` 按 `item` 的实际类型解析

#### Scenario: 对象字面量作为属性值

- **GIVEN** 模板中存在 `data-report="{{ { type: item.Type, id: item.Id } }}"`
- **WHEN** 语言服务检查该属性值
- **THEN** 该表达式按对象字面量检查，不产生 key 相关误报

### Requirement: 表达式诊断映射回模板位置

由 mustache 表达式产生的诊断 SHALL 映射回 WXML 源文件中的对应位置。映射后的范围 MUST 覆盖出错的值或运算符，且 MUST NOT 因表达式包装而偏移到相邻字符或对象 key。

#### Scenario: 属性值类型错误定位到值

- **GIVEN** 对象字面量中某个 value 的类型不匹配
- **WHEN** 语言服务返回该诊断
- **THEN** 诊断范围覆盖该 value 在 WXML 中的原始文本
- **AND** 不覆盖对象 key 或外层花括号

#### Scenario: 包装后位置不偏移

- **GIVEN** 一个以 `{` 开头的多行 mustache 表达式
- **WHEN** 语言服务为其中的成员访问返回诊断或悬停范围
- **THEN** 返回的范围与 WXML 中的原始字符一一对应
- **AND** 换行与缩进不导致范围偏移

### Requirement: 非法表达式仍报告真实语法错误

对对象字面量的解析修正 MUST NOT 掩盖真实的语法错误。缺少逗号、缺少冒号、未闭合花括号等非法输入 SHALL 继续产生语法诊断，并定位到实际出错位置。

#### Scenario: 缺少逗号

- **GIVEN** mustache 中存在 `{{ { a: 1 b: 2 } }}`
- **WHEN** 语言服务执行语法检查
- **THEN** 返回缺少逗号或等价的语法诊断
- **AND** 诊断位置位于 `b` 附近

#### Scenario: 未闭合对象字面量

- **GIVEN** mustache 中存在 `{{ { a: 1 }`
- **WHEN** 语言服务执行语法检查
- **THEN** 返回未闭合或等价的语法诊断
- **AND** 不把该表达式静默当作合法对象

### Requirement: 非对象表达式行为不变

不以 `{` 开头的 mustache 表达式 SHALL 保持现有检查行为。字符串、数字、条件表达式、成员访问和函数调用 MUST NOT 因本次改动新增诊断或改变诊断范围。

#### Scenario: 成员访问保持稳定

- **GIVEN** 模板中存在 `{{ user.name }}`
- **WHEN** 语言服务执行类型检查
- **THEN** 诊断结果与改动前一致
- **AND** 不产生额外的括号或语法诊断

#### Scenario: 条件表达式保持稳定

- **GIVEN** 模板中存在 `{{ visible ? user.name : fallback }}`
- **WHEN** 语言服务执行类型检查
- **THEN** 各分支按原有规则检查
- **AND** 诊断范围仍对应原表达式片段
