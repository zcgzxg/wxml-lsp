# ADR-0013: 组件属性与事件的引擎判定绑定检查

**Date**: 2026-11-22

**Status**: accepted（架构约定；实现与验收进度以代码、测试和 openspec 变更记录为准）

**Deciders**: 项目维护者

**局部补充**：[ADR-0002](0002-template-type-checking-and-source-maps.md) 的「组件属性」检查边界与 [ADR-0011](0011-component-type-declarations.md) 第 4 节的事件签名校验。本文同时替代 ADR-0011 中“Rust 类型子集决定是否检查”的边界：模板解析、事实来源、发现规则、只读边界、类型后端与映射原则均不变，兼容性判断改由引擎执行。ADR-0001 的模块依赖方向仍是硬约束。

## Context

组件属性与事件处理函数的兼容性此前由 Rust 侧第二套类型判断承担：

- 属性：`component_bindings` 用 `type_query` 查询表达式类型，只接受标识符/成员链，并按数组、对象、联合大类粗比。
- 事件：`typing_contract` 用 `TypeText` 做规范化结构相等（见 [ADR-0011](0011-component-type-declarations.md) 第 4 节），把类型兼容近似成「成员与联合数量相同」。

这套实现在原理上无法表达 TypeScript 的可赋值性：结构兼容的额外字段、必填到可选、字面量到基础类型、参数宽化会被误判为不兼容；`unknown`、导入泛型、额外必填参数会被漏检；数组元素、对象字段、联合、条件、索引、算术、泛型与映射类型不匹配大多没有进入比较。补全这些规则等于在 Rust 侧重写半个类型检查器，并持续与引擎语义漂移。

同时有五条既定约束决定实现形态：

- [ADR-0002](0002-template-type-checking-and-source-maps.md) 已经规定：模板属性不能一律当作目标类型的赋值，因为字面量与混合文本有运行时字符串化、布尔属性有真值转换。该约束仍然正确，问题只在于它没有区分「纯 mustache 表达式」和「值形态」。
- [ADR-0011](0011-component-type-declarations.md) 的声明发现、声明优先和事件名规则只解决「契约从哪来」，没有解决「兼容性怎么判」。
- [ADR-0012](0012-in-process-tsrs-session.md) 提供进程内类型会话；shadow 已经在同一份快照上插入片段并运行 `getSemanticDiagnostics`，映射层已能把 mount 内的诊断映射回模板。
- 语言服务对用户工作区只读：不能为了事件校验改写处理函数形参，也不能把生成的声明写回磁盘。
- 默认 `cargo test` 不要求真实类型引擎；降级必须可见，不能用空诊断表示「检查通过」（[类型诊断参考](../reference/type-diagnostics.md)）。

## Decision

### 1. 兼容性归引擎，事实与 WXML 规则归 Rust

- **Rust 保留**：组件解析、声明与脚本 `properties` 的事实解析、属性名 kebab/camel 归一化、保留属性豁免、字面量/空属性/混合文本的值形态规则、事件名检查、处理函数存在性、hover/definition/补全、`Partial` 原因与上限。
- **引擎判定**：纯 mustache 组件属性表达式对目标类型的可赋值性；静态处理函数对事件回调类型的函数参数兼容性。
- 不保留第三条判断路径：被替代的手写比较删除（见第 6 节），引擎不可用时不回退到手写结构比较。

理由：类型身份、泛型实例化、条件/索引/映射类型、`strictFunctionTypes` 等语义只有引擎能给出；Rust 侧只需提供事实和挂载点。

### 2. 契约探针挂在既有 fragment 上，不新增引擎 API

`wxml-backend-tsrs` 在 shadow 层为每条契约生成一次上下文类型赋值：

```ts
const __wxml_bind_<id>: <expected> = <rewritten>;
void __wxml_bind_<id>;
```

JS 在 `checkJs` 或文件 `@ts-check` 生效时使用 JSDoc 形式：

