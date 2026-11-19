# P7 stdio 9 gate retarget 证据（2026-11-20）

## 0. 结论先行

1. **G-P5-E2E-06..14 已 retarget 为 native 薄壳。** 9 个函数名、文件、`#[ignore]`
   坐标不变；每个壳用 disposable native package 启动一次真实
   `<package>/bin/wxml-lsp --stdio --backend native`，保留该 gate 的 signature
   断言，并以 `shutdown`/`exit`、非零退出失败、无 Node exec、tripwire 未触发收尾。
2. **深度 oracle 没有复制。** 9 个 gate 的完整断言仍在
    `crates/wxml-lsp/tests/native_parity/native_engine_real.rs`；本文件只保留
   “独立测试目标 + fresh 进程 + clean shutdown” 的 gate 身份。
3. **runner 已从 26 扩到 35。** 23 native engine + 3 symbols/edits + 9 stdio；
   `run_gate` 支持 `--test <target>`；缺包仍 exit 2；实测 35/35 PASS。
4. **冻结证据已重新生成。** 9 个 stdio gate 的 `file_sha256`、`line_ignore`/
   `line_fn`、`node_dependency` 和 `WXML_TEST_NODE` gate-only env 同步；12 delete /
   11 retarget 分类与 `p7_action` 未变。
5. **仍阻塞 2 个 retarget：** `G-P5-PERF-01`（performance_bench.rs）和
   `G-P5-STABILITY-01`（q2_stability.rs）仍依赖 Node，未在本切片处理。
6. 未改生产代码、ledger、README、delivery；未新增测试文件或 `#[ignore]`
   函数；未 commit、未 review。

## 1. 身份、边界与并行工作树

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`。
- 任务开始 HEAD：`1c89338`。实施期间并行会话提交了
  `befba03 fix(tsgo): accept behavior trailing commas and backfill parity`，因此实际
  冻结基线落在 `befba03e60f9c65e20e75b0c4c873d828fcb61b3`。
- 本切片只修改：
  - `crates/wxml-lsp/tests/stdio_type_loop.rs`
  - `scripts/native-gates.sh`
  - `scripts/p7-freeze.sh`
  - `docs/evidence/data/legacy-p7-freeze.json`
  - 本文
- 未触碰并行会话的 `crates/wxml-lsp/tests/native_parity.rs`、
  `scenario_matrix.rs`、未跟踪的 `real_matrix.rs`/fixtures 等。

## 2. 9 个 stdio gate 的薄壳

每个薄壳共享同一组 harness（`#[path]` 复用，不复制 package builder）：

```text
crates/wxml-lsp/tests/stdio_type_loop.rs:44  native_gate_env
crates/wxml-lsp/tests/stdio_type_loop.rs:46  native_lsp
crates/wxml-lsp/tests/stdio_type_loop.rs:48  native_package
```

本地 `mod ledger` 只提供 `workspace_root()`，满足
`native_gate_env.rs` / `native_package.rs` 的唯一 `crate::ledger` 依赖。

| Gate | 函数（行） | 薄壳保留的 signature 断言 | 深度 oracle |
| --- | --- | --- | --- |
| G-P5-E2E-06 | ` ac001_page_data_and_jsdoc_hover`（112） | 空 diagnostics；`user.name` hover 含 `string` + `用户显示名` | ` native_engine_real.rs:1042` |
| G-P5-E2E-07 | ` ac002_field_typo_diagnostic_and_fix_clear`（149） | `nmae` 恰好 1 条 error 且含 `does not exist`；修复后清空 | ` native_engine_real.rs:1148` |
| G-P5-E2E-08 | ` ac003_unicode_and_crlf_positions`（189） | 2 条 error；UTF-16 列 13/17 与 12/16；行 0/1 | ` native_engine_real.rs:1203` |
| G-P5-E2E-09 | ` ac011_unknown_owner_degrades_honestly`（230） | 1 条 degradation warning；hover 为空；无 error | ` native_engine_real.rs:225` |
| G-P5-E2E-10 | ` ac013_component_properties_hover`（280） | 无 error；Partial warning 含 “registration has no `data` member”；`title` hover string | ` native_engine_real.rs:1255` |
| G-P5-E2E-11 | ` component_behavior_data_and_properties_work_over_stdio`（353） | 空 diagnostics；`sharedCount` number + `共享计数`；definition 指向 `card.ts`；`cardTitle` 为 string 或 number | ` native_engine_real.rs:2867` |
| G-P5-E2E-12 | ` static_wxs_and_template_semantics_work_over_stdio`（435） | WXS hover 含 `utils.format`/`=> string`/JSDoc；definition 行 1；template hover；`is="sh"` 与 `utils.` 补全 | ` native_engine_real.rs:288` |
| G-P5-E2E-13 | ` static_wxs_answers_survive_an_unavailable_type_backend`（531） | 启动后删除 `<package>/engine/lib/tsc`；unavailable warning；静态 WXS hover/definition 存活；typed hover 为空 | ` native_engine_real.rs:1380` |
| G-P5-E2E-14 | ` unsaved_wxs_overlay_updates_static_hover_and_completion`（614） | disk string → overlay number → 补全 `renamed` → close 恢复 string + `label` | ` native_engine_real.rs:409` |

