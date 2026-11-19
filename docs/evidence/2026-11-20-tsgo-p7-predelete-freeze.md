# P7-0 预删除冻结（2026-11-20，HEAD `75040c2`）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`。
- 本次冻结基线 HEAD：`75040c2`（`test(tsgo): freeze legacy oracle and backfill parity ledger`）。
- 上一版冻结记录的是 `52ae408` 基线 / `807a157` 实际 HEAD。`75040c2` 已提交 oracle manifest、wave-7/wave-8 ledger 回填和 parity 归档读取，因此按 `75040c2` 重新生成。`backends/typescript` 与 `backend_node` 的树哈希没有变化；变化集中在 ledger 计数、`ledger.rs`/`fixture_hash.rs` 行号和 TS oracle 绑定方式。
- 冻结时共享工作树有并行未提交改动（`docs/evidence/README.md`、`scripts/build-release.sh` 以及其它 shadow/tests WIP），都不在冻结路径内；`git status --porcelain -- <冻结路径>` 为空。
- 性质：**只读冻结切片**。只修改 `docs/evidence/data/legacy-p7-freeze.json`、`scripts/p7-freeze.sh` 和本文；不修改 ledger、oracle manifest、README、N2 build、生产代码、Cargo、CI 或默认 backend；不删除文件；不 commit；不启动 review。

**结论先行：**

1. 本次按 `75040c2` 重新冻结：`backends/typescript` 99 个 tracked 文件 + 34 个 `dist` 文件、`backend_node` 8 个文件、23 个旧 `#[ignore]` gate、15 个 v1 写路径、5 个 legacy 环境变量、2 个 silent return + 1 个可选 `tsc` 分支。
2. **P7-2 的 archive 绑定已经落地**：`ledger.rs` 和 `fixture_hash.rs` 以 `docs/plans/tsgo-legacy-oracle-manifest.json` 为权威；live `backends/typescript/test|samples` 存在时重哈希，不存在时走 Frozen 模式。P7-0 的“ledger/fixture 解耦”子项因此可以标为完成。
3. **P7-0 整体仍是 partial**：`p7-pre-delete` tag、独立的 legacy-node-tests 清单、`native_gates.rs` 的 `old_test_removed` 支持仍未完成。
4. **删除仍然 BLOCKED**：ledger 仍是 **55 green / 248 planned / 36 retired**；23 个 gate 仍是 `#[ignore]`；live TS 树仍在；v1 写路径和 2 个 silent return 仍在。

---

## 1. 冻结范围与身份

数据文件：[legacy-p7-freeze.json](data/legacy-p7-freeze.json)。Schema 为 `wxml-tsgo-p7-freeze/1`。

### 1.1 `backends/typescript/**`

| 树 | 类型 | 文件数 | 字节 | tree SHA-256 |
| --- | --- | ---: | ---: | --- |
| `backends/typescript` | 仓库跟踪树 | 99 | 746,335 | `ef41f49c939cb881c2028c791b68731250184f38218c5aa71339f7702f02623f` |
| `backends/typescript/src` | 子树的独立 digest | 34 | 466,392 | `26e377e77040ac6adfe78a4730944cd08ce7e5d2345efa3c1b25726f87bb946b` |
| `backends/typescript/test` | 子树的独立 digest | 22 | 253,965 | `630a16af41623c78f19f47bcaba1a7f1282201ac27b4414115ac0b3764afc32e` |
| `backends/typescript/dist` | 未跟踪本地构建产物 | 34 | 419,511 | `37753181846b843f2ba9086bb98b2db8246bb74f820329b0b717b987bccb9898` |

- 主树列出 99 个 tracked 文件的逐文件 `{path, bytes, sha256}`；`src`/`test`/`dist` 另有独立文件清单和 digest。`test`/`samples` 的 digest 与 `docs/plans/tsgo-legacy-oracle-manifest.json` 的 `fixture_trees` 一致。
- `samples` 的 35 个文件、`scripts` 的 2 个文件、`package.json`/`package-lock.json`/`tsconfig*.json`/`README.md`/`.gitignore` 都在主树里逐文件固定。
- `dist/**` 是 `.gitignore` 的本地构建输出，但 23 个旧 gate 都依赖 `dist/server.js`，所以它被纳入冻结并在 `--check` 中严格校验。干净 checkout 没有 `dist/`，必须先构建（或从同一工作树复制）才能通过完整 `--check`。
- `node_modules/**` 被明确排除：它是 gitignored 的 npm 安装树，`.bin` 里有 symlink，不属于仓库身份。JSON 的 `excluded[]` 记录原因；脚本不遍历它。

