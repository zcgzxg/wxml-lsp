# Rust + tsgo 测试迁移账本（P1-B + P3-S/P4-B + P5/P5.1 状态回填）

生成时间：2026-11-17T23:08:57+08:00；P3-S/P4-B 证据与 P5 状态回填：2026-11-18；P5.1 适配器/定义/性能回填：2026-11-19

状态：**green 48 / planned 255 / retired-with-decision 36**（P5 将 32 个 exact-parity 用例提升为 `green`；P5.1 将 1 个 adapter exact 用例提升为 `green`；P6 回填再提升 3 个 exact 用例并登记 2 个 weaker 证据；wave-5 回填再提升 8 个 exact 用例并登记 26 个新证据；wave-6 先新增 6 个 weaker + 3 个 blocked 证据，随后的组件绑定跟进把其中 4 个提升为 exact `green`、2 个明确保持 blocked；3 个 near-exact 保留 `planned` + green recommendation；36 个 Candidate B 不变；未删除任何旧测试）

基线：`b9616c557e54a06b7d756e516e6dd7c4016664cf`（`tsgo/migration`，包含 `4d59ca7`）

实现 HEAD（P3-S/P4-B）：`47947da4878212cb8bef7d1505c313b9b51b0f48`

实现 HEAD（P5.1 回填）：`6ba407b4cb8d3ea124276337e6da983a1e9f3a51`

实现 HEAD（wave-6 组件绑定跟进）：`0bf666b994163d2a44a4b87b0b8bb5d4e9101bf1`

机器可读账本：`docs/plans/tsgo-test-ledger.json`

P0 证据：`docs/evidence/2026-11-17-tsgo-migration-baseline.md`

## 1. 结论摘要

- 21 个 TS 测试文件 + `support.ts`；**274 个静态 `test()` 声明**（273 个字面标题 + 1 个动态标题），**339 个展开注册**。
- 33 个 scenario；matrix 文件 78 个注册（12 个顶层声明 + 66 个 scenario × layout）。
- 1145 个直接 `assert.*` 调用点，其中按领域关键词判定 930 个为语义断言；循环会让实际执行次数更高。
- 处置分布（2026-11-17 集成决策后）：direct 240、rewrite 52、split 11、obsolete 36、blocked-decision 0；P5.1 后 status 曾为 33 `green`、270 `planned`、36 `retired-with-decision`。
- P5 状态回填（2026-11-18）：100 个用例登记了现有 Rust 测试；32 个 exact-parity 用例提升为 `green`，3 个 near-exact 保留 `planned` + `status_recommendation=green`，44 个 weaker、5 个 structural、16 个 blocked 仍为 `planned`。最终 **32 green / 271 planned / 36 retired-with-decision**。逐条见 §14。
- P5.1 回填（2026-11-19）：新增 14 个用例证据（114 assessed）；`TS-COMPLETION-0220` 由 P4-C adapter 测试提升为 `green`，其余 13 个保持 `planned`（7 个新增 weaker adapter、4 个 weaker definition、1 个 blocked definition、1 个 structural perf）。P5.1 后为 **33 green / 270 planned / 36 retired-with-decision**；当时仍有 189 个 planned 用例没有 Rust 证据。
- P6 回填（2026-11-19，实现 HEAD `9a8b65caac38e2d04a08550e0506408f7af967f5`）：event completion 与 trusted type-assets 的 3 个 exact 用例提升为 `green`（`TS-COMPLETION-0175`、`TS-COMPLETION-0193`、`TS-PROBES-0169`）；`TS-META-0009` 与 `TS-SERVER-0237` 登记 weaker 证据，`TS-COMPLETION-0207` 追加新的 native event-completion 证据但仍为 weaker。最终 **36 green / 267 planned / 36 retired-with-decision**；仍有 184 个 planned 用例没有 Rust 证据。逐条见 §14 和 `docs/evidence/2026-11-19-tsgo-ledger-backfill.md`。
- Wave-5 回填（2026-11-20，实现边界 HEAD `375cde24a61141f2b122bdf6ca9c23fbfc0fb1a6`）：root/trailing-dot completion、event-handler hover、native symbols 与 overlay replay 的 8 个 exact 用例提升为 `green`（`TS-COMPLETION-0127`、`TS-COMPLETION-0153`、`TS-EVENTS-0108`、`TS-SYMBOLS-0061/0150/0215/0226/0312`）；其中 7 个由无证据转为 exact，`TS-EVENTS-0108` 由 blocked 证据转为 exact。另新增 17 个 weaker、1 个 blocked、1 个 structural 证据用例，并对 14 个既有 weaker/structural/blocked 用例追加 wave-5 测试。最终 **44 green / 259 planned / 36 retired-with-decision**；仍有 158 个 planned 用例没有 Rust 证据。逐条见 §15 和 `docs/evidence/2026-11-20-tsgo-ledger-backfill-wave5.md`。
- Wave-6 回填（2026-11-20，记录 HEAD `1d5222eef18904ee056d7da97de3b011feb65cc8`）：E2E-20 component binding diagnostics、native symbols 边界与 script diagnostics routing 没有新增 exact 证据，因此不提升任何 case；新增 6 个 weaker（`TS-COMPONENT-0147/0193/0206/0229/0251/0268`）和 3 个 blocked（`TS-COMPONENT-0279`、`TS-SYMBOLS-0299/0425`），并对 `TS-ANALYSIS-0297`、`TS-COMPLETION-0104`、`TS-SYMBOLS-0116`、`TS-EVENTS-0108` 追加测试。最终仍为 **44 green / 259 planned / 36 retired-with-decision**；有证据的 planned 从 101 升到 110，无证据从 158 降到 149。逐条见 §16 和 `docs/evidence/2026-11-20-tsgo-ledger-backfill-wave6.md`。
- Wave-6 组件绑定跟进（2026-11-20，基线 HEAD `0bf666b994163d2a44a4b87b0b8bb5d4e9101bf1`）：补齐 per-tag 全矩阵、8 个 skipped pair、no-mustache literal、无 properties、path-free 遍历后，`TS-COMPONENT-0147/0206/0251/0268` 提升为 exact `green`；`TS-COMPONENT-0193` 因 Node-specific `probeProblems` 无 native 对应保持 blocked，`TS-COMPONENT-0229` 因 union 成员无结构 flag 保持 blocked。最终 **48 green / 255 planned / 36 retired-with-decision**；`exact` 48、`weaker` 74、`blocked` 22，`assessed_cases` 154 与无证据 planned 149 不变。逐条见 §17 和 `docs/evidence/2026-11-20-tsgo-ledger-wave6-component-exact.md`。
- 本轮 TS 套件实际执行：339 个，通过 338，失败 1，跳过 0。
- 计划对账：计划 273 静态 / ~338 展开，实际 **274 / 339**（+1）；原因是 `events.test.ts` 实际 24 个声明而非计划表中的 23。
- 23 个 Rust `#[ignore]` 测试全部映射到必跑原生命令；2 个 `semantic_tools` 静默 return 路径同样映射；均不计为已迁移。
- 旧 TS 全量套件在 BASE 仍是 red（338/339）：perf 在全量并行下 cold start 8325.25 ms > 8000 ms；同一测试 isolated 串行 3/3 通过（1058.25 / 1005.64 / 675.60 ms），属负载相关门禁，需 P2 前单独决定。

## 2. 基线（P0）

| 项目 | 结果 |
| --- | --- |
| 工作区 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| 分支 / HEAD | `tsgo/migration` / `b9616c557e54a06b7d756e516e6dd7c4016664cf` |
| Rust 测试 | `cargo test --workspace --all-features`：1297 passed / 0 failed / 23 ignored |
| TS 全量测试 | `npm test`：339 tests / 338 pass / 1 fail / 0 skip（仅 perf） |
| TS perf isolated | 3/3 pass；coldStart 1058.25 / 1005.64 / 675.60 ms（page-basic / in-place-shadow） |
| fmt / check / clippy | 全部 exit 0 |
| Node | v24.22.2（仅 P1 证据环境；不是最终 no-Node 门禁） |

> P0 原始命令、退出码、耗时、失败断言、夹具 hash 与并发写入观察见证据文件。

## 3. 计划计数对账

| 计数 | 计划 | 实际 | 差异 |
| --- | ---: | ---: | --- |
| TS 静态 `test()` 声明 | 273 | 274 | +1 |
| TS 展开注册 | 338（~） | 339 | +1 |
| TS 文件数 | 21 + support | 21 + support | 一致 |
| matrix 注册 | 78 | 78 | 一致 |
| Rust 属性（src/非 Windows） | 945 | 945 | 一致 |
| Rust 属性（tests） | 374 | 374 | 一致 |
| Rust 编译测试函数 | 1319 | 1319 | 一致 |
| Rust doctest | 未计 | 1 | 说明性差异 |
| proptest 定义 | 13 | 12（6 个 block） | -1 |

原始 `#[test]` 属性总数为 1322（src 948 / tests 374）：其中 3 个 `#[cfg(windows)]` 在 Linux 不编译，得到 1319 个测试函数；`cargo test --list` 另含 1 个 doctest，共 1320 条。

计划裁决：The plan's 273 static / ~338 expanded figures are wrong at BASE; actual is 274 / 339. The matrix '12+66' wording is also wrong: 13 declarations = 12 literal top-level + 1 dynamic loop-body, 78 registrations.

## 4. 文件清单

| 文件 | 声明 | 字面/动态 | 注册 | 直接断言 | 语义断言 | 分区 | 处置 |
| --- | ---: | --- | ---: | ---: | ---: | --- | --- |
| `analysis.test.ts` | 23 | 23/0 | 23 | 100 | 97 | behaviors, hover_docs, loops, project_overlay | direct 21, rewrite 2 |
| `compiler.test.ts` | 6 | 6/0 | 6 | 21 | 7 | project_overlay | rewrite 6 |
| `completion.test.ts` | 9 | 9/0 | 9 | 25 | 8 | completion | direct 9 |
| `componentBindings.test.ts` | 28 | 28/0 | 28 | 138 | 132 | behaviors, components | direct 25, rewrite 2, split 1 |
| `definitions.test.ts` | 5 | 5/0 | 5 | 14 | 13 | definitions | direct 4, rewrite 1 |
| `events.test.ts` | 24 | 24/0 | 24 | 115 | 110 | events | direct 24 |
| `evidence.test.ts` | 3 | 3/0 | 3 | 12 | 5 | messages_evidence | direct 3 |
| `guards.test.ts` | 16 | 16/0 | 16 | 61 | 53 | guards_js | direct 16 |
| `layouts.test.ts` | 12 | 12/0 | 12 | 33 | 24 | components | direct 12 |
| `locals.test.ts` | 12 | 12/0 | 12 | 46 | 33 | loops | direct 12 |
| `mapping.test.ts` | 16 | 16/0 | 16 | 76 | 59 | mapping_utf16 | direct 16 |
| `matrix.test.ts` | 13 | 12/1 | 78 | 84 | 67 | scenario_matrix | obsolete 35, direct 33, split 10 |
| `messages.test.ts` | 5 | 5/0 | 5 | 9 | 5 | messages_evidence | direct 5 |
| `meta.test.ts` | 2 | 2/0 | 2 | 9 | 3 | protocol_limits | rewrite 2 |
| `overlay.test.ts` | 5 | 5/0 | 5 | 17 | 14 | project_overlay | direct 5 |
| `perf.test.ts` | 1 | 1/0 | 1 | 5 | 3 | perf_budget | rewrite 1 |
| `probes.test.ts` | 10 | 10/0 | 10 | 30 | 10 | guards_js, project_overlay | direct 10 |
| `protocol.test.ts` | 30 | 30/0 | 30 | 138 | 101 | protocol_limits | obsolete 1, rewrite 29 |
| `rewrite.test.ts` | 24 | 24/0 | 24 | 104 | 94 | mapping_utf16, shadow_rewrite | direct 24 |
| `server.test.ts` | 9 | 9/0 | 9 | 31 | 20 | protocol_limits | rewrite 9 |
| `symbols.test.ts` | 21 | 21/0 | 21 | 77 | 72 | symbols_edits | direct 21 |
| **合计** | **274** | **273/1** | **339** | **1145** | **930** | | |

`support.ts`：`9b4fcb34…ef36`，无 `test()` 声明；导出 `analyzeRequest`、`fragment`、`template`、`byteIndexOf`、`fragmentDiagnostics` 等内存请求构造器。

## 5. 展开模型

- `matrix.test.ts:102` 的 ``test(`${spec.id} / ${layout}`)`` 在 33 个 scenario × 2 个 layout 上注册 66 个用例；其余 12 个顶层声明各注册 1 个，共 78。
- `matrix.test.ts:92` 的“every scenario runs both candidates”在一个测试体内调用 `runScenario` 66 次，注册数仍为 1，执行次数为 66。
- 内部参数化未单独注册为 test（如 `locals.test.ts:261` 的 `LOCAL_CASES × 8`、`protocol.test.ts:606` 的 24 个非法变更、`componentBindings.test.ts:147` 的 8 个 skipped 检查），已记入 `inner_expansions`；迁移时必须逐子用例建 ID。
- 逐断言的执行次数会因循环放大；账本中的 assertion count 是静态调用点。

## 6. 夹具 hash 与覆盖维度

### 6.1 夹具 hash（v1，已验证）

算法 v1：regular files only（symlink 直接失败）；root-relative path 使用 `/`；按路径 UTF-8 字节排序；对每个文件累加 `rel_path_utf8 + NUL + raw bytes + NUL`；lowercase SHA-256。脚本 `b54fcad3…`，两次运行输出完全一致，0 个 symlink。

| 树 | 文件数 | 字节 | SHA-256 (v1) |
| --- | ---: | ---: | --- |
| `backends/typescript/test` | 22 | 253965 | `630a16af41623c78f19f47bcaba1a7f1282201ac27b4414115ac0b3764afc32e` |
| `backends/typescript/samples` | 35 | 6069 | `00a7fa3d6962da50581c396f73c13f7ffb043efae060a0801b373928726e5c1f` |
| `tests/fixtures` | 136 | 61928 | `7c9ddedda18c70bc217af6b3bbd23dd22a33be9c885276cbd587adc0596f04a9` |
| `crates/wxml-lsp/tests/fixtures` | 43 | 6456 | `834eea4defcb7565c4b84e91a173079cf3be85201d44fb4a7a6d0a32af3da073` |

计划 hash 陈旧/未定义，已不再沿用；本表是唯一验证值。

### 6.2 覆盖维度

`covers` 由标题、断言文本与 scenario 元数据生成；`recovery` 包含 degraded/partial/unavailable/fallback 等降级路径，`security` 包含路径/泄漏/symlink/读边界/拒绝路径。dirty 增量 D1–D7 单独映射。

| 维度 | 用例数 | 说明 |
| --- | ---: | --- |
| `loops` | 25 | wx:for / locals / 嵌套循环 / 遮蔽 |
| `guards` | 65 | wx:if/elif、truthiness、条件收窄、JS pragma/checkJs |
| `behavior` | 28 | Behavior 合成、spread、别名、缺失行为 |
| `props` | 66 | properties/optionalTypes、属性绑定、usingComponents、spread |
| `events` | 25 | 事件 handler、绑定形式、签名/文档、缺失 handler |
| `symbols` | 19 | symbols、setData、observer、references、destructuring |
| `rename` | 24 | rename/WorkspaceEdit、插入、编辑拒绝 |
| `unsaved` | 22 | unsaved overlay、snapshot/revision、跨文件未保存 |
| `utf16` | 38 | UTF-16/UTF-8、astral、CRLF、BOM、byte range |
| `protocol` | 106 | C1/JSON-RPC、limits、budget、server、validation |
| `recovery` | 136 | restart/stale/degraded/partial/unavailable/fallback |
| `security` | 37 | 路径/sanitizer/symlink/读边界/拒绝路径 |

## 7. 迁移处置与 oracle

| 处置 | 数量 | 含义 |
| --- | ---: | --- |
| direct | 240 | 同一行为断言可直接落到选定的原生架构；不得弱化。 |
| rewrite | 52 | 旧机制被替换（Node LanguageService/C1 信封/pool/transport），保留断言语义，改写为原生合同测试；含 TS-META-0027 → F0 version pin。 |
| split | 11 | 选定布局断言直接迁移；candidate B（symbol-bridge）部分作为历史证据拆分处理。 |
| blocked-decision | 0 | 已全部由集成负责人裁决，无剩余。 |
| obsolete | 36 | Candidate B/symbol-bridge 为内部实验布局，LSP 不可选择；生产使用原位 shadow。历史证据保留，原位 shadow 用例仍为强制。 |

集成决策：36 个 Candidate B/symbol-bridge 用例（33 个 matrix + TS-MATRIX-0197/0205 + TS-PROTOCOL-0463）标记 `obsolete`，因为 `server.ts:152` 生产调用不传 layoutId，`analysis.ts:59-60,416` 默认原位 shadow；它们仅由 matrix/perf/evidence 脚本遍历。原位 shadow 用例、协议验证、映射、诊断和编辑安全用例仍为强制。TS-META-0027 标记 `rewrite`，由 F0 原生版本/类型资产 pin 测试替代。

