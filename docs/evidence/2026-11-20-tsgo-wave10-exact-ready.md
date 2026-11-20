# tsgo wave-10 低风险 exact 实现证据（2026-11-20）

## 0. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务书给定 HEAD：`6727059`。读取与实现期间并行提交
  `8a56f27`（`docs: sync F3 closure and native default status`）落地；本文件验证时
  HEAD 为 `8a56f27edcc9b91264f4b2bd0f3609450b17ab7c`。
- 记录日期：2026-11-20
- 范围：只新增 Rust 测试断言与本文。生产行为、 `Cargo.toml`、delivery、默认后端均未改。
  - 新增断言全部位于测试代码：`#[cfg(test)] mod tests`
    （ `crates/wxml-backend-tsgo/src/shadow/layout.rs`、
     `crates/wxml-backend-tsgo/src/shadow/rewrite.rs`、
     `crates/wxml-backend-tsgo/src/shadow/locals.rs`、
     `crates/wxml-backend-tsgo/src/input.rs`、
     `crates/wxml-lsp/src/server.rs` ）以及
     `crates/wxml-backend-tsgo/tests/shadow_layout.rs`。
- 未修改： `docs/plans/tsgo-test-ledger.json`、
  `docs/plans/2026-11-17-tsgo-test-ledger.md`、
  `docs/reference/tsgo-test-contract.md`、 `docs/evidence/README.md`、
  `crates/wxml-lsp/tests/native_parity.rs`、
  `crates/wxml-lsp/tests/native_parity/ledger.rs`、
  `crates/wxml-lsp/tests/native_parity/fixture_hash.rs`、
  `crates/wxml-lsp/tests/native_parity/oracle_manifest*`、
  `docs/plans/tsgo-legacy-oracle-manifest.json`、README、CI/scripts。
- 工作树非独占：并行 owner 在本轮期间修改了
  `crates/wxml-backend-tsgo/tests/script_behavior.rs`、
  `crates/wxml-backend-tsgo/tests/shadow_rewrite.rs`、
  `crates/wxml-lsp/tests/native_parity/ledger.rs`、
  `docs/plans/tsgo-test-ledger.json`、台账 Markdown、contract，并新增
  `docs/evidence/2026-11-20-tsgo-ledger-backfill-wave10.md`。
  本轮不引用、不覆盖这些改动；本文件只登记本轮的 9 个 case。
- 未 commit，未启动 review。

**结论：9/9 建议 `exact`，0 blocked。**

基线以写入时实际 JSON 为准（并行 wave-10 weaker 回填已落盘）：
**58 `green` / 245 `planned` / 36 `retired`**，`exact` 58、`weaker` 82、
`assessed_cases` 172、无证据 planned **131**。

本轮回填后建议：**67 `green` / 236 `planned`**，`exact` 67、`weaker` 82、
`assessed_cases` 181、无证据 planned **122**。增量固定为
`+9 green / −9 planned / +9 exact / +9 assessed / −9 no-evidence planned`；
回填前必须重新读取 JSON，不能把本文件的绝对数当成回填时的固定值。

判定口径沿用 wave-8/wave-9：TS oracle 的每条可观察断言都能在同一 Rust 测试里
找到等价断言；typed error 替代 TS 协议错误码时，必须钉死 native 的 `feature`/
`reason`，并给出正向对照，不能从实现输出反推期望。

---

## 1. 逐 case 结论

### 1.1 `TS-PROBES-0048` → exact

TS oracle：`the last pragma in the leading block wins (TypeScript semantics)`
（`backends/typescript/test/probes.test.ts:48`）。

Rust 证据：`case_probes_0048_the_last_pragma_in_the_leading_block_wins`
（ `crates/wxml-backend-tsgo/src/shadow/layout.rs:2579` ）。测试直接调用私有
`scan_pragmas`，与 oracle 使用同一个 `wantPragma=false` 入口。

