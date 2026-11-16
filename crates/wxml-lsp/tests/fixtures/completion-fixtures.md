# 补全样本说明

本目录的 `completion-*` 是 `wxml-lsp` 补全功能的样本，行为基线见 [功能基线 §3.8](../../../../docs/reference/wxml-language-feature-baseline.md#38-补全目录现状wave-5)。目录不参与 `wxml-fixtures` 的 manifest 校验，也不属于 AC-001..AC-013。

## 1. completion-basic

静态补全站点清单，不依赖 Node。

| 文件 | 内容 | 支持的补全 |
| --- | --- | --- |
| `index.wxml` | `<vi` | 标签名：`view` 等基础组件 |
| | `<view cla` | 属性名：`class`（全局属性）及 `view` 专属属性 |
| | `<button type="` | 引号内枚举值：`default` `primary` `warn` |
| | `<picker mode="` | 引号内枚举值：`selector` `multiSelector` `time` `date` `region` |
| | `<view wx:` | 指令：`wx:if` `wx:for` 等 |
| | `<view bind` | 事件属性：`bindtap`、`catchtap`、`bind:tap` 等 |
| | `</` | 闭合标签：仅当前未闭合元素栈（`view`） |
| `index.js` | `Page({ data: { user, items }, onTap })` | 成员补全（`user.na`）与事件处理函数补全的数据源 |
| `index.json` | 空 `usingComponents` | 无自定义组件候选 |

该样本目前用于编辑器内手动检查；对应的自动化断言以 `crates/wxml-lsp/tests/lsp.rs` 中的内联文本为准。

## 2. completion-context

反向站点样本：验证这些位置**不产生**补全。

| 文件 | 内容 | 预期 |
| --- | --- | --- |
| `index.wxml` | 裸文本 `plain text has no completion` | 无补全 |
| | `<!-- <vi -->` | HTML 注释内无补全 |
| | `<wxs module="m">var value = 1</wxs>` | WXS 元素体内无补全 |
| | `{{ user.na }}` | Mustache 成员补全（需要类型后端） |
| | `bindtap="on"` | 事件处理函数值补全（需要类型后端） |
| `index.js` | `Page({ data: { user: { name } }, onTap })` | 上述两条后端补全的数据源 |

## 3. completion-components

自定义组件样本，同时覆盖 `usingComponents` 解析与降级。

| 文件 | 内容 | 支持的补全 |
| --- | --- | --- |
| `app.json` | `pages: ["index"]`、空全局 `usingComponents` | 页面发现 |
| `index.json` | `my-card` → `/components/my-card/index`；`local-badge` → `./components/badge`（目标不存在） | 组件标签候选；未解析目标仍进入候选，`detail` 降级为 specifier |
| `index.wxml` | `<my-card` | 组件标签名候选 |
| | `<my-card title="hello" />` | 已完成的组件标签（不作为补全站点，仅作上下文） |
| `components/my-card/index.js` | `Component({ properties: { title } })` | 组件声明，供类型后端布局；静态补全不读取其属性 |
| `components/my-card/index.json` | `{ "component": true }` | 组件归属识别 |
| `components/my-card/index.wxml` | `<view><slot /></view>` | 组件自身模板；当前不列出其内部自定义组件 |
| `index.js` | `Page({ data: {} })` | 页面注册 |

引用该样本的自动化测试：

- `crates/wxml-lsp/tests/lsp.rs` 的 `custom_components_complete_from_using_components`：`<my` 必须给出 `my-card`。
- `crates/wxml-lsp/src/project_host.rs` 的 `using_components_are_exposed_for_a_page`：页面可见组件为 `local-badge`、`my-card`，且至少一个已解析到目标文件。
