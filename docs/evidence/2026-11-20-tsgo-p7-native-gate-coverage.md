# P7 native gate 覆盖补齐（2026-11-20）

## 0. 结论

- `scripts/native-gates.sh` 从 **35** 扩到 **40** 个 gate：23 engine + 3
  symbols/edits + 9 stdio + 1 perf + 1 stability + 3 `real_matrix`。
- 本机使用 pinned `7.1.0-beta` package 完整运行：**PASS 40/40**，`build: PASS`。
- 新增一个 LSP 级测试文件，覆盖 semantic tokens、inlay hint、code lens、
  workspace symbol、signature help 和 template rename：**4 passed**。
- 审计中的大部分高/中风险缺口仍 **BLOCKED**，本文件不把它们写成已覆盖。

## 1. 边界与身份

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| 任务起始 HEAD | `3e24c444e87de13a20d4a67367e4d8e99b940524` |
| 40/40 运行时的 HEAD | `3e24c444e87de13a20d4a67367e4d8e99b940524`（`summary.txt` 的 `head:` 字段） |
| 工作树 | 运行时有未提交的 P7 删除补丁；本切片只改测试、脚本、CI 和证据 |
| 后续提交 | 并行 worker 在本证据运行后提交 `c4ddf32`（`feat(p7): remove node backend and legacy typescript tree`）；本切片的脚本/测试/CI 变更包含在该提交中，证据文档仍是未跟踪文件。本切片自身未执行 `git commit` |
| 未修改 | 生产 `crates/**`、`docs/plans/tsgo-test-ledger.json`、README、`scripts/p7-freeze.sh` |
| 日期 | 2026-11-20 |

本切片改动的文件：

| 文件 | 变更 |
| --- | --- |
| `scripts/native-gates.sh` | 35 → 40；新增 perf/stability/real_matrix 三组；`--verify-no-node` 日志改为 `07-` |
| `.github/workflows/ci.yml` | native gate 步骤文案 26 → 40，并记录 40 的组成；未在 GitHub 实际运行 |
| `crates/wxml-lsp/tests/semantic_surface.rs` | 新增 4 个 LSP 级 native 语义测试 |
| `crates/wxml-lsp/tests/native_parity/real_matrix.rs` | 让 `TSGO_ASSET_ROOT=<package>/engine` 可解析到 `<package>/engine/lib`；修正 samples 注释 |
| `crates/wxml-lsp/tests/{performance_bench,q2_stability,stdio_type_loop}.rs` | 示例命令的 typings 路径从已删除的 `backends/typescript` 改为 package 路径 |

## 2. Runner 合同

`scripts/native-gates.sh` 仍保持原有 fail-closed 语义：

- 在 build 之前校验 `lib/tsc --version`、SHA-256、size 和 typings 布局；
- 输入缺失、相对路径、非 pinned 引擎或输出在仓库内时 **exit 2**；
- 每个 gate 使用 `cargo test --release` 和 `--ignored --test-threads=1`；
- 每个 gate 的通过数必须精确等于冻结计数，少跑、过滤掉或多跑都失败；
- 缺少资产、缺少 package 或缺失 engine 时不会变成 skip。

新增的计数与目标：

| # | Gate | Cargo target / filter | 数量 | 日志 |
| ---: | --- | --- | ---: | --- |
| 1 | `native_parity::native_engine_real` | `native_parity` / `native_engine_real` | 23 | `01-native-engine-real.log` |
| 2 | `native_parity::symbols_edits` | `native_parity` / `symbols_edits` | 3 | `02-symbols-edits.log` |
| 3 | `stdio_type_loop` | `stdio_type_loop` / 全部 | 9 | `03-stdio-type-loop.log` |
| 4 | `G-P5-PERF-01` | `performance_bench` / 全部 | 1 | `04-performance-bench.log` |
| 5 | `G-P5-STABILITY-01` | `q2_stability` / 全部 | 1 | `05-q2-stability.log` |
| 6 | `real_matrix` | `native_parity` / `real_matrix` | 3 | `06-real-matrix.log` |
| 7 | `--verify-no-node`（可选） | `scripts/verify-no-node.sh` | — | `07-verify-no-node.log` |

