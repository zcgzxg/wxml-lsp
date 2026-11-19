# tsgo 测试台账回填与 native gate 复核（P6，2026-11-19）

## 0. 记录身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD：`9a8b65caac38e2d04a08550e0506408f7af967f5`（`fix(delivery): remove node from legacy installer paths`）
- 记录日期：2026-11-19
- 范围：只改测试台账（`docs/plans/tsgo-test-ledger.json`）、ledger validator
  （`crates/wxml-lsp/tests/native_parity/{ledger,native_gates,native_engine_real}.rs`）、
  Markdown 台账、contract 计数和证据文档。未修改生产代码、`crates/wxml-delivery/**`、
  `crates/wxml-backend-tsgo/**`、`crates/wxml-lsp/src/**`、Cargo 或默认后端；未提交。
- 工作树状态：记录和运行期间存在另一 P6-3-4 会话的未提交改动
  （`crates/wxml-backend-tsgo/src/native/*`、`crates/wxml-lsp/src/backend_tsgo/*`、
  `crates/wxml-delivery/src/*` 等）。本记录不把这些改动当作已落地能力，也不修改它们。
- 历史快照 `docs/evidence/2026-11-19-tsgo-native-gate-status.md` 未被改写；该记录中
  “当前工作树复跑被 type-assets WIP 阻塞”的结论由本记录追加更新。

**结论：case 级从 33 `green` / 270 `planned` 推进到 36 `green` / 267 `planned` /
36 `retired-with-decision`；3 个既有 `gate_green` 在带 `TSGO_TYPINGS_ROOT` 的当前工作树
复跑仍为 3 passed / 36 filtered；20 个 gate 仍没有 native replacement，保持
`required_not_migrated`。这不是默认 CI、不是 `mandatory_green`，也不关闭 P6/G5/G6/F3。**

---

## 1. 用例回填结果

### 1.1 新增 `green`（3，全部 `parity=exact`）

| Case | Rust 证据 | 判定 |
| --- | --- | --- |
| `TS-COMPLETION-0175` |  `crates/wxml-lsp/src/backend_tsgo/tests.rs:4185`<br> `crates/wxml-lsp/src/backend_tsgo/event_completion/tests.rs:166` | TS 断言 Supported、`onTap`/`onLoad` 标签、Method kind、replace 长度 2。Rust adapter 测试用同一 Page literal 断言精确标签集合、Method kind、2 字节前缀，并额外证明不启动 native session；单元测试还覆盖 arrow 形。断言强度不低于 TS。 |
| `TS-COMPLETION-0193` |  `crates/wxml-lsp/src/backend_tsgo/event_completion/tests.rs:188` | TS 断言 Supported 且标签包含 `save`。Rust 用 Component `methods` 断言精确方法表、排除 `attached`/`lifetimes`，并断言 Supported。同一可观察语义，强度更高。 |
| `TS-PROBES-0169` |  `crates/wxml-lsp/src/backend_tsgo/environment/tests.rs:136`<br> `crates/wxml-lsp/src/backend_tsgo/environment/tests.rs:159`<br> `crates/wxml-lsp/src/backend_tsgo/environment/gate.rs:449`<br> `crates/wxml-lsp/src/backend_tsgo/environment/gate.rs:458` | TS 只断言缺失后端时抛出 `EnvironmentError`。Rust 断言缺失 typings 根为 `RootUnavailable`、空 lib/typings 树为 `MissingRequired`，required gate 对缺失 `lib.d.ts` 或 `TSGO_TYPINGS_ROOT` panic。同一 fail-fast 语义，错误类型更精确。 |

`TS-COMPLETION-0175` 同时被 `G-P5-E2E-02`/`G-P5-E2E-03` 引用；这两个 gate 仍是
`required_not_migrated`。case 的 green 只来自 completion 层的 exact 证据，不代表 E2E gate
green；两个状态轴保持分离。

### 1.2 新增或更新的 `weaker` 证据（保持 `planned`）

