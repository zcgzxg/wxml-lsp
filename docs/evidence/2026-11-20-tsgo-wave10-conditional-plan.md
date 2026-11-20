# tsgo wave-10 三个条件 exact 评估（2026-11-20）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 任务书给定 HEAD：`6727059`。本次读取期间 HEAD 先后到
  `8a56f27`（`docs: sync F3 closure and native default status`）、
  `c53d9ab`（`docs(evidence): plan remaining ledger wave9`）、
  `643a4aa`（`test(tsgo): close near-exact and exact-ready parity cases`）
  和 `456a1bb`（`test(tsgo): backfill wave10 exact parity cases`）。
  9 个 exact-ready 断言与对应回填已提交；本文件按**读取完成时实际状态**记录，
  不把并行改动当成自己的结论。
- 记录日期：2026-11-20
- 范围：**只新增本文件**。未修改生产代码、测试源码、`Cargo`、delivery、
  台账 JSON（ ` docs/plans/tsgo-test-ledger.json` ）、台账 Markdown、contract、README、CI 或脚本；
  未 commit，未启动 review；未运行测试，只做只读读取与 grep。
  （§10.5 记录后续由 ledger owner 完成的台账回填结果；本节仍是规划轮的边界。）
- 本文不把 `gate_green`、`#[ignore]` 原生 gate 或并行 wave-10 回填结果当作 case green。

**结论先行：**

1. **`TS-ANALYSIS-0184` 与 `TS-REWRITE-0197` 建议 `exact`（有条件）；`TS-ANALYSIS-0161` 建议保持 `weaker`。**
   0184/0197 的 oracle 唯一语义断言是“最终 WXML 字节范围”，这是 Rust 的 shadow mapping 决定的；
   fake 只提供 arrange 输入（一条落在 target 上的 raw diagnostic），与现有 green 用 fake 提供
   type/completion 条目同类。0161 断言的 type 字符串与 JSDoc 本身就是被断言的 DTO 值，
   fake 只能回显，无法证明 checker 语义。
2. 0184/0197 升 exact 前必须先处理两个**本仓既有反向记录**，不能只改台账：
   -  ` crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:26` 的 `BLOCKED_CASES` 把
     `TS-REWRITE-0197` 标为 “exact diagnostic range requires the checker”，
     且  ` crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:471` 的
     `blocked_cases_are_documented_and_not_claimed` **硬断言 0197 在该列表里**；
   -  ` docs/plans/2026-11-19-tsgo-remaining-ledger-plan.md:611` 把
     `TS-REWRITE-0197`、`TS-MAPPING-0188/0208/0306` 归为
     “published diagnostics reverse mapping is not a frozen contract”。
   这两条的理由都比 oracle 实际断言更宽：**范围**由 Rust 决定，只有**诊断存在性**需要 checker。
3. 若不接受“映射等价”的裁决（或不愿改 `BLOCKED_CASES`），退回全 `weaker`：
   三例都可登记 §4 的测试，只是 parity 记 weaker、`green` 不变（§6 情景 A）。
4. 0161 的唯一 exact 路径是让 checker 在**默认运行**里可失败：内置 pinned 引擎，
   或 integrator 书面接受“opt-in native gate + 默认运行输入接线”的组合证据（§7.1）。
5. 三例都不应该为了计数写 green；0161 尤其不能把 fake 回显的 `number`/`未保存的名称`
   当成 checker 证据。

---

## 1. 基线（写入时实际值）

| 来源 | green | planned | retired | exact | near_exact | weaker | assessed | 无证据 planned |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `6727059`（任务书起点，已提交） | 58 | 245 | 36 | 58 | 3 | 70 | 160 | 143 |
| wave-10 weaker 回填（中途） | 59 | 244 | 36 | 59 | 2 | 82 | 172 | 131 |
| **写入完成时工作树（含 9 个 exact-ready 回填，`456a1bb`）** | **68** | **235** | 36 | 68 | 2 | 82 | **181** | **122** |

写入完成时 `counts.migration_evidence` 含：wave-10 weaker 回填（weaker 70→82、assessed 160→172）、
`TS-REWRITE-0207` 的 near-exact→exact、以及 `643a4aa`/`456a1bb` 的 9 个 exact-ready case
（green 59→68、planned 244→235、assessed 172→181）。
`crates/wxml-lsp/tests/native_parity/ledger.rs` 的 `GREEN_CASES=68`、`PLANNED_CASES=235`、
`WEAKER_CASES=82`、`NEAR_EXACT_CASES=2` 与 JSON 一致；回填前必须再读一次。

**这三个 case 在写入时仍是 `planned` / `target_rust_test_status=proposed` / 无 `migration_evidence`：**

| case | source | 断言数 | target_rust_test |
| --- | --- | ---: | --- |
| `TS-ANALYSIS-0161` | `backends/typescript/test/analysis.test.ts:161` | 4 | `crates/wxml-lsp/tests/native_parity/hover_docs.rs::case_analysis_0161_analyze_uses_the_unsaved_file_text_from_the_request_no_disk_read` |
| `TS-ANALYSIS-0184` | `backends/typescript/test/analysis.test.ts:184` | 3 | `crates/wxml-lsp/tests/native_parity/hover_docs.rs::case_analysis_0184_target_selects_the_reported_token` |
| `TS-REWRITE-0197` | `backends/typescript/test/rewrite.test.ts:197` | 3 | `crates/wxml-lsp/tests/native_parity/shadow_rewrite.rs::case_rewrite_0197_a_member_typo_keeps_the_exact_wxml_byte_range` |

