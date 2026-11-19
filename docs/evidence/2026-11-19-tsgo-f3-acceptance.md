# tsgo F3 剩余验收矩阵（2026-11-19）

## 0. 结论摘要

本切片只新增验收测试与证据，不改生产代码、不 commit。真实资产 opt-in；
缺失显式 skip，`WXML_REQUIRE_REAL_ENGINE=1` fail closed。6 个新测试全部通过，
覆盖 F3-05/06/08/09/10/11/15/16。

| ID | 本次结论 | 证据类型 | 说明 |
| --- | --- | --- | --- |
| F3-05 | **PASS** | 真实包 + 单点受控变异 | 缺标准库、缺 typings、引擎探针输出错误三例均在激活前拒绝；旧 `current=0.1.0`、`state.json`、N1 树均不变 |
| F3-06 | **PASS（库 seam）+ BLOCKED（进程级）** | 真实包 + `InstallHooks` | upgrade switch 前/后、rollback switch 前/后四个窗口均验证；真实 CLI 进程在精确窗口被 kill 需要生产 failpoint seam，当前 `wxml-lsp-install` 只暴露 `NoHooks` |
| F3-08 | **PASS** | 真实 N1 + v1 fixture | `rollback --to 0.0.9` 被拒绝，未改变 current/state/N1/legacy 树 |
| F3-09 | **PASS** | 真实 N1 + v1 fixture | 迁移成功、迁移前失败、rename 后中断、switch 后中断全部验证；legacy 树字节不变，legacy 永不作为 `previous` 或 rollback 目标 |
| F3-10 | **PASS** | 真实包 + 单点受控变异 | manifest/逐文件/许可/额外文件/缺失文件/缺失许可六类篡改均在激活前拒绝，空根无 current/versions/state |
| F3-11 | **PASS** | 真实 installer archive 路径 | traversal/symlink/hardlink/PAX/解压预算五类恶意归档均经真实 `--from-dir` 获取路径拒绝，无激活 |
| F3-15 | **PASS** | 真实 native 包 + v1 fixture | legacy `current/list/verify/env` 不执行旧入口；installer `env` 只输出 `WXML_LSP_BIN`；native 树无 legacy backend/JS 入口；打包二进制忽略 legacy 环境变量 |
| F3-16 | **PASS** | 真实包 + 受控 journal | staged/committed/activated recover、并发 install、损坏 active 后 rollback 全部通过 |
| F3-02 | **不变，BLOCKED** | — | 主机仍无 `strace`/等价 `execve` 审计；PATH tripwire 与 `/proc` 观察只是辅助 |
| F3-14 | **不变，BLOCKED** | — | 未运行 Zed UI/查询 smoke；本切片不声称 F3-14 |

## 1. 身份、工作树与改动边界

- 任务指定工作区：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务指定 HEAD：`0b3b3da`
- 验收运行期间并发会话推进到：`8014739`（`feat(tsgo): add standalone script diagnostics`）
- 该提交只改 `wxml-lsp`/`wxml-backend-tsgo`，未改 `wxml-delivery`；本切片编译的
  delivery 源码与 `0b3b3da` 相同。
- 本次新增（未提交）：
  - ` crates/wxml-delivery/tests/f3_acceptance.rs`
  - ` crates/wxml-delivery/tests/f3_acceptance/`（`support.rs`、`package_guards.rs`、
    `transactions.rs`、`legacy.rs`）
  - ` scripts/f3-acceptance.sh`
  - 本文件
- 未修改 `crates/wxml-delivery/tests/f3_real_release.rs` 或其子模块；新套件通过
  `#[path]` 只读复用其 package-building support。
- 未修改生产源码、CLI、manifest、provision、Cargo 文件；未删除 Node；未切换默认
  backend；未访问网络；未 commit；未启动 review。
- 工作树在运行前已有并发会话的未提交改动（`wxml-lsp`/`wxml-backend-tsgo`），
  不属于本切片；本切片未主动编辑生产源码，也未回退这些并发改动。

## 2. 输入与真实/仿真边界

