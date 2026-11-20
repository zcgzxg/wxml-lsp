# Spec Delta

## ADDED Requirements

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
