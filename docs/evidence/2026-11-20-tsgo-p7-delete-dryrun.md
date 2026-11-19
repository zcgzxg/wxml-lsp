# P7 删除演练：12 个 delete gate（2026-11-20，基线 `8a56f27`）

## 0. 结论先行

1. **测试层面已验证 12 个 `delete` gate 可删除。** 在干净拷贝中删除
   `crates/wxml-lsp/tests/real_node.rs`（5 个）和
   `crates/wxml-lsp/tests/wave5_acceptance.rs`（7 个），并把 ledger 的对应
   12 个 gate 标为 `old_test_removed: true` 后：
   - `native_parity`：`75 passed / 0 failed / 26 ignored`；
   - `scripts/native-gates.sh`：`PASS 26/26`（23 native engine + 3 native edit）；
   - 默认 `cargo test -p wxml-lsp`：`1246 passed / 0 failed / 37 ignored`。
2. **必须先进入 Frozen oracle。** 删除后若 live `backends/typescript/test` 仍在，
   `native_parity` 按设计失败：
   `G-P5-E2E-01 is marked old_test_removed but the legacy oracle manifest is not frozen`。
   把该目录移出后同一套测试通过；manifest 内容本身不需要修改。
3. **11 个 `retarget` 不能删除。** 9 个 `stdio_type_loop` 场景虽都有 native
   替代，但删除会失去 opt-in 的工作区 debug binary stdio 进程边界覆盖；`performance_bench`
   和 `q2_stability` 的 native 替代**不等价**，会分别丢失取消/慢调用占用、
   10 轮 server 生命周期/泄漏覆盖。
4. **隐藏依赖主要在证据工具链，而不是测试代码。** `scripts/p7-freeze.sh`
   硬编码了这两个文件和 23 个 gate 的行号/锚点；删除后 `--check` 必然失败，
   必须与 `docs/evidence/data/legacy-p7-freeze.json` 一起更新。
5. **`old_test_removed` 路径可用，但没有 delete/retarget 区分。** 它只校验
   `gate_green`、replacement、Frozen oracle 和 related cases；11 个 retarget
   gate 若被删除并标记，同样会通过。删除候选必须由评审或新增字段约束。

---

## 1. 实验身份与边界

