# P7 删除前 native 独立基线（2026-11-20）

## 0. 结论先行

1. **native 功能、交付与门禁在当前 HEAD 可用。** `cargo fmt --check`、`cargo check`、
   `cargo clippy -D warnings` 全部 exit 0；workspace 全量测试 **2813 passed / 0 failed /
   87 ignored**；`native_parity` **78 passed / 29 ignored**；真实 package 的
   `scripts/native-gates.sh` **35/35 PASS**；无 package 负例 **exit 2**。
2. **release 构建与包验证通过（未安装）。** `wxml-lsp-0.1.0-linux-x86_64.tar.gz`
   sha256 `76f95a30…`，157 files，installer `verify-package` 通过。
3. **唯一不符合预期的是 `scripts/p7-freeze.sh --check`：exit 2，不是 OK。**
   冻结 JSON 仍以 `befba03` 为 `requested_head`，而其后 `b9291a0`、`b0f6cc1` 修改了
   冻结路径（ledger、`native_parity.rs`、`stdio_type_loop.rs` 等），且冻结 JSON 是在
   dirty 工作树生成的（`frozen_paths_clean=false`）。这是**冻结清单过期**，不是 native
   功能失败；恢复为绿需要重新生成 `docs/evidence/data/legacy-p7-freeze.json`
   （仓库写入，超出本只读切片）。
4. **Node 仍存在；服务端默认不运行，但默认测试套件并非 Node-free。**
   `node v24.22.2`、`backends/typescript/dist`（34 文件）与 `node_modules` 都在；
   `--stdio` 默认 native，Node 仅在显式 `--backend node`（deprecated）时使用。
   release 构建与 35 个 native gate 未执行 Node；但 `cargo test --workspace` 因旧
   `dist/server.js` 存在，默认执行了 2 个 `semantic_tools` 的真实 Node 测试
   （`G-P5-E2E-22/23` 两个 silent-skip gate）。删掉旧树后这两个测试会静默 return，
   测试仍“绿”但不再断言——这是 P7 已知阻塞项，不是 native 失败。
5. 未改生产代码、测试、ledger、README、脚本；未安装 release；未 commit。

因此：**P7 删除前的 native 基线成立；freeze 身份检查 BLOCKED，需在删除前显式重新冻结。**

---

## 1. 身份与环境

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `b0f6cc12e3bc58b2edac8714cf57b9634846240f` |
| HEAD subject | `test(p7): retarget stdio gates to native` |
| Branch | `tsgo/migration` |
| 工作树 | 无 tracked 修改；仅有其他会话的未跟踪 `docs/evidence/*.md` |
| rustc / cargo | `1.100.0`（`0ef1bc386 2026-11-09` / `3acc79c0c 2026-11-05`） |
| OS | Linux 7.2.15+deb14-amd64 x86_64 |
| Node | `/usr/bin/node`，`v24.22.2`（存在；服务端默认不执行，默认测试套件执行 2 个 legacy Node 测试） |
| 真实 package | `/tmp/opencode/tsgo-native-pkg` |
| engine | `<package>/engine/lib/tsc`，`Version 7.1.0-beta`，sha256 `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70`，size `24522912` |
| typings | `<package>/typings/miniprogram-api-typings` |

各命令执行前显式清除 `WXML_TEST_NODE`、`WXML_TEST_NATIVE`、`TSGO_*`、
`WXML_REQUIRE_REAL_ENGINE`（release/gate 由脚本自行注入），避免上一次实验环境污染默认路径。

---

## 2. 静态门禁

| 命令 | 结果 | exit |
| --- | --- | --- |
| `cargo fmt --all -- --check` | 无输出 | 0 |
| `cargo check --workspace --all-targets` | `Finished dev profile … in 2.74s` | 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | `No issues found` | 0 |

---

## 3. workspace 全量测试

命令：

```text
cargo test --workspace --all-features
```

日志：`/tmp/opencode/p7-predeletion-workspace.log`（305.5 KiB）。

| 指标 | 值 |
| --- | --- |
| test result 行 | 86（78 个 `Running` + 8 个 `Doc-tests`） |
| passed | **2813** |
| failed | **0** |
| ignored | **87** |
| exit code | **0** |

`ignored` 分布（全部是既有 opt-in gate/真实资产测试，默认不运行）：

