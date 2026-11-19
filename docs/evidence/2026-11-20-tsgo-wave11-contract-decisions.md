# tsgo wave-11 D 类契约裁决决策矩阵（2026-11-20）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务基线：HEAD `5cc1255`（`docs(evidence): record P7 deletion dry run`）。
  写入期间并行提交 `43625ee`（`test(p7): enforce retarget gate retention`）与
  `51abc13`（`test(tsgo): close conditional parity and ledger backfill`）落地。
  本文件按**写入完成时实际 JSON** 记录：`70 green / 233 planned / 36 retired`；
  D 集合本身未变，仍全部 `planned`、无证据。若回填继续移动，以 JSON 重新读取为准。
- 记录日期：2026-11-20
- 范围：**只新增本文件**。未修改生产代码、测试源码、`Cargo`、delivery、
  台账 JSON（`docs/plans/tsgo-test-ledger.json`）、台账 Markdown、contract、
  ADR、README、CI 或脚本；未 commit，未启动 review；**未运行任何测试**，只做只读
  读取、`jq`/`grep`/`sed`。
- 写入完成时工作树只有本文件未跟踪；并行内容已由 `43625ee`/`51abc13` 提交
  （ledger、tests、contract、ledger Markdown）。它们与本文件的 41 个 case
  无直接关系，本文件不修改、不评估。
- 本文件不把 `gate_green`、opt-in native gate、`near_exact`/`weaker` 推荐、
  并行 wave-10 回填结果当作 case `green`。

**结论先行：**

1. wave-9 计划 §2.4 的 D 类 **41 个 case 仍然全部是 `planned`、无
   `migration_evidence`、`target_rust_test_status=proposed`、`requires_decision=false`**。
   以写入完成时的实际 JSON 为准，当前是 **70 `green` / 233 `planned` / 36
   `retired-with-decision`**（wave-9 文档里的 58/245 已被 wave-10 与 wave-10b
   推进；D 集合本身未变）。
2. 41 个 D case 的处置分解：
   - **2 个可以 exact（无需新 native API，无需裁决）**：
     `TS-MATRIX-0102-INTERFACE_ROOT-IN_PLACE_SHADOW`、
     `TS-MATRIX-0102-DATA_SHORTHAND-IN_PLACE_SHADOW`。
     这两个 scenario 的 oracle 在 `hover=unsupported` 时提前返回，只断言三能力状态、
     reason 非空、hovers/diagnostics 为空；native 的 `CapabilityStatus::Unsupported`
     与空结果已经能逐条对应。
   - **2 个条件 exact（需评审接受 `contract_translated`）**：
     `TS-COMPILER-0063`（nested options → `options_digest` / 环境重开）、
     `TS-MATRIX-0092`（"两 candidate 不抛" → 只保留 in-place 的 "33 个 scenario 构建
     不 panic"）。
   - **6 个可以在不新增 native API 的情况下，经书面裁决 + ledger/validator 扩展后
     `retired-with-decision`**：`TS-COMPILER-0050`、`TS-COMPILER-0086`、
     `TS-PROTOCOL-0021`、`TS-PROTOCOL-0395`、`TS-SERVER-0094`、`TS-SERVER-0109`。
   - **31 个必须保持 blocked（禁止伪造）**：41 减去 2 个无条件 exact、2 个条件
     exact、6 个 retire 之后的 31 个 `TS-MATRIX-0102-*-IN_PLACE_SHADOW`。其中 30 个
     断言 `diagnosticsExact`、`perturbsUserTypes`、`userScriptDiagnosticsIntroduced`
     至少一项，这些值只能由真实 TS checker 在**默认运行**中产生；另外还需要：
     5 个发布 `Disabled` 状态（policy off/`@ts-nocheck`）、1 个 probe health gate
     （`NO_IMPLICIT_THIS_OFF`）、1 个 checker-based data-root 分类
     （`INTERFACE_ROOT_IMPORT`）、3 个 `documentationSource`、5 个 `definition.kind`。
3. **不新增 native API 时，D 类最多消减 10 个 planned**：
   乐观 `74 green / 223 planned / 42 retired`；保守（只做 2 个无条件 exact + 6 个
   retire）`72 / 225 / 42`。
4. **P7-1 归零不能由 D 类完成。** 即使 D 类全部按乐观方案处置，planned 仍有 223：
   127 个是已评估的 `near_exact`/`weaker`/`structural`/`blocked`，96 个是无证据
   planned（106 − 10）。D 类对 233 → 0 的贡献上限是 10（约 4.3%）。
5. 31 个 matrix case 的"retire"不是机制替换，而是**删除 scenario 级语义覆盖**。
   若 owner 选择这条路，必须写成明确的 coverage waiver，并同步 G5 与 ledger
   validator；不能把它描述成"已迁移/已等价"。

---

## 1. 基线与 D 集合

### 1.1 台账实际值（写入完成时）

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
| 有证据 planned | 127 = 2 + 96 + 7 + 22 |

`docs/plans/tsgo-test-ledger.json` 的 `counts.statuses` 与
`counts.migration_evidence` 一致；` crates/wxml-lsp/tests/native_parity/ledger.rs:26`
的 `GREEN_CASES=70`、`:28` 的 `PLANNED_CASES=233`、`:30` 的 `RETIRED_CASES=36`、
`:38` 的 `WEAKER_CASES=96` 与 JSON 一致。wave-9 文档的 58/245 是 `6727059` 的快照，
已被 wave-10/10b 推进；**D 集合未变**。
并行说明：任务基线 `5cc1255` 读取时是 68/235/36；写入期间 `43625ee` 与 `51abc13`
（wave-10b 回填）把 `TS-ANALYSIS-0184`、`TS-REWRITE-0197` 升为 green、
`TS-ANALYSIS-0161` 登记 weaker，计数变为 70/233/36。这三个 case 都不在 D 集合内。

### 1.2 D 集合的组成（41）

| 组 | 数量 | case |
| --- | ---: | --- |
| matrix in-place | 33 | `TS-MATRIX-0102-<SCENARIO>-IN_PLACE_SHADOW` |
| matrix 顶层 | 1 | `TS-MATRIX-0092` |
| compiler 池 | 3 | `TS-COMPILER-0050`、`TS-COMPILER-0063`、`TS-COMPILER-0086` |
| C1 request/response | 2 | `TS-PROTOCOL-0021`、`TS-PROTOCOL-0395` |
| C1 server | 2 | `TS-SERVER-0094`、`TS-SERVER-0109` |

