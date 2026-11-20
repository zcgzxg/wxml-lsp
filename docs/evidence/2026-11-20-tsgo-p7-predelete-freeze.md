# P7-0 冻结与删除后 archived 校验（2026-11-20，HEAD `3e24c44`）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`。
- 当前 HEAD：`3e24c44`（`test(p7): retarget perf and stability gates to native`）。
- 上一版冻结基线是 `8bda306`（预删除、live TS 树仍在）。当前工作树已应用 P7 Node/TS/v1 删除补丁（未提交），因此本文件同时记录：
  1. **pre-delete 历史身份**：`8bda306` 冻结的 99 + 34 + 22 + 34 + 8 文件树、23 个旧 gate、15 个 v1 写路径等；
  2. **post-delete archived 校验**：删除后的工作树用 `wxml-tsgo-p7-freeze/2` 重新生成，缺失的冻结路径记为 `archived` 并保留最后已知 hash。
- 性质：只读冻结/校验切片。只修改 `scripts/p7-freeze.sh`、`docs/evidence/data/legacy-p7-freeze.json` 和本文；不修改 ledger、oracle manifest、测试、生产代码、README、Cargo、CI 或默认 backend；不删除文件；不 commit；不启动 review。
- 当前工作树有 P7 删除补丁及其它文档改动，冻结路径本身也不干净，所以 JSON 记录 `commit.frozen_paths_clean=false`。`--check` 比较时会忽略 `generated_at`、`commit.head`、`commit.head_subject`、`commit.branch` 和 `commit.frozen_paths_clean`。

**结论先行：**

1. **archived 模式已生效**：`backends/typescript`、`crates/wxml-lsp/src/backend_node`、`real_node.rs`、`wave5_acceptance.rs`、`semantic_tools.rs` 缺失且 oracle manifest 为 **Frozen**，在 `frozen_paths_state` 中记为 `archived`，不失败。
2. **仍在路径仍逐文件校验**：`stdio_type_loop.rs`、`performance_bench.rs`、`q2_stability.rs`、`native_parity/**`、ledger、oracle manifest 等 13 个冻结路径仍按当前字节验证；v1 delivery 目录是 `partially-archived`（4 个写符号 anchor 仍在，11 个已归档），keep/只读边界 3 个仍在。
3. **缺失不是匹配**：archived 条目的 `file_count`/`total_bytes`/`tree_sha256`/`file_sha256` 为 `null`，只携带 `last_known`；`--check` 不会把 archived 当成 hash 匹配。
4. **删除仍然 BLOCKED**：`deletion_allowed=false`，由 ledger/gates 计算，不由 archived 模式自动置 true。ledger 仍是 **109 green / 0 planned / 130 reviewed / 29 waived / 71 retired**（339 total）；11 个 retarget gate 仍是 `#[ignore]` 且非 mandatory/reviewed；v1 仍有 4 个写 anchor，2 个 silent return 仅存归档坐标。

---

## 1. 当前 `frozen_paths_state`

数据文件：[legacy-p7-freeze.json](data/legacy-p7-freeze.json)。Schema 为 `wxml-tsgo-p7-freeze/2`；oracle manifest 模式为 `Frozen`（`backends/typescript/test` 不存在，manifest 的 test/samples 均 `archived_ok=true`）。

### 1.1 冻结路径汇总

| 状态 | 数量 | 路径 |
| --- | ---: | --- |
| `present` | 13 | `crates/wxml-lsp/tests/stdio_type_loop.rs`、`performance_bench.rs`、`q2_stability.rs`、`native_parity/{native_gates,ledger,fixture_hash,native_engine_real,symbols_edits,oracle_manifest,oracle_manifest_tests}.rs`、`native_parity.rs`、`docs/plans/tsgo-test-ledger.json`、`docs/plans/tsgo-legacy-oracle-manifest.json` |
| `archived` | 5 | `backends/typescript`、`crates/wxml-lsp/src/backend_node`、`crates/wxml-lsp/tests/real_node.rs`、`crates/wxml-lsp/tests/wave5_acceptance.rs`、`crates/wxml-lsp/tests/semantic_tools.rs` |
| `partially-archived` | 1 | `crates/wxml-delivery/src`（4 个 v1 写 anchor 仍在，11 个归档；3 个 keep 边界仍在） |
| `missing` | 0 | 非 P7 归档范围的缺失会直接失败，不会进入 JSON |

### 1.2 archived 树（最后已知身份）

`trees` 中 5 棵树全部为 `state:"archived"`；当前 `file_count`/`total_bytes`/`tree_sha256` 为 `null`，`last_known` 保存上一版冻结值：