### 1.2 `crates/wxml-lsp/src/backend_node/**`

| 树 | 文件数 | 字节 | tree SHA-256 |
| --- | ---: | ---: | --- |
| `crates/wxml-lsp/src/backend_node` | 8 | 264,166 | `2f131507696b7163ce776c290cd1420551aae0df40eac3015ac6fad78ee0c710` |

8 个文件全部在 `files[]` 中：`completion.rs`、`config.rs`、`mod.rs`、`process.rs`、`protocol.rs`、`request.rs`、`response.rs`、`tests.rs`。删除前置仍是 P7-3。

### 1.3 23 个旧 `#[ignore]` gate

完整 23 行在 JSON 的 `old_ignored_gates.gates[]`。每个 gate 固定：

- `gate_id`、`file`、`test`、`fully_qualified`（集成测试二进制名 + `::` + 函数名）；
- `line_ignore` 与 `line_fn`（HEAD `75040c2` 的工作树行号）；
- `status`、`mandatory_green=false`、`replacement_reviewed=false`、`old_test_present=true`；
- `replacement_test`、`replacement_kind`、`old_reason`；
- `p7_action`（`delete` 或 `retarget`）；
- `node_dependency`（opt-in 变量、`dist/server.js`、backend 目录、进程边界、Node 版本要求）；
- 源文件 SHA-256。

按文件：

| 文件 | gate 数 | P7 动作 |
| --- | ---: | --- |
| `real_node.rs` | 5 | 5 delete |
| `wave5_acceptance.rs` | 7 | 7 delete |
| `stdio_type_loop.rs` | 9 | 9 retarget |
| `performance_bench.rs` | 1 | 1 retarget |
| `q2_stability.rs` | 1 | 1 retarget |

合计 23：12 `delete` / 11 `retarget`。所有 23 个 gate 的 ledger 状态都是 `gate_green`，但它们在源码里仍是 `#[ignore]`；`gate_green` 是 opt-in native job 的结果，不是 `mandatory_green`。行号在 `75040c2` 上未变化。

Node 依赖按文件分为三类：

- `real_node.rs` / `wave5_acceptance.rs`：测试进程内构造 `NodeBackend`，`#[ignore]` reason 明确要求 Node 24 + `backends/typescript/dist/server.js`。
- `stdio_type_loop.rs`：启动 `target/debug/wxml-lsp --stdio`，通过 `WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` 让 LSP 进程再启动 Node；代码本身不直接 `Command::new("node")`。
- `performance_bench.rs` / `q2_stability.rs`：`NodeBackend` 加 `node` 版本探测/进程表扫描；`#[ignore]` reason 只要求 `WXML_TEST_NODE=1`，未写死 Node 版本。

### 1.4 v1 打包/安装写路径

JSON `v1_delivery.write_paths[]` 固定 15 个写路径符号，`keep_paths[]` 固定 3 个必须保留的只读/原生符号。每个条目有 `file`、`symbol`、`line`、`anchor`、`action`、`role` 和文件 SHA-256。

写路径覆盖：

- packager：`wxml-lsp-package` 默认 v1 分支（`run`、`build_package` 调用点）、`build_package`、`build_manifest`、`update_index`；
- installer：`install`、`upgrade`、`install_and_report` 的 schema v1 分支、`verify_package` 的 v1 分支、`install_tree`；
- acquire/cache：`acquire`、`store_verified`；
- verify/manifest：`check_backend`、`verify_archive`、`parse_index`。

必须保留的边界：

- `install.rs::verify_installed`（legacy current 的只读校验）；
- `manifest_v2.rs::read_legacy_manifest`（legacy 只读投影）；
- `package.rs::build_native_package`（v2 native 打包器，不是删除候选）。

这与 P6 设计 §6.5 一致：P7-5 只删除 v1 **写路径**，`list/current/verify/env` 的 legacy 只读分支和 `rollback` 拒绝必须保留。

### 1.5 Legacy 环境变量

