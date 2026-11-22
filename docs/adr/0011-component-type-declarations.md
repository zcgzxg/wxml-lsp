# ADR-0011: 组件类型声明与 components.d.ts 固定发现约定

**Date**: 2026-11-20

**Status**: accepted（架构约定；实现与验收进度以代码、测试和变更记录为准）

**Deciders**: 项目维护者

**局部替代**：[ADR-0013](0013-engine-checked-component-bindings.md)（2026-11-22 accepted）替代本文第 4 节的事件签名结构比较，改为引擎函数赋值判定；发现规则、声明优先、事件名 `Warning` 与 hover 形态不变。

**局部补充**：[ADR-0002](0002-template-type-checking-and-source-maps.md) 的模板类型检查与 [ADR-0010](0010-zero-config-lazy-discovery.md) 的零配置发现——本文只新增「组件属性与事件的类型来源」，分层依赖、引擎虚拟快照、不读取 `node_modules` 等既有边界不变。ADR-0001 的模块依赖方向仍是硬约束。

## Context

组件属性与事件的类型目前只有两条来源，且都不覆盖第三方库组件：

1. **脚本静态推断**。`Component({ properties })` 能被解析成属性事实，事件只能知道「有绑定」而不知道契约。npm 包组件（`/miniprogram_npm/@vant/weapp/button/index` 这类路径）的脚本不在项目源码内，推断结果为空。
2. **类型引擎查询**。引擎只挂载固定库内 typings 与项目文件；ADR-0010 明确不扫描 `node_modules`，因此包内 `.d.ts` 不可达，引擎也回答不了「这个组件接受哪些属性」。

同时有三条既定约束决定了实现形态：

- 生产路径没有 Node 运行时（[ADR-0012](0012-in-process-tsrs-session.md)），不能引入 `typescript` 包做声明解析或生成。
- 语言服务对用户工作区只读：不能为了事件校验给 handler 形参注入类型，也不能把生成的 `.d.ts` 写回磁盘。
- 诊断必须可解释降级：按 [类型诊断参考](../reference/type-diagnostics.md) 的约定，能力不完整时状态为 `Partial` 并给出原因，不能静默猜测类型。

可选的事实来源已经存在：源码收集器 `collect_sources` 按预算收集 `.d.ts`（overlay 优先、不跟随 symlink、带排除目录表）。声明发现因此只需要一个稳定的「哪份 `.d.ts` 生效」规则，不需要新的遍历器。

## Decision

### 1. 发现规则是固定约定：`tsconfig.json` 同级的 `components.d.ts`

- 组件类型声明只从**项目 `tsconfig.json` 所在目录的同级 `components.d.ts`** 读取；该文件是唯一入口，不按目录名约定、不按命名模式匹配。
- 不读取 tsconfig 的 `files`/`include`/`exclude`/`paths` 来决定声明文件，不实现 glob 匹配，不扫描 `node_modules`、`bower_components`、`jspm_packages` 或编译输出目录，也不做向上/向下多目录查找。
- 启用 = 新增该文件；关闭 = 删除该文件。用户不需要改 tsconfig，也不需要理解 tsconfig 的匹配语义。
- 候选集仍来自既有 `SourceSet`（因此 overlay 中的 `components.d.ts` 与磁盘文件同等可见），只做「路径等于 `<tsconfig 目录>/components.d.ts`」的精确过滤，不引入第二套遍历、预算或排除表。
- 声明文件的规模不单独设上限：文件数与总字节由 `SourceSet` 的既有预算（`SourceLimits`）约束，单个 `components.d.ts` 受 `files::MAX_FILE_BYTES` 限制。
- `components.d.ts` 缺失、为空或无法读取时不产生任何组件事实，也不发布诊断——没有声明文件的项目行为与本次改动前完全一致。

### 2. 声明格式以全局映射表为入口，类型只接受自包含子集

声明文件通过 `WxmlComponentMap` 把 WXML 标签映射到组件接口，接口内用 `props`/`events` 两段描述契约：

```ts
declare global {
  interface WxmlComponentMap {
    'van-button': VanButtonProps;
  }
  interface VanButtonProps {
    props: { type?: 'primary' | 'default'; loading?: boolean };
    events: { change: { detail: { index: number } } };
  }
}
export {};
```

