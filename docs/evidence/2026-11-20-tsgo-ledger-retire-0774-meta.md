# tsgo wave-11 ledger retire：0774 + META-0027（2026-11-20）

## 0. 身份、边界与结论

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD：`51abc13384f91329135943b383ec4f578a5cc3c6`（`51abc13`）
- 记录日期：2026-11-20
- 范围：只改台账、validator、契约、台账 Markdown 与本证据文件；未改生产代码、测试断言、
  README、`Cargo`、manifest、`fixture_hash.rs` 或 oracle live/frozen；未 commit，未 review。
- 输入：
  - `docs/evidence/2026-11-20-tsgo-wave11-retire-audit.md` §2.4/§2.5（E 类退休审计）
  - `docs/evidence/2026-11-20-tsgo-wave11-contract-decisions.md` §5.2（`retirement_decision` 建议）
  - 当前 JSON 实际计数，而不是任务书或旧 wave 的数字

**结论：** 只应用两个最安全的 retired-with-decision：

1. `TS-COMPONENT-0774` → `candidate_b_layout`（decision id `wave11-e-0774`）
2. `TS-META-0027` → `superseded_contract`（decision id `wave11-e-meta-0027`）

基线为 **70 green / 233 planned / 36 retired**，退休后为 **70 green / 231 planned / 38 retired**。
`exact` 70、`near_exact` 2、`weaker` 96、`structural` 7、`blocked` 22 不变；`assessed_cases` 197
不变；无证据 planned 106→104。**退休不是迁移**，这两个 case 不进入 `green` 或 `exact`。

---

## 1. 基线读取

读取 `docs/plans/tsgo-test-ledger.json` 与 `crates/wxml-lsp/tests/native_parity/ledger.rs` 的实际值：

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
| `dispositions` | direct 240 / rewrite 52 / split 11 / obsolete 36 |

两个 case 在退休前都是 `planned`、无 `migration_evidence`、`target_rust_test_status=proposed`。
`TS-META-0027` 另有 `requires_decision=true`、`decision_status=accepted-rewrite`；该历史 decision
在本轮被 `retirement_decision` 取代，而不是把旧字段继续挂在一个已退休的 case 上。

---

## 2. `TS-COMPONENT-0774`（candidate_b_layout）

### 2.1 为什么退休

- oracle：`backends/typescript/test/componentBindings.test.ts:774`
  “symbol-bridge does not report resolved behavior roots as missing”，断言 `result.ok===true`、
  `buildStatus==="partial"`、`behavior` 诊断中没有 `code===2339`。
- oracle 显式设置 `layoutId: "symbol-bridge"`；这是内部 Candidate-B 布局，生产不可选。
- 生产代码证据：
  - `backends/typescript/src/server.ts:152` 调 `analyze(params, { backendDir })`，不传 `layoutId`；
  - `backends/typescript/src/analysis.ts:59-60,416` 明确 candidate layout 为内部布局，生产默认
    `in-place-shadow`。
- 与 2026-11-17 退休的 36 个 Candidate-B/symbol-bridge case 同源，因此作为第 37 个 `obsolete`
  处理；生产 in-place 路径仍由其他 behavior 测试和 opt-in gate 覆盖。

### 2.2 保留的 Candidate-B 规则

- `disposition="obsolete"`
- `status="retired-with-decision"`
- `requires_decision=true`
- `target_rust_test_status="not_migrated_internal_evidence"`
- `migration_evidence` 缺失
- `decision_status="accepted-obsolete"`

`disposition_reason` 仍写明 Candidate B/symbol-bridge 是内部布局；validator 对新增 obsolete case
同时接受 `retirement_decision.scope="candidate_b_layout"`，不再只依赖字符串匹配。

### 2.3 替代证据（不是 exact 2339 断言）

| 证据 | path:line | strength | 说明 |
| --- | --- | --- | --- |
| `native_component_behavior_data_and_properties` |  crates/wxml-lsp/tests/native_parity/native_engine_real.rs:2867 | `opt_in` | `G-P5-E2E-11` 需要 `WXML_TEST_NATIVE=1` + 真引擎；断言 behavior-backed component data，不是默认 2339 断言 |
| `case_component_0570_only_unresolved_behavior_is_partial_and_invents_no_field` |  crates/wxml-backend-tsgo/tests/script_behavior.rs:340 | `blocked` | 只覆盖 partial behavior；测试自己记录“不断言 checker diagnostics/2339” |
| `case_component_0763_non_probe_double_underscore_data_names_remain_usable` |  crates/wxml-backend-tsgo/tests/script_behavior.rs:430 | `near_exact` | near-exact behavior facts；不复现 symbol-bridge 的 2339 过滤 |

