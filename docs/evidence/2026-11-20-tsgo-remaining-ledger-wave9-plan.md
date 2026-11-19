# tsgo 测试台账 wave-9 剩余 planned 规划（2026-11-20）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务书给定 HEAD：`807a157`。读取开始时工作树确实在 `807a157`；读取过程中并行提交
  `75040c2`（wave-8）、`16e88b2`（P7 冻结）、`6fc7a00`（wave-9 回填）、
  `fb6047e`（delivery N2）、`6727059`（P7 gate 准备）先后落地，
  读取完成时 HEAD 为 `6727059`。本文件按读取完成时的实际 JSON 计数；
  若后续回填继续移动，以 JSON 重新读取为准。
- 记录日期：2026-11-20
- 范围：**只新增本文件**。未修改生产代码、测试源码、Cargo、delivery、ledger
  (`docs/plans/tsgo-test-ledger.json`)、Markdown 台账、contract、README、CI 或脚本；
  未 commit，未启动 review。
- wave-7/9 的补断言与台账回填已经由 `6fc7a00` 提交：
  `TS-PROBES-0027`、`TS-ANALYSIS-0139`、`TS-ANALYSIS-0174` 从 weaker 升 exact，
  `green` 55→58、`planned` 248→245。这 3 个 case 原本已有 `migration_evidence`，
  **不属于**本文件要处理的 143 个 no-evidence case；不要重复登记。
- 当前工作树仍有其他并行内容（读取完成时可见 `docs/adr/0008-rust-tsgo-no-node-backend.md`
  修改；以实际 `git status` 为准），与本文件的 143 个 case 无直接关系；不要覆盖。
- 本文件只读上述内容，不把 gate_green、opt-in native gate 或并行 P7 冻结结果
  当作 case green。

**结论先行：**

1. 以实际 JSON 为准，当前是 **58 `green` / 245 `planned` / 36 `retired`**，
   `assessed_cases=160`，无证据 planned **143**。任务书中的 48/255/36 是
   `807a157` 的旧快照；wave-8/wave-9 已分别由 `75040c2` 和 `6fc7a00` 落账。
2. 143 个 no-evidence case 可分为：**12 exact-ready、25 少量测试/夹具、59 需要
   checker/native 新能力、41 需要契约裁决、6 应 retired/waiver**。
3. 下一波不新增 native 能力时，**最乐观可把 planned 从 245 降到 233**（12 个
   exact）；**保守可降到 236**（只落 9 个低风险 exact）。剩下 3 个条件 exact
   取决于评审是否接受 fake-engine/mapping-layer 等价。
4. **不能降到 0。** 即使把其余 131 个无证据 case 全部 waiver/retire，planned
   仍有 **102** 个已评估的 weaker/near-exact/structural/blocked；P7-1 的零目标
   还必须处理这 102 个，不能由 wave-9 单独完成。

---

## 1. 当前实际计数

### 1.1 台账实际值

| 项 | 值 |
| --- | ---: |
| 总 case | 339 |
| `green` | 58 |
| `planned` | 245 |
| `retired-with-decision` | 36 |
| `assessed_cases` | 160 |
| `exact` | 58 |
| `near_exact` | 3 |
| `weaker` | 70 |
| `structural` | 7 |
| `blocked` | 22 |
| 无证据 planned | 143 |
| 有证据 planned | 102 = 70 weaker + 3 near-exact + 7 structural + 22 blocked |

### 1.2 与旧文档的差异

| 来源 | green | planned | 说明 |
| --- | ---: | ---: | --- |
| 任务书 / `807a157` 旧快照 | 48 | 255 | 读取开始时 |
| `2026-11-20-tsgo-ledger-backfill-wave8.md` | 55 | 248 | wave-8 |
| `2026-11-20-tsgo-ledger-backfill-wave9.md`（`6fc7a00`） | **58** | **245** | wave-9 |
| 实际 JSON（读取完成时） | **58** | **245** | 本文件使用 |

`docs/plans/2026-11-17-tsgo-test-ledger.md` 顶部和 contract 也已经是 245；
该 Markdown 的 §20.5 仍保留 wave-8 历史段（248），不要拿它当当前值。后续任何
回填必须以 JSON 的 `counts.statuses` / `counts.migration_evidence` 重新读取为准；
不要把本文件的 245 当作下一次回填后的固定值。