| 树 | 文件数 | 字节 | last-known tree SHA-256 |
| --- | ---: | ---: | --- |
| `backends/typescript` | 99 | 746,335 | `ef41f49c939cb881c2028c791b68731250184f38218c5aa71339f7702f02623f` |
| `backends/typescript/src` | 34 | 466,392 | `26e377e77040ac6adfe78a4730944cd08ce7e5d2345efa3c1b25726f87bb946b` |
| `backends/typescript/test` | 22 | 253,965 | `630a16af41623c78f19f47bcaba1a7f1282201ac27b4414115ac0b3764afc32e` |
| `backends/typescript/dist` | 34 | 419,511 | `37753181846b843f2ba9086bb98b2db8246bb74f820329b0b717b987bccb9898` |
| `crates/wxml-lsp/src/backend_node` | 8 | 264,166 | `2f131507696b7163ce776c290cd1420551aae0df40eac3015ac6fad78ee0c710` |

`backends/typescript/test` 与 `samples` 的 last-known digest 与 `docs/plans/tsgo-legacy-oracle-manifest.json` 的 `fixture_trees` 一致；因此 ledger/fixture gate 可以在 live 树缺失时使用 archive authority。

Oracle manifest 当前身份：`docs/plans/tsgo-legacy-oracle-manifest.json`，83,170 B，SHA-256 `ed820c1f39eafd4011ab139772f07bc2b3b5daaa9d1358583282d5fb65322278`，schema `wxml-tsgo-legacy-oracle-manifest/1`，21 test files + 1 support、339 cases、4 fixture trees，`mode:"Frozen"`。

### 1.3 archived 单文件

| 文件 | last-known 字节 | last-known SHA-256 | 归档原因 |
| --- | ---: | --- | --- |
| `crates/wxml-lsp/tests/real_node.rs` | 26,150 | `fc948a4bf7838c93693fdbc1b104ee96119a959835ed640077221d47af9849c4` | 12 个 `p7_action=delete` gate 的旧测试源 |
| `crates/wxml-lsp/tests/wave5_acceptance.rs` | 41,910 | `d28c9eace06a401ad632d162518e3243b1557f993748aec4fb5d8770249c2af3` | 7 个 delete gate 的旧测试源 |
| `crates/wxml-lsp/tests/semantic_tools.rs` | 100,857 | `359e5d9a38d88d75617dc4effbad240782b1f356bbaa4a1e1c2f2d7a45572773` | 2 个 silent-return gate 的旧测试源 |

`crates/wxml-lsp/src/backend_node/config.rs` 及 5 个 legacy env 行同样按 archived 记录，最后已知文件 SHA-256 为 `cdc40ec0a265f6cca172a76a2c9fd60ce8228d338a89ea209ed6184d3d81d642`。

---

## 2. 历史 pre-delete 身份（`8bda306` 冻结，仅供追溯）

以下是上一版冻结的 live 身份，当前工作树已不再满足；它们作为 `last_known` 保留，不是当前匹配结果：

- `backends/typescript`：99 tracked + 34 `dist`；`src` 34、`test` 22。
- `crates/wxml-lsp/src/backend_node`：8 文件。
- 23 个旧 `#[ignore]` gate：12 `delete` + 11 `retarget`；全部 `gate_green`，但 `mandatory_green=false`、`replacement_reviewed=false`。
- 15 个 v1 写路径、3 个 keep 边界；5 个 legacy 运行时环境变量；2 个 silent return + 1 个可选 `tsc` 分支。
- 行号来自 `8bda306` 工作树；当前 present 路径的行号已按删除后工作树重新记录，archived 条目保留 last-known 行号。

pre-delete 的逐文件清单、23 个 gate 的 FQ test ID/Node 依赖、v1 anchor 和 legacy oracle 绑定仍在 JSON 的 `old_ignored_gates`、`v1_delivery`、`legacy_env`、`legacy_ts_oracle` 中；缺文件条目使用 `state:"archived"` + `last_known`，不再保留当前 `file_sha256`。

---

## 3. archived 模式规则

`scripts/p7-freeze.sh` 的 `--check`：

