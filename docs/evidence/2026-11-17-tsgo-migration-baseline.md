# Rust + tsgo 迁移基线（P0）

日期：2026-11-17  
工作区：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`  
分支：`tsgo/migration`  
基线 HEAD：`b9616c557e54a06b7d756e516e6dd7c4016664cf`（`docs: add rust tsgo migration plan and ADR-0008`）  
状态：**证据记录完成；不是迁移完成报告**。本文件只新增 `docs/evidence/`、`docs/plans/` 产物，未修改源码、测试、Cargo 文件、计划或 ADR，未提交。

## 1. 结论摘要

| 项目 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0，无 diff |
| `cargo check --workspace --all-features` | exit 0 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | exit 0 |
| `cargo test --workspace --all-features` | exit 0；**1297 passed / 0 failed / 23 ignored** |
| Rust 编译测试函数 | 1319（另有 1 个 doctest；`cargo test --list` 共 1320 条） |
| 旧 TS 套件（`npm test`） | **exit 1；339 tests / 338 pass / 1 fail / 0 skipped / 0 todo**（失败仅 perf） |
| TS perf 复跑 | **isolated 3/3 pass**：coldStart 1058.25 / 1005.64 / 675.60 ms（page-basic / in-place-shadow） |
| TS perf 全量失败 | 全量并行套件 `node --test test/*.test.ts` 下 cold start 8325.25 ms > 8000 ms；isolated 不复现 |
| 静态 TS 盘点 | 274 个 `test()` 声明；339 个展开注册；1145 个直接断言调用点（930 个语义断言） |
| 计划计数 | 273 / ~338；实际 274 / 339，差 +1（`events.test.ts` 实际 24 而非 23） |
| proptest | 6 个 `proptest!` block，12 个 property 函数；计划写 13，差 -1 |
| 夹具 hash | 统一 v1 算法复算 4 棵树；计划 hash 陈旧/未定义，不复用 |
| dirty 增量 | 计划提到的 19 个 dirty 路径已成为提交 `4d59ca7`，包含在 BASE 中 |
| ignored/静默跳过 | 23 个 `#[ignore]` + 2 个 `semantic_tools` 静默 return + 1 个可选 tsc 检查；均不计为通过 |

结论：**Rust 基线的四个门禁全绿；旧 TS 全量套件按运行记录仍是 red（338/339）**。唯一失败是 perf 在全量并行负载下的 cold-start 超时；同一测试 isolated 串行 3/3 通过。因此它是**负载相关/不稳定门禁**，不是可复现的产品回归；BASE 的全量套件仍必须记为 red，不能静默修正、改阈值或当作原生目标。23 个 ignored 与 2 个静默 return 是 P5 必跑原生门禁，不是已迁移测试。

## 2. 环境与工作区冻结

### 2.1 时间点

- P0 开始：`2026-11-17 21:52:45 +0800 (CST)`。
- 最终复查：`2026-11-17 22:29:25 +0800 (CST)`（HEAD 与初始相同）。
- 本文件的命令在 21:52–22:30 之间执行；日志位于 `/tmp/opencode/p0-p1b-20261117/`，未写入项目目录。

### 2.2 Git 状态

初始 `git status --porcelain`（21:52:45）：**空**。  
初始 `git rev-parse HEAD`：`b9616c557e54a06b7d756e516e6dd7c4016664cf`。  
初始 `git log -1 --format='%H %s'`：`b9616c557e54a06b7d756e516e6dd7c4016664cf docs: add rust tsgo migration plan and ADR-0008`。

最终复查（22:29:25）HEAD 未变；`git status --porcelain` 出现两个**并发 P1 agent 写入的未跟踪文件**，不是本任务写入：

```text
?? docs/evidence/2026-11-17-tsgo-delivery-spike.md
?? docs/evidence/2026-11-17-tsgo-workspace-baseline.md
```

22:36:34 写盘后复查又出现第三个并发文件；HEAD 仍为 `b9616c5`，无 tracked 文件修改（`git diff --stat` 为空）：

```text
?? docs/evidence/2026-11-17-tsgo-delivery-spike.md        (22:06:09)
?? docs/evidence/2026-11-17-tsgo-migration-baseline.md    (本文件)
?? docs/evidence/2026-11-17-tsgo-native-api-spike.md      (22:34:25)
?? docs/evidence/2026-11-17-tsgo-workspace-baseline.md    (22:05:05)
?? docs/plans/2026-11-17-tsgo-test-ledger.md              (P1-B)
?? docs/plans/tsgo-test-ledger.json                       (P1-B)
```

P1-B 只写本文件、`docs/plans/2026-11-17-tsgo-test-ledger.md`、`docs/plans/tsgo-test-ledger.json`；其余三个 evidence 文件属于并发 P1-A/P1-C，未读取后修改，也未纳入本账本声明。

23:08:24 最终复查：HEAD 仍为 `b9616c5`，但并发 agent 已修改以下 tracked docs（**不是 P1-B 写入**）：

```text
 M docs/README.md
 M docs/adr/0001-layered-language-service-architecture.md
 M docs/adr/0002-template-type-checking-and-source-maps.md
 M docs/adr/0003-versioned-document-synchronization.md
 M docs/adr/0006-explicit-mini-program-roots.md
 M docs/adr/0007-local-delivery-trust-model.md
 M docs/adr/0008-rust-tsgo-no-node-backend.md
 M docs/adr/README.md
 M docs/plans/2026-11-17-rust-tsgo-migration.md
```

同一时刻未跟踪文件仍为三个并发 evidence + P1-B 的三个文件。`git diff --stat` 仅涉及上述 docs；**没有 source/test/Cargo 修改**。P1-B 的计数与用例来自 HEAD `b9616c5` 的 TS 测试文件；并发 docs 修改不改变本账本的测试基线。

已有 worktree（`git worktree list`，只读复核）：

```text
~/code/rust/wxml-lsp                                          4d59ca7 [master]
~/.local/share/opencode/worktree/e15985/tsgo-migration        b9616c5 [tsgo/migration]
~/code/rust/wxml-lsp-worktrees/full-components                0c88899 [feat/full-components]
~/code/rust/wxml-lsp-worktrees/full-editing                   69b70d2 [feat/full-editing]
~/code/rust/wxml-lsp-worktrees/full-integration               eedef06 [feat/full-integration]
~/code/rust/wxml-lsp-worktrees/full-local-delivery            6eb43ff [feat/full-local-delivery]
~/code/rust/wxml-lsp-worktrees/full-performance-local         42a82ba [feat/full-performance-local]
~/code/rust/wxml-lsp-worktrees/full-projects                  50786dd [feat/full-projects]
~/code/rust/wxml-lsp-worktrees/full-semantic-tools            dd15bc1 [feat/full-semantic-tools]
~/code/rust/wxml-lsp-worktrees/full-styles                    856c1ac [feat/full-styles]
~/code/rust/wxml-lsp-worktrees/full-type-semantics            36bba6d [feat/full-type-semantics]
~/code/rust/wxml-lsp-worktrees/wave5-completion               4fb185c [feat/wave5-completion]
~/code/rust/wxml-lsp-worktrees/wave5-contracts                26c2766 [feat/wave5-contracts]
~/code/rust/wxml-lsp-worktrees/wave5-events                   98bc529 [feat/wave5-events]
~/code/rust/wxml-lsp-worktrees/wave5-templates                dc8c7eb [feat/wave5-template-types]
~/code/rust/wxml-lsp-worktrees/wave5-zed-sync                 cad3c76 [feat/wave5-zed-sync]
```

主工作区 `/home/zxg/code/rust/wxml-lsp` 在 P0 时位于 `master` `4d59ca7`，`git status --porcelain` 为空。它不是本次执行目录，只用于只读复制 `node_modules`。

### 2.3 工具链

```text
rustc 1.100.0 (0ef1bc386 2026-11-09)
binary: rustc
commit-hash: 0ef1bc386059596dada8fb65b50425aab2ea637a
commit-date: 2026-11-09
host: x86_64-unknown-linux-gnu
release: 1.100.0
LLVM version: 23.1.5

cargo 1.100.0 (3acc79c0c 2026-11-05)

node v24.22.2
npm 11.19.1
```

`rust-toolchain.toml`：`channel = "stable"`，components `clippy`、`rustfmt`，profile `minimal`。  
平台：`Linux 7.2.15+deb14-amd64 x86_64 GNU/Linux`，24 CPU，约 30 GiB RAM，根分区可用约 287 GiB。

## 3. Rust 门禁

全部在项目根目录直接执行；未使用 `--locked`、`--doc`、`cargo doc`、`llvm-cov`、`audit`、`deny`（这些属于 P5 的完整命令集，本 P0 不冒充已通过）。

| 门禁 | 命令 | Exit | 墙钟 | 结果 |
| --- | --- | ---: | ---: | --- |
| 格式 | `cargo fmt --all -- --check` | 0 | 0.807 s | 无 diff |
| 检查 | `cargo check --workspace --all-features` | 0 | 9.482 s | `Finished dev profile` |
| Clippy | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 0 | 12.155 s | `Finished dev profile`，无 warning |
| 测试 | `cargo test --workspace --all-features` | 0 | 36.474 s | 1297 passed / 0 failed / 23 ignored |

### 3.1 测试结果汇总

日志中 49 条 `test result:`：

- 42 个普通 target run（lib/bin/integration）。
- 7 个 `Doc-tests` target（多数 0 test）。
- 合计：`1297 passed; 0 failed; 23 ignored; 0 measured; 0 filtered out`。

其中：

- 1319 个编译测试函数 = 1296 passed + 23 ignored。
- 1 个 doctest（`crates/wxml-syntax/src/lib.rs` line 28）passed。
- 1297 = 1296 + 1 doctest。

没有任何 Rust 失败。`#[ignore]` 的 23 个测试见 §4.2；它们不是通过，也不代表真实引擎门禁已运行。

### 3.2 命令来源与日志

| 文件 | 内容 |
| --- | --- |
| `/tmp/opencode/p0-p1b-20261117/fmt.log` | fmt 输出 + `real` 时间 |
| `/tmp/opencode/p0-p1b-20261117/check.log` | check 输出 + 时间 |
| `/tmp/opencode/p0-p1b-20261117/clippy.log` | clippy 输出 + 时间 |
| `/tmp/opencode/p0-p1b-20261117/test.log` | 全量测试输出 |
| `/tmp/opencode/p0-p1b-20261117/ts-test.log` | `npm test` 输出与失败栈 |
| `/tmp/opencode/p0-p1b-20261117/ts-per-file/*.tap` | 21 个 TS 文件逐文件 TAP 计数 |

## 4. Rust 测试属性独立盘点

### 4.1 计数

| 口径 | 数量 | 说明 |
| --- | ---: | --- |
| 原始 `#[test]` 属性 | 1322 | `grep -rn '^\s*#\[test\]'` |
| 其中 `crates/*/src` | 948 | 包含 3 个 `#[cfg(windows)]` |
| 其中 `crates/*/tests` | 374 | 32 个文件 |
| Linux 编译的 src 测试函数 | 945 | 948 − 3 个 Windows-only |
| 编译测试函数合计 | 1319 | 945 src + 374 tests |
| `cargo test --list` | 1320 | 1319 + 1 doctest |
| `#[ignore]` 属性 | 23 | grep 的 28 行中有 5 行是文档注释 |
| `proptest!` block | 6 | 3 个 src 文件 + 1 个 tests 文件 |
| proptest property 函数 | 12 | configuration 3、line_index 1、range 2、properties 6 |