| TS 断言 | Rust 断言 |
| --- | --- |
| `checkThenNoCheck.check === false` | `!check_then_no_check.check`（layout.rs:2585） |
| `checkThenNoCheck.noCheck === true` | `check_then_no_check.no_check`（layout.rs:2586） |
| `checkThenNoCheck.neutralizeOffset === "// @ts-check\n// @ts-nocheck\n".length` | 偏移等于该字符串 `.len()`（layout.rs:2587-2590），并按 UTF-16 精确为 28（layout.rs:2591-2595） |
| `noCheckThenCheck.check === true` | `no_check_then_check.check`（layout.rs:2599） |
| `noCheckThenCheck.noCheck === false` | `!no_check_then_check.no_check`（layout.rs:2600）；反向 fixture 的 `neutralize_offset` 同样为 28（layout.rs:2601-2608） |

说明：`want_pragma` 在 native `scan_pragmas` 中当前不参与判定
（`let _ = want_pragma;`）；本 case 的 oracle 只使用 `wantPragma=false`，四条断言
均不依赖该参数。policy 为 on 时的 `@ts-check` 注入属于 `TS-PROBES-0027`，不在本
case 重复。

### 1.2 `TS-PROBES-0059` → exact

TS oracle：`` `@ts-nocheck` outside the leading comments is ignored ``
（`backends/typescript/test/probes.test.ts:59`）。

Rust 证据：`case_probes_0059_ts_nocheck_outside_the_leading_comments_is_ignored`
（ `crates/wxml-backend-tsgo/src/shadow/layout.rs:2612` ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `scanPragmas("// ordinary comment\nPage(...)", false).noCheck === false` | `!ordinary.no_check`（layout.rs:2617）；同时 `!ordinary.check`（2616），`neutralize_offset` 精确等于普通注释长度（2618-2622） |
| `scanPragmas("const a = 1;\n// @ts-nocheck\n", false).noCheck === false` | `!after_code.no_check`（layout.rs:2627）；同时 `!after_code.check`（2626），`neutralize_offset == 0`（2628-2631） |

说明：第二个断言钉死“扫描在第一个非注释 token 处停止”，块外 pragma 不会被误读；
`neutralize_offset == 0` 同时排除了“把整个文件当 leading block”的实现。

### 1.3 `TS-ANALYSIS-0196` → exact

TS oracle：`target must be a whole identifier in the expression`
（`backends/typescript/test/analysis.test.ts:196`）。

Rust 证据：`case_analysis_0196_target_must_be_a_whole_identifier_in_the_expression`
（ `crates/wxml-backend-tsgo/src/shadow/rewrite.rs:774` ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `analyze` 对 `fragments[0].target = "nmae"` 抛 `RequestError` | `rewrite_expression("user.name", Some("nmae"), ...)` 返回 `Err(ShadowError::BadExpression { detail })`（rewrite.rs:778-789） |
| `error.code === "bad-fragment"` | typed error 的 `detail` 精确包含 `not a whole identifier`（rewrite.rs:781-788） |
| （同一 "whole identifier" 语义） | 部分 token `"nam"` 与限定串 `"user.name"` 同样被拒绝（rewrite.rs:778） |
| （正向对照，排除“任意 target 都失败”） | `"user"` 解析到 mounted offset 12，`"name"` 解析到 17（rewrite.rs:793-798） |

说明：TS 的 `bad-fragment` 是协议级错误码；native 用更细的 typed
`BadExpression` 替代。这与
 `docs/evidence/2026-11-20-tsgo-near-exact-three.md` §2 的
“typed error replacing a TS error code” 判定一致。正向对照证明根标识符与成员名
都能解析，负例不是“所有 target 被一刀切”。

### 1.4 `TS-ANALYSIS-0349` → exact

TS oracle：`duplicate fragment ids are a structured error`
（`backends/typescript/test/analysis.test.ts:349`）。

Rust 证据：`case_analysis_0349_duplicate_fragment_ids_are_a_structured_error`
（ `crates/wxml-backend-tsgo/src/shadow/layout.rs:2635` ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `analyze` 对两个 `fragmentId: "f0"` 抛 `RequestError` | `build_in_place_shadow` 返回 `Err(ShadowError::Invalid { feature, reason })`（layout.rs:2652-2659） |
| `error.code === "bad-fragment"` | `feature == "fragment.id"`（layout.rs:2655），`reason == "fragment ids must be non-empty and unique"`（layout.rs:2656） |

