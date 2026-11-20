# Wave 11-C/D：重复表达式 mount 与每请求状态隔离

- 工作区：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD：`68f852c`（测试提交；本切片验证时基于 `51abc13`）
- 日期：2026-11-20
- 范围：`TS-ANALYSIS-0332`（W11-C）、`TS-ANALYSIS-0315`（W11-D）
- 结论：**2 exact / 0 blocked**。台账回填后本切片为 72 green / 229 planned / 38 retired；与 messages/probes 10 个 exact 合并后当前台账为 **82 green / 219 planned / 38 retired**（`exact` 82）。生产代码未改；本轮只改台账、validator、契约与证据，未 commit。

## 汇总

| Case | Oracle | 结果 | Rust 证据 |
| --- | --- | --- | --- |
| `TS-ANALYSIS-0332` | `backends/typescript/test/analysis.test.ts:332-347` | exact |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:273 `case_analysis_0332_one_expression_may_appear_under_two_fragment_ids` |
| `TS-ANALYSIS-0315` | `backends/typescript/test/analysis.test.ts:315-330` | exact（每请求状态口径） |  crates/wxml-lsp/src/backend_tsgo/tests.rs:2804 `case_analysis_0315_a_partial_definition_does_not_degrade_later_hover_or_completion` |

两个测试都只追加断言，没有触发布局合并或共享状态的生产修正。

## TS-ANALYSIS-0332

### Oracle 断言

`analysis.test.ts:332` 用两个 fragment id（`f0`、`f1`）携带同一表达式 `title`，然后断言：

- `result.mounts.length === 2`
- `result.mounts.map(mount => mount.mountId).sort()` 为 `["f0#0", "f1#0"]`
- `result.response.fragments.map(entry => entry.fragmentId).sort()` 为 `["f0", "f1"]`

### Rust 行为

生产布局在 `crates/wxml-backend-tsgo/src/shadow/layout.rs:684-...` 按 fragment 逐个校验并生成 `PendingFragment`，只在 `fragment.id` 上做唯一性检查，没有按 `expr` 去重；每个 fragment 在 `crates/wxml-backend-tsgo/src/shadow/layout.rs:977` 生成独立的 `mount_id = "{fragment_id}#0"`。因此不需要改布局语义。

新增测试使用 `Page({ data: { title: 'x' } });` 和两个 `FragmentSpec::new(_, "title", SourceRange::new(0, 5))`，精确断言：

- `status == BuildStatus::Supported`，`data_root == DataRootKind::Object`
- `view.mounts.len() == 2`，顺序为 `f0`、`f1`
- `mount_id` 顺序为 `f0#0`、`f1#0`
- 两个 mount 的 `text` 都是 `__wxml_root.title`
- 每个 mount 的 derived `[start_utf16, end_utf16)` 切片恰好等于 `mount.text`，且 `mounts[0].start_utf16 < mounts[1].start_utf16`，前一个 mount 的结束不晚于后一个 mount 的开始（没有折叠）
- 每个 mount 的 target range 都是模板标识符 `title`（长度 5）
- 每个 mount 的 segments 精确为：
  - synthetic 前缀 `__wxml_root.`：`length_utf16 == 12`，`source == None`
  - 源片段：`length_utf16 == 5`，`source == Some(SourceRange::new(0, 5))`
- 两个 mount 各自把相同的 WXML `0..5` 映射回模板，互不合并

### exact 边界

Oracle 的 `result.mounts` 断言直接对应 `view.mounts`；`result.response.fragments` 的 id 列表在 native layout 边界由 `view.mounts[].fragment_id` 表达（当前没有 C1 response envelope）。该替换已在测试注释和本条证据中说明，未改变布局行为。

## TS-ANALYSIS-0315

### Oracle 断言

`analysis.test.ts:315` 给 `finishedStates` 传入 `droppedDefinitions: 1`，断言：

- `states.definition.status === "partial"`
- `states.hover.status === "supported"`
- `states.diagnostics.status === "supported"`

Oracle 只断言三个 capability 的 status，不断言 reason 文本。

### Rust 行为

适配器没有跨请求的 capability 状态；每个请求独立计算 `CapabilityStatus`，partial 结果不会写回 session 或 adapter。测试用现有 fake session 和同一个 adapter 顺序发起五个请求：

1. `definition`：native 返回一个只存在于生成 probe 文本中的 target（`__wxml_check_0`）。`definition::map_target` 无法把它映射回模板/原始脚本，丢弃该 target，并精确返回
   `CapabilityStatus::Partial { reason: "some definition targets could not be represented" }`，`locations` 为空。
2. `hover`：仍为 `Supported`，`quality == Known`，`contents == "```ts\nstring\n```"`，range 精确覆盖模板中的 `na`。
3. `completion`：仍为 `Supported`，条目为空、`truncated == false`。
4. `diagnostics`：仍为 `Supported`，诊断列表为空（对应 oracle 的 diagnostics supported）。
5. 再次 `definition`：仍为同一 `Partial` 和空 `locations`，证明 status 按请求重新计算，没有被前一次定义请求或后续请求污染。

