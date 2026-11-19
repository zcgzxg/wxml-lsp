# tsgo wave-11 C 类（59 个 checker/native 能力 case）能力矩阵与分诊（2026-11-20）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD：任务给定 `43625ee`（`test(p7): enforce retarget gate retention`）。读取期间
  并行提交 `51abc13`（`test(tsgo): close conditional parity and ledger backfill`）
  落地；台账最终值以 `51abc13` 为准。
- 记录日期：2026-11-20
- 范围：**只新增本文件**。未修改生产代码、测试源码、`Cargo`、delivery、ledger
  JSON（`docs/plans/tsgo-test-ledger.json`）、ledger Markdown、contract、ADR、
  README、CI 或脚本；未 commit，未启动 review；**未运行任何测试**，只做只读
  读取、`jq`/`grep`/`sed`。
- 读取开始时工作树有 7 个已修改文件和 3 个未跟踪文档；读取期间并行提交
  `51abc13` 把 ledger/contract/wave-10b 落账，最终 `git status` 只剩本文件与
  两个并行未跟踪文档（`2026-11-20-tsgo-wave11-contract-decisions.md`、
  `2026-11-20-tsgo-wave11-retire-audit.md`）。本文件不修改、不评估这些并行内容；
  D 类裁决文件的 41 个 case 与本文 59 个 case **没有 id 交集**，但 §8 会指出
  C/D 分类边界存在重叠（C 里有 36 个在断言 C1 字段），需要 integrator 合并处理。
- 本文不把 `gate_green`、opt-in `native_engine_real`、`near_exact`/`weaker`
  推荐或并行 wave-10 回填结果当作 case `green`。

**结论先行：**

1. 59 个 C case 在读取时**全部仍为 `planned`、无 `migration_evidence`**。
   按 case 级 status 计数是 **70 `green` / 233 `planned` / 36
   `retired-with-decision`**，无证据 planned **142**；`counts.statuses` 在
   `51abc13` 已同步为 70/233，但 `counts.migration_evidence.
   remaining_planned_without_evidence` 仍写 106（与 case 级 142 不一致）。
   本文按 case 级 233 计算，回填时必须重新读取 JSON。
2. 59 个 C case 的真正依赖不是"全部需要 checker"：
   - **10 个可以只用现有 pinned API + 新 Rust 纯函数/组合做到 exact**（5 个消息
     清洗、5 个 probe health）。
   - **3 个是条件 exact**：`TS-ANALYSIS-0332`（layout 边界）、
     `TS-ANALYSIS-0315`（每请求状态替代 C1 `states`）、
     `TS-ANALYSIS-0243`（需先有 `jsCheck: off` 配置）。
   - **18 个必须真实 checker**（其中 14 个还同时断言 C1 字段），默认测试套件
     不能 exact。
   - **36 个（61%）断言 C1 `analyze` 响应字段**（`states`、`probeProblems`、
     `build.*`、`jsCheckApplied`、`diagnosticsExact`、`suppressedDiagnostics`、
     `eventResults`、`insertions`/`mounts`、`moduleKind`）。C1 不在 ADR-0008 的
     C0/LSP 目标内；建议**书面 waiver/retire**，不要为测试补一个 C1 服务器。
   - **9 个是 pinned API 不存在、契约禁止或行为已被 native 有意取代**，必须
     保持 blocked 或 retire。

   这些数字**有重叠**（14 个同时属于 checker 与 C1，1 个同时属于 C1 与 pinned），
   不是相加关系；互斥分组见 §3，全量判定见 §4。
3. 下一波不建议再按 wave-9 的"59 个一起 blocked"处理。推荐 **12-13 个**：
   5 个 `TS-MESSAGES-*`、5 个 `TS-PROBES-*`、`TS-ANALYSIS-0332`、
   `TS-ANALYSIS-0315`，可选 `TS-ANALYSIS-0243`。预计 3-5 个工作日，能把
   exact 从 70 提到 80（只做前 10 个）或 82-83（含条件 exact），
   planned 233 → 223/221/220。
4. **不能把这 59 个都做成 exact。** 36 个 C1 case 需要产品/架构裁决；18 个
   checker case 在没有"默认运行的 checker"前只能 weaker/blocked。P7-1 的
   planned=0 不能由 C 类单独完成。

---

## 1. 基线与计数

### 1.1 台账实际值（`51abc13`）

| 项 | 值 | 说明 |
| --- | ---: | --- |
| 总 case | 339 | |
| `green`（case 级） | 70 | `counts.statuses` 已同步 |
| `planned`（case 级） | **233** | `counts.statuses` 已同步 |
| `retired-with-decision` | 36 | 一致 |
| 无证据 planned（case 级） | 142 | `counts.migration_evidence.remaining_planned_without_evidence` 仍写 106 |
| `exact` | 70 | 全部为 `green` |
| `near_exact` | 2 | |
| `weaker` | 96 | |
| `structural` | 7 | |
| `blocked` | 22 | |
| `assessed_cases` | 197 | |

> 并行 wave-10b 已由 `51abc13` 落账，`counts.statuses` 一致；但
> `counts.migration_evidence.remaining_planned_without_evidence` 仍是 106，
> 与 case 级的 142 不一致。本文的 59 集合未受影响（全部无 evidence），但任何
> 回填都必须以 case 级字段重新计数，不能复制本表或 wave-9 的 58/245。

### 1.2 C 集合

来自 `docs/evidence/2026-11-20-tsgo-remaining-ledger-wave9-plan.md` §2.3：

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

合计 **59**，与 wave-9 一致。

---

## 2. 能力盘点（代码事实）

