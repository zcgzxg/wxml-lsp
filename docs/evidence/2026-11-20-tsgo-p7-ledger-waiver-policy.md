# tsgo P7 台账收口策略：C-lite functional waiver（2026-11-20）

> 实施记录：本策略已于 2026-11-20 落地到 ledger/validator/contract/
> `p7-freeze`；最终分类与计数见
> ` docs/evidence/2026-11-20-tsgo-p7-closeout-ledger.md`。下文 §0 的
> “未改代码/未运行测试”只描述本文件最初的设计轮，不代表实施轮。

## 0. 结论摘要

- **推荐 B+：单账本、四终态**：`green` / `reviewed` / `waived` /
  `retired-with-decision`，P7-1 的 `planned=0` 由这四个终态共同满足。
  `green` 仍然只允许 `parity=exact`；`reviewed` / `waived` / `retired`
  都不计入 `exact`，不写成“已迁移/已等价”。这与
  ` docs/evidence/2026-11-20-tsgo-p7-scope-decision.md:7` 的 C-lite 定义一致：
  旧 C1/Node/dual-layout/superseded 以 retired 归档，其余以
  reviewed-as-weaker + residual risk 归档；不是新建账本。
- **不推荐 A**：把 192 个 planned 全部 `retired-with-decision` 会把
  96 个 weaker、24 个 blocked 覆盖缺口和 53 个无证据 case 混进同一个
  `retired` 桶，`retired` 占 68%，审计上无法区分“机制被替换”和
  “没测过”。这是形式上的 `planned=0`，不是可审计的收口。
- **不推荐 C（本阶段）**：新建 native acceptance ledger 会形成双源：
  旧账本仍有 192 planned，`p7-freeze.sh` 仍读旧账本，P7 门禁必须切换
  权威源；如果新账本只收录当前 green 的 case，就是移动球门，而不是
  记录残余风险。C 适合 P7 删除完成后的历史归档，不适合作为删除前的
  收口机制。
- **B+ 的目标计数**（2026-11-20 快照 `109 green / 192 planned / 38 retired`）：

  | 桶 | 数量 | 含义 |
  | --- | ---: | --- |
  | `green` | 109 | 仅 `parity=exact`，有真实 Rust 测试 |
  | `reviewed` | 130 | 96 weaker + 3 near_exact + 7 structural + 24 blocked 覆盖缺口；owner 接受残余风险，**非 green** |
  | `waived` | 29 | 22 无 Rust 证据 + 7 已知反向行为；owner 书面 waiver，**非 green、非 reviewed** |
  | `retired-with-decision` | 71 | 37 历史 obsolete + 34 非 obsolete（1 个历史 + 33 个本轮机制/契约退休） |
  | `planned` | **0** | P7-1 目标 |
  | 合计 | 339 | 109 + 130 + 29 + 71 |

- **绝不能 green 的 38 个**：30 个机制退休（C1 19 + D Node pool 6 +
  dual-layout 2 + superseded contract 3）、1 个 engine-id waiver
  （`TS-COMPILER-0095`）、7 个已知反向行为（见 §4.6）。这 38 个必须
  逐条有 `retirement_decision` 或 `waiver`，不能靠改 `status` 变绿。
- **可 retire 的 33 个**：上述 30 个机制/契约 case + 3 个 internal
  evidence（`TS-EVIDENCE-0011/0025/0050`）。EVIDENCE 不是产品 never-green，
  是内部证据工具被 native gate 取代。
- **accepted reviewed 的 130 个**：96 weaker + 3 near_exact + 7 structural +
  24 blocked（17 matrix checker 缺口 + 7 非 matrix checker 缺口）。
- **waived 的 29 个**：14 checker-unavailable + 5 contract/API +
  2 conditional-exact 未执行 + 1 native perf 缺测试 + 7 known defects。
- **P7-1 的 `planned=0` 只是账本状态，不是删除授权**。23 个旧 gate 仍是
  `gate_green`，`mandatory_green=false`、`replacement_reviewed=false`、
  `old_test_removed=false`；`p7-freeze.sh` 的 `deletion_allowed` 仍是
  `false`。Frozen oracle、12 个 delete gate、11 个 retarget gate、
  no-Node、G6/G7 都还是独立前置。
- 本轮只新增本文件；未改代码、测试、ledger、contract、README、脚本或
  fixture；未 commit；未启动 review；**未运行任何测试**。

---

## 1. 快照、边界与硬约束

### 1.1 工作区快照

| 项 | 值 |
| --- | ---: |
| 工作区 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `befba03`（初次读取）；写入过程中并行提交 `b9291a0`、`b0f6cc1` |
| `green` | 109 |
| `planned` | 192 |
| `retired-with-decision` | 38 |
| `exact` | 109 |
| `near_exact` | 3 |
| `weaker` | 96 |
| `structural` | 7 |
| `blocked` | 33 |
| 无证据 planned | 53 |
| 23 个旧 gate | 23 `gate_green` / 0 `mandatory_green` / 0 `replacement_reviewed` |

**当前冻结证据已过期**：` docs/evidence/data/legacy-p7-freeze.json` 仍记录
`83 green / 218 planned / 38 retired`，`frozen_paths_clean=false`；而
ledger 已是 `109/192/38`。因此 ` scripts/p7-freeze.sh --check` 现在不可能
通过。任何 ledger 收口都必须先按 §5.4 重新冻结，不能把旧 freeze JSON
当作 P7-0 的有效证据。

计数以工作树实际 JSON 为准。写入过程中并行 session 已把
`TS-MATRIX-0092` 从 planned/no-evidence 提升为 green，台账从
`108/193/38` 变为 `109/192/38`；本文件使用后者。应用本策略前必须重新
读取一次，不要复用文档里的绝对数：

```bash
jq '.counts.statuses, .counts.migration_evidence' \
  docs/plans/tsgo-test-ledger.json
```

### 1.2 现有硬约束（决定了哪些“最小改法”不可行）

1. `ALLOWED_STATUSES` 只有 `planned`、`green`、`retired-with-decision`
   （ ` crates/wxml-lsp/tests/native_parity/ledger.rs:105`）。`reviewed`
   只存在于契约词表（ ` docs/reference/tsgo-test-contract.md:112`），
   而且当前定义是 **green 之后**的评审态，不是 waiver 态。
