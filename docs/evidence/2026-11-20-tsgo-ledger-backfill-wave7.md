# tsgo 测试台账 wave-7 回填（2026-11-20）

## 0. 身份与边界

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- 记录/验证 HEAD：`94c8a668b3903904a00343459563181c265925f8`
- 记录日期：2026-11-20
- 范围：只改测试台账（`docs/plans/tsgo-test-ledger.json`）、Markdown 台账
  （`docs/plans/2026-11-17-tsgo-test-ledger.md`）、contract 计数
  （`docs/reference/tsgo-test-contract.md`）、ledger validator 常量
  （`crates/wxml-lsp/tests/native_parity/ledger.rs` 的 `WEAKER_CASES`）、证据索引
  （`docs/evidence/README.md`）和本证据文档。未修改生产代码、测试源码、Cargo、
  delivery、Node 或默认后端；未提交。
- 记录期间工作树有并行会话的未提交改动
  （`crates/wxml-lsp/src/backend_tsgo/{completion,mod,tests}.rs`、
  `docs/plans/*`、`scripts/f3-*`、`crates/wxml-delivery/tests/f3_legacy_real.rs`
  等）。本回填只读它们**已提交**的内容，不把未提交测试当作本 case 证据；也没有
  回滚或覆盖并行会话在台账中的未闭合成员补全条目。
- `gate_green` 与 case `green` 保持两个状态轴：本轮没有因为任何 gate 变绿而提升
  case，也没有修改 gate 状态。

**结论：没有新的 exact case。** case 级保持 **48 `green` / 255 `planned` /
36 retired**，339 总数不变。新增 6 个带证据的 planned（全部 weaker），
`assessed_cases` 154 → 160，`weaker` 74 → 80，无 Rust 证据的 planned 149 → 143；
`exact` 48、`near_exact` 3、`structural` 7、`blocked` 22 不变。

---

## 1. 逐条 exact 缺口（definitions / hover / diagnostics / completion / events / guards / locals）

判定口径：TS oracle 的每条可观察断言都能在已提交 Rust 测试中找到等价断言，才可
`exact`。只缺一条（status、reason、range、fileId、absent definition、`probeProblems`
等）即保持 weaker/blocked；本轮不允许改测试源码，因此缺断言不能靠补测试消除。

