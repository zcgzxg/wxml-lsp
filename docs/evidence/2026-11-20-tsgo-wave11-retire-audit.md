# tsgo wave-11 E 类退休审计（2026-11-20）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务书给定 HEAD：`43625ee`。读取开始后并行提交 `51abc13`
  （`test(tsgo): close conditional parity and ledger backfill`）落地，本文按写入前
  实际 HEAD `51abc13` 的已提交台账记录。任务书里的 `43625ee` 快照是
  `68 green / 235 planned / 36 retired`；`51abc13` 是
  `70 green / 233 planned / 36 retired`。若后续继续回填，以 JSON 重新读取为准；
  本文的 retire 结论只依赖 6 个 E case 仍为 `planned`，不依赖 green 的绝对值。
- 记录日期：2026-11-20
- 范围：**只新增本文件**。未修改生产代码、测试源码、`Cargo`、delivery、台账
  (`docs/plans/tsgo-test-ledger.json`)、台账 Markdown、contract、ADR、README、CI
  或脚本；未 commit，未启动 review；未运行任何测试，只做只读读取与 `grep`/`sed`/
  `python3` 统计。
- 并行状态：工作树另有未跟踪的 wave-11 D 类文档
  `docs/evidence/2026-11-20-tsgo-wave11-contract-decisions.md` 和 C 类 triage 文档
  `docs/evidence/2026-11-20-tsgo-wave11-native-capability-triage.md`；wave-10b
  回填已包含在 `51abc13`。本文不修改、不覆盖，也不把它们的结论当作 E 类结论。
  D 类文档提出的
  `retirement_decision` validator 扩展与本文建议同源，落地时必须合并成一次改动，
  不能各改一套。
- 输入：
  - `docs/evidence/2026-11-20-tsgo-remaining-ledger-wave9-plan.md` §2.5 / §3
  - `docs/plans/tsgo-test-ledger.json` 的 6 个 E case 与 `counts`
  - `docs/reference/tsgo-test-contract.md` §3 / §8
  - `docs/adr/0008-rust-tsgo-no-node-backend.md`
  - 旧 TS 测试与现有 Rust 测试/能力

**结论先行：**

1. **按契约文字可以退休 5 个，按当前 validator 实现只有 1 个不需要改退休判定逻辑。**
   `TS-COMPONENT-0774` 是 6 个 E case 中唯一的 Candidate-B-only case，能复用现有
   `obsolete + requires_decision` 分支；`TS-META-0027` 和 3 个 `TS-EVIDENCE-*`
   若直接改 JSON，会撞上 `reason.contains("Candidate B")`、
   `decision_ids.len()==37`、`non_obsolete_decision_ids==["TS-META-0027"]` 等硬断言。
2. **`TS-META-0027` 的替代证据最强，但退休前必须处理 C1 文档的状态。**
   F0 引擎/类型资产 pin 已在默认 Rust 测试中存在；但
   `docs/reference/type-bridge-c1.md:391` 仍写 TypeScript `6.0.3`，并声称
   “均有断言测试”。只退休测试、不改文档，会留下一个假陈述。
3. **3 个 `TS-EVIDENCE-*` 退休在语义上成立，但与 2026-11-19 计划的 WP8
   exact acceptance 冲突。** 该计划明确要求 “sanitizer POSIX/Windows path forms”
   和 “no absolute paths in committed evidence”；退休它们等于书面缩小 WP8 范围，
   必须由 integrator 改计划/契约，不能只改台账。`TS-EVIDENCE-0050` 是最弱的一个：
   `matrix.json` 仍在仓库里，补一个 10 行左右的 hygiene 测试就能保留检查。
4. **`TS-REWRITE-0378` 不能按“无替代退休”处理。** 它保护大表达式近线性发射；Rust 有
   `rewrite_expression`，但没有 1 MiB / 4 MiB / 9k roots 的默认 perf 测试，
   `perf_budget` 分区仍是 0 migrated 的 blocked skeleton，`TS-PERF-0012`
   也不测 rewrite。退休等于放弃唯一的大输入回归保护。
5. **推荐方案：退休 5 个（3 evidence + META + 0774），保留 0378 planned。**
   以 `51abc13` 的 `70 / 233 / 36` 为基线，得到 **70 green / 228 planned /
   41 retired**；green/exact 不变。保守方案只退休 0774，得到
   70 / 232 / 37。无论哪个方案，**planned 减少都不是迁移完成**，不能写成
   `green`、`exact` 或“已等价”。

---

## 1. 台账快照与合法退休规则

### 1.1 快照

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

6 个 E case 在 `51abc13` 全部是 `planned`、无 `migration_evidence`、
`target_rust_test_status=proposed`；只有 `TS-META-0027` 的
`requires_decision=true`、`decision_status=accepted-rewrite`。`TS-META-0027`
的 `decision_reason` 已写明“replacement is an F0/F1 native version/schema pin
test, which does not exist in this ledger yet”，这是本轮要解决的历史遗留。

### 1.2 契约允许的状态机

`docs/reference/tsgo-test-contract.md:92` 只允许：

```
planned → red → green → reviewed → closed
planned → retired-with-decision
```

