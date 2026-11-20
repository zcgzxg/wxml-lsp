# tsgo P7-1：真实 pinned 引擎 native matrix parity 实现证据（2026-11-20）

## 0. 身份、边界与结论先行

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 实现 HEAD：`1c89338`（`test(tsgo): retire legacy cases and backfill wave11 parity`）
- 台账回填 HEAD：`befba03`（`fix(tsgo): accept behavior trailing commas and backfill parity`，
  并行提交）+ 本工作树未提交的 `real_matrix.rs`
- 实现阶段只改测试、冻结数据和证据；台账回填阶段只改 ledger JSON、validator、
  台账 md、contract §1 和本证据；语义修复后的最后回填只改 projection 中
  `methods-nonliteral` 的分类、`real_matrix.rs` 的 exact/blocked 计数与分支，
  并重算 projection SHA；未改 oracle `matrix.json`、sample 字节、README 或生产
  代码。未 commit，未 review。
- **台账回填快照（2026-11-20）：** `green` 83→109、`planned` 218→192、
  `retired` 38 不变；`exact` 83→109、`weaker` 100→96、`blocked` 16→33、
  `assessed_cases` 209→248、无证据 planned 92→53。`scenario_matrix` 43 routed /
  43 evidence / 26 green / 17 blocked / 0 unassessed。26 个 promoted case 与
  17 个 blocked case 的逐条记录见 §11。
- 任务书写的是 `backends/typescript/test/matrix.json`，仓库中不存在该文件；
  实际 oracle 是按 plan §1.2 指定的
  ` docs/evidence/data/type-bridge-c1/matrix.json`
  （SHA-256 与任务书的 `69bc6c...` 一致）。
- 冻结源：` docs/evidence/data/type-bridge-c1/matrix.json`
  （SHA-256 `69bc6c57405f664ad3195b566be7b821a5273fe807a8b58f1eb08b93b1afd6db`，
  577,049 bytes，TypeScript 6.0.3，in-place-shadow only）。
- 没有 `BLESS`/`UPDATE_EXPECT`/`INSTA_UPDATE`、native 回放或自动快照更新路径。

**结论先行：真实 pinned 引擎严格能证明 22 个 in-place case，不是设计预期的
23 个。** 设计中的 21 个“纯 checker”里有 1 个仍被真实引擎否定；另 1 个已由
语义修复解决：

| scenario | 设计预期 | 真实 native 观察 | 处理 |
| --- | --- | --- | --- |
| `page-closure` | `Supported`（函数内 `identity({...})` 泛型 data root） | `Partial`，`DataRootKind::Expression`，reason `data` member is not an inline object literal | 保持 `parity=blocked` / `blocked_by=checker`，不降断言 |
| `methods-nonliteral` | 三能力 `Partial` | 修复后 build `Partial`，同一 reason，无 probe/无 mount | 已提升为 `exact` / `green` |

因此本切片的严格结果是：

- **22 exact**：20 个 checker + `interface-root` + `data-shorthand` 短路径。
- **11 blocked**：DOCS 3、STATE 5、PROBE 1、ROOT 1、CHECKER 1。
- **10 个顶层 case 只有 in-place 半边**：4 个 exact/条件 exact
  （`TS-MATRIX-0092/0228/0301/0319`），6 个 blocked；symbol-bridge 半边保持退休，
  不伪造 both-layout exact。
- 33/33 scenario 的 17 条预期诊断、`introduced`、`perturbsUserTypes`
  全部匹配；oracle 声明的 hover type/doc 和 5 个 definition kind 也全部匹配。
  1 个 CHECKER gap（`page-closure`）只影响 build 状态，不影响 checker 值。

`docs/evidence/2026-11-20-tsgo-native-matrix-parity-plan.md` 的 23/10 投影因此
修正为 **22/11**。`methods-nonliteral` 的语义修复证据见
` docs/evidence/2026-11-20-tsgo-matrix-semantic-gaps.md`。本文件记录修正依据，
不把 23 写成已达成。

## 1. 新增与修改的测试产物

