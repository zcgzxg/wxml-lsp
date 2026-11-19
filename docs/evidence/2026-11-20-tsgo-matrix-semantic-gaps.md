# tsgo matrix 语义差异：`page-closure` 与 `methods-nonliteral`（2026-11-20）

## 0. 身份、边界与结论先行

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD：`befba03`（`fix(tsgo): accept behavior trailing commas and backfill parity`）
- 本切片只处理 matrix 新发现的两个语义差异；未改 ledger、README、矩阵投影
  ` crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/projection.json`、
  freeze、`native_gates`、`native_parity.rs` 或 `scenario_matrix.rs`；未 commit，
  未 review。
- 任务书写的是 `backends/typescript/test/matrix.json`；仓库中不存在该文件。
  实际 oracle 是 ` docs/evidence/data/type-bridge-c1/matrix.json`
  （SHA-256 `69bc6c...`），matrix runner 的冻结投影是
  ` crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/projection.json`。

| case | 旧 TS oracle | native 修复前 | 决定 | native 修复后 |
| --- | --- | --- | --- | --- |
| `page-closure` | build `Supported`，三能力 `supported` | build `Partial`，`DataRootKind::Expression` | **contract/oracle drift**，保持保守 `Partial` | 不变；新增回归测试钉住 `Partial + Expression + mount` |
| `methods-nonliteral` | build `Partial`，三能力 `partial`，reason 为 ``component `methods` is not an object literal; the in-place probe cannot be mounted`` | `Err(ShadowError::Unsupported { feature: "component.methods" })` | **fix** | build `Partial`，同一 reason，原文/无 probe/无 mount，matrix 观察到 `build=partial` |

结论：case 2 是 native 把“覆盖缺口”错误升级成“脚本不支持”，已按 TS 语义修复；
case 1 不是可安全恢复的 bug，而是 native 缺少 checker 级 data-root 分类时更诚实
的能力边界，记录为 contract/oracle drift，不强行 exact。

> 旧的 ` docs/evidence/2026-11-20-tsgo-native-matrix-implementation.md` §0/§6
> 记录的 `methods-nonliteral = Unsupported` 是本次修复前的观察；该 case 的当前
> native 行为以本文件为准。`page-closure` 的观察未变。

## 1. 复现材料

- 冻结场景：` crates/wxml-lsp/tests/native_parity/real_matrix.rs`
  - `assert_build_facts` / `assert_gap_expectation` 中的 `blockedBy=checker` 分支。
- 样本：
  - ` backends/typescript/samples/page-closure/index.ts`
  - ` backends/typescript/samples/methods-nonliteral/index.ts`
- 旧 Node/TS 场景定义：` backends/typescript/src/scenarios.ts:109` 与
  ` backends/typescript/src/scenarios.ts:509`。
- 旧 Node/TS 期望：` backends/typescript/test/matrix.test.ts:17-47`、
  ` backends/typescript/test/matrix.test.ts:71-82`。
- oracle 状态：
  - `page-closure`：`hover/diagnostics/definition = supported`，`mounts` 为
    `config.title`、`user.name`，0 diagnostics。
  - `methods-nonliteral`：三能力 `partial`，reason 为
    ``component `methods` is not an object literal; the in-place probe cannot be mounted``，
    0 diagnostics、0 hover、0 mount。

## 2. `page-closure`：`data: identity({...})`

### 2.1 旧 Node/TS 行为

1. ` classifyDataRoot`（` backends/typescript/src/dataroot.ts:15`）只看
   `data` 初始化节点本身：
   - `ObjectLiteralExpression` -> `object`；
   - `Identifier` -> `expression`，再由 checker 诊断
     （2693/2749）区分为 `type-reference`；
   - 其他节点（包括 `CallExpression`）-> `expression`。
2. ` buildInPlaceShadow`（` backends/typescript/src/layouts/inPlaceShadow.ts:56`）
   只对 `type-reference`、`absent`、`unsupported` 降级；`expression` 没有降级。
3. 探针仍插入 `Page` options；`this.data` 由 TypeScript 从
   `Page({ data: identity({...}) })` 上下文推断，泛型 `identity<T>(value: T): T`
   返回 `T`，所以 `config.title`/`user.name` 都能得到正确类型和 JSDoc。
   因此旧 oracle 是 `Supported`。

### 2.2 native 行为

- ` build_data`（` crates/wxml-backend-tsgo/src/script/properties.rs:133`）只在
  `data` 初始化是对象字面量时枚举字段；其他形状返回
  `DataRootKind::Expression` + `CapabilityStatus::Partial`，reason 含
  `data member is not an inline object literal; field-level roots are not enumerated`。
- shadow 构建复用同一分类（` crates/wxml-backend-tsgo/src/shadow/layout.rs:548`），
  把 `DataRootKind::Expression` 映射为 `BuildStatus::Partial`。
- 探针仍然挂载，fragment 仍然通过 `this.data` 检查；matrix 观察到的
  hover type/doc 和 0 diagnostics 与 oracle 一致，唯一差异是 build 状态。

