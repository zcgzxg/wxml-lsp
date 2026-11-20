# Proposal

## Why

小程序 WXML 对自定义组件属性与事件允许多种等价拼写：属性可用 `some-prop="{{ a }}"` 或 `someProp="{{ a }}"`；事件绑定前缀 `bind` / `catch` / `capture-bind` / `capture-catch` / `mut-bind` 都可带冒号或不带冒号，事件名本身也可用 kebab（`some-event`）或 camel（`someEvent`）书写。当前语言服务各消费点规则不一致：属性比对只在部分路径做了单向 camel→kebab 映射，事件名仍是精确字符串匹配，导致合法写法被误报为未声明属性/事件，补全、hover 与 definition 也可能落空。

## What Changes

- 新增统一的组件成员名比较规则：声明名与模板写法按 kebab↔camel 双向归一化后比较，属性与事件共用，作为唯一权威。
- 事件绑定支持全部合法形式：`bindxxx`、`bind:xxx`、`bindsome-event`、`bind:some-event`、`bindsomeEvent`、`bind:someEvent`，以及 `catch` / `capture-bind` / `capture-catch` / `mut-bind` 的冒号与紧凑前缀。
- 事件名检查、事件 hover、事件补全按归一化匹配；事件补全覆盖紧凑前缀，并按用户已输入的拼写风格（kebab 或 camel）生成插入文本。
- 属性 hover、definition、inlay hint、字面量取值 hover、属性补全与属性诊断统一使用同一比较规则，声明为 kebab 时用 camel 写、声明为 camel 时用 kebab 写都能命中。
- 没有组件声明或声明不完整时保持现有行为，不新增诊断，不降低既有能力状态语义。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `component-type-declarations`: 属性绑定检查与 discovery 要求补充“属性名 kebab/camel 等价”这一可观察行为。
- `component-event-checking`: 事件名补全、未声明事件告警、事件 hover 与处理函数参数校验要求补充“事件名 kebab/camel 等价”和全部合法绑定前缀。

## Impact

- `crates/wxml-project/src/typing.rs`：新增共享的成员名归一化比较与声明查找辅助方法。
- `crates/wxml-lsp/src/`：`semantics/target.rs`、`semantics/hints.rs`、`semantics/event_diagnostics.rs`、`metadata.rs`、`metadata/docs.rs`、`completion/static_items.rs`、`backend_tsgo/event_completion.rs`、`backend_tsgo/component_bindings.rs`。
- 测试：相关模块单元测试与 `tests/fixtures/components-typings` 覆盖的集成路径；无新增依赖，无协议或配置变更。