| 路径 | 状态 | 作用 |
| --- | --- | --- |
| ` crates/wxml-lsp/tests/native_parity/real_matrix.rs` | 新增 | 3 个 required-native 测试 + 2 个默认证据测试；冻结 projection 的比较器 |
| ` crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/projection.json` | 新增 | `matrix.json` 的 33 个 in-place 冻结投影；123,307 bytes |
| ` crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/samples/**` | 新增 | 33 个字节级样本文件（脚本 + 额外输入文件） |
| ` crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/.gitattributes` | 新增 | `samples/** -text`，保留 CRLF/BOM |
| ` crates/wxml-lsp/tests/native_parity/scenario_matrix.rs` | 修改 | 薄包装；43 个 ledger target 路径不变，`MIGRATED_CASES` 指向真实测试 |
| ` crates/wxml-lsp/tests/native_parity.rs` | 修改 | 接入 `real_matrix`；允许 `scenario_matrix` 的 5 个迁移测试；只读扫描改到 `real_matrix.rs` |
| ` crates/wxml-lsp/tests/native_parity/native_gates.rs` | 修改 | 把 3 个 matrix `#[ignore]` 测试登记为第三类 required-native 测试，不计入 23 个 P5/P6 旧 gate |

没有复制 `backends/typescript/samples` 的 `tsconfig*.json`。`alias-project` 的
有效 options 直接冻结在 projection 中：

```json
{"baseUrl": ".", "paths": {"@models/*": ["models/*"]}}
```

`baseUrl: "."` 由 VFS 在项目根合成 tsconfig 时解析，等价于旧 oracle 的
sample-dir 绝对 `baseUrl`；测试不解析 tsconfig，也不让 engine 把 sandbox cwd
当作项目根。

## 2. 冻结 projection 的身份

| 项 | 值 |
| --- | --- |
| projection 路径 | ` crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/projection.json` |
| source SHA-256 | `69bc6c57405f664ad3195b566be7b821a5273fe807a8b58f1eb08b93b1afd6db` |
| source size | 577,049 bytes |
| source environment | Node `v24.22.2`，TypeScript `6.0.3`，typings `5.3.1` |
| selection | `in-place-shadow`，33 scenarios，17 diagnostics |
| projection SHA-256 | `9d47f7d0e9510c4c735e3290d0b2cb0d66fb713042e2fa0c59f6e7d5b752d911` |
| 冻结样本 | 33 个文件；逐文件 SHA-256 + size |
| scenario 源锚点 | ` backends/typescript/src/scenarios.ts` 的 SHA-256 |

`projectionSha256` 是 projection 中 `scenarios` 数组的 canonical compact JSON
SHA-256。默认测试从 projection 重新序列化并重算；漂移即失败，不会自动更新。

每个 scenario 冻结了：

- id、ledger case id、sample dir/script/extra files、registration；
- `allowJs`/`checkJs`/`jsCheck`、有效 compiler options；
- WXML 文本、fragment expr/target/WXML byte range；
- query（expr/target/expectType/expectDoc/expectDocSource/definition/
  expectDefinition）；
- expectedDiagnostics（expr/target/WXML byte range）；
- oracle：states、jsCheckApplied、request、mounts、hovers、definitions、
  diagnostics、introduced、perturbs、registration。

默认测试 `real_matrix_manifest_is_frozen_and_oracle_derived` 校验：

1. `matrix.json` 的实际 SHA/size；
2. 33 个 scenario、case id 机械映射、22 个 exact 集合与 11 个 blocked 类别；
3. 从 `scenarios` 重算 projection digest；
4. 每个 fragment 的 `wxmlRange` 与 `locateExpression` 语义一致；
5. 每条 expected diagnostic 的 target byte range 与 oracle range 一致；
6. 33 个 sample 文件逐个 SHA/size，磁盘集合无缺无多；
7. projection 不含 `observedAt`/`nativeEngine`/`bless` 等回放字段；
8. 顶层 10 个 case 的 4 exact + 6 blocked 表。

`real_matrix_comparator_rejects_missing_shifted_and_perturbed_results` 反向证明
比较器：缺失诊断、意外诊断、range 偏移 1 byte、同 range 不同 code、丢失
hover doc、type 扰动、introduced 新 key、空 vs 空（scenario=0）都会失败。