```js
/** @type {<expected>} */
const __wxml_bind_<id> = <rewritten>;
void __wxml_bind_<id>;
```

- 探针描述（种类、期望类型、展示文本、WXML 锚点）挂在 `FragmentSpec` 上，与 fragment 共享 block 作用域、`wx:for` 局部变量和 `wx:if` 收窄，不另建表达式挂载体系。
- `<rewritten>` 复用 fragment 现有重写结果；事件使用 member 模式 `this.<handler>`。
- 每条契约替换该 fragment 原有的裸表达式语句，不追加第二条，避免同一表达式被检查两次；`void` 消费合成变量，避免 `noUnusedLocals` 噪音。
- 不新增 `isTypeAssignableTo` 或 checker 内部 API，不新增进程、超时或资源上限常量；契约探针复用同一次 shadow 与同一次 `getSemanticDiagnostics`。

### 3. 期望类型引用原声明，不重新打印

| 来源 | 期望类型 |
| --- | --- |
| 声明 `props` | `WxmlComponentMap['<tag>']['props']['<declaredName>']`；`optional` 为真时与 `undefined` 求联合 |
| 脚本 `properties` | 已合并 `type`/`optionalTypes` 的规范化 `type_text`，不使用 `this.properties` 实例类型 |
| 声明 `events` | `(event: WechatMiniprogram.CustomEvent<WxmlComponentMap['<tag>']['events']['<declaredName>']['detail']>) => void` |

- 索引使用声明的成员名（归一化后），不使用模板拼写。
- 声明路径始终引用原声明，保留泛型、类型别名、导入类型与私有名的真实身份；不把 `TypeText` 重新打印成期望类型。
- 声明文件必须进入 shadow 的 `AnalysisInput`；模块顶层映射通过类型级引用指向该声明文件，仍不扫描 `node_modules`。引用无法解析时标 `Partial`，不猜测类型。
- 现状缺口：当 source root 覆盖 `components.d.ts` 所在目录（例如 `include_project_root` 为真）时该要求成立；但在显式分层根（例如 `root`/source root 为 `apps/a/src`、`projectRoot` 为 `apps/a`、`components.d.ts` 位于 `apps/a`）下，预备输入只从 source roots 收集文件，声明文件没有独立挂载路径，存在漏挂载。该路径尚未完整验证，列为待后续实现，不宣称已普遍落实。

### 4. 诊断锚点显式归属于契约

合成赋值产生的错误通常落在生成变量名 `__wxml_bind_<id>` 上，现有 source map 无法反向映射。shadow 因此为每条探针输出锚点（生成文本中的变量名范围、原 WXML 范围、种类、期望类型展示文本），诊断层按以下顺序处理：

1. 命中锚点且 code 为 `2322`/`2345`/`2739`/`2741`：发布到 WXML 锚点，使用引擎消息并做有界截断；同一绑定只保留一条。`2739`/`2741` 是缺少必需属性的赋值错误，与 `2322`/`2345` 同样归属到绑定表达式。
2. 契约探针上的 `2353`（对象字面量多余属性）：丢弃。WXML 语义允许结构兼容的额外字段，其余属性/值错误仍由 `2322`/`2345`/`2739`/`2741` 报告。
3. `6133`/`6196` 等未使用或合成声明噪音：丢弃。
4. 落在源表达式段内的其他诊断：沿用现有映射，例如属性不存在、表达式内部类型错误。
5. 落在声明、类型注解等纯生成文本，或期望类型解析失败（如 `2304`/`2503`）：不发布模板诊断，把该组件/事件标为 `Partial` 并给出原因。

不按「最近 token」猜测位置；无法归属就不发布。这条规则是 ADR-0002「跨来源或跨 synthetic 的诊断须分类归因，不能整体平移或贴到最近字符」在契约探针上的具体化。

### 5. 不进入引擎赋值路径的输入