| 能力 | 现状 | 证据 |
| --- | --- | --- |
| 原生方法白名单 | 26 个方法；无 `getQuickInfoAtPosition`/`getDefinitionAtPosition`/`getRenameInfo`/`getCompletionEntryDetails` |  crates/wxml-backend-tsgo/src/config.rs:135； crates/wxml-backend-tsgo/src/native/protocol.rs:490 |
| 类型查询 | `getTypeAtPosition` + `typeToString`；质量分 `Known`/`Any`/`Unknown`/`Missing` |  crates/wxml-backend-tsgo/src/native/session.rs:1125； crates/wxml-backend-tsgo/src/query/type_info.rs:51 |
| 符号 | `getSymbolAtPosition`；`null` = 位置无符号 |  crates/wxml-backend-tsgo/src/native/session.rs:1167 |
| 文档/JSDoc | `getDocumentationComment` + `getJsDocTags` |  crates/wxml-backend-tsgo/src/native/session.rs:1399 |
| 定义 | symbol → identifier → `getReferencedSymbolsForNode` → 同快照 AST；目标为项目相对 `FileId`，不可表示则 `Partial` |  crates/wxml-backend-tsgo/src/native/session.rs:1795； crates/wxml-backend-tsgo/src/query/definition.rs:70 |
| 引用 | `references` + handle 解码 |  crates/wxml-backend-tsgo/src/native/session.rs:1745 |
| 补全 | `includeSymbol` 可选；无 per-entry 文档/详情 |  crates/wxml-backend-tsgo/src/native/session.rs:2218 |
| 文档符号 | AST 扫描，无 checker |  crates/wxml-backend-tsgo/src/native/session.rs:2275； crates/wxml-backend-tsgo/src/native/document_symbol.rs:155 |
| 诊断 | semantic + syntactic；`RawDiagnostic.file_name` 已解析；script-owned 诊断排除并 `Partial` |  crates/wxml-backend-tsgo/src/native/session.rs:1109； crates/wxml-backend-tsgo/src/query/diagnostics.rs:102； crates/wxml-lsp/src/backend_tsgo/diagnostics.rs:47 |
| 原位 shadow | `BuiltShadow` 暴露 status/reason/js_check_applied/registration/data_root/properties/view |  crates/wxml-backend-tsgo/src/shadow/layout.rs:348 |
| 派生视图 | `insertions`/`mounts`/`generated_declarations`/`probe_anchors` 全部可用 |  crates/wxml-backend-tsgo/src/shadow/layout.rs:327； crates/wxml-backend-tsgo/src/shadow/mapping.rs:171 |
| locals/guards | 类型化 locals、嵌套同名不 TDZ、guard 渲染；超限走 `FragmentLimit` + `Partial` |  crates/wxml-backend-tsgo/src/shadow/locals.rs:149； crates/wxml-types/src/template.rs:73 |
| JS check policy | 枚举有 `Inherit`/`Pragma`/`Off`；**生产适配器硬编码 `Inherit`** |  crates/wxml-backend-tsgo/src/shadow/layout.rs:113； crates/wxml-lsp/src/backend_tsgo/input.rs:732 |
| 消息清洗 | 只做控制字符/空白折叠/512 字符截断；不删 `__wxml_*`、不展开 typings 泛型、不折叠 intersection |  crates/wxml-lsp/src/handlers/mod.rs:174； crates/wxml-lsp/src/handlers/mod.rs:52 |
| probe health | **不存在**（全仓无 `probe_health`/`probeProblems`） | `grep` 无命中 |
| module kind | **不存在**；`getSourceFileMetadata` 在白名单但无 session 方法；其 `impliedNodeFormat` 也不是 `script/module` |  crates/wxml-backend-tsgo/src/native/protocol.rs:56 |
| run 级 `states` | **不存在**；只有每请求一个 `CapabilityStatus` |  crates/wxml-backend-tsgo/src/query/mod.rs； crates/wxml-lsp/src/backend_tsgo/mod.rs:1084 |
| C1 `analyze` | **不存在**；ADR-0008 的目标是 C0/LSP，不是 C1 analyze |  docs/adr/0008-rust-tsgo-no-node-backend.md； docs/reference/type-bridge-c1.md |

---

## 3. 分组矩阵

分组口径（一个 case 只归一个主组）：

- **G1**：现有 pinned API 之外只需要一个 Rust 纯函数，等价语义直接可移植。
- **G2**：现有 pinned API + 现有 shadow 产物足够，需要新的 Rust 组合（证据注入/
  决策逻辑），但不需要 C1 envelope。
- **G3**：断言真实 checker 结果，**且**同时断言 C1 字段；默认套件不可 exact。
- **G4**：只断言真实 checker 结果，不依赖 C1；默认套件不可 exact。
- **G5**：只断言 C1 `analyze` 请求/响应/状态聚合；native C0 没有对应物。
- **G6**：pinned API 不存在、契约禁止，或 native 已用不同行为有意取代。

**与任务书四类口径的映射：**

| 任务书口径 | 本文组 | case 数 | 说明 |
| --- | --- | ---: | --- |
| 现有 API 可直接补（只需测试/编排） | — | **0** | 严格意义上没有：G1 仍要新纯函数，G2 仍要新组合，A/B 类才是这一口径 |
| 需要新 Rust 组合（AST/checker 组合） | G1 + G2 | **11** | 5 个消息纯函数 + 5 个 probe health 决策层 + 1 个 layout mount 断言 |
| 需要 pinned API 不存在的能力 | G6 | **9** | QuickInfo 来源、script/module kind、`ts.ScriptTarget`、option digest、engine project id、bad-frame 继续 + `ping` |
| 契约禁止 | G6（部分）+ G5（部分） | **3 + 1** | `TS-SERVER-0203`（§3.3 禁止坏帧继续）、`TS-COMPILER-0095`（engine id 不可发布）、`TS-LAYOUTS-0101`（native 有意取代）；`TS-ANALYSIS-0287` 的 `moduleKind` 属 C1 形状 |
| 需要真实 checker（默认套件不可用） | G3 + G4 | **18** | 其中 14 个同时断言 C1 字段 |
| C1 envelope（不在 ADR-0008 目标内） | G5 | **21** | 建议 waiver/retire；其中 2 个可条件 exact |