## 3. required-native 运行模式与引擎 pin

三个引擎测试都是 `#[ignore]`：

```bash
WXML_TEST_NATIVE=1 \
WXML_REQUIRE_REAL_ENGINE=1 \
TSGO_BIN=/tmp/opencode/tsgo-engine-7.1.0-beta/package/lib/tsc \
TSGO_ASSET_ROOT=/tmp/opencode/tsgo-engine-7.1.0-beta/package/lib \
TSGO_TYPINGS_ROOT=/tmp/opencode/cli-typings-root \
cargo test -p wxml-lsp --test native_parity real_matrix -- --ignored --nocapture
```

启动前强制检查：

- `WXML_TEST_NATIVE=1`；缺少时运行 ignored test 直接 panic，不是 skip。
- `TSGO_BIN` 是普通文件，SHA-256 等于
  ` a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70`；
  `--version` 含 `Version 7.1.0-beta`。
- `TypeAssets::load_pinned` 校验 136 个 `.d.ts`（116 lib + 20 typings）；
  失败即 panic，不降级到 `noLib:true`。
- sandbox 在 `/tmp/opencode` 下的新建私有目录；virtual project root 是不存在
  的绝对路径，测试结束断言没有落盘；`TestRoot` Drop 清理。
- `NativeSession` 的 `initialize.currentDirectory` 等于 sandbox；
  baseline/shadow 两个 revision 后 `live_snapshot_count()==1`，release 后为 0。

**与设计文档的偏差：** 设计 §4.6 计划一个 session 跑 33 个 scenario，但
`NativeSession::update_project` 的 `validate_environment_transition` 把
`CompilerOptions` digest 纳入 `TypeEnvironmentIdentity`，不允许跨 scenario
改变 options。本切片因此每个 scenario 起一个 session；每个 session 内仍是
baseline revision 1 + shadow revision 2，不重复 `open_project`。

## 4. 真实引擎断言内容

对每个非短路径 scenario：

1. 用冻结样本构造 `SourceFile` 与 `FragmentSpec`，调用
   `build_in_place_shadow`；断言 build status、reason root cause、
   `jsCheckApplied`、registration kind/count、mount fragment id 顺序与
   target 文本。
2. `interface-root`/`data-shorthand` 断言
   `Err(ShadowError::Unsupported { feature: "data.root" })`，reason 分别含
   `type-only` / `not a property assignment`。
3. baseline snapshot：原始脚本 + extra files；查询 `this.<member>` 类型和
   semantic diagnostics。
4. shadow snapshot：`built.view.text` + extra files；断言 syntactic
   diagnostics 为空；semantic diagnostics 经 `map_diagnostics_for_file`
   映射到 WXML byte range。
5. 每条诊断按 `(code, start, end)` 严格比较；`nullable`、`unchecked-index`、
   `js-shebang-bom`、`js-source-check-inherit`、`js-bom-check-inherit`、
   `unicode-crlf`、`js-jsdoc-pragma` 作为正例 sentinel 必须出现。
6. hover：每个 oracle `hover=supported` 的 query 在 mount target 位置调用
   `hover_evidence`；type 与旧 `typeOnly` 比较，documentation trim 后与旧
   `expectDoc` 比较。无 type/doc 期望的 query 要求存在 type 或 symbol。
7. definition：5 个 definition query 调用 `session.definition` +
   `map_definition`；第一个 target 按 `DefinitionTarget.path` +
   `generated_declarations` 分类为 `generated`/`script`/`external`，断言
   kind 与 range 非空；`generated` 额外断言 `source` 区间非空。
8. `perturbsUserTypes`：轻量 lexer 只识别 `this.<identifier>`，跳过字符串、
   注释和模板文本；baseline 与 shadow 逐一 `type_query`，断言没有 `<none>`、
   offset 数一致、结果等于 oracle（33/33 false）。空指纹只记录，不当作强证据。
9. `userScriptDiagnosticsIntroduced`：移植 oracle 的 multiset key
   `(code,start,length)`，跳过 mount 内诊断；`js-jsdoc-pragma` 必须得到
   `[2322]`，其余 exact case 为 `[]`。

## 5. 22 个 exact in-place case