2. `retirement_decision.scope` 只允许 `candidate_b_layout` 和
   `superseded_contract`（ ` crates/wxml-lsp/tests/native_parity/ledger.rs:119`）。
   新增 scope 必须同时改 validator、契约和 case。
3. `retired-with-decision` 不允许携带 `migration_evidence`
   （ ` crates/wxml-lsp/tests/native_parity/ledger.rs:1238` 附近的
   status 分支）。因此不能简单把 140 个有证据的 planned 改成 retired；
   要么删掉证据（审计损失），要么放宽 validator（破坏“retired 不是
   迁移证据”的不变量）。
4. `dirty_increments[].status` 目前必须等于 `planned`
   （ ` crates/wxml-lsp/tests/native_parity/ledger.rs:1290`）。如果
   `planned=0` 而 D1–D7 仍写 `planned`，测试会失败；这是一个容易漏掉的
   联动点。
5. `p7-freeze.sh` 把 ledger 计数写进冻结证据，并硬编码
   `deletion_allowed:false` 和 “not 0 planned” 的 blocked 文案
   （ ` scripts/p7-freeze.sh:649`、`:767`、`:769`）。任何 status 变化都必须
   重新生成 ` docs/evidence/data/legacy-p7-freeze.json`，否则 `--check`
   会失败。
6. 旧 TS oracle manifest 会逐 case 交叉核对 ledger 的
   `case_id` / `file` / `line` / `body_sha256`
   （ ` crates/wxml-lsp/tests/native_parity/oracle_manifest.rs:636`）。
   **只改 status 不会影响 projection**；但不要顺手改 case 锚点，也不要
   把 ledger schema 从 `/1` 升到 `/2`，否则要重算 manifest projection 和
   `FROZEN_MANIFEST_SHA256`（ ` crates/wxml-lsp/tests/native_parity/oracle_manifest.rs:45`、`:83`）。
7. 23 个旧 gate 是独立状态轴：`gate_green` 是 opt-in required-native
   job 的结果，不是 `mandatory_green`，也不是默认 CI 通过
   （ ` crates/wxml-lsp/tests/native_parity/native_gates.rs:26`、`:89`）。
   台账 waiver 不会自动完成 gate review/retarget。

---

## 2. 方案对比

### 2.1 方案 A：剩余 planned 全部 `retired-with-decision`

**做法**：新增 scope `owner-scope-waiver`，把 192 个 planned 全部写成
`retired-with-decision`，每个 case 带 `retirement_decision`。

**结果**：109 green / 0 planned / 230 retired；`exact` 仍 109。

**改动量**：中。192 个 `retirement_decision`（每个要有 `id`、`date`、
`owner`、`required_by`、`scope`、`reason`、`product_impact`、`evidence`、
`replacement[]`）；140 个 `migration_evidence` 要么删除，要么让 retired
允许证据（后者要改 validator 的核心不变量）；`RETIREMENT_DECISIONS`
sentinel 从 2 条变成 194 条；`counts.integrator_decisions`、contract、
freeze JSON 都要改。

**审计性**：低到中。`retired` 会占 230/339（68%），其中 96 个 weaker、
24 个 blocked 覆盖缺口、53 个无证据 case 都叫“retired”。如果 scope 全是
`owner-scope-waiver`，scope 本身不携带任何信息；审阅者必须逐条读
`reason` 才能知道是机制替换还是没测过。这不是伪造 green，但把
“accepted weaker”误标成“retired”，审计语义不诚实。

**P7 validator/CI**：中风险。`ledger_migration_evidence_is_complete_and_consistent`
当前对 retired 直接 `continue`；如果删除 140 份证据，`assessed_cases`
会从 248 掉到 109，历史证据消失；如果保留证据，就必须改掉
“retired evidence must stay outside the migration ledger”的断言，
这会让 retired 和 reviewed 的边界失效。`p7-freeze.sh` 的 blocked 文案
也必须改，否则它会说“not 0 planned”，与实际不符。

**是否违反“功能合理即可”**：形式上不违反（不要求 parity），但把
“有证据的 weaker”和“无证据”混为一谈，不符合“不伪造完成”的审计要求。

### 2.2 方案 B（原始表述）：新增 `reviewed`，weaker/blocked 标 reviewed

**做法**：`ALLOWED_STATUSES` 增加 `reviewed`；96 weaker + 3 near_exact +
7 structural + 31 blocked = 137 个 planned 改为 `reviewed`；只退休
C1/Node/dual-layout/superseded 约 30 个；剩余 25 个无证据 case 保持
planned（若再把 3 个 EVIDENCE 也退休，则剩 22 个）。

**结果**：109 green / **25 planned** / 137 reviewed / 68 retired；
若退休 EVIDENCE 3 个，则为 109 / **22 planned** / 137 / 71。
P7-1 的 `planned=0` 不成立。

**改动量**：中。`reviewed` 状态、target status、counts、契约状态机、
G5、freeze JSON。

**审计性**：中。`reviewed` 保留 `parity=weaker/blocked`，比 A 好；但
它有两个硬伤：

1. 22–25 个无证据 case 不能标 `reviewed`。`reviewed` 应当意味着“有证据且
   被审阅”；没有 Rust 证据的 case 只能补测试或 waiver。若强行标
   `reviewed`，validator 就不能要求证据，`reviewed` 会退化成
   “waiver 的别名”。
2. 契约里的 `reviewed` 是 `green` 之后的状态
   （ ` docs/reference/tsgo-test-contract.md:114`）。把它改造成非 green
   终态，必须同时重写状态机，否则审阅者会以为 `reviewed` 已经 green。
   这不是不能做，但必须显式改名/改语义，不能悄悄复用。

**P7 validator/CI**：高风险。若把 `PLANNED_CASES` 改成 0 但仍有 22–25 个
planned，测试直接失败；若保留 22–25，P7-1 不成立。若把这些也标
`reviewed` 并放宽 validator，CI 就无法区分“审阅过的 weaker”和
“没测过的 waiver”，这是最容易被误判为“伪造完成”的方案。

**是否违反“功能合理即可”**：部分符合；但它把“没测过”包装成
“reviewed”，不符合“不伪造 green/reviewed”的底线。

### 2.3 方案 C：旧 339-case ledger 归档为 historical，新建 native acceptance ledger