1. **判定 oracle 模式**：`backends/typescript/test` 存在为 `Live`，缺失为 `Frozen`；`Frozen` 还要求 manifest 对 `backends/typescript/test` 与 `samples` 都有 `archived_ok=true`。
2. **present 路径**：按原算法逐文件重算 SHA-256、tree digest、gate `fn`/`#[ignore]` 行号、v1/env/silent/oracle anchor；任何字节或行号漂移失败。
3. **archived 路径**：仅当路径在 P7 归档范围、oracle 为 `Frozen`、且上一版冻结存在时，记录 `state:"archived"` 和 `last_known`；当前 digest/hash 为 `null`，不参与匹配。
4. **部分删除**：tracked 树同时存在 present/missing 条目时直接失败；非归档范围的缺失、keep 边界缺失、anchor 多义也都失败。
5. **`deletion_allowed`**：仍由 ledger 和 gates 计算。当前 ledger 有 130 reviewed + 29 waived（非 green），所有 gate 的 `mandatory_green=false`、`replacement_reviewed=false`，所以结果为 `false`。

`Frozen` 模式不执行 `REQUESTED_HEAD..HEAD` 的冻结路径 diff 守卫（删除补丁本身会改变这些路径）；身份以当前 worktree 的 present hash 和 archived `last_known` 为准。`Live` 模式仍保留该守卫。

身份算法未变：普通文件、root-relative UTF-8 路径、`LC_ALL=C` 字节序、`path + NUL + bytes + NUL` 的 SHA-256；行尾仍是工作树原始字节。

---

## 4. 当前 ledger / gates / v1 计数

| 项 | 当前值 |
| --- | --- |
| ledger cases | 109 green / 0 planned / 130 reviewed / 29 waived / 71 retired（339 total） |
| ignored gates | 23 total，23 `gate_green`；12 `state:"archived"`（delete），11 `state:"present"`（retarget） |
| silent-skip gates | 0；2 个旧 silent return 进入 `semantic_tools.archived_silent_returns` |
| v1 write symbols | 11 archived / 4 present（`install`、`upgrade`、`verify_package`、`run` 的 anchor 仍存在，但不代表 v1 分支仍可达） |
| v1 keep symbols | 3 present（`verify_installed`、`read_legacy_manifest`、`build_native_package`） |
| legacy runtime env | 5 archived（`WXML_NODE`、`WXML_TYPE_BACKEND`、`WXML_BACKEND_DIR`、`WXML_ANALYZE_TIMEOUT_MS`、`WXML_TEMPLATE_CHECK`） |

`blocking_state.blocked_by` 记录当前仍阻止 `deletion_allowed=true` 的原因：ledger 的 reviewed/waived 非 green、11 个 present gate 仍非 mandatory/reviewed、12 个 delete gate 只是 archived（不是 live match）、oracle 为 Frozen、4 个 v1 写 anchor 仍存在、silent-return 证据仅为归档。

---

## 5. 验证

在当前共享工作树（HEAD `3e24c44`，删除补丁未提交）执行：

| 命令 | 结果 |
| --- | --- |
| `bash -n scripts/p7-freeze.sh` | 通过 |
| `scripts/p7-freeze.sh --allow-dirty --output /tmp/opencode/p7-freeze-new.json` | 生成成功；`Frozen` 模式；记录 `frozen_paths_clean=false` |
| 将生成结果写入 `docs/evidence/data/legacy-p7-freeze.json` 后运行 `scripts/p7-freeze.sh --check` | `p7-freeze: OK (3e24c44…; Frozen mode: present paths verified, archived paths are last-known only; docs/evidence/data/legacy-p7-freeze.json)`，exit 0 |
| `jq empty docs/evidence/data/legacy-p7-freeze.json` | 通过 |
| `git diff --check` | 通过 |

负向行为：

- 非归档冻结路径缺失、keep 边界缺失、partial tree、anchor 多义、present 文件字节/行号漂移都会非零退出；
- archived 条目没有当前 digest，不会被当作匹配；
- 脚本不执行 Node/npm/cargo，不删除文件，不写仓库内临时文件。

---

## 6. 明确不声称

- 不声称 P7 删除已完成最终验收；本切片只证明删除后冻结证据可校验。
- 不声称 23 个 gate 已 `mandatory_green` 或 reviewed；11 个 retarget gate 仍是 opt-in `#[ignore]`，12 个 delete gate 只是归档。
- 不声称 `reviewed`/`waived`/`retired` 是 green；ledger 仍按原状态记录。
- 不声称 archived 条目逐字节匹配；它们只有最后已知 hash。
- 不声称可以删除 `backends/typescript/**`、`backend_node/**`、旧 gate 或 v1 写路径中的剩余部分；`deletion_allowed` 仍为 `false`。
- 不声称 `dist/` 或 `node_modules/` 是仓库跟踪内容；`dist/` 是本地构建产物，`node_modules/` 被明确排除。
