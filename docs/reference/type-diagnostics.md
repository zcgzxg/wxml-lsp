# 模板类型诊断策略

更新日期：2026-11-16。

本文件规定 WXML 模板在什么情况下产生诊断、什么情况下必须降级、hover 的类型与说明如何取舍，以及诊断落在哪个 token 上。它是实现目标，不代表功能已实现；实际进度与证据仍由[项目 README](../../README.md) 维护。契约定义见 [C0 基础契约](contracts.md)。

## 1. 原则

1. **只发布可归因于模板的结果。** 诊断必须映射回 WXML 原文中的具体 token；无法安全映射时宁可不发布，也不贴到最近字符。
2. **不伪造类型。** 缺少类型来源、`any`、桥接失败时如实返回 `TypeQuality` 与 `CapabilityStatus`，不用空诊断集合表示“检查通过”。
3. **不照搬 TS 规则。** 模板有运行时转换和绑定语义；只有当模板语义与 JS/TS 语义一致时才直接复用后端诊断。
4. **正例与反例同等重要。** 每个规则都要有“不误报”的正例和“必须报”的反例。
5. **降级可见。** `Partial`/`Unsupported`/`Disabled`/`Unavailable`/`Unknown` 必须带原因并随结果返回。

## 2. 状态与类型质量

`CapabilityStatus` 与 `TypeQuality` 分别回答不同问题，不能互相替代：

| `CapabilityStatus` | 含义 | 展示规则 |
| --- | --- | --- |
| `Supported` | 已按样本范围检查 | 可展示具体结果 |
| `Partial` | 部分输入可用 | 展示结果并标注限制 |
| `Unsupported` | 语言形式不支持 | 不展示猜测的类型或“无错误” |
| `Disabled` | 策略关闭 | 明确说明检查关闭 |
| `Unavailable` | 后端不可用 | 说明暂不可用，不发布空诊断 |
| `Unknown` | 未确认 | 不声称检查通过 |

| `TypeQuality` | 含义 | hover 展示 |
| --- | --- | --- |
| `Known` | 具体类型 | 实际类型 + 字段说明 |
| `Any` | 显式 `any` | 显示 `any`，不伪造成员说明 |
| `Unknown` | `unknown`、未分析、根不可用 | 显示 `unknown` |
| `Missing` | 属性/符号不存在 | 不展示具体类型 |

规则：

- `DiagnosticsResult.diagnostics` 为空**只有**在 `status == Supported` 时才表示“干净”；其他状态必须显示降级。
- `HoverResult.quality` 为 `Any`/`Unknown`/`Missing` 时，`contents` 不得出现具体类型文本。
- 请求失败用 `BackendError`，检查质量用 `CapabilityStatus`；不要把超时/退出包装成空诊断。

## 3. 诊断发布与清理

| 场景 | 行为 |
| --- | --- |
| 检查完成、无错误 | 发布空诊断集合，清除该文件旧诊断 |
| 检查完成、存在模板可归因错误 | 只发布精确映射成功的诊断 |
| 后端超时、退出或未就绪 | 返回 `BackendError` / `Unavailable`，不发布空集合 |
| 诊断落在 helper、探针或生成代码 | 视为桥接失败或覆盖不足，不发布猜测位置 |
| 表达式跨 helper 与原标识符 | 归入覆盖不足，不整体平移 |
| 文档关闭或项目移除 | 清理该文档诊断 |
| 后端重启（新 `BackendEpoch`） | 旧请求、ACK、诊断全部失效；重放快照后重新计算 |
| JS/TS 主服务报告脚本错误 | 不由 WXML 服务重复发布 |

发布前校验：文件 + `SourceRevision` + `SnapshotId` + `ProjectRevision` + `ViewRevision` + `BackendEpoch` 均有效。

## 4. 诊断 target 规则

fixtures 中的 `target` 是类型检查器实际报错/查询的 token，不是整段表达式；运行器断言 `Diagnostic.range` **精确等于** target 的字节范围，不能用 `contains` 放宽：