**做法**：旧 `tsgo-test-ledger.json` 保持 109/192/38，加
`role=historical-reference` 和 `successor=tsgo-native-acceptance.json`；
新建 ` docs/plans/tsgo-native-acceptance.json`，按 native 能力/F3/23 gate/
矩阵重新定义验收 case；P7-1 的 `planned=0` 针对新账本。

**结果**：旧账本仍 192 planned；新账本可以做到 `planned=0`。P7 的删除
权威从旧账本切到新账本。

**改动量**：高。新 JSON + 新 validator（约 500–800 行）+ 新 module 接线 +
contract 新章节 + `p7-freeze.sh` 切换权威源 + CI。旧账本还要加
`closeout_register` 把 192 个 planned 分类（否则旧账本仍然是一份
“192 planned”的红账）。如果新账本只收录当前 green 的 case，它就是一个
更小的、自选的测试集；如果收录 17 个 blocked matrix 行，则新账本也有
planned，P7-1 仍不归零。

**审计性**：高，但前提很硬：旧账本必须不可变、保留全部证据，新账本
必须有 `legacy_case_ids` 映射和 `coverage_matrix`，并且每个 native
capability 都能追到真实测试。否则审阅者会认为“把门禁换成了自己
能过的测试”。C 的最大价值是 P7 之后停止维护 339-case 账本，而不是
在删除前用它替代 339-case 的残余风险登记。

**P7 validator/CI**：高风险双源。旧 `ledger.rs` 仍断言 109/192/38；
新 validator 断言新账本 `planned=0`。`p7-freeze.sh` 当前读旧账本
（ ` scripts/p7-freeze.sh:649`），如果不切换，它永远说“not 0 planned”；
如果切换，旧账本 drift 就不再被默认 CI 检查，除非保留旧 validator。
两个 validator 同时跑会更安全，但 CI 成本和维护面都上升。

**是否违反“功能合理即可”**：方向上最符合（不再要求 339 对齐），但
“换账本”本身不是 waiver 记录；如果没有完整的旧 case 映射，它会把
“未验证”从账本里移走，而不是记录为残余风险。

### 2.4 方案对照

| 维度 | A 全量 retire | B 原始 reviewed | **B+ 推荐** | C 双账本 |
| --- | --- | --- | --- | --- |
| 新增状态 | 0（只加 scope） | `reviewed` | `reviewed` + `waived` | 旧账本 0，新账本自定义 |
| `planned=0` | 是 | 否（22–25 无证据） | **是** | 仅新账本 |
| `green` 语义 | 不变（exact） | 不变 | **不变（exact）** | 旧账本不变 |
| 审计性 | 低–中 | 中 | **高** | 中–高（依赖映射） |
| P7/CI 误判 | 中 | 高 | **低–中** | 高（双源） |
| 改动量 | 中（JSON 大） | 中 | **中** | 高 |
| 功能合理即可 | 形式符合 | 部分符合 | **符合** | 最符合，但易走样 |
| 删除授权 | 仍需 gate/oracle | 不成立 | **仍需 gate/oracle** | 需切换 freeze 权威 |

---

## 3. 推荐方案 B+：单账本、四终态

### 3.1 状态语义

| 状态 | 允许的 `parity` | 证据 | 是否计入 `exact/green` | 必需记录 |
| --- | --- | --- | --- | --- |
| `green` | 仅 `exact` | 至少一个真实 Rust 测试 | 是 | `migration_evidence`，`green_eligible=true` |
| `reviewed` | `near_exact` / `weaker` / `structural` / `blocked` | 必须有 Rust 证据 | **否** | `migration_evidence` + `review`（group_id + note） |
| `waived` | 无 evidence，或 `blocked` + known defect | 无证据或已有 blocked 证据 | **否** | `waiver`（group_id + gap_kind + note + follow_up） |
| `retired-with-decision` | 无 | 无 `migration_evidence` | **否** | `retirement_decision` |
| `planned` | 任意（仅迁移中） | 任意 | 否 | P7-1 前必须为 0 |

**关键不变量**：

1. `green` 必须 `parity=exact`，`parity=exact` 必须 `green`。
2. `reviewed` 不得 `parity=exact`，不得 `green_eligible=true`。
3. `waived` 不得 `parity=exact`，不得 `green_eligible=true`。
4. `retired-with-decision` 不得携带 `migration_evidence`。
5. `reviewed` / `waived` / `retired` 都不计入 `exact`、`green` 或
   `promoted_to_green`。
6. `planned=0`；`green + reviewed + waived + retired = 339`。

### 3.2 契约状态机改写

当前契约是：

```text
planned → red → green → reviewed → closed
planned → retired-with-decision
```

B+ 改为：

```text
planned → green                    (仅 exact；green 之后可 closed)
planned → reviewed                 (有证据、owner 接受残余风险；非 green)
planned → waived                   (无证据或 known defect；owner 签字；非 green)
planned → retired-with-decision    (机制/契约/内部证据被替换)
green   → closed                   (post-green 评审收口)
```

`reviewed` 不再表示“green 之后的评审”，而是“非 green 的、有证据的
残余风险终态”；`green` 之后的评审收口用 `closed`。这是一次显式的
语义澄清，必须在契约 §3 和 validator 同时落地，不能只改 JSON。

### 3.3 `closeout` 块（新顶层键）

建议在 ledger 顶层新增 `closeout`，而不是改写 2026-11-17 的 `contract`
历史块：

```json
"closeout": {
  "policy": "c-lite-owner-scope-waiver",
  "date": "2026-11-20",
  "owner": "<owner>",
  "scope": "owner-scope-waiver",
  "planned_target": 0,
  "green_definition": "parity=exact + at least one real Rust test",
  "reviewed_definition": "non-exact Rust evidence reviewed; accepted residual risk; not green",
  "waived_definition": "owner-signed gap; no evidence or known defect; not green",
  "retired_definition": "mechanism or contract replaced; no migration evidence",
  "review_groups": [
    {
      "id": "rg-weaker-subset",
      "count": 96,
      "accepted_risk": "...",
      "product_impact": "...",
      "follow_up": "..."
    }
  ],
  "waiver_groups": [
    {
      "id": "wg-checker-unavailable",
      "gap_kind": "no-rust-evidence",
      "count": 14,
      "reason": "...",
      "product_impact": "...",
      "follow_up": "..."
    }
  ],
  "non_claims": [
    "reviewed is not green",
    "waived is not reviewed",
    "retired is not migrated",
    "planned=0 is not deletion authorization"
  ]
}
```