计划第 6.1.1 节写“src 945（74 文件）、tests 374（32 文件）、13 个 proptest 定义”。前两项与实际一致（945 是**非 Windows 编译数**，不是 raw grep 的 948）；proptest 实际为 12 个 property 函数（6 个 block），差 -1。该差异不影响迁移用例数，但 P2/F2 不应沿用“13”。

### 4.2 23 个 `#[ignore]`

全部位于 `crates/wxml-lsp/tests`，共 5 个文件。每个测试体内的 `server.is_file()` 或等价检查在 `--ignored` 下会硬失败，因此它们是显式门禁，不是静默 skip；普通 `cargo test` 只报告 ignored。

| # | 文件 | 测试 | ignore 原因 |
| ---: | --- | --- | --- |
| 1 | `performance_bench.rs` | `q2_performance_benchmark` | performance gate: set `WXML_TEST_NODE=1` and run with `--ignored` |
| 2 | `q2_stability.rs` | `ten_round_lifecycle_stability` | stability gate: set `WXML_TEST_NODE=1` and run with `--ignored` |
| 3 | `real_node.rs` | `non_owner_script_overlay_propagates_and_survives_a_node_restart` | requires Node 24 + built P5 `dist/server.js` |
| 4 | `real_node.rs` | `real_node_event_handler_diagnostics_definition_and_completion` | 同上 |
| 5 | `real_node.rs` | `real_node_expression_and_event_completion_smoke` | 同上 |
| 6 | `real_node.rs` | `real_node_hover_and_diagnostics_smoke` | 同上 |
| 7 | `real_node.rs` | `real_node_symbols_smoke` | 同上 |
| 8 | `stdio_type_loop.rs` | `ac001_page_data_and_jsdoc_hover` | requires `WXML_TEST_NODE=1` + built `backends/typescript/dist/server.js` |
| 9 | `stdio_type_loop.rs` | `ac002_field_typo_diagnostic_and_fix_clear` | 同上 |
| 10 | `stdio_type_loop.rs` | `ac003_unicode_and_crlf_positions` | 同上 |
| 11 | `stdio_type_loop.rs` | `ac011_unknown_owner_degrades_honestly` | 同上 |
| 12 | `stdio_type_loop.rs` | `ac013_component_properties_hover` | 同上 |
| 13 | `stdio_type_loop.rs` | `component_behavior_data_and_properties_work_over_stdio` | real server binary + Node fixture |
| 14 | `stdio_type_loop.rs` | `static_wxs_and_template_semantics_work_over_stdio` | 同上 |
| 15 | `stdio_type_loop.rs` | `static_wxs_answers_survive_an_unavailable_type_backend` | real server binary |
| 16 | `stdio_type_loop.rs` | `unsaved_wxs_overlay_updates_static_hover_and_completion` | real server binary + Node fixture |
| 17 | `wave5_acceptance.rs` | `ac005_loop_scope_typed_locals` | requires Node 24 + built `dist/server.js` |
| 18 | `wave5_acceptance.rs` | `ac006_conditional_union_narrowing` | 同上 |
| 19 | `wave5_acceptance.rs` | `ac008_alias_project_hover_and_definition` | 同上 |
| 20 | `wave5_acceptance.rs` | `ac008_multi_view_fixture_isolation` | 同上 |
| 21 | `wave5_acceptance.rs` | `ac008_simultaneous_views_do_not_pollute_each_other` | 同上 |
| 22 | `wave5_acceptance.rs` | `ac012_component_property_bindings` | 同上 |
| 23 | `wave5_acceptance.rs` | `ac012_event_handlers_basic` | 同上 |

