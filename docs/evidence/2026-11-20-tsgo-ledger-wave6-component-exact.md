# tsgo 测试台账 wave-6 组件绑定跟进（2026-11-20）

## 0. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务基线 HEAD：`0bf666b994163d2a44a4b87b0b8bb5d4e9101bf1`
- 记录日期：2026-11-20
- 记录期间并行会话把工作树 HEAD 推进到 `7ca92a3`；本跟进没有修改它们的
  native edit、delivery、gate 或 Cargo 改动，也没有把那些测试当作本 case 证据。
- 范围：`crates/wxml-lsp/src/backend_tsgo/component_bindings.rs`（测试与测试夹具）、
  测试台账 `docs/plans/tsgo-test-ledger.json`、Markdown 台账
  `docs/plans/2026-11-17-tsgo-test-ledger.md`、contract
  `docs/reference/tsgo-test-contract.md`、ledger validator
  `crates/wxml-lsp/tests/native_parity/ledger.rs`、证据索引和本文件。
  未修改默认 backend、Node、delivery、Cargo 或 `backend_node`；未提交。

**结论：4 个 case 达到 exact 并提升为 `green`，2 个 case 明确保持 blocked。**
`green` 44 → 48，`planned` 259 → 255，`exact` 44 → 48，
`weaker` 80 → 74，`blocked` 20 → 22，`assessed_cases` 154 与
无证据 planned 149 不变。

---

## 1. 逐 case 结论

### 1.1 `TS-COMPONENT-0147` → exact / `green`

TS oracle：`component bindings produce per-tag diagnostics and count skipped checks`
（`backends/typescript/test/componentBindings.test.ts:147`）。

| TS 断言 | native 断言 |
| --- | --- |
| `valid`/`valid-literal`/`valid-boolean`/`undeclared` 的 diagnostics 为 `[]` | `component_binding_matrix_matches_the_typescript_oracle` 逐个 usage 运行；status `None` 证明每个 usage 至少检查了一个属性且没有 skip |
| `bad-mustache` 单条诊断，range 为 `total` 表达式范围，消息含 `expects string, got number` 与 `bound expression "total"` | 同 range、`Error` 严重度、逐字消息 `component property "title" expects string, got number (bound expression "total")` |
| `bad-union` 消息含 `expects string \| number, got boolean` | `string \| number` 声明文本与 boolean 实际类型的逐字消息 |
| `bad-literal`/`bad-boolean` 使用 attribute range 和 `string literal`/`boolean literal` 消息 | 同 attribute range、同消息 |
| `array-ok`/`object-ok` 清洁；`array-bad` 为 `expects any[], got string`；`object-bad` 为 `expects Record<string, any>, got number` | 同四类结果与逐字消息 |
| 8 个 skipped usage 不出现在结果里 | 每个 usage 单独运行：无诊断、无 query，Partial reason 精确到 `tag.attribute` 或裸 `tag` |
| `states.diagnostics.status = partial`，reason 引用前三对 `tag.attribute` | 组合矩阵的 status Partial，reason 逐字等于 `... (any-prop.loose, function-prop.handler, mixed.title)` |

native 没有 per-tag 响应数组，因此 clean/skip 分类按 usage 逐个断言；在矩阵规模下
没有触发 `MAX_TAGS`/`MAX_QUERIES`，任一跳过都会把 status 变成 Partial，所以
“结果存在/不存在”的语义与 TS 的 map 查询等价。没有硬编码期望：11 次 query 全部
经过真实 `diagnostics_with` 管线，消息、range 和 skip 详情都由夹具字节与声明事实
推导。

### 1.2 `TS-COMPONENT-0193` → blocked

TS oracle：`a fully checkable component request stays Supported`
（`componentBindings.test.ts:193`）。

已覆盖：

- 三个属性 `title`/`count`/`items` 全部检查，零诊断；
- status `None`（adapter 投影为 Supported），即 `skipped=0`；
- 三个真实 mount、三次 type query；
- 单一 clean tag 的 `components=[{tag, diagnostics: []}]` 语义。

未覆盖：`probeProblems=[]`。TS 的 `probeProblems` 是 Node probe root/helper 的
健康检查；native shadow 没有 probe anchor，也没有
`probeProblems` 可观察面。用 `fragment_count` 或 status 代替会把“没有 probe
机制”伪装成“probe 健康”，因此保持 `parity=blocked`，不提升。

### 1.3 `TS-COMPONENT-0206` → exact / `green`

TS oracle：`literal bindings are checked even when the template has no mustache`
（`componentBindings.test.ts:206`）。