### 2.3 决定：contract/oracle drift，不强行 exact

现有 `ScriptAnalysis`/表达式分析无法安全恢复 `Supported`：

- ` resolve_value_context`（` crates/wxml-backend-tsgo/src/script/data_path.rs:564`）
  只接受“完整对象字面量”或“可解析到对象字面量的单个标识符”；
  `identity({...})` 是调用表达式，直接返回 `None`。
- ` infer_initializer`（` crates/wxml-backend-tsgo/src/script/values.rs:50`）对调用
  结果没有类型推断；文档明确“Identifier or call results ... become `any` with
  `exact = false`”。因此 `identity` 的返回类型在这里不可知。

只对“调用表达式”做语法特判也不安全：调用可能返回与参数不同的形状，native
无法证明 `identity` 返回的就是传入对象；而且 native 的 `BuildStatus` 还承载
data-path/补全所需的字段枚举契约，把没有枚举字段的 `Expression` 标成
`Supported` 会过度声明覆盖范围。

如果为了让 `page-closure` 变 `Supported` 而删除所有 `Expression` 的降级，会
同时把 `interface-root-import`（`import type { User } ...; Page({ data: User })`）
从应有的 `Unsupported` 变成 `Supported`：native 没有 checker 诊断，无法区分
“导入的类型别名”和普通值标识符。旧 Node 正是用
` languageService.getSemanticDiagnostics` 的 2693/2749 才区分出
`type-reference`（` backends/typescript/src/dataroot.ts:31-44`）。

因此本切片选择：

- 保持 `page-closure` 的 `Partial` + `DataRootKind::Expression`；
- 新增回归测试
  ` crates/wxml-backend-tsgo/tests/shadow_layout.rs:505`
  `case_matrix_0102_page_closure_expression_data_stays_partial_with_mounts`，
  钉住“保守 Partial、但探针仍然挂载 fragment”；
- 不声称 `Supported`，不写 checker 级 data-root 分类的伪实现。

后续若要 exact，需要生产切片把 checker 的 data-root 分类（或等价的类型诊断）
接回 native shadow 构建，而不是在纯文本层特判 `identity`。

## 3. `methods-nonliteral`：`Component({ methods: sharedMethods })`

### 3.1 旧 Node/TS 行为

1. ` probePlacement`（` backends/typescript/src/layouts/inPlaceShadow.ts:448`）
   在组件 `methods` 不是 property assignment，或其初始化不是对象字面量时返回
   ``{ problem: "component `methods` is not an object literal; the in-place probe cannot be mounted" }``。
2. ` buildInPlaceShadow` 随后调用
   ` emptyBuild(context, "partial", placement.problem, notes)`
   （` backends/typescript/src/layouts/inPlaceShadow.ts:202-204`）：
   - build 状态 `partial`；
   - 脚本保持原文，没有 probe insertion；
   - `fragments`、`mounts`、`generatedDeclarations` 为空；
   - 三能力由 build 状态派生为 `partial`，reason 就是上面的 problem。
3. 这是“覆盖缺口”，不是“脚本不支持”：用户脚本本身仍然可以分析，只是模板
   fragment 无法挂载。

### 3.2 native 修复前

- ` probe_placement`（` crates/wxml-backend-tsgo/src/shadow/layout.rs:1524`）对同一
  形状返回
  `Err(ShadowError::Unsupported { feature: "component.methods", ... })`。
- ` build_in_place_shadow` 把错误向上抛；matrix 的 `BuiltFacts::Unsupported`
  把三能力都映射成 `unsupported`，与 oracle 的 `partial` 不符。
- ` script/events.rs` 的 `MethodTable` 其实已经是 `Partial`
  （` crates/wxml-backend-tsgo/tests/script_events.rs:334`），差异只在 shadow
  build 层。

### 3.3 生产修复

> 路径说明：任务书的范围提示是 `script/**`，但 matrix 的 `shadow_build` 直接调用
> `build_in_place_shadow`，错误源在 ` crates/wxml-backend-tsgo/src/shadow/layout.rs`。
> `script/events.rs` 的 `MethodTable` 本来已是 `Partial`，所以最小修复只能落在
> shadow 构建层；没有为了路径形式把逻辑搬到 `script/**`。

` crates/wxml-backend-tsgo/src/shadow/layout.rs:894`：

- 只拦截 `component.methods` 的 `Unsupported`，其他错误继续抛；
- 返回 `BuiltShadow`：
  - `status = BuildStatus::Partial`；
  - public `reason` 使用旧 oracle 的同一句 problem（其他 coverage gap 放进
    `notes`，不改变 public reason）；
  - `view.text` 是原文，`edits`/`insertions`/`mounts`/`generated_declarations`
    全空，`probe_anchors` 为默认值，`ShadowMap` 是恒等映射；
  - 保留 `registration.kind/count`、`data_root`、`properties`；
  - 未注入 `@ts-check` 时不声称 `Pragma`：`Pragma` 收敛为 `Inherit`，
    仅 `Off` 保留 `Off`。
