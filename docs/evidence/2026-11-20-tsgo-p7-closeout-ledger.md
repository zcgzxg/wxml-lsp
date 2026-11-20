# tsgo P7 closeout ledger（2026-11-20）

> **状态：历史归档（Superseded）。** 本文件记录 2026-11-20 P7 closeout 当时
> 的状态与数字；P7 已由 tag `p7-removed` 完成删除，Node/旧 TS 已删除。下文
> 所有“未删除 / `deletion_allowed=false` / 尚未完成”表述均为当时快照，不代表
> 当前状态；当前状态见 `docs/plans/2026-11-17-rust-tsgo-migration.md` §12.16。

## 0. 结论

P7-1 台账已按 **C-lite owner-scope waiver** 收口：

| 状态 | 数量 | 说明 |
| --- | ---: | --- |
| `green` | 109 | 仅 `parity=exact`，有真实 Rust 测试 |
| `planned` | 0 | P7-1 目标 |
| `reviewed` | 130 | 96 weaker + 3 near_exact + 7 structural + 24 blocked |
| `waived` | 29 | 22 无 Rust 证据 + 7 named known defect |
| `retired-with-decision` | 71 | 37 obsolete + 34 非 obsolete |
| 合计 | 339 | |

`reviewed` / `waived` / `retired-with-decision` 都是**非 green 终态**：不计入
`exact`，不写成“已迁移/已等价”。`green` 仍然只允许 `parity=exact`；
`parity=exact` 也只能是 `green`。

Owner 决策：**不需要严格按照原来的 TypeScript/Node 功能写测试，功能合理即可**。
本文件只记录收口分类和理由，不声称 parity、不声称 Node/旧 TS 已删除
（closeout 当时口径；后续已由 tag `p7-removed` 完成删除）。

## 1. 计数与不变量

| 项 | 值 |
| --- | ---: |
| `green` | 109 |
| `planned` | 0 |
| `reviewed` | 130 |
| `waived` | 29 |
| `retired-with-decision` | 71 |
| `assessed_cases` | 246 |
| `exact` | 109 |
| `near_exact` | 3 |
| `weaker` | 96 |
| `structural` | 7 |
| `blocked` | 31（24 reviewed + 7 known-defect waiver） |
| `waived_without_evidence` | 22 |
| `known_defects` | 7 |
| `never_green` | 38 |

`never_green = 30 个新机制/契约退休 + 1 个 engine-id waiver
（`TS-COMPILER-0095`）+ 7 个 known defect`。这 38 个在本次收口后仍非 green。

## 2. 33 个 `retired-with-decision`

来源：30 个机制/契约 case + 3 个 internal evidence。每个 case 都带
`retirement_decision`（id/date/owner/required_by/scope/reason/product_impact/
evidence/replacement），且**不携带** `migration_evidence`。

| group | scope | 数量 | case |
| --- | --- | ---: | --- |
| C1 transport/envelope | `superseded_contract` | 16 | `TS-ANALYSIS-0226/0272/0371`、`TS-EVENTS-0375/0387/0399`、`TS-PROTOCOL-0178/0250/0329/0348/0368/0421/0450/0561/0606/0644` |
| C1 input policy | `superseded_contract` | 3 | `TS-PROTOCOL-0141/0154/0653` |
| Node pool/C1 lifecycle | `mechanism_replaced` | 6 | `TS-COMPILER-0050/0086`、`TS-PROTOCOL-0021/0395`、`TS-SERVER-0094/0109` |
| dual-layout/old placement | `mechanism_replaced` | 2 | `TS-COMPONENT-0279`、`TS-LAYOUTS-0062` |
| superseded contract | `superseded_contract` | 3 | `TS-LAYOUTS-0084/0101`、`TS-SERVER-0203` |
| internal evidence tooling | `internal_evidence` | 3 | `TS-EVIDENCE-0011/0025/0050` |

理由：

- **C1**：native 路径不实现 C1 transport/envelope，也没有 native 消费者；C1
  的 malformed/duplicate 输入策略被 native 的去重/截断/限制策略替代。
- **Node pool/C1 lifecycle**：`MAX_LIVE_SNAPSHOTS=8` ≠
  `MAX_POOLED_PROJECTS=2`；native 是单引擎/session 模型，不再有 Node pool
  身份或 C1 request/response。
- **dual-layout**：symbol-bridge/old placement 已 retired；native 只使用
  in-place path-free shadow 和 `methods` probe placement。
- **superseded contract**：QuickInfo、union overlay 和 bad-frame 拒绝已被
  native 合同有意替代。
- **internal evidence**：旧 evidence sanitizer/absolute-path 断言是内部工具
  检查，由 native gate/closeout/freeze 证据替代。

注意：`TS-COMPONENT-0279` 和 `TS-LAYOUTS-0062` 原来有 blocked 证据；转
retired 时证据摘要已移入 `retirement_decision.evidence`，没有保留
`migration_evidence`。`TS-SERVER-0109` 的 title hover 替代不完全对称；本次
按 owner 的 C-lite 决策退休，若不接受应改为 waiver（见策略文档 §4.2）。

