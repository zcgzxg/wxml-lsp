# tsgo 测试台账 wave-10b weaker 回填（2026-11-20）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 验证 HEAD：`43625ee7cd6233b5fbac7d8cc4f2d1cbdd469f12`（任务书给定
  `456a1bb`；读取期间并行提交了 `5cc1255`、`43625ee`，按实际 HEAD 记录）。
- 范围：只改台账/validator/台账 Markdown/contract/证据文档。未改生产代码、
  测试源码、Cargo、delivery、`fixture_hash.rs`、oracle manifest JSON、README；
  未 commit、未 review。
- 输入：`docs/evidence/2026-11-20-tsgo-remaining-ledger-wave9-plan.md` §3.2 的
  25 个 B 类 weaker 候选。其中前 12 个已由
  `docs/evidence/2026-11-20-tsgo-ledger-backfill-wave10.md` 登记，本轮只处理
  **剩余 13 个**，并逐条确认它们在台账中原本没有 `migration_evidence`。
- 并行状态：工作树仍有其他 session 的未提交改动
  （`crates/wxml-backend-tsgo/tests/shadow_rewrite.rs`、
  `crates/wxml-lsp/src/backend_tsgo/tests.rs` 等），本轮未修改、未覆盖；
  `path:line` 为写入时重新 `grep` 的值。

**结论：13 个 weaker 登记，0 个 weaker 被误升为 green。**

| 项 | wave-10 后 | wave-10b 后 |
| --- | ---: | ---: |
| `green` | 68 | **68** |
| `planned` | 235 | **235** |
| `retired-with-decision` | 36 | 36 |
| `exact` | 68 | **68** |
| `near_exact` | 2 | **2** |
| `weaker` | 82 | **95** |
| `structural` | 7 | 7 |
| `blocked` | 22 | 22 |
| `assessed_cases` | 181 | **194** |
| 无 Rust 证据的 planned | 122 | **109** |

判定口径与 wave-10 相同：`parity=weaker` 表示 oracle 的可观察断言只有一部分能在
默认运行的 Rust 测试中找到落点，缺口是字段、聚合、真实 checker 或
symbol-bridge 布局，不是“再补一条断言”就能等价。opt-in `native_engine_real`
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
| `migration_evidence.implementation_head` | `43625ee7cd6233b5fbac7d8cc4f2d1cbdd469f12` |

> validator 只允许 `planned` 作为非 green 的推荐状态；`weaker` 是 parity 不是
> status，因此 recommendation 统一写 `planned`，缺口由 `parity=weaker` 表达。

### 1.1 `TS-COMPLETION-0066`

- Oracle：`backends/typescript/test/completion.test.ts:66` “complete rejects
  malformed query points”：null request / offset `-1` / offset `1.5` /
  unknown kind → `bad-params`；257 字节 prefix → `bad-limit`。
- Rust：
  -  crates/wxml-lsp/tests/lsp.rs:771
    `invalid_completion_parameters_are_rejected`
  -  crates/wxml-lsp/src/backend_tsgo/tests.rs:2411
    `a_non_expression_completion_kind_is_explicitly_unsupported`
- 已覆盖：真实 stdio session 对缺失 completion params 与非数字 position 返回
  `-32602` 且服务器存活；unknown kind 返回空 typed `Unsupported`，不伪造 item。
- 仍缺：没有 `RequestError`/`bad-params`/`bad-limit` 码，也没有 256 字节
  prefix 上限；`u32` offset 让负数/小数 offset 不可表示（不是被拒绝）；
  unknown kind 是降级结果而非 throw。

### 1.2 `TS-MATRIX-0271`

- Oracle：`backends/typescript/test/matrix.test.ts:271` “component properties
  union needs the declaration overlay in both candidates”：
  `typeOnly == "string | number"`、documentation 为固定中文串、
  `documentationSource == "copied"`，且两个 layout 都成立。