| Case | Rust 证据 | 为什么不是 exact |
| --- | --- | --- |
| `TS-COMPLETION-0207` |  `crates/wxml-lsp/src/backend_tsgo/tests.rs:2172`<br> `crates/wxml-lsp/src/backend_tsgo/event_completion/tests.rs:539`<br> `crates/wxml-lsp/src/backend_tsgo/event_completion/tests.rs:524` | TS 用 unavailable layout 触发 `Unsupported`、空列表、`truncated=false`。Rust 的 target=None 返回 `Unsupported`，缺失 owning script 返回 `Unavailable` 错误；触发方式和结果形状不同，仍缺 `truncated=false`。 |
| `TS-META-0009` |  `crates/wxml-delivery/tests/assets_lock_root.rs:130`<br> `crates/wxml-delivery/tests/native_delivery.rs:650` | `assets.lock.json` 冻结 native engine `7.1.0-beta` 和 typings `5.3.1` 的 tarball/SRI/SHA-256/payload digest。TS oracle 断言旧 Node TypeScript `6.0.3`、`package.json` 依赖范围和 `@types/node` dev-only 项；pin 的意图保留，可观察版本和字段不同。 |
| `TS-SERVER-0237` |  `crates/wxml-delivery/tests/installer_flow.rs:598`<br> `crates/wxml-delivery/tests/native_cli.rs:391` | TS 断言 `resolveBackendDir` 支持 source/dist/packaged Node 后端目录。去 Node installer 已删除该 resolver；Rust 只证明 native install/verify/upgrade/rollback 在无 `node` 的 PATH 下工作，以及 legacy read-only 命令不读取 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`。机制被替换，不是同一可观察结果。 |

### 1.3 只记录、不进入 ledger 的原生面

- **document symbols**：`crates/wxml-lsp/tests/native_lsp_document_symbols.rs` 是 native-only
  LSP gate，TS oracle 没有 `textDocument/documentSymbol` 用例，因此没有可回填的 case ID；
  不把 `TS-SYMBOLS-*`（SymbolRequest）当作 document symbols。
- **de-Node installer / F3 lock/cache/root lock**：`assets.lock.json`、
  `crates/wxml-delivery` 的 package/verify/installer 测试属于交付合同，不是 TS 测试套件的
  oracle；只有 `TS-META-0009` 和 `TS-SERVER-0237` 有语义关联，已按 weaker 登记。

---

## 2. Native gate 复核

### 2.1 结果

| 项 | 数量 | 说明 |
| --- | ---: | --- |
| `gate_green` | 3 | `G-P5-E2E-09/12/14`；本次在当前工作树复跑 3 passed |
| `required_not_migrated` | 20 | 没有任何新 native replacement，状态不变 |
| 合计 | 23 | 未删除、合并或改名任何 gate |

**本轮新增 `gate_green`：0。** 逐项检查 20 个 required gate 后，没有发现新的 native
replacement 测试；`native_engine_real.rs` 仍只声明 3 个 replacement，
`symbols_edits.rs` 的 3 个 edit gate 仍是 `BLOCKED_NATIVE_GATES`，没有 `#[test]`。

### 2.2 复跑命令与结果

```bash
WXML_TEST_NATIVE=1 \
WXML_REQUIRE_REAL_ENGINE=1 \
TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
TSGO_TYPINGS_ROOT=/tmp/opencode/assets-lock-root/typings \
cargo test -p wxml-lsp --test native_parity native_engine_real \
  -- --ignored --test-threads=1
```

结果：**3 passed / 0 failed / 36 filtered**，exit 0。通过项：

- `native_ac011_unknown_owner_degrades_honestly`
- `native_static_wxs_and_template_semantics_work_over_stdio`
- `native_unsaved_wxs_overlay_updates_static_hover_and_completion`

引擎身份与 ledger 的 `gate_evidence` 一致：TypeScript `7.1.0-beta`，git head
`3e9236361fa97a9e6c685f4833ac457ae2d8c50a`，AST `9`，`lib/tsc` SHA-256
`a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70`，大小 24,522,912 B。
typings 根为 `miniprogram-api-typings` 5.3.1 的上游目录（26 个文件，其中 20 个
`.d.ts` 进入 asset loader；其余非声明文件被 loader 跳过）。

**限制：** 运行 HEAD 是 `9a8b65c` 加并行会话未提交改动；不是干净 HEAD，也不是默认 CI。
3 个测试仍是 `#[ignore]`，只有 `--ignored` required native job 会执行。没有一条命令在
不带 `WXML_TEST_NATIVE=1` 时被执行并声称通过。

### 2.3 仍 BLOCKED 的 gate

20 个 required gate 按原因分组：

- `G-P5-E2E-01`：non-owner script overlay + native restart/epoch。
- `G-P5-E2E-02/03`：event-handler definition/diagnostics 或完整 E2E；event completion 已完成，
  但 definition 仍未实现。