`review_groups` / `waiver_groups` 的作用是：130 个 review 和 29 个 waiver
不需要复制 130 份长篇理由；每个 case 只写 `group_id` + 个案 note，
组级记录统一的 accepted risk / product impact / follow-up。validator 校验
组存在、组 `count` 与 case 数一致、组的 `gap_kind` 与 case 一致。
上面的 JSON 只列了一个组作示例；实际是 9 个 review group
（96 weaker / 3 near / 7 structural / 4 DOCS / 8 STATE / 2 PROBE /
2 ROOT / 1 CHECKER / 7 non-matrix）和 5 个 waiver group
（14 checker / 5 contract / 2 conditional / 1 perf / 7 known）。

### 3.4 逐 case 记录

`reviewed` case：

```json
"status": "reviewed",
"target_rust_test_status": "reviewed_residual_risk",
"migration_evidence": {
  "parity": "weaker",
  "green_eligible": false,
  "status_recommendation": "planned",
  "status_applied": "reviewed",
  "rust_tests": ["..."]
},
"review": {
  "group_id": "rg-weaker-subset",
  "note": "case-specific residual risk; >= 20 chars"
}
```

`status_recommendation` 是证据层建议，不是最终状态：3 个 near_exact 可以保留
`status_recommendation=green`（证据支持改名后 exact），但 `status` 仍是
`reviewed`，`green_eligible=false`；validator 不应把 near_exact 强制回
`planned`。

`waived` case（无证据）：

```json
"status": "waived",
"target_rust_test_status": "waived_owner_accepted",
"waiver": {
  "group_id": "wg-checker-unavailable",
  "gap_kind": "no-rust-evidence",
  "note": "...",
  "follow_up": "required-native checker job / issue id"
}
```

`waived` case（known defect）：保留原 `migration_evidence`（`parity=blocked`），
`status_applied` 改为 `waived`，并增加：

```json
"waiver": {
  "group_id": "wg-known-defect",
  "gap_kind": "known-defect",
  "note": "...",
  "product_impact": "...",
  "follow_up": "bug/issue id; must be non-empty"
}
```

`retired` case：只带 `retirement_decision`，不带 `migration_evidence`。
新 scope 建议：

- `mechanism_replaced`：D Node pool/C1 6 个、dual-layout 2 个；
- `superseded_contract`：C1 19 个、superseded 3 个；
- `internal_evidence`：EVIDENCE 3 个。

`retirement_decision.replacement[]` 允许 `kind=none` + `strength=blocked`
表示“没有等价替代”；不允许为了凑数写 `exact`。

---

## 4. 计数目标与分类

### 4.1 快照计数

以 `109 green / 192 planned / 38 retired` 为基线：

| 项 | 值 | 说明 |
| --- | ---: | --- |
| `green` | 109 | 仅 exact |
| `reviewed` | 130 | 96 weaker + 3 near_exact + 7 structural + 24 blocked |
| `waived` | 29 | 22 no-evidence + 7 known-defect |
| `retired-with-decision` | 71 | 37 obsolete + 34 non-obsolete |
| `planned` | 0 | P7-1 |
| 合计 | 339 | |
| `assessed_cases` | 246 | 109 + 96 + 3 + 7 + 31 |
| `exact` | 109 | |
| `near_exact` / `weaker` / `structural` / `blocked` | 3 / 96 / 7 / 31 | 2 个 dual-layout blocked 已转 retired，故 blocked 从 33 降到 31 |
| `waived_without_evidence` | 22 | 无证据 waiver |
| `known_defects` | 7 | waiver 子集 |
| `never_green` | 38 | 30 机制退休 + 1 engine-id waiver + 7 known defects |

`counts.integrator_decisions` 建议（8 个键，含 `remaining_blocked`）：

| 键 | 值 |
| --- | ---: |
| `accepted_obsolete` | 37 |
| `accepted_rewrite` | 0 |
| `accepted_superseded_contract` | 23（1 历史 + 19 C1 + 3 superseded） |
| `accepted_mechanism_replaced` | 8（6 Node pool + 2 dual-layout） |
| `accepted_internal_evidence` | 3 |
| `accepted_reviewed` | 130 |
| `accepted_waived` | 29 |
| `remaining_blocked` | 0 |

37 + 23 + 8 + 3 = 71 retired；+ 130 + 29 = 230 非 green；+ 109 = 339。

### 4.2 可 retire 的 33 个

| 组 | 数量 | case |
| --- | ---: | --- |
| C1 pure transport/envelope | 16 | `TS-ANALYSIS-0226/0272/0371`、`TS-EVENTS-0375/0387/0399`、`TS-PROTOCOL-0178/0250/0329/0348/0368/0421/0450/0561/0606/0644` |
| C1 input policy | 3 | `TS-PROTOCOL-0141/0154/0653` |
| D Node pool/C1 | 6 | `TS-COMPILER-0050/0086`、`TS-PROTOCOL-0021/0395`、`TS-SERVER-0094/0109` |
| dual-layout/old placement | 2 | `TS-COMPONENT-0279`、`TS-LAYOUTS-0062` |
| superseded contract | 3 | `TS-LAYOUTS-0084/0101`、`TS-SERVER-0203` |
| internal evidence tooling | 3 | `TS-EVIDENCE-0011/0025/0050` |

现有 38 个 retired（37 个 obsolete，含 `TS-COMPONENT-0774`；1 个
non-obsolete `TS-META-0027`）不变；新增 33 个后为 71。

注意：`TS-COMPONENT-0279` 和 `TS-LAYOUTS-0062` 当前有 blocked 证据；
转 retired 时必须删除 `migration_evidence`，把证据摘要移入
`retirement_decision.evidence[]`/`replacement[]`，否则 validator 会拒绝
“retired + evidence”。

`TS-SERVER-0109` 的 title hover 替代证据不完全对称（旧 C1 analyze 的
title hover 与 native 的 title hover 不是逐断言等价）。如果 owner 不接受
这条退休，把它移到 `waived` 的 `contract/API gap` 组：结果为
**130 reviewed / 30 waived / 70 retired**，`waived_without_evidence` 23，
`never_green` 仍 38。不要把 `TS-SERVER-0109` 写成 reviewed 或 green。