`target_rust_test` 只是路由标签：`native_parity` 的 16 个分区仍是 skeleton，
` crates/wxml-lsp/tests/native_parity.rs:174` 的
`partition_skeletons_match_manifest_and_fail_closed` 断言除 `symbols_edits` 外
`MIGRATED_CASES` 必须为空。**不要把新测试写进 `crates/wxml-lsp/tests/native_parity/*.rs`**，
否则会连带改 skeleton 清单。证据测试仍放在 backend/adapter 测试文件里（wave-8/9 的既有做法）。

---

## 2. F2 契约：fake session 精确断言 DTO 映射是否允许

### 2.1 契约原文的约束

| 位置 | 约束 | 对三例的含义 |
| --- | --- | --- |
| `docs/reference/tsgo-test-contract.md` §3 | 每个 case 至少保留 “RED/GREEN 原始证据、reviewer、原生身份（engine version/gitHead/AST version/artifact hash）” | 字面要求 per-case 原生身份；fake 测试无 engine 身份 |
| 同 §4 | 禁止归一化“诊断 code、severity、range、message”和“hover 的类型字符串、文档、JSDoc” | 0161 的 type/doc、0184/0197 的 range 必须逐字对照，不能用泛化值替代 |
| 同 §5 | 缺引擎/资产 hash 不符必须失败，不得 skip/return | fake 测试不 skip，但也不证明引擎在 |
| 同 §8（G5） | 不得有“真实引擎缺失导致的 skip” | 仅靠 `#[ignore]` 原生 gate 不能把 case 记 green |
| `native-contract` §9.3 | “shadow 诊断按同一映射规则回原 WXML/脚本” | 诊断反向映射有方向性要求，但没有冻结“crop 到 target”的细则 |

`§3` 的 per-case 原生身份在实际台账里并未逐 case 记录（68 个 green 只记
`implementation_head` + 套件级原生命令）。所以本文**不把 §3 字面要求当作唯一否决理由**，
只作为“exact 证据强度不足”的辅助证据。

### 2.2 现有 green 的先例（fake 被允许的边界）

| case | 证据形态 | 为什么可以 exact |
| --- | --- | --- |
| `TS-COMPLETION-0220` | fake 返回 >256 条 completion | oracle 只断言“上限 256 + truncated”，由 adapter 计算 |
| `TS-COMPONENT-0206` | Rust 组件绑定检查产出的诊断 | 声明 `type: Number` 与字面量的比较完全由 Rust 决定（测试还断言 `calls.is_empty()`） |
| `TS-COMPONENT-0147` | fake session 提供表达式类型 | 被断言的是 Rust 的 mismatch 判定/range/message；fake 只提供 type |
| `TS-MAPPING-0126` | 手搓 `MountRecord` + 合成 span | oracle 本身就是 mapping 单测；输入 span 只是 arrange |
| `TS-REWRITE-0136` | 纯 rewrite 文本 | 备注明确写 “the TS `result.ok` assertion is implied by the outputs” |
| `TS-ANALYSIS-0139` | fake 故意返回 `string`，adapter 用声明 `Number` 覆盖 | 被断言的事实由 adapter 的 declaration overlay 决定 |
| `TS-ANALYSIS-0174` | fake + `opened_inputs` 记账 | 被断言的是“不读 project root”，由输入清单/根路径决定 |

**共同点：fake 只当 arrange 输入，被断言的值由 Rust 代码决定；oracle 里没有任何
“checker 必须产出 X”的值断言。**

反例（本轮 0161）：把 fake 配成 `type=number` / `doc=未保存的名称`，断言的**值**就是 fake
自己的答案；Rust 侧没有决策步骤（data 字段不做声明覆盖），所以即使 Rust 的输入接线坏了，
只要 fake 不变，测试仍会通过。

### 2.3 判定规则（本文采用）

一个 fake-session 测试可以算 exact，当且仅当：

1. oracle 的**值断言**落在 Rust 决定的字段上；fake 只提供 arrange 输入（type、completion 条目、
   raw diagnostic、mount segment）；
2. 测试对 Rust 决策有**正负对照**：Rust 逻辑退化时测试会失败，而不是只回显 fake；
3. oracle 的 arrange 前提（例如“checker 会报一条 2339”）被显式写进 evidence note，
   并说明由哪个默认运行或原生 gate 覆盖。

不满足第 1 条 → 最多 `weaker`。满足第 1、2 条但第 3 条未写清 → 不能 exact。

### 2.4 套用到三例

| case | 被断言的值 | Rust 决定？ | 结论 |
| --- | --- | --- | --- |
| 0161 `typeOnly/documentation` | fake 返回的 `number`/`未保存的名称` | 否（adapter 只透传 DTO） | 不允许 exact |
| 0161 `no disk read` 半边 | `opened_inputs` 的文本/根路径 | 是 | 可写 weaker 证据 |
| 0184/0197 `range = 134..138` | 由 mount target + `map_span_to_source` 算出的 WXML 字节 | 是 | exact-eligible |
| 0184/0197 的“诊断存在” | 隐含在 `diagnostics[0]` 里；不是被断言的字节值 | — | 写进 note，由 G-P5-E2E-07 覆盖 |

结论：**“用 fake session 精确断言 DTO 映射”被 F2 允许，但仅在 fake 提供 arrange 输入、
Rust 决定被断言值时。** 0184/0197 属于这一类；0161 不属于。

---

## 3. 逐 case 分析

### 3.1 `TS-ANALYSIS-0161` — analyze uses the unsaved file text from the request (no disk read)

**oracle（` analysis.test.ts:161`）**