`docs/reference/tsgo-test-contract.md:91` 同时要求：任何 `obsolete` 必须有集成负责人
裁决与代码证据；不得用“旧测试已删除”代替新测试通过。`retired-with-decision`
是**终态非 green 桶**，不是迁移证据，不得计入 `exact` 或 `green`。

### 1.3 当前 validator 的硬约束

`crates/wxml-lsp/tests/native_parity/ledger.rs` 比契约更窄，直接决定“能不能落地”：

| 约束 | 位置 | 对 E 类的影响 |
| --- | --- | --- |
| `ALLOWED_STATUSES` 只有 `planned/green/retired-with-decision` | `:88` | 没有 `waived` 桶；waiver 只能写成 retired 或另加状态 |
| `retired` 只能配 `disposition=obsolete` | `:979-984` | `direct/rewrite/split` 的 case 不能直接 retired |
| `obsolete` 的 `disposition_reason` 必须包含 `"Candidate B"` | `:984` | META/evidence 若改成 obsolete，会被迫写一句假话 |
| retired 必须 `requires_decision=true`、`target_rust_test_status=not_migrated_internal_evidence` | `:981-985` | 非 Candidate-B 的退休也要套 “internal evidence” 标签 |
| `obsolete == RETIRED_CASES`、`non_obsolete == PLANNED+GREEN` | `:1022-1023` | 每退休一个都要同步改常量与 disposition 计数 |
| `decision_ids.len()==37`、`non_obsolete_decision_ids==["TS-META-0027"]` | `:1025-1027` | META 是唯一非 obsolete 的 decision 哨兵；退休它会改这两个断言 |
| retired 不得有 `migration_evidence` | `:1100-1107` | 不能把“没有 Rust 测试”包装成 weaker 证据 |

因此本文把“合法”分两层：

- **契约合法**：满足 §1.2，有 integrator 裁决 + 代码证据；5 个非用户可见/非产品路径
  case 都可以。
- **当前 validator 可直接通过**：只有 `TS-COMPONENT-0774`；其余 4 个必须先扩展
  validator/contract，属于“合法但未落地”，不是“不合法”。

---

## 2. 逐 case 审计

### 2.1 TS-EVIDENCE-0011

| 项 | 内容 |
| --- | --- |
| Oracle | `backends/typescript/test/evidence.test.ts:11` “sanitizer replaces POSIX and Windows path forms” |
| 断言 | 4 条：POSIX backend 路径 → `<backend-root>/samples/page-basic/index.ts`；Windows backend 路径 → `<backend-root>\samples\page-basic\index.ts`；repo 路径 → `<repo-root>/docs/evidence/type-bridge-c1.md`；序列化结果不含 `home/` |
| 现 TS 实现 | `backends/typescript/src/evidence.ts:59` `sanitizeEvidence`，生产代码只被 `backends/typescript/scripts/emit-evidence.ts:58` 调用（其余调用在 `evidence.test.ts`） |
| 建议 | 条件退休：`legacy_tooling` / `accepted-out-of-scope` |
| 替代 Rust 测试 | **无等价物。** `crates/wxml-backend-tsgo/src/input.rs` 的 `AssetPath` 只校验资产/源码路径形态，不做 JSON 值中的 POSIX/Windows 根替换；不能声称等价 |
| 是否仍属产品需求 | 否。它不是 LSP 用户可见行为，只是 legacy evidence emitter 的隐私清洗 |
| 风险 | 低。若 P7 后 emitter 被重新启用或 evidence 重新生成，绝对路径可能泄漏；当前没有回归保护 |

**为什么建议退休：** wave-9 §2.5 的判断成立——native 没有 evidence writer，也没有
`assertEvidenceDir`。`emit-evidence.ts` 是唯一 writer，P7-4 删除旧 TS 实现/测试/
构建时会一起删除；Rust 侧没有任何等价写入路径需要清洗。

**合法处置：** 只有在 integrator 书面接受“legacy evidence 工具链不属于 native
迁移范围”后，才能退休。decision 需要写清：

- 代码证据：`backends/typescript/scripts/emit-evidence.ts:55,58` 是生产代码的唯一调用点；
  `backends/typescript/package.json:18` 的 `npm run evidence` 只服务旧 backend；
  `docs/adr/0008-rust-tsgo-no-node-backend.md:110` 记录旧 TS 源码/测试树将删除。
- 替代证据：`none`（不要拿 `AssetPath` 冒充）。
- 若 integrator 不接受范围缩小，则保持 `planned`；缺的是 Rust sanitizer + 一个
  真实 writer，或者一份明确的 out-of-scope 裁决。

**冲突点：** `docs/plans/2026-11-19-tsgo-remaining-ledger-plan.md:453` 把
“sanitizer POSIX/Windows path forms”列为 WP8 exact acceptance。退休必须同步改这份
计划，否则台账与计划自相矛盾。

### 2.2 TS-EVIDENCE-0025

