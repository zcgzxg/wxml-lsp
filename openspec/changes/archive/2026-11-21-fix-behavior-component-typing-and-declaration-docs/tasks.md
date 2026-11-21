# Tasks

并行边界（供执行时拆分，避免同文件冲突）：

- 组 1 独占 `wxml-lsp/src/semantics/tsconfig_paths.rs`（新）与 `semantics/event_definition.rs`（改为 `use`）。
- 组 2 独占 `wxml-backend-tsgo/src/script/properties.rs`。
- 组 4 独占 `wxml-project/src/typing.rs`。
- 组 1、2、4 互不重叠，可同时开跑。
- 组 3 触达 `wxml-lsp/src/backend_tsgo/component_bindings.rs`，依赖组 1、2。
- 组 5 触达 `wxml-project/src/script.rs` 与 `wxml-lsp/src/metadata/docs.rs`，依赖组 4。
- 组 6（文档 + fixture）可在组 1 之后并行，最终措辞在组 3/5 定稿后复核。

## 1. 共享 tsconfig paths 解析器

- [x] 1.1 新建 `crates/wxml-lsp/src/semantics/tsconfig_paths.rs`，迁入 `TsconfigPaths`、`PathMapping`、`split_pattern`、`project_relative`、`join_project_path`、`candidate_paths`，`TsconfigPaths` 与 `from_view` 提升为 `pub(crate)`。验证：`cargo test -p wxml-lsp` 中既有 event definition 用例全部通过，别名 / 相对路径 / 越界 `paths` 行为不变。
- [x] 1.2 `event_definition.rs` 改为引用迁移后的类型，删除本地定义；`cargo clippy` 无未使用项告警。

## 2. 属性 overlay 的自身完整性

- [x] 2.1 `PropertyOverlay` 增加 `own_complete`：只反映组件自身 `properties` 成员是否完整枚举，不包含 behavior 的 `properties_incomplete` 与 `options` spread；`complete` 保持原语义。验证：单测覆盖“无 `properties` 成员”“对象字面量完整”“spread/非字面量/超预算”四种取值。
- [x] 2.2 `build_properties` 的所有早退分支都正确填充 `own_complete`（尤其 behavior 不完整但自身完整的路径）。验证：单测断言 behavior 不完整 + 自身完整时 `own_complete == true && complete == false`。

## 3. 绑定分析接入 resolver 与回退

- [x] 3.1 `analyze_component` 改为接收 `&PreparedView`，用 `TsconfigPaths::from_view` 装饰 `ScriptRequest`。验证：新增 lsp 单测——behavior 通过 `paths` 别名引入时属性参与校验（构造 `PreparedView` 的 `compiler_options.paths`）。
- [x] 3.2 `ComponentDeclarations` 增加 `complete`；判定改为 `!registration.complete || (!properties.complete && !properties.own_complete)` 才返回 `None`。验证：单测覆盖三种分支。
- [x] 3.3 `Pass::run` 在 `declarations.complete == false` 时置 `partial`，仍按已知属性校验且不把未声明属性当错误。验证：单测断言 behavior 不可读时 `title="x"` 仍报类型不匹配、未知属性不报、状态为 `Partial` 且原因来自 `problems`。
- [x] 3.4 回归：完整 behavior 场景的诊断与 before 一致，声明优先路径仍跳过 `analyze_component`。验证：既有 `component_bindings` 测试全绿。

## 4. 声明成员说明解析

- [x] 4.1 `TypingProperty`/`TypingEvent` 增加 `documentation: Option<String>`；解析成员键前回看紧邻 `/** ... */`，去分隔符与行首 `*`、折叠空白、空注释为 `None`、512 字符截断。验证：单测覆盖单行、多行、空注释、`//` 注释不识别、超长截断。
- [x] 4.2 说明不影响 `proven`/`complete`/`type_text`。验证：单测断言带注释与不带注释的组件事实除 `documentation` 外完全相同。

## 5. 合并与渲染

- [x] 5.1 `script::merge_declared_properties` 透传 `documentation`（`default_text` 保持 `None`）。验证：单测断言合并后 `ComponentPropertyFacts` 携带说明。
- [x] 5.2 `metadata::docs::declared_event_document` 追加事件说明（在未证明标注之后），复用 `escape`。验证：`docs.rs` 单测断言带述与不带述两种输出。
- [x] 5.3 属性 hover 端到端：声明属性带注释时 hover 含说明。验证：`docs.rs` 单测 + lsp 集成测试。

## 6. 文档与 fixture

- [x] 6.1 `docs/reference/type-diagnostics.md` §7.1 增加 behavior 经 `tsconfig` paths 解析、behavior 不可读时回退自身属性并记 `Partial`；§7.4 增加声明成员 `JSDoc` 进入 hover、事件签名后展示说明。
- [x] 6.2 `tests/fixtures/` 样例：复用既有 `components-typings` 样本的 `/** */` 成员，在 `project_host` 测试中断言说明进入属性/事件事实；带 `paths` 别名的 behavior 由 `component_bindings` 内存内 `PreparedView` 单测覆盖（不新增 fixture 目录）。
- [x] 6.3 `cargo fmt` / `clippy -D warnings` / `test --workspace --all-features` / `doc` 全绿；真实引擎可选验证记录在 PR 描述，不落仓库。

## 7. 仓库外跟随工作（应用侧）

- [x] 7.1 `jielong4/apps/miniprogram/components.d.ts` 为 props/events 补 `/** 说明 */`（来源 `vant-jl` 各包 README 的 Props/Events 表格），并用 `typing-check` 与 LSP hover 实测确认说明出现。