```ts
const request = pageBasicRequest();
const script = request.files.find((file) => file.fileId === request.scriptFileId);
assert.ok(script);                                     // fixture 前提
script.text = script.text.replace(
  "/** 用户显示名称 */\n      name: 'Ada',",
  "/** 未保存的名称 */\n      name: 42,");
const result = analyze(request, analysisConfig);
assert.equal(result.ok, true);
const name = result.response.fragments.find((entry) => entry.fragmentId === "user.name");
assert.equal(name?.hover?.typeOnly, "number");
assert.equal(name?.hover?.documentation, "未保存的名称");
```

**当前 Rust 行为**

| 环节 | 现状 |
| --- | --- |
| WXML 请求的 owner 文本 |  ` crates/wxml-lsp/src/backend_tsgo/input.rs:850` 起：owner 文件文本用 `derived`（把 shadow 注入 `PreparedView.files` 的 `prepared.text`），非 owner 用 `prepared.text` |
| 磁盘 | adapter 自身不读文件；`ProjectHost` 统一 overlay-first，但 fake 测试里没有 fs 观察到 |
| hover 类型/文档 |  ` crates/wxml-lsp/src/backend_tsgo/hover.rs:59` 起：`session.hover_evidence` 的 `type_info.text`/`documentation` 直接组成 Markdown；adapter 不对 data 字段做类型推导（只有 `apply_declared_property_type` 覆盖 component `properties`） |
| 已有相关测试 | `hover_composes_type_documentation_and_jsdoc`（ ` crates/wxml-lsp/src/backend_tsgo/tests.rs:1125` ，固定 `string` + 固定 `COMMENT`）、`a_non_owner_overlay_content_change_updates_the_same_session`（overlay 文本进 session）、`case_analysis_0174_...`（不读 project root）、`optional_real_engine_hover_smoke`（opt-in） |

**缺的断言**

- hover `typeOnly == number`、`documentation == 未保存的名称`（checker 产物，fake 无法证明）；
- “unsaved 文本而不是磁盘副本进入 session”的输入断言（可补，但只覆盖 half）。

**依赖**

- type/doc 断言依赖**真实原生引擎**；
- 输入断言依赖 `FakeViews` + `opened_inputs`，不依赖引擎。

**结论：`weaker`。** 与 `TS-ANALYSIS-0139` 不同，0161 没有 adapter 侧覆盖逻辑可以“打败” fake；
type/doc 是 oracle 的**值**，不是 arrange 输入。

### 3.2 `TS-ANALYSIS-0184` — target selects the reported token

**oracle（` analysis.test.ts:184`）**

```ts
const request = pageBasicRequest();
const fragment = request.fragments.find((f) => f.fragmentId === "user.nmae");
assert.ok(fragment);                                   // fixture 前提
fragment.target = "nmae";
const result = analyze(request, analysisConfig);
assert.equal(result.ok, true);
const typo = result.response.fragments.find((f) => f.fragmentId === "user.nmae");
assert.deepEqual(typo?.diagnostics[0]?.range, { start: 134, end: 138 });
```

**关键事实**

- `page-basic` 的 `user.nmae` fragment 在 `inspectRequest` 里**本来就带 `target: "nmae"`**
  （` backends/typescript/src/run.ts:185-204` 从 `queries[].target` 取值）。
- TS 的默认 target 也是最右 value identifier（` backends/typescript/src/expression.ts:177`
  `rightmostIdentifierToken`），对 `user.nmae` 同样是 `nmae`。
- 所以 `fragment.target = "nmae"` 在 TS 和 Rust 两边都是**无变化的重复赋值**；
  这个 case 的可观察内容与 `TS-ANALYSIS-0033` 的 typo range 断言一致，只是多了一次显式赋值。
  `assert.ok(fragment)` 是 fixture 前提，不是语义断言。

**当前 Rust 行为**

| 环节 | 现状 |
| --- | --- |
| target 透传 |  ` crates/wxml-lsp/src/backend_tsgo/input.rs:1287` `fragment_spec()` 复制 `fragment.target` |
| target 解析 |  ` crates/wxml-backend-tsgo/src/shadow/rewrite.rs:335` `resolve_target()`：显式 target 取 `all` 中同名 identifier 的第一个；默认取 `values.last()`（`user.nmae` 为 `nmae`） |
| mount target | `MountRecord.target_start_utf16/target_length_utf16`（`RewriteOptions` 结果） |
| 映射 |  ` crates/wxml-backend-tsgo/src/shadow/mapping.rs:247` `map_span_to_source()`：span 在单 segment 内 → 精确；跨 synthetic gap →  ` crates/wxml-backend-tsgo/src/shadow/mapping.rs:341` 起的 target crop。`user.nmae` 的两个 source piece 会合并成一个 segment，**没有内部 gap**，所以 target crop 在本 fixture 不可达 |
| adapter 发布 |  ` crates/wxml-lsp/src/backend_tsgo/diagnostics.rs:100-145`：位置→derived offset→`map_native_range`→WXML range；按 `(code,start,end,message)` 去重；crop 时标 `Partial` |
| 已有测试 | `diagnostics_map_semantic_and_syntactic_ranges`（fake `user.name` → 整个表达式 range）、`case_rewrite_0309_rewritten_mounts_map_target_spans_and_trim_synthetic_prefixes`（backend 手搓 mount 验证 crop 到 target，但 origin=200、不是 page-basic；那里有内部 synthetic gap） |

**缺的断言**

