# P7 删除旧 Node gate 的语义准备（2026-11-20，HEAD `16e88b2`）

## 0. 结论先行

1. `crates/wxml-lsp/tests/native_parity/native_gates.rs` 已支持可选的
   `old_test_removed: bool`（缺省 `false`）。旧 `#[ignore]` 测试消失时不再先红；
   但只有在 gate 仍为 `gate_green`、replacement 仍有链接、legacy oracle manifest
   已处于 Frozen、且该 gate 的全部 `related_ledger_cases` 都能在冻结 manifest 中
   找到时，才允许标记 removed。
2. 当前 23 个 gate **一个都没有标记 removed**，23 个旧 `#[ignore]` 测试也都还在。
   把仍在树里的 gate 标成 `old_test_removed: true` 会硬失败，而不是被当成通过。
3. `semantic_tools` 的两个 silent return 和可选 `tsc` 分支保持默认 Node 行为不变；
   `WXML_TEST_NATIVE=1` 或 `WXML_REQUIRE_REAL_ENGINE=1` 下缺少 native
   package/engine 时直接 panic。该 guard 由 `899a89a` 引入，本切片补充了源码级
   双 flag 校验，防止其中一个 flag 被静默移除。
4. `symbols_edits` 的 `BLOCKED_NATIVE_GATES` 仍为空：两个 silent return + `tsc`
   分支已由三个 native edit gate 明确覆盖（`COVERED_OLD_SURFACES`）。旧测试本身
   仍保留，ledger 状态仍是 `required_not_migrated`。
5. **删除仍未放行。** 23 个 gate 都有 replacement 和 opt-in `gate_green` 证据，
   但 live legacy TypeScript 树仍在（oracle 不是 Frozen），ledger 也没有
   `mandatory_green`/`replacement_reviewed`。本切片只做语义准备，不删除任何
   生产、delivery、默认 backend、Node 路径或旧测试。

---

## 1. 边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`，
  任务基线 HEAD `16e88b2`（`test(p7): freeze legacy deletion evidence`）。
  验证期间共享工作树因并行提交推进到 `fb6047e`；本次改动路径（两个测试文件和
  本文）未被这些并行提交修改。
- 只改测试、台账语义准备、证据和文档：
  - `crates/wxml-lsp/tests/native_parity/native_gates.rs`
  - `crates/wxml-lsp/tests/native_parity/symbols_edits.rs`
  - 本文
- 不改生产代码、`wxml-delivery`、默认 backend、Node 删除、README、
  `docs/plans/tsgo-test-ledger.json`、`docs/plans/tsgo-legacy-oracle-manifest.json`
  和 `tsgo-test-ledger.md`。未 commit。

---

## 2. `old_test_removed` schema

### 2.1 字段

`ledger.ignored_rust_tests[]` 的每个 gate 可带：

| 字段 | 类型 | 缺省 | 含义 |
| --- | --- | --- | --- |
| `old_test_removed` | `bool` | `false` | 旧 `#[ignore]` 测试是否已从 Rust 测试树删除 |

任何非布尔值（`"true"`、`1`、`null`）都按 schema 错误硬失败，避免“truthy 字符串
把 gate 静默删除”。

### 2.2 校验规则

`validate_old_test_removals(...)` 对每个 gate 执行：

- `false`/缺省：`file::test` 必须仍出现在 `#[ignore]` 清单中；测试消失但没有显式
  标记 removed 会失败，防止静默丢 gate。
- `true`：
  1. `status` 必须是 `gate_green`；`required_not_migrated` 不得删除。
  2. `file::test` 不得再带 ignore attribute；仍存在则失败。
  3. gate id 必须出现在 `GATE_REPLACEMENTS` 的 native replacement 链接中。
  4. legacy oracle gate 必须报告 `LegacySourceMode::Frozen`，即 live
     `backends/typescript/test` 已删除、manifest 是唯一权威。
  5. 每个 `related_ledger_cases` 都必须存在于冻结 manifest 的 `cases[]` 中。
