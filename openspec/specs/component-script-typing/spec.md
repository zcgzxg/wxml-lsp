# component-script-typing Specification

## Purpose

TBD - created by archiving change fix-behavior-component-typing-and-declaration-docs. Update Purpose after archive.

## Requirements

### Requirement: behavior 属性事实经 tsconfig paths 解析

组件脚本分析 SHALL 使用项目 `tsconfig.json` 的 `paths`/`baseUrl` 解析 `behaviors` 中的模块说明符，与事件 definition 使用同一套解析规则。通过别名或相对路径引入的 behavior MUST 参与属性事实合并；解析成功时该 behavior 贡献的 `properties` MUST 出现在组件属性检查与 hover 中。

#### Scenario: 别名 behavior 的属性参与校验
- **WHEN** 组件 `behaviors: [Base]`，且 `Base` 通过 `@/behaviors/base` 引入，`Base.properties` 声明 `count: Number`
- **THEN** 模板写 `count="x"` 时发布类型不匹配诊断

#### Scenario: 相对路径 behavior 的属性参与校验
- **WHEN** 组件通过 `./behavior` 相对路径引入 behavior，behavior 声明了属性
- **THEN** 该属性按 behavior 声明的类型参与检查

#### Scenario: 组件自身属性覆盖 behavior
- **WHEN** 组件自身 `properties` 与 behavior 声明了同名属性
- **THEN** 属性类型取组件自身声明，行为与既有合并优先级一致

### Requirement: behavior 不可读时回退到自身属性

当 behavior 事实不完整、但组件自身的 `properties` 字面量可以完整枚举时，语言服务 SHALL 继续用自身属性做类型检查，而不是跳过整个组件。此时 MUST NOT 把未声明属性当作错误，MUST 把能力状态标为 `Partial` 并给出可读原因。

#### Scenario: behavior 解析失败仍检查自身属性
- **WHEN** behavior 的模块无法解析，组件自身声明了 `title: String`
- **THEN** `title="x"` 按 `string` 检查
- **AND** 模板中的其他未声明属性不报错
- **AND** 能力状态为 `Partial` 并说明 behavior 属性不完整

#### Scenario: 自身属性含 spread 时整体降级
- **WHEN** 组件自身 `properties` 使用 spread 或非字面量，无法枚举成员
- **THEN** 该组件不参与属性检查，状态为 `Partial`，不产生猜测性诊断

#### Scenario: 声明优先关系不变
- **WHEN** 组件同时拥有完整类型声明与不完整的 behavior 事实
- **THEN** 属性检查仍以声明为准，回退逻辑不生效
