# tsgo 测试台账 wave-8 断言补齐回填（2026-11-20）

## 0. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 验证基线 HEAD：`807a1578173992ad16f15e22b45f8dd62b024df3`
- 记录期间并行会话提交了 `036855f`（delivery real legacy release migration）；
  本轮验证与结论不依赖该提交，7 个 case 的断言改动仍在其之上的工作树中、未提交。
- 记录日期：2026-11-20
- 本轮测试改动（仍未提交）：
  - `crates/wxml-backend-tsgo/tests/script_events.rs`
  - `crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs`
- 本轮台账/文档改动：
  - `docs/plans/tsgo-test-ledger.json`
  - `docs/plans/2026-11-17-tsgo-test-ledger.md`
  - `docs/reference/tsgo-test-contract.md`
  - `crates/wxml-lsp/tests/native_parity/ledger.rs` 的计数常量
  - 本文件
- 未修改生产代码、`Cargo.toml`、delivery、Node backend 或默认后端；未提交、
  未启动 review。
- 工作树已有并行会话的未提交 oracle manifest + wave-7 内容
  （`oracle_manifest*.rs`、`fixture_hash.rs`、`native_parity.rs`、
  `docs/plans/tsgo-legacy-oracle-manifest.json`、台账 wave-7 证据等）。
  本轮保留其全部内容：`oracle_manifest::*`、live/frozen 逻辑、wave-7
  `migration_evidence` 和 README 均未覆盖。

**结论：wave-7 逐条列出的 7 个“只差少量断言”case 全部达到 exact，0 blocked。**
`green`/`exact` 48 → 55、`planned` 255 → 248、`weaker` 80 → 73；
`assessed_cases` 160、无证据 planned 143、`near_exact` 3、`structural` 7、
`blocked` 22、`retired` 36 不变。

判定口径：TS oracle 的每条可观察断言都能在同一 Rust 测试里找到等价断言；
新增断言全部来自 oracle 原文，没有从实现输出反推期望。

---

## 1. 逐 case 结论

### 1.1 `TS-EVENTS-0301` → exact

TS oracle：`a component without methods reports a real missing handler`
（`backends/typescript/test/events.test.ts:301`）。

Rust 证据：`case_events_0301_component_without_methods_reports_a_real_missing_handler`
（ crates/wxml-backend-tsgo/tests/script_events.rs:305 ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `state.status === "supported"` | `resolution.status.is_supported()`（script_events.rs:316） |
| `definition === undefined` | `resolution.definition.is_none()`（script_events.rs:317，**本轮新增**） |
| `diagnostic.severity === "error"` | `diagnostic.severity == Some("error")`（script_events.rs:318-324） |
| `diagnostic.message` 匹配 `/component/` | `message.contains("component")`（script_events.rs:325-330） |

### 1.2 `TS-EVENTS-0322` → exact

TS oracle：`reserved page keys and __wxml members are ignored`
（`backends/typescript/test/events.test.ts:322`）。

Rust 证据：`case_events_0322_reserved_page_keys_and_probe_members_are_ignored`
（ crates/wxml-backend-tsgo/tests/script_events.rs:363 ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| 前三条结果 `state.status === "supported"` | 循环内 `resolution.status.is_supported()`（script_events.rs:379-380，**本轮新增**） |
| 前三条结果 `diagnostic.severity === "error"` | `severity == Some("error")`（script_events.rs:381-387） |
| 第四条结果 `state.status === "supported"` | `collection.results[3].status.is_supported()`（script_events.rs:389，**本轮新增**） |
| 第四条结果 `definition` 存在 | `collection.results[3].definition.is_some()`（script_events.rs:390） |

### 1.3 `TS-EVENTS-0461` → exact

TS oracle：`a component method overrides the same-named behavior method`
（`backends/typescript/test/events.test.ts:461`）。