| 项 | 内容 |
| --- | --- |
| Oracle | `backends/typescript/test/evidence.test.ts:25` “evidence output must stay inside the repository layout” |
| 断言 | 5 条：`<repo>/docs/evidence/data/type-bridge-c1` 不抛；`/tmp/docs/...` 抛 `EvidencePathError`；`<repo>/docs/elsewhere` 抛；repo 根本身抛；没有项目 markers 的复制树抛 |
| 现 TS 实现 | `backends/typescript/src/evidence.ts:28` `assertEvidenceDir`，生产代码只被 `backends/typescript/scripts/emit-evidence.ts:55` 调用（其余调用在 `evidence.test.ts`） |
| 建议 | 条件退休：`legacy_tooling` / `accepted-out-of-scope` |
| 替代 Rust 测试 | **无等价物。** `AssetPath`、`PinnedEngine::launch_plan` 的路径校验针对资产/启动目录，不是 evidence 输出目录；不能声称等价 |
| 是否仍属产品需求 | 否。它是 dev 工具写路径的安全边界，不是 LSP 行为 |
| 风险 | 低到中。若未来加 Rust evidence writer，必须重新引入同样的写入边界；退休不会阻止未来实现，但 ledger 不再追踪该语义 |

**为什么建议退休：** 与 0011 同源。更硬的代码证据是：`assertEvidenceDir` 的 marker
检查依赖 `docs/reference/type-bridge-c1.md` 和
`backends/typescript/package.json`；P7 删除旧 backend 后第二个 marker 不存在，
这个测试即使原样搬进 Rust 也无法成立。

**合法处置：** 与 0011 相同，`legacy_tooling` + `accepted-out-of-scope`。decision
证据需要包含 `backends/typescript/src/evidence.ts:28-44` 的 marker/目录绑定，以及
P7 删除范围。若 integrator 不接受，保持 `planned`；缺的是 Rust evidence 目录策略或
书面 out-of-scope 裁决。

**冲突点：** 同一份 2026-11-19 计划第 453 行；另外第 457 行把
`TS-EVIDENCE-0025/0050` 列为 “need the evidence tree”，说明当时预期是移植，不是退休。
退休必须改这段“Known blocked”。

### 2.3 TS-EVIDENCE-0050

| 项 | 内容 |
| --- | --- |
| Oracle | `backends/typescript/test/evidence.test.ts:50` “the committed evidence contains no absolute paths” |
| 断言 | 3 条：`matrix.json` 含 `<backend-root>`；不含 repo root；`runs.length >= 40` |
| 当前 artifact（读取时） | `docs/evidence/data/type-bridge-c1/matrix.json` 存在，575,245 bytes，66 runs，124 个 `<backend-root>`，无 `/home/`、无 repo root；断言今天为真 |
| 建议 | 条件退休（最弱）：`frozen_artifact`；或保持 planned 并补一个小 hygiene test |
| 替代 Rust 测试 | **无。** 但这是 6 个里最容易补的：读 JSON、断言 placeholder/不含 repo root、`runs >= 40`，不需要任何产品能力 |
| 是否仍属产品需求 | 否。它是仓库证据卫生，不是 LSP 行为 |
| 风险 | 低。artifact 已冻结、无 writer；风险只是未来手工编辑引入绝对路径，且没有检查 |

**为什么建议退休：** artifact 是 legacy A/B evidence，emitter 在 P7 删除；没有
Rust writer，也没有 Rust 测试读它。这个 case 验证的是 emitter 的输出，而不是产品
能力。

**为什么它最弱：** 与 0011/0025 不同，artifact 本身会留在仓库里（ADR 保留历史证据）。
如果没有 emitter，文件不会再变；但“不再变”也意味着一个一次性检查就足够。若
integrator 不想把 evidence 工具链整体判为 out-of-scope，最保守的做法是：
保持 `planned`，在 `messages_evidence` 或 delivery 测试里补一个只读 `matrix.json`
的 hygiene test；这样 planned 只多 1，但保留了实际检查。

**合法处置：** 退休时 `retirement_decision.scope="frozen_artifact"`，reason 必须写
“artifact frozen; emitter removed; no Rust writer”；替代证据可以为空，或指向一个
一次性的 audit 记录（不是 ledger case）。不能把“文件现在没有绝对路径”写成
“已验证迁移”。

**冲突点：** 2026-11-19 计划第 454 行 “no absolute paths in committed evidence”
是 WP8 exact acceptance 的一部分；退休必须改计划。

### 2.4 TS-META-0027

| 项 | 内容 |
| --- | --- |
| Oracle | `backends/typescript/test/meta.test.ts:27` “the C1 contract documents the pinned versions” |
| 断言 | 2 条：`docs/reference/type-bridge-c1.md` 匹配 `6\.0\.3`；匹配 `5\.3\.1` |
| 现文档 | `docs/reference/type-bridge-c1.md:391` 仍写 TypeScript `6.0.3`、typings `5.3.1`、Node ≥ 24，并称“均有断言测试” |
| native pin | `crates/wxml-backend-tsgo/src/config.rs:20` `ENGINE_VERSION="7.1.0-beta"`；`crates/wxml-backend-tsgo/src/input.rs:128` `PINNED_TYPINGS_VERSION="5.3.1"` |
| 建议 | 退休：`superseded_contract` / `accepted-replaced` |
| 替代 Rust 测试 | **有，且是默认运行：**（1）`crates/wxml-backend-tsgo/src/config.rs:415` `pinned_identity_uses_literal_f0_values` 断言 7.1.0-beta、gitHead、tarball、binary、AST v9；（2）`crates/wxml-backend-tsgo/src/input.rs:2201` `type_assets_validate_pin_identity_and_paths` 断言 typings 5.3.1 pin；（3）`crates/wxml-delivery/tests/assets_lock_root.rs:130` `repository_root_lock_is_frozen_and_canonical` 断言 committed lock 的 7.1.0-beta / 5.3.1 与摘要 |
| 是否仍属产品需求 | 版本 pin 是产品需求，但已由 F0 覆盖；C1 文档字符串本身不是 |
| 风险 | 中。C1 文档仍是 `accepted` 且被 `docs/reference/tsgo-native-contract.md:24` 引用；只退休测试、不改文档，会留下 “6.0.3 均有断言” 的 stale claim |