校验：`jq` 过滤 33 个 `*-IN_PLACE_SHADOW` + 上述 8 个，合计 41；41 个全部
`status=planned`、`migration_evidence=null`、`target_rust_test_status=proposed`。
同 33 个 scenario 的 `*-SYMBOL_BRIDGE` 已经全部 `retired-with-decision`
（Candidate B），不属于本文件的 D 集合。

`comparison_type` 分布：`semantic_exact` 34（33 matrix in-place + `TS-MATRIX-0092`）、
`contract_translated` 5（3 compiler + 2 protocol）、`mechanism_replaced` 2（2 server）。

### 1.3 为什么"retire"不是免费的：ledger 的四条硬约束

这些约束决定了本文件的建议必须同时给出 validator 改动，不能只改 JSON。

1. **状态词表**：` crates/wxml-lsp/tests/native_parity/ledger.rs:88` 的
   `ALLOWED_STATUSES = ["planned","green","retired-with-decision"]`，没有 `waived`
   桶。任何 waiver 只能表达成 `retired-with-decision`（或新增状态，改动更大）。
2. **retire 只允许 Candidate B**：`:984` 对 `disposition == "obsolete"` 硬断言
   `reason.contains("Candidate B")` 且 `requires_decision == true`。
   非 Candidate-B 的 case 即使机制被替换也不能 retire。
3. **计数耦合**：`:1023` `obsolete == RETIRED_CASES(36)`；`:1022`
   `non_obsolete == PLANNED_CASES + GREEN_CASES`；`:1025` `decision_ids.len() == 37`；
   `:1027` `non_obsolete_decision_ids == ["TS-META-0027"]`。新增任何 retire 或
   `requires_decision` 都会同时打破这四条。
4. **G5 措辞**：` docs/reference/tsgo-test-contract.md:150` 要求"G5 通过需要全部
   `direct/rewrite/split` 用例为 `green/reviewed`"。如果 31 个 matrix case 以
   `retired` 结束，G5 必须改写，否则账面上自相矛盾。

另外：`retired` 的 `disposition` 目前只能是 `obsolete`；`rewrite` 分支只接受
`green` 或 `planned`（`:996`）。所以 6 个 `rewrite` case 若要 retire，validator
需要允许 `rewrite + retired-with-decision + 决策引用`，或者把它们改成 `obsolete`
并接受 `counts.dispositions` 的变化。前者更小、也更符合 "mechanism_replaced" 语义。

---

## 2. Matrix harness 的 oracle 分解

### 2.1 33 个 matrix case 共用的 24 条断言

oracle 在 ` backends/typescript/test/matrix.test.ts:102`，静态 24 条、语义 23 条。
按"能否在 native 默认运行中表达"分组：

| 断言族 | 具体内容 | 来源字段 | native 现状 | 结论 |
| --- | --- | --- | --- | --- |
| 能力状态 | 三能力 `status` 等于 `EXPECTED_STATES`；非 supported 时 `reason` 非空；`EXPECTED_REASON` 的 root cause 正则 | C1 `run.states` | C0 `CapabilityStatus` 有同样的 6 态；adapter 逐能力返回 | 测试侧可聚合；不是硬阻塞 |
| unsupported 短路 | hover unsupported 时断言 `hovers.length==0`、`diagnostics.length==0` 后 return | `run.hovers`/`run.diagnostics` | `unsupported_hover` 返回空、`unsupported_diagnostics` 只带 WXML 语法项 | **可 exact**（见 §3.1） |
| hover 值 | `observation.typeOnly`、`documentation`、`documentationSource` | C1 `fragments[].hover` | `query/hover.rs` 有 type/doc；**没有 documentationSource** | 需真实 checker + 新字段 |
| 诊断精确性 | `diagnosticsExact === true`（in-place 全部；`EXPECTED_INEXACT` 只含 symbol-bridge） | C1 运行指标 | 无同名字段；native 只能给诊断列表 | 需 checker 对照 |
| 诊断内容 | `missingDiagnostics[].expr` / `unexpectedDiagnostics[].range` | C1 运行指标 | 无 | 需 checker 对照 |
| definition | `definition.kind`（script/external/generated）、`range` 可映射、目标文本非空 | C1 `fragments[].definition` | range 有；**没有 kind 分类** | 需新字段 + checker |
| 非干扰 | `run.perturbsUserTypes === false` | C1 运行指标 | 无；native 没有"用户脚本类型前后对比"路径 | 需 checker/新测量 |
| 引入诊断 | `userScriptDiagnosticsIntroduced[].code`（in-place 仅 `js-jsdoc-pragma` 期望 `[2322]`，其余 `[]`） | C1 运行指标 | 无；script-owned diagnostics 仍被丢弃（F1 缺口 §5） | 需 checker + 诊断归属 |
| 请求信封 | `request.ownerKind`、`fileCount>=1`、`fragmentCount==表达式数`、`registration.count>=1` | C1 `request` | `PreparedView`/`BuiltShadow.registration.count` 有等价事实 | 测试侧可断言 |
| 注册计数 | `run.registration.count >= 1` | C1 `registration` | ` crates/wxml-backend-tsgo/src/shadow/layout.rs:277` `RegistrationInfo { kind, count }` | 可断言 |

**关键事实：** 只有 `run.states.hover.status === "unsupported"` 时，
` backends/typescript/test/matrix.test.ts:123` 会提前 return，跳过
`diagnosticsExact`/`perturbsUserTypes`/`userScriptDiagnosticsIntroduced`/request
信封。33 个 in-place scenario 中只有 3 个走这条路径：
`interface-root`、`interface-root-import`、`data-shorthand`
（`EXPECTED_STATES` 定义为 `ALL("unsupported")`）。

### 2.2 三个短路 scenario 与 native 的实际差异

| scenario | 脚本 | oracle 期望 | native 实际 | 能否 exact |
| --- | --- | --- | --- | --- |
| `interface-root` | 同文件 `interface User` + `Page({data: User})` | 三能力 `unsupported`、空 hovers/diagnostics | `classify_data_root` 命中同文件类型（` layout.rs:1887`）→ `DataRootKind::TypeReference` → `ShadowError::Unsupported`（` layout.rs:526`）→ adapter 三能力 `Unsupported` | **是** |
| `data-shorthand` | `declare const data` + `Page({data})` | 三能力 `unsupported`；root cause 正则 `property assignment` | `data` 成员是 shorthand → `DataRootKind::Unsupported`（` layout.rs:533`）→ reason 含 "not a property assignment" | **是** |
| `interface-root-import` | `import type { User }` + `Page({data: User})` | 三能力 `unsupported` | `is_same_file_type` 只查同文件（` layout.rs:1896`）→ 命中不了 import → `DataRootKind::Expression` → `Partial`，不是 `Unsupported` | 否：需要 checker 分类 |

