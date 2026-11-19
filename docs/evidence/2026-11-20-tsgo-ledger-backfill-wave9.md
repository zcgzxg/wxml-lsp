# tsgo 测试台账 wave-9 回填（2026-11-20）

## 0. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 验证 HEAD：`16e88b24917a34e225a4ec66b6ea0e0751262dc4`（`test(p7): freeze legacy deletion evidence`）
- 记录日期：2026-11-20
- 输入：
  - `docs/evidence/2026-11-20-tsgo-wave7-weaker-exact.md`（wave-7 的 6 个 weaker/blocked 逐条判定）
  - 工作树中已存在、未提交的 3 个 case 测试改动：
    - `crates/wxml-backend-tsgo/src/shadow/layout.rs`（仅 `#[cfg(test)] mod tests`，新增测试辅助与 `case_probes_0027_*`）
    - `crates/wxml-backend-tsgo/tests/script_properties.rs`（新增 `case_analysis_0139_*`）
    - `crates/wxml-lsp/src/backend_tsgo/tests.rs`（新增 0139 hover 与 0174）
- 本轮台账/文档改动：
  - `docs/plans/tsgo-test-ledger.json`
  - `docs/plans/2026-11-17-tsgo-test-ledger.md`
  - `docs/reference/tsgo-test-contract.md`
  - `crates/wxml-lsp/tests/native_parity/ledger.rs` 的计数常量
  - 本文件（新增）
- 未修改生产代码、`Cargo.toml`、delivery、Node backend、默认后端、测试断言本身、
  `fixture_hash.rs`、`docs/plans/tsgo-legacy-oracle-manifest.json`、`oracle_manifest::*`
  live/frozen 逻辑、wave-7/wave-8 证据或 README；未提交、未启动 review。
- 工作树非独占：并行会话同时改动了 `scripts/build-release.sh`、`scripts/f3-kill-window.sh`、
  `crates/wxml-delivery/tests/*`、`crates/wxml-lsp/tests/native_parity/{native_gates,symbols_edits}.rs`
  和 delivery contract。本轮不引用、不覆盖这些文件；其未完成状态只影响全量
  `native_parity` 运行，见 §3。

**结论：wave-7 证据文档中 3 个 weaker 用例达到 exact，提升为 `green`；其余 3 个保持原状态。**
计数：`green` 55 → 58、`planned` 248 → 245、`exact` 55 → 58、`weaker` 73 → 70；
`assessed_cases` 160、无证据 planned 143、`near_exact` 3、`structural` 7、
`blocked` 22、`retired` 36 不变。

判定口径与 wave-8 相同：TS oracle 的每条可观察断言都能在同一 Rust 测试里找到等价断言，
才记 `exact`；所有断言均取自 oracle 原文，没有从实现输出反推期望。

---

## 1. 逐 case 结论

### 1.1 `TS-PROBES-0027` → exact

TS oracle：`pragma scanning handles plain files, BOM and hashbang`
（`backends/typescript/test/probes.test.ts:27`）。

Rust 证据：`case_probes_0027_pragma_scanning_handles_plain_files_bom_and_hashbang`
（ crates/wxml-backend-tsgo/src/shadow/layout.rs:2467 ）。测试直接调用私有
`scan_pragmas`，并用新增的 `build_js_with_policy` 走真实 `build_in_place_shadow`
断言 BOM drop 与 pragma 编辑。

