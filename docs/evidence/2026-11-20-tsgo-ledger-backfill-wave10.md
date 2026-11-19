# tsgo 测试台账 wave-10 weaker + exact 回填（2026-11-20）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 验证 HEAD：`8a56f27edcc9b91264f4b2bd0f3609450b17ab7c`
- 任务书给定 HEAD：`6727059`；读取与写入期间并行提交了
  `8a56f27`（F3 closure/native default 文档同步），因此按实际 HEAD 记录。
  工作树同时存在并行 session 的未提交测试改动（`input.rs`、`shadow/layout.rs`、
  `shadow/locals.rs`、`shadow/rewrite.rs`、`server.rs`、`shadow_layout.rs`、
  `shadow_rewrite.rs`、`script_behavior.rs`），本轮未修改、未覆盖。
- 范围：只改台账/validator/台账 Markdown/contract/证据文档。未改生产代码、
  测试源码、Cargo、delivery、`fixture_hash.rs`、oracle manifest JSON、README；
  未 commit、未 review。
- 输入：`docs/evidence/2026-11-20-tsgo-remaining-ledger-wave9-plan.md` §3.2 的
  12 个 B 类 weaker 候选，以及 §3.1 的 9 个 exact-ready 候选；`TS-REWRITE-0207`
  来自 near-exact 会话的后续补齐。

**结论：12 个 weaker 登记 + 1 个 near-exact 提升 + 9 个 exact 提升，0 个
weaker 被误升为 green。**

| 项 | wave-9 后 | wave-10 最终 |
| --- | ---: | ---: |
| `green` | 58 | **68** |
| `planned` | 245 | **235** |
| `retired-with-decision` | 36 | 36 |
| `exact` | 58 | **68** |
| `near_exact` | 3 | **2** |
| `weaker` | 70 | **82** |
| `structural` | 7 | 7 |
| `blocked` | 22 | 22 |
| `assessed_cases` | 160 | **181** |
| 无 Rust 证据的 planned | 143 | **122** |

分两段完成：

1. 12 个 weaker 候选登记，快照为 58 green / 245 planned / 172 assessed /
   131 无证据（见 §1.1–§1.12）；
2. `TS-REWRITE-0207` 从 near-exact 提升为 exact，随后 9 个 exact-ready 候选
   从 planned 提升为 green，最终 68/235/181/122（见 §1.13）。

判定口径：`parity=weaker` 表示 oracle 的可观察断言只有一部分能在默认运行的
Rust 测试中找到落点，且缺口是字段、聚合、真实 checker 或 symbol-bridge 布局，
不是“再补一条断言”就能等价。`parity=exact` 表示 oracle 的每条可观察断言都有
等价断言，或已书面记录的 typed-error/机制替代。opt-in `native_engine_real`
gate 未计入任何 case。

---

## 1. 逐 case 落点与缺口

所有 case 保持：

| 字段 | 值 |
| --- | --- |
| `status` | `planned` |
| `target_rust_test_status` | `proposed` |
| `migration_evidence.parity` | `weaker` |
| `migration_evidence.green_eligible` | `false` |
| `migration_evidence.status_recommendation` | `planned` |
| `migration_evidence.status_applied` | `planned` |
| `migration_evidence.assessed_at` | `2026-11-20` |
| `migration_evidence.implementation_head` | `8a56f27edcc9b91264f4b2bd0f3609450b17ab7c` |

> 任务书写 `status_recommendation=weaker`；但 validator 和既有 70 个 weaker
> case 都把 recommendation 定义为“推荐状态”，只允许 `green`/`planned`，
> `weaker` 是 parity 不是 status。为了让 `native_parity` 保持绿色并与既有
> 语义一致，本轮使用 `planned`，缺口由 `parity=weaker` 表达。

### 1.1 `TS-ANALYSIS-0205`

- Oracle：`backends/typescript/test/analysis.test.ts:205`
  “locals mount the expression without a root prefix”。
- Rust：
  -  crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:85
    `case_rewrite_0088_template_locals_are_not_prefixed_anywhere_in_the_expression`
  -  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:317
    `case_locals_0343_a_locals_only_request_still_degrades_to_partial_with_a_typed_reason`