wave-9 提交 `6fc7a00` 把 `TS-PROBES-0027`、`TS-ANALYSIS-0139`、
`TS-ANALYSIS-0174` 从 weaker 补成 exact（+3 green / −3 planned）。这 3 个 case
原本已有 `migration_evidence`，所以**无证据 planned 仍是 143**；本文 A/B/C/D/E
分类不变。若该回填被回退到 55/248，本文所有 planned 结果整体 +3。

---

## 2. 143 个 no-evidence case 的分类

判定口径：

- 先看 TS oracle 的每条断言是否能在**默认运行的 Rust 测试**中等价表达；
- 再看是否已有真实 Rust 落点（函数、测试、fixture 或 adapter 管线），
  不把 opt-in `native_engine_real` gate 当作 case green；
- 再看该 case 是否已经断言 **symbol-bridge**（Candidate B）布局。生产架构只保留
  in-place shadow，凡是同时断言两个 layout 的 case，不能仅凭 in-place 结果升 exact；
- 最后看是否需要 C1 envelope、probe health、module kind、真实 TS checker 等
  native 目前没有的可观察字段。

| 类 | 含义 | 数量 | 对 planned 的影响 |
| --- | --- | ---: | --- |
| A | 可直接用现有 Rust 行为补 exact（低风险） | 12 | 全部落 exact 可 −12 |
| B | 需少量新增测试/夹具，预计 weaker | 25 | 0（可能 −若干，若出 layout/字段 waiver） |
| C | 需 checker/native 新能力 | 59 | 0，维持 blocked/planned |
| D | 需契约裁决（C1/池/机制替换） | 41 | 0，除非书面 waiver/retire |
| E | 应 retired/waiver | 6 | 0，除非书面裁决 |

### 2.1 A：exact-ready（12）

全部为默认运行、无需真实引擎的纯函数/布局/服务边界测试；其中 9 个是低风险，
3 个是条件 exact（fake engine 或 mapping-layer 等价）。

`TS-ANALYSIS-0161`, `TS-ANALYSIS-0184`, `TS-ANALYSIS-0196`, `TS-ANALYSIS-0349`,
`TS-PROBES-0048`, `TS-PROBES-0059`, `TS-PROTOCOL-0126`, `TS-PROTOCOL-0280`,
`TS-PROTOCOL-0319`, `TS-PROTOCOL-0600`, `TS-REWRITE-0197`, `TS-SERVER-0150`

### 2.2 B：少量新增测试/夹具，预计 weaker（25）

`TS-ANALYSIS-0205`, `TS-ANALYSIS-0255`, `TS-COMPLETION-0066`,
`TS-MAPPING-0188`, `TS-MAPPING-0208`,
`TS-MATRIX-0215`, `TS-MATRIX-0228`, `TS-MATRIX-0271`, `TS-MATRIX-0301`,
`TS-OVERLAY-0036`, `TS-OVERLAY-0076`, `TS-PROBES-0176`,
`TS-PROTOCOL-0053`, `TS-PROTOCOL-0098`, `TS-PROTOCOL-0306`,
`TS-PROTOCOL-0438`, `TS-PROTOCOL-0505`, `TS-PROTOCOL-0525`,
`TS-PROTOCOL-0549`, `TS-REWRITE-0349`, `TS-REWRITE-0368`,
`TS-SERVER-0129`, `TS-SERVER-0171`, `TS-SERVER-0216`, `TS-SERVER-0267`

这些 case 的 in-place 半可测，但 TS 断言了以下至少一项，native 目前没有等价字段：
symbol-bridge 布局、`diagnosticsExact`、`probeProblems`、`states` 聚合、
`suppressedDiagnostics`、C1 response envelope 或真实 checker 产物。可登记 weaker，
不能按 exact 升 green。

### 2.3 C：需要 checker/native 新能力（59）

- analysis（7）：`TS-ANALYSIS-0226/0243/0272/0287/0315/0332/0371`
- compiler（2）：`TS-COMPILER-0095/0132`
- component（3）：`TS-COMPONENT-0550/0628/0697`
- events（3）：`TS-EVENTS-0375/0387/0399`
- guards（5）：`TS-GUARDS-0128/0152/0174/0321/0329`
- layouts（3）：`TS-LAYOUTS-0084/0101/0147`
- locals（3）：`TS-LOCALS-0261/0288/0312`
- mapping（1）：`TS-MAPPING-0306`
- matrix（5）：`TS-MATRIX-0239/0252/0283/0319/0335`
- messages（5）：`TS-MESSAGES-0005/0016/0025/0032/0039`
- probes（5）：`TS-PROBES-0064/0080/0098/0135/0153`
- protocol（16）：`TS-PROTOCOL-0141/0154/0178/0234/0250/0329/0348/0368/0421/0450/0479/0500/0561/0606/0644/0653`
- server（1）：`TS-SERVER-0203`

