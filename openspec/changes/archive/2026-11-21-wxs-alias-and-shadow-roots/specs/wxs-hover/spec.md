# Spec Delta

## ADDED Requirements

### Requirement: 别名导出的 WXS 成员保留函数签名

当 `module.exports` 的成员值是本地标识符别名（如 `{ getLimitSettingText: getLimitSettingText }`）时，导出的成员 SHALL 继承该本地函数的 kind、类型与签名。hover MUST 展示完整签名与参数名，而不是只展示返回类型或 `string`。非函数别名 SHALL 继续按值展示；无法静态解析的别名 MUST 保持 `any` 或既有 Partial 提示，不得伪造签名。

#### Scenario: 函数别名展示参数名

- **GIVEN** WXS 文件定义 `function getLimitSettingText(attendLimitSetting, userAppointLimit) { ... }`
- **AND** 使用 `module.exports = { getLimitSettingText: getLimitSettingText }` 导出
- **WHEN** 用户在模板中 hover `m1.getLimitSettingText`
- **THEN** hover 显示 `(attendLimitSetting: any, userAppointLimit: any) => string`
- **AND** 参数名与 WXS 源码一致

#### Scenario: 值别名仍按值展示

- **GIVEN** WXS 文件定义 `const version = "1.0"` 并以 `{ version: version }` 导出
- **WHEN** 用户 hover `m1.version`
- **THEN** 显示值的类型 `string`
- **AND** 不显示函数签名

#### Scenario: 简写导出行为不变

- **GIVEN** WXS 文件使用 `module.exports = { someFunc }` 简写导出
- **WHEN** 用户 hover `m1.someFunc`
- **THEN** 仍展示本地函数的完整签名

#### Scenario: 未解析别名不伪造签名

- **GIVEN** 别名指向无法静态确定的本地成员
- **WHEN** 用户 hover 该导出成员
- **THEN** 展示保守类型或保留 Partial 提示
- **AND** 不生成虚构的参数列表