- Rust：
  -  crates/wxml-lsp/src/backend_tsgo/hover/tests.rs:113
    `a_declared_union_replaces_an_under_reported_native_type`
  -  crates/wxml-lsp/src/backend_tsgo/hover/tests.rs:597
    `the_template_plan_merges_behavior_optional_types_into_hover_facts`
  -  crates/wxml-lsp/tests/component_intelligence.rs:515
    `unsaved_component_properties_update_static_queries`
- 已覆盖：声明式 `type + optionalTypes` union 覆盖 native 低报类型；request plan
  把 `cardTitle: string | number` 写进 derived layout 与 hover facts；未保存
  component 的 completion detail / documentation / hover 同时带 union 与 JSDoc。
- 仍缺：没有 `documentationSource="copied"` 字段；没有对 oracle 固定文档串
  `标题，支持字符串或数字` 的端到端断言；symbol-bridge layout 已退役。

### 1.3 `TS-OVERLAY-0076`

- Oracle：`backends/typescript/test/overlay.test.ts:76` “a model file outside
  the snapshot is never read from disk”：snapshot 去掉 `models.ts` 后 hover 必须
  保持 `any`/`unknown`，证明没有落盘读取。
- Rust：
  -  crates/wxml-backend-tsgo/src/native/vfs.rs:1385
    `reads_only_virtual_files_and_never_host_files`
  -  crates/wxml-lsp/src/backend_tsgo/tests.rs:1251
    `hover_unknown_quality_keeps_backend_partial_without_a_type_fence`
- 已覆盖：VFS 只服务项目虚拟文件，缺失路径返回 `missing`，root 外的真实 host
  文件永不返回；unknown hover 保持 Partial 且不伪造 type fence。
- 仍缺：没有“snapshot 缺 models.ts 后 hover 仍为 any/unknown”的 cross-file
  overlay 测试；unknown 测试只证明降级形态，不是同一场景；无第二 layout。

### 1.4 `TS-PROTOCOL-0053`

- Oracle：`backends/typescript/test/protocol.test.ts:53` “protocol v2 accepts
  optional bindings, guards, components and events”：`protocolVersion == 2` 且
  带四类可选结构时 `validateRequest` 不抛错。
- Rust：
  -  crates/wxml-backend-tsgo/src/shadow/locals.rs:569
    `case_protocol_0126_a_same_name_nested_chain_is_accepted_and_groups_two_levels`
  -  crates/wxml-backend-tsgo/src/shadow/guards.rs:310
    `render_guards_opens_if_elif_and_closes_one_block`
  -  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:2053
    `two_components_with_the_same_property_keep_their_own_types`
  -  crates/wxml-backend-tsgo/tests/script_events.rs:83
    `case_events_0071_page_method_shorthand_function_and_arrow_resolve`
- 已覆盖：bindings 被接受并分组、guards 被渲染为 narrowing block、component
  usage 按组件保留属性类型、events 解析 shorthand/function/arrow handler。
- 仍缺：没有单一 C1 `validateRequest`，也没有 `protocolVersion` 字段；四类结构
  分散在各自 typed API，不能在同一 request 里一次断言。

### 1.5 `TS-PROTOCOL-0098`

- Oracle：`backends/typescript/test/protocol.test.ts:98` “protocol v2 rejects
  malformed structured bindings”：非法 role（`element`）与 reversed
  `sourceRange` 都抛 `RequestError`，`code == "bad-fragment"`。
- Rust：
  -  crates/wxml-backend-tsgo/src/shadow/locals.rs:558
    `invalid_binding_names_are_ignored`
  -  crates/wxml-backend-tsgo/src/shadow/layout.rs:2724
    `invalid_fragment_ranges_are_rejected`
- 已覆盖：非法/重复 binding name 被丢弃并降级；fragment range 长度不符返回
  typed `ShadowError::Invalid { feature: "fragment.range" }`。
- 仍缺：非法 role 由 enum 变成不可表示；native 对非法 name 是丢弃而非拒绝；
  没有 `bad-fragment` 请求错误码，也没有 request 级 validator。

