# tsgo P7-1：真实 pinned 引擎运行 Rust matrix parity 的可行性设计与证据规范（2026-11-20）

## 0. 身份、边界与结论先行

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD：`51abc13`（`test(tsgo): close conditional parity and ledger backfill`）
- 记录日期：2026-11-20
- 范围：**只新增本文件**。未修改生产代码、测试源码、`Cargo`、delivery、
  ledger JSON（`docs/plans/tsgo-test-ledger.json`）、ledger Markdown、contract、
  ADR、README、CI、脚本或 fixture；未 commit，未启动 review。
- **未运行 `cargo test` / `cargo clippy` / `cargo fmt`**。本文只做只读读取、
  `node --experimental-strip-types` 读取 `scenarios.ts`、`python3` 统计 JSON、
  `git log` / `sha256sum` / `stat`。所有“exact/blocked”判断是源码与旧 oracle
  数据的分析结果，落地后必须逐断言复核。
- 主要输入：
  - ` backends/typescript/test/matrix.test.ts:92`（33×2 循环与 24 条断言）、
    `:197` 起 10 个顶层 case
  - ` backends/typescript/src/scenarios.ts:38`（33 个 scenario 的定义）、
    ` backends/typescript/src/run.ts:294`（oracle harness）、
    ` backends/typescript/src/analysis.ts:494`（states/diagnostics/metrics）
  - ` docs/evidence/data/type-bridge-c1/matrix.json`（66 个旧 oracle run，
    577,049 bytes，SHA-256 `69bc6c57405f664ad3195b566be7b821a5273fe807a8b58f1eb08b93b1afd6db`）
  - ` docs/plans/tsgo-test-ledger.json` 的 78 个 matrix case、`counts`、
    `migration_evidence`
  - ` crates/wxml-backend-tsgo/src/shadow/layout.rs:453`、
    ` crates/wxml-backend-tsgo/src/shadow/mapping.rs:247`、
    ` crates/wxml-backend-tsgo/src/native/session.rs:1109`、
    ` crates/wxml-backend-tsgo/src/query/diagnostics.rs:84`、
    ` crates/wxml-backend-tsgo/src/query/hover.rs`、
    ` crates/wxml-backend-tsgo/src/query/definition.rs:70`
  - ` crates/wxml-lsp/tests/native_parity/*`、` crates/wxml-lsp/src/backend_tsgo/*`
  - ` docs/reference/tsgo-test-contract.md`、` docs/reference/tsgo-native-contract.md`

**结论先行：**

1. 33 个 in-place scenario 共用 24 条静态断言（23 条语义断言）。旧 oracle 的
   in-place 结果全部 `diagnosticsExact=true`、`perturbsUserTypes=false`；
   `userScriptDiagnosticsIntroduced` 只有 `js-jsdoc-pragma` 为 `[2322]`，其余为空。
   33 个 scenario 共 17 条预期诊断（16 个 scenario 有诊断）。
2. 真实 pinned 引擎能覆盖全部 **30 个 checker case** 的 checker 半边，但一份
   real-engine matrix test **严格只能把 23 个 in-place case 做成 exact**：
   21 个纯 checker case + 2 个短路径 case（`interface-root`、`data-shorthand`）。
   其余 **10 个仍 blocked**：3 个 `documentationSource`（DOCS）、5 个
   `Diagnostics=Disabled` 状态（STATE）、1 个 probe health（PROBE）、
   1 个 checker-based data-root 分类（ROOT）。
3. 10 个顶层 case 的 oracle 都循环 `LAYOUTS = ["in-place-shadow","symbol-bridge"]`。
   在 symbol-bridge 已被退休的前提下，必须先做 in-place re-scope 裁决；即使裁决通过，
   严格也只有 4 个能 exact/条件 exact（`TS-MATRIX-0092/0228/0301/0319`），
   其余 6 个随上述 10 个 gap 继续 blocked。
4. 严格计数投影：**70 green / 233 planned / 36 retired → 97 / 206 / 36**；
   `blocked` 证据桶 **22 → 38**；`weaker` **96 → 92**；
   `assessed_cases` **197 → 236**；无证据 planned **106 → 67**；
   `scenario_matrix` 分区 **27 green / 16 blocked**。
   分阶段补齐 STATE → DOCS → PROBE → ROOT 后最多到 **113 / 190 / 36**；
   仍剩 190 planned、22 个原有 blocked、67 个无证据 planned。**本切片不能完成
   P7-1 的 planned=0。**
5. 期望数据必须来自旧 oracle 的**冻结投影**：`matrix.json` 的文件 SHA-256 +
   规范化 projection SHA-256，测试禁止 `--bless`/native 回放/快照自动更新。
   TS 6.0.3（旧 oracle）与 7.1.0-beta（native pin）的版本差是最大风险。
6. required/optional：`#[ignore]` + `WXML_TEST_NATIVE=1` +
   `WXML_REQUIRE_REAL_ENGINE=1`；optional 模式的 skip **不是证据**。
   ledger 需要新增 `run_mode=required_native` / `required_env` 或等价的
   `evidence_run` 字段；default 运行的 manifest 测试只证明期望数据来源，
   不证明引擎跑过。**不能把 skip、`#[ignore]` 未执行或 fake engine 当 green。**

---

## 1. 基线与旧 oracle

### 1.1 台账快照（HEAD `51abc13`）

> **并行漂移说明：** 任务书给定的 HEAD `51abc13` 是
> `70 green / 233 planned / 36 retired`；本文 §6 的阶段表以这个基线计算，
> 与任务书一致。读取期间工作树已有并行未提交的 wave-11 回填，当前
> `docs/plans/tsgo-test-ledger.json` 是
> `82 green / 219 planned / 38 retired`（`exact=82`、`assessed=209`、
> 无证据 `92`，`blocked=22` 不变）。matrix 分区本身未变：仍是 43 planned、
> 4 weaker、39 unassessed。若按当前工作树合并，阶段 1 的绝对计数是
> `109/192/38`（`exact=109`、`weaker=92`、`assessed=248`、无证据 `53`）、
> 阶段 5 是 `125/176/38`（`assessed=248`、无证据 `53`）；各阶段的增量
> （green/planned/blocked/assessed/无证据）与本文相同。回填前必须以 JSON
> 重新计数，不能复制本表。

| 项 | 值 |
| --- | ---: |
| 总 case | 339 |
| `green` | 70 |
| `planned` | 233 |
| `retired-with-decision` | 36 |
| `assessed_cases` | 197 |
| `exact` | 70 |
| `near_exact` | 2 |
| `weaker` | 96 |
| `structural` | 7 |
| `blocked` | 22 |
| 无证据 planned | 106 |
| `scenario_matrix` routed | 43 |
| `scenario_matrix` evidence | 4（全部 `weaker`） |
| `scenario_matrix` green | 0 |
| `scenario_matrix` unassessed | 39 |

`scenario_matrix` 的 43 个 planned 由 33 个 `TS-MATRIX-0102-*-IN_PLACE_SHADOW`
加 10 个顶层 case 组成；同 33 个 scenario 的 `*-SYMBOL_BRIDGE` 已全部
`retired-with-decision`，不属于本切片。

### 1.2 旧 oracle artifact

| 项 | 值 |
| --- | --- |
| 路径 | ` docs/evidence/data/type-bridge-c1/matrix.json` |
| 大小 | 577,049 bytes |
| SHA-256 | `69bc6c57405f664ad3195b566be7b821a5273fe807a8b58f1eb08b93b1afd6db` |
| runs | 66（33 scenario × 2 layout） |
| in-place runs | 33 |
| 生成环境 | Node `v24.22.2`，TypeScript `6.0.3`，typings `5.3.1` |
| 生成来源 | ` backends/typescript/src/cli.ts` / `npm run evidence` |
| 关键字段 | `states`、`jsCheckApplied`、`perturbsUserTypes`、`userScriptDiagnosticsIntroduced`、`diagnostics[]`（code + WXML byte range）、`hovers[]`（typeOnly/documentation/documentationSource）、`definitions[]`（kind）、`registration.count`、`request` |
| 入库提交 | `36aad34`（`feat: add WXML language server and Zed extension`，2026-11-16） |

**旧 oracle 的 in-place 全量结果：**

- `diagnosticsExact = true`：33/33。
- `perturbsUserTypes = false`：33/33。
- `userScriptDiagnosticsIntroduced`：仅 `js-jsdoc-pragma` 为 `[2322]`，其余 32 个为空。
- 16 个 scenario 有预期诊断，共 17 条；`nullable` 有 2 条，其余 15 个各 1 条。
- 3 个 scenario 走 `hover.status === "unsupported"` 的提前 return：
  `interface-root`、`interface-root-import`、`data-shorthand`。
- 3 个 scenario 的 in-place hover 是 `partial`，query 循环被跳过：
  `properties-spread`、`multi-registration`、`methods-nonliteral`。