| Oracle | 数量 | 用途 |
| --- | ---: | --- |
| assertion | 125 | 直接行为断言（类型、文档、范围、诊断、编辑）。 |
| unit | 114 | 纯函数/codec/验证：locals、guards、rewrite、mapping math、protocol validation。 |
| parity | 100 | scenario matrix、overlay、mapping/run、性能采样。 |

48 条 `status=green`、255 条 `status=planned`、36 条 `status=retired-with-decision`；`oracle_revision` 已冻结为 `F2 frozen 2026-11-17 (dcc1f15)`；独立 `reviewer` 尚未签署。P5/P5.1/P6/wave-5/wave-6 只提升 exact-parity 用例；near-exact/weaker/structural/blocked 的逐条证据和状态见 §14/§15/§16/§17。

## 8. Dirty 必保增量（D1–D7）

来源：`4d59ca7 fix: resolve component targets and improve WXML hover metadata`，19 个文件，1606 insertions / 563 deletions。

| ID | 行为 | TS 账本用例 | Rust 证据 | 必跑门禁 |
| --- | --- | --- | --- | --- |
| D1 | Behavior data spreads expand to boolean/Supported | TS-ANALYSIS-0380 | — | P4 slice B + native behavior parity: spread-derived isFlag must be boolean and hover Supported; missing native checker result must fail, not degrade silently. |
| D2 | Aliased Behavior handlers resolve to the declaring file and keep Chinese docs; unresolved alias is Partial, never fabricated missing-handler | TS-EVENTS-0188, TS-EVENTS-0236, TS-EVENTS-0425, TS-EVENTS-0461, TS-EVENTS-0498 | `backends/typescript/src/behaviors.ts behavior-method source file/path fields; events.ts event result definition/documentation` | P4 slice B/D: alias behavior definition must point at the behavior file; Chinese documentation preserved byte-for-byte; unresolved alias Partial with no diagnostic; no native result may claim missing-handler. |
| D3 | Custom typeRoots do not evict the pinned mini-program declarations | TS-COMPILER-0132, TS-ANALYSIS-0093 | — | P3/P4: native program with a project typeRoots must still resolve Component and pinned globals; missing pinned asset must fail the gate. |
| D4 | Hover with valid content does not append degradation text | 无（Rust-only） | `crates/wxml-lsp/src/handlers/hover.rs:639 hover_notes_make_degradation_explicit, new assertion at :662 for documented Partial content` | P4 slice C: decorated_contents must return non-empty backend text unchanged; degradation notes only for empty results. |
| D5 | Chinese metadata, official links and enumerated values | 无（Rust-only） | `crates/wxml-lsp/src/metadata/tests.rs:328 builtin_tag_docs_link_to_the_official_page`<br>`crates/wxml-lsp/src/metadata/tests.rs:352 enumerated_values_are_a_bullet_list`<br>`crates/wxml-lsp/src/metadata/tests.rs:122 builtin_tags_attributes_and_values_have_static_docs (updated)`<br>`crates/wxml-lsp/src/server.rs:1302 (视图容器 hover assertion)` | P4 slice C: Chinese tag/attribute docs, `官方文档` URL links, and bullet-list 可选值 survive native hover adaptation; no English-only regression. |
| D6 | Unknown attribute routing (complete facts go to the backend; incomplete facts get a short hint) | 无（Rust-only） | `crates/wxml-lsp/src/metadata/tests.rs:370 complete_component_facts_leave_unknown_attributes_to_the_backend`<br>`crates/wxml-lsp/src/metadata/tests.rs:230 incomplete_component_properties_are_not_reported_as_empty (updated)` | P4 slice C: complete facts -> NotStatic (backend answers); incomplete facts -> 暂无属性说明; no fabricated 'cannot verify property' text. |
| D7 | Root component fallback for root-absolute specifiers outside the app directory | 无（Rust-only） | `crates/wxml-project/src/discovery.rs:2010 retargets_root_components_kept_outside_the_app_directory`<br>`crates/wxml-project/src/discovery.rs:2057 retargets_root_components_declared_by_a_component_json`<br>`crates/wxml-project/src/discovery.rs:2087 keeps_app_directory_components_when_the_same_path_also_exists_at_the_root` | P4 slice B: retarget only when the app-relative WXML is absent; app-directory path wins when both exist; no discovery issue for a resolved root component. |

D4–D7 没有对应 TS 测试，但有现存 Rust 测试；迁移时必须把这些断言接到原生引擎门禁，不能以“旧 TS 没测”为由删除。

## 9. Rust ignored 测试与静默跳过

### 9.1 23 个 `#[ignore]`

| Gate | 文件:测试 | 目标门禁 | 关联账本用例 |
| --- | --- | --- | --- |
| G-P5-PERF-01 | `crates/wxml-lsp/tests/performance_bench.rs:q2_performance_benchmark` | P5 performance budget (native engine): cold/hot p50/p95/p99, CPU/RSS, 10-round growth, per plan section 5. | — |
| G-P5-STABILITY-01 | `crates/wxml-lsp/tests/q2_stability.rs:ten_round_lifecycle_stability` | P5 recovery/stability: 10 open/edit/close rounds, no monotonic RSS/process growth, no stale diagnostics. | — |
| G-P5-E2E-01 | `crates/wxml-lsp/tests/real_node.rs:non_owner_script_overlay_propagates_and_survives_a_node_restart` | P5 real-native E2E: unsaved non-owner script overlay propagates, survives engine restart, no stale results. | TS-OVERLAY-0055, TS-ANALYSIS-0161 |
| G-P5-E2E-02 | `crates/wxml-lsp/tests/real_node.rs:real_node_event_handler_diagnostics_definition_and_completion` | P5 real-native E2E: event handler hover/diagnostic/definition/completion against a real project. | TS-EVENTS-0071, TS-EVENTS-0143, TS-COMPLETION-0175 |
| G-P5-E2E-03 | `crates/wxml-lsp/tests/real_node.rs:real_node_expression_and_event_completion_smoke` | P5 real-native E2E: expression and event completion smoke on a real project. | TS-COMPLETION-0078, TS-COMPLETION-0175 |
| G-P5-E2E-04 | `crates/wxml-lsp/tests/real_node.rs:real_node_hover_and_diagnostics_smoke` | P5 real-native E2E: hover type/docs and diagnostics on a real project. | TS-ANALYSIS-0033, TS-MATRIX-0102-PAGE_BASIC-IN_PLACE_SHADOW |
| G-P5-E2E-05 | `crates/wxml-lsp/tests/real_node.rs:real_node_symbols_smoke` | P5 real-native E2E: symbols request over a real project with exact byte ranges. | TS-SYMBOLS-0061, TS-SYMBOLS-0116 |
| G-P5-E2E-06 | `crates/wxml-lsp/tests/stdio_type_loop.rs:ac001_page_data_and_jsdoc_hover` | P5 real-native E2E: Page data + JSDoc hover through the public LSP entry point. | TS-ANALYSIS-0033, TS-ANALYSIS-0054 |
| G-P5-E2E-07 | `crates/wxml-lsp/tests/stdio_type_loop.rs:ac002_field_typo_diagnostic_and_fix_clear` | P5 real-native E2E: field typo diagnostic appears and clears after fix (versioned publication). | TS-ANALYSIS-0033, TS-MATRIX-0102-PAGE_BASIC-IN_PLACE_SHADOW |
| G-P5-E2E-08 | `crates/wxml-lsp/tests/stdio_type_loop.rs:ac003_unicode_and_crlf_positions` | P5 real-native E2E: Unicode + CRLF position mapping through LSP UTF-16 to source bytes. | TS-MAPPING-0010, TS-MAPPING-0145, TS-EVENTS-0130 |
| G-P5-E2E-09 | `crates/wxml-lsp/tests/stdio_type_loop.rs:ac011_unknown_owner_degrades_honestly` | P5 real-native E2E: unknown owner degrades with a reason, no fabricated result. | TS-COMPLETION-0207, TS-ANALYSIS-0297 |
| G-P5-E2E-10 | `crates/wxml-lsp/tests/stdio_type_loop.rs:ac013_component_properties_hover` | P5 real-native E2E: component property hover type/docs over the public LSP entry point. | TS-COMPONENT-0147, TS-MATRIX-0102-COMPONENT_PROPERTIES-IN_PLACE_SHADOW |
| G-P5-E2E-11 | `crates/wxml-lsp/tests/stdio_type_loop.rs:component_behavior_data_and_properties_work_over_stdio` | P5 real-native E2E: Behavior data/properties join the component instance type over stdio. | TS-COMPONENT-0292, TS-COMPONENT-0357, TS-COMPONENT-0416 |
| G-P5-E2E-12 | `crates/wxml-lsp/tests/stdio_type_loop.rs:static_wxs_and_template_semantics_work_over_stdio` | P5 real-native E2E: WXS static semantics and template semantics through the public LSP entry point. | — |
| G-P5-E2E-13 | `crates/wxml-lsp/tests/stdio_type_loop.rs:static_wxs_answers_survive_an_unavailable_type_backend` | P5 recovery: static WXS answers survive an unavailable type backend (honest degradation, no empty success). | TS-COMPLETION-0207, TS-ANALYSIS-0358 |
| G-P5-E2E-14 | `crates/wxml-lsp/tests/stdio_type_loop.rs:unsaved_wxs_overlay_updates_static_hover_and_completion` | P5 real-native E2E: unsaved WXS overlay updates static hover/completion without disk writes. | TS-OVERLAY-0023, TS-ANALYSIS-0161 |
| G-P5-E2E-15 | `crates/wxml-lsp/tests/wave5_acceptance.rs:ac005_loop_scope_typed_locals` | P5 real-native E2E: wx:for locals typed in loop scope. | TS-LOCALS-0261, TS-GUARDS-0174 |
| G-P5-E2E-16 | `crates/wxml-lsp/tests/wave5_acceptance.rs:ac006_conditional_union_narrowing` | P5 real-native E2E: wx:if/elif conditional union narrowing. | TS-GUARDS-0097, TS-GUARDS-0128 |
| G-P5-E2E-17 | `crates/wxml-lsp/tests/wave5_acceptance.rs:ac008_alias_project_hover_and_definition` | P5 real-native E2E: alias project hover/definition with project-relative ids. | TS-PROTOCOL-0450, TS-MATRIX-0102-ALIAS_PROJECT-IN_PLACE_SHADOW |
| G-P5-E2E-18 | `crates/wxml-lsp/tests/wave5_acceptance.rs:ac008_multi_view_fixture_isolation` | P5 project isolation: two views/fixtures do not share types or diagnostics. | TS-ANALYSIS-0161, TS-PROTOCOL-0450 |
| G-P5-E2E-19 | `crates/wxml-lsp/tests/wave5_acceptance.rs:ac008_simultaneous_views_do_not_pollute_each_other` | P5 project isolation: simultaneous views do not pollute each other's results. | TS-ANALYSIS-0161 |
| G-P5-E2E-20 | `crates/wxml-lsp/tests/wave5_acceptance.rs:ac012_component_property_bindings` | P5 real-native E2E: component property bindings produce per-tag diagnostics. | TS-COMPONENT-0147, TS-COMPONENT-0206 |
| G-P5-E2E-21 | `crates/wxml-lsp/tests/wave5_acceptance.rs:ac012_event_handlers_basic` | P5 real-native E2E: basic event handler diagnostics/definition/completion. | TS-EVENTS-0071, TS-EVENTS-0143 |

### 9.2 静默跳过 / 可选检查