G-P5-STABILITY-01 保留“工作树必须干净”的严格合同。本工作树有未提交的 P7
补丁，因此证据运行显式导出 `WXML_STABILITY_ALLOW_DIRTY=1`；这只跳过 clean
检查，不跳过 10 轮生命周期/SIGKILL gate，也不改变断言。

## 3. 实际运行

```bash
bash -n scripts/native-gates.sh
WXML_STABILITY_ALLOW_DIRTY=1 ./scripts/native-gates.sh \
  --package /tmp/opencode/tsgo-native-pkg \
  --output /tmp/opencode/native-gates/p7-40-gate-final
```

package/引擎身份：

```text
engine: /tmp/opencode/tsgo-native-pkg/engine/lib/tsc
engine_version: Version 7.1.0-beta
engine_sha256: a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70
engine_size: 24522912
asset_root: /tmp/opencode/tsgo-native-pkg/engine
typings_root: /tmp/opencode/tsgo-native-pkg/typings/miniprogram-api-typings
build: PASS
```

结果（`/tmp/opencode/native-gates/p7-40-gate-final/summary.txt`）：

```text
gate: PASS native_parity::native_engine_real 23/23
gate: PASS native_parity::symbols_edits 3/3
gate: PASS stdio_type_loop 9/9
gate: PASS performance_bench::G-P5-PERF-01 1/1
gate: PASS q2_stability::G-P5-STABILITY-01 1/1
gate: PASS native_parity::real_matrix 3/3
no_node_scope: NOT_REQUESTED
NATIVE_GATES: PASS 40/40
```

各日志的 `test result`：

| 日志 | 结果 | 用时 |
| --- | --- | ---: |
| `01-native-engine-real.log` | 23 passed / 0 failed | 41.64 s |
| `02-symbols-edits.log` | 3 passed / 0 failed | 2.28 s |
| `03-stdio-type-loop.log` | 9 passed / 0 failed | 3.83 s |
| `04-performance-bench.log` | 1 passed / 0 failed | 186.26 s |
| `05-q2-stability.log` | 1 passed / 0 failed | 4.39 s |
| `06-real-matrix.log` | 3 passed / 0 failed | 14.64 s |

`real_matrix` 首次接入 runner 时暴露了环境约定不一致：runner 按其他 gate 的
约定传 `TSGO_ASSET_ROOT=<package>/engine`（含 `lib/lib.d.ts`），而
`real_matrix` 只接受 `lib.d.ts` 直接在 root 下。已将解析器扩展为同时接受
`<root>/lib/lib.d.ts`（并保留 `<root>/lib.d.ts` 与 `<root>/engine/lib/lib.d.ts`），
随后 3/3 通过。没有修改生产加载器 `TypeAssets::load_pinned`。

## 4. 新增 LSP 级 native 语义测试

文件：`crates/wxml-lsp/tests/semantic_surface.rs`。测试启动真实
`wxml-lsp --stdio` 进程，默认 HostOnly（Node 变量指向不存在路径），只验证
native 静态事实，不声称 engine-backed 类型查询。

| 测试 | 覆盖 |
| --- | --- |
| `native_semantic_capabilities_follow_configuration` | initialize 返回 `semanticTokens`/`workspaceSymbol`/`signatureHelp`/`inlayHint`/`codeLens`/`executeCommand`；`semanticTools.inlayHints=false` / `codeLens=false` 时对应 provider 消失且请求返回空 |
| `native_semantic_tokens_are_utf16_for_cjk_emoji_and_crlf` | `textDocument/semanticTokens/full` 的 data 为 5 的倍数；CJK `名字` 是 2 个 UTF-16 单元的 variable；`view` 是 4 单元的 namespace；emoji 后位置按 UTF-16 代理对计算；CRLF 行内不越界 |
| `native_inlay_hints_signature_help_code_lens_and_workspace_symbols` | 组件属性 inlay hint `: number`/`: string` 及其精确位置；WXS `greet` 的 2 参数 signature 和 `activeParameter=1`；template call 与 component use code lens；`workspace/symbol` 聚合和 query 过滤 |
| `native_template_rename_returns_document_changes_for_definition_and_call` | `textDocument/prepareRename` 返回 range/placeholder；template rename 返回 `documentChanges`，definition + call 共 2 个 edit |

运行：

```bash
cargo test -p wxml-lsp --test semantic_surface
# 4 passed; 0 failed
```

补充验证：