### 2.4 D：需要契约裁决（41）

- 33 个 `TS-MATRIX-0102-<SCENARIO>-IN_PLACE_SHADOW` + `TS-MATRIX-0092`：
  C1 的 `states`、`diagnosticsExact`、`perturbsUserTypes`、
  `userScriptDiagnosticsIntroduced`、`request.ownerKind/fileCount/fragmentCount`、
  `moduleKind` 与两 candidate 运行器没有 native 等价物。
- compiler 池（3）：`TS-COMPILER-0050/0063/0086`
- C1 request/response（2）：`TS-PROTOCOL-0021/0395`
- C1 server ping/analyze（2）：`TS-SERVER-0094/0109`

### 2.5 E：应 retired/waiver（6）

- `TS-EVIDENCE-0011/0025/0050`：legacy evidence 写入/路径清洗工具；native 没有
  evidence writer，也没有 `assertEvidenceDir`。
- `TS-META-0027`：C1 contract pin；F0 已用 engine/type-asset pin 替代。注意台账
  validator 目前只允许它是唯一非 obsolete 的 `requires_decision` case，退休需要
  同步改 validator/decision 记录。
- `TS-COMPONENT-0774`：symbol-bridge 专属行为；生产已退役 Candidate B。
- `TS-REWRITE-0378`：1 MiB / 4 MiB Node 侧 perf 阈值；native 的 perf gate 是
  `TS-PERF-0012` structural，不是同一采样。

### 2.6 分区分布

| 分区 | no-evidence | A | B | C | D | E |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `behaviors` | 4 | 0 | 0 | 3 | 0 | 1 |
| `completion` | 1 | 0 | 1 | 0 | 0 | 0 |
| `components` | 3 | 0 | 0 | 3 | 0 | 0 |
| `events` | 3 | 0 | 0 | 3 | 0 | 0 |
| `guards_js` | 12 | 2 | 0 | 10 | 0 | 0 |
| `hover_docs` | 3 | 3 | 0 | 0 | 0 | 0 |
| `loops` | 7 | 0 | 2 | 5 | 0 | 0 |
| `mapping_utf16` | 6 | 0 | 4 | 1 | 0 | 1 |
| `messages_evidence` | 8 | 0 | 0 | 5 | 0 | 3 |
| `project_overlay` | 14 | 1 | 3 | 7 | 3 | 0 |
| `protocol_limits` | 38 | 5 | 11 | 17 | 4 | 1 |
| `scenario_matrix` | 43 | 0 | 4 | 5 | 34 | 0 |
| `shadow_rewrite` | 1 | 1 | 0 | 0 | 0 | 0 |
| **合计** | **143** | **12** | **25** | **59** | **41** | **6** |

---

## 3. 下一波 28 个最高收益 case

选择原则：先取能真正减少 planned 的 A 类；再取可用现有 in-place 管线登记 weaker
的 B 类；最后保留 4 个必须做裁决/能力判断的高杠杆 case，防止把 blocked 误写成
green。

### 3.1 exact 候选（12）