| Case | parity | 缺口（TS 断言 vs 当前 Rust 断言） |
| --- | --- | --- |
| `TS-ANALYSIS-0033` | weaker | hover 类型/文档和 diagnostic range 分散在两个 adapter 测试；没有同一次 analyze 同时断言 definition 的 script kind + 精确 range，也没有 protocolVersion/requestId 回显。 |
| `TS-ANALYSIS-0054` | weaker | native 不发布 per-identifier `hovers[]` 候选数组，只覆盖单个 identifier。 |
| `TS-ANALYSIS-0123` | weaker | Page→Component fallback 已断言；`registration.count == 1` 与 hover `"string"` 未断言。 |
| `TS-ANALYSIS-0297` | weaker | native 没有 `finishedStates` 式 per-capability 聚合；diagnostic 丢弃/Partial 已断言，hover 与 definition 仍 supported 未同测断言。 |
| `TS-COMPLETION-0078` | weaker | labels 与 replace range 精确；native 返回 `Partial`，TS 断言 `supported`。 |
| `TS-COMPLETION-0104` | weaker | replace range 精确；pinned completion API 无 per-entry documentation/auto-import origin，native 非空结果必带 `Partial`，TS 断言 `supported`。 |
| `TS-COMPLETION-0207` | weaker | 触发条件不同；`truncated == false` 未断言。 |
| `TS-DEFINITIONS-0013` | weaker | 真实引擎 gate 断言 cross-file declaration 的 UTF-8/UTF-16 range；不用 `.d.ts` target、不分类 external snapshot、不禁用多 target。 |
| `TS-DEFINITIONS-0036` | blocked | native 无 generated-member 分类，也没有 overlay `properties.<name>` 反向映射；无法断言 generated kind/range。 |
| `TS-DEFINITIONS-0052` | weaker | 无 external/generated/dropped 分类，也没有 3 scenario × 2 layout 的 sweep。 |
| `TS-DEFINITIONS-0074` | weaker | undecodable/invalid target 会 drop 并 Partial；没有 derived query file 模型或 `/derived file/` reason。 |
| `TS-DEFINITIONS-0093` | weaker | 无 `.d.ts` library target 与 TypeScript 版本 guard；gate 仍是 opt-in。 |
| `TS-EVENTS-0093` | weaker | `skipped=0`、Supported、definition 存在已断言；fileId、精确 range、signature 未断言。 |
| `TS-EVENTS-0162` | weaker | 两个 handler 都 Partial、无诊断已断言；`multiple Page registrations` reason 与 definition 起始位置未断言。 |
| `TS-EVENTS-0176` | weaker | 两个非字面量注册 Partial、无诊断已断言；`registration is not a literal object` reason 未断言。 |
| `TS-EVENTS-0236` | weaker | Partial/no diagnostic/no definition 已断言；`behavior methods` reason 未断言。 |
| `TS-EVENTS-0268` | weaker | 已知 handler 有 definition、两者无诊断；两个 handler 的 Partial status 未断言。 |
| `TS-EVENTS-0280` | weaker | 已知 definition、无诊断已断言；spread reason、Partial status、精确 start 未断言。 |
| `TS-EVENTS-0301` | weaker | Supported、error severity、`component` message 已断言；absent definition 未断言。 |
| `TS-EVENTS-0322` | weaker | 三个保留键的 error severity 与 onTap definition 已断言；四条结果的 Supported status 未断言。 |
| `TS-EVENTS-0339` | weaker | 第一个 handler Partial/no diagnostic、第二个无 diagnostic/definition 已断言；第二个 Partial 与 `not a literal function` reason 未断言。 |
| `TS-EVENTS-0461` | weaker | component path、中文文档、`detail: number` signature 已断言；Supported status 未断言。 |
| `TS-EVENTS-0188` | blocked | inline Behavior 方法的文档已断言；TS 还断言 behavior fileId 与 handler signature，当前 inline fixture 不建模该行为文件。 |
| `TS-GUARDS-0075` | weaker | open 文本与 `unresolved=false` 已断言；`close == "}\n"` 未断言。 |
| `TS-GUARDS-0097/0218/0261` | blocked | 需要 checker 的分支/真值 narrowing；Rust 只固定渲染结构和 closer 顺序。 |
| `TS-GUARDS-0201` | blocked | 需要 diagnostics Partial + typed reason + 空诊断 + 无伪造 2339；Rust 只固定 layout status/reason/text。 |
| `TS-GUARDS-0312` | blocked | 需要发布 typo diagnostic；Rust 只固定 `JsCheckApplied::Pragma`、`// @ts-check` 前缀与 note。 |
| `TS-LOCALS-0063` | weaker | 精确 text 与 closer 已断言；`unresolved == false` 未断言。 |
| `TS-LOCALS-0089` | weaker | capture 与无自引用已断言；完整 text 与 closer 未断言。 |
| `TS-LOCALS-0144` | weaker | level 数与 item 名已断言；`levels[0].index.name` 未断言。 |
| `TS-LOCALS-0343` | blocked | 需要 diagnostics Partial + typed reason + 空 fragment diagnostics；Rust 只固定 layout status/reason/fallback text。 |
| `TS-COMPONENT-0193/0229/0279/0292/0498/0570/0676` | blocked | 与 wave-6 相同：Node `probeProblems`、union 成员结构 flag、symbol-bridge layout、behavior binding diagnostics 缺失。 |

`TS-COMPONENT-0521/0763`、`TS-REWRITE-0207` 是 near-exact（字段改名/typed error 替代），
按 P5 策略保留 `planned` + green recommendation，本轮不提升。

### 1.1 已检查但未提升的“看似只差一条”候选

| Case | 为什么不能按“已隐含”提升 |
| --- | --- |
| `TS-LOCALS-0063` | 精确 text 只证明三条 binding 都渲染为 resolved 形态，不能证明实现没有同时把 `unresolved` 置 true；该 flag 是独立可观察字段。 |
| `TS-GUARDS-0075` | `close` 是 `RenderGuards` 的独立字段，未断言就可能回归为空串。 |
| `TS-EVENTS-0301/0322/0461` | `status` 与 `definition` 是独立字段；native 已有 “Partial + definition” 组合（0162），因此 definition/severity 不能反推 Supported。 |
| `TS-EVENTS-0093` | signature/range/fileId 都没有在 component `methods` fixture 上断言。 |
| `TS-COMPLETION-0078/0104` | status 差异是 pinned API 的架构边界，不是断言遗漏；伪造 `supported` 会违反 fail-closed。 |

---

## 2. 新增 weaker 证据（6）

全部 `parity=weaker`、`green_eligible=false`、`status_applied=planned`、
`target_rust_test_status=proposed`。逐条 note 在 JSON
`cases[].migration_evidence.note`。