本 P0 **没有运行** `--ignored`：工作区没有 `backends/typescript/dist/server.js`，且 P0 不构建旧后端。它们不是通过，也不是迁移完成。逐项到必跑原生 gate 的映射在 `docs/plans/tsgo-test-ledger.json` 的 `ignored_rust_tests`（G-P5-*）。

### 4.3 静默跳过与可选检查（必须 fail-closed）

这些路径在普通套件中报告 pass，但没有真正验证目标行为。三个 Rust 路径的替换测试冻结如下；替换测试均为 **native real-engine**，不是旧 Node 测试。

| 文件:行 | 旧行为 | 原生替换测试 | 缺引擎/缺资产时必须失败？ | 旧测试要求 |
| --- | --- | --- | --- | --- |
| `crates/wxml-lsp/tests/semantic_tools.rs:2081` | `real_node_paths()` 为 `None` 时 `eprintln!` + `return`；普通套件 pass | `crates/wxml-lsp/tests/native_parity/symbols_edits.rs::native_component_property_edit_applies_and_reparses` | **是**。测试开场做必需原生资产检查，缺失即 panic/assert；不得返回 | 把 Option/early-return 改成 panic 前置条件；或在替换测试 GREEN、review 通过后才删除旧测试。不得在缺引擎时保持绿色 |
| `crates/wxml-lsp/tests/semantic_tools.rs:2568` | 同样静默 return；普通套件 pass | `crates/wxml-lsp/tests/native_parity/symbols_edits.rs::native_rejects_cross_namespace_and_dynamic_edits` | **是**。缺引擎即失败 | 同上；该测试是安全拒绝门禁，不得以环境缺失跳过 |
| `crates/wxml-lsp/tests/semantic_tools.rs:2169–2194` | `node_modules/typescript/bin/tsc` 不存在时打印 skipped 并继续 | `crates/wxml-lsp/tests/native_parity/symbols_edits.rs::assert_native_edit_reparses_and_typechecks`（两个编辑测试共用的强制 helper） | **是**。helper 无条件调用；原生 engine/类型信息缺失即失败 | 删除 tsc `exists()`/skipped 分支；不保留 Node/tsc 前置条件 |

