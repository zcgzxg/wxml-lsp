# P7 残余 native gates 复验（q2_stability 修改后，2026-11-20）

## 0. 结论

在 HEAD `b6c1ced`（tag `p7-removed`）加本轮未提交 q2_stability 稳定性/测试/文档
修复的工作树上，以 post-delete 安装包作为 pinned engine/typings 来源，完整运行
`scripts/native-gates.sh`。

**PASS：40/40，exit code 0，build PASS，无失败 gate，无真实回归。**

| 项 | 结果 |
| --- | --- |
| 40 项 native gates | **PASS 40/40** |
| 进程 exit code | **0** |
| build | PASS（`cargo build --release -p wxml-lsp`，0.06s，复用既有产物） |
| 失败项 | **无** |
| 失败日志 | `/tmp/opencode/p7-residual-gates.log`（本次运行的完整 stdout+stderr，18 行汇总；逐 gate 明细见输出目录） |

本次 `q2_stability` 修改后未出现回归：G-P5-STABILITY-01 仍在 10 轮生命周期、
每轮 engine `SIGKILL`、4 个场景和 40 请求队列突发下通过，RSS 增长 0.219%
（上限 15%），场景 4 项 IMPLEMENTED、0 项 FAILED，外部 baseline 场景按合同保持
BLOCKED（不参与失败判定）。

## 1. 身份、时间与边界

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| 分支 | `tsgo/migration` |
| HEAD | `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b` |
| Tag `p7-removed` | `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b`（与 HEAD 相同） |
| HEAD 提交时间 | `2026-11-20T09:30:07+08:00` |
| 运行窗口（本地） | 2026-11-20 10:16:20 ～ 10:20:41 +0800（UTC 02:16:20Z ～ 02:20:41Z） |
| 平台 | Linux x86_64，24 逻辑核 |
| 工作树状态 | dirty（本轮未提交修改，见 §1.2） |
| 本切片改动 | 只新增本证据文件；未改生产/测试/脚本/README/ledger；未 commit |

### 1.1 Package 与引擎身份

`--package` 使用 post-delete 安装包（未回退到 `p7-final-install`）：