```text
page-export-empty
page-cjs
page-esm
component-module
properties-spread
multi-registration
methods-nonliteral
js-jsdoc-pragma
js-checkjs
js-shebang-bom
interface-root
nullable
cross-file-overlay
alias-project
component-data-cross-file
crlf-script
data-shorthand
js-source-check-inherit
unchecked-index
nullable-no-strict-null
js-bom-check-inherit
unicode-crlf
```

其中 `properties-spread`/`multi-registration`/`methods-nonliteral` 由
`BuildStatus::Partial` + reason 直接断言（`methods-nonliteral` 额外断言无 probe、
无 mount）；`js-jsdoc-pragma` 的 diagnostics Partial 由 `introduced=[2322]`
推导；`interface-root`/`data-shorthand` 是短路径 exact。

`page-closure` 不在 exact 列表，原因见 §0/§6。

## 6. 11 个仍 blocked 的 in-place case（observed vs expected）

| # | scenario | blocked_by | observed（真实 native） | expected（旧 oracle） |
| ---: | --- | --- | --- | --- |
| 1 | `page-basic` | docs | build supported；2339@134-138；hover `title=string`/`user.name=string` + 中文 doc；definition kind `script` | 同上，但 `documentationSource=quickinfo`；native 无该 DTO |
| 2 | `component-properties` | docs | build supported；2339@130-138；hover `title=string \| number` + doc；definition kind `generated` | 同上 + `documentationSource=copied` |
| 3 | `component-js` | docs | build supported；2339@130-138；hover `title=string \| number` + doc | 同上 + `documentationSource=copied` |
| 4 | `js-jsdoc-off` | state | build supported；`jsCheckApplied=Off`；0 diagnostics；hover 正常 | `diagnostics=disabled`；adapter 仍发布 `Supported` |
| 5 | `ts-nocheck` | state | build supported；`Off`；0 diagnostics；hover 正常 | `diagnostics=disabled` |
| 6 | `js-nocheck` | state | build supported；`Off`；0 diagnostics；hover 正常 | `diagnostics=disabled` |
| 7 | `js-source-check-off` | state | build supported；`Off`；0 diagnostics；hover 正常 | `diagnostics=disabled` |
| 8 | `js-bom-check-off` | state | build supported；`Off`；0 diagnostics；hover 正常 | `diagnostics=disabled` |
| 9 | `no-implicit-this-off` | probe | build supported；hover `title=any`；0 diagnostics | 三能力 `unavailable`，reason 含 `probe root`；native 无 probe health 层 |
| 10 | `interface-root-import` | root | build `Partial`；`DataRootKind::Expression`；0 diagnostics | 三能力 `unsupported`，reason 含 `type-only`；native 未做 checker data-root 分类 |
| 11 | `page-closure` | checker | build `Partial`，reason 含 `inline object literal`；`DataRootKind::Expression`；2 个 mount 的 hover type/doc 与 0 diagnostics 仍匹配 | build `Supported`；旧 oracle 支持函数内泛型 `identity({...})` data root |

CHECKER 的 1 个 case（`page-closure`）是 contract/oracle drift，不属于原设计的
DOCS/STATE/PROBE/ROOT 分类；它没有被写成 exact，也没有降低任何断言。
`methods-nonliteral` 已由语义修复恢复为 native `Partial` + 同一 reason，并进入
§5 的 22 个 exact 列表。

## 7. 顶层 10 个 case 的 in-place 半边

| case | in-place 半边 | 依据 |
| --- | --- | --- |
| `TS-MATRIX-0092` | exact（条件） | 33/33 in-place build 不抛；symbol-bridge 已退休，需 owner 接受 re-scope |
| `TS-MATRIX-0228` | exact | `data-shorthand` Unsupported reason 含 `not a property assignment`，且不是 `no data member` |
| `TS-MATRIX-0301` | exact | `multi-registration` Partial + `2 page registrations` + count=2 + hover `title=string` |
| `TS-MATRIX-0319` | exact | `unchecked-index` 2532@44-52；`nullable-no-strict-null` 无 18047 且 `maybeUser.name=string` |
| `TS-MATRIX-0215` | blocked | ROOT；`interface-root-import` 仍是 Partial/Expression |
| `TS-MATRIX-0239` | blocked | STATE；checker 半边 exact，adapter 未发布 Disabled |
| `TS-MATRIX-0252` | blocked | STATE；同上 |
| `TS-MATRIX-0271` | blocked | DOCS；缺 `documentationSource=copied` DTO |
| `TS-MATRIX-0283` | blocked | STATE；BOM/shebang off 半边 |
| `TS-MATRIX-0335` | blocked | PROBE；checker 证明 root 是 any，但产品未发布 probe health |