- page-basic 的绝对 `134..138`（`nmae` 在 WXML 的字节区间）；
- 整个表达式 span 的对照断言（`129..138`，证明 mapping 不会被无条件强制成 target token）；
  **注意**：`user.nmae` 没有内部 synthetic gap，target crop 分支在本 fixture 不可达。

**依赖**

- 映射/目标选择：Rust（可测）；
- 诊断存在：arrange 前提（checker 侧，写进 note，由 G-P5-E2E-07 覆盖）。

**结论：推荐 `exact`（条件见 §7.2）。** 唯一语义值是 range，由 Rust 计算；
`result.ok` 由 adapter 返回 `Ok(DiagnosticsResult)` 等价（`TS-REWRITE-0136` 先例：
“`result.ok` is implied by the outputs”）。

### 3.3 `TS-REWRITE-0197` — a member typo keeps the exact WXML byte range

**oracle（` rewrite.test.ts:197`）**

```ts
const request = pageRequest();                          // page-basic，3 个 fragment
assert.ok(request.fragments.find((f) => f.fragmentId === "user.nmae"));
const result = run(request);
assert.equal(result.ok, true);
const diagnostic = result.response.fragments
  .find((f) => f.fragmentId === "user.nmae")?.diagnostics[0];
assert.deepEqual(diagnostic?.range, { start: 134, end: 138 });
```

与 0184 的差别只有两点：没有显式 `fragment.target = "nmae"`；走 `rewrite.test.ts` 的 `run`
（内部仍是 `analyze`）。断言集合完全相同。

**当前 Rust 行为**

同 0184 的映射链。backend 侧 `map_span_to_source` 已覆盖“单 segment 精确”和
“跨 internal synthetic gap crop 到 target”（`case_rewrite_0309_rewritten_mounts_map_target_spans_and_trim_synthetic_prefixes`），
但没有 page-basic 的绝对偏移。`user.nmae` 的 source pieces 会合并成一个 segment，
所以本 case 只走“单 segment 精确”分支，不走 crop。

**缺的断言**

- page-basic mount 上 `nmae` 的绝对 `134..138`；
- 诊断存在性作为 arrange 前提写进 note。

**本仓已存在的反向记录（必须先处理）**

1.  ` crates/wxml-backend-tsgo/tests/shadow_rewrite.rs:26` `BLOCKED_CASES` 含
   `TS-REWRITE-0197`，理由 “exact diagnostic range requires the checker”；
2. 同文件 `:470` 的 `blocked_cases_are_documented_and_not_claimed` 硬断言该条目存在；
3.  ` docs/plans/2026-11-19-tsgo-remaining-ledger-plan.md:611` 把 0197 归为
   “published diagnostics reverse mapping is not a frozen contract”。

**对这三条的处理建议**

- 第 1 条的理由应改写为“checker-emitted diagnostic existence”，而不是 “exact diagnostic range”；
  范围本身由 Rust 决定，`MAPPING-0126`（合成 span → 精确 range）已是 exact 先例。
- 第 2 条测试只需把断言改成“0197 仍在 blocked 列表且原因包含 checker-emitted”，
  或（若升 exact）删除 0197 条目并把 `case_` 测试改为已声称。**不能只改 JSON。**
- 第 3 条的“not a frozen contract”需要 integrator 明确：是把 `map_span_to_source` 的
  crop-to-target 补进 native contract，还是用书面 waiver 承认 MAPPING-0126/0309 的既有语义。
  在该裁决前，0197 应保持 `weaker`；裁决后 exact。

**结论：推荐 `exact`（条件：先完成上述记录改写 + §7.2 裁决）；否则 `weaker`。**

---

## 4. 推荐测试落点

所有落点都在 backend/adapter 测试文件，**不进** `crates/wxml-lsp/tests/native_parity/*.rs`，
不改 `Cargo`、fixture 树或台账。

### 4.1 总表

| case | 文件 | 建议测试名 | 覆盖 | 可支撑级别 |
| --- | --- | --- | --- | --- |
| 0161 |  ` crates/wxml-lsp/src/backend_tsgo/tests.rs` | `case_analysis_0161_unsaved_script_text_and_no_disk_read_reach_the_session` | 输入接线 + 不存在 project root + 只提交内存文件 | weaker |
| 0184 |  ` crates/wxml-lsp/src/backend_tsgo/tests.rs` | `case_analysis_0184_page_basic_typo_maps_to_wxml_134_138` | adapter 全链路 + `134..138` + 宽 span 对照（`129..138`） | exact（§7.2 裁决后） |
| 0197 |  ` crates/wxml-backend-tsgo/tests/shadow_rewrite.rs` | `rewrite_0197_member_typo_stays_at_wxml_byte_134_138` | 真 shadow build + `map_span_to_source` 的绝对偏移 | exact（§7.2 裁决后） |

> 0197 在裁决前不用 `case_` 前缀，避免与 `BLOCKED_CASES` 的“不声称已迁移”语义冲突；
> 裁决升 exact 后再改成 `case_rewrite_0197_...` 并同步 `BLOCKED_CASES`。

### 4.2 0161 测试骨架（adapter，weaker）