### 4.3 可 waiver 的 29 个

| 组 | 数量 | case |
| --- | ---: | --- |
| checker-unavailable | 14 | `TS-GUARDS-0128/0152/0174/0321/0329`、`TS-LOCALS-0261/0288/0312`、`TS-MAPPING-0306`、`TS-COMPONENT-0550/0628/0697`、`TS-COMPILER-0132`、`TS-LAYOUTS-0147` |
| contract/API gap | 5 | `TS-ANALYSIS-0287`、`TS-COMPILER-0095`、`TS-PROTOCOL-0234/0479/0500` |
| conditional exact not executed | 2 | `TS-ANALYSIS-0243`、`TS-COMPILER-0063` |
| native perf test missing | 1 | `TS-REWRITE-0378` |
| known defect | 7 | `TS-COMPONENT-0229/0570/0676`、`TS-DEFINITIONS-0036`、`TS-SYMBOLS-0116/0299/0425` |

`TS-COMPILER-0095` 是 engine-id 语义差异，默认放在 waiver（never-green），
不放在 retired；如果 owner 接受“engine identity 替代 path-only project key”，
可改为 `mechanism_replaced`，则计数变为 130 reviewed / 28 waived / 72 retired，
`waived_without_evidence` 21，`never_green` 仍 38。

### 4.4 可 reviewed 的 130 个

| 组 | 数量 | case |
| --- | ---: | --- |
| weaker | 96 | 由 ledger `parity=weaker` 查询；不逐条列在本文 |
| near_exact | 3 | `TS-COMPONENT-0521/0763`、`TS-EVENTS-0188` |
| structural | 7 | `TS-ANALYSIS-0380`、`TS-COMPONENT-0357/0476/0514/0539`、`TS-PERF-0012`、`TS-REWRITE-0413` |
| blocked matrix（17） | 17 | DOCS 4（`PAGE_BASIC`、`COMPONENT_PROPERTIES`、`COMPONENT_JS`、`0271`）、STATE 8（`JS_JSDOC_OFF`、`TS_NOCHECK`、`JS_NOCHECK`、`JS_SOURCE_CHECK_OFF`、`JS_BOM_CHECK_OFF`、`0239/0252/0283`）、PROBE 2（`NO_IMPLICIT_THIS_OFF`、`0335`）、ROOT 2（`INTERFACE_ROOT_IMPORT`、`0215`）、CHECKER 1（`PAGE_CLOSURE`） |
| blocked 非 matrix（7） | 7 | `TS-GUARDS-0097/0218/0261/0312`、`TS-LAYOUTS-0049/0072/0117` |

这 24 个 blocked 都是“覆盖缺口/能力未接线”，不是已知反向行为；因此
可以 reviewed，但 `review.note` 必须写清缺的是 checker、STATE、
PROBE、ROOT 还是 DOCS，并给 follow-up。

**狭义的 accepted weaker 只有 96 个**；3 个 near_exact、7 个 structural、
24 个 blocked 是不同层的残余风险，报告里不要统称为 “weaker”。

### 4.5 绝不能 green 的 38 个

| 组 | 数量 | 原因 |
| --- | ---: | --- |
| C1 transport/envelope/input policy | 19 | native 不实现 C1；没有 native 消费者 |
| D Node pool/C1 | 6 | `MAX_LIVE_SNAPSHOTS=8` ≠ `MAX_POOLED_PROJECTS=2`；C1 无 native 消费者 |
| dual-layout/old placement | 2 | symbol-bridge 已 retired；probe 永远放 `methods` |
| superseded contract | 3 | QuickInfo 不存在；union overlay 是 native 有意修复；bad-frame 继续被契约禁止 |
| engine-id | 1 | `TS-COMPILER-0095` 的 path-only project 语义没有 native 等价 |
| known defects | 7 | 见 §4.6；修复前是 false positive / false negative / unsupported |

其中 30 个机制/契约 case 走 retired，`TS-COMPILER-0095` 走 waived，
7 个 known defect 走 waived。`TS-EVIDENCE-0011/0025/0050` 也 retired，
但它们是内部证据工具，不是“产品 never-green”。

### 4.6 7 个 known defect 的单独登记

| case | 当前风险 |
| --- | --- |
| `TS-COMPONENT-0570` | unresolved behavior 可能发布 2339（false positive） |
| `TS-COMPONENT-0676` | external behavior 未解析时整段跳过，不检查 own property（false negative） |
| `TS-SYMBOLS-0116` | `observerPath` 未实现；observer 半段是反面 |
| `TS-SYMBOLS-0299` | renamed destructuring 期望 complete=true，native 是 Partial |
| `TS-SYMBOLS-0425` | 另一个 script 的 `setData` 阻断；期望 per-page isolation/complete |
| `TS-COMPONENT-0229` | optional-object 期望 clean，native 是 Partial；是反例不是 weaker |
| `TS-DEFINITIONS-0036` | definition DTO 无 `kind`/generated relationship |

这 7 个不能写 `reviewed`（会暗示“已接受的 weaker”），必须
`waived` + `gap_kind=known-defect` + 非空 `follow_up`（bug/issue id）+
逐 case `product_impact`。如果 owner 不签这 7 个，P7-1 保持未完成；
这是本策略的硬边界，不能用“功能合理即可”掩盖已知反向行为。

---

## 5. 最小改动清单（B+）

### 5.1 ` docs/plans/tsgo-test-ledger.json`

- 顶层新增 `closeout`（19th top-level key；validator 的
  `REQUIRED_TOP_LEVEL_KEYS` 同步）。
- `contract.status_values` 增加 `waived`（`reviewed` 已在词表中）。
- 192 个 planned 的 `status` / `target_rust_test_status`：
  - 33 → `retired-with-decision` + `retirement_decision`；
  - 130 → `reviewed` + `review`；
  - 29 → `waived` + `waiver`。
- 新 retired/reviewed/waived 的 `requires_decision` 必须是 `false`；
  只有 37 个 obsolete case 保留 `requires_decision=true` 和 2026-11-17
  `accepted-obsolete` 决定。