共同边界断言（每个函数体末尾）：

```text
session.assert_no_node_exec();
session.shutdown();          // shutdown/exit + 非零退出失败 + 禁止 detach worker
gate.tripwire().assert_absent();
```

- 使用 `NativeGate::require("G-P5-E2E-0X")`；无 `WXML_TEST_NATIVE=1` 直接失败，
  `WXML_REQUIRE_REAL_ENGINE=1` 时缺包/缺 engine 直接 panic。
- 子进程环境清空 Node 变量、注入 poison 值、PATH 只含 tripwire shim；与
  `native_engine_real` 使用同一个 `native_lsp::Session`。
- 不复制 06 的 `user` 形状/range、12 的第三段 data completion、13 的
  no-sandbox、14 的磁盘哈希；这些仍在 replacement 中。

## 3. runner：26 → 35

` `scripts/native-gates.sh:53` 起：

- `NATIVE_ENGINE_GATES=23`、`NATIVE_EDIT_GATES=3`、`NATIVE_STDIO_GATES=9`；
  `EXPECTED_NATIVE_GATES=35`。
- ` `scripts/native-gates.sh:329` 的 `run_gate` 现在接收
  `--test <target>` 与可选 filter；stdio 组用
  `--test stdio_type_loop`（无 filter）。
- 新增日志 `03-stdio-type-loop.log`；`--verify-no-node` 日志顺延为
  `04-verify-no-node.log`。
- fail-closed 未变：包目录/engine/typings/identity 任一缺失或非法 exit 2；
  build/gate 失败 exit 1；35/35 且可选 no-node 检查通过才 exit 0。
- `WXML_TEST_NATIVE_PACKAGE` 仍故意不导出：E2E-13 会删除自己 fixture 的
  `engine/lib/tsc`，每个 gate 必须构建独立 package。

## 4. freeze 重新生成

` `scripts/p7-freeze.sh`：

- 新增显式 `--allow-dirty`：只用于“故意未提交的 retarget 切片”生成；
  `--check` 仍按当前工作树比对，且不允许与 `--check` 同用。
- 生成时记录 `commit.frozen_paths_clean` 的实际值；本切片为 `false`
  （stdio 文件与并行会话的 `native_parity.rs` 未提交）。
- `DEFAULT_REQUESTED_HEAD` 更新为 `befba03e…`（并行提交后的实际基线）。
- `node_dependency_json` 对 `stdio_type_loop.rs` 改为
  `{p7_state:"retargeted-native", ...}`，不再声称 Node backend。
- `GATE_ONLY_ENV_JSON`：
  - `WXML_TEST_NODE` 只剩 `real_node.rs`、`wave5_acceptance.rs`、
    `performance_bench.rs`、`q2_stability.rs`；
  - `WXML_TEST_EXTERNAL_BACKEND` 只剩 `real_node.rs`。

` `docs/evidence/data/legacy-p7-freeze.json` 由脚本重新生成：

- `old_ignored_gates.summary.by_action`：`delete: 12`、`retarget: 11`（未变）。
- 9 个 stdio gate 的 `p7_action` 仍为 `retarget`。
- 9 个 `file_sha256` 全部更新为
  `e51899c291eded7ec0f7f52fa3619c1d6791331a20c40f7363b1ac13dfdd4bc3`。
- `line_ignore`/`line_fn` 更新为 111/112、148/149、188/189、229/230、
  279/280、352/353、434/435、530/531、613/614。