运行时变量（`backend_node/config.rs`，每个都记录定义行和文件哈希）：

| 变量 | 行 | 作用 |
| --- | ---: | --- |
| `WXML_NODE` | 166 | Node 可执行名或绝对路径 |
| `WXML_TYPE_BACKEND` | 179 | 绝对 `server.js` 路径 |
| `WXML_BACKEND_DIR` | 185 | 绝对 backend 目录 |
| `WXML_ANALYZE_TIMEOUT_MS` | 196 | Node 请求超时，默认 15000 |
| `WXML_TEMPLATE_CHECK` | 203 | `off`/`pragma`/`inherit` |

测试专用变量：

- `WXML_TEST_NODE`：23 个旧 gate 的 opt-in，随旧 Node gate 一起删除；
- `WXML_TEST_EXTERNAL_BACKEND`：复用外部 backend 的测试逃生口，当前只在 `real_node.rs`/`stdio_type_loop.rs` 使用。

### 1.6 `semantic_tools` 2 个 silent return + 可选 `tsc`

| Gate | 函数 | 行 | 静默行为 |
| --- | --- | --- | --- |
| `G-P5-E2E-22` | `real_node_lsp_component_property_edit_applies_and_parses` | fn 2086；asset lookup 2091；`return;` 2099；`if tsc.exists()` 2184；tsc skip 2215 | `dist/server.js` 缺失时打印 `skipping real Node LSP test` 并 return；`tsc` 缺失时打印 `tsc not installed; skipped post-edit type check` |
| `G-P5-E2E-23` | `real_node_lsp_rejects_cross_namespace_and_dynamic_edits` | fn 2589；asset lookup 2594；`return;` 2602 | 同样的 `dist/server.js` 缺失静默 return |

这两个 gate 的 ledger 状态仍是 `required_not_migrated`，`replacement_test` 指向 `symbols_edits.rs` 的 native edit gate；旧测试仍在，且只有在 native gate 请求模式下才 fail-closed。

### 1.7 Legacy TS oracle 归档绑定

`75040c2` 新增并提交了 `docs/plans/tsgo-legacy-oracle-manifest.json`，`ledger.rs`/`fixture_hash.rs` 现在以它为权威：

| 项 | 值 |
| --- | --- |
| manifest 路径 | `docs/plans/tsgo-legacy-oracle-manifest.json` |
| schema | `wxml-tsgo-legacy-oracle-manifest/1` |
| 文件大小 / SHA-256 | 83,170 B / `ed820c1f39eafd4011ab139772f07bc2b3b5daaa9d1358583282d5fb65322278` |
| baseline head | `807a157`（oracle 冻结时的基线，不是本冻结 HEAD） |
| counts | 21 test files + 1 support、339 cases、4 fixture trees |
| projection SHA-256 | `14cd870e8ce7d15294285fe9205bb98bb121164e44f8c67ee3c433aa43508123` |
| fixture trees | `backends/typescript/test` 22/253,965/`630a16af…`、`samples` 35/6,069/`00a7fa3d…`、`tests/fixtures` 136/61,928/`7c9ddedd…`、`crates/wxml-lsp/tests/fixtures` 43/6,456/`834eea4d…` |

当前绑定位置（JSON `legacy_ts_oracle.bindings[]`）：

| 文件:行 | 符号/锚点 | 作用 |
| --- | --- | --- |
| `oracle_manifest.rs:34` | `ORACLE_MANIFEST_RELATIVE_PATH` | manifest 路径常量 |
| `oracle_manifest.rs:37` | `LEGACY_TEST_SOURCE_ROOT` | legacy source root |
| `oracle_manifest.rs:42` | `ORACLE_BASELINE_HEAD` | oracle 冻结基线 |
| `oracle_manifest.rs:45` | `FROZEN_MANIFEST_SHA256` | manifest 完整性 pin |
| `ledger.rs:1426` | `verify_legacy_oracle(...)` | 把 live root 交给 verifier |
| `ledger.rs:1438` | `live_root` | live 存在走 Live，缺失走 Frozen |
| `fixture_hash.rs:71` | `path: "backends/typescript/test"` | `legacy_archived=true` |
| `fixture_hash.rs:78` | `path: "backends/typescript/samples"` | `legacy_archived=true` |
| `fixture_hash.rs:415` | `verify_fixture_trees(...)` | fixture gate 使用 oracle manifest |