如果 owner 不接受 `TS-MATRIX-0092` 的 build-only re-scope，顶层 exact 从 4
降为 3，顶层 blocked 从 6 升为 7。

## 8. 验证命令与结果

```bash
cargo fmt --all
# 无输出；通过

cargo check --workspace --all-targets
# Finished dev profile

cargo clippy --workspace --all-targets -- -D warnings
# cargo clippy: No issues found

cargo test -p wxml-lsp --test native_parity
# 78 passed; 0 failed; 29 ignored

WXML_TEST_NATIVE=1 \
WXML_REQUIRE_REAL_ENGINE=1 \
TSGO_BIN=/tmp/opencode/tsgo-engine-7.1.0-beta/package/lib/tsc \
TSGO_ASSET_ROOT=/tmp/opencode/tsgo-engine-7.1.0-beta/package/lib \
TSGO_TYPINGS_ROOT=/tmp/opencode/cli-typings-root \
cargo test -p wxml-lsp --test native_parity real_matrix -- --ignored --nocapture
# 3 passed; 0 failed; 0 ignored; finished in 35.00s
# real_matrix: exact=22 gaps=11 scenarios=33 engine=7.1.0-beta
#   oracle=69bc6c57405f664ad3195b566be7b821a5273fe807a8b58f1eb08b93b1afd6db
#   projection=9d47f7d0e9510c4c735e3290d0b2cb0d66fb713042e2fa0c59f6e7d5b752d911
# real_matrix: top_level_in_place_exact=4 top_level_blocked=6 engine=7.1.0-beta
```

默认 `cargo test` 只报告 29 个 ignored（23 个旧 gate + 3 个 native edit gate +
3 个 matrix 测试），不把 ignored 当 pass。optional 模式缺引擎只打印
`SKIP(real_matrix): ...` 并返回；该结果不进入任何 ledger 建议。

## 9. 建议的 ledger 增量（已在 §11 执行）

基线以当前工作树 ` docs/plans/tsgo-test-ledger.json` 的实际计数为准：
`green=83`、`planned=218`、`retired=38`、`exact=83`、`weaker=100`、
`blocked=16`、`assessed=209`、无证据 planned=92；`scenario_matrix`
43 routed / 4 evidence（全部 weaker）/ 39 unassessed。

建议（owner 接受 `TS-MATRIX-0092` 条件 exact 时）：

| 项 | 当前 | 建议 |
| --- | ---: | ---: |
| overall green | 83 | 109（+22 in-place +4 顶层） |
| overall planned | 218 | 192 |
| exact | 83 | 109 |
| weaker | 100 | 96（matrix 4 个 weaker 重新定级） |
| blocked | 16 | 33（+11 in-place +6 顶层） |
| assessed_cases | 209 | 248（43 个 matrix case 全部有证据） |
| 无证据 planned | 92 | 53 |
| `scenario_matrix` green | 0 | 26 |
| `scenario_matrix` planned_evidence | 4 | 17 |
| `scenario_matrix` unassessed | 39 | 0 |
| `scenario_matrix` weaker | 4 | 0 |
| `scenario_matrix` blocked | 0 | 17 |

若 `TS-MATRIX-0092` 不被接受：`green=108`、`planned=193`、`exact=108`、
`blocked=34`，其余不变。

建议的 ledger 记录要点：

- 22 个 in-place exact 引用
  ` real_matrix_in_place_matches_frozen_oracle`（`run_mode=required_native`）
  与默认 `real_matrix_manifest_is_frozen_and_oracle_derived` 的组合证据；
  `required_env` 至少写 `WXML_TEST_NATIVE=1`、`WXML_REQUIRE_REAL_ENGINE=1`、
  `TSGO_BIN`、`TSGO_ASSET_ROOT`、`TSGO_TYPINGS_ROOT`，`oracle_sha256` 写
  `69bc6c...`。