- 1 个 scenario 的 hover 是 `unavailable`，query 循环被跳过但不提前 return：
  `no-implicit-this-off`。

### 1.3 为什么必须冻结 projection

`matrix.json` 是 2026-11-16 旧 Node backend 的输出，不是 native 输出。若 Rust
测试在 native 回放后把实际值写回期望（bless/snapshot update），就会把 native
行为变成“期望”，违反“expected data from old oracle”。本设计强制：

1. 测试只读 `matrix.json` 的冻结 projection，不写任何期望文件。
2. `matrix.json` 的文件 SHA-256 和规范化 projection SHA-256 都固定；
   两者任一漂移都失败，不是自动更新。
3. 旧 `scenarios.ts` 的 query 表在新 fixture manifest 中冻结；WXML 文本、
   sample 文件 SHA-256、有效 compiler options 一并冻结。
4. 只有 owner 书面裁决的 TS 6.0.3 → 7.1.0-beta 偏差才允许改 projection；
   偏差必须逐 case 记录 old/new/reason/owner，不能用一次 native 回放覆盖全表。

---

## 2. 33 个 in-place scenario 的实际断言与 fixture

### 2.1 共用的 24 条断言

` backends/typescript/test/matrix.test.ts:102` 的每个 scenario case 执行：

| # | 断言 | 说明 |
| ---: | --- | --- |
| 1-3 | `hover/diagnostics/definition.status` 等于 `EXPECTED_STATES`（in-place 默认 `supported`） | 6 态：`supported/partial/unsupported/disabled/unavailable/unknown` |
| 4-6 | 非 `supported` 时 `reason` 非空 | 每个能力各一条 |
| 7 | `EXPECTED_REASON` 的 root cause 正则匹配 | 本切片 in-place 只涉及 `properties-spread`、`multi-registration`、`data-shorthand`、`methods-nonliteral`、`no-implicit-this-off` |
| 8-9 | `hover.status === "unsupported"` 时 `hovers.length == 0` 且 `diagnostics.length == 0`，随后 return | 短路路径 |
| 10-14 | 每个 query：hover 存在、observation 存在、`typeOnly`、`documentation`、`documentationSource` | 只有 hover `supported` 的 scenario 执行 |
| 15 | `diagnosticsExact === true`，否则 `missingDiagnostics[].expr` / `unexpectedDiagnostics[].range` 进入失败信息 | in-place 全部期望 true |
| 16-19 | 每个 `definition:true` 的 query：definition 存在、`kind`、`range` 可映射、目标文本非空 | 只有 definition `supported` 的 scenario 执行 |
| 20 | `perturbsUserTypes` 等于期望 | in-place 全部 false |
| 21 | `userScriptDiagnosticsIntroduced[].code` 排序后 deepEqual | in-place 只有 `[2322]` |
| 22 | `request.ownerKind === spec.registration` | page/component |
| 23 | `request.fileCount >= 1` | |
| 24 | `request.fragmentCount ===` 去重后的表达式数 | |
| 25 | `registration.count >= 1` | |

账本记为 24 条静态断言（23 语义）；上面的编号把同一个循环里的多个
`assert` 拆开展示，实际以 `assertions.count=24` 为准。

### 2.2 逐 scenario 表

状态简写：`S=supported`、`P=partial`、`U=unsupported`、`D=disabled`、
`N=unavailable`；顺序为 `hover/diagnostics/definition`。
`diag` 为旧 oracle 的 `code@WXML byte range`。
`value` 列只列 **实际会被断言** 的值：hover 为 `partial/unavailable` 时
query 循环跳过，标 `—`。
`gap` 是严格模式下仍需 product/contract 处理的阻塞；`—` 表示可由
real-engine matrix test 单独证明。

| # | scenario | fixture（dir/script + extra） | reg / jsCheck | h/d/def | diag（oracle） | value：type / doc / docSource / defKind | perturbs / introduced | gap |
| ---: | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | `page-basic` | `page-basic/index.ts` | page / inherit | S/S/S | `2339@134-138` | `title=string`、`user.name=string` / `页面标题`、`用户显示名称` / `quickinfo`×2 / `user.name=script` | false / — | DOCS |
| 2 | `page-export-empty` | `page-export-empty/index.ts` | page / inherit | S/S/S | `2339@134-138` | `title=string`、`user.name=string` / 同左 / — / — | false / — | — |
| 3 | `page-cjs` | `page-cjs/index.ts` | page / inherit | S/S/S | `2339@134-138` | `title=string`、`user.name=string` / 同左 / — / — | false / — | — |
| 4 | `page-esm` | `page-esm/index.ts` + `helper.ts` | page / inherit | S/S/S | `2339@134-138` | `title=string`、`user.name=string` / 同左 / — / — | false / — | — |
| 5 | `page-closure` | `page-closure/index.ts` | page / inherit | S/S/S | — | `config.title=string`、`user.name=string` / `页面标题`、`用户显示名称` / — / — | false / — | — |
| 6 | `component-properties` | `component-properties/index.ts` | component / inherit | S/S/S | `2339@130-138` | `title=string \| number`、`state=string` / `标题，支持字符串或数字`、`内部状态` / `title=copied` / `title=generated` | false / — | DOCS |
| 7 | `component-module` | `component-module/index.ts` | component / inherit | S/S/S | `2339@130-138` | `title=string \| number`、`state=string` / 同左 / — / — | false / — | — |
| 8 | `component-js` | `component-js/index.js` | component / pragma | S/S/S | `2339@130-138` | `title=string \| number`、`state=string` / 同左 / `title=copied` / — | false / — | DOCS |
| 9 | `properties-spread` | `properties-spread/index.ts` | component / inherit | P/P/P | — | —（hover partial，query 跳过） | false / — | — |
| 10 | `multi-registration` | `multi-registration/index.ts` | page / inherit | P/P/P | — | —（hover partial，query 跳过；顶层 301 另断言 `title=string`） | false / — | — |
| 11 | `js-jsdoc-pragma` | `js-jsdoc/index.js` | page / pragma | S/P/S | `2339@140-144` | `greeting=string`、`user.name=string` / `问候语`、`用户显示名称` / — / — | false / `[2322]` | — |
| 12 | `js-jsdoc-off` | `js-jsdoc/index.js` | page / off | S/D/S | — | `greeting=string`、`user.name=string` / 同左 / — / — | false / — | STATE |
| 13 | `js-checkjs` | `js-jsdoc/index.js` | page / inherit（`checkJs:true`） | S/S/S | `2339@140-144` | `greeting=string`、`user.name=string` / 同左 / — / — | false / — | — |
| 14 | `ts-nocheck` | `ts-nocheck/index.ts` | page / inherit | S/D/S | — | `title=string` / `页面标题` / — / — | false / — | STATE |
| 15 | `js-nocheck` | `js-nocheck/index.js` | page / pragma | S/D/S | — | `greeting=string` / `问候语` / — / — | false / — | STATE |
| 16 | `js-shebang-bom` | `js-shebang-bom/index.js` | page / pragma | S/S/S | `2339@91-98` | `title=string` / `页面标题` / — / — | false / — | — |
| 17 | `interface-root` | `interface-root/index.ts` | page / inherit | U/U/U | — | 短路：无 hover/diagnostics | false / — | — |
| 18 | `interface-root-import` | `interface-root-import/index.ts` + `models.ts` | page / inherit | U/U/U | — | 短路：oracle 期望 Unsupported；native 目前是 Partial | false / — | ROOT |
| 19 | `nullable` | `nullable/index.ts` | page / inherit | S/S/S | `18047@45-54`、`18046@98-106` | `users[0].name=string` / `用户显示名称` / — / — | false / — | — |
| 20 | `cross-file-overlay` | `cross-file-overlay/index.ts` + `models.ts` | page / inherit | S/S/S | — | `user.name=string` / `用户显示名称` / — / `user.name=external` | false / — | — |
| 21 | `alias-project` | `alias-project/index.ts` + `models/user.ts`（tsconfig extends/paths） | page / inherit | S/S/S | — | `user.name=string` / `用户显示名称` / — / `user.name=external` | false / — | — |
| 22 | `component-data-cross-file` | `component-data-cross-file/index.ts` + `models.ts` | component / inherit | S/S/S | — | `user.name=string`、`title=string` / `用户显示名称`、`标题` / — / `user.name=external` | false / — | — |
| 23 | `crlf-script` | `crlf-script/index.ts`（脚本 CRLF） | page / inherit | S/S/S | `2339@95-99` | `user.name=string` / `用户显示名称` / — / — | false / — | — |
| 24 | `data-shorthand` | `data-shorthand/index.ts` | page / inherit | U/U/U | — | 短路：无 hover/diagnostics | false / — | — |
| 25 | `js-source-check-off` | `js-source-check/index.js`（源码 `@ts-check`） | page / off | S/D/S | — | `title=string` / `页面标题` / — / — | false / — | STATE |
| 26 | `js-source-check-inherit` | `js-source-check/index.js` | page / inherit | S/S/S | `2339@91-98` | `title=string` / `页面标题` / — / — | false / — | — |
| 27 | `methods-nonliteral` | `methods-nonliteral/index.ts` | component / inherit | P/P/P | — | —（hover partial，query 跳过） | false / — | — |
| 28 | `unchecked-index` | `unchecked-index/index.ts`（`noUncheckedIndexedAccess:true`） | page / inherit | S/S/S | `2532@44-52` | —（无 query） | false / — | — |
| 29 | `nullable-no-strict-null` | `nullable/index.ts`（`strictNullChecks:false`） | page / inherit | S/S/S | — | `maybeUser.name=string`（无 doc 断言） / — / — | false / — | — |
| 30 | `no-implicit-this-off` | `no-implicit-this/index.ts`（`strict:false,noImplicitThis:false`） | page / inherit | N/N/N | — | —（hover unavailable，query 跳过） | false / — | PROBE |
| 31 | `js-bom-check-off` | `js-bom-shebang-check/index.js`（BOM+shebang，`checkJs:true`） | page / off | S/D/S | — | `title=string` / `页面标题` / — / — | false / — | STATE |
| 32 | `js-bom-check-inherit` | `js-bom-shebang-check/index.js` | page / inherit | S/S/S | `2339@91-98` | `title=string` / `页面标题` / — / — | false / — | — |
| 33 | `unicode-crlf` | `unicode-crlf/index.ts`（CRLF+CJK+emoji） | page / inherit | S/S/S | `2339@103-107` | `user.name=string`、`标题=string` / `用户显示名称`、`标题` / — / — | false / — | — |