- 已覆盖：local 表达式不加 `__wxml_root.`；legacy locals 请求降级为 Partial，
  reason 含 `could not be resolved`，并发出 `const item = undefined as any;`。
- 仍缺：`mounts[0].text == "item.name"`、`segments.length == 1`、
  `probeProblems == []`，以及 hover/diagnostics/definition 三个 `states` 聚合。

### 1.2 `TS-ANALYSIS-0255`

- Oracle：`backends/typescript/test/analysis.test.ts:255`
  “the default target is the rightmost identifier”。
- Rust：
  -  crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:153
    `case_rewrite_0156_repeated_identifiers_keep_the_rightmost_target_offset`
  -  crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:196
    `case_rewrite_0183_object_keys_are_not_default_targets`
- 已覆盖：默认 target 是 rightmost identifier（`a.a`、`user.user`、
  `title + title`），并断言 derived offset 与映射后的 WXML range；
  `user.name` 的 target 是 `name`，对象 key 不会成为 target。
- 仍缺：C1 response 的 fragment-scoped `target` 字段；同一响应里的
  `f0` hover `typeOnly: "string"`。

### 1.3 `TS-MAPPING-0188`

- Oracle：`backends/typescript/test/mapping.test.ts:188`
  “nullable receiver diagnostics land on the receiver, not the member”。
- Rust：
  -  crates/wxml-backend-tsgo/src/shadow/mapping.rs:882
    `a_rewritten_expression_maps_the_nullable_object_to_its_identifier`
  -  crates/wxml-backend-tsgo/src/shadow/mapping.rs:860
    `a_leading_rewrite_prefix_is_not_an_approximate_crop`
- 已覆盖：`__wxml_root.maybeUser` span 精确映射到 `maybeUser`；
  leading prefix 不算近似 crop。
- 仍缺：`diagnosticsExact`、两个 layout、message helper 过滤。
- **语义分歧（不是缺字段）**：oracle 对 receiver 诊断断言
  `trimmedSyntheticPrefix === true`；native 把该字段当近似信号，纯 leading
  rewrite prefix 返回 `false`（`mapping.rs` 注释明确记录这是有意行为）。
  因此该 case 即使补测试也不能按 oracle 字段升级 exact。

### 1.4 `TS-MAPPING-0208`

- Oracle：`backends/typescript/test/mapping.test.ts:208`
  “one expression mounted twice keeps both mappings and publishes one diagnostic”。
- Rust：
  -  crates/wxml-backend-tsgo/tests/shadow_mapping.rs:305
    `case_mapping_0145_every_mount_carries_an_explicit_synthetic_source_split`
  -  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:2147
    `caps_and_dedup`
- 已覆盖：两个 fragment 生成两条 mount，segment 覆盖完整且 offset 绝对；
  component binding 的相同诊断会折叠为一条。
- 仍缺：没有“同一表达式 mount 两次”后比较 `diagnostics.length` 的测试；
  template diagnostics 的 `seen` 去重（code+range+message）未被直接断言；
  `diagnosticsExact` 和同 run 的 `user.name` hover 缺失。

### 1.5 `TS-MATRIX-0215`

- Oracle：`backends/typescript/test/matrix.test.ts:215`
  “interface used as `data` value is reported, never fabricated”。
- Rust： crates/wxml-backend-tsgo/src/shadow/layout.rs:2357
  `non_literal_methods_and_data_are_unsupported`
- 已覆盖：`Page({ data: IData })` 且同文件 interface 时返回
  `ShadowError::Unsupported { feature: "data.root" }`。
- 仍缺：oracle 断言 reason 匹配 `/type-only/`、`states.hover.status` 为
  `unsupported`、`diagnostics.length == 0`；native 无 `states` 聚合，
  symbol-bridge layout 已退役。

### 1.6 `TS-MATRIX-0228`

- Oracle：`backends/typescript/test/matrix.test.ts:228`
  “a shorthand `data` member is unsupported, not 'missing'”。