- 4 个顶层 exact 引用 `real_matrix_top_level_cases` + 默认 manifest 测试；
  092 的 note 必须写 “in-place build only; symbol-bridge retired”。
- 11 个 in-place blocked 与 6 个顶层 blocked 写
  `parity=blocked` + `blocked_by` + observed/expected 摘要（可引用本文件 §6/§7）。
- `page-closure` 的 `blocked_by=checker`，不能复用 DOCS/STATE/PROBE/ROOT 的
  说明；`methods-nonliteral` 已由语义修复提升 exact，不再是 blocked。
- 更新 `GREEN_CASES/PLANNED_CASES/BLOCKED_CASES/WEAKER_CASES` 和
  `counts.by_partition.scenario_matrix` 时，从 JSON 重新计数，不复制本表。

## 10. 本文件不声称

1. 不声称 23 个 in-place case 已 exact；真实结果是 22 exact / 11 blocked。
2. 不声称 `page-closure` 的泛型 data root 已修复；它仍需要 checker 级
   data-root 分类生产切片。`methods-nonliteral` 的 Unsupported/Partial 差异已由
   ` 2026-11-20-tsgo-matrix-semantic-gaps.md` 记录的语义修复解决。
3. 不声称 DOCS/STATE/PROBE/ROOT 的产品状态已发布；checker 半边通过不等于
   adapter 已发布 `documentationSource`/`Disabled`/`Unavailable`/`Unsupported`。
4. 不声称 optional skip、默认 ignored 或空诊断是证据；正例 sentinel 和
   `scenarios=33` 计数是必需条件。
5. 不声称 TS 6.0.3 与 7.1.0-beta 完全一致；本轮 17 条诊断、oracle 声明的
   hover type/doc、5 个 definition kind、introduced 和 perturbs 全部匹配，
   但发现 1 个 build 状态差异（`page-closure`），已按 blocked 记录；
   `methods-nonliteral` 的差异已修复并提升 exact。
6. 不声称 symbol-bridge 半边的 10 个顶层 case 可 exact；只有 in-place 半边
   被评估。
7. 实现阶段未修改 ledger、contract、README、生产代码；随后的台账回填只改
   ledger/validator/台账 md/contract §1/本证据，见 §11；未 commit，未 review。


## 11. 台账回填结果（2026-11-20）

本节记录 §9 建议的实际落地。回填只改
`docs/plans/tsgo-test-ledger.json`、
`crates/wxml-lsp/tests/native_parity/ledger.rs`、
`docs/plans/2026-11-17-tsgo-test-ledger.md`、`docs/reference/tsgo-test-contract.md`
和本文件；未改 README、生产或其余测试。语义修复后的最后回填把 projection 中
`methods-nonliteral` 的分类从 `blocked(checker)` 改为 `exact`，同步
`real_matrix.rs` 的 exact/blocked 计数与分支，并重算 SHA；oracle `matrix.json`
与 sample 字节未变。

### 11.1 26 个 promoted case

22 个 in-place exact：

