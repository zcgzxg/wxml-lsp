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

当处理函数存在、其事件参数已被显式标注类型、且事件的 `detail` 类型可证明时，语言服务 SHALL 校验两者兼容性；引擎能证明不兼容时 MUST 发布 `Error` 级别诊断，范围落在处理函数名 token 上。

#### Scenario: 参数类型不兼容
- **WHEN** 事件 detail 为 `{ index: number }`，处理函数声明为 `(e: WechatMiniprogram.CustomEvent<{ name: string }>) => void`
- **THEN** 在处理函数名 token 上发布 `Error`

#### Scenario: 参数类型兼容
- **WHEN** 处理函数参数标注与 detail 契约兼容
- **THEN** 不发布诊断

#### Scenario: 参数未标注
- **WHEN** 处理函数为 `onChange(e) {}`（参数无类型标注）
- **THEN** 不发布诊断，能力状态为 `Partial` 并给出原因

#### Scenario: 处理函数缺失优先报缺失
- **WHEN** 绑定的处理函数在页面或组件中不存在
- **THEN** 发布处理函数缺失的 `Error`，不再叠加参数类型诊断

### Requirement: 既有事件检查不回归

本次改动 MUST NOT 改变处理函数存在性检查、definition、handler hover 与处理函数补全的现有行为。没有组件声明的项目 MUST 保持与改动前完全一致的事件诊断结果。

#### Scenario: 无声明项目结果不变
- **WHEN** 项目中没有任何组件类型声明
- **THEN** 事件绑定只保留处理函数存在性等现有检查，诊断集合不变

#### Scenario: definition 仍指向脚本方法
- **WHEN** 用户在事件属性值上执行 definition
- **THEN** 仍然跳转到脚本中的处理函数名 token