`ShadowError::Unsupported` 在 adapter 里被转成
`PlanError::Unsupported("the template owner is not statically analyzable (data.root): …")`
（` crates/wxml-lsp/src/backend_tsgo/input.rs:1392`），hover/diagnostics/definition
分别返回 `unsupported_hover`/`unsupported_diagnostics`/`unsupported_definition`
（` crates/wxml-lsp/src/backend_tsgo/mod.rs:1228`、`:1241`、`:1251`）。
WXML 本身合法，`parse_syntax_diagnostics` 为 0，因此三条"空结果"断言也成立。

### 2.3 非 matrix 的 8 个 case

| case | oracle 摘要 | 依赖的旧机制 | native 现状 |
| --- | --- | --- | --- |
| `TS-COMPILER-0050` | 2 条 `notEqual`：文件集变、编译器选项变 → 不复用旧 project | Node `acquireProject` 的 shape key + LanguageService 对象身份 | 每个 `update_project` 建新 snapshot 并释放旧的；没有"同 shape 复用"的对象 |
| `TS-COMPILER-0063` | 1 条 `notEqual` + 2 条 `deepEqual(paths)`：nested options 参与 key | 同上 | `CompilerOptions` 接受 `paths` 并纳入 `options_digest`；`TypeEnvironmentIdentity` 用它判定必须重开 |
| `TS-COMPILER-0086` | `projectPoolSize() == MAX_POOLED_PROJECTS`（=2） | Node project pool 的 LRU 上限 | `MAX_LIVE_SNAPSHOTS=8`（` config.rs:102`）+ ledger 淘汰；资源与数值都不同 |
| `TS-PROTOCOL-0021` | 21 条：`protocolVersion`、owner/viewId、policy、typings、projectRoot、compilerOptions、files/fragments/limits | C1 `AnalyzeRequest` 组装 | `AnalysisInput`/`PreparedView` 有 files/fragments/owner/policy；没有 `protocolVersion`/`viewId`/`propertiesOverlay` |
| `TS-PROTOCOL-0395` | 15 条：`protocolVersion`、`requestId`、`states`、`moduleKind`、insertions/mounts 长度、typo range、generated definition | C1 `AnalyzeResponse` 组装 | 无同响应；insertions/mounts/诊断/定义在内部 DTO 有，但不组成 C1 信封 |
| `TS-SERVER-0094` | `ping` 返回 `{ok,protocolVersion:2,backendVersion}`；`shutdown` 返回 `{ok:true}` 后进程 exit 0 | C1 NDJSON server | native 是 LSP；生命周期是 `initialize`/`shutdown`/`exit`，没有 `ping` 和方法级 result 形状 |
| `TS-SERVER-0109` | `analyze` 返回 C1 response；title hover `string`、typo range `134..138` | C1 `analyze` | 无 `analyze`；语义值分别由 0184/0197 等测试覆盖 |
| `TS-MATRIX-0092` | 1 条 `doesNotThrow`：33 scenario × 2 layout | `runScenario` 全链路（含 checker） | 可对 33 个脚本逐个 `build_in_place_shadow`；无法在没有引擎时跑 checker |

---

## 3. D 类逐项决策矩阵

### 3.1 33 个 matrix in-place case

状态简写 `S/P/U/D/N` = `supported/partial/unsupported/disabled/unavailable`，
顺序为 `hover/diagnostics/definition`。阻塞码：

- `CHK` = `diagnosticsExact` 需要真实 checker 的诊断对照（expected vs actual）。
- `TYP` = hover 的 `typeOnly`/`documentation` 值需要真实 checker。
- `PERT` = `perturbsUserTypes` 需要 checker + 用户脚本类型前后对比。
- `INTRO` = `userScriptDiagnosticsIntroduced` 需要 checker + script 诊断归属。
- `DOCS` = `documentationSource`（`quickinfo`/`copied`）需要新 DTO 字段。
- `DEFK` = `definition.kind`（`script`/`external`/`generated`）需要新 DTO 字段。
- `PROBE` = probe health gate（`probe root` → `unavailable`）native 不存在。
- `ROOT` = checker-based data-root 分类。
- `STATE` = policy off / `@ts-nocheck` 的 `diagnostics: disabled`；native 目前只靠
  派生视图的 `@ts-nocheck` 得到空诊断 + `Supported`，adapter 不发布 `Disabled`。
- `ENV` 不列为阻塞：`ownerKind`/`fileCount`/`fragmentCount`/`registration.count`
  在 `PreparedView`/`BuiltShadow` 有等价事实，测试侧可断言。