替换映射也写入账本 `silent_skip_gates` 和 `silent_skips.rust_silent_skips`，字段包括 `replacement_test`、`replacement_kind`、`old_test_must_fail_when_engine_missing=true`。

TS 条件跳过（本次完整 checkout 中实际执行，`skipped=0`；迁移后仍需 fail-closed）：

| 文件:行 | 测试 | 当前行为 | 迁移要求 |
| --- | --- | --- | --- |
| `backends/typescript/test/meta.test.ts:27` | `the C1 contract documents the pinned versions` | 缺 `docs/reference/type-bridge-c1.md` 时 `context.skip` | F0/F1 版本/合同测试；缺文档/版本即失败 |
| `backends/typescript/test/evidence.test.ts:25` | `evidence output must stay inside the repository layout` | 缺合同文档时 `context.skip` | Rust evidence 路径测试；无条件 skip |
| `backends/typescript/test/evidence.test.ts:50` | `the committed evidence contains no absolute paths` | 缺 `matrix.json` 时 `context.skip` | Rust evidence 测试；缺 fixture 即失败 |

两个 Rust 静默 return 在本次普通套件中确实走了 return 分支（因为 `dist/server.js` 不存在），却计为 pass；它们不是真实 E2E 证据。

## 5. 旧 TS 套件基线

