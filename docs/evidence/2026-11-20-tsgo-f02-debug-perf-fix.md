# F-02：debug 全工作区 full-resend 性能测试假失败修复

- 日期：2026-11-20
- 工作区：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD：`b6c1ced`（tag `p7-removed`）；最终计数待 `p7-final-verification` 追加
- 工具链：rustc 1.100.0 / cargo 1.100.0
- 改动文件：`crates/wxml-backend-tsgo/tests/type_assets.rs`、本证据
- 未改动：生产代码、ledger、`q2_stability.rs`、release 脚本、任一门禁脚本、`docs/evidence/README.md` 索引

## 1. 问题

`fake_full_resend_ten_updates_stays_within_budget` 在 debug 全工作区下 p95 超过 200 ms，
导致 `cargo test --workspace --all-features` 出现 1 failed；同一测试在 release 下稳定通过。

修复前实测（本工作区，同一机器）：

```
# debug
full resend: p95=246.886646ms bytes=6399411..6399411
p95 full-resend update exceeded 200 ms: 246.886646ms   # FAILED
# release
full resend: p95=8.622305ms bytes=6399411..6399411     # ok
```

该测试测量的是 136 个类型资产、约 6.39 MiB 文本的完整重发路径：VFS 构建、136 个文件
open/close、`updateSnapshot` JSON 序列化。debug 下的未优化代码把这条纯 CPU/序列化路径放大约
23–25 倍，而 200 ms 是原计划针对优化构建冻结的延迟预算；用 debug 的 p95 去撞 release 预算，
失败信号来自构建 profile，而非性能回归。

## 2. 修改内容

1. 抽出共享测量逻辑 `measure_full_resends` / `p95_latency`，保留原工作负载：
   1 次预热 + 20 次测量（共 21 个样本），仍覆盖要求的 10 次更新；每次序列化都断言
   `bytes.len() <= MAX_REQUEST_BODY_BYTES`。
2. 预算常量化：`FULL_RESEND_P95_BUDGET = 200ms`，数值未变。
3. 原测试 `fake_full_resend_ten_updates_stays_within_budget`（debug/release 都运行）：
   - 功能断言保留：136 个资产、VFS 可构建、21 次重发、每次请求体在预算内、
     序列化字节在 8 字节内稳定、样本数 > 10；
   - debug：打印 `p95`，不执行延迟断言；
   - release：额外断言 `p95 <= 200ms`。
4. 新增 release-only 入口 `fake_full_resend_release_p95_stays_within_budget`：
   - 用 `#[cfg(not(debug_assertions))]` 只在 release 目标编译；**不是 `#[ignore]`**，
     也没有环境变量/过滤器开关；
   - release 下由默认命令无条件执行，并严格断言 `p95 <= 200ms` 与字节稳定性；
   - debug 下该测试不存在于测试二进制，主测试仍完整测量并打印 p95。

## 3. 为什么不是放宽预算

- 预算数值仍是 **200 ms**，没有提高阈值、没有加容差、没有改为相对基线。
- release 下现在有 **两个** 断言点（主测试 + 专用 gate）都要求 `p95 <= 200ms`，门的强度增加而非减少。
- 功能、字节和 10 次循环覆盖全部保留在 debug；debug 唯一去掉的是“用未优化构建的延迟去判断优化构建预算”这一无效断言。
- 没有新增 `#[ignore]`、没有新增 ignored 测试、没有用 `--skip`/环境变量静默绕过；release 默认测试命令必然执行该预算。
- debug 仍打印 p95（226.7 ms 量级），性能变化保持可观察，只是不再冒充 release 门禁。

## 4. 验证结果

| 命令 | 退出码 | 结果 |
| --- | --- | --- |
| `cargo test --workspace --all-features` | 0 | **待 `p7-final-verification` 追加**；既有自报为 **2592 passed / 0 failed / 73 ignored**（原始 stdout 未保留） |
| `cargo test --workspace --doc` | — | **无对应结果记录**；本文件不据此声称 doc 测试通过 |
| `cargo test -p wxml-backend-tsgo --release --test type_assets` | 0 | **11 passed / 0 failed / 0 ignored**；release gate `p95=9.302853ms <= 200ms`，主测试 `p95=10.424811ms`；bytes=6399411..6399411 |
| `cargo test -p wxml-backend-tsgo --test type_assets`（debug） | 0 | **10 passed / 0 failed / 0 ignored**；打印 `p95=226.731752ms`，不执行延迟断言；bytes=6399411..6399411 |
| `cargo fmt --all -- --check` | 0 | 无输出 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 0 | No issues found |

补充：

- 修复前 debug 全工作区失败计数为 1 failed；修复后自报 0 failed，且没有新增 ignored；最终计数待 `p7-final-verification` 追加。
- release 全量 `type_assets` 从 10 passed 变为 11 passed（新增 release-only 预算入口）。
- release p95 约 9–10 ms，相对 200 ms 预算仍有约 20 倍余量，说明预算本身没有被逼近或改写。

证据边界（2026-11-20 crosscheck）：

- **F-02 的 2592/0/73 是既有自报值，最终以 `p7-final-verification` 为准；它不是 tag `b6c1ced`/`p7-removed` 的事实**（tag 内仍是 2587/1/73）。
- 原始 `cargo test --workspace --all-features` stdout 未保留；73 个 ignored 的逐条组成未列出，不能仅凭本表证明其全部为既有 opt-in native gate。
- workspace 命令是 debug 构建；200 ms 延迟断言只在 release `type_assets` 执行，不由 workspace 结果强制。

## 5. 残余与边界

- 本测试仍只覆盖客户端 VFS + 序列化耗时；引擎 apply 时间与进程 RSS 不可在此测量，继续按原报告标为 BLOCKED，本次未改变该边界。
- debug 构建不提供延迟门禁；这是 profile 语义，不是覆盖缺失。CI/本地需要执行延迟门禁时使用：
  `cargo test -p wxml-backend-tsgo --release --test type_assets`。
