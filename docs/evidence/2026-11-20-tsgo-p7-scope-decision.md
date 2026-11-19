# tsgo P7 收口范围裁决简报（2026-11-20）

## 0.1 Owner 决策（2026-11-20）

用户明确答复：**“不需要严格按照原来的功能写测试。功能合理即可”**。

据此采用 **C-lite（owner-scoped functional waiver）**：
- 不再要求旧 TypeScript/Node 断言逐条 parity；旧 C1 transport、Node pool、
  dual-layout、superseded/Node 专有 case 以 `retired-with-decision` 或
  `reviewed-as-weaker` 归档，保留证据与残余风险，**不伪造 green**。
- native 功能以现有真实引擎、native gate、F3 交付和合理行为测试为准；
  仍必须保持默认 native、F3 全绿、native-gates 全绿、无 Node 运行路径。
- P7 可在功能验证通过后删除 Node/旧 TS/旧 v1 写路径；不声称完整旧测试
  parity，ADR/README 必须写明 scope waiver。

## 0. 结论先行

- 工作区：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`，HEAD
  `befba03`。本文件**只新增本文件**；未改代码、测试、ledger、contract、README、
  脚本或 fixture；未 commit；未启动 review；**未运行任何测试**。
- 当前台账（JSON 实测）：**83 green / 218 planned / 38 retired**；
  `exact=83`、`near_exact=3`、`weaker=100`、`structural=7`、`blocked=16`、
  `assessed=209`、无证据 planned **92**。
- **严格 parity 的 green 上限是 128 / 339（37.8%）**。前提是矩阵阶段 1–5 全部
  落地、D 的 `TS-COMPILER-0063` 接受 digest/重开翻译、C 的
  `TS-ANALYSIS-0243` 接受 `jsCheck` 配置；若再把 D 的 6 个机制 case 退休，
  得到 **128 green / 167 planned / 44 retired**。P7-1 的 `planned=0`
  **不能由矩阵/C/D/blocked 四条线完成**。这里的“做完”指四份输入已文档化的
  exact/conditional exact/retire 路径，不是把 16 blocked/41 C 全部修复；
  后者是另一条工作流。
- 剩余 167 planned 的构成：**C 独有 41 + blocked 16 + 四条线之外 110**。
  110 中 **96 weaker + 3 near_exact + 7 structural + 4 无证据**；它们不在
  矩阵/C/D/blocked 的证据路径里，不能乐观地假设会转绿。
- 可 `retired-with-decision` 的窄口径约 **30 个**：C1 transport/pure envelope、
  Node pool/C1 server、dual-layout/old placement、以及明确 superseded 的
  QuickInfo/union/bad-frame。**13 个 C checker + 6 个 C pinned/contract +
  14 个 blocked + 4 个无证据不能靠 waiver 变成 green**；96 weaker/3 near/7
  structural 只能作为
  “reviewed-as-weaker/残余风险”，不是当前 ledger 的 green。
- **绝不能 green** 的至少 31 个：C1 transport 19 + D 机制 6 + dual-layout 2 +
  superseded 3 + engine-id 1；另有 7 个已知反向行为
  （`TS-COMPONENT-0570/0676`、`TS-SYMBOLS-0116/0299/0425`、
  `TS-COMPONENT-0229`、`TS-DEFINITIONS-0036`）在修复前绝不能 green。
- **推荐方案 B（窄口径 scoped waiver）**：只退休 30 个机制/契约 case，
  保留 110 四线外（含 96 weaker/3 near/7 structural/4 无证据）、C 19
  （13 checker + 6 pinned）、blocked 14 为 planned；得到
  **128 / 143 / 68**。B 不关闭 P7-1、不授权删除 Node/旧 TS；这是
  “不伪造完成”的默认选择。若必须关闭 P7-1，只能选 **C（coverage waiver）**：
  **128 / 0 / 211**，需要 owner 明确签字，不能包装成 parity。
- 最大阻塞不是矩阵测试本身，而是：**110 个四条线外 case + 41 个 C 语义缺口**
  没有 green 路径；23 个旧 gate 仍全部 `gate_green` 但
  `mandatory_green=false`/`replacement_reviewed=false`；旧 TS 树仍是 Live，
  `backend_node` 仍在 release 树中。

---

## 1. 快照、边界与已成立事实

### 1.1 台账快照

| 项 | 值 |
| --- | ---: |
| 总 case | 339 |
| `green` | 83 |
| `planned` | 218 |
| `retired-with-decision` | 38 |
| `exact` | 83 |
| `near_exact` | 3 |
| `weaker` | 100 |
| `structural` | 7 |
| `blocked` | 16 |
| `assessed_cases` | 209 |
| 无证据 planned | 92 |

与 ` crates/wxml-lsp/tests/native_parity/ledger.rs:28` 的
`GREEN_CASES=83`、`:30` 的 `PLANNED_CASES=218`、`:33` 的
`RETIRED_CASES=38`、`:44` 的 `NEAR_EXACT_CASES=3`、`:50` 的
`WEAKER_CASES=100`、`:56` 的 `BLOCKED_CASES=16` 一致。

ledger 的 `migration_evidence.implementation_head` 是 `1c89338`，不是
`befba03`；本轮没有回填 ledger，所以所有数字都是 `1c89338` 的已提交快照。
工作树另有并行未提交改动（`real_matrix.rs`、`scenario_matrix.rs`、
`stdio_type_loop.rs`、`native-gates.sh`、`p7-freeze.sh`、fixtures 等）；
本简报**不把这些未提交改动当作已完成证据**，也不把它们计入任何 green。

### 1.2 外部事实

| 事实 | 状态 | 依据 |
| --- | --- | --- |
| F3 交付合同 | **16 closed / 0 partial / 0 blocked** | ` docs/evidence/README.md`、` docs/plans/2026-11-17-rust-tsgo-migration.md` §12.13 |
| 默认 `--stdio` | **native/HostOnly**；只有 `NotInstalled` 降级 HostOnly，损坏安装 fail closed | ` crates/wxml-lsp/src/main.rs:200`、` docs/adr/0008-rust-tsgo-no-node-backend.md:106` |
| Node 路径 | `--backend node` 显式保留，deprecated；**未删除** | ` crates/wxml-lsp/src/main.rs:41`、` crates/wxml-lsp/src/lib.rs:44` |
| 旧 TS 测试树 | 仍 Live：22 个 test 文件、35 个 sample 文件 | `backends/typescript/test`、`backends/typescript/samples` |
| legacy oracle | 仍 `LegacySourceMode::Live`，不是 Frozen | ` crates/wxml-lsp/tests/native_parity/oracle_manifest.rs:172` |
| 23 个旧 gate | 23/23 `gate_green`，但 `mandatory_green=false`、`replacement_reviewed=false`、`old_test_removed=false` | ` docs/evidence/data/legacy-p7-freeze.json`、ledger `ignored_rust_tests[]` |
| ADR-0008 | `proposed`；默认 native 已实现，但 P7 删除/G5/G6/G7 未完成 | ` docs/adr/0008-rust-tsgo-no-node-backend.md:4` |
| C1 契约 | `accepted`，但明确 **native 路径不实现 C1** | ` docs/reference/type-bridge-c1.md:3` |
| 平台 | 仅 Linux x86_64 有证据 | F3/矩阵计划；其余平台未声明 |

**F3 16/0/0 不等于迁移完成。** 它只覆盖交付合同；不等价于 ledger 归零、
23 个 gate 的 `mandatory_green`/review、Node 删除或旧 TS 树删除。

### 1.3 四条输入线的集合与重叠

| 集合 | 数量 | 当前状态 | 备注 |
| --- | ---: | --- | --- |
| 矩阵（33 in-place + 10 顶层） | 43 | 43 planned（4 weaker） | `scenario_matrix` 分区 |
| C（checker/native 能力） | 59 | 12 green + 47 planned | 12 = messages 5 + probes 5 + ANALYSIS-0315/0332 |
| D（契约裁决） | 41 | 41 planned | 34 个是矩阵 case（33 in-place + 0092） |
| blocked（22 审计后的剩余） | 16 | 16 planned/blocked | 已应用 1 exact + 1 near + 4 weaker |
| **四线并集 planned** | **92** | | 43 + C-only 42 + D-only 7 |
| 四线之外 planned | 126 | | 218 − 92 |

重叠关系：

- 矩阵 43 = 33 个 `TS-MATRIX-0102-*-IN_PLACE_SHADOW` + 10 个顶层。
- D 41 包含全部 33 个 in-place + `TS-MATRIX-0092`；D 独有 7 个非矩阵 case。
- C 59 包含 5 个矩阵顶层（`0239/0252/0283/0319/0335`）；C 独有 42 个
  planned（47 − 5 矩阵重叠）。
- blocked 16 与 C/D/矩阵**无交集**。
- 因此不能把 43 + 59 + 41 + 16 相加；并集是 92。

矩阵计划是以 `70 green / 233 planned / 36 retired` 为基线写的，当前
`83/218/38` 已经包含 12 个 C green 和 1 个 blocked exact（`TS-COMPONENT-0292`）。
所以本简报用**当前台账 + 增量**计算，而不是复制矩阵计划的绝对数。

---

## 2. 严格 parity 的可达上限

### 2.1 分阶段投影

矩阵计划的阶段增量（见 ` docs/evidence/2026-11-20-tsgo-native-matrix-parity-plan.md`
§6.1）叠加到当前台账：

| 阶段 | 内容 | green | planned | retired | ledger blocked | matrix green/blocked |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 0 | 当前 | 83 | 218 | 38 | 16 | 0/0（43 planned） |
| 1 | 严格 real matrix（23 in-place + 4 顶层） | 110 | 191 | 38 | 32 | 27/16 |
| 2 | + STATE adapter（5+3） | 118 | 183 | 38 | 24 | 35/8 |
| 3 | + DOCS 语义分类（3+1） | 122 | 179 | 38 | 20 | 39/4 |
| 4 | + PROBE health（1+1） | 124 | 177 | 38 | 18 | 41/2 |
| 5 | + ROOT 分类（1+1） | 126 | 175 | 38 | 16 | 43/0 |
| 5a | + `TS-COMPILER-0063`（digest/重开） | 127 | 174 | 38 | 16 | 43/0 |
| 5b | + `TS-ANALYSIS-0243`（jsCheck off，条件） | 128 | 173 | 38 | 16 | 43/0 |
| 5c | + D 6 个机制 case 退休 | **128** | **167** | **44** | 16 | 43/0 |

说明：

- 阶段 1 的 27 个 exact 是 23 个 in-place + `0092/0228/0301/0319` 四个顶层；
  16 个 blocked 是 10 个 in-place gap + 6 个顶层 gap。阶段 1 会把 ledger
  `blocked` 从 16 临时抬到 32，这是**诚实登记**，不是退步。
- 阶段 2–5 的增量来自矩阵计划：STATE +8、DOCS +4、PROBE +2、ROOT +2；
  每个阶段都需要生产/适配器改动和独立测试，不能只改 ledger。
- `TS-COMPILER-0063` 和 `TS-ANALYSIS-0243` 都是条件 exact；D 的 6 个 retire
  是 `0050/0086/0021/0395/0094/0109`。
- 128 是 **green 上限**；167 是**执行 D 6 退休后的 planned 下限**。如果不退休
  D 6，则是 128 / 173 / 38。

### 2.2 128 的前提（缺一不可）

| # | 前提 | 若撤回 |
| ---: | --- | --- |
| A1 | 矩阵阶段 1–5 全部落地；43 个矩阵 case 都有 required-native 证据 | 按阶段回退；少一个阶段少对应数量 |
| A2 | `TS-MATRIX-0092` 接受 build-only re-scope（symbol-bridge 已 retired） | −1 green；`0092` 回 planned/blocked |
| A3 | `documentationSource` 由 test-side 语义分类（generated 区间 → copied，script 声明 → quickinfo），不要求 native 新 DTO | −4 green（3 in-place DOCS + 顶层 0271） |
| A4 | `definition.kind` 由 test-side 纯分类（target path + generated 区间），不要求 native 新 DTO | −3 green（`cross-file-overlay`、`alias-project`、`component-data-cross-file`；`page-basic`/`component-properties` 已在 DOCS） |
| A5 | STATE 5 个 case 由 adapter 真正发布 `Diagnostics=Disabled`，而不是测试推导 | −8 green（5 in-place + `0239/0252/0283`） |
| A6 | PROBE health 真正接线（root any → `Unavailable`） | −2 green（`no-implicit-this-off` + `0335`） |
| A7 | ROOT checker-based data-root 分类落地（import type → `Unsupported`） | −2 green（`interface-root-import` + `0215`） |
| A8 | `TS-COMPILER-0063` 接受“对象身份 → digest/重开身份”的翻译 | −1 green |
| A9 | `TS-ANALYSIS-0243` 接受新增 `jsCheck` 配置并发布 `Disabled` | −1 green |
| A10 | `TS-EVENTS-0188` 不升 exact（保持 near_exact；`fileId`→`path` 改名未接受） | 若接受，+1 green（见 §2.3） |

> **口径冲突（必须知道）**：C triage 把 `TS-MATRIX-0239/0252/0283/0319/0335`
> 归为 blocked，因为默认套件没有 checker；矩阵计划用 required-native matrix
> job 关闭它们。本简报按矩阵计划的 required-native 口径计入阶段 2/4/5。
> 如果 owner 不接受 required-native 证据作为 case 证据，这 5 个回 blocked，
> green 上限相应下降 5；这不是“严格 parity 上限”的乐观假设，而是两条输入
> 线的口径差异。

**禁止的伪 green**：

- optional skip、`#[ignore]` 未执行、fake engine、native 回放写期望，都不能算
  矩阵 exact。