> 任务书示例里的 **rename/roles/auto-import edit/document symbol check 不是这 59 个
> case 的缺口**：没有一个 C case 断言 rename/roles；`getRenameInfo`/
> `getEditsForRename`/`rename` 仍在拒绝清单（ §7 第 6 条）。真正缺的是
> QuickInfo/moduleKind 这类单点 API，以及 C1 层本身。

| 组 | 依赖 | case 数 | 代表 case | 当前已有能力 | 缺口 | 建议 slice | 工作量 |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| G1 | Rust 纯函数移植 | 5 | `TS-MESSAGES-0005/0016/0025/0032/0039` | 诊断消息发布、`sanitized_message`（仅控制字符/512 截断） | C1 §7 的内部符号清洗/泛型展开/intersection 折叠/200 截断 | 新 `query/message.rs`，~200 LOC + 5 测试 | 低（0.5-1 天） |
| G2 | 新 Rust 组合（无 C1） | 6 | `TS-PROBES-0064/0080/0098/0135/0153`、`TS-ANALYSIS-0332` | `ProbeAnchors`+`insertions`+`mounts`+symbol/type/diagnostics 证据；layout 已按 fragment 生成 mount | 无 probe health 决策层；无"同一表达式两个 fragment"断言 | 新 `query/probe_health.rs`（~300 LOC）+ layout 测试 | 中（1.5-2.5 天） |
| G3 | checker + C1 | 14 | `TS-GUARDS-0152/0174`、`TS-LOCALS-0261/0288`、`TS-MATRIX-0239/0252/0283/0319`、`TS-COMPONENT-0628/0697`、`TS-MAPPING-0306` | shadow/guards/locals 结构；真实 hover/diagnostics 管线（需真实引擎） | 默认套件无 checker；`states`/`probeProblems`/`build.*`/`jsCheckApplied`/`diagnosticsExact`/`suppressed*` 不存在 | 不实现；登记 weaker 或等 checker 门禁 | 高（不可在默认套件 exact） |
| G4 | 仅 checker | 4 | `TS-GUARDS-0128/0329`、`TS-COMPONENT-0550`、`TS-COMPILER-0132` | shadow + 真实 hover/diagnostics | 默认套件无 checker | 不实现；保留 blocked | 高 |
| G5 | C1 envelope | 21 | `TS-ANALYSIS-0272/0315/0371`、`TS-PROTOCOL-0141/0154/0178/0250/0329/0348/0368/0421/0450/0561/0606/0644/0653`、`TS-EVENTS-0387/0399` | 部分事实在 shadow/identity/options 层已存在 | 无 C1 request/response；`states`、`insertions[]`、`mounts[]`、`eventResults`、limits、fileId 空间 | **不实现**；写 waiver/retire 或把子事实登记 weaker | 低（决策）/高（实现 C1） |
| G6 | pinned 缺失/契约禁止/已取代 | 9 | `TS-LAYOUTS-0084/0101/0147`、`TS-ANALYSIS-0287`、`TS-COMPILER-0095`、`TS-PROTOCOL-0234/0479/0500`、`TS-SERVER-0203` | — | QuickInfo 来源、module kind、ScriptTarget 枚举、option digest 归一化、engine project id、bad-frame 继续 + ping | 保持 blocked 或 retire | 高/不可做 |

计数：5 + 6 + 14 + 4 + 21 + 9 = **59**。

**交叉计数（重要，存在重叠）：**

- 断言 C1 字段的：**36 / 59（61%）** = G5 21 + G3 14 + `TS-ANALYSIS-0287` 1。
- 需要真实 checker 的：**18 / 59** = G3 14 + G4 4。
- 同时需要 checker 与 C1 的：**14**；因此 "C1 或 checker" 的并集是
  **40**（36 + 4 个仅 checker）。
- 不依赖 checker、也不依赖 C1 的：**11**（G1 5 + G2 6）。
- 其余 8 个是 G6 中不含 C1/checker 的 pinned/契约项
  （`TS-LAYOUTS-0084/0101/0147`、`TS-COMPILER-0095`、
  `TS-PROTOCOL-0234/0479/0500`、`TS-SERVER-0203`）。
  40 + 11 + 8 = **59**，与上面的六组计数一致。
- 可无条件 exact 的是 G1 5 + G2 中的 5 个 probe = **10**；
  `TS-ANALYSIS-0332` 是第 11 个可移植项，但需评审接受 layout 边界。

> 口径说明：上面的 "C1 依赖 36" 按"**必须引入 C1 envelope 才能满足 oracle**"
> 计数。`TS-ANALYSIS-0332` 的 oracle 虽然写 `result.mounts`，但 native
> `DerivedView.mounts` 是同一事实，不需要 C1 层，所以归 G2 而不计入 36。
> `TS-PROTOCOL-0421/0450` 则因为断言 `insertions[].file`/`mounts[].file`
> 而必须计入 G5。

---

## 4. 全量判定表（59）

判定列含义：
`exact` = 可用默认运行的 Rust 测试等价；`条件 exact` = 需评审接受边界/裁决；
`weaker` = 只能断言 in-place 半；`retire/waiver` = 建议书面裁决；
`blocked` = 不得伪造，保持 planned。

### 4.1 G1：消息清洗（5，exact）