- 主工作区：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`。
- 任务基线：`8a56f27`（`docs: sync F3 closure and native default status`）。
  实验期间共享工作树因并行提交推进到 `c53d9ab`（写作时）；本演练的所有输入都用
  `git archive 8a56f27` 固定，不受这些并行改动影响。
- 拷贝：`/tmp/opencode/p7-delete-dryrun`（无 `.git`、无 `target/`、
  无 `backends/typescript/dist`、无 `node_modules`）。
- 主工作区只新增本文件；未改代码、README、ledger、freeze JSON 或脚本，
  未 commit，未 review，未执行 `git clean/stash/reset`。
- 构建缓存放在拷贝的 `target/`；因 `/tmp` 是 tmpfs，debug 测试使用
  `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_INCREMENTAL=false`，
  只影响调试信息/增量，不影响测试语义。

---

## 2. 删除清单（12 个）

`p7_action=delete` 的两个文件都只包含这些旧 gate 和它们的私有 helper；
删除 gate 后文件没有剩余测试，因此整文件删除。

| Gate | 文件 | fn 行 | native replacement |
| --- | --- | ---: | --- |
| G-P5-E2E-01 |  `crates/wxml-lsp/tests/real_node.rs` | 496 | `native_non_owner_overlay_replay_survives_restart` |
| G-P5-E2E-02 |  `crates/wxml-lsp/tests/real_node.rs` | 221 | `native_event_handler_hover_diagnostics_definition_completion` |
| G-P5-E2E-03 |  `crates/wxml-lsp/tests/real_node.rs` | 105 | `native_expression_and_event_completion_smoke` |
| G-P5-E2E-04 |  `crates/wxml-lsp/tests/real_node.rs` | 31 | `native_hover_and_diagnostics_smoke` |
| G-P5-E2E-05 |  `crates/wxml-lsp/tests/real_node.rs` | 627 | `native_symbols_smoke` |
| G-P5-E2E-15 |  `crates/wxml-lsp/tests/wave5_acceptance.rs` | 671 | `native_ac005_loop_scope_typed_locals` |
| G-P5-E2E-16 |  `crates/wxml-lsp/tests/wave5_acceptance.rs` | 724 | `native_ac006_conditional_union_narrowing` |
| G-P5-E2E-17 |  `crates/wxml-lsp/tests/wave5_acceptance.rs` | 1113 | `native_ac008_alias_project_hover_and_definition` |
| G-P5-E2E-18 |  `crates/wxml-lsp/tests/wave5_acceptance.rs` | 996 | `native_ac008_multi_view_fixture_isolation` |
| G-P5-E2E-19 |  `crates/wxml-lsp/tests/wave5_acceptance.rs` | 1030 | `native_ac008_simultaneous_views_do_not_pollute_each_other` |
| G-P5-E2E-20 |  `crates/wxml-lsp/tests/wave5_acceptance.rs` | 772 | `native_ac012_component_property_bindings` |
| G-P5-E2E-21 |  `crates/wxml-lsp/tests/wave5_acceptance.rs` | 847 | `native_ac012_event_handlers_basic` |

行号来自 `docs/evidence/data/legacy-p7-freeze.json`（基线 `75040c2`；
这两个文件在 `8a56f27` 未变动）。

拷贝中的配套改动（仅拷贝）：

- 删除上述两个文件。
-  `docs/plans/tsgo-test-ledger.json`：只给这 12 个 gate 增加
  `"old_test_removed": true`，其余 11 个 retarget gate 保持缺省 `false`。
- `backends/typescript/test` 移到 `/tmp/opencode/p7-archived-typescript-test`，
  用于模拟 P7-4 已归档的 Frozen 状态；`samples` 保持 live。

---

## 3. 命令与结果

在 `/tmp/opencode/p7-delete-dryrun` 执行；`native_parity` 使用拷贝的
`target/` 和 `--offline`。

| # | 命令 | 结果 |
| ---: | --- | --- |
| 1 | 删除前：`cargo test -p wxml-lsp --test native_parity` | `75 passed / 0 failed / 26 ignored`，exit 0 |
| 2 | 删除 + 12 个 flag，live TS 树仍在 | `74 passed / 1 failed / 26 ignored`，exit 101 |
| 3 | 同上，移走 `backends/typescript/test` | `75 passed / 0 failed / 26 ignored`，exit 0 |
| 4 | 负向探针：移除 `G-P5-E2E-01` 的 flag，测试仍已删除 | exit 101，报 `is not marked old_test_removed but ... is missing` |
| 5 | 默认：`cargo test -p wxml-lsp` | `1246 passed / 0 failed / 37 ignored`，23 个测试目标（21 integration + 2 unit），exit 0 |
| 6 | `scripts/native-gates.sh --package /tmp/opencode/tsgo-native-pkg` | `PASS 26/26`（23/23 + 3/3），exit 0 |

第 2 步的完整失败信息（关键行）：

```text
old_test_removed schema: G-P5-E2E-01 is marked old_test_removed but the legacy
oracle manifest is not frozen; the live TypeScript tree must be gone before an
old gate is deleted
```

第 5 步的 37 个 ignored 正好是：23 native engine + 3 native edit +
9 `stdio_type_loop` + 1 `performance_bench` + 1 `q2_stability`。
删除前按 ignored 组成推算为 49；减少的 12 个全部是本次删除的旧 gate，没有隐藏的 ignore 变化。

注意：干净 archive 没有 `backends/typescript/dist` 和 `node_modules`，
因此 `semantic_tools` 的 2 个 real Node LSP 测试走默认的
`skipping real Node LSP test` 分支后返回；`41 passed` 不证明 Node 路径。
本演练的目标是 native 套件，native-gates 使用的是
`/tmp/opencode/tsgo-native-pkg` 的 pinned engine。

---

## 4. `old_test_removed` 路径是否真的可用

可用，且正负路径都用真实树验证过：

- **正向**：12 个 gate 全部 `gate_green`、replacement 在
  `GATE_REPLACEMENTS` 中、Frozen oracle、相关 ledger cases 都在冻结
  manifest 中；`native_parity` 通过。
- **负向**：删掉一个 `old_test_removed` flag 后立即失败，说明删除不会在
  没有显式标记时被静默接受，也不会把 retarget gate 的缺失误判为通过。
- 校验逻辑在  `crates/wxml-lsp/tests/native_parity/native_gates.rs:375`
  （`validate_old_test_removals`），Frozen 前置在同文件 `:608`。
- 局限：该函数不读取 `p7_action`；11 个 retarget gate 同样满足
  `gate_green` + replacement + Frozen 条件。**“哪些 gate 允许删除”目前只由
  freeze JSON/评审约束，不由测试约束。**

遗留的注释/常量漂移（不影响本次通过，但 P7 补丁应同步）：

-  `crates/wxml-lsp/tests/native_parity/native_gates.rs:35`
  “No gate is removed by this slice.”
- 同文件 `:86` 的 `IGNORED_GATE_COUNT` 注释 “P6-2 never deletes a gate”。

---

## 5. Oracle manifest：需要 Frozen，不需要改 manifest 内容

-  `crates/wxml-lsp/tests/native_parity/oracle_manifest.rs:813`
  `verify_legacy_oracle` 在 live root 存在时走 Live，不存在时走
  `LegacySourceMode::Frozen`（`:821`）。
- `old_test_removed` 只接受 Frozen（`native_gates.rs:608`），因此
  **删除 12 个旧测试之前或同一变更中，必须让
  `backends/typescript/test` 不再存在**。
- `docs/plans/tsgo-legacy-oracle-manifest.json` 与
  `docs/plans/tsgo-test-ledger.json` 的 cases 不需要改；本次只移走 live
  tree，Frozen 校验通过。
- `backends/typescript/samples` 可以继续 live；manifest 对
  `test`/`samples` 都标记 `archived_ok=true`
  （ `crates/wxml-lsp/tests/native_parity/fixture_hash.rs:75`、
  `:82`）。

这与当前计划有顺序冲突： `docs/reference/tsgo-test-contract.md:60` 和
`docs/plans/2026-11-17-rust-tsgo-migration.md:817` 都把 live TS 树删除列为
P7-4/删除阻塞，而 `old_test_removed` 要求它在 P7-3 删除旧 Rust gate 前就消失。

---

## 6. 11 个 retarget：若删除会怎样（未实际删除）

| 组 | 数量 | 删除后果 | 结论 |
| --- | ---: | --- | --- |
| `stdio_type_loop.rs`（G-P5-E2E-06..14） | 9 | 9 个场景都有 native replacement，且 replacement 通过打包的 `--stdio --backend native` 运行；但删除会移除**工作区 debug binary + Node env（`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`/`WXML_TEST_EXTERNAL_BACKEND`）的 stdio 进程边界覆盖**。`real_node.rs` 删除后，`WXML_TEST_EXTERNAL_BACKEND` 只剩这个文件使用。 | 不删除；按计划 retarget 到 native stdio |
| `performance_bench.rs`（G-P5-PERF-01） | 1 | native perf gate 只测 hover/completion/diagnostics p95 和 RSS growth；会丢失 `cancel_queued_request` p95（100 ms 阈值）、`cancel_running_response`、`single_ts_call_occupancy`、static/semantic completion 拆分、baseline JSON 的 10% p95 / 15% RSS 回归检查，以及 node/rustc/CPU/loadavg 证据。 | 不删除；retarget 并保留独有场景 |
| `q2_stability.rs`（G-P5-STABILITY-01） | 1 | native stability gate 是在同一 server 内 12 轮 update/release/SIGKILL engine；旧测试是 **10 轮完整 server 生命周期**（typing burst、cancel、unsaved overlay、Node 子进程/RSS/线程泄漏）。两者不等价。 | 不删除；retarget 并保留 server 生命周期覆盖 |

另外，`stdio_type_loop`、`performance_bench`、`q2_stability` 三个文件都只有
这 11 个测试；若真的删除，测试文件会整体消失，而不只是少一个 case。

---

## 7. 隐藏依赖

1.  `scripts/p7-freeze.sh` 把两个待删文件写死在
   `FROZEN_PATHS`（`:117`、`:118`）、`node_dependency_json`（`:302`、`:313`）、
   gate-only env（`:496`、`:497`）和 `REFERENCE_PATHS`（`:599`、`:600`）。
   删除后 `--check` 会在 `file_sha`/行号锚点处失败；必须同步修改脚本并重新
   生成 `docs/evidence/data/legacy-p7-freeze.json`（或让新 schema 显式记录
   removed gate 并跳过源文件哈希）。
2.  `docs/evidence/data/legacy-p7-freeze.json` 的 23 gate、`old_test_present`
   和 `p7_action=delete` 清单会过期。它不被 Rust 测试读取，所以不影响
   `cargo test`，但会破坏冻结证据。
3. ledger 必须“删测试”和 `old_test_removed: true` 同时落地：先标 flag 后删
   会报 “still carries the ignore attribute”，先删后不标会报 “is missing”。
   两者都在 `native_gates.rs:399`/`:427` 硬失败。
4. 文档表格仍引用旧文件： `docs/plans/2026-11-17-tsgo-test-ledger.md` 的
   G-P5-E2E-04/05/15..21 行、 `docs/reference/tsgo-test-contract.md` 和
   `docs/plans/2026-11-17-rust-tsgo-migration.md` 的 P7 描述。没有测试强制
   同步，但提交前应更新，避免“文档仍指向已删除测试”。
5. `native_gates.rs` 的模块注释仍声称 “No gate is removed”；`GATE_GREEN_COUNT`
   仍是 23（正确），`IGNORED_GATE_COUNT` 仍是 23（正确，表示 ledger 条目数）。
   只改注释，不改计数。
6. 默认 CI 的 `native-gates` 仍是 `workflow_dispatch` opt-in
   （ `.github/workflows/ci.yml:156` 起）。26/26 通过不是 required CI 保证；
   P7-1 的 `mandatory_green` 仍未完成。
7. 删除两个测试文件后，integration 测试目标从 23 个变为 21 个（另有
   lib/main 两个 unit target）；目前没有脚本断言精确目标数量。

---

## 8. 必须先修的阻塞

1. **Frozen oracle 顺序**：先归档/删除 `backends/typescript/test`（或把它作为
   与 12 gate 删除同一提交的前置步骤），否则 `old_test_removed` 不可用。
   不要为了绕过它而放宽 `validate_old_test_removals` 的 Frozen 检查。
2. **freeze 工具链**：更新 `scripts/p7-freeze.sh` 与
   `docs/evidence/data/legacy-p7-freeze.json`，否则删除后冻结证据检查必失败。
3. **P7-1 治理**：ledger 的 23 个 gate 仍是 `gate_green`，不是
   `mandatory_green`；`replacement_reviewed` 未完成，`planned` 未归零（或需
   G5 waiver）。本演练只证明“删了测试后 native 套件仍绿”，不证明 P7-1 完成。
4. **retarget 未做**：11 个 gate 仍保留；删除它们会丢覆盖，且会绕过
   `p7_action=retarget` 的人工约束。
5. **delete/retarget 自动约束缺失**：如需要机器保证，给 ledger gate 增加
   `p7_action` 或 `removal_allowed` 字段并在 `validate_old_test_removals`
   中校验。

---

## 9. 建议执行顺序

1. 完成 P7-1：23 个 gate 的 `mandatory_green` + replacement review；
   ledger planned 归零或记录 G5 waiver。
2. 先落 Frozen：归档 `backends/typescript/test`（P7-4 的 test 子树前置），
   确认 `native_parity` 仍 `75/0/26`。
3. 同一变更删除 `real_node.rs` + `wave5_acceptance.rs`，只给 12 个 delete
   gate 加 `old_test_removed: true`，并更新 `native_gates.rs` 的过时注释。
4. 同一变更更新 `scripts/p7-freeze.sh` 和 freeze JSON，使 `--check` 能表达
   “gate 已删除、替代仍在”。
5. 跑验收： `native_parity`、 `scripts/native-gates.sh`、默认
    `cargo test -p wxml-lsp`、 `scripts/p7-freeze.sh --check`。
6. 再做 11 个 retarget：`stdio_type_loop` 换成 native stdio；
   `performance_bench`/`q2_stability` 保留独有断言后再切换 backend。
7. retarget 完成后才进入 `backend_node` 和其余旧 TS/v1 写路径删除。

---

## 10. 明确不声称

- 不声称 P7-1/P7-3/P7-4 已完成；只证明 12 个 delete gate 的删除路径在
  Frozen oracle 下可通过 native 套件。
- 不声称 11 个 retarget gate 可删除；分析结论是删除会丢覆盖。
- 不声称 `scripts/p7-freeze.sh --check` 在删除后可通过；它在本次拷贝中
  因无 `.git`/无 `dist` 未运行，但静态依赖已确认会失败。
- 不声称默认 CI 已强制运行 native gates；它仍是 opt-in job。
- 不声称主工作区当前 HEAD 仍是 `8a56f27`；实验固定使用该提交的 archive。