**明确限制：** 没有默认运行的 exact 2339 断言。替代强度是 opt-in gate + blocked/near_exact，
不能写成 exact 或“已等价”。如果未来要求默认 exact 覆盖，应新增 in-place behavior-data 2339
测试，而不是把本 case 提升为 green。

---

## 3. `TS-META-0027`（superseded_contract）

### 3.1 为什么退休

- oracle：`backends/typescript/test/meta.test.ts:27` “the C1 contract documents the pinned versions”，
  断言 `docs/reference/type-bridge-c1.md` 匹配 `6\.0\.3` 和 `5\.3\.1`。
- 该文档是历史 Node C1 桥契约；native 路径已迁移到 TypeScript `7.1.0-beta`，不再实现 C1。
- 保留这个测试等于用一个通过的文档字符串证明产品不再使用的版本；版本 pin 意图已由 F0 默认测试覆盖。

### 3.2 前置文档修正

`docs/reference/type-bridge-c1.md` 在退休前仍写 TypeScript `6.0.3` 且声称“均有断言测试”。本轮先修正：

- 头部 Status 增加“native 路径不实现 C1；C1 只保留为历史 Node 桥契约”；
- §9 的版本段改为 native `7.1.0-beta` / typings `5.3.1`，指向 F0 测试；
- 旧 `6.0.3` 明确标为历史 Node 桥版本，不再作为当前门禁。

如果不改文档只退休测试，会留下“6.0.3 均有断言”的假陈述；因此文档修正是本 case 的前置条件。

### 3.3 替代证据（默认运行）

| 证据 | path:line | strength | 说明 |
| --- | --- | --- | --- |
| `pinned_identity_uses_literal_f0_values` |  crates/wxml-backend-tsgo/src/config.rs:415 | `weaker` | 断言 7.1.0-beta、gitHead、tarball、binary、AST v9；engine 版本有意偏离 6.0.3 |
| `type_assets_validate_pin_identity_and_paths` |  crates/wxml-backend-tsgo/src/input.rs:2201 | `weaker` | 断言 typings `5.3.1` package pin；版本相同，但断言机制是 package pin，不是 C1 文档字符串 |
| `repository_root_lock_is_frozen_and_canonical` |  crates/wxml-delivery/tests/assets_lock_root.rs:130 | `weaker` | 断言 committed lock 的 7.1.0-beta/5.3.1 与摘要；engine 版本有意偏离 6.0.3 |

`replacement_case_id="F0-version-pin"` 是外部标签，不是 ledger case；真实替代以上述 3 个测试为准。

**不声称版本 exact 等价：** 旧 6.0.3 → 新 7.1.0-beta 是有意偏差。`TS-META-0009`（同文件 sibling）
仍是 `planned`/`weaker`，本轮没有提升任何 sibling case。

---

## 4. 台账、validator 与契约改动

### 4.1 台账 JSON

- 两个 case 的 `status` 改为 `retired-with-decision`、`target_rust_test_status` 改为
  `not_migrated_internal_evidence`，且不写 `migration_evidence`。
- `TS-COMPONENT-0774`：`disposition` rewrite→obsolete、`requires_decision=true`、
  `decision_status=accepted-obsolete`，新增 `retirement_decision`。
- `TS-META-0027`：保留 `rewrite` disposition，`requires_decision=false`，删除旧的
  `decision_*`/`proposed_replacement` 字段，新增 `retirement_decision`；保留外部
  `replacement_case_id="F0-version-pin"`。
- `counts.statuses` 与 `counts.migration_evidence.statuses`：70 / 231 / 38。
- `counts.dispositions`：direct 240、rewrite 51、split 11、obsolete 37、blocked-decision 0。
- `counts.integrator_decisions`：`accepted_obsolete` 37、`accepted_rewrite` 0、
  `accepted_superseded_contract` 1、`remaining_blocked` 0。
- `counts.migration_evidence`：`assessed_cases` 197、`exact` 70、`near_exact` 2、`weaker` 96、
  `structural` 7、`blocked` 22、`remaining_planned_without_evidence` 104。
- `by_partition`：`behaviors` routed 21 / unassessed 3；`protocol_limits` routed 39 / unassessed 21；
  其余分区不变。

### 4.2 Validator 新规则

`crates/wxml-lsp/tests/native_parity/ledger.rs`：

- 常量：`PLANNED_CASES=231`、`RETIRED_CASES=38`、`NON_OBSOLETE_RETIRED_CASES=1`、
  `OBSOLETE_CASES=37`。
- 新增 `ALLOWED_RETIREMENT_SCOPES = ["candidate_b_layout", "superseded_contract"]`。
- 新增精确哨兵 `RETIREMENT_DECISIONS`：
  `[("TS-COMPONENT-0774","wave11-e-0774","candidate_b_layout"),
    ("TS-META-0027","wave11-e-meta-0027","superseded_contract")]`。