| Case | Oracle | 现有 Rust 落点 | 缺什么 | 预计 |
| --- | --- | --- | --- | --- |
| `TS-PROBES-0048` | `backends/typescript/test/probes.test.ts:48` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2159 | 在 `layout.rs` 私有 tests 加 table：`@ts-check` 后 `@ts-nocheck` 的 `check/no_check/neutralize_offset` 精确值 | exact |
| `TS-PROBES-0059` | `backends/typescript/test/probes.test.ts:59` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2159 | 加 leading-comment-only 断言；块外 `@ts-nocheck` 必须不生效 | exact |
| `TS-ANALYSIS-0196` | `backends/typescript/test/analysis.test.ts:196` |  crates/wxml-backend-tsgo/src/shadow/rewrite.rs:335 | 加 `resolve_target` 负例：非 whole identifier/字符串内 target -> typed error | exact |
| `TS-ANALYSIS-0349` | `backends/typescript/test/analysis.test.ts:349` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:686 | 加重复 `fragment.id` -> `ShadowError::Unsupported { feature: "fragment.id" }` | exact |
| `TS-PROTOCOL-0126` | `backends/typescript/test/protocol.test.ts:126` |  crates/wxml-backend-tsgo/src/shadow/locals.rs:149 | 加 same-name nested chain 被接受、无 TDZ 的 `group_local_levels` 断言 | exact |
| `TS-PROTOCOL-0280` | `backends/typescript/test/protocol.test.ts:280` |  crates/wxml-backend-tsgo/src/input.rs:198 | 扩展 `CompilerOptions` 测试：未知 key 拒绝，`strictNullChecks/noUncheckedIndexedAccess/strictBuiltinIteratorReturn` 保留 | exact |
| `TS-PROTOCOL-0319` | `backends/typescript/test/protocol.test.ts:319` |  crates/wxml-backend-tsgo/tests/shadow_layout.rs:550 | 已有 policy-off test；补 `nocheck` insertion、`@ts-check` 后 `@ts-nocheck` 顺序 | exact |
| `TS-PROTOCOL-0600` | `backends/typescript/test/protocol.test.ts:600` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:176 | 加无 target/locals 的 legacy `FragmentSpec::new` 构建成功 | exact |
| `TS-SERVER-0150` | `backends/typescript/test/server.test.ts:150` |  crates/wxml-lsp/src/server.rs:2539 | 断言 notification 的 `handle_incoming` 不产生 response（可产生 diagnostics）；native 用任意 notification 等价 | exact |
| `TS-ANALYSIS-0161` | `backends/typescript/test/analysis.test.ts:161` |  crates/wxml-lsp/src/backend_tsgo/input.rs:977； crates/wxml-lsp/tests/zed_sync.rs:592 | fake view 带 unsaved script；断言 `opened_inputs` 文本、hover type/doc；不读磁盘 | 条件 exact |
| `TS-ANALYSIS-0184` | `backends/typescript/test/analysis.test.ts:184` |  crates/wxml-backend-tsgo/src/shadow/rewrite.rs:299； crates/wxml-backend-tsgo/src/shadow/mapping.rs:247 | 显式 target `nmae` 映射到 WXML 134..138；加 mapping/rewrite 测试 | 条件 exact |
| `TS-REWRITE-0197` | `backends/typescript/test/rewrite.test.ts:197` |  crates/wxml-lsp/src/backend_tsgo/diagnostics.rs:88 | fake diagnostic 落在 `nmae`，断言映射后的 WXML 134..138；不伪造 checker | 条件 exact |

低风险 9 个：除 `TS-ANALYSIS-0161`、`TS-ANALYSIS-0184`、`TS-REWRITE-0197`
之外的 A 类。条件 exact 若评审要求真实 checker，应降为 weaker，不能为了计数
强行 green。

### 3.2 weaker 候选（12）

| Case | Oracle | 现有 Rust 落点 | 缺什么 | 预计 |
| --- | --- | --- | --- | --- |
| `TS-ANALYSIS-0205` | `backends/typescript/test/analysis.test.ts:205` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:879 | mounts 的 text/segments 可断言；`probeProblems` 与三 capability `states` 无等价 | weaker |
| `TS-ANALYSIS-0255` | `backends/typescript/test/analysis.test.ts:255` |  crates/wxml-backend-tsgo/src/shadow/rewrite.rs:299 | target offsets 可断言；C1 response mounts 与 hover type 无同一 envelope | weaker |
| `TS-MAPPING-0188` | `backends/typescript/test/mapping.test.ts:188` |  crates/wxml-backend-tsgo/src/shadow/mapping.rs:328 | `trimmed_synthetic_prefix` 有；`diagnosticsExact`、message helper、symbol-bridge 无 | weaker |
| `TS-MAPPING-0208` | `backends/typescript/test/mapping.test.ts:208` |  crates/wxml-lsp/src/backend_tsgo/diagnostics.rs:88 | 诊断去重有；两个 layout 与 `diagnosticsExact` 无 | weaker |
| `TS-MATRIX-0215` | `backends/typescript/test/matrix.test.ts:215` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:521 | type-only data root 的 `Unsupported` 有；symbol-bridge 半与 C1 states 无 | weaker |
| `TS-MATRIX-0228` | `backends/typescript/test/matrix.test.ts:228` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:533 | shorthand data root 的 `Unsupported` 有；symbol-bridge 半无 | weaker |
| `TS-MATRIX-0301` | `backends/typescript/test/matrix.test.ts:301` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:510； crates/wxml-backend-tsgo/tests/shadow_layout.rs:367 | count/reason/first-wins 有；symbol-bridge 与 response envelope 无 | weaker |
| `TS-OVERLAY-0036` | `backends/typescript/test/overlay.test.ts:36` |  crates/wxml-lsp/src/backend_tsgo/input.rs:977 | JSDoc-only overlay 可断言；两个 layout、docSource、真实 checker 无 | weaker |
| `TS-PROBES-0176` | `backends/typescript/test/probes.test.ts:176` |  crates/wxml-backend-tsgo/src/native/vfs.rs:1385 | VFS 只读快照有；TS 的 `diskReadable` 与 2307 观测无同一 API | weaker |
| `TS-REWRITE-0349` | `backends/typescript/test/rewrite.test.ts:349` |  crates/wxml-backend-tsgo/src/shadow/mapping.rs:328； crates/wxml-backend-tsgo/tests/shadow_mapping.rs:284 | cross-gap crop 有；code/attribution/status 与 symbol-bridge 无 | weaker |
| `TS-SERVER-0129` | `backends/typescript/test/server.test.ts:129` |  crates/wxml-lsp/tests/lsp.rs:354 | unknown/-32700 有；`analyze` 的 `-32602 data.code` 与 `ping` 无 native 对应 | weaker |
| `TS-SERVER-0171` | `backends/typescript/test/server.test.ts:171` |  crates/wxml-lsp/src/server.rs:2564 | native 只有通用 `-32602`；`analyze` 的 `data.code=bad-params/bad-fragment` 无 | weaker |