| Case | 关键断言 | 判定 | 下一步 |
| --- | --- | --- | --- |
| `TS-MESSAGES-0005` | `__wxml_root.` 去除；`__wxml_check_0` → `helper` | exact | 新 `query/message.rs` |
| `TS-MESSAGES-0016` | `FilterUnknownType<>`/`PropertyOptionToData<>` 展开 | exact | 同上 |
| `TS-MESSAGES-0025` | `__WxmlProps_0` → `declared properties` | exact | 同上 |
| `TS-MESSAGES-0032` | 重复 intersection 折叠 | exact | 同上 |
| `TS-MESSAGES-0039` | ≤200 字符且以 `…` 结尾 | exact | 同上 |

实现依据： backends/typescript/src/messages.ts:1（`MAX_LENGTH=200`，9 个 wrapper），
断言逐字见 backends/typescript/test/messages.test.ts:5。

**契约前提**：`docs/reference/tsgo-native-contract.md` §9.3 目前写"message 保留原文，
仅做换行/长度预算处理"，与 C1 §7 的清洗要求冲突。建议在 contract 加一句
"用户可见消息先按 C1 §7 去内部符号，原文只进内部日志"，否则实现会被认为违反
native 合同。

### 4.2 G2：新 Rust 组合（6，exact/条件 exact）

| Case | 关键断言 | 判定 | 下一步 |
| --- | --- | --- | --- |
| `TS-PROBES-0064` | probe root 解析为 `any` → problem 含 `probe root` | exact | `probe_health` + 注入 type 证据 |
| `TS-PROBES-0080` | 诊断与 insertion 相交 → problem 含 `intersects insertion` | exact | 同上，注入 diagnostic |
| `TS-PROBES-0098` | 按精确 offset 取符号；keyword 处 `has no symbol` | exact | 同上，注入 symbol 证据 |
| `TS-PROBES-0135` | 重复 helper 通过 insertion 相交被发现 | exact | 同上，注入 diagnostic |
| `TS-PROBES-0153` | `moduleBefore != moduleAfter` → `module classification flipped` | exact | 同上，两个值都是注入参数 |
| `TS-ANALYSIS-0332` | 同一表达式两个 fragment → 2 个 mount，id `f0#0`/`f1#0` | 条件 exact（layout 边界） | shadow_layout 测试 |

probe health 的输入/输出可直接照 `backends/typescript/src/probes.ts:92`：
输入 `helperOffsets`/`rootOffset`/`insertions`/`mounts`/`moduleBefore`/`moduleAfter`，
输出 5 类固定字符串。native 侧 `ProbeAnchors`（ crates/wxml-backend-tsgo/src/shadow/layout.rs:314）、
`InsertionRecord`（ crates/wxml-backend-tsgo/src/shadow/layout.rs:286）、
`MountRecord`（ crates/wxml-backend-tsgo/src/shadow/mapping.rs:171）、
`DerivedView.mounts`（ crates/wxml-backend-tsgo/src/shadow/layout.rs:327）已经齐备；
`type_query`（ crates/wxml-backend-tsgo/src/native/session.rs:1125）和
`symbol_at_position`（ crates/wxml-backend-tsgo/src/native/session.rs:1167）提供 checker 证据。

**为什么可以 exact**：这 5 个 TS oracle 都是 `ok=true` 的 unit 测试，被测函数
`checkProbeHealth` 的决策逻辑只依赖注入的 facts；native 用同一决策逻辑 + 注入
同构 facts，属于同一函数的等价移植，不需要真的跑 checker。若评审不接受"证据注入"
作为等价边界，则降为 `near_exact`，仍不实现 checker。

`TS-ANALYSIS-0332` 的 oracle 断言 `result.mounts`/`result.response.fragments`；
native `view.mounts` 与输入 `FragmentSpec` 是同一事实，`mount_id` 生成规则
`format!("{}#0", fragment_id)` 与 C1 一致（ crates/wxml-backend-tsgo/src/shadow/layout.rs:977）。若评审坚持
"必须是 C1 response"，则降为 weaker。

### 4.3 G3：checker + C1（14，不可 exact）

| Case | 关键断言 | 判定 | 说明 |
| --- | --- | --- | --- |
| `TS-GUARDS-0152` | 嵌套 guard 累积收窄；`probeProblems` | blocked | checker + C1 |
| `TS-GUARDS-0174` | guard 条件看到 loop local；`probeProblems` | blocked | checker + C1 |
| `TS-GUARDS-0321` | `jsCheckApplied=="inherit"` + 诊断数 >0 | blocked | checker + C1 |
| `TS-LOCALS-0261` | 8 个 typed local 的 type/quality + `states.*` | blocked | checker + C1 |
| `TS-LOCALS-0288` | 内层同名 loop 不 TDZ；`probeProblems` | blocked | checker + C1 |
| `TS-LOCALS-0312` | JS 下 locals any；`states.*` + `build.unresolvedBindings` | blocked | checker + C1 |
| `TS-MAPPING-0306` | `suppressedDiagnostics>0`、`suppressedDetails` codes `[2322]` | blocked | checker + C1 指标 |
| `TS-MATRIX-0239` | `states.hover/diagnostics` + hover `string` | blocked | checker + C1 |
| `TS-MATRIX-0252` | `jsCheckApplied` + `states.*` + 诊断数 | blocked | checker |
| `TS-MATRIX-0283` | BOM+hashbang+pragma + 2339 精确 range | blocked | checker + B-11 |
| `TS-MATRIX-0319` | `noUncheckedIndexedAccess` → 2532；`diagnosticsExact` | blocked | checker |
| `TS-MATRIX-0335` | `noImplicitThis:false` → hover `unavailable` + reason | blocked | checker + C1 |
| `TS-COMPONENT-0628` | `probeProblems` + `build.behaviorIncomplete` + 2339 过滤 | blocked | checker + C1 |
| `TS-COMPONENT-0697` | `behaviorIncomplete` + 2339 codes | blocked | checker + C1 |