补充说明：

- `interface-root-import` 的旧 oracle `states` 是 `U/U/U`，但 native 的
  `classify_data_root` 只识别同文件 interface/type alias；import 类型会落到
  `DataRootKind::Expression`，shadow 构建为 `Partial`。这是 ROOT gap，
  不是 checker 输出差异。
- `no-implicit-this-off` 的旧 oracle `states` 是 `N/N/N`，reason 含
  `probe root`；native 没有 probe health 层，当前会报 `Supported`。真实引擎可以
  证明 `title` 的类型是 `any`，但不能替代 product 的 `Unavailable` 决策。
- `js-jsdoc-off`/`ts-nocheck`/`js-nocheck`/`js-source-check-off`/
  `js-bom-check-off` 的 `jsCheckApplied` 都是 `off`；native `BuiltShadow` 能给出
  `JsCheckApplied::Off`，但 adapter 目前只发布空诊断 + `Supported`，没有
  `Disabled` 状态。这是 STATE gap。
- `properties-spread`/`multi-registration`/`methods-nonliteral` 的 `P/P/P`
  来自 `BuiltShadow.status=Partial` 与 reason；native 已有相同 reason
  （`spread assignment` / `2 page registrations` / `object literal`），
  可以由 matrix test 直接断言。
- `page-closure` 的 shadow 必须支持函数内 `Page(...)`、局部 interface/type alias、
  泛型调用 `identity({...})`；旧 oracle 支持，native shadow 也应支持。
- `alias-project` 需要把 tsconfig extends/paths 的**有效 options** 冻结进
  manifest，不能让测试去解析 tsconfig（P7 删除后 tsconfig 也不再存在）。
- `definition.kind` 在本表按 test-side 分类处理（`DefinitionTarget.path` +
  `generated_declarations`），所以 `page-basic`/`component-properties` 的 gap
  只写 DOCS；若 owner 要求 native 提供 `definition.kind` DTO，这两个 case 以及
  `cross-file-overlay`/`alias-project`/`component-data-cross-file` 会额外受
  DEFK 影响，见 §3.2 的敏感性分析。

### 2.3 断言族计数

| 断言族 | 数量 | scenario |
| --- | ---: | --- |
| 有预期诊断 | 16 个 scenario / 17 条诊断 | 见 §2.2 |
| hover 值断言（type/doc/docSource） | 26 个 scenario（`hover=supported`） | `properties-spread`/`multi-registration`/`methods-nonliteral` 为 partial，`no-implicit-this-off` 为 unavailable，3 个短路 |
| `documentationSource` | 3 | `page-basic`、`component-properties`、`component-js` |
| `definition.kind` | 5 | `page-basic`、`component-properties`、`cross-file-overlay`、`alias-project`、`component-data-cross-file` |
| `Diagnostics=Disabled`（STATE） | 5 | `js-jsdoc-off`、`ts-nocheck`、`js-nocheck`、`js-source-check-off`、`js-bom-check-off` |
| probe health（PROBE） | 1 | `no-implicit-this-off` |
| checker data-root（ROOT） | 1 | `interface-root-import` |
| 短路（Unsupported） | 3 | `interface-root`、`interface-root-import`、`data-shorthand` |
| `perturbsUserTypes=true` | 0 | — |
| `introduced` 非空 | 1 | `js-jsdoc-pragma` → `[2322]` |

---

## 3. 一份 real-engine matrix test 能覆盖哪些 ledger case

### 3.1 覆盖口径

把每个 case 拆成两层：

- **checker 半边**：`diagnosticsExact`、诊断 code/range、hover type/doc、
  definition target、`perturbsUserTypes`、`userScriptDiagnosticsIntroduced`。
  这些可以由 shadow + pinned engine 直接证明。
- **product/contract 半边**：`states=disabled/unavailable/unsupported` 的
  能力状态、`documentationSource`、`definition.kind` 的 DTO/分类。
  这些要么需要 adapter 行为改变，要么需要 owner 接受 test-side semantic
  classification。

**不能把“test 里用 native facts 推导出 Disabled/Unavailable/Unsupported”
当成 product 已经发布该状态。** 这正是 fake-green 的变体：C1 case 断言的是
run-level 状态，native adapter 若仍发布 `Supported`，测试就不应判 green。

### 3.2 33 个 in-place case 的严格覆盖

| 结果 | 数量 | case |
| --- | ---: | --- |
| **可 exact（strict）** | **23** | 21 个纯 checker：`page-export-empty`、`page-cjs`、`page-esm`、`page-closure`、`component-module`、`properties-spread`、`multi-registration`、`js-jsdoc-pragma`、`js-checkjs`、`js-shebang-bom`、`nullable`、`cross-file-overlay`、`alias-project`、`component-data-cross-file`、`crlf-script`、`js-source-check-inherit`、`methods-nonliteral`、`unchecked-index`、`nullable-no-strict-null`、`js-bom-check-inherit`、`unicode-crlf`；2 个短路径：`interface-root`、`data-shorthand` |
| **仍 blocked（strict）** | **10** | DOCS：`page-basic`、`component-properties`、`component-js`；STATE：`js-jsdoc-off`、`ts-nocheck`、`js-nocheck`、`js-source-check-off`、`js-bom-check-off`；PROBE：`no-implicit-this-off`；ROOT：`interface-root-import` |

严格 23 的前提：

1. `definition.kind` 由 test 根据 `DefinitionTarget.path` +
   `BuiltShadow.view.generated_declarations` 分类。这是
   ` backends/typescript/src/analysis.ts:1111` `classifyDefinitionTarget` 的直接移植：
   target 在 generated 区间 → `generated`；target 在 script → `script`；
   其他文件 → `external`。不需要新生产 DTO。
2. `properties-spread`/`multi-registration`/`methods-nonliteral` 的 `Partial`
   由 `BuiltShadow.status=Partial` + reason 直接断言；query 值不参与（oracle
   也跳过）。
3. `js-jsdoc-pragma` 的 `diagnostics=partial` 由 `introduced=[2322]` 推导；
   `introduced` 算法是 ` backends/typescript/src/analysis.ts:908` 的直接移植。
4. 23 个 case 的 `states` 都能由 `BuiltShadow` 的
   `status/reason/js_check_applied/registration` 加上 introduced/unmapped/
   dropped 事实推导，不依赖 probe health、behavior incomplete 或 checker
   data-root 分类。

**DEFK 敏感性：** 如果 owner 认为 `definition.kind` 必须由 native 生产 DTO
提供，而不是 test-side 分类，则 `cross-file-overlay`、`alias-project`、
`component-data-cross-file` 三个额外 blocked（`page-basic`/`component-properties`
已在 DOCS 中 blocked）。严格 exact 从 23 降到 20；整体投影从 97/206 变为
94/209；`blocked` 从 38 变为 41。本文推荐 test-side 分类，因为
`classifyDefinitionTarget` 是纯分类函数，事实（target path/range +
generated 区间）在 native 已完整可用，没有伪造。

