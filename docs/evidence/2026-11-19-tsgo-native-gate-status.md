# tsgo native gate 台账提升（2026-11-19）

## 0. 记录身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 分支：`tsgo/migration`
- 台账记录锚点：HEAD `7bdab9647c58bee818e8efccf2137d876d383563`（`docs(evidence): plan native event-handler completion`，2026-11-19 21:27:58 +0800）
- 运行证据锚点：`953ee1efc00b2e65d74e7b17276afd73181d9649`（`feat(lsp): consume native hover evidence`，2026-11-19 20:03:43 +0800），即 `docs/evidence/2026-11-19-tsgo-hover-gates.md` §4 记录的 3 passed / 36 filtered 运行
- 记录日期：2026-11-19
- 方式：只改测试台账（`docs/plans/tsgo-test-ledger.json`）、ledger validator（`crates/wxml-lsp/tests/native_parity/native_gates.rs`）和证据文档；未修改生产代码、测试行为、`#[ignore]` 属性、Node、默认后端或 `Cargo.toml`；未提交。
- 工作树状态：记录时存在并行 type-hover/completion 会话的未提交改动（`crates/wxml-lsp/src/backend_tsgo/{environment.rs,environment/,mod.rs,session.rs,input.rs,engine.rs,sandbox.rs}`、`crates/wxml-lsp/src/completion/{catalog.rs,expression.rs}` 等），不属于本次台账证据；§3 记录它们对复跑与 clippy 的阻塞。
- 历史快照 `docs/evidence/2026-11-18-tsgo-p6-gate-conversion-plan.md` 未被改写；本记录只追加当前台账状态。

**结论：台账从 0/23 推进到 3 `gate_green` / 20 `required_not_migrated` / 23 total；提升仅覆盖 `G-P5-E2E-09/12/14`，且只代表 opt-in required native job（`--ignored`）绿，不是默认 CI 绿，也不是 `mandatory_green`。**

---

## 1. 状态计数与提升对象

| 状态 | 数量 | 说明 |
| --- | --- | --- |
| `gate_green` | 3 | 仅在 `953ee1e` 的 opt-in required native job 中实际通过 |
| `required_not_migrated` | 20 | 未验证或未迁移，保持原状 |
| 合计 | 23 | 未删除任何 gate |

提升的三个 gate（ledger /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json:1321 、 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json:1398 、 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json:1456 ）：

| Gate | Replacement | 结果 |
| --- | --- | --- |
| `G-P5-E2E-09` | `native_ac011_unknown_owner_degrades_honestly` | 3 passed 批次内通过 |
| `G-P5-E2E-12` | `native_static_wxs_and_template_semantics_work_over_stdio` | 3 passed 批次内通过 |
| `G-P5-E2E-14` | `native_unsaved_wxs_overlay_updates_static_hover_and_completion` | 3 passed 批次内通过 |

三个 replacement 仍带 `#[ignore]`；默认 `cargo test` 只报告 ignored，不执行它们。其余 20 个 gate 的 `status` 仍是 `required_not_migrated` 且 `must_migrate=true`。

---

## 2. 台账证据字段

每个 `gate_green` 条目新增以下字段，validator 强制非空或匹配 pin：

- `replacement_test`：`crates/wxml-lsp/tests/native_parity/native_engine_real.rs::<replacement>`
- `replacement_kind`：`native_engine_real_stdio`
- `native_prerequisite`：pinned TypeScript `7.1.0-beta` native engine + engine `lib/` assets；`WXML_TEST_NATIVE=1` 与 `WXML_REQUIRE_REAL_ENGINE=1`；opt-in `--ignored` native job
- `removal_condition`：只有在该 gate 达到 `mandatory_green` 后的 P7 才可删除旧 `#[ignore]` 测试；P6-2 不删除 gate
- `gate_evidence`：命令、日期、运行 HEAD、记录 HEAD、结果、退出码、范围、引擎身份、资产身份，以及 fail-closed RED 证据

