# WXML 语言功能基线

更新日期：2026-11-15。

本文件是本项目 WXML 通用语言能力的自有需求基线。功能取舍、默认行为和验收条件以本文为准，不再依赖外部插件的 README、源码或本机实验目录。历史来源仅用于许可证与归属说明，见 [来源与许可证记录](provenance.md)。

当前状态：截至 2026-11-16，F-01–F-04、F-06、F-07 的补全子集已实现并通过测试，实际覆盖范围与降级行为见 §3.8；其余条目仍为计划或待实现，不代表已经具备对应能力。模板类型体验的优先级与验收见[项目 README](../../README.md#31-第一优先级模板类型体验)。

## 1. 优先级与范围

1. 先完成 T1–T9 模板类型闭环：真实类型、字段说明、精确诊断、模板作用域、条件窄化、项目类型上下文和未保存一致性。
2. 在类型闭环可验收后，按日常使用价值补齐 F-01–F-18 通用语言功能。
3. 每项功能必须说明数据来源、支持范围、降级行为和验收样本。不能只以“补全列表非空”或“hover 有返回”作为通过条件。

首阶段只面向原生微信小程序 `.wxml` 及其关联 JS/TS、JSON、样式文件。Vue/Pug 内嵌模板、Emmet、一键创建组件、JS 变量自定义装饰、自动修改编辑器配置、扩展市场发布和上游 PR 工作流不属于需求。

## 2. 功能清单

| 编号 | 功能 | 行为基线 | 主要数据来源 | 阶段 |
| --- | --- | --- | --- | --- |
| F-01 | 标签名补全 | 输入 `<` 后给出标签候选及说明；保留已输入前缀，不破坏后续文本 | 本地版本化基础组件元数据 | 5 |
| F-02 | 属性名补全 | 标签内输入空格或属性前缀后给出适用属性；按标签、组件和已有属性过滤 | 本地元数据、`usingComponents`、组件声明 | 5 |
| F-03 | 属性值补全 | 仅对枚举型属性给出值候选；替换完整属性值范围 | 本地元数据中的枚举定义 | 5 |
| F-04 | 指令与事件补全 | 补全 `wx:if/elif/else/for/key/for-item/for-index` 及常用事件绑定形式 | 本地元数据、模板语法 | 5 |
| F-05 | 上下文筛选 | 根据标签、组件和模式筛选候选，例如 `picker` 的 `mode` 影响可用属性 | 本地元数据中的条件规则 | 5 |
| F-06 | 自定义组件补全 | 从页面/组件 JSON 的 `usingComponents` 解析组件，补全标签和属性说明 | 项目 JSON、组件声明、类型后端 | 5 |
| F-07 | 表达式成员与事件处理补全 | 在 `{{ }}` 中按模板作用域补全成员；事件属性中补全处理函数 | 类型后端、模板作用域、脚本声明 | 4–5 |
| F-08 | Hover | 显示表达式类型、字段 JSDoc、声明归属；显示组件属性、标签和属性说明 | 类型后端、项目模型、本地元数据 | 2–5 |
| F-09 | 定义跳转 | 跳转到字段声明、组件文件、class、资源文件和事件处理函数 | 类型后端、项目索引、样式索引 | 4–5 |
| F-10 | 诊断 | 标记未知字段、非法类型操作和不兼容绑定；范围精确，修复后清除 | 类型后端、模板语义、诊断策略 | 2–4 |
| F-11 | Class 补全与导航 | 从同名样式文件和配置的全局样式补全 class，并展示注释说明 | 项目样式索引 | 5 |
| F-12 | 资源路径链接 | 对配置的路径属性提供可点击链接；只链接存在的文件 | 项目文件解析 | 5 |
| F-13 | 闭合标签与配对 | 自动闭合、标签配对高亮和成对重命名；不破坏 `wxs` 与表达式 | Zed 查询、必要的 LSP 编辑 | 5 |
| F-14 | Snippets | 提供内置片段和用户片段；占位符跳转与说明稳定 | 本地片段定义、用户配置 | 5 |
| F-15 | 格式化 | 保真格式化；解析失败时不修改文件 | 语法树、格式化器 | 6 |
| F-16 | 配置、依赖、缓存 | 解析项目根、页面/组件归属、配置变更和依赖失效；内存覆盖层优先 | 项目模型、文件监听、LSP 协调器 | 3–4 |
| F-17 | 类型与项目上下文 | 尊重适用 `tsconfig`、路径别名、模块解析和未保存文档 | 类型后端、项目模型 | 2–4 |
| F-18 | 多根与多项目 | 多工作区、多小程序根、多 `tsconfig` 和重启后状态隔离 | 项目模型、服务实例标识 | 3–6 |

## 3. 行为细则

### 3.1 标签、属性和值（F-01–F-05）

- 标签补全在 `<` 后触发，候选至少覆盖微信小程序基础组件。候选项带一句用途说明；选中后保留用户已输入的前缀，不自动添加未请求的属性。
- 属性补全在标签名后的空白处触发，按当前标签或自定义组件的声明过滤。已经出现的属性默认不再重复推荐；允许同一属性多次出现的场景需有显式规则。
- 属性名匹配同时支持源码中的写法和元数据中的规范名，例如连字符与驼峰之间的对应关系必须确定，不能依赖模糊字符串搜索。
- 属性值补全只在枚举值明确时提供，例如 `picker` 的 `mode` 及其相关属性。替换范围覆盖原值内容，不残留引号或半截值。
- 条件筛选规则必须数据化，不能把 `picker` 等特例硬编码在补全流程里。规则至少能表达“某属性取某值时，另一属性可用/不可用/改变候选”。
- 指令和事件补全使用独立分类：`wx:` 指令、`bind*`/`catch*`/`capture-bind*`/`mut-bind*` 事件、普通属性。事件名与处理函数补全走不同请求通道，避免把属性候选混入表达式成员。

### 3.2 自定义组件（F-06）

- 从最近的页面或组件 JSON 读取 `usingComponents`，解析本地路径、绝对路径和项目支持的包形式。
- 组件标签候选优先于同名基础组件；出现重名时保留两者并标注来源，不静默取第一个。
- 组件属性来自实际 `properties` 声明或类型后端。无法解析、插件组件、远程组件或声明不完整时，明确标记为降级，不伪造属性列表。
- 配置未保存时以内存内容为准；组件文件或属性声明变化后刷新引用它的模板。
- 重复声明、循环引用和缺失目标文件必须可诊断，不能让补全或诊断流程崩溃。

### 3.3 表达式、Hover 与定义（F-07–F-09）

- `{{ }}` 内按模板作用域解析：页面/组件数据、组件属性、`wx:for` 项与索引、自定义循环变量、模板调用上下文、`wxs` 模块。
- 页面/组件的数据根来自脚本声明：页面为 `data`，自定义组件为 `data` 与 `properties`；字段在 WXML 中直接作为顶层标识符访问。独立 `interface`/`type` 只有被 `data`/`properties` 字段引用时才参与，不能作为模板根。
- 成员补全使用真实类型，不用正则枚举字段。`any`、`unknown` 和缺失类型必须显示真实状态，不能生成看似确定的候选。
- 事件属性中的处理函数补全来自脚本声明：`Component` 读取 `methods`，`Page` 读取选项对象上的函数；方法引用、`data-*` 参数和绑定形式分别处理。
- Hover 至少包含类型、字段说明和声明归属。字段说明优先使用实际 `data`/`properties` 字段或其引用类型自身的 JSDoc；没有说明时明确省略，不复制其他字段的文档。
- 定义跳转优先返回原始声明，而不是内存检查视图中的辅助符号。无法安全反向映射的编辑或位置不返回。
- 资源路径和 class 的定义跳转见 F-11、F-12；组件标签跳转到组件文件或声明入口。

### 3.4 诊断（F-10）

- 诊断范围映射回原始 WXML，精确到出错字段或表达式片段；中文、emoji 和不同换行格式不能导致偏移。
- 未知字段、非法成员访问、非法操作数和可判定的绑定类型不兼容是首阶段重点。模板运行时转换、可空访问等规则必须有独立策略，不能直接照搬 TypeScript 诊断。
- 只发布可归因到 WXML 的问题；辅助代码、检查视图包装和探针声明产生的错误属于桥接失败或覆盖不足，不能过滤后当作“检查通过”。
- 修复后旧诊断必须清除。后端不可用、检查关闭或只完成部分分析时，显示对应能力状态，不把空诊断当作严格通过。
- 诊断去重以“源文件、范围、代码、消息类别”为键，避免同一问题因多个检查视图重复发布。

### 3.5 样式与资源（F-11–F-12）

- Class 补全默认读取同名样式文件；全局样式文件必须由项目配置显式给出，不假设固定文件名一定存在。
- class 说明来自样式声明前的块注释。`@import` 展开、预处理器和缩进式 Sass 的支持范围需单独声明；未支持时明确降级。
- 资源链接只处理配置允许的属性名，并通过项目根和解析根计算目标。目标不存在时不生成可点击链接。
- 路径解析必须限制在项目可解析范围内，不因绝对路径、`..` 或符号链接越出预期工作区而不提示。

### 3.6 编辑体验（F-13–F-15）

- 闭合标签、配对高亮和成对重命名优先使用 Zed 的语法查询能力；需要跨文件或语义信息时才通过 LSP 提供编辑。
- 编辑必须保持原文空白、注释、表达式和 `<wxs>` 内容。不能为了格式化或重命名重新打印整份文档。
- Snippets 区分内置和用户定义。用户片段冲突时给出确定优先级；占位符顺序和最终光标位置必须有测试。
- 格式化后置到类型闭环和常用补全之后。格式化必须幂等、保留语义，解析失败或存在未恢复语法错误时不修改文件；具体格式化器在实施时评估。

### 3.7 项目与状态（F-16–F-18）

- 项目模型负责文件身份、内存覆盖层、磁盘回退、配置候选和依赖图。语言服务协调器是文档状态的唯一写入入口，分析只读不可变快照。
- 未保存的 WXML、脚本、导入类型和 JSON 配置优先于磁盘内容；关闭文档后回退到磁盘并产生新的内部版本。
- 项目根、页面/组件归属和 `tsconfig` 归属必须有确定规则。多候选时不能简单选择“最近的一个”；无法确定时显式标注推断状态。
- 依赖变化只刷新受影响的模板。旧请求、旧快照和旧后端世代的结果不得覆盖新文本。
- 多工作区、多小程序根、多 `tsconfig` 和服务重启必须隔离；日志至少包含实例、事件和版本标识。
- 缓存键包含文档版本、项目版本、检查视图版本和后端世代。缓存失效条件必须可测试，不能只按文件路径缓存。

### 3.8 补全目录现状（Wave 5）

Wave 5 实现了 F-01–F-04 与 F-06/F-07 的目录驱动子集；F-05 的条件属性规则尚未数据化，F-11 的 class 补全不在本次范围。标签名、属性名、枚举值和指令/事件补全是**静态、同步**的，完全在 Rust 内完成，不启动 Node；只有 Mustache 成员补全和事件处理函数值补全经过类型后端。以下目录是**有意限定的子集**，不宣称完整微信小程序组件覆盖：未收录的标签、属性照常可用于模板，只是没有静态候选。

#### 3.8.1 标签

| 标签 | 专属属性（全局属性、指令、事件同时可用） | 枚举值属性 |
| --- | --- | --- |
| `view` | `hover-class` `hover-stop-propagation` `hover-start-time` `hover-stay-time` | `hover-stop-propagation` |
| `text` | `selectable` `user-select` `space` `decode` | `space` |
| `button` | `type` `size` `plain` `disabled` `loading` `form-type` `open-type` `hover-class` | `type` `size` `form-type` `open-type` |
| `image` | `src` `mode` `lazy-load` `binderror` `bindload` | `mode` |
| `input` | `type` `value` `placeholder` `disabled` `maxlength` `focus` `confirm-type` `password` | `type` `confirm-type` |
| `picker` | `mode` `range` `range-key` `value` `start` `end` `bindchange` `bindcancel` | `mode` |
| `scroll-view` | `scroll-x` `scroll-y` `enable-flex` `scroll-top` `scroll-left` `enable-back-to-top` `bindscroll` `bindscrolltolower` | `scroll-y` |
| `swiper` | `indicator-dots` `autoplay` `interval` `duration` `circular` `vertical` `bindchange` | `indicator-dots` `autoplay` `circular` |
| `navigator` | `url` `open-type` `delta` `hover-class` | `open-type` |
| `textarea` | `value` `placeholder` `maxlength` `auto-height` `disabled` | `auto-height` |
| `form` | `bindsubmit` `bindreset` | — |
| `label` | `for` | — |
| `checkbox` | `value` `checked` `disabled` | `checked` |
| `radio` | `value` `checked` `disabled` | `checked` |
| `template` | `is` `data` | — |
| `block` `swiper-item` | — | — |
| `wxs` | `module` `src` | — |

全局属性：`id` `class` `style` `hidden`。补全候选按当前标签、全局属性、指令、事件分组排序；已输入前缀大小写不敏感匹配，不覆盖光标后的文本。

#### 3.8.2 指令与事件

- 指令：`wx:if` `wx:elif` `wx:else` `wx:for` `wx:for-item` `wx:for-index` `wx:key`。
- 固定事件属性：`bindtap` `catchtap` `bind:tap` `catch:tap` `capture-bind:tap` `capture-catch:tap` `mut-bind:tap` `bindinput` `bindchange` `bindconfirm` `bindsubmit` `bindreset` `bindscroll` `bindload` `binderror` `bindtouchstart` `bindtouchmove` `bindtouchend`。
- 由事件名生成的绑定属性：仅当前缀为空或已是事件前缀时，为下列事件名生成 `bind<name>`、`catch<name>`、`bind:<name>` 三种写法：空前缀下便于发现事件，输入事件前缀后则只保留匹配项。事件名：`tap` `longpress` `longtap` `touchstart` `touchmove` `touchend` `touchcancel` `input` `change` `confirm` `blur` `focus` `submit` `reset` `scroll` `scrolltoupper` `scrolltolower` `load` `error` `message` `animationstart` `animationiteration` `animationend` `transitionend`。
- 闭合标签补全（`</`）只来自当前未闭合元素栈：`<view><text></` 给出 `view`、`text`，不给出其他标签。

#### 3.8.3 枚举值

| 标签.属性 | 候选值 |
| --- | --- |
| `view.hover-stop-propagation` | `true` `false` |
| `text.space` | `ensp` `emsp` `nbsp` |
| `button.type` | `default` `primary` `warn` |
| `button.size` | `default` `mini` |
| `button.form-type` | `submit` `reset` |
| `button.open-type` | `contact` `share` `getPhoneNumber` `getUserInfo` `openSetting` `feedback` |
| `image.mode` | `scaleToFill` `aspectFit` `aspectFill` `widthFix` `heightFix` `center` `top` `bottom` `left` `right` |
| `input.type` | `text` `number` `idcard` `digit` `safe-password` `nickname` |
| `input.confirm-type` | `send` `search` `next` `go` `done` |
| `picker.mode` | `selector` `multiSelector` `time` `date` `region` |
| `scroll-view.scroll-y` | `true` `false` |
| `swiper.indicator-dots` `autoplay` `circular` | `true` `false` |
| `navigator.open-type` | `navigate` `redirect` `switchTab` `reLaunch` `navigateBack` `exit` |
| `textarea.auto-height` | `true` `false` |
| `checkbox.checked` `radio.checked` | `true` `false` |

值补全只对表内属性生效，候选按已输入前缀做 ASCII 大小写不敏感匹配；表外属性不产生值候选。

#### 3.8.4 Node 后端补全

- Mustache 成员补全：`{{ user.na }}` 经 C1 虚拟文件布局复用 TypeScript `getCompletionsAtPosition`，候选来自页面/组件真实类型。
- 事件处理函数值补全：`bindtap="on"` 枚举 Page 选项上的顶层函数，或 Component `methods` 的成员；注册容器名（`data`、`properties`、`methods` 等）不进入候选。路由规则：属性名以 `bind`、`catch`、`capture-bind`、`capture-catch`、`mut-bind` 开头且后缀为 `:` 或小写字母/事件名时按事件值处理，因此目录中未逐一列出的 `bindcancel`、`bindcolumnchange` 等也能得到方法补全；`binding-mode` 之类普通属性不受影响。
- 后端不可用、未实现、超时或不支持时返回空列表并记录日志，不返回协议错误，也不影响静态补全。

#### 3.8.5 替换范围、触发与能力声明

- 所有候选项都带精确 `textEdit`：标签名和属性名只替换已输入 token；引号内的值只替换值前缀（保留引号）；未加引号的值插入带引号的字面量。不整行替换。
- 在纯文本、HTML 注释、`<wxs>` 元素体和 Mustache 字符串字面量内不产生补全；非模板文档（`.ts`、`.json` 等）经项目路由判定后直接返回空列表。
- `initialize` 无条件声明 `completionProvider`，`resolveProvider: false`，触发字符为 `<`、空格、`:`、`"`、`'`、`.`、`{`、`=`；结果 `isIncomplete: false`，单次最多 256 项。参数缺失或结构非法返回 LSP `-32602`。

#### 3.8.6 已知边界

- 自定义组件标签来自页面 JSON 的 `usingComponents`（`app.json` 全局表合并，同名时页面本地项生效）。组件 JSON 目前只为属性声明解析，**组件自身 WXML 中的自定义组件尚不列出**；组件属性名也不参与属性补全。
- 静态目录是手写受限子集，不是官方组件表的自动镜像；新增条目需同时补测试与本文档。

目录溯源：条目定义在 `crates/wxml-lsp/src/completion/catalog.rs`，取值来源与许可证说明见[来源与许可证记录](provenance.md)；目录不下载、不执行外部元数据。

## 4. 数据来源与元数据规则

允许的数据来源：

1. 仓库内版本化的基础组件、属性、指令、事件和片段元数据。
2. 项目内的 `app.json`、页面/组件 JSON、`usingComponents`、`tsconfig`、样式文件和用户片段配置。
3. 通过类型后端获得的脚本类型、JSDoc 和声明位置；数据根限定为页面/组件 `data` 与组件 `properties` 及其引用类型。

约束：

- 运行时不依赖外部插件、外部仓库或临时目录。缺少本地元数据时功能降级并报告，不静默下载。
- 每条内置元数据记录应能追溯到来源、版本和许可证。来源标识只出现在[来源与许可证记录](provenance.md)，功能文档不把外部项目当作需求依据。
- 元数据结构至少包含：规范名称、别名、适用标签/组件、值类型、枚举值、条件规则、说明、来源版本。
- 不执行工作区脚本或 `new Function` 提取组件信息；需要脚本语义时交给类型后端做静态分析。
- 不把生成代码写入业务项目。检查视图、辅助符号和缓存均存在于内存或项目自有缓存目录。

## 5. 验收要求

- 每项功能至少有正例、反例和降级样本；补全类功能还要验证替换范围、触发字符和候选过滤。
- 页面/组件数据根必须来自 `data`/`properties`，不得以独立 `interface`/模型文件替代；跨文件类型只能作为字段类型来源。
- 位置相关功能覆盖中文、emoji、LF/CRLF 和文档边界。
- 项目相关功能覆盖多根、路径别名、未保存配置和依赖变化。
- 与已有 JS/TS 服务并存时，不重复发布无关脚本诊断，不抢占原服务职责。
- 文档和交付说明必须区分：已实现、部分实现、明确降级、未实现。
  - 已实现：F-01–F-07 补全；表达式成员 hover；表达式目标 definition；未知字段/成员/部分类型诊断；AC-005/006/008/012 模板类型；事件处理函数诊断/definition/补全；Zed 未保存 JS/TS 同步。
  - 部分实现/明确降级：组件/标签 hover 仅表达式字段；definition 仅可映射表达式目标和事件处理函数；组件内 `usingComponents` 未建模；复杂守卫、复杂组件属性声明、`jsCheck` 关闭时组件绑定、插件/包组件、多根工作区。
  - 未实现：F-08 组件属性/标签/基础组件说明 hover；F-09 组件文件/class/资源跳转；F-10 其余诊断；F-11 class/WXSS；F-12 资源路径；F-13 成对重命名；F-14 snippets；F-15 格式化；F-16 配置热更新剩余部分；F-18 多根；发布流水线。