```bash
cargo test -p wxml-lsp
# 1097 passed; 40 ignored
cargo test -p wxml-lsp --test native_parity
# 79 passed; 29 ignored（默认治理项）
cargo clippy -p wxml-lsp --tests -- -D warnings
# No issues found
cargo fmt --all --check
# OK
```

## 5. 仍 BLOCKED / 未覆盖

以下项目本切片没有补测，不能从 40/40 或 4 个语义测试推导为已覆盖：

> 快照说明：本节状态是 2026-11-20 早先记录。其中 4 个 stability 场景
> （typing burst/cancel、资源删除恢复、app.json overlay、40 请求 burst）后续已由
> `q2_stability_support` 推进为 **IMPLEMENTED**，仅外部 RSS baseline 仍 **BLOCKED**；
> **ledger 尚未回填**。其余缺口状态未变。见
> `docs/evidence/2026-11-20-tsgo-stability-scenarios.md`。

| 缺口 | 状态 | 原因 |
| --- | --- | --- |
| `definition` 读盘预算（8 文件 / 64 MiB） | **BLOCKED** | `handlers/hover.rs` 的预算仍生效；原 `lsp_p7.rs` 测试已删除，尚无 fake-backend 替代 |
| workspace root 移除/重加、in-flight exactly-once | **BLOCKED** | `didChangeWorkspaceFolders` 生命周期无 LSP 级测试 |
| v2 CLI `--enable-download`、plain-http 拒绝、相对 `XDG_DATA_HOME`、只读命令不改根 | **BLOCKED** | 生产代码仍在，v1 `installer_flow` 测试已删除 |
| multi-app stdio、`client/registerCapability` watch 注册 | **BLOCKED** | host 单测存在，但没有 initialize/stdio 级断言 |
| G-P5-STABILITY-01 的 typing burst + in-flight cancel | **BLOCKED** | 薄壳只做 12 轮顺序 update；未复制旧 10 次连打后 cancel |
| 资源删除/恢复、`app.json` overlay 移除/恢复 | **BLOCKED** | 无 native watched-files 删除/恢复往返断言 |
| 40 请求有界队列 burst | **BLOCKED** | 深层 oracle 只有串行 hover；并发 burst 未复证 |
| 外部 RSS baseline ratio | **BLOCKED** | 旧 baseline 混合 Rust parent 与 Node 子进程，不能直接比较 |
| `real_matrix` 11 个 blocked 场景 | **BLOCKED（产品/合同事实）** | 22 exact / 11 blocked 已由 `real_matrix_known_gaps_are_characterized` 如实记录，不伪造 product state |
| F3-09 真实 v1 包证据 | **BLOCKED（历史）** | `f3_legacy_real.rs` / `scripts/f3-legacy-real.sh` 已删除，合同引用仍是 fixture 级 |
| `scripts/p7-freeze.sh --check` | **BLOCKED（治理）** | 基线/树/锚点仍失败；本切片未修改该脚本 |
| `--verify-no-node` | **未运行** | 本次没有传 `--installer`/`--install-root`；`no_node_scope: NOT_REQUESTED` |

## 6. 明确不声称

1. 不声称 40/40 覆盖全部语言功能；它覆盖 engine、edits、stdio、perf、
   stability 和 33 场景 matrix 的 native 入口。
2. 不声称 4 个语义测试覆盖 engine-backed hover/definition/completion；它们
   只覆盖 HostOnly 静态语义面。
3. 不声称 GitHub `native-gates` job 已实际运行；只改了文案/组成，未触发 CI。
4. 本切片记录时 stability 的 5 个 BLOCKED 场景尚未补；后续 `q2_stability_support` 场景证据已把其中 4 项推进为 IMPLEMENTED，仅外部 RSS baseline 仍 BLOCKED，且 **ledger 尚未回填**（见 `docs/evidence/2026-11-20-tsgo-stability-scenarios.md`）。`WXML_STABILITY_ALLOW_DIRTY=1` 始终只是本地脏树证据开关。
5. 不声称 `p7-freeze --check`、F3-09 真实 v1 证据或 `--verify-no-node` 已通过。
6. 本切片未执行 commit、未 review；并行 worker 的 `c4ddf32` 提交包含本切片的
   脚本/测试/CI 内容。未修改生产代码、ledger、README 或 `scripts/p7-freeze.sh`。