| 错误种类 | target |
| --- | --- |
| 属性不存在 | 属性名（`nmae`、`age`、`subtitle`） |
| 可空错误（`possibly null`） | 可空对象表达式（`maybeUser`） |
| `unknown` 上的访问 | `unknown` 表达式（`external`） |
| 组件属性类型不匹配 | 绑定的表达式（`total`、`cardObject`） |
| 事件 handler 缺失 | handler 名（`missingHandler`） |
| hover | 用户悬停的标识符/表达式（`user.name`、`state.user.name`） |

如果实现只能返回更宽的范围，必须调整 fixture 并明确记录；不能通过放宽运行器断言掩盖。

## 5. 严格空值

| 表达式 | 数据声明 | 期望 |
| --- | --- | --- |
| `{{ maybeUser.name }}` | `maybeUser: User \| null` | 诊断落在 `maybeUser`：对象可能为 `null` |
| `{{ maybeUser }}` | 同上 | 不诊断；模板把 `null`/`undefined` 渲染为空串 |
| `{{ maybeUser.name }}`，位于 `wx:if="{{ maybeUser }}"` 内 | 同上 | 通过（模板条件窄化），不误报 |
| `{{ maybeUser.name }}`，位于守卫外 | 同上 | 仍诊断 `maybeUser` |
| `{{ user.profile.name }}`，`profile?: Profile` | 可选属性 | 按实际选项诊断；未支持时 `Partial` |
| `{{ list[0].name }}` | `list: Item[]` | `list[0]` 视为 `Item`；不做越界诊断 |
| `wx:for` 的 `item.name` | 元素类型 | 元素类型非空；默认 `index` 为 `number`（`loop-scope` 已验证） |

规则：

- 窄化只在支持的 `wx:if`/`wx:elif`/`wx:else` 链内生效，且条件与正文使用同一访问路径。
- 复杂布尔组合、赋值表达式、函数调用等未支持守卫返回 `Partial`，不产生猜测性错误。
- `wx:for` 变量在循环结束后不可见；泄漏访问报“未声明名字”。
- 对象遍历（`Record<string, T>`）时 `index` 为 `string`、`item` 为 `T`（`loop-scope` 已验证）。

## 6. 模板运行时转换

| 形式 | 运行时行为 | 诊断策略 |
| --- | --- | --- |
| 文本节点 `{{ value }}` | 任意值转展示文本；`null`/`undefined` 为空 | 不因“不是 string”报错 |
| 混合文本 `前缀 {{ value }}` | 结果恒为字符串 | 只检查表达式本身 |
| 算术/比较/逻辑运算 | 按 JS 语义求值 | 复用后端对生成表达式的诊断 |
| 对象/数组直接插值 | 渲染为字符串形式 | 第一版不诊断，记为未检查 |
| 布尔属性 `disabled="{{ value }}"` | 按真值转换 | 不要求 `boolean`；只检查表达式内部错误 |
| `wx:for="{{ value }}"` | 要求可遍历 | 可遍历性交给后端表达式检查 |

- 不得把 `{{ value }}` 包成 `String(value)` 后再检查，那会掩盖真实类型问题并破坏窄化。
- 混合文本中的表达式仍保留精确映射，不用 `string` 结果覆盖内部字段范围。
- 表达式必须按语法解析；正则拆分会破坏对象键、简写属性与嵌套结构。

## 7. 属性绑定与事件

### 7.1 自定义组件属性

- 目标类型来自组件 `properties` 的实际声明：`type` 与 `optionalTypes` 共同构成允许类型。
- 纯表达式绑定（`count="{{ total }}"`）按可赋值性检查；`number → Number` 通过，`number → String` 报错。
- 混合文本绑定（`title="共 {{ n }} 个"`）第一版只做 `Partial` 覆盖。
- 未声明属性访问（组件 WXML 使用 `subtitle` 但 `data`/`properties` 都没有）必须报错。
- 页面/组件根数据只来自脚本 `data`（组件另加 `properties`）；独立 `interface`/`type` 只有被字段引用时才参与，不能作为模板根。`Page({ data: User })` 这类把 interface 当值的写法必须返回 `Unknown`，不得伪造字段类型。
- 同一项目中多个组件可以各自声明同名 `title` 且类型不同；hover 必须解析到当前组件自己的 `properties`，不能串到页面或另一个组件。