Rust 证据：`case_events_0461_component_method_overrides_the_same_named_behavior_method`
（ crates/wxml-backend-tsgo/tests/script_events.rs:505 ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `state.status === "supported"` | `resolution.status.is_supported()`（script_events.rs:526，**本轮新增**） |
| `definition.fileId === SCRIPT_PATH` | `definition.path == "index.ts"`；fixture 的 request script path 即 `index.ts`（script_events.rs:527-528） |
| `documentation === "页面自己的实现"` | `documentation == Some("页面自己的实现")`（script_events.rs:529） |
| `signature` 匹配 `/detail: number/` | `signature.contains("detail: number")`（script_events.rs:530-535） |

### 1.4 `TS-LOCALS-0063` → exact

TS oracle：`nested levels indent, and closers are innermost first`
（`backends/typescript/test/locals.test.ts:63`）。

Rust 证据：`case_locals_0063_nested_levels_indent_and_closers_are_innermost_first`
（ crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:170 ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `rendered.unresolved === false` | `!rendered.unresolved`（shadow_locals_guards.rs:180，**本轮新增**） |
| `rendered.text` 精确等于两层的 `join("\n")` | 同文本逐字 `assert_eq!`（shadow_locals_guards.rs:181-192） |
| `rendered.close === "    }\n  }\n"` | 同 close（shadow_locals_guards.rs:193） |

### 1.5 `TS-LOCALS-0089` → exact

TS oracle：`a same-name nested chain captures the outer binding (no TDZ self-reference)`
（`backends/typescript/test/locals.test.ts:89`）。

Rust 证据：`case_locals_0089_a_same_name_nested_chain_captures_the_outer_binding_no_tdz`
（ crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:197 ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `rendered.unresolved === false` | `!rendered.unresolved`（shadow_locals_guards.rs:208） |
| `rendered.text` 精确等于含 `__wxml_capture_item_0` 的完整文本 | 同文本逐字 `assert_eq!`（shadow_locals_guards.rs:209-222，**本轮由 contains 收紧**） |
| `rendered.close === "  }\n}\n"` | 同 close（shadow_locals_guards.rs:223，**本轮新增**） |
| 内层声明不得读取自己声明的名字 | 负向 `contains`（shadow_locals_guards.rs:224-228） |

### 1.6 `TS-LOCALS-0144` → exact

TS oracle：`levels are grouped by array order, not by source expression`
（`backends/typescript/test/locals.test.ts:144`）。

Rust 证据：`case_locals_0144_levels_are_grouped_by_array_order_not_source_expression`
（ crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:274 ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `levels.length === 2` | `levels.len() == 2`（shadow_locals_guards.rs:285） |
| `levels[0].item.name === "item"` | 同（shadow_locals_guards.rs:286-289） |
| `levels[0].index.name === "index"` | 同（shadow_locals_guards.rs:290-293，**本轮新增**） |
| `levels[1].item.name === "member"` | 同（shadow_locals_guards.rs:294-297） |

### 1.7 `TS-GUARDS-0075` → exact

TS oracle：`plain value-level || guards are not degraded`
（`backends/typescript/test/guards.test.ts:75`）。

Rust 证据：`case_guards_0075_plain_value_level_or_guards_are_not_degraded`
（ crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:394 ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `open === "if (flagA || flagB) {\n"` | 同（shadow_locals_guards.rs:403） |
| `close === "}\n"` | 同（shadow_locals_guards.rs:404，**本轮新增**） |
| `unresolved === false` | `!rendered.unresolved`（shadow_locals_guards.rs:405） |

`RenderedGuards` 只有 `open`/`close`/`unresolved` 三个字段，逐字段断言等价于
TS 的 `deepEqual`。

---

## 2. 台账字段回填

对 7 个 case，`cases[]` 的最终字段：

| 字段 | 值 |
| --- | --- |
| `status` | `green` |
| `target_rust_test_status` | `migrated_green` |
| `migration_evidence.parity` | `exact` |
| `migration_evidence.green_eligible` | `true` |
| `migration_evidence.status_recommendation` | `green` |
| `migration_evidence.status_applied` | `green` |
| `migration_evidence.assessed_at` | `2026-11-20` |
| `migration_evidence.implementation_head` | `807a1578173992ad16f15e22b45f8dd62b024df3` |
| `migration_evidence.rust_tests[*].line` | 0301=305、0322=363、0461=505、0063=170、0089=197、0144=274、0075=394 |