其中 **7 个**的 in-place 半已经在现有 `BLOCKED_CASES` 中记录：
`TS-GUARDS-0152/0174/0321`（ crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:30）、
`TS-LOCALS-0261/0288/0312`（同文件）、`TS-MAPPING-0306`
（ crates/wxml-backend-tsgo/tests/shadow_mapping.rs:25）。
这 7 个可以**只做台账回填**登记为 `weaker`（引用现有测试），把"无证据 planned"
减少 7，但 planned 不变；不要把它们写成 exact。
其余 7 个（`TS-COMPONENT-0628/0697`、`TS-MATRIX-0239/0252/0283/0319/0335`）
连 blocked 断言都还没有，不能直接登记 weaker。

### 4.4 G4：仅 checker（4，不可 exact）

| Case | 关键断言 | 判定 | 说明 |
| --- | --- | --- | --- |
| `TS-GUARDS-0128` | truthiness 去 null；narrowed hover `string`；unnarrowed 有诊断 | blocked | 需要真实 checker |
| `TS-GUARDS-0329` | JS 视图保留 JSDoc；hover `string` + `Visible title.` | blocked | 需要真实 checker |
| `TS-COMPONENT-0550` | `checkJs` 下 JSDoc cast；hover `number` + `js count` + 诊断空 | blocked | 需要真实 checker |
| `TS-COMPILER-0132` | `typeRoots` 加载 typings；`typeText` 不是 `any` | blocked | 需要真实 checker + project 配置 |

这 4 个连 C1 字段都不依赖，产品上最接近 F1 验收；但默认套件没有 checker，
只能由 opt-in `native_engine_real` gate 证明，而 gate 不能当 case 证据。

### 4.5 G5：C1 envelope / 请求校验（21，retire/waiver 或 weaker 子事实）

| Case | 关键断言 | 处理建议 |
| --- | --- | --- |
| `TS-ANALYSIS-0226` | `probeProblems` + `states` + `definition.kind=="generated"` | retire/waiver（definition kind 是 C1 DTO） |
| `TS-ANALYSIS-0243` | `states.diagnostics=="disabled"` + hover/definition `partial` + 诊断空 | 条件 exact（见 §5）；否则 waiver |
| `TS-ANALYSIS-0272` | `insertions[].viewRevision/backendFileVersion` | retire/waiver；facts 在 `RequestIdentity`，无响应字段 |
| `TS-ANALYSIS-0315` | `droppedDefinitions:1` → 仅 definition `partial` | 条件 exact（见 §5）；否则 waiver |
| `TS-ANALYSIS-0371` | `validateRequest` 抛 `bad-protocol` | retire/waiver |
| `TS-EVENTS-0375` | event 字符串计入 `maxTotalBytes` | retire/waiver |
| `TS-EVENTS-0387` | 早返回路径 `eventResults` | retire/waiver；native 无 event 分析管线 |
| `TS-EVENTS-0399` | 完整路径 `eventResults` + 诊断 | retire/waiver |
| `TS-PROTOCOL-0141` | 同层重复 binding → `bad-fragment` | waiver 或政策裁决（native 是去重，不是拒绝） |
| `TS-PROTOCOL-0154` | guard 链/条件超限 → `bad-fragment` | waiver 或政策裁决（native 是 `FragmentLimit` + `Partial`） |
| `TS-PROTOCOL-0178` | bindings/guards/components 计入字节预算 | retire/waiver |
| `TS-PROTOCOL-0250` | 输入 limits 在语言服务前生效 | retire/waiver |
| `TS-PROTOCOL-0329` | limits 合并默认值 + server ceiling | retire/waiver |
| `TS-PROTOCOL-0348` | enum/protocolVersion 校验 | retire/waiver（native 只校验 shape） |
| `TS-PROTOCOL-0368` | request 驱动 mounts/policy | retire/waiver；policy Off 也未接线 |
| `TS-PROTOCOL-0421` | insertions/mounts 使用 script fileId | weaker 可选（`fileIdOf` 半）；否则 waiver |
| `TS-PROTOCOL-0450` | alias project 相对 fileId + definition fileId | weaker 可选（definition 半）；否则 waiver |
| `TS-PROTOCOL-0561` | C1.1 target/locals 接受/拒绝 malformed | retire/waiver（typed Rust 不可构造 malformed） |
| `TS-PROTOCOL-0606` | files/view/analysisStamp 校验 | retire/waiver |
| `TS-PROTOCOL-0644` | `propertiesOverlay: off` 显式拒绝 | retire/waiver（native 无此 policy 字段） |
| `TS-PROTOCOL-0653` | 非法 local 名 → `bad-fragment` | waiver 或政策裁决（native 静默跳过） |

`TS-ANALYSIS-0243` 和 `TS-ANALYSIS-0315` 虽然主组是 C1，但它们的断言本质是
**状态隔离**，可以用 typed 断言转换（见 §5）；其余 19 个建议直接裁决。

### 4.6 G6：pinned 缺失/契约禁止/已取代（9，blocked）

