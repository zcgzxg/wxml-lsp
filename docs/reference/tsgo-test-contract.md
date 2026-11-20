# Rust + tsgo 迁移测试契约（F2）

日期：2026-11-17（2026-11-20 P7 删除完成后最终冻结）。状态：**accepted / frozen**；F2 用例账本终态为 **planned=0**（109 `green` / 130 `reviewed` / 29 `waived` / 71 `retired-with-decision`），C-lite owner-scope waiver 见 §9。

本契约定义“如何证明迁移没有回归”。原始 2026-11-17 版本只定义方法、不表示任何用例已经迁移；该历史状态已被 2026-11-20 P7-1 closeout 取代。逐用例的唯一事实源是
[`docs/plans/tsgo-test-ledger.json`](../plans/tsgo-test-ledger.json)，审阅摘要见
[`docs/plans/2026-11-17-tsgo-test-ledger.md`](../plans/2026-11-17-tsgo-test-ledger.md)。
原生引擎与查询契约见 [`tsgo-native-contract.md`](tsgo-native-contract.md)。

## 1. 范围与计数

| 项 | 冻结值 |
| --- | ---: |
| TS 测试文件 | 21 + `support.ts` |
| 静态 `test()` 声明 | 274（273 字面 + 1 动态） |
| 展开注册 | 339 |
| `direct` | 240 |
| `rewrite` | 51 |
| `split` | 11 |
| `obsolete` | 37（36 个历史 Candidate B + wave-11 `TS-COMPONENT-0774`） |
| `blocked-decision` | 0 |
| `green` | 109（真实 Rust 测试证据，exact-only） |
| `planned` | **0**（P7-1 closeout） |
| `reviewed` | 130（非 exact 证据 + owner 接受残余风险） |
| `waived` | 29（22 无证据 + 7 named known defect） |
| `retired-with-decision` | 71（37 obsolete + 34 非 obsolete） |
| `assessed_cases` | 246 |
| `exact` | 109 |
| `near_exact` | 3 |
| `weaker` | 96 |
| `structural` | 7 |
| `blocked` | 31（24 reviewed + 7 known-defect waiver） |
| `waived_without_evidence` | 22 |
| `known_defects` | 7 |

**2026-11-20 C-lite closeout**：owner 明确“不需要严格按照原来的功能写测试，
功能合理即可”，据此把剩余 192 个 `planned` 收口为 33
`retired-with-decision` + 130 `reviewed` + 29 `waived`，`planned=0`。
`reviewed` 保留 `near_exact`/`weaker`/`structural`/`blocked` 原值，不写成
`exact`；`waived` 是没有 Rust 证据或 named known defect 的 owner-signed
gap；`retired-with-decision` 不携带 `migration_evidence`。四者都不计入
`exact`/`green`。逐条分类见
[`docs/evidence/2026-11-20-tsgo-p7-closeout-ledger.md`](../evidence/2026-11-20-tsgo-p7-closeout-ledger.md)，
策略见
[`docs/evidence/2026-11-20-tsgo-p7-ledger-waiver-policy.md`](../evidence/2026-11-20-tsgo-p7-ledger-waiver-policy.md)。
这不是 parity 完成声明，也不是 Node/旧 TS 删除授权。