说明（与计划表的差异，不影响 exact）：计划 §3.1 写的是
`ShadowError::Unsupported { feature: "fragment.id" }`，生产代码实际返回
`ShadowError::Invalid { feature: "fragment.id" }`。TS oracle 只要求“结构化错误”，
没有规定 variant 名；本测试按实际 typed error 钉死 `feature` 与 `reason`，
没有修改生产来迁就预期。评审若要求 `Unsupported`，需要先改生产代码，本轮不做；
当前建议仍是 exact。

### 1.5 `TS-PROTOCOL-0126` → exact

TS oracle：`protocol v2 accepts a same-name nested binding chain`
（`backends/typescript/test/protocol.test.ts:126`）。

Rust 证据：`case_protocol_0126_a_same_name_nested_chain_is_accepted_and_groups_two_levels`
（ `crates/wxml-backend-tsgo/src/shadow/locals.rs:569` ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `validateRequest(request)` 不抛异常 | `group_local_levels(&bindings, DEFAULT_LOCAL_BUDGET)` 返回 `Ok`（locals.rs:578-579） |
| “inner loop may shadow an outer name: the chain encodes nesting, not uniqueness” | 4 个 binding 分成 2 层：外层 `item`/`index`，内层 `item`/`index`，内层 item 的 `source_expr == "item.members"`（locals.rs:580-603） |
| （隐性要求：生成的源码不能有 TDZ） | `render_locals` 生成 `const __wxml_capture_item_0 = item;`，内层声明使用 capture 别名，且不出现 `const item = __wxml_item(item.members)`（locals.rs:607-624） |

说明：`case_locals_0089_*`（ `crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:197` ）
已断言同一渲染结果；本 case 直接落在 `group_local_levels` 的验收入口，覆盖
“同层重名才应拒绝、嵌套同名应接受”的校验语义。

### 1.6 `TS-PROTOCOL-0280` → exact

TS oracle：`unknown compiler options are rejected instead of silently dropped`
（`backends/typescript/test/protocol.test.ts:280`）。

Rust 证据：`case_protocol_0280_unknown_compiler_options_are_rejected_and_strict_switches_pass_through`
（ `crates/wxml-backend-tsgo/src/input.rs:2102` ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `validateRequest(withOption({ allowImportingTsExtensions: true }))` 抛 `unsupported-option` | `CompilerOptions::new` 返回 `Err(InputError::UnsupportedCompilerOption { key })`，`key == "allowImportingTsExtensions"`（input.rs:2105-2112） |
| `validateRequest` 接受 `strictNullChecks: false`、`noUncheckedIndexedAccess: true`、`strictBuiltinIteratorReturn: true` | 三键 `CompilerOptions::new(...)` 返回 `Ok`（input.rs:2115-2120） |
| `toCompilerOptions(...).strictBuiltinIteratorReturn === true` | `options.get("strictBuiltinIteratorReturn") == Some(&Value::Bool(true))`（input.rs:2126-2130）；另两键的值也逐一保留（input.rs:2121-2125） |

说明：native 没有独立的 `toCompilerOptions`，但 `CompilerOptions` 的
`get`/`values` 就是转换后的有效值；逐个值断言等价于 TS 的“新 strict 家族开关
必须透传”，不是只检查“没有报错”。

### 1.7 `TS-PROTOCOL-0319` → exact

TS oracle：`explicit off neutralises a source `@ts-check` in the derived view`
（`backends/typescript/test/protocol.test.ts:319`，scenario `js-source-check-off`）。

Rust 证据：`case_protocol_0319_explicit_off_neutralises_a_source_ts_check_in_the_derived_view`
（ `crates/wxml-backend-tsgo/tests/shadow_layout.rs:567` ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `insertions.find(id === "nocheck")` 存在 | `insertion_id == "nocheck"` 的 insertion 存在（shadow_layout.rs:580-585） |
| `build.notes.some(note.includes("neutralised"))` | `notes.iter().any(note.contains("neutralised"))`（shadow_layout.rs:590-593） |
| `text` 匹配 `/@ts-check[\s\S]*@ts-nocheck/` | 派生文本精确以 `"// @ts-check\n// @ts-nocheck\n"` 开头（shadow_layout.rs:594-601） |
| `build.jsCheckApplied === "off"` | `built.js_check_applied == JsCheckApplied::Off`（shadow_layout.rs:578） |
| （坐标精确性） | insertion `start_utf16 == 13`、`length_utf16 == 15`、`replace_length_utf16 == 0`（shadow_layout.rs:586-588） |