### 3.3 blocked / waiver 候选（4）

| Case | Oracle | 现状 | 缺什么 | 预计 |
| --- | --- | --- | --- | --- |
| `TS-PROTOCOL-0395` | `backends/typescript/test/protocol.test.ts:395` | native 没有 C1 response envelope | `protocolVersion/requestId/states/moduleKind/insertions/mounts` 同一响应；新建 C1 层或书面 waiver | blocked |
| `TS-MATRIX-0092` | `backends/typescript/test/matrix.test.ts:92` | native 只有 in-place 单 candidate | 两 candidate runner；Candidate B 已退役，建议 waiver/retire | blocked/waiver |
| `TS-MESSAGES-0005` | `backends/typescript/test/messages.test.ts:5` |  crates/wxml-lsp/src/handlers/mod.rs:174 只做 control-char/truncate | `sanitizeMessage` 的 helper/typings/generated-name/intersection 语义；新增 sanitizer 或 waiver | blocked |
| `TS-COMPILER-0086` | `backends/typescript/test/compiler.test.ts:86` | native 没有 project pool | pool 上限本身无对应架构；建议 waiver/retire | waiver |

---

## 4. 可一次批量补的测试文件

`target_rust_test` 仍保持台账里的 partition 路径（如 `hover_docs.rs`、`guards_js.rs`），
真实证据写在 `migration_evidence.rust_tests[].path` 指向的实际测试文件；wave-8 的
green case 已经采用这种路由，不需要为了证据去改 `target_rust_test` 或
`MIGRATED_CASES`。

| 文件 | 建议 case | 说明 | 冲突 |
| --- | --- | --- | --- |
|  crates/wxml-backend-tsgo/src/shadow/layout.rs | `PROBES-0048/0059`, `ANALYSIS-0349` | 私有 `scan_pragmas` / duplicate fragment 可直接在同模块 tests 断言；批次最小 | `TS-PROBES-0027` 已由 `6fc7a00` 提交；追加在文件末尾，不覆盖 |
|  crates/wxml-backend-tsgo/tests/shadow_layout.rs | `PROTOCOL-0319`, `MATRIX-0215/0228/0301` 的 layout 半 | 已有 policy-off / multi-registration 测试，可扩展 | 文件当前干净；不要改 fixture hash |
|  crates/wxml-backend-tsgo/src/shadow/rewrite.rs | `ANALYSIS-0196/0184` | `resolve_target` 与 target offset 的纯函数测试 | 干净 |
|  crates/wxml-backend-tsgo/src/shadow/locals.rs | `PROTOCOL-0126` | `group_local_levels` 同层/嵌套断言 | 干净 |
|  crates/wxml-backend-tsgo/src/input.rs | `PROTOCOL-0280` | `CompilerOptions` 未知 key / strict switches | 干净 |
|  crates/wxml-lsp/tests/lsp.rs | `SERVER-0129/0150/0171` | 真实 stdio LSP 的 JSON-RPC 错误与 notification 无响应 | 干净 |
|  crates/wxml-lsp/src/server.rs（tests mod） | `SERVER-0150/0171` | 单测可直接断言 `Incoming -> Vec<Outgoing>` | 干净 |
|  crates/wxml-lsp/src/backend_tsgo/tests.rs | `ANALYSIS-0161/0184`, `MAPPING-0208`, `REWRITE-0197` | adapter/fake-engine 的 unsaved、target、dedupe；与现有 helper 同文件 | `ANALYSIS-0139/0174` 已由 `6fc7a00` 提交；基于该提交追加，仍建议单写者 |
|  crates/wxml-backend-tsgo/src/native/vfs.rs | `PROBES-0176`（weaker） | 已有 `reads_only_virtual_files_and_never_host_files`，可补 read-policy 断言 | 干净 |
|  crates/wxml-backend-tsgo/tests/shadow_mapping.rs | `MAPPING-0188/0208`, `REWRITE-0349`（weaker） | 已有 mapping/`BLOCKED_CASES` 文档，可扩展 | 干净；不要移除现有 blocked 列表 |