### 1.6 `TS-PROTOCOL-0306`

- Oracle：`backends/typescript/test/protocol.test.ts:306` “the response reports
  registration, applied policy and request digest”：`{kind:"page",count:2,
  topLevel:true}`、`jsCheckApplied=="inherit"`、`compilerOptionsDigest` 匹配
  `/^fnv1a:/`、`fileCount==1`、`fragmentCount==1`、off 场景
  `jsCheckPolicy/jsCheckApplied=="off"`。
- Rust：
  -  crates/wxml-backend-tsgo/tests/script_registration.rs:394
    `s1_two_top_level_pages_are_ambiguous_and_partial`
  -  crates/wxml-backend-tsgo/tests/shadow_layout.rs:367
    `multiple_registrations_are_partial_and_choose_the_first`
  -  crates/wxml-backend-tsgo/tests/shadow_layout.rs:550
    `source_ts_check_is_neutralised_when_the_policy_is_off`
  -  crates/wxml-backend-tsgo/src/shadow/layout.rs:2467
    `case_probes_0027_pragma_scanning_handles_plain_files_bom_and_hashbang`
- 已覆盖：top-level 选择/歧义、`registration.count == 2`、`JsCheckApplied::Off`
  的 `@ts-nocheck` 中和、以及 `Inherit` 的应用结果都在内部结构上有断言。
- 仍缺：没有 response envelope，也没有 `fnv1a:` options digest、
  `fileCount`/`fragmentCount`、`registration` 对象字段。

### 1.7 `TS-PROTOCOL-0438`

- Oracle：`backends/typescript/test/protocol.test.ts:438` “BOM + pragma + probe
  insertions share one fileId”：`insertionId` 排序后为
  `["bom","nocheck","probe"]`，所有 `insertion.file == "index.js"`，且
  `request.scriptFileId == "index.js"`。
- Rust：
  -  crates/wxml-backend-tsgo/tests/shadow_mapping.rs:529
    `case_mapping_0239_published_evidence_carries_insertions_and_mounts_for_every_run`
  -  crates/wxml-backend-tsgo/tests/shadow_layout.rs:78
    `case_layouts_0049_page_options_accept_an_injected_probe_member`
  -  crates/wxml-backend-tsgo/src/shadow/layout.rs:2427
    `js_pragma_and_bom_edits_are_recorded`
- 已覆盖：同一 run 的 insertions/mounts 都被发布且互相引用；probe insertion
  可删除还原原始脚本；BOM+hashbang 同时记录 `bom` 与 `pragma`。
- 仍缺：没有一次断言 `{bom,nocheck,probe}` 三个 id 的组合；shadow 是单文件，
  `insertion.file` 与 `request.scriptFileId` 字段不存在。

### 1.8 `TS-PROTOCOL-0505`

- Oracle：`backends/typescript/test/protocol.test.ts:505` “fragment ids from
  the request are echoed, not replaced by expressions”：rename 成
  `f0/f1/f2` 后 response fragment ids 仍为 `f0/f1/f2`，mount ids 为
  `f0#0/f1#0/f2#0`。
- Rust：
  -  crates/wxml-backend-tsgo/tests/shadow_mapping.rs:305
    `case_mapping_0145_every_mount_carries_an_explicit_synthetic_source_split`
  -  crates/wxml-backend-tsgo/src/shadow/layout.rs:2663
    `case_protocol_0600_legacy_fragments_without_target_or_locals_still_build`
- 已覆盖：fragment id 与表达式文本不同的两个 fragment 各自 mount，且每个 mount
  能回指自己的 fragment id 与 insertion；legacy fragment 同样独立 mount。
- 仍缺：没有把 id 改成 `f0/f1/f2` 并断言 response 的 fragment/mount id 列表；
  mount id 的 `{id}#0` 格式没有直接断言。

### 1.9 `TS-PROTOCOL-0525`

