# ADR-0002: 模板类型检查、类型后端与源码映射

**Date**: 2026-11-15  
**Status**: accepted  
**Deciders**: 项目维护者、规划协作 agent

**2026-11-17 后续**：[ADR-0008](0008-rust-tsgo-no-node-backend.md)（proposed）拟替代类型桥的实现归属；本文的原位 shadow、模板语义和映射原则继续有效。

## Context

模板必须在不修改普通 Page/Component 源码的前提下保留真实类型、字段 JSDoc、声明位置和精确错误范围。WXML 的页面/组件数据根只能来自脚本中的 `data`（自定义组件还包括 `properties`），独立 `interface`/`type` 只有在被这些字段引用时才进入类型上下文。WXML 外层解析、模板作用域和 TS 类型检查属于不同问题，某个后端能识别联合类型不代表模板转换已经保留了窄化。JS 能返回 hover 也不代表启用了错误检查，过滤生成代码诊断后得到空结果更不能视为检查通过。类型上下文桥的具体形式仍需验证，已有有限实验不作为完整实现证据。

## Decision

Rust 独占模板表达式解析、绑定、检查片段内容和模板映射；Node 独占真实脚本类型上下文、完整片段插入及脚本 shadow 映射。通过内存检查视图复用真实 TS 符号关系，类型层组合映射后向 LSP 提供原文结果；辅助代码不写入业务项目，不执行工作区脚本。采用该边界，但暂不锁定未经验证的上下文桥实现。

### Rust 模板语义管线

```text
SyntaxSnapshot + AnalysisContext
  → 名字绑定与作用域
  → TemplateIR（绑定和控制流，引用 syntax 表达式）
  → CheckFragment（JS/TS 文本与模板映射）
  → TypeBackend 查询
  → 原 WXML 范围上的类型结果
```

WXML 外层优先采用固定版本的 Tree-sitter grammar，Mustache 内部独立解析。表达式实现建议采用词法器和 Pratt parser，语法集合在样本验证后冻结；支持合法输入过程中的不完整节点，禁止以正则承担完整表达式解析和改写。grammar 的插值文本范围不能当作完整表达式 AST。

增量解析按编辑前后文本构造 `InputEdit`，先更新旧树，再传入新文本复用旧树解析；每次解析后重新取得节点，不跨源码版本缓存旧节点。使用 `ERROR`/`MISSING` 等信息保留输入中间态，并以增量结果和全量解析一致作为回归条件。

| 语义 | 实现约定 | 验收或降级要求 |
| --- | --- | --- |
| 根数据 | 自由标识符在绑定后统一访问 Node 提供的模板数据根，如 `__wxml_root.user`、`__wxml_root.title`；根由脚本 `data`（自定义组件还包括 `properties`）合成 | 按小程序语义读取 `data`/`properties` 的实际声明、类型和 JSDoc；`properties` 的类型来自 `type`/`optionalTypes` 等声明，不能只按默认值推断；`interface`/`type` 仅通过字段引用参与，不作为独立根；不用简单交叉类型代替重名规则，不暴露完整实例方法 |
| 名字改写 | 区分局部变量、WXS 名字、成员名、对象键和简写属性；保持每个原 token 的范围 | 不将 `user.name` 的 `name` 当作根变量；简写属性单独 lowering |
| 循环 | 显式表示 item/index、默认/自定义名字、嵌套遮蔽与作用域退出 | 数组/对象遍历和循环条件同节点顺序用独立样本确认，不用通用 JS 迭代规则猜测 |
| 条件链 | 将 `wx:if/elif/else` 表示为连续控制流，条件与正文采用一致的访问路径 | 分别测试可空和判别联合，避免生成方式破坏窄化 |
| 事件 | 独立请求携带 owner、属性种类、handler 范围和绑定形式 | 候选、hover、定义和存在性检查独立验收；动态绑定不支持时明确降级 |
| 组件属性 | 读取目标 `properties` 的实际类型，按纯表达式/混合文本等绑定形式检查 | 不把所有属性插值直接当作对目标类型的赋值 |
| 特殊属性 | `wx:key` 等采用专用语义，不统一视为自由表达式 | 规则与运行时一致后开放检查 |
| 共享模板 | 将调用实例及其上下文纳入片段与映射身份 | 不能只凭模板 URI 缓存不同调用的类型 |
| WXS、template/import/include、Behavior | 分别建立支持范围和样本 | 未覆盖情况返回 partial/unsupported，不能靠生成 `any` 声称完整支持 |