| 情况 | 行为 |
| --- | --- |
| 字面量属性（`type="primary"`） | 继续按 WXML 值形态规则在 Rust 判定，不生成探针 |
| 空属性（`disabled`） | 同上 |
| 混合文本（`title="共 {{ n }} 个"`） | 结果恒为字符串，保持既有 `Partial` 覆盖；表达式内部错误仍走普通映射 |
| 表达式有语法错误 | 不生成探针，沿用既有 `Malformed` 计数并按需 `Partial` |
| 目标类型为 `any` | 不生成探针，视为干净，状态不变 |
| 目标类型为 `unknown`/`never` | 不生成探针，`Partial` 并给出原因 |
| 脚本 `properties` 不可完整规范化 | 不生成探针，该属性 `Partial`，其他属性照常检查 |
| 声明成员无法被引擎解析 | 不发布该成员诊断，`Partial` |
| 处理函数无显式参数标注 | 不生成探针，`Partial`；`arity == 0` 仍生成探针并视为兼容 |
| 处理函数缺失 | 不生成探针；缺失 `Error` 由既有事件链发布，不叠加参数诊断 |
| 引擎不可用 | 走既有 HostOnly/Unavailable 路径，不发布契约诊断 |

编译选项与 JS：

- 探针不修改项目编译选项，结果按项目实际 `strict`/`strictFunctionTypes` 判定；关闭 `strictFunctionTypes` 时不额外施加严格函数参数检查。
- JS 只有在项目 `checkJs` 为真或文件已有 `@ts-check` 时才生成探针；否则不生成并标 `Partial`，不声称已检查。
- 方法简写（`onChange(event: E) {}`）与函数属性/箭头函数的参数双变差异属于 TypeScript 自身规则；本 ADR 不复制矩阵，按[类型诊断参考 §7.2](../reference/type-diagnostics.md#72-事件绑定)执行。
- 生成代码自身的错误（声明解析、期望类型无法解析、合成变量上无法归属的诊断）一律不外溢为模板诊断。

### 6. 被删除的手写比较

删除：

- `component_bindings` 中 mustache 的 `type_query` 路径：`Target`/`Actual`/`Verdict`、`PropertyContract` 的数组/对象/联合分类、`chain_target`、`type_flags`、`check_declared_mustache`。
- `typing_contract` 中事件参数的结构相等比较：`compatible`、`union_eq`、`object_eq`、`is_permissive`；`event_diagnostics` 不再调用 `check_handler`。
- `component_bindings` 自己重复的组件脚本事实解析，改为与 `input` 共用同一份解析结果。

保留：

- 未声明属性、保留属性、kebab/camel、组件解析、`Partial` 原因与上限。
- 字面量/空属性/混合文本的 WXML 值规则。
- `TypeText` 的声明读取、hover 展示与可选性判断；它不再承担 mustache 可赋值性。
- 事件名检查、缺失处理函数、hover/definition/补全，以及 `expected_signature` 的 hover/补全签名展示。

因此同一绑定只可能产生一条兼容性诊断。

## 与 ADR-0002 的关系

- ADR-0002「组件属性」行的结论**继续有效**：不把所有属性插值直接当作目标类型赋值。本文只把它细化为两类输入：纯 mustache 组件属性与静态事件处理函数走引擎赋值探针；字面量、空属性、混合文本、布尔属性真值转换与文本节点渲染不进入该路径。
- ADR-0002 的 shadow、坐标分层、诊断归因与「helper 错误不得计作检查成功」原则不变；契约锚点是「合成文本诊断须分类归因」的一个实现，而不是新的映射体系。
- ADR-0002 要求类型质量与检查状态分开报告，本文沿用：`any` 视为干净但不代表严格校验，`unknown`/`never`/未标注参数/生成代码错误一律 `Partial`。

## 与 ADR-0011 的关系

- 发现规则、声明优先、`components.d.ts` 固定入口、事件名 `Warning` 与 hover 形态不变。
- 第 4 节的「静态标注 + `TypeText` 结构比较」由本文替代为「静态标注 + 引擎函数赋值」；「Rust 类型子集决定是否检查」的边界也由本文替代：`TypeText` 只负责声明结构与 hover，不再决定可赋值性。「不注入形参、不改写脚本、只读工作区」的约束保留，因此未标注参数仍为 `Partial`。
- ADR-0011 的非目标「不实现完整 TypeScript 类型检查器」仍然成立：Rust 不新增类型系统，只把判断交给已有引擎。

## Alternatives Considered

### 继续扩展手写比较（`TypeText` 结构化子类型）

- **Pros**：不依赖真实引擎，默认 `cargo test` 可覆盖全部规则；改动局限在 Rust。
- **Cons**：需要自行实现结构兼容、函数参数、泛型实例化、条件/索引/映射类型与导入类型身份；规则会与引擎持续漂移。
- **Why not**：这是重写类型检查器，而不是复用已有能力；正确性上限由手写子集决定。

### 直接调用 checker 的 `isTypeAssignableTo` 或新增引擎 API

- **Pros**：无需生成代码，诊断位置天然精确。
- **Cons**：绑定 `tsrs` 内部 checker 结构，超出 `Session` 请求/响应合同面；需要新协议、版本校验与升级差分。
- **Why not**：现有 shadow 与 `getSemanticDiagnostics` 已能表达同一判断，收益不足以抵消耦合。

### 把字面量、空属性和混合文本也改成 TS 赋值

- **Pros**：检查路径统一，规则只剩一套。
- **Cons**：与运行时字符串化、布尔真值转换和「文本节点不因非字符串报错」冲突，会产生误报。
- **Why not**：ADR-0002 已明确这些形式不是目标类型赋值。

### 给处理函数形参注入类型后比较

- **Pros**：无需显式标注也能检查。
- **Cons**：需要改写用户脚本或派生可变视图，破坏只读边界，且默认测试无法覆盖。
- **Why not**：只读工作区是硬约束；未标注参数保持 `Partial`。

### 用 `typeToString`/`TypeText` 重新打印声明类型作为期望类型

- **Pros**：生成文本更短，便于阅读。
- **Cons**：丢失导入、泛型参数、私有名与声明身份，可能把可解析类型变成不可解析文本。
- **Why not**：期望类型必须引用原声明。

### 手写比较与引擎检查并存

- **Pros**：引擎不可用时仍有近似结果。
- **Cons**：同一绑定可能出现两条口径不同的诊断；两套语义需要同时维护。
- **Why not**：宁可 `Partial`，也不发布互相矛盾的诊断。

## Consequences

### Positive

- 属性与事件使用真实可赋值性：结构兼容的额外字段、必填到可选、字面量到基础类型、参数宽化不再误报；`unknown`、导入泛型、额外必填参数与复杂属性类型不再漏检。
- Rust 不再维护第二套类型比较，兼容性规则只有引擎一处来源。
- 事实解析收敛为共享步骤，属性检查与 shadow 规划看到同一份组件事实。
- 生成变量上的兼容性错误有了显式锚点：不再被静默丢弃，也不会贴到最近字符。
- 复用同一次 shadow、快照与诊断请求，不增加逐表达式查询或第二个进程。

### Negative

- 契约检查依赖真实引擎；默认 `cargo test` 只能验证探针生成、锚点与降级，行为矩阵需要 opt-in 真实引擎门禁（见 [原生合同](../reference/tsrs-native-contract.md)）。
- Rust 仍需解析声明成员名、可选性与类型原文，并规范化脚本 `properties`；两层事实必须保持一致。
- 事件检查要求显式参数标注；未标注处理函数永远无法获得签名诊断。
- JS 项目在 `checkJs` 关闭且无 `@ts-check` 时没有契约诊断，只能得到 `Partial`。
- 声明文件必须能被 `AnalysisInput` 解析；模块布局超出引擎模块解析能力时退化为 `Partial`。

### Risks

- **生成代码诊断外溢**：`2353`、`6133`、`6196` 或期望类型解析错误若被当作模板诊断，会出现在用户没写过的位置 → 只发布命中锚点且 code 在 `{2322, 2345, 2739, 2741}` 的诊断，其余按第 4 节丢弃或降级，并用正反例固定。
- **声明文件未进入 shadow**：期望类型引用无法解析，只剩生成代码错误 → source root 覆盖声明目录时由 input 收集阶段补入，不扫描 `node_modules`；显式分层根（`root` 与 `projectRoot` 不同，声明位于 `projectRoot` 而非 source root）仍有漏挂载缺口，标为待后续实现，不作为已完成能力。
- **方法简写双变**：`onChange(event: E) {}` 在 `strictFunctionTypes` 下仍按方法双变处理，比函数属性/箭头宽松 → 这是 TypeScript 规则，按[类型诊断参考 §7.2](../reference/type-diagnostics.md#72-事件绑定)记录预期，不在本 ADR 另立一套判定。
- **编译选项差异**：`strictFunctionTypes` 开关会改变事件判定结果 → 以项目实际选项为准，双开关各有样本，不强行统一。
- **降级被误读为通过**：`any`、未标注参数、不可解析声明都返回 `Partial`/`Any` → 状态与原因必须随结果返回，文档不得把 `Partial` 描述成已支持。
- **回归**：删除手写比较可能改变未声明属性、事件名、handler 存在性或 hover → 无声明项目与 HostOnly 的诊断集合必须与改动前一致，并由测试断言。

## Validation

验收口径（实现完成后按此核对，本文不声称已通过）：

- 属性矩阵：基础类型、数组元素、对象字段、联合、条件、索引、算术、函数调用、泛型、映射类型、字面量联合；结构兼容的额外字段通过；`2353` 不发布；诊断落在绑定表达式。
- 事件矩阵：参数宽化、对象额外字段、必填到可选、字面量到基础类型、`unknown` detail、额外必填参数、无参数处理函数、未标注 `Partial`、缺失处理函数优先，以及 `strictFunctionTypes` 开关两侧。
- JS：`checkJs: false` 不生成探针并 `Partial`；`checkJs: true` 与 `@ts-check` 产生正确诊断。
- 范围：CJK、CRLF、多行 mustache 的锚点与现有映射一致；生成变量名上的错误映射到表达式或处理函数名 token。
- 声明可见性：source root 覆盖声明目录时断言声明文件进入 `AnalysisInput`；显式分层根（source root 与 `projectRoot` 不同）暂按待实现缺口记录，不能在验收中宣称已完整通过。
- 去重：同一绑定只出现一条兼容性诊断；旧路径删除后不残留手工比较结果。
- 无声明项目与 HostOnly：诊断集合与改动前一致；引擎不可用时不发布契约诊断。
- 统一门禁：`cargo fmt`、`cargo clippy -D warnings`、`cargo test --workspace --all-features`、`cargo doc`、`cargo deny --offline check`；真实引擎矩阵按[原生合同](../reference/tsrs-native-contract.md)的 opt-in 门禁串行运行。

任务拆分与规格增量见 [openspec 变更](../../openspec/changes/archive/2026-11-22-engine-checked-component-bindings/proposal.md)。

## References

- [ADR-0001：分层语言服务与模块依赖边界](0001-layered-language-service-architecture.md)
- [ADR-0002：模板类型检查、类型后端与源码映射](0002-template-type-checking-and-source-maps.md)
- [ADR-0011：组件类型声明与 components.d.ts 固定发现约定](0011-component-type-declarations.md)
- [ADR-0012：进程内 tsrs 类型引擎会话与故障隔离](0012-in-process-tsrs-session.md)
- [模板类型诊断策略](../reference/type-diagnostics.md)
- [原生类型后端合同](../reference/tsrs-native-contract.md)
- [共享验收样本说明](../../tests/fixtures/README.md)
- [openspec 变更：engine-checked-component-bindings](../../openspec/changes/archive/2026-11-22-engine-checked-component-bindings/proposal.md)