此前 wave-11 matrix backfill（2026-11-20，验证 HEAD `1c89338` + 未提交
`real_matrix.rs`）后为 `green` 109 / `planned` 192 / `retired-with-decision` 38，
`assessed_cases` 248（`exact` 109、`near_exact` 3、`weaker` 96、`structural` 7、
`blocked` 33），无证据 planned 53。该步把 22 个 in-place + 4 个顶层 in-place
半边提升为 exact `green`，并把 11 个 in-place + 6 个顶层 blocked 半边登记为
`parity=blocked`、`blocked_by=checker`（DOCS 4、STATE 8、PROBE 2、ROOT 2、
CHECKER 1）。`scenario_matrix` 从 4 evidence/39 unassessed 变为 43 evidence/
26 green/17 blocked；projection SHA 固定为 `9d47f7d0…`。`methods-nonliteral`
的语义修复见 ` docs/evidence/2026-11-20-tsgo-matrix-semantic-gaps.md`。此前 wave-11 blocked
followup（2026-11-20，基线 HEAD `1c89338`）后为 `green` 83 / `planned` 218 /
`retired-with-decision` 38，`assessed_cases` 209（`exact` 83、`near_exact` 3、
`weaker` 100、`structural` 7、`blocked` 16），无证据 planned 92。该步把 `TS-COMPONENT-0292` 提升为 exact `green`，
把 `TS-EVENTS-0188` 登记为 near_exact（唯一差异 `fileId`→`EventDefinition.path`），
并把 `TS-COMPONENT-0193`、`TS-GUARDS-0201`、`TS-LOCALS-0343`、`TS-COMPONENT-0498`
从 `blocked` 重新登记为 `weaker`（`blocked` 22→16）。此前 wave-11 exact 回填
（2026-11-20，HEAD `68f852c`）后为 `green` 82 / `planned` 219 /
`retired-with-decision` 38，`assessed_cases` 209（`exact` 82、`near_exact` 2、
`weaker` 96、`structural` 7、`blocked` 22），无证据 planned 92。wave-11 先把
`TS-COMPONENT-0774`（`candidate_b_layout`）与 `TS-META-0027`（`superseded_contract`）
移到 `retired-with-decision`，再把 `TS-ANALYSIS-0315/0332` 与 10 个 messages/probes case
提升为 exact `green`（`exact` 70→82）。此前的 wave-10 conditional 回填（2026-11-20）后为
`green` 70 / `planned` 233 / `assessed_cases` 197（`exact` 70、`near_exact` 2、`weaker` 96、
`structural` 7、`blocked` 22），无证据 planned 106；wave-10b weaker 回填结束时为 68/235/194
（`weaker` 95、无证据 109），wave-10 exact 回填结束时为 68/235/181
（`weaker` 82、无证据 122），wave-10 near-exact 跟进结束时为 59/244/172
（`exact` 59、`near_exact` 2、无证据 131），wave-10 weaker 回填结束时为
58/245/172（`weaker` 82、无证据 131），wave-9 台账回填结束时为 58/245/160
（`weaker` 70、无证据 143），wave-8 断言补齐回填结束时为 55/248/160
（`weaker` 73），wave-7 台账回填结束时为 48/255/160
（`weaker` 80），wave-6 组件绑定跟进结束时为 48/255/154
（`weaker` 74、`blocked` 22），wave-6 回填结束时为 44/259/154（`weaker` 80、
`blocked` 20），wave-5 后为 44/259/145，P6 回填（2026-11-19）后为 36/267，
P5/P5.1 的历史快照见台账 §14，wave-5/wave-6/组件绑定/wave-7/wave-8/wave-9/wave-10/wave-10b/wave-10-conditional/wave-11/wave-11-blocked-followup 见 §15/§16/§17/§19/§21/§22/§23/§24/§25/§26。

`obsolete` 不等于“未迁移的通过项”。37 个 Candidate B/symbol-bridge 用例（36 个历史 +
wave-11 `TS-COMPONENT-0774`）只作为历史 A/B 证据保留；生产 LSP 不传 `layoutId`，原位 shadow
用例仍全部强制。`TS-META-0027` 已按 `superseded_contract` 退休，由 F0 引擎/类型资产 pin 测试
替代；C1 文档的版本段已改为 native `7.1.0-beta`。

## 2. Oracle 与 fixture 冻结

- 测试源码树（`backends/typescript/test`）：`630a16af41623c78f19f47bcaba1a7f1282201ac27b4414115ac0b3764afc32e`
- samples：`00a7fa3d6962da50581c396f73c13f7ffb043efae060a0801b373928726e5c1f`
- `tests/fixtures`：`7c9ddedda18c70bc217af6b3bbd23dd22a33be9c885276cbd587adc0596f04a9`
- `crates/wxml-lsp/tests/fixtures`：`834eea4defcb7565c4b84e91a173079cf3be85201d44fb4a7a6d0a32af3da073`