| TS 断言 | Rust 断言 |
| --- | --- |
| `plain.check === true` | `plain.check`（layout.rs:2470），并以 `JsCheckPolicy::Inherit` 构建后 `js_check_applied == Inherit` 佐证（layout.rs:2481-2486） |
| `plain.injectionOffset === 0` | `plain.injection_offset == 0`（layout.rs:2472） |
| `plain.neutralizeOffset === "// @ts-check\n".length`（13） | `utf16_len(&plain_text[..neutralize_offset]) == 13`（layout.rs:2473-2477），且 policy `Off` 的 `nocheck` 编辑 `at_utf16 == 13`（layout.rs:2567-2575） |
| `plain.dropBom === false` | `Inherit` 构建后没有任何 `bom` insertion（layout.rs:2487-2494） |
| `bom.bomLength === 1` | `bom.bom_length_utf16 == 1`（layout.rs:2498） |
| `bom.dropBom === true` | `Pragma` 构建后存在 `bom` insertion，`replace_length_utf16 == 1`（layout.rs:2504-2511） |
| `bom.injectionOffset === 1` | 扫描偏移 `utf16_len(...) == 1`（layout.rs:2499-2503），且 `bom` 的 `pragma` 编辑 `at_utf16 == 1`（layout.rs:2512-2518） |
| `shebang.injectionOffset === 20` | 扫描偏移 `== 20`（layout.rs:2522-2526），且 `shebang` 的 `pragma` 编辑 `at_utf16 == 20`（layout.rs:2537-2543） |
| `shebang.dropBom === false` | `Pragma` 构建后没有 `bom` insertion（layout.rs:2528-2536） |
| `both.dropBom === true` | `both` 构建后存在 `bom` insertion（layout.rs:2549-2557） |
| `both.injectionOffset === 21` | 扫描偏移 `== 21`（layout.rs:2547-2548），且 `both` 的 `pragma` 编辑 `at_utf16 == 21`（layout.rs:2558-2564） |

说明：

- oracle 的 `injectionOffset` 是原始源码坐标；`DerivedView::insertions.start_utf16`
  是派生文本坐标（BOM 删除后会前移），所以 BOM 用例改用
  `DerivedView::edits[].at_utf16`（原始文本坐标）断言 1/21。这是坐标语义差异，
  不是断言放宽。
- 布局要求一个可识别的注册；测试在 oracle 前缀后补 `Component({ data: {} });`。
  pragma/BOM/hashbang 前缀与偏移完全保留，补的尾部不改变扫描结果。
- `dropBom` 在 TS 中是 `wantPragma && bomLength > 0`；native 由
  `bom_length > 0 && (policy == Pragma || check || no_check)` 决定。四种 fixture
  与 oracle 一一对应。

### 1.2 `TS-ANALYSIS-0139` → exact

TS oracle：`declared property types survive an unresolved default value`
（`backends/typescript/test/analysis.test.ts:139`）。

Rust 证据（两部分）：

1. 声明事实：`case_analysis_0139_declared_property_types_survive_an_unresolved_default_value`
   （ crates/wxml-backend-tsgo/tests/script_properties.rs:99 ）断言
   `type_text == "number"`、`type_complete`、`default_text == "MissingEnum.Preview"`、
   `properties.complete`。
2. hover：同名测试
   （ crates/wxml-lsp/src/backend_tsgo/tests.rs:4251 ）用同一 fixture 走真实
   adapter/shadow/declared-property 覆盖层，断言
   `contents == "```ts\nnumber\n```"`、`quality == Known`、`status == Supported`，
   range 精确覆盖 `approvalStatus`。

| TS 断言 | Rust 断言 |
| --- | --- |
| `assert.ok(script)` | `input.file(SCRIPT_PATH).is_some()`（tests.rs:4284-4288） |
| `result.ok === true` | `adapter.hover(...).expect("hover")` 返回 Ok（tests.rs:4267-4272） |
| `approval?.hover?.typeOnly === "number"` | `contents == "```ts\nnumber\n```"` 且 `quality == Known`（tests.rs:4273-4275） |

说明：

- 测试故意让 fake native session 返回 `string`（tests.rs:4262-4265）；只有
  `properties.approvalStatus.type: Number` 的声明覆盖层能产出 `number`，因此该
  断言确实检验了“unresolved default 不影响声明类型”。
- native 组件根要求 `data` 成员；fixture 补了 `data: {}`。oracle 断言的
  `type`/`value`/hover 均未改变。
- 派生脚本中 `approvalStatus` 被挂到 `__wxml_props_0.` 前缀
  （tests.rs:4289-4293），与声明属性根一致。

### 1.3 `TS-ANALYSIS-0174` → exact

TS oracle：`analyze never reads project files or tsconfig from the project root`
（`backends/typescript/test/analysis.test.ts:174`）。

