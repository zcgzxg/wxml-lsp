# tsgo wave-7 weaker 补断言（2026-11-20）

## 0. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务下发时 HEAD：`036855f`；记录/验证 HEAD：`75040c2`（并行会话在本轮开始前提交了
  `75040c2 test(tsgo): freeze legacy oracle and backfill parity ledger`，本文件与
  测试改动都在该提交之上、未提交）
- 记录日期：2026-11-20
- 本轮只改 Rust 测试与本证据文档：
  - `crates/wxml-backend-tsgo/src/shadow/layout.rs`（仅 `#[cfg(test)] mod tests`）
  - `crates/wxml-backend-tsgo/tests/script_properties.rs`
  - `crates/wxml-lsp/src/backend_tsgo/tests.rs`
  - 本文件（新增）
- 未修改非测试生产代码、`Cargo.toml`、delivery、Node backend、默认后端、
  `docs/plans/tsgo-test-ledger.json`、Markdown 台账、contract、
  `crates/wxml-lsp/tests/native_parity/ledger.rs`、`native_parity.rs` 或 README；
  未提交、未启动 review。
- 工作树仍有并行会话的未提交内容（`scripts/build-release.sh`、
  `scripts/f3-kill-window.sh`、`crates/wxml-delivery/tests/*` 等）。本文件不引用
  它们，也没有覆盖或回滚它们。
- `gate_green` 与 case `green` 仍是两个状态轴；本轮只建议 case 级变更。

**结论：6 个 wave-7 weaker 中 3 个补到 exact，另外 3 个保持 weaker/blocked。**
建议主线程回填：`green`/`exact` 55 → 58、`planned` 248 → 245、`weaker` 73 → 70；
`assessed_cases` 160、`near_exact` 3、`structural` 7、`blocked` 22、`retired` 36
不变。`TS-ANALYSIS-0078/0093` 的 hover/2693/2304 与 `TS-GUARDS-0302` 的
diagnostics `disabled` 需要真实引擎或新的生产字段，不能在不改生产代码的前提下
补成 exact。

判定口径与 wave-8 相同：TS oracle 的每条可观察断言都能在同一 Rust 测试里找到
等价断言，才记 `exact`；新增断言全部取自 oracle 原文，没有从实现输出反推期望。

---

## 1. 逐 case 结论

### 1.1 `TS-PROBES-0027` → exact

TS oracle：`pragma scanning handles plain files, BOM and hashbang`
（`backends/typescript/test/probes.test.ts:27`）。

Rust 证据：`case_probes_0027_pragma_scanning_handles_plain_files_bom_and_hashbang`
（ crates/wxml-backend-tsgo/src/shadow/layout.rs:2467 ，测试模块内可直接调用私有
`scan_pragmas`，并用 `build_in_place_shadow` 断言 `dropBom` 的实际编辑）。

| TS 断言 | Rust 断言 |
| --- | --- |
| `plain.check === true` | `plain.check`（layout.rs:2470），并以 `JsCheckPolicy::Inherit` 构建后 `js_check_applied == Inherit` 佐证（layout.rs:2481-2486） |
| `plain.injectionOffset === 0` | `plain.injection_offset == 0`（layout.rs:2472） |
| `plain.neutralizeOffset === "// @ts-check\n".length`（13） | `utf16_len(&plain[..neutralize_offset]) == 13`（layout.rs:2473-2477），且 policy `Off` 的 `nocheck` 编辑 `at_utf16 == 13`（layout.rs:2567-2575） |
| `plain.dropBom === false` | policy `Inherit` 构建后没有任何 `bom` insertion（layout.rs:2487-2494） |
| `bom.bomLength === 1` | `bom.bom_length_utf16 == 1`（layout.rs:2498） |
| `bom.dropBom === true` | policy `Pragma` 构建后存在 `bom` insertion，`replace_length_utf16 == 1`（layout.rs:2504-2511） |
| `bom.injectionOffset === 1` | `bom` 行的 `pragma` 编辑 `at_utf16 == 1`（layout.rs:2512-2518），扫描偏移经 `utf16_len` 断言为 1（layout.rs:2499-2503） |
| `shebang.injectionOffset === 20` | `shebang` 的 `pragma` 编辑 `at_utf16 == 20`（layout.rs:2537-2543），扫描偏移断言为 20（layout.rs:2522-2526） |
| `shebang.dropBom === false` | policy `Pragma` 构建后没有 `bom` insertion（layout.rs:2528-2536） |
| `both.dropBom === true` | `both` 构建后存在 `bom` insertion（layout.rs:2549-2557） |
| `both.injectionOffset === 21` | `both` 的 `pragma` 编辑 `at_utf16 == 21`（layout.rs:2558-2564），扫描偏移断言为 21（layout.rs:2547-2548） |

说明：

