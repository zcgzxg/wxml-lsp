# Tasks

并行边界（供执行时拆分，避免同文件冲突）：

- 组 1 独占 `crates/wxml-project/src/typings.rs`（新），不改 `config.rs`。
- 组 2 独占 `crates/wxml-project/src/typing.rs`（新）；不碰 `lib.rs`。
- 组 3 触达 `wxml-project/src/{lib,discovery,script}.rs` 与 `wxml-lsp/src/project_host.rs`，须在组 1/2 之后串行。
- 组 4 触达 `wxml-lsp/src/backend_tsgo/component_bindings.rs`；组 5 触达 `event_completion.rs`、`wxml-backend-tsgo/src/{script/events.rs,script.rs}`。二者可在组 3 之后并行。
- 组 6（文档 + fixture）可在组 1 之后与代码并行，但组 6.2 需在组 4/5 行为定稿后复核。

## 1. 声明文件发现

- [x] 1.1 `typings.rs` 实现固定路径：`tsconfig.path.parent()/components.d.ts`；无 tsconfig 时返回 `None`。验证：单测覆盖同级命中、无 tsconfig、更深一层同名文件不生效。
- [x] 1.2 安全读取：走 overlay 优先路径，只接受项目根内普通 `.d.ts`，大小复用 `sources.rs` 单文件预算；缺失返回 `None` 且无 issue，目录/路径逃逸/不可读/超限记 issue 并标 `Partial`。验证：单测覆盖 overlay 优先、缺失无副作用、异常路径降级且不 panic。

## 2. 声明解析

- [x] 2.1 实现 `TypeText` 解析子集（基础类型、字面量、联合、数组、`Record<string,T>`、嵌套对象），未解析成分标记为未证明。验证：单测覆盖每个语法分支与"外部标识符 → 未证明"。
- [x] 2.2 实现 `ComponentTypingFacts` 解析：`WxmlComponentMap`（`declare global` 内或顶层）+ 同文件 `interface`/`type` 别名，产出 props/events 与组件级 `complete`。验证：单测覆盖合法声明、未被引用接口不生效、无关声明被忽略、events 带 detail。
- [x] 2.3 加入 `MAX_TYPING_COMPONENTS`/`MAX_TYPING_MEMBERS` 截断与映射表重复键处理（取首个 + issue 文案）。验证：单测断言截断标记与冲突说明。
- [x] 2.4 `Project::typing_facts()` 暴露事实并在 `rediscover` 重算。验证：`cargo test -p wxml-project` 新增用例，断言重算后事实随 tsconfig/文件变化。

## 3. 事实合并与优先级

- [x] 3.1 `ComponentPropertyFacts` 增加 `PropertySource`，实现声明覆盖脚本的合并 helper，并让 `ViewComponent::apply_live_facts` 在 `Typing` 来源时早退。验证：单测覆盖冲突取声明、overlay 改动不影响、无声明回落脚本。
- [x] 3.2 `ViewComponent` 携带 `typing`，`components_from_config` 接入声明事实；按 `(path, revision)` + tsconfig 版本做 digest 缓存失效。验证：lsp 单测断言修改 `.d.ts` overlay 后事实更新、无关 overlay 不触发重算。
- [x] 3.3 降级原因透传：声明未命中、解析不完整、类型未证明、handler 参数未标注、同名冲突都映射到 `Partial` + 可读原因。验证：lsp 单测断言 `status_hint`/原因字符串。

## 4. 属性检查接入

- [x] 4.1 新增 `PropertyContract`（`Unknown`/`Any`/`Kind`/字符串字面量集/数字字面量集）与可空性处理。验证：单测覆盖每种 `TypeText` → 契约映射，未知映射一律 `Unknown`。
- [x] 4.2 视图已带声明事实时跳过 `analyze_component` 的脚本推导；绑定诊断使用契约，范围落在绑定表达式。验证：假查询/HostOnly 单测覆盖未声明属性、字面量不匹配、可选缺省、未证明不报。
- [x] 4.3 补齐真实引擎路径（`TSGO_BIN` + typings）下的表达式类型不匹配用例；无引擎时按既有方式跳过。验证：`WXML_REQUIRE_REAL_ENGINE=1` 时该测试执行且通过。

## 5. 事件能力

- [x] 5.1 `script.rs` 在方法提取时记录形参类型标注原文（`HandlerFact`），未标注为 `None`。验证：单测覆盖方法简写、`function` 属性、箭头函数、无参数。
- [x] 5.2 实现 `EventContractTable` 与 `HandlerTypeCheck`：规范化结构比较，`Skipped(Reason)`/`Compatible`/`Incompatible` 三态。验证：表驱动单测，覆盖不兼容、兼容、未证明、未标注、handler 缺失优先。
- [x] 5.3 事件名补全改为优先读取声明事件（仅 `complete` 的组件），保持内置组件与无声明组件现有候选。验证：lsp 单测断言候选集合与替换范围。
- [x] 5.4 未声明事件名 `Warning`（事件名 token）+ handler 参数不兼容 `Error`（handler token），且与 handler 缺失诊断互斥。验证：诊断单测断言 severity、range、互斥。
- [x] 5.5 事件属性 hover 展示 `(e: WechatMiniprogram.CustomEvent<T>) => void`，未证明时明确标注。验证：hover 单测断言文本与 range。

## 6. 文档与 fixture

- [x] 6.1 新增 `tests/fixtures/components-typings/` 真实小程序工程（app.json、usingComponents、`tsconfig.json` 与**同级**的 `components.d.ts`），并更新 `tests/fixtures/README.md`。验证：fixture 被现有 crate 测试读取，`cargo test --workspace --all-features` 通过；文件保持 LF。
- [x] 6.2 更新 `docs/reference/type-diagnostics.md`：组件属性来源与优先级、事件诊断级别表、hover/补全契约、声明格式与发现规则、子集与非目标。验证：文档与实现/测试的级别、range、状态描述一致。
- [x] 6.3 新增 `docs/adr/0011-component-type-declarations.md` 并登记到 `docs/adr/README.md`。验证：ADR 只描述已实现决策，链接均为仓库内相对路径。

## 7. 端到端验收

- [x] 7.1 全量验收：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`cargo doc --workspace --no-deps`、`cargo deny --offline check` 全部通过。
- [x] 7.2 回归确认：无声明文件的项目诊断集合与改动前一致；新增诊断只出现在声明命中的 fixture 上。验证：对比 fixture 诊断快照与既有 lsp 集成测试结果。