- 为 130 个 reviewed 保留/补齐 `migration_evidence`，`status_applied`
  改为 `reviewed`，`green_eligible=false`。
- 为 7 个 known defect 保留原 blocked 证据，`status_applied=waived`，
  增加 `waiver`。
- 删除 2 个 dual-layout（`TS-COMPONENT-0279`、`TS-LAYOUTS-0062`）的
  `migration_evidence`，把摘要移入 `retirement_decision`。
- `counts.statuses`：`planned:0, green:109, reviewed:130, waived:29, retired:71`。
- `counts.migration_evidence`：
  - `assessed_cases:246`、`exact:109`、`near_exact:3`、`weaker:96`、
    `structural:7`、`blocked:31`；
  - `remaining_planned_without_evidence:0`，新增
    `waived_without_evidence:22`；
  - `statuses` 增加 `reviewed/waived`；
  - `by_partition` 按新状态重算（不要手改 16 个块，用脚本从 cases 生成）。
- `counts.integrator_decisions`：8 个键，见 §4.1。
- `dirty_increments[].status`：D1–D3 改为 `reviewed`/`waived` 对应终态；
  D4–D7 若没有 ledger case，改为 `not-applicable` 或 `closed`，并同步
  validator 的“不得声称完成”断言。
- **不要改**：`files`、`cases[].source`、`case_id`、`body_sha256`、
  `fixture_trees`、`ignored_rust_tests`、`silent_skip*`、`discrepancies`。
  这些是 oracle manifest / fixture hash / native gate 的交叉锚点。

### 5.2 ` crates/wxml-lsp/tests/native_parity/ledger.rs`

- 常量（ ` crates/wxml-lsp/tests/native_parity/ledger.rs:30` 附近）：
  `GREEN_CASES=109`、`PLANNED_CASES=0`、`REVIEWED_CASES=130`、
  `WAIVED_CASES=29`、`RETIRED_CASES=71`、`NON_OBSOLETE_RETIRED_CASES=34`、
  `OBSOLETE_CASES=37`；`NEAR_EXACT_CASES=3`、`WEAKER_CASES=96`、
  `STRUCTURAL_CASES=7`、`BLOCKED_CASES=31`。
- `ALLOWED_STATUSES`（`:105`）增加 `reviewed`、`waived`；
  `ALLOWED_RETIREMENT_SCOPES`（`:119`）增加 `mechanism_replaced`、
  `internal_evidence`。
- 新增 `REVIEWED_TARGET_STATUS="reviewed_residual_risk"`、
  `WAIVED_TARGET_STATUS="waived_owner_accepted"`。
- `RETIREMENT_DECISIONS` sentinel 从 2 条扩到 35 条（33 新 + 2 历史）；
  每条 `(case_id, decision_id, scope)` 唯一。
- 新增 `validate_review_decision`：`group_id` 存在、`parity` 非 exact、
  `note` > 20 字符、review 组 count 匹配。
- 新增 `validate_waiver`：`gap_kind` 只能是 `no-rust-evidence` /
  `known-defect`；no-evidence 不得有 `migration_evidence`；known-defect
  必须有 blocked 证据、`product_impact`、`follow_up`；owner 必须与
  `closeout.owner` 一致。
- 新增 `validate_closeout`：policy/owner/date/scope/planned_target=0；
  `review_groups`/`waiver_groups` 的 id 唯一、count 与 cases 匹配；
  `non_claims` 非空。
- `ledger_schema_is_frozen`（`:569` 附近）：top-level key 增加 `closeout`，
  `root.len()` 更新；`CONTRACT_STATUS_VALUES` 增加 `waived`。
- `ledger_counts_and_dispositions_are_frozen`（`:798`）：`statuses.len()==5`；
  `planned==0`；sum 339；`counts.statuses` 5 个键；integrator 8 个键。
- `ledger_cases_are_unique_and_routed_without_claims`（`:1058`）：
  `non_obsolete == green + reviewed + waived + non_obsolete_retired`
  （109 + 130 + 29 + 34 = 302）；`targets.len()` 同式；`decision_ids.len()
  == obsolete`（37）；新增 reviewed/waived 分支；禁止
  `retirement_decision` 与 `review`/`waiver` 混用。
- `ledger_migration_evidence_is_complete_and_consistent`（`:1326`）：
  retired 仍然 `continue`；reviewed/waived 按 §3.1 分支；exact 只允许
  green；`without_evidence` 只统计 waived no-evidence（22）；
  `parity_counts` 为 109/3/96/7/31；`by_partition` 的
  `planned_evidence_cases` 改为 `reviewed_cases` + `waived_cases`
  （或保留旧键但在契约中重新定义为 non-green evidence）。
- 新增 `ledger_closeout_policy_is_frozen`：断言 `planned=0`、
  `green==exact`、`reviewed/waived/retired` 不进入 exact、
  `never_green=38`、`known_defects=7`、`waived_without_evidence=22`。
- `dirty_increments` 断言（`:1290`）从“必须 planned”改为“必须是终态
  且不得声称 green”。

### 5.3 ` docs/reference/tsgo-test-contract.md`

- §1 计数表更新为 109/130/29/71/0，并注明 C-lite。
- §3 状态机改为 §3.2 的 DAG；明确 `reviewed`/`waived`/`retired` 不是
  迁移证据，不计入 `exact`。
- §3 `retired-with-decision` 来源增加 `mechanism_replaced`、
  `internal_evidence`；保留 `candidate_b_layout`、`superseded_contract`。
- §8 G5 改写：全部 `direct/rewrite/split` 为 `green` / `reviewed` /
  `waived` / `retired-with-decision` 且有记录；`planned=0`；`green==exact`；
  `known_defects` 必须逐条列出并有 follow-up。明确“retired/waived 不是
  green，不是 parity”。
  **G5 必须显式接受 `waived`**；否则 29 个 waiver 仍会阻塞 G5，
  P7-1 只是账本归零、G5 仍红。接受 `waived` 是本策略唯一的契约放宽点，
  必须由 owner 签字。
- 新增 §9 “C-lite owner-scope waiver”：owner/date/scope、四桶定义、
  never-green 清单、残余风险分组、不可用“旧测试已删除”代替 waiver。

### 5.4 ` scripts/p7-freeze.sh`