### 5.1 前置条件

- Node v24.22.2、npm 11.19.1 已存在。
- `backends/typescript/node_modules` 在 worktree 中原本不存在；从 `/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules` 原样复制 29 MB，未修改源目录。该路径由嵌套 `backends/typescript/.gitignore` 的 `node_modules/` 忽略；复制后 `git status --porcelain` 仍为空，`git check-ignore -v` 显示 `backends/typescript/.gitignore:1:node_modules/`。
- **未运行 `npm install`、`npm ci` 或任何下载**。
- 使用包内文档脚本 `npm test`，即 `node --test test/*.test.ts`。
- `npm test` 不构建 `dist/`；`server.test.ts` 以 `process.execPath` 直接启动 `src/server.ts`（Node 24 type stripping）。因此本运行不需要 `dist`，也没有生成它。

### 5.2 命令与结果

```sh
cd backends/typescript
npm test
```

Exit **1**，墙钟 45.255 s：

```text
ℹ tests 339
ℹ suites 0
ℹ pass 338
ℹ fail 1
ℹ cancelled 0
ℹ skipped 0
ℹ todo 0
ℹ duration_ms 45108.387113
```

唯一失败：

```text
test at test/perf.test.ts:12:1
✖ rough timings for cold start, hover and diagnostics (8603.115403ms)
  AssertionError [ERR_ASSERTION]: cold start 8325.25 ms
      at TestContext.<anonymous> (test/perf.test.ts:19:12)
```

`perf.test.ts` 的断言为 `timing.coldStartMs < 8000`（第一个 layout 是 `in-place-shadow`）。全量套件按运行记录仍是 red；但该失败不是 isolated 可复现的，见 §5.3。

### 5.3 Perf 串行复跑（3 次）

命令（每次只跑 perf 文件，TAP reporter；未改阈值、未改测试、未跑 cargo）：

```sh
cd backends/typescript
node --test --test-reporter=tap test/perf.test.ts
```

三次串行，记录 page-basic / in-place-shadow 的 `coldStartMs`（其余三组也记录）：

| Run | 时间 (+08:00) | load average | Exit | page-basic / in-place | page-basic / symbol | wide-40 / in-place | wide-40 / symbol | 墙钟 |
| ---: | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 23:00:50 | 0.26 / 0.28 / 0.46 | 0 | **1058.25 ms** | 279.42 ms | 297.16 ms | 286.84 ms | 2.759 s |
| 2 | 23:00:58 | 0.24 / 0.28 / 0.45 | 0 | **1005.64 ms** | 266.79 ms | 220.05 ms | 220.85 ms | 2.493 s |
| 3 | 23:01:04 | 0.78 / 0.39 / 0.49 | 0 | **675.60 ms** | 212.21 ms | 234.22 ms | 238.83 ms | 1.972 s |