```text
TS-MATRIX-0102-PAGE_EXPORT_EMPTY-IN_PLACE_SHADOW
TS-MATRIX-0102-PAGE_CJS-IN_PLACE_SHADOW
TS-MATRIX-0102-PAGE_ESM-IN_PLACE_SHADOW
TS-MATRIX-0102-COMPONENT_MODULE-IN_PLACE_SHADOW
TS-MATRIX-0102-PROPERTIES_SPREAD-IN_PLACE_SHADOW
TS-MATRIX-0102-MULTI_REGISTRATION-IN_PLACE_SHADOW
TS-MATRIX-0102-METHODS_NONLITERAL-IN_PLACE_SHADOW
TS-MATRIX-0102-JS_JSDOC_PRAGMA-IN_PLACE_SHADOW
TS-MATRIX-0102-JS_CHECKJS-IN_PLACE_SHADOW
TS-MATRIX-0102-JS_SHEBANG_BOM-IN_PLACE_SHADOW
TS-MATRIX-0102-INTERFACE_ROOT-IN_PLACE_SHADOW
TS-MATRIX-0102-NULLABLE-IN_PLACE_SHADOW
TS-MATRIX-0102-CROSS_FILE_OVERLAY-IN_PLACE_SHADOW
TS-MATRIX-0102-ALIAS_PROJECT-IN_PLACE_SHADOW
TS-MATRIX-0102-COMPONENT_DATA_CROSS_FILE-IN_PLACE_SHADOW
TS-MATRIX-0102-CRLF_SCRIPT-IN_PLACE_SHADOW
TS-MATRIX-0102-DATA_SHORTHAND-IN_PLACE_SHADOW
TS-MATRIX-0102-JS_SOURCE_CHECK_INHERIT-IN_PLACE_SHADOW
TS-MATRIX-0102-UNCHECKED_INDEX-IN_PLACE_SHADOW
TS-MATRIX-0102-NULLABLE_NO_STRICT_NULL-IN_PLACE_SHADOW
TS-MATRIX-0102-JS_BOM_CHECK_INHERIT-IN_PLACE_SHADOW
TS-MATRIX-0102-UNICODE_CRLF-IN_PLACE_SHADOW
```

4 个顶层 in-place 半边 exact：

```text
TS-MATRIX-0092
TS-MATRIX-0228
TS-MATRIX-0301
TS-MATRIX-0319
```

每个 case 的 `migration_evidence` 都带真实 `rust_tests`（manifest + in-place 或
top-level required-native 测试的路径/行/测试名）；`methods-nonliteral` 额外引用
` crates/wxml-backend-tsgo/tests/shadow_layout.rs::case_matrix_0102_non_literal_component_methods_are_partial_without_a_probe`
钉住语义修复。`scenario_matrix` 的 oracle scenario/断言、projection SHA
`9d47f7d0…`、`green_eligible=true`、
`status_recommendation=green`、`status_applied=green`；case 为 `status=green`、
`target_rust_test_status=migrated_green`。

### 11.2 17 个 blocked case

11 个 in-place：`PAGE_BASIC`、`COMPONENT_PROPERTIES`、`COMPONENT_JS`（docs）；
`JS_JSDOC_OFF`、`TS_NOCHECK`、`JS_NOCHECK`、`JS_SOURCE_CHECK_OFF`、
`JS_BOM_CHECK_OFF`（state）；`NO_IMPLICIT_THIS_OFF`（probe）；
`INTERFACE_ROOT_IMPORT`（root）；`PAGE_CLOSURE`（checker）。

6 个顶层：`TS-MATRIX-0215`（root）、`0239/0252/0283`（state）、`0271`（docs）、
`0335`（probe）。

全部保持 `status=planned`、`target_rust_test_status=proposed`、
`parity=blocked`、`green_eligible=false`、`status_recommendation=planned`，
并写 `blocked_by=checker`、`blocked_group`、`observed`、`expected`。真实原因与
§6/§7 一致，未把检查器半边或产品状态差异写成 exact。

### 11.3 实际计数修正

任务书预估 `assessed=227`、无证据 `74`；实际为 **248 / 53**。原因是 43 个 matrix
case 全部新增证据：4 个原 weaker 重新定级，39 个原 unassessed 首次评估；不是只增加
17 个 blocked。最终 `green=109`、`planned=192`、`retired=38`、`exact=109`、
`near_exact=3`、`weaker=96`、`structural=7`、`blocked=33`、
`green_eligible=109`、`promoted_to_green=109`、
`remaining_planned_without_evidence=53`。

### 11.4 验证

```text
jq empty docs/plans/tsgo-test-ledger.json
cargo test -p wxml-lsp --test native_parity ledger::   # 7 passed
cargo test -p wxml-lsp --test native_parity            # 78 passed / 29 ignored
cargo fmt --all -- --check
git diff --check
```

`real_matrix.rs` 的 required-native 结果在语义修复后为 `exact=22 gaps=11`、
`top_level_in_place_exact=4 top_level_blocked=6`；本轮已重新运行 pinned 引擎，
projection SHA 更新为 `9d47f7d0…`。
