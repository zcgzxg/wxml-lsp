# real_matrix 去 legacy TS 源依赖：归档 scenarios.ts（2026-11-20）

## 0. 结论先行

1. `real_matrix` 的默认测试不再读取 `backends/typescript/src/scenarios.ts`。
   该文件的字节已复制到
    `crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/scenarios.ts`
   （21,463 bytes，SHA-256 `5f0c35f3…`），测试只读 fixture 并继续校验冻结哈希/大小。
2. 冻结 projection 未被放宽，也未改动 `projection.json`：
   digest 仍为 `9d47f7d0…`，33 scenarios、17 in-place diagnostics、
   22 exact + 11 blocked（docs 3 / state 5 / probe 1 / root 1 / checker 1）、
   33 个 sample file 全部原样通过自校验。
3. 真实 required matrix 在 **`backends/typescript` 整个目录不存在**时通过：
   3 个 `#[ignore]` matrix 测试全绿（48.42 s）。默认 `native_parity` 套件同样在
   该目录不存在时通过（81 passed / 29 ignored）。
4. 未删除旧树、未改生产代码/ledger/README、未 commit、未 review。
5. projection 中的 `scenarioManifest.path` 仍是历史路径字符串，现在只作 provenance；
   本改动后它确实不再被运行时打开。

## 1. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD：`4dd62b1`
- 起点状态：工作树已有并行切片的未提交改动
  （`native_gates.rs`、`performance_bench.rs`、`q2_stability.rs`、
  `docs/plans/tsgo-test-ledger.json`、`docs/evidence/2026-11-20-tsgo-p7-retarget-native-tests.md`）；
  本切片未修改这些文件的内容，只在 `cargo fmt --all` 时可能触及其格式。
- 本切片改动：
  - `crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/scenarios.ts`（新增，字节副本）
  - `crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/.gitattributes`
  - `crates/wxml-lsp/tests/native_parity/real_matrix.rs`
  - `crates/wxml-lsp/tests/native_parity/scenario_matrix.rs`
  - `crates/wxml-lsp/tests/native_parity.rs`
  - 本文件
- 未做：删 `backends/typescript`、改 `projection.json`/`matrix.json`、
  改 ledger/README/脚本、commit。

## 2. 归档 fixture

| 项 | 值 |
| --- | --- |
| 原路径 | `backends/typescript/src/scenarios.ts` |
| fixture 路径 | `crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/scenarios.ts` |
| 字节数 | 21,463 |
| SHA-256 | `5f0c35f341c92e8b903c6b0e9ef5127598e76aa46bd51de942bbc2ca367f5bef` |
| 行数 / 编码 | 663 行 / UTF-8、无 BOM、0 个 CRLF |
| 校验方式 | `cp` 后 `cmp` 原文件与 fixture，结果 `IDENTICAL`；两侧 SHA-256 相同 |

`.gitattributes` 增加 `scenarios.ts -text`，防止 checkout 时做行尾归一化，
保持字节可复现。`samples/** -text` 的既有规则未动。

`projection.json` 保持冻结：`source.scenarioManifest.path` 仍记录历史路径，
`source.scenarioManifest.sha256` 仍是上述值；测试把它当 provenance 断言，
但读取目标固定为 fixture。没有新增/修改任何哈希或计数白名单。

## 3. 代码改动

### 3.1 `crates/wxml-lsp/tests/native_parity/real_matrix.rs`

- 新增常量（路径约 90 行起）：
  - `SCENARIO_MANIFEST_ORIGINAL_PATH = "backends/typescript/src/scenarios.ts"`
    —— 仅 provenance，明确不读取；
  - `SCENARIO_MANIFEST_RELATIVE_PATH` 指向归档 fixture；
  - `SCENARIO_MANIFEST_SIZE = 21_463`；
  - `SCENARIO_MANIFEST_SHA256 = "5f0c35f3…"`。
- 新增 `read_archived_scenario_manifest(root)`：只拼接 fixture 路径并读取，
  不会 fallback 到旧树。
- `real_matrix_manifest_is_frozen_and_oracle_derived`：
  - 仍断言 `projection.source.scenario_manifest.path` 是历史路径、
    `sha256` 是冻结值；
  - manifest 字节改为从 fixture 读取，并新增大小断言；
  - `matrix.json` 的 SHA/大小、projection digest、33/17/22/11、
    sample 集合逐文件哈希等原有自校验全部保留。
- 新增 `real_matrix_scenario_manifest_is_archived`：
  - 校验 fixture 的 SHA/大小；
  - 在 `TEST_TMPDIR`（默认 `/tmp/opencode`，`TestRoot` 保证在仓库外）构造
    一个**只有 fixture、没有 `backends/` 目录**的模拟根，再从该根验证同一哈希，
    证明 archived 分支不依赖旧树。`TestRoot` 的 `Drop` 自动清理。