**DOCS 敏感性：** 如果把 `documentationSource` 定义为
“hover 的声明任一落在 generated 区间 → `copied`，否则原 script 声明 →
`quickinfo`”，则 3 个 DOCS case 可以 exact；这是 `contract_translated` 的
语义分类，不是 fake。若 owner 要求 native 必须有 `documentationSource`
字段（native 没有 QuickInfo，只有 `getDocumentationComment`），则 3 个保持
blocked，且 `component-properties` 的顶层 271 也 blocked。

### 3.3 10 个顶层 case

这 10 个 case 的 oracle 都遍历两个 layout。symbol-bridge 已按 2026-11-17
决策退休；因此必须先做 **in-place re-scope** 裁决，否则严格意义上一个都不能
exact（只测 in-place 不等于原 case 的 both-layout 断言）。

| case | oracle 场景 | 断言要点 | 严格结果（in-place re-scope 后） | 依赖 |
| --- | --- | --- | --- | --- |
| `TS-MATRIX-0092` | 33×2 `doesNotThrow` | 全链路不抛 | 条件 exact（只保留 in-place build；checker 半边不在本测试） | owner 接受 re-scope |
| `TS-MATRIX-0215` | interface-root + interface-root-import | `/type-only/` reason、空结果 | blocked | ROOT |
| `TS-MATRIX-0228` | data-shorthand | `/not a property assignment/`、非 missing | exact | — |
| `TS-MATRIX-0239` | js-jsdoc-off | hover supported、diagnostics disabled、`/checkJs\|policy/`、hover `greeting=string` | blocked | STATE |
| `TS-MATRIX-0252` | js-source-check off + inherit | `jsCheckApplied`、诊断数、hover | blocked | STATE |
| `TS-MATRIX-0271` | component-properties | union、doc、`copied` | blocked | DOCS |
| `TS-MATRIX-0283` | BOM off + inherit | `jsCheckApplied`、诊断数、range 91..98 | blocked | STATE |
| `TS-MATRIX-0301` | multi-registration | partial reason、count=2、hover `title=string` | exact | — |
| `TS-MATRIX-0319` | unchecked-index + nullable-no-strict-null | 2532、diagnosticsExact、无 18047、hover string | exact | — |
| `TS-MATRIX-0335` | no-implicit-this-off | `unavailable` + `/probe root/` | blocked | PROBE |

严格 4 个 exact/条件 exact（092/228/301/319），6 个 blocked。
如果 owner 不接受 092 的 build-only re-scope，则 3 个 exact，7 个 blocked。

### 3.4 “一份 test”到底覆盖多少

- **一份** `real_matrix_in_place_matches_frozen_oracle` 可以覆盖 23 个 in-place
  ledger case 的 checker 半边，并直接把 23 个 case 做成 exact（在 §3.2 前提下）。
- **另一份** `real_matrix_known_gaps_are_characterized` 覆盖 10 个 blocked
  in-place case 的当前 native 行为，作为 `parity=blocked` 证据；它不是 parity
  green。
- **第三份** `real_matrix_top_level_cases` 覆盖 10 个顶层 case 的 in-place
  半边；只有 4 个可 exact/条件 exact，其余 6 个登记 blocked。
- 因此“一份 test 覆盖 43 个 case”不成立；正确说法是：
  **一个 test 模块 + 三组 test 覆盖 43 个 case，其中 27 个可 exact，
  16 个严格 blocked。**

---

## 4. `real_matrix.rs` 设计

### 4.1 位置与模块接线

推荐放在：

```text
crates/wxml-lsp/tests/native_parity/real_matrix.rs
```

理由：

1. ledger 的 43 个 `target_rust_test` 都指向
   ` crates/wxml-lsp/tests/native_parity/scenario_matrix.rs`；把真实引擎测试放在
   同目录，可直接通过 `scenario_matrix.rs` 的薄包装调用，保持 target 路径稳定。
2. 该目录已经能使用 `wxml_backend_tsgo` 公开 API、`crate::native_gate_env`、
   `crate::native_package`、`crate::ledger`、`crate::source_scan`；
   `crates/wxml-backend-tsgo/tests/real_matrix.rs` 无法复用这些 ledger/gate
   基础设施（也不应让 `wxml-backend-tsgo` 反向依赖 `wxml-lsp`）。
3. 如果后续要在同一 fixture 上跑 LSP product smoke，`wxml-lsp` 测试目录是
   唯一能同时使用 adapter 和 backend 的位置。

需要改的接线（落地时，不在本文件修改）：

- ` crates/wxml-lsp/tests/native_parity.rs` 增加
  `#[path = "native_parity/real_matrix.rs"] mod real_matrix;`。
- ` crates/wxml-lsp/tests/native_parity/scenario_matrix.rs` 的
  `MIGRATED_CASES` 从空改为 23 个 exact case 的测试标签；`PARTITION` 仍是 43。
- ` crates/wxml-lsp/tests/native_parity.rs:170` 的
  `partition_skeletons_match_manifest_and_fail_closed` 目前只允许
  `symbols_edits` 有 migrated cases；需要像 `SYMBOLS_EDITS_MIGRATED_CASES`
  一样增加 `SCENARIO_MATRIX_MIGRATED_CASES`，否则该测试会红。
- ` crates/wxml-lsp/tests/native_parity/ledger.rs` 的
  `GREEN_CASES/PLANNED_CASES/BLOCKED_CASES/WEAKER_CASES` 和 `counts` 同步更新。
- ` docs/reference/tsgo-test-contract.md` §8 G5 需要允许
  `parity=blocked` 的书面证据，或保持 G5 红；不能只改 JSON。

### 4.2 test 函数与运行模式

```rust
// crates/wxml-lsp/tests/native_parity/real_matrix.rs

#[test]
fn real_matrix_manifest_is_frozen_and_oracle_derived() { /* 默认运行，无引擎 */ }

#[test]
fn real_matrix_comparator_rejects_missing_shifted_and_perturbed_results() { /* 默认运行，无引擎 */ }

#[test]
#[ignore = "required native matrix job: WXML_TEST_NATIVE=1 + WXML_REQUIRE_REAL_ENGINE=1"]
fn real_matrix_in_place_matches_frozen_oracle() { /* 23 exact */ }

#[test]
#[ignore = "required native matrix job: WXML_TEST_NATIVE=1 + WXML_REQUIRE_REAL_ENGINE=1"]
fn real_matrix_known_gaps_are_characterized() { /* 10 blocked 的当前行为 */ }

#[test]
#[ignore = "required native matrix job: WXML_TEST_NATIVE=1 + WXML_REQUIRE_REAL_ENGINE=1"]
fn real_matrix_top_level_cases() { /* 10 顶层 case 的 in-place 半边 */ }
```

- 默认运行的两个测试是“证据来源测试”：它们证明 manifest 与 `matrix.json`
  的投影一致、比较器能反向失败。它们**不证明**引擎跑过。
- 三个 `#[ignore]` 测试是“required native job”测试：`WXML_TEST_NATIVE=1`
  必须显式设置；`WXML_REQUIRE_REAL_ENGINE=1` 时缺引擎/资产必须 panic。
  没有 required 标志的 optional 运行可以打印 skip 并 return，但该结果不能
  写进 ledger 的 `parity=exact` 证据。
- 三个测试的 `--ignored` 运行必须由 blocking native job 执行；默认
  `cargo test` 只报告 ignored，不报告 passed。

### 4.3 pinned engine 与 assets

required job 的环境（与 `native_gate_env.rs` 对齐）：

```text
WXML_TEST_NATIVE=1
WXML_REQUIRE_REAL_ENGINE=1
TSGO_BIN=/abs/extracted-package/lib/tsc
TSGO_ASSET_ROOT=/abs/extracted-package
TSGO_TYPINGS_ROOT=/abs/dir-containing-miniprogram-api-typings
WXML_TEST_TMPDIR=/tmp/opencode
```

测试启动前必须验证：

1. `TSGO_BIN` 是普通文件，SHA-256 等于
   ` PinnedEngine::pinned().executable_sha256()`；
   `--version` 输出 `Version 7.1.0-beta`。
2. `TSGO_ASSET_ROOT`（或 `TSGO_BIN` 的 `lib` 父目录）包含 pinned
   `lib.*.d.ts`；`TSGO_TYPINGS_ROOT` 包含 `miniprogram-api-typings` 5.3.1。
3. `TypeAssets::load_pinned(engine_lib, typings_root)` 成功。它会对 116 个
   standard-lib 文件和 20 个 typings 文件做 count/bytes/bundle digest 校验；
   任一不符立即失败，**不得**降级为 legacy `AnalysisInput::new` 的 `noLib:true`。
4. sandbox 是 `/tmp/opencode` 下的空目录；virtual project root 是不存在的
   绝对路径。测试结束断言 project root 没有落盘、sandbox 已被清理。