算法（v1）：regular files only，拒绝 symlink；按 UTF-8 相对路径字节排序；对每个文件累加
`relative_path_utf8 + NUL + raw_bytes + NUL` 后取 SHA-256。任何 fixture 变更必须同时更新台账与 hash，
不得只改 hash 掩盖内容变化。

行尾（M5）：v1 hash 基于 LF checkout 的原始字节；CRLF checkout 会改变 hash。本地和 CI 必须保持 LF
（`git config core.autocrlf false`，仓库 `.gitattributes` 约束），不得通过改 hash 掩盖行尾变化。

### 2.1 P7-2 预删除 oracle 清单

`backends/typescript/test` 在 P7-4 删除后，台账里的旧 TS 锚点不能再从 live 源重算。P7-2 增加只读冻结
清单 [`docs/plans/tsgo-legacy-oracle-manifest.json`](../plans/tsgo-legacy-oracle-manifest.json)：

- schema `wxml-tsgo-legacy-oracle-manifest/1`；`baseline.head = 807a1578173992ad16f15e22b45f8dd62b024df3`
  （生成时的 HEAD，不是当前 checkout）。
- 21 个测试文件的 `sha256`、`lines` 与声明计数；`support.ts` 的 `sha256`/`lines`；339 个 case 的
  `case_id`/`file`/`line`/`body_sha256`；4 棵树的 `files`/`bytes`/`sha256`/`archived_ok`。
- 排序：case 按 `case_id`、文件与树按路径的 UTF-8 字节序（`LC_ALL=C`），数组必须严格递增；
  重复、未排序、缺字段、数量不符都是失败。
- 完整性：`projection_sha256` 是语义投影的 SHA-256；Rust 侧
  `FROZEN_MANIFEST_SHA256 = 14cd870e8ce7d15294285fe9205bb98bb121164e44f8c67ee3c433aa43508123`
  再钉一次。改任何字段必须同时改清单与 Rust 常量，不能只改清单。
- 双源校验：live 树存在时仍按 v1 从字节重算文件 hash、行数和每个 case 的 `body_sha256`，
  并要求 live、台账、清单三者一致（防止删除前漂移）；live 树不存在时逐项对照台账
  （文件 hash、case 行/body、support、4 棵树），绝不降级为 skip 或“无验证”。
- 只有 `backends/typescript/test` 与 `backends/typescript/samples` 允许在清单标记
  `archived_ok=true` 后缺席；`tests/fixtures` 与 `crates/wxml-lsp/tests/fixtures` 必须始终存在。

清单不是第二台账：case 状态、disposition、Rust 证据仍以 `tsgo-test-ledger.json` 为唯一事实源；
清单只冻结旧 TS 的字节锚点，供删除后审计。

## 3. 逐用例记录

每个 case 至少保留：`case_id`、旧文件/标题/行、`body_sha256`、完整展开标题、参数子 ID、断言摘要、
覆盖维度、fixture、目标 Rust 全限定测试 ID、比较类型、RED/GREEN 原始证据、reviewer、原生身份
（engine version/gitHead/AST version/artifact hash）。P7 closeout 后 `status` 是以下终态之一：

```text
planned → green                    (仅 parity=exact；green 之后可 closed)
planned → reviewed                 (有真实证据；owner 接受非 exact 残余风险；非 green)
planned → waived                   (无 Rust 证据，或 named known defect；owner 签字；非 green)
planned → retired-with-decision    (机制/契约/内部证据被替换；无 migration_evidence)
green   → closed                   (post-green 评审收口)
```

`planned=0` 是 P7-1 的收口条件；`planned` 只用于迁移过程。

`retired-with-decision` 的合法来源：

