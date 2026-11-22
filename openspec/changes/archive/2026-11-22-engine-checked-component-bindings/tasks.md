# Tasks

并行说明：每波内任务互不修改同一文件；有依赖的任务放到下一波。子任务只跑本 crate 的 `cargo check`/`cargo test`，全量验证由主代理统一执行。

## 1. 基础数据与探针设施（Wave 1，可并行）

- [x] 1.1 **wxml-types**：`EventHandler` 新增 `tag: String`（所属元素标签），模板分析器在生成事件时写入；更新所有构造点与测试。`cargo test -p wxml-types` 通过。
- [x] 1.2 **wxml-backend-tsrs shadow**：新增 `ContractKind`/`ContractSpec`/`ContractAnchor`，`FragmentSpec` 增加 `contract: Option<ContractSpec>`；layout 在同一 block 生成 `const __wxml_bind_<id>: <expected> = <rewritten>; void ...`，JS 用 JSDoc；member 模式复用 `this.<handler>`；`BuiltShadow`/`ShadowLayout` 暴露锚点。补 layout/mapping 单测（TS、JS、`wx:for`/`wx:if` 作用域、CJK/CRLF、探针只挂载一次）。`cargo test -p wxml-backend-tsrs` 通过。
- [x] 1.3 **wxml-lsp shared facts**：把组件属性事实解析（声明优先、脚本 `properties` 回退、behaviors 不完整）抽成 `component_bindings` 与 `input` 共用的函数，一次请求内按组件路径缓存；行为保持不变。`cargo test -p wxml-lsp` 通过。
- [x] 1.4 **文档**：新增 `docs/adr/0013-engine-checked-component-bindings.md`（决策、边界、与 ADR-0002 的关系），更新 `docs/adr/README.md`。不改代码。

## 2. 契约构建（Wave 2，单个子代理；集中修改 `input.rs`）

- [x] 2.1 属性契约：对每个 `ComponentAttribute` 的纯 mustache，按 `expr_range == CheckFragment.wxml_range` 找到 fragment，附加 `ContractSpec`；声明来源生成 `WxmlComponentMap['tag']['props']['name']`（可选追加 `| undefined`），脚本来源使用规范化 `type_text`。
- [x] 2.2 事件契约：从 `EventHandler.tag` + 事件声明 + `MethodTable` 生成 member 探针；仅显式参数标注、detail 可解析时生成；`arity == 0` 也生成。
- [x] 2.3 去重与挂载：契约 fragment 不受 `NoFreeRoot` 影响；与普通 fragment 合并为同一 spec，绝不重复挂载；`Malformed` 仍跳过。
- [x] 2.4 降级：`any` 跳过；`unknown`/`never`、声明/脚本事实不完整、期望类型不可解析、JS `checkJs` 未启用时标 `Partial` 并给出原因，不生成探针。
- [x] 2.5 确保声明文件进入 shadow `AnalysisInput`（模块顶层映射用类型级引用），不扫描 `node_modules`。
- [x] `cargo check -p wxml-lsp`、`cargo test -p wxml-lsp backend_tsrs::input` 通过。

## 3. 属性检查收敛（Wave 2，单个子代理；修改 `component_bindings.rs`）

- [x] 3.1 删除 mustache 的 `type_query` 路径：`Target`、`Actual`、`Verdict`、`chain_target`、`type_flags`、`check_declared_mustache`。
- [x] 3.2 保留未声明属性、保留属性、kebab/camel、组件解析、`Partial` 原因与每 tag 上限；复用 1.3 的共享事实。
- [x] 3.3 字面量/空属性只保留小判定，混合文本继续 `Partial`；不生成引擎探针。
- [x] 3.4 删除只服务旧 mustache 比较的测试，保留字面量/未声明属性/behavior 降级断言；新增“mustache 不再走独立查询”的断言。
- [x] `cargo test -p wxml-lsp component_bindings` 通过（事件/探针相关失败可留待 Wave 3 集成）。