**为什么建议退休：** 这个 oracle 钉的是历史 Node 后端的 C1 文档版本，而产品已经
迁移到 7.1.0-beta。保留它等于用一个通过的测试证明一个产品不再使用的版本；这不是
迁移证据，是历史残留。F0 pin 的意图已由 3 个默认 Rust 测试覆盖。

**替代关系要写清：** `TS-META-0009`（“TypeScript and the typings package are
pinned”）是同文件的 sibling case，读取时是 `planned` / `weaker`；它记录的是 pin 意图
被翻译到 native，**不是 green**。退休 0027 不能把 0009 升 green，也不能声称
“版本 pin 已 exact 等价”（旧 6.0.3 vs 新 7.1.0-beta 是有意偏差）。

**合法处置：** 退休为 `superseded_contract`，`replacement_case_id` 可保留现有的
外部 id `F0-version-pin`（它不是 ledger case，必须注明），并在
`retirement_decision.replacement` 里列出上面 3 个真实测试。前置条件：

1. validator 允许非 Candidate-B 的 retired（见 §4.3）。
2. `docs/reference/type-bridge-c1.md` 的 §9 和头部 Status 改为
   “native 路径不实现 C1” 或把版本段改为 7.1.0-beta + 指向 F0 测试；否则退休与
   文档的 “均有断言测试” 冲突。
3. 如果 integrator 想保留 `TS-META-0027` 作为唯一非 obsolete 的 decision 哨兵，
   则不要退休；但这会把一个已有替代的 stale doc 测试永久留在 planned，本文不推荐。

### 2.5 TS-COMPONENT-0774

| 项 | 内容 |
| --- | --- |
| Oracle | `backends/typescript/test/componentBindings.test.ts:774` “symbol-bridge does not report resolved behavior roots as missing” |
| 断言 | 3 条：`result.ok===true`；`build.buildStatus==="partial"`；`behavior` fragment 中没有 `code===2339` 的诊断 |
| Layout | 显式 `layoutId: "symbol-bridge"`；Candidate B 是内部布局，生产不可选 |
| 生产代码证据 | `backends/typescript/src/server.ts:152` 调 `analyze(params,{backendDir})`，不传 layoutId；`backends/typescript/src/analysis.ts:416` 默认 `in-place-shadow`；`backends/typescript/src/analysis.ts:59-60` 注释说明 production 用 in-place |
| 建议 | 退休：`candidate_b_layout` / `accepted-obsolete` |
| 替代 Rust 测试 | **部分。** opt-in gate `G-P5-E2E-11` `native_component_behavior_data_and_properties`（`crates/wxml-lsp/tests/native_parity/native_engine_real.rs:2867`）断言 behavior-backed component 无诊断；`crates/wxml-backend-tsgo/tests/script_behavior.rs:340` `case_component_0570...`（ledger 为 blocked）和 `:430` `case_component_0763...`（near_exact）覆盖 behavior facts。**没有默认运行的 exact 2339 断言。** |
| 是否仍属产品需求 | 一般行为（resolved behavior root 不能被报 missing）是产品需求；symbol-bridge 这个机制不是 |
| 风险 | 中。退休后默认套件不再有 “behavior data fragment 不产生 2339” 的 exact 检查；只有 opt-in gate 覆盖，而 gate 不是默认 CI 绿 |

**为什么建议退休：** 它与 2026-11-17 退休的 36 个 Candidate-B/symbol-bridge case
同源：只断言内部布局，不是用户可选择的 LSP 行为。它可以按同一理由作为第 37 个
B-only case 处理。生产 in-place 路径仍由其他 behavior 测试和 gate 覆盖。

**但不要把“部分替代”写成“等价”：** gate 是 opt-in（`WXML_TEST_NATIVE=1` +
`WXML_REQUIRE_REAL_ENGINE=1`），这些 partial/near-exact 测试不跑真实 checker。
若 integrator 要求默认 exact 覆盖，先补一个 in-place 测试（resolved behavior
data field 作为 fragment，断言 native diagnostics 中没有 2339），再退休；否则在
decision 里明确写 “replacement strength = opt-in gate + partial/near_exact,
not exact”。

**合法处置：** 这是唯一能复用现有 validator `obsolete` 分支的 case：

- `disposition="obsolete"`；
- `disposition_reason` 必须包含 `"Candidate B"`（事实成立）；
- `requires_decision=true`；
- `target_rust_test_status="not_migrated_internal_evidence"`；
- `migration_evidence` 保持缺失；
- 把 2026-11-17 的 “36 个 Candidate B” 决策扩展为 37，新增一条
  `retirement_decision` 或更新 `integrator_decision` 的 scope；