| ignored | target |
| ---: | --- |
| 29 | `tests/native_parity.rs` |
| 9 | `tests/stdio_type_loop.rs` |
| 8 | `tests/f3_acceptance.rs` |
| 8 | `tests/f3_kill_window.rs` |
| 7 | `tests/f3_real_release.rs` |
| 7 | `tests/wave5_acceptance.rs` |
| 5 | `tests/real_node.rs` |
| 4 | `tests/asset_loader.rs` |
| 3 | `tests/native_real_upgrade.rs` |
| 2 | `tests/f3_exec_audit.rs` |
| 2 | `tests/f3_legacy_real.rs` |
| 1 | `tests/typings_payload.rs` |
| 1 | `tests/performance_bench.rs`（`G-P5-PERF-01`） |
| 1 | `tests/q2_stability.rs`（`G-P5-STABILITY-01`） |

其中 `native_parity.rs` 的 78 passed / 29 ignored 单独复跑结果一致（见 §5）。

注意：由于 `backends/typescript/dist/server.js` 仍存在，默认 workspace 套件会执行
`semantic_tools` 中两个**非 ignored**的真实 Node 测试
（`real_node_lsp_component_property_edit_applies_and_parses`、
`real_node_lsp_rejects_cross_namespace_and_dynamic_edits`，即 ledger 的
`G-P5-E2E-22/23` silent-skip gate），日志中二者均为 `ok`。这两项不是 native 门禁。

---

## 4. 真实 native package 的 35 个 gate

命令：

```text
scripts/native-gates.sh \
  --package /tmp/opencode/tsgo-native-pkg \
  --output /tmp/opencode/p7-native-gates
```

脚本注入的 pinned 环境：

| 变量 | 值 |
| --- | --- |
| `WXML_TEST_NATIVE` | `1` |
| `WXML_REQUIRE_REAL_ENGINE` | `1` |
| `TSGO_BIN` | `/tmp/opencode/tsgo-native-pkg/engine/lib/tsc` |
| `TSGO_ASSET_ROOT` | `/tmp/opencode/tsgo-native-pkg/engine` |
| `TSGO_TYPINGS_ROOT` | `/tmp/opencode/tsgo-native-pkg/typings/miniprogram-api-typings` |
| `WXML_TEST_NATIVE_PACKAGE` | 故意不导出（E2E-13 会删除自己的 disposable package engine） |

结果：**exit 0，35/35 PASS**。

| Gate 组 | 命令目标 | 结果 | 日志 |
| --- | --- | --- | --- |
| native engine | `native_parity::native_engine_real`（filter） | `23 passed; 0 failed`（84 filtered） | `/tmp/opencode/p7-native-gates/01-native-engine-real.log` |
| symbols/edits | `native_parity::symbols_edits`（filter） | `3 passed; 0 failed`（104 filtered） | `/tmp/opencode/p7-native-gates/02-symbols-edits.log` |
| stdio shells | `stdio_type_loop`（无 filter） | `9 passed; 0 failed`（9 filtered） | `/tmp/opencode/p7-native-gates/03-stdio-type-loop.log` |

release build：`cargo build --release -p wxml-lsp` 通过（`00-build.log`，38.36s）。
runner 汇总：`/tmp/opencode/p7-native-gates/summary.txt`，记录 head、engine 身份、
三组 `PASS`，`no_node_scope: NOT_REQUESTED`。

负例（未提供 `--package`）：

```text
scripts/native-gates.sh
ERROR: --package is required
Run scripts/native-gates.sh --help for usage.
exit 2
```

日志：`/tmp/opencode/p7-native-gates-negative.log`。

---

## 5. native_parity 与 freeze

### 5.1 native_parity

```text
cargo test -p wxml-lsp --test native_parity
test result: ok. 78 passed; 0 failed; 29 ignored; 0 measured; 0 filtered out
exit 0
```

与期望的 **78 passed / 29 ignored** 一致。

### 5.2 p7-freeze：FAIL（BLOCKED，非 native 失败）

```text
scripts/p7-freeze.sh --check
p7-freeze: frozen paths changed between befba03e60f9c65e20e75b0c4c873d828fcb61b3 and b0f6cc12e3bc58b2edac8714cf57b9634846240f; freeze the reviewed baseline deliberately
exit 2
```

原因链（可复现）：

1. `docs/evidence/data/legacy-p7-freeze.json` 的 `commit.requested_head` 与 `commit.head`
   都是 `befba03`，`generated_at=2026-11-20T05:38:29+08:00`。