- test-side 推导 `Disabled`/`Unavailable`/`Unsupported`，不能算 adapter 已发布
  该状态。
- `diagnosticsExact`、`perturbsUserTypes`、`userScriptDiagnosticsIntroduced`
  没有真实 checker 结果，不能算 exact。
- `documentationSource`/`definition.kind` 从 Markdown 或 fileId 猜，不能算
  exact；只有纯分类函数 + native 事实才可接受。

### 2.3 敏感性

| 变体 | green | planned | retired | 说明 |
| --- | ---: | ---: | ---: | --- |
| 严格上限（A1–A9 全接受，0188 保持 near） | 128 | 167 | 44 | 本简报基准 |
| 0188 接受改名 → exact | 129 | 166 | 44 | +1 green |
| 092 不接受 re-scope | 127 | 168 | 44 | −1 |
| DEFK 必须新 DTO | 125 | 170 | 44 | −3 |
| DOCS 必须 native 字段 | 124 | 171 | 44 | −4 |
| STATE 不做 adapter | 120 | 175 | 44 | −8 |
| PROBE 不做 | 126 | 169 | 44 | −2 |
| ROOT 不做 | 126 | 169 | 44 | −2 |
| 0063 不接受 | 127 | 168 | 44 | −1 |
| 0243 不做 | 127 | 168 | 44 | −1 |