- 同步 `RETIRED_CASES`、`PLANNED_CASES`、`decision_ids.len()` 和
  `counts.dispositions.obsolete`。

### 2.6 TS-REWRITE-0378

| 项 | 内容 |
| --- | --- |
| Oracle | `backends/typescript/test/rewrite.test.ts:378` “expression emission is linear for 1 MiB and 4 MiB / 9k roots” |
| 断言 | 4 条：`roots.length===9000`；1 MiB < 300 ms；4 MiB < 600 ms；`large < small*8 + 500` |
| 现 Rust 能力 | `crates/wxml-backend-tsgo/src/shadow/rewrite.rs:146` `rewrite_expression` 是单游标 emitter；`crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:327-346` 只测深度/畸形，不测 1 MiB/4 MiB |
| 建议 | **保持 planned，不退休** |
| 替代 Rust 测试 | **无。** `perf_budget` 分区仍是 blocked skeleton（`crates/wxml-lsp/tests/native_parity/perf_budget.rs` 的 `MIGRATED_CASES` 为空）；`TS-PERF-0012` 只测 cold start/hover/diagnostics，不测 rewrite |
| 是否仍属产品需求 | 是。大模板表达式发射的近线性是用户可感知的响应性/回归保护 |
| 风险 | 高。退休会放弃唯一的大输入回归保护；Rust emitter 若退化为二次复杂度，默认套件不会发现 |

**为什么不能按无替代退休：** 它不是 Candidate-B 布局，不是 legacy evidence 工具，
也不是过时文档 pin；它是真实性能断言。TS 测试注释记录 pre-fix O(n²) 发射器的最坏情况约
4.1 s / 15.6 s，说明这个断言曾经抓到真实回归。Rust 侧有对应实现，只是没有测。

**缺什么：**

1. 一个 Rust perf/线性测试：构造 1 MiB / 4 MiB、9k roots 的表达式，调用
   `rewrite_expression`，断言 9k roots 和按 Rust 基线设定的线性上界。阈值不能
   照搬 300/600 ms（Node 机器相关），应使用 Rust release/debug 的实测基线。
2. 或者一份 explicit waiver：integrator 接受“无大输入 rewrite perf 回归保护”，
   写明产品风险、owner、复查日期；但这属于 waiver，不是 retired，不能把
   planned 减掉的数字写成完成。

**重要：即使补了 Rust perf 测试，root count/内容部分可以 exact，但 timing 阈值
只能 `weaker`/`structural`。** 因为阈值是 mechanism-replaced，不是同一采样。
整体不能按 exact 升 green，除非评审明确接受“采样机制替换后的等价”；台账必须
如实写 `comparison_type=mechanism_replaced`，不能为了 green 强行 exact。

---

## 3. 处置矩阵与计数

### 3.1 逐 case 矩阵

| Case | Oracle 核心 | 当前 disposition | 建议 | 契约合法前提 | 替代证据 |
| --- | --- | --- | --- | --- | --- |
| `TS-EVIDENCE-0011` | sanitizer POSIX/Windows 路径 | direct | 条件退休 `legacy_tooling` | integrator 接受 evidence 工具链 out-of-scope；改 WP8 计划 | 无；`AssetPath` 不等价 |
| `TS-EVIDENCE-0025` | evidence 输出目录边界 | direct | 条件退休 `legacy_tooling` | 同上 | 无；marker 依赖旧 backend |
| `TS-EVIDENCE-0050` | committed matrix.json 无绝对路径 | direct | 条件退休 `frozen_artifact`；或保持 planned + 小测试 | 接受 artifact frozen；改 WP8 计划 | 可补 10 行 hygiene test |
| `TS-META-0027` | C1 文档含 6.0.3 / 5.3.1 | rewrite | 退休 `superseded_contract` | validator 扩展；改 C1 文档状态/§9 | F0 pin 3 个默认测试；TS-META-0009 weaker |
| `TS-COMPONENT-0774` | B 布局 behavior root 无 2339 | rewrite | 退休 `candidate_b_layout` | 现有 obsolete 分支；扩 36→37 | opt-in gate + partial/near_exact，非 exact |
| `TS-REWRITE-0378` | 1 MiB/4 MiB/9k roots 线性 | direct | **保持 planned** | 不适用 | 无；需新 perf 测试或 waiver |

### 3.2 计数方案

基线：`51abc13` 的 **70 green / 233 planned / 36 retired**。

| 方案 | 退休 case | green | planned | retired | exact | 说明 |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| 当前 | — | 70 | 233 | 36 | 70 | 基线 |
| A：仅 0774 | 0774 | 70 | 232 | 37 | 70 | 当前 validator 唯一可直接通过 |
| B：A + META | +META | 70 | 231 | 38 | 70 | 需 validator 扩展 + 改 C1 文档 |
| C：B + 0011/0025 | +2 evidence | 70 | 229 | 40 | 70 | 需 WP8 范围裁决 |
| D：C + 0050（推荐） | +0050 | **70** | **228** | **41** | **70** | 5 个退休，0378 保留 |
| E：D + 0378 | +0378 | 70 | 227 | 42 | 70 | 不推荐：丢失大输入 perf 保护 |