5. `NativeSession::start(SessionOptions::new(engine, sandbox))` 后断言
   `initialize.current_directory() == sandbox`；`live_snapshot_count()` 始终为 1。

### 4.4 冻结 fixture 与 oracle manifest

新增目录（不进入 `tests/fixtures` 的 v1 hash 树）：

```text
crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/
  oracle.json
  scenarios.json
  samples/<dir>/...
```

- `samples/` 是 `backends/typescript/samples` 的字节级冻结副本（33 个 scenario
  用到 35 个文件；`alias-project` 含 `models/user.ts`，`page-esm` 含
  `helper.ts`）。P7 删除旧 samples 后，测试仍能跑。
- `.gitattributes` 必须为 `crlf-script/index.ts` 和
  `js-bom-shebang-check/index.js` 加 `-text`，保留 CRLF/BOM 字节；否则
  Windows checkout 会改变 fixture，导致诊断 range 漂移。
- `scenarios.json` 是 `scenarios.ts` 的冻结投影：scenario id、ledger case id、
  sample dir/script/extra files、registration、allowJs/checkJs/jsCheck、
  有效 compiler options（含 `alias-project` 的 `baseUrl`/`paths`）、
  WXML 文本、fragments（expr/target/WXML byte range）、queries
  （expr/expectType/expectDoc/expectDocSource/definition/expectDefinition）、
  expectedDiagnostics（expr/target/WXML range）。
- `oracle.json` 是 `matrix.json` 的 in-place 投影：每个 scenario 的
  `states`、`jsCheckApplied`、`diagnosticsExact`、`perturbsUserTypes`、
  `userScriptDiagnosticsIntroduced`、`diagnostics[]`（code/range）、
  `hovers[]`（query/typeOnly/documentation/documentationSource）、
  `definitions[]`（query/kind）、`registration.count`、`request`。
- `oracle.json` 记录：
  - `source.path = docs/evidence/data/type-bridge-c1/matrix.json`
  - `source.sha256 = 69bc6c...`
  - `source.environment = {node, typescript: 6.0.3, typings: 5.3.1}`
  - `selection = {layout: "in-place-shadow", scenarios: 33}`
  - `projection_sha256`：对排序后的 scenario 投影做 canonical JSON SHA-256。
  - `fixture.source = backends/typescript/src/scenarios.ts` 的 SHA-256（历史锚点）。
- 默认测试 `real_matrix_manifest_is_frozen_and_oracle_derived` 必须：
  1. `oracle.json` 的 `source.sha256` 等于 `matrix.json` 的实际 SHA；
  2. `oracle.json` 的 scenario 集合恰好 33 个 `in-place-shadow`，无重复/缺失；
  3. 从 `oracle.json` 重算 `projection_sha256`，等于固定常量；
  4. 从 `scenarios.json` 的 WXML + expectedDiagnostics 重算每条预期诊断的
     WXML range，与 `oracle.json` 的 `diagnostics[].range` 一致；
  5. 对每个 sample 文件校验 SHA-256；
  6. 确认没有 native 录制字段（例如 `observed_at`、`native_engine` 写入期望）。
- **禁止**在测试中提供 `BLESS`/`UPDATE_EXPECT`/`INSTA_UPDATE`；测试只读。

### 4.5 shadow 构建

对每个 scenario：

1. `SourceFile::new(script, text, DocumentRevision::new(1))`；
   extra files 同样。
2. 从 `scenarios.json` 的 fragments 构造 `FragmentSpec`：
   - `id = expr`（同一 expression 只建一个 fragment）；
   - `expr`、`target`（可为 `None`）；
   - `wxml_range = SourceRange::new(start, end)`，长度必须等于 expr 的 UTF-8
     字节数；`locateExpression` 的语义是 `{{ expr }}` 内部的 expr 起点，
     不是 `class="..."` 里的同名文本。
   - `locals/bindings/guards` 全空（33 个 scenario 没有 `wx:for`/`wx:if`）。
3. `ShadowRequest { script, root, fragments, js_check, check_js }`，
   `build_in_place_shadow`。
4. 断言：
   - `built.registration.count` / `kind` 等于 oracle；
   - `built.status`/`reason` 等于 oracle 的 build 侧期望；
   - `built.js_check_applied` 映射到 oracle 的 `jsCheckApplied`；
   - `built.view.mounts` 覆盖每个 fragment（除非短路）。
5. 对 `interface-root`/`data-shorthand` 断言
   `Err(ShadowError::Unsupported { feature: "data.root", .. })`，reason 含
   `type-only` / `not a property assignment`；这是短路径 exact。
6. 对 `interface-root-import` 断言当前是 `DataRootKind::Expression` /
   `Partial`，并标记 ROOT gap；不要把它写成 Unsupported。

### 4.6 baseline / shadow 两个 snapshot

为计算 `perturbsUserTypes` 与 `userScriptDiagnosticsIntroduced`，每个 scenario
需要两个 program：

- **baseline**：原始 script + extra files，不含 shadow。
- **shadow**：`built.view.text` 作为 script + 原 extra files。

同一 `NativeSession` 内：

1. 第一个 scenario：`open_project(baseline_input)`（snapshot revision 1）。
2. 查询 baseline 的 `this.<member>` 类型与 semantic diagnostics。
3. `update_project(shadow_input)`（revision 2）。
4. 查询 shadow 的 diagnostics、hover、definition。
5. 之后的每个 scenario 继续用 `update_project`：baseline revision 3、
   shadow revision 4、baseline 5、shadow 6……不重复 `open_project`；
   保持 `live_snapshot_count()==1`，并在每个 scenario 结束时确认旧 snapshot
   已释放。

`AnalysisInput::with_environment` 必须同时带 `CompilerOptions` 与
`Arc<TypeAssets>`；不能省略 typings。`ProjectRoot` 使用同一个虚拟绝对路径；
`ProjectKey` 使用固定 `"real-matrix"`。

### 4.7 `perturbsUserTypes`

移植 ` backends/typescript/src/analysis.ts:1236` `thisFingerprint`：

1. 在原始 script 中找到所有 `this.<identifier>` 的 `<identifier>` start
   offset。必须跳过字符串、注释、模板文本；不能用 `indexOf`/正则直接扫
   全文。建议在测试里实现一个只识别 `this` `.` identifier 的轻量 lexer，
   或复用 `script::scanner` 的 token 结果（如果它保留了 `this`/`.` 的
   位置）。
2. baseline：对每个 offset 调 `session.type_query(script, position)`，
   `text = type_result.info.map(|i| i.text).unwrap_or("<none>")`。
3. shadow：用 `built.view.map.to_derived_offset(original_offset)` 映射，
   再 `type_query`，同样取 text。
4. `perturbs = baseline_types != shadow_types`。
5. 断言：
   - 每个 baseline offset 的 type text 不是 `<none>`（否则测量是空的，
     false 不可信）；
   - offsets 数量与 shadow offsets 数量一致；
   - `perturbs` 等于 oracle（in-place 全部 false）。
6. **反向自测**（无引擎）：把 baseline `["string"]` 与 shadow `["number"]`
   比较，必须得到 `perturbs=true`；两边都 `<none>` 不得被当成有意义的
   equal。`no-implicit-this` 没有 `this.`，应记录 `offsets=0`，不能把
   空指纹的 false 当作强证据。

### 4.8 diagnostics

1. `session.semantic_diagnostics(script)` + `session.syntactic_diagnostics(script)`。
   对 shadow 文本断言 syntactic 为空；非空是 shadow 生成 bug，直接失败。
2. `map_diagnostics(&semantic, &syntactic, built.view.text)`；要求
   `status == Supported`（或记录 Partial 原因），否则不能声称 exact。
3. 对每条 `NativeDiagnostic`：
   - 用 `position_to_offset(shadow_text, diagnostic.start)` 得到 shadow
     UTF-16 offset；
   - 找到第一个 `mount.start_utf16 <= offset < mount.end_utf16()` 的 mount；
   - 不在任何 mount 内 → script-owned diagnostic，不发布（suppressed）；
   - 在 mount 内 → `map_span_to_source(mount, offset, length)`；
     `unattributable`/无 range → `unmapped += 1`，diagnostics state 不能声称
     Supported；
   - 得到 WXML byte `SourceRange`，按 `(code, range.start, range.end)` 去重。
4. 与 oracle 比较：
   - `expected` ranges 来自 `oracle.json` 的 `diagnostics[].range`；
   - `missing`/`unexpected` 与 `diagnosticsExact` 语义与
     ` backends/typescript/src/run.ts:337` 一致；
   - 16 个有诊断的 scenario 额外断言 code 与 severity=Error；message
     只记录不 block（message sanitization 是 W11-A 的独立切片，
     ` backends/typescript/src/messages.ts` 尚未移植）。