说明：native 的 `neutralize_offset` 是整个 leading pragma block 的末尾；该 fixture
只有一行 `@ts-check`，因此位置 13 与 TS “neutralising pragma 在 source pragma
之后”完全一致。原有 `source_ts_check_is_neutralised_when_the_policy_is_off`
（shadow_layout.rs:550）只断言包含关系；本 case 补了顺序、insertion id、坐标与长度。

### 1.8 `TS-PROTOCOL-0600` → exact

TS oracle：`legacy fragments without target/locals still validate`
（`backends/typescript/test/protocol.test.ts:600`）。

Rust 证据：`case_protocol_0600_legacy_fragments_without_target_or_locals_still_build`
（ `crates/wxml-backend-tsgo/src/shadow/layout.rs:2663` ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `validateRequest({ fragments: legacy })` 不抛异常 | `FragmentSpec::new` 产生 `target == None`、空 `locals`/`bindings`/`guards`（layout.rs:2670-2675），`build_in_place_shadow` 返回 `Ok`（layout.rs:2684） |
| （legacy fragment 仍然参与布局） | 两个 legacy fragment 都挂载：`mounts.len() == 2`，文本分别为 `__wxml_root.title`、`__wxml_root.user.name`（layout.rs:2685-2687） |

说明：TS 仅断言“不抛”，native 额外断言两个 legacy fragment 真的进入
`mounts`，排除“解析成功但被静默丢弃”的伪通过；两条 mount 文本均来自 oracle
同一 `page-basic` 表达式形状。

### 1.9 `TS-SERVER-0150` → exact

TS oracle：`JSON-RPC notifications get no response`
（`backends/typescript/test/server.test.ts:150`）。

Rust 证据：`case_server_0150_json_rpc_notifications_never_produce_a_response`
（ `crates/wxml-lsp/src/server.rs:2564` ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| 三条 notification（unknown method、非法 analyze、合法 analyze）不产生 reply | `handle_incoming(notification(...))` 对 unknown、malformed `didOpen`、valid `didOpen` 均返回空 `Vec<Outgoing>`（server.rs:2571-2598） |
| `client.next()` 的第一条 reply 的 `id === 7`（ping） | 之后的请求经 `only(...)` 只产生一条 `Outgoing::Response`，其 `id == 7`（server.rs:2600-2606） |
| （notification 生命周期） | `exit` notification 仍返回空（server.rs:2607-2611） |

说明：native LSP 没有自定义 `ping` 方法；本 case 断言的 oracle 可观察语义是
“notification 无响应、下一条请求的响应必须是第一条消息”，因此用同一位置上的
`shutdown` 请求（id 7）作为载体，不冒充 native 存在 `ping`。TS 的合法 analyze
notification 在 native 用 valid `textDocument/didOpen` 等价（可调度 diagnostics，
但不产生 response）；本测试不依赖 diagnostics 产物。

---

## 2. 建议 ledger 字段

9 个 case 使用同一组状态字段：

| 字段 | 建议值 |
| --- | --- |
| `status` | `green` |
| `target_rust_test_status` | `migrated_green` |
| `migration_evidence.parity` | `exact` |
| `migration_evidence.green_eligible` | `true` |
| `migration_evidence.status_recommendation` | `green` |
| `migration_evidence.status_applied` | `green` |
| `migration_evidence.assessed_at` | `2026-11-20` |
| `migration_evidence.implementation_head` | 回填时取实际提交；当前工作树基于 `8a56f27edcc9b91264f4b2bd0f3609450b17ab7c` |

`migration_evidence.rust_tests`（`line` 为写入时真实行号）：