**无论哪个方案，`green` 和 `exact` 都是 70。** 退休不是迁移；不要把
planned 的减少写成 “完成 5 个 case”。如果 `TS-EVIDENCE-0050` 保留 planned
（改为补一个只读 hygiene test），方案 D 降为 4 个退休：70 / 229 / 40。

### 3.3 分区计数影响（推荐方案 D，5 个退休）

| 分区 | routed | evidence | unassessed | 说明 |
| --- | --- | --- | --- | --- |
| `behaviors` | 22 → 21 | 18 | 4 → 3 | 0774 |
| `messages_evidence` | 8 → 5 | 0 | 8 → 5 | 0011/0025/0050 |
| `protocol_limits` | 40 → 39 | 18 | 22 → 21 | META |
| `mapping_utf16` | 21（不变） | 19 | 2（不变） | 0378 保留 |
| 合计 | 303 → 298 | — | −5 | 只影响 unassessed |

全台账 `assessed_cases` 仍为 197，`evidence_cases` 不变，因为 6 个 case 都没有
`migration_evidence`；上表的 `evidence` 只是各分区内的证据数，不是合计。
如果选方案 E（再退休 0378），`mapping_utf16` 变为 20 routed / 1 unassessed，
合计 routed 297。

### 3.4 这不是迁移进度

- `exact` / `green` 不变；退休的 5 个不会出现在任何 Rust 测试证据里。
- 无证据 planned 从 106 减到 101；这 5 个仍然是“没有迁移证据”，只是从 planned
  移到 retired。P7-1 的 `planned=0` 目标仍剩 228 个。
- 如果把这 5 个写成 “已迁移/已等价”，就是伪完成；正确措辞是
  “有书面裁决的 retired，不是 green”。

---

## 4. 建议 ledger 字段与 validator/contract 改动

### 4.1 统一使用 `retirement_decision`

wave-11 D 类文档 §5.2 已经提出：不要复用 `requires_decision` 给新的非
Candidate-B retire，否则会打破 `decision_ids.len()==37` 和
`non_obsolete_decision_ids==["TS-META-0027"]`。E 类应复用同一扩展，只加一个
per-case 对象：

```json
{
  "status": "retired-with-decision",
  "target_rust_test_status": "not_migrated_internal_evidence",
  "retirement_decision": {
    "id": "wave11-e-<case>",
    "date": "2026-11-20",
    "owner": "integrator",
    "required_by": "P7-1 ledger zero",
    "scope": "legacy_tooling",
    "reason": "为什么旧 oracle 不再是产品门禁",
    "product_impact": "none-user-visible",
    "evidence": [
      "code path:line that shows the capability is legacy/removed"
    ],
    "replacement": [
      {
        "kind": "rust_test",
        "path": "crates/...",
        "line": 0,
        "test": "test_name",
        "strength": "weaker"
      }
    ]
  }
}
```

字段取值：`scope` 是
`legacy_tooling | superseded_contract | frozen_artifact | candidate_b_layout |
mechanism_replaced`（最后一个是 D 类的值，E/D 共用同一个 enum）；
`replacement[].kind` 是 `rust_test | ledger_case | none`；
`replacement[].strength` 是
`exact | near_exact | weaker | structural | blocked | opt_in`。

硬性要求：

- `migration_evidence` 必须缺失，不能写 `rust_tests`、`parity` 或
  `green_eligible`。
- `replacement` 里每一项都要标 `strength`；`opt_in` 不能写成 `exact`。
- `product_impact` 不能只写 `none`，reason 必须解释为什么不是用户可见行为。
- `retirement_decision.id` 必须稳定，供 contract/ADR/后续 wave 引用。

### 4.2 逐 case 建议值

| Case | `disposition` | `status` | `retirement_decision.scope` | `replacement` | 备注 |
| --- | --- | --- | --- | --- | --- |
| `TS-EVIDENCE-0011` | direct（保留，走新分支） | retired | `legacy_tooling` | none | 需改 WP8 计划；代码证据 emit-evidence.ts |
| `TS-EVIDENCE-0025` | direct（保留，走新分支） | retired | `legacy_tooling` | none | marker 依赖旧 backend |
| `TS-EVIDENCE-0050` | direct（保留，走新分支） | retired | `frozen_artifact` | 可选：一个只读 hygiene test | 最弱；可改为保持 planned |
| `TS-META-0027` | rewrite（保留，走新分支） | retired | `superseded_contract` | F0 pin 3 测试 + `TS-META-0009`（weaker） | 必须改 C1 文档 §9/Status |
| `TS-COMPONENT-0774` | obsolete（rewrite→obsolete） | retired | `candidate_b_layout` | `G-P5-E2E-11`（opt_in）+ `script_behavior`（blocked/near_exact） | 扩 36→37；`requires_decision=true` 可复用 |
| `TS-REWRITE-0378` | direct | planned | — | — | 保持；补 perf 测试或 waiver |

`TS-COMPONENT-0774` 建议继续用现有 `obsolete` 分支（`requires_decision=true`），
因为 36 个同类 case 已经这么做；可选再加一个 `retirement_decision` 对象供统一检索，
但 validator 不依赖它。其余 4 个保留原 disposition、走新的 `retirement_decision`
分支，避免把 `disposition` 改成 `obsolete` 后被迫写 “Candidate B”。