**不要碰**：

-  crates/wxml-lsp/tests/native_parity/ledger.rs
-  crates/wxml-lsp/tests/native_parity.rs
-  crates/wxml-lsp/tests/native_parity/oracle_manifest.rs / `oracle_manifest_tests.rs`
-  crates/wxml-lsp/tests/native_parity/fixture_hash.rs
-  docs/evidence/README.md
-  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs / `script_events.rs`
  （wave-8 刚改过，避免行号/断言冲突）

---

## 5. 风险

### 5.1 高

1. **ledger 是唯一事实源，且正在被并行写入。** wave-8 `75040c2` 把 48/255 改成
   55/248，wave-9 `6fc7a00` 又改成 58/245；任何回填必须重新读取 JSON、
   `counts.statuses` 和 validator 常量，不能复制本文件或旧 wave 文档的数字。
2. **wave-9 已经改过同一批测试文件。** `layout.rs`、`backend_tsgo/tests.rs`、
   `script_properties.rs` 的 0139/0174/0027 断言已由 `6fc7a00` 提交；新增测试必须
   基于该提交追加，不能覆盖。当前工作树仍有其他并行内容（以 `git status` 为准），
   仍需先检查再改。
3. **不要把 fake-engine/mapping-layer 结论当成真实 checker 结论。**
   `ANALYSIS-0161/0184`、`REWRITE-0197` 只有映射/输入层等价；若评审要求真实
   TS checker，应降为 weaker。

### 5.2 中

4. **symbol-bridge 是硬边界。** `MATRIX-0215/0228/0301`、`MAPPING-0188/0208`、
   `OVERLAY-0036` 等 TS 都遍历两个 layout；native 只保留 in-place。没有 layout
   waiver 就不能 exact。
5. **行号会漂移。** wave-8 与并行 WIP 已移动 `layout.rs`/`tests.rs` 行号；本文件的
   `path:line` 是 `75040c2` 读取值，回填前必须重新 `grep`。
6. **ledger validator 的计数是硬编码。** 新增 exact 需要同时改
   `GREEN_CASES/PLANNED_CASES/WEAKER_CASES`；新增 weaker 还要改 `WEAKER_CASES`。
   任何“只改 JSON 不改常量”都会让 `cargo test -p wxml-lsp --test native_parity` 红。

### 5.3 低

7. A 类中纯函数测试（PROBES、target、duplicate fragment、CompilerOptions、
   locals）没有生产路径变更，风险最低；建议先做这 9 个（8 个 shadow + 1 个
   server）。

---

## 6. 明确不可伪造项

以下任一出现，都不能算 exact；一旦发现，应改为 weaker/blocked 并记录原因：

1. **C1 envelope 字段**：`viewRevision`、`backendFileVersion`、`moduleKind`、
   `insertions[].file`、`mounts[].file`、`request.compilerOptionsDigest`、
   `response.protocolVersion/requestId`、`states` 聚合。native LSP 没有同响应。
2. **C1 运行指标**：`diagnosticsExact`、`perturbsUserTypes`、
   `userScriptDiagnosticsIntroduced`、`suppressedDiagnostics/suppressedDetails`。
3. **probe 专属字段**：`probeProblems`、probe health 的
   `intersects insertion`/`has no symbol`/`module classification flipped`。
