# P7 retarget 计划：11 个旧 gate 的最小 native 替代（2026-11-20）

## 0. 身份、边界与结论先行

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`。
- 任务 HEAD：`74669c3`（`feat(tsgo): add message sanitization and probe health`）。
- 冻结基线：`docs/evidence/data/legacy-p7-freeze.json` 的 `75040c2`。三个待 retarget 文件
  自冻结点以来没有改动，冻结行号仍然有效。
- 输入：冻结 JSON、`docs/evidence/2026-11-20-tsgo-p7-delete-dryrun.md`、
  crates/wxml-lsp/tests/stdio_type_loop.rs、performance_bench.rs、q2_stability.rs、
  `crates/wxml-lsp/tests/native_parity/**`、`scripts/native-gates.sh`。
- 性质：**只读规划**。只新增本文件；未改代码、测试、ledger、README、freeze JSON 或脚本；
  未 commit，未 review；未运行任何测试。工作树已有的并行未提交改动未触碰。
- 本文的“直接断言数”是测试函数体内的 `assert!/assert_eq!/assert_ne!` 调用点；helper 内部的
  断言单独说明，不展开计数。

**结论先行：**

1. **9 个 `stdio_type_loop` gate 的 native 替代已经等价或更强。** 旧测试直接断言 54 处，
   `native_engine_real` 对应测试直接断言 60 处；逐项比对后没有丢失的场景。retarget 本身
   **不产生新断言**，它保留的是“独立测试目标 + 每次 fresh 进程 + clean shutdown”的
   gate 身份；deep parity 已经在 replacement 里。
2. **`performance_bench` 不等价。** 现有 native perf 覆盖 hover/completion/diagnostics
   p95、RSS growth、cold start、engine 进程数；缺 `cancel_queued_request`、
   `cancel_running_response`、`single_backend_call_occupancy`、static/semantic completion
   拆分、baseline JSON 回归、环境 provenance。
3. **`q2_stability` 不等价。** 现有 native stability 已有 12 轮 update/release/SIGKILL
   engine、overlay replay、单 engine 计数、RSS/leak/shutdown；缺 typing burst + in-flight
   cancel、40 请求 bounded queue burst、资源删除/恢复、app.json overlay 移除/恢复、
   template close/reopen、外部 RSS baseline。
4. **不能新增 `#[ignore]` 测试函数。** `native_gates.rs` 用
   `count == 23 old - removed + native + edit` 的方程约束 `#[ignore]` 集合；新增 ignored
   gate 还要进 `IGNORED_GATE_TESTS` 并被 `GATE_REPLACEMENTS` 唯一链接，实际上等于重开
   ledger。所有新断言必须放进既有 gate 函数体。
5. **任何 retarget 编辑都会让 `scripts/p7-freeze.sh --check` 失败。** 三个文件的
   `file_sha256`、`line_ignore`/`line_fn`、硬编码 `node_dependency` 和
   `WXML_TEST_NODE` gate-only env 都会过期；必须同批重新生成冻结证据（建议 schema 加
   `p7_state`，不要把 retarget 后的状态覆盖成“仍依赖 Node”）。
6. **修正 dry-run 的一个口径：** 旧 `q2_stability` 是**同一个 server 内的 10 轮**，每轮
   SIGKILL Node 子进程，不是 10 次 server 重启。native 的 12 轮结构形状相同，缺的是
   workspace/document 生命周期操作，不是“server 重启次数”。
7. **计数：** 11 gate = 9 个断言等价 + 2 个部分覆盖；需要新增的断言组约 10 组；
   新增 ignored 函数 0；新增测试文件 0；推荐工作量 **6.0–9.5 人日**。只做“去 Node、
   不补覆盖”的最小壳约 **2–3 人日**，但不建议单独交付。

---

## 1. 口径与运行时模式

### 1.1 retarget 的定义

`p7_action=retarget` 的机器约束只要求旧 `#[ignore]` 测试**仍在树里**
（`crates/wxml-lsp/tests/native_parity/native_gates.rs:474` 的
`validate_old_test_removals`；retarget gate 标记 removed 会直接失败）。retarget 的语义是：

- 函数名、文件、`#[ignore]` 不变，gate 坐标不变；
- 把 Node backend 边界换成 `--stdio --backend native` 的 disposable package；
- 不再需要 `WXML_TEST_NODE`、`WXML_TYPE_BACKEND`、`WXML_BACKEND_DIR`、`dist/server.js`。

9 个 E2E gate 的 deep oracle 已经在 ledger 的 `replacement_test` 里；PERF/STABILITY 的
replacement 只覆盖一部分，缺口见 §4/§5。retarget 不改变 `replacement_test`，也不改变
`GATE_REPLACEMENTS`。

### 1.2 native 运行模式

| 模式 | 环境 | 行为 |
| --- | --- | --- |
| `required/fixture` | `WXML_TEST_NATIVE=1` + `WXML_REQUIRE_REAL_ENGINE=1` + `TSGO_BIN` + `TSGO_ASSET_ROOT` + `TSGO_TYPINGS_ROOT` | 从 pinned engine 构建一次性 package，缺任何输入都 panic；`scripts/native-gates.sh` 使用此模式 |
| `required/existing` | 同上 + `WXML_TEST_NATIVE_PACKAGE` | 使用已安装 package；只适合本地复跑；runner 故意不导出该变量（E2E-13 会删除自身 engine） |
| `optional/fixture` | 只设 `WXML_TEST_NATIVE=1` | 缺 package 时打印显式 skip 并返回 `None`；required job 不使用 |

实现入口：`crates/wxml-lsp/tests/native_parity/native_gate_env.rs:124`（`PackageRequest`）、
`crates/wxml-lsp/tests/native_parity/native_package.rs:336`（`NativeGate::require`）、
`crates/wxml-lsp/tests/native_parity/native_package.rs:452`（`build_fixture`，复制
`CARGO_BIN_EXE_wxml-lsp` 到 `<package>/bin/wxml-lsp`）。

### 1.3 缺口代码

| 代码 | 含义 |
| --- | --- |
| B | stdio 进程边界：独立测试目标、fresh 进程、clean shutdown |
| C | cancel：queued 与 running 两条路径 |
| Q | bounded queue / “server is busy” |
| O | 单次慢 backend 调用的 occupancy |
| P | static vs semantic completion 拆分 |
| BL | 外部 baseline JSON 回归（p95/RSS） |
| L | 多轮 document/workspace 生命周期与泄漏 |
| N | watched files / 资源删除恢复 / app overlay / close-reopen |
| E | 环境与 provenance（rustc、OS、kernel、loadavg、dirty） |

---

## 2. 逐 gate 总表

旧文件行号取冻结 JSON 的 `line_fn`；native 行号是当前 HEAD 的 `fn` 行。

| Gate | 旧测试（文件:行） | 旧直接断言 | native 替代（行） | native 直接断言 | 等价性 | 缺口 | 建议动作 |
| --- | --- | ---: | --- | ---: | --- | --- | --- |
| G-P5-E2E-06 |  crates/wxml-lsp/tests/stdio_type_loop.rs:505 | 14 |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:1042 | 14 | 等价 | B | 薄壳 retarget |
| G-P5-E2E-07 |  crates/wxml-lsp/tests/stdio_type_loop.rs:607 | 3 |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:1148 | 4 | 等价（更严） | B | 薄壳 retarget |
| G-P5-E2E-08 |  crates/wxml-lsp/tests/stdio_type_loop.rs:659 | 8 |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:1203 | 8 | 等价 | B | 薄壳 retarget |
| G-P5-E2E-09 |  crates/wxml-lsp/tests/stdio_type_loop.rs:781 | 3 |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:225 | 4 | 等价（更严） | B | 薄壳 retarget |
| G-P5-E2E-10 |  crates/wxml-lsp/tests/stdio_type_loop.rs:707 | 3 |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:1255 | 3 | 等价 | B | 薄壳 retarget |
| G-P5-E2E-11 |  crates/wxml-lsp/tests/stdio_type_loop.rs:948 | 5 |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:2867 | 4 | 等价（helper 内多断言） | B | 薄壳 retarget |
| G-P5-E2E-12 |  crates/wxml-lsp/tests/stdio_type_loop.rs:837 | 10 |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:288 | 10 | 等价 | B | 薄壳 retarget |
| G-P5-E2E-13 |  crates/wxml-lsp/tests/stdio_type_loop.rs:1084 | 3 |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:1380 | 7 | 等价（更强） | B | 薄壳 retarget |
| G-P5-E2E-14 |  crates/wxml-lsp/tests/stdio_type_loop.rs:1014 | 5 |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:409 | 6 | 等价（更严） | B | 薄壳 retarget |
| G-P5-PERF-01 |  crates/wxml-lsp/tests/performance_bench.rs:782 | 3 |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:3131 | 9 | 部分 | C,Q,O,P,BL,E | 原地重写 |
| G-P5-STABILITY-01 |  crates/wxml-lsp/tests/q2_stability.rs:203 | 14 |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:3467 | 14 | 部分 | C,Q,N,BL,L | 原地重写 |

合计：旧直接断言 71，native 直接断言 83；9 个 E2E 是 54 vs 60。

---

## 3. 9 个 stdio gate：现有 native 覆盖是否等价

结论：**全部等价或更强**。逐条对照：

| Gate | 旧断言 | native 断言 | 差异 |
| --- | --- | --- | --- |
| 06 | 空 diagnostics；`user.name` string + JSDoc + 精确 range；`user` 形状且不带 JSDoc + range；表达式末尾可 hover；mustache 外 null；`title` string + range | 同左，逐项相同 | 无 |
| 07 | `nmae` 处一条 error；无 status；error 数 = 1；精确 range；修复后清空 | 同左 + 显式检查 message 含 `does not exist` | native 更严 |
| 08 | 两处 `nmae`；UTF-16 列 13/17、12/16；行号 0/1 | 同左 | 无 |
| 09 | degradation warning + `unsupported`/`unavailable`；hover 为空；无 error | 同左 + warning 恰好 1 条 | native 更严 |
| 10 | 无 error；Partial warning 含 “registration has no `data` member”；`title` hover string | 同左 | 无 |
| 11 | 空 diagnostics；`sharedCount` number + JSDoc；definition 指向 `card.ts`；`cardTitle` `string \| number` | 同左；definition 用 `assert_definition_covers` 检查文件 + token | 等价 |
| 12 | WXS `format` hover 含 `utils.format`、`=> string`、JSDoc；definition 同行；template hover；imported template definition；`is="sh"` 补全 `shared`；`utils.` 补全 `format`；`data="{{ { ti` 补全 `title` | 同左 | 无 |
| 13 | 静态 WXS hover `utils.format`/`u.format` + `=> string`；definition 非空 | 同左 + unavailable status warning + 无 error + typed hover 为空 + 无 sandbox | native 更强 |
| 14 | disk hover string；overlay hover number；补全 `renamed`；close 后恢复 string + label | 同左 + 磁盘文件未被写入 | native 更严 |

注：13 的“不可用”机制不同——旧测试给一个不存在的 Node server 路径；native 在启动后
删除 `<package>/engine/lib/tsc`。两者的断言意图相同（静态 WXS 答案存活），native 还
多断言 degradation status 和“不伪造 typed hover”。

**关键区别不是断言，而是边界。** 旧文件用
`target/debug/wxml-lsp --stdio --backend node` + Node env（
`crates/wxml-lsp/tests/stdio_type_loop.rs:118`）；native 用
`<package>/bin/wxml-lsp --stdio --backend native`，清空 Node env、注入 poison
env、PATH tripwire、私有 TMPDIR（
`crates/wxml-lsp/tests/native_parity/native_lsp.rs:74`）。两者都是“每次 fresh 进程 +
`shutdown`/`exit` + 非零退出即失败”，native 还额外禁止 shutdown 时 detach worker。

因此 9 个 gate 的 retarget **不增加覆盖**；它是冻结证据要求的治理动作。最小实现应
保留 gate 身份和边界，不复制 replacement 的全部断言。连 “debug profile” 也不是旧文件
独有：native replacement 同样可以在 `cargo test`（debug）下用同一 package 运行；
真正的独有属性只有“独立测试目标 + 冻结的 gate 坐标”。

---

## 4. `performance_bench`：旧场景 vs 现有 native

旧测试的硬门（
`crates/wxml-lsp/tests/performance_bench.rs:782`）：

| 旧场景 | 旧断言 | 现有 native | 差距 |
| --- | --- | --- | --- |
| `static_completion` | 10,000 次独立调用（50/轮 × 200 轮），p95 ≤ 50 ms | 无 static 拆分；只有 member completion | P |
| `semantic_completion` | 32 个不同前缀，p95 ≤ 200 ms | 有 `PERF_COMPLETE_WXML = "{{ user. }}"` 200 次，p95 ≤ 200 ms | 基本等价；可用 32 前缀增强 |
| `semantic_hover` | 200 次，报告 | 有 200 次，p95 ≤ 200 ms | 等价 |
| `diagnostics_stop` | 200 次 didChange → publish，p95 ≤ 500 ms | 有 200 次，p95 ≤ 500 ms | 等价 |
| `cancel_queued_request` | 400 ms SlowTransport；提交 busy、再提交 queued、cancel queued；p95 ≤ 100 ms；error | 无 | C,Q |
| `cancel_running_response` | cancel 运行中的请求；立即 error；报告延迟 | 无 | C |
| `single_ts_call_occupancy` | 400 ms SlowTransport，20 次，报告 | 无 | O |
| baseline JSON | 5 个 p95 +10%，parent RSS +15%；缺失且 require 时失败 | 无外部 baseline；只有 in-run RSS growth | BL |
| 环境 | node、rustc、CPU、mem、OS、kernel、loadavg、git dirty、target dir | engine identity、cpus、git head；无 rustc/OS/loadavg/dirty | E |
| 资源 | parent RSS/threads before/after | server tree RSS/threads/CPU；parent RSS 只记录 | 基本等价 |
| 报告 | `WXML_PERF_OUT` | `WXML_NATIVE_PERF_OUT` | 等价 |

现有 native perf 还额外有：release-only guard（debug 直接失败）、engine 子进程恰好 1、
无 Node exec、shutdown 后 server/engine 全部消失、tripwire 未触发
（ crates/wxml-lsp/tests/native_parity/native_engine_real.rs:3131 ）。

**一个证据准确性问题：** `native_lsp::Session::start` 明确设置
`WXML_DIAGNOSTICS_DEBOUNCE_MS=0`（
 crates/wxml-lsp/tests/native_parity/native_lsp.rs:84），且 initialize 不带
`initializationOptions`；但 native perf 报告写
`diagnostics_debounce_included_ms: 120` 并声称包含 120 ms debounce（
 crates/wxml-lsp/tests/native_parity/native_engine_real.rs:3315、
 :3353）。这是报告文案错误，不是门禁失败。retarget 时应二选一：显式传 120 ms
（与旧 gate 对齐），或保留 0 并修正报告。

---

## 5. `q2_stability`：旧轮次 vs 现有 native

旧测试是**一个 server 内的 10 轮**（
 crates/wxml-lsp/tests/q2_stability.rs:203），每轮：

| 旧轮次步骤 | 旧断言 | 现有 native 12 轮 | 差距 |
| --- | --- | --- | --- |
| 10 次连续 didChange typing burst | 最终 version 的 diagnostics | 每轮 3 次顺序 update，每次等 diagnostics | C,L |
| 请求 completion 后立即 cancel | result 或 error 至少一个 | 无 | C |
| 打开 script overlay，`name` 变 number | hover 含 number | 有（overlay hover before restart） | 等价 |
| SIGKILL Node 子进程 | 至少 1 个被杀；后续 diagnostics 不含 unavailable；hover 恢复 number | SIGKILL engine、等 PID 消失、first request class 记录、第二次 hover number、新 PID ≠ 旧 PID | 等价（native 更强） |
| 删除/恢复 `images/logo.png` + watched files | 先出现 `does not exist`，恢复后消失 | 无 | N |
| 打开 `app.json` overlay `{"pages":[]}` | hover 变 null；close 后恢复 number | 无 | N |
| close/reopen WXML | 无残留；diagnostics 正常 | 有 overlay release（script），无 template close/reopen | L,N |
| 每轮 Node 子进程 ≤ 1；记录 RSS/threads | 泄漏门 + 证据 | descendant ≤ 1；engine 恰好 1；RSS/threads/processes 记录 | 等价（native 更强） |
| 最终 40 个并发 hover | 不能出现 `server is busy` | 16 次**串行** hover | Q |
| RSS growth ≤ 15%（Rust + Node） | in-run growth | 12 轮 cycle growth + 8 次 settle steady growth ≤ 15%（server tree） | 等价（Node 维度消失） |
| 外部 RSS baseline ratio ≤ 1.15 | 可选 | 无 | BL |
| strict（默认）时 git dirty 必须为空 | lenient 仅 W3 对比时关闭 | 无 | E |
| clean shutdown | 有 | 有 + 不 detach worker + 无 sandbox + 无 Node exec | 等价（更强） |

结论：native stability 的 **engine 生命周期与泄漏覆盖已经更强**；缺的是
**document/workspace 生命周期**和 **bounded queue burst**，以及可选 baseline。

---

## 6. 最小替代测试方案

### 6.0 共同基础设施（R0，所有方案的前置）

**建议：不搬文件，用 `#[path]` 复用现有 native harness。**

每个 retarget 文件增加三件套；perf/stability 再额外加 `native_measure`：

```rust
#[path = "native_parity/native_gate_env.rs"] mod native_gate_env;
#[path = "native_parity/native_package.rs"] mod native_package;
#[path = "native_parity/native_lsp.rs"] mod native_lsp;
#[cfg(target_os = "linux")]
#[path = "native_parity/native_measure.rs"] mod native_measure;

mod ledger {
    pub(crate) fn workspace_root() -> std::path::PathBuf {
        let start = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for ancestor in start.ancestors() {
            let manifest = ancestor.join("Cargo.toml");
            let Ok(text) = std::fs::read_to_string(&manifest) else { continue };
            if text.lines().any(|line| line.trim() == "[workspace]") {
                return ancestor.to_path_buf();
            }
        }
        panic!("workspace root not found above {}", start.display());
    }
}
```

理由：

- `native_gate_env.rs` 和 `native_package.rs` 对 `crate::ledger` 的唯一依赖是
  `workspace_root`（ crates/wxml-lsp/tests/native_parity/native_gate_env.rs:30、crates/wxml-lsp/tests/native_parity/native_package.rs:19），十几行 shim
  足够。
- `native_lsp.rs` 只依赖 `crate::native_package::NativeGate`；`descendants` 是它自己的
  Linux/非 Linux 双实现（crates/wxml-lsp/tests/native_parity/native_lsp.rs:647），不需要 `native_measure`。stdio 薄壳只需
  三件套；perf/stability 再额外包含 `native_measure.rs`。
- `native_package.rs` 的 `#[cfg(test)] mod tests` 会在新 target 里再跑一遍（9 个
  无 engine 的快速单测），不影响 ignored 计数。
- **不动** `native_parity` 绿色套件，也不触发
  `crates/wxml-lsp/tests/native_parity/native_gates.rs:908` 对 harness 文件路径的 forbidden-string 检查。

备选：把 `native_gate_env.rs`/`native_package.rs`/`native_lsp.rs` 移到
`tests/common/`。语义更干净，但要同步 `native_parity.rs` 的 `#[path]` 和
`crates/wxml-lsp/tests/native_parity/native_gates.rs:908` 的文件列表，风险和工作量都更高（+0.25–0.5 人日）。本计划推荐
`#[path]` 方案。

**必须补充的 harness helper（仅测试代码）：**

| helper | 位置 | 用途 | 工作量 |
| --- | --- | --- | --- |
| `Session::request_id(method, params) -> i64` | native_lsp.rs | 支持 cancel/burst 不等待响应 | 0.1 d |
| `Session::cancel(id)` | native_lsp.rs | 发送 `$/cancelRequest` | 0.05 d |
| `Session::wait_next_diagnostics(uri)` | native_lsp.rs | watched/app overlay 后的通知 | 0.05 d |
| `Session::wait_diagnostics_matching(uri, pred, timeout)` | native_lsp.rs | 资源删除/恢复的 5 s 有界等待 | 0.1 d |
| `Session::watched(uri, kind)` | native_lsp.rs | `workspace/didChangeWatchedFiles` | 0.05 d |
| `Session::open_document(uri, language_id, text, version)` | native_lsp.rs | app.json overlay | 0.05 d |
| `Session::start_with_debounce(gate, 120)` | native_lsp.rs | perf/stability 对齐旧 120 ms | 0.1 d |

所有 helper 都是 `pub(crate)`，不改变生产代码。注意：`native_parity` 的 crate 级
clippy allow 较宽，`stdio_type_loop`/`performance_bench`/`q2_stability` 的 allow 列表
可能不够；R0 先跑一次 `cargo clippy --all-targets -- -D warnings`，按需补测试文件级
allow，不要改被 include 的模块语义。

### 6.1 9 个 stdio gate（R1，薄壳）

**文件**：原地改 crates/wxml-lsp/tests/stdio_type_loop.rs；删除 `backend_paths()`、
`Session::start_with_server`、Node env 和 `TempProject` 依赖；`write_page_fixture` 改收
`&NativeGate`。

**fixture**：与旧测试完全相同（见下表），由 `NativeGate` 写到 `/tmp/opencode` 的一次性
project 目录。

**运行模式**：`required/fixture`；在 `scripts/native-gates.sh` 之外单独跑时同样要求
`WXML_TEST_NATIVE=1` + `WXML_REQUIRE_REAL_ENGINE=1` + `TSGO_*`。

| Gate | fixture | 薄壳断言（保留 signature） | 工作量 |
| --- | --- | --- | ---: |
| 06 | `app.json` + `pages/index/index.{wxml,ts}` + `tsconfig.json`；`{{ user.name }} {{ title }}` | 空 diagnostics；`name` hover 含 `string` + `用户显示名` | 0.1 d |
| 07 | 同 06；`{{ user.nmae }}` → `{{ user.name }}` | `nmae` 恰好 1 条 error（含 `does not exist`）；修复后清空 | 0.1 d |
| 08 | 同 06；`标题😀 {{ user.nmae }}\r\n第二行 {{ user.nmae }}\r\n` | 2 条 error，UTF-16 列 13/17 与 12/16 | 0.1 d |
| 09 | 06 的 page + `orphan.wxml` = `{{ unknown.value }}` | 1 条 degradation warning；hover 为空；无 error | 0.1 d |
| 10 | `app.json` 注册 card；`index.wxml` = `<card title="{{ user.name }}" />`；`components/card/card.{wxml,ts}` | 无 error；Partial warning 含 `no \`data\` member`；`title` hover string | 0.1 d |
| 11 | `app.json`；`index.wxml` = `<card />`；`index.json` 注册 card；`card.ts` 含 Behavior data/properties；`card.wxml` = `{{ sharedCount }}{{ cardTitle }}` | 空 diagnostics；`sharedCount` number + `共享计数`；definition 指向 `card.ts`；`cardTitle` `string \| number` | 0.15 d |
| 12 | `app.json`；`index.wxml` 含 inline WXS、template、import；`index.ts`；`shared.wxml` | WXS `format` hover + `=> string` + JSDoc；definition line 1；template hover；`is="sh"` 补全 `shared`；`utils.` 补全 `format` | 0.15 d |
| 13 | `app.json`；inline WXS + `{{ u.format(title) }}`；`index.ts`；启动后删除 `<package>/engine/lib/tsc` | unavailable warning；静态 WXS hover + definition 存活；typed hover 为空 | 0.1 d |
| 14 | `app.json`；`<wxs module="u" src="./u.wxs" />` + `{{ u.label }}`；`u.wxs` | disk string → overlay number → close 恢复 string + `label` | 0.1 d |

合计 0.75–1.25 人日。**不做**：不复制 06 的 `user` 形状/range、12 的三段 completion 细节、
13 的 no-sandbox、14 的磁盘哈希；这些留在
 crates/wxml-lsp/tests/native_parity/native_engine_real.rs 的 replacement 里。

**边界断言（每个壳都加）：**

- `session.assert_no_node_exec()`；
- `session.shutdown()`：server 非零退出、stderr 出现 `detaching it` 都失败；
- `gate.tripwire().assert_absent()`。

这样薄壳保留的正是 dry-run 所说的唯一损失：stdio 进程边界。

### 6.2 `performance_bench`（R2，原地重写）

**文件**：原地改 crates/wxml-lsp/tests/performance_bench.rs；保留 `Client`、报告、环境
helper；删除 `NodeBackend` 依赖和 `slow_session` 的 Node transport；如果不再使用
`mod common;`，一并删除，避免 dead-code/clippy 噪声。

**两段式结构（同一个 ignored 函数）：**

1. **real-engine 段**：用 `native_lsp::Session`（package binary，`--backend native`）
   跑 hover/completion/diagnostics/RSS。
2. **scheduler 段**：用进程内 `run_with_host` + 测试本地 `SlowBackend`（`Backend` trait
   默认方法 + 400 ms sleep）复刻旧 `SlowTransport` 的 cancel/occupancy。它不启动
   Node，也不启动 engine；测的是同一个 server/scheduler，不冒充 engine 延迟。

**fixture**：real-engine 段使用与
 crates/wxml-lsp/tests/native_parity/native_engine_real.rs:3078 的
`write_perf_fixture` 相同的内容（`pages/index/index` + `pages/complete/complete` +
`tsconfig.json`；测试文件在另一个 target，不能直接调用该 helper）；
scheduler 段用 `app.json` + 一个 `pages/index/index.wxml`（`{{ user. }}`）+
`pages/index/index.ts`（`Page({ data: { user: { name: 'Ada' } } })`）+ `tsconfig.json`。

**运行模式**：`required/fixture`（WXML_TEST_NATIVE=1 + WXML_REQUIRE_REAL_ENGINE=1 +
TSGO_*）；release-only，debug 直接失败。

| 场景 | 实现 | 断言 | 新旧 | 工作量 |
| --- | --- | --- | --- | ---: |
| static completion | fixture `<view cla`；50 次/轮 × 200 轮 = 10,000 次；每次单独计时 | p95 ≤ 50 ms | 新增 P | 0.3 d |
| semantic completion | 32 个 `{{ user.naN }}` 前缀，200 次 | p95 ≤ 200 ms | 增强 | 0.2 d |
| semantic hover | 200 次 | p95 ≤ 200 ms | 保留 | 0 |
| diagnostics stop | 200 次 didChange，显式 debounce 120 ms | p95 ≤ 500 ms | 保留 + 对齐 120 ms | 0.2 d |
| cancel queued | SlowBackend 400 ms；busy → queued → cancel queued；30 轮 | p95 ≤ 100 ms；error code REQUEST_CANCELLED；completion 调用数 = 1/轮（open 后清零） | 新增 C,Q | 0.5 d |
| cancel running | SlowBackend；cancel 运行中的请求；30 轮 | 立即 error；p95 ≤ 100 ms；backend 调用数不增加 | 新增 C | 0.3 d |
| occupancy | SlowBackend；20 次无缓存调用 | p50 ≥ 300 ms（证明 synthetic delay 被看到）；报告 p95 | 新增 O | 0.2 d |
| baseline | `WXML_NATIVE_PERF_BASELINE` JSON；`WXML_NATIVE_PERF_REQUIRE_BASELINE=1` | 5 个 p95 +10%；scheduler 段 parent RSS +15%（等价旧 parent RSS）；real-engine 段 server/tree RSS 报告 | 新增 BL | 0.4 d |
| provenance | rustc、OS、kernel、loadavg、CPU、mem、dirty、target dir | 记录 | 新增 E | 0.2 d |
| clean worktree | `WXML_NATIVE_PERF_REQUIRE_CLEAN=1` | dirty 为空 | 新增 E | 0.1 d |
| 资源/泄漏 | server tree RSS/threads/CPU、parent RSS/threads | 报告；engine 1 个；shutdown 后无残留 | 保留 + 增强 | 0.2 d |

合计 2.0–3.0 人日。现有 native perf 的 200 样本、release-only、RSS growth、
engine/tripwire/shutdown 断言全部保留。

### 6.3 `q2_stability`（R3，原地重写）

**文件**：原地改 crates/wxml-lsp/tests/q2_stability.rs；删除 `NodeBackend`、
`node_children`、`WXML_TEST_NODE`；用 `native_lsp::Session` + `engine_children` +
`sample_tree`；保留 10 轮（可继续 12 轮）和报告；`mod common;`/`mod q2_support;`
在不再使用后删除。

**fixture**：`app.json`（`pages/index/index`）+ `pages/index/index.wxml`
（`<view><image src="images/logo.png" />{{ user.name }}</view>`）+
`pages/index/index.ts`（`Page({ data: { user: { name: 'Ada' } } })`）+
`pages/index/images/logo.png` + `tsconfig.json`；每轮再写 `app.json` overlay
`{"pages":[]}` 和 script overlay。

**运行模式**：`required/fixture`（WXML_TEST_NATIVE=1 + WXML_REQUIRE_REAL_ENGINE=1 +
TSGO_*）；debug 或 release 均可，证据以 release 为准。

| 每轮步骤 | 实现 | 断言 | 新旧 | 工作量 |
| --- | --- | --- | --- | ---: |
| typing burst + cancel | 连续 10 次 didChange（不逐次等待），然后 completion + `$/cancelRequest`，等最终 version | 最终 diagnostics 干净；cancel 得到 result 或 REQUEST_CANCELLED | 新增 C,L | 0.5 d |
| script overlay + replay | 复用 native replacement 的 12 轮逻辑（overlay number、SIGKILL engine、第二次 hover replay、新 PID） | number；新 PID ≠ 旧；1 个 engine | 保留 | 0 |
| 资源删除/恢复 | `images/logo.png`；remove + didChangeWatchedFiles → 等 `does not exist`；写回 + watched → 等消失 | 两段各有界 5 s；失败时打印完整 diagnostics | 新增 N | 0.4 d |
| app overlay 移除/恢复 | `didOpen app.json {"pages":[]}` → 等 next diagnostics → hover 为空；`didClose` → 等 next → hover 有类型 | 移除后 null；恢复后 string（本轮 overlay 已释放） | 新增 N | 0.5 d（含 probe） |
| close/reopen WXML | `didClose` + `didOpen` 基础文本 | diagnostics 干净；hover 恢复 string；无旧 overlay 残留 | 新增 L | 0.2 d |
| 40 请求 burst | 12 轮后连续发 40 个 hover id，再逐个收 | 无 `server is busy`；每个都有 result 或非 busy error | 新增 Q | 0.2 d |
| RSS/leak | 每轮 `sample_tree` + `engine_children` + `descendant_pids` | RSS > 0；cycle growth ≤ 15%；descendant ≤ 1；engine = 1 | 保留 | 0 |
| steady RSS | 保留 8 次 settle edit | steady growth ≤ 15% | 保留 | 0 |
| baseline | `WXML_NATIVE_STABILITY_BASELINE`；`..._REQUIRE_BASELINE=1` | final server 进程 RSS ratio ≤ 1.15；tree RSS 报告 | 新增 BL | 0.3 d |
| clean worktree | 默认断言 dirty 为空；`WXML_NATIVE_STABILITY_ALLOW_DIRTY=1` 只跳过 dirty 检查 | dirty 为空；其它 strict 断言不受影响 | 新增 E | 0.1 d |
| shutdown | `shutdown_with_evidence` | 不 detach worker；server/engine 全部消失；无 sandbox；tripwire 未触发 | 保留 | 0 |

合计 2.0–3.0 人日。注：native 当前每轮会关闭 script overlay（release），旧测试跨轮
保持 overlay 打开；retarget 保留 native 的 release + replay（更严），不复制旧的
“永不关闭”行为。旧的 `WXML_STABILITY_LENIENT` 会同时关闭 overlay/restart/page 断言，
不能保留；retarget 只保留一个 `WXML_NATIVE_STABILITY_ALLOW_DIRTY=1` 的本地逃生口，
其余 strict 断言始终生效。

**备选（更低工作量，不推荐单独采用）**：把 PERF/STABILITY 的新断言加到
`native_engine_real` 的既有 replacement，三个旧文件只做薄壳。这样单一真相源、约省
1.5 人日；但 gate 本身不再携带独有覆盖，评审必须接受 “replacement 是唯一 oracle”。
本计划推荐原地重写，因为 `p7_action=retarget` 的语义是让旧 gate 继续作为 gate，而不是
把它降级成 smoke。

### 6.4 runner 与证据（R4/R5）

| 项 | 改动 | 工作量 |
| --- | --- | ---: |
| `scripts/native-gates.sh` | `run_gate` 增加 `--test <target>` 参数；新增 3 组：`stdio_type_loop` 9、`performance_bench` 1（release）、`q2_stability` 1；`EXPECTED_NATIVE_GATES` 26 → 37；help/summary/CI step 文案同步 | 0.3–0.5 d |
| `.github/workflows/ci.yml` | 只改 step 文案 “26” → “26 + 11”；job 仍是 `workflow_dispatch` opt-in | 0.05 d |
| `scripts/p7-freeze.sh` | 三个 retarget 文件的 `node_dependency_json` 改为 `{p7_state:"retargeted-native", ...}` 或空；`GATE_ONLY_ENV_JSON` 去掉 3 个文件；重新生成 `docs/evidence/data/legacy-p7-freeze.json` | 0.3–0.5 d |
| ledger | `old_reason` 保留为历史（它记录旧 gate 为什么被 `#[ignore]`）；在 gate 文档或 retarget 证据里记录 native 命令；更新 PERF/STABILITY 的 `proposed_command`（当前仍是 `native_perf_gate`/`native_stability` TBD）；不改状态/计数 | 0.2 d |
| 文档 | `docs/plans/2026-11-17-tsgo-test-ledger.md`、`docs/reference/tsgo-test-contract.md`、迁移计划 P7 段落的 gate 表同步 | 0.3 d |

---

## 7. 现有覆盖 vs 必须新增（计数）

| 项 | 现有 native 直接覆盖 | 必须新增 | 说明 |
| --- | ---: | ---: | --- |
| E2E 06–14 断言 | 9/9 | 0 | native 是等价或超集 |
| E2E 进程边界（gate 文件） | 0/9 | 9 个薄壳 | 只保留 B |
| PERF 场景 | 6/11 | 5 + 1 可选 | static split、queued cancel、running cancel、occupancy、baseline（+provenance/clean） |
| STABILITY 场景 | 6/11 | 5 + 1 可选 | burst+cancel、40 请求 burst、资源、app overlay、close/reopen（+baseline/clean） |
| 新 ignored 函数 | — | **0** | `native_gates.rs` 计数方程 |
| 新测试文件 | — | **0** | 原地改 3 个文件 |

按断言组：新增 **约 10 组硬断言**（PERF 5 + STABILITY 5），另有 4 组可选
（两个 baseline、两个 clean）。

**可直接复用 vs 必须新增：**

| 来源 | 可直接覆盖 | 必须新增（测试代码） |
| --- | --- | --- |
| `native_engine_real.rs` | 9 个 E2E 的全部断言；PERF 的 hover/completion/diagnostics/RSS/cold start/engine 计数；STABILITY 的 12 轮 overlay/restart/leak/shutdown | 无（只在其外部新增或原地扩展，见下） |
| `native_measure.rs` | `Stats`、`check_p95`、`growth_percent`、`median`、`sample_tree`、`engine_children`、`kill_process`、`wait_gone`、`write_report` | 无；baseline 的 f64 p95 比较需要 10 行组合代码 |
| `native_package.rs` / `native_gate_env.rs` / `native_lsp.rs` | disposable package、required/optional 模式、LSP client、shutdown evidence | 7 个 Session helper（cancel、watched、bounded wait、debounce 等，见 §6.0） |
| `performance_bench.rs` 旧代码 | `Client`、报告、环境 helper、thresholds/baseline 结构 | `SlowBackend`（测试本地，~80 行） + static completion fixture |
| `q2_stability.rs` 旧代码 | 轮次结构、报告、RSS/baseline 结构 | `watched`/`wait_next_diagnostics`/close-reopen/app overlay 流程 |

没有需要新增的生产代码；唯一待确认的生产行为是 app.json overlay 后的 hover-null
（见 §9 风险 5）。

---

## 8. 推荐顺序与工作量

| 顺序 | 切片 | 内容 | 依赖 | 工作量 |
| ---: | --- | --- | --- | ---: |
| 1 | R0 harness 复用 | `#[path]` 三件套（perf/stability 加 `native_measure`）+ `ledger` shim + 7 个 Session helper | 无 | 0.3–0.5 d |
| 2 | R1 stdio 9 gate | 薄壳 retarget + signature 断言 + clean shutdown | R0 | 0.75–1.25 d |
| 3 | R2 perf | real-engine 段 + SlowBackend 段 + baseline/provenance | R0 | 2.0–3.0 d |
| 4 | R3 stability | typing burst/cancel、资源、app overlay、close/reopen、40 burst | R0，R2 的 cancel helper 经验 | 2.0–3.0 d |
| 5 | R4 runner | `native-gates.sh` 26→37 + CI 文案 | R1–R3 | 0.3–0.5 d |
| 6 | R5 证据 | re-freeze、ledger 文案、文档表格、全量验收 | R1–R4 | 0.5–1.0 d |
| | **合计** | | | **6.0–9.5 d** |

**为什么这个顺序：**

1. R0 先做，避免在每个文件里复制 package builder；R1 最便宜且能验证 harness 的
   required/fixture 路径。
2. R2 的 `SlowBackend` 是独立模式，先于 R3 做，R3 的 cancel 断言可以直接复用
   `Session::cancel`。
3. R4 最后做 runner，避免在 11 个测试还没稳定时反复改计数。
4. R5 必须与 retarget 同批落地，否则 `p7-freeze --check` 一定红。

**如果只要求“去掉 Node 依赖”**：R0 + 三个文件的薄壳（perf/stability 也只保留
最小 native 场景）约 **2–3 d**；代价是 PERF/STABILITY 的独有覆盖被丢弃（或被推迟到
另开的原生测试），P7-3 的覆盖门槛并不会因此关闭。不建议单独交付。

---

## 9. 风险与阻塞

1. **re-freeze 是硬前置。** 任何对三个文件的编辑都会改 `file_sha256` 和
   `line_ignore`/`line_fn`；`scripts/p7-freeze.sh:299` 的 `node_dependency_json` 仍会把
   它们写成 Node 依赖，`scripts/p7-freeze.sh:496` 的 `WXML_TEST_NODE` gate-only env
   也会过期。必须在同一变更里重新生成，且建议保留旧 freeze 作为历史文件，不要直接
   覆盖成“已 retarget”而丢失 P7-0 的 pre-delete 证据。
2. **不能新增 ignored gate。** `crates/wxml-lsp/tests/native_parity/native_gates.rs:92` 的 `IGNORED_GATE_COUNT=23` 和
   `:788` 的计数方程会把任何新增 `#[ignore]` 当作 drift；`GATE_REPLACEMENTS` 还要求
   每个 replacement 唯一链接一个 ledger gate。新增断言只能进既有函数体。
3. **R0 的备选方案会碰绿色套件。** 推荐 `#[path]` 方案不动 `native_parity`；如果改为移动
   `native_gate_env.rs`/`native_package.rs`/`native_lsp.rs`，必须同步
   `crates/wxml-lsp/tests/native_parity.rs:46` 的模块声明和 `crates/wxml-lsp/tests/native_parity/native_gates.rs:908` 的 harness 文件列表，
   否则 `read_source` 会 panic。任何 R0 改动后先跑
   `cargo test -p wxml-lsp --test native_parity`，期望与改动前一致（dry-run 基线是
   75 passed / 26 ignored）。
4. **PERF 的 cancel/occupancy 是 scheduler 级，不是 engine 级。** `SlowBackend` 复刻
   旧的 `SlowTransport`，能证明 queued/running cancel 和 bounded queue，但不能声称
   “native engine 的取消延迟”。报告和评审说明里必须写清楚，否则会被当成伪造 native
   能力。
5. **app.json overlay 的 hover-null 行为未经 native 证明。** 旧 `q2_stability` 是唯一
   断言“移除 page 后 hover null”的地方；`q2_recovery` 只断言 diagnostics 清除。
   native 的 ownerless hover 为空（E2E-09）和 workspace overlay（multi_app_projects）
   各自成立，但组合未测。R3 必须先做 probe；如果 native 仍返回类型，记录为生产 gap，
   退化为 diagnostics-only 断言，不能静默放宽。
6. **资源删除/恢复的等待必须有界。** `native_lsp::next_matching` 的超时是 30 s；12 轮
   失败会拖到 6 分钟。用 5 s 的 `wait_diagnostics_matching` 并在失败时打印完整
   diagnostics。
7. **debounce 语义要对齐。** native harness 默认 0 ms，旧 perf/stability 是 120 ms
   （initialize options 和 env 都支持，见
    crates/wxml-lsp/src/handlers/lifecycle.rs:151 和 crates/wxml-lsp/src/server.rs:598）。retarget 的
   perf/stability 显式设 120 ms；否则 diagnostics p95 和 typing burst 的语义都不是
   旧 gate 的那个。
8. **native perf 报告已有的 120 ms 文案错误要一起修。** 见 §4 末尾；不改会让新 baseline
   的解释自相矛盾。
9. **`native-gates.sh` 26→37 会改变 opt-in CI 的期望和时长。** perf 目前约 84 s、
   stability 约 1–2 min、stdio 9 个约 30–60 s；60 min timeout 足够，但 summary/help/
   CI step 文案必须同步，否则出现“26/26 绿但实际跑了 37”的假证据。
10. **retarget 不是 P7-3 的完整解。** 这三个文件只是 `backend_node` 的一部分消费者；
    `node_backend.rs`、`semantic_tools`、`q2_recovery`、`zed_sync` 等仍导入
    `wxml_lsp::backend_node`。retarget 完成后才能开始 `backend_node` 删除，但不要
    声称 P7-3 已就绪。
11. **工具链只支持 Linux x86_64。** pinned engine 是 Linux 二进制；PERF/STABILITY 仍是
    `#![cfg(target_os = "linux")]`。stdio 的 9 个 gate 原本可以靠 Node 在其它平台跑，
    retarget 后同样受 pinned engine 的平台限制；这是 engine 的既有边界，不是本次新增的
    产品限制，但会缩小 gate 的平台证据范围。
12. **共享工作树有并行未提交改动。** 本计划不触碰它们；实施时先确认
    `git status` 和 ledger/freeze 的归属，避免把别人的 wave 改动卷进 retarget 提交。

---

## 10. 验收命令（计划，不执行）

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
GATE_ENV=(env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  -u WXML_TEST_NATIVE_PACKAGE \
  WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
  TSGO_TYPINGS_ROOT="$PWD/backends/typescript/node_modules/miniprogram-api-typings")

# 1. 现有绿色套件不回归
cargo test -p wxml-lsp --test native_parity

# 2. 9 个 stdio retarget（required/fixture）
"${GATE_ENV[@]}" cargo test -p wxml-lsp --test stdio_type_loop -- --ignored --test-threads=1

# 3. perf（release）
"${GATE_ENV[@]}" cargo test -p wxml-lsp --release --test performance_bench -- --ignored --test-threads=1

# 4. stability
"${GATE_ENV[@]}" cargo test -p wxml-lsp --test q2_stability -- --ignored --test-threads=1

# 5. runner 26 + 11
scripts/native-gates.sh --package /tmp/opencode/tsgo-native-pkg

# 6. 默认套件（ignored 数量应保持不变：P7-3 删除前 49，删除后 37）
cargo test -p wxml-lsp

# 7. 冻结证据（re-freeze 之后）
scripts/p7-freeze.sh --check

# 8. 静态检查
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
git diff --check
```

---

## 11. 明确不声称

- 不声称 11 个 retarget gate 可以删除；它们必须保留旧 `#[ignore]` 测试。
- 不声称 9 个 stdio gate 的 retarget 增加了覆盖；它只保留 gate 身份和进程边界。
- 不声称 PERF/STABILITY 的现有 native replacement 已经等价；本文列出了明确缺口。
- 不声称 `SlowBackend` 的 cancel/occupancy 证明了 native engine 的取消能力。
- 不声称 app.json overlay 的 hover-null 在 native 上已成立；它是 R3 的 probe 项。
- 不声称 `scripts/p7-freeze.sh --check` 在 retarget 后会自动通过；必须重新生成。
- 不声称 P7-1（mandatory_green/reviewed/planned=0）或 P7-3（backend_node 删除）已完成。
- 本文未运行任何测试；所有结论来自源码、冻结 JSON 和 dry-run 证据的只读比对。