## 3. 29 个 `waived`

`gap_kind=no-rust-evidence` 的 22 个没有 `migration_evidence`；
`gap_kind=known-defect` 的 7 个保留 `parity=blocked` 证据并写
`product_impact` + `follow_up`。所有 29 个都非 green、非 reviewed。

### 3.1 无 Rust 证据（22）

| group | 数量 | case | 理由 |
| --- | ---: | --- | --- |
| checker-unavailable | 14 | `TS-GUARDS-0128/0152/0174/0321/0329`、`TS-LOCALS-0261/0288/0312`、`TS-MAPPING-0306`、`TS-COMPONENT-0550/0628/0697`、`TS-COMPILER-0132`、`TS-LAYOUTS-0147` | 默认 native 套件没有真实 TypeScript checker；checker narrowing/diagnostic/JSDoc 证据无法在默认套件产生。 |
| contract/API gap | 5 | `TS-ANALYSIS-0287`、`TS-COMPILER-0095`、`TS-PROTOCOL-0234/0479/0500` | 依赖 C1/Node 时代契约或 engine-project identity；native 没有等价语义。 |
| conditional exact not executed | 2 | `TS-ANALYSIS-0243`、`TS-COMPILER-0063` | 可以用小测试补齐，但 P7 收口前没有写；显式 waiver，不伪装 green。 |
| native perf missing | 1 | `TS-REWRITE-0378` | 旧的 1 MiB / 4 MiB rewrite 线性度断言没有 native 等价测试。 |

这 22 个的 `follow_up` 已写在 ledger 的 `waiver` 对象里；其中 14 个 checker
case 指向 required-native checker job。

### 3.2 Known defects（7）

| case | 当前风险 |
| --- | --- |
| `TS-COMPONENT-0570` | unresolved behavior 可能发布 2339（false positive） |
| `TS-COMPONENT-0676` | external behavior 未解析时整段跳过，不检查 own property（false negative） |
| `TS-SYMBOLS-0116` | `observerPath` 未实现；observer 半段是反面 |
| `TS-SYMBOLS-0299` | renamed destructuring 期望 complete=true，native 是 Partial |
| `TS-SYMBOLS-0425` | 另一个 script 的 `setData` 阻断；期望 per-page isolation/complete |
| `TS-COMPONENT-0229` | optional-object 期望 clean，native 是 Partial；是反例不是 weaker |
| `TS-DEFINITIONS-0036` | definition DTO 无 `kind`/generated relationship |

这 7 个是**named known defect**，不是“已接受的 weaker”。每个都带
`product_impact`、`follow_up`，并计入 `known_defects=7` 和
`never_green=38`。如果 owner 不签，P7-1 不应标记完成。

## 4. 130 个 `reviewed`

`reviewed` 保留原 parity，不写 exact；每个 case 都有 `review.group_id` 和
非空 note。组级理由见 ledger `closeout.review_groups`。

### 4.1 Weaker（96）

| 分区 | 数量 | case |
| --- | ---: | --- |
| behaviors | 7 | `TS-COMPONENT-0403/0416/0498/0591/0607/0654/0724` |
| completion | 4 | `TS-COMPLETION-0066/0078/0104/0207` |
| components | 2 | `TS-COMPONENT-0193`、`TS-LAYOUTS-0216` |
| definitions | 4 | `TS-DEFINITIONS-0013/0052/0074/0093` |
| events | 7 | `TS-EVENTS-0093/0162/0176/0236/0268/0280/0339` |
| guards_js | 2 | `TS-GUARDS-0201/0302` |
| hover_docs | 6 | `TS-ANALYSIS-0033/0054/0078/0093/0123/0161` |
| loops | 3 | `TS-ANALYSIS-0205/0255`、`TS-LOCALS-0343` |
| mapping_utf16 | 12 | `TS-MAPPING-0078/0091/0110/0145/0188/0208/0223/0239/0261`、`TS-REWRITE-0309/0349/0368` |
| project_overlay | 9 | `TS-ANALYSIS-0297/0358`、`TS-COMPILER-0035`、`TS-OVERLAY-0023/0036/0047/0055/0076`、`TS-PROBES-0176` |
| protocol_limits | 13 | `TS-META-0009`、`TS-PROTOCOL-0053/0098/0306/0438/0505/0525/0549`、`TS-SERVER-0129/0171/0216/0237/0267` |
| shadow_rewrite | 14 | `TS-REWRITE-0051/0078/0088/0101/0118/0127/0156/0183/0190/0224/0239/0250/0260/0285` |
| symbols_edits | 13 | `TS-SYMBOLS-0098/0164/0204/0242/0264/0286/0324/0336/0346/0367/0391/0406/0439` |