## 4. 事件检查收敛（Wave 3，单个子代理；修改 `event_diagnostics.rs`、`typing_contract.rs`）

- [x] 4.1 `event_diagnostics` 不再调用 `typing_contract::check_handler`；事件参数兼容性由 2.2/5.1 的引擎探针发布。
- [x] 4.2 删除 `compatible`、`union_eq`、`object_eq`、`is_permissive`；保留 `expected_signature` 等 hover/展示用途；`normalize_annotation`、`classify` 等已无引用并删除。
- [x] 4.3 保留未知事件 `Warning`、缺失处理函数 `Error`、无参数/未标注的既有策略；无声明项目结果不变。
- [x] `cargo test -p wxml-lsp semantics::event_diagnostics` 通过（引擎矩阵在 Wave 4）。

## 5. 诊断归属（Wave 3，单个子代理；修改 `backend_tsrs/diagnostics.rs`）

- [x] 5.1 引擎诊断命中 `ContractAnchor.probe` 且 code ∈ `{2322, 2345, 2739, 2741}` 时发布到 `anchor`；同一绑定只保留一条。`2739`/`2741` 是缺少必需属性的赋值错误。
- [x] 5.2 契约探针的 `2353`（多余属性）与 `6133`/`6196` 等合成噪音丢弃；源表达式内部的诊断仍走 `map_native_range`。
- [x] 5.3 纯生成文本诊断或期望类型解析失败（`2304`/`2503`）不发布模板诊断，标 `Partial` 并说明原因；不向最近 token 猜测。
- [x] 5.4 与现有组件诊断合并时去重；旧路径删除后同一绑定不得出现两条消息。
- [x] `cargo test -p wxml-lsp backend_tsrs::diagnostics` 通过。

## 6. 真实引擎与回归测试（Wave 4，两个子代理按文件拆分）

- [x] 6.1 **属性矩阵**（新测试文件）：覆盖 `string`、`string[]`、对象、联合、条件、索引、算术、泛型、映射类型、字面量联合、`any`/`unknown`，以及额外字段通过、`2353` 不发布、诊断落在表达式。
- [x] 6.2 **事件矩阵**（新测试文件）：参数宽化、对象额外字段、必填→可选、字面量→基础类型、`unknown` 严格检查、额外必填参数、无参数处理函数、未标注 `Partial`、缺失处理函数优先、`strictFunctionTypes` 开关。
- [x] 6.3 **JS**：`checkJs: false` 无探针/`Partial`；`checkJs: true` 与 `@ts-check` 产生正确诊断。
- [x] 6.4 **范围**：CJK、CRLF、多行 mustache 的诊断锚点与现有映射一致；生成变量名上的错误映射到表达式/处理函数 token。
- [x] 6.5 **无声明/HostOnly**：无组件声明项目诊断与改动前一致；引擎不可用时不发布契约诊断。
- [x] 6.6 真实引擎命令：`TSRS_TYPINGS_ROOT=/home/zxg/.local/share/wxml-lsp/typings WXML_REQUIRE_REAL_ENGINE=1 cargo test -p wxml-lsp --test <name>`。

## 7. 文档收尾（Wave 4，可与测试并行）

- [x] 7.1 更新 `docs/reference/type-diagnostics.md`：属性/事件改由引擎可赋值性判定，字面量/混合文本规则、`any`/`unknown`、`2353` 取舍、诊断范围与 `Partial` 条件。
- [x] 7.2 如 README 的能力说明受影响，做最小更新；不写未验证的收益数字。
- [x] 7.3 `openspec validate engine-checked-component-bindings --strict` 通过。

## 8. 全量验证（主代理）

- [x] 8.1 `cargo fmt --all -- --check`
- [x] 8.2 `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [x] 8.3 `cargo test --workspace --all-features`
- [x] 8.4 真实引擎矩阵（6.6）串行运行
- [x] 8.5 `cargo doc --workspace --no-deps`
- [x] 8.6 `git status --short` 确认无临时文件；清理未使用的旧比较代码与测试
