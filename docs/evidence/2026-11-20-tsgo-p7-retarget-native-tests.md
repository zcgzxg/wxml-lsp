# P7 retarget closeout：PERF/STABILITY 原生薄壳与 overlay 证据修复（2026-11-20）

## 0. 结论先行

1. `crates/wxml-lsp/tests/performance_bench.rs` 与
   `crates/wxml-lsp/tests/q2_stability.rs` 已原地改为 native-only 薄壳：
   不再导入 `backend_node` / `NodeBackend` / `NodeTransport`，不再读取
   `WXML_TEST_NODE`、`WXML_BACKEND_DIR`、`dist/server.js`。
2. 两个 gate 的旧 `#[ignore]` 测试名、文件坐标和 `p7_action=retarget` 不变；
   都**没有** `old_test_removed`。深层 oracle 仍是
   `crates/wxml-lsp/tests/native_parity/native_engine_real.rs` 的
   `native_q2_performance_benchmark` / `native_ten_round_lifecycle_stability`。
3. 性能 gate 在真实 pinned engine 上按冻结预算通过；取消/排队/慢调用使用
   进程内 400 ms `SlowBackend`（与旧 `SlowTransport` 同层），明确不声称
   native engine 的取消延迟。
4. 稳定性 gate 在一个 fresh native server 内完成 10 轮
   update/query/SIGKILL/replay/close-reopen，engine 每轮新 PID，
   server-tree RSS 未增长，clean shutdown 无泄漏。
5. `TS-OVERLAY-0036` 的 ledger evidence 不再指向将被删除的
   `crates/wxml-lsp/tests/zed_sync.rs`，改用现有
   `native_non_owner_overlay_replay_survives_restart` 与
   `hover_composes_type_documentation_and_jsdoc`。case 仍是 `reviewed`，
   closeout 计数保持 109 / 0 / 130 / 29 / 71。
6. 未改生产代码、未删 Node、未改 README/脚本、未 commit、未 review。

## 1. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD：`4dd62b119dc12d904324d2ad20f636ea2e69c70e`
- 改动文件：
  - `crates/wxml-lsp/tests/performance_bench.rs`
  - `crates/wxml-lsp/tests/q2_stability.rs`
  - `crates/wxml-lsp/tests/native_parity/native_gates.rs`
  - `docs/plans/tsgo-test-ledger.json`
  - 本文件
- 输入：`docs/evidence/2026-11-20-tsgo-p7-retarget-plan.md`、
  `crates/wxml-lsp/tests/stdio_type_loop.rs` 的薄壳模式、
  `/tmp/opencode/p7-node-removal.patch`。
- 边界：只动测试、ledger 与证据文档；不执行 `/tmp/opencode/p7-node-removal.patch`。

## 2. 两个 retarget 阻塞

删除 `backend_node` 的旧 patch 会直接删掉
`performance_bench.rs` / `q2_stability.rs`，但这两个 gate 是
`p7_action=retarget`，按冻结证据必须保留旧 `#[ignore]` 测试。现改为
native-only 后，删除 `backend_node` 不再需要删除这两个文件；后续 patch
必须改为“保留这两个文件”的版本，不能再带 `deleted file mode`。

## 3. 性能 gate：`G-P5-PERF-01`

### 3.1 结构

同一 `q2_performance_benchmark` ignored 函数内两段：

| 段 | 运行方式 | 内容 |
| --- | --- | --- |
| real engine | `NativeGate` + `<package>/bin/wxml-lsp --stdio --backend native` | cold start、10,000 次 static completion、200 次 semantic completion/hover、200 次 diagnostics、server-tree RSS、engine 计数、shutdown |
| scheduler | 进程内 `run_with_host` + 400 ms `SlowBackend` | running cancel、queued cancel、20 次慢调用 occupancy |

`scheduler` 段不启动 engine，也不启动 Node；它复刻的是旧 gate 的
`SlowTransport` 层，不是 native engine 的取消能力。