- `direct/rewrite/split + retired-with-decision` 必须有 `retirement_decision`；校验：
  - 稳定 `id`（小写 slug）、`date="2026-11-20"`、`owner="integrator"`；
  - 非空 `required_by`、`reason`（>40 字符）、`product_impact`、`evidence`（字符串数组）；
  - 非空 `replacement`；每项校验 `kind`（`rust_test`/`none`）、`strength`
    （`exact`/`near_exact`/`weaker`/`structural`/`blocked`/`opt_in`）、`note`；
  - `rust_test` 的 `path` 必须是仓库相对 `.rs`，`line` 必须正好指向 `fn test(` 声明。
- 旧 36 个 `obsolete` 继续使用 2026-11-17 `accepted-obsolete` 与 `requires_decision=true`；
  validator 还要求无 `retirement_decision` 的 obsolete 带 `decision_status="accepted-obsolete"`
  和 `decision_at="2026-11-17"`，防止新增 case 只靠字符串“Candidate B”通过。新增 obsolete case
  若带 `retirement_decision`，以 `scope="candidate_b_layout"` 为准。
- 旧哨兵替换：
  - `decision_ids.len()==37` → `decision_ids.len()==OBSOLETE_CASES(37)`；
  - `non_obsolete_decision_ids==["TS-META-0027"]` → `non_obsolete_decision_ids` 为空；
  - 新增 `retirement_decisions==RETIREMENT_DECISIONS` 精确集合断言。
- `non_obsolete == PLANNED+GREEN+NON_OBSOLETE_RETIRED`、`targets.len()` 同式；
  `obsolete == OBSOLETE_CASES`。
- migration evidence 测试：所有 retired case 跳过分区 routed 计数；非 obsolete retired 必须有
  `retirement_decision`，且 `migration_evidence` 必须缺失。

### 4.3 契约与路由常量

- `docs/reference/tsgo-test-contract.md`：§1 计数同步（obsolete 37、planned 231、retired 38、
  无证据 104）；§3 增加 `direct/rewrite/split + retired-with-decision` 的 `retirement_decision`
  规则与两个 scope；§8 G5 改为“green/reviewed 或 retired-with-decision + retirement_decision”。
- `docs/reference/type-bridge-c1.md`：头部与 §9 的 stale 6.0.3 修正为 native 7.1.0-beta/5.3.1。
- `crates/wxml-lsp/tests/native_parity/behaviors.rs`：路由常量 22→21（0774 转 obsolete）。
- `crates/wxml-lsp/tests/native_parity/protocol_limits.rs`：只改注释，常量 40 保留（META 仍是
  non-obsolete 路由标签，但不再计入 JSON evidence routing）。

---

## 5. 影响

| 项 | before | after |
| --- | ---: | ---: |
| `green` | 70 | 70 |
| `planned` | 233 | 231 |
| `retired-with-decision` | 36 | 38 |
| `exact` | 70 | 70 |
| `near_exact` | 2 | 2 |
| `weaker` | 96 | 96 |
| `structural` | 7 | 7 |
| `blocked` | 22 | 22 |
| `assessed_cases` | 197 | 197 |
| 无证据 planned | 106 | 104 |
| `obsolete` | 36 | 37 |
| `rewrite` | 52 | 51 |

- `planned` 减少不等于迁移完成；`exact` / `green` 不变。
- 默认套件没有新增 exact 2339 覆盖（0774），也没有新增 rewrite 大输入 perf 覆盖（0378 仍 planned）。
- `TS-EVIDENCE-0011/0025/0050` 与 `TS-REWRITE-0378` 保持 `planned`；本轮没有缩小 WP8 或 perf 范围。

---

## 6. 未改动

- 未改 README、`Cargo.toml`/`Cargo.lock`、`docs/plans/tsgo-legacy-oracle-manifest.json`、
  `crates/wxml-lsp/tests/native_parity/fixture_hash.rs`。
- oracle live/frozen 双源逻辑、wave-7/8/9/10/10b/10-conditional 证据全部保留。
- 未改生产代码与测试断言；`behaviors.rs`/`protocol_limits.rs` 只同步 ledger 路由常量/注释。
- 未 commit，未 review。

---

## 7. 验证

| 命令 | 结果 |
| --- | --- |
| `jq empty docs/plans/tsgo-test-ledger.json` | 通过 |
| `cargo test -p wxml-lsp --test native_parity` | 76 passed / 26 ignored / 0 failed |
| `rustfmt --check`（本轮改动的 Rust 文件） | 通过 |
| `git diff --check` | 通过 |

说明：工作树同时存在其他 wave-11 session 的未提交改动（`crates/wxml-backend-tsgo/src/query/`、
`shadow_layout.rs`、`backend_tsgo/tests.rs` 及若干 evidence 文档），本文件不把它们计为本轮产物。