敏感性是**单独撤回一项**的近似；多项同时撤回会叠加，但 DEFK/DOCS 有重叠，
不能简单相加。最保守的组合（092 + DEFK + DOCS + STATE + PROBE + ROOT 全拒绝）
会把 green 拉到 120 以下；这不是“严格路径”，而是“保留所有产品缺口”。

### 2.4 剩余不可达项（按 class）

128 green 之外还有 **211 个非 green**（167 planned + 44 retired）。四条线的
剩余 planned：**矩阵 0 / C 41 / D 0 / blocked 16 / 四线外 110**。下表只列
仍可处置的 167 planned，按 class 分解：

| class | 数量 | case / 说明 | 能否 exact |
| --- | ---: | --- | --- |
| C1 pure transport / C1 envelope | 16 | `ANALYSIS-0226/0272/0371`、`EVENTS-0375/0387/0399`、`PROTOCOL-0178/0250/0329/0348/0368/0421/0450/0561/0606/0644` | 否；native 无 C1 |
| C1 input policy | 3 | `PROTOCOL-0141/0154/0653` | 否；native 是去重/截断/跳过，不是拒绝 |
| C checker + C1 | 9 | `GUARDS-0152/0174/0321`、`LOCALS-0261/0288/0312`、`MAPPING-0306`、`COMPONENT-0628/0697` | 仅真实 checker + C1；默认套件不可 |
| C checker only | 4 | `GUARDS-0128/0329`、`COMPONENT-0550`、`COMPILER-0132` | 仅真实 checker；默认套件不可 |
| C pinned/contract/superseded | 9 | `LAYOUTS-0084/0101/0147`、`ANALYSIS-0287`、`COMPILER-0095`、`PROTOCOL-0234/0479/0500`、`SERVER-0203` | 3 可 retire；1 engine-id 不可 green；5 需契约/API 变更 |
| blocked API / DTO | 2 | `COMPONENT-0229`、`DEFINITIONS-0036` | 需新 native 类型关系/DTO |
| blocked checker | 7 | `GUARDS-0097/0218/0261/0312`、`LAYOUTS-0049/0072/0117` | 需真实 checker |
| blocked production | 5 | `COMPONENT-0570/0676`、`SYMBOLS-0116/0299/0425` | 需生产行为修复；部分当前相反 |
| blocked mechanism | 2 | `COMPONENT-0279`、`LAYOUTS-0062` | 旧布局/探针放置已替换；只能 retire/rewrite |
| 四条线之外 weaker | 96 | behaviors 7、completion 4、components 2、definitions 4、events 7、guards 2、hover 6、loops 3、mapping 12、overlay 9、protocol 13、shadow 14、symbols 13 | 需逐 case 补证据；不在四线范围 |
| 四条线之外 near | 3 | `COMPONENT-0521/0763`、`EVENTS-0188` | 改名/字段裁决；0188 可 +1 |
| 四条线之外 structural | 7 | `ANALYSIS-0380`、`COMPONENT-0357/0476/0514/0539`、`PERF-0012`、`REWRITE-0413` | 不同可观察层；需新测试 |
| 四条线之外无证据 | 4 | `EVIDENCE-0011/0025/0050`、`REWRITE-0378` | 3 个 evidence 可 retire；0378 应保留 planned |

