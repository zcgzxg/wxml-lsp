# Spec Delta

## MODIFIED Requirements

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

## ADDED Requirements

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