5. 正向 sentinel：
   - `nullable` 必须观测到 18047 和 18046；
   - `unchecked-index` 必须观测到 2532；
   - `js-shebang-bom`/`js-source-check-inherit`/`js-bom-check-inherit` 必须
     观测到 2339@91-98；
   - `unicode-crlf` 必须观测到 2339@103-107。
   这些正例证明引擎真的跑了；没有它们，“空诊断 + diagnosticsExact=true”
   可能只是引擎没启动。

### 4.9 hover / type / documentation

1. 对每个 `hover=supported` 的 scenario，取 `spec.queries`；对每个 query：
   - 在 `built.view.mounts` 找 `fragment_id == expr` 的 mount；
   - 位置 = `mount.target_start_utf16`；
   - `session.hover_evidence(script, position)`；
   - 若 query 有 `expectType`：
     - `type_info.text` 或 `built.properties` 的 declared union；
     - `component-properties`/`component-js` 的 `title` 需要
       `DeclaredProperty.type_text` 覆盖（` built.properties.complete == true`
       且 `built.properties.properties` 中存在 `name=="title"` 的
       `DeclaredProperty`）；
     - 与 oracle `typeOnly` 逐字比较。
   - 若 query 有 `expectDoc`：
     - `hover.documentation.comment` trim 后与 oracle `documentation` 比较；
     - 空 doc 但 oracle 非空 = 失败。
   - 若 query 有 `expectDocSource`：
     - `copied`：hover 的任一 declaration 落在
       `built.view.generated_declarations` 的 derived 区间；
     - `quickinfo`：hover 的 declaration 在原始 script 区间（不是 generated）；
     - 这是 test-side semantic classification，需 owner 在
       `docs/reference/tsgo-test-contract.md` 记录 “native 没有 QuickInfo，
       quickinfo 标签表示原始声明文档，不表示 display parts 来源”。
   - 若 query 没有 `expectType`/`expectDoc`（`user.nmae` 等）：
     - oracle 仍断言 hover 存在；测试至少要求
       `map_type_query(&session.type_query(..)).info.is_some()` 或 symbol
       存在；不能要求 `HoverEvidence.status` 必须 Supported（native 对
       missing property 的质量映射与旧 QuickInfo 不同）。
2. `properties-spread`/`multi-registration`/`methods-nonliteral` 的 hover
   是 partial → query 循环跳过；只断言 `built.status=Partial` + reason。
3. `no-implicit-this-off` hover 是 unavailable → query 跳过；gap 测试断言
   root type 是 `any`，并记录 product 仍报 Supported 的差异。
4. 不把 `typeToString` 的完整 QuickInfo 前缀与旧 `typeText` 比较；只比较
   旧 matrix 真正断言的 `typeOnly`。`quality` 不在 matrix 断言内。

### 4.10 definition

1. 对 `query.definition == true` 的 5 个 query：
   - `session.definition(script, position)`；
   - `map_definition`；
   - 对每个 target 分类：
     - `target.path == script` 且 `target.utf16` 落在
       `built.view.generated_declarations` 的 derived 区间 → `generated`；
     - `target.path == script` → `script`；
     - 其他 → `external`。
   - 断言 oracle `kind` 等于**第一个** target 的分类；这与
     ` backends/typescript/src/analysis.ts:1179` `const first = raw[0]` /
     `classifyDefinitionTarget(first, ...)` 一致。如果 native 同时返回
     script/generated 且第一个不是 oracle kind，这是 target 顺序 finding，
     不能用“任意 target 是 generated”掩盖；此时该 case 降为 weaker/blocked，
     或先修 native definition 排序。
   - range 非空：
     - generated → `generated.source` 在原 script 的字节区间非空；
     - script → `ShadowMap::to_original_offset` 后字节非空；
     - external → 目标文件文本的字节区间非空。
2. `component-properties` 的 `generated` 依赖 native definition target
   第一个就是 overlay 声明；如果只返回原始 `properties.title`，这是 target
   顺序/回归风险（见 §7），不能用“任意 target 是 script”替代。
3. `cross-file-overlay`/`alias-project`/`component-data-cross-file` 的
   `external` 依赖 `models.ts`/`models/user.ts` 在 `AnalysisInput` 中；
   `alias-project` 的有效 `baseUrl`/`paths` 来自 manifest，不解析 tsconfig。
   `baseUrl` 在 manifest 里用 `<project-root>` 占位符记录，测试启动时替换为
   同一个虚拟 `ProjectRoot`；不能让 engine 把 sandbox cwd 当作 baseUrl。

### 4.11 `userScriptDiagnosticsIntroduced`

移植 ` backends/typescript/src/analysis.ts:908`：

1. baseline semantic diagnostics 建 multiset：
   `(code, start_utf16, length_utf16)`。
2. shadow semantic diagnostics：
   - 在任何 mount 内 → 跳过；
   - `built.view.map.to_original_offset(shadow_start)`：
     - Ok → key `(code, original_offset, length)`；
     - Err（inserted text）→ key `(code, synthetic, length)`；
   - 从 baseline multiset 扣除；扣不到的进入 `introduced`，上限 8。
3. 断言 `js-jsdoc-pragma` 得到 `[2322]`，其余 exact case 得到 `[]`。
   `[2322]` 是 positive sentinel；如果引擎/shadow 坏了，这个 case 会失败。
4. 反向自测：baseline 有一条 `(2322, 10, 3)`，shadow 也有一条相同 key，
   introduced 必须为空；shadow 多一条不同 key，必须出现。

### 4.12 states

移植 ` backends/typescript/src/analysis.ts:795` `computeStates` 的决策表：

- `ShadowError::Unsupported`（短路径）→ 三能力 `Unsupported`。
- `build.status == Partial` → 三能力 `Partial`（带 build reason）。
- `js_check_applied == Off` → `diagnostics=Disabled`，reason 按 policy/
  `@ts-nocheck` 区分。
- `introduced.len() > 0` → `diagnostics=Partial`，reason 含
  “script diagnostic(s) outside the fragments”。
- `unmapped_diagnostics > 0` → `diagnostics` 降级。
- `dropped_definitions > 0` → `definition` 降级。
- 否则三能力 `Supported`。

**边界：**

- `no-implicit-this-off` 的 `Unavailable` 需要 probe health；在 product
  没有该 gate 前，不要用 test-side “root type any → Unavailable” 冒充 exact；
  登记 `parity=blocked`。
- 5 个 STATE case 的 `Disabled` 需要 adapter 在
  ` crates/wxml-lsp/src/backend_tsgo/diagnostics.rs` 发布
  `DiagnosticsResult.status=Disabled`；在 product 仍报 Supported 前，
  不要用 test-side 推导冒充 exact。
- `interface-root-import` 的 `Unsupported` 需要 checker-based data-root
  分类；在 product 仍报 Partial 前，不要用 test-side 推导冒充 exact。

### 4.13 比较器反向自测

`real_matrix_comparator_rejects_missing_shifted_and_perturbed_results`
必须覆盖 `docs/reference/tsgo-test-contract.md` §4 的要求：

- 注入缺失诊断 → `diagnosticsExact=false`；
- 注入 unexpected 诊断 → false；
- range 偏移 1 byte → false；
- code 不同但 range 相同 → strict comparator 必须 false（比旧 oracle 的
  range-only 更强一层）；
- 丢失 hover doc → false；
- `perturbsUserTypes` 注入 type 变化 → true；
- introduced 注入新 key → 非空；
- 空 vs 空：只在 expected 为空且 actual 为空、且已执行 scenario 计数 >0 时
  才 pass；不能让“两个空结果”自动 PASS。

---

## 5. required / optional 运行模式与 ledger 合法性

### 5.1 现有 gate 模型

` crates/wxml-lsp/tests/native_parity/native_engine_real.rs:1` 的模式是：

- 测试 `#[ignore]`；
- `WXML_TEST_NATIVE=1` 显式请求；没有它运行 ignored test 是失败；
- `WXML_REQUIRE_REAL_ENGINE=1` 时缺引擎是 hard panic；
- 没有 required 标志时，缺引擎是**显式打印的 optional skip**，返回 `None`，
  不是 pass。

` crates/wxml-lsp/tests/native_parity/native_gates.rs:1` 明确写：
`gate_green` 是 opt-in required-native-job 结果，不是 default-CI pass，
也不是 `mandatory_green`。matrix parity 必须沿用这个语义，并更进一步：
**ledger 的 `parity=exact` 不能由 optional skip 产生。**

### 5.2 防止“绿而无证据”的硬规则