理由：有 Rust 证据，但只断言了旧 hover/diagnostic/reason/per-fragment 表面
的一部分；owner 接受剩余细节为残余风险。**狭义的 accepted weaker 只有这
96 个**，不要把 near/structural/blocked 也叫 weaker。

### 4.2 Near-exact（3）

`TS-COMPONENT-0521`、`TS-COMPONENT-0763`、`TS-EVENTS-0188`。差异是字段/
身份改名（例如 `fileId` → `path`），证据本身接近 exact，但 owner 没有选择
把 rename 接受为 exact。

### 4.3 Structural（7）

`TS-ANALYSIS-0380`、`TS-COMPONENT-0357/0476/0514/0539`、`TS-PERF-0012`、
`TS-REWRITE-0413`。Rust 测试断言的是 script facts、overlay/layout 结构或
性能层，不是旧 hover/checker payload 的同一可观察层。

### 4.4 Blocked 覆盖缺口（24）

| group | 数量 | case |
| --- | ---: | --- |
| matrix DOCS | 4 | `TS-MATRIX-0102-PAGE_BASIC`、`COMPONENT_PROPERTIES`、`COMPONENT_JS`、`TS-MATRIX-0271` |
| matrix STATE | 8 | `TS-MATRIX-0102-JS_JSDOC_OFF`、`TS_NOCHECK`、`JS_NOCHECK`、`JS_SOURCE_CHECK_OFF`、`JS_BOM_CHECK_OFF`、`TS-MATRIX-0239/0252/0283` |
| matrix PROBE | 2 | `TS-MATRIX-0102-NO_IMPLICIT_THIS_OFF`、`TS-MATRIX-0335` |
| matrix ROOT | 2 | `TS-MATRIX-0102-INTERFACE_ROOT_IMPORT`、`TS-MATRIX-0215` |
| matrix CHECKER | 1 | `TS-MATRIX-0102-PAGE_CLOSURE` |
| non-matrix checker | 7 | `TS-GUARDS-0097/0218/0261/0312`、`TS-LAYOUTS-0049/0072/0117` |

这些是覆盖缺口/能力未接线（`documentationSource`、`Diagnostics=Disabled`、
probe health、root classification、checker narrowing），不是已知反向行为；
因此可以 reviewed，但 `review.note` 已写明缺的是 DOCS/STATE/PROBE/ROOT/
CHECKER，并带 group follow-up。

> `counts.migration_evidence.by_partition.planned_evidence_cases` 是历史键名；
> P7 closeout 后它表示 **non-green evidence**（reviewed + known-defect
> waiver），不是 `planned`。`planned` 已为 0。

## 5. 38 个绝不能 green

| 组 | 数量 | 处置 |
| --- | ---: | --- |
| C1 transport/envelope/input policy | 19 | retired（`superseded_contract`） |
| Node pool/C1 lifecycle | 6 | retired（`mechanism_replaced`） |
| dual-layout/old placement | 2 | retired（`mechanism_replaced`） |
| superseded contract | 3 | retired（`superseded_contract`） |
| engine-id（`TS-COMPILER-0095`） | 1 | waived（`no-rust-evidence`，never-green） |
| known defect | 7 | waived（`known-defect`） |

`TS-EVIDENCE-0011/0025/0050` 也 retired，但它们是内部证据工具，不计入
product never-green。

## 6. 验证记录

本轮执行：

```bash
cargo test -p wxml-lsp --test native_parity ledger_
cargo test -p wxml-lsp --test native_parity
scripts/p7-freeze.sh --check
cargo fmt --all --check
```

结果：

```text
ledger_ tests: 8 passed, 0 failed
native_parity: 79 passed, 29 ignored, 0 failed
p7-freeze --check: OK (b65d76c; tracked trees and local dist match)
cargo fmt --all --check: clean
```

`p7-freeze` 的 `deletion_allowed` 仍是 `false`；23 个旧 gate 仍是
`gate_green`，不是 `mandatory_green`；Node/旧 TS 未删除
（以上均为 closeout 当时快照；删除已由 tag `p7-removed` 完成）。

## 7. 不声称

> 以下为 closeout 当时口径；P7 删除已由 tag `p7-removed` 完成，当前状态见
> `docs/plans/2026-11-17-rust-tsgo-migration.md` §12.16。

1. 不声称 192 个旧 planned 已迁移或通过；130 reviewed + 29 waived + 33
   retired 都是非 green。
2. 不声称 `reviewed` 等于 green/exact；`exact` 仍只有 109。
3. 不声称 7 个 known defect 已修复；它们是 named waiver，需要 follow-up。
4. 不声称 22 个无证据 case 已被测试覆盖；它们是 owner 接受的 gap。
5. 不声称 P7-1 的 gate 轴完成；23 个 gate 仍需 `mandatory_green`/
   `replacement_reviewed`。
6. 不声称 Node/旧 TS 可以删除；`deletion_allowed` 仍为 false
   （closeout 当时；tag `p7-removed` 已完成删除）。