### 7.2 事件绑定

| 形式 | 行为 |
| --- | --- |
| `bindtap="onTap"` / `bind:tap="onTap"` | 检查处理函数是否存在；定义跳转回脚本原声明 |
| `catchtap` / `capture-bind:tap` 等 | 同类处理，支持范围按实现记录 |
| `bindtap="{{ handler }}"` | 动态绑定第一版 `Unsupported`，不发布存在性诊断 |
| 页面选项函数 / 组件 `methods` | 均可作为候选；封装注册未覆盖时降级 |

### 7.3 特殊属性

`wx:key` 使用专用语义，不按自由表达式检查；第一版不发布其成员诊断。

## 8. 定义（Definition）边界

- `DefinitionResult.locations` 为空 + `Supported`：已检查，确实没有定义。
- 空 + `Unsupported`/`Disabled`/`Unavailable`/`Unknown`：没有能力查，必须显示降级，不能显示成“无定义”。
- 只有能安全映射回原文的目标才返回；无法映射时不返回，不贴到最近字符。
- 正向编辑/重命名结果不在 C0 内，AC-012 的 `coverage.edits` 记录为 `deferred-to-C1`。

## 9. `any`、`unknown` 与缺少类型

| 情况 | hover | 诊断 | 状态 |
| --- | --- | --- | --- |
| 已知具体类型 | 实际类型 + 说明 | 正常检查 | `Supported` / `Known` |
| 显式 `any` | `any` | 成员访问不报错 | `Supported` / `Any` |
| `unknown` | `unknown` | 成员访问报错，target 为 unknown 表达式 | `Supported` / `Unknown` |
| 裸 `Object` / `{}` | 实际类型文本 | 按 TS 可访问性检查 | `Partial` |
| 无脚本、无注册或数据根不是对象 | 不返回猜测类型 | 不发布诊断 | `Unsupported` 或 `Unknown` / `Unknown` |
| 桥接失败或 helper 报错 | 不返回 | 不发布模板诊断 | `Unavailable` / 部分覆盖 |

说明优先级：

1. 字段自身声明处的 JSDoc；
2. 字段类型引用目标（`interface`/`type` 属性）的 JSDoc；
3. 类型声明自身 JSDoc，仅作补充；
4. 都没有时留空，不生成通用文案。

`any` 是合法状态，但不能因此声称严格校验通过；`payload.anything` 显示 `any`，`external.anything`（`unknown`）必须报错。

## 10. 未支持形式与降级

| 形式 | 初始状态 | 行为 |
| --- | --- | --- |
| WXS 模块与 `<wxs>` | `Unsupported` | 不把 WXS 名字当数据字段 |
| `template` / `import` / `include` | `Partial`/`Unsupported` | 调用上下文未解析时不发布猜测诊断 |
| Behavior / 封装注册 / 闭包内注册 | `Partial` | 识别失败报告原因，不静默取第一个候选 |
| 多候选注册或类型来源不明 | `Partial` + 候选计数 | 使用首个注册、不合并；reason 必须包含候选数，见 [C1 类型桥契约](type-bridge-c1.md) §5.2 |
| 动态事件绑定 | `Unsupported` | 不发布存在性诊断 |
| 复杂窄化守卫 | `Partial` | 不产生猜测性空值错误 |
| 可选链等未覆盖表达式 | `Partial` | 记录支持范围，不把整段模板当字符串 |
| 共享模板的不同调用实例 | `Partial` | 视图身份包含调用上下文 |
| 无适用 `tsconfig` | `Partial` | 报告 inferred project 与实际选项 |
| JS 文件 `checkJs: false` | `Supported`（模板检查独立启用） | 从 JSDoc 取类型；不重复发布脚本错误 |