- Rust： crates/wxml-backend-tsgo/src/shadow/layout.rs:2357
  `non_literal_methods_and_data_are_unsupported`
- 已覆盖：共享的非对象 `data` root 走 `data.root` 的 `Unsupported` arm。
- 仍缺：测试只走 type-only 分支；`Page({ data })` shorthand 分支没有独立断言，
  reason `/not a property assignment/` 与“不是 missing”的负向断言也未覆盖。

### 1.7 `TS-MATRIX-0301`

- Oracle：`backends/typescript/test/matrix.test.ts:301`
  “multi-registration uses the first registration and does not merge”。
- Rust： crates/wxml-backend-tsgo/tests/shadow_layout.rs:367
  `multiple_registrations_are_partial_and_choose_the_first`
- 已覆盖：两个 page 注册时 `status == Partial`、`registration.count == 2`、
  reason 含 `2 page registrations`，且只挂载第一个注册。
- 仍缺：第二个注册的 `title: 42` 未合并到 hover 的 `typeOnly: "string"`；
  `states.hover` 和 symbol-bridge 半边缺失。

### 1.8 `TS-OVERLAY-0036`

- Oracle：`backends/typescript/test/overlay.test.ts:36`
  “JSDoc-only edits are visible without a type change”。
- Rust：
  -  crates/wxml-lsp/src/backend_tsgo/tests.rs:1125
    `hover_composes_type_documentation_and_jsdoc`
  -  crates/wxml-lsp/tests/zed_sync.rs:592
    `unsaved_script_overlay_reaches_wxml_analysis`
- 已覆盖：hover 组合 type fence、注释与 `@deprecated`；未保存脚本改动会进入
  WXML hover 和 diagnostics，不需要落盘。
- 仍缺：没有“只改 JSDoc、type 不变”的 overlay 测试；`docSource` 和
  symbol-bridge 缺失。

### 1.9 `TS-PROBES-0176`

- Oracle：`backends/typescript/test/probes.test.ts:176`
  “read policy keeps project files out of disk reads”。
- Rust： crates/wxml-backend-tsgo/src/native/vfs.rs:1385
  `reads_only_virtual_files_and_never_host_files`
- 已覆盖：项目虚拟文件可读，缺失路径返回 `missing`，root 之外的真实 host
  文件永远不可读。
- 仍缺：没有 `diskReadable()` 枚举（只包含 engine/typings 资产）；oracle 的
  `2307` 未解析 import 观测依赖真实 checker。

### 1.10 `TS-REWRITE-0349`

- Oracle：`backends/typescript/test/rewrite.test.ts:349`
  “cross-gap diagnostics are cropped to a real operand”。
- Rust：
  -  crates/wxml-backend-tsgo/src/shadow/mapping.rs:900
    `cross_gap_and_right_side_crops_stay_approximate`
  -  crates/wxml-backend-tsgo/tests/shadow_mapping.rs:284
    `case_mapping_0126_cross_gap_target_is_used_only_when_fully_inside_the_span`
- 已覆盖：cross-gap span 裁到真实 operand（`b` 的 4..5），并置
  `trimmed_synthetic_prefix`；target 只有在 span 完全包含时才使用。
- 仍缺：诊断 code `2365`/`2363`、`attribution=trimmed-synthetic` 和
  `states.diagnostics.status == "supported"` 来自 checker/C1。

### 1.11 `TS-SERVER-0129`

- Oracle：`backends/typescript/test/server.test.ts:129`
  “unknown methods, bad JSON and bad params do not kill the server”。
- Rust： crates/wxml-lsp/tests/lsp.rs:354
  `unknown_methods_and_malformed_json_are_reported`
- 已覆盖：真实 stdio session 对未知方法返回 `-32601`，对坏 JSON 返回
  `-32700` 且 `id: null`，随后正常 shutdown。
- 仍缺：native 没有自定义 `analyze`/`ping`；`analyze` 的 `-32602 data.code`
  和 `ping.result.ok == true` 无等价契约。

### 1.12 `TS-SERVER-0171`

- Oracle：`backends/typescript/test/server.test.ts:171`
  “malformed requests are rejected as invalid params, not internal errors”。