| TS 断言 | native 断言 |
| --- | --- |
| `result.mounts = []` | 夹具不含 `{{`，`layout.fragment_count() == 0` |
| 一条诊断，range `{40,52}` | 诊断 range 等于真实 attribute range，且该范围切片就是 `count="oops"` |
| 消息 `expects number, got string literal` | 逐字相同 |
| `states.diagnostics.status = supported` | pass status `None`（adapter 投影 Supported） |
| `componentOutcome(request).skipped = 0` | status `None` 同时证明没有 skip：`finish` 在 `skipped > 0` 时必返回 Partial |

### 1.4 `TS-COMPONENT-0229` → blocked

TS oracle：`optional data members may be absent for Array/Object bindings`
（`componentBindings.test.ts:229`）。

已覆盖：

- `Item[] | undefined` 对 `Array` 清洁；
- 纯 `undefined` 对 `Number` 视为 absent，清洁；
- 上述两个 usage 没有 skip（reason 只列另外两个 usage）。

无法覆盖：

- `User | undefined` 对 `Object` 应清洁；
- `User | undefined` 对 `String` 应报
  `expects string, got User | undefined`。

固定 wire 只提供整型 `flags` 和 `typeToString` 文本，不提供 union 每个成员的
结构 flag，也没有 `isTypeAssignableTo`/`isArrayLikeType`。名为 `User` 的成员
既不能证明是 Object，也不能证明不是 String；按名字猜 object-ness 属于伪造。
因此保持 blocked，并将现状固定在
`optional_named_union_members_remain_partial_not_guessed`。

### 1.5 `TS-COMPONENT-0251` → exact / `green`

TS oracle：`a component with no properties member is skipped, not guessed`
（`componentBindings.test.ts:251`）。

- `Component({ data: { state: 'idle' } })`：`analyze_script` 得到空 property 集，
  pass 将整个 usage 跳过，不做任何 type query；
- 无诊断，`status` 为 Partial，reason 精确为
  `... (bare)`，即 native 投影的“没有 component 结果”，等价于
  `response.components === undefined`；
- status Partial 与 TS 的 `states.diagnostics.status = partial` 一致。

### 1.6 `TS-COMPONENT-0268` → exact / `green`

TS oracle：`component diagnostics never leak a file path`
（`componentBindings.test.ts:268`）。

`component_diagnostics_never_leak_a_path` 运行完整矩阵，遍历全部 6 条诊断
（mustache、literal、boolean 三类消息），逐条断言不含 `/`；同时断言 Partial
reason 也不含 `/`。这与 TS 遍历 `response.components[].diagnostics` 的
per-diagnostic 不变量相同，不是只检查单条样例。

---

## 2. 计数变化

| 项 | 跟进前 | 跟进后 |
| --- | ---: | ---: |
| `green` | 44 | 48 |
| `planned` | 259 | 255 |
| `retired-with-decision` | 36 | 36 |
| `exact` | 44 | 48 |
| `near_exact` | 3 | 3 |
| `weaker` | 80 | 74 |
| `structural` | 7 | 7 |
| `blocked` | 20 | 22 |
| `assessed_cases` | 154 | 154 |
| 无 Rust 证据的 planned | 149 | 149 |

`components` 分区：routed 19、有证据 16、green 4 → 8、planned 有证据 12 → 8、
exact 4 → 8、weaker 7 → 1、blocked 5 → 7、无证据 3 不变。

---

## 3. 验证

| 命令 | 结果 |
| --- | --- |
| `cargo fmt -p wxml-lsp -- --check` | exit 0 |
| `cargo check -p wxml-lsp --all-targets` | exit 0 |
| `cargo clippy -p wxml-lsp --all-targets -- -D warnings` | exit 0 |
| `cargo test -p wxml-lsp --lib backend_tsgo::component_bindings` | 22 passed / 0 failed |
| `cargo test -p wxml-lsp --lib` | 940 passed / 0 failed |
| `cargo test -p wxml-lsp --test native_parity` | 37 passed / 0 failed / 26 ignored |
| `jq empty docs/plans/tsgo-test-ledger.json` | exit 0 |
| `git diff --check` | exit 0 |

---

## 4. 限制

1. `TS-COMPONENT-0193` 的唯一缺口是 Node-specific `probeProblems`；其余四条
   断言已覆盖，但没有把“无 probe 机制”当作“probe 健康”。
2. `TS-COMPONENT-0229` 的唯一缺口是 pinned wire 没有 union 成员结构 flag；
   `Item[] | undefined` 与 absent 已覆盖，`User | undefined` 保持保守 Partial。
3. 矩阵测试使用 `type_query` 的既有测试 seam 提供固定类型答案；真实
   `ProjectState` + 真实 fixture 路径仍由
   `fixture_component_usages_publish_one_property_error` 覆盖，CJK/emoji/CRLF
   仍由 `cjk_crlf_expression_range_is_exact` 覆盖且未改动。
4. 本轮未做独立 review；所有 `green` 仍处于 `reviewed` 之前的状态。