4. **symbol-bridge/Candidate B 布局**：native 只构建 in-place shadow；任何同时断言
   两个 layout 的 case，不能只凭 in-place 通过就 exact。
5. **真实 checker 产物**：`2339`、`2353`、`2322`、`2365`、truthiness/nested/
   loop-local narrowing、`this.data` documentation/quality。fake engine 只能证明
   adapter 映射，不能证明 TS checker 语义。
6. **`sanitizeMessage` 语义**：native `sanitized_message` 只做 control-char/空白/
   截断，不删 `__wxml_root`/`__wxml_check_0`，不展开
   `FilterUnknownType`/`PropertyOptionToData`，不折叠重复 intersection。
7. **`TS-EVIDENCE-*` 的 evidence writer/path policy**：native 没有
   `assertEvidenceDir`/`sanitizeEvidence`；不能用通用路径清洗冒充。
8. **project pool**：native 是 session update/restart，不是 `MAX_POOLED_PROJECTS`
   LRU；不能用“重启一次”冒充 pool 复用/上限。
9. **C1 server 的 `ping`**：native LSP 没有自定义 `ping` 方法；用 `initialize` 或
   `textDocument/hover` 只能证明“错误后仍存活”，不能证明 `ping` 契约。
10. **oversized frame 语义**：native `jsonrpc` 对超长 frame 是 fatal/退出；TS
    legacy server 是丢弃后继续。`TS-SERVER-0203` 不能按现有 native 行为 exact。
11. **option digest / enum 校验**：native `options_digest` 直接序列化 options，
    `baseUrl` 绝对路径不同会改变 digest；native 也不把 target 字符串映射成
    `ts.ScriptTarget`。`TS-PROTOCOL-0234/0348/0479/0500` 不能伪造。
12. **`gate_green` / `#[ignore]` native gate**：opt-in `native_engine_real` 通过
   不等于 case `green`；不能在默认套件缺失时把 gate 结果写成 case 证据。

---

## 7. 并行 owner 建议

建议 4 个实现 owner + 1 个 ledger owner，互不重叠：

| Owner | 范围 | 文件 | 备注 |
| --- | --- | --- | --- |
| `wave9-shadow` | A 类纯函数 8 个：`PROBES-0048/0059`、`ANALYSIS-0196/0349`、`PROTOCOL-0126/0280/0319/0600`；另加 `MATRIX-0215/0228/0301` 的 layout 半 | `shadow/layout.rs` tests、`shadow/rewrite.rs` tests、`shadow/locals.rs` tests、`input.rs` tests、`tests/shadow_layout.rs` | 先做，零生产改动；与当前 `PROBES-0027` WIP 同文件，串行追加 |
| `wave9-adapter` | A 条件 3 个 + B adapter/mapping 8 个：`ANALYSIS-0161/0184`、`REWRITE-0197`、`MAPPING-0188/0208`、`MATRIX-0215/0228/0301` 的 adapter 半、`OVERLAY-0036`、`ANALYSIS-0205/0255` | `backend_tsgo/tests.rs`、`backend_tsgo/diagnostics.rs`、`shadow_mapping.rs` | `ANALYSIS-0139/0174` 已由 `6fc7a00` 提交，可基于该提交追加；`shadow_layout.rs` 归 shadow owner，避免同文件冲突 |
| `wave9-lsp` | `SERVER-0150`（exact）+ `SERVER-0129/0171/0216/0267`（weaker） | `tests/lsp.rs`、`src/server.rs` tests | 与 shadow/adapter 无文件交集 |
| `wave9-decision` | D/E 共 47 个：C1/matrix/compiler/evidence/meta/perf 的 waiver/retire 文档 | 只读台账 + 新 evidence/ADR；不改测试 | 需要 integrator 签字；不要在 wave-9 代码评审里夹带 |
| `wave9-ledger` | 代码 owner 全部提交后统一回填 | `docs/plans/tsgo-test-ledger.json`、`2026-11-17-tsgo-test-ledger.md`、`tsgo-test-contract.md`、`native_parity/ledger.rs` 常量、`README.md` | 单写者；每次改前重新读取 JSON 和并行提交 |

合并顺序建议：

1. `wave9-shadow` 与 `wave9-lsp` 并行；
2. `wave9-adapter` 在当前 adapter WIP 提交后开始；
3. `wave9-decision` 并行产出 waiver 清单，不改 status；
4. `wave9-ledger` 最后一次性回填 exact/weaker 计数；
5. 不要先改 `ledger.rs` 常量再等测试；否则并行会话的 `native_parity` 会红。