- `LEDGER_STATE_JSON`（`:649`）增加 `reviewed`、`waived`、
  `planned_zero`、`closeout_policy`、`planned_target`。
- `blocking_state.ledger`（`:746`）输出新计数。
- `blocked_by`（`:768`）把 “not 0 planned” 改为条件文案：
  `planned==0` 时写 “ledger closeout reached; reviewed/waived are not
  green”，否则写 “ledger has N planned, target 0”。
- **`deletion_allowed` 保持 `false`**（`:767`）。台账 waiver 不是删除
  授权；23 gate、Frozen oracle、旧 TS 树、v1 写路径仍是 blocker。
- 重新生成 ` docs/evidence/data/legacy-p7-freeze.json`。因为 ledger 和
  `ledger.rs` 在 `FROZEN_PATHS`（`:123`–`:141`），必须在同一提交里
  重新冻结；干净提交后生成，确保 `frozen_paths_clean=true`。当前
  committed freeze JSON 记录的是 `83/218/38` 且 `frozen_paths_clean=false`，
  不能作为 `109/192/38` 的冻结证据；重新冻结会同时修正这两个漂移。
- `DEFAULT_REQUESTED_HEAD` 更新到本次 re-freeze 的提交；否则
  “frozen paths changed between baseline and HEAD” 会失败。

### 5.5 ` docs/plans/tsgo-legacy-oracle-manifest.json`

**不需要改。** 只改 status 不改变 manifest 的 `case_id/file/line/
body_sha256` projection。实现时跑：

```bash
cargo test -p wxml-lsp --test native_parity oracle_manifest
```

不要因为“台账不再是 /1 语义”就升 schema；`LEDGER_SCHEMA` 在
` crates/wxml-lsp/tests/native_parity/oracle_manifest.rs:83` 固定为
`wxml-tsgo-test-ledger/1`，升 `/2` 会连锁要求重算 manifest
`projection_sha256` 和 `FROZEN_MANIFEST_SHA256`（`:45`），与本策略无关。

### 5.6 文档与冻结证据

- ` docs/plans/2026-11-17-tsgo-test-ledger.md`：在顶部状态行和末尾追加
  “2026-11-20 C-lite closeout” 段；不要重写历史 wave 记录。
- ` docs/evidence/data/legacy-p7-freeze.json`：按 §5.4 重新生成。
- 本文件：作为 owner waiver 的证据索引；实现时在 `closeout` 或
  contract §9 引用它。

### 5.7 可选（非最小）

- 新增 ` scripts/p7-ledger-closeout.sh`：用 jq 独立检查
  `planned=0`、`green==exact`、`review/waiver/retirement` 记录齐全。
  默认 CI 已运行 `cargo test --workspace`（含 ledger 测试），所以不是
  必需；它的价值是给非 Rust 审阅者一个只读命令。
- 给 ` docs/adr/0008-rust-tsgo-no-node-backend.md` 增加 C-lite waiver 的
  decision 记录；ADR 仍是 `proposed`，不影响本策略。

---

## 6. 对 P7 validator / CI 的影响

| 检查 | 当前 | B+ 之后 | 误判风险 |
| --- | --- | --- | --- |
| 默认 `cargo test --workspace` 的 ledger 测试 | 109/192/38 | 109/130/29/71/0 | 低；JSON 与常量任一漂移都会失败 |
| `p7-freeze.sh --check` | 109/192/38 | 109/130/29/71/0 | 中；必须同提交重新冻结，否则 drift 失败 |
| oracle manifest 交叉核对 | 339 | 339（不变） | 低；只改 status，不动 case 锚点 |
| `native_gates.rs` | 23 `gate_green` | 23（不变） | 低；waiver 不提升 gate |
| ` scripts/native-gates.sh` | 35 gate | 35（不变） | 无 |
| 默认 CI | 跑 ledger 测试 | 同左 | 无需新 job |

**会误判的写法（必须避免）**：

1. 把 `reviewed` 计入 `exact` 或 `green`。反例：`counts.migration_evidence.exact`
   写成 239。validator 的 `parity_counts["exact"] == GREEN_CASES` 会失败。
2. 把 `waived` 写成 `reviewed`。这会让 `known_defects=7` 消失，审阅者
   看不到已知反向行为。
3. 把 `retired` 写成 `green`，或删掉 `retirement_decision`。sentinel
   会失败。
4. 只改 ledger 不改 `ledger.rs` 常量：默认 CI 失败（这是好事）。
5. 只改 `ledger.rs` 不改 ledger：默认 CI 失败（这是好事）。
6. 忘记 `dirty_increments`：默认 CI 失败；如果只 grep `planned`，会看到
   一个假的 planned 字符串。
7. 忘记重新冻结 `legacy-p7-freeze.json`：`p7-freeze.sh --check` 失败；
   这不是 CI 默认 job，但 P7-0 审计会红。
8. 把 `deletion_allowed` 改成 `true`：这是最危险的误判。台账 waiver
   不满足 gate review / Frozen oracle / no-Node / G6/G7。

---

## 7. 命令

### 7.1 只读盘点（现在就可以跑，未在本文执行）

```bash
git status --short
jq '.counts.statuses, .counts.migration_evidence' \
  docs/plans/tsgo-test-ledger.json
jq -r '.cases | group_by(.status) | map("\(.[0].status)\t\(length)")[]' \
  docs/plans/tsgo-test-ledger.json
jq -r '[.cases[] | select(.status=="green" and .migration_evidence.parity!="exact")] | length' \
  docs/plans/tsgo-test-ledger.json
jq -r '[.cases[] | select(.status=="reviewed" and (.review == null))] | length' \
  docs/plans/tsgo-test-ledger.json
jq -r '[.cases[] | select(.status=="waived" and (.waiver == null))] | length' \
  docs/plans/tsgo-test-ledger.json
jq -r '[.cases[] | select(.status=="retired-with-decision" and (.retirement_decision == null))] | length' \
  docs/plans/tsgo-test-ledger.json
```

### 7.2 应用后的验证（实现时跑）

```bash
cargo test -p wxml-lsp --test native_parity ledger_
cargo test -p wxml-lsp --test native_parity oracle_manifest
# 只在 CI/nightly 跑全量：
cargo test -p wxml-lsp --test native_parity
cargo test --workspace --all-features
```