- `node_dependency.p7_state = "retargeted-native"`，`opt_in = "WXML_TEST_NATIVE=1"`，
  `env` 为 `WXML_TEST_NATIVE`/`WXML_REQUIRE_REAL_ENGINE`/`TSGO_BIN`/
  `TSGO_ASSET_ROOT`/`TSGO_TYPINGS_ROOT`。
- 其余 gate（PERF/STABILITY 与 12 个 delete gate）仍保留原有的 Node
  dependency 描述。

## 5. 验证命令与结果

环境：`/tmp/opencode/tsgo-native-pkg`（engine `7.1.0-beta`，
sha256 `a183b195…`，size `24522912`）。

| 命令 | 结果 |
| --- | --- |
| `bash -n scripts/native-gates.sh` | 通过 |
| `bash -n scripts/p7-freeze.sh` | 通过 |
| `rustfmt --edition 2024 --check crates/wxml-lsp/tests/stdio_type_loop.rs` | 通过 |
| `cargo clippy -p wxml-lsp --test stdio_type_loop -- -D warnings` | 通过 |
| `cargo test -p wxml-lsp --test stdio_type_loop --no-run` | 通过 |
| `WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 TSGO_BIN=… TSGO_ASSET_ROOT=… TSGO_TYPINGS_ROOT=… cargo test -p wxml-lsp --test stdio_type_loop -- --ignored --test-threads=1` | `9 passed; 0 failed` |
| `scripts/native-gates.sh --package /tmp/opencode/tsgo-native-pkg` | `PASS 35/35`（23/23 + 3/3 + 9/9） |
| `scripts/native-gates.sh --package /tmp/opencode/does-not-exist-tsgo-pkg` | exit 2，明确报包目录不存在 |
| `scripts/p7-freeze.sh --check` | OK（`befba03`；工作树有未提交 frozen path，按 `--check` 规则提示后通过） |
| `cargo test -p wxml-lsp --test native_parity` | `78 passed; 0 failed; 29 ignored` |
| `cargo test -p wxml-lsp` | 全绿（各 target 均 `0 failed`；native_parity `78 passed / 29 ignored`，stdio target `9 passed`（`native_package` 单测）+ `9 ignored`） |

## 6. 阻塞与未完成

1. **`G-P5-PERF-01` 仍 blocked。** `performance_bench.rs` 仍导入
   `wxml_lsp::backend_node`，缺 static/semantic completion 拆分、queued/running
   cancel、occupancy、baseline JSON 与环境 provenance。按重定计划 R2 原地重写。
2. **`G-P5-STABILITY-01` 仍 blocked。** `q2_stability.rs` 仍依赖 NodeBackend；
   缺 typing burst + in-flight cancel、40 请求 bounded queue burst、资源删除/恢复、
   app.json overlay、template close/reopen、外部 RSS baseline。按 R3 原地重写。
3. **12 个 `p7_action=delete` gate 仍 blocked。** 本切片不删除旧测试；P7-3 仍要
   等 PERF/STABILITY retarget 完成后再评估。
4. **平台边界。** 9 个 stdio gate 现在与 native engine 一样只支持 Linux x86_64；
   旧 Node 路径的跨平台证据不再适用。
5. **CI 文案未在本切片更新。** `.github/workflows/ci.yml` 的 step 名仍写
   “Run the 26 native gates”；本切片限定只改测试/脚本/冻结证据/证据文档，
   该文案留到 runner/CI 同步切片处理，不影响 `scripts/native-gates.sh` 的 35/35
   行为。
6. **freeze 的工作树状态。** `frozen_paths_clean=false` 是当前未提交切片的
   真实状态。提交本切片后需要重新生成一次 freeze JSON，才能把该字段恢复为
   `true`；在那之前 `--check` 只在 dirty 工作树上成立。
7. **并行工作树。** 本切片期间并行会话提交了 `befba03`，并继续修改
   `native_parity.rs`、`native_gates.rs`、`scenario_matrix.rs` 与未跟踪的
   `real_matrix.rs`。freeze JSON 是这些当前工作树状态的快照；本切片不声称
   这些文件或计数变更由 retarget 引入。

## 7. 明确不声称

- 不声称 9 个 stdio gate 新增了覆盖；它们只保留 gate 身份和进程边界。
- 不声称 PERF/STABILITY 已 retarget 或等价。
- 不声称并行会话的 `real_matrix`/`native_gates` 变更属于本切片。
- 不声称 12 个 delete gate 可以删除，或 P7-3 已就绪。
- 不声称 `backend_node` 已无消费者；本切片只改了 3 个文件中的 1 个。