| 来源 | 测试 | 当前行为 | 原生替换测试 | 缺引擎必须失败 |
| --- | --- | --- | --- | --- |
| `crates/wxml-lsp/tests/semantic_tools.rs:2081` | real_node_lsp_component_property_edit_applies_and_parses | silent return when dist/server.js is absent; optional tsc post-check | `crates/wxml-lsp/tests/native_parity/symbols_edits.rs::native_component_property_edit_applies_and_reparses` | True |
| `crates/wxml-lsp/tests/semantic_tools.rs:2568` | real_node_lsp_rejects_cross_namespace_and_dynamic_edits | silent return when dist/server.js is absent | `crates/wxml-lsp/tests/native_parity/symbols_edits.rs::native_rejects_cross_namespace_and_dynamic_edits` | True |
| `crates/wxml-lsp/tests/semantic_tools.rs:2169` | real_node_lsp_component_property_edit_applies_and_parses (post-edit check) | If node_modules/typescript/bin/tsc is absent, prints 'tsc not install… | `crates/wxml-lsp/tests/native_parity/symbols_edits.rs::assert_native_edit_reparses_and_typechecks` | True |
| `backends/typescript/test/meta.test.ts:27` | the C1 contract documents the pinned versions | context.skip when docs/reference/type-bridge-c1.md is absent (backend… | — | — |
| `backends/typescript/test/evidence.test.ts:25` | evidence output must stay inside the repository layout | context.skip when docs/reference/type-bridge-c1.md is absent. | — | — |
| `backends/typescript/test/evidence.test.ts:50` | the committed evidence contains no absolute paths | context.skip when docs/evidence/data/type-bridge-c1/matrix.json is ab… | — | — |

`cargo test` 的 23 ignored 与 0 failed 不等于真实引擎通过；上述所有路径在最终门禁中必须 fail-closed。

## 10. 关键差异与开放决策

| ID | 严重度 | 结论 | 动作 |
| --- | --- | --- | --- |
| X1 | high | Plan says 273 static test() calls (272 literal + 1 dynamic) and ~338 expanded registrations; actual is 274 static decla… | P1-B ledger uses 274/339. P2/F2 must freeze the ledger count, not the plan's 273/338. |
| X2 | medium | The full TS suite is red at BASE (338/339), but the perf failure is not reproducible in isolation: 3 serial perf-only r… | Record both facts; do not change the 8000 ms threshold or edit the test. Keep BASE full suite marked red as-run. P2 mus… |
| X3 | high | The plan's four fixture hashes were stale/undefined and are not used. One v1 algorithm now yields deterministic hashes … | Use the v1 hashes in docs/plans/tsgo-test-ledger.json fixture_trees. Do not copy the plan values forward. Recompute and… |
| X4 | medium | Plan says 1,319 test attributes, src 945, tests 374, 13 proptest definitions; raw grep finds 1,322 attributes (src 948,… | P0 evidence records both raw and compiled counts. P2/F2 should use compiled test functions plus the 12 property functio… |
| X5 | high | Two Rust LSP tests silently return when backends/typescript/dist/server.js is missing, and a post-edit typecheck is opt… | Ledger silent_skips maps both plus the tsc branch to mandatory P5 real-native gates. No final gate may return green wit… |
| X6 | medium | Three TS tests conditionally skip when the docs tree/evidence file is absent. | Port to Rust evidence/contract tests that require the fixture and fail when absent. |
| X7 | medium | 23 Rust tests are #[ignore]; none is a migrated or passing real-engine gate. | Ledger ignored_rust_tests maps each to a required native gate (G-P5-*); none may remain #[ignore] in the final no-Node … |
| X8 | medium | The matrix, protocol 463 and some component tests encode candidate B (symbol-bridge) behavior. Code inspection shows production does not select it. | Resolved 2026-11-17: 36 candidate-B cases are `obsolete`; in-place-shadow and all mandatory semantic/safety cases remain. |
| X9 | medium | The P0 TS baseline is reproducible only because Node v24.22.2 and a copied node_modules are present; this is a P1 evide… | Never count this run as NF-01/NF-02 evidence. P5/P7 must run native gates with Node unavailable. |
| X10 | low | The plan's '19 dirty paths' snapshot is no longer dirty; the same 19 files are commit 4d59ca7, which is included in BAS… | Dirty increments D1-D7 are mapped in the ledger from this commit; no uncommitted behavior is at risk of deletion at BAS… |
| X11 | low | Three untracked evidence docs appeared during P1-B from other agents; the worktree is not exclusively owned. | P0 records both snapshots. P1-B writes only its three allowed files. P2 must re-check BASE/dirty state before freezing … |
| X12 | low | P0 used the task-specified commands, which differ slightly from plan section 7 (--locked/--all-targets/doc/doc-lint/cov… | Do not claim the plan's full command matrix passed. P5 must run it; record missing tools if absent. |

原待确认决策（2026-11-17 已裁决）：

1. **candidate B（symbol-bridge，33 个 matrix case + matrix 197/205 + protocol 463）**：裁决为 `obsolete`。代码证据：`analysis.ts:59-60` 注释“production uses the default in-place shadow”、`:416` 默认、`server.ts:152` 不传 layoutId；`cli.ts:17` / `scripts/emit-evidence.ts:20` 只在 matrix/perf/evidence 中同时跑两个 layout。历史证据保留，原位 shadow 用例仍强制。
2. **旧 C1 合同文档测试**：`meta.test.ts:27` 裁决为 `rewrite`，替换为 F0 引擎/类型资产 pin 测试（`F0-version-pin`）。
3. **性能门禁**：裁决采用专用串行 `perf.test.ts` 作为权威门禁；全量并行 perf 结果（8325.25 ms）保留为已知环境记录，不作为迁移失败。阈值 8000 ms 与测试内容不变。
4. **夹具 hash 约定**：冻结 v1 算法（sorted UTF-8 relative path + NUL + bytes + NUL，regular files only）；计划旧 hash 不再沿用。
5. **新增分区**：批准 `shadow_rewrite.rs`、`mapping_utf16.rs`、`perf_budget.rs` 作为 13 个分区之外的独立测试文件；每个文件单一 owner，不得复制用例。
6. **纯 codec 用例落点**：账本统一给出 `native_parity/*.rs` 目标；P2 可按计划把纯 emitter/map 断言移到生产模块旁，case ID 不变。

## 11. 用例索引

### 11.1 matrix 展开（66）

| Scenario | in-place-shadow | symbol-bridge |
| --- | --- | --- |
| `page-basic` | `TS-MATRIX-0102-PAGE_BASIC-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-PAGE_BASIC-SYMBOL_BRIDGE` (obsolete) |
| `page-export-empty` | `TS-MATRIX-0102-PAGE_EXPORT_EMPTY-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-PAGE_EXPORT_EMPTY-SYMBOL_BRIDGE` (obsolete) |
| `page-cjs` | `TS-MATRIX-0102-PAGE_CJS-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-PAGE_CJS-SYMBOL_BRIDGE` (obsolete) |
| `page-esm` | `TS-MATRIX-0102-PAGE_ESM-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-PAGE_ESM-SYMBOL_BRIDGE` (obsolete) |
| `page-closure` | `TS-MATRIX-0102-PAGE_CLOSURE-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-PAGE_CLOSURE-SYMBOL_BRIDGE` (obsolete) |
| `component-properties` | `TS-MATRIX-0102-COMPONENT_PROPERTIES-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-COMPONENT_PROPERTIES-SYMBOL_BRIDGE` (obsolete) |
| `component-module` | `TS-MATRIX-0102-COMPONENT_MODULE-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-COMPONENT_MODULE-SYMBOL_BRIDGE` (obsolete) |
| `component-js` | `TS-MATRIX-0102-COMPONENT_JS-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-COMPONENT_JS-SYMBOL_BRIDGE` (obsolete) |
| `properties-spread` | `TS-MATRIX-0102-PROPERTIES_SPREAD-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-PROPERTIES_SPREAD-SYMBOL_BRIDGE` (obsolete) |
| `multi-registration` | `TS-MATRIX-0102-MULTI_REGISTRATION-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-MULTI_REGISTRATION-SYMBOL_BRIDGE` (obsolete) |
| `js-jsdoc-pragma` | `TS-MATRIX-0102-JS_JSDOC_PRAGMA-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-JS_JSDOC_PRAGMA-SYMBOL_BRIDGE` (obsolete) |
| `js-jsdoc-off` | `TS-MATRIX-0102-JS_JSDOC_OFF-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-JS_JSDOC_OFF-SYMBOL_BRIDGE` (obsolete) |
| `js-checkjs` | `TS-MATRIX-0102-JS_CHECKJS-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-JS_CHECKJS-SYMBOL_BRIDGE` (obsolete) |
| `ts-nocheck` | `TS-MATRIX-0102-TS_NOCHECK-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-TS_NOCHECK-SYMBOL_BRIDGE` (obsolete) |
| `js-nocheck` | `TS-MATRIX-0102-JS_NOCHECK-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-JS_NOCHECK-SYMBOL_BRIDGE` (obsolete) |
| `js-shebang-bom` | `TS-MATRIX-0102-JS_SHEBANG_BOM-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-JS_SHEBANG_BOM-SYMBOL_BRIDGE` (obsolete) |
| `interface-root` | `TS-MATRIX-0102-INTERFACE_ROOT-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-INTERFACE_ROOT-SYMBOL_BRIDGE` (obsolete) |
| `interface-root-import` | `TS-MATRIX-0102-INTERFACE_ROOT_IMPORT-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-INTERFACE_ROOT_IMPORT-SYMBOL_BRIDGE` (obsolete) |
| `nullable` | `TS-MATRIX-0102-NULLABLE-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-NULLABLE-SYMBOL_BRIDGE` (obsolete) |
| `cross-file-overlay` | `TS-MATRIX-0102-CROSS_FILE_OVERLAY-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-CROSS_FILE_OVERLAY-SYMBOL_BRIDGE` (obsolete) |
| `alias-project` | `TS-MATRIX-0102-ALIAS_PROJECT-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-ALIAS_PROJECT-SYMBOL_BRIDGE` (obsolete) |
| `component-data-cross-file` | `TS-MATRIX-0102-COMPONENT_DATA_CROSS_FILE-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-COMPONENT_DATA_CROSS_FILE-SYMBOL_BRIDGE` (obsolete) |
| `crlf-script` | `TS-MATRIX-0102-CRLF_SCRIPT-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-CRLF_SCRIPT-SYMBOL_BRIDGE` (obsolete) |
| `data-shorthand` | `TS-MATRIX-0102-DATA_SHORTHAND-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-DATA_SHORTHAND-SYMBOL_BRIDGE` (obsolete) |
| `js-source-check-off` | `TS-MATRIX-0102-JS_SOURCE_CHECK_OFF-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-JS_SOURCE_CHECK_OFF-SYMBOL_BRIDGE` (obsolete) |
| `js-source-check-inherit` | `TS-MATRIX-0102-JS_SOURCE_CHECK_INHERIT-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-JS_SOURCE_CHECK_INHERIT-SYMBOL_BRIDGE` (obsolete) |
| `methods-nonliteral` | `TS-MATRIX-0102-METHODS_NONLITERAL-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-METHODS_NONLITERAL-SYMBOL_BRIDGE` (obsolete) |
| `unchecked-index` | `TS-MATRIX-0102-UNCHECKED_INDEX-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-UNCHECKED_INDEX-SYMBOL_BRIDGE` (obsolete) |
| `nullable-no-strict-null` | `TS-MATRIX-0102-NULLABLE_NO_STRICT_NULL-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-NULLABLE_NO_STRICT_NULL-SYMBOL_BRIDGE` (obsolete) |
| `no-implicit-this-off` | `TS-MATRIX-0102-NO_IMPLICIT_THIS_OFF-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-NO_IMPLICIT_THIS_OFF-SYMBOL_BRIDGE` (obsolete) |
| `js-bom-check-off` | `TS-MATRIX-0102-JS_BOM_CHECK_OFF-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-JS_BOM_CHECK_OFF-SYMBOL_BRIDGE` (obsolete) |
| `js-bom-check-inherit` | `TS-MATRIX-0102-JS_BOM_CHECK_INHERIT-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-JS_BOM_CHECK_INHERIT-SYMBOL_BRIDGE` (obsolete) |
| `unicode-crlf` | `TS-MATRIX-0102-UNICODE_CRLF-IN_PLACE_SHADOW` (direct) | `TS-MATRIX-0102-UNICODE_CRLF-SYMBOL_BRIDGE` (obsolete) |

### 11.2 其余用例

#### `analysis.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-ANALYSIS-0033` | 33 | analyze returns hover, diagnostic and definition for a page | 9 | direct | assertion | hover_docs | — |
| `TS-ANALYSIS-0054` | 54 | hover candidates cover every identifier in the expression | 8 | direct | assertion | hover_docs | — |
| `TS-ANALYSIS-0078` | 78 | tsconfig lib and target spellings load the core libraries | 4 | direct | assertion | hover_docs | — |
| `TS-ANALYSIS-0093` | 93 | typeRoots declarations are readable without project files on disk | 4 | direct | assertion | hover_docs | D3 |
| `TS-ANALYSIS-0123` | 123 | a page route may be registered with Component instead of Page | 5 | direct | assertion | hover_docs | — |
| `TS-ANALYSIS-0139` | 139 | declared property types survive an unresolved default value | 3 | direct | assertion | hover_docs | — |
| `TS-ANALYSIS-0161` | 161 | analyze uses the unsaved file text from the request (no disk read) | 4 | direct | assertion | hover_docs | — |
| `TS-ANALYSIS-0174` | 174 | analyze never reads project files or tsconfig from the project root | 2 | direct | assertion | hover_docs | — |
| `TS-ANALYSIS-0184` | 184 | target selects the reported token | 3 | direct | assertion | hover_docs | — |
| `TS-ANALYSIS-0196` | 196 | target must be a whole identifier in the expression | 1 | direct | assertion | hover_docs | — |
| `TS-ANALYSIS-0205` | 205 | locals mount the expression without a root prefix | 12 | direct | assertion | loops | — |
| `TS-ANALYSIS-0226` | 226 | component locals keep overlay definitions mapped to the original script | 6 | direct | assertion | loops | — |
| `TS-ANALYSIS-0243` | 243 | off + locals keeps diagnostics Disabled and the rest Partial | 5 | direct | assertion | loops | — |
| `TS-ANALYSIS-0255` | 255 | the default target is the rightmost identifier | 5 | direct | assertion | loops | — |
| `TS-ANALYSIS-0272` | 272 | insertions carry viewRevision and backendFileVersion | 5 | rewrite | assertion | project_overlay | — |
| `TS-ANALYSIS-0287` | 287 | a zero-fragment view still reports its real insertions and module kind | 3 | rewrite | assertion | project_overlay | — |
| `TS-ANALYSIS-0297` | 297 | an unmapped in-mount diagnostic downgrades diagnostics, not hover | 4 | direct | assertion | project_overlay | — |
| `TS-ANALYSIS-0315` | 315 | a dropped definition downgrades only the definition capability | 3 | direct | assertion | project_overlay | — |
| `TS-ANALYSIS-0332` | 332 | one expression may appear under two fragment ids | 4 | direct | assertion | project_overlay | — |
| `TS-ANALYSIS-0349` | 349 | duplicate fragment ids are a structured error | 1 | direct | assertion | project_overlay | — |
| `TS-ANALYSIS-0358` | 358 | a missing snapshot file is Unavailable, never read from disk | 5 | direct | assertion | project_overlay | — |
| `TS-ANALYSIS-0371` | 371 | a malformed request is rejected before analysis | 1 | direct | assertion | project_overlay | — |
| `TS-ANALYSIS-0380` | 380 | behavior data spreads are expanded through the checker | 3 | direct | assertion | behaviors | D1 |

#### `compiler.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-COMPILER-0035` | 35 | the same shape reuses one project and refreshes text in place | 6 | rewrite | assertion | project_overlay | — |
| `TS-COMPILER-0050` | 50 | a different file set or compiler option set gets a fresh project | 2 | rewrite | assertion | project_overlay | — |
| `TS-COMPILER-0063` | 63 | nested compiler options such as paths are part of the key | 3 | rewrite | assertion | project_overlay | — |
| `TS-COMPILER-0086` | 86 | the pool is bounded | 1 | rewrite | assertion | project_overlay | — |
| `TS-COMPILER-0095` | 95 | a path-only change resolves the new alias instead of the old project | 3 | rewrite | assertion | project_overlay | — |
| `TS-COMPILER-0132` | 132 | project typeRoots still load the pinned mini program typings | 6 | rewrite | assertion | project_overlay | D3 |

#### `completion.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-COMPLETION-0066` | 66 | complete rejects malformed query points | 0 | direct | assertion | completion | — |
| `TS-COMPLETION-0078` | 78 | expression member completion lists data fields of a page | 4 | direct | assertion | completion | — |
| `TS-COMPLETION-0104` | 104 | expression member completion replaces the partial member prefix | 3 | direct | assertion | completion | — |
| `TS-COMPLETION-0127` | 127 | bare root completion lists data keys | 4 | direct | assertion | completion | — |
| `TS-COMPLETION-0153` | 153 | empty mustache completes data keys | 2 | direct | assertion | completion | — |
| `TS-COMPLETION-0175` | 175 | event handler completion lists page methods | 5 | direct | assertion | completion | — |
| `TS-COMPLETION-0193` | 193 | event handler completion lists component methods | 2 | direct | assertion | completion | — |
| `TS-COMPLETION-0207` | 207 | an unavailable layout degrades to unsupported without throwing | 3 | direct | assertion | completion | — |
| `TS-COMPLETION-0220` | 220 | completion items are capped and report truncation | 2 | direct | assertion | completion | — |

#### `componentBindings.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-COMPONENT-0147` | 147 | component bindings produce per-tag diagnostics and count skipped checks | 21 | direct | assertion | components | — |
| `TS-COMPONENT-0193` | 193 | a fully checkable component request stays Supported | 5 | direct | assertion | components | — |
| `TS-COMPONENT-0206` | 206 | literal bindings are checked even when the template has no mustache | 7 | direct | assertion | components | — |
| `TS-COMPONENT-0229` | 229 | optional data members may be absent for Array/Object bindings | 7 | direct | assertion | components | — |
| `TS-COMPONENT-0251` | 251 | a component with no properties member is skipped, not guessed | 3 | direct | assertion | components | — |
| `TS-COMPONENT-0268` | 268 | component diagnostics never leak a file path | 2 | direct | assertion | components | — |
| `TS-COMPONENT-0279` | 279 | symbol-bridge component messages are path-free as well | 4 | rewrite | assertion | components | — |
| `TS-COMPONENT-0292` | 292 | behavior properties and kebab/camel bindings are checked | 9 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0357` | 357 | inline Behavior data and properties join the component instance type | 10 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0403` | 403 | component's own data wins over a behavior field with the same name | 2 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0416` | 416 | imported behavior default export contributes data and properties | 9 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0452` | 452 | nested behaviors apply before their owner and later behaviors win | 3 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0476` | 476 | behavior cycles and factory calls degrade with a reason, not a crash | 4 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0498` | 498 | spread and dynamic data members are reported and stay any | 2 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0514` | 514 | behavior type inference is conservative for literals | 1 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0521` | 521 | named behavior imports and re-export aliases resolve statically | 2 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0539` | 539 | a behavior data member initialized from a local const keeps its checked type | 2 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0550` | 550 | JavaScript behavior data uses JSDoc casts under checkJs | 3 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0570` | 570 | unresolved behavior does not fabricate 2339 in either layout | 5 | split | assertion | behaviors | — |
| `TS-COMPONENT-0591` | 591 | CommonJS require behavior resolves statically | 3 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0607` | 607 | non-literal behavior data/properties mark the view Partial | 5 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0628` | 628 | behavior data keys that cannot be probe locals never break the probe | 6 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0654` | 654 | a Page and Component in one file use the selected registration's behaviors | 5 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0676` | 676 | an unresolved external behavior keeps the component's own property checks | 3 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0697` | 697 | behaviorIncomplete suppresses only unknown behavior roots, not known-root typos | 4 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0724` | 724 | unrelated import-equals does not hijack behavior binding resolution | 6 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0763` | 763 | behavior data names with a non-probe double underscore remain usable | 2 | direct | assertion | behaviors | — |
| `TS-COMPONENT-0774` | 774 | symbol-bridge does not report resolved behavior roots as missing | 3 | rewrite | assertion | behaviors | — |

#### `definitions.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-DEFINITIONS-0013` | 13 | a definition target in a .d.ts file is an external snapshot file | 3 | direct | assertion | definitions | — |
| `TS-DEFINITIONS-0036` | 36 | overlay-generated members map back to `properties.<name>` | 4 | direct | assertion | definitions | — |
| `TS-DEFINITIONS-0052` | 52 | every query definition is classified and maps to original bytes | 3 | direct | assertion | definitions | — |
| `TS-DEFINITIONS-0074` | 74 | a definition inside the derived query file is dropped, never published | 2 | direct | assertion | definitions | — |
| `TS-DEFINITIONS-0093` | 93 | TypeScript uses the same `.d.ts` target through the language service API | 2 | rewrite | assertion | definitions | — |

#### `events.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-EVENTS-0071` | 71 | page method shorthand, function property and arrow property resolve | 8 | direct | assertion | events | — |
| `TS-EVENTS-0093` | 93 | component methods shorthand and arrow property resolve | 5 | direct | assertion | events | — |
| `TS-EVENTS-0108` | 108 | every supported binding form maps to the same handler | 3 | direct | assertion | events | — |
| `TS-EVENTS-0130` | 130 | definition ranges are UTF-8 bytes after multi-byte text | 3 | direct | assertion | events | — |
| `TS-EVENTS-0143` | 143 | a missing handler is supported with an error at the exact handler range | 9 | direct | assertion | events | — |
| `TS-EVENTS-0162` | 162 | multiple page registrations are partial, use the first and suppress missing diagnostics | 6 | direct | assertion | events | — |
| `TS-EVENTS-0176` | 176 | non-literal registrations are partial without a missing-handler diagnostic | 3 | direct | assertion | events | — |
| `TS-EVENTS-0188` | 188 | component behaviors may define handlers and suppress missing diagnostics | 4 | direct | assertion | events | D2 |
| `TS-EVENTS-0236` | 236 | an unresolved behavior graph keeps missing handlers unproven | 3 | direct | assertion | events | D2 |
| `TS-EVENTS-0259` | 259 | an empty literal behaviors array keeps the methods table complete | 2 | direct | assertion | events | — |
| `TS-EVENTS-0268` | 268 | a literal registration is partial when a non-literal one also exists | 5 | direct | assertion | events | — |
| `TS-EVENTS-0280` | 280 | spread in options or methods is partial; known handlers still resolve | 10 | direct | assertion | events | — |
| `TS-EVENTS-0301` | 301 | a component without methods reports a real missing handler | 4 | direct | assertion | events | — |
| `TS-EVENTS-0312` | 312 | non-literal component methods are partial without a diagnostic | 3 | direct | assertion | events | — |
| `TS-EVENTS-0322` | 322 | reserved page keys and __wxml members are ignored | 4 | direct | assertion | events | — |
| `TS-EVENTS-0339` | 339 | non-identifier and non-literal-function handlers never become diagnostics | 6 | direct | assertion | events | — |
| `TS-EVENTS-0352` | 352 | long signatures are capped to one bounded line | 3 | direct | assertion | events | — |
| `TS-EVENTS-0363` | 363 | event results are capped at the request length bound | 3 | direct | assertion | events | — |
| `TS-EVENTS-0375` | 375 | event strings count against the request byte budget | 2 | direct | assertion | events | — |
| `TS-EVENTS-0387` | 387 | the early-return analyze path returns eventResults for events only | 5 | direct | assertion | events | — |
| `TS-EVENTS-0399` | 399 | the full analyze path returns eventResults without altering fragment results | 8 | direct | assertion | events | — |
| `TS-EVENTS-0425` | 425 | a handler declared by an aliased behavior resolves to the behavior file | 7 | direct | assertion | events | D2 |
| `TS-EVENTS-0461` | 461 | a component method overrides the same-named behavior method | 5 | direct | assertion | events | D2 |
| `TS-EVENTS-0498` | 498 | an unresolved behavior alias stays partial and never reports a missing handler | 4 | direct | assertion | events | D2 |

#### `evidence.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-EVIDENCE-0011` | 11 | sanitizer replaces POSIX and Windows path forms | 4 | direct | unit | messages_evidence | — |
| `TS-EVIDENCE-0025` | 25 | evidence output must stay inside the repository layout | 5 | direct | unit | messages_evidence | — |
| `TS-EVIDENCE-0050` | 50 | the committed evidence contains no absolute paths | 3 | direct | unit | messages_evidence | — |

#### `guards.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-GUARDS-0047` | 47 | renderGuards opens if/elif and closes one block | 3 | direct | unit | guards_js | — |
| `TS-GUARDS-0057` | 57 | renderGuards renders the else branch as a trailing `} else {` | 2 | direct | unit | guards_js | — |
| `TS-GUARDS-0066` | 66 | renderGuards never renders a condition after the selected branch | 2 | direct | unit | guards_js | — |
| `TS-GUARDS-0075` | 75 | plain value-level \|\| guards are not degraded | 1 | direct | unit | guards_js | — |
| `TS-GUARDS-0087` | 87 | an unrewritable condition yields no narrowing block | 1 | direct | unit | guards_js | — |
| `TS-GUARDS-0097` | 97 | if/elif/else narrows a discriminated union per branch | 7 | direct | unit | guards_js | — |
| `TS-GUARDS-0128` | 128 | a truthiness guard removes null | 4 | direct | unit | guards_js | — |
| `TS-GUARDS-0152` | 152 | nested chains narrow cumulatively | 4 | direct | unit | guards_js | — |
| `TS-GUARDS-0174` | 174 | guard conditions see loop locals | 4 | direct | unit | guards_js | — |
| `TS-GUARDS-0201` | 201 | an unparseable guard does not fabricate a diagnostic and reports Partial | 6 | direct | unit | guards_js | — |
| `TS-GUARDS-0218` | 218 | &&, \|\| and ! guards narrow by truthiness | 8 | direct | unit | guards_js | — |
| `TS-GUARDS-0261` | 261 | guards interleave with nested shadowed loops | 4 | direct | unit | guards_js | — |
| `TS-GUARDS-0302` | 302 | checkJs off keeps hover but disables template diagnostics | 5 | direct | unit | guards_js | — |
| `TS-GUARDS-0312` | 312 | pragma injects a shadow-only @ts-check and reports the typo | 4 | direct | unit | guards_js | — |
| `TS-GUARDS-0321` | 321 | checkJs true inherits the project option | 3 | direct | unit | guards_js | — |
| `TS-GUARDS-0329` | 329 | JSDoc documentation survives a JavaScript view | 3 | direct | unit | guards_js | — |

#### `layouts.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-LAYOUTS-0049` | 49 | page options accept an injected probe member | 1 | direct | unit | components | — |
| `TS-LAYOUTS-0062` | 62 | component options reject an injected probe member (2353) | 1 | direct | unit | components | — |
| `TS-LAYOUTS-0072` | 72 | component `methods` accepts the injected probe | 1 | direct | unit | components | — |
| `TS-LAYOUTS-0084` | 84 | `this.data` keeps documentation of data fields | 3 | direct | unit | components | — |
| `TS-LAYOUTS-0101` | 101 | `optionalTypes` is invisible on the instance data type | 2 | direct | unit | components | — |
| `TS-LAYOUTS-0117` | 117 | JavaScript overlay uses a JSDoc typedef and keeps documentation | 3 | direct | unit | components | — |
| `TS-LAYOUTS-0147` | 147 | an undeclared property is reported with 2339 and hover quality Missing | 5 | direct | unit | components | — |
| `TS-LAYOUTS-0167` | 167 | optionalTypes boundary mapping | 6 | direct | unit | components | — |
| `TS-LAYOUTS-0190` | 190 | unknown property shapes degrade instead of guessing | 3 | direct | unit | components | — |
| `TS-LAYOUTS-0206` | 206 | non-literal `properties` is reported, never narrowed silently | 2 | direct | unit | components | — |
| `TS-LAYOUTS-0216` | 216 | the mounted target is a whole identifier outside string literals | 5 | direct | unit | components | — |
| `TS-LAYOUTS-0224` | 224 | quoted property names keep the definition target inside the quotes | 1 | direct | unit | components | — |

#### `locals.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-LOCALS-0048` | 48 | renderLocals emits nested __wxml_item/__wxml_index declarations | 3 | direct | unit | loops | — |
| `TS-LOCALS-0063` | 63 | nested levels indent, and closers are innermost first | 3 | direct | unit | loops | — |
| `TS-LOCALS-0089` | 89 | a same-name nested chain captures the outer binding (no TDZ self-reference) | 4 | direct | unit | loops | — |
| `TS-LOCALS-0124` | 124 | an unparseable source expression falls back to any and reports unresolved | 2 | direct | unit | loops | — |
| `TS-LOCALS-0130` | 130 | JavaScript mode binds locals as any without helper types | 4 | direct | unit | loops | — |
| `TS-LOCALS-0138` | 138 | invalid binding names are ignored defensively | 2 | direct | unit | loops | — |
| `TS-LOCALS-0144` | 144 | levels are grouped by array order, not by source expression | 4 | direct | unit | loops | — |
| `TS-LOCALS-0261` | 261 | typed locals resolve element and key types end-to-end | 8 | direct | unit | loops | — |
| `TS-LOCALS-0288` | 288 | an inner loop that shadows the outer name does not hit the TDZ | 4 | direct | unit | loops | — |
| `TS-LOCALS-0312` | 312 | JavaScript projects bind locals as any and report Partial | 6 | direct | unit | loops | — |
| `TS-LOCALS-0334` | 334 | an extremely deep `wx:for` source degrades instead of overflowing the stack | 2 | direct | unit | loops | — |
| `TS-LOCALS-0343` | 343 | a `locals`-only request still degrades to Partial with a typed reason | 4 | direct | unit | loops | — |

#### `mapping.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-MAPPING-0010` | 10 | UTF-16 and byte offsets agree around astral characters | 6 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0020` | 20 | mapped span trims a synthetic prefix and keeps the source part | 2 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0034` | 34 | a span entirely inside synthetic text is not attributable | 3 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0048` | 48 | assemble maps offsets after an insertion with several synthetic prefixes | 3 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0078` | 78 | a mapped span never exceeds the recorded source range | 2 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0091` | 91 | out-of-bounds spans and non-character boundaries are unattributable | 6 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0110` | 110 | fractional and negative span bounds are unattributable | 3 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0126` | 126 | cross-gap target is used only when fully inside the span | 2 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0145` | 145 | every mount carries an explicit synthetic/source split | 9 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0188` | 188 | nullable receiver diagnostics land on the receiver, not the member | 4 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0208` | 208 | one expression mounted twice keeps both mappings and publishes one diagnostic | 4 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0223` | 223 | BOM is dropped whenever a pragma must be honoured | 6 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0239` | 239 | published evidence carries insertions and mounts for every run | 8 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0261` | 261 | mount target length is published and matches the mounted identifier | 3 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0279` | 279 | BOM removal is a one-character replacement, not an insertion | 12 | direct | parity | mapping_utf16 | — |
| `TS-MAPPING-0306` | 306 | suppressed diagnostics stay out of the published set | 3 | direct | parity | mapping_utf16 | — |

#### `messages.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-MESSAGES-0005` | 5 | helper names are removed from user-visible messages | 2 | direct | unit | messages_evidence | — |
| `TS-MESSAGES-0016` | 16 | typings internals are unwrapped | 3 | direct | unit | messages_evidence | — |
| `TS-MESSAGES-0025` | 25 | generated overlay type names are replaced | 1 | direct | unit | messages_evidence | — |
| `TS-MESSAGES-0032` | 32 | duplicated intersection literals collapse | 1 | direct | unit | messages_evidence | — |
| `TS-MESSAGES-0039` | 39 | long messages are truncated | 2 | direct | unit | messages_evidence | — |

#### `meta.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-META-0009` | 9 | TypeScript and the typings package are pinned | 7 | rewrite | unit | protocol_limits | — |
| `TS-META-0027` | 27 | the C1 contract documents the pinned versions | 2 | rewrite | unit | protocol_limits | — |

#### `overlay.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-OVERLAY-0023` | 23 | unsaved type and JSDoc changes reach the template without touching disk | 4 | direct | parity | project_overlay | — |
| `TS-OVERLAY-0036` | 36 | JSDoc-only edits are visible without a type change | 3 | direct | parity | project_overlay | — |
| `TS-OVERLAY-0047` | 47 | closing the overlay falls back to the original content | 2 | direct | parity | project_overlay | — |
| `TS-OVERLAY-0055` | 55 | unsaved changes in an imported model file propagate (AC-004) | 7 | direct | parity | project_overlay | — |
| `TS-OVERLAY-0076` | 76 | a model file outside the snapshot is never read from disk | 1 | direct | parity | project_overlay | — |

#### `perf.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-PERF-0012` | 12 | rough timings for cold start, hover and diagnostics | 5 | rewrite | parity | perf_budget | — |

#### `probes.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-PROBES-0027` | 27 | pragma scanning handles plain files, BOM and hashbang | 11 | direct | unit | guards_js | — |
| `TS-PROBES-0048` | 48 | the last pragma in the leading block wins (TypeScript semantics) | 5 | direct | unit | guards_js | — |
| `TS-PROBES-0059` | 59 | `@ts-nocheck` outside the leading comments is ignored | 2 | direct | unit | guards_js | — |
| `TS-PROBES-0064` | 64 | probe health rejects a probe root that resolves to any | 1 | direct | unit | guards_js | — |
| `TS-PROBES-0080` | 80 | probe health rejects diagnostics that intersect an insertion | 1 | direct | unit | guards_js | — |
| `TS-PROBES-0098` | 98 | probe health uses the exact offsets it is given, not a name search | 4 | direct | unit | guards_js | — |
| `TS-PROBES-0135` | 135 | a duplicate helper is reported through the insertion check | 1 | direct | unit | guards_js | — |
| `TS-PROBES-0153` | 153 | probe health reports a module classification flip | 1 | direct | unit | guards_js | — |
| `TS-PROBES-0169` | 169 | environment check fails fast when lib or typings are missing | 1 | direct | unit | guards_js | — |
| `TS-PROBES-0176` | 176 | read policy keeps project files out of disk reads | 3 | direct | unit | project_overlay | — |

#### `protocol.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-PROTOCOL-0021` | 21 | the request carries files, fragments, options, policy and owner | 21 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0053` | 53 | protocol v2 accepts optional bindings, guards, components and events | 2 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0098` | 98 | protocol v2 rejects malformed structured bindings | 2 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0126` | 126 | protocol v2 accepts a same-name nested binding chain | 1 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0141` | 141 | protocol v2 rejects a duplicate binding name inside one nesting level | 1 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0154` | 154 | protocol v2 bounds guard chains and their conditions | 2 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0178` | 178 | v2 structured payloads count against the request byte budget | 6 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0234` | 234 | the option digest is independent of the checkout path | 1 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0250` | 250 | input limits are enforced before any language-service work | 7 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0280` | 280 | unknown compiler options are rejected instead of silently dropped | 3 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0306` | 306 | the response reports registration, applied policy and request digest | 7 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0319` | 319 | explicit off neutralises a source `@ts-check` in the derived view | 4 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0329` | 329 | limits merge with defaults and respect the server ceiling | 6 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0348` | 348 | enum values and the protocol version are validated | 3 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0368` | 368 | the request drives mounted fragments and policy | 11 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0395` | 395 | the C1 response is assembled from the run | 15 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0421` | 421 | response file identities use the request's fileId space | 5 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0438` | 438 | BOM + pragma + probe insertions share one fileId | 3 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0450` | 450 | alias-project keeps project-relative ids and the model definition fileId | 6 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0463` | 463 | candidate B generated files use a synthetic fileId and definitions point at source | 5 | obsolete | unit | protocol_limits | — |
| `TS-PROTOCOL-0479` | 479 | lib, target and new strict-family switches pass through the request | 5 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0500` | 500 | case-insensitive target names map to script targets | 2 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0505` | 505 | fragment ids from the request are echoed, not replaced by expressions | 3 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0525` | 525 | the same expression may be mounted under two fragment ids | 5 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0549` | 549 | duplicate fragment ids are a structured request error, not a crash | 4 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0561` | 561 | C1.1 fragments accept target/locals and reject malformed ones | 3 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0600` | 600 | legacy fragments without target/locals still validate | 1 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0606` | 606 | files[], view and analysisStamp are validated before analysis | 1 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0644` | 644 | propertiesOverlay off is rejected explicitly, not ignored | 1 | rewrite | unit | protocol_limits | — |
| `TS-PROTOCOL-0653` | 653 | invalid binding names are rejected in locals | 2 | rewrite | unit | protocol_limits | — |

#### `rewrite.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-REWRITE-0051` | 51 | unary, parenthesized and multi-root expressions are fully rewritten | 14 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0078` | 78 | indexed array roots are rewritten before the index | 4 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0088` | 88 | template locals are not prefixed anywhere in the expression | 7 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0101` | 101 | template locals may shadow global builtins | 7 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0118` | 118 | string and template text stay verbatim while substitutions are rewritten | 5 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0127` | 127 | numeric literals do not become default targets | 5 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0136` | 136 | member names, object keys, strings and globals are not rewritten | 7 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0156` | 156 | repeated identifiers keep the rightmost target offset | 5 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0183` | 183 | object keys are not default targets | 3 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0190` | 190 | object shorthand is expanded safely | 3 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0197` | 197 | a member typo keeps the exact WXML byte range | 3 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0207` | 207 | wxmlRange must match the expression byte length | 1 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0224` | 224 | targets inside comments or strings are rejected | 1 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0239` | 239 | wrapper escapes and extra statements are rejected | 1 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0250` | 250 | unsupported expression shapes fail closed | 1 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0260` | 260 | depth limit is counted from the parsed root expression | 2 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0274` | 274 | deeply nested and malformed expressions are bad-fragment, not RangeError | 1 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0285` | 285 | error messages truncate huge expressions | 5 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0299` | 299 | expressions without a free root identifier are rejected | 1 | direct | unit | shadow_rewrite | — |
| `TS-REWRITE-0309` | 309 | rewritten mounts map target spans and trim any synthetic prefix | 13 | direct | unit | mapping_utf16 | — |
| `TS-REWRITE-0349` | 349 | cross-gap diagnostics are cropped to a real operand | 6 | direct | unit | mapping_utf16 | — |
| `TS-REWRITE-0368` | 368 | number+string concatenation is not fabricated as an error | 3 | direct | unit | mapping_utf16 | — |
| `TS-REWRITE-0378` | 378 | expression emission is linear for 1 MiB and 4 MiB / 9k roots | 4 | direct | unit | mapping_utf16 | — |
| `TS-REWRITE-0413` | 413 | rewritten roots keep mapping for hover and diagnostics | 2 | direct | unit | mapping_utf16 | — |

#### `server.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-SERVER-0094` | 94 | ping and shutdown follow JSON-RPC 2.0 | 5 | rewrite | assertion | protocol_limits | — |
| `TS-SERVER-0109` | 109 | analyze returns the C1 response for a page | 5 | rewrite | assertion | protocol_limits | — |
| `TS-SERVER-0129` | 129 | unknown methods, bad JSON and bad params do not kill the server | 5 | rewrite | assertion | protocol_limits | — |
| `TS-SERVER-0150` | 150 | JSON-RPC notifications get no response | 1 | rewrite | assertion | protocol_limits | — |
| `TS-SERVER-0171` | 171 | malformed requests are rejected as invalid params, not internal errors | 5 | rewrite | assertion | protocol_limits | — |
| `TS-SERVER-0203` | 203 | oversized lines are rejected and dropped without killing the server | 3 | rewrite | assertion | protocol_limits | — |
| `TS-SERVER-0216` | 216 | ten concurrent requests are all answered on the same stream | 1 | rewrite | assertion | protocol_limits | — |
| `TS-SERVER-0237` | 237 | backend directory resolution supports source, dist and packaged layouts | 4 | rewrite | assertion | protocol_limits | — |
| `TS-SERVER-0267` | 267 | entry detection follows a symlinked install path | 2 | rewrite | assertion | protocol_limits | — |

#### `symbols.test.ts`

| Case | 行 | 标题 | 断言 | 处置 | Oracle | 目标分区 | Dirty |
| --- | ---: | --- | ---: | --- | --- | --- | --- |
| `TS-SYMBOLS-0061` | 61 | page data path returns declaration, this.data references and setData segments | 10 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0098` | 98 | root rename covers the first setData path segment and sibling fields | 5 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0116` | 116 | component properties expose observers and a safe missing-handler insertion | 11 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0150` | 150 | missing component methods are created around a methods object | 4 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0164` | 164 | shared data objects resolve to their declaration file | 6 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0204` | 204 | dynamic data/setData shapes are reported as incomplete | 3 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0215` | 215 | a missing event handler on an ambiguous owner table has no insertion | 2 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0226` | 226 | UTF-8 offsets are byte ranges, not UTF-16 indices | 2 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0242` | 242 | symbols request validation rejects malformed queries | 3 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0264` | 264 | event handler references include this.method calls | 3 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0286` | 286 | shorthand destructuring is reported incomplete and never edited | 2 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0299` | 299 | renamed destructuring binding stays safe | 2 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0312` | 312 | other.setData receivers are not edited or claimed complete | 2 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0324` | 324 | dynamic this.data access and spreads make coverage incomplete | 1 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0336` | 336 | sourceComplete=false forces a partial empty answer | 3 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0346` | 346 | component methods insertion with existing members parses without TS1005 | 3 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0367` | 367 | component reserved names include data, properties and methods | 4 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0391` | 391 | nested setData in another script is incomplete | 3 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0406` | 406 | this aliases with dynamic data access are incomplete | 3 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0425` | 425 | another page's own this.setData does not block the current page | 2 | direct | assertion | symbols_edits | — |
| `TS-SYMBOLS-0439` | 439 | nested this.setData edits the nested target segment | 3 | direct | assertion | symbols_edits | — |

### 11.3 matrix 的 12 个顶层声明

| Case | 行 | 标题 | 断言 | 处置 | Oracle |
| --- | ---: | --- | ---: | --- | --- |
| `TS-MATRIX-0092` | 92 | every scenario runs both candidates without throwing | 1 | split | parity |
| `TS-MATRIX-0197` | 197 | candidate B cannot anchor a registration inside a function | 3 | obsolete | parity |
| `TS-MATRIX-0205` | 205 | module components expose candidate B's overlay-scope failure | 5 | obsolete | parity |
| `TS-MATRIX-0215` | 215 | interface used as `data` value is reported, never fabricated (same file and import) | 4 | split | parity |
| `TS-MATRIX-0228` | 228 | a shorthand `data` member is unsupported, not 'missing' | 4 | split | parity |
| `TS-MATRIX-0239` | 239 | JavaScript hover works without checking, diagnostics do not | 6 | split | parity |
| `TS-MATRIX-0252` | 252 | explicit off beats a source `@ts-check` and publishes no diagnostics | 9 | split | parity |
| `TS-MATRIX-0271` | 271 | component properties union needs the declaration overlay in both candidates | 4 | split | parity |
| `TS-MATRIX-0283` | 283 | BOM + hashbang + source pragma works for off and inherit | 8 | split | parity |
| `TS-MATRIX-0301` | 301 | multi-registration uses the first registration and does not merge | 6 | split | parity |
| `TS-MATRIX-0319` | 319 | documented option switches change the derived check | 6 | split | parity |
| `TS-MATRIX-0335` | 335 | noImplicitThis:false degrades the probe instead of claiming Supported | 4 | split | parity |

### 11.4 原 blocked-decision 全量列表（37，均已裁决）

| # | Case | 来源 | 具体原因（摘要） | replacement_case_id |
| ---: | --- | --- | --- | --- |
| 1 | `TS-MATRIX-0102-ALIAS_PROJECT-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (alias-project / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP la… | null |
| 2 | `TS-MATRIX-0102-COMPONENT_DATA_CROSS_FILE-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (component-data-cross-file / symbol-bridge) exercises an internal candidate layout, not a user-selec… | null |
| 3 | `TS-MATRIX-0102-COMPONENT_JS-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (component-js / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP lay… | null |
| 4 | `TS-MATRIX-0102-COMPONENT_MODULE-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (component-module / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP… | null |
| 5 | `TS-MATRIX-0102-COMPONENT_PROPERTIES-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (component-properties / symbol-bridge) exercises an internal candidate layout, not a user-selectable… | null |
| 6 | `TS-MATRIX-0102-CRLF_SCRIPT-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (crlf-script / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP layo… | null |
| 7 | `TS-MATRIX-0102-CROSS_FILE_OVERLAY-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (cross-file-overlay / symbol-bridge) exercises an internal candidate layout, not a user-selectable L… | null |
| 8 | `TS-MATRIX-0102-DATA_SHORTHAND-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (data-shorthand / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP l… | null |
| 9 | `TS-MATRIX-0102-INTERFACE_ROOT-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (interface-root / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP l… | null |
| 10 | `TS-MATRIX-0102-INTERFACE_ROOT_IMPORT-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (interface-root-import / symbol-bridge) exercises an internal candidate layout, not a user-selectabl… | null |
| 11 | `TS-MATRIX-0102-JS_BOM_CHECK_INHERIT-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (js-bom-check-inherit / symbol-bridge) exercises an internal candidate layout, not a user-selectable… | null |
| 12 | `TS-MATRIX-0102-JS_BOM_CHECK_OFF-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (js-bom-check-off / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP… | null |
| 13 | `TS-MATRIX-0102-JS_CHECKJS-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (js-checkjs / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP layou… | null |
| 14 | `TS-MATRIX-0102-JS_JSDOC_OFF-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (js-jsdoc-off / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP lay… | null |
| 15 | `TS-MATRIX-0102-JS_JSDOC_PRAGMA-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (js-jsdoc-pragma / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP … | null |
| 16 | `TS-MATRIX-0102-JS_NOCHECK-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (js-nocheck / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP layou… | null |
| 17 | `TS-MATRIX-0102-JS_SHEBANG_BOM-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (js-shebang-bom / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP l… | null |
| 18 | `TS-MATRIX-0102-JS_SOURCE_CHECK_INHERIT-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (js-source-check-inherit / symbol-bridge) exercises an internal candidate layout, not a user-selecta… | null |
| 19 | `TS-MATRIX-0102-JS_SOURCE_CHECK_OFF-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (js-source-check-off / symbol-bridge) exercises an internal candidate layout, not a user-selectable … | null |
| 20 | `TS-MATRIX-0102-METHODS_NONLITERAL-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (methods-nonliteral / symbol-bridge) exercises an internal candidate layout, not a user-selectable L… | null |
| 21 | `TS-MATRIX-0102-MULTI_REGISTRATION-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (multi-registration / symbol-bridge) exercises an internal candidate layout, not a user-selectable L… | null |
| 22 | `TS-MATRIX-0102-NO_IMPLICIT_THIS_OFF-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (no-implicit-this-off / symbol-bridge) exercises an internal candidate layout, not a user-selectable… | null |
| 23 | `TS-MATRIX-0102-NULLABLE-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (nullable / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP layout;… | null |
| 24 | `TS-MATRIX-0102-NULLABLE_NO_STRICT_NULL-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (nullable-no-strict-null / symbol-bridge) exercises an internal candidate layout, not a user-selecta… | null |
| 25 | `TS-MATRIX-0102-PAGE_BASIC-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (page-basic / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP layou… | null |
| 26 | `TS-MATRIX-0102-PAGE_CJS-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (page-cjs / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP layout;… | null |
| 27 | `TS-MATRIX-0102-PAGE_CLOSURE-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (page-closure / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP lay… | null |
| 28 | `TS-MATRIX-0102-PAGE_ESM-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (page-esm / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP layout;… | null |
| 29 | `TS-MATRIX-0102-PAGE_EXPORT_EMPTY-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (page-export-empty / symbol-bridge) exercises an internal candidate layout, not a user-selectable LS… | null |
| 30 | `TS-MATRIX-0102-PROPERTIES_SPREAD-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (properties-spread / symbol-bridge) exercises an internal candidate layout, not a user-selectable LS… | null |
| 31 | `TS-MATRIX-0102-TS_NOCHECK-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (ts-nocheck / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP layou… | null |
| 32 | `TS-MATRIX-0102-UNCHECKED_INDEX-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (unchecked-index / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP … | null |
| 33 | `TS-MATRIX-0102-UNICODE_CRLF-SYMBOL_BRIDGE` | `matrix.test.ts:102` | symbol-bridge case (unicode-crlf / symbol-bridge) exercises an internal candidate layout, not a user-selectable LSP lay… | null |
| 34 | `TS-MATRIX-0197` | `matrix.test.ts:197` | symbol-bridge case (candidate B cannot anchor a registration inside a function) exercises an internal candidate layout,… | null |
| 35 | `TS-MATRIX-0205` | `matrix.test.ts:205` | symbol-bridge case (module components expose candidate B's overlay-scope failure) exercises an internal candidate layou… | null |
| 36 | `TS-META-0027` | `meta.test.ts:27` | Asserts the historical C1 TS-backend contract document versions. The replacement is an F0/F1 native version/schema pin … | null |
| 37 | `TS-PROTOCOL-0463` | `protocol.test.ts:463` | Candidate-B-only synthetic fileId/definition test. The production protocol has no layout selector; whether the B-specif… | null |

## 12. 复现

```sh
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
cargo test --workspace --all-features
cd backends/typescript && npm test   # 需要 Node 24 与已存在的 node_modules；P1 证据，不是最终门禁
```

账本 JSON 的 `cases` 是唯一逐用例契约；本 Markdown 是审阅摘要。修改旧测试或新增用例时必须同步更新 JSON 的 case ID、body hash、计数与状态。P3-S/P4-B 的定向复现命令见 §14.7。


## 13. 集成裁决（2026-11-17）

- 36 个 Candidate B/symbol-bridge 用例：`obsolete`（历史证据保留，不进入原生 parity 强制集）。
- 1 个 `TS-META-0027`：`rewrite` → F0 原生引擎/类型资产 pin 测试。
- 剩余 `blocked-decision`：0。
- 依据：生产 LSP 不传 `layoutId`（`server.ts:152`）；`analysis.ts:59-60,416` 明确默认原位 shadow；Candidate B 只由 matrix/perf/evidence 脚本遍历。
- 不降低要求：原位 shadow 的 39 个 matrix 用例、协议/映射/诊断/编辑安全/FS 用例继续强制迁移。

## 14. P5/P5.1/P6 Rust 证据与状态回填（历史快照，2026-11-18/19；wave-5 见 §15，wave-6 见 §16）

本节保留 P5/P5.1/P6 的历史计数与证据表；当前计数以 §15 和 JSON `counts` 为准。P5 将 **exact parity** 的 32 个用例提升为 `green`；P5.1 追加 P4-C adapter、native definition slice 与 perf budget gate 的逐条证据，只把 1 个 adapter exact 用例（`TS-COMPLETION-0220`）提升为 `green`。P6 回填（2026-11-19）再提升 event completion 与 trusted type-assets 的 3 个 exact 用例，并为 delivery lock/de-Node 登记 weaker 证据。3 个 near-exact 用例按裁决保留 `planned` 并记录 `status_recommendation=green`；其余 weaker/structural/blocked 全部保持 `planned`。未修改生产 Rust、ADR、交付、server/config 或 Cargo。

P5 已解冻 `crates/wxml-lsp/tests/native_parity/ledger.rs` 的旧 303 planned 常量：现在校验 total=339、retired=36、状态总和=339、green 必须有非空 Rust 证据、分区计数从实际 cases 派生。P6 后常量更新为 green=36、planned=267；`reviewed`/`closed` 仍未使用（无独立评审证据）。

逐用例证据写在 JSON 的 `cases[].migration_evidence`，本节计数由该字段和 `counts` 派生。

### 14.1 状态计数 before / after

P5 初次回填（2026-11-18）：303 planned / 0 green → 271 planned / 32 green。P5.1 追加回填（2026-11-19）：271 planned / 32 green → 270 planned / 33 green。P6 回填（2026-11-19）：270 planned / 33 green → 267 planned / 36 green。

| status | P5.1 | P6 回填后 | 说明 |
| --- | ---: | ---: | --- |
| planned | 270 | 267 | 3 near-exact + 57 weaker + 6 structural + 17 blocked + 184 无证据 |
| green | 33 | 36 | 仅 `parity=exact` 且证据完整的用例；P6 新增 `TS-COMPLETION-0175`、`TS-COMPLETION-0193`、`TS-PROBES-0169` |
| retired-with-decision | 36 | 36 | 36 个 Candidate B/symbol-bridge 用例不变 |
| red | 0 | 0 | 不新增 red |
| reviewed / closed | 0 | 0 | 无独立评审证据 |
| **合计** | **339** | **339** | 用例总数不变 |

### 14.2 分区计数（从实际 cases 派生）

| 分区 | routed | 有证据 | green | planned | exact | near-exact | weaker | structural | blocked | 无证据 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `behaviors` | 22 | 18 | 1 | 21 | 1 | 2 | 6 | 5 | 4 | 4 |
| `completion` | 9 | 5 | 3 | 6 | 3 | 0 | 2 | 0 | 0 | 4 |
| `components` | 19 | 9 | 4 | 15 | 4 | 0 | 1 | 0 | 4 | 10 |
| `definitions` | 5 | 5 | 0 | 5 | 0 | 0 | 4 | 0 | 1 | 0 |
| `events` | 24 | 21 | 9 | 15 | 9 | 0 | 10 | 0 | 2 | 3 |
| `guards_js` | 25 | 11 | 5 | 20 | 5 | 0 | 1 | 0 | 5 | 14 |
| `hover_docs` | 10 | 3 | 0 | 10 | 0 | 0 | 3 | 0 | 0 | 7 |
| `loops` | 16 | 9 | 5 | 11 | 5 | 0 | 3 | 0 | 1 | 7 |
| `mapping_utf16` | 21 | 14 | 6 | 15 | 6 | 0 | 8 | 0 | 0 | 7 |
| `messages_evidence` | 8 | 0 | 0 | 8 | 0 | 0 | 0 | 0 | 0 | 8 |
| `perf_budget` | 1 | 1 | 0 | 1 | 0 | 0 | 0 | 1 | 0 | 0 |
| `project_overlay` | 20 | 3 | 0 | 20 | 0 | 0 | 3 | 0 | 0 | 17 |
| `protocol_limits` | 40 | 2 | 0 | 40 | 0 | 0 | 2 | 0 | 0 | 38 |
| `scenario_matrix` | 43 | 0 | 0 | 43 | 0 | 0 | 0 | 0 | 0 | 43 |
| `shadow_rewrite` | 19 | 18 | 3 | 16 | 3 | 1 | 14 | 0 | 0 | 1 |
| `symbols_edits` | 21 | 0 | 0 | 21 | 0 | 0 | 0 | 0 | 0 | 21 |
| **合计** | **303** | **119** | **36** | **267** | **36** | **3** | **57** | **6** | **17** | **184** |

### 14.3 exact-parity（36，已提升 `green`）

判定标准：Rust 断言覆盖 TS case 的同一可观察语义，且不依赖未执行的 checker/native query。`status=green`；`status_applied=green`；`target_rust_test_status=migrated_green`。

| Case | Rust test（path:line） | 证据摘要 |
| --- | --- | --- |
| `TS-COMPLETION-0175` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:4185<br> crates/wxml-lsp/src/backend_tsgo/event_completion/tests.rs:166 | Same Supported status, `onTap`/`onLoad` method labels, Method kind and 2-byte replace prefix; Rust also requires zero native session starts. |
| `TS-COMPLETION-0193` |  crates/wxml-lsp/src/backend_tsgo/event_completion/tests.rs:188 | Same Supported status and `save` method label; Rust asserts the methods-only table and excludes `attached`/`lifetimes`. |
| `TS-COMPLETION-0220` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:916 | Both tests cap the list at 256 items and report truncated=true; the native adapter test additionally reports Partial. |
| `TS-COMPONENT-0452` |  crates/wxml-backend-tsgo/tests/script_behavior.rs:172 | Same three winning types (later number, nested string object, onlyNested number) after nested/later behavior ordering. |
| `TS-EVENTS-0071` |  crates/wxml-backend-tsgo/tests/script_events.rs:83 | Same three handler forms: zero skipped, Supported, no diagnostic, definition path/range and signature. |
| `TS-EVENTS-0130` |  crates/wxml-backend-tsgo/tests/script_events.rs:160 | Same UTF-8 byte and UTF-16 definition ranges after multi-byte text. |
| `TS-EVENTS-0143` |  crates/wxml-backend-tsgo/tests/script_events.rs:183 | Same missing-handler Supported state, absent definition, exact range, error severity, message fields and path-free text. |
| `TS-EVENTS-0259` |  crates/wxml-backend-tsgo/tests/script_events.rs:246 | Same Supported state and error-severity missing-handler diagnostic for an empty literal behaviors array. |
| `TS-EVENTS-0312` |  crates/wxml-backend-tsgo/tests/script_events.rs:333 | Same Partial state, no diagnostic and the 'methods is not a literal object' reason. |
| `TS-EVENTS-0352` |  crates/wxml-backend-tsgo/tests/script_events.rs:409 | Same non-empty, bounded, single-line signature. |
| `TS-EVENTS-0363` |  crates/wxml-backend-tsgo/tests/script_events.rs:429 | Same result cap, one skipped result and truncation detail. |
| `TS-EVENTS-0425` |  crates/wxml-backend-tsgo/tests/script_events.rs:459 | Same aliased behavior file, Supported/no diagnostic, documentation, signature and exact UTF-8 range. |
| `TS-EVENTS-0498` |  crates/wxml-backend-tsgo/tests/script_events.rs:535 | Same unresolved alias Partial, no diagnostic and no definition. |
| `TS-GUARDS-0047` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:329 | Same unresolved flag, if/elif open text and closer. |
| `TS-GUARDS-0057` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:350 | Same else-branch open text and closer. |
| `TS-GUARDS-0066` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:367 | Same selected-branch open text and absence of the later condition. |
| `TS-GUARDS-0087` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:398 | Same empty open/close and unresolved=true. |
| `TS-LAYOUTS-0167` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:192 | Same completeness flag and all five optionalTypes boundary type texts (any, Record<string, any>, any[], function, number \| string). |
| `TS-LAYOUTS-0190` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:218 | Same incomplete flag, 'no readable' problem and surviving property list (['b']). |
| `TS-LAYOUTS-0206` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:250 | Same incomplete flag, 'not an object literal' problem and empty property list. |
| `TS-LAYOUTS-0224` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:302 | Same quoted property-name slice ('a-b') at the published declaration range; the Rust test additionally pins the surrounding quotes. |
| `TS-LOCALS-0048` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:152 | Same unresolved flag, nested helper declarations and closer. |
| `TS-LOCALS-0124` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:226 | Same unresolved flag and 'undefined as any' fallback text. |
| `TS-LOCALS-0130` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:241 | Same unresolved flag, JavaScript any binding text and JS/TS helper strings. |
| `TS-LOCALS-0138` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:257 | Same empty text and close for an invalid binding name. |
| `TS-LOCALS-0334` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:291 | Same unresolved flag and typed fallback for a 40k-term wx:for source. |
| `TS-MAPPING-0010` |  crates/wxml-backend-tsgo/tests/shadow_mapping.rs:93 | Same utf16_to_byte/byte_to_utf16 boundaries and rejection of mid-surrogate/mid-codepoint offsets. |
| `TS-MAPPING-0020` |  crates/wxml-backend-tsgo/tests/shadow_mapping.rs:104 | Same mapped source range and trimmedSyntheticPrefix flag. |
| `TS-MAPPING-0034` |  crates/wxml-backend-tsgo/tests/shadow_mapping.rs:119 | Same unattributable spans and in-source range mapping. |
| `TS-MAPPING-0048` |  crates/wxml-backend-tsgo/tests/shadow_mapping.rs:147 | Same offsets after a multi-character insertion; Rust additionally pins the end offset. |
| `TS-MAPPING-0126` |  crates/wxml-backend-tsgo/tests/shadow_mapping.rs:228 | Same cross-gap target selection, trimmed prefix and mapped range. |
| `TS-MAPPING-0279` |  crates/wxml-backend-tsgo/tests/shadow_mapping.rs:384 | Same BOM-removal edit (kind, start 0, length 0, replace 1) and no pragma injection. |
| `TS-PROBES-0169` |  crates/wxml-lsp/src/backend_tsgo/environment/tests.rs:136<br> crates/wxml-lsp/src/backend_tsgo/environment/tests.rs:159<br> crates/wxml-lsp/src/backend_tsgo/environment/gate.rs:449<br> crates/wxml-lsp/src/backend_tsgo/environment/gate.rs:458 | Same fail-fast environment error for missing lib/typings; Rust asserts typed `RootUnavailable`/`MissingRequired` and the required-mode panic remediation. |
| `TS-REWRITE-0136` |  crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:133 | Rust matches all six rewritten texts; the TS result.ok assertion is implied by the outputs. |
| `TS-REWRITE-0274` |  crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:276 | Rust asserts BadExpression for the deeply nested and malformed expressions, matching TS bad-fragment (not RangeError). |
| `TS-REWRITE-0299` |  crates/wxml-backend-tsgo/src/shadow/rewrite.rs:857<br> crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:295 | The unit test pins BadExpression for '1 + 2' and 'Math.random()'; the integration test rejects the same shapes. TS asserts bad-fragment. |

### 14.3.1 near-exact（3，保留 `planned` + green recommendation）

按 P5 裁决，near-exact 不提升；`green_eligible=false`，`status_recommendation=green`，`status_applied=planned`。

| Case | Rust test（path:line） | 证据摘要 |
| --- | --- | --- |
| `TS-COMPONENT-0521` |  crates/wxml-backend-tsgo/tests/script_behavior.rs:254 | Same named/re-exported behavior resolution: number type and Supported status (the field is renamed namedCount). |
| `TS-COMPONENT-0763` |  crates/wxml-backend-tsgo/tests/script_behavior.rs:369 | Same non-probe __state number type and complete behavior data. |
| `TS-REWRITE-0207` |  crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:209 | Rust returns a typed Invalid(fragment.range) for the wrong wxmlRange length; this is the native equivalent of the TS RequestError code bad-fragment. |

### 14.4 blocked（17，保留 `planned`）

这些用例的 TS 断言需要 checker 诊断、hover/definition 查询或完整模板链；现有 Rust 测试只覆盖布局/结构/脚本事实，不能按同强度判定。

| Case | Rust test | 缺口 |
| --- | --- | --- |
| `TS-COMPONENT-0292` |  crates/wxml-backend-tsgo/tests/script_properties.rs:72 | TS asserts component-binding diagnostics for behavior properties; Rust asserts declaration facts only and never runs the binding check. |
| `TS-COMPONENT-0498` |  crates/wxml-backend-tsgo/tests/script_behavior.rs:236<br> crates/wxml-backend-tsgo/tests/script_properties.rs:179 | TS asserts hover any plus a behavior reason; the two Rust tests assert data/property spread completeness facts only. |
| `TS-COMPONENT-0570` |  crates/wxml-backend-tsgo/tests/script_behavior.rs:279 | TS asserts behaviorIncomplete/Partial and no 2339 diagnostics; Rust asserts Partial and no invented field only. |
| `TS-COMPONENT-0676` |  crates/wxml-backend-tsgo/tests/script_properties.rs:156 | TS asserts the component-binding diagnostic and skipped details; Rust asserts the own-property overlay only. |
| `TS-DEFINITIONS-0036` |  crates/wxml-backend-tsgo/tests/real_engine.rs:470 | TS asserts a generated-member kind and the generated declaration's source range; native definition decoding has no generated-member classification or overlay source mapping, and the gate is opt-in. |
| `TS-EVENTS-0108` |  crates/wxml-backend-tsgo/tests/script_events.rs:131 | TS asserts the full template event chain; Rust classifies the seven binding forms and rejects malformed attributes only. |
| `TS-EVENTS-0188` |  crates/wxml-backend-tsgo/tests/script_events.rs:558 | TS asserts the behavior fileId/signature/docs from a stub view; Rust resolves inline behavior script facts only. |
| `TS-GUARDS-0097` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:413 | TS asserts checker narrowing/union behavior; Rust pins the rendered if/elif structure only. |
| `TS-GUARDS-0201` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:448 | TS asserts diagnostics Partial, the reason, empty diagnostics and no fabricated 2339; Rust pins the rendered structure and Partial reason only. |
| `TS-GUARDS-0218` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:475 | TS asserts &&/\|\| narrowing; Rust pins the rendered structure and Partial only. |
| `TS-GUARDS-0261` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:514 | TS asserts nested loop/guard narrowing; Rust pins local capture and closer order only. |
| `TS-GUARDS-0312` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:566 | Rust checks pragma application, the @ts-check prefix and the note; TS also asserts a published diagnostic for the typo. |
| `TS-LAYOUTS-0049` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:78 | TS asserts the checker reports no diagnostics for the injected probe; the Rust test asserts Supported layout, probe placement and byte-exact reconstruction but never runs diagnostics. |
| `TS-LAYOUTS-0062` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:142 | TS expects diagnostic 2353 for a probe placed as a direct Component option; the Rust test pins the production placement inside methods and cannot prove the diagnostic absence/presence without a checker. |
| `TS-LAYOUTS-0072` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:118 | TS asserts the checker reports no diagnostics for the injected probe; the Rust test asserts probe placement inside Component methods only. |
| `TS-LAYOUTS-0117` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:442 | TS asserts hover type/documentation and empty diagnostics for the JavaScript JSDoc overlay; the Rust test pins the pragma/JSDoc layout only. |
| `TS-LOCALS-0343` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:307 | TS asserts diagnostics Partial, the typed reason and empty fragment diagnostics; Rust pins the layout status/reason and fallback text only. |

### 14.5 weaker / structural（63，保留 `planned`）

`weaker` 57 条：同一层次但缺少 TS 的部分断言（多为 hover 类型、诊断、reason、精确 range）；`structural` 6 条：只断言 overlay/布局结构或不同粒度的性能采样，不是 TS 的同强度可观察结果。逐条摘要见 JSON `migration_evidence.note`。

- **weaker（57）**：`TS-ANALYSIS-0033`, `TS-ANALYSIS-0054`, `TS-ANALYSIS-0123`, `TS-ANALYSIS-0297`, `TS-ANALYSIS-0358`, `TS-COMPILER-0035`, `TS-COMPLETION-0104`, `TS-COMPLETION-0207`, `TS-COMPONENT-0403`, `TS-COMPONENT-0416`, `TS-COMPONENT-0591`, `TS-COMPONENT-0607`, `TS-COMPONENT-0654`, `TS-COMPONENT-0724`, `TS-DEFINITIONS-0013`, `TS-DEFINITIONS-0052`, `TS-DEFINITIONS-0074`, `TS-DEFINITIONS-0093`, `TS-EVENTS-0093`, `TS-EVENTS-0162`, `TS-EVENTS-0176`, `TS-EVENTS-0236`, `TS-EVENTS-0268`, `TS-EVENTS-0280`, `TS-EVENTS-0301`, `TS-EVENTS-0322`, `TS-EVENTS-0339`, `TS-EVENTS-0461`, `TS-GUARDS-0075`, `TS-LAYOUTS-0216`, `TS-LOCALS-0063`, `TS-LOCALS-0089`, `TS-LOCALS-0144`, `TS-MAPPING-0078`, `TS-MAPPING-0091`, `TS-MAPPING-0110`, `TS-MAPPING-0145`, `TS-MAPPING-0223`, `TS-MAPPING-0239`, `TS-MAPPING-0261`, `TS-META-0009`, `TS-REWRITE-0051`, `TS-REWRITE-0078`, `TS-REWRITE-0088`, `TS-REWRITE-0101`, `TS-REWRITE-0118`, `TS-REWRITE-0127`, `TS-REWRITE-0156`, `TS-REWRITE-0183`, `TS-REWRITE-0190`, `TS-REWRITE-0224`, `TS-REWRITE-0239`, `TS-REWRITE-0250`, `TS-REWRITE-0260`, `TS-REWRITE-0285`, `TS-REWRITE-0309`, `TS-SERVER-0237`
- **structural（6）**：`TS-ANALYSIS-0380`, `TS-COMPONENT-0357`, `TS-COMPONENT-0476`, `TS-COMPONENT-0514`, `TS-COMPONENT-0539`, `TS-PERF-0012`

### 14.6 无 Rust 证据的 planned 用例（184）

这些用例尚未有同名或同语义的 Rust 测试，仍需要对应的 native/query/协议能力。按目标分区列出：

- **`behaviors`（4，需要 complete behavior graph）**：`TS-COMPONENT-0550`, `TS-COMPONENT-0628`, `TS-COMPONENT-0697`, `TS-COMPONENT-0774`
- **`completion`（4，需要 native completion query）**：`TS-COMPLETION-0066`, `TS-COMPLETION-0078`, `TS-COMPLETION-0127`, `TS-COMPLETION-0153`
- **`components`（10，需要 component-binding diagnostics/hover）**：`TS-COMPONENT-0147`, `TS-COMPONENT-0193`, `TS-COMPONENT-0206`, `TS-COMPONENT-0229`, `TS-COMPONENT-0251`, `TS-COMPONENT-0268`, `TS-COMPONENT-0279`, `TS-LAYOUTS-0084`, `TS-LAYOUTS-0101`, `TS-LAYOUTS-0147`
- **`events`（3，需要 full analyze path / event result budget）**：`TS-EVENTS-0375`, `TS-EVENTS-0387`, `TS-EVENTS-0399`
- **`guards_js`（14，需要 native checker narrowing + probe diagnostics）**：`TS-GUARDS-0128`, `TS-GUARDS-0152`, `TS-GUARDS-0174`, `TS-GUARDS-0302`, `TS-GUARDS-0321`, `TS-GUARDS-0329`, `TS-PROBES-0027`, `TS-PROBES-0048`, `TS-PROBES-0059`, `TS-PROBES-0064`, `TS-PROBES-0080`, `TS-PROBES-0098`, `TS-PROBES-0135`, `TS-PROBES-0153`
- **`hover_docs`（7，需要 native hover/quickinfo + checker）**：`TS-ANALYSIS-0078`, `TS-ANALYSIS-0093`, `TS-ANALYSIS-0139`, `TS-ANALYSIS-0161`, `TS-ANALYSIS-0174`, `TS-ANALYSIS-0184`, `TS-ANALYSIS-0196`
- **`loops`（7，需要 native locals/loop-scope hover/definition）**：`TS-ANALYSIS-0205`, `TS-ANALYSIS-0226`, `TS-ANALYSIS-0243`, `TS-ANALYSIS-0255`, `TS-LOCALS-0261`, `TS-LOCALS-0288`, `TS-LOCALS-0312`
- **`mapping_utf16`（7，需要 remaining mapping/rewrite queries）**：`TS-MAPPING-0188`, `TS-MAPPING-0208`, `TS-MAPPING-0306`, `TS-REWRITE-0349`, `TS-REWRITE-0368`, `TS-REWRITE-0378`, `TS-REWRITE-0413`
- **`messages_evidence`（8，需要 evidence sanitizer/contract）**：`TS-EVIDENCE-0011`, `TS-EVIDENCE-0025`, `TS-EVIDENCE-0050`, `TS-MESSAGES-0005`, `TS-MESSAGES-0016`, `TS-MESSAGES-0025`, `TS-MESSAGES-0032`, `TS-MESSAGES-0039`
- **`project_overlay`（17，需要 compiler/session overlay + transport）**：`TS-ANALYSIS-0272`, `TS-ANALYSIS-0287`, `TS-ANALYSIS-0315`, `TS-ANALYSIS-0332`, `TS-ANALYSIS-0349`, `TS-ANALYSIS-0371`, `TS-COMPILER-0050`, `TS-COMPILER-0063`, `TS-COMPILER-0086`, `TS-COMPILER-0095`, `TS-COMPILER-0132`, `TS-OVERLAY-0023`, `TS-OVERLAY-0036`, `TS-OVERLAY-0047`, `TS-OVERLAY-0055`, `TS-OVERLAY-0076`, `TS-PROBES-0176`
- **`protocol_limits`（38，需要 native JSON-RPC/C1 server replacement）**：`TS-META-0027`, `TS-PROTOCOL-0021`, `TS-PROTOCOL-0053`, `TS-PROTOCOL-0098`, `TS-PROTOCOL-0126`, `TS-PROTOCOL-0141`, `TS-PROTOCOL-0154`, `TS-PROTOCOL-0178`, `TS-PROTOCOL-0234`, `TS-PROTOCOL-0250`, `TS-PROTOCOL-0280`, `TS-PROTOCOL-0306`, `TS-PROTOCOL-0319`, `TS-PROTOCOL-0329`, `TS-PROTOCOL-0348`, `TS-PROTOCOL-0368`, `TS-PROTOCOL-0395`, `TS-PROTOCOL-0421`, `TS-PROTOCOL-0438`, `TS-PROTOCOL-0450`, `TS-PROTOCOL-0479`, `TS-PROTOCOL-0500`, `TS-PROTOCOL-0505`, `TS-PROTOCOL-0525`, `TS-PROTOCOL-0549`, `TS-PROTOCOL-0561`, `TS-PROTOCOL-0600`, `TS-PROTOCOL-0606`, `TS-PROTOCOL-0644`, `TS-PROTOCOL-0653`, `TS-SERVER-0094`, `TS-SERVER-0109`, `TS-SERVER-0129`, `TS-SERVER-0150`, `TS-SERVER-0171`, `TS-SERVER-0203`, `TS-SERVER-0216`, `TS-SERVER-0267`
- **`scenario_matrix`（43，需要 full in-place-shadow scenario matrix）**：`TS-MATRIX-0092`, `TS-MATRIX-0102-PAGE_BASIC-IN_PLACE_SHADOW`, `TS-MATRIX-0102-PAGE_EXPORT_EMPTY-IN_PLACE_SHADOW`, `TS-MATRIX-0102-PAGE_CJS-IN_PLACE_SHADOW`, `TS-MATRIX-0102-PAGE_ESM-IN_PLACE_SHADOW`, `TS-MATRIX-0102-PAGE_CLOSURE-IN_PLACE_SHADOW`, `TS-MATRIX-0102-COMPONENT_PROPERTIES-IN_PLACE_SHADOW`, `TS-MATRIX-0102-COMPONENT_MODULE-IN_PLACE_SHADOW`, `TS-MATRIX-0102-COMPONENT_JS-IN_PLACE_SHADOW`, `TS-MATRIX-0102-PROPERTIES_SPREAD-IN_PLACE_SHADOW`, `TS-MATRIX-0102-MULTI_REGISTRATION-IN_PLACE_SHADOW`, `TS-MATRIX-0102-JS_JSDOC_PRAGMA-IN_PLACE_SHADOW`, `TS-MATRIX-0102-JS_JSDOC_OFF-IN_PLACE_SHADOW`, `TS-MATRIX-0102-JS_CHECKJS-IN_PLACE_SHADOW`, `TS-MATRIX-0102-TS_NOCHECK-IN_PLACE_SHADOW`, `TS-MATRIX-0102-JS_NOCHECK-IN_PLACE_SHADOW`, `TS-MATRIX-0102-JS_SHEBANG_BOM-IN_PLACE_SHADOW`, `TS-MATRIX-0102-INTERFACE_ROOT-IN_PLACE_SHADOW`, `TS-MATRIX-0102-INTERFACE_ROOT_IMPORT-IN_PLACE_SHADOW`, `TS-MATRIX-0102-NULLABLE-IN_PLACE_SHADOW`, `TS-MATRIX-0102-CROSS_FILE_OVERLAY-IN_PLACE_SHADOW`, `TS-MATRIX-0102-ALIAS_PROJECT-IN_PLACE_SHADOW`, `TS-MATRIX-0102-COMPONENT_DATA_CROSS_FILE-IN_PLACE_SHADOW`, `TS-MATRIX-0102-CRLF_SCRIPT-IN_PLACE_SHADOW`, `TS-MATRIX-0102-DATA_SHORTHAND-IN_PLACE_SHADOW`, `TS-MATRIX-0102-JS_SOURCE_CHECK_OFF-IN_PLACE_SHADOW`, `TS-MATRIX-0102-JS_SOURCE_CHECK_INHERIT-IN_PLACE_SHADOW`, `TS-MATRIX-0102-METHODS_NONLITERAL-IN_PLACE_SHADOW`, `TS-MATRIX-0102-UNCHECKED_INDEX-IN_PLACE_SHADOW`, `TS-MATRIX-0102-NULLABLE_NO_STRICT_NULL-IN_PLACE_SHADOW`, `TS-MATRIX-0102-NO_IMPLICIT_THIS_OFF-IN_PLACE_SHADOW`, `TS-MATRIX-0102-JS_BOM_CHECK_OFF-IN_PLACE_SHADOW`, `TS-MATRIX-0102-JS_BOM_CHECK_INHERIT-IN_PLACE_SHADOW`, `TS-MATRIX-0102-UNICODE_CRLF-IN_PLACE_SHADOW`, `TS-MATRIX-0215`, `TS-MATRIX-0228`, `TS-MATRIX-0239`, `TS-MATRIX-0252`, `TS-MATRIX-0271`, `TS-MATRIX-0283`, `TS-MATRIX-0301`, `TS-MATRIX-0319`, `TS-MATRIX-0335`
- **`shadow_rewrite`（1，需要 definition range through native query）**：`TS-REWRITE-0197`
- **`symbols_edits`（21，需要 LSP symbols + rename/WorkspaceEdit）**：`TS-SYMBOLS-0061`, `TS-SYMBOLS-0098`, `TS-SYMBOLS-0116`, `TS-SYMBOLS-0150`, `TS-SYMBOLS-0164`, `TS-SYMBOLS-0204`, `TS-SYMBOLS-0215`, `TS-SYMBOLS-0226`, `TS-SYMBOLS-0242`, `TS-SYMBOLS-0264`, `TS-SYMBOLS-0286`, `TS-SYMBOLS-0299`, `TS-SYMBOLS-0312`, `TS-SYMBOLS-0324`, `TS-SYMBOLS-0336`, `TS-SYMBOLS-0346`, `TS-SYMBOLS-0367`, `TS-SYMBOLS-0391`, `TS-SYMBOLS-0406`, `TS-SYMBOLS-0425`, `TS-SYMBOLS-0439`

### 14.7 复现与验证

```sh
# P5 基线（2026-11-18）
cargo test -p wxml-backend-tsgo --test shadow_layout --test shadow_rewrite \
  --test shadow_mapping --test shadow_locals_guards --test script_registration \
  --test script_behavior --test script_properties --test script_events --test script_symbols
# 176 passed, 0 failed, 0 ignored

cargo test -p wxml-backend-tsgo --lib
# 235 passed, 0 failed, 0 ignored

# P5.1 定向证据（2026-11-19）
cargo test -p wxml-lsp --lib backend_tsgo::tests
# 51 passed, 0 failed

cargo test -p wxml-backend-tsgo --lib query::definition::tests
# 6 passed, 0 failed

jq empty docs/plans/tsgo-test-ledger.json
cargo test -p wxml-lsp --test native_parity
# 25 passed, 0 failed
git diff --check
cargo fmt --all -- --check
```

P6 回填（2026-11-19，HEAD `9a8b65c` + 并行 WIP）的批量验证：

```sh
jq empty docs/plans/tsgo-test-ledger.json
cargo test -p wxml-lsp --test native_parity
# 36 passed / 0 failed / 3 ignored
cargo test -p wxml-lsp --lib
# 789 passed / 0 failed / 0 ignored
WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
TSGO_TYPINGS_ROOT=/tmp/opencode/assets-lock-root/typings \
cargo test -p wxml-lsp --test native_parity native_engine_real \
  -- --ignored --test-threads=1
# 3 passed / 0 failed / 36 filtered
cargo fmt -p wxml-lsp -- --check
cargo fmt --all -- --check   # exit 0（最终复跑）
git diff --check
```

### 14.8 Validator 改动

- `ALLOWED_STATUSES` 增加 `green`；状态计数校验改为 green=36、planned=267、retired=36，且三者之和必须等于 339，不再硬编码 pre-migration 的 303 planned。
- P5.1 把 parity bucket 校验更新为 exact=33、near_exact=3、weaker=55、structural=6、blocked=17；P6 回填后更新为 exact=36、weaker=57，并继续由实际 `migration_evidence` 反算 `assessed_cases`/无证据数。
- 新增 `ledger_migration_evidence_is_complete_and_consistent`：green 必须是 `parity=exact`、`green_eligible=true`、`status_applied=green`，并具备非空 `rust_tests`；每个测试必须有非空 path、line、test、fully-qualified name 和 note，且源文件中确实存在该 `fn`。
- near-exact 必须保持 planned + recommendation=green + green_eligible=false；weaker/structural/blocked 必须 planned + recommendation=planned；空证据直接失败。
- 分区计数由 `cases[].target_rust_test` 和 `counts.migration_evidence.by_partition` 双向校验；`Partition.planned_cases` 改为 `Partition.routed_cases`，总数由 `ledger::non_obsolete_case_count` 派生，不再固定 303。
- 保留原有 schema、top-level key、case ID、disposition、oracle/coverage、fixture hash、旧 TS source hash/line、ignored/silent-skip gate 检查。

## 15. Wave-5 Rust 证据回填（2026-11-20）

### 15.1 身份、边界与结果

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 记录时 HEAD：`28c5e18eebfc1fe7c5df14e9a3655404185065a2`。任务指定的回填边界是
  `375cde24a61141f2b122bdf6ca9c23fbfc0fb1a6`；之后并行会话又提交了
  `04fa0fe`（E2E-20）、`2f072a6`（final gate）和 `28c5e18`（F3 kill window）。
  本节**只映射到 `375cde2` 及之前**的 Rust 测试；E2E-20、`f3_kill_window` 和 Zed smoke
  没有映射，也没有把 gate 状态当作 case 状态。
- 范围：只改 `docs/plans/tsgo-test-ledger.json`、本 Markdown 台账、
  `docs/reference/tsgo-test-contract.md`、`crates/wxml-lsp/tests/native_parity/ledger.rs`
  和证据文档。未修改生产代码、测试源码、Cargo、delivery、Node 或默认后端；未提交。
- 结果：**44 `green` / 259 `planned` / 36 `retired-with-decision`**，339 总数不变；
  `assessed_cases` 119 → 145，无 Rust 证据的 planned 184 → 158。
  `near_exact` 仍为 3；`weaker` 57 → 74；`structural` 6 → 7；`blocked` 17 → 17。

### 15.2 新增 exact → `green`（8）

判定标准与 §14.3 相同：Rust 断言覆盖 TS case 的同一可观察语义，且不把 gate_green
自动等同为 case green。`status_applied=green`、`target_rust_test_status=migrated_green`。

| Case | Rust test（path:line） | 证据摘要 |
| --- | --- | --- |
| `TS-COMPLETION-0127` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:4356 | Typed-prefix root completion returns the exact data-overlay root and replaces only the prefix range; Supported and zero native session starts. |
| `TS-COMPLETION-0153` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:4315 | Empty-mustache root completion returns the exact data-overlay root with a zero-width replace; Supported and zero native session starts. |
| `TS-EVENTS-0108` |  crates/wxml-lsp/src/semantics/event_definition.rs:857 | Seven binding forms plus `bind:my-event`; each is Supported and maps to the exact declaration token/file. |
| `TS-SYMBOLS-0061` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:68<br> crates/wxml-lsp/tests/native_parity/native_engine_real.rs:2700 | Unit test covers Supported/complete, declaration/read starts, setData path/segment; required-native symbols smoke adds exact declaration/read/setData byte locations and rename edits. |
| `TS-SYMBOLS-0150` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:127 | Complete=true, zero-width insertion on the options close, text contains `methods: {` and `onSave()`. |
| `TS-SYMBOLS-0215` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:480 | Opaque and spread owner tables are complete=false with no declaration and no insertion. |
| `TS-SYMBOLS-0226` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:161 | CJK declaration has exact UTF-8 start/end plus the UTF-16 start after emoji/CRLF text. |
| `TS-SYMBOLS-0312` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:370 | `other.setData` is complete=false with zero occurrences; literal keys remain pinned next to a computed key. |

`TS-SYMBOLS-0061` 的两个证据分开记录：非 ignored 单元测试负责 `Supported`/`complete`/
path/segment，`#[ignore]` 的 required-native smoke 负责补齐 exact declaration/read/setData
字节位置和 rename 编辑。默认 validator 不运行后者；这不是用 gate_green 代替 case green。

### 15.3 新增非 exact 证据（19 个新 case）

#### weaker（17）

| Case | Rust test（path:line） | 为什么不是 exact |
| --- | --- | --- |
| `TS-COMPLETION-0078` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:4452<br> crates/wxml-lsp/src/backend_tsgo/tests.rs:4276 | Member labels/range match, but native status is Partial while TS expects Supported. |
| `TS-SYMBOLS-0098` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:209 | Root segment/text match; exact occurrence start/end and sibling fields are not asserted. |
| `TS-SYMBOLS-0164` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:546 | Cross-file behavior declaration path exists; TS shared-data-object scenario is not covered. |
| `TS-SYMBOLS-0204` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:318<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:344<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:370 | Dynamic data/setData shapes are Partial with reasons, but no single test covers the exact TS fixture. |
| `TS-SYMBOLS-0242` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:614<br> crates/wxml-lsp/src/backend_tsgo/tests.rs:2424 | Malformed queries return Unsupported rather than the TS RequestError channel. |
| `TS-SYMBOLS-0264` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:525 | Declaration/this-alias read exist, but computed access makes the result Partial and the exact read range is not asserted. |
| `TS-SYMBOLS-0286` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:397 | Destructuring is Partial with a reason; the TS never-edited assertion is not exercised through the edit plan. |
| `TS-SYMBOLS-0324` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:344<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:318 | Dynamic this.data/spread are Partial, but the TS rest/Object.keys shapes are not all covered. |
| `TS-SYMBOLS-0336` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:2446 | Partial/reason/no session match; empty `fields` is not asserted. |
| `TS-SYMBOLS-0346` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:108 | Insertion is exact; the applied-text parse (TS1005) is missing. |
| `TS-SYMBOLS-0367` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:504<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:242 | Reserved data/property/method names are asserted for DataRoots, not both TS dataPath queries. |
| `TS-SYMBOLS-0391` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:438 | Another-script setData is incomplete with a reason; TS status/zero-occurrence assertions are missing. |
| `TS-SYMBOLS-0406` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:397 | Alias/destructuring is Partial with a reason; dynamic access through the alias is not asserted. |
| `TS-SYMBOLS-0439` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:209 | Nested segment/text match; exact nested occurrence start/end are missing. |
| `TS-OVERLAY-0023` |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:511 | Unsaved model type reaches two templates; JSDoc replacement is not asserted and the fixture uses a literal type. |
| `TS-OVERLAY-0047` |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:511 | Close restores the disk string, but only in the opt-in ignored job and with a different fixture. |
| `TS-OVERLAY-0055` |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:511 | Imported model overlay crosses views/restart; the TS number/JSDoc overlay is not identical. |

#### blocked（1）

| Case | Rust test（path:line） | 缺口 |
| --- | --- | --- |
| `TS-SYMBOLS-0116` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:108<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:573 | Missing-handler insertion is exact, but `observerPath` is not implemented and the component dataPath stays Partial; TS expects complete=true plus an observerPath occurrence. |

#### structural（1）

| Case | Rust test（path:line） | 缺口 |
| --- | --- | --- |
| `TS-REWRITE-0413` |  crates/wxml-backend-tsgo/tests/shadow_guard_ranges.rs:112<br> crates/wxml-backend-tsgo/tests/shadow_guard_ranges.rs:196<br> crates/wxml-backend-tsgo/tests/shadow_guard_ranges.rs:353 | Guard wrapper/value spans map to exact original bytes, but hover/diagnostics never run through the checker. |

### 15.4 既有非 exact 用例追加 wave-5 证据（14）

这些用例的状态/parity 不变，只在 `migration_evidence.rust_tests` 增加本轮测试：

- `TS-COMPONENT-0357`（structural）：  `script_optional_types.rs:269`
- `TS-COMPONENT-0403`（weaker）：  `script_optional_types.rs:176`
- `TS-COMPONENT-0416`（weaker）：  `script_optional_types.rs:196`
- `TS-COMPONENT-0607`（weaker）：  `script_optional_types.rs:221`
- `TS-EVENTS-0093`（weaker）：  `event_definition.rs:894`
- `TS-EVENTS-0162`（weaker）：  `event_diagnostics.rs:524`、  `event_definition.rs:712`
- `TS-EVENTS-0176`（weaker）：  `event_diagnostics.rs:540`、  `backend_tsgo/tests.rs:3807`
- `TS-EVENTS-0188`（blocked）：  `event_diagnostics.rs:383`、  `event_definition.rs:910`
- `TS-EVENTS-0236`（weaker）：  `event_diagnostics.rs:466`
- `TS-EVENTS-0268`（weaker）：  `event_diagnostics.rs:524`、  `event_diagnostics.rs:540`
- `TS-EVENTS-0280`（weaker）：  `event_diagnostics.rs:479`、  `event_definition.rs:1011`
- `TS-EVENTS-0301`（weaker）：  `event_diagnostics.rs:452`
- `TS-EVENTS-0339`（weaker）：  `event_diagnostics.rs:512`、  `event_diagnostics.rs:554`
- `TS-PERF-0012`（structural）：  `native_engine_real.rs:3131`

完整全限定测试名、行号和逐条 note 以 JSON `cases[].migration_evidence` 为准。

### 15.5 当前分区计数

| 分区 | routed | 有证据 | green | planned 有证据 | exact | near-exact | weaker | structural | blocked | 无证据 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `behaviors` | 22 | 18 | 1 | 17 | 1 | 2 | 6 | 5 | 4 | 4 |
| `completion` | 9 | 8 | 5 | 3 | 5 | 0 | 3 | 0 | 0 | 1 |
| `components` | 19 | 9 | 4 | 5 | 4 | 0 | 1 | 0 | 4 | 10 |
| `definitions` | 5 | 5 | 0 | 5 | 0 | 0 | 4 | 0 | 1 | 0 |
| `events` | 24 | 21 | 10 | 11 | 10 | 0 | 10 | 0 | 1 | 3 |
| `guards_js` | 25 | 11 | 5 | 6 | 5 | 0 | 1 | 0 | 5 | 14 |
| `hover_docs` | 10 | 3 | 0 | 3 | 0 | 0 | 3 | 0 | 0 | 7 |
| `loops` | 16 | 9 | 5 | 4 | 5 | 0 | 3 | 0 | 1 | 7 |
| `mapping_utf16` | 21 | 15 | 6 | 9 | 6 | 0 | 8 | 1 | 0 | 6 |
| `messages_evidence` | 8 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 8 |
| `perf_budget` | 1 | 1 | 0 | 1 | 0 | 0 | 0 | 1 | 0 | 0 |
| `project_overlay` | 20 | 6 | 0 | 6 | 0 | 0 | 6 | 0 | 0 | 14 |
| `protocol_limits` | 40 | 2 | 0 | 2 | 0 | 0 | 2 | 0 | 0 | 38 |
| `scenario_matrix` | 43 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 43 |
| `shadow_rewrite` | 19 | 18 | 3 | 15 | 3 | 1 | 14 | 0 | 0 | 1 |
| `symbols_edits` | 21 | 19 | 5 | 14 | 5 | 0 | 13 | 0 | 1 | 2 |
| **合计** | **303** | **145** | **44** | **101** | **44** | **3** | **74** | **7** | **17** | **158** |

### 15.6 未映射的本轮原生面

- **standalone script diagnostics**（`8014739`）：旧 TS 套件没有独立的 `.ts/.js`
  diagnostics case；`script_diagnostics_*` 测试属于 native-only 行为，不新增 case ID，
  也不把 `TS-MATRIX-*` 或 `TS-ANALYSIS-*` 当作同义证据。
- **perf/stability**（`8b6ca14`）：`native_q2_performance_benchmark` 已作为 structural
  证据追加到 `TS-PERF-0012`；`native_ten_round_lifecycle_stability` 没有对应 TS case，
  只记录在 gate 证据中。
- **gate wave 3/4**（`c5ee251`、`375cde2`）：gate 状态已由对应提交和
  `native_gates.rs` 维护；本节只使用 `native_symbols_smoke` 和
  `native_non_owner_overlay_replay_survives_restart` 的断言作为 case 证据，不因为
  `gate_green` 自动提升任何 case。
- **E2E-20 / f3_kill_window / Zed smoke**：不在本次边界内，未映射。

### 15.7 验证

```sh
jq empty docs/plans/tsgo-test-ledger.json
cargo test -p wxml-lsp --test native_parity
cargo fmt --all -- --check
git diff --check
```

结果见 `docs/evidence/2026-11-20-tsgo-ledger-backfill-wave5.md`。

## 16. Wave-6 Rust 证据回填（2026-11-20）

### 16.1 边界与结论

- 记录/验证 HEAD：`1d5222eef18904ee056d7da97de3b011feb65cc8`；工作树含并行会话未提交的
  native edit/CLI 改动，本 wave 没有修改它们，也没有把它们的测试当作 wave-6 证据。
- 范围：只改 `docs/plans/tsgo-test-ledger.json`、本 Markdown、contract
  （`docs/reference/tsgo-test-contract.md`）、ledger validator
  （`crates/wxml-lsp/tests/native_parity/ledger.rs`）、`docs/evidence/README.md` 和
  `docs/evidence/2026-11-20-tsgo-ledger-backfill-wave6.md`。未修改生产代码、测试源码、
  Cargo、delivery、Node 或默认后端；未提交。
- 盘点对象：script diagnostics routing（`814ca5e`）、event handler hover
  （`3220362`）、E2E-21 non-literal（`4fd45bd`）、native symbols
  （`304aa20`/`4140e9b`）、PERF-01/STABILITY-01（`8b6ca14`）、E2E-20 component
  bindings（`04fa0fe`）、native gate wave-5（`2f072a6`）、F3 kill（`28c5e18`），以及
  `backend_tsgo/tests.rs`、`handlers/diagnostics*`、`semantics/event_definition.rs`、
  `native_parity/native_engine_real.rs`、`script_symbol_facts.rs`、
  `component_bindings.rs`、`native_measure.rs` 的新测试。
- 结论：**没有新的 exact case**。`green` 保持 44，`planned` 保持 259，339 总数与
  36 retired 不变；`assessed_cases` 145 → 154，`weaker` 74 → 80，
  `blocked` 17 → 20，`structural` 7，`near_exact` 3，无 Rust 证据的 planned
  158 → 149。没有因为任何 gate 变绿而提升 case，也没有把 E2E-21 的 template-level
  `Unsupported` 当作 event-result `Partial`。

### 16.2 新增 exact → `green`（0）

本轮没有 case 满足 exact。重点核对后仍不提升的原因：

- E2E-20 的 component-binding pass 只断言单个 `bad-mustache` 的精确表达式范围/消息、
  一个 clean 双属性样例、字面量/布尔属性范围和 Array/Object 保守比较；
  `TS-COMPONENT-0147` 还要求 per-tag clean/union/literal/array/object 全矩阵、8 个
  skipped tag 和精确的 skipped reason，不能算等价。
- `TS-COMPONENT-0193` 的 `probeProblems`、`components` 数组和 `skipped=0` 没有
  native 断言；`TS-COMPONENT-0206` 的 no-mustache fixture、Supported 和 `skipped=0`
  也没有。
- `TS-COMPONENT-0229` 未覆盖 `User | undefined` / `Item[] | undefined` 和 absent
  绑定；`TS-COMPONENT-0251` 没有 `Component({ data })` 无 properties 的 fixture；
  `TS-COMPONENT-0268` 只断言 Partial reason path-free，没有遍历每条 diagnostic。
- `TS-SYMBOLS-0299` 与 `TS-SYMBOLS-0425` 的 native 行为与 TS 期望相反（见 §16.3），
  不能升级。

### 16.3 新增非 exact 证据（9 个新 case）

#### weaker（6）

| Case | Rust test（path:line） | 为什么不是 exact |
| --- | --- | --- |
| `TS-COMPONENT-0147` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1233<br> crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1663<br> crates/wxml-lsp/src/backend_tsgo/tests.rs:6439 | 单个 title mismatch 的范围/严重度/消息精确；TS 的 21 断言 per-tag 矩阵、union/literal/array/object 和 8 个 skipped pair 未覆盖。 |
| `TS-COMPONENT-0193` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1206 | 两个匹配绑定 clean 且非 Partial；TS 的三属性请求、`probeProblems`、`components` 数组和 `skipped=0` 未断言。 |
| `TS-COMPONENT-0206` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1289 | literal/boolean 使用精确 attribute range 和消息；TS 的 no-mustache fixture、Supported 和 `skipped=0` 未断言。 |
| `TS-COMPONENT-0229` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1612 | Array/Object 保守比较；未覆盖 optional `User \| undefined` / `Item[] \| undefined`、absent 绑定和 `skipped=0`。 |
| `TS-COMPONENT-0251` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1365 | missing component/incomplete properties 都 Partial；没有 TS 的 `Component({ data })` 无 properties fixture，也没有 `components=undefined`。 |
| `TS-COMPONENT-0268` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:6476<br> crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1233 | Partial reason 已断言 path-free，mismatch 消息只含属性/表达式；没有遍历所有 tag diagnostic 的 TS 断言。 |

#### blocked（3）

| Case | Rust test（path:line） | 缺口 |
| --- | --- | --- |
| `TS-COMPONENT-0279` |  crates/wxml-lsp/src/backend_tsgo/tests.rs:6476<br> crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1663 | native 只保留原位 shadow；symbol-bridge layout 已退役，无法复现该 layout 的 path-free 消息断言。 |
| `TS-SYMBOLS-0299` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:344<br> crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:397 | native 把 `this.data` 解构视为 Partial 且不给 declaration；TS 要求 renamed destructuring complete=true + declaration。 |
| `TS-SYMBOLS-0425` |  crates/wxml-backend-tsgo/tests/script_symbol_facts.rs:438 | native 让另一个 script 的 setData 阻断 completeness，与 TS 的 per-page isolation 相反，无法断言 complete=true。 |

完整全限定测试名和逐条 note 以 JSON `cases[].migration_evidence` 为准。

### 16.4 既有用例追加 wave-6 证据（4）

状态/parity 不变：

- `TS-ANALYSIS-0297`（weaker）：新增  `backend_tsgo/tests.rs:4959`、
   `backend_tsgo/tests.rs:4987`；script-owned / external-file diagnostic 只降级
  diagnostics 并给 path-free reason，没有断言 hover/definition 仍 Supported。
- `TS-COMPLETION-0104`（weaker）：新增  `backend_tsgo/tests.rs:4418`；未闭合成员表达式
  仍用精确 replace range 补全 typed prefix，但状态仍是 Partial。
- `TS-SYMBOLS-0116`（blocked）：新增  `script_symbol_facts.rs:127`；组件
  missing-handler 在 options close 上包一层新的 `methods`，`observerPath` 仍缺失。
- `TS-EVENTS-0108`（green）：新增  `full_feature_matrix_editing.rs:891`；真实 LSP
  session 对 bindtap/catchtap/capture-bind/capture-catch/mut-bind 和 CJK handler
  的 hover 范围精确，支持原有 definition exact 证据。
- `TS-COMPLETION-0078`（weaker）：只修正既有测试行号到  `backend_tsgo/tests.rs:4594`
  和  `backend_tsgo/tests.rs:4418`，不改变强度。

### 16.5 Wave-6 结束时的分区计数

| 分区 | routed | 有证据 | green | planned 有证据 | exact | near-exact | weaker | structural | blocked | 无证据 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `behaviors` | 22 | 18 | 1 | 17 | 1 | 2 | 6 | 5 | 4 | 4 |
| `completion` | 9 | 8 | 5 | 3 | 5 | 0 | 3 | 0 | 0 | 1 |
| `components` | 19 | 16 | 4 | 12 | 4 | 0 | 7 | 0 | 5 | 3 |
| `definitions` | 5 | 5 | 0 | 5 | 0 | 0 | 4 | 0 | 1 | 0 |
| `events` | 24 | 21 | 10 | 11 | 10 | 0 | 10 | 0 | 1 | 3 |
| `guards_js` | 25 | 11 | 5 | 6 | 5 | 0 | 1 | 0 | 5 | 14 |
| `hover_docs` | 10 | 3 | 0 | 3 | 0 | 0 | 3 | 0 | 0 | 7 |
| `loops` | 16 | 9 | 5 | 4 | 5 | 0 | 3 | 0 | 1 | 7 |
| `mapping_utf16` | 21 | 15 | 6 | 9 | 6 | 0 | 8 | 1 | 0 | 6 |
| `messages_evidence` | 8 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 8 |
| `perf_budget` | 1 | 1 | 0 | 1 | 0 | 0 | 0 | 1 | 0 | 0 |
| `project_overlay` | 20 | 6 | 0 | 6 | 0 | 0 | 6 | 0 | 0 | 14 |
| `protocol_limits` | 40 | 2 | 0 | 2 | 0 | 0 | 2 | 0 | 0 | 38 |
| `scenario_matrix` | 43 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 43 |
| `shadow_rewrite` | 19 | 18 | 3 | 15 | 3 | 1 | 14 | 0 | 0 | 1 |
| `symbols_edits` | 21 | 21 | 5 | 16 | 5 | 0 | 13 | 0 | 3 | 0 |
| **合计** | **303** | **154** | **44** | **110** | **44** | **3** | **80** | **7** | **20** | **149** |

### 16.6 未映射的本轮原生面

- **script diagnostics routing**（`814ca5e`）：旧 TS 套件没有独立 `.ts/.js`
  diagnostics case；只把 script-owned / external-file diagnostic 降级证据追加到
  `TS-ANALYSIS-0297`，不新增 case，也不把 template diagnostics 混入。
- **event handler hover**（`3220362`）：`TS-EVENTS-0108` 已由 definition exact 证据
  提升；本轮追加 IT-15 hover E2E，不重复提升。
- **E2E-21 non-literal**（`4fd45bd`）：`the_e2e21_non_literal_page_fixture_is_unsupported_not_unavailable`
  断言的是 template-level `Unsupported`，`TS-EVENTS-0176` 断言 event-result
  `Partial + reason + no diagnostic`；两者不等价，保持 weaker。
- **native symbols**（`304aa20`/`4140e9b`）：`TS-SYMBOLS-0061/0150/0215/0226/0312`
  已 green，其余 symbols 证据保持原强度；`TS-SYMBOLS-0299/0425` 记录 blocked。
- **PERF-01 / STABILITY-01**（`8b6ca14`）：`native_q2_performance_benchmark` 继续作为
  `TS-PERF-0012` structural 证据；`native_ten_round_lifecycle_stability` 没有对应
  TS case，只保留 gate 证据。
- **E2E-20 component bindings**（`04fa0fe`）：新增 6 weaker + 1 blocked，没有 exact；
  不把 gate 的 Supported 当作 `TS-COMPONENT-0193` 的 exact。
- **native gate wave-5**（`2f072a6`）：只更新 gate 证据；`gate_green` 不等于 case green。
- **F3 kill**（`28c5e18`）：delivery 进程/kill-window 证据，没有 TS oracle case。
- **document symbols**：旧 TS 套件没有 document-symbol case，不新增 case ID。
- **Zed smoke**：不是测试迁移 case，不进入 ledger。

### 16.7 验证

```sh
jq empty docs/plans/tsgo-test-ledger.json
cargo test -p wxml-lsp --test native_parity
cargo fmt --all -- --check
git diff --check
```

实际结果见 `docs/evidence/2026-11-20-tsgo-ledger-backfill-wave6.md`。工作树并行会话的
未提交 native-edit 测试会让 `native_parity` 计数高于 HEAD-only 基线；本 wave 没有把它们
当作 case 证据。

## 17. Wave-6 组件绑定跟进（2026-11-20）

### 17.1 边界与结论

- 任务基线 HEAD：`0bf666b994163d2a44a4b87b0b8bb5d4e9101bf1`；记录期间并行会话把
  工作树 HEAD 推进到 `7ca92a3`，本跟进没有修改或占用它们的 native edit、gate、
  delivery 与 Cargo 改动。
- 范围：`crates/wxml-lsp/src/backend_tsgo/component_bindings.rs` 的测试与夹具、
  台账 JSON/Markdown、ledger validator 常量、contract 计数、证据索引和
  `docs/evidence/2026-11-20-tsgo-ledger-wave6-component-exact.md`。
  未修改默认 backend、Node、delivery、Cargo 或 `backend_node`；未提交。
- 结论：4 个 case 达到 exact 并提升为 `green`，2 个保持 blocked。
  `green` 44 → 48，`planned` 259 → 255，`exact` 44 → 48，
  `weaker` 80 → 74，`blocked` 20 → 22；`assessed_cases` 154、
  `near_exact` 3、`structural` 7 和无证据 planned 149 不变。

### 17.2 新增 exact → `green`（4）

| Case | Rust test（path:line） | 覆盖的 TS 断言 |
| --- | --- | --- |
| `TS-COMPONENT-0147` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1647<br> crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:2291<br> crates/wxml-lsp/src/backend_tsgo/tests.rs:6439 | per-tag clean/literal/boolean/undeclared 矩阵、union/literal/array/object 的精确 range 与消息、8 个 skipped usage、`partial` 状态和前三对 skipped 的精确 reason；clean/skip 因无 per-tag 响应数组而按 usage 逐个断言，任一 skip 都会翻转 status |
| `TS-COMPONENT-0206` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1806<br> crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1606 | 无 mustache、零 mount、一条 attribute-range 诊断、`supported` 和 `skipped=0`（status `None` 等价证明） |
| `TS-COMPONENT-0251` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1919<br> crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1993 | `Component({ data })` 无 properties：无诊断、无 query、`partial`，reason 精确到裸 tag；native 投影的 `components=undefined` |
| `TS-COMPONENT-0268` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1941<br> crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1550<br> crates/wxml-lsp/src/backend_tsgo/tests.rs:6476 | 遍历完整矩阵全部诊断与 Partial reason，逐条断言不含 `/` |

### 17.3 仍 blocked（2）

| Case | Rust test（path:line） | 缺口 |
| --- | --- | --- |
| `TS-COMPONENT-0193` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1771<br> crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1523 | 三属性 clean、三 mount、零 skip 与 `supported` 已覆盖；TS 的 `probeProblems=[]` 是 Node probe root/helper 健康检查，native shadow 没有 probe anchor 或 `probeProblems` 可观察面，不能用 status/fragment_count 伪装等价 |
| `TS-COMPONENT-0229` |  crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:1843<br> crates/wxml-lsp/src/backend_tsgo/component_bindings.rs:2240 | `Item[] \| undefined` 与纯 `undefined` 已清洁覆盖；pinned wire 没有 union 成员结构 flag，`User \| undefined` 既不能证明满足 Object，也不能报 `expects string, got User \| undefined`，按名字猜测属于伪造 |

### 17.4 当前分区计数

| 分区 | routed | 有证据 | green | planned 有证据 | exact | near-exact | weaker | structural | blocked | 无证据 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `components` | 19 | 16 | 8 | 8 | 8 | 0 | 1 | 0 | 7 | 3 |

其余分区不变；全表以 JSON `counts.migration_evidence.by_partition` 为准。

### 17.5 验证

```sh
cargo fmt -p wxml-lsp -- --check
cargo check -p wxml-lsp --all-targets
cargo clippy -p wxml-lsp --all-targets -- -D warnings
cargo test -p wxml-lsp --lib backend_tsgo::component_bindings
cargo test -p wxml-lsp --test native_parity
jq empty docs/plans/tsgo-test-ledger.json
git diff --check
```

实际结果见 `docs/evidence/2026-11-20-tsgo-ledger-wave6-component-exact.md`。