Ambient load/other workers observable: load average 0.24–0.78 on 24 CPUs; top CPU consumers were `zed-editor` (~11.6%), `kwin_wayland` (~7.9%), `DingTalk` renderers (~7.7% / 6.2%), `opencode` (~4.5%), plus this shell. No cargo/rustc build was running during these three runs. Logs: `/tmp/opencode/p0-p1b-20261117/perf-tap-run{1,2,3}.log`.

结论：

- isolated 串行 **3/3 pass**，coldStart 675.60–1058.25 ms，远低于 8000 ms。
- 全量 `npm test` 的 8325.25 ms 出现在 `node --test test/*.test.ts` 的**文件级并行**运行中（21 个测试文件同时启动，多个 TypeScript 编译并发），是负载相关 outlier，不是 isolated 可复现的硬失败。
- **BASE 的全量套件仍记为 red（338/339）**；不能把 isolated 3/3 当作全量通过。
- 不修改 8000 ms 阈值、不编辑测试。P2 需要一个**独立于迁移前的决定**：把 perf 作为串行专用门禁（例如在 CI 中单独串行运行 perf），或明确把全量并行下的 perf 记为已知环境门禁；在决定前，旧 perf 测试不能作为绿色 oracle。

### 5.4 逐文件执行计数

用 `node --test --test-reporter=tap test/<file>.test.ts` 逐文件 isolated 运行得到。注意：这是**逐文件串行等价**的计数，不等于全量 `npm test` 的文件级并行结果；全部 21 个文件 isolated 均通过（339/339），包括 perf（coldStart 1235.14 ms），而全量并行时 perf 失败（338/339）。

| 文件 | tests | pass | fail |
| --- | ---: | ---: | ---: |
| analysis.test.ts | 23 | 23 | 0 |
| compiler.test.ts | 6 | 6 | 0 |
| completion.test.ts | 9 | 9 | 0 |
| componentBindings.test.ts | 28 | 28 | 0 |
| definitions.test.ts | 5 | 5 | 0 |
| events.test.ts | 24 | 24 | 0 |
| evidence.test.ts | 3 | 3 | 0 |
| guards.test.ts | 16 | 16 | 0 |
| layouts.test.ts | 12 | 12 | 0 |
| locals.test.ts | 12 | 12 | 0 |
| mapping.test.ts | 16 | 16 | 0 |
| matrix.test.ts | 78 | 78 | 0 |
| messages.test.ts | 5 | 5 | 0 |
| meta.test.ts | 2 | 2 | 0 |
| overlay.test.ts | 5 | 5 | 0 |
| perf.test.ts | 1 | 1 | 0 |
| probes.test.ts | 10 | 10 | 0 |
| protocol.test.ts | 30 | 30 | 0 |
| rewrite.test.ts | 24 | 24 | 0 |
| server.test.ts | 9 | 9 | 0 |
| symbols.test.ts | 21 | 21 | 0 |
| **合计** | **339** | **339** | **0** |

### 5.5 静态盘点

- 21 个 `*.test.ts` + `support.ts`（无 `test()`）。
- 274 个静态 `test()` 声明：273 个顶层字面标题 + 1 个 matrix 循环体动态标题。
- 1 个动态标题展开 33 个 scenario × 2 个 layout = 66 个注册。
- 339 个注册 = 273 个字面注册 + 66 个动态注册。
- 33 个 scenario；matrix 文件 78 个注册（12 个顶层声明 + 66 个展开）。
- 1145 个直接 `assert.*` 调用点；按领域关键词判定 930 个为语义断言。循环会放大实际执行次数；helper 内的断言（如 `completion.test.ts` 的 `throwsCode`）未计入。
- `support.ts` SHA-256：`9b4fcb347885dbeff1d1711b5f606aa48586cdf86b291ee5f868b36fbc4cef36`。