Rust 证据：`case_analysis_0174_hover_never_reads_the_project_root_tsconfig`
（ crates/wxml-lsp/src/backend_tsgo/tests.rs:4297 ）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `result.ok === true` | `adapter.hover(...).expect(...)` 返回 Ok（tests.rs:4308-4313） |
| `title?.hover?.typeOnly === "string"` | `contents == "```ts\nstring\n```"`、`quality == Known`、`status == Supported`（tests.rs:4314-4316） |
| 前置：`projectRoot` 在磁盘不存在 | `!Path::new(PROJECT_ROOT).exists()`（tests.rs:4301-4304） |
| “不读 project root” | session 收到的 `input.root() == PROJECT_ROOT`，且 `input.files()` 只有快照中的 `pages/index/index.ts`（tests.rs:4322-4333） |

说明：

- native 没有独立的 `analyze` 聚合 API；等价路径是 adapter 的 per-fragment hover，
  它复用同一 `build_request_plan`（host view + shadow + environment）。
- fake session 提供 `string` 文本；本 case 的行为主体是“project root 不存在也不
  影响 plan/查询”，由不存在路径、session 输入根和仅快照文件三项断言固定。
  VFS 永不回退 host 文件由
   crates/wxml-backend-tsgo/src/native/vfs.rs:1385 与
   crates/wxml-backend-tsgo/src/native/vfs.rs:1600 的既有测试覆盖，本轮作为补充证据
  保留在 `rust_tests`。

### 1.4 未提升的 3 个 case

`TS-ANALYSIS-0078`、`TS-ANALYSIS-0093`、`TS-GUARDS-0302` 的缺口与
`docs/evidence/2026-11-20-tsgo-wave7-weaker-exact.md` §1.4–§1.6 一致，本轮没有新测试：

- `TS-ANALYSIS-0078/0093` 的 hover 与诊断码 `2693/2304` 需要真实 pinned engine；
  本轮无 `TSGO_BIN`，opt-in gate 不能记为 case green。二者保持 `planned` + `weaker`。
- `TS-GUARDS-0302` 的 `jsCheckApplied` 与 diagnostics `Disabled` 在 native adapter
  中没有对应字段/状态，必须改生产代码才能 exact。台账维持 wave-7 的 `weaker`
  （wave-7 证据文档建议可改 `blocked`，但本轮不改 parity，避免与 22 个既有
  blocked 计数混淆）。

---

## 2. 台账字段回填

3 个 case 的 `cases[]` 最终字段完全一致：

| 字段 | 值 |
| --- | --- |
| `status` | `green` |
| `target_rust_test_status` | `migrated_green` |
| `migration_evidence.parity` | `exact` |
| `migration_evidence.green_eligible` | `true` |
| `migration_evidence.status_recommendation` | `green` |
| `migration_evidence.status_applied` | `green` |
| `migration_evidence.assessed_at` | `2026-11-20` |
| `migration_evidence.implementation_head` | `16e88b24917a34e225a4ec66b6ea0e0751262dc4` |

`migration_evidence.rust_tests`：

| Case | path:line | test |
| --- | --- | --- |
| `TS-PROBES-0027` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2467 | `case_probes_0027_pragma_scanning_handles_plain_files_bom_and_hashbang` |
| `TS-ANALYSIS-0139` |  crates/wxml-backend-tsgo/tests/script_properties.rs:99 | `case_analysis_0139_declared_property_types_survive_an_unresolved_default_value` |
| `TS-ANALYSIS-0139` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:4251 | `case_analysis_0139_declared_property_types_survive_an_unresolved_default_value` |
| `TS-ANALYSIS-0174` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:4297 | `case_analysis_0174_hover_never_reads_the_project_root_tsconfig` |

`TS-ANALYSIS-0174` 另保留既有 VFS 证据（`reads_only_virtual_files_and_never_host_files`、
`startup_canary_rejects_a_real_host_file_outside_the_root`）作为补充，不改变 exact 判定。
`target_rust_test` 保持既有 `native_parity/*.rs` 路由不变：validator 用文件 stem
计算 16 个 partition 的 routed count，改成实际执行文件会破坏 partition 契约。

计数（`counts.statuses` 与 `counts.migration_evidence` 同步）：