- Rust： crates/wxml-lsp/tests/lsp.rs:771
  `invalid_completion_parameters_are_rejected`
- 已覆盖：真实 stdio session 对缺失 completion params 和非数字 position 返回
  JSON-RPC `-32602`。
- 仍缺：`analyze` 的 `data.code=bad-params`/`bad-fragment` 分类；native 只有
  通用 `INVALID_PARAMS`。

---

### 1.13 Wave-10 exact（9）

9 个 case 均为默认运行的 Rust 测试，`parity=exact`、`green_eligible=true`、
`status=green`、`target_rust_test_status=migrated_green`。

| Case | Oracle | Rust（path:line） | 覆盖说明 |
| --- | --- | --- | --- |
| `TS-PROBES-0048` | `backends/typescript/test/probes.test.ts:48` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2579 | leading block 内最后一个 pragma 生效：check→nocheck 得到 `check=false`/`no_check=true`，`neutralize_offset` 在两条 pragma 之后；反向顺序也覆盖。 |
| `TS-PROBES-0059` | `backends/typescript/test/probes.test.ts:59` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2612 | leading comment 之外的 `@ts-nocheck` 被忽略：普通注释和代码后的 pragma 都是 `no_check=false`。 |
| `TS-ANALYSIS-0196` | `backends/typescript/test/analysis.test.ts:196` |  crates/wxml-backend-tsgo/src/shadow/rewrite.rs:774 | 非 whole identifier target 被拒绝；note 如实记录 typed `BadExpression` 替代 TS `bad-fragment` error code，并有 `user`/`name` 正向对照。 |
| `TS-ANALYSIS-0349` | `backends/typescript/test/analysis.test.ts:349` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2635 | 重复 fragment id 返回 `Invalid { feature: "fragment.id" }` 和唯一性 reason；note 如实记录这是 TS 结构化 `bad-fragment` 的 typed-error 替代口径。 |
| `TS-PROTOCOL-0126` | `backends/typescript/test/protocol.test.ts:126` |  crates/wxml-backend-tsgo/src/shadow/locals.rs:569 | same-name nested binding chain 被接受并分成 2 层；inner `item.members` 用 capture helper，无 TDZ 自引用。 |
| `TS-PROTOCOL-0280` | `backends/typescript/test/protocol.test.ts:280` |  crates/wxml-backend-tsgo/src/input.rs:2102 | 未知 option 返回 `UnsupportedCompilerOption { key }`；`strictNullChecks`/`noUncheckedIndexedAccess`/`strictBuiltinIteratorReturn` 保留。 |
| `TS-PROTOCOL-0319` | `backends/typescript/test/protocol.test.ts:319` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:567 | policy off 在 source `@ts-check` 后插入 `nocheck`（offset 13、length 15、replace 0），notes 记录 neutralised，derived text 顺序正确。 |
| `TS-PROTOCOL-0600` | `backends/typescript/test/protocol.test.ts:600` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2663 | 无 target/locals 的 legacy `FragmentSpec` 仍可构建，两个 mount 都带 `__wxml_root.` 前缀。 |
| `TS-SERVER-0150` | `backends/typescript/test/server.test.ts:150` |  crates/wxml-lsp/src/server.rs:2564 | unknown/malformed/valid notification 都不产生响应；后续 request 拿到第一个 reply（id 7）。 |

## 2. 台账与文档改动

| 文件 | 改动 |
| --- | --- |
| `docs/plans/tsgo-test-ledger.json` | 12 个 case 新增 weaker 证据；`TS-REWRITE-0207` 提升为 exact；9 个 exact-ready case 提升为 green。最终 `green` 59→68、`planned` 244→235、`exact` 59→68、`assessed_cases` 172→181、无证据 131→122；同步 9 个相关分区的 `evidence_cases`/`green_cases`/`unassessed_cases`/`exact`；更新汇总 `implementation_head`/`rust_test_result`。 |
| `crates/wxml-lsp/tests/native_parity/ledger.rs` | `WEAKER_CASES` 70→82、`GREEN_CASES` 59→68、`PLANNED_CASES` 244→235、`NEAR_EXACT_CASES` 3→2；更新注释。 |
| `docs/plans/2026-11-17-tsgo-test-ledger.md` | 顶部状态/HEAD、§1 摘要、§7 计数句、§23 与 §23.6/§23.7。 |
| `docs/reference/tsgo-test-contract.md` | `green` 68、`planned` 235、`exact` 68、`near_exact` 2、`assessed_cases` 181、无证据 122，并补 wave-10 历史段。 |
| 本文件 | 12 个 weaker + 9 个 exact case 的真实落点和缺口。 |