`migration_evidence.note` 和各 `rust_tests[].note` 改为记录补齐后的完整覆盖，
不再列缺口。`TS-EVENTS-0301` 保留既有 `event_diagnostics.rs` 单元测试证据。

计数（`counts.statuses` 与 `counts.migration_evidence` 同步）：

| 项 | wave-7 后 | wave-8 后 |
| --- | ---: | ---: |
| `green` | 48 | 55 |
| `planned` | 255 | 248 |
| `retired-with-decision` | 36 | 36 |
| `exact` | 48 | 55 |
| `near_exact` | 3 | 3 |
| `weaker` | 80 | 73 |
| `structural` | 7 | 7 |
| `blocked` | 22 | 22 |
| `assessed_cases` | 160 | 160 |
| `green_eligible` | 48 | 55 |
| `promoted_to_green` | 48 | 55 |
| 无 Rust 证据的 planned | 143 | 143 |
| 339 总数 | 339 | 339 |

分区变化：

- `events`：green/exact 10→13、planned 有证据 11→8、weaker 10→7。
- `loops`：green/exact 5→8、planned 有证据 4→1、weaker 3→0。
- `guards_js`：green/exact 5→6、planned 有证据 8→7、weaker 3→2。
- 其余分区不变。

validator 常量：`GREEN_CASES` 48→55、`PLANNED_CASES` 255→248、
`WEAKER_CASES` 80→73；`TOTAL_CASES` 339、`RETIRED_CASES` 36、
`NEAR_EXACT_CASES` 3、`STRUCTURAL_CASES` 7、`BLOCKED_CASES` 22 不变。
`oracle_manifest::*`、live/frozen 校验、`fixture_hash.rs`、`native_parity.rs`
和 manifest JSON 未改动。

---

## 3. 验证

在本工作树、HEAD `807a157` 加本轮未提交改动上执行：

| 命令 | 结果 |
| --- | --- |
| `jq empty docs/plans/tsgo-test-ledger.json` | exit 0 |
| `cargo test -p wxml-lsp --test native_parity` | 68 passed / 0 failed / 26 ignored |
| `cargo fmt --all -- --check` | exit 0 |
| `git diff --check` | exit 0 |

7 个测试同时在 `wxml-backend-tsgo` 集成套件中运行：

| 命令 | 结果 |
| --- | --- |
| `cargo test -p wxml-backend-tsgo --test script_events` | 26 passed / 0 failed |
| `cargo test -p wxml-backend-tsgo --test shadow_locals_guards` | 21 passed / 0 failed |

断言在真实 `resolve_events`、`render_locals`、`render_guards`、`group_local_levels`
管线上运行；events 使用与 oracle 相同的 Page/Component 脚本和 alias behavior
fixture，locals/guards 使用与 oracle 相同的绑定、缩进和 condition 文本。

---

## 4. 限制

1. 本轮只加断言，没有新增生产路径；所有 exact 结论都来自现有实现的真实输出。
2. `TS-EVENTS-0461` 的 fileId 断言使用 fixture 的相对脚本路径 `index.ts`；
   TS 的 `SCRIPT_PATH` 是 `pages/index/index.ts`。二者都表示“definition 位于
   请求脚本自身”，没有跨文件路径歧义。
3. 7 个 case 的 Rust 证据仍在 `wxml-backend-tsgo` 集成测试，而不是
   `target_rust_test` 指向的 `native_parity/*.rs`；这与既有 green case 的
   路由方式一致，`target_rust_test` 路径未改。
4. `implementation_head` 记录验证时的 HEAD；断言改动仍未提交，等主线程提交后
   可在同一次台账更新中指向该提交。
5. 未做独立 review；7 个 `green` 仍处于 `reviewed` 之前的状态。
6. README 证据索引由并行会话在统一时补行，本轮未改。