### 3.2 预算（未放宽）

| 指标 | 阈值 | 本次实测 |
| --- | ---: | ---: |
| static completion p95 | ≤ 50 ms | 0.058 ms |
| semantic completion p95 | ≤ 200 ms | 0.501 ms |
| semantic hover p95 | ≤ 200 ms | 1.118 ms |
| diagnostics stop p95 | ≤ 500 ms | 88.431 ms |
| cancel queued p95 | ≤ 100 ms | 0.325 ms |
| cancel running p95 | ≤ 100 ms | 0.320 ms |
| single slow call p50 | ≥ 300 ms（证明 400 ms synthetic delay 被看到） | 400.480 ms |
| server-tree RSS growth | ≤ 15% | -4.933% |

其它保留/新增证据：cold start 记录（initialize 53.828 ms、首屏 326.043 ms、
首次 engine hover 5.604 ms）、engine 恰好 1 个、server tree 无额外 child、
`assert_no_node_exec`、`WXML_PERF_BASELINE` 可选回归（5 个 p95 +10%，
parent RSS +15%，子毫秒指标保留 0.5 ms noise floor）、
`WXML_PERF_REQUIRE_CLEAN`、rustc/OS/kernel/CPU/mem/loadavg/engine 身份。

> 说明：native harness 固定 `WXML_DIAGNOSTICS_DEBOUNCE_MS=0`。这比旧 gate 的
> 120 ms 更严；报告里显式记录 `diagnostics_debounce_ms: 0`，没有把
> “包含 120 ms debounce”写成 native 事实。

### 3.3 证据

- 命令：
  ```bash
  env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE -u WXML_TEST_NATIVE_PACKAGE \
    WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
    TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
    TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
    TSGO_TYPINGS_ROOT=$PWD/backends/typescript/node_modules/miniprogram-api-typings \
    WXML_PERF_LABEL=p7-retarget WXML_PERF_OUT=/tmp/opencode/p7-retarget/perf.json \
    cargo test -p wxml-lsp --release --test performance_bench -- \
      --ignored --test-threads=1 --nocapture
  ```
- 结果：`1 passed / 0 failed / 9 filtered`，exit 0，181.61 s，release。
- 报告：`/tmp/opencode/p7-retarget/perf.json`（未提交）。
- 本次 worktree 带未提交的 retarget 测试改动，未设置
  `WXML_PERF_REQUIRE_CLEAN`；报告 `git_dirty` 记录了这一点。
- `blocked`：空。旧 gate 的 11 类场景都有落点；区别仅在于 cancel/occupancy
  是 scheduler 级而非 engine 级，报告和 ledger 均已写明。

## 4. 稳定性 gate：`G-P5-STABILITY-01`

### 4.1 结构

同一个 fresh `--stdio --backend native` server，10 轮，每轮：

1. `didChange` 更新，diagnostics 必须为空；
2. disk hover 必须是 `string`；
3. 确认恰好 1 个 engine，`SIGKILL` 它，等 PID 消失；
4. hover 重试直到 `string`（记录首次请求 class），新 engine PID 必须 ≠ 旧 PID；
5. `didClose` + `didOpen` 重开模板，diagnostics 为空、hover 恢复 `string`；
6. 采样 server-tree RSS/threads/processes；round 1 作为基线。

结束后再确认最终 engine 恰好 1 个，`shutdown` 不 detach worker，server 与
engine 全部消失，private TMPDIR 无 sandbox，PATH tripwire 未触发。

### 4.2 阈值与实测

| 指标 | 阈值 | 本次实测 |
| --- | ---: | ---: |
| 10 轮 server-tree RSS growth | ≤ 15% | -1.352% |
| engine 数 | 每轮/最终恰好 1 | 1 |
| engine 重启 | 每轮新 PID | 10/10 |
| clean shutdown | 无 detach/child/sandbox | 通过 |

