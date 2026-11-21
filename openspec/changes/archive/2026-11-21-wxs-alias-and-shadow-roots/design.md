# Design

## Context

模板表达式进入 shadow 后，`rewrite_expression` 把每个自由根改写为 `__wxml_root.<name>`。WXS 模块名只存在于 `<wxs module="...">`，不在组件 data 上，因此 `m1` 被挂成 `__wxml_root.m1` 并触发 tsgo 的 `Property 'm1' does not exist`。静态 WXS hover 走另一条路径，所以出现“hover 正常但诊断标红”。

WXS 摘要对 `module.exports = { name: local }` 只识别函数字面量、`require` 与箭头函数。裸标识符落到 `Value`，类型推断为返回值的类型（如 `string`），丢失签名。

## Decisions

### 1. 模块名作为 shadow 局部值，而非 data 根

- `PreparedView` 增加 `wxs_modules: Arc<[String]>`，来源是 `wxml_types::analyze_template_semantics(&parsed.tree).wxs` 中 `module` 为合法绑定标识符的值，去重并保持首次出现顺序。
- 过滤 shadow 保留名（`__wxml_root`、`__wxml_props_0`、`__wxml_props_raw_0`、`__wxml_check_0` 以及 `__wxml_` 前缀），避免 probe 内重复声明。
- `FragmentSpec` 增加 `wxs_modules: Vec<String>`；`fragment_spec` 从 view 填充。`FragmentSpec::new` 默认为空，成员/补全字面量同步补字段。
- `effective_local_names` 把片段携带的 WXS 模块名并入 locals，使 `rewrite_expression` 保留原文。
- 在 probe 方法体顶部、`const __wxml_root = this.data;` 之后，按所有片段模块名的并集声明一次：
  `const <name>: any = null as any;`
  声明是纯生成文本，不建立 source 映射，也不增加 `unresolved_bindings`，因此不会降级诊断状态。
- `fragment_eligibility` 继续只用 `bindings` 判断自由根，WXS 模块根仍使片段 Eligible，保证只引用模块的表达式也会挂载。

选择 `any` 而不是从 WXS 摘要生成结构化类型：WXS 的既有能力声明是“摘要，不执行、不类型检查”；动态导出、`module.exports[key]`、require 链都无法稳定建模。用 `any` 消除误报，同时不制造虚假的类型保证。

### 2. 别名导出继承本地成员

在 `classify_initializer` 中，当初始值在跳过注释后恰好是一个标识符，且 `local_by_name` 中存在同名条目时，返回该条目的 `(kind, type_text, signature)`，与简写导出分支一致。`module_specifier` 保持 `None`；本地 `require` 别名的模块信息不在此处重建，避免猜错路径。

查找使用与简写分支相同的“首个匹配”语义。由于摘要按源码顺序建立，声明位于导出之后（依赖函数提升）时不会命中，此时保持原有的保守 `Value`/`any` 行为，不新增伪签名。

## Alternatives

- **为 WXS 生成完整结构化类型**：能给模板中的 WXS 调用做类型检查，但与“不做类型检查”的公开约定冲突，且动态导出必须整体退化为 `any`，收益不稳定。暂不做。
- **在 shadow 侧删除 WXS 根表达式**：能消除误报，但会连“混合 data 表达式”中的真实错误一起跳过，违反既有检查。
- **把模块名声明成 `const m1 = {}`**：会让成员访问报错或得到错误的字面量类型，不如 `any` 诚实。

## Risks

- 模块名与 `wx:for` 局部变量同名：顶层 `const` 与块内局部声明并存，块内局部遮蔽；与 WXML 作用域一致。
- 模块名与 data 字段同名：表达式按模块解析（局部优先），符合模板中 WXS 模块可直接引用的语义。
- 声明所有静态模块可能引入少量未使用常量：模块数量受模板静态预算限制，且只存在于内存 probe，不影响用户文件。
- 别名导出的查找依赖声明顺序：与现有简写导出一致；不命中时保守降级。

## Migration

无配置或协议变化。已安装引擎与 typings 不变。