| 项 | wave-8 后 | wave-9 后 |
| --- | ---: | ---: |
| `green` | 55 | 58 |
| `planned` | 248 | 245 |
| `retired-with-decision` | 36 | 36 |
| `exact` | 55 | 58 |
| `near_exact` | 3 | 3 |
| `weaker` | 73 | 70 |
| `structural` | 7 | 7 |
| `blocked` | 22 | 22 |
| `assessed_cases` | 160 | 160 |
| `green_eligible` | 55 | 58 |
| `promoted_to_green` | 55 | 58 |
| 无 Rust 证据的 planned | 143 | 143 |
| 339 总数 | 339 | 339 |

分区变化：

- `guards_js`：green/exact 6→7、planned 有证据 7→6、weaker 2→1。
- `hover_docs`：green/exact 0→2、planned 有证据 7→5、weaker 7→5。
- 其余分区不变。

validator 常量：`GREEN_CASES` 55→58、`PLANNED_CASES` 248→245、
`WEAKER_CASES` 73→70；`TOTAL_CASES` 339、`RETIRED_CASES` 36、
`NEAR_EXACT_CASES` 3、`STRUCTURAL_CASES` 7、`BLOCKED_CASES` 22 不变。
`oracle_manifest::*`、live/frozen 校验、`fixture_hash.rs`、`native_parity.rs`
和 manifest JSON 未改动。

---

## 3. 验证

在本工作树、HEAD `16e88b2` 加并行未提交改动上执行：

| 命令 | 结果 |
| --- | --- |
| `jq empty docs/plans/tsgo-test-ledger.json` | exit 0 |
| `cargo test -p wxml-lsp --test native_parity ledger::` | 7 passed / 0 failed（台账 schema、计数、路由、证据、oracle manifest 全部通过） |
| `cargo test -p wxml-lsp --test native_parity` | 75 passed / 0 failed / 26 ignored（包含并行会话新增的 native gate 用例） |
| `cargo fmt --all -- --check` | exit 0 |
| `git diff --check` | exit 0 |

全量 `native_parity` 首轮曾在并行会话的 `native_gates/symbols_edits` 未完成改动上出现
2 个 `native_gates::*` 失败；并行会话随后补齐这些文件，末轮重跑 **75 passed / 0 failed**。
两个失败均不在本轮文件内，也不涉及 ledger validator。

`ledger::` 过滤运行 7/7 通过，其中
`ledger_migration_evidence_is_complete_and_consistent`、`ledger_counts_and_dispositions_are_frozen`
均已覆盖本轮 3 个 case 与计数常量。

3 个 case 的测试实际执行：

| 命令 | 结果 |
| --- | --- |
| `cargo test -p wxml-backend-tsgo --lib shadow::layout` | 通过；含 PROBES-0027 |
| `cargo test -p wxml-backend-tsgo --test script_properties` | 通过；含 0139 事实 |
| `cargo test -p wxml-lsp backend_tsgo::tests::case_analysis_` | 通过；含 0139 hover 与 0174 |

（具体计数见 `docs/evidence/2026-11-20-tsgo-wave7-weaker-exact.md` §3；本轮没有改测试，
引用其已记录的运行结果。）

---

## 4. 限制

1. `TS-ANALYSIS-0139/0174` 的 hover 文本由 fake session 提供；0139 检验的是
   “声明属性覆盖层压过 native 答案”，0174 检验的是“project root 不存在时
   plan/session 输入仍只含内存快照”。引擎自身的类型计算不在两条测试内。
2. `TS-ANALYSIS-0078/0093` 的 hover/`2693`/`2304` 只有真实 pinned engine 能产生；
   `TS-GUARDS-0302` 需要新的 adapter 生产字段，均只能改生产代码后 exact。
3. 工作树非独占；本轮验证期间并行会话仍在改 delivery/F3 与 native-gate 文件，
   全量 `native_parity` 首轮因此短暂失败，末轮重跑已 75/0。台账结论只依赖
   `docs/plans/tsgo-test-ledger.json` 与 ledger validator。
4. 未提交、未做独立 review；3 个 `green` 仍需主线程按 ledger 流程复核。