- oracle 的 `injectionOffset` 是原始源码坐标；`DerivedView::insertions.start_utf16`
  是派生文本坐标（BOM 删除后会前移），所以 BOM 用例改用
  `DerivedView::edits[].at_utf16`（原始文本坐标）断言 1/21。这个差异是坐标语义，
  不是断言放宽。
- 布局要求一个可识别的注册；测试在 oracle 前缀后补
  `Component({ data: {} });`。pragma/BOM/hashbang 前缀与偏移完全保留，补的尾部
  不改变扫描结果。
- `dropBom` 在 TS 中是 `wantPragma && bomLength > 0`；native 中由
  `bom_length > 0 && (policy == Pragma || check || no_check)` 决定。本测试的
  `plain`/`shebang` 没有 BOM，`bom`/`both` 在 `Pragma` 策略下必删，四种组合与
  oracle fixture 一一对应。

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
   adapter/shadow/declared-property 覆盖层，断言 `contents == "```ts\nnumber\n```"`、
   `quality == Known`、`status == Supported`、range 精确覆盖 `approvalStatus`。

| TS 断言 | Rust 断言 |
| --- | --- |
| `assert.ok(script)` | `input.file(SCRIPT_PATH).is_some()`（tests.rs:4284-4288） |
| `result.ok === true` | `adapter.hover(...).expect("hover")` 返回 Ok（tests.rs:4267-4272） |
| `approval?.hover?.typeOnly === "number"` | `contents == "```ts\nnumber\n```"` 且 `quality == Known`（tests.rs:4273-4275） |

说明：

- 测试故意让 fake native session 返回 `string`（tests.rs:4262-4265）；只有
  `properties.approvalStatus.type: Number` 的声明覆盖层能产出 `number`，因此该
  断言确实检验了 “unresolved default 不影响声明类型”。
- native 组件根要求 `data` 成员；测试 fixture 补了 `data: {}`。oracle 断言的
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

- native 没有独立的 `analyze` 聚合 API；这里的等价路径是 adapter 的 per-fragment
  hover，它复用同一 `build_request_plan`（host view + shadow + environment）。
- fake session 提供 `string` 文本；本 case 的行为主体是“project root 不存在也不
  影响 plan/查询”，该点由不存在路径、session 输入根和仅快照文件三项断言固定。
  VFS 永不回退 host 文件由
   crates/wxml-backend-tsgo/src/native/vfs.rs:1385 与
   crates/wxml-backend-tsgo/src/native/vfs.rs:1600 的既有测试覆盖。

### 1.4 `TS-ANALYSIS-0078` → weaker（保持）

TS oracle：`tsconfig lib and target spellings load the core libraries`
（`backends/typescript/test/analysis.test.ts:78`）。

缺口（wave-7 原文，本轮复核仍成立）：

- `result.ok === true`、`title.hover.typeOnly === "string"`；
- 诊断码不含 `2693`（core lib 未加载）和 `2304`（全局名未解析）。

现有 Rust 证据（ crates/wxml-backend-tsgo/src/native/vfs.rs:1704 、
 crates/wxml-backend-tsgo/src/native/vfs.rs:1670 ）只固定了生成 tsconfig 保留
`lib`/`target`/`module`，以及 core lib 与 pinned typings 以 virtual asset 挂载。
`2693/2304` 和真实 hover 都必须由 pinned engine 运行后产生；本轮没有
`TSGO_BIN`，按策略 opt-in `native_engine_real` 也不能作为 case green 证据。
因此保持 `planned` + `weaker`，没有新增测试。

### 1.5 `TS-ANALYSIS-0093` → weaker（保持）

TS oracle：`typeRoots declarations are readable without project files on disk`
（`backends/typescript/test/analysis.test.ts:93`）。

缺口：`hover.typeOnly === "\"from-type-root\""` 与诊断不含 `2304` 都要求引擎真正
解析 typeRoot 下的 `.d.ts`。

现有 Rust 证据（ crates/wxml-backend-tsgo/src/native/vfs.rs:1922 ）固定
`typeRoots`/`types` 保留、本地 `types/local/index.d.ts` 来自 snapshot、`types`
未降级。它证明 VFS 环境正确，但不能替代引擎 hover/诊断。保持 `planned` +
`weaker`。

### 1.6 `TS-GUARDS-0302` → blocked（台账当前 weaker）

TS oracle：`checkJs off keeps hover but disables template diagnostics`
（`backends/typescript/test/guards.test.ts:302`）。

oracle 五条断言：

1. `result.ok === true`；
2. `result.response.jsCheckApplied === "off"`；
3. `result.states.diagnostics.status === "disabled"`；
4. `fragments[0].hover.typeOnly === "any"`；
5. `fragments[0].diagnostics` 为空。

现有 Rust 证据（ crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:581 、
 crates/wxml-backend-tsgo/tests/shadow_layout.rs:550 ）只证明
`JsCheckPolicy::Off`/`Inherit` 映射，以及 source `@ts-check` 被 shadow-only
`@ts-nocheck` 中和。

