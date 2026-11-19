# tsgo native hover 与首波 native gate 证据（2026-11-19）

## 0. 记录身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 分支：`tsgo/migration`
- 记录开始 HEAD：`953ee1efc00b2e65d74e7b17276afd73181d9649`（`feat(lsp): consume native hover evidence`，2026-11-19 20:03:43 +0800）
- 记录期间 HEAD 变化：并行会话在 2026-11-19 20:10:44 提交了 `ca0b55479b12b430bd0d592a52cc96178d13ca09`（`fix(tsgo): harden trusted type asset loading`），HEAD 推进到该提交。`ca0b554` 是并行 loader 修复，不属于本文证据；本文所有运行事实仍锚定在 `953ee1e`，行号除特别注明外也以 `953ee1e` 为准。因此 `crates/wxml-backend-tsgo/src/{input,asset_loader}.rs` 的行号在当前工作树可能偏移；需要时用 `git show 953ee1e:<path>` 对照。
- 记录日期：2026-11-19
- 范围：`e303e1d`、`1e35fe8`、`a032a1f`、`953ee1e` 四个提交，以及三组已完成的运行：`wxml-lsp` lib 单元测试、真实引擎 hover smoke、首波 native gate。
- 方式：只读记录。未修改生产代码、测试、`Cargo.toml`、合同、ADR 或计划正文；未运行 `cargo`；未提交。新增本文，并在 `docs/evidence/README.md` 增加一行索引。
- 工作树状态（记录开始）：`M crates/wxml-backend-tsgo/src/asset_loader.rs`、`M crates/wxml-backend-tsgo/src/input.rs`、`M crates/wxml-backend-tsgo/tests/asset_loader.rs`、`M crates/wxml-delivery/src/lib.rs`；`?? crates/wxml-delivery/src/assets_lock.rs`、`?? crates/wxml-delivery/tests/assets_lock.rs`。这些是并行 delivery 会话的未提交 WIP，不属于 HEAD `953ee1e`；其中前三个 backend 文件在记录期间被提交为 `ca0b554`。§5 的 clippy 阻塞描述的是记录开始时的工作树。
- 记录结束时工作树又出现并行会话对 `crates/wxml-lsp/src/scheduler.rs`、`crates/wxml-lsp/src/server.rs`、`crates/wxml-lsp/src/structure.rs` 等的修改；这些同样不属于本文证据。
- 本目录 §3.2 的 P6 状态表是 `6358925` 快照，早于本文四个提交；本文不重写该表，只追加本记录。

**结论：P6 gate conversion 从 0/3 推进到 3/3 首波 native gate 绿；这是证据，不是默认切换或 F3 验收。**

具体地：

- 3 个首波 native replacement 在 opt-in native job 中通过，证明公共 `wxml-lsp --stdio --backend native` 路径可以完成静态 WXS/template 与 unknown-owner 降级行为。
- 3 个测试仍是 `#[ignore]`；ledger 中对应 gate 仍是 `required_not_migrated`；默认 `cargo test` 不运行它们。
- 默认后端仍是 Node，Node 未删除；F3、P6、G5、G6 均未关闭。
- asset loader 的 trusted pin 生产接线 / P6-3-4 仍未完成；Windows/UNC 未验证。

---

## 1. 提交序列

`git log --oneline -8` 在记录时为：

```text
953ee1e feat(lsp): consume native hover evidence
a032a1f feat(tsgo): load trusted type assets
1e35fe8 fix(lsp): dispatch partial backend capabilities
e303e1d fix(tsgo): harden native hover evidence
27bec99 feat(tsgo): wire script document symbols into lsp
...
```

记录期间并行会话提交 `ca0b554` 后，HEAD 已推进；本表仍只列 `953ee1e` 及其祖先，不把 `ca0b554` 当作本文证据。

