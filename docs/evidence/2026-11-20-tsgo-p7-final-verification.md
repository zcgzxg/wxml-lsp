# P7 最终验证：workspace + native gates（2026-11-20）

## 0. 结论先行

在干净工作树 HEAD `a93086e` 上，用独立 target
`/var/tmp/opencode-p7-final-verify` 按顺序完成 9 项命令，**全部 exit 0**：

| 项 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | **PASS**，无 diff、无输出 |
| `cargo check --workspace --all-targets` | **PASS**，0 error / 0 warning |
| `cargo clippy --workspace --all-targets -- -D warnings` | **PASS**，0 warning |
| `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps` | **PASS**，0 warning |
| `cargo test --workspace --all-features` | **PASS**，**2603 passed / 0 failed / 73 ignored** |
| `cargo test --workspace --doc` | **PASS**，**1 passed / 0 failed / 0 ignored**（8 个 doctest target） |
| `cargo test -p wxml-lsp --test native_parity` | **PASS**，**79 passed / 0 failed / 29 ignored** |
| `cargo test -p wxml-backend-tsgo --release --test type_assets` | **PASS**，**11 passed / 0 failed / 0 ignored** |
| clean native gates 40/40 | **PASS 40/40**（`WXML_STABILITY_ALLOW_DIRTY` 未设置） |

关键数字：

- workspace 默认测试总计 **2602 个非 doctest 通过 + 1 个 doctest 通过 = 2603 passed**，
  **0 failed**，**73 ignored**；73 个 ignored 与仓库中 73 行真实
  `#[ignore]` 属性一一对应（40 个 opt-in native gate + 33 个 pinned
  real-asset/fixture 测试）。
- F-02 release p95：专用 gate **10.308872 ms**，主测试 **9.472676 ms**，均低于
  **200 ms** 预算；bytes 稳定为 `6399411..6399411`。
- native gates 六组全部满额：23/23 + 3/3 + 9/9 + 1/1 + 1/1 + 3/3 = **40/40**，
  引擎为 pinned TypeScript `7.1.0-beta`，SHA-256
  `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70`。
- 验证开始与结束前工作树均 clean；本文件是唯一新增文件，未 commit。

一次环境故障需如实记录：workspace 测试**第一次**因 `/tmp` tmpfs 被旧的 9.3 GB
cargo target 缓存占满而 exit 101（`StorageFull`，非断言失败），清理该可再生成
缓存后原命令复跑 exit 0。详见 §7。

## 1. 身份、范围与环境

| 项 | 值 |
| --- | --- |
| 工作树 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `a93086e9a4df8436da71af801955b9e92ae0a0fa` |
| 验证开始时 `git status --porcelain` | 空（clean） |
| Toolchain | `cargo 1.100.0` / `rustc 1.100.0`（stable，见 `rust-toolchain.toml`） |
| 独立 target | `/var/tmp/opencode-p7-final-verify`（验证后约 7.0 GB） |
| 平台 | Linux x86_64 |
| Native package | `/tmp/opencode/p7-license-release-install/versions/0.1.0` |
| Engine | `engine/lib/tsc`，`Version 7.1.0-beta`，SHA-256 `a183b195…faaa70`，24,522,912 B |
| Typings | `typings/miniprogram-api-typings` |
| 修改范围 | 仅本证据文档；未改代码、测试、README、ledger；未 commit |

所有命令均显式设置 `CARGO_TARGET_DIR=/var/tmp/opencode-p7-final-verify`，未复用
工作树内 `target/`（该目录约 28 GB，未被本次写入）。native gates 通过
`env -u WXML_STABILITY_ALLOW_DIRTY` 运行，未设置任何 dirty 豁免。

## 2. 命令与 exit code