最后断言 fake session 的日志：`starts == 1`、`shutdowns == 0`、`definition_calls.len() == 2`、`hover_evidence_calls.len() == 1`、`completion_calls.len() == 1`。partial definition 既不重启 session，也不把状态泄漏到 hover/completion。

### exact 边界

Oracle 在纯 `finishedStates` 聚合器上断言；Rust 没有同名聚合器，本测试在同一条 adapter 请求序列上断言相同的三个 status，并额外精确断言 Rust 适配器实际产生的 definition reason。reason 文本属于 Rust 行为，不是对 oracle 文本的臆造。

## 台账回填（已应用）

### Markdown ledger

`docs/plans/2026-11-17-tsgo-test-ledger.md` §25 记录本切片的逐 case 证据；用例索引表保持结构不变。

| Case | 建议 status | 建议证据 |
| --- | --- | --- |
| `TS-ANALYSIS-0315`（第 327 行附近） | `planned` → `green` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:2804；note：每请求状态隔离，definition `Partial` 不影响后续 hover/diagnostics/completion `Supported`；oracle reason 未断言，Rust reason 为 `some definition targets could not be represented` |
| `TS-ANALYSIS-0332`（第 328 行附近） | `planned` → `green` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:273；note：同一表达式两个 fragment 产生两个 mount，id `f0#0`/`f1#0`，各自 range/segment 映射到 WXML `0..5`，不合并；`response.fragments` 由 `view.mounts[].fragment_id` 在 layout 边界表达 |

### JSON ledger

`docs/plans/tsgo-test-ledger.json` 已应用：

- `TS-ANALYSIS-0332`
  - `target_rust_test`: 保持 partition 路由 `crates/wxml-lsp/tests/native_parity/project_overlay.rs::case_analysis_0332_one_expression_may_appear_under_two_fragment_ids`；真实测试在 `migration_evidence.rust_tests`
  - `target_rust_test_status`: `migrated_green`
  - `status`: `green`
  - `migration_evidence.parity`: `exact`
  - `migration_evidence.rust_tests`:  crates/wxml-backend-tsgo/tests/shadow_layout.rs:273，note 记录“两个 mount、顺序、derived range、target range、segments、不合并；C1 `response.fragments` 由 mounts fragment_id 表达”
- `TS-ANALYSIS-0315`
  - `target_rust_test`: 保持 partition 路由 `crates/wxml-lsp/tests/native_parity/project_overlay.rs::case_analysis_0315_a_dropped_definition_downgrades_only_the_definition_capability`；真实测试在 `migration_evidence.rust_tests`
  - `target_rust_test_status`: `migrated_green`
  - `status`: `green`
  - `migration_evidence.parity`: `exact`
  - `migration_evidence.rust_tests`:  crates/wxml-lsp/src/backend_tsgo/tests.rs:2804，note 记录“dropped generated target → definition `Partial`；hover/completion/diagnostics `Supported`；session 不重启；每请求状态不共享”

## 验证

### 本切片隔离验证

共享工作区在验证期间有并行 session 的未提交 W11 改动（`crates/wxml-backend-tsgo/src/query/mod.rs`、`query/message.rs`、`query/probe_health.rs`、`crates/wxml-lsp/tests/native_parity/ledger.rs`）。为排除这些改动，本切片在 `/tmp/opencode/w11-cd-verify` 的 detached worktree（HEAD `51abc13`）中只应用本文件记录的两个测试 diff，并使用独立 `CARGO_TARGET_DIR` 重新执行：

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all --check` | 通过（无输出） |
| `cargo check --workspace --all-targets --all-features` | 通过，`Finished dev profile` |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 通过，无 issue |
| `cargo test -p wxml-backend-tsgo --test shadow_layout` | 35 passed |
| `cargo test -p wxml-lsp --lib backend_tsgo::` | 312 passed，639 filtered |
| `cargo test -p wxml-lsp --test native_parity` | 76 passed，26 ignored |
| `git diff --check` | 通过（无 whitespace 错误） |

隔离 worktree、临时 patch 和独立 target 目录在验证后已删除。

### 共享工作区状态

共享工作区中的两个目标测试同样通过：`shadow_layout` 35 passed，`backend_tsgo::` 312 passed。共享工作区的全量 clippy 当前被并行 session 的未提交 `query/message.rs` 与 `query/probe_health.rs` 中的 5 个 lint 阻断（`?` 运算符和 `const fn` 建议），与本次两个测试文件无关；本切片在隔离 worktree 中已确认无 lint。

`git status --short` 中属于本切片的改动只有：

- `crates/wxml-backend-tsgo/tests/shadow_layout.rs`
- `crates/wxml-lsp/src/backend_tsgo/tests.rs`
- 本证据文件

其余未跟踪/修改文件（`query/mod.rs`、`query/message.rs`、`query/probe_health.rs`、`native_parity/ledger.rs` 及三份 wave-11 证据）均为并行 session 的产物，本任务未修改。未生成项目内临时文件，未 commit。