| 提交 | 时间（+0800） | 提交信息 | 关键内容 | 本文用途 |
| --- | --- | --- | --- | --- |
| `e303e1d` | 2026-11-19 19:43:05 | `fix(tsgo): harden native hover evidence` | backend 的 `native/session.rs`、`query/hover.rs`、`query/type_info.rs`、`query/symbol.rs`、`native/handles.rs`；`tests/f1_hover.rs` 84 行变更 | 本文不单独引用其运行结果；它是 `953ee1e` 的 hover evidence 前置 |
| `1e35fe8` | 2026-11-19 19:48:23 | `fix(lsp): dispatch partial backend capabilities` | `handlers/mod.rs` 新增 `backend_available`，把 `CapabilityStatus::Partial` 视为可回答；`handlers/hover.rs`、`handlers/definition.rs` 的早退改为该判定；新增 574 行 `capability_gate_tests` | 解释为什么 adapter 的 `Partial` hover 不会被 handler 当作 unavailable 而丢弃 |
| `a032a1f` | 2026-11-19 19:49:02 | `feat(tsgo): load trusted type assets` | 新增 600 行 `asset_loader.rs`；`input.rs` 增加 `TypeAssets`/`TypeAssetsPin`/`with_environment`；新增 1,016 行 `tests/asset_loader.rs` | 证明 trusted loader 的数据层已落；不证明 adapter 生产请求已接线 |
| `953ee1e` | 2026-11-19 20:03:43 | `feat(lsp): consume native hover evidence` | `backend_tsgo/hover.rs` 改为消费单个 `HoverEvidence` 并发布后端组合的 Markdown；`sandbox.rs`/`session.rs` 配套；`backend_tsgo/tests.rs` 515 行变更；`docs/reference/tsgo-native-contract.md` §9.8 新增 7 行状态合成规则 | 本文三组运行使用的 HEAD |

提交内容是本文范围，但不是“默认切换”证据：`main.rs` 的默认 `--stdio` 仍构造 `NodeBackend`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:184 ），usage 也写明 `--backend native` 只是 opt-in（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:28 ）；同一 usage 在 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:31 写明 type assets 尚未接线。

---

## 2. `wxml-lsp` lib 单元测试

命令（工作区根目录）：

```bash
cargo test -p wxml-lsp --lib
```

结果（执行会话记录）：**668 passed / 0 failed**。

该次运行在 HEAD `953ee1e` 的工作区完成；执行会话记录并行 delivery 改动未影响该 suite。记录者未重跑该命令，原始 stdout 也未入库，因此本文只把计数作为执行会话事实，不把它升级为可复跑证据。

解释与限制：

- 668 只覆盖 `wxml-lsp` 的 lib 单元测试。它不运行 `crates/wxml-backend-tsgo/tests/*`（例如 `asset_loader.rs`、`f1_hover.rs`），也不运行 `crates/wxml-lsp/tests/*` 的 integration/parity 测试。
- `backend_tsgo::tests::optional_real_engine_hover_smoke` 是这 668 项之一；没有 `TSGO_BIN` 时该测试会打印 `optional real-engine adapter test skipped` 后直接返回，Rust 记为 passed。因此 **668 不能证明真实引擎 hover**；真实引擎结果只由第 3 节证明。
- `1e35fe8` 的 `capability_gate_tests` 属于 lib 单元测试，包含在这 668 中；它证明 handler 对 `Partial` 的分发，不证明真实 engine 或 LSP stdio。
- 运行期间工作树已有并行 WIP；该 suite 的通过不覆盖 `wxml-backend-tsgo` 的 lint（见 §5），也不覆盖 integration gate。

---

## 3. 真实引擎 adapter hover smoke

命令（工作区根目录）：

```bash
TSGO_BIN=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc \
WXML_REQUIRE_REAL_ENGINE=1 \
cargo test -p wxml-lsp --lib backend_tsgo::tests::optional_real_engine_hover_smoke
```

结果（执行会话记录）：**1 passed / 0 failed**。

环境与身份：