`gate_evidence` 的公共值：

```bash
WXML_TEST_NATIVE=1 \
WXML_REQUIRE_REAL_ENGINE=1 \
TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
cargo test -p wxml-lsp --test native_parity native_engine_real \
  -- --ignored --test-threads=1
```

| 项 | 值 |
| --- | --- |
| `run_date` | `2026-11-19` |
| `run_head` | `953ee1efc00b2e65d74e7b17276afd73181d9649` |
| `recorded_at_head` | `7bdab9647c58bee818e8efccf2137d876d383563` |
| `result` | `3 passed / 0 failed / 36 filtered` |
| `exit` | `0` |
| `engine_identity.version` | `7.1.0-beta` |
| `engine_identity.package` | `@typescript/typescript-linux-x64` |
| `engine_identity.git_head` | `3e9236361fa97a9e6c685f4833ac457ae2d8c50a` |
| `engine_identity.ast_protocol` | `9` |
| `engine_identity.executable_sha256` | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| `engine_identity.executable_size_bytes` | `24522912` |
| `asset_identity.tsgo_asset_root` | `/tmp/opencode/tsgo-native-pkg/engine` |
| `asset_identity.standard_library` | `engine/lib` 的 116 个 `lib*.d.ts` 被复制进一次性 gate package |
| `asset_identity.typings_payload` | 未使用；miniprogram-api-typings payload 不在本证据范围内 |
| `fail_closed_evidence.result` | `0 passed / 3 failed / 36 filtered`（不带 `WXML_TEST_NATIVE=1` 的 RED 运行） |
| `fail_closed_evidence.reason` | 各 gate panic：`WXML_TEST_NATIVE=1 is not set; the native gate job must request it explicitly` |

`36 filtered` 表示该过滤命令没有运行其他 36 个 parity 测试（含 ledger/gate guard），不是“其余测试通过”。

Validator（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:142 ）强制：

- `gate_green == 3`、`required_not_migrated == 20`、总数 `23`；
- `gate_green` 集合精确等于 `G-P5-E2E-09/12/14`，其他 gate 不得标绿；
- 每个 green 的 `replacement_test`/`replacement_kind`/`native_prerequisite`/`removal_condition` 非空；
- `gate_evidence.command` 同时含 `WXML_TEST_NATIVE=1`、`WXML_REQUIRE_REAL_ENGINE=1`、`native_engine_real`、`--ignored`；
- `run_head`/`recorded_at_head` 是 40 位十六进制 git object id；
- `engine_identity` 的 version/git head/SHA-256/AST protocol/size 与 `PinnedEngine::pinned()` 和本地 pin 常量一致；
- `asset_identity` 非空；
- `fail_closed_evidence` 的 command 不带 `WXML_TEST_NATIVE=1`、result 记录 `0 passed / 3 failed`。

---

## 3. 本次运行结果

| 命令 | 结果 | 说明 |
| --- | --- | --- |
| `cargo fmt --all -- --check` | 通过（exit 0） | 只读检查 |
| `cargo test -p wxml-lsp --test native_parity` | 36 passed / 3 ignored / 0 failed | 默认不运行 ignored native gate；ledger validator 已执行 |
| 3 个 native gate 的 fail-closed RED 命令（不带 `WXML_TEST_NATIVE=1`） | 0 passed / 3 failed / 36 filtered | 三个 gate 分别 panic `WXML_TEST_NATIVE=1 is not set; the native gate job must request it explicitly`；证明缺失 opt-in 时不是 skip/伪绿 |
| `cargo clippy -p wxml-lsp --tests -- -D warnings` | **BLOCKED**（exit 101） | 并行 completion WIP： /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/expression.rs:153 的 `clippy::comparison_chain`，以及 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/catalog.rs:1526 与 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/completion/catalog.rs:1530 的 `E0277`/`E0308` |
| 3 个 native gate 的 required 命令（§2） | **BLOCKED**：0 passed / 3 failed / 36 filtered | 当前工作树返回 `error: native type assets are unavailable: a required type declaration is missing`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:122 包装 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/environment.rs:134 ）；由并行未提交的 trusted type-assets handoff 引入，gate fixture 只有 engine `lib/` 资产，没有新的 required typings |