合计：41（C）+ 16（blocked）+ 110（四线之外）= **167**。

补充：

- 上述 C1 16 是 G5 剩余 19 中的纯 envelope 部分；另外 3 个是 input policy。
- C 的 9 个 pinned/contract 中，`0084/0101/0203` 可 retire；
  `0095` 在当前合同下不可 green；`0147/0287/0234/0479/0500` 需要契约/API
  变更，不能靠 waiver 假装 exact。
- 96 weaker 不是“已评估所以快好了”；它们仍然是 planned，G5 要求
  green/reviewed，weaker 不够。

### 2.5 结论

**口径**：128 是“四份输入已文档化的 exact/conditional exact 路径”的上限，
不是理论上限。18 个 C/blocked checker case 理论上可以另建 per-case
required-native checker harness，但四份输入没有给出等价映射、fixture 和验收
标准；本简报不把这种未定义工作算作可达，否则就是乐观假设。要突破 128，
必须先新增一个独立工作流并重新评估，不能直接改 ledger。

- **严格 parity 上限是 128 green（37.8%）**；如果只做矩阵/C/D/blocked 的
  exact 路径，最多到这里。
- **planned 仍有 167（49.3%）**；即使把 D 6 退休也算上。
- **P7-1 的 planned=0 不可达**；这不是执行顺序问题，而是覆盖范围问题。
  任何声称“矩阵做完 P7-1 就归零”的说法都与台账不符。

---

## 3. scoped waiver 方案 A/B/C

### 3.1 先明确 ledger 的硬约束

1. `ALLOWED_STATUSES` 只有 `planned`、`green`、`retired-with-decision`；
   没有 `waived`/`reviewed` 桶（` crates/wxml-lsp/tests/native_parity/ledger.rs:99`）。
2. `reviewed` 是契约中的**green 之后**的评审态
   （` docs/reference/tsgo-test-contract.md:106`），不是 waiver 态；
   `weaker`/`blocked` 仍然是 `planned`。
3. 当前 `retirement_decision.scope` 只允许 `candidate_b_layout`、
   `superseded_contract`（` crates/wxml-lsp/tests/native_parity/ledger.rs:104`）。
   要退休 C1/Node pool/coverage，必须新增 scope 和 sentinel，不能只改 JSON。
4. G5 要求全部 `direct/rewrite/split` 为 `green/reviewed`，或
   `retired-with-decision` + `retirement_decision`
   （` docs/reference/tsgo-test-contract.md:174`）。**planned 会挡住 G5。**
5. 退休不是 green；`retired-with-decision` 不能计入 `exact`，也不能写成
   “已迁移/已等价”。

因此，“reviewed + 残余风险”在当前 ledger 里**不是可收口状态**。若 owner
选择它，必须二选一：

- 新增 `waived`/`accepted-risk` 状态，并改 `ALLOWED_STATUSES`、G5、counts；
- 新增 `coverage_waiver` retirement scope，把所有非 green 归入
  `retired-with-decision`，并明确它不是机制替换。

### 3.2 方案 A：严格 parity，只做已文档化的 D 机制退休

**内容**：矩阵阶段 1–5 + D `0063` + C `0243`（条件）+ D 6 个机制退休；
不做任何 coverage waiver。保留 C 41（19 C1 + 13 checker + 9 pinned）、
blocked 16、四线外 110（含 96 weaker）为 planned。

**结果**：128 green / 167 planned / 44 retired。

**影响**：

- P7-1 不关闭；G5 不通过；Node/旧 TS 不删除。
- 23 个旧 gate：12 个 delete 仍被 P7-1/Frozen/review 阻塞；11 个 retarget
  仍待做。
- 覆盖最完整；风险最低；但 P7 永远差 167 个 planned。

**风险/工作量**：矩阵 28–46 天 + D/C 3–5 天 + 退休治理 3–5 天；总计约
34–56 天。结果仍是“P7 未完成”。

### 3.3 方案 B：窄口径 scoped waiver（推荐）

**内容**：在 A 的基础上，只退休**机制/契约已被替换**的 30 个：

| 组 | 数量 | case | 建议 scope |
| --- | ---: | --- | --- |
| C1 pure transport/envelope | 16 | `ANALYSIS-0226/0272/0371`、`EVENTS-0375/0387/0399`、`PROTOCOL-0178/0250/0329/0348/0368/0421/0450/0561/0606/0644` | `superseded_contract` |
| C1 input policy | 3 | `PROTOCOL-0141/0154/0653` | `superseded_contract` + 明确“native 采用去重/截断/跳过”政策 |
| D C1/Node pool | 6 | `COMPILER-0050/0086`、`PROTOCOL-0021/0395`、`SERVER-0094/0109` | 新增 `mechanism_replaced`（或 `superseded_contract`） |
| dual-layout / old placement | 2 | `COMPONENT-0279`、`LAYOUTS-0062` | `candidate_b_layout` / `mechanism_replaced` |
| superseded contract | 3 | `LAYOUTS-0084/0101`、`SERVER-0203` | `superseded_contract` |

**结果**：128 green / **143 planned** / **68 retired**。

> **一个不对称点**：D 6 中 `TS-SERVER-0109` 的 title hover 替代证据不完全对称
> （D 文档 §7 第 9 点）。若 owner 要求逐断言替代，它应保持 blocked，
> B 的 retire 数变为 29、planned 144；其余结论不变。

**保留 planned 的 143**：

- C 19：9 checker+C1 + 4 checker + 6 pinned/contract
  （`0147/0287/0095/0234/0479/0500`）。
- blocked 14：API 2 + CHK 7 + PROD 5（MECH 2 已退休）。
- 四线之外 110：96 weaker + 3 near + 7 structural + 4 无证据。

**影响**：

- P7-1 **仍不关闭**（planned 143）；G5 仍红；Node/旧 TS 不删除。
- 23 个旧 gate 不变；退休 30 个 case 不会让 gate 变成
  `mandatory_green`/`replacement_reviewed`。
