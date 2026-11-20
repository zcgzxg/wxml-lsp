# tsgo wave-11 W11-A/W11-B：消息清洗与 probe health（2026-11-20）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务给定基线：`51abc13`（`test(tsgo): close conditional parity and ledger
  backfill`）。实现与验证期间并行提交 `68f852c`（`test(tsgo): cover layout
  fragment reuse and request isolation`）；本文件记录验证时的实际工作树。
- 记录日期：2026-11-20
- 范围：原始切片只新增/修改生产 query 模块、`query/mod.rs` 导出和本证据文件；
  原始切片未改 `native/session.rs`、adapter、`native_parity`、ledger JSON、ledger
  Markdown、README、contract、ADR、fixture 冻结树、Cargo、脚本。后续 wave-11 exact
  台账回填只改 ledger/validator/契约/证据（见 §5）；未 commit，未 review。
- 台账回填：本切片 10 个 exact 与 `TS-ANALYSIS-0315/0332` 合并落账；当前
  **82 green / 219 planned / 38 retired**，`exact` 82、`assessed_cases` 209、
  无证据 planned 92（HEAD `68f852c`）。
- 工作树并行状态：`crates/wxml-lsp/tests/native_parity/*`、
  `docs/plans/tsgo-test-ledger.json`、`docs/reference/*` 等由其他 session
  修改，原始切片没有触碰；后续 wave-11 exact 回填只同步 ledger/validator/契约/证据，
  并以当时实际计数重新计算。
- 输入：`docs/evidence/2026-11-20-tsgo-wave11-native-capability-triage.md`
  §4.1/§4.2/§6.2/§6.3，C 类 59 个 case 中可无条件 exact 的 10 个。

**结论：10 个 case 全部 exact，0 个 blocked。**

| Case | 判定 | 落点 |
| --- | --- | --- |
| `TS-MESSAGES-0005/0016/0025/0032/0039` | **exact** | 新 `query/message.rs` + 5 个同名单测 |
| `TS-PROBES-0064/0080/0098/0135/0153` | **exact** | 新 `query/probe_health.rs` + 5 个同名单测 |

没有把 probe health 接入 hover/diagnostics 状态机；没有伪造 checker、C1
`analyze` 或 `probeProblems` 响应字段。`grep` 确认新 API 在 query 模块之外没有
调用者。

---

## 1. 生产改动

| 文件 | 内容 |
| --- | --- |
|  crates/wxml-backend-tsgo/src/query/message.rs:41 | `sanitize_message(&str) -> String`，C1 消息清洗纯函数 |
|  crates/wxml-backend-tsgo/src/query/probe_health.rs:180 | `check_probe_health(&ProbeHealthInput) -> Result<ProbeHealth, ProbeHealthError>` |
|  crates/wxml-backend-tsgo/src/query/mod.rs:17 | `pub mod message; pub mod probe_health;` |

### 1.1 消息清洗

按 `backends/typescript/src/messages.ts:28` 的步骤顺序移植：

1. 9 个 wrapper 的单层泛型展开（`FilterUnknownType`、`PropertyOptionToData`、
   `FullPropertyToData`、`MixinProperties`、`MixinMethods`、`MixinData`、
   `ExtractProperties`、`ExtractMethods`、`ExtractBehaviorType`）。
2. `__wxml_root_?[0-9]*\.` / `__wxml_props_[0-9]*\.` 前缀删除。
3. `__wxml_[A-Za-z0-9_]*` → `helper`。
4. `__WxmlProps_[0-9]*` → `declared properties`。
5. `WechatMiniprogram.[A-Za-z0-9_.$]*` → `miniprogram instance`。
6. 相同 intersection 成员去重、重复 `{ ... }` 字面量删除。
7. `& ...` / `& {}`、引号前/后的 `&`、`helper .` 的清理。
8. 空白折叠、`& &` 折叠。
9. 200 UTF-16 code unit 预算，超限时 `trimEnd` + `…`。

边界：`MAX_MESSAGE_UNITS = 200`；输入先按 `MAX_INPUT_CHARS = 8_192` 字符
截断，避免恶意长消息让中间 pass 无界；截断发生在字符边界，最终输出仍由 200
单位预算约束。函数无 `unwrap`/`expect`，无 panic 路径。

### 1.2 probe health

按 `backends/typescript/src/probes.ts:100` 的决策顺序移植，输入是已采集事实，
函数本身不调用引擎、不按名字搜索：

1. derived script 不在 program → 只返回 `derived script is not part of the
   program`（与 oracle 早返回一致）。
2. helper 精确 offset 的符号证据：`Symbol` / `NoSymbol` / `OutsideProgram`
   → `probe helper {name} has no symbol` 或
   `probe helper {name} lives in a file outside the program`。
3. root：`NotChecked` / `NoQuickInfo` / `Quality(...)`；`any`、`missing` 分别
   产生 `probe root resolves to any|missing`，无 quick info 产生
   `probe root has no quick info`。
4. `moduleBefore != moduleAfter` → `module classification flipped from {a} to
   {b}`。
5. `checkDiagnostics == false` 时在诊断阶段前返回。
6. 诊断先跳过 mount 内（只看 start，与 oracle 相同），再按
   `start < insertionEnd && end > insertionStart` 找第一个相交 insertion，
   产生 `diagnostic {code} intersects insertion {id}`。

类型与边界：

| 项 | 值/行为 |
| --- | --- |
| `MAX_PROBE_HELPERS` | 256，超限 `ProbeHealthError::TooMany` |
| `MAX_PROBE_DIAGNOSTICS` | 1_000，同上 |
| `MAX_PROBE_INSERTIONS` | 1_000，同上 |
| `MAX_PROBE_MOUNTS` | 1_000，同上 |
| `MAX_PROBE_LABEL_BYTES` | 256（helper 名/insertion id），超限 `LabelTooLong` |
| `MAX_PROBE_PROBLEMS` | 64，超限置 `truncated = true`，不伪造问题 |
| span 加法 | 全部 `checked_add`；溢出 `SpanOverflow` |

`ProbeHelperFact` 与 `ProbeAnchors.helpers` 一一对应，同名不同 offset 是两个
条目；这使 `TS-PROBES-0098` 的“按精确 offset、不按名字搜索”在决策层仍可断言。
TS 侧的 `path.resolve(...) === scriptPath` 过滤在 native 没有对应字段：native
shadow 是单脚本，传入的 insertions/mounts 都来自同一 derived script，过滤条件
恒真；此差异已在模块文档中记录。

---

## 2. 逐 case 对照

### 2.1 `TS-MESSAGES-0005`

- Oracle：`backends/typescript/test/messages.test.ts:5` “helper names are
  removed from user-visible messages”。
- Rust： crates/wxml-backend-tsgo/src/query/message.rs:485
  `case_messages_0005_helper_names_are_removed`
- 断言（精确相等，不是回显）：
  - `'__wxml_root.maybeUser' is possibly 'null'.` →
    `'maybeUser' is possibly 'null'.`
  - `Property '__wxml_check_0' does not exist on type 'X'.` →
    `Property 'helper' does not exist on type 'X'.`

### 2.2 `TS-MESSAGES-0016`

- Oracle：`backends/typescript/test/messages.test.ts:16` “typings internals are
  unwrapped”。
- Rust： crates/wxml-backend-tsgo/src/query/message.rs:497
  `case_messages_0016_typings_internals_are_unwrapped`
- 断言：结果不含 `FilterUnknownType`、`PropertyOptionToData`，含
  `{ state: string; }`；另加一条由同一 Rust 逻辑导出的完整字符串相等断言：
  `Property 'subtitle' does not exist on type '{ state: string; } & { title:
  { type: StringConstructor; }; }'.`

### 2.3 `TS-MESSAGES-0025`

- Oracle：`backends/typescript/test/messages.test.ts:25` “generated overlay type
  names are replaced”。
- Rust： crates/wxml-backend-tsgo/src/query/message.rs:510
  `case_messages_0025_generated_overlay_type_names_are_replaced`
- 断言：`{ state: string; } & __WxmlProps_0` →
  `{ state: string; } & declared properties`，整句精确相等。

### 2.4 `TS-MESSAGES-0032`

- Oracle：`backends/typescript/test/messages.test.ts:32` “duplicated intersection
  literals collapse”。
- Rust： crates/wxml-backend-tsgo/src/query/message.rs:520
  `case_messages_0032_duplicated_intersection_literals_collapse`
- 断言：`{ name: string; } & { name: string; }` →
  `{ name: string; }`，整句精确相等。

### 2.5 `TS-MESSAGES-0039`

- Oracle：`backends/typescript/test/messages.test.ts:39` “long messages are
  truncated”。
- Rust： crates/wxml-backend-tsgo/src/query/message.rs:530
  `case_messages_0039_long_messages_are_truncated`
- 断言：60 个 `fieldN: string` 成员的消息，结果 UTF-16 长度 ≤ 200（实际为
  199，因为 `trimEnd` 去掉了第 199 位的空格）、以 `…` 结尾、保留
  `Property 'x' does not exist` 前缀。

### 2.6 `TS-PROBES-0064`

- Oracle：`backends/typescript/test/probes.test.ts:64` “probe health rejects a
  probe root that resolves to any”。
- Rust： crates/wxml-backend-tsgo/src/query/probe_health.rs:455
  `case_probes_0064_probe_health_rejects_a_probe_root_that_resolves_to_any`
- 断言：`root = Quality(Any)` → problems 精确等于
  `["probe root resolves to any"]`，`truncated == false`。

### 2.7 `TS-PROBES-0080`

- Oracle：`backends/typescript/test/probes.test.ts:80` “probe health rejects
  diagnostics that intersect an insertion”。
- Rust： crates/wxml-backend-tsgo/src/query/probe_health.rs:466
  `case_probes_0080_probe_health_rejects_diagnostics_that_intersect_an_insertion`
- 断言：诊断 `2339` 在 insertion `probe`（start 10, length 40）内 →
  `["diagnostic 2339 intersects insertion probe"]`。

### 2.8 `TS-PROBES-0098`

- Oracle：`backends/typescript/test/probes.test.ts:98` “probe health uses the
  exact offsets it is given, not a name search”。
- Rust： crates/wxml-backend-tsgo/src/query/probe_health.rs:486
  `case_probes_0098_probe_health_uses_the_exact_offsets_it_is_given_not_a_name_search`
- 断言：
  - 两个同名 `__wxml_root` 条目都 `Symbol` → problems 为空；
  - 同名条目中第二个为 `NoSymbol` → 只有
    `["probe helper __wxml_root has no symbol"]`。
  这证明判定按条目（offset）而不是按名字聚合；证据采集端必须用
  `ProbeAnchors` 的 offset + `getSymbolAtPosition`，不能用 `indexOf`。

### 2.9 `TS-PROBES-0135`

- Oracle：`backends/typescript/test/probes.test.ts:135` “a duplicate helper is
  reported through the insertion check”。
- Rust： crates/wxml-backend-tsgo/src/query/probe_health.rs:508
  `case_probes_0135_a_duplicate_helper_is_reported_through_the_insertion_check`
- 断言：重复标识符诊断 `2300` 与 insertion `probe` 相交 →
  `["diagnostic 2300 intersects insertion probe"]`。

### 2.10 `TS-PROBES-0153`

- Oracle：`backends/typescript/test/probes.test.ts:153` “probe health reports a
  module classification flip”。
- Rust： crates/wxml-backend-tsgo/src/query/probe_health.rs:528
  `case_probes_0153_probe_health_reports_a_module_classification_flip`
- 断言：`Script` → `Module` →
  `["module classification flipped from script to module"]`。

附加边界单测（不属于 case 证据，但证明 typed 边界）：
`missing_derived_script_is_the_only_problem`、
`diagnostics_inside_a_mount_are_skipped`、
`disabled_diagnostics_keep_pre_diagnostic_problems`、
`zero_length_and_boundary_diagnostics_do_not_intersect`、
`input_caps_are_typed_errors`、`overflowing_spans_are_typed_errors`、
`root_quality_is_distinct_from_no_quick_info_and_not_checked`。

---

## 3. 与 oracle 的等价边界

1. **消息**：oracle 是纯字符串函数，Rust 是同一变换顺序的纯函数；所有 case
   断言由 Rust 计算结果与手算期望比较，没有把 oracle 期望作为输入回显。
2. **probe health**：oracle 的决策逻辑只依赖注入 facts（helper 符号、root
   quality、module 前后、诊断、insertion、mount）。Rust 用同构 typed facts
   重放同一决策表，未调用 checker；这正是 triage §4.2 接受的“证据注入”边界。
3. **未覆盖的 TS 分支**：`helper.file` 不在 program、root 无 quick info、
   `checkDiagnostics == false`、mount 内诊断、span 边界均已实现并加边界测试；
   五个 case 只固定了 oracle 实际断言的子集。
4. **未做**：没有把 probe health 接到 hover/diagnostics 的 `CapabilityStatus`；
   当前 `Supported` 语义不变，避免回归已 green 的 hover/diagnostics。
5. **显示预算分离**：没有改 `wxml-lsp` 的 `MAX_DISPLAY_MESSAGE_CHARS(512)`；
   新常量 200 只属于 C1 消息清洗。

---

## 4. 验证

在并行改动后的工作树执行（HEAD 实际为 `68f852c`）：

```text
cargo fmt --all -- --check                         # FMT_OK
cargo check --workspace --all-targets              # Finished dev profile
cargo clippy -p wxml-backend-tsgo -p wxml-lsp \
  --all-targets -- -D warnings                     # No issues found
cargo test -p wxml-backend-tsgo                    # 658 passed, 4 ignored
cargo test -p wxml-backend-tsgo --lib query::message
                                                    # 8 passed
cargo test -p wxml-backend-tsgo --lib query::probe_health
                                                    # 12 passed
cargo test -p wxml-lsp --lib backend_tsgo          # 312 passed, 639 filtered
cargo test -p wxml-lsp --test native_parity        # 76 passed, 26 ignored
git diff --check                                   # DIFF_CHECK_OK
```

只读参照：`jq empty docs/plans/tsgo-test-ledger.json` 通过；本切片未写该文件。

---

## 5. 台账建议

### 5.1 字段

10 个 case 当前（读取时工作树）均为 `planned`、无
`migration_evidence`。建议逐条应用：

| 字段 | 值 |
| --- | --- |
| `status` | `green` |
| `target_rust_test_status` | `migrated_green` |
| `migration_evidence.parity` | `exact` |
| `migration_evidence.green_eligible` | `true` |
| `migration_evidence.status_recommendation` | `green` |
| `migration_evidence.status_applied` | `green` |
| `migration_evidence.assessed_at` | `2026-11-20` |
| `migration_evidence.implementation_head` | 落账 commit 的 40 位 sha（当前基线 `68f852c`，回填时替换） |
| `migration_evidence.rust_tests[].path` | 见 §5.2 |
| `target_rust_test` | **保持不变**（ledger 的 partition 路由；validator 不要求指向真实测试） |

`migration_evidence.note` 至少写清：oracle 的字符串变换/决策表由 Rust 纯函数
决定，测试是精确相等，且没有 adapter 接线。

### 5.2 测试映射

| Case | path | line | test / fully_qualified 后缀 |
| --- | ---: | ---: | --- |
| `TS-MESSAGES-0005` | crates/wxml-backend-tsgo/src/query/message.rs | 485 | `case_messages_0005_helper_names_are_removed` |
| `TS-MESSAGES-0016` | crates/wxml-backend-tsgo/src/query/message.rs | 497 | `case_messages_0016_typings_internals_are_unwrapped` |
| `TS-MESSAGES-0025` | crates/wxml-backend-tsgo/src/query/message.rs | 510 | `case_messages_0025_generated_overlay_type_names_are_replaced` |
| `TS-MESSAGES-0032` | crates/wxml-backend-tsgo/src/query/message.rs | 520 | `case_messages_0032_duplicated_intersection_literals_collapse` |
| `TS-MESSAGES-0039` | crates/wxml-backend-tsgo/src/query/message.rs | 530 | `case_messages_0039_long_messages_are_truncated` |
| `TS-PROBES-0064` | crates/wxml-backend-tsgo/src/query/probe_health.rs | 455 | `case_probes_0064_probe_health_rejects_a_probe_root_that_resolves_to_any` |
| `TS-PROBES-0080` | crates/wxml-backend-tsgo/src/query/probe_health.rs | 466 | `case_probes_0080_probe_health_rejects_diagnostics_that_intersect_an_insertion` |
| `TS-PROBES-0098` | crates/wxml-backend-tsgo/src/query/probe_health.rs | 486 | `case_probes_0098_probe_health_uses_the_exact_offsets_it_is_given_not_a_name_search` |
| `TS-PROBES-0135` | crates/wxml-backend-tsgo/src/query/probe_health.rs | 508 | `case_probes_0135_a_duplicate_helper_is_reported_through_the_insertion_check` |
| `TS-PROBES-0153` | crates/wxml-backend-tsgo/src/query/probe_health.rs | 528 | `case_probes_0153_probe_health_reports_a_module_classification_flip` |

`fully_qualified` 前缀分别为
`wxml_backend_tsgo::query::message::tests::` 和
`wxml_backend_tsgo::query::probe_health::tests::`。

示例（其余 9 条同构替换 `case_id`/`line`/`test`/`note`）：

```json
{
  "case_id": "TS-MESSAGES-0005",
  "assessed_at": "2026-11-20",
  "implementation_head": "<landing-sha>",
  "parity": "exact",
  "green_eligible": true,
  "status_recommendation": "green",
  "status_applied": "green",
  "note": "Rust `sanitize_message` removes the `__wxml_root.` prefix and replaces `__wxml_check_0` with `helper`; both oracle assertions are exact string equality and no adapter path is involved.",
  "rust_tests": [{
    "path": "crates/wxml-backend-tsgo/src/query/message.rs",
    "line": 485,
    "test": "case_messages_0005_helper_names_are_removed",
    "fully_qualified": "wxml_backend_tsgo::query::message::tests::case_messages_0005_helper_names_are_removed",
    "note": "Asserts both oracle inputs produce the exact cleaned strings."
  }]
}
```

### 5.3 计数变化（读取时 70/231/38）

> 本表是 10 个 messages/probes case 的独立增量，基线为读取时的 70/231/38。
> 与 `TS-ANALYSIS-0315/0332` 合并落账后的最终台账为 **82 green / 219 planned /
> 38 retired**，`exact` 82、`assessed_cases` 209、无证据 planned 92。

当前读取值：`green 70`、`planned 231`、`retired 38`；`exact 70`、
`near_exact 2`、`weaker 96`、`structural 7`、`blocked 22`、`assessed_cases
197`、无证据 planned `104`。

10 个 case 单独 promoted 后的增量为：

| 字段 | 建议值 |
| --- | ---: |
| `green` | 80 |
| `planned` | 221 |
| `retired-with-decision` | 38 |
| `exact` | 80 |
| `near_exact` | 2 |
| `weaker` | 96 |
| `structural` | 7 |
| `blocked` | 22 |
| `assessed_cases` | 207 |
| `green_eligible` / `promoted_to_green` | 80 |
| `remaining_planned_without_evidence` | 94 |

`by_partition`：

- `messages_evidence`：`evidence_cases 0 → 5`、`green_cases 0 → 5`、
  `unassessed_cases 8 → 3`、`exact 0 → 5`，其余不变。
- `guards_js`：`evidence_cases 15 → 20`、`green_cases 9 → 14`、
  `exact 9 → 14`、`unassessed_cases 10 → 5`，`planned_evidence_cases` 保持 6，
  `weaker/structural/blocked` 不变。

`crates/wxml-lsp/tests/native_parity/ledger.rs` 的冻结常量需同步为
`GREEN_CASES = 82`、`PLANNED_CASES = 219`（已与 0315/0332 合并）；`NEAR_EXACT_CASES = 2`、
`WEAKER_CASES = 96`、`STRUCTURAL_CASES = 7`、`BLOCKED_CASES = 22` 不变。
本切片按要求没有修改该文件或 ledger。

> 若并行 session 已先落账部分 case 或改写计数，回填时必须以当时的 case 级
> `status` 和无证据 planned 重新计数，不能直接复制上表。

---

## 6. 风险与未决

1. **probe health 无生产调用者**：本切片按任务要求只做决策层，未接线。
   接线会改变 hover/diagnostics 的 `Supported` 语义，必须单独 gate、单独
   回归已 green 的 hover。
2. **消息清洗未接入发布路径**：本切片只提供纯函数。接入
   `handlers/diagnostics` 时需同步 contract §9.3 的“清洗例外”，并补 raw 只进
   日志的回归测试；不能把 512 显示预算改成 200。
3. **ledger 建议可能过期**：本文件读取期间有其他 session 并行修改 ledger/
   native gate；§5 的数值是读取时快照。
4. **证据采集端约束**：`TS-PROBES-0098` 的精确 offset 语义由调用方保证；
   后续接线必须从 `ProbeAnchors.helpers` 取 offset，禁止按名字搜索。