```rust
#[test]
fn case_analysis_0161_unsaved_script_text_and_no_disk_read_reach_the_session() {
    // page-basic 的 unsaved 版本：name: 42 + 未保存的 JSDoc。
    const UNSAVED: &str = "Page({\n  data: {\n    /** 页面标题 */\n    title: '首页',\n    /** 当前用户 */\n    user: {\n      /** 未保存的名称 */\n      name: 42,\n      /** 用户年龄 */\n      age: 36,\n    },\n  },\n});\n";
    const PROJECT_ROOT: &str = "/nonexistent/wxml-analysis-0161-project-root";
    assert!(!std::path::Path::new(PROJECT_ROOT).exists());
    let mut view = prepared_view(WXML, UNSAVED, "user.name"); // WXML 同 §5
    view.project_root = PathBuf::from(PROJECT_ROOT);
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    // 注意：`offset_of(WXML, "name")` 会命中 `class="name"`，必须从表达式定位。
    let name = offset_of(WXML, "user.name") + u32::try_from("user.".len()).expect("prefix");
    adapter
        .hover(HoverRequest { context: context(&adapter, WXML), offset: name })
        .expect("hover");
    let log = log.lock().expect("log");
    let input = log.opened_inputs.first().expect("opened input");
    assert_eq!(input.root().as_str(), PROJECT_ROOT);
    let text = input.file(SCRIPT_PATH).expect("owner script").text();
    assert!(text.contains("未保存的名称"), "{text}");
    assert!(text.contains("name: 42"), "{text}");
    assert_eq!(input.files().len(), 1, "only the in-memory snapshot is submitted");
}
```

**不要**把这个测试写成 exact：fake 的 `type_outcome` 只是回显，`result.contents` 的
`number` 不是 checker 证据。可以保留 `result.status`/`quality` 作为 DTO 合成证据，
但 note 必须写明 type/doc 来自 fake。

### 4.3 0184 测试骨架（adapter，page-basic 全量 fragment）

> 本节是规划稿；实施时的实际常量/helper 名与最终断言见 §10.1/§10.2。

```
title      = 35..40
user.name  = 85..94
user.nmae  = 129..138   (nmae = 134..138)
```

`fragment()` helper 用 `wxml.find(expr)`，对 `title` 会先命中 `class="title"`，
**必须显式构造 3 个 `CheckFragment` 的 `wxml_range`**（需要在测试文件内新增一个小 helper
`check_fragment(id, expr, start, end)`，`target: Some(..)` 时再覆盖）：

```rust
#[test]
fn case_analysis_0184_page_basic_typo_maps_to_wxml_134_138() {
    let fragments = vec![
        check_fragment("title", "title", 35, 40),
        check_fragment("user.name", "user.name", 85, 94),
        check_fragment("user.nmae", "user.nmae", 129, 138), // target: Some("nmae")
    ];
    let view = prepared_view_with_fragments(
        WXML,
        vec![prepared_file(SCRIPT_PATH, PAGE_BASIC_SCRIPT)], // 不是文件顶部的最小 SCRIPT 常量
        fragments,
    );
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "nmae",
            code: 2339,
            category: 1,
            message: "Property 'nmae' does not exist.",
            file_name: None,
        }]),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .diagnostics(DiagnosticsRequest { context: context(&adapter, WXML) })
        .expect("diagnostics");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].range.start(), 134);
    assert_eq!(result.diagnostics[0].range.end(), 138);
}
```

再加一个**对照断言**（同文件第二个测试）：needle 换成整个
`"__wxml_root.user.nmae"`。`user.nmae` 没有内部 synthetic gap，leading
`__wxml_root.` 是精确投影，所以真实结果是整个表达式 `129..138` 且 `Supported`，
**不是** crop 到 `134..138`/`Partial`。这一条是 Rust 侧对照，不是 oracle 原文断言；
它证明 mapping 不会被无条件强制成 target token，满足 §2.3 第 2 条（正负对照）。
不要为了凑“crop 判别”伪造 `134..138`/`Partial`——那需要手搓一个带内部 gap 的 mount，
与本 fixture 的真实 shadow 不符。

### 4.4 0197 测试骨架（backend，无 fake session）

> 本节是规划稿；实施时的实际测试与宽 span 对照见 §10.1/§10.2。

```rust
#[test]
fn rewrite_0197_member_typo_stays_at_wxml_byte_134_138() {
    let script = source_file(PAGE_BASIC_SCRIPT); // page-basic 的 index.ts 原文
    let fragments = [
        FragmentSpec::new("title", "title", SourceRange::new(35, 40)),
        FragmentSpec::new("user.name", "user.name", SourceRange::new(85, 94)),
        // oracle 没有显式 target：默认取最右 value identifier，即 `nmae`。
        FragmentSpec::new("user.nmae", "user.nmae", SourceRange::new(129, 138)),
    ];
    let built = build_in_place_shadow(&ShadowRequest {
        script: &script, root: RootKind::Page, fragments: &fragments,
        js_check: JsCheckPolicy::Inherit, check_js: false,
    }).expect("layout");
    let mount = built.view.mounts.iter().find(|m| m.fragment_id == "user.nmae").expect("mount");
    assert_eq!(mount.target_length_utf16, 4, "default target must be `nmae`");
    assert_eq!(
        utf16_slice(&mount.text, mount.target_start_utf16 - mount.start_utf16, 4),
        "nmae"
    );
    let tight = map_span_to_source(
        mount, Utf16Offset::new(mount.target_start_utf16), mount.target_length_utf16,
    ).expect("mapped");
    assert_eq!(tight.range, Some(SourceRange::new(134, 138)));
    assert!(!tight.trimmed_synthetic_prefix);

    // 对照（非 oracle 断言）：`user.nmae` 没有内部 synthetic gap，leading
    // `__wxml_root.` 是精确投影，所以整个 mount span 会映射到 `129..138`，
    // 不是 crop 到 `134..138`。
    let whole = map_span_to_source(
        mount, Utf16Offset::new(mount.start_utf16), utf16_len(&mount.text),
    ).expect("whole mount");
    assert_eq!(whole.range, Some(SourceRange::new(129, 138)));
    assert!(!whole.trimmed_synthetic_prefix);
}
```