| # | scenario（case 后缀） | 期望 S/P/U/D/N | 期望诊断 | native 现状 | 阻塞 | 处置 |
| ---: | --- | --- | --- | --- | --- | --- |
| 1 | `PAGE_BASIC` | S/S/S | `nmae` | shadow/mapping/hover/definition 管线齐；无 docSource/defKind；引擎 opt-in | CHK,TYP,PERT,INTRO,DOCS,DEFK | blocked |
| 2 | `PAGE_EXPORT_EMPTY` | S/S/S | `nmae` | 同上（module 形态由脚本决定） | CHK,TYP,PERT,INTRO | blocked |
| 3 | `PAGE_CJS` | S/S/S | `nmae` | 同上 | CHK,TYP,PERT,INTRO | blocked |
| 4 | `PAGE_ESM` | S/S/S | `nmae` | 同上 + 相对 import（host 提供） | CHK,TYP,PERT,INTRO | blocked |
| 5 | `PAGE_CLOSURE` | S/S/S | — | shadow 是否构建不是本 case 的阻塞点；hover 值需 checker | CHK,TYP,PERT,INTRO | blocked |
| 6 | `COMPONENT_PROPERTIES` | S/S/S | `subtitle` | properties overlay 有；docSource=`copied`、definition=`generated` 缺 | CHK,TYP,PERT,INTRO,DOCS,DEFK | blocked |
| 7 | `COMPONENT_MODULE` | S/S/S | `subtitle` | 同上，无 copied/generated 标记 | CHK,TYP,PERT,INTRO | blocked |
| 8 | `COMPONENT_JS` | S/S/S | `subtitle` | shadow/overlay 路径存在；docSource=`copied` 缺 | CHK,TYP,PERT,INTRO,DOCS | blocked |
| 9 | `PROPERTIES_SPREAD` | P/P/P | — | `BuildStatus::Partial` + reason 含 "spread assignment" 已有 | CHK,PERT,INTRO | blocked |
| 10 | `MULTI_REGISTRATION` | P/P/P | — | `registration.count=2` + "2 page registrations" reason 已有 | CHK,PERT,INTRO | blocked |
| 11 | `JS_JSDOC_PRAGMA` | S/P/S | `nmae` | pragma 注入有；script 诊断被排除时会降 `Partial`；`introduced=[2322]` 无字段 | CHK,TYP,PERT,INTRO | blocked |
| 12 | `JS_JSDOC_OFF` | S/D/S | — | `js_check_applied=Off` 有，但 adapter 不发布 `Disabled` | STATE,CHK,TYP,PERT,INTRO | blocked |
| 13 | `JS_CHECKJS` | S/S/S | `nmae` | checkJs 透传有 | CHK,TYP,PERT,INTRO | blocked |
| 14 | `TS_NOCHECK` | S/D/S | — | `@ts-nocheck` 注入有，但状态不是 `Disabled` | STATE,CHK,TYP,PERT,INTRO | blocked |
| 15 | `JS_NOCHECK` | S/D/S | — | 同上 | STATE,CHK,TYP,PERT,INTRO | blocked |
| 16 | `JS_SHEBANG_BOM` | S/S/S | `title.missing` | BOM/shebang/pragma insertion 有 | CHK,TYP,PERT,INTRO | blocked |
| 17 | `INTERFACE_ROOT` | U/U/U | — | `TypeReference` → `Unsupported`，三能力空结果 | 无 | **exact** |
| 18 | `INTERFACE_ROOT_IMPORT` | U/U/U | — | 只查同文件类型 → `Expression` → `Partial` | ROOT | blocked |
| 19 | `NULLABLE` | S/S/S | `maybeUser`,`external` | shadow/映射有；诊断在 receiver 上需 checker | CHK,TYP,PERT,INTRO | blocked |
| 20 | `CROSS_FILE_OVERLAY` | S/S/S | — | 跨文件由 host 提供；definition=`external` 缺分类 | CHK,TYP,PERT,INTRO,DEFK | blocked |
| 21 | `ALIAS_PROJECT` | S/S/S | — | tsconfig extends/paths 属 host；definition=`external` 缺分类 | CHK,TYP,PERT,INTRO,DEFK | blocked |
| 22 | `COMPONENT_DATA_CROSS_FILE` | S/S/S | — | 同上 | CHK,TYP,PERT,INTRO,DEFK | blocked |
| 23 | `CRLF_SCRIPT` | S/S/S | `nmae` | CRLF 偏移由 Rust 文本层保证 | CHK,TYP,PERT,INTRO | blocked |
| 24 | `DATA_SHORTHAND` | U/U/U | — | shorthand → `Unsupported`，reason 含 "property assignment" | 无 | **exact** |
| 25 | `JS_SOURCE_CHECK_OFF` | S/D/S | — | 显式 off 会 neutralizing 源 pragma，但状态不是 `Disabled` | STATE,CHK,TYP,PERT,INTRO | blocked |
| 26 | `JS_SOURCE_CHECK_INHERIT` | S/S/S | `title.missing` | 同上 | CHK,TYP,PERT,INTRO | blocked |
| 27 | `METHODS_NONLITERAL` | P/P/P | — | `methods` 非字面量 → Partial + "object literal" reason | CHK,PERT,INTRO | blocked |
| 28 | `UNCHECKED_INDEX` | S/S/S | `users[0]` | `noUncheckedIndexedAccess` 透传有；2532 需 checker | CHK,TYP,PERT,INTRO | blocked |
| 29 | `NULLABLE_NO_STRICT_NULL` | S/S/S | — | `strictNullChecks:false` 透传有；无假阳性需 checker | CHK,TYP,PERT,INTRO | blocked |
| 30 | `NO_IMPLICIT_THIS_OFF` | N/N/N | — | 无 probe health gate；native 目前会 `Supported` | PROBE,CHK,PERT,INTRO | blocked |
| 31 | `JS_BOM_CHECK_OFF` | S/D/S | — | BOM+shebang+off 注入有，但状态不是 `Disabled` | STATE,CHK,TYP,PERT,INTRO | blocked |
| 32 | `JS_BOM_CHECK_INHERIT` | S/S/S | `title.missing` | 同上 | CHK,TYP,PERT,INTRO | blocked |
| 33 | `UNICODE_CRLF` | S/S/S | `nmae` | CJK/emoji/CRLF 映射有；hover/诊断需 checker | CHK,TYP,PERT,INTRO | blocked |

补充：

- `#9/#10/#27` 的 hover 期望是 `partial`，harness 因
  `hover.status !== "supported"` 跳过所有 query 循环，所以不出现 `TYP`；但
  `diagnosticsExact`/`perturbsUserTypes`/`userScriptDiagnosticsIntroduced` 仍执行。
- `#11` 的 `EXPECTED_INTRODUCED` 是 in-place 唯一非空值 `[2322]`。
- `#30` 的 hover 是 `unavailable`（不是 `unsupported`），所以**不会**提前 return；
  `diagnosticsExact` 等仍执行，同时 states 需要 probe gate。
- `#18` 的 oracle 走提前 return，因此只需要 states/reason/空结果；native 的差异只在
  `Partial` vs `Unsupported`。

### 3.2 非 matrix 的 8 个 case