### 3.2 分区清单

新增的默认测试需要同步两个镜像清单，否则 `native_parity` 自身的
partition 门会失败：

- `crates/wxml-lsp/tests/native_parity/scenario_matrix.rs` 的 `MIGRATED_CASES`；
- `crates/wxml-lsp/tests/native_parity.rs` 的 `SCENARIO_MATRIX_MIGRATED_CASES`。

只加了测试名，未改 ledger 路由、case 数或 ignored gate 集合。

## 4. 冻结不变量（改动后仍由测试强制）

| 不变量 | 值 | 结果 |
| --- | --- | --- |
| projection digest | `9d47f7d0e9510c4c735e3290d0b2cb0d66fb713042e2fa0c59f6e7d5b752d911` | 未变 |
| scenarios | 33 | 未变 |
| in-place diagnostics | 17 | 未变 |
| exact / blocked | 22 / 11 | 未变 |
| blocked 分组 | docs 3、state 5、probe 1、root 1、checker 1 | 未变 |
| sample files | 33，逐文件 size + SHA-256 | 未变 |
| oracle `matrix.json` | 577,049 bytes，`69bc6c57…` | 未变 |
| scenario manifest | 21,463 bytes，`5f0c35f3…` | 未变（改读 fixture） |

## 5. 验证

环境：Linux x64，pinned engine `Version 7.1.0-beta`，
engine 包 `/tmp/opencode/tsgo-native-pkg`，typings
`/tmp/opencode/tsgo-native-pkg/typings/miniprogram-api-typings`。
注意：`TSGO_ASSET_ROOT` 必须指向能解析出 `lib.d.ts` 的根；本次用
`/tmp/opencode/tsgo-native-pkg`（命中 `<root>/engine/lib/lib.d.ts`）。
旧证据里的 `.../engine` 在本代码路径下不命中，会报
“must contain lib.d.ts”；这是路径口径问题，不是 matrix 回归。

```bash
cargo fmt --all --check
# FMT_OK

cargo test -p wxml-lsp --test native_parity -- --test-threads=1
# 81 passed; 29 ignored（此时 backends/typescript 已临时移走）

cargo clippy -p wxml-lsp --test native_parity -- -D warnings
# No issues found
```

模拟 `backends/typescript` 不存在的方式（不删除主树）：

```bash
bak=/home/zxg/.local/share/opencode/worktree/e15985/ts-backends-archived-$$
mv backends/typescript "$bak"
trap 'mv "$bak" .../tsgo-migration/backends/typescript' EXIT
# 断言 test ! -e backends/typescript 通过后运行测试
```

真实 required matrix（`WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1`、
完整 `TSGO_BIN`/`TSGO_ASSET_ROOT`/`TSGO_TYPINGS_ROOT`）：

| 场景 | 命令过滤 | 结果 |
| --- | --- | --- |
| 旧树存在 | `cargo test -p wxml-lsp --test native_parity real_matrix -- --ignored --test-threads=1` | 3 passed，48.71 s |
| 旧树移走 | 同上 | 3 passed，48.42 s |

两次都覆盖：

- `real_matrix_in_place_matches_frozen_oracle`
- `real_matrix_known_gaps_are_characterized`
- `real_matrix_top_level_cases`

`git diff --check` 通过，无空白错误。

## 6. 剩余依赖与说明

1. **`real_matrix` 运行时读取集合**：`projection.json`、`fixtures/real_matrix/samples/**`、
   `fixtures/real_matrix/scenarios.ts`，以及历史 oracle
   `docs/evidence/data/type-bridge-c1/matrix.json`（只哈希/保留证据）。
   没有任何 `backends/typescript/**` 运行时读取。
2. `real_matrix.rs` 的 `resolve_engine` 仍保留 typings fallback
   `backends/typescript/node_modules`（仅当 `TSGO_TYPINGS_ROOT` 未设置时尝试）。
   旧树删除后该目录不存在，行为退化为既有的显式 skip / required panic，
   不会误读其他路径。按本任务边界未改。
3. `projection.json` 和历史 ledger/freeze 证据中的 `scenarios.ts` 字符串是
   provenance，不是读取目标。`docs/evidence/2026-11-20-tsgo-p7-deletion-surface.md`
   第 242 行曾把 projection 路径描述为“不是运行时文件读取”；在本改动之前
   该描述与实际不符（manifest 测试确实读了旧树），现在才成立。
4. `backends/typescript/test` 的其他 gate（`oracle_manifest`、`fixture_hash`）
   仍按各自的 live/frozen 分支处理，本切片未触碰。
5. 未运行非 `native_parity` 的其他测试目标；未 commit。