| Case | path:line | test |
| --- | --- | --- |
| `TS-PROBES-0048` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2579 | `case_probes_0048_the_last_pragma_in_the_leading_block_wins` |
| `TS-PROBES-0059` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2612 | `case_probes_0059_ts_nocheck_outside_the_leading_comments_is_ignored` |
| `TS-ANALYSIS-0196` |  crates/wxml-backend-tsgo/src/shadow/rewrite.rs:774 | `case_analysis_0196_target_must_be_a_whole_identifier_in_the_expression` |
| `TS-ANALYSIS-0349` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2635 | `case_analysis_0349_duplicate_fragment_ids_are_a_structured_error` |
| `TS-PROTOCOL-0126` |  crates/wxml-backend-tsgo/src/shadow/locals.rs:569 | `case_protocol_0126_a_same_name_nested_chain_is_accepted_and_groups_two_levels` |
| `TS-PROTOCOL-0280` |  crates/wxml-backend-tsgo/src/input.rs:2102 | `case_protocol_0280_unknown_compiler_options_are_rejected_and_strict_switches_pass_through` |
| `TS-PROTOCOL-0319` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:567 | `case_protocol_0319_explicit_off_neutralises_a_source_ts_check_in_the_derived_view` |
| `TS-PROTOCOL-0600` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2663 | `case_protocol_0600_legacy_fragments_without_target_or_locals_still_build` |
| `TS-SERVER-0150` |  crates/wxml-lsp/src/server.rs:2564 | `case_server_0150_json_rpc_notifications_never_produce_a_response` |

`fully_qualified` 建议：

- 单元测试：`wxml_backend_tsgo::shadow::layout::tests::<test>`、
  `wxml_backend_tsgo::shadow::rewrite::tests::<test>`、
  `wxml_backend_tsgo::shadow::locals::tests::<test>`、
  `wxml_backend_tsgo::input::tests::<test>`、
  `wxml_lsp::server::tests::<test>`。
- 集成测试： `crates/wxml-backend-tsgo/tests/shadow_layout.rs::<test>`
  （沿用台账现有 integration 命名）。

建议 `note`（逐 case，英文，供 ledger owner 压缩）：

| Case | 建议 note |
| --- | --- |
| `TS-PROBES-0048` | `Calls the private scan_pragmas with check→nocheck and nocheck→check fixtures; asserts check/no_check and the exact 28 UTF-16-unit neutralize_offset for both orders.` |
| `TS-PROBES-0059` | `Leading comment block only: an ordinary leading comment leaves no_check false with neutralize_offset equal to the comment length, and a later @ts-nocheck after code is not scanned (offset 0).` |
| `TS-ANALYSIS-0196` | `rewrite_expression rejects "nmae", partial "nam" and qualified "user.name" with BadExpression("not a whole identifier"); positive controls resolve "user" at offset 12 and member "name" at 17.` |
| `TS-ANALYSIS-0349` | `Two FragmentSpecs with id "f0" make build_in_place_shadow return the typed Invalid { feature: "fragment.id", reason: "fragment ids must be non-empty and unique" }; the oracle only requires a structured bad-fragment error.` |
| `TS-PROTOCOL-0126` | `group_local_levels accepts item/index/item/index with repeated names as two levels and render_locals captures the outer item (__wxml_capture_item_0), so there is no TDZ; same-level duplicates are a different case.` |
| `TS-PROTOCOL-0280` | `CompilerOptions::new rejects allowImportingTsExtensions with UnsupportedCompilerOption and preserves strictNullChecks=false, noUncheckedIndexedAccess=true and strictBuiltinIteratorReturn=true.` |
| `TS-PROTOCOL-0319` | `Policy off inserts the "nocheck" insertion at UTF-16 13 with length 15, notes "neutralised", and the derived text starts with // @ts-check then // @ts-nocheck before the registration.` |
| `TS-PROTOCOL-0600` | `Legacy FragmentSpec::new fragments (no target/locals/bindings/guards) build successfully and both expressions appear in view.mounts with the __wxml_root prefix.` |
| `TS-SERVER-0150` | `handle_incoming returns no Outgoing for unknown, malformed and valid didOpen notifications; the next request (id 7) is the first and only response, and the exit notification is empty.` |

分区影响（按 `target_rust_test` 路由，只算本轮 9 个）：

| 分区 | 本轮 case | 增量 |
| --- | --- | --- |
| `guards_js` | `PROBES-0048/0059` | green/exact +2，evidence_cases +2，unassessed −2 |
| `hover_docs` | `ANALYSIS-0196` | green/exact +1，evidence_cases +1，unassessed −1 |
| `project_overlay` | `ANALYSIS-0349` | green/exact +1，evidence_cases +1，unassessed −1 |
| `protocol_limits` | `PROTOCOL-0126/0280/0319/0600`、`SERVER-0150` | green/exact +5，evidence_cases +5，unassessed −5 |