这个测试完全不依赖 fake session；它证明“page-basic 的 mount 上，target span 落到 WXML
`134..138`；整个表达式 span 落到 `129..138`（leading prefix 精确投影，不是 crop）”。
它**不能**证明 checker 会报诊断——这是 arrange 前提，必须写进 evidence note。

### 4.5 不要做

- 不要在 `crates/wxml-lsp/tests/native_parity/hover_docs.rs` / `shadow_rewrite.rs`
  新增 `case_*` 并登记 `MIGRATED_CASES`（会触发
  `partition_skeletons_match_manifest_and_fail_closed`）。
- 不要新增 `crates/wxml-lsp/tests/fixtures/**` 文件去放 page-basic：
  `tests/fixtures` 与 `crates/wxml-lsp/tests/fixtures` 的 v1 hash 已冻结
  （ ` docs/reference/tsgo-test-contract.md:48-49` ），加文件必须同时改台账与 hash。
  page-basic 的 WXML 在 TS 侧是 `scenarios.ts` 常量，不是 fixture 文件；Rust 侧用
  测试内 `const` 即可。
- 不要改 `backends/typescript/test/**` 或 `samples/**`（oracle/manifest 冻结）。
- 不要为 0184/0197 伪造 `code`/`message` 并声称 checker 语义；code 只作为 adapter 透传证据。

---

## 5. Fixture 与命令

### 5.1 page-basic 常量（来自 ` backends/typescript/src/scenarios.ts:11` 与 `samples/page-basic/index.ts`）

```text
<view class="page">
  <text class="title">{{ title }}</text>
  <text class="name">{{ user.name }}</text>
  <text class="typo">{{ user.nmae }}</text>
</view>
```

| 表达式 | WXML 字节 |
| --- | --- |
| `title` | 35..40 |
| `user.name` | 85..94 |
| `user.nmae` / `nmae` | 129..138 / 134..138 |

脚本用 `samples/page-basic/index.ts` 原文（含 `页面标题`/`用户显示名称` JSDoc）；
0161 用 §4.2 的 unsaved 版本。

### 5.2 命令（实现后，按需最小化）

```text
cargo test -p wxml-lsp --lib backend_tsgo
cargo test -p wxml-backend-tsgo --test shadow_rewrite
cargo fmt --all -- --check
cargo clippy -p wxml-lsp -p wxml-backend-tsgo --all-targets -- -D warnings
```

回填后（ledger owner）：

```text
jq empty docs/plans/tsgo-test-ledger.json
cargo test -p wxml-lsp --test native_parity
git diff --check
```

本文没有运行上述命令，也没有运行全量测试。

---

## 6. 若回填，计数影响

以写入完成时基线 `68 green / 235 planned / 36 retired`（exact 68、near 2、weaker 82、
assessed 181、无证据 122）为起点：

| 情景 | green | planned | exact | near | weaker | assessed | 无证据 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| **B（推荐）：0184/0197 exact + 0161 weaker** | **70** | **233** | **70** | 2 | **83** | **184** | **119** |
| A（保守）：三例都 weaker | 68 | 235 | 68 | 2 | **85** | **184** | **119** |

增量固定：三例都从“无证据”变成“有证据”，所以 `assessed +3`、`无证据 −3`；
B 另加 `GREEN_CASES +2`、`PLANNED_CASES −2`、`exact +2`、`WEAKER_CASES +1`；
A 只加 `WEAKER_CASES +3`。

逐 case 字段（B 情景）：

- 0184/0197：`status=green`、`target_rust_test_status=migrated_green`、
  `migration_evidence.parity=exact`、`green_eligible=true`、
  `status_recommendation=green`、`status_applied=green`；
- 0161：`status=planned`、`target_rust_test_status=proposed`、
  `migration_evidence.parity=weaker`、`green_eligible=false`、
  `status_recommendation=planned`、`status_applied=planned`。

注意：`ledger.rs` 的常量、`counts.statuses`、`counts.migration_evidence`、
`docs/reference/tsgo-test-contract.md` §1、`docs/plans/2026-11-17-tsgo-test-ledger.md`
必须由 ledger 单写者在**回填时重新读取**后一起改；并行 WIP 正在改这些文件。

---

## 7. 升 exact 的条件与真实证据

### 7.1 0161

必须让“checker 根据 unsaved 文本推出 number/未保存的名称”在**默认运行**里可失败。当前只有两条现实路径：

1. **默认内置原生引擎**：把 pinned `7.1.0-beta` 资产随测试/CI 提供，测试缺引擎即失败
   （符合 contract §5/§8）。这属于 delivery/CI 变更，不是测试-only。
2. **opt-in 原生 gate + 书面 waiver**：新增 `#[ignore]` 测试（建议放
   ` crates/wxml-lsp/tests/native_parity/native_engine_real.rs`），用 page-basic 的 unsaved
   脚本断言 hover type `number`、JSDoc `未保存的名称`、`name` 的 WXML range `90..94`；
   若要顺带覆盖 typo，则是 `134..138`。命令：

   ```text
   TSGO_BIN=<pinned>/lib/tsc WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
     cargo test -p wxml-lsp --test native_parity -- --ignored native_ac001b_unsaved_page_script
   ```

   但该 gate 绿只等于 `gate_green`。要转 case green，integrator 必须书面接受
   “gate 覆盖 checker 半边 + 默认运行覆盖输入半边”，并同步
   `ignored_rust_tests`（当前 `IGNORED_GATE_COUNT` 是冻结值，新增 gate 会改计数）。