因此：

- 台账的 `gate_green` 证据仍锚定 `953ee1e` 的执行会话记录（`docs/evidence/2026-11-19-tsgo-hover-gates.md` §4），不是本次工作树复跑结果。
- 本次复跑没有复现绿色；在并行 WIP 合入或 gate fixture 补齐 required typings 之前，当前工作树不能声称 gate 可复跑为绿。
- `cargo test -p wxml-lsp --test native_parity` 的 36 passed 只覆盖默认（非 ignored）parity 测试，不含 3 个 native gate。

---

## 4. 限制与未关闭项

1. **默认 CI 不跑这 3 个 gate。** 它们仍是 `#[ignore]`；默认 `cargo test` 只报告 3 ignored。`gate_green` 是 opt-in required native job 状态，不是 `mandatory_green`。
2. **20 个 gate 仍 `required_not_migrated`。** 没有删除、合并或改名任何 gate；总数仍是 23。
3. **默认后端仍是 Node，Node 未删除。** `--backend native` 只是 opt-in；P6-3/P6-4 未完成。
4. **P6、G5、G6 均未关闭。** F3 也未关闭；本记录不构成 F3 资产验收。
5. **case 级状态未变。** ledger 仍为 339 total / 33 `green` / 270 `planned` / 36 `retired-with-decision`。`G-P5-E2E-09`/`G-P5-E2E-14` 关联的 `TS-COMPLETION-0207`、`TS-ANALYSIS-0297`、`TS-OVERLAY-0023`、`TS-ANALYSIS-0161` 仍是 `planned`；`G-P5-E2E-12` 没有 `related_ledger_cases`，只有 gate-local evidence。
6. **`#[ignore]` reason 文本仍是旧标签。** 三个测试的 reason 仍写 `BLOCKED by P6-3 native capabilities`，与 `gate_green` 冲突；本次不修改测试体，留待 S8 校正。`native_engine_real.rs` 顶部“ledger statuses stay required_not_migrated”的注释同样过期，属于已知标签漂移。
7. **平台/UI 未扩展。** 只有 Linux x86_64 的既有运行；Windows/UNC、macOS、编辑器运行时未验证。
8. **证据等级 E0。** 原始 stdout 未入库，记录者未在干净 HEAD 上复跑出绿色（见 §3）；结论只能从仓库源码、pin 和既有运行记录复核。
9. **阶梯状态未逐级落账。** 本次按任务指定的 `gate_green` 记录；fail-closed RED 已作为 `gate_evidence.fail_closed_evidence` 记录（0 passed / 3 failed），但没有把 `replacement_authored`/`replacement_red` 写成独立台账状态，也不声称 `mandatory_green` 或 `replacement_reviewed`。

---

## 5. 复跑条件

1. pinned TypeScript `7.1.0-beta` native 引擎（`lib/tsc`），SHA-256 为 `a183b195…`；
2. `TSGO_BIN` 指向该引擎，`TSGO_ASSET_ROOT` 指向含 `lib/*.d.ts` 的引擎根；
3. `WXML_TEST_NATIVE=1` 与 `WXML_REQUIRE_REAL_ENGINE=1`；
4. `--ignored --test-threads=1`；
5. 工作树不得含改变 native type-assets 前置条件的未提交改动（§3 的阻塞）。

---

## 6. 结论

**P6-2 native gate 台账：3 `gate_green`（`G-P5-E2E-09/12/14`）/ 20 `required_not_migrated` / 23 total。这 3 个 green 是 `953ee1e` 上 opt-in required native job 的既有结果；默认 CI、默认 Node 后端、P6、G5、G6、F3 和其余 20 个 gate 均未因此改变。**