与计划对账：计划 273（272 字面 + 1 动态）→ 338；实际 274（273 字面 + 1 动态）→ 339。差异来自 `events.test.ts`：计划表写 23，实际 24。matrix 的 13 个声明 / 78 个注册与计划一致，但计划“12+66”把循环体测试误算为字面声明。

## 6. 夹具盘点与 hash（v1，已验证）

在 HEAD `b9616c5`、工作区仅有 docs 未跟踪文件时复算（2026-11-17 22:40:52 +08:00）。**不再沿用计划中的 hash**：计划值陈旧且未定义路径前缀/排序/NUL 约定，不能作为 F2 基线。

### 6.1 算法 v1

- root 为 worktree 根；
- 只接受 regular file；遇到 symlink 直接失败（本次四棵树 symlink 数为 0）；
- 相对路径使用 `/` 分隔、无前导 `./`；
- 按相对路径的 UTF-8 字节排序；
- 对每个文件依次更新 SHA-256：`rel_path_utf8 + NUL + raw_file_bytes + NUL`；
- 最终输出小写 hex。

脚本：`/tmp/opencode/p0-p1b-20261117/hash_trees.py`，SHA-256 `b54fcad3b0c6f91eb3e22256d42404619c6726d3d663cf2b23fe9388658e46e4`。

```python
#!/usr/bin/env python3
import hashlib, json, os
ROOT = "/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration"
TREES = ["backends/typescript/test", "backends/typescript/samples",
         "tests/fixtures", "crates/wxml-lsp/tests/fixtures"]

def tree_hash(rel_tree):
    base = os.path.join(ROOT, rel_tree)
    entries = []
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames.sort()
        for name in sorted(filenames):
            full = os.path.join(dirpath, name)
            if os.path.islink(full):
                raise SystemExit(f"symlink not allowed: {full}")
            rel = os.path.relpath(full, base).replace(os.sep, "/")
            entries.append((rel.encode("utf-8"), rel, full))
    entries.sort(key=lambda item: item[0])
    digest = hashlib.sha256(); total = 0
    for _, rel, full in entries:
        data = open(full, "rb").read()
        total += len(data)
        digest.update(rel.encode("utf-8") + b"\0" + data + b"\0")
    return {"tree": rel_tree, "files": len(entries), "bytes": total,
            "sha256": digest.hexdigest()}

print(json.dumps({"trees": [tree_hash(t) for t in TREES]}, indent=1))
```

执行命令：

```sh
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
python3 /tmp/opencode/p0-p1b-20261117/hash_trees.py > /tmp/opencode/p0-p1b-20261117/fixture-hashes-1.json
python3 /tmp/opencode/p0-p1b-20261117/hash_trees.py > /tmp/opencode/p0-p1b-20261117/fixture-hashes-2.json
diff -u /tmp/opencode/p0-p1b-20261117/fixture-hashes-1.json /tmp/opencode/p0-p1b-20261117/fixture-hashes-2.json
```

两次输出完全一致（`DETERMINISTIC_OK`）。

### 6.2 验证值

| 树 | 文件数 | 字节 | SHA-256 (v1) |
| --- | ---: | ---: | --- |
| `backends/typescript/test` | 22 | 253965 | `630a16af41623c78f19f47bcaba1a7f1282201ac27b4414115ac0b3764afc32e` |
| `backends/typescript/samples` | 35 | 6069 | `00a7fa3d6962da50581c396f73c13f7ffb043efae060a0801b373928726e5c1f` |
| `tests/fixtures` | 136 | 61928 | `7c9ddedda18c70bc217af6b3bbd23dd22a33be9c885276cbd587adc0596f04a9` |
| `crates/wxml-lsp/tests/fixtures` | 43 | 6456 | `834eea4defcb7565c4b84e91a173079cf3be85201d44fb4a7a6d0a32af3da073` |

文件数与计划一致；hash 以本表为准。任何 fixture 编辑后必须重算并更新 `docs/plans/tsgo-test-ledger.json` 的 `fixture_trees`。

## 7. Dirty 行为增量

计划第 2 节记录 19 个 dirty 路径；在 BASE 中它们已是提交：

```text
4d59ca7 fix: resolve component targets and improve WXML hover metadata
19 files changed, 1606 insertions(+), 563 deletions(-)
```

