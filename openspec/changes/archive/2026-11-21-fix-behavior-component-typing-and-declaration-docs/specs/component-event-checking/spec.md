## ADDED Requirements

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