| # | 命令 | exit | 精确结果 | 日志 |
| --- | --- | --- | --- | --- |
| 1 | `cargo fmt --all -- --check` | **0** | 空输出，无格式 diff | `/tmp/opencode/p7-final-01-fmt.log` |
| 2 | `cargo check --workspace --all-targets` | **0** | 0 error / 0 warning | `/tmp/opencode/p7-final-02-check.log` |
| 3 | `cargo clippy --workspace --all-targets -- -D warnings` | **0** | 0 warning | `/tmp/opencode/p7-final-03-clippy.log` |
| 4 | `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps` | **0** | 0 warning | `/tmp/opencode/p7-final-04-doc.log` |
| 5 | `cargo test --workspace --all-features` | **0** | 2603 passed / 0 failed / 73 ignored | `/tmp/opencode/p7-final-05-test.log` |
| 6 | `cargo test --workspace --doc` | **0** | 1 passed / 0 failed / 0 ignored | `/tmp/opencode/p7-final-06-doctest.log` |
| 7 | `cargo test -p wxml-lsp --test native_parity` | **0** | 79 passed / 0 failed / 29 ignored | `/tmp/opencode/p7-final-07-native-parity.log` |
| 8 | `cargo test -p wxml-backend-tsgo --release --test type_assets` | **0** | 11 passed / 0 failed / 0 ignored | `/tmp/opencode/p7-final-08-type-assets.log` |
| 8b | 同 8 的 `--nocapture` 诊断复跑（仅 `fake_full_resend` 两个测试） | **0** | 2 passed / 0 failed / 0 ignored；p95 见 §5 | `/tmp/opencode/p7-final-08b-type-assets-p95.log` |
| 9 | `env -u WXML_STABILITY_ALLOW_DIRTY scripts/native-gates.sh --package /tmp/opencode/p7-license-release-install/versions/0.1.0 --output /tmp/opencode/p7-final-native-gates` | **0** | **40/40 PASS** | `/tmp/opencode/p7-final-09-native-gates.log` |
| 10 | `git status --porcelain=v1`（写本文前） | **0** | 空（clean） | 本文 §7 |

步骤 5 的第 1 次尝试（环境故障）exit 101，不计入上表最终结果；见 §7。

## 3. stdout 尾部

### 3.1 fmt / check / clippy / doc

`fmt` 输出为空。其余尾部：

```text
# cargo check
    Checking wxml-delivery v0.1.0 (/…/tsgo-migration/crates/wxml-delivery)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.19s

# cargo clippy -- -D warnings
    Checking wxml-delivery v0.1.0 (/…/tsgo-migration/crates/wxml-delivery)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.57s

# cargo doc --no-deps (RUSTDOCFLAGS=-D warnings)
    Checking wxml-delivery v0.1.0 (/…/tsgo-migration/crates/wxml-delivery)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.63s
   Generated /var/tmp/opencode-p7-final-verify/doc/wxml_backend_tsgo/index.html and 9 other files
```

三项日志中 `^warning` 计数均为 0。

### 3.2 workspace 测试

```text
   Doc-tests wxml_syntax

running 1 test
test crates/wxml-syntax/src/lib.rs - (line 28) ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

all doctests ran in 0.23s; merged doctests compilation took 0.22s
   Doc-tests wxml_text

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests wxml_types

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

日志含 **65 个 `Running` target + 8 个 `Doc-tests` target = 73 条
`test result` 行**；无 `failures:` 行，无编译 warning。

聚合方式与结果：

```bash
awk '/^test result:/ {p+=$4; f+=$6; i+=$8; n++} END {printf "%d %d %d %d\n", n,p,f,i}' \
  /tmp/opencode/p7-final-05-test.log
# 73 2603 0 73
```

按 doctest 分段：

```text
non_doc: targets=65 passed=2602 failed=0 ignored=73
doc:     targets=8  passed=1    failed=0 ignored=0
```

### 3.3 doc 测试

```text
   Doc-tests wxml_types

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

8 个 doctest target 合计 **1 passed / 0 failed / 0 ignored**（唯一用例为
` crates/wxml-syntax/src/lib.rs:28`）。

### 3.4 native_parity

```text
running 108 tests
…
test fixture_hash::fixture_tree_hashes_match_frozen_contract ... ok
test native_gates::all_ignored_rust_tests_are_mandatory_gates_with_frozen_status ... ok
test ledger::ledger_migration_evidence_is_complete_and_consistent ... ok

test result: ok. 79 passed; 0 failed; 29 ignored; 0 measured; 0 filtered out; finished in 0.62s
```

