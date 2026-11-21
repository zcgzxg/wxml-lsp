# 模板类型诊断策略

更新日期：2026-11-21。

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

| 错误种类 | 级别 | target |
| --- | --- | --- |
| 属性不存在 | `Error` | 属性名（`nmae`、`age`、`subtitle`） |
| 可空错误（`possibly null`） | `Error` | 可空对象表达式（`maybeUser`） |
| `unknown` 上的访问 | `Error` | `unknown` 表达式（`external`） |
| 组件属性类型不匹配 | `Error` | 绑定的表达式（`total`、`cardObject`） |
| 组件未声明属性 | `Error` | 属性名（`round`） |
| 组件事件名未声明 | `Warning` | 事件名 token（`chaneg`） |
| 事件 handler 缺失 | `Error` | handler 名（`missingHandler`） |
| 组件事件 handler 参数不兼容 | `Error` | 绑定值的方法名 token |
| hover | — | 用户悬停的标识符/表达式（`user.name`、`state.user.name`） |

组件声明引入的诊断只有“事件名未声明”是 `Warning`，其余模板可归因诊断均为 `Error`；`Warning` 仅在组件 `events` 声明完整时发布。

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

组件属性的类型来源有两条，按优先级取第一条可用的：

| 优先级 | 来源 | 生效条件 |
| --- | --- | --- |
| 1 | 组件类型声明 | 项目 `tsconfig.json` 同级的 `components.d.ts`（见 [§7.4](#74-组件类型声明)）把该标签映射到组件接口，且该属性类型可证明 |
| 2 | 脚本 `properties` | 标签没有声明映射，或声明把该属性标为不完整/未证明 |

- 声明覆盖脚本：同一标签同时有声明与脚本 `properties` 时，属性检查与 hover 使用声明；两份事实不合并，冲突按声明处理。
- 未保存改动不改变声明来源：组件已使用声明事实时，脚本 `properties` 的 overlay 改动不影响组件事实；只有声明文件自身的 overlay 改动会触发重算。
- 声明缺失或不完整时回落到脚本 `properties`，行为与本次改动前一致。两条来源的未知属性处理不同：声明映射生效时，组件的 `props` 就是完整属性面，声明未包含的属性报未声明属性（`class`/`style`/`id`/`hidden`/`slot`/`data-*`/`aria-*`/`wx:*` 等通用属性豁免）；只有脚本 `properties` 时沿用既有行为，不把未知属性当作错误（保留 `custom-class` 一类透传写法）。
- 声明成员类型未证明时，该成员不参与类型诊断，状态 `Partial` 并给出原因，其他成员照常生效。
- 可选属性（`?` 或类型含 `undefined`）允许缺省和 `undefined`；hover 展示声明中的类型原文，不重排 union 顺序。
- 脚本来源下，`type` 与 `optionalTypes` 共同构成允许类型。
- 脚本来源的组件带 `behaviors` 时，behavior 模块按 tsconfig `paths` 解析后并入属性面。behavior 无法解析（别名失配、文件缺失、behavior 自身写法超出静态可读范围）时不再让整个组件失效：仍校验组件自身 `properties` 声明的属性；其余属性可能由 behavior 提供，按脚本来源惯例不报错，状态 `Partial`，原因形如 `组件 behaviors 未能完全解析，仅校验了组件自身 properties 声明的属性（card）`。
- 纯表达式绑定（`count="{{ total }}"`）按可赋值性检查；`number → Number` 通过，`number → String` 报错；范围落在绑定表达式而非属性名。
- 混合文本绑定（`title="共 {{ n }} 个"`）第一版只做 `Partial` 覆盖。
- 未声明属性访问（组件 WXML 使用 `subtitle` 但 `data`/`properties` 都没有）必须报错。
- 页面/组件根数据只来自脚本 `data`（组件另加 `properties`）；独立 `interface`/`type` 只有被字段引用时才参与，不能作为模板根。`Page({ data: User })` 这类把 interface 当值的写法必须返回 `Unknown`，不得伪造字段类型。
- 同一项目中多个组件可以各自声明同名 `title` 且类型不同；hover 必须解析到当前组件自己的声明或 `properties`，不能串到页面或另一个组件。

### 7.2 事件绑定

绑定形式与既有检查不因声明改变：

| 形式 | 行为 |
| --- | --- |
| `bindtap` / `bind:tap` | 检查处理函数是否存在；definition 跳回脚本方法名 token |
| `catchtap` / `catch:tap` | 同类处理，缺失同样报错 |
| `capture-bind:tap` / `capture-catch:tap` / `mut-bind:tap` | 同类处理，事件名去掉绑定前缀 |
| `bindtap="{{ handler }}"`、`bindtap=""` | 动态/空绑定记为 `Unsupported`，不发布存在性诊断，不制造状态噪声 |
| 非标识符 handler（如 `this.onTap`） | 视为非静态方法名，不发布存在性诊断 |
| 页面选项函数 / 组件 `methods` | 方法简写、`fn: function () {}`、箭头函数属性均可解析 |
| 多注册 | 使用首个页面/组件注册表，状态 `Partial`，不伪造缺失错误 |
| 组件 `behaviors` | behavior 可能提供 handler，状态 `Partial`，不伪造缺失错误 |
| 非字面量根（如 `Page(options)`） | 沿用既有视图策略，`Unsupported`，不伪造方法或诊断 |

组件拥有事件声明（`events`）时，额外提供以下能力；没有声明的项目保持原有行为：

| 能力 | 来源 | 行为与级别 |
| --- | --- | --- |
| 事件名补全 | 声明 `events`，仅当 `events` 声明完整 | `bind:`/`catch:`/`capture-bind:`/`capture-catch:`/`mut-bind:` 之后返回声明中的事件名；只替换已输入的事件名片段，不改写绑定前缀；声明缺失或不完整时沿用静态目录候选 |
| 未声明事件名 | 声明 `events`，仅当 `events` 声明完整 | 事件名不在声明中：在事件名 token 上发布 `Warning`；动态绑定（`bind:change="{{ name }}"`）、声明缺失或不完整时不发布 |
| 事件属性 hover | 声明 `events` 的 `detail` | 展示 `(e: WechatMiniprogram.CustomEvent<T>) => void`，`T` 为声明中 `detail` 的类型原文；`detail` 未证明时明确标注未证明，不展示猜测类型；成员上方的 `/** */` 说明作为独立段落追加在签名之后 |
| handler 签名校验 | 声明 `detail` + 脚本形参标注 | 处理函数无参数（`arity == 0`）视为兼容；有参数时仅在已显式标注类型时比较：可证明不兼容时在 handler 名 token 上发布 `Error`；兼容时不发布诊断；参数未标注或任一侧未证明时不发布诊断，状态 `Partial` 并给出原因 |
| handler hover / 补全 / definition | 脚本 | 保持既有行为，见下文 |

说明：

- 事件属性 hover 的展示形态固定为 `(e: WechatMiniprogram.CustomEvent<T>) => void`；例如 `detail` 为 `{ index: number }` 时展示 `(e: WechatMiniprogram.CustomEvent<{ index: number }>) => void`。
- 未声明事件名是唯一由声明引入的 `Warning`：仅在 `events` 完整时启用，因为库可能 `triggerEvent` 未声明的事件；补齐声明或删除映射即可关闭。
- 处理函数缺失优先：发布缺失 `Error` 后不再叠加参数类型诊断。
- 参数未标注导致的 `Partial` 原因固定形如 `组件 'card' 的事件处理函数参数未标注类型，事件参数类型未校验`；与其他降级原因（声明不可读、声明不完整、类型未证明）共存时保留先到者，不叠加展示。
- 事件名补全与事件属性值补全互不替代：属性名候选来自声明（仅完整声明），属性值候选仍来自脚本方法。

hover：悬停在 handler token 上返回后端提供的脚本方法签名，范围精确到 handler token。

补全：事件属性值（`bindtap="on"`）返回当前 Page 顶层方法或 Component `methods`，由 native 后端脚本查询提供事件处理函数候选（对应原 Node `complete` 的 `eventHandler` kind）；事件属性名补全见上表，不在该链路重复。

### 7.3 特殊属性

`wx:key` 使用专用语义，不按自由表达式检查；第一版不发布其成员诊断。

### 7.4 组件类型声明

组件声明是可选的事实来源，入口只有一个文件：项目 `tsconfig.json` 同级的 `components.d.ts`；完整决策见 [ADR-0011](../adr/0011-component-type-declarations.md)。

#### 7.4.1 发现规则

| 情况 | 行为 |
| --- | --- |
| `tsconfig.json` 同级存在 `components.d.ts` | 读取并参与属性与事件检查；使用编辑器缓冲区内容，未保存改动立即生效 |
| 文件缺失 | 不产生组件事实、不发布诊断，能力状态不变 |
| 文件为空或没有 `WxmlComponentMap` | 不产生组件事实、不发布诊断，能力状态不变 |
| 项目没有 `tsconfig.json` | 不寻找声明文件，行为与本次改动前一致 |
| 声明放在更深层目录（如 `types/components.d.ts`） | 不生效，只读取与 `tsconfig.json` 同级的那一个文件 |
| 声明文件是目录、逃逸项目根、不可读或超出单文件大小预算 | 记录一条可读的降级说明，能力状态 `Partial`；不产生组件事实，不崩溃 |
| 声明组件数或成员数超出上限 | 按声明顺序截断，记录降级说明，状态 `Partial` |

- 不读 tsconfig 的 `files`/`include`/`exclude`/`paths`，不做 glob，不扫描 `node_modules`。
- 一个 tsconfig 目录只对应一份声明；需要不同声明时，把声明放到各自 app 的 tsconfig 目录。

#### 7.4.2 声明格式

入口是 `WxmlComponentMap`，把 WXML 标签名映射到同文件内的组件接口；接口用 `props` 与 `events` 两段描述契约。`WxmlComponentMap` 位于 `declare global` 内或文件顶层时同样生效；未被映射表引用的接口不产生事实。

```ts
// 与 tsconfig.json 同级
declare global {
  interface WxmlComponentMap {
    'van-button': VanButtonProps;
  }

  interface VanButtonProps {
    props: {
      /** 按钮类型，字符串字面量联合 */
      type?: 'primary' | 'default' | 'warning';
      /** 是否显示加载中，可选 boolean */
      loading?: boolean;
      /** 按钮文案，必填字符串 */
      text: string;
    };
    events: {
      /** 点击确认后触发，detail 携带列表下标 */
      change: { detail: { index: number } };
      /** 输入时触发 */
      input: { detail: { value: string } };
    };
  }
}

export {};
```

- `events` 的事件值用 `{ detail: T }` 声明 detail；`T` 直接决定 hover 签名中的 `CustomEvent<T>`。
- `props` 与 `events` 成员上方的 `/** ... */` 说明会进入 hover：属性说明追加在类型与声明位置之后，事件说明追加在签名之后。多行说明折叠为单行，超过 512 字符截断；`//` 与 `/* */` 不算说明；没有说明时不生成空行，也不伪造描述。说明只用于展示，不参与类型证明、可空性或绑定检查。
- 映射表内重复映射同一标签时取第一个，并记录冲突说明；只影响重复项与截断项，其他组件照常生效。
- 真实样例见 [`components-typings` 样本](../../tests/fixtures/README.md)：`round` 未声明、`text="{{ loading }}"` 类型不匹配、`bind:chaneg` 未声明事件名。

#### 7.4.3 可证明的类型子集

| 接受 | 示例 |
| --- | --- |
| 基础类型 | `string`、`number`、`boolean` |
| 字符串/数字/布尔字面量与联合 | `'primary' \| 'default'`、`1 \| 2`、`true` |
| 数组 | `string[]`、`{ id: number }[]` |
| `Record` | `Record<string, string>` |
| 嵌套对象字面量 | `{ index: number; tags?: string[] }` |
| 同文件 `interface`/`type` 别名 | 先声明 `interface Options { ... }`，再引用 `Options` |

超出子集的成员标记为“未证明”：不参与类型诊断，hover 明确标注未证明，状态 `Partial`。包括但不限于：外部导入类型、跨文件标识符（如 `SomeLib.Options`）、泛型、条件/映射类型、`typeof import(...)`、`node_modules` 内的类型。

#### 7.4.4 非目标

- 不给处理函数形参注入类型，也不改写用户脚本；签名校验只读静态标注。
- 不为未标注参数的处理函数补全 `e.detail`。
- 不检查内置组件（`view`、`input` 等）的事件；声明只影响 `WxmlComponentMap` 映射到的自定义组件。
- 不生成 `components.d.ts`，也不从组件源码推导声明。
- 不把 tsconfig 的 `include`/`paths` 语义引入声明发现。

## 8. 定义（Definition）边界

- `DefinitionResult.locations` 为空 + `Supported`：已检查，确实没有定义。
- 空 + `Unsupported`/`Disabled`/`Unavailable`/`Unknown`：没有能力查，必须显示降级，不能显示成“无定义”。
- 只有能安全映射回原文的目标才返回；无法映射时不返回，不贴到最近字符。
- 反向编辑/重命名至今未实现；AC-012 的 `coverage.edits` 记录为 `deferred-to-C1`（字段名保留为历史）。

## 9. `any`、`unknown` 与缺少类型

| 情况 | hover | 诊断 | 状态 |
| --- | --- | --- | --- |
| 已知具体类型 | 实际类型 + 说明 | 正常检查 | `Supported` / `Known` |
| 声明中可证明的属性/事件类型 | 声明中的类型原文 | 按声明检查 | `Supported` / `Known` |
| 声明的成员类型未证明 | 标注“未证明”，不展示猜测类型 | 不发布该成员诊断 | `Partial` |
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
| 多候选注册或类型来源不明 | `Partial` + 候选计数 | 使用首个注册、不合并；reason 必须包含候选数，见 [ADR-0012](../adr/0012-in-process-tsrs-session.md) 与[原生合同](tsrs-native-contract.md) |
| 组件声明文件不可读（目录、越界、超限） | `Partial` | 不产生组件事实，不发布猜测诊断，原因不含绝对路径 |
| 组件声明不完整（截断、重复映射、解析失败） | `Partial` | 受影响的组件或成员不参与检查，其他组件照常生效 |
| handler 参数未标注（或标注未证明） | `Partial` | 不发布签名诊断，原因可读 |
| 事件 `detail` 未证明 | `Partial` | 不发布签名诊断；hover 标注未证明 |
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
| 组件类型声明 | `components-typings` | 声明覆盖脚本：`round` 报未声明属性、`{{ loading }}` 报绑定不匹配、`chaneg` 报 `Warning`；补全与 hover 来自声明 |
| 事件处理函数 | `events-basic` | 7 种绑定形式解析到 Page/Component 方法；缺失 handler 精确报错；definition 正反例；动态/空/非标识符静默；多注册 `Partial`；非字面量 `Unsupported`；事件值补全 |
| 多视图与组件隔离 | `multi-view` | `card.title` 为 string、`badge.title` 为 number，两个 WXML 各自解析 |
| any/unknown | `any-unknown` | `any` 不伪造说明；`external.anything` 的 target 为 `external` |
| interface 根 | `interface-root` | `Page({ data: User })` 返回 `Unknown`，不伪造 string |
| 未支持形式 | `unsupported-forms` | WXS/import/include/动态事件返回 `Unsupported`，不伪造 `Supported` |
| 未保存修改 | `unsaved-overlay`（runtime） | 不保存即更新类型与说明 |
| 旧结果与重启 | `stale-request`（runtime） | 旧结果不发布，重启后恢复 |
| Zed 多文档 | `zed-multi-doc`（runtime） | 事件进入正确实例，不重复诊断 |

WT-B 验证（2026-11-16；**历史**）：

> `backends/typescript` 与 `wave5_acceptance` 已随 P7 删除，以下命令当前不可执行，仅保留为历史证据。
> 当前 native 覆盖见 `crates/wxml-lsp/tests/native_lsp_document_symbols.rs` 与
> `crates/wxml-backend-tsrs/tests/real_engine.rs`；`native_parity` 已随 ADR-0009 删除，历史版本见
> tag `pre-slim-20261120`。

```bash
(cd backends/typescript && npm ci && npm run build)
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test wave5_acceptance -- --ignored
```

上表中与 WT-B 相关的 `loop-scope`、`conditional-union`、`multi-view`、`alias-project`、`component-bindings` 已由该 runner 执行：`wx:for` locals 按元素/索引类型解析（嵌套、遮蔽、`wx:for-item`/`wx:for-index`、退出循环后不可见、`Record` 键为 `string`），`wx:if/elif/else` 分支窄化生效，不同页面/组件脚本的视图在交错请求下互不污染（发现模型为 1 WXML ↔ 1 脚本，同一脚本多视图不在模型内），`@models/*` hover 与 definition 可解析，组件属性绑定不匹配精确落在绑定表达式。该 runner 被 `#[ignore]` 保护（常规门禁只编译不执行），并且不覆盖属性名补全、重命名及其他未列出的 AC。

WT-C 验证（2026-11-16）：同一 runner 当时执行 `component-bindings` 的事件缺失诊断与正反 definition，以及 `events-basic` 的 Page/Component 方法、7 种绑定形式、缺失 handler 精确范围、动态/空/非标识符静默、多注册 `Partial`、非字面量根 `Unsupported`、事件值补全（`eventHandler` kind）。`real_node` 另有真实 Node 冒烟覆盖同一链路的诊断、definition 与补全（历史，P7 删除）；当前 native 冒烟见 `crates/wxml-lsp/tests/native_lsp_document_symbols.rs`（opt-in）；`stdio_type_loop` 已随 ADR-0009 删除，历史版本见 tag `pre-slim-20261120`。

## 12. 变更规则

- 修改策略时同步更新本文件、相关 fixtures 与 `manifest.json`；不接受只改实现或只改样本。
- 新增支持的模板形式必须同时提供正例和反例；只证明“能返回 hover”不构成验收。
- 关键路线（类型桥、未保存同步）方案变化先更新 ADR。
- 任何降级都要在能力状态中可见；文档不得把 `Partial` 描述为已支持。