在没有 1 或 2 的情况下，0161 保持 `weaker`；§4.2 的测试只登记输入半边。

### 7.2 0184 / 0197

升 exact 需要三件事，缺一不可：

1. **evidence note 写清 arrange 前提**：诊断由 fake 提供；本 case 的 evidence 只声称
   “映射结果正确”，不声称已证明 checker 会产出 `2339`。该存在性由 `G-P5-E2E-07`
   （`native_ac002_field_typo_diagnostic_and_fix_clear`，
   ` crates/wxml-lsp/tests/native_parity/native_engine_real.rs:1148` ）覆盖，
   并注明该 gate 是 opt-in、不是默认 CI。
2. **contract/决策记录**：把 `BLOCKED_CASES`（`shadow_rewrite.rs:26`）的理由从
   “exact diagnostic range requires the checker” 改为 “checker-emitted diagnostic existence”，
   并同步 `blocked_cases_are_documented_and_not_claimed`（`shadow_rewrite.rs:471`）；
   对 0197 也可以选择删除该条目并改名为 `case_rewrite_0197_...`。
   同时处理 ` docs/plans/2026-11-19-tsgo-remaining-ledger-plan.md:611` 的
   “not a frozen contract” 结论（补 contract 或书面 waiver）。
3. **映射测试**：§4.3/§4.4 的 page-basic 绝对偏移 + 宽 span 对照（不是 crop；见 §4.3 说明）。

若 integrator 要求“默认运行必须证明诊断存在”，则 0184/0197 只能走 §7.1 的路径 1（内置引擎），
否则保持 `weaker`（§6 情景 A）。

可选的加强（不改变结论）：在 `native_engine_real.rs` 增加 page-basic 版本的 AC-002，
断言 `nmae` 的 WXML 字节 `134..138`（而不是一行模板的 `character+4`），
作为 native gate 的补充证据。

---

## 8. 风险

| 级别 | 风险 | 缓解 |
| --- | --- | --- |
| 高 | ledger/contract/ledger.rs 正被并行写入；本次读取期间已从 58/245/160 漂到 68/235/181 | 回填前重新读 JSON、常量与 contract，不复制本文数字 |
| 高 | 0197 已在 `BLOCKED_CASES` 且有硬断言测试；不改就升 exact 会自相矛盾 | 先按 §7.2 第 2 条改记录，再回填；不要只改 JSON |
| 高 | 评审可能不接受“映射等价”，要求默认运行证明诊断存在 | 退回 §6 情景 A；不强行 green |
| 中 | fake 诊断会被误读成“checker 证据” | note 逐条写清“range 由 Rust 映射决定；诊断存在性是 arrange 前提” |
| 中 | page-basic 的 `title` 与 `class="title"` 同名，`fragment()` helper 会取错范围 | 显式构造 range（§4.3） |
| 中 | 把 `user.nmae` 误当成有内部 synthetic gap，断言 crop/`Partial` | 实测该 mount 只有一个 source segment；宽 span 断言 `129..138`/`Supported`，crop 只在 `case_rewrite_0309` 的手搓 mount 上测 |
| 中 | 新增 `native_parity/*.rs` case 会触发 skeleton 清单断言 | 证据只放 backend/adapter 测试 |
| 中 | 新增 fixture 文件会改冻结 hash | 一律用测试内 `const`，不建 fixture 文件 |
| 低 | `fragment_spec`/`resolve_target` 后续改成“取最后一个同名 identifier”会改变默认 target | 测试同时断言 `user.name` 与 `user.nmae` 两个 mount |
| 低 | 行号漂移（本文件的行号是写入时值） | 回填用函数名/测试名定位，不用行号 |

---

## 9. 本文不声称

1. 不声称三例已 `green`、`reviewed` 或 `closed`；0184/0197 的 exact 仍需 §7.2 的记录改写。
2. 不声称 fake session 能证明 checker 语义；也不声称 `gate_green` 等于 case green。
3. 不声称 0161 可以靠 fake 升 exact；它的 type/doc 断言必须等真实引擎路径。
4. 不声称 `TS-ANALYSIS-0033`、`TS-REWRITE-0309` 等相关 case 的状态发生变化。
5. 不声称本文的绝对计数在回填时仍成立；只固定增量（assessed +3、无证据 −3）。

---

## 10. 实施结果（2026-11-20，未 commit）

### 10.1 实际改动

| 文件 | 改动 |
| --- | --- |
|  ` crates/wxml-lsp/src/backend_tsgo/tests.rs` | 新增 `PAGE_BASIC_WXML` / `PAGE_BASIC_SCRIPT` / `PAGE_BASIC_UNSAVED_SCRIPT` 常量与 `page_basic_fragment()` helper；新增 `case_analysis_0161_...`、`case_analysis_0184_page_basic_typo_maps_to_wxml_134_138`、`case_analysis_0184_wider_spans_map_to_the_expression_range` |
|  ` crates/wxml-backend-tsgo/tests/shadow_rewrite.rs` | 新增 `PAGE_BASIC_SCRIPT` 与 `case_rewrite_0197_a_member_typo_keeps_the_exact_wxml_byte_range`；`BLOCKED_CASES` 移除 0197；`blocked_cases_are_documented_and_not_claimed` 改为断言 0197 不在列表、0349 仍在 |
|  ` docs/plans/2026-11-19-tsgo-remaining-ledger-plan.md` | 在原表下方追加一行限定：0197 的 range 已由真实 shadow mapping 覆盖，剩余限定只适用于 `MAPPING-0188/0208/0306` |
| 本文件 | 新增本节；修正 §4.3/§4.4 关于“crop”的旧假设 |

