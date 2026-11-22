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

### Requirement: 组件属性名 kebab 与 camel 拼写等价

对已声明的组件属性，语言服务 SHALL 把模板中的 kebab 写法与 camel 写法视为同一属性，且该等价 MUST 双向成立：声明名为 camel 时可用 kebab 引用，声明名为 kebab 时也可用 camel 引用。该等价 MUST 一致应用于属性诊断、属性 hover、definition、inlay hint、字面量取值 hover 与属性补全。仅大小写或连字符结构无法归一化的名字（例如 `someprop` 与 `someProp`）MUST NOT 视为等价。

#### Scenario: 声明 camel 时用 kebab 引用

- **WHEN** 组件声明属性 `cardTitle?: 'small' | 'large'`，模板写 `<item card-title="medium" />`
- **THEN** 该属性被视为已声明，并按 `cardTitle` 的声明类型发布取值诊断
- **AND** 不发布未声明属性诊断

#### Scenario: 声明 kebab 时用 camel 引用

- **WHEN** 组件声明属性 `'card-title'?: string`，模板写 `<item cardTitle="x" />`
- **THEN** 该属性被视为已声明，属性绑定检查正常进行

#### Scenario: kebab hover 命中 camel 声明

- **WHEN** 组件声明属性 `cardTitle`，用户 hover `card-title`
- **THEN** hover 展示 `cardTitle` 的声明类型

#### Scenario: kebab definition 命中 camel 声明

- **WHEN** 组件声明属性 `cardTitle`，用户对 `card-title` 执行 definition
- **THEN** 跳转到脚本中 `cardTitle` 的声明位置

#### Scenario: 补全按输入风格插入

- **WHEN** 组件声明属性 `cardTitle`，用户输入 `card-` 后触发属性补全并选择该属性
- **THEN** 插入的是 kebab 写法 `card-title`
- **AND** 用户输入 `cardT` 时插入 camel 写法 `cardTitle`

#### Scenario: 非等价拼写不命中

- **WHEN** 组件声明属性 `cardTitle`，模板写 `cardtitle="x"`
- **THEN** 不把该属性视为 `cardTitle`，按既有未声明属性路径处理

#### Scenario: inlay hint 覆盖两种拼写

- **WHEN** 组件声明属性 `cardTitle`，模板以 `card-title` 绑定
- **THEN** 该属性仍产生声明类型 inlay hint

### Requirement: 声明成员的 JSDoc 说明进入 hover

组件声明解析 SHALL 读取 `props` 成员前的 `/** ... */` 注释，并把其中的说明文本随该属性事实保留。属性 hover MUST 在类型与声明位置之后展示该说明；多行注释 MUST 折叠为单行可读文本。成员没有注释时 MUST NOT 伪造说明。

#### Scenario: 属性 hover 展示声明说明
- **WHEN** 声明写 `/** 按钮类型 */ type?: 'primary' | 'default'`
- **THEN** hover `type` 时展示类型、声明位置和“按钮类型”

#### Scenario: 多行注释折叠
- **WHEN** 成员前是多行 `/** ... */` 注释
- **THEN** hover 中说明为单行文本，不保留星号与换行

#### Scenario: 没有注释不伪造
- **WHEN** 声明成员前没有 `/** ... */`
- **THEN** hover 只展示类型与声明位置，不出现空说明或占位文字

#### Scenario: 说明不参与类型判定
- **WHEN** 成员说明包含反引号、尖括号或中文标点
- **THEN** 说明按普通文本渲染，不影响该成员的类型诊断结果

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