| 项 | 值 |
| --- | --- |
| 仓库 lock | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/assets.lock.json` |
| lock SHA-256 | `ce33fa960aa3eaf433419e2baeb6709587783ec11450642b6b1188c597725456` |
| 引擎 tarball | ` /tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz` |
| typings tarball | ` /tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz` |
| N1 archive SHA-256 | `3fa97f2e5c054ad9a0db6c7c5d5deb2985d516aaeaee5db128f737103c393c93` |
| N2 archive SHA-256 | `bb3c8332aa8524696b1ce3c6b9a2c48ca6077f70e0a70027cecb9f8c277dcdc8` |
| 生产 Rust/Zed 产物 | 复用 2026-11-19 F3 real-release 运行产物（`--skip-build`） |

真实/仿真标注：

- **真实包**：`wxml-lsp-package native` 从 committed lock + 真实 tarball cache
  构建；`bin/wxml-lsp`、`bin/wxml-lsp-install`、`zed-extension/extension.wasm`
  是生产二进制。
- **真实包 + 单点受控变异**：F3-05/10 的负例从真实 archive 解包后只改一个文件或
  一个字节，再重新打包并重算 archive 的 SRI/SHA-256/size 与 `index.json`；因此
  archive 完整性通过，失败发生在树校验/探针。它们不是已发布缺陷包。
- **FIXTURE**：F3-08/09/15 的 schema v1 Node 包由生产 v1 packager 生成，属于
  测试 fixture；F3-06/09/16 的 journal 与 failpoint 状态也是 fixture。测试输出以
  `F3-EVIDENCE: ... FIXTURE` 明确标注。
- **N2 限制**：没有第二个真实生产 release；N2 的 Rust 二进制是 N1 生产二进制
  仅替换同长度版本常量，engine/typings 及其它字节保持真实。该限制与
  `2026-11-19-tsgo-f3-real-release.md` 一致。

## 3. 运行命令与结果

```bash
scripts/f3-acceptance.sh \
  --output /tmp/opencode/f3-acceptance \
  --skip-build --network-isolate