| Case | 缺口 | 判定 | 依据 |
| --- | --- | --- | --- |
| `TS-LAYOUTS-0084` | `documentationSource=="quickinfo"` | blocked/retire | 契约 §9.8：无 QuickInfo/display parts； native 只有 docs/type |
| `TS-LAYOUTS-0101` | hover 只有 `string`（union 不可见） | **retire** | native 有意用 `apply_declared_property_type` 发布 `string \| number`（ crates/wxml-lsp/src/backend_tsgo/hover.rs:101），与本 oracle 相反 |
| `TS-LAYOUTS-0147` | 2339 + hover `type=any` + `quality=missing` | blocked/retire | native 质量映射：`any`→`Any`，error→`Unknown`，不会出现 `Missing+any`（ crates/wxml-backend-tsgo/src/query/type_info.rs:51） |
| `TS-ANALYSIS-0287` | `moduleKind.after=="script"` | blocked | 无 script/module 分类；`getSourceFileMetadata.impliedNodeFormat` 是 CJS/ESM，不是 `externalModuleIndicator` |
| `TS-COMPILER-0095` | `projectB != projectA`（engine id） | blocked | 契约 §6.1/§7.4：engine id 不得作为稳定标识；语义版需真实引擎 |
| `TS-PROTOCOL-0234` | option digest 与 checkout path 无关 | blocked | native `options_digest` 直接序列化 options，`baseUrl` 绝对路径会改 digest（ crates/wxml-backend-tsgo/src/input.rs:1738） |
| `TS-PROTOCOL-0479` | `lib`→`lib.*.d.ts`、`target`→`ts.ScriptTarget` 枚举 | blocked | native 只做 shape 校验并原样转发字符串；无 `ts.ScriptTarget` |
| `TS-PROTOCOL-0500` | target 名大小写 → `ScriptTarget` | blocked | 同上 |
| `TS-SERVER-0203` | 超长 frame 丢弃后继续 + `ping.ok` | blocked | 契约 §3.3：坏帧终止/重启，不重新同步；native 无 `ping`；wave-9 §6.10 已列为不可伪造 |

---

## 5. 可用 typed 断言转 exact 的 case

### 5.1 可直接 exact（10）

| Case | TS 断言 | native 等价落点 | 前提 |
| --- | --- | --- | --- |
| `TS-MESSAGES-0005/0016/0025/0032/0039` | 纯字符串变换 | 新 `query/message.rs` + 5 个单测 | contract §9.3 加一句清洗例外 |
| `TS-PROBES-0064/0080/0098/0135/0153` | `checkProbeHealth` 的 problem 列表 | 新 `query/probe_health.rs` + 注入 facts + 5 个测试 | 评审接受"证据注入"作为 unit oracle 等价边界 |

### 5.2 条件 exact（3）

| Case | TS 断言 | native 等价 | 需要什么 | 风险 |
| --- | --- | --- | --- | --- |
| `TS-ANALYSIS-0315` | `droppedDefinitions:1` → definition `partial`，hover/diagnostics `supported` | 每请求 `CapabilityStatus` 隔离：definition `Partial` + hover/diagnostics `Supported` | 评审接受"每请求状态"替代 C1 `states`；或新增一个纯 `run_status` 聚合器 | 若坚持 C1 `states`，只能 weaker |
| `TS-ANALYSIS-0332` | 同一表达式两个 fragment → 2 mounts，id `f0#0`/`f1#0` | `view.mounts.len()==2` + `fragment_id`/`mount_id` 断言 | 评审接受 layout 边界 | 若坚持 C1 response，只能 weaker |
| `TS-ANALYSIS-0243` | `states.diagnostics=="disabled"`，hover/definition `partial`，诊断空 | 新 `jsCheck: off` 配置 + shadow `JsCheckPolicy::Off` + 每请求状态 | 产品决定是否在 native LSP 暴露 `jsCheck` 配置；当前硬编码 `Inherit` | 工作量中；若不做策略，保持 planned |

### 5.3 **不能**靠 typed 断言转 exact

| Case | 为什么不行 |
| --- | --- |
| `TS-LAYOUTS-0147` | oracle 要求 `type=="any"` 且 `quality=="missing"`；native 的质量映射不产生该组合，改映射会破坏契约 §9.4 |
| `TS-LAYOUTS-0084` | oracle 要求 `documentationSource=="quickinfo"`；pinned API 没有 QuickInfo，native 不能伪造来源 |
| `TS-PROTOCOL-0141/0154/0653` | oracle 要求 **拒绝**；native 是去重/截断/跳过。用 typed `Partial` 断言等于承认行为不同，只能 weaker 或政策裁决，不是 exact |
| `TS-MATRIX-0335` | oracle 的 `noImplicitThis:false → root any` 是 checker 事实；native 只能注入 any 再断言策略，不能证明前提 |
| `TS-GUARDS-0321` / `TS-MATRIX-0252` / `0283` / `0319` | 断言里有诊断**数量/代码**（1 条 2339、1 条 2532、`diagnosticsExact`），typed 状态不能替代 checker 产物 |

---

## 6. 推荐下一波 10-15 个 case

### 6.1 推荐清单（12 个，可选 +1）

| # | Case | 切片 | 落点 | 工作量 |
| ---: | --- | --- | --- | --- |
| 1-5 | `TS-MESSAGES-0005/0016/0025/0032/0039` | W11-A 消息清洗 | 新 `crates/wxml-backend-tsgo/src/query/message.rs` + `crates/wxml-backend-tsgo/tests/messages_sanitize.rs` | 低（0.5-1 天） |
| 6-10 | `TS-PROBES-0064/0080/0098/0135/0153` | W11-B probe health | 新 `crates/wxml-backend-tsgo/src/query/probe_health.rs` + `crates/wxml-backend-tsgo/tests/probe_health.rs` | 中（1.5-2.5 天） |
| 11 | `TS-ANALYSIS-0332` | W11-C mount 去重 | 追加到 `crates/wxml-backend-tsgo/tests/shadow_layout.rs` | 低（2-4 小时） |
| 12 | `TS-ANALYSIS-0315` | W11-D 状态隔离 | `crates/wxml-lsp/src/backend_tsgo/tests.rs`（每请求状态）或 `crates/wxml-backend-tsgo/src/query/run_status.rs`（聚合器） | 低-中（0.5-1 天） |
| +1 | `TS-ANALYSIS-0243` | W11-E jsCheck off | `crates/wxml-lsp/src/backend_tsgo/input.rs:732` + 配置/能力文案 + 测试 | 中（2-3 天） |