2. 其后两个提交修改了冻结路径：
   - `b9291a0 feat(tsgo): add native scenario matrix parity`：
     `native_parity.rs`、`native_parity/ledger.rs`、`native_parity/native_gates.rs`、
     `docs/plans/tsgo-test-ledger.json` 等；
   - `b0f6cc1 test(p7): retarget stdio gates to native`：
     `stdio_type_loop.rs`。
3. 冻结 JSON 自身记录 `frozen_paths_clean=false`，说明它是在 `befba03` 的 dirty 工作树上
   生成的（当时 stdio retarget 尚未提交）；提交后 `--check` 的
   `git diff --quiet requested_head..HEAD -- <frozen paths>` 守卫必然失败。
4. 使用 `P7_REQUESTED_HEAD=b0f6cc1` 将当前工作树生成到
   `/tmp/opencode/p7-freeze-current.json`（不写仓库）后，与冻结 JSON 做归一化 diff，
   仅剩 20 行内容差异：
   - ledger `83 green / 218 planned / 38 retired` → `109 green / 192 planned / 38 retired`；
   - `native_parity/ledger.rs` `74430 bytes / f423cacb…` → `80065 bytes / c233734c…`，
     两个 source anchor 行号 `1667/1679` → `1807/1819`；
   - `frozen_paths_clean` `false` → `true`。
   `trees`（`backends/typescript`、其 `src`/`test`/`dist`、`backend_node`）、23 个 gate
   清单、legacy env、v1 写路径、oracle 绑定均未漂移。

因此这不是 35 个 native gate 或 workspace 测试的问题，而是**冻结清单落后两个 retarget
提交**。要让 `--check` 恢复 OK，必须重新生成并提交
`docs/evidence/data/legacy-p7-freeze.json`；该操作会写仓库，本只读切片不做。

辅助文件：`/tmp/opencode/p7-freeze-drift.diff`、`/tmp/opencode/p7-freeze-current.json`。

---

## 6. 可选 release 构建（未安装）

命令（复用 P6-7 的 cargo/zed target 与已验证 asset cache，仍从 HEAD 源码构建；
`wasm32-wasip2` 已安装，因此完整构建 Zed 扩展，未使用 `--skip-zed`）：

```text
WXML_BUILD_CACHE=/tmp/opencode/p6-7-release/cache \
CARGO_TARGET_DIR=/tmp/opencode/p6-7-release/target \
ZED_CARGO_TARGET_DIR=/tmp/opencode/p6-7-release/zed-target \
scripts/build-release.sh \
  --output /tmp/opencode/p7-predeletion-release \
  --from-dir /tmp/opencode/p6-7-release/from-dir
```

日志：`/tmp/opencode/p7-predeletion-build.log`。构建 exit 0，脚本内
`wxml-lsp-install verify-package` 与 `SHA256SUMS` 校验均通过；**未执行 install**。

| 产物 | 值 |
| --- | --- |
| package | `wxml-lsp 0.1.0 linux-x86_64`，protocol 2，157 files |
| builtAt | `2026-11-19T22:53:26Z` |
| engine | TypeScript `7.1.0-beta`，binary `engine/lib/tsc` |
| archive | `wxml-lsp-0.1.0-linux-x86_64.tar.gz`，15354060 bytes |
| archive sha256 | `76f95a300e3c460897ffa1ee2900fc6dbe372bc3b046fc3aa8e473f8d0036ab2` |
| archive SRI | `sha512-i61rg082kl0HWZeXee2da60sxefsNvNCcPT0G6Fe5TgfriOYt+QN+mlsnnoWEZKA+Wh9kbkCgYCo9u5EcnqfEQ==` |
| manifest.json sha256 | `436aa68b43cbac34b673521d1573a95fab669ddf32e9c67c2e8541dc5beb59b2` |
| provenance.json sha256 | `b3001987c54845fd42f1c19484f78bdc4ccf9442f73bb25b78612764826addf7` |
| index.json sha256 | `e5e5c7c16c8a46122eb1e79766426f0585c56c71df32d63314b145177a3d1005` |
| SHA256SUMS sha256 | `9f9a89dbf5dbaf856242483899f5945a6f909977e7ac7dcf872e0571ffbe1926` |

构建日志末尾明确：`node: not executed (PATH tripwire clean)`。

---

## 7. Node 存在性与默认路径