- Oracle：`backends/typescript/test/protocol.test.ts:525` “the same expression
  may be mounted under two fragment ids”：同一 `title` 表达式挂到 `f0`/`f1`，
  mounts 长度为 2 且 id 为 `f0#0`/`f1#0`。
- Rust：
  -  crates/wxml-backend-tsgo/tests/shadow_mapping.rs:305
    `case_mapping_0145_every_mount_carries_an_explicit_synthetic_source_split`
  -  crates/wxml-backend-tsgo/src/shadow/layout.rs:2663
    `case_protocol_0600_legacy_fragments_without_target_or_locals_still_build`
- 已覆盖：每个 FragmentSpec 独立 plan/mount，两个 fragment 得到两个 mount。
- 仍缺：现有测试是两个不同表达式；没有“同一表达式挂两次”的断言；也没有
  response envelope。

### 1.10 `TS-PROTOCOL-0549`

- Oracle：`backends/typescript/test/protocol.test.ts:549` “duplicate fragment
  ids are a structured request error, not a crash”：不抛错、
  `states.hover.status == "unavailable"`、reason 匹配 `/bad-fragment/`、
  `response.fragments.length == 0`。
- Rust： crates/wxml-backend-tsgo/src/shadow/layout.rs:2635
  `case_analysis_0349_duplicate_fragment_ids_are_a_structured_error`
- 已覆盖：重复 id 在 build 前返回 typed
  `ShadowError::Invalid { feature: "fragment.id" }` 与唯一性 reason。
- 仍缺：native 是 typed error 而不是“降级响应”；没有 `states` 聚合，也没有
  “不抛错 + 0 fragments”的同一行为。

### 1.11 `TS-REWRITE-0368`

- Oracle：`backends/typescript/test/rewrite.test.ts:368` “number+string
  concatenation is not fabricated as an error”：`user.age + user.name` 与
  `-title` 都 `ok`、每个 fragment `diagnostics == []`、
  `states.diagnostics.status == "supported"`。
- Rust：
  -  crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:85
    `case_rewrite_0051_unary_parenthesized_and_multi_root_expressions_are_fully_rewritten`
  -  crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:364
    `case_rewrite_0309_rewritten_mounts_map_target_spans_and_trim_synthetic_prefixes`
- 已覆盖：unary/parenthesized/multi-root 表达式完整 rewrite，mount/target 映射
  正确，`-title` 与成员算术保持合法 host 语法。
- 仍缺：native 没有 per-fragment diagnostics 或 `states`；“不伪造诊断”仍被
  `crates/wxml-backend-tsgo/tests/shadow_rewrite.rs` 的 `BLOCKED_CASES` 记为
  需要真实 checker。

### 1.12 `TS-SERVER-0216`

- Oracle：`backends/typescript/test/server.test.ts:216` “ten concurrent
  requests are all answered on the same stream”：10 个 `ping`/`analyze`
  pipeline 后，收到的 id 集合等于发送集合。
- Rust： crates/wxml-lsp/tests/lsp.rs:504
  `many_frames_without_reading_do_not_drop_responses`
- 已覆盖：200 个 request 在未读取的情况下连续发送，最终每个 response id 与
  shutdown 都在同一 stdout stream 上收到。
- 仍缺：native 没有 `ping`/`analyze` 方法；单 worker 串行执行，不等价于 10 个
  并发 analyze 的交错语义。

### 1.13 `TS-SERVER-0267`

- Oracle：`backends/typescript/test/server.test.ts:267` “entry detection
  follows a symlinked install path”：从 symlink 入口启动后 `ping` 返回
  `id == "symlink"` 且 `result.ok == true`。
- Rust：
  -  crates/wxml-delivery/src/install.rs:721
    `versions_are_sorted_and_current_requires_a_valid_link`
  -  crates/wxml-lsp/tests/lsp.rs:233
    `initialize_hover_shutdown_flow`
- 已覆盖：安装根的 `current` symlink 可切换到版本目录并读回版本；真实 stdio
  session 对 initialize/hover 返回匹配 id 并正常 shutdown。