| 项 | 值 |
| --- | --- |
| 引擎 | TypeScript `7.1.0-beta`（native tsgo，AST 协议 v9） |
| `TSGO_BIN` | `/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc` |
| 引擎 SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70`，与 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/config.rs:44 的 pin 相同 |
| 必需模式 | `WXML_REQUIRE_REAL_ENGINE=1`；缺少 `TSGO_BIN` 会失败，不会 skip |

测试行为（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/tests.rs:3487 ）：

- 使用内联的 `SCRIPT` 与 `globals.d.ts`，不加载 `miniprogram-api-typings`，也不经过 F3 资产布局。
- 通过 `TsgoBackendAdapter` + `PinnedEngine::pinned()` + 真实 `NativeSessionFactory` 启动引擎。
- 在 `user.name` 的 `name` 上请求 hover，断言：quality 为 `Known`；status 为 `Supported` 或 `Partial`（不得静默 `Unsupported`）；range 等于模板中 `name` 的字节范围；contents 含 `string`。
- session 启动时会校验引擎可执行文件 SHA（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:379 ），所以 1 passed 也意味着该路径接受了 pin 的引擎。

它证明了什么：

- adapter 能把模板 offset 映射到 shadow script，调用真实引擎取得 `HoverEvidence`，再把类型/范围/状态映射回 LSP 结果。
- `1e35fe8` 的 `Partial` 分发路径在本测试的 `Supported`/`Partial` 断言下没有被当成 unavailable。

它不证明什么：

- 不是 LSP stdio、CLI 或 Zed 行为；只是 lib 级 adapter + 真实引擎。
- 不证明 `wx`/`Page`/`Component` 类型资产、项目级 typings、completion/definition 或 unsaved overlay。
- 不证明默认后端切换；默认仍是 Node。

---

## 4. 首波 native gates

命令（工作区根目录，单线程）：

```bash
WXML_TEST_NATIVE=1 \
WXML_REQUIRE_REAL_ENGINE=1 \
TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
cargo test -p wxml-lsp --test native_parity native_engine_real \
  -- --ignored --test-threads=1