文件列表与计划第 2 节一致：`backends/typescript/src/{analysis,behaviors,compiler,events,protocol}.ts`、`test/{analysis,compiler,events}.test.ts`、`crates/wxml-lsp/src/backend_node/{request,response,tests}.rs`、`completion/{catalog,static_items}.rs`、`handlers/hover.rs`、`metadata/{docs,tests}.rs`、`server.rs`、`tests/component_intelligence.rs`、`wxml-project/src/discovery.rs`。

七项必保增量与落点：

| ID | 增量 | 证据 | 迁移落点 |
| --- | --- | --- | --- |
| D1 | Behavior data spread 展开后仍为 boolean/Supported | `analysis.test.ts:380`（新增） | native behavior parity + checker assignability |
| D2 | 别名 Behavior handler 定义回原文件、保留中文文档；缺失别名 Partial 而非伪造 missing-handler | `events.test.ts:425/461/498`（新增），`:188/:236`（修改） | native events/behaviors parity |
| D3 | 自定义 typeRoots 不挤掉固定声明 | `compiler.test.ts:132`（新增）、`analysis.test.ts:93` | native project + fixed typings asset |
| D4 | 有效 hover 不追加降级文案 | `handlers/hover.rs:639` 测试，`:662` 新断言 | 保留 Rust 单测 + native hover parity |
| D5 | 中文元数据/官方链接/枚举 | `metadata/tests.rs:328/352`、`server.rs:1302` | native hover docs parity |
| D6 | unknown attribute 路由 | `metadata/tests.rs:370/230` | native components/hover |
| D7 | root component 回退 | `discovery.rs:2010/2057/2087` | wxml-project 单测 + native project/model parity |

D4–D7 没有对应 TS 测试；不能因为“旧 TS 没测”而删除。逐用例的 `dirty_increments` 与 `dirty_increment_summary` 在 `docs/plans/tsgo-test-ledger.json`。

## 8. P0 出口状态（G0）

已满足：

- BASE 固定：分支、HEAD、工具版本、初始干净状态、worktree 列表。
- 四个 Rust 门禁实际执行，退出码与计数保存。
- TS 套件实际执行，失败精确记录。
- 23 个 ignored、2 个 Rust 静默 return、3 个 TS 条件 skip 已登记。
- 19 个 dirty 路径已定位为 `4d59ca7`，七项行为增量有权属与落点。
- 夹具文件数与 hash 已用统一 v1 算法复核（§6）；计划 hash 不复用。

未满足 / 不可声称：

- 没有 `backends/typescript/dist/server.js`，23 个 ignored 与 2 个真实 LSP 测试未实际运行；不能声称真实 Node E2E 通过。
- 没有原生 tsgo 资产/进程；P1-A/P1-C 的能力、FS、查询、离线资产尚未验证。
- 旧 TS **全量**套件 1 个失败（perf，338/339），不是全绿基线；perf isolated 3/3 通过，属负载相关门禁，仍需 P2 前的独立决定。
- 未运行计划第 7 节的 `--locked`、`--doc`、`cargo doc -D warnings`、`llvm-cov`、`audit`、`deny`。
- worktree 有并发 agent 写入；P2 冻结前必须重新检查 HEAD/status。
- 37 个旧 `obsolete` 候选（33 个 symbol-bridge matrix + matrix B-only 2 + protocol B-only 1 + meta C1 合同 1）已改为 `blocked-decision`，等待集成负责人决定；没有任何语义/安全/FS 用例被删除。

因此 G0 的“行为归属、BASE 可重放、同一 BASE”已记录；但“独立集成基线可重放”仍需在 P2 前用一次干净的 status/HEAD 复核确认。本文件不把 P0 写成 G0 全部通过。

## 9. 复现命令

```sh
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration

cargo fmt --all -- --check
cargo check --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features

# 旧 TS 套件（仅 P1 证据；需要 Node 24 与已存在的 node_modules）
cd backends/typescript
npm test
```

临时日志只在 `/tmp/opencode/p0-p1b-20261117/`；**本任务**在项目目录只新增三个账本/证据文件，未生成其他临时文件。并发 P1-A/P1-C 另写了三个 evidence 文件，已在 §2.2 列出，不属于本任务。`backends/typescript/node_modules` 为 gitignored 的原样复制，不属于提交内容。