```

该命令使用预构建生产 release 二进制，在 `unshare -rn` 网络命名空间内执行：

```text
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 145.11s
F3-TEST: f3_acceptance suite passed
```

日志：` /tmp/opencode/f3-acceptance/logs/f3-acceptance.log`

补充 fail-closed 检查：

```bash
TSGO_NATIVE_TARBALL=/nonexistent/engine.tgz \
TSGO_TYPINGS_TARBALL=/nonexistent/typings.tgz \
WXML_REQUIRE_REAL_ENGINE=1 \
cargo test -p wxml-delivery --test f3_acceptance f3_11 -- --ignored
```

结果：`exit 101`，`cannot resolve /nonexistent/engine.tgz`，符合 fail closed。
日志：` /tmp/opencode/f3-acceptance/logs/fail-closed.log`

## 4. 逐 ID 实测

### F3-05 缺 stdlib / 缺 typings / 引擎探针失败

```text
F3-EVIDENCE: F3-05 PASS :: missing engine/lib/lib.es2020.d.ts rejected before activation; current=0.1.0; archive=f8cfe083...
F3-EVIDENCE: F3-05 PASS :: missing typings/miniprogram-api-typings/index.d.ts rejected before activation; current=0.1.0; archive=c372bac2...
F3-EVIDENCE: F3-05 PASS :: tree-valid package with wrong engine probe output rejected before activation; current=0.1.0; archive=19a975e5...
```

三例均从已安装 N1 的根执行，失败后 `current=0.1.0`、`state.json` 字节不变、
N1 树指纹不变、`versions/` 只有 `0.1.0`、`tmp/`/`txn/` 为空。引擎探针变体通过
`verify_tree_v2`，仅 `engine/lib/tsc --version` 输出 `Version 9.9.9` 触发拒绝。

### F3-06 真实 N1→N2 与 rollback 失败/中断窗口

```text
F3-EVIDENCE: F3-06 PASS :: upgrade failpoint BeforeSwitch left N1 current, state bytes unchanged, N1 byte-identical, N2 committed but inactive
F3-EVIDENCE: F3-06 PASS :: upgrade failpoint AfterSwitch restored N1 current, kept state bytes and both real trees byte-identical
F3-EVIDENCE: F3-06 PASS :: rollback failpoint BeforeSwitch kept N2 current, state bytes unchanged and both real trees byte-identical; rollback journal stays committed
F3-EVIDENCE: F3-06 PASS :: rollback failpoint AfterSwitch restored N2 current, kept state bytes and both real trees byte-identical
F3-EVIDENCE: F3-06 BLOCKED :: a real CLI process kill exactly before/after the symlink switch needs a production failpoint env/config seam; `wxml-lsp-install` currently exposes only `NoHooks`
```

`BeforeSwitch` / `AfterSwitch` 通过公共 `InstallHooks` 注入；失败点由确定性
checkpoint 触发，不依赖 sleep/race。四个窗口均使用真实 package tree 与真实
`CommandEngineProbe`。

**仍需生产 seam 的项（BLOCKED）**：用真实 CLI 进程在精确 `current` rename 前/后
被 `SIGKILL`，当前没有生产 failpoint 环境变量或配置入口，无法在不改生产代码的
前提下确定性命中。

### F3-08 v1 rollback 拒绝

```text
F3-EVIDENCE: F3-08 PASS :: native rollback --to legacy v1 was rejected without touching current/state/or either tree
```

在真实 N1 current 上执行 `rollback --to 0.0.9`，错误包含 `legacy Node package`；
`current=0.1.0`、`state.json` 与 N1/legacy 两棵树均不变，legacy 入口未执行。

### F3-09 legacy → native 迁移

```text
F3-EVIDENCE: F3-09 PASS :: legacy fixture migrated to real N1; state.previous=null; legacy tree byte-identical; legacy entry point was not executed
F3-EVIDENCE: F3-09 PASS :: real package missing typings rejected before activation; legacy current/tree/state unchanged
F3-EVIDENCE: F3-09 PASS :: interruption after rename left legacy current and tree unchanged; recovery did not auto-activate; explicit retry migrated to N1
F3-EVIDENCE: F3-09 PASS :: interruption after switch rolled the verified native target forward; the legacy tree stayed byte-identical and was never reactivated
F3-EVIDENCE: F3-09 BLOCKED :: SIGKILL at an exact pre/post-switch instruction is not a public production seam; this evidence uses the deterministic `InstallHooks` checkpoints
```

- 成功：`migrated to 0.1.0 (native)`，`state.active=0.1.0`、`previous=null`。
- 失败：真实包缺 typings，legacy current/state/tree 均不变。
- rename 后中断：committed journal 保留，`recover` 明确 `not auto-activated`；
  显式重试才迁移。
- switch 后中断：按合同前滚到已验证 native target，legacy 树字节不变、永不恢复为
  current；journal 的 `previous` 为 `null`，legacy 从不进入 state。

### F3-10 manifest / 逐文件 / 许可 / 额外 / 缺失篡改

```text
F3-EVIDENCE: F3-10 PASS :: manifest tamper at `manifest.json` rejected before activation; ...
F3-EVIDENCE: F3-10 PASS :: payload tamper at `typings/miniprogram-api-typings/index.d.ts` rejected before activation; ...
F3-EVIDENCE: F3-10 PASS :: license tamper at `engine/LICENSE` rejected before activation; ...
F3-EVIDENCE: F3-10 PASS :: extra tamper at `engine/lib/extra-acceptance.d.ts` rejected before activation; ...
F3-EVIDENCE: F3-10 PASS :: missing tamper at `engine/lib/tsc.sig` rejected before activation; ...
F3-EVIDENCE: F3-10 PASS :: missing-license tamper at `THIRD_PARTY_LICENSES.md` rejected before activation; ...
```

每个变体的 archive SRI/SHA-256/size 已在 `index.json` 中重算，失败来自
`verify_tree_v2` 的 manifest/逐文件/文件集校验；空根没有 `current`、
`versions/` 或 `state.json`，`tmp/` 为空，root 仍为 `0700`。

### F3-11 恶意归档

```text
F3-EVIDENCE: F3-11 PASS :: traversal archive rejected before activation; detail=... `..` path component is not allowed
F3-EVIDENCE: F3-11 PASS :: symlink archive rejected before activation; detail=... entry `link` is not a regular file or directory
F3-EVIDENCE: F3-11 PASS :: hardlink archive rejected before activation; detail=... entry `link` is not a regular file or directory
F3-EVIDENCE: F3-11 PASS :: pax archive rejected before activation; detail=... PAX record has no newline
F3-EVIDENCE: F3-11 PASS :: budget archive rejected before activation; detail=... entry declares 134217729 bytes, above the per-file limit
```

恶意归档通过真实 `install --from-dir` 源路径获取，且 archive 的 SRI/SHA-256/size
与 index 记录一致；拒绝发生在 `extract_tar_gz` 的 pre-scan/entry 校验，未创建任何
激活状态。

### F3-15 legacy 环境变量与入口 inert

```text
F3-EVIDENCE: F3-15 PASS :: legacy `current` stayed read-only; ... schema: 1 (legacy, read-only; nothing executed)
F3-EVIDENCE: F3-15 PASS :: legacy `list` stayed read-only; ... * 0.0.9 (legacy, read-only)
F3-EVIDENCE: F3-15 PASS :: legacy `verify` stayed read-only; ... note: nothing was executed; no Node runtime is required
F3-EVIDENCE: F3-15 PASS :: legacy `env` printed only `export WXML_LSP_BIN=...`; no legacy backend variables and no legacy process execution
F3-EVIDENCE: F3-15 PASS :: native tree has no wxml-type-backend/JavaScript entry; native env printed only WXML_LSP_BIN; the packaged binary ignored legacy env variables
```

legacy fixture 的 `bin/wxml-lsp` 是 tripwire；安装后清 marker，随后
`current/list/verify/env` 与迁移均未重新触发。native 安装树的 `env` 只有一行
`export WXML_LSP_BIN=...`，无 `WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`/`WXML_NODE`。

### F3-16 journal recover / 并发 / 损坏 active

```text
F3-EVIDENCE: F3-16 PASS :: staged journal for 0.3.0 removed; current/state unchanged; tmp cleaned
F3-EVIDENCE: F3-16 PASS :: committed N2 target recovered without auto-activation; N1 current/state/tree unchanged
F3-EVIDENCE: F3-16 PASS :: explicit upgrade after committed recovery activated N2 and removed the retained journal
F3-EVIDENCE: F3-16 PASS :: activated journal re-verified the real engine/binary probes and rewrote a missing state.json
F3-EVIDENCE: F3-16 PASS :: rollback from a corrupted active N2 tree verified the N1 target and activated it
F3-EVIDENCE: F3-16 PASS :: two concurrent real installs both succeeded under the exclusive root lock; final current=0.1.0
```

`activated` 分支删除了 `state.json` 后从 journal 重新执行真实引擎/二进制探针并
重写 state；损坏 active 指 N2 树内的 `engine/lib/tsc.sig` 被改，rollback 仍验证
N1 target 后成功切换。

## 5. 回归、格式与 Lint

```bash
CARGO_NET_OFFLINE=true cargo test -p wxml-delivery
```

结果：所有非 opt-in delivery 测试通过。关键计数：

- lib: 103 passed
- installer_flow: 20 passed
- installer_security: 13 passed
- native_cli: 16 passed
- native_delivery: 52 passed
- native_package: 25 passed
- native_rollback: 18 passed
- provision: 47 passed
- 新 `f3_acceptance`: 0 passed, 6 ignored（默认不跑真实资产）
- 旧 `f3_real_release`: 0 passed, 7 ignored
- 0 failed

日志：` /tmp/opencode/f3-acceptance/logs/delivery-regression.log`

```bash
CARGO_NET_OFFLINE=true cargo clippy -p wxml-delivery --tests -- -D warnings
```

结果：`exit 0`，无 warning。日志：
` /tmp/opencode/f3-acceptance/logs/clippy.log`

```bash
rustfmt --edition 2024 --check <new files>
```

结果：`exit 0`。新增文件已格式化。

## 6. 未关闭 / BLOCKED

1. **F3-06 进程级 kill 窗口 — BLOCKED**：需要生产 failpoint env/config seam；
   本切片不允许改生产代码。库级 `InstallHooks` 证据已覆盖四个窗口。
2. **F3-09 精确指令级 SIGKILL — BLOCKED**：同上；已在 rename/switch checkpoint
   用确定性注入覆盖迁移中断语义。
3. **F3-05/F3-10 变异性质**：负例是真实包 + 单点受控变异，不是已发布缺陷包；
   报告已明确标注，不作为“真实发布缺陷”证据。
4. **F3-02 — 仍 BLOCKED**：无 `strace`/等价 `execve` 审计。
5. **F3-14 — 仍 BLOCKED**：未运行 Zed UI/查询 smoke。
6. **N2 版本重标**：N2 Rust 二进制仍是 N1 生产二进制替换同长度版本常量；与已有
   F3 real-release 证据一致。
7. 本次未 commit、未启动 review、未关闭任何合同/台账状态。