合计 **10 个无条件 exact**（W11-A 5 + W11-B 5）+ **2 个条件 exact**
（W11-C `TS-ANALYSIS-0332`、W11-D `TS-ANALYSIS-0315`）；W11-E
`TS-ANALYSIS-0243` 是第三个条件 exact，可选。
建议顺序：W11-A → W11-B → W11-C → W11-D；W11-E 等产品确认 `jsCheck` 配置后再做。

### 6.2 W11-A：消息清洗（5 case）

- 端口 `backends/typescript/src/messages.ts:28` 的 `sanitizeMessage`：
  - 9 个 wrapper 的单层泛型展开（`FilterUnknownType`、`PropertyOptionToData`、
    `FullPropertyToData`、`MixinProperties`、`MixinMethods`、`MixinData`、
    `ExtractProperties`、`ExtractMethods`、`ExtractBehaviorType`）；
  - `ROOT_PREFIX_PATTERN`、`HELPER_PATTERN`、`PROPS_TYPE_PATTERN`、`TYPINGS_PATTERN`；
  - `collapseDuplicatedIntersections`、`dedupeRepeatedLiterals`；
  - 空白折叠、`& &`→`&`、`MAX_LENGTH=200` + `…`。
- 不改 LSP 发布路径也能先 exact：测试直接调新函数。
- 若同一切片把它接到 `crates/wxml-lsp/src/handlers/diagnostics.rs:49`，必须加
  "raw 只进日志"的回归测试，并同步 contract §9.3。
- 不建议把 `MAX_DISPLAY_MESSAGE_CHARS` 从 512 改成 200；那是显示层预算，
  与 C1 消息清洗是两件事。

### 6.3 W11-B：probe health（5 case）

- 新模块输入：
  - `probe_anchors: &ProbeAnchors`（ crates/wxml-backend-tsgo/src/shadow/layout.rs:314）；
  - `helper_symbols: Vec<(name, Option<Symbol>)>`（来自 `symbol_at_position`）；
  - `root_type: Option<&TypeResult>`（来自 `type_query`）；
  - `insertions: &[InsertionRecord]`、`mounts: &[MountRecord]`；
  - `module_before/module_after: Option<&str>`；
  - `diagnostics: &[RawDiagnostic]`。
- 输出 problem 字符串必须与 TS 逐字一致（oracle 用 `includes`）：
  - `probe helper {name} has no symbol`
  - `probe root resolves to {any|missing}`
  - `diagnostic {code} intersects insertion {id}`
  - `module classification flipped from {before} to {after}`
  - `derived script is not part of the program`
- **本切片不接线**到 hover/diagnostics 的 status。接线会改变现有 green 的
  `Supported` 语义（例如 root 为 any 时当前仍 `Supported`），必须单独切片、
  单独 gate。可先加一条 `capabilities()` 文案说明 probe health 尚未发布。
- 风险：模块暂时没有生产调用者。缓解：它是 F1 truthfulness 的既定落点，
  在 B-05/F1 验收前允许作为 pub API 存在；若 owner 不接受，改为 `near_exact`
  并在接线切片里再回填。

### 6.4 W11-C：同一表达式两个 fragment（1 case）

在 ` crates/wxml-backend-tsgo/tests/shadow_layout.rs` 增加：
两个 `FragmentSpec { id:"f0"/"f1", expr:"title", ... }`，
断言 `view.mounts.len()==2`、`mount_id` 排序为 `["f0#0","f1#0"]`、
`fragment_id` 集合为 `{"f0","f1"}`、两个 mount 的 `start_utf16` 不同且
各自映射回自己的 `wxml_range`。

### 6.5 W11-D：状态隔离（1 case）

两个可选实现：
1. **最小**：adapter 测试，fake session 返回 definition `Partial`、hover/diagnostics
   `Supported`，断言三个结果互不影响。改动 0 行生产代码。
2. **完整**：新增纯 `run_status` 聚合器（`dropped_definitions`/
   `unmapped_diagnostics`/`probe_problems` → 三能力状态），逐条对应
   `finishedStates`（ backends/typescript/test/analysis.test.ts:315）。
   但该聚合器只有 C1 形态的消费者，建议先做方案 1，把方案 2 留给 C1 裁决。

---

## 7. 明确不可做/不应做

1. **不要为 18 个 checker case 造 fake checker。** `native_engine_real` gate
   通过不等于 case green（wave-9 §6.12）。这 18 个是
   `TS-COMPILER-0132`、`TS-COMPONENT-0550/0628/0697`、
   `TS-GUARDS-0128/0152/0174/0321/0329`、
   `TS-LOCALS-0261/0288/0312`、`TS-MAPPING-0306`、
   `TS-MATRIX-0239/0252/0283/0319/0335`。
2. **不要为 36 个 C1 case 补一个 C1 `analyze` 服务器。** ADR-0008 的目标是
   Rust 语义适配器 + C0/LSP；C1 请求/响应（`states`、`mounts`、`insertions`、
   `eventResults`、limits、`moduleKind`）没有生产消费者。正确动作是
   waiver/retire 或把可测子事实登记 weaker，不是复活 Node 协议。
3. **不要伪造 QuickInfo。** `documentationSource=="quickinfo"`（`TS-LAYOUTS-0084`）
   在 pinned API 与契约 §9.8 下都不存在；type + merged docs + tags 是唯一组合。
4. **不要为了 `TS-LAYOUTS-0101` 删除 union overlay。**
   ` crates/wxml-lsp/src/backend_tsgo/hover.rs:101` 的
   `apply_declared_property_type` 是 native 的有意修复；该 case 应 retire/waive，
   不是让 native 退回"union 不可见"。
5. **不要把 `TS-LAYOUTS-0147` 的 `Missing+any` 做成质量映射。**
   契约 §9.4 规定 `any`→`Any`、error/unknown→`Unknown`、`null`→`Missing`；
   改映射会破坏所有 hover 质量断言。