- 覆盖风险中等：放弃的是无 native 消费者的 C1/Node pool/旧布局，保留
  96 weaker 和所有 checker/blocked 语义。

**风险/工作量**：A 的基础上加 3–5 天退休治理 + 2–3 天 ledger/validator；
总计约 36–59 天。适合“先把无争议的机制债清掉，但不伪造 P7 完成”。

**B  core / B+ 说明**：

- 若 3 个 C1 input policy 不获 owner 批准，则只退休 27 个，结果
  **128 / 146 / 65**。
- 若再退休 6 个可选 pinned/contract（`ANALYSIS-0287`、
  `COMPILER-0095`、`LAYOUTS-0147`、`PROTOCOL-0234/0479/0500`），结果
  **128 / 137 / 74**；这需要额外契约/质量裁决，不建议默认。
- B 故意不把 6 个可选 pinned/contract 全部退休：其中 `0095` 虽在当前合同下
  不可 green，但先保留 planned，避免把契约变更面扩大到 engine identity。
- E 类的 `EVIDENCE-0011/0025/0050` 已有退休审计，若一并接受，可再 −3
  planned（不在四条输入线内，单列决策）。

### 3.4 方案 C：P7 coverage waiver（唯一能归零的方案）

**内容**：在 B 的基础上，把剩余 143 个非 green case 全部登记为
`coverage_waiver`（建议新增 scope）或 `waived` 终态；明确“不是已迁移、
不是 exact”。

**结果**：128 green / **0 planned** / **211 retired-or-waived**。
若 0188 升 exact，则 129 / 0 / 210。

**影响**：

- P7-1 可以在**书面 waiver** 下归零；G5 必须改写为接受
  `coverage_waiver`/`waived`，否则自相矛盾。
- Node/旧 TS 删除**才**有治理依据；但仍需 P7-2 Frozen、12 个 delete gate
  的 review/`old_test_removed`、11 个 retarget gate 的 retarget 工作。
- 覆盖风险高：四线外 110（96 weaker + 3 near + 7 structural + 4 无证据）、
  C 19、blocked 14 全部被放弃或降为风险登记；其中包含 protocol limits、
  shadow rewrite、mapping、overlay 等产品可见面。
- 23 个 gate 不会因为 case waiver 自动完成；`gate_green` ≠
  `mandatory_green`，`replacement_reviewed` 仍是 false。

**风险/工作量**：矩阵 28–46 天 + B 治理 5–8 天 + coverage waiver 治理
3–5 天 + P7-2/12 gate 3–5 天 + 11 retarget 6–9.5 天 + Node/TS 删除与
G6/G7 5–10 天；总计约 50–84 天。最大风险是产品回归和“用 waiver 换计数”
的评审反弹。

### 3.5 方案对照

| 项 | A 严格 | B 窄口径（推荐） | C coverage waiver |
| --- | ---: | ---: | ---: |
| green | 128 | 128 | 128 |
| planned | 167 | 143 | 0 |
| retired/waived | 44 | 68 | 211 |
| P7-1 `planned=0` | 否 | 否 | 是（waiver） |
| G5 | 红 | 红 | 需改写 |
| Node 删除 | 否 | 否 | 是（完成 P7-2/23 gate 后） |
| 旧 TS 树 | Live | Live | 归档/删除 |
| 12 delete gates | 阻塞 | 阻塞 | 可进入删除 |
| 11 retarget gates | 待做 | 待做 | 仍需 retarget（或改 p7_action） |
| 覆盖风险 | 最低 | 中 | 高 |
| 工作量（估） | 34–56 d | 36–59 d | 50–84 d |

> A/B 的 167/143 是“执行 D 6 退休后”的数字；若不做 D 6，A 是 128/173/38。
> B 的 30 含 D 6；B-core 27 是 128/146/65。

---

## 4. 逐类处置：retire / reviewed-risk / never green

### 4.1 可 `retired-with-decision`（B 的 30）

| 组 | 数量 | 依据 | 替代证据要求 |
| --- | ---: | --- | --- |
| C1 pure envelope | 16 | C1 只保留为历史 Node 桥；native 无 `analyze`/`states`/`insertions`/`mounts` 响应 | 指向 native input/identity 测试；`strength=weaker` 或 `opt_in`；不得写 exact |
| C1 input policy | 3 | native 对 malformed/重复输入是去重/截断/`FragmentLimit`，不是 C1 拒绝 | 指向 shadow layout/limits 测试；owner 明确接受政策差异 |
| D Node pool/C1 | 6 | `MAX_LIVE_SNAPSHOTS=8` ≠ `MAX_POOLED_PROJECTS=2`；C1 request/response 无 native 消费者 | 指向 session/snapshot 测试、0184/0197 语义测试；不得声称等价 |
| dual-layout/placement | 2 | symbol-bridge 已 retired；probe 永远放 `methods` | 指向 in-place path-free / placement 测试 |
| superseded contract | 3 | QuickInfo 不存在；union overlay 是 native 有意修复；bad-frame 继续被契约禁止 | 指向 native hover contract、`apply_declared_property_type`、§3.3 |

**前置条件**：

1. 新增/扩展 `retirement_decision.scope`（至少 `mechanism_replaced`；
   若做 C，再加 `coverage_waiver`）。
2. 更新 `ALLOWED_RETIREMENT_SCOPES`、`RETIREMENT_DECISIONS` sentinel、
   `RETIRED_CASES`、`NON_OBSOLETE_RETIRED_CASES`、`OBSOLETE_CASES`、
   `counts.statuses`、`counts.integrator_decisions` 和 G5 文档。
3. 每个 case 有稳定 `id`、`date=2026-11-20`、`owner=integrator`、
   `required_by`、非空 `evidence`、`product_impact`、`replacement[]`。
4. `migration_evidence` 必须为空；retired 不能带 exact/weaker 证据。

### 4.2 只能 `reviewed` + 残余风险，或补证据（110）

这里的“reviewed”指**owner 评审并接受 weaker/结构性残余**，不是 ledger 的
`reviewed` 状态；当前 ledger 仍会显示 planned。110 中前 106 个
（96 weaker + 3 near + 7 structural）可以走 reviewed-as-weaker；4 个无证据
不能 review，只能补测试或退休。