`weaker`/`near_exact`/`structural`/`blocked`/`retired` 不变。
validator 常量建议：`GREEN_CASES` 58→67、`PLANNED_CASES` 245→236；
`WEAKER_CASES` 本轮不改（并行 wave-10 backfill 已把它改为 82，回填前重读）；
`TOTAL_CASES` 339、`RETIRED_CASES` 36、`NEAR_EXACT_CASES` 3、
`STRUCTURAL_CASES` 7、`BLOCKED_CASES` 22 不变。
`migration_evidence.assessed_cases` 172→181、`exact` 58→67、
无证据 planned 131→122；`green_eligible`/`promoted_to_green` 58→67。

---

## 3. 验证

在工作树 HEAD `8a56f27` 加并行未提交改动上执行：

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo check -p wxml-backend-tsgo -p wxml-lsp --all-targets` | exit 0 |
| `cargo clippy -p wxml-lsp -p wxml-backend-tsgo --all-targets -- -D warnings` | No issues found |
| `cargo test -p wxml-backend-tsgo --lib` | 359 passed |
| `cargo test -p wxml-backend-tsgo --test shadow_layout` | 34 passed |
| `cargo test -p wxml-lsp --lib` | 947 passed（含 `case_server_0150_*`） |
| `cargo test -p wxml-lsp --test native_parity` | 75 passed / 26 ignored |
| `jq empty docs/plans/tsgo-test-ledger.json` | exit 0（只读检查，未改文件） |
| `git diff --check` | exit 0 |

9 个新测试均随上述命令实际执行；其中：

- `cargo test -p wxml-backend-tsgo --lib` 覆盖 0048/0059/0349/0600、0196、0126、0280；
- `cargo test -p wxml-backend-tsgo --test shadow_layout` 覆盖 0319；
- `cargo test -p wxml-lsp --lib` 覆盖 0150。

`native_parity` 的 75/26 结果包含并行 owner 对 `ledger.rs`/台账 JSON 的
wave-10 weaker 回填；本轮没有修改这些文件。若并行回填继续移动计数，重跑结果
以实际 JSON 为准。

---

## 4. 限制与待裁决

1. **`TS-ANALYSIS-0349` 的 variant 名与计划不同。** 计划写
   `ShadowError::Unsupported`，生产返回 `ShadowError::Invalid`；oracle 只要求
   结构化错误，测试已钉死 `feature`/`reason`。本轮未改生产；若 integrator 只接受
   `Unsupported`，该 case 应降为 blocked，而不是改生产造绿。
2. **typed error 替代协议错误码。** `TS-ANALYSIS-0196`（`bad-fragment`）和
   `TS-ANALYSIS-0349`（`bad-fragment`）都沿用
   `docs/evidence/2026-11-20-tsgo-near-exact-three.md` 的 typed-error 口径；
   若 reviewer 不接受该口径，两者应降为 near_exact/weaker。
3. **`TS-SERVER-0150` 是 dispatch 层证据。** 测试断言
   `Server::handle_incoming(Incoming) -> Vec<Outgoing>`，不启动 stdio 读循环；
   TS oracle 的“第一条消息属于 ping”在 native 用 `shutdown`（id 7）作为请求载体。
   这不影响“notification 无 response”的语义，但不覆盖 JSON-RPC framing 错误；
   `crates/wxml-lsp/tests/lsp.rs` 已有独立的 transport 层通知测试，本 case 不重复。
4. **`TS-PROTOCOL-0126` 的“接受”由 `group_local_levels` 代表。** native 没有
   独立 `validateRequest` DTO；同层重名（TS 的负例）由 `group_local_levels`
   静默丢弃而非报错，属于 `TS-PROTOCOL-0141`（C 类）的范围，不能用本 case 的
   正例掩盖。
5. 工作树非独占，并行 owner 仍在改台账/contract/weaker 证据；本文件的绝对计数
   只对写入时的 JSON 有效，回填前必须重读。
6. 未 commit、未做独立 review；9 个 `exact` 仍需主线程按 ledger 流程复核。