因此 P7-2 的“读 archive manifest，而不是强制读 live tree”已经成立；live `backends/typescript/test`/`samples` 仍存在，存在时仍逐字节复核。删除旧树是 P7-4，不是本冻结。

---

## 2. 算法

`scripts/p7-freeze.sh` 与 `docs/reference/tsgo-test-contract.md` 的 v1 fixture 算法一致：

1. 只接受普通文件；symlink、特殊文件直接失败。空目录不进入清单。
2. 路径是相对树根的 UTF-8 路径，统一 `/` 分隔。
3. 条目按 root-relative path 的原始字节排序，显式 `LC_ALL=C`。
4. 每个文件的输入是 `relative_path + NUL + raw_file_bytes + NUL`；所有条目顺序拼接后取 SHA-256，小写 hex。
5. `bytes` 是原始字节数；`files[]` 的 `path` 是树根相对路径。
6. 行号是 HEAD `75040c2` 的工作树行号；`--check` 会重新定位 `fn`、`#[ignore]`、v1 anchor、legacy env anchor、silent return anchor 和 legacy oracle anchor，任何移动都会失败。

**行尾**：哈希读取工作树原始字节。LF checkout 是当前基线；CRLF checkout 会改变所有哈希，和既有 fixture gate 的 LF 说明一致。

`--check` 的比较范围是 JSON 中除 `generated_at`、`commit.head`、`commit.head_subject`、`commit.branch` 之外的全部内容；commit 信息是 provenance，不是内容身份。任何缺失文件、字节变化、树 digest 变化、gate 行号移动、anchor 消失都会以非零退出。

---

## 3. 与 P7-1 / P7-2 的关系

### 3.1 P7-1（台账归零）

本冻结只记录 **55 green / 248 planned / 36 retired** 和 23 个 gate 的当前状态，不推进任何 case，也不把 `gate_green` 写成 `mandatory_green` 或 `reviewed`。P7-1 仍需：

- `planned=0`（或书面 G5 waiver）；
- 23 个 gate 全部 `mandatory_green` + `replacement_reviewed`；
- 2 个 silent return 被替换，可选 `tsc` 分支删除；
- 只有完成这些，P7-3 的删除才有 F2 依据。

### 3.2 P7-2（归档旧 TS oracle）

`75040c2` 已提交 archive manifest 和 read-side 绑定。当前状态：

- `ledger.rs` 通过 `oracle_manifest::load_manifest` 读取 manifest，再用 `verify_legacy_oracle` 选择 Live/Frozen；
- `fixture_hash.rs` 的 `legacy_archived=true` 允许 `backends/typescript/test`/`samples` 缺失；
- manifest 的 `fixture_trees` 与本冻结的 tree digest 一致；
- live 树仍存在，存在时作为逐字节交叉校验，而不是唯一 authority。

P7-2 剩余的字节/哈希工作已由 manifest 覆盖；P7-4 仍要把 35 个 `samples` 迁到统一 fixture 并保留 CRLF/BOM/shebang/许可。

### 3.3 与 P7-3..P7-5 的关系

- P7-3 删除 `backend_node/**` 和 12 个 delete-at-P7 旧测试，前置是 P7-1/P7-2 和默认 native/HostOnly；P7-2 已落地，P7-1 未完成。
- P7-4 删除旧 TS 实现/测试/构建，前置是 archive manifest 和 samples 迁移。
- P7-5 删除 v1 写路径，前置是 F3-08/F3-09 的真实 legacy 证据；本冻结明确保留 3 个只读/原生边界。

---

## 4. 仍不能删除的条件