**关于 disposition 语义：** `direct` 的原意是“可直接移植”；退休后它变成“可移植但
裁决不移植”，语义略有拉伸。如果评审要求更纯粹，可以把 3 个 evidence 改成
`obsolete`，并把现有 `reason.contains("Candidate B")` 泛化为
`retirement_kind`；代价是 `counts.dispositions.direct` 240 → 237、
`obsolete` 36 → 40（含 0774），而且要改 36 个旧 case 共享的分支。本文推荐
“保留 disposition + `retirement_decision`”，因为改动局部、历史可追溯；两种都可
接受，但不能只改 JSON 不改 validator。

### 4.3 validator 改动清单

以推荐方案 D（5 个退休）为例；下面的绝对计数只含 E 类，如果 wave-11 D 类的
6 个 retire 同时落地，`RETIRED_CASES`/`PLANNED_CASES` 要在同一组常量上叠加，
不能各写一个版本：

1. **新增非 Candidate-B 的 retired 分支**：允许
   `disposition in ["direct","rewrite","split"] && status=="retired-with-decision"`
   + `retirement_decision`，且 `migration_evidence` 必须缺失；保留现有
   `obsolete` 分支给 36+0774。
2. **计数**：`RETIRED_CASES` 36 → 41；`PLANNED_CASES` 233 → 228；
   `GREEN_CASES` 70 不变。当前 `non_obsolete == PLANNED+GREEN` 要改成
   `non_obsolete == PLANNED+GREEN+NON_OBSOLETE_RETIRED`（或按 status 统计）。
   retired case 仍保留唯一的 `target_rust_test` 标签（现有 36 个都有），
   不加入 `MIGRATED_CASES`，但不能为空或重复。
3. **decision 计数**：如果 META 退休且改用 `retirement_decision`，
   `decision_ids.len()` 与 `non_obsolete_decision_ids` 必须重写。推荐改成
   “每个 retired case 必须有 `retirement_decision` 或
   `decision_status=accepted-obsolete`”，不再硬编码 37 / `["TS-META-0027"]`。
4. **target 状态**：`RETIRED_TARGET_STATUS` 目前叫
   `not_migrated_internal_evidence`；对 legacy tooling/superseded contract 不准确。
   建议新增 `not_migrated_retired`（或按 scope 细分），或至少改注释说明它覆盖
   所有非产品 retire。
5. **counts**：同步 JSON 的 `counts.statuses`、
   `counts.migration_evidence.statuses`、`by_partition`（behaviors /
   messages_evidence / protocol_limits）。`counts.dispositions` 只因 0774
   发生 `rewrite 52 → 51`、`obsolete 36 → 37`；其余 4 个保留原 disposition，
   所以 direct 240 / rewrite 51 / split 11 / obsolete 37（合计 339）。
   `counts.integrator_decisions.accepted_obsolete` 36 → 37；
   `accepted_rewrite` 若把 META 记为 replaced 则 1 → 0，并新增
   `accepted_superseded_contract=1`、`accepted_legacy_tooling=2`、
   `accepted_frozen_artifact=1`（或一个 `accepted_retired_e=5`）。
6. **contract**：`docs/reference/tsgo-test-contract.md:92` 增加
   “`direct/rewrite/split + retired-with-decision` 需 `retirement_decision`”；
   `:150-151` 的 G5 退出条件改为
   “全部 direct/rewrite/split 为 green/reviewed，或 retired 且有
   `retirement_decision`”。否则 G5 会与台账矛盾。
7. **C1 文档**：`docs/reference/type-bridge-c1.md` 头部 Status 增加
   “native 路径不实现 C1；C1 只保留为历史 Node 桥契约”，并把 §9 的 6.0.3
   改为 7.1.0-beta（指向 F0 测试）或标注为历史。只改台账不改这里，会留下
   “TypeScript 6.0.3 均有断言测试” 的假陈述。
8. **计划文档**：
   `docs/plans/2026-11-19-tsgo-remaining-ledger-plan.md:453-457` 的 WP8 exact
   acceptance / Known blocked 要改，明确 evidence 工具链已 out-of-scope；
   `docs/plans/2026-11-17-rust-tsgo-migration.md` 的 P7-1 状态数字也要随基线更新。
9. **台账 Markdown**：`docs/plans/2026-11-17-tsgo-test-ledger.md` 追加 wave-11
   retire 段，不覆盖 2026-11-17 的 36-case 历史段。

### 4.4 落地顺序

1. 先拿到 §5 的 1-5 项书面裁决；没有裁决的 case 保持 planned。
2. 合并 E/D 共用的 validator 扩展和 contract §3/G5 改动，避免两个 wave 各改一套。
3. 更新要退休的 case（推荐 5 个）的
   `disposition`/`status`/`retirement_decision`/`requires_decision` 字段；
   不要写 `migration_evidence`。
4. 重新读取 live JSON，再更新 `counts.statuses`、`counts.migration_evidence.statuses`、
   `by_partition`、`counts.dispositions`、`counts.integrator_decisions`。
5. 跑 `jq empty`、`cargo test -p wxml-lsp --test native_parity`、`cargo fmt --check`、
   `cargo clippy`，确认没有把 retired 计入 `MIGRATED_CASES`。
