# tsgo events / locals / guards exact 补断言（2026-11-20）

## 0. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 验证基线 HEAD：`807a1578173992ad16f15e22b45f8dd62b024df3`
- 记录日期：2026-11-20
- 范围：只改 Rust 测试和本证据文档：
  - `crates/wxml-backend-tsgo/tests/script_events.rs`
  - `crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs`
  - 本文件
- 未修改生产代码、`Cargo.toml`、`docs/plans/tsgo-test-ledger.json`、Markdown
  台账、contract、`crates/wxml-lsp/tests/native_parity/ledger.rs`、README 或
  Node backend；未提交。
- 记录期间工作树仍有并行会话的未提交改动（ledger/fixture-hash/oracle-manifest
  等）。本文件只引用本轮验证过的两个测试文件，不把它们之外的未提交内容当作
  本 case 证据。
- `gate_green` 与 case `green` 仍是两个状态轴；本文只建议 case 级变更。

**结论：wave-7 列出的 7 个“只差少量断言”case 全部达到 exact，0 blocked。**
建议主线程回填为 `green`：`green`/`exact` 48 → 55、`planned` 255 → 248、
`weaker` 80 → 73；`assessed_cases` 160、无证据 planned 143、retired 36 不变。

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

缺口（wave-7 记录）：只有 “absent definition” 未断言。本轮补上后四条断言齐全。

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

缺口（wave-7 记录）：四条结果的 Supported status 未断言。本轮补齐。

### 1.3 `TS-EVENTS-0461` → exact

TS oracle：`a component method overrides the same-named behavior method`
（`backends/typescript/test/events.test.ts:461`）。

Rust 证据：`case_events_0461_component_method_overrides_the_same_named_behavior_method`
（ crates/wxml-backend-tsgo/tests/script_events.rs:505 ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `state.status === "supported"` | `resolution.status.is_supported()`（script_events.rs:526，**本轮新增**） |
| `definition.fileId === SCRIPT_PATH` | `definition.path == "index.ts"`；fixture 的 request script path 即 `index.ts`（script_events.rs:527-528） |
| `signature` 匹配 `/detail: number/` | `signature.contains("detail: number")`（script_events.rs:530-535） |
| `documentation === "页面自己的实现"` | `documentation == Some("页面自己的实现")`（script_events.rs:529） |

缺口（wave-7 记录）：Supported status 未断言。本轮补齐。

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

绑定名、role、sourceExpr、indent 与 TS fixture 一致。

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

本轮把两条 `contains` 断言替换为 oracle 的完整文本断言（更严格），并补 close；
没有放宽既有断言。

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

缺口（wave-7 记录）：`levels[0].index.name` 未断言。本轮补齐。

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

## 2. 验证

在本工作树、上述 HEAD 加本轮未提交测试改动上执行：

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo check -p wxml-backend-tsgo -p wxml-lsp --all-targets` | exit 0 |
| `cargo clippy -p wxml-backend-tsgo -p wxml-lsp --all-targets -- -D warnings` | no issues |
| `cargo test -p wxml-backend-tsgo --test shadow_locals_guards --test shadow_layout` | 54 passed / 0 failed |
| `cargo test -p wxml-backend-tsgo --test script_events` | 26 passed / 0 failed |
| `cargo test -p wxml-lsp --lib event` | 134 passed / 0 failed / 810 filtered |
| `cargo test -p wxml-lsp --test native_parity` | 68 passed / 0 failed / 26 ignored |
| `git diff --check` | exit 0 |

本轮新增/收紧断言都在真实 `render_locals`、`render_guards`、`resolve_events`
管线上运行；events 使用与 oracle 相同的 Page/Component 脚本和 alias behavior
fixture，locals/guards 使用与 oracle 相同的绑定、缩进和 condition 文本。

---

## 3. 建议 ledger 变更（由主线程统一回填）

### 3.1 逐 case

对 `TS-EVENTS-0301`、`TS-EVENTS-0322`、`TS-EVENTS-0461`、`TS-LOCALS-0063`、
`TS-LOCALS-0089`、`TS-LOCALS-0144`、`TS-GUARDS-0075`：

| 字段 | 变更 |
| --- | --- |
| `status` | `planned` → `green` |
| `migration_evidence.parity` | `weaker` → `exact` |
| `migration_evidence.green_eligible` | `false` → `true` |
| `migration_evidence.status_recommendation` | `planned` → `green` |
| `migration_evidence.status_applied` | `planned` → `green` |
| `target_rust_test_status` | `proposed` → `migrated_green` |
| `migration_evidence.rust_tests[*].line` | 更新为本轮行号（见 §1） |
| `migration_evidence.note` | 改为记录补齐的断言，不再列缺口 |

`migration_evidence.implementation_head` 应在主线程提交本轮测试后指向该提交。

### 3.2 计数

| 项 | 现在 | 建议 |
| --- | ---: | ---: |
| `green` | 48 | 55 |
| `planned` | 255 | 248 |
| `exact` | 48 | 55 |
| `weaker` | 80 | 73 |
| `assessed_cases` | 160 | 160 |
| 无证据 planned | 143 | 143 |
| `retired-with-decision` | 36 | 36 |

validator 常量：`GREEN_CASES` 48 → 55、`PLANNED_CASES` 255 → 248、
`WEAKER_CASES` 80 → 73；`NEAR_EXACT_CASES` 3、`STRUCTURAL_CASES` 7、
`BLOCKED_CASES` 22、`RETIRED_CASES` 36 不变。

分区：

- `events`：green/exact 10 → 13、planned_evidence 11 → 8、weaker 10 → 7。
- `loops`：green/exact 5 → 8、planned_evidence 4 → 1、weaker 3 → 0。
- `guards_js`：green/exact 5 → 6、planned_evidence 8 → 7、weaker 3 → 2。

Markdown 台账、`docs/reference/tsgo-test-contract.md` 和
`docs/evidence/README.md` 的证据索引需要按上述计数同步；本轮未改它们。

---

## 4. 限制

1. 本轮只加断言，没有新增生产路径；所有 exact 结论都来自现有实现的真实输出。
2. `TS-EVENTS-0461` 的 fileId 断言使用 fixture 的相对脚本路径 `index.ts`；
   TS 的 `SCRIPT_PATH` 是 `pages/index/index.ts`。二者都表示“definition 位于
   请求脚本自身”，没有跨文件路径歧义。
3. 7 个 case 的 Rust 证据仍在 `wxml-backend-tsgo` 集成测试，而不是
   `target_rust_test` 指向的 `native_parity/*.rs`；这与既有 green case 的
   路由方式一致，不改变 target 路径。
4. 本轮未做独立 review；建议的 `green` 仍处于 `reviewed` 之前的状态。