| case | oracle 断言 | 旧 Node/TS 行为 | native 当前能力 | 处置 | 依据 |
| --- | --- | --- | --- | --- | --- |
| `TS-COMPILER-0050` | 2×`notEqual(project)`：文件集变、选项变不复用 | `acquireProject` 的 `(rootDir, options, file set)` key + LanguageService 身份 | 每次 `update_project` 建新 snapshot、释放旧；无 pool/对象身份 | **retire（裁决）** | 机制被 ADR-0008 替换；"不复用"由 snapshot/epoch 失效保证 |
| `TS-COMPILER-0063` | 1×`notEqual` + 2×`deepEqual(paths)` | nested options 参与 pool key | `CompilerOptions` 保留 `paths`；`options_digest` 与 `TypeEnvironmentIdentity` 决定必须重开 | **条件 exact** | 需评审接受"对象身份 → digest/重开身份"的翻译 |
| `TS-COMPILER-0086` | `projectPoolSize()==MAX_POOLED_PROJECTS(2)` | Node LRU pool 上限 2 | `MAX_LIVE_SNAPSHOTS=8` + ledger 淘汰（不同资源、不同数值） | **retire（裁决）** | 不能用 8 个 snapshot 冒充 2 个 project；禁止替换 |
| `TS-PROTOCOL-0021` | 21 条 request 字段 | C1 `AnalyzeRequest` | `AnalysisInput`/`PreparedView` 有 files/fragments/owner/policy；无 `protocolVersion`/`viewId`/`propertiesOverlay` | **retire（裁决）** | C1 信封随 Node 桥删除；语义半边由 input 测试覆盖 |
| `TS-PROTOCOL-0395` | 15 条 response 组装 | C1 `AnalyzeResponse` | 无同响应；insertions/mounts/range/definition 在内部 DTO | **retire（裁决）** | 语义值已由 0184/0197 + mapping 测试覆盖 |
| `TS-SERVER-0094` | `ping` 结果 + `shutdown` + exit 0 | C1 NDJSON server | LSP `initialize`/`shutdown`；无 `ping` | **retire（裁决）** | 方法契约不存在；LSP 生命周期测试保留 |
| `TS-SERVER-0109` | `analyze` 返回 C1 response + title/typo 值 | C1 `analyze` | 无 `analyze`；语义值由 0184/0197 覆盖 | **retire（裁决）** | 同上 |
| `TS-MATRIX-0092` | 1×`doesNotThrow`（33×2） | `runScenario` 全链路（含 checker） | 可对 33 脚本跑 `build_in_place_shadow`；无默认引擎 | **条件 exact** | 需把断言 re-scope 为 in-place build（symbol-bridge 已 retired） |

### 3.3 阻塞与处置汇总

| 处置 | 数量 | case |
| --- | ---: | --- |
| exact（无条件，测试即可） | 2 | `INTERFACE_ROOT`、`DATA_SHORTHAND` |
| 条件 exact（需评审接受翻译） | 2 | `COMPILER-0063`、`MATRIX-0092` |
| retire（需书面裁决 + validator 扩展） | 6 | `COMPILER-0050`、`COMPILER-0086`、`PROTOCOL-0021`、`PROTOCOL-0395`、`SERVER-0094`、`SERVER-0109` |
| blocked（禁止伪造） | 31 | 其余 31 个 `TS-MATRIX-0102-*-IN_PLACE_SHADOW` |
| **合计** | **41** | |

硬阻塞分布（31 个 blocked）：

| 阻塞 | 数量 | 说明 |
| --- | ---: | --- |
| `CHK` + `PERT` + `INTRO` | 30 | 所有非提前 return 的 matrix case；`diagnosticsExact`/`perturbsUserTypes`/`userScriptDiagnosticsIntroduced` 都只能由真实 checker 产生 |
| 其中额外 `TYP` | 26 | hover 值断言（`#9/#10/#27` 因 partial 跳过 query；`#30` 因 unavailable 跳过 query） |
| 其中额外 `DOCS` | 3 | `PAGE_BASIC`、`COMPONENT_PROPERTIES`、`COMPONENT_JS` |
| 其中额外 `DEFK` | 5 | `PAGE_BASIC`、`COMPONENT_PROPERTIES`、`CROSS_FILE_OVERLAY`、`ALIAS_PROJECT`、`COMPONENT_DATA_CROSS_FILE` |
| 其中额外 `STATE` | 5 | `JS_JSDOC_OFF`、`TS_NOCHECK`、`JS_NOCHECK`、`JS_SOURCE_CHECK_OFF`、`JS_BOM_CHECK_OFF` |
| 其中额外 `PROBE` | 1 | `NO_IMPLICIT_THIS_OFF` |
| `ROOT` | 1 | `INTERFACE_ROOT_IMPORT` |

> `blocked` 不等于"永远做不了"。它表示**当前默认运行套件里没有真实 checker 证据**，
> 且 `diagnosticsExact`/`perturbsUserTypes`/`documentationSource`/`definition.kind`
> 等在 native 侧还没有等价字段。把这些 case 记成 `green` 或 `weaker` 都不能降低
> planned；只有新 native 能力或书面裁决可以。
>
> 其中 `STATE` 的 5 个是**小的生产改动**（adapter 把 policy off / `@ts-nocheck`
> 映射为 `Disabled`），不是新查询 API；但它仍需要测试与评审，不能靠改 ledger 完成。
> `PROBE` 需要新增 probe health 判定；`DOCS`/`DEFK` 需要 C0 DTO 字段；其余需要
> 默认运行的 checker + 诊断归属。

---

## 4. 不新增 native API 时的可 retire 清单

### 4.1 严格可 retire（6）

这 6 个的共同点：oracle 断言的是**旧机制本身**（C1 信封、NDJSON server、Node
project pool），native 没有、也不需要同样的机制；断言的语义价值要么由 native
现有测试覆盖，要么本来就不是产品可观察行为。

| case | retire 理由 | 替代证据（不新增 case） | 保留的语义 |
| --- | --- | --- | --- |
| `TS-COMPILER-0050` | Node LanguageService pool 被"一 project 一 snapshot"替换 | ` crates/wxml-backend-tsgo/src/native/session.rs:3081` `type_environment_transitions_require_reopen`；` crates/wxml-backend-tsgo/src/input.rs:2075` options 校验 | "不同输入不共享旧 program"由 snapshot/epoch 失效保证 |
| `TS-COMPILER-0086` | `MAX_POOLED_PROJECTS=2` 是 Node 实现细节 | ` crates/wxml-backend-tsgo/src/native/session.rs:3222` `snapshot_ledger_tracks_only_confirmed_releases_and_enforces_the_cap` | "保留资源有界"由 `MAX_LIVE_SNAPSHOTS` 保证；**数值/资源不同，禁止声称等价** |
| `TS-PROTOCOL-0021` | C1 `AnalyzeRequest` 随 Node 桥删除 | ` crates/wxml-lsp/src/backend_tsgo/input.rs` 的 `fragment_spec`/`build_request_plan` 测试；` crates/wxml-lsp/src/backend_tsgo/tests.rs` 的 opened-inputs 测试 | files/fragments/owner/policy 仍随请求进入 native |
| `TS-PROTOCOL-0395` | C1 `AnalyzeResponse` 不再存在 | ` crates/wxml-lsp/src/backend_tsgo/tests.rs:4460` `case_analysis_0184_page_basic_typo_maps_to_wxml_134_138`；` crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:465` `case_rewrite_0197_...` | typo `134..138`、generated definition 的语义由 shadow/mapping 测试覆盖 |
| `TS-SERVER-0094` | native 是 LSP，没有 `ping`；`shutdown` 形状不同 | ` crates/wxml-lsp/src/server.rs:1554` `shutdown_before_initialize_is_invalid_request`；` crates/wxml-lsp/src/server.rs:2564` `case_server_0150_...` | JSON-RPC 生命周期仍由 LSP 测试覆盖 |
| `TS-SERVER-0109` | 没有 `analyze` 方法 | 同 0395 的 0184/0197；hover 测试 | title hover / typo range 已覆盖 |