- 仍缺：没有 Node `resolveBackendDir(server.ts)` 的入口探测等价物；没有
  `ping` 方法，symlink 路径只覆盖 native 安装布局。

---

## 2. 台账与文档改动

| 文件 | 改动 |
| --- | --- |
| `docs/plans/tsgo-test-ledger.json` | 13 个 case 新增 `migration_evidence`（weaker/planned）；`counts.migration_evidence` 更新为 `assessed_cases` 194、`weaker` 95、无证据 109，并同步 5 个分区的 `evidence_cases`/`planned_evidence_cases`/`unassessed_cases`/`weaker`；更新 `implementation_head` 与 `rust_test_result`。 |
| `crates/wxml-lsp/tests/native_parity/ledger.rs` | `WEAKER_CASES` 82→95，更新注释；其余常量不变。 |
| `docs/plans/2026-11-17-tsgo-test-ledger.md` | 顶部日期/状态句、wave-10b HEAD、§1 摘要、§7 计数句、新增 §23.8。 |
| `docs/reference/tsgo-test-contract.md` | `assessed_cases` 194、`weaker` 95、无证据 109，并补 wave-10b 历史段。 |
| 本文件 | 13 个 case 的真实落点和缺口。 |

分区变化：

| 分区 | 有证据 | planned 有证据 | weaker | 无证据 |
| --- | ---: | ---: | ---: | ---: |
| `completion` | 8→9 | 3→4 | 3→4 | 1→0 |
| `mapping_utf16` | 18→19 | 12→13 | 11→12 | 3→2 |
| `project_overlay` | 9→10 | 8→9 | 8→9 | 11→10 |
| `protocol_limits` | 9→18 | 4→13 | 4→13 | 31→22 |
| `scenario_matrix` | 3→4 | 3→4 | 3→4 | 40→39 |

`green`/`exact`/`near_exact`/`structural`/`blocked`/`routed_cases` 不变；全表以
JSON `counts.migration_evidence.by_partition` 为准。

---

## 3. 验证

在 HEAD `43625ee` 加本轮未提交改动上执行：

| 命令 | 结果 |
| --- | --- |
| `jq empty docs/plans/tsgo-test-ledger.json` | exit 0 |
| `cargo test -p wxml-lsp --test native_parity` | 76 passed / 0 failed / 26 ignored |
| `cargo fmt --all -- --check` | exit 0 |
| `git diff --check` | exit 0 |

validator 逐条检查了：evidence 的 `case_id`、非空 `assessed_at`、40 位 hex
`implementation_head`、`parity`/`green_eligible`/`status_recommendation`/
`status_applied` 一致性、`note` 长度、每个 `rust_tests[]` 的 `.rs` 路径、
行号在文件内、测试函数真实声明、fully-qualified 名称结尾，以及
`status=planned` + `target_rust_test_status=proposed` 的组合。

---

## 4. 限制

1. **未改测试。** 本轮只登记现有测试；weaker 结论不声称 case 达到 exact，也不把
   opt-in native gate 当作 green。
2. **行号来自并发工作树。** 记录期间 `crates/wxml-lsp/src/backend_tsgo/tests.rs`
   与 `crates/wxml-backend-tsgo/tests/shadow_rewrite.rs` 正被并行 session 修改，
   引用行号以写入时 `grep` 为准；validator 只要求行号在文件内且函数存在。
3. **没有新增能力。** `TS-PROTOCOL-*` 的 C1 字段、`TS-REWRITE-0368` 的真实
   checker、`TS-SERVER-0216/0267` 的 `ping` 与 Node 入口探测都仍是缺口，不能
   用现有 in-place 行为冒充 exact。
4. **`status_recommendation` 使用 `planned`。** 见 §1 开头说明；`weaker` 记录在
   `parity` 字段。
5. 未做独立 review；本轮后 `green` 68 / `planned` 235，`retired` 36。
6. README 证据索引未改；如需要，由统一收口的 session 追加本文件。
