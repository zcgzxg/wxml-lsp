# Proposal

## Why

组件属性与事件兼容性目前由 Rust 侧一套受限的“第二类型系统”判断：属性表达式只接受标识符/成员链并按数组、对象、联合大类粗比；事件把类型兼容近似成结构相等。真实引擎对照实验显示，这会误报合法的事件参数宽化、对象额外字段、可选字段与字面量，同时漏检 `unknown`、导入泛型和额外必填参数；属性的数组元素、对象字段、联合、条件、索引、算术、泛型与映射类型不匹配也多未检查。同一批案例交给现有类型引擎后全部得到正确结果，说明这部分重复实现既增加代码，也降低正确性。

## What Changes

- **属性兼容判断交给引擎**：对属于组件属性的纯 mustache 表达式，在现有 shadow 内生成带期望类型的赋值检查。声明来源优先引用 `WxmlComponentMap['tag']['props']['name']` 原声明，保留泛型、类型别名与导入关系；脚本来源使用既有规范化 `type_text`（含 `optionalTypes`）。任意表达式形态由此获得真实可赋值性判断。
- **事件参数判断交给引擎**：用“把处理函数赋给接受该事件类型的回调”替代结构相等比较。保留无参数处理函数可用，并检出额外必填参数；遵循项目 `strictFunctionTypes` 等编译选项。
- **为契约检查建立诊断归属**：合成赋值产生的引擎诊断常落在生成的变量名上，现有 source map 会丢弃。为每条契约检查记录其对应的属性表达式或处理函数 token，只把确认属于该检查的兼容性错误映射回 WXML；声明解析失败等生成代码错误不得猜测位置。
- **删除被替代的手写比较**：移除 `compatible`/`union_eq`/`object_eq` 事件比较、属性 `PropertyContract` 粗分类与单 token 查询路径中仅供兼容判断的部分。声明发现、WXML 值形态语义、`Partial` 原因与上限继续保留。
- **保留 WXML 规则**：组件解析、属性命名与保留属性、字面量/空属性/混合文本、可选与未设置策略仍由 Rust 决定；`any`/`unknown`/`never` 的产品策略不随 TS 默认规则改变。
- **记录决策**：新增 ADR，明确 ADR-0002 “不把所有属性插值直接当作目标类型赋值”的边界——只对纯 mustache 组件属性与事件处理函数使用引擎赋值探针，字面量与混合文本不进入该路径。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

- `component-type-declarations`: “属性绑定按声明检查”从可证明类型子集 + 粗分类扩展为引擎判定的任意表达式可赋值性；明确诊断范围、未证明/不可解析时的行为与不重复发布。
- `component-event-checking`: “已标注处理函数按 detail 校验”从结构相等近似改为引擎函数类型赋值；明确无参数处理函数、额外必填参数、严格函数参数检查与诊断范围。

## Impact

- 代码：`crates/wxml-backend-tsrs/src/shadow/{layout,layout 探针构建}.rs`、`crates/wxml-lsp/src/backend_tsrs/{input,diagnostics,component_bindings}.rs`、`crates/wxml-project/src/typing_contract.rs` 及相关测试。
- 数据流：`CheckFragment` 需要携带期望类型与诊断归属；`PreparedView` 需要把组件属性事实传给 shadow 规划，避免 LSP 层再次独立查询同一表达式。
- 行为：合法绑定不再误报；引擎可判定的不匹配新增诊断。`Partial` 的触发条件随之缩小，但声明不完整、类型不可解析、混合文本等仍保持降级。
- 公开 API：`component_bindings` 内部的兼容枚举与比较函数删除或收敛；跨 crate 的 `Backend`/`Project` 接口不新增类型查询 API。
- 文档：新增 `docs/adr/0013-*.md`，更新 `docs/reference/type-diagnostics.md` 中属性/事件检查规则。
- 依赖与资产：无新增依赖，`tsrs` pin 与 `miniprogram-api-typings` 版本不变。
- 验证：默认 cargo 测试保持绿；真实引擎套件需覆盖属性/事件矩阵、诊断锚点、CJK/CRLF 范围与 JS `@ts-check`。