### 3.5 release type_assets

```text
running 11 tests
…
test fake_full_resend_release_p95_stays_within_budget ... ok
test fake_full_resend_ten_updates_stays_within_budget ... ok

test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.19s
```

### 3.6 native gates

```text
NATIVE_GATES runner
package: /tmp/opencode/p7-license-release-install/versions/0.1.0
engine: /tmp/opencode/p7-license-release-install/versions/0.1.0/engine/lib/tsc (Version 7.1.0-beta, sha256=a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70, size=24522912)
output: /tmp/opencode/p7-final-native-gates
build: PASS
GATE native_parity::native_engine_real: PASS 23/23
GATE native_parity::symbols_edits: PASS 3/3
GATE stdio_type_loop: PASS 9/9
GATE performance_bench::G-P5-PERF-01: PASS 1/1
GATE q2_stability::G-P5-STABILITY-01: PASS 1/1
GATE native_parity::real_matrix: PASS 3/3
NATIVE_GATES: PASS 40/40
```

## 4. workspace 精确计数与 73 ignored 组成

### 4.1 按测试二进制统计 ignored

| 测试二进制 | ignored |
| --- | ---: |
| `native_parity` | 29 |
| `stdio_type_loop` | 9 |
| `f3_acceptance` | 8 |
| `f3_kill_window` | 8 |
| `f3_real_release` | 7 |
| `asset_loader` | 4 |
| `native_real_upgrade` | 3 |
| `f3_exec_audit` | 2 |
| `performance_bench` | 1 |
| `q2_stability` | 1 |
| `typings_payload` | 1 |
| **合计** | **73** |

### 4.2 按 `#[ignore]` 原因分类

仓库中**实际属性行**（`^\s*#\[ignore`，不含文档注释里的字样）恰好 **73** 行，
与运行时 ignored 一一对应：

```bash
grep -rEn --include='*.rs' '^[[:space:]]*#\[ignore' . | wc -l
# 73
```

| 类别 | 数量 | 组成 |
| --- | ---: | --- |
| Opt-in native gate | **40** | `native_engine_real` 23 + `symbols_edits` 3 + `real_matrix` 3 + `stdio_type_loop` 9 + perf 1 + stability 1 |
| Pinned real assets | **17** | `f3_acceptance` 8 + `f3_real_release` 7 + `f3_exec_audit` 2 |
| Pinned real assets + ptrace | **8** | `f3_kill_window` 8 |
| 需要 `TSGO_NATIVE_TARBALL` | **3** | `native_real_upgrade` 3 |
| 需要 `TSGO_BIN`/`TSGO_ASSET_ROOT` | **3** | `asset_loader` 3 |
| 需要 repository `node_modules` fixture | **1** | `asset_loader` 1 |
| 需要 `WXML_TYPINGS_FIXTURE_DIR` | **1** | `typings_payload` 1 |
| **合计** | **73** | — |

这 40 个 opt-in native gate 正是步骤 9 以 `--ignored` 执行并通过的 40 个测试；
其余 33 个属于 pinned real-asset / 外部 fixture 测试，本次未用 `--ignored` 执行，
也未声称通过（见 §8）。

## 5. F-02 release p95

官方步骤 8 的 11 个 release 测试全部通过；cargo 默认捕获成功测试的 stdout，因此
在同一 release 二进制上以 `--nocapture` 仅复跑两个 `fake_full_resend` 测试来读取
数值（未改代码）：

```bash
CARGO_TARGET_DIR=/var/tmp/opencode-p7-final-verify \
  cargo test -p wxml-backend-tsgo --release --test type_assets \
  fake_full_resend -- --nocapture
```

```text
running 2 tests
release full resend gate: p95=10.308872ms <= 200ms bytes=6399411..6399411
full resend: p95=9.472676ms bytes=6399411..6399411 profile=release
test fake_full_resend_release_p95_stays_within_budget ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.20s
```