### Node 类型宿主和上下文桥

Node 使用脚本 AST 与符号信息识别真实注册，不能只搜索名为 Page/Component 的字符串。存在多个候选、封装注册或类型来源不明时返回明确状态，不静默选取第一个。

保持原项目编译选项、文件种类和模块解析环境；为分析建立内存派生视图，在其中捕获模板数据上下文。定位 `data`/`properties` 的声明节点、类型注解和 JSDoc；组件 `properties` 的字段在模板中以顶层标识符出现，不生成 `properties.` 前缀。查询追到这些字段及其引用类型，不把独立 `interface`/导出类型当作模板根，也不要求用户把 `data` 提取为单独模型。查询应追到原字段 symbol 及声明，不只返回桥接别名的说明。

先比较以下两个候选，验证后再冻结 helper ABI 和具体布局：

| 候选 | 实现方向 | 必须验证的边界 |
| --- | --- | --- |
| A：原位 shadow 检查 | 在原脚本 contextual typing 位置插入内存检查体，保留原文件种类和路径环境 | 不破坏泛型与 contextual `this`；JS 只使用合法 JS 和适用 JSDoc；插入前后原代码类型不变 |
| B：符号桥加独立 TS 检查文件 | 在同一派生类型上下文中，通过真实声明与可引用锚点连接类型 | 无导出/闭包内注册、未导出局部类型、类型身份和文档保留；不能靠新增 export 或提取 options 后假设推断不变 |

A 通过可先用于 JS 原位检查；B 通过才考虑统一为独立 TS 检查文件。两者均不能满足某种写法时明确局部不支持，不要求用户改源码，也不伪造类型补洞。禁止将 `typeToString()` 输出重新编译作为完整类型桥，禁止将整个 JS 改名为 TS 后视为等价。

### JS 与模板检查策略

定义显式 `templateCheckPolicy`，建议提供启用、继承脚本设置、关闭三种行为，并返回实际生效策略；配置字面值在接口冻结时确定。建议初始默认独立启用模板检查，但原脚本仍按其原配置分析，`strictNullChecks` 等增强策略不能静默开启。

候选 A 可验证只在派生 JS 视图启用文件级检查，但该设置并非只影响生成方法，必须对比注入前后语义，仅发布可归因于 WXML 的诊断。`@ts-nocheck` 的处理单独定义，不能由 pragma 插入顺序偶然决定。错误落在 helper 或探针声明上应报告桥接失败/覆盖不足，不能过滤后返回“检查成功”。

检查状态与类型质量分别描述：

- 检查状态：`Checked / Partial / Unsupported / Disabled / Unavailable`。
- 类型质量：已知真实类型、`any`、缺少类型来源等；合法存在的 `any` 也不代表严格校验。

### 源码映射与坐标

```text
WXML 原文字节范围
  ↕ Rust 模板映射
CheckFragment 字节范围
  ↕ Node 原样片段插入映射
shadow 脚本的绝对 UTF-16 范围

原业务脚本范围 ↔ shadow 脚本范围（定义跳转还原）
```

| 层次 | 坐标单位 | 绑定身份 |
| --- | --- | --- |
| 原始 WXML、JS/TS、syntax AST | 全文 UTF-8 字节半开范围 | 文件 + `SourceRevision` |
| Tree-sitter Point | 零基行号 + 行内字节列 | 对应文本版本 |
| 检查片段 | 全文字节半开范围 | `FragmentId` + `ViewRevision` |
| Node 查询和返回 span | 全文绝对 UTF-16 偏移与长度 | 实际文件 + `BackendFileVersion` + 分析快照 + 后端世代 |
| LSP Position/Range | 零基行号 + 行内 UTF-16 列 | 对应源文件版本 |

实现时用不同的类型表示源文件字节范围、片段字节范围和后端 UTF-16 span。转换必须使用同一文本快照；定义目标使用目标文件自己的版本和索引。CRLF 不静默归一化，若引入转换则显式维护映射。

Node 不重排、缩进、规范化换行或重新打印 Rust 片段，分隔符放在 wrapper 内。为每个实际插入落点返回 `fragmentId`、`fragmentRevision`、`viewRevision`、`backendFileVersion`、`startUtf16`、`lengthUtf16`，偏移计入此前所有插入；一个全局 wrapper 偏移不足以表示多个落点。