retire 的前置条件（缺一不可）：

1. integrator/owner 书面记录：C1 桥与 Node pool 在 native 路径上被替换，这 6 个 case
   的断言不再作为迁移门禁。
2. ledger validator 允许"非 Candidate-B 的 mechanism_replaced retire"（见 §5.2）。
3. 每个 case 的 `disposition_reason`/`decision` 写明替代证据路径；`migration_evidence`
   保持 `null`，不得伪造 Rust 测试。
4. `docs/reference/type-bridge-c1.md` 的状态要说明"native 路径不实现 C1"；否则
   retire 与仍为 accepted 的 C1 契约冲突。

### 4.2 必须保持 blocked（31）

这 31 个不能通过"机制替换"裁决 retire，理由：

1. 它们的断言不是 C1 信封本身，而是**33 个真实小程序 scenario 的语义**：
   hover 类型/文档、诊断存在性与精确范围、definition 目标、非干扰、引入诊断。
   这些正是迁移要对齐的产品行为；native 目前没有默认运行的 checker 证据。
2. 把 31 个整体 retire 等价于宣布"scenario 级 parity 不在迁移范围内"。这是
   **coverage waiver**，不是 mechanism replacement。若 owner 选择它，必须：
   - 明确写出被放弃的覆盖面和替代测试（例如新的 native scenario 套件，但不是
     ledger case）；
   - 改写 G5（` docs/reference/tsgo-test-contract.md:150`）；
   - 在 ledger 里记录为 waiver，而不是"已迁移"。
3. 单独 retire 其中一部分（例如只 retire `no-implicit-this-off`）没有依据：31 个
   共享同一 harness，单独豁免会制造不一致。

**禁止伪造的具体项**（与本仓 wave-9 §6 一致，本轮复核仍成立）：

- `states` 聚合：可以由测试把三个 C0 结果拼起来断言；**不能**声称 native 有一个
  C1 `states` 响应。
- `diagnosticsExact`：没有真实 checker 就没有 expected/actual 对照；不能用
  "诊断列表为空"冒充 exact。
- `perturbsUserTypes`：不能用"in-place 不重写用户脚本"推断 false；必须实测
  `this.data` 类型是否变化。
- `userScriptDiagnosticsIntroduced`：不能把 shadow 诊断当 script 诊断。
- `documentationSource`/`definition.kind`：C0 DTO 没有字段，不能从 Markdown 文本或
  fileId 猜。
- `probeProblems`/`probe root`：native 没有 probe health gate，不能把
  `BuildStatus::Supported` 改写成 `Unavailable`。
- `MAX_LIVE_SNAPSHOTS` ≠ `MAX_POOLED_PROJECTS`。

---

## 5. 建议的 ledger / contract 改动

### 5.1 ledger JSON（只列会变的字段）

以乐观方案（2 exact + 2 条件 exact + 6 retire）为例：

| 字段 | 现值 | 建议值 | 说明 |
| --- | ---: | ---: | --- |
| `counts.statuses.green` | 70 | 74 | +2 无条件 +2 条件 |
| `counts.statuses.planned` | 233 | 223 | −10 |
| `counts.statuses.retired-with-decision` | 36 | 42 | +6 |
| `counts.migration_evidence.exact` | 70 | 74 | |
| `counts.migration_evidence.assessed_cases` | 197 | 201 | 4 个 exact 带入证据 |
| `counts.migration_evidence.remaining_planned_without_evidence` | 106 | 96 | |
| `near_exact`/`weaker`/`structural`/`blocked` | 2/96/7/22 | 不变 | D 类不改变这些桶 |
| `counts.integrator_decisions.accepted_obsolete` | 36 | 42 | 或新增 `accepted_mechanism_replaced=6` |
| `integrator_decision.remaining_blocked` | 0 | 0 | 不变 |

保守方案（只做 2 exact + 6 retire）：`72 / 225 / 42`，`assessed=199`，
`remaining_planned_without_evidence=98`。

逐 case 建议：

| case | `status` | `target_rust_test_status` | `migration_evidence` | 备注 |
| --- | --- | --- | --- | --- |
| `INTERFACE_ROOT`、`DATA_SHORTHAND` | `green` | `migrated_green` | `parity=exact`，`green_eligible=true`，1 条 adapter 测试 | 测试走真实 `prepare` → `build_in_place_shadow`，断言三能力 `Unsupported` + reason + 空结果 |
| `COMPILER-0063` | `green`（条件） | `migrated_green` | `parity=exact`，`comparison_type=contract_translated` | 断言不同 `paths` → 不同 digest + 必须重开；note 写清"对象身份被 digest/重开身份替代" |
| `MATRIX-0092` | `green`（条件） | `migrated_green` | `parity=exact`，`comparison_type=contract_translated` | 只保留 in-place；symbol-bridge 半边已 obsolete；note 写清 checker 半边不在本测试 |
| 6 个 retire | `retired-with-decision` | `not_migrated_internal_evidence` | `null` | 新增 `retirement_decision` 引用；**不要**复用 `requires_decision`（会破坏 `decision_ids.len()==37`）；不得写 `rust_tests` |
| 其余 31 | `planned` | `proposed` | `null` 或新的 `weaker` 证据 | 不改 status；若要登记 layout 半，必须 `parity=weaker` |

### 5.2 建议的 validator 改动（`crates/wxml-lsp/tests/native_parity/ledger.rs`）

最小、可审计的改法：

1. 保留 `obsolete` 专用于 Candidate B；**新增**允许
   `disposition in ["rewrite","split"] && status == "retired-with-decision"` 的分支。