1. **测试层**
   - 三个 real-engine 测试都是 `#[ignore]`；default `cargo test` 只显示
     ignored，不显示 passed。
   - `WXML_TEST_NATIVE != 1` 时运行 ignored test → panic，不是 skip。
   - `WXML_REQUIRE_REAL_ENGINE == 1` 且 `TSGO_BIN`/assets/typings 缺失 →
     panic，不是 skip。
   - 测试开头加载 manifest，断言 `scenarios.len() == 33`；
     结束时断言 `compared == 23`（exact test）或 `gaps == 10`（gap test）。
     任何一个 scenario 被跳过/缺失都失败。
   - 每个正例 sentinel（2339/2532/18047/18046/2322）必须出现；否则
     “空诊断全过”不可能发生。
   - 测试只读；没有 `BLESS`/`UPDATE_EXPECT`/`INSTA_UPDATE`/写入期望的分支。
2. **ledger 层**
   - 新增 `migration_evidence.run_mode`：
     `default | required_native`。
   - `parity=exact` + `run_mode=default` → 被引用的 `test` 不得是
     `#[ignore]`。
   - `parity=exact` + `run_mode=required_native` → 被引用的 `test` 必须是
     `#[ignore]`，且 evidence 必须包含 `required_env`（至少
     `WXML_TEST_NATIVE=1`、`WXML_REQUIRE_REAL_ENGINE=1`、`TSGO_BIN`、
     `TSGO_ASSET_ROOT`、`TSGO_TYPINGS_ROOT`）和 `oracle_sha256`。
   - `run_mode` 缺失时只能用于 2026-11-20 之前的 legacy evidence（ grandfather），
     新回填必须显式写。
   - `parity=blocked` 的 evidence 必须包含 `blocked_by`
     （`docs|state|probe|root|checker|other`）和 `observed`/`expected`
     的差异摘要；不能只有一句空 note。
   - 每个 `parity=exact` 的 case 至少引用一个 `scenario_matrix` 分区的测试，
     或引用默认运行的 manifest 测试 + required-native parity 测试的组合。
3. **CI 层**
   - blocking native job 运行：
     `WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 TSGO_BIN=... TSGO_ASSET_ROOT=... TSGO_TYPINGS_ROOT=... cargo test -p wxml-lsp --test native_parity real_matrix -- --ignored --nocapture`
   - job 失败即 case 不能 green；job 未运行不能由 default job 的 ignored
     结果替代。
   - 测试打印一行机器可读摘要：
     `real_matrix: exact=23 gaps=10 scenarios=33 engine=7.1.0-beta oracle=69bc6c... projection=<sha>`；
     该行进入 CI 日志，便于审计。

### 5.3 证据 note 模板

```text
Required-native matrix parity: `real_matrix_in_place_matches_frozen_oracle`
(`crates/wxml-lsp/tests/native_parity/real_matrix.rs`), scenario `page-basic`,
run with WXML_TEST_NATIVE=1 + WXML_REQUIRE_REAL_ENGINE=1 + pinned
TSGO_BIN/ASSET_ROOT/TYPINGS_ROOT. Expected values are a frozen projection of
`docs/evidence/data/type-bridge-c1/matrix.json`
(sha256 69bc6c..., TypeScript 6.0.3, in-place-shadow only); no native replay was
recorded. The test asserts the 2339@134-138 diagnostic, title/user.name hover
type+doc, user.name definition kind, perturbs=false and introduced=[].
```

对 blocked case：

```text
Blocked by STATE: the checker half is proven by the required-native matrix test
(js_check_applied=Off, no diagnostics), but the adapter still reports
`Supported`; C1 `diagnostics=disabled` is not published. Case stays planned
until `DiagnosticsResult.status=Disabled` lands and is tested.
```

### 5.4 为什么 default manifest 测试必要但不充分

- 必要：它证明 `oracle.json` 确实是 `matrix.json` 的投影，且 scenario 集合、
  WXML range、sample SHA 完整；没有它，required job 可能拿一份手改的期望跑绿。
- 不充分：它不启动引擎，不能证明 native 7.1.0-beta 真的产生了那些值。
- 因此 ledger 的 exact evidence 必须同时引用：
  1. `real_matrix_manifest_is_frozen_and_oracle_derived`（默认运行）；
  2. `real_matrix_in_place_matches_frozen_oracle`（required native）。
  这与 `TS-SYMBOLS-0061` 同时引用默认 structural 测试 + required-native
  smoke 的现有模式一致。

---

## 6. 覆盖后的 ledger 计数投影

### 6.1 阶段表

基线：`70 green / 233 planned / 36 retired`；`exact=70`、`near=2`、
`weaker=96`、`structural=7`、`blocked=22`、`assessed=197`、无证据 `106`。

| 阶段 | 新增 | matrix green/blocked | overall green/planned/retired | exact | weaker | blocked | assessed | 无证据 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 基线 | — | 0/0（43 planned） | 70/233/36 | 70 | 96 | 22 | 197 | 106 |
| 1 严格 real matrix | 23 in-place + 4 顶层 | **27/16** | **97/206/36** | 97 | 92 | 38 | 236 | 67 |
| 2 + STATE adapter | +5 in-place +3 顶层 | 35/8 | 105/198/36 | 105 | 92 | 30 | 236 | 67 |
| 3 + DOCS 语义分类 | +3 in-place +1 顶层 | 39/4 | 109/194/36 | 109 | 92 | 26 | 236 | 67 |
| 4 + PROBE health | +1 in-place +1 顶层 | 41/2 | 111/192/36 | 111 | 92 | 24 | 236 | 67 |
| 5 + ROOT 分类 | +1 in-place +1 顶层 | 43/0 | 113/190/36 | 113 | 92 | 22 | 236 | 67 |

说明：

- `blocked` 列是 ledger `counts.migration_evidence.blocked` 桶，不是 D 类
  31 个 blocked。阶段 1 把 16 个 matrix gap 登记为 `parity=blocked`，
  所以 22 → 38；阶段 5 回到 22（剩余的是本切片之外的原有 blocked）。
- matrix 分区原有 4 个 `weaker`（`TS-MATRIX-0215/0228/0271/0301`）。阶段 1
  重新评估：228/301 → exact，215/271 → blocked；所以 `weaker` 96 → 92，
  `assessed` 197 → 236（39 个无证据 matrix case 新增证据；4 个 weaker 原本
  已在 assessed 里），无证据 106 → 67。
- 后续阶段只是把 blocked 改成 exact，不再增加 assessed；`weaker` 保持 92。
- `scenario_matrix` 分区：routed 43 不变；阶段 1 evidence 43、
  green 27 / planned_evidence 16 / unassessed 0、weaker 0；阶段 5
  green 43 / blocked 0。

### 6.2 D 类 31 个 blocked 的变化

D 类 31 = 30 个 in-place checker case + `interface-root-import`（ROOT）。
它与 ledger 的 `blocked=22` 不是同一个集合。

| 阶段 | D 31 green | D 31 blocked | 说明 |
| --- | ---: | ---: | --- |
| 0 | 0 | 31 | 全部 planned、无证据 |
| 1 | 21 | 10 | 21 个纯 checker；10 = 3 DOCS + 5 STATE + 1 PROBE + 1 ROOT |
| 2 | 26 | 5 | STATE 关闭；剩 3 DOCS + PROBE + ROOT |
| 3 | 29 | 2 | DOCS 关闭；剩 PROBE + ROOT |
| 4 | 30 | 1 | PROBE 关闭；剩 ROOT |
| 5 | 31 | 0 | ROOT 关闭 |

另外 2 个短路径 case（`interface-root`、`data-shorthand`）不在 D 31 内，
阶段 1 即可 exact；所以 33 个 in-place 的 green 是 23，而不是 21。

### 6.3 严格 / 保守变体

| 变体 | matrix green/blocked | overall green/planned/retired | exact | blocked |
| --- | ---: | ---: | ---: | ---: |
| 严格（推荐，092 接受，DEFK test-side） | 27/16 | 97/206/36 | 97 | 38 |
| 不接受 092 的条件 exact | 26/17 | 96/207/36 | 96 | 39 |
| DEFK 必须新 DTO | 24/19 | 94/209/36 | 94 | 41 |
| 两者都不接受 | 23/20 | 93/210/36 | 93 | 42 |

### 6.4 P7-1 的现实

- 阶段 1 只减少 27 个 planned（233→206，约 11.6%）；阶段 5 也只到
  190 planned（约 18.5%）。
- 剩余 190 planned 包含 67 个无证据 planned、92 weaker、2 near_exact、
  7 structural、22 原有 blocked。matrix parity 不处理这些。
- 因此 **P7-1 planned=0 不能由本切片完成**。本切片的价值是把“31 个 D 类
  blocked”中的 checker 半边变成真实引擎证据，并把 23-43 个 case 合法推进；
  它不是 P7-1 的收尾。

---

## 7. 风险