类型层组合两张映射。根访问前缀和 helper 标为 synthetic，原标识符保留精确映射；允许原表达式映射到多处生成代码。跨来源或跨 synthetic 的诊断须分类归因，不能整体平移或贴到最近字符。补全的替换范围及附加编辑无法安全反向映射时不返回；后端 helper 名称不能泄露为用户候选。

## Alternatives Considered

| 方案 | Pros | Cons | Why not |
| --- | --- | --- | --- |
| 正则提取并改写表达式 | 原型代码少 | 无法可靠识别嵌套、作用域、对象键和输入中间态 | 不满足类型语义与精确映射要求 |
| Rust 与 Node 各自重写模板 | 单端局部实现方便 | 双份语义、位置信息漂移、难以定位责任 | Rust 独占片段，Node 只插入 |
| 将推断类型转成字符串再生成声明 | 容易建立独立检查文件 | 丢失类型身份、私有名、声明及文档关系 | 不作为准确类型来源 |
| 要求导出或改写用户 Page/Component | 上下文更易引用 | 侵入业务源码，偏离普通写法零改动需求 | 仅允许内存视图 |
| 将独立 `interface`/模型文件作为模板数据根 | 检查视图可直接引用类型，构造简单 | WXML 运行时只绑定 `data`/`properties`；独立类型不是数据源，会产生虚假根和漏检 | 仅让 `data`/`properties` 字段引用该类型 |
| 直接固定某种探针注入形式 | 更早冻结实现 | 当前证据不足，可能破坏 JS 或 contextual typing | 先验收候选方案，再冻结布局 |

## Consequences

### Positive

- 模板语义与真实类型检查边界明确，两端不会重复维护表达式改写。
- 字段文档、定义及泛型等沿真实声明关系查询。
- 精确映射与覆盖状态可以独立测试，能识别“没有检查”的空结果。

### Negative

- 类型桥和双向映射是核心实现成本，必须先完成验证。
- 输入中间态、特殊属性及共享模板增加语义和映射分支。
- 需要维护 JS/TS 方言、helper ABI 和显式诊断策略。

### Risks

- 注入可能改变模块分类或上下文推断：以原项目/派生视图对比测试阻止回归。
- JS 关闭检查造成漏检：错误字段必须产生精确模板诊断，hover 不作为充分证据。
- 映射落入 helper 或跨来源：标记覆盖不足，不发布猜测位置，不返回不安全编辑。

## Validation

类型桥候选共用以下淘汰条件，全部保留正例与反例：

1. 裸 Page/Component（含其 `data`/`properties` 字段）、局部 JSDoc typedef 和闭包内注册不要求改源码；未覆盖写法明确记录。
2. 跨文件类型、泛型、递归、联合和带私有成员的类保持预期类型关系，不只比较 hover 字符串。
3. `user.name` 展示 `data.user.name` 的实际类型和该字段文档，`user.nmae` 只标记错误字段；`title` 展示 `properties.title` 声明处的类型和文档。
4. JS 的 `checkJs` 开关、文件 pragma、`allowJs` 和 `noImplicitThis` 的适用组合逐项记录支持或降级；无关 JS 错误不重复发布。
5. 未保存的 `data`/`properties` 字段类型及纯 JSDoc 更新可见，定义跳转回到原声明。
6. 中文、emoji、CRLF、多个片段插入、一对多映射和不完整表达式位置正确。
7. 原始脚本模块分类、类型和诊断对比通过；helper 错误不能被计作模板检查成功。
8. 独立 `interface`/模型文件不被当作模板根；只有被 `data`/`properties` 字段引用时才生效；未声明的 `properties` 不被伪造。

这些是待运行的验收，不是本 ADR 新增实验结果。

## References

- [需求 T1–T9 与类型增强](../../README.md#31-第一优先级模板类型体验)
- [已有实验的证据边界](../../README.md#6-已有验证与证据边界)
- [证据索引](../evidence/README.md)
- [ADR-0001：分层与职责](0001-layered-language-service-architecture.md)
- [ADR-0003：版本和后端协议](0003-versioned-document-synchronization.md)
- [ADR-0004：验收闸门](0004-parallel-delivery-and-validation-gates.md)