- `G-P5-E2E-04/06/07/08/10/11/15/16/17/18/19/20/21`：需要 type hover/diagnostics、
  component property diagnostics、project/view isolation、alias resolution 或完整
  in-place-shadow 场景。
- `G-P5-E2E-05`：native symbols（adapter 仍返回 `Unsupported`）。
- `G-P5-E2E-13`：unavailable-engine startup decision；当前是 fail-closed startup failure，
  尚未改写为“static answers survive”的 native 测试。
- `G-P5-PERF-01` / `G-P5-STABILITY-01`：native 性能/稳定性基线未建立。

这些 gate 的 `replacement_test`/`replacement_kind`/`gate_evidence` 保持为空；
`native_gates.rs` 新增了冻结的 20-ID 集合和“required gate 不得携带 replacement 字段”的检查。

---

## 3. 批量验证

| 命令 | 结果 |
| --- | --- |
| `jq empty docs/plans/tsgo-test-ledger.json` | exit 0 |
| `cargo test -p wxml-lsp --test native_parity` | 36 passed / 0 failed / 3 ignored |
| `cargo test -p wxml-lsp --lib` | 789 passed / 0 failed / 0 ignored |
| 3 个 native gate required 命令（§2.2） | 3 passed / 0 failed / 36 filtered |
| `cargo test -p wxml-lsp --test native_lsp_document_symbols -- --test-threads=1`（带同一 `TSGO_*`/required 环境） | 7 passed / 0 failed / 0 ignored |
| `cargo test -p wxml-delivery --test assets_lock_root` | 2 passed / 0 failed |
| `cargo test -p wxml-delivery --test installer_flow legacy_read_only_commands_never_consult_node -- --exact` | 1 passed / 0 failed |
| `cargo test -p wxml-delivery --test native_cli installs_native_verify_upgrade_rollback_env_without_node -- --exact` | 1 passed / 0 failed |
| `cargo fmt -p wxml-lsp -- --check` | exit 0 |
| `cargo fmt --all -- --check` | exit 0（最终复跑；早先并行会话的 `crates/wxml-delivery/tests/provision/cache.rs` 未格式化，随后由其会话修正） |
| `git diff --check` | exit 0 |

---

## 4. 计数变化

| 项 | 回填前 | 回填后 |
| --- | ---: | ---: |
| `green` | 33 | 36 |
| `planned` | 270 | 267 |
| `retired-with-decision` | 36 | 36 |
| 有 `migration_evidence` 的 case | 114 | 119 |
| `exact` | 33 | 36 |
| `near_exact` | 3 | 3 |
| `weaker` | 55 | 57 |
| `structural` | 6 | 6 |
| `blocked` | 17 | 17 |
| 无 Rust 证据的 planned | 189 | 184 |
| 339 总数 | 339 | 339 |
| gate `gate_green` | 3 | 3 |
| gate `required_not_migrated` | 20 | 20 |

分区变化只在 `completion`（green 1→3、weaker 2、无证据 6→4）、`guards_js`
（green 4→5、exact 4→5、无证据 15→14）和 `protocol_limits`（weaker 0→2、
无证据 40→38）。其余分区不变。

---

## 5. 未关闭项

1. **默认 CI 不跑 native gate。** 3 个 green 仍是 `#[ignore]`；`gate_green` 是 opt-in
   required native job 状态，不是 `mandatory_green`。
2. **默认后端仍是 Node。** 本次没有切换 `--stdio`，没有删除 Node。
3. **P6/G5/G6 未关闭。** 20 个 required gate 仍在；`G5` 需要全部 case green/reviewed 和
   23 个 gate 转为强制门禁。
4. **F3 未关闭。** `assets.lock.json` 和 package/verify/installer 测试只证明 lock 与
   delivery 合同；不证明 F3-02 的 `execve` 审计、原生 stdio 会话或平台覆盖。
5. **document symbols 无 ledger case。** 真实引擎测试已通过，但 TS oracle 没有该 LSP 方法，
   不新增 case ID，也不把它算作 `TS-SYMBOLS-*` 的迁移。
6. **平台/UI 未扩展。** 只有 Linux x86_64；Windows/UNC、macOS、编辑器运行时未验证。
7. **工作树不是干净 HEAD。** 本次运行期间有并行 P6-3-4 未提交改动；证据等级 E0，
   原始 stdout 未入库（仅在 `/tmp/opencode/p6-*.log` 的本地运行记录中）。
