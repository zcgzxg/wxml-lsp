# tsgo P7-2 预删除 oracle 清单（2026-11-20）

## 0. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务给定 HEAD：`807a1578173992ad16f15e22b45f8dd62b024df3`
- 记录日期：2026-11-20
- 范围：只改 `crates/wxml-lsp/tests/native_parity/**`、
  `docs/plans/tsgo-legacy-oracle-manifest.json`、contract/Markdown 台账和本证据文档。
  没有删除任何源码、没有改生产 backend/Cargo/default/Node、没有 commit。
- 工作树存在并行 wave-7 的未提交改动（`ledger.rs` 的
  `WEAKER_CASES=80`、`tsgo-test-ledger.json`、`2026-11-17-tsgo-test-ledger.md` 等）。
  本轮保留这些常量与台账改动，只追加 manifest/source-scan 逻辑。
- 本文件不声称已有 P7-3/P7-4 删除完成；它只证明删除后 oracle 仍可验证。

**结论：P7-2 的 oracle 解耦已就绪。** 新增冻结清单在 live 树存在时与 live/台账三方一致；
在 legacy 树缺席时改走 frozen 校验，并保持 339 case / 21 file / 4 tree 的完整约束。

---

## 1. 冻结清单

文件：`docs/plans/tsgo-legacy-oracle-manifest.json`。

| 项 | 值 |
| --- | --- |
| schema | `wxml-tsgo-legacy-oracle-manifest/1` |
| `baseline.head` | `807a1578173992ad16f15e22b45f8dd62b024df3` |
| `projection_sha256` | `14cd870e8ce7d15294285fe9205bb98bb121164e44f8c67ee3c433aa43508123` |
| 清单文件 SHA-256（原始字节） | `ed820c1f39eafd4011ab139772f07bc2b3b5daaa9d1358583282d5fb65322278` |
| 测试文件 | 21 |
| case | 339 |
| `support.ts` | `9b4fcb347885dbeff1d1711b5f606aa48586cdf86b291ee5f868b36fbc4cef36`，144 行 |
| fixture 树 | 4 |

fixture 树记录与台账一致：

| 树 | 文件 | 字节 | SHA-256 (v1) | 可缺席 |
| --- | ---: | ---: | --- | --- |
| `backends/typescript/test` | 22 | 253965 | `630a16af41623c78f19f47bcaba1a7f1282201ac27b4414115ac0b3764afc32e` | 是 |
| `backends/typescript/samples` | 35 | 6069 | `00a7fa3d6962da50581c396f73c13f7ffb043efae060a0801b373928726e5c1f` | 是 |
| `tests/fixtures` | 136 | 61928 | `7c9ddedda18c70bc217af6b3bbd23dd22a33be9c885276cbd587adc0596f04a9` | 否 |
| `crates/wxml-lsp/tests/fixtures` | 43 | 6456 | `834eea4defcb7565c4b84e91a173079cf3be85201d44fb4a7a6d0a32af3da073` | 否 |

排序与算法：case 按 `case_id`、文件与树按路径 UTF-8 字节序（`LC_ALL=C`），
严格递增；file hash 为原始字节 SHA-256，行数为 `str::lines()`；case body 为
`test(...)`/`it(...)` CallExpression 文本（不含尾分号）的 SHA-256；树 hash 为 v1
（regular files only，symlink 拒绝，`path + NUL + bytes + NUL`）。

---

## 2. 校验实现

### 2.1 新增/修改

| 文件 | 内容 |
| --- | --- |
| `crates/wxml-lsp/tests/native_parity/oracle_manifest.rs` | 清单 schema/排序/计数/hash 校验；`projection_sha256` 自校验与 `FROZEN_MANIFEST_SHA256` 硬 pin；台账交叉校验；live/frozen 两种源模式 |
| `crates/wxml-lsp/tests/native_parity/oracle_manifest_tests.rs` | 合成 1 file / 3 case oracle 的正负例，删除后仍运行 |
| `crates/wxml-lsp/tests/native_parity/ledger.rs` | `ledger_old_test_sources_and_lines_exist` 改走 live/frozen 双模式；新增真清单/模拟删除测试 |
| `crates/wxml-lsp/tests/native_parity/fixture_hash.rs` | 4 棵树的 live/archived 双模式；manifest/ledger/`FROZEN_TREES` 三方校验；合成负例 |
| `crates/wxml-lsp/tests/native_parity.rs` | 注册 `oracle_manifest` 模块 |

### 2.2 失败语义

- live 存在：逐文件重算 SHA-256/行数，逐 case 重算 CallExpression body；live、台账、
  清单任一不一致即失败。额外文件、缺失文件、symlink、非目录根都失败。
- live 不存在：清单逐项对照台账；缺字段、未知字段、重复、未排序、数量不符、
  body/file/support/tree hash 或行号漂移都失败。没有 skip 分支。
- 只改清单字段而不更新 `projection_sha256`：解析失败；同时更新清单 digest 但
  不更新 Rust `FROZEN_MANIFEST_SHA256`：pin 失败；两者都改但台账不同：
  交叉校验失败。
- fixture 树：legacy 两棵可在 `archived_ok=true` 时缺席；保留两棵缺席直接失败。

---

## 3. 验证结果

```text
cargo test -p wxml-lsp --test native_parity
  -> 68 passed; 0 failed; 26 ignored

cargo fmt --all -- --check
  -> pass

cargo clippy -p wxml-lsp --all-targets -- -D warnings
  -> pass

jq empty docs/plans/tsgo-legacy-oracle-manifest.json
  -> pass

git diff --check
  -> pass
```

### 3.1 模拟删除

没有移动或删除 live 树，使用不存在的根模拟：

| 测试 | 结果 |
| --- | --- |
| `ledger::frozen_oracle_gate_passes_when_legacy_tree_is_absent` | PASS；模式 `Frozen`，21 files / 339 cases |
| `fixture_hash::synthetic_tests::fixture_gate_passes_after_legacy_trees_are_deleted` | PASS；legacy 两棵 `Archived`，保留两棵 `Live` |

### 3.2 负例

| 篡改/漂移 | 结果 |
| --- | --- |
| manifest case `body_sha256` | FAIL（projection）；重算 projection 后仍 FAIL（台账 case anchor） |
| manifest file `sha256` | FAIL（projection）；重算后仍 FAIL（台账 file hash） |
| manifest case 数量 | FAIL（counts 自校验）；一致改为 2 后仍 FAIL（台账 3） |
| 缺 `line` 字段 / 多余 top-level key / case 未排序 / 重复 case ID | FAIL（schema） |
| manifest support/tree hash | FAIL（projection 或台账） |
| live 文件漂移 / 额外文件 / symlink / 非目录根 | FAIL（live 重算） |
| 保留 fixture 树缺席 / manifest 或 ledger 树 hash 不一致 | FAIL（fixture gate） |

---

## 4. 未闭合 / BLOCKED

- P7-2 的删除动作未执行；`backends/typescript/test` 仍在，当前 gate 走 live 模式。
- 旧 TS 源字节若变化，必须与台账同一提交重新生成 manifest，否则 live 模式先红；
  这是防止“删除前漂移”的预期行为。
- 台账 `planned` 仍为 255，P7-1 未完成。
- P7-3（23 个 gate + 删除 `backend_node`/Node 测试）、P7-4（samples 迁移与旧 TS
  删除）、P7-5（F3-09/v1 packager）仍依赖后续工作。
- 并行 wave-7 的 `WEAKER_CASES=80` 等常量已保留；本轮未改变任何 case 状态计数。