### 7.3 重新冻结（刻意执行，不在本轮）

```bash
# clean commit 之后
P7_REQUESTED_HEAD=<new-head> scripts/p7-freeze.sh --output /tmp/opencode/p7-freeze.json
diff -u <(jq -S . docs/evidence/data/legacy-p7-freeze.json) \
        <(jq -S . /tmp/opencode/p7-freeze.json)
# 确认 diff 只有 ledger counts / blocked_by / frozen_paths_clean 的变化
```

---

## 8. 风险与缓解

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| 审阅者把 `reviewed` 读成 green | 误以为 130 个已迁移 | 契约/计数/报告统一写 “reviewed ≠ green”；`green_eligible=false`；`green==exact` 断言 |
| known defects 被埋进 29 个 waiver | 已知 false positive/negative 上线 | `known_defects=7` 独立计数；逐 case `product_impact` + `follow_up`；owner 单独签字 |
| 22 个无证据 waiver 掩盖缺口 | 14 个 checker case 没有 native 证据 | `gap_kind=checker-unavailable` 分组；follow-up 指向 required-native checker job；优先补 2 个 conditional exact + 1 个 perf test |
| 计数手改漂移 | CI 红或审计不可信 | 所有 count 由 cases 计算；`closeout` + `counts` 双声明；sentinel；禁止手改 `by_partition` |
| freeze JSON 与 ledger 不同步 | `p7-freeze.sh --check` 失败 | 同一提交重新冻结；干净 worktree；更新 `DEFAULT_REQUESTED_HEAD` |
| oracle manifest 被误改 | 339 case 锚点失败 | 只改 status；跑 `oracle_manifest` 测试；不升 schema `/2` |
| 并行 session 改动台账 | 计数/分类错位 | 应用前 `git status` + `jq` 重读；不要在 dirty 状态批量改；必要时另开 worktree |
| P7 被误认为可删除 | 提前删 Node/旧 TS | `deletion_allowed=false`；gate review/Frozen/no-Node/G6/G7 独立清单；waiver 文档不写“删除完成” |
| waiver 范围扩大 | `waived` 超过 29 | 设预算：`waived<=29`、`known_defects==7`、`reviewed<=130`；超出必须新 owner 决策 |

---

## 9. 回滚

1. 本策略的实现应是一个原子提交：ledger JSON + `ledger.rs` + contract +
   ledger md + freeze JSON + 本文件。不要拆成“先改 JSON 再改 validator”。
2. 回滚命令：`git revert <commit>`。恢复后台账应回到
   `109 green / 192 planned / 38 retired`；没有生产代码、测试、oracle
   manifest 或 fixture 被改，回滚是安全的。
3. 如果只回滚 ledger JSON 而不回滚 `ledger.rs`，默认 CI 会因常量不匹配
   而失败；这是预期的保护，不要通过改常量“修绿”。
4. 如果 `p7-freeze.sh --check` 在回滚后失败，按回滚后的 HEAD 重新
   freeze；不要手改 hash 或 `frozen_paths_clean`。
5. 本策略不删除 Node、旧 TS、`backend_node`、v1 写路径或任何测试；
   回滚不涉及数据迁移或运行时状态。

---

## 10. 本文件不声称

1. 不声称 192 个 planned 已经迁移、等价或通过；`reviewed`/`waived` 都是
   非 green。
2. 不声称 `reviewed` 等于 `green` 或 `exact`；`exact` 仍只有 109。
3. 不声称 7 个 known defect 已修复；它们需要 owner waiver 或先修 bug。
4. 不声称 22 个无证据 case 已被测试覆盖；它们只是 owner 接受的 gap。
5. 不声称 P7-1 的 gate 轴完成；23 个 gate 仍是 `gate_green`，不是
   `mandatory_green`/`replacement_reviewed`。
6. 不声称 Node/旧 TS 可以现在删除；`deletion_allowed` 仍为 false。
7. 不声称本文件修改了 ledger、contract、测试、代码、README 或脚本；
   本文件只新增自己。
8. 不声称计数永久稳定；工作树有并行改动，应用前必须重读 JSON。

---

## 11. 只读复核记录

本文件写入前执行的只读检查（未运行测试、未改代码）：

```text
git status --short
git log --oneline -3
jq '.counts.statuses, .counts.migration_evidence' docs/plans/tsgo-test-ledger.json
jq '[.cases[] | .status] | group_by(.) | map({k:.[0],n:length})' docs/plans/tsgo-test-ledger.json
jq '[.cases[] | .migration_evidence.parity] | group_by(.) | map({k:.[0],n:length})' docs/plans/tsgo-test-ledger.json
grep -nE 'ALLOWED_STATUSES|ALLOWED_RETIREMENT_SCOPES|PLANNED_CASES|dirty increment' crates/wxml-lsp/tests/native_parity/ledger.rs
grep -nE 'status 只允许|G5 通过需要' docs/reference/tsgo-test-contract.md
grep -nE 'LEDGER_STATE_JSON|deletion_allowed|not 0 planned' scripts/p7-freeze.sh
grep -nE 'LEDGER_SCHEMA|verify_frozen_projection' crates/wxml-lsp/tests/native_parity/oracle_manifest.rs
jq '.old_ignored_gates.gates | map({status,mandatory_green,replacement_reviewed}) | group_by(.) | map({k:.[0],n:length})' docs/evidence/data/legacy-p7-freeze.json
```

复核结果（写入时快照）：

```text
statuses = { green: 109, planned: 192, retired-with-decision: 38 }
migration_evidence = { assessed_cases: 248, exact: 109, near_exact: 3,
  weaker: 96, structural: 7, blocked: 33, remaining_planned_without_evidence: 53 }
23 gates = 23 gate_green; mandatory_green/replacement_reviewed/old_test_removed = 0
ledger.rs = ALLOWED_STATUSES 3 values; scopes 2 values; PLANNED_CASES 192
freeze = deletion_allowed:false; blocked_by includes "not 0 planned"
oracle manifest = ledger_schema wxml-tsgo-test-ledger/1; 339 cases
```

写入过程中观察到并行 session 把 `TS-MATRIX-0092` 从 planned/no-evidence
提升为 green，故台账从 `108/193/38` 变为 `109/192/38`；本文件以
`109/192/38` 为准，但所有绝对数都应在实施时重新查询。
