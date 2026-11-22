# Spec Delta

## MODIFIED Requirements

### Requirement: 已标注处理函数按 detail 校验

当处理函数存在、其事件参数已被显式标注类型、且事件的 `detail` 类型可解析时，语言服务 SHALL 用类型引擎按函数参数可赋值性判定兼容性。判定 MUST 允许参数宽化、对象额外字段、必填到可选与字面量到基础类型；MUST NOT 要求结构成员数量或联合成员数量相等。无参数处理函数 MUST 视为兼容；多出必填参数 MUST 视为不兼容。不兼容时 MUST 发布 `Error` 级别诊断，范围落在处理函数名 token。

#### Scenario: 参数类型不兼容
- **WHEN** 事件 detail 为 `{ index: number }`，处理函数声明为 `(e: WechatMiniprogram.CustomEvent<{ name: string }>) => void`
- **THEN** 在处理函数名 token 上发布 `Error`

#### Scenario: 参数类型兼容
- **WHEN** 处理函数参数标注与 detail 契约兼容
- **THEN** 不发布诊断

#### Scenario: 参数宽化通过
- **WHEN** 事件 detail 为 `{ value: number }`，处理函数接受 `{ value: number | string }`
- **THEN** 不发布诊断

#### Scenario: 对象额外字段通过
- **WHEN** 事件 detail 为 `{ index: number; label: string }`，处理函数只要求 `{ index: number }`
- **THEN** 不发布诊断

#### Scenario: 必填字段传给可选字段通过
- **WHEN** 事件 detail 为 `{ index: number }`，处理函数声明 `{ index?: number }`
- **THEN** 不发布诊断

#### Scenario: 字面量传给基础类型通过
- **WHEN** 事件 detail 为 `{ value: 'ready' }`，处理函数声明 `{ value: string }`
- **THEN** 不发布诊断

#### Scenario: unknown detail 交给严格函数检查
- **WHEN** 事件 detail 为 `{ value: unknown }`，处理函数为函数表达式或箭头函数属性并要求 `{ value: number }`，且项目启用 `strictFunctionTypes`（方法简写按 TypeScript 方法双变规则处理，不在本场景内）
- **THEN** 在处理函数名 token 上发布 `Error`

#### Scenario: 额外必填参数不兼容
- **WHEN** 处理函数为 `(e: CustomEvent<{ value: number }>, extra: string) => void`
- **THEN** 在处理函数名 token 上发布 `Error`

#### Scenario: 无参数处理函数兼容
- **WHEN** 处理函数声明为 `onChange() {}`，事件带有 detail
- **THEN** 不发布诊断

#### Scenario: 参数未标注
- **WHEN** 处理函数为 `onChange(e) {}`（参数无类型标注）
- **THEN** 不发布诊断，能力状态为 `Partial` 并给出原因

#### Scenario: 处理函数缺失优先报缺失
- **WHEN** 绑定的处理函数在页面或组件中不存在
- **THEN** 发布处理函数缺失的 `Error`，不再叠加参数类型诊断

#### Scenario: 检查遵循项目编译选项
- **WHEN** 项目关闭 `strictFunctionTypes`，处理函数参数比 detail 更宽
- **THEN** 按项目编译选项判定，不额外施加更严格的函数参数检查

#### Scenario: 生成代码错误不贴处理函数
- **WHEN** 事件契约检查生成的赋值或类型注解本身无法解析
- **THEN** 不发布处理函数参数诊断，能力状态为 `Partial` 并说明原因