| 组 | 数量 | 为什么不能 exact | 需要的记录 |
| --- | ---: | --- | --- |
| 96 weaker | 96 | 有 Rust 证据，但缺 hover/诊断/reason/per-fragment 等断言 | 逐 case residual-risk note；可选择 `coverage_waiver` 或保持 planned |
| 3 near_exact | 3 | `fileId`→`path` 等改名；0188 可 exact | owner 接受改名则 +1 green；否则 near |
| 7 structural | 7 | 只断言 overlay/layout 结构或不同性能层，不是同一可观察结果 | 保持 planned；补新测试才可 green |
| 4 无证据 | 4 | 没有 Rust 测试/证据 | EVIDENCE 3 可 retire；REWRITE-0378 应补测试 |

**关键限制**：这 110 个中的 106 个 reviewed-as-weaker 仍不能计入 G5 的
`green/reviewed`；它们仍然是 planned。4 个无证据更不能 review。若 owner
要 P7-1 归零，必须为它们新增 waiver 状态/scope，或补足测试。

### 4.3 绝不能 green

**在当前 native 合同/产品语义下，至少 31 个不能记为 green**：

- C1 pure transport 16 + C1 input policy 3 = 19；
- D Node pool/C1 6；
- dual-layout/old placement 2（`0279/0062`）；
- superseded 3（`0084/0101/0203`）；
- engine-id 1（`0095`）。

这些只能 retire/waive，或修改产品契约；不能通过 test-side 推导、字段改名、
fake engine 或“weaker”包装成 green。

**另有 7 个已知反向行为，在修复前绝不能 green**：

| case | 当前风险 |
| --- | --- |
| `TS-COMPONENT-0570` | unresolved behavior 仍可能发布 2339（false positive） |
| `TS-COMPONENT-0676` | external behavior 未解析时整段跳过，不检查 own property（false negative） |
| `TS-SYMBOLS-0116` | `observerPath` 未实现；observer 半段是反面 |
| `TS-SYMBOLS-0299` | renamed destructuring 期望 complete=true，native 是 Partial |
| `TS-SYMBOLS-0425` | 另一个 script 的 `setData` 阻断；期望 per-page isolation/complete |
| `TS-COMPONENT-0229` | optional-object 期望 clean，native 是 Partial；是反例不是 weaker |
| `TS-DEFINITIONS-0036` | definition DTO 无 `kind`/generated relationship |

这些是 production bug/unsupported，不是“缺证据”；不能记 weaker，也不能
在没有修复的情况下 waiver 成 green。

**13 个 C checker + 7 个 blocked checker** 不是“绝不能”，但只能由真实
checker 证据转绿；默认套件、test-side 状态或 fake engine 都不行。

**4 个无证据 + 96 weaker + 3 near + 7 structural** 也不是“绝不能”，但需要
新测试/新证据/rename 裁决；不在当前四线范围。

### 4.4 对 Node 删除、旧 TS 树、23 个旧 gate 的影响

| 项 | A 严格 | B 窄口径 | C coverage waiver |
| --- | --- | --- | --- |
| Node `backend_node` | 保留 | 保留 | 可在 P7-5 删除 |
| `--backend node` | 保留 | 保留 | 删除后仅 native/HostOnly |
| 旧 TS test/samples | Live，保留 | Live，保留 | 需先 Frozen，再归档/删除 |
| 12 delete gates | 不可删 | 不可删 | 可删（需 Frozen + review） |
| 11 retarget gates | 待 retarget | 待 retarget | 仍需 retarget；`p7_action` 禁止直接删 |
| 2 silent return + tsc | 保留 | 保留 | 需随 Node 路径删除 |
| G5 | 红 | 红 | 需改写为接受 waiver |
| G6/G7 | 未关闭 | 未关闭 | 删除后重新验收 |

**23 个 gate 的独立事实**：

- 全部 23 个 `gate_green`，但 `mandatory_green=false`、
  `replacement_reviewed=false`、`old_test_removed=false`。
- 12 个 `p7_action=delete`：必须先让 legacy oracle Frozen；`old_test_removed`
  只接受 `delete`，且要求 replacement 仍链接。
- 11 个 `p7_action=retarget`：不能通过 `old_test_removed` 删除；必须把
  `stdio_type_loop` 9 个薄壳、`performance_bench`、`q2_stability` 原地
  retarget，约 6–9.5 人日。
- 因此 **C 的 case waiver 不会自动完成 gate review/retarget**；这是两条
  独立的工作流。

---

## 5. 需要 owner / integrator 决策的问题

| # | 问题 | 选项 | 建议 | 不接受的后果 |
| ---: | --- | --- | --- | --- |
| D1 | C1 兼容性仍是产品要求吗？ | 是 / 否（仅历史） | **否**；native 不实现 C1 | 若“是”，Node 不能删，C1 19+ 必须保持 planned；P7 无法收口 |
| D2 | real-checker matrix 是否必须？ | blocking required-native / opt-in / 不做 | **必须，至少 required-native blocking job** | 若 opt-in，43 个矩阵 case 不能声称 exact；只能 weaker/blocked |
| D3 | 接受 `documentationSource`/`definition.kind` 的 test-side 分类吗？ | 接受 / 新增 DTO | **接受**（纯分类，事实在 native） | 不接受则 −7 green（DOCS 4 + DEFK 3 额外） |
| D4 | 接受 `0092` build-only re-scope 吗？ | 接受 / 拒绝 | **接受**；symbol-bridge 已 retired | −1 green，`0092` 回 planned |
| D5 | 接受 `0063` digest/重开翻译吗？ | 接受 / 拒绝 | **接受**；对象身份不可发布 | −1 green |
| D6 | 是否为 `0243` 增加 `jsCheck` 配置？ | 做 / 不做 | **可延后**；不做则保持 planned | −1 green |
| D7 | 接受 `0188` 的 `fileId`→`path` 改名吗？ | 接受 / 保持 near | **接受**；同层语义；接受则 +1 green | 保持 near，不加 green |
| D8 | 批准窄口径退休清单/新 scope 吗？ | 批准 / 拒绝 | **批准**（B 的 30 个） | B 不成立；只能 A 或 C |
| D9 | 是否签署 coverage waiver（143 个）？ | 签 / 不签 | **默认不签**；只有排期必须收口时签 C | 不签则 P7-1 不归零，Node/TS 不删 |
| D10 | 现在删除 Node/旧 TS 吗？ | 现在 / P7-2 后 / 不删 | **不现在**；先 Frozen + 23 gate + waiver | 现在删会违反 G5/G6，且无法回滚证据 |
| D11 | 23 个 gate 是否全部 `mandatory_green`？ | 全部 / 仅 delete 组 / 保持 opt-in | **12 delete 组先 review；11 retarget 组完成 retarget 后再定** | 全部标 mandatory 会让默认 CI 长时间红；全部 opt-in 则 G5 不成立 |
| D12 | 平台证据是否只接受 Linux x86_64？ | 接受 / 扩平台 | **接受当前边界**，文档写清 | 若要求 macOS/Windows，需另开平台验证，不能复用 Linux 证据 |