| # | 条件 | 当前证据 |
| ---: | --- | --- |
| 1 | Ledger 未归零 | `55 green / 248 planned / 36 retired`（339 total）；`exact=55`，其余 planned 仍混有 `near_exact`/`weaker`/`structural`/`blocked` 证据 |
| 2 | 23 个旧 gate 仍 `#[ignore]` | 23 `gate_green` / 0 `required_not_migrated`，但 `mandatory_green=false`、`replacement_reviewed=false`、`old_test_present=true` |
| 3 | native job 不是 required | `.github/workflows/ci.yml:156-158` 的 `native-gates` 只在 `workflow_dispatch` + `inputs.native_gates` 时运行；默认 CI 不跑 23 gate |
| 4 | `native_gates.rs` 仍要求旧测试存在 | `IGNORED_GATE_COUNT=23`；没有 `old_test_removed`，删除旧 `#[ignore]` 会让 `native_parity` 先红 |
| 5 | live TS 树仍在 | `backends/typescript/test` 22 文件 / 253,965 B；`samples` 35 文件 / 6,069 B；P7-4 samples 迁移未完成 |
| 6 | v1 写路径仍可达 | 15 个写路径符号仍在；`wxml-lsp-package` 无子命令仍是 v1；`install`/`upgrade` 仍有 schema v1 分支 |
| 7 | `semantic_tools` 仍有静默/可选分支 | 2 个 `return;` + 1 个 `if tsc.exists()` |
| 8 | P7-0 其余子项未完成 | 未打 `p7-pre-delete` tag；没有独立 legacy-node-tests 清单；`native_gates.rs` 未支持 `old_test_removed` |

因此 JSON 的 `blocking_state.deletion_allowed` 是 `false`。

---

## 5. P7-0 是否完成

**不是 complete，是 partial。** 本切片完成/确认的部分：

- [x] 冻结 `backends/typescript`（tracked + dist）与 `backend_node` 的文件/树身份；
- [x] 冻结 23 个旧 gate 的 FQ test ID、文件、行、Node 依赖、replacement 和 P7 动作；
- [x] 冻结 v1 写路径、legacy env、2 个 silent return + `tsc` 分支；
- [x] `ledger.rs` 读 archive manifest（`75040c2`）；
- [x] `fixture_hash.rs` 支持 legacy tree archived（`75040c2`）；
- [x] 提供只读生成/校验脚本和证据索引。

P7-0 仍未完成：

- [ ] `git tag p7-pre-delete`；
- [ ] 独立的 legacy-node-tests 清单（12 gate + 13 文件 + blob sha/name/gate id）；TS 侧已由 `tsgo-legacy-oracle-manifest.json` 覆盖逐文件/逐 case 身份，但 P6 计划里的两个 JSON 文件名没有单独创建；
- [ ] `native_gates.rs` 支持 `old_test_removed`，允许旧 `#[ignore]` 消失但 replacement 仍在。

P7-0 整体完成仍要另开实现提交。

---

## 6. 验证

在当前共享工作树（HEAD `75040c2`，冻结路径无未提交改动）执行：

| 命令 | 结果 |
| --- | --- |
| `bash -n scripts/p7-freeze.sh` | 通过 |
| `scripts/p7-freeze.sh --check` | `p7-freeze: OK (75040c2…; tracked trees and local dist match docs/evidence/data/legacy-p7-freeze.json)`，exit 0 |
| `jq empty docs/evidence/data/legacy-p7-freeze.json` | 通过 |
| `git diff --check` | 通过 |

负向行为保持：

- `--output` 指向仓库内会 exit 2，并打印 `--output must be outside the repository`；
- 缺失冻结文件或字节/行 drift 会以非零退出；
- 脚本不执行 Node/npm/cargo，不删除文件，不写临时文件。

共享工作树有 `docs/evidence/README.md`、N2 `scripts/build-release.sh` 以及其它并行 WIP 的未提交改动，但都不在冻结路径内，因此不影响 `--check`。README 的 P7-0 索引行仍是上一版 `807a157` / `48/255/36`，按任务要求未修改；主线程提交时需要同步为 `75040c2` / `55/248/36`。

---

## 7. 明确不声称

- 不声称 P7-0 已完成；只完成冻结清单/校验器及 P7-2 读侧确认。
- 不声称 P7-1 已完成；ledger 仍是 55/248/36。
- 不声称 23 个 gate 已 `mandatory_green` 或 reviewed；它们仍是 opt-in `#[ignore]`。
- 不声称可以删除 `backends/typescript/**`、`backend_node/**`、旧 gate 或 v1 写路径。
- 不声称 F3-06/F3-09 或 G5/G6/G7 已关闭。
- 不声称 `dist/` 或 `node_modules/` 是仓库跟踪内容；`dist/` 是本地构建产物，`node_modules/` 被明确排除。