- `probe_placement` 的 shorthand/非 property 分支也统一使用同一 reason
  （` crates/wxml-backend-tsgo/src/shadow/layout.rs:71`），与旧 oracle 文本一致。

修复后的行为：

- 不再把 `methods` 非字面量当成 hard error；
- 不伪造字段、不挂载 fragment、不发布模板 diagnostics；
- 产品层 `input.rs` 会因为没有 fragment map 而合并一条 degradation reason，
  但脚本仍可打开，capability 状态是 `Partial`，符合旧 oracle。

### 3.4 回归测试与 matrix 断言

- ` crates/wxml-backend-tsgo/tests/shadow_layout.rs:470`
  `case_matrix_0102_non_literal_component_methods_are_partial_without_a_probe`：
  覆盖 `methods: shared` 与 shorthand `methods`，断言 `Partial`、精确 reason、
  原文不变、0 mount、0 insertion、registration 事实保留、`data_root=Absent`。
- ` crates/wxml-backend-tsgo/src/shadow/layout.rs:2415`
  `non_literal_methods_are_partial_and_expression_data_is_partial`：单元层同样
  钉住新语义。
- ` crates/wxml-lsp/tests/native_parity/real_matrix.rs:976`：`blockedBy=checker`
  的 `methods-nonliteral` 断言从 `Unsupported` 改为 `Built { Partial }` 且
  `mounts` 为空；`assert_gap_expectation`（` real_matrix.rs:1710`）同步改为
  `Partial`。
- exact 分支的 `assert_partial_reason` 分组也加入 `methods-nonliteral`
  （` real_matrix.rs:944`），这样 owner 将来只改投影分类时 runner 不会把它误判
  成 `Supported`。
- 冻结投影仍是 `blockedBy=checker`，所以 matrix 汇总仍是
  `exact=21 gaps=12`；但真实 native 观察已从 `unsupported` 变为
  `blocked(checker): methods-nonliteral build=partial diagnostics=0`。
  owner 若接受本证据，可把该 case 重分类为 exact（三能力 `partial` 与 oracle
  一致）；本次不改投影。

## 4. 改动文件

| 路径 | 改动 |
| --- | --- |
| ` crates/wxml-backend-tsgo/src/shadow/layout.rs` | `component.methods` 的非字面量 `methods` 改为 `Partial` 无 probe 构建；统一 reason；单元测试 |
| ` crates/wxml-backend-tsgo/tests/shadow_layout.rs` | 移除旧的 `Unsupported` 期望；新增两个 matrix 语义回归测试 |
| ` crates/wxml-lsp/tests/native_parity/real_matrix.rs` | `methods-nonliteral` 的 checker gap 断言改为 `Partial` |
| ` docs/evidence/2026-11-20-tsgo-matrix-semantic-gaps.md` | 本证据 |

未改：ledger JSON/Markdown、README、矩阵投影、freeze、`native_gates`、
`native_parity.rs`、`scenario_matrix.rs`。

## 5. 验证命令与结果

```bash
cargo fmt --all
# 通过

cargo check --workspace --all-targets
# Finished dev profile

cargo clippy --workspace --all-targets -- -D warnings
# No issues found

cargo test -p wxml-backend-tsgo
# 667 passed; 0 failed; 4 ignored (19 suites)

cargo test -p wxml-backend-tsgo \
  --test script_registration --test script_events --test script_properties \
  --test script_optional_types --test script_behavior --test script_symbols \
  --test script_symbol_facts
# 136 passed; 0 failed (7 suites)

cargo test -p wxml-lsp --test native_parity
# 78 passed; 0 failed; 29 ignored

cargo test -p wxml-lsp
# 1264 passed; 0 failed; 52 ignored (26 suites)
```

真实 pinned 引擎 matrix：

```bash
WXML_TEST_NATIVE=1 \
WXML_REQUIRE_REAL_ENGINE=1 \
TSGO_BIN=/tmp/opencode/tsgo-engine-7.1.0-beta/package/lib/tsc \
TSGO_ASSET_ROOT=/tmp/opencode/tsgo-engine-7.1.0-beta/package/lib \
TSGO_TYPINGS_ROOT=/tmp/opencode/cli-typings-root \
cargo test -p wxml-lsp --test native_parity real_matrix -- --ignored --nocapture
# 3 passed; 0 failed

# 关键观察：
# blocked(checker): page-closure build=partial diagnostics=0
# blocked(checker): methods-nonliteral build=partial diagnostics=0
# real_matrix: exact=21 gaps=12 scenarios=33 engine=7.1.0-beta
#   oracle=69bc6c... projection=ae1f...
```

## 6. 剩余风险与后续

1. `methods-nonliteral` 的 exact 化需要 owner 更新冻结投影/ledger；本切片只把
   native 行为修正到与 oracle 的 `Partial` 语义一致，不修改分类。
2. `page-closure` 仍是保守 `Partial`；要 exact 需要 checker 级 data-root 分类，
   否则会把 `interface-root-import` 的 `Unsupported` 一起放宽，属于 contract
   回退。
3. 无 commit、无 review；未把 optional skip、默认 ignored 或空诊断当作证据。