| 证据 | 结论 |
| --- | --- |
| `node --version` → `v24.22.2` | Node 仍安装 |
| `backends/typescript/dist`（34 文件）、`backends/typescript/node_modules` | 旧 TS/Node 交付树仍存在 |
| ` crates/wxml-lsp/src/main.rs:25` | `--backend <backend> … native (default) or node` |
| ` crates/wxml-lsp/src/main.rs:201` | 无 `--backend` 走 `run_stdio_native_default()` |
| ` crates/wxml-lsp/src/main.rs:243` | 单测 `stdio_defaults_to_native` |
| `cargo test --workspace` | `no_node_guard` 4 passed：installer 在 Node tripwire PATH 下工作；静态检查 `serve_default` 不构造 `NodeBackend`，`serve_node` 仍显式构造 |
| `semantic_tools`（默认套件） | 因 `dist/server.js` 存在，2 个非 ignored 测试真实启动 Node 并通过：`real_node_lsp_component_property_edit_applies_and_parses`、`real_node_lsp_rejects_cross_namespace_and_dynamic_edits` |
| 空 PATH 单测复跑 | 直接运行 `semantic_tools` 二进制、`PATH=/tmp/opencode/p7-empty-path` 时，`real_node_lsp_component_property_edit_applies_and_parses` exit 101，报 `type backend unavailable: node failed to start`；证明默认套件确实执行了 Node |
| `native-gates` / release build | 未执行 Node；gate 子进程还注入 poison 与 tripwire，构建日志 `node: not executed (PATH tripwire clean)` |

边界：`no_node_guard` 自述其 PATH shim 不能排除绝对路径 Node 调用，也不替代 F3-02 的
`strace` 审计；本次未重新运行 `strace`。Node 代码与 `backends/typescript` 均未删除。
空 PATH 复跑日志：`/tmp/opencode/p7-semantic-node-onpath.log`（未改代码，只验证行为）。

---

## 8. 失败、BLOCKED 与未证明项

1. **BLOCKED：freeze 身份检查。** `scripts/p7-freeze.sh --check` exit 2，原因是冻结 JSON
   落后 `b9291a0` + `b0f6cc1` 两个提交；需要重新生成冻结证据。此项未通过。
2. **删除仍未授权。** 冻结 JSON 仍记录 `deletion_allowed=false`、23 个旧 gate 仍
   `#[ignore]`、`mandatory_green=false`、replacement 未 review；ledger 当前
   109 green / 192 planned / 38 retired，planned 非 0。
3. **默认测试不执行 opt-in gate。** 87 ignored 未在 `cargo test --workspace` 中运行；
   35 个 native gate 的结果来自显式 `--ignored` 运行，不是默认 CI。
4. **默认套件仍依赖旧 Node 树才能完整跑完。** `G-P5-E2E-22/23` 两个
   `semantic_tools` 测试不是 `#[ignore]`，只在 `dist/server.js` 缺失时静默
   `return`。当前它们真实执行 Node 并通过；删除旧树后它们会变成 silent skip，
   workspace 仍绿但覆盖变弱。freeze 已把这两个 `silent_skip_gates` 记录为阻塞项。
5. **未证明：**
   - freeze JSON 与 HEAD 的身份一致（当前不一致）；
   - 删除 Node 后的行为（本切片未删除、未模拟删除）；
   - 29 个 `native_parity` ignored gate 的真实引擎路径之外的其他平台；
   - 安装后的运行路径（release 只构建和 verify，未 install）。
6. workspace 测试的 87 ignored 中包含 PERF/STABILITY 与旧 Node gate；它们未被本基线
   转化为 mandatory green。

---

## 9. 复现命令

```text
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration

cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-features \
  > /tmp/opencode/p7-predeletion-workspace.log 2>&1

scripts/native-gates.sh \
  --package /tmp/opencode/tsgo-native-pkg \
  --output /tmp/opencode/p7-native-gates
scripts/native-gates.sh            # 期望 exit 2：--package is required

cargo test -p wxml-lsp --test native_parity
scripts/p7-freeze.sh --check       # 当前 exit 2（见 §5.2）
```

---

## 10. 变更边界

- 本切片只新增本文；未修改生产代码、测试、脚本、ledger、README、delivery。
- 未 commit，未 review，未安装 release，未删除 Node 或 `backends/typescript`。
- 所有构建/测试产物、日志和可选 release 均在 `/tmp/opencode/` 下，未在仓库内创建临时文件。
