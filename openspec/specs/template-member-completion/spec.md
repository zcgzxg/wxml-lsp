# template-member-completion Specification

## Purpose

让 mustache 中点号后的成员补全覆盖脚本 `data` 根与 `wx:for` 模板局部变量，并在链式前缀、嵌套循环和同名遮蔽场景下返回与模板作用域一致的候选。补全只替换用户已输入的部分，不破坏前缀表达式。

## Requirements

### Requirement: 循环局部变量支持尾点补全

当光标前的成员访问根是当前作用域内的 `wx:for` 局部变量时，成员补全 SHALL 返回该变量元素类型的可访问成员。默认 `item`、自定义 `wx:for-item` 名称以及嵌套循环中的内层变量都 MUST 按同一规则处理。

#### Scenario: 默认 item 的成员补全

- **GIVEN** 模板位于 `wx:for` 内，元素类型为 `FunctionComponentResult`
- **WHEN** 用户在 `{{ item. }}` 的点号后请求补全
- **THEN** 候选包含 `Name`、`Icon`、`Setting`、`ElementId` 等该类型的成员

#### Scenario: 自定义 wx:for-item

- **GIVEN** 模板使用 `wx:for-item="settingItem"`
- **WHEN** 用户在 `{{ settingItem. }}` 的点号后请求补全
- **THEN** 候选来自 `settingItem` 的元素类型
- **AND** 不要求变量名为 `item`

#### Scenario: 嵌套循环使用内层变量

- **GIVEN** 模板存在外层与内层 `wx:for`，两层都使用默认 `item`
- **WHEN** 用户在内层循环中请求 `{{ item. }}` 补全
- **THEN** 候选来自内层循环的元素类型
- **AND** 不使用外层元素类型

### Requirement: 脚本 data 根的尾点补全保持可用

对脚本 `data` 中声明的根对象，尾点成员补全 SHALL 保持现有行为。新增的模板局部变量解析 MUST NOT 改变 `data` 根的候选集合或替换范围。

#### Scenario: data 对象成员补全

- **GIVEN** 脚本 `data` 中存在 `user: { name: string; age: number }`
- **WHEN** 用户在 `{{ user. }}` 的点号后请求补全
- **THEN** 候选包含 `name` 与 `age`

#### Scenario: data 数组元素补全

- **GIVEN** 脚本 `data` 中存在 `users: User[]`
- **WHEN** 用户在 `{{ users. }}` 的点号后请求补全
- **THEN** 候选包含数组类型自身的成员
- **AND** 不因模板局部变量逻辑返回无关候选

### Requirement: 链式前缀支持成员补全

点号前的前缀 SHALL 允许是完整的成员链，而不只是单个标识符。只要链的第一个根能在当前模板作用域中解析，补全 MUST 基于整条链的类型返回成员。

#### Scenario: 模板局部变量的链式前缀

- **GIVEN** `item.Setting` 的类型为 `ComponentExtSetting`
- **WHEN** 用户在 `{{ item.Setting. }}` 的点号后请求补全
- **THEN** 候选来自 `ComponentExtSetting`
- **AND** 不回退到 `item` 的成员列表

#### Scenario: data 根的链式前缀

- **GIVEN** `user.profile` 的类型为 `Profile`
- **WHEN** 用户在 `{{ user.profile. }}` 的点号后请求补全
- **THEN** 候选来自 `Profile`

### Requirement: 局部变量遮蔽遵循模板作用域

当 `wx:for` 局部变量与脚本 `data` 根同名时，补全 SHALL 使用局部变量的类型。嵌套循环中同名变量 MUST 由内层声明遮蔽外层声明。

#### Scenario: 循环变量遮蔽 data 根

- **GIVEN** 脚本 `data` 中存在 `item` 字段，同时 `wx:for` 也声明了 `item`
- **WHEN** 用户在循环内请求 `{{ item. }}` 补全
- **THEN** 候选来自循环元素类型
- **AND** 不使用 `data.item` 的类型

#### Scenario: 内层循环遮蔽外层循环

- **GIVEN** 外层与内层 `wx:for` 都声明了 `item`，且元素类型不同
- **WHEN** 用户在内层循环中请求 `{{ item. }}` 补全
- **THEN** 候选只来自内层元素类型

### Requirement: 未知根返回明确的空结果

当点号前的根既不是脚本 `data` 根，也不是当前作用域内的模板局部变量时，补全 SHALL 返回空候选，并给出“未知根”或等价的状态说明。系统 MUST NOT 返回其他数据根的成员作为兜底。

#### Scenario: 未知根不返回候选

- **GIVEN** 模板中不存在名为 `missing` 的 data 根或循环变量
- **WHEN** 用户在 `{{ missing. }}` 的点号后请求补全
- **THEN** 返回空候选
- **AND** 状态说明指出该根无法解析

#### Scenario: 拼写错误的局部变量

- **GIVEN** 当前循环变量名为 `settingItem`
- **WHEN** 用户输入 `{{ settingItm. }}` 并请求补全
- **THEN** 返回空候选
- **AND** 不静默使用 `settingItem` 的类型

### Requirement: 补全只替换点号后的输入

成员补全 SHALL 只替换点号后用户已输入的标识符片段，点号前的前缀 MUST 保持不变。应用补全后不得出现重复点号、缺失点号或前缀被改写。

#### Scenario: 替换部分标识符

- **GIVEN** 用户已输入 `{{ item.Na }}` 且光标位于 `Na` 之后
- **WHEN** 用户选择候选 `Name`
- **THEN** 结果为 `{{ item.Name }}`
- **AND** `item.` 前缀保持不变

#### Scenario: 点号后为空

- **GIVEN** 用户只输入了 `{{ item. }}`
- **WHEN** 用户选择候选 `Name`
- **THEN** 结果为 `{{ item.Name }}`
- **AND** 不产生第二个点号