分区变化：

| 分区 | 有证据 | planned 有证据 | weaker | 无证据 |
| --- | ---: | ---: | ---: | ---: |
| `loops` | 9→11 | 1→3 | 0→2 | 7→5 |
| `mapping_utf16` | 15→18 | 9→12 | 8→11 | 6→3 |
| `scenario_matrix` | 0→3 | 0→3 | 0→3 | 43→40 |
| `project_overlay` | 6→8 | 6→8 | 6→8 | 14→12 |
| `protocol_limits` | 2→4 | 2→4 | 2→4 | 38→36 |

上表是 12 个 weaker 登记完成时的分区快照。随后 9 个 exact 提升：

| 分区 | 有证据 | green | exact | 无证据 |
| --- | ---: | ---: | ---: | ---: |
| `guards_js` | 13→15 | 7→9 | 7→9 | 12→10 |
| `hover_docs` | 7→8 | 2→3 | 2→3 | 3→2 |
| `project_overlay` | 8→9 | 0→1 | 0→1 | 12→11 |
| `protocol_limits` | 4→9 | 0→5 | 0→5 | 36→31 |

`near_exact`/`weaker`/`structural`/`blocked` 和 `routed_cases` 不变；全表以
JSON `counts.migration_evidence.by_partition` 为准。

---

## 3. 验证

在 HEAD `8a56f27` 加本轮未提交改动上执行：

| 命令 | 结果 |
| --- | --- |
| `jq empty docs/plans/tsgo-test-ledger.json` | exit 0 |
| `cargo test -p wxml-lsp --test native_parity` | 75 passed / 0 failed / 26 ignored |
| `cargo fmt --all -- --check` | exit 0 |
| `git diff --check` | exit 0 |

validator 逐条检查了：evidence 的 `case_id`、非空 `assessed_at`、40 位 hex
`implementation_head`、`parity`/`green_eligible`/`status_recommendation`/
`status_applied` 一致性、`note` 长度、每个 `rust_tests[]` 的 `.rs` 路径、
行号在文件内、测试函数真实声明、fully-qualified 名称结尾，以及
`status=planned` + `target_rust_test_status=proposed` 或
`status=green` + `target_rust_test_status=migrated_green` 的组合。

---

## 4. 限制

1. **未改测试。** 本轮只登记现有测试；12 个 weaker 结论不声称 case 达到 exact，
   9 个 exact 结论也不把 opt-in native gate 当作 green。
2. **行号来自并发工作树。** `server.rs` 在记录期间被并行 session 反复修改；
   `TS-SERVER-0150` 指向  crates/wxml-lsp/src/server.rs:2564，`TS-SERVER-0171`
   仍只引用稳定的  crates/wxml-lsp/tests/lsp.rs:771。其余引用在写入时重新
   `grep` 过；若并行改动继续移动行号，以再次读取为准。
3. **`TS-MAPPING-0188` 存在语义分歧。** native 的
   `trimmed_synthetic_prefix` 是近似信号，oracle 是 reporting label；该 case
   不应为了计数改成 exact。
4. **weaker 的 `status_recommendation` 使用 `planned`。** 见 §1 开头说明；
   `weaker` 记录在 `parity` 字段；9 个 exact case 使用 `green`。
5. 未做独立 review；21 个 case 仍处于 `reviewed` 之前的状态。最终
   `green` 68 / `planned` 235，`retired` 36。
6. README 证据索引未改；如需要，由统一收口的 session 追加本文件。