- `WxmlComponentMap` 位于 `declare global` 内或文件顶层时同样生效；映射目标接口必须在同一文件内声明。未被映射表引用的接口不产生任何事实（样本里的 `OrphanButtonProps` 即此用途）。
- 类型判定统一走一个 `TypeText` 迷你语法树：基础类型、字符串/数字/布尔字面量与联合、数组、`Record<string, T>`、嵌套对象。解析不出的成分把该成员标记为未证明，`type_text` 保留原文用于 hover，不重排用户的 union 顺序。
- 成员级 fail-closed：某个属性/事件类型未证明只影响该成员，其他成员照常生效，组件与整体状态降级为 `Partial` 并给出可读原因（声明文件缺失、声明文件解析不完整、类型未证明等）。
- 单个文件入口也消除了跨文件同名标签的优先级问题：一个标签只可能有一个声明；文件内重复映射按文档顺序取首个并记录冲突说明。
- 事实对象在 `Project` 上以 `Arc` 持有，`rediscover` 时重算；视图层用「`components.d.ts` 的 `(path, revision)` + 项目版本」做 digest，digest 变化才重新解析，因此修改该文件的 overlay 会触发重算。

`typeRoots`、`types`、`references`、把 `paths` 重写到 npm 包、从组件源码生成 `.d.ts` 都不在本决策范围内。

### 3. 声明优先于脚本推断，冲突在合并点一次决定

- 属性事实增加来源标记（脚本 / 声明）。视图组件构造时先取脚本事实，再用声明事实整表替换并置为声明来源，事件链路直接读声明事实。
- `apply_live_facts` 在来源为声明时直接返回，用户改动脚本 `properties` 的 overlay 不会覆盖声明。
- 引擎侧的脚本推导（`analyze_component`）在视图已带声明事实时跳过，省一次查询。
- 声明缺失或不完整时回落到脚本事实，未命中声明文件的项目行为与本次改动前完全一致。

因此优先级只有一条规则：**声明 > 脚本推断**，且规则落在合并点，而不是分散在各消费方。

### 4. 事件契约用静态参数标注校验，不做形参注入

服务只读，无法改写脚本，所以事件签名校验按「静态标注 + 结构比较」实现：

- 脚本提取时同时记录处理函数第一个形参的类型标注原文（方法简写、`function` 属性、箭头函数一致处理；无参数或未标注为 `None`）。
- 事件解析接收可选的「事件名 → detail 类型 + 可证明性」表，返回三态：`Skipped(原因)` / `Compatible` / `Incompatible`。比较用 `TypeText` 做规范化结构比较（空白、union 顺序、可选标记无关），不做结构化子类型。
- 只有 `Incompatible` 才在 handler 名 token 上发布 `Error`；处理函数不存在时沿用既有缺失 `Error` 且不叠加类型诊断；detail 未证明或参数未标注时 `Skipped` 并把状态置为 `Partial`。
- 未声明事件名诊断只在组件 `events` 声明完整时启用，级别 `Warning`，范围落在事件名 token；动态绑定与声明缺失不告警。
- 事件名补全优先返回声明事件（仅完整声明），内置组件与无声明组件保持既有候选；hover 用 `WechatMiniprogram.CustomEvent<T>` 展示 detail 原文，未证明时明确标注而不是猜测。
- 处理函数存在性检查、definition、handler hover 与 handler 补全的行为不变。

### 5. 非目标

本次决策不实现完整的 TypeScript 类型检查器，只覆盖上表子集；不做声明生成器；不新增 LSP 协议或 `components.d.ts` 以外的 IO；不改写用户工作区文件。

## Alternatives Considered

| 方案 | 优点 | 代价及不采用原因 |
| --- | --- | --- |
| 用 tsconfig 的 `files`/`include`/`exclude` 决定声明文件 | 与编辑器类型项目边界一致，可精确控制 | 需要实现模式 rebase、glob 匹配、`extends` 分层语义和逃逸处理；用户写错 `include` 时只有「零效果」，难以解释。收益（多文件、多目录）在当前场景不存在 |
| 约定固定目录（如 `typings/components.d.ts`） | 位置明确 | 多一层目录，且与「项目根只有一个 tsconfig」的直觉不一致；`tsconfig.json` 同级更直观 |
| 扫描项目内所有 `.d.ts` 并解析其中的 `WxmlComponentMap` | 用户无需新增文件 | 需要遍历与预算控制，且会命中 engine typings、生成产物和无关库声明；解析成本与误报面都不可控 |
| 引入 glob crate 实现模式匹配 | 少写代码 | 触发 `cargo deny` 许可与重复版本评估；在固定单文件规则下完全没有必要 |
| 让引擎挂载 `node_modules` 或用 Node 解析声明 | 类型信息最完整，无需自研解析器 | 破坏 ADR-0010 的「不扫 `node_modules`」与快照确定性，并重新引入 Node 运行时（Node 后端已删除，见 ADR-0012） |
| 只扩展脚本推断，不引入声明文件 | 无新格式 | 第三方库组件没有项目内脚本，永远拿不到属性与事件契约 |
| 事件校验用 shadow 形参注入 + 引擎类型比较 | 复用真实类型检查器，语义最准 | 服务只读，注入需要改写用户脚本；且默认门禁不启动引擎，该行为无法用普通单测覆盖 |

