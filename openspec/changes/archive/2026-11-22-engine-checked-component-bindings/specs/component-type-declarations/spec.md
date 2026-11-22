# Spec Delta

## MODIFIED Requirements

### Requirement: 声明类型只在可证明时生效

声明解析 SHALL 识别 `props`/`events` 成员及其原始类型表达式。成员类型能否参与检查 MUST 由项目类型上下文能否解析该声明决定：Rust 解析器只负责读取成员名、可选性与类型原文，MUST NOT 因自身无法建模泛型、导入或条件类型而阻止引擎检查。类型无法解析、声明文件不可读或成员结构不完整时 MUST NOT 产生类型诊断，能力状态为 `Partial` 并给出原因。

#### Scenario: 可证明的类型参与检查
- **WHEN** 属性声明为 `'small' | 'large'` 且模板绑定 `size="medium"`
- **THEN** 发布属性类型不匹配的诊断

#### Scenario: 外部导入类型可解析时参与检查
- **WHEN** 属性声明引用本文件外但项目内可解析的类型，模板绑定了不兼容的值
- **THEN** 按该声明的真实类型发布诊断

#### Scenario: 泛型声明可解析时参与检查
- **WHEN** 属性声明为 `Box<number>` 且模板绑定 `{ id: 'wrong' }`
- **THEN** 发布属性类型不匹配的诊断

#### Scenario: 未证明的类型不产生诊断
- **WHEN** 属性声明引用了项目类型上下文也无法解析的类型
- **THEN** 该属性不参与类型诊断，能力状态为 `Partial` 并给出原因

#### Scenario: 声明文件解析失败
- **WHEN** 声明文件包含本解析子集之外的语法，导致某组件声明不完整
- **THEN** 受影响组件标记为不完整，其他组件照常生效，且整体状态为 `Partial`

#### Scenario: 映射表内重复声明同名标签
- **WHEN** `WxmlComponentMap` 内把同一标签映射了两次
- **THEN** 使用第一个映射，并记录一条可读的冲突说明

### Requirement: 属性绑定按声明检查

当组件拥有声明事实时，模板中该组件的每个属性绑定 SHALL 按声明检查：未在 `props` 中声明的属性 MUST 报错；可选属性 MUST 允许 `undefined`；纯 mustache 表达式的兼容性 MUST 由类型引擎按声明类型的可赋值性判定，不得降级为标识符/成员链或数组/对象大类比较。不兼容时 MUST 发布诊断，范围落在绑定表达式。字面量、空属性与混合文本 MUST 继续按 WXML 值形态规则检查。

#### Scenario: 未声明属性报错
- **WHEN** 组件声明只有 `type` 属性，模板写 `<van-button size="small">`
- **THEN** 在 `size` 属性上发布未声明属性诊断

#### Scenario: 绑定类型不匹配
- **WHEN** 属性声明为 `boolean`，模板绑定 `loading="{{ 'yes' }}"`
- **THEN** 发布类型不匹配诊断，范围覆盖 `{{ 'yes' }}`

#### Scenario: 可选属性允许缺省
- **WHEN** 属性声明为 `loading?: boolean` 且模板未写该属性
- **THEN** 不发布任何诊断

#### Scenario: 联合类型不匹配
- **WHEN** 属性声明为 `string | number`，模板绑定布尔表达式
- **THEN** 发布类型不匹配诊断，范围覆盖绑定表达式

#### Scenario: 数组元素类型不匹配
- **WHEN** 属性声明为 `string[]`，模板绑定 `number[]` 表达式
- **THEN** 发布类型不匹配诊断，而不是只判定“是数组”

#### Scenario: 对象字段类型不匹配
- **WHEN** 属性声明为 `{ id: number }`，绑定对象的 `id` 为 `string`
- **THEN** 发布类型不匹配诊断

#### Scenario: 结构兼容的额外字段通过
- **WHEN** 属性声明为 `{ id: number }`，绑定对象为 `{ id: number; extra: string }`
- **THEN** 不发布类型诊断

#### Scenario: 复杂表达式参与检查
- **WHEN** 属性绑定为条件、索引、算术或函数调用表达式，且结果类型与声明不兼容
- **THEN** 发布类型不匹配诊断，范围覆盖该表达式

#### Scenario: 泛型与导入类型按真实类型检查
- **WHEN** 属性声明为可解析的泛型或导入类型，绑定值与实例化结果不兼容
- **THEN** 发布类型不匹配诊断

#### Scenario: 任意类型目标跳过检查
- **WHEN** 属性声明为 `any`
- **THEN** 不发布类型诊断，能力状态不变

#### Scenario: 无法判定的目标降级
- **WHEN** 属性声明为 `unknown`、`never` 或无法解析的类型
- **THEN** 不发布类型诊断，能力状态为 `Partial` 并给出原因

#### Scenario: 字面量与混合文本保持 WXML 规则
- **WHEN** 属性值是字符串字面量、空属性或混合文本
- **THEN** 按现有 WXML 值形态规则检查，不进入引擎赋值路径

## ADDED Requirements

### Requirement: 脚本属性绑定按规范化类型检查

当属性事实来自脚本 `properties` 时，纯 mustache 绑定的兼容性 SHALL 按 `type` 与 `optionalTypes` 规范化后的目标类型判定。语言服务 MUST NOT 直接使用组件实例 `this.properties` 的推导结果作为目标类型，因为其中可能遗漏 `optionalTypes`。数组、对象、联合与函数属性 MUST 按引擎可赋值性判定，而不是只比较大类。

#### Scenario: optionalTypes 联合生效
- **WHEN** 脚本属性声明 `{ type: String, optionalTypes: [Number] }`，模板绑定布尔表达式
- **THEN** 发布类型不匹配诊断

#### Scenario: 数组属性拒绝非数组值
- **WHEN** 脚本属性声明 `{ type: Array }`，模板绑定字符串表达式
- **THEN** 发布类型不匹配诊断

#### Scenario: 函数属性可判定
- **WHEN** 脚本属性声明 `{ type: Function }`，模板绑定非函数值
- **THEN** 发布类型不匹配诊断

#### Scenario: 不可读的构造器降级
- **WHEN** `properties` 使用无法规范化的构造器或动态表达式
- **THEN** 该属性不参与类型诊断，能力状态为 `Partial` 并给出原因
- **AND** 组件自身可完整枚举的其他属性仍照常检查

### Requirement: 组件绑定诊断必须可归属于绑定表达式

组件属性契约检查产生的引擎诊断 SHALL 只在该诊断可归属于对应绑定时发布。引擎报告在契约探针（包括合成变量名）上的赋值错误 MAY 重锚到绑定表达式；落在期望类型注解、无法归属的生成声明或帮助类型上的错误 MUST NOT 作为模板诊断发布；无法归属时 MUST 保持 `Partial` 并说明原因。同一次绑定不兼容 MUST NOT 同时出现手工比较和引擎比较两条诊断。

#### Scenario: 合成变量名上的错误归属回绑定
- **WHEN** 引擎在生成的赋值检查中报告绑定表达式与声明不兼容
- **THEN** 诊断范围落在 WXML 的绑定表达式上，不落在合成变量名上

#### Scenario: 生成代码自身的错误不外溢
- **WHEN** 生成的检查声明或期望类型本身无法解析
- **THEN** 不发布模板诊断，能力状态为 `Partial` 并说明原因

#### Scenario: 同一绑定只发布一条诊断
- **WHEN** 属性绑定不兼容
- **THEN** 该绑定只出现一条类型不匹配诊断