2. 该分支要求 case 有 `retirement_decision`（建议对象：
   `{ "id": "...", "date": "...", "scope": "mechanism_replaced", "replacement": [...] }`），
   而不是复用 `requires_decision`。这样不破坏
   `decision_ids.len()==37` 和 `non_obsolete_decision_ids==["TS-META-0027"]`。
3. 把 `obsolete == RETIRED_CASES` 改为
   `obsolete + rewrite_retired == RETIRED_CASES`；`RETIRED_CASES` 36 → 42。
4. `non_obsolete == PLANNED_CASES + GREEN_CASES` 改为
   `non_obsolete == PLANNED_CASES + GREEN_CASES + REWRITE_RETIRED_CASES`。
5. `targets` 唯一性继续要求每个 case 的 `target_rust_test` 不同；retire 的 case 也要
   有稳定的 target 标签（现有 36 个 retired 都有）。
6. 如果未来加 `waived` 桶，必须同时改 `ALLOWED_STATUSES`、`counts.statuses`、
   contract §3 和 G5；不建议在本轮做。

替代方案（更小但语义更差）：把 6 个 case 的 `disposition` 改成 `obsolete`，
让现有 validator 通过。必须同时改 `counts.dispositions.rewrite`（52→46）和
`obsolete`（36→42），并在 `disposition_reason` 里写 "Candidate B"——这与事实不符。
**不推荐**。

> 并行产物 `docs/evidence/2026-11-20-tsgo-wave11-retire-audit.md`（E 类）提出了同源的
> `retirement_decision` 扩展。D/E 两类的 validator 改动必须合并成一次提交，不能各改
> 一套；否则 `RETIRED_CASES`、`obsolete` 计数和 `non_obsolete` 等式会互相冲突。

### 5.3 contract 改动

| 文件 | 位置 | 建议 |
| --- | --- | --- |
| `docs/reference/tsgo-test-contract.md` | §3 状态机 | 明确 `rewrite + retired-with-decision` 合法，条件是 `retirement_decision` 存在 |
| 同上 | §8 G5（`:148`） | 改为"全部 `direct/rewrite/split` 为 `green/reviewed`，或 `retired-with-decision` 且有 `retirement_decision`" |
| `docs/reference/type-bridge-c1.md` | 头部 Status | 增加一句"native 路径不实现 C1；C1 只保留为历史 Node 桥契约"（或标记 superseded for native） |
| `docs/plans/2026-11-17-tsgo-test-ledger.md` | §13 集成裁决 | 追加 2026-11-20/21 的 D 类裁决记录与 6 个 retire 清单 |
| `docs/adr/0008-rust-tsgo-no-node-backend.md` | Status/Consequences | 不要求本轮改；但 retire 记录应链接 ADR-0008，并注明它仍是 `proposed` |

注意：ADR-0008 仍是 `proposed`，而 C1 契约仍是 `accepted`。在 ADR 通过前 retire
C1 相关 case，是一个"方向已定、验收未完成"的决策；记录里必须写明这一点，不能
暗示 C1 已经正式废止。

### 5.4 计数投影

| 方案 | green | planned | retired | exact | assessed | 无证据 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 当前（写入完成时） | 70 | 233 | 36 | 70 | 197 | 106 |
| 只做 2 个无条件 exact | 72 | 231 | 36 | 72 | 199 | 104 |
| 2 exact + 6 retire（保守） | 72 | 225 | 42 | 72 | 199 | 98 |
| 4 exact + 6 retire（乐观） | 74 | 223 | 42 | 74 | 201 | 96 |
| 把 31 个 matrix 也 retire（coverage waiver） | 74 | 192 | 73 | 74 | 201 | 65 |

最后一行的 192 仍不是 0；P7-1 还需要处理 96 weaker + 2 near_exact + 7 structural +
22 blocked + 65 无证据。**任何 D 类方案都不能单独完成 P7-1。**

> 本表只含 D 类增量。并行的 E 类退休审计（
> ` docs/evidence/2026-11-20-tsgo-wave11-retire-audit.md`）另有自己的 retire 数；
> 两者合并时必须重新计算 `counts.statuses`、`RETIRED_CASES` 和
> `non_obsolete` 等式，不能把两张表的 retired 直接相加后就提交。

---

## 6. 风险

| 级别 | 风险 | 缓解 |
| --- | --- | --- |
| 高 | 把 31 个 matrix retire 当成"机制替换"，实际删掉 scenario 级 parity 覆盖 | 单独写成 coverage waiver；要求 owner 签字；不要与 6 个 C1 retire 混在一起 |
| 高 | 在 ADR-0008 仍 `proposed`、C1 仍 `accepted` 时 retire C1 case | 决策记录里显式写"方向已定、验收未完成"；链接 ADR-0008；不改 C1 文档状态为 superseded，除非 owner 明确要求 |
| 高 | validator 只改 JSON 不改常量/分支，`native_parity` 立刻红 | 按 §5.2 一次改完；回填前重新读 `ledger.rs` 常量与 JSON |
| 高 | 用 `MAX_LIVE_SNAPSHOTS=8` 冒充 `MAX_POOLED_PROJECTS=2` | 明确拒绝；`TS-COMPILER-0086` 只能 retire 或保持 blocked |
| 中 | 2 个 exact 测试用 fake 状态注入而不是真实 `prepare` 路径 | 测试必须走 `PreparedView` → `build_in_place_shadow`；断言 `PlanError::Unsupported` 的可见结果 |
| 中 | `TS-MATRIX-0092` 的 build-only 翻译被误读成"checker 也证明了" | evidence note 写清 checker 半边不在本测试；不要把它当 33 个 scenario 的语义证据 |
| 中 | `TS-COMPILER-0063` 的 digest 翻译被误读成"program 不会复用" | note 写清 digest/重开身份只证明 key 分离，不证明 program 内容；残余风险保留 |
| 中 | samples 在 P7-4 后被删除，matrix 测试读不到 | 测试用 inline `const` 或新的非 hash-pinned 测试数据目录；不要改 `tests/fixtures` 的 v1 hash |
| 中 | `native_parity/scenario_matrix.rs` 的 `MIGRATED_CASES` 为空，且有 `partition_skeletons_match_manifest_and_fail_closed` 断言 | 证据测试放 `backend_tsgo/tests.rs` 等既有文件；不要为 2 个 case 改 skeleton 清单 |
| 低 | 行号漂移 | 回填时用测试名/函数名定位，不复制本文件的 `path:line` |
| 低 | 并行写入 ledger/contract | 本文件写入期间已实测从 68/235/36 漂到 70/233/36（`43625ee` + `51abc13`）；回填前必须重新 `jq` 读 JSON 与 `ledger.rs` 常量；本文件不写 ledger |