| 测试 | p95 | 预算 | 字节范围 |
| --- | ---: | ---: | --- |
| `fake_full_resend_release_p95_stays_within_budget`（专用 release gate） | **10.308872 ms** | 200 ms | `6399411..6399411` |
| `fake_full_resend_ten_updates_stays_within_budget`（主测试，release 下断言） | **9.472676 ms** | 200 ms | `6399411..6399411` |

预算定义位于 ` crates/wxml-backend-tsgo/tests/type_assets.rs:568`，release 专用入口
位于 ` crates/wxml-backend-tsgo/tests/type_assets.rs:661`；测量为 20 次更新
（21 个样本），见 ` crates/wxml-backend-tsgo/tests/type_assets.rs:571`。

注意：workspace 步骤 5 是 debug profile，只打印 p95、不执行 200 ms 断言；release
预算的实际强制点是步骤 8（及本节 8b）。

## 6. Native gates 40/40 分组

| 分组 | passed/expected | 日志 |
| --- | ---: | --- |
| `native_parity::native_engine_real` | **23/23** | `/tmp/opencode/p7-final-native-gates/01-native-engine-real.log` |
| `native_parity::symbols_edits` | **3/3** | `/tmp/opencode/p7-final-native-gates/02-symbols-edits.log` |
| `stdio_type_loop` | **9/9** | `/tmp/opencode/p7-final-native-gates/03-stdio-type-loop.log` |
| `performance_bench::G-P5-PERF-01` | **1/1** | `/tmp/opencode/p7-final-native-gates/04-performance-bench.log` |
| `q2_stability::G-P5-STABILITY-01` | **1/1** | `/tmp/opencode/p7-final-native-gates/05-q2-stability.log` |
| `native_parity::real_matrix` | **3/3** | `/tmp/opencode/p7-final-native-gates/06-real-matrix.log` |
| **合计** | **40/40** | `/tmp/opencode/p7-final-native-gates/summary.txt` |

每组 `test result`：

```text
01: ok. 23 passed; 0 failed; 0 ignored; 85 filtered out; finished in 41.45s
02: ok.  3 passed; 0 failed; 0 ignored; 105 filtered out; finished in 2.35s
03: ok.  9 passed; 0 failed; 0 ignored; 9 filtered out; finished in 3.72s
04: ok.  1 passed; 0 failed; 0 ignored; 9 filtered out; finished in 185.96s
05: ok.  1 passed; 0 failed; 0 ignored; 9 filtered out; finished in 5.87s
06: ok.  3 passed; 0 failed; 0 ignored; 105 filtered out; finished in 15.64s
```

- build：`cargo build --release -p wxml-lsp`，PASS，56.79 s。
- 引擎身份：`Version 7.1.0-beta`，SHA-256 `a183b195…faaa70`，24,522,912 B；
  typings 解析为 `…/versions/0.1.0/typings/miniprogram-api-typings`。
- stability gate：命令以 `env -u WXML_STABILITY_ALLOW_DIRTY` 启动，
  `q2_stability::G-P5-STABILITY-01` 仍 PASS，说明本轮满足 clean-worktree 契约。
- `no_node_scope: NOT_REQUESTED`（本次未要求 `--verify-no-node`，不做相关声称）。

## 7. 异常、失败与环境事件

### 7.1 workspace 测试第一次尝试：`/tmp` 满（非代码失败）

第一次执行步骤 5 时 exit **101**，失败集中在 `wxml-delivery` 的
`native_delivery` 二进制（该二进制 50 passed / 2 failed）：

```text
---- upgrade_before_switch_and_before_state_write_keep_old_state_bytes stdout ----
thread '…' panicked at crates/wxml-delivery/tests/native_delivery.rs:128:40:
called `Result::unwrap()` on an `Err` value: Os { code: 28, kind: StorageFull, message: "No space left on device" }

---- hand_crafted_manifest_rejects_excluded_typings_payload_paths stdout ----
thread '…' panicked at crates/wxml-delivery/tests/native_delivery.rs:128:40:
called `Result::unwrap()` on an `Err` value: Os { code: 28, kind: StorageFull, message: "No space left on device" }
```

