## ADDED Requirements

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