---

## 7. 需要 owner / integrator 决策的点

1. **C1 是否算"已替换"？** 需要决定：在 ADR-0008 仍 proposed 时，是否允许把
   `PROTOCOL-0021/0395`、`SERVER-0094/0109` 记为 `retired-with-decision`。
   建议：允许，但记录必须写明 ADR 状态，且 C1 文档标记"native 不实现"。
2. **ledger 是否允许非 Candidate-B retire？** 需要批准 §5.2 的 validator 扩展
   （`rewrite + retired-with-decision + retirement_decision`）。
   建议：批准；这比把 `rewrite` 改成 `obsolete` 更小、语义也更准确，但 G5 仍需按
   §5.3 增加 retired 例外。
3. **33 个 matrix 的 scenario 级 parity 是否仍在 P7-1 范围内？**
   - 若"在"：31 个保持 blocked，等默认运行的真实 checker + 新 DTO；D 类只能
     消减 10 个 planned。
   - 若"不在"：需要 coverage waiver + G5 改写；这不是 exact，也不能写成 green。
   建议：保持在范围内；不要整体 retire。
4. **是否新增 `documentationSource` / `definition.kind` 到 C0？**
   影响 `PAGE_BASIC`、`COMPONENT_PROPERTIES`、`COMPONENT_JS`（docSource）和
   `PAGE_BASIC`、`COMPONENT_PROPERTIES`、`CROSS_FILE_OVERLAY`、`ALIAS_PROJECT`、
   `COMPONENT_DATA_CROSS_FILE`（defKind）。
   建议：如果要让这些 case 有机会 exact，必须新增；否则只能保持 blocked。
5. **是否实现 `perturbsUserTypes` / `userScriptDiagnosticsIntroduced` 的 native 测量？**
   影响 30 个 matrix case。建议：作为 F1 diagnostics ownership 的后续切片，而不是
   用 fake 断言。
6. **默认运行是否提供 pinned 引擎？** F2 §5 要求缺引擎失败；当前 matrix 的 checker
   半边只有 opt-in gate。若不把引擎纳入默认套件，31 个 case 永远不能 exact。
7. **`TS-MATRIX-0092` 是否接受 build-only re-scope？**
   建议：接受，`comparison_type=contract_translated`，note 写清 checker 半边。
8. **`TS-COMPILER-0063` 是否接受 digest/重开翻译？**
   建议：接受为 exact；若 owner 要求对象身份，则降为 near_exact（仍 planned）。
9. **6 个 retire 的替代证据是否足够？** 特别是 `TS-SERVER-0109` 的 title hover
   目前只有 0184/0197 的 typo range，没有等价的 "title hover string" 测试。
   建议：补一个 native hover 测试，或把该 case 保持 blocked 而不是 retire。

> 第 9 点是本文件唯一"建议 retire 但替代证据不完全对称"的 case。若 owner 要求
> 逐断言替代，`TS-SERVER-0109` 应改为 blocked，D 类可 retire 数降为 5。

---

## 8. 本文件不声称

1. 不声称任何 case 已 `green`、`reviewed` 或 `closed`；2 个 exact 只是"可由默认运行
   的 Rust 测试表达"的评估，测试尚未编写。
2. 不声称 31 个 matrix case 可以 waiver 或 retire；它们必须保持 blocked，除非 owner
   明确选择 coverage waiver。
3. 不声称 `retire` 等于"已验证"；retire 只表示"不再作为迁移门禁"。
4. 不声称 D 类可以完成 P7-1；乐观方案仍有 223 planned。
5. 不声称 C1 已正式废止；ADR-0008 仍是 `proposed`，`type-bridge-c1.md` 仍是
   `accepted`。
6. 不声称 `MAX_LIVE_SNAPSHOTS` 与 `MAX_POOLED_PROJECTS` 等价。
7. 不声称 fake engine 能证明 checker 语义；`diagnosticsExact` 尤其不能。

---

## 9. 只读复核记录

本文件写入前执行的只读检查（未运行测试）。D 过滤为 33 个
`TS-MATRIX-0102-*-IN_PLACE_SHADOW` + `TS-MATRIX-0092`、`TS-COMPILER-0050/0063/0086`、
`TS-PROTOCOL-0021/0395`、`TS-SERVER-0094/0109`：

```text
jq '.counts.statuses, .counts.migration_evidence' docs/plans/tsgo-test-ledger.json
jq '[.cases[] | select(<D 过滤>)] | length' docs/plans/tsgo-test-ledger.json      # 41
jq '[.cases[] | select(<D 过滤>) | select(.migration_evidence != null)] | length' # 0
jq '[.cases[] | select(<D 过滤>) | select(.requires_decision)] | length'          # 0
jq '[.cases[] | select(<D 过滤>) | group_by(.status)]'                            # 全部 planned
grep -n "ALLOWED_STATUSES\|must cite the retired Candidate B\|decision_ids.len" \
  crates/wxml-lsp/tests/native_parity/ledger.rs
grep -n "run.diagnosticsExact\|run.perturbsUserTypes\|userScriptDiagnosticsIntroduced" \
  backends/typescript/test/matrix.test.ts
grep -n "classify_data_root\|DataRootKind::TypeReference\|DataRootKind::Unsupported" \
  crates/wxml-backend-tsgo/src/shadow/layout.rs
grep -n "MAX_LIVE_SNAPSHOTS" crates/wxml-backend-tsgo/src/config.rs
grep -n "MAX_POOLED_PROJECTS" backends/typescript/src/compiler.ts
git status --short
```

写入完成时复核（并行回填后）：

```text
counts.statuses = { planned: 233, green: 70, retired-with-decision: 36 }
counts.migration_evidence = { assessed_cases: 197, exact: 70, near_exact: 2,
  weaker: 96, structural: 7, blocked: 22, remaining_planned_without_evidence: 106 }
D 过滤结果 = 41 个全部 planned / 0 证据 / 0 requires_decision
ledger.rs: GREEN_CASES=70, PLANNED_CASES=233, RETIRED_CASES=36, WEAKER_CASES=96
```

未执行 `cargo test`、`cargo clippy`、`cargo fmt` 或任何 workspace 级命令；
未修改 `docs/plans/tsgo-test-ledger.json` 或 `crates/wxml-lsp/tests/native_parity/ledger.rs`。