---

## 8. planned 能降到多少

以实际 `58 green / 245 planned / 143 no-evidence` 为基线：

| 方案 | green | planned | exact | assessed | 无证据 planned |
| --- | ---: | ---: | ---: | ---: | ---: |
| 当前 | 58 | 245 | 58 | 160 | 143 |
| 保守：9 个低风险 exact + 12 个 weaker | 67 | **236** | 67 | 181 | 122 |
| 推荐：12 个 A exact + 12 个 weaker | 70 | **233** | 70 | 184 | 119 |
| 仅 A exact（不登记 weaker） | 70 | **233** | 70 | 172 | 131 |
| A exact + 全部 25 个 B weaker | 70 | **233** | 70 | 197 | 106 |

若 wave-9 回填被回退到 wave-8 的 55/248，本表整体 +3 planned（推荐 **236**，
保守 **239**）；无证据 planned 仍从 143 起算。

关键结论：

- **不新增 native 能力、只做低风险测试，planned 最低约 236；** 把 3 个条件
  exact 也算上，**最低约 233**。
- 这 12 个 A 类之外，25 个 B 类最多只能登记 weaker，不能降低 planned；
  59 个 C 类需要 checker/native 能力，41 个 D 类需要 C1/池/机制裁决，6 个 E 类
  需要 retire/waiver。
- 即使将剩余 131 个无证据 case 全部 waiver/retire，planned 仍有 **102** 个
  已评估的 weaker/near-exact/structural/blocked。P7-1 的 `planned=0` 还需要第二轮
  处理这 102 个；wave-9 不能单独完成 P7-1。
- 若 integrator 对 D/E 的 47 个出书面 waiver，并对 B 的 25 个出 layout/字段
  waiver，planned 名义上可降到 161（C 59 + 已评估证据 102）；若再把 C 59 也
  waiver，才到 102（仅剩已评估证据）。这不是 exact，也不应写成 green。

---

## 9. 验证

本文件是只读规划；没有运行全量测试。下一波实现后至少执行：

```text
cargo test -p wxml-backend-tsgo --lib
cargo test -p wxml-backend-tsgo --test shadow_layout
cargo test -p wxml-backend-tsgo --test shadow_rewrite
cargo test -p wxml-backend-tsgo --test shadow_mapping
cargo test -p wxml-lsp --lib backend_tsgo
cargo test -p wxml-lsp --test lsp
jq empty docs/plans/tsgo-test-ledger.json
cargo test -p wxml-lsp --test native_parity
cargo fmt --all -- --check
cargo clippy -p wxml-lsp -p wxml-backend-tsgo --all-targets -- -D warnings
git diff --check
```

回填时额外检查：

- `docs/plans/tsgo-test-ledger.json` 的 `counts.statuses`、
  `counts.migration_evidence.exact/weaker/remaining_planned_without_evidence`；
- `crates/wxml-lsp/tests/native_parity/ledger.rs` 的
  `GREEN_CASES`/`PLANNED_CASES`/`WEAKER_CASES`；
- 每个 `migration_evidence.rust_tests[].path/line/test` 在**回填时**真实存在；
- `docs/plans/2026-11-17-tsgo-test-ledger.md` 与 contract 的计数同步，但不要覆盖
  并行 wave-9/README 改动。

---

## 10. 未决与限制

1. 本文件没有逐条重读 143 个 TS 函数体；分类基于台账的断言摘要、Rust 代码中的
   `BLOCKED_CASES` 注释、现有 Rust 落点和 native 架构边界。A/B 边界在逐条实现时
   仍需按 wave-8 的“逐断言对照表”复核。
2. `TS-MATRIX-0102-*` 33 个 case 每个有约 24 条断言，本文件只按共享 harness 分类，
   没有逐个 scenario 审计。任何 scenario 的 exact 结论都必须重新列出 24 条断言。
3. 3 个条件 exact（`ANALYSIS-0161/0184`、`REWRITE-0197`）可能在评审时被降为
   weaker；保守计数请用 9 个低风险 exact。
4. `TS-META-0027` 是唯一非 obsolete 的 `requires_decision` case；退休会改动
   ledger validator 的 `non_obsolete_decision_ids` 约束，不能只改 status。
5. 本文件不声称 P7-1/P7-2 完成，也不声称任何 case 已 `reviewed` 或 `closed`。