| 项 | 值 |
| --- | --- |
| Package 根 | `/tmp/opencode/p7-post-delete-install/versions/0.1.0` |
| `manifest.json` | schemaVersion 2，`wxml-lsp` 0.1.0，`linux-x86_64`，protocol 2，builtAt `2025-11-21T00:00:00Z`，157 files |
| `manifest.json` SHA-256 | `2a6cddc2969df9177b13718d16a21809c0ce6d050f28a35f199ac91cd5555677` |
| `SHA256SUMS` SHA-256 | `815fcc1fef43ce567b147b25d18d6dda45727dde72a62482e0ef47a044d44db0` |
| 引擎 | TypeScript `Version 7.1.0-beta`，source commit `3e9236361fa97a9e6c685f4833ac457ae2d8c50a` |
| `engine/lib/tsc` SHA-256 / size | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` / 24,522,912 bytes（与 runner 的 pin 完全一致） |
| Typings | `miniprogram-api-typings` 5.3.1（`typings/miniprogram-api-typings`，payload sha256 `ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b`） |
| Package 目录大小 | 41 MB |

被测二进制不是安装包里的 `bin/wxml-lsp`：gate 的 disposable package 由
`CARGO_BIN_EXE_wxml-lsp` 复制而来，即本轮 `cargo build --release` 的
`target/release/wxml-lsp`（SHA-256 `59a90738e96007dd80a45088e29d247fadc27e3196ccc8e21cb13674116f2eba`，
2026-11-20 09:14:28 +0800 构建）。安装包只提供 pinned engine 与 typings。
09:14 ～ 运行开始之间 `crates/*/src/**` 无任何改动，因此该二进制与 HEAD 源码
输入一致；runner 的 0.06s `Finished` 也确认无需重新编译。

### 1.2 dirty worktree 原因（仅本地记录）

运行前 `git status --porcelain` 为 43 项；q2_stability 报告在 02:20:24Z 记录
`git_dirty` 44 项；运行结束后为 46 项。变化来自并行会话继续写入证据文档和
`crates/wxml-delivery/src/{lib,licenses}.rs`（10:22，晚于本次运行结束；且
`wxml-lsp` 不依赖 `wxml-delivery`）。本次判定不受这些后写文件影响。

参与本轮修改、也是本次要复验的对象：

| 类别 | 内容 |
| --- | --- |
| 稳定性测试 | `crates/wxml-lsp/tests/q2_stability.rs`（+633 行场景扩展）、新增 `crates/wxml-lsp/tests/q2_stability_support/` |
| 其他测试 | `crates/wxml-backend-tsgo/tests/{asset_loader,f1_hover,type_assets}.rs`、新增 `tests/support/`、`crates/wxml-lsp/tests/native_lsp_document_symbols/support.rs` |
| 编辑器 | `editors/zed/src/lib.rs` |
| 文档 | README/ADR/plans/reference/evidence 若干 |

`WXML_STABILITY_ALLOW_DIRTY=1` 只在本次命令前显式导出，用于本地证据运行；
生产语义未改：测试默认仍要求 clean worktree，设置该变量只是显式跳过 clean
检查，不跳过 10 轮生命周期/SIGKILL/场景断言。稳定性报告记录
`strict_semantics: true`、`allow_dirty: true`、`git_dirty` 44 项。

## 2. 命令

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
WXML_STABILITY_ALLOW_DIRTY=1 scripts/native-gates.sh \
  --package /tmp/opencode/p7-post-delete-install/versions/0.1.0 \
  --output /tmp/opencode/p7-residual-native-gates-20261120T021620Z \
  > /tmp/opencode/p7-residual-gates.log 2>&1
echo $?   # 0
```

runner 自行导出 gate 子进程环境：

```text
WXML_TEST_NATIVE=1
WXML_REQUIRE_REAL_ENGINE=1
TSGO_BIN=/tmp/opencode/p7-post-delete-install/versions/0.1.0/engine/lib/tsc
TSGO_ASSET_ROOT=/tmp/opencode/p7-post-delete-install/versions/0.1.0/engine
TSGO_TYPINGS_ROOT=/tmp/opencode/p7-post-delete-install/versions/0.1.0/typings/miniprogram-api-typings
```

runner 在 build 前重新校验了 engine `--version`、SHA-256、size、asset-root
`lib/lib.d.ts` 和 typings 布局；全部通过后才开始 build 与 gate。

## 3. 分组结果

| # | Gate 组 | Cargo target / filter | 数量 | 结果 | 耗时 |
| ---: | --- | --- | ---: | --- | ---: |
| 1 | `native_parity::native_engine_real` | `native_parity` / `native_engine_real` | 23 | **PASS 23/23** | 43.73s |
| 2 | `native_parity::symbols_edits` | `native_parity` / `symbols_edits` | 3 | **PASS 3/3** | 2.53s |
| 3 | `stdio_type_loop` | `stdio_type_loop` / 全部 | 9 | **PASS 9/9** | 4.02s |
| 4 | `G-P5-PERF-01` | `performance_bench` / 全部 | 1 | **PASS 1/1** | 186.13s |
| 5 | `G-P5-STABILITY-01` | `q2_stability` / 全部 | 1 | **PASS 1/1** | 6.41s |
| 6 | `real_matrix` | `native_parity` / `real_matrix` | 3 | **PASS 3/3** | 16.81s |
|  | **合计** |  | **40** | **PASS 40/40** | 4m21s |

每个 gate 的 `test result` 行均精确匹配冻结计数，未出现 filtered-out 少跑或
额外测试；日志中 `0 failed`。

## 4. 40 项明细

### 4.1 native_engine_real（23/23）

```text
native_ac001_page_data_and_jsdoc_hover
native_ac002_field_typo_diagnostic_and_fix_clear
native_ac003_unicode_and_crlf_positions
native_ac005_loop_scope_typed_locals
native_ac006_conditional_union_narrowing
native_ac008_alias_project_hover_and_definition
native_ac008_multi_view_fixture_isolation
native_ac008_simultaneous_views_do_not_pollute_each_other
native_ac011_unknown_owner_degrades_honestly
native_ac012_component_property_bindings
native_ac012_event_handlers_basic
native_ac013_component_properties_hover
native_component_behavior_data_and_properties
native_event_handler_hover_diagnostics_definition_completion
native_expression_and_event_completion_smoke
native_hover_and_diagnostics_smoke
native_non_owner_overlay_replay_survives_restart
native_q2_performance_benchmark
native_static_wxs_and_template_semantics_work_over_stdio
native_static_wxs_answers_survive_an_unavailable_type_backend
native_symbols_smoke
native_ten_round_lifecycle_stability
native_unsaved_wxs_overlay_updates_static_hover_and_completion
```

说明：`native_q2_performance_benchmark` 与
`native_ten_round_lifecycle_stability` 属于本组 23 个 engine gate 的一部分；
下面的第 4/5 组是独立的 perf/stability retarget gate，不是重复计数。

### 4.2 symbols_edits（3/3）

```text
symbols_edits::assert_native_edit_reparses_and_typechecks
symbols_edits::native_component_property_edit_applies_and_reparses
symbols_edits::native_rejects_cross_namespace_and_dynamic_edits
```

### 4.3 stdio_type_loop（9/9）

```text
ac001_page_data_and_jsdoc_hover
ac002_field_typo_diagnostic_and_fix_clear
ac003_unicode_and_crlf_positions
ac011_unknown_owner_degrades_honestly
ac013_component_properties_hover
component_behavior_data_and_properties_work_over_stdio
static_wxs_and_template_semantics_work_over_stdio
static_wxs_answers_survive_an_unavailable_type_backend
unsaved_wxs_overlay_updates_static_hover_and_completion
```

### 4.4 performance_bench / G-P5-PERF-01（1/1）

```text
q2_performance_benchmark
```

### 4.5 q2_stability / G-P5-STABILITY-01（1/1）

```text
ten_round_lifecycle_stability
```

### 4.6 real_matrix（3/3）

```text
real_matrix::real_matrix_in_place_matches_frozen_oracle
real_matrix::real_matrix_known_gaps_are_characterized
real_matrix::real_matrix_top_level_cases
```

## 5. q2_stability 运行证据

报告：`/tmp/opencode/q2-bench/stability-run.json`（02:20:24Z 写入，与 gate 日志
同一分钟）。

| 项 | 值 |
| --- | --- |
| gate / head | `G-P5-STABILITY-01` / `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b` |
| 轮次 | 10（version v3 ～ v21，每轮 close/reopen） |
| 每轮 engine 重启 | 均已 `SIGKILL` 旧 PID 并获得新 PID（2461775 → 2462171，逐轮递增） |
| 重启后首请求分类 | 10/10 轮均为 `string`（无需重试即恢复 disk type） |
| RSS（round 1 → final） | 150,072 → 150,400 KiB |
| RSS 增长 | **0.219%**（上限 15%） |
| 场景 | 4 IMPLEMENTED，0 FAILED，1 BLOCKED（外部 native RSS baseline 未提交，按合同不失败） |
| 场景明细 | typing burst + cancel、image 删除/恢复、app.json overlay 删除/恢复、40 请求队列突发，全部 IMPLEMENTED |
| shutdown | 20.3 ms，无 detached worker，sandbox/进程残留 0 |
| 阈值 | `thresholds.pass = true`，`failures = []` |

## 6. 失败项与日志

**失败项：无。** 因此没有需要区分“真实回归 vs 环境/包路径”的失败。

| 产物 | 路径 |
| --- | --- |
| 本次完整 stdout+stderr（含 `EXIT_CODE=0`） | `/tmp/opencode/p7-residual-gates.log` |
| 分组/汇总日志目录 | `/tmp/opencode/p7-residual-native-gates-20261120T021620Z/` |
| runner `summary.txt` | 同上目录 `summary.txt`（`gate:` 6 行全 PASS，`no_node_scope: NOT_REQUESTED`） |
| q2_stability 原始报告 | `/tmp/opencode/q2-bench/stability-run.json` |

`summary.txt` 关键字段：

```text
head: b6c1ceda5baa9b55ec1537cff278ece3fc883a9b
build: PASS
gate: PASS native_parity::native_engine_real 23/23
gate: PASS native_parity::symbols_edits 3/3
gate: PASS stdio_type_loop 9/9
gate: PASS performance_bench::G-P5-PERF-01 1/1
gate: PASS q2_stability::G-P5-STABILITY-01 1/1
gate: PASS native_parity::real_matrix 3/3
```

## 7. 边界

- 本次只运行 40 个 native gates（`--verify-no-node` 未请求），不代替 workspace
  全量测试、release 构建/安装复验、Zed 冒烟、升级/回滚或跨平台验证。
- 结论只覆盖 HEAD `b6c1ced` 加本轮未提交测试/文档改动的 native 交付路径；
  并行会话在运行结束后写入的 `crates/wxml-delivery/src/**` 不在本次范围内。
- 稳定性场景 5（外部 RSS baseline 比值）按设计保持 BLOCKED，等待独立的 native
  server-tree RSS baseline 工件；它不计入失败，也不改变 40/40 的判定。
- 本次未产生任何代码、测试、README、ledger 改动，也未 commit。