其它：round 1 baseline 158880 KiB，final 156732 KiB，CPU 1.39 s，wall 3.97 s，
shutdown 20.249 ms。报告 `WXML_STABILITY_ALLOW_DIRTY=1` 是因为当前 worktree
带未提交改动；`WXML_STABILITY_REQUIRE_CLEAN` 仍是默认严格模式，旧
`WXML_STABILITY_LENIENT` 逃生口已移除（不放宽任何阈值）。

### 4.3 BLOCKED（明确记录，不静默放宽）

| 旧场景 | 状态 | 原因 / follow-up |
| --- | --- | --- |
| typing burst + in-flight cancel | **BLOCKED** | 薄壳不复制旧 10 次连打后立即 cancel；深层 oracle 覆盖更新节奏，性能 gate 的 scheduler 段覆盖取消。follow-up：在 `native_engine_real` 或薄壳加 burst/cancel 轮。 |
| 资源删除/恢复 + watched files | **BLOCKED** | 当前没有 native gate 断言 `does not exist` 的删除/恢复往返。follow-up：加有界 `didChangeWatchedFiles` 断言。 |
| `app.json` overlay 移除/恢复 | **BLOCKED** | 旧 gate 是唯一断言“移除 page 后 hover 为 null”的地方；native ownerless hover 由 `G-P5-E2E-09` 证明，但 app overlay 组合未测。follow-up：先 probe。 |
| 40 请求有界队列 burst | **BLOCKED** | 深层 oracle 只有 16 次串行 hover；并发 40 请求未复证。follow-up：加并发 burst。 |
| 外部 RSS baseline ratio | **BLOCKED** | 旧 baseline 混合 Rust parent RSS 与 Node 子进程；native server-tree RSS 不可直接比较。follow-up：先单独 re-baseline。 |

这些 BLOCKED 不改变 gate 的 `p7_action=retarget`，也不改变
`status=gate_green`（该状态由深层 native oracle 的既有证据支撑）。

### 4.4 证据

- 命令：
  ```bash
  env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE -u WXML_TEST_NATIVE_PACKAGE \
    WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
    TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
    TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
    TSGO_TYPINGS_ROOT=$PWD/backends/typescript/node_modules/miniprogram-api-typings \
    WXML_STABILITY_ALLOW_DIRTY=1 WXML_STABILITY_LABEL=retarget-p7 \
    WXML_STABILITY_OUT=/tmp/opencode/p7-retarget/stability.json \
    cargo test -p wxml-lsp --release --test q2_stability -- \
      --ignored --test-threads=1
  ```
- 结果：`1 passed / 0 failed / 9 filtered`，exit 0，4.04 s，release。
- 报告：`/tmp/opencode/p7-retarget/stability.json`（未提交）。

## 5. `native_gates.rs` 与 ledger

### 5.1 新机器约束

`native_gates.rs` 新增
`retargeted_gates_are_native_only_and_keep_their_old_test`，对全部 11 个
`p7_action=retarget` gate（9 个 `stdio_type_loop` + `PERF-01` +
`STABILITY-01`）断言：

- ledger 里没有 `old_test_removed`；
- `p7_action=retarget`；
- 旧文件与函数名仍在；
- 文件里必须出现该 gate 的 `NativeGate::require("<gate id>")` 和 `Session::start(`；
- 文件不得再出现 `wxml_lsp::backend_node`、`NodeBackend`、`NodeTransport`、
  `WXML_TEST_NODE`、`dist/server.js`；
- `replacement_test` 必须仍指向 `native_engine_real.rs` 中真实存在的函数。

### 5.2 ledger 变更

- `proposed_command` 从 TBD 改为真实 retarget 命令。
- `removal_condition` 明确：retarget gate 永不标 removed。
- 新增 `retarget_evidence`：state、命令、run_head、结果、实测值、预算、
  BLOCKED 列表（仅稳定性 5 项）。
