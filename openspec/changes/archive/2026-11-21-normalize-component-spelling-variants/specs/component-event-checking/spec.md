# Spec Delta

## ADDED Requirements

### Requirement: 事件名 kebab 与 camel 拼写等价

对已声明组件事件，语言服务 SHALL 把绑定中的 kebab 事件名与 camel 事件名视为同一事件，且该等价 MUST 双向成立：声明事件为 `someEvent` 时 `some-event` 必须命中，声明为 `some-event` 时 `someEvent` 必须命中。事件名匹配 MUST 一致应用于未声明事件告警、事件 hover、事件名补全与处理函数参数校验。仅大小写或连字符结构无法归一化的名字 MUST NOT 视为等价。

#### Scenario: 声明 camel 时用 kebab 绑定

- **WHEN** 组件声明事件 `someEvent`，模板写 `<card bind:some-event="onSome" />`
- **THEN** 该事件被视为已声明，不发布未声明事件告警
- **AND** hover 与处理函数参数校验使用 `someEvent` 的 detail 契约

#### Scenario: 声明 kebab 时用 camel 绑定

- **WHEN** 组件声明事件 `'some-event'`，模板写 `<card bindSomeEvent="onSome" />`
- **THEN** 该事件被视为已声明，不发布未声明事件告警

#### Scenario: 未声明事件仍告警

- **WHEN** 组件声明事件 `someEvent`，模板写 `<card bind:someevnt="onSome" />`
- **THEN** 在 `someevnt` token 上发布未声明事件 `Warning`

#### Scenario: kebab 前缀补全 camel 声明

- **WHEN** 组件声明事件 `someEvent`，用户在 `<card bind:some-|>` 触发补全并选择该事件
- **THEN** 结果为 `bind:some-event`，绑定前缀不被改写

#### Scenario: 无声明项目行为不变

- **WHEN** 组件没有任何事件声明
- **THEN** 不新增未声明事件告警，事件诊断与改动前一致

### Requirement: 全部合法事件绑定前缀参与声明检查

语言服务 SHALL 把 `bind`、`catch`、`capture-bind`、`capture-catch`、`mut-bind` 的冒号形式与紧凑形式都识别为事件绑定，并 MUST 让这些形式都参与声明事件补全、未声明事件告警与 hover。至少 MUST 覆盖 `bindxxx`、`bind:xxx`、`bindsome-event`、`bind:some-event`、`bindsomeEvent`、`bind:someEvent`。

#### Scenario: 紧凑前缀加 camel 事件名

- **WHEN** 模板写 `<card bindsomeEvent="onSome" />` 且组件声明事件 `someEvent`
- **THEN** 该绑定按 `someEvent` 检查，不发布未声明事件告警

#### Scenario: 紧凑前缀加 kebab 事件名

- **WHEN** 模板写 `<card bindsome-event="onSome" />` 且组件声明事件 `someEvent`
- **THEN** 该绑定按 `someEvent` 检查，不发布未声明事件告警

#### Scenario: 冒号前缀各绑定族

- **WHEN** 模板分别写 `catch:some-event`、`capture-bind:some-event`、`capture-catch:some-event`、`mut-bind:some-event`
- **THEN** 每个绑定都按声明事件 `someEvent` 检查

#### Scenario: 紧凑前缀补全只替换事件名片段

- **WHEN** 组件声明事件 `someEvent`，用户在 `<card bindsomeE|>` 触发补全并选择该事件
- **THEN** 结果事件名片段补全为 `someEvent`，`bind` 前缀保持原样

#### Scenario: 紧凑前缀未声明事件告警落在事件名

- **WHEN** 模板写 `<card bindsomeevnt="onSome" />` 且组件声明事件 `someEvent`
- **THEN** `Warning` 的范围落在 `someevnt` token 上，而不是 `bind` 前缀

#### Scenario: 动态绑定不告警

- **WHEN** 事件属性为 `bindsomeEvent="{{ handler }}"` 这类动态形式
- **THEN** 不发布未声明事件告警