6. 最后更新 contract/台账 Markdown/C1 文档；不要把顺序反过来先改数字。

### 4.5 不要做的事

- 不要把 `migration_evidence` 加到 retired case 上；validator 会拒绝。
- 不要把 `TS-META-0009` 或任何 sibling case 顺手升 green；它们仍是 planned/weaker。
- 不要把 `TS-REWRITE-0378` 的退休写成 “perf 已由 TS-PERF-0012 覆盖”；后者不测
  rewrite。
- 不要只改 JSON 常量而不改 contract/G5/计划；否则下一次 wave 会重新引入相同冲突。

---

## 5. 需要契约/用户裁决的点

以下按阻塞程度排序；前 4 项不裁决就无法合法落地推荐方案。

1. **Evidence 工具链范围（阻塞 3 个 case）。**
   是否接受 `backends/typescript/scripts/emit-evidence.ts` 及其 sanitizer/
   path policy 在 P7 后整体删除、native 不再提供 evidence writer？
   如果接受，3 个 `TS-EVIDENCE-*` 可以按 `legacy_tooling`/`frozen_artifact`
   退休，但必须同步改 2026-11-19 WP8 的 exact acceptance。
   如果不接受，3 个保持 planned；`TS-EVIDENCE-0050` 可以补一个只读 hygiene
   test，0011/0025 需要 Rust sanitizer/writer，或登记为 `parity=blocked` 并继续
   留在 planned。

2. **META 哨兵与 C1 文档（阻塞 META）。**
   是否退休 `TS-META-0027`，并把 `docs/reference/type-bridge-c1.md` §9 更新到
   7.1.0-beta 或标记为历史？退休会让
   `non_obsolete_decision_ids==["TS-META-0027"]` 失效，必须接受这个 validator
   不变量改变。若 integrator 想保留哨兵，META 保持 planned，但它是已有替代的
   stale doc pin，会永久占用 planned。

3. **0774 的替代强度（阻塞 0774 或要求先补测试）。**
   opt-in `G-P5-E2E-11` + partial/near_exact `script_behavior` 是否足以替代
   symbol-bridge 的 exact 2339 断言？如果不足，先补一个默认运行的 in-place
   behavior-data 2339 测试，再退休；否则 decision 必须写
   `strength=opt_in`（partial/near_exact 只作辅助），不能写 exact。

4. **0378 的性能门禁（阻止无替代退休）。**
   是补一个 Rust 大输入 rewrite perf 测试（预计 weaker/structural），
   还是书面 waiver 接受“无 rewrite perf 回归保护”？如果是后者，写明 owner、
   产品风险、复查日期；waiver 不是 retired，planned 不能因此减少。若 integrator
   进一步把大输入 perf 判为 out-of-scope，才可以按 coverage waiver/retired 处理，
   但那必须明确写出被放弃的覆盖面，不能写成“已迁移”。

5. **Schema 选择（阻塞所有非 0774 的 retire）。**
   采用 wave-11 D 文档的 `retirement_decision` 对象 + 新 validator 分支；
   或把 evidence 改成 `obsolete` 并泛化现有 Candidate B 检查；或把所有 case
   改成 `obsolete` 并在 reason 里写 “Candidate B”。最后一种更小但语义错误；
   本文推荐第一种，第二种可接受但会改 36 个旧 case 的共享分支。三种都必须改
   `RETIRED_CASES`、`PLANNED_CASES`、contract §3 和 G5。

6. **`not_migrated_internal_evidence` 命名（非阻塞但影响可读性）。**
   这个值原本为 Candidate B 设计；用于 legacy tooling / superseded contract
   会误导。是否新增 `not_migrated_retired` 或按 scope 细分？

7. **计数基线（非阻塞但必须统一）。**
   本文用 `51abc13` 的 70/233/36；任务书给的是 `43625ee` 的 68/235/36。
   落地时必须重新读取 JSON/validator 常量，不能复制本文或旧 wave 的数字。

8. **G5 措辞（非阻塞但必须同步）。**
   `docs/reference/tsgo-test-contract.md:150-151` 目前要求全部
   direct/rewrite/split 为 green/reviewed。退休 5 个后必须改成
   “green/reviewed 或 retired-with-decision + retirement_decision”，
   否则 G5 与台账自相矛盾。

---

## 6. 验证与限制

- 本文件是只读审计，未运行 `cargo test`、未改任何代码/测试/ledger/contract/README，
  未 commit、未 review。
- 台账计数在读取期间被并行 wave-11 工作推进：`43625ee` 是 68/235/36，
  `51abc13` 是 70/233/36；本文用后者作基线，只依赖 6 个 E case 仍为 planned。
  落地前必须重新读取 `docs/plans/tsgo-test-ledger.json` 的
  `counts.statuses` 和 `crates/wxml-lsp/tests/native_parity/ledger.rs` 常量。
- 所有 `path:line` 是读取时值；`script_behavior.rs` 等文件已被并行 wave 改过，
  落地前要重新 `grep`。
- 本文的建议不替代 integrator 裁决；特别是 §5 的 1-5 项没有书面结论时，
  只能保持 planned 或已记录的 retired，不能把“建议退休”写成“已退休”。