> D9 是分水岭：不签，P7-1 不可能归零；签，等于书面承认 143 个 case 不再
> 是迁移门禁。两者都不能被描述成“parity 完成”。
>
> 本简报的 128/143/68 是**保守基准**：D7 接受则 0188 升 exact，
> 变为 129/142/68；D6 不做则 0243 回 planned，变为 127/144/68。
> 这些是单点变化，不改变“P7-1 不能由 parity 归零”的结论。

---

## 6. 推荐方案与 P7 执行顺序

### 6.1 推荐：B（窄口径 scoped waiver），C 作为显式升级选项

**推荐 B 的理由**：

1. 保留 110 四线外（含 96 weaker）+ C 19 + blocked 14 的真实覆盖，
   不把“没测过”包装成“已退休”。
2. 只清掉无 native 消费者的 C1/Node pool/旧布局/superseded 契约，这些是
   机制债，不是产品语义。
3. 不授权删除 Node/旧 TS；P7 保持 open，符合“不伪造完成”。
4. 如果之后 owner 决定必须收口，B 已经把可安全退休的部分清掉，C 只需
   处理剩余 143 个，决策边界更清楚。

**B 的代价**：P7-1 仍是 143 planned；Node 仍保留；G5 仍红。团队需要接受
“这一阶段不做 P7 完成声明”。

> 如果组织的硬目标是“本季度完成 P7 删除”，B 不能达成；此时唯一可行的是
> C，且必须按 §6.4 由 owner 签字。技术团队不应自行把 B 升级成 C。

**风险/工作量**：

| 项 | 估计 |
| --- | ---: |
| 矩阵阶段 1（fixture/manifest/comparator + 23 exact + 4 顶层） | 13–21 d |
| 矩阵阶段 2–5（STATE/DOCS/PROBE/ROOT） | 15–25 d |
| D `0063` + C `0243` | 3–5 d |
| B 退休治理 + ledger/validator | 5–8 d |
| **合计** | **36–59 d** |

风险：中。最大风险是 TS 6.0.3 旧 oracle 与 7.1.0-beta native 的诊断/类型
偏差；必须逐 case calibration，不能 native 回放覆盖期望。

### 6.2 P7 执行顺序

**Phase 0：决策与契约（1 周）**

1. 回答 §5 的 D1–D8；记录 owner/date/scope。
2. 决定 B 的 30 个退休清单；扩展 `retirement_decision.scope` 与 sentinel。
3. 更新 G5 文案：明确 retired 例外；若不做 C，保留 planned 阻塞。
4. **不要把工作树里未提交的 `real_matrix.rs`/retarget 改动直接算绿**；
   先作为独立 slice 评审/提交，再回填 ledger。

**Phase 1：矩阵阶段 1（13–21 d）**

1. 冻结 35 个 sample、33 个 WXML、`matrix.json` projection；
   `.gitattributes` 保护 CRLF/BOM。
2. 默认 manifest/comparator 测试；required-native job：
   `WXML_TEST_NATIVE=1` + `WXML_REQUIRE_REAL_ENGINE=1` + pinned `TSGO_*`。
3. 断言 23 个 in-place exact + 4 个顶层 exact；16 个 gap 登记 blocked。
4. 正例 sentinel：2339/2532/18047/18046/2322 必须观测到。
5. 回填：110 green / 191 planned / 38 retired；blocked 32。

**Phase 2：矩阵阶段 2–5（15–25 d）**

1. STATE adapter：5 个 `Diagnostics=Disabled`；单独 gate，不能和矩阵测试
   混在一个 PR。
2. DOCS 语义分类：3 个 `documentationSource`；contract 记录 native 无
   QuickInfo。
3. PROBE health：root any → `Unavailable`；先接线，再改矩阵测试。
4. ROOT checker classification：import type → `Unsupported`。
5. 每阶段回填：118/183 → 122/179 → 124/177 → 126/175。

**Phase 3：D/C 条件 exact + B 退休（8–13 d）**

1. `TS-COMPILER-0063`：digest/重开测试。
2. `TS-ANALYSIS-0243`：若做，`jsCheck` 配置 + `Disabled`。
3. D 6 退休；B 的 C/blocked 24 退休；更新 validator/contract/G5。
4. 重新计数：**128 / 143 / 68**。如果对不上，停止，不要改常量硬凑。

**Phase 4：收口决策门（1 d）**

- 若 owner 不签 D9：**停止**。P7-1 保持 143 planned；Node/TS 保留；
  输出“P7 未完成”的报告。
- 若 owner 签 D9：进入 Phase 5，使用 `coverage_waiver`，不要写 exact。

**Phase 5：只有签了 C 才执行**

1. P7-2：归档/删除 `backends/typescript/test`，让 oracle 进入 Frozen；
   确认 `native_parity` 仍绿。
2. P7-3：12 个 delete gate 标记 `old_test_removed`；同一变更更新
   `scripts/p7-freeze.sh` 和 freeze JSON。
3. P7-3b：11 个 retarget gate（9 stdio + perf + stability）；不要删除
   retarget gate，`p7_action` guard 会拒绝。