### 10.2 与 §4 计划的差异（重要）

计划里写的 “mount span crop 判别 → `134..138` + `Partial`” **不成立**，实施时按真实行为修正：

- `user.nmae` 的 rewrite pieces 是 synthetic `__wxml_root.` + source `user` + source `.nmae`，
  `build_segments` 会把两个相邻 source piece 合并成一个 segment；
- 因此该 mount 没有**内部** synthetic gap，leading prefix 只是 leading；
- 整个 mount span 走 “leading synthetic skip → 单 segment 精确” 分支，映射到
  `129..138`（整个 `user.nmae`），状态 `Supported`，**不是** crop 到 `134..138`/`Partial`；
- `map_span_to_source` 的 target crop 只会在跨内部 gap 时触发（如
  `case_rewrite_0309` 的 `title + user.name` 手搓 mount）。

所以实施为：

- oracle 断言（tight `nmae` span）：`134..138`，`Supported`；
- Rust 侧对照（whole mount span）：`129..138`，`Supported`，并断言
  `!trimmed_synthetic_prefix`，证明 mapping 不会被无条件强制成 target token。

没有为了凑 “crop” 伪造 `134..138`/`Partial`；那需要手搓一个带内部 gap 的 mount，
与本 fixture 的真实 shadow 不符。

### 10.3 级别与计数建议

| case | 实施级别 | 证据 |
| --- | --- | --- |
| `TS-ANALYSIS-0184` | **exact** | `case_analysis_0184_page_basic_typo_maps_to_wxml_134_138`（`134..138`、`Supported`）+ `case_analysis_0184_wider_spans_map_to_the_expression_range`（`129..138` 对照） |
| `TS-REWRITE-0197` | **exact** | `case_rewrite_0197_a_member_typo_keeps_the_exact_wxml_byte_range`（真 shadow build + `map_span_to_source`，`134..138` + 宽 span 对照） |
| `TS-ANALYSIS-0161` | **weaker** | `case_analysis_0161_unsaved_script_text_reaches_the_session_without_a_disk_read`（输入接线；type/doc 仍是 checker 产物） |

以实现时实际 JSON 为基线（`456a1bb` 的 wave-10 exact 回填：68 green / 235 planned /
181 assessed / 122 无证据；本次写文档期间并行 weaker 回填又把工作树推进到
assessed 194 / weaker 95 / 无证据 109，`green`/`planned` 不变），本批增量固定为：

- `+2 exact / +1 weaker / assessed +3 / 无证据 −3`；
- 实际回填（相对 wave-10b 后的 68/235/194/95/109）为 **70 green / 233 planned、
  exact 70、weaker 96、assessed 197、无证据 106**；`near_exact` 2、`structural` 7、
  `blocked` 22、`retired` 36 不变（见 §10.5）。

“B13” 若在回填前再落一批 exact/weaker，绝对数会整体平移；ledger owner 必须重新读取
JSON、`GREEN_CASES/PLANNED_CASES/WEAKER_CASES` 与 contract §1 后再写，不要复制本节数字。

### 10.4 验证（已执行）

```text
cargo fmt --all -- --check                      # 通过
cargo check -p wxml-lsp -p wxml-backend-tsgo --all-targets   # 通过
cargo clippy -p wxml-lsp -p wxml-backend-tsgo --all-targets -- -D warnings  # 通过
cargo test -p wxml-lsp --lib backend_tsgo::tests::case_analysis_01  # 5 passed
cargo test -p wxml-backend-tsgo --test shadow_rewrite               # 23 passed
cargo test -p wxml-lsp --test native_parity                         # 76 passed / 26 ignored
git diff --check                                                    # 通过
```

说明：`native_parity` 有一次 75 passed / 1 failed，失败发生在并行 owner 正在写
`docs/plans/tsgo-test-ledger.json` 的瞬间；并行写入完成后立即重跑为
76 passed / 26 ignored。该 suite 直接读台账文件，回填时必须串行执行。

### 10.5 回填结果（2026-11-20，未 commit）

- 台账已回填：`TS-ANALYSIS-0184`（exact/green，2 条证据）、
  `TS-REWRITE-0197`（exact/green，1 条证据）、`TS-ANALYSIS-0161`
  （weaker/planned，1 条证据）。字段分别为
  `green` + `migrated_green` + `exact` + `green_eligible=true` +
  `status_recommendation=green`，以及 `planned` + `proposed` + `weaker` +
  `green_eligible=false` + `status_recommendation=planned`。
- 最终计数：**70 green / 233 planned / 36 retired**；`exact` 70、`weaker` 96、
  `assessed_cases` 197、无证据 106；`near_exact` 2、`structural` 7、`blocked` 22、
  `retired` 36 不变。validator 为 `GREEN_CASES=70`、`PLANNED_CASES=233`、
  `WEAKER_CASES=96`；台账 Markdown §23.9 与 contract §1 已同步。
- 0184/0197 的 integrator 裁决按 §7.2 采纳：oracle 的唯一语义断言是最终 WXML
  字节范围，由 Rust mapping 决定；checker 只提供诊断存在性这一 arrange 前提。
  若后续评审拒绝该口径，应整体退回 weaker（§6 情景 A），不能保留 green。
- 0161 的 type/doc 断言仍缺默认运行的真实引擎证据；保持 weaker。
- 本轮未改 README、生产代码、测试源码、manifest 或 `fixture_hash.rs`；未 commit，
  未 review。