| Case | Rust 证据（path:line） | 已覆盖 | 缺口 |
| --- | --- | --- | --- |
| `TS-ANALYSIS-0078` |  crates/wxml-backend-tsgo/src/native/vfs.rs:1704<br> crates/wxml-backend-tsgo/src/native/vfs.rs:1670 | `lib`/`target`/`module` 进入生成的 tsconfig；core lib 与 pinned typings 以 virtual asset 挂载 | 没有引擎运行后的 hover `"string"`，也没有 2693/2304 缺失断言 |
| `TS-ANALYSIS-0093` |  crates/wxml-backend-tsgo/src/native/vfs.rs:1922 | typeRoots/types 保留，`types/local/index.d.ts` 来自 snapshot，`types` 未降级 | hover `"from-type-root"` 与 2304 缺失断言未覆盖 |
| `TS-ANALYSIS-0139` |  crates/wxml-backend-tsgo/tests/script_properties.rs:72 | 声明 `number` 在 default 值不匹配时仍保留为 `type_text` | default 是字面量而非 unresolved enum；hover `typeOnly` 未断言 |
| `TS-ANALYSIS-0174` |  crates/wxml-backend-tsgo/src/native/vfs.rs:1385<br> crates/wxml-backend-tsgo/src/native/vfs.rs:1600 | VFS 只回答 snapshot 路径；startup canary 拒绝 host 文件 | 没有“project root 不存在时 analyze 仍返回 hover”的断言 |
| `TS-PROBES-0027` |  crates/wxml-backend-tsgo/src/shadow/layout.rs:2415 | BOM+hashbang 脚本下 pragma 插入与 BOM 替换被记录 | `scanPragmas` 的 `injectionOffset`/`neutralizeOffset`/`dropBom` 元组与 plain/BOM 单独变体未断言 |
| `TS-GUARDS-0302` |  crates/wxml-backend-tsgo/tests/shadow_locals_guards.rs:581<br> crates/wxml-backend-tsgo/tests/shadow_layout.rs:550 | `Off`/`Inherit` 映射；`@ts-check` 被 shadow-only `@ts-nocheck` neutralise | hover 仍返回、template diagnostics 变 `disabled` 未断言 |

这 6 个 case 此前没有任何 `migration_evidence`；本轮只登记已提交测试，不新增测试、
不改变 status。

---

## 3. 计数变化

| 项 | wave-7 前 | wave-7 后 |
| --- | ---: | ---: |
| `green` | 48 | 48 |
| `planned` | 255 | 255 |
| `retired-with-decision` | 36 | 36 |
| `assessed_cases` | 154 | 160 |
| `exact` | 48 | 48 |
| `near_exact` | 3 | 3 |
| `weaker` | 74 | 80 |
| `structural` | 7 | 7 |
| `blocked` | 22 | 22 |
| 无 Rust 证据的 planned | 149 | 143 |
| 339 总数 | 339 | 339 |

分区变化：

- `hover_docs`：有证据 3→7、planned 有证据 3→7、weaker 3→7、无证据 7→3。
- `guards_js`：有证据 11→13、planned 有证据 6→8、weaker 1→3、无证据 14→12。
- 其余分区不变。

validator 只改 `WEAKER_CASES` 74→80；`GREEN_CASES` 48、`PLANNED_CASES` 255、
`RETIRED_CASES` 36、`NEAR_EXACT_CASES` 3、`STRUCTURAL_CASES` 7、`BLOCKED_CASES`
22 均不变。`ledger.rs` 保存前重新读取，未发现并行 oracle-manifest/source-root
改动，因此除常量外没有触碰任何 validator 逻辑。

---

## 4. 验证

| 命令 | 结果 |
| --- | --- |
| `jq empty docs/plans/tsgo-test-ledger.json` | exit 0 |
| `cargo test -p wxml-lsp --test native_parity` | **68 passed / 0 failed / 26 ignored**（含并行 oracle-manifest 套件的 31 个新测试；wave-7 之前同一命令为 37 passed） |
| `cargo fmt --all -- --check` | exit 0 |
| `git diff --check` | exit 0 |

---

## 5. 限制

1. 本轮没有新增 exact case；所有 weaker 的缺口都需要改测试源码才能补齐，不在
   docs/validator-only 范围内。
2. `TS-COMPLETION-0104/0078` 的更精确未闭合成员证据来自并行会话未提交的
   `backend_tsgo/tests.rs`；按规则保持 `planned`，等提交后重新评估。
3. 工作树不是独占的；并行 completion/ledger 改动会改变 `tests.rs` 行号和台账
   note。本轮保留其已写入的 `TS-COMPLETION-0104` note，不覆盖、不回滚。
4. 未做独立 review；所有 `green` 仍是 `reviewed` 之前的状态。
5. 新增 weaker 只引用已提交测试；没有把 opt-in gate（`native_engine_real`）当作
   case green 或 blocked 证据。