native adapter 当前没有 `jsCheckApplied` 响应字段，`backend_tsgo/diagnostics.rs`
也从不产生 `CapabilityStatus::Disabled`（只有 `Supported`/`Partial`/
`Unsupported`/`Unavailable` 路径）。因此第 2、3 条没有可观察字段，hover “any”
又依赖 checker。按 `BLOCKED_CASES` 的既有判定保持不提升；建议 parity 标为
`blocked`（若主线程希望保守，也可继续保留 `weaker`），不要 green。

---

## 2. 建议 ledger 回填（未修改 ledger）

| Case | status 建议 | parity 建议 | green_eligible | target_rust_test_status | 实际 Rust 证据 |
| --- | --- | --- | --- | --- | --- |
| `TS-PROBES-0027` | `green` | `exact` | `true` | `migrated_green` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2467 |
| `TS-ANALYSIS-0139` | `green` | `exact` | `true` | `migrated_green` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:4251 ； crates/wxml-backend-tsgo/tests/script_properties.rs:99 |
| `TS-ANALYSIS-0174` | `green` | `exact` | `true` | `migrated_green` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:4297 |
| `TS-ANALYSIS-0078` | `planned` | `weaker` | `false` | 不变 | 既有 vfs 测试 |
| `TS-ANALYSIS-0093` | `planned` | `weaker` | `false` | 不变 | 既有 vfs 测试 |
| `TS-GUARDS-0302` | `planned` | `blocked`（或保留 `weaker`） | `false` | 不变 | 既有 layout/guards 测试 |

- `target_rust_test` 建议保持现有 `native_parity/*.rs::case_*` 路径不变：ledger
  validator 用文件 stem 计算 16 个 partition 的 routed count，改成
  `backend_tsgo/tests.rs` 会破坏 partition 契约。实际执行测试放在
  `migration_evidence.rust_tests`。
- 计数影响：`green`/`exact` 55 → 58、`planned` 248 → 245、`weaker` 73 → 70；
  其余计数不变。若采纳 `TS-GUARDS-0302` 的 `weaker` → `blocked` 建议，则
  `weaker` 70 → 69、`blocked` 22 → 23，其余不变；只做 3 个 green 提升时按前一组
  数字。
- 新增测试：4 个 `#[test]` 函数（backend-tsgo 2：PROBES、0139 事实；wxml-lsp 2：
  0139 hover、0174）。

## 3. 验证

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo check --all-targets` | exit 0 |
| `cargo clippy -p wxml-backend-tsgo -p wxml-lsp --all-targets -- -D warnings` | exit 0（No issues found） |
| `cargo clippy --all-targets -- -D warnings`（全工作区） | **被并行未提交文件阻断**： crates/wxml-delivery/tests/f3_acceptance/support.rs:208 `clippy::manual_assert`；与本轮改动无关，未修改 |
| `cargo test -p wxml-backend-tsgo` | 628 passed / 0 failed / 4 ignored |
| `cargo test -p wxml-lsp --lib` | 946 passed / 0 failed |
| `cargo test -p wxml-lsp --test native_parity` | 68 passed / 0 failed / 26 ignored |
| `cargo test -p wxml-backend-tsgo --lib shadow::layout` | 11 passed；含 PROBES-0027 |
| `cargo test -p wxml-backend-tsgo --test script_properties` | 14 passed；含 0139 事实 |
| `cargo test -p wxml-lsp backend_tsgo::tests::case_analysis_` | 2 passed；含 0139 hover 与 0174 |
| `git diff --check` | exit 0 |

新增 green 数：**3**（`TS-PROBES-0027`、`TS-ANALYSIS-0139`、`TS-ANALYSIS-0174`）。
`cargo test -p wxml-backend-tsgo` 从 626 增至 628（+2；0139 在 wxml-lsp 侧再 +1，
0174 再 +1，wxml-lsp lib 从 944 增至 946）。

## 4. 限制

1. `TS-ANALYSIS-0139/0174` 的 hover 文本由 fake session 提供；0139 实际检验的是
   “声明属性覆盖层压过 native 答案”，0174 实际检验的是“project root 不存在时
   plan/session 输入仍只含内存快照”。引擎自身的类型计算不在这两条测试内。
2. `TS-ANALYSIS-0078/0093` 的 hover/`2693`/`2304` 只有真实 pinned engine 能产生；
   本轮无 `TSGO_BIN`，且 opt-in gate 不能记为 case green。
3. `TS-GUARDS-0302` 的 `jsCheckApplied` 与 diagnostics `Disabled` 在 native
   adapter 中没有对应字段/状态，必须改生产代码才能 exact。
4. 工作树非独占；`crates/wxml-delivery/tests/*` 的并行改动会让全工作区 clippy
   失败，因此本轮只对 `wxml-backend-tsgo` 与 `wxml-lsp` 做 scoped clippy。
5. 未提交、未做独立 review；所有建议的 `green` 仍需主线程按 ledger 流程复核。