- 删除后的 `#[ignore]` 总数按 `23 - removed` 计算；native gate 和 native edit gate
  的 23 + 3 个 `#[ignore]` 仍必须全部存在。

`native_replacements_are_linked_with_gate_green_evidence` 和
`native_gate_tests_are_declared_ignored_and_fail_closed` 继续独立校验 replacement，
因此旧测试消失后 replacement 仍会被检查，不会出现“旧测试没了、替代也没跑”的空洞。

### 2.3 新增 fail-closed 测试

`cargo test -p wxml-lsp --test native_parity` 新增 7 个测试：

| 测试 | 覆盖 |
| --- | --- |
| `old_test_removed_defaults_to_false_and_rejects_a_vanished_gate` | 缺省 false + 测试消失 |
| `old_test_removed_rejects_a_gate_that_is_still_ignored` | 模拟“未删除却标 removed” |
| `old_test_removed_requires_gate_green_and_a_linked_replacement` | 非 green / 漏 replacement |
| `old_test_removed_requires_the_frozen_oracle_manifest` | Live oracle / 漏 frozen case |
| `old_test_removed_rejects_non_boolean_tamper` | 字符串/数字/null tamper |
| `old_test_removed_accepts_a_simulated_deletion_with_frozen_evidence` | 合法模拟删除 |
| `native_edit_guard_honors_both_opt_in_flags` | 双 flag guard 未被删减 |

这些测试使用内存中的合成 gate 和合成 manifest case，不删除、不改写真实树。

---

## 3. `semantic_tools` fail-closed 语义

默认 Node 环境的行为保持不变：

- `backends/typescript/dist/server.js` 缺失时仍打印
  `skipping real Node LSP test: build backends/typescript first` 并 return；
- `node_modules/typescript/bin/tsc` 缺失时仍打印
  `tsc not installed; skipped post-edit type check`。

请求 native edit gate 时（`WXML_TEST_NATIVE=1` **或**
`WXML_REQUIRE_REAL_ENGINE=1`）：

- `common::native_edit::requested()` 为 true；
- 两个 real Node 测试在读取旧资产前调用
  `common::native_edit::require("G-P5-E2E-22"/"G-P5-E2E-23")`；
- 没有 `WXML_TEST_NATIVE_PACKAGE` / `TSGO_ASSET_ROOT` / `TSGO_BIN` 时 panic；
- 即使 native package 存在，旧 `dist/server.js` 缺失或 `tsc` 缺失也分别 panic，
  不再静默返回或跳过。

本切片在 `native_gates.rs` 中新增源码扫描断言：两个旧测试必须包含
`common::native_edit::requested()` 和对应 gate id 的 `require(...)`；guard 必须同时
读取 `WXML_TEST_NATIVE` 和 `WXML_REQUIRE_REAL_ENGINE`，只接受 `1`/`true`，且无
package/engine 时必须 `panic!`。真实通过路径未改动。

---

## 4. `symbols_edits` 覆盖与 BLOCKED

| 旧 silent surface | 覆盖它的 native gate |
| --- | --- |
| `G-P5-E2E-22` `dist/server.js` 缺失时 silent return | `native_component_property_edit_applies_and_reparses` |
| `G-P5-E2E-22` 可选 `tsc` 分支 | `assert_native_edit_reparses_and_typechecks` |
| `G-P5-E2E-23` `dist/server.js` 缺失时 silent return | `native_rejects_cross_namespace_and_dynamic_edits` |

`COVERED_OLD_SURFACES` 明确记录该映射；`native_gates.rs` 断言它必须与 ledger 的
`replacement_test` + `tsc_replacement` 集合完全一致（3 个）。

`BLOCKED_NATIVE_GATES` 仍为空：没有“尚未编写的 replacement”。但旧测试的删除仍是
BLOCKED，原因不在 replacement 缺失，而在 ledger 仍记录
`required_not_migrated`，且要求 required native job 记录绿色证据并完成替换审查。

---

## 5. 删除就绪矩阵