降级的必要条件：

- 返回 `CapabilityStatus` 或 `BackendError` 说明缺什么；
- 不发布无法归因的诊断；
- 不生成 `any` 或空实现“让检查通过”；
- 在 fixtures 中保留该形式的正例或反例。

## 11. 与验收样本的对应

| 规则 | 样本 | 必须观察到的行为 |
| --- | --- | --- |
| 字段类型与说明来自真实声明 | `page-basic`、`cross-file-type` | `user.name`、`title` 的类型和 JSDoc 可追溯 |
| 错误字段精确标记 | `page-basic` | `user.nmae` 报错，target 为 `nmae` |
| 修复后清除 | `page-basic-clean` | 同一页面无 `nmae` 时诊断为空且 `Supported` |
| 循环作用域与遮蔽 | `loop-scope` | 内层默认 `item` 遮蔽外层，退出后恢复；对象键为 string；循环外 `item` 报错 |
| 条件与联合 | `conditional-union` | `state.code` 在 `else` 可取；`state.user.age` 报错；守卫内可空访问通过，守卫外 target 为 `maybeUser` |
| JS JSDoc | `js-jsdoc` | `checkJs: false` 下仍从 JSDoc 获得类型；无关 JS 错误不重复发布 |
| 路径别名 | `alias-project` | `tsconfig.base.json` + `extends` 的 `@models` 解析到实际类型 |
| 组件绑定 | `component-bindings` | 属性类型不匹配 target 为绑定表达式；缺失事件 target 为 handler；definition 正反例 |
| 组件 properties | `component-properties` | `type`/`optionalTypes`/JSDoc 生效；`cardObject` 与 `subtitle` 各自报错 |
| 多视图与组件隔离 | `multi-view` | `card.title` 为 string、`badge.title` 为 number，两个 WXML 各自解析 |
| any/unknown | `any-unknown` | `any` 不伪造说明；`external.anything` 的 target 为 `external` |
| interface 根 | `interface-root` | `Page({ data: User })` 返回 `Unknown`，不伪造 string |
| 未支持形式 | `unsupported-forms` | WXS/import/include/动态事件返回 `Unsupported`，不伪造 `Supported` |
| 未保存修改 | `unsaved-overlay`（runtime） | 不保存即更新类型与说明 |
| 旧结果与重启 | `stale-request`（runtime） | 旧结果不发布，重启后恢复 |
| Zed 多文档 | `zed-multi-doc`（runtime） | 事件进入正确实例，不重复诊断 |

WT-B 验证（2026-11-16）：

```bash
(cd backends/typescript && npm ci && npm run build)
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test wave5_acceptance -- --ignored
```

上表中与 WT-B 相关的 `loop-scope`、`conditional-union`、`multi-view`、`alias-project`、`component-bindings` 已由该 runner 执行：`wx:for` locals 按元素/索引类型解析（嵌套、遮蔽、`wx:for-item`/`wx:for-index`、退出循环后不可见、`Record` 键为 `string`），`wx:if/elif/else` 分支窄化生效，不同页面/组件脚本的视图在交错请求下互不污染（发现模型为 1 WXML ↔ 1 脚本，同一脚本多视图不在模型内），`@models/*` hover 与 definition 可解析，组件属性绑定不匹配精确落在绑定表达式。该 runner 被 `#[ignore]` 保护（常规门禁只编译不执行），并且不覆盖补全、事件处理函数及其他未列出的 AC。

## 12. 变更规则

- 修改策略时同步更新本文件、相关 fixtures 与 `manifest.json`；不接受只改实现或只改样本。
- 新增支持的模板形式必须同时提供正例和反例；只证明“能返回 hover”不构成验收。
- 关键路线（类型桥、未保存同步）方案变化先更新 ADR。
- 任何降级都要在能力状态中可见；文档不得把 `Partial` 描述为已支持。