6. **不要用 rename/roles/auto-import edit 来关闭这些 case。**
   59 个 C case 没有一个断言 rename/roles；`getRenameInfo`/
   `getEditsForRename`/`rename` 仍在拒绝清单（ crates/wxml-backend-tsgo/src/native/protocol.rs:490）。
   合同 §9.9 要求完整引用集 + `expected_text` + recheck，当前连完整性 flag 都没有。
7. **不要用文本搜索补 probe health 的符号。** `TS-PROBES-0098` 明确断言
   "按精确 offset，不按名字搜索"；native 必须用 `ProbeAnchors` 的 offset +
   `getSymbolAtPosition`，不能用 `indexOf`。
8. **不要把 `TS-SERVER-0203` 的"丢弃后继续"做成 native 行为。**
   契约 §3.3 要求坏帧终止/重启；当前 native 是 fatal。该 case 保持 blocked。
9. **不要改 `MAX_DISPLAY_MESSAGE_CHARS` 冒充 C1 的 200 截断。**
   它是 LSP 显示预算；C1 清洗需要独立函数与独立常量。

---

## 8. 对台账分类的反哺（C/D 边界）

wave-9 把"需要 checker/native 能力"归为 C，把"需要契约裁决"归为 D；但实际
**36 个 C case 的断言主字段是 C1 响应**，与 D 的 C1/matrix 集合同类。当前
`docs/evidence/2026-11-20-tsgo-wave11-contract-decisions.md` 只处理 41 个 D case，
不含这 36 个。建议 integrator 二选一：

1. **扩 D 集合**：把 C 中 19 个纯 C1（`TS-ANALYSIS-0226/0272/0371`、
   `TS-EVENTS-0375/0387/0399`、`TS-PROTOCOL-0141/0154/0178/0250/0329/0348/0368/0421/0450/0561/0606/0644/0653`）
   并入 D 的 waiver/retire 清单；C 缩减为 40 个（含 5 个 messages、5 个 probes、
   `ANALYSIS-0332`、`ANALYSIS-0315`、`ANALYSIS-0243` 等）。
2. **保持 C/D 不动，但在 C 内部开一个"C1-waiver"子类**：不回填 exact，
   只写书面 waiver，并在 ledger 加 decision 记录；不要把它们误写成
   `weaker`（会掩盖"从未测过"）。

无论哪种，**不要**把 36 个 C1 case 的 "not testable" 当成 "weaker 已评估"。

---

## 9. 验证与回填注意

下一波实现后至少执行：

```text
cargo test -p wxml-backend-tsgo --lib
cargo test -p wxml-backend-tsgo --test messages_sanitize
cargo test -p wxml-backend-tsgo --test probe_health
cargo test -p wxml-backend-tsgo --test shadow_layout
cargo test -p wxml-lsp --lib backend_tsgo
cargo test -p wxml-lsp --test native_parity
jq empty docs/plans/tsgo-test-ledger.json
cargo fmt --all -- --check
cargo clippy -p wxml-lsp -p wxml-backend-tsgo --all-targets -- -D warnings
git diff --check
```

回填时额外检查：

- `counts.statuses` 在 `51abc13` 是 70/233，与 case 级一致；但
  `counts.migration_evidence.remaining_planned_without_evidence` 仍是 106，
  case 级无证据 planned 是 142。必须重新 `jq` 计数，不要复制本文件或
  wave-9/wave-10 的数字。
- `crates/wxml-lsp/tests/native_parity/ledger.rs` 的
  `GREEN_CASES`/`PLANNED_CASES`/`WEAKER_CASES` 常量必须同步；
  `BLOCKED_CASES` 当前是 22，不要因为 C 类 waiver 改它，除非 decision 记录
  明确要求。
- 每个 `migration_evidence.rust_tests[].path/line/test` 在回填时真实存在。
- 5 个 messages + 5 个 probes 的测试名要带 case id（如
  `case_messages_0005_helper_names_are_removed`），便于 validator 路由；
  `target_rust_test` 仍保留 ledger 里的 partition 路径，不需要改。

---

## 10. 风险与未决

1. **probe health 无生产调用者**：reviewer 可能要求先接线。建议把"模块 + 测试"
   与"接线 + 状态矩阵"拆成两个 PR；接线 PR 必须先跑现有 hover/diagnostics
   回归，确认不会把已 green 的 `Supported` 降级。
2. **消息清洗与 contract §9.3 冲突**：不改 contract 就实现，会在评审中被指
   "违反 native 合同"；改 contract 需要 owner 签字。两条路都必须显式记录。
3. **C/D 重叠 36 个**：如果 D owner 和 C owner 并行处理，会重复登记或互相覆盖；
   必须先合并清单。
4. **`TS-ANALYSIS-0243` 需要产品决策**：native 目前没有 `jsCheck` 配置，
   硬编码 `Inherit`。加配置会改变默认行为和 capabilities 文案，不能当成
   "只是测试"。
5. **`TS-ANALYSIS-0315` 的每请求状态**：如果评审坚持 C1 `states`，它只能
   weaker；不要为了计数把它写成 exact。
6. **本文件没有逐条运行测试**：所有"exact"判断基于源码事实与 oracle 文本；
   实现后仍需按 wave-8 的逐断言对照表复核。
7. **行号会漂移**：本文行号以 `43625ee`/`51abc13` 读取值为准；并行 wave-10b/11
   改动后需重新 `grep`。

---

## 11. 一句话给 integrator

C 类不是"59 个 checker case"：**10 个可以马上 exact，3 个条件 exact，18 个必须
真实 checker，36 个是 C1 协议遗留**。下一波做 messages + probe health +
两个 layout/status 断言（12 个），其余按 §7/§8 写明确的 blocked/waiver，不要用
fake checker 或 C1 服务器换计数。