| 组 | 数量 | replacement / 证据 | `old_test_removed` | 结论 |
| --- | ---: | --- | --- | --- |
| `G-P5-E2E-01..05, 15..21`（`p7_action=delete`） | 12 | 23/23 `gate_green` + linked native replacement | 全部 false | **唯一删除候选，但当前不可删** |
| `G-P5-E2E-06..14, PERF-01, STABILITY-01`（`p7_action=retarget`） | 11 | 同上 | 全部 false | **不做删除，保持 retarget** |
| `G-P5-E2E-22` 两个 silent surface + `G-P5-E2E-23` | 2 个 return + 1 个 tsc 分支 | 3 个 native edit gate 已编写 | 不适用 | **BLOCKED（旧测试保留）** |

逐项判断：

- **替代覆盖：23/23。** 23 个旧 `#[ignore]` gate 都有 `gate_green` 证据和
  `GATE_REPLACEMENTS` 链接。
- **实际可删除：0/23。** 所有 gate 的 `old_test_removed` 都是 false，旧测试都还在；
  live `backends/typescript/test` 仍在，oracle 是 Live 而不是 Frozen；ledger 也没有
  `mandatory_green`/`replacement_reviewed`。
- **12 个 `delete` gate** 是 schema 层面仅有的删除候选；它们要等 P7-1 的
  `mandatory_green` + review，以及 legacy oracle 进入 Frozen 后，才能逐个把
  `old_test_removed` 改为 true。
- **11 个 `retarget` gate** 的 P7 动作是把旧测试重定向到 native，不是删除，不能借
  `old_test_removed` 绕过 retarget 工作。
- **2 个 silent return + 1 个 `tsc` 分支** 仍 BLOCKED：旧测试保留，
  ledger 状态 `required_not_migrated`；`899a89a` 只保证 native gate 请求模式下
  fail-closed，默认 Node 模式仍是原来的 skip 行为。

---

## 6. 验证

在共享工作树执行（任务基线 `16e88b2`；执行时 HEAD 已因并行提交推进到
`fb6047e`，两个测试文件仍是本文描述的版本）：

| 命令 | 结果 |
| --- | --- |
| `cargo test -p wxml-lsp --test native_parity` | `75 passed, 26 ignored` |
| `cargo test -p wxml-lsp --test semantic_tools` | `41 passed` |
| `env -u WXML_TEST_NATIVE_PACKAGE -u TSGO_ASSET_ROOT -u TSGO_BIN WXML_TEST_NATIVE=1 cargo test -p wxml-lsp --test semantic_tools -- real_node_lsp` | `0 passed; 2 failed`（exit 101，两个 gate 都在 `native_edit.rs:74` panic） |
| `env -u WXML_TEST_NATIVE -u WXML_TEST_NATIVE_PACKAGE -u TSGO_ASSET_ROOT -u TSGO_BIN WXML_REQUIRE_REAL_ENGINE=1 cargo test -p wxml-lsp --test semantic_tools -- real_node_lsp` | `0 passed; 2 failed`（exit 101） |
| `cargo clippy -p wxml-lsp --all-targets -- -D warnings` | 通过，无 issue |
| `cargo fmt --check`（仅本次改动的两个测试文件） | 通过 |
| `git diff --check` | 通过 |

默认 `semantic_tools` 的 41 个测试包含两个 real Node LSP 测试；本工作树
`backends/typescript/dist/server.js` 和 `node_modules/typescript/bin/tsc` 都存在，
因此默认路径真实通过，不是 skip。

---

## 7. 明确不声称

- 不声称 23 个旧 gate 已删除或已 `mandatory_green`；它们仍是 `#[ignore]` +
  `gate_green` opt-in 证据。
- 不声称 2 个 silent return 或 `tsc` 分支已删除；它们仍在默认 Node 路径上。
- 不声称 legacy TypeScript 树、`backend_node`、v1 写路径、默认 backend 或 Node
  依赖已删除。
- 不声称 P7-1 台账归零、P7-2 oracle 已 Frozen 或 P7-4/P7-5 已完成。
- 不声称 `old_test_removed` 已被任何真实 gate 使用；本切片只提供 schema、校验和
  fail-closed 模拟测试。