- `replacement_test` / `status` / `gate_evidence` 未改；深层 oracle 证据仍是
  既有 release 运行，不伪造新 green 记录。

## 6. `TS-OVERLAY-0036` 悬空引用

旧 evidence：

- `crates/wxml-lsp/src/backend_tsgo/tests.rs::hover_composes_type_documentation_and_jsdoc`
- `crates/wxml-lsp/tests/zed_sync.rs::unsaved_script_overlay_reaches_wxml_analysis`（将被删除）

现 evidence：

- `crates/wxml-lsp/src/backend_tsgo/tests.rs:1198::hover_composes_type_documentation_and_jsdoc`
- `crates/wxml-lsp/tests/native_parity/native_engine_real.rs:511::native_non_owner_overlay_replay_survives_restart`

note 明确仍然缺口：没有 JSDoc-only（type 不变、doc 变）的 native overlay 断言；
`native_non_owner_overlay_replay_survives_restart` 使用的是 type-changing overlay。
`follow_up` 指向 `native_parity/project_overlay.rs` 的 doc-only overlay 断言。
case 保持 `reviewed`，因此 closeout 计数不变：

| green | planned | reviewed | waived | retired |
| ---: | ---: | ---: | ---: | ---: |
| 109 | 0 | 130 | 29 | 71 |

## 7. 验证

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `cargo check -p wxml-lsp --tests` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test -p wxml-lsp` | 1285 passed / 52 ignored（26 suites） |
| `cargo test -p wxml-lsp --test native_parity` | 81 passed / 29 ignored |
| `q2_stability` ignored（release，fixture） | 1 passed / 0 failed / 9 filtered |
| `performance_bench` ignored（release，fixture） | 1 passed / 0 failed / 9 filtered |
| `git diff --check` | PASS |

`scripts/native-gates.sh` 未运行：provisioned
`/tmp/opencode/tsgo-native-pkg/bin/wxml-lsp` 是更早 HEAD 的包，runner 不会用
当前 HEAD 重新打包；本次改用 `NativeGate` fixture 模式直接用当前 release
二进制跑两个 retarget gate。`native_parity`（含 23 个 ledger gate 的静态约束）
已单独通过。

> 最终全量 `cargo test -p wxml-lsp` 时 `/tmp` tmpfs 被大量旧的一次性测试临时目录
> 占满（`StorageFull` 只出现在 `cli.rs`/`configuration.rs` 的 fixture 复制）；
> 清理 `wxml-package-*` / `wxml-lsp-cli-*` / `wxml-engine-*` 测试临时目录，并用
> `WXML_TEST_TMPDIR=/var/tmp/opencode-wxml-tests` 重跑后通过。未删除
> `/tmp/opencode` 下的证据目录，也未改测试语义。

## 8. 明确不声称

- 不声称性能 gate 的 cancel/occupancy 证明 native engine 的取消能力；它们是
  scheduler 级测试替身。
- 不声称稳定性 gate 已等价覆盖旧 `q2_stability` 的全部场景；本切片记录时 **5 项 BLOCKED** 已在
  ledger 和本文列明。后续 `q2_stability_support` 场景证据把其中 4 项推进为 IMPLEMENTED
  （typing burst/cancel、资源删除恢复、app.json overlay、40 请求 burst），仅外部 RSS baseline
  仍 BLOCKED；**ledger 尚未回填**，仍把这 4 项记为 BLOCKED。见
  `docs/evidence/2026-11-20-tsgo-stability-scenarios.md`。
- 不声称 `TS-OVERLAY-0036` 已 exact；它仍是 `reviewed`。
- 不声称 Node 已删除或 `/tmp/opencode/p7-node-removal.patch` 可直接应用；该
  patch 仍需去掉对两个 retarget 文件的删除 hunk。
- 不声称 `scripts/native-gates.sh` 的 35 gate 计数已包含这两个 retarget 文件；
  runner 扩展不在本次范围。