- `disposition=obsolete`：沿用 2026-11-17 Candidate-B 裁决（36 个历史 case），或带
  `retirement_decision.scope=candidate_b_layout`（wave-11 `TS-COMPONENT-0774`）；
- `direct/rewrite/split`：必须带 `retirement_decision` 对象，包含稳定 `id`、`date`、`owner`、
  `required_by`、`scope`、非空 `evidence` 与 `replacement`；`scope` 允许
  `candidate_b_layout`、`superseded_contract`、`mechanism_replaced`、`internal_evidence`、
  `owner_scope_waiver`。新增 scope 必须同时扩展 validator、本契约与 case 证据。

`reviewed` 必须带 `migration_evidence` 和 `review`（`group_id` + 非空 note），
parity 只能是 `near_exact`/`weaker`/`structural`/`blocked`；`green_eligible` 必须为
false。`waived` 必须带 `waiver`（`group_id`、`gap_kind`、非空 note/follow_up）：
`gap_kind=no-rust-evidence` 时不得有 `migration_evidence`；`gap_kind=known-defect`
时必须保留 `parity=blocked` 证据并写 `product_impact`。`waived` 不是 `reviewed`。

不得把 `planned` 批量改成 `green`；不得用“旧测试已删除”代替新测试通过。任何 `obsolete` 必须有
集成负责人裁决与代码证据。`reviewed`/`waived`/`retired-with-decision` 都是终态非 green 桶，
不是迁移证据，不得计入 `exact` 或 `green`。`green` 必须 `parity=exact`；`parity=exact`
必须 `green`。

## 4. 差分比较规则

允许归一化：

- 临时根路径与工作区路径前缀的一一对应；
- 请求 ID、进程 ID、时间戳、明确的非语义序号；
- 明确声明为无序集合的引用/诊断/补全列表。

禁止归一化：

- 诊断 code、severity、range、message 的可观察语义；
- hover 的类型字符串、文档、JSDoc、来源链接；
- definition/reference/rename 的目标文件、range、symbol 身份与编辑安全条件；
- completion 的 label、kind、insertText、排序等级；
- 空结果与“查询失败/引擎缺失”的区分。

比较器必须有反向自测：注入缺失结果、range 偏移、文档丢失、越界编辑、双空结果，均必须失败。
双空结果不得自动 PASS。

## 5. 原生执行门禁

- 缺少原生引擎、资产 hash 不符、协议版本不符：测试必须失败，不得 skip/return。
- `semantic_tools` 中缺少 `dist/server.js` 的两个静默 return 必须改为原生强制门禁：
  - `native_component_property_edit_applies_and_reparses`
  - `native_rejects_cross_namespace_and_dynamic_edits`
- 23 个现有 `#[ignore]` 必须逐项映射为必跑原生命令；在最终无 Node 候选构建中不得保持 ignored。
- 性能测试权威门禁是串行 `perf.test.ts`；全量并行结果只作环境记录。阈值 8000 ms 与测试内容不变。
- 原生不可用的平台必须显式失败并给出平台/资产原因，不得回退 Node。

## 6. 测试分区

公共入口：`crates/wxml-lsp/tests/native_parity.rs`。

分区：`hover_docs`、`definitions`、`completion`、`loops`、`guards_js`、`components`、`behaviors`、
`events`、`project_overlay`、`symbols_edits`、`scenario_matrix`、`protocol_limits`、`messages_evidence`、
`shadow_rewrite`、`mapping_utf16`、`perf_budget`。每个文件单一 owner；纯 codec/map/rewrite 单元测试放在
生产模块旁的 `#[cfg(test)]`，不得用 LSP 矩阵代替底层合同测试。

## 7. 必跑矩阵

