# component-type-declarations Specification

## Purpose

让第三方与自定义组件的属性（`props`）拥有可声明、可发现的类型来源：声明文件固定放在 `tsconfig.json` 同级，声明优先于脚本静态推断，并直接参与 WXML 属性绑定的类型检查。

## Requirements

### Requirement: 声明文件位置为 tsconfig 同级约定路径

组件类型声明 SHALL 只从项目 `tsconfig.json` 同级的 `components.d.ts` 读取。该文件缺失、项目没有 `tsconfig.json`，或文件不是项目根内的普通 `.d.ts` 时 MUST NOT 产生组件事实，也不得为此发布诊断。

#### Scenario: 同级声明文件生效
- **WHEN** `tsconfig.json` 同级存在 `components.d.ts`，文件声明了 `WxmlComponentMap`
- **THEN** 该文件的声明参与组件属性与事件检查

#### Scenario: 声明文件缺失无副作用
- **WHEN** `tsconfig.json` 同级没有 `components.d.ts`
- **THEN** 不产生组件事实、不发布诊断、能力状态不变

#### Scenario: 项目没有 tsconfig
- **WHEN** 项目未发现任何 `tsconfig.json`
- **THEN** 不寻找声明文件，行为与改动前一致

#### Scenario: 未保存内容优先
- **WHEN** 用户在编辑器中修改了 `components.d.ts` 但尚未保存
- **THEN** 使用编辑器缓冲区内容解析，不使用磁盘旧内容

#### Scenario: 更深的同名文件不生效
- **WHEN** `tsconfig.json` 同级的子目录里还有另一个 `components.d.ts`
- **THEN** 只读取与 `tsconfig.json` 同级的那个文件

#### Scenario: 声明文件不可读或超限
- **WHEN** 声明的 `components.d.ts` 实际是目录、超出单文件大小预算或解析必然失败
- **THEN** 记录一条可读的降级说明并把能力状态标为 `Partial`，服务不崩溃

### Requirement: 声明格式以全局映射表为入口

声明文件 SHALL 提供 `WxmlComponentMap` 接口，把 WXML 标签名映射到组件声明接口；映射目标接口 MUST 在同一文件内声明，并可包含 `props` 与 `events` 两个段。`WxmlComponentMap` 位于 `declare global` 内或文件顶层时 SHALL 同样生效。未被映射表引用的接口 MUST NOT 生效。

#### Scenario: 映射表引用组件接口
- **WHEN** 声明文件包含 `interface WxmlComponentMap { 'van-button': VanButton }` 与 `interface VanButton { props: { type?: 'primary' | 'default' } }`
- **THEN** 标签 `van-button` 拥有一个可证明类型的 `type` 属性

#### Scenario: 未被引用的接口不生效
- **WHEN** 文件声明了 `interface Orphan { props: { a: string } }` 但 `WxmlComponentMap` 未引用它
- **THEN** 该接口不产生任何组件事实

#### Scenario: 无关声明被忽略
- **WHEN** 声明文件同时包含普通全局类型、工具类型和其他 `declare global` 内容
- **THEN** 组件声明照常解析，无关内容不产生诊断

### Requirement: 声明类型只在可证明时生效

声明解析 SHALL 只接受项目内自包含的类型表达：基础类型、字符串/数字/布尔字面量与联合、数组、`Record<string, T>`、嵌套对象字面量。无法在本文件内解析的标识符、外部导入、泛型与复杂条件类型 MUST 标记为未证明，且 MUST NOT 产生类型诊断。

#### Scenario: 可证明的类型参与检查
- **WHEN** 属性声明为 `'small' | 'large'` 且模板绑定 `size="medium"`
- **THEN** 发布属性类型不匹配的诊断

#### Scenario: 未证明的类型不产生诊断
- **WHEN** 属性声明引用了本文件之外的类型（如 `SomeLib.Options`）
- **THEN** 该属性不参与类型诊断，能力状态为 `Partial` 并给出原因

#### Scenario: 声明文件解析失败
- **WHEN** 声明文件包含本解析子集之外的语法，导致某组件声明不完整
- **THEN** 受影响组件标记为不完整，其他组件照常生效，且整体状态为 `Partial`

#### Scenario: 映射表内重复声明同名标签
- **WHEN** `WxmlComponentMap` 内把同一标签映射了两次
- **THEN** 使用第一个映射，并记录一条可读的冲突说明

### Requirement: 属性绑定按声明检查

当组件拥有声明事实时，模板中该组件的属性绑定 SHALL 按声明检查：未在 `props` 中声明的属性 MUST 报错；声明为可选（`?`）的属性 MUST 允许 `undefined`；可证明类型与绑定表达式不兼容时 MUST 报错，诊断范围 MUST 落在绑定表达式而非属性名。

#### Scenario: 未声明属性报错
- **WHEN** 组件声明只有 `type` 属性，模板写 `<van-button size="small">`
- **THEN** 在 `size` 属性上发布未声明属性诊断

#### Scenario: 绑定类型不匹配
- **WHEN** 属性声明为 `boolean`，模板绑定 `loading="{{ 'yes' }}"`
- **THEN** 发布类型不匹配诊断，范围覆盖 `{{ 'yes' }}`

#### Scenario: 可选属性允许缺省
- **WHEN** 属性声明为 `loading?: boolean` 且模板未写该属性
- **THEN** 不发布任何诊断

### Requirement: 声明优先于脚本静态推断

当同一标签同时存在声明事实与脚本 `properties` 推断时，声明事实 SHALL 覆盖脚本推断结果，且该覆盖 MUST 在脚本 overlay 变更后保持稳定。声明缺失或不完整时 SHALL 回落到脚本推断。

#### Scenario: 冲突时取声明
- **WHEN** 声明把 `type` 定义为 `string`，脚本 `properties` 把它定义为 `Boolean`
- **THEN** 属性检查与 hover 使用声明中的 `string`

#### Scenario: overlay 不覆盖声明
- **WHEN** 组件已经使用声明事实，用户改动脚本 `properties` 但未保存
- **THEN** 组件事实仍来自声明，不随 overlay 变化

#### Scenario: 无声明时回落到脚本
- **WHEN** 标签没有任何声明映射
- **THEN** 属性事实继续来自脚本 `properties`，行为与本次改动前一致