## Consequences

### Positive

- 第三方与自定义组件的属性、事件 detail 有了可声明、可发现的类型来源，不必把包源码放进项目。
- 发现规则只有一个路径，用户不需要理解 tsconfig 匹配语义；启用/关闭就是新增/删除文件，便于回滚与逐项目试点。
- 复用既有源码收集预算与 overlay 逻辑，不新增遍历器、排除表或 glob 语义。
- 优先级规则集中在合并点，overlay、hover、补全、诊断看到的是同一份事实。
- 事件签名校验完全静态，可在默认 `cargo test` 下用普通单测覆盖；owner scope 不需要引擎进程。
- 未放置声明文件的项目行为与改动前一致。

### Negative

- 一个项目只能有一份组件声明文件：多个第三方库、多个小程序包需要合并进同一文件，文件会变长；monorepo 中每个 tsconfig 目录各放一份，内容可能重复。
- 声明解析器天然是「半个 TS 编译器」：子集固定，新语法必须伴随测试再扩展，否则成员降级为未证明。
- 声明文件放在 tsconfig 同级而 tsconfig 的 `include` 未覆盖它时，编辑器自身的类型检查看不到该文件；它只对语言服务生效。
- 声明优先会让脚本里 `properties` 的改动「看起来没生效」，需要在文档里解释这是设计意图。
- 事件名 `Warning` 有误报可能：库可能 `triggerEvent` 未声明的事件，只能靠声明完整性与删除声明文件控制。

### Risks

- 固定单文件规则可能无法满足大型多包项目 → 保留「声明缺失即回落到脚本」的路径；后续以新 ADR 扩展多文件规则，而不是在本文里预留隐式行为。
- 声明文件被误放到子目录导致零效果 → 文档与样本固定「与 `tsconfig.json` 同级」，状态原因中给出「声明文件未命中」。
- 静态签名比较漏报 → 只承诺「可证明时报告」，未知类型与未标注参数一律 `Partial`，不声称检查通过。
- 声明与脚本冲突导致诊断翻转 → 冲突取声明是明确规则，并用样本固定行为（见 [`components-typings`](../../tests/fixtures/README.md)）。

## Validation

验收口径（实现完成后按此核对，本文不声称已通过）：

- `wxml-project` 单测覆盖：同级 `components.d.ts` 被读取、其他位置同名文件被忽略、文件缺失时无事实、声明解析（含未被引用接口不生效、events 带 detail、截断）、`rediscover` 后事实重算。
- `wxml-lsp` 用 HostOnly 覆盖属性未声明、字面量不匹配、可选缺省、未证明不报、事件名 `Warning`、补全候选、hover 文本与降级原因。
- 无声明文件的项目诊断集合与改动前一致；新增诊断只出现在放置了声明文件的样本上。
- `tests/fixtures/components-typings/` 作为真实小程序工程承载上述样例，其目录与人工样例见[共享验收样本说明](../../tests/fixtures/README.md)。
- 统一门禁：`cargo fmt`、`cargo clippy -D warnings`、`cargo test --workspace --all-features`、`cargo doc`、`cargo deny --offline check`。

## References

- [ADR-0001：分层语言服务与模块依赖边界](0001-layered-language-service-architecture.md)
- [ADR-0002：模板类型检查、类型后端与源码映射](0002-template-type-checking-and-source-maps.md)
- [ADR-0012：进程内 tsrs 类型引擎会话与故障隔离](0012-in-process-tsrs-session.md)
- [ADR-0009：项目瘦身](0009-project-slimming.md)
- [ADR-0010：零配置惰性小程序发现与工作区包挂载](0010-zero-config-lazy-discovery.md)
- [类型诊断参考](../reference/type-diagnostics.md)
- [共享验收样本说明](../../tests/fixtures/README.md)