1. 全 workspace `cargo fmt --check`、`cargo check --workspace --all-features`、
   `cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。
2. 原生 parity 全分区 + 23 个原 ignored 门禁。
3. 坐标矩阵：ASCII/CJK/emoji/BOM/CRLF；API/AST UTF-16、字符串池 UTF-8/WTF-8、孤立 surrogate。
4. codec/协议：畸形长度、越界、循环、未知 kind、超大 frame、快照 v1→v2 迟到、lease/epoch、stale handle。
5. 恢复：子进程崩溃、超时、重启、未保存更新、旧结果丢弃。
6. FS/安全：缺失回调 fail-closed、路径逃逸、symlink、写/emit 拒绝、`--runExternalCode` 禁止。
7. 性能：串行冷启动/热查询/诊断/取消，200 样本，现有 p95 回退 ≤10%，RSS ≤15%。
8. 无 Node：PATH 无 node/npm、进程审计、`--offline`、打包布局、Linux Zed 冒烟。

## 8. 退出条件

F2 冻结需要：原生契约 F0/F1 已评审；测试分区 owner 已指定；比较器反向自测通过；fixture v1 hash
可复现；37 个 `obsolete` 与 1 个 `superseded_contract` 有裁决记录。

G5 / P7-1 closeout 需要：

- 全部 `direct/rewrite/split` 用例为 `green` / `reviewed` / `waived` /
  `retired-with-decision`，且有对应证据或记录；`planned=0`；
- `green` 只允许 `parity=exact`，并且 `parity=exact` 只能是 `green`；
- `reviewed`/`waived`/`retired-with-decision` 不计入 `exact`，不得描述为
  “已迁移/已等价”；
- 7 个 named known defect 必须在 `waived` 中逐条列出，带 `product_impact`
  和非空 `follow_up`；
- 22 个无证据 waiver 必须按 `gap_kind` 分组并带 follow-up；
- `closeout` 记录 owner/date/scope/planned_target 和 group 计数；
- 没有真实引擎缺失导致的 skip，且 23 个 ignored 与 2 个静默 return 已转为
  强制门禁。

**G5 明确接受 `reviewed`/`waived` 作为 C-lite closeout**，但这不是 parity
完成：`reviewed` 只管“有证据、owner 接受残余风险”，`waived` 只管“无证据或
known defect、owner 签字”。两者都不能被任何脚本、报告或 README 写成
`green`/`exact`。

## 9. C-lite owner-scope waiver（2026-11-20）

Owner 决策：不需要严格按照原来的 TypeScript/Node 功能写测试，功能合理即可。
因此 P7-1 的收口状态是：

| 桶 | 数量 | 含义 | 是否 green/exact |
| --- | ---: | --- | --- |
| `green` | 109 | exact parity + 真实 Rust 测试 | 是 |
| `reviewed` | 130 | 96 weaker + 3 near_exact + 7 structural + 24 blocked 覆盖缺口 | **否** |
| `waived` | 29 | 22 无 Rust 证据 + 7 named known defect | **否** |
| `retired-with-decision` | 71 | 37 obsolete + 34 非 obsolete（30 机制/契约 + 3 internal evidence + 1 历史） | **否** |
| `planned` | 0 | P7-1 目标 | 否 |

逐条分类、group 和残余风险见
[`docs/evidence/2026-11-20-tsgo-p7-closeout-ledger.md`](../evidence/2026-11-20-tsgo-p7-closeout-ledger.md)；
策略与回滚见
[`docs/evidence/2026-11-20-tsgo-p7-ledger-waiver-policy.md`](../evidence/2026-11-20-tsgo-p7-ledger-waiver-policy.md)。

**不可用替代**：不能用“旧测试已删除”代替 waiver；不能把 `reviewed` 批量改成
`green`；不能把 known defect 改成 `reviewed`；不能因为 `planned=0` 就删除
Node/旧 TS。P7 删除于 2026-11-20 在其前置门禁（23 个 gate 的
`mandatory_green`/`replacement_reviewed`、Frozen oracle、no-Node guard、G6/G7 和
真实安装/升级/回滚验收）完成后执行；上述规则仍是删除后的审计基线。