```

结果（执行会话记录）：**3 passed / 0 failed / 36 filtered**。

计数可由仓库源码静态复核：

- `crates/wxml-lsp/tests/native_parity.rs` 与 `native_parity/*.rs` 共声明 39 个 `#[test]`。
- `native_engine_real.rs` 中有 3 个 `#[ignore]`，过滤器 `native_engine_real` 只选中这 3 个；39 − 3 = 36 filtered，与结果一致。
- 因此 `36 filtered` 包含 ledger/gate guard 等其他 parity 测试；**本命令没有运行它们**。

环境与门禁语义：

| 项 | 事实 |
| --- | --- |
| `WXML_TEST_NATIVE=1` | 显式请求。ignored gate 若没有该变量是失败，不是 skip（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gate_env.rs:12 ） |
| `WXML_REQUIRE_REAL_ENGINE=1` | 必需模式；缺包/缺引擎以 remediation panic，不会静默跳过 |
| 引擎校验 | `NativeGate::require` 会校验引擎 size/SHA-256；错误引擎始终 fatal（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_package.rs:553 ） |
| 引擎路径 | `/tmp/opencode/tsgo-native-pkg/engine/lib/tsc`，SHA-256 同为 `a183b195…` |
| 公共入口 | `Session::start` 启动 `<package>/bin/wxml-lsp --stdio --backend native`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_lsp.rs:56 ） |
| Node 隔离 | 子进程清除 `WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`/`WXML_NODE`/`TSGO_*` 等，注入 poison 值，`PATH` 只含 node/npm/npx tripwire；测试断言无 Node 后代且 tripwire marker 不存在 |

逐 gate：

| Gate | Replacement | 已通过的行为 |
| --- | --- | --- |
| `G-P5-E2E-09` | `native_ac011_unknown_owner_degrades_honestly`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_engine_real.rs:100 ） | ownerless 模板发布 1 条 message 含 `type checking is` 的 severity=2 降级 warning，且这样的 warning 恰好 1 条；消息点名 unsupported/unavailable；`value` hover 为空；无 severity=1 的伪造类型诊断；`assert_no_engine_exec` 检查该 gate 没有包内引擎子进程（该 gate 不应发起类型查询）；无 Node 后代、tripwire 未触发；session 存活时断言无 native sandbox 残留 |
| `G-P5-E2E-12` | `native_static_wxs_and_template_semantics_work_over_stdio`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_engine_real.rs:163 ） | WXS `utils.format` hover 含 `utils.format`、`=> string` 与文档 `Formats a value.`；definition 的 `range.start.line` 为 1（0-based）；`template card` hover 含 “template `card`”；import 的 `shared` definition 指向 `shared.wxml`；`is="sh"` 补全含 `shared`，`utils.` 补全含 `format`，data `{ ti` 补全含 `title`；无 Node |
| `G-P5-E2E-14` | `native_unsaved_wxs_overlay_updates_static_hover_and_completion`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_engine_real.rs:284 ） | 磁盘 WXS hover 为 `string`；未保存 overlay 改为 `label: 456` 后 hover 为 `number`，补全含 `renamed`；`didClose` 后 hover 恢复 `string` 与 `label`；磁盘文件内容未被写穿；无 Node |

它证明了什么：

- 公共 `wxml-lsp --stdio --backend native` 能在真实 pinned 引擎上完成静态 WXS/template hover、definition、completion 与 unknown-owner 降级；子进程环境没有真实 Node（`PATH` 仅 tripwire），且测试断言没有 Node 后代。
- 这不是单纯的库函数测试：JSON-RPC 帧、stdin/stdout、shutdown 和进程清理都走了真实路径。

它不证明什么：

- 3 个测试仍带 `#[ignore]`。默认 `cargo test` 不会运行它们；默认 workspace 计数只会报告 ignored。
- ledger 中 `G-P5-E2E-09`、`G-P5-E2E-12`、`G-P5-E2E-14` 的 `status` 仍是 `required_not_migrated`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json:1333 、 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json:1376 、 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json:1406 ）。`native_gates.rs` 的 `first_wave_native_replacements_are_linked_without_promoting_the_ledger` 明确要求这三个 gate 在 ledger 中保持该状态（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:216 ）；该 guard 未在本文的测试命令中执行，这里只引用其源码约束，不声称它本轮通过。
- 按 `docs/evidence/2026-11-18-tsgo-p6-gate-conversion-plan.md` §5.1 的阶梯，本轮最多是 `gate_green`（required native job + `--ignored`）；不是 `mandatory_green`（必须去掉 `#[ignore]`）、不是 `replacement_reviewed`，更不是 `closed`。
- `G-P5-E2E-12` 在 ledger 中没有 `related_ledger_cases`；即使 gate 运行绿，也不会产生 case 级 green。`G-P5-E2E-09`/`G-P5-E2E-14` 关联的 `TS-COMPLETION-0207`、`TS-ANALYSIS-0297`、`TS-OVERLAY-0023`、`TS-ANALYSIS-0161` 在 ledger 中仍是 `planned`，本轮没有晋级。
- 两个 `#[ignore]` reason 文本仍写着 `BLOCKED by P6-3 native capabilities`，与本轮在给定 package/assets 下通过的结果不一致。这是标签漂移，不影响上述命令结果，但 S8 更新 ledger/CI 时应校正，避免读者把“已能通过”误读为“仍被 P6-3 阻塞”或反过来。

---

## 5. Clippy 状态

| 命令/范围 | 时间点 | 记录结果 | 说明 |
| --- | --- | --- | --- |
| `cargo clippy -p wxml-lsp --lib --tests -- -D warnings` | hover 切片验证时 | 执行会话记录为 **no issues** | 只覆盖 `wxml-lsp` 的 lib/tests 目标；不能外推为 workspace clippy 绿 |
| 当前 workspace clippy（`953ee1e` 工作树，记录开始时） | 记录时 | **被并行 loader 修复阻塞，未绿** | 并行未提交的 `crates/wxml-backend-tsgo/src/asset_loader.rs` 工作树第 100 行触发 `clippy::unreadable_literal` |

细节：

- 当前工作树的 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/asset_loader.rs:100 是 `const O_NOFOLLOW: i32 = 0o400000;`（Linux/Android 的 `cfg` 分支）。`0o400000` 没有分隔符，clippy 报 `unreadable_literal`。
- HEAD `953ee1e` 的 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/asset_loader.rs:100 是文档注释 `/// # Errors`；该错误属于并行 WIP，不是 HEAD 的 lint 结果。但**记录开始时的工作树 clippy 确实不能声称全绿**。
- workspace 在 `Cargo.toml` 把 `clippy::all` 设为 `deny`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/Cargo.toml:46 ），`unreadable_literal` 属于该组；即使不加 `-D warnings` 也会是 error。
- 记录者未重跑 clippy（按本次指令不运行 `cargo`）；上述阻塞状态来自并行 delivery 会话在记录开始时的工作树。
- 记录期间并行会话提交了 `ca0b554`；该提交把同一字面量改为 `0o400_000`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/asset_loader.rs:103 ，按 `ca0b554` 行号），看起来正是该 lint 的修复。本文未重跑 clippy，因此**既不声称记录开始时的阻塞仍是当前状态，也不声称 `ca0b554` 之后已经全绿**。
- 因此本文只记录“hover 切片验证时 scoped clippy 无问题”，**不把它写成 workspace clippy 已通过**，也不把记录开始时的工作树写成可合并状态。

---

## 6. 已知边界与残余风险

1. **默认测试不跑这些 gate。** HEAD `953ee1e` 的 `wxml-lsp` 测试树仍有 26 个 `#[ignore]`（23 个旧 gate + 3 个本轮 native replacement）。默认 `cargo test` 只报告 ignored，不会执行 native gate。
2. **默认后端仍是 Node，Node 未删除。** `serve_real()` 仍构造 `NodeBackend`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:184 ）；`--backend native` 只是 opt-in，且缺包时 fail closed。
3. **F3/P6/G5/G6 均未关闭。** 本文只把首波 3 个 gate 的 replacement 跑绿；没有关闭任何 F3 ID，也没有完成 P6-3/P6-4，更没有关闭 G5/G6。
4. **asset loader 的 trusted pin 生产接线 / P6-3-4 未完成。** backend 已有 `TypeAssetsPin::pinned_runtime`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:466 ）和 `with_environment`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:1268 ），但 adapter 的 `RequestPlan::input` 仍调用 `AnalysisInput::new`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:121 ）。生产请求没有把 trusted loader 的结果送进 native 环境；`main.rs` usage 也仍写 “Type assets are not wired yet (P6-3)”（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:31 ）。
5. **F3 资产验收未发生。** hover smoke 用内联 `globals.d.ts`；native gate 用引擎 `lib/` 资产（`TSGO_ASSET_ROOT`），没有使用 23 文件 miniprogram typings payload，也没有验证 `assets.lock.json`/真实 tarball。本文不构成 F3-02/F3-03 或 F3 任何一项的关闭证据。
6. **G5 parity 仍为空。** 16 个 parity 分区的 `MIGRATED_CASES` 仍为空，`partition_skeleton_is_empty_and_fails_closed` 仍强制这一点；本轮没有新增 migrated case。
7. **Windows/UNC 未验证。** 仅 Linux x86_64 有证据；`asset_loader.rs` 在 HEAD 的 `#[cfg(unix)]` 分支使用 `std::os::unix::fs::MetadataExt`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/asset_loader.rs:55 ），Windows 路径语义、UNC 与 win32 文件属性未测试。本文不把 Linux 结果外推到其他平台。
8. **UI 未验证。** 原生后端没有 Zed/macOS/Windows 运行时或标准安装冒烟；历史 Node/W4 的 Zed 结果不能替代；本文的 3 个 gate 是 CLI/stdio 层，不是编辑器层。
9. **`36 filtered` 不是“其余测试通过”。** 它只表示该过滤命令没有运行其他 36 个 parity 测试，包括 ledger/gate guard。

---

## 7. 证据等级与复跑

- 本文整体标 **E0**：环境、命令、测试源、固定引擎 SHA 与结论在库内可复查，但三组运行的原始 stdout 未入库，记录者也没有重跑 `cargo`。按 `docs/evidence/README.md` 的等级定义，这不足以标 E1/E2。
- `/tmp/opencode/...` 的引擎/包目录只是执行输入，不作为仓库内唯一证据；引擎身份由 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/config.rs:44 的 SHA pin 约束。记录时对两个 `TSGO_BIN` 路径执行 `sha256sum`，结果均为 `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70`。
- 复跑时需要：
  1. pinned TypeScript `7.1.0-beta` native 引擎（`lib/tsc`），且 SHA-256 匹配 pin；
  2. `WXML_TEST_NATIVE=1` 与 `WXML_REQUIRE_REAL_ENGINE=1`（native gate）；
  3. `TSGO_BIN`，以及 native gate 的 `TSGO_ASSET_ROOT`（引擎 `lib/` 资产）；
  4. 单线程运行 native gate，避免 package/sandbox 竞争。
- 若要升级为 E1/E2，应把三组命令的完整 stdout/stderr 与引擎身份、包摘要一起保存到 `docs/evidence/data/`，并保持本文命令与环境的对应关系。

---

## 8. 结论

**P6 gate conversion 从 0/3 推进到 3/3 首波 native gate 绿；这是证据，不是默认切换或 F3 验收。**

按计划的 gate 阶梯，这组运行最多证明 `gate_green` 级别的 opt-in 结果；不证明 `mandatory_green`、`replacement_reviewed`、`closed`，也不证明 ledger 已晋级。默认后端切换、Node 删除、F3 资产验收、P6-3/P6-4 完成、G5/G6 关闭、Windows/UNC 或编辑器运行时，均不在本文范围内。