---

## 8. 增补：`p7_action` 删除 guard（2026-11-20，HEAD `5cc1255`）

本节补上第 2 节当时的缺口：`old_test_removed` 只校验 `gate_green`、linked
replacement、Frozen oracle 和 related cases；11 个 `p7_action=retarget` gate
同样满足这些条件，误删并标记 `old_test_removed: true` 会通过。ledger 没有
`p7_action` 字段，因此 guard 从冻结证据派生分类。

### 8.1 实现

- `crates/wxml-lsp/tests/native_parity/native_gates.rs` 新增
  `load_p7_actions()`，只读加载
  `docs/evidence/data/legacy-p7-freeze.json` 的
  `old_ignored_gates.gates[].p7_action`。缺失 `gate_id`、未知 action、重复
  gate 都是硬失败，**不会默认成 `delete`**。
- 受控常量 `P7_DELETE_GATE_IDS`（12 个）和 `P7_RETARGET_GATE_IDS`（11 个）
  与 freeze JSON 逐项相等；主测试同时断言 action map 恰好覆盖 ledger 的 23 个
  `gate_id`，且 `12 + 11 = 23`。分类来自现有 freeze 证据，没有新增或改动任何
  gate 的 delete/retarget 归属。
- `validate_old_test_removals(...)` 新增 `p7_actions` 参数：当
  `old_test_removed=true` 时，除原有 `gate_green` 检查外，必须先满足
  `p7_action == "delete"`；`retarget` gate 直接失败，不会进入后续
  replacement/Frozen 检查。
- ledger、freeze JSON、`scripts/p7-freeze.sh`、生产代码、README 均未修改。
  23 个 gate 当前仍是 `old_test_removed=false`，真实树状态不变。

错误文本（`retarget` 标记 removed）：

```text
G-P5-E2E-06 is marked old_test_removed but its p7_action is "retarget"; only
p7_action=delete gates may leave the old test tree; retarget gates must keep the
old #[ignore] test
```

缺少 action 记录时：

```text
<gate_id> is marked old_test_removed but has no recorded p7_action in
docs/evidence/data/legacy-p7-freeze.json
```

### 8.2 测试

保留原有 6 个 `old_test_removed_*` 正向/负向测试（现在显式传入合成
`p7_action` map，错误断言不变），并新增
`retarget_cannot_be_marked_removed`：

- 合成 `p7_action=retarget` gate 标记 removed：断言错误包含
  `p7_action is "retarget"` 和 `only p7_action=delete gates may leave the old
  test tree`；
- 没有 action 记录的 gate：断言 `has no recorded p7_action`；
- 真实 freeze 证据仍把 `G-P5-E2E-06` 分类为 `retarget`、`G-P5-E2E-01`
  分类为 `delete`，防止常量与证据漂移。

### 8.3 验证

| 命令 | 结果 |
| --- | --- |
| `cargo test -p wxml-lsp --test native_parity` | `76 passed, 26 ignored`（新增 1 个测试） |
| `cargo fmt --check -- crates/wxml-lsp/tests/native_parity/native_gates.rs` | 通过 |
| `cargo clippy -p wxml-lsp --test native_parity -- -D warnings` | 通过 |
| `git diff --check` | 通过 |

`cargo clippy -p wxml-lsp --all-targets -- -D warnings` 当时还会报
`crates/wxml-lsp/src/backend_tsgo/tests.rs` 的 3 个未提交并行改动告警
（`needless_raw_string_hashes` ×2、`doc_markdown` ×1）；该文件不属于本切片，
本切片涉及的 `native_parity` target clippy 无 issue。

### 8.4 明确不声称

- 不声称任何 gate 已删除或 `old_test_removed` 已置 true；本补丁只收紧删除条件。
- 不声称 freeze JSON 的 `p7_action` 分类一定长期正确；后续若要调整分类，必须
  同时改 freeze 证据、受控常量和评审记录，否则测试会失败。
- 不声称 `p7-freeze.sh --check` 或 P7-1/P7-3/P7-4 已完成。