| 级别 | 风险 | 缓解 |
| --- | --- | --- |
| 高 | TS 6.0.3 旧 oracle vs 7.1.0-beta native：诊断 code/range/type text 可能变化 | 先跑 calibration-only 报告，逐 case 分类 native bug / version change / harness bug；只有 owner 书面接受 deviation 才改 projection；禁止 native 回放覆盖 |
| 高 | optional skip / `#[ignore]` 未执行被写成 green | required job + `run_mode=required_native` + `required_env` + `compared==23` + positive sentinels；default job 只报告 ignored |
| 高 | 用 test-side 推导的 Disabled/Unavailable/Unsupported 冒充 product 状态 | 5 STATE + 1 PROBE + 1 ROOT 保持 blocked，直到 adapter 真正发布对应状态并有 adapter 测试 |
| 高 | DEFK/DOCS 的 test-side 分类被 reviewer 认为不 exact | 书面 `contract_translated` 裁决；或新增 native DTO 字段；两者都不做就保持 blocked |
| 中 | `user.nmae` 等 typo query 在 native `hover_evidence` 没有 observation | 用 `map_type_query(..).info.is_some()` 或 symbol 存在作为“observation 存在”的等价；若仍无，登记 hover 子 gap |
| 中 | `component-properties` 的 generated definition 目标顺序与旧 oracle 不同 | 断言 target 集合中存在 generated；记录顺序；若 owner 要求第一个 target，先修 native definition 排序 |
| 中 | `alias-project` 的 `baseUrl`/`paths` 在虚拟 VFS 下解析差异 | manifest 冻结有效 options；用 AC-008 native gate 的 alias 项目经验；必要时单独 LSP smoke |
| 中 | frozen fixture 的 CRLF/BOM 被 git 归一化 | 新 fixtures 目录 + `.gitattributes -text` + 逐文件 SHA-256 |
| 中 | 33 scenario × 2 snapshot 的启动/查询时间过长或超时 | 一个 session、顺序 update、每次 release、固定 deadline；required job 单独运行，不和 default suite 并行 |
| 中 | definition `generated`/`documentationSource=copied` 依赖 `generated_declarations` 区间 | 用 `built.view.generated_declarations` 的 derived UTF-16 区间；同时校验 `source` 原 script 区间非空 |
| 低 | ledger 常量/计数漂移 | 回填时从 JSON 重新计数；同步 `GREEN_CASES/PLANNED_CASES/BLOCKED_CASES/WEAKER_CASES`、`counts`、`by_partition` |
| 低 | P7 删除 `backends/typescript/samples`/`scenarios.ts` 后测试读不到 | 先把 35 个 sample 和 33 个 WXML 冻结到 `native_parity/fixtures/real_matrix/`；`matrix.json` 按 ADR 保留为历史证据 |
| 低 | G5 仍红 | 明确写：31 个 D blocked 里仍有 10 个（阶段 1）或 0 个（阶段 5）未关；G5 需要 contract 允许 blocked 证据或全部转 green |

---

## 8. 工作量

| 工作项 | 估计 | 评级 |
| --- | ---: | --- |
| 旧 oracle projection + fixture 冻结 + manifest 默认测试 | 2-3 天 | 低-中 |
| pinned engine/assets 解析 + `TypeAssets::load_pinned` + required gate | 1-2 天 | 中 |
| shadow + baseline/shadow session + diagnostics 映射 + hover/definition | 5-8 天 | 中-高 |
| 比较器反向自测 + positive sentinels + calibration | 2-3 天 | 中 |
| 23 exact + 10 gap + 10 顶层测试 | 2-3 天 | 中 |
| ledger/validator/contract 回填 | 1-2 天 | 低-中 |
| **阶段 1 合计** | **13-21 天（约 3-4 周）** | **中-高** |
| STATE adapter（5 case） | 3-5 天 | 中 |
| DOCS 语义分类（3 case） | 2-4 天 | 中 |
| PROBE health（1 case，含接线） | 5-8 天 | 中-高 |
| ROOT checker classification（1 case） | 5-8 天 | 中-高 |
| **阶段 5 合计** | **28-46 天（6-9 周）** | **高** |

阶段 1 的 13-21 天不含 TS 7.1 calibration 发现的修复；如果版本差导致多个
scenario 失败，按每个 finding 1-3 天追加。阶段 4/5 的生产改动会触碰现有
green 的 `Supported` 语义，必须单独切片、单独 gate，不能和 matrix test 混在
一个 PR。

---

## 9. 本文件不声称

1. 不声称任何 case 已 `green`、`reviewed` 或 `closed`；23/27 是“可由
   默认运行 + required-native 测试表达”的评估，测试尚未编写。
2. 不声称 fake engine、fake diagnostics、test-side 状态推导或 native 回放
   可以替代 checker。
3. 不声称 optional skip 是证据；不声称 default `cargo test` 的 ignored 是
   pass。
4. 不声称 43 个 matrix case 都能由一份 test 覆盖；严格只有 23 个 in-place
   + 4 个顶层可 exact/条件 exact。
5. 不声称 P7-1 可以由本切片归零；阶段 5 仍有 190 planned。
6. 不声称 TS 6.0.3 与 7.1.0-beta 行为完全一致；版本差是最大未测量风险。
7. 不声称 `documentationSource`/`definition.kind` 已有 native DTO；它们是
   test-side 语义分类，需 owner 接受。
8. 不声称 `perturbsUserTypes=false` 是强证据；`this.` 为空的 scenario 和
   空指纹必须用反向自测补强。
9. 不声称本轮修改了 ledger、contract、validator 或生产代码；本文件只是
   设计。

---

## 10. 附录

### 10.1 in-place case id 映射

case id 的机械规则：
`TS-MATRIX-0102-<SCENARIO_UPPER_UNDERSCORE>-IN_PLACE_SHADOW`。
33 个 scenario 与 §2.2 的 33 行一一对应；顶层 10 个的 case id 见 §3.3。

### 10.2 预期诊断清单（17 条）

| scenario | code | WXML range | target |
| --- | ---: | --- | --- |
| `page-basic` | 2339 | 134-138 | `nmae` |
| `page-export-empty` | 2339 | 134-138 | `nmae` |
| `page-cjs` | 2339 | 134-138 | `nmae` |
| `page-esm` | 2339 | 134-138 | `nmae` |
| `component-properties` | 2339 | 130-138 | `subtitle` |
| `component-module` | 2339 | 130-138 | `subtitle` |
| `component-js` | 2339 | 130-138 | `subtitle` |
| `js-jsdoc-pragma` | 2339 | 140-144 | `nmae` |
| `js-checkjs` | 2339 | 140-144 | `nmae` |
| `js-shebang-bom` | 2339 | 91-98 | `missing` |
| `nullable` | 18047 | 45-54 | `maybeUser` |
| `nullable` | 18046 | 98-106 | `external` |
| `crlf-script` | 2339 | 95-99 | `nmae` |
| `js-source-check-inherit` | 2339 | 91-98 | `missing` |
| `unchecked-index` | 2532 | 44-52 | `users[0]` |
| `js-bom-check-inherit` | 2339 | 91-98 | `missing` |
| `unicode-crlf` | 2339 | 103-107 | `nmae` |

### 10.3 required native job 命令

```bash
WXML_TEST_NATIVE=1 \
WXML_REQUIRE_REAL_ENGINE=1 \
TSGO_BIN=/abs/extracted-package/lib/tsc \
TSGO_ASSET_ROOT=/abs/extracted-package \
TSGO_TYPINGS_ROOT=/abs/dir-containing-miniprogram-api-typings \
WXML_TEST_TMPDIR=/tmp/opencode \
cargo test -p wxml-lsp --test native_parity real_matrix -- --ignored --nocapture
```

默认运行（只证明 manifest 来源，不证明引擎）：

```bash
cargo test -p wxml-lsp --test native_parity real_matrix_manifest
cargo test -p wxml-lsp --test native_parity real_matrix_comparator
```

### 10.4 只读复核记录

本文件写入前执行的只读检查（未运行 cargo test）：

```text
git log --oneline -5
git log -1 --format='%H %ad %s' --date=iso -- docs/evidence/data/type-bridge-c1/matrix.json
sha256sum docs/evidence/data/type-bridge-c1/matrix.json
stat -c '%s' docs/evidence/data/type-bridge-c1/matrix.json
node --experimental-strip-types --input-type=module <读取 scenarios.ts 的统计脚本>
python3 <读取 tsgo-test-ledger.json 的统计脚本>
grep -n "EXPECTED_STATES|EXPECTED_INTRODUCED|EXPECTED_PERTURBATION" backends/typescript/test/matrix.test.ts
grep -n "build_in_place_shadow|map_span_to_source|to_original_offset" crates/wxml-backend-tsgo/src
grep -n "GREEN_CASES|PLANNED_CASES|RETIRED_CASES|BLOCKED_CASES" crates/wxml-lsp/tests/native_parity/ledger.rs
```

未执行 `cargo test`、`cargo clippy`、`cargo fmt`；未修改
`docs/plans/tsgo-test-ledger.json`、`crates/wxml-lsp/tests/native_parity/ledger.rs`
或任何测试/生产文件。
