# component-event-checking Specification

## Purpose

让自定义组件的事件契约（事件名与 `detail` 类型）成为 WXML 事件绑定的检查依据：补全事件名、对未声明事件名给出警告、在 hover 中展示处理函数签名，并在可证明时校验处理函数参数类型。

## Requirements

### Requirement: 事件名补全来自声明

对于拥有完整事件声明的自定义组件，`bind:`、`catch:`、`capture-bind:`、`capture-catch:`、`mut-bind:` 之后的事件名补全 SHALL 返回声明中的事件名。补全 MUST 只替换用户已输入的事件名片段，不得改写绑定前缀。

#### Scenario: 补全返回声明事件
- **WHEN** 组件声明了 `change` 与 `input` 事件，用户在 `<van-field bind:|>` 触发补全
- **THEN** 候选包含 `change` 与 `input`

#### Scenario: 只替换已输入片段
- **WHEN** 用户已输入 `bind:cha` 后触发补全并选择 `change`
- **THEN** 结果为 `bind:change`，前缀与其他属性不受影响

#### Scenario: 无声明时保持现状
- **WHEN** 组件没有任何事件声明
- **THEN** 不返回来自声明的候选，行为与本次改动前一致

### Requirement: 未声明事件名报 Warning

当组件声明完整（可解析）且包含 `events` 段时，绑定一个未声明的事件名 SHALL 发布 `Warning` 级别的诊断，范围 MUST 落在事件名 token 上。声明不完整、没有 `events` 段、动态绑定或声明缺失时 MUST NOT 发布该诊断。

#### Scenario: 未声明事件名告警
- **WHEN** 组件声明了 `change`，模板写 `<van-field bind:chaneg="onInput">`
- **THEN** 在 `chaneg` token 上发布 `Warning`

#### Scenario: 已声明事件不告警
- **WHEN** 模板绑定的事件名在声明中存在
- **THEN** 不发布未知事件名诊断

#### Scenario: 声明不完整时不告警
- **WHEN** 该组件的事件声明被标记为不完整
- **THEN** 不发布未知事件名诊断，状态为 `Partial`

#### Scenario: 动态绑定不告警
- **WHEN** 事件属性为 `bind:change="{{ name }}"` 这类动态形式
- **THEN** 不发布未知事件名诊断

### Requirement: 事件 hover 展示 detail 契约

hover 事件属性名时，语言服务 SHALL 展示期望的处理函数签名，并把声明中的 `detail` 类型原文写入 `WechatMiniprogram.CustomEvent<T>`。`detail` 不可证明时 MUST 明确标注为未证明，不得伪造类型。

#### Scenario: 展示带 detail 的签名
- **WHEN** 事件声明为 `change: { detail: { index: number } }`，用户 hover `bind:change`
- **THEN** hover 展示 `(e: WechatMiniprogram.CustomEvent<{ index: number }>) => void`

#### Scenario: detail 未证明
- **WHEN** 事件声明引用了无法解析的类型
- **THEN** hover 明确标注该 `detail` 未证明，不展示猜测类型

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

### Requirement: 既有事件检查不回归

本次改动 MUST NOT 改变处理函数存在性检查、definition 的目标语义、处理函数补全与事件名补全的现有行为。没有组件声明的项目 MUST 保持与改动前完全一致的事件诊断结果。处理函数 hover 的内容与覆盖范围改为由原生类型引擎提供，见“处理函数 hover 与 definition 来自原生类型”。

#### Scenario: 无声明项目结果不变
- **WHEN** 项目中没有任何组件类型声明
- **THEN** 事件绑定只保留处理函数存在性等现有检查，诊断集合不变

#### Scenario: definition 仍指向脚本方法
- **WHEN** 用户在事件属性值上执行 definition
- **THEN** 仍然跳转到脚本中的处理函数名 token

#### Scenario: 处理函数补全不变
- **WHEN** 用户在事件属性值上触发处理函数补全
- **THEN** 候选仍来自项目的静态方法表，本改动不改变候选集合

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

### Requirement: 处理函数 hover 与 definition 来自原生类型

事件属性值中的静态处理函数名 hover SHALL 由原生类型引擎给出处理函数的类型与 JSDoc，definition SHALL 指向该处理函数在脚本中的声明。查询 MUST 复用模板表达式的 shadow 注入与位置映射，不得依赖静态行为图；hover 范围 MUST 落在 WXML 的处理函数名 token 上。

#### Scenario: 别名 behavior 的处理函数可 hover
- **WHEN** 组件通过 tsconfig `paths` 别名引入 behavior，behavior 中定义了带 JSDoc 的 `onBackTap`，模板写 `bind:click-left="onBackTap"`
- **THEN** hover 返回 `onBackTap` 的类型与 JSDoc
- **AND** definition 跳转到 behavior 脚本中的 `onBackTap` 声明

#### Scenario: 相对路径 behavior 的处理函数可 hover
- **WHEN** 组件通过相对路径引入 behavior 并绑定其处理函数
- **THEN** hover 返回该处理函数的类型与 JSDoc
- **AND** definition 跳转到 behavior 脚本中的声明

#### Scenario: Page 自身方法可 hover
- **WHEN** `Page({ onRefresh() {} })` 中定义处理函数并在模板绑定
- **THEN** hover 返回该方法的类型，definition 跳转到页面脚本中的声明

#### Scenario: Page 继承的 behavior 方法明确降级
- **WHEN** 处理函数只定义在 `Page` 的 behavior 中，页面自身没有同名方法
- **THEN** 语言服务不返回静态签名
- **AND** hover/definition 返回空结果，不伪造位置或类型

#### Scenario: 无引擎时明确降级
- **WHEN** 原生类型引擎不可用（HostOnly）
- **THEN** 事件处理函数 hover/definition 返回空结果
- **AND** 不返回来自静态方法表的签名或位置

#### Scenario: 动态处理函数不伪造结果
- **WHEN** 事件属性值为 `{{ handler }}` 等非静态标识符
- **THEN** 不按静态处理函数解析
- **AND** hover/definition 返回空结果或沿用表达式 hover 的既有行为

### Requirement: 事件 hover 展示声明说明

组件声明解析 SHALL 读取 `events` 成员前的 `/** ... */` 注释，并在事件 hover 中于处理函数签名之后展示该说明。说明缺失时 MUST NOT 伪造文字；说明 MUST 不改变 detail 类型与登记为未证明的判定。

#### Scenario: 事件 hover 展示说明
- **WHEN** 声明写 `/** 选项变化 */ change: { detail: { index: number } }`
- **THEN** hover `bind:change` 时展示签名与“选项变化”

#### Scenario: detail 未证明同时展示说明
- **WHEN** 事件 detail 引用外部类型、hover 标注未证明
- **THEN** 说明文字仍然展示，两者互不覆盖

#### Scenario: 没有说明不伪造
- **WHEN** 事件成员前没有 `/** ... */`
- **THEN** hover 只展示签名与既有说明，不出现空描述