根因：`/tmp` 是 16 GB tmpfs，已 100% 使用；旧的
`/tmp/opencode/p7-integration-target`（9.3 GB，2026-11-20 08:29 的 cargo target
缓存）占满空间。处理：

1. 删除该**可再生成**的 cargo target 缓存（不涉及仓库文件或用户交付物）；
2. `/tmp` 可用空间恢复到约 9.4 GB；
3. 原命令完整复跑，exit 0，得到 §3.2 / §4 的 2603/0/73。

该事件的两次失败均为文件写入 `StorageFull`，没有断言失败、没有代码或测试改动。
最终结果以复跑为准，但本节保留第一次 exit 101 的事实。

### 7.2 未发现代码/测试失败

除上述环境事件外，9 项命令均一次通过；无 clippy/doc warning；无测试断言失败。

### 7.3 工作树状态

- 验证开始前：`git status --porcelain=v1` 输出为空。
- native gates 启动前再次检查：输出为空（stability gate 因此可按 clean 契约运行）。
- 写入本文件前：输出仍为空。
- 写入本文件后：预期且实际仅有
  `?? docs/evidence/2026-11-20-tsgo-p7-final-verification.md`；其余代码、测试、
  README、ledger 均未改动，未 commit。

## 8. 边界与未声称

1. 本记录只覆盖上述 9 项命令与本机环境；未重新执行 release 打包、安装/升级/回滚、
   Zed smoke、无 Node 安装器检查或跨平台验收。
2. 73 个 ignored 中，本次只有 40 个 opt-in native gate 在步骤 9 以 `--ignored`
   实际执行；其余 33 个 pinned real-asset / 外部 fixture 测试未被本记录执行或
   声称通过。
3. workspace 命令是 debug profile；200 ms F-02 预算只在 release `type_assets`
   （步骤 8/8b）强制。
4. `--nocapture` 复跑用于读取 p95；它不替代步骤 8 的完整 11 项 release 结果，二者
   共同构成本节证据。
5. 未做 execve/strace 级审计；native gates 的引擎身份来自脚本强校验的版本、
   SHA-256 与大小。
6. 本文件不改变 ledger/README 的既有结论，也不构成 commit 或发布动作。

## 9. 日志与身份索引

| 文件 | 大小（B） | SHA-256（关键项） |
| --- | ---: | --- |
| `/tmp/opencode/p7-final-01-fmt.log` | 0 | —（空） |
| `/tmp/opencode/p7-final-02-check.log` | 192 | — |
| `/tmp/opencode/p7-final-03-clippy.log` | 192 | — |
| `/tmp/opencode/p7-final-04-doc.log` | 410 | — |
| `/tmp/opencode/p7-final-05-test.log` | 275,946 | `3d41b79b11532652fb8f880b7586727b6c8731c850e5ecea81d8481be5d103e0` |
| `/tmp/opencode/p7-final-06-doctest.log` | 1,301 | — |
| `/tmp/opencode/p7-final-07-native-parity.log` | 11,155 | — |
| `/tmp/opencode/p7-final-08-type-assets.log` | 1,024 | `9725093b3a3fffad72c8ac953635fb1410e7df37e5234c570fa0a68b49204404` |
| `/tmp/opencode/p7-final-08b-type-assets-p95.log` | 590 | `ab3cc98a5ec054375ff620367083470da5fd3c79dd72f8208d25c94ea096d3a6` |
| `/tmp/opencode/p7-final-09-native-gates.log` | 956 | `44b110dd68c7ed4adbe60e88463f0bbb26e5003d4ef9094336b4ebd708064024` |
| `/tmp/opencode/p7-final-native-gates/00-build.log` | 332 | — |
| `/tmp/opencode/p7-final-native-gates/summary.txt` | 897 | `3cceb6364676735ce9952a3fa77e0c9ade306ae89f5b2917eb2fa016b0e21052` |

验证时间：2026-11-20 11:21（+08:00）。HEAD
`a93086e9a4df8436da71af801955b9e92ae0a0fa`；独立 target
`/var/tmp/opencode-p7-final-verify`。