4. P7-4/5：删除旧 TS 树/samples（fixtures 已冻结）、`backend_node`、
   `semantic_tools` Node 路径、v1 packager 写路径。
5. P7-6/7：全量门禁、release、install/upgrade/rollback、Zed smoke、
   G6/G7 报告。

### 6.3 Stop conditions

**矩阵/证据**

- required-native job 未运行，或 `#[ignore]` 只显示 ignored 而非 passed → 停。
- 缺少 positive sentinel（2339/2532/18047/18046/2322）→ 停。
- 比较器反向自测失败，或存在 `BLESS`/`UPDATE_EXPECT`/`INSTA_UPDATE` 写期望
  → 停。
- TS 6.0.3 → 7.1.0-beta 的偏差没有 owner 逐 case 书面接受 → 停。

**状态/契约**

- adapter 仍报 `Supported`，测试却推导 `Disabled`/`Unavailable`/`Unsupported`
  → 停。
- DOCS/DEFK 从 Markdown/fileId 猜，而不是纯分类 + native 事实 → 停。
- STATE/DOCS/PROBE/ROOT 改动导致既有 green 的 `Supported` 回归 → 停。

**退休/waiver**

- 没有 `retirement_decision`（owner/date/scope/evidence/replacement）→ 停。
- 用 `retired` 包装 `weaker`/`blocked` 或“没测过” → 停。
- coverage waiver 没有 owner 签字，或 G5 未改写 → 停。
- ledger 计数、validator 常量、`counts.statuses` 对不上 → 停。

**删除**

- legacy oracle 不是 Frozen → 停（`old_test_removed` 会硬失败）。
- 12 个 delete gate 未 `mandatory_green`/review，或 `old_test_removed=false`
  → 停。
- 11 个 retarget gate 未 retarget，且 `p7_action=retarget` → 停。
- `backend_node`/Node env/`semantic_tools` 静默 return/v1 packager 仍有活跃
  引用 → 停。
- G6/G7 未通过 → 停。

### 6.4 什么情况下改推荐 C

只有在以下条件**同时**满足时，才建议从 B 升级到 C：

1. Owner 书面接受 143 个 planned 不再是迁移门禁（coverage waiver）。
2. 矩阵阶段 1–5 已落地，128 green 是真实证据，不是计划。
3. 23 个 gate 的 review/retarget 已排期，删除后不会留下无替代的 Node 路径。
4. 产品团队接受 110 四线外（含 96 weaker）/C 19/blocked 14 的残余风险，
   并有回归/监控计划。

否则，C 只是把“未完成”改写成“已 waiver”，会让 P7 报告失真。

---

## 7. 本文件不声称

1. 不声称任何 case 已在本轮转 green、reviewed 或 closed；128 是评估上限，
   测试尚未全部编写/运行。
2. 不声称 `real_matrix.rs` 或工作树里未提交的 retarget 改动已通过；它们
   不在 ledger 证据里。
3. 不声称矩阵阶段 2–5 的生产改动已完成；STATE/DOCS/PROBE/ROOT 仍是设计。
4. 不声称 C1 已正式废止；ADR-0008 仍是 `proposed`，C1 文档仍 `accepted`
   但注明 native 不实现。
5. 不声称 `retired-with-decision` 等于迁移完成；它是非 green 终态。
6. 不声称 `reviewed + 残余风险` 是当前 ledger 的合法状态；它需要新增
   waiver 状态/scope 和 G5 改写。
7. 不声称 Node/旧 TS 可以现在删除；P7-1/P7-2/23 gate 尚未满足。
8. 不声称 23 个 gate 已 `mandatory_green` 或 reviewed；它们仍是 opt-in
   `gate_green`。
9. 不声称本简报修改了 ledger、contract、测试、代码、README、CI 或脚本；
   只新增本文件。
10. 未运行 `cargo test`、`cargo clippy`、`cargo fmt`；所有数字来自只读
    JSON/文档/源码统计。

---

## 8. 只读复核记录

本文件写入前执行的只读检查（未运行测试）：

```text
git log --oneline -3
git status --short
jq '.counts.statuses, .counts.migration_evidence' docs/plans/tsgo-test-ledger.json
jq '[.cases[] | .status] | group_by(.) | map({k:.[0],n:length})' docs/plans/tsgo-test-ledger.json
jq '[.cases[] | .migration_evidence.parity] | group_by(.) | map({k:.[0],n:length})' docs/plans/tsgo-test-ledger.json
jq '[.ignored_rust_tests[] | .status] | group_by(.) | map({k:.[0],n:length})' docs/plans/tsgo-test-ledger.json
grep -n "GREEN_CASES|PLANNED_CASES|RETIRED_CASES|NEAR_EXACT_CASES|WEAKER_CASES|BLOCKED_CASES" crates/wxml-lsp/tests/native_parity/ledger.rs
grep -n "G5 通过需要|retired-with-decision 有两种" docs/reference/tsgo-test-contract.md
grep -n "fn serve_default|run_stdio_native_default|pub mod backend_node" crates/wxml-lsp/src/main.rs crates/wxml-lsp/src/lib.rs
jq '.old_ignored_gates.gates | group_by(.p7_action) | map({action:.[0].p7_action,n:length})' docs/evidence/data/legacy-p7-freeze.json
```

复核结果：

```text
counts.statuses = { planned: 218, green: 83, retired-with-decision: 38 }
counts.migration_evidence = { assessed_cases: 209, exact: 83, near_exact: 3,
  weaker: 100, structural: 7, blocked: 16,
  remaining_planned_without_evidence: 92 }
matrix = 43 planned (33 in-place + 10 top-level; 4 weaker)
C = 12 green + 47 planned
D = 41 planned (34 matrix + 7 non-matrix)
blocked = 16
union planned = 92; outside = 126
23 gates = 23 gate_green; mandatory/reviewed/removed = 0/23
legacy-p7-freeze = 12 delete + 11 retarget
```

未执行 `cargo test`、`cargo clippy`、`cargo fmt`；未修改
` docs/plans/tsgo-test-ledger.json`、` crates/wxml-lsp/tests/native_parity/ledger.rs`
或任何测试/生产/README 文件。
