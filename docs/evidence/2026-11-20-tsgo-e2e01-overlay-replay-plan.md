# tsgo `G-P5-E2E-01` non-owner overlay replay 设计与复现计划

- 日期：2026-11-20
- 范围：P6-2 剩余 gate `G-P5-E2E-01`（unsaved non-owner script overlay propagates / survives restart / no stale results）
- 性质：read-only 设计切片。本文件之外不修改生产代码、测试、Cargo、台账或 gate 断言，不 commit。
- 结论先行：**overlay 传播/关闭/重开的主路径在当前工作树上已经可以跑通；gate 仍是结构性红灯**（没有 native replacement test），且 restart 后首个 hover 会因 epoch 推进被丢弃（真实 LSP 红）。最小实现应聚焦“显式输入指纹 + restart 首请求重试语义 + 一个 LSP 级 gate 测试”，而不是重写 session。

---

## 0. 身份与工作树漂移

任务给出的基线是 `0ca4375`。实际读取和探测期间，另一个会话在本工作树提交了：

| 提交 | 说明 |
| --- | --- |
| `393ff0d` | `feat(tsgo): complete optional types and guard ranges` |
| `814ca5e` | `feat(tsgo): route script diagnostics and expression completion` |

本切片读取代码时的 HEAD 是 `814ca5e`。探测使用的 ` target/debug/wxml-lsp` 构建于 2026-11-20 00:45；构建时这些改动已在工作树中（00:51 才提交），所以按时间线应对应 `814ca5e` 的改动内容，但**没有做逐字节重建验证**。它不是 release 产物，也不是 gate 证据。

工作树仍有其他会话的未提交改动（`crates/wxml-delivery/tests/f3_acceptance/*`、`editors/zed/*`、若干 docs 和 `scripts/f3-exec-audit.sh`）。本切片没有触碰这些文件。

> 证据记录必须写明实际 HEAD、二进制构建时间和包内引擎/typings 身份；不能把“读取时 HEAD”写成任务给的 `0ca4375`。

---

## 1. Gate 现状：先区分“结构性红灯”和“行为性红灯”

### 1.1 Gate 文件里没有 E2E-01

`native_engine_real.rs` 目前只有 13 个 native gate，`GATE_REPLACEMENTS` 也是 13 行：

-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_engine_real.rs:41`：`GATE_REPLACEMENTS`，没有 `G-P5-E2E-01`
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_engine_real.rs:83`：`IGNORED_GATE_TESTS`，没有 `native_non_owner_script_overlay_survives_restart`
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:52`：`REQUIRED_NOT_MIGRATED_IDS` 仍包含 `G-P5-E2E-01`
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:65-69`：冻结计数仍是 `13 gate_green / 10 required_not_migrated / 23 old ignored`

因此当前**不存在一个可以失败的 native E2E-01 测试**。这是 gate 的结构性红灯：不是“测出来产品行为错了”，而是“还没有 native 证据”。

### 1.2 唯一的旧测试是 Node-only

旧 gate 在：

-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:493`：`non_owner_script_overlay_propagates_and_survives_a_node_restart`
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:549`：`ProjectState::new()` + `open_root`
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:562`：`host.open_document(models/user.ts, OVERLAY, version 1)`
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:576`：`NodeBackend::new(...)`
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:601`：`host.close_document`
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:616`：`drop(backend)`，再 `NodeBackend::new(...)`

旧测试的四个冻结断言：

| 步骤 | 请求/版本 | 期望 |
| --- | --- | --- |
| 1 | 打开 `models/user.ts` overlay v1，hover `pages/index/index.wxml` 的 `name` | hover 文本包含 `OVERLAY` |
| 2 | `close_document(models/user.ts)`，同一 WXML snapshot | hover 不包含 `OVERLAY`，回到 disk 的 `string` |
| 3 | 重新 `open_document` overlay v2 | hover 再次包含 `OVERLAY` |
| 4 | drop 旧 backend，创建新 backend | 新 backend 仍包含 `OVERLAY`（overlay 被 replay） |

该测试直接构造 `NodeBackend`/`NodeBackendConfig`，没有 `--backend native` 入口，不能把环境变量改成 native 来复用。

### 1.3 “restart” 在 native 侧的真实含义

native 没有公开的 `restart` JSON-RPC 方法。实际有两条路径：

1. `Backend::invalidate()`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:825` 推进 epoch，`try_lock` 成功时立即 drop session；失败时由 in-flight 查询在返回时发现 generation 变化并 drop。
2. 引擎进程死亡：下一次 `ensure_open` 走 `start_and_open`（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:468`），从**当前 host view** 重建 `AnalysisInput`，所以 overlay 会被隐式 replay。

旧 Node 测试的 `drop(backend)` 是进程内重建；native 的对应物是 1 或 2。LSP 级 gate 没有 `invalidate` 入口，只能用 2（测试侧杀 engine child）或在 `backend_tsgo/tests.rs` 用 1 做 in-process 证据。两者不是同一层，gate 需要明确选一个。

---

## 2. 当前失败：精确复现

### 2.1 失败 F1：gate 无 native replacement（今天就能复现）

命令（只读，不启动引擎）：

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  cargo test -p wxml-lsp --test native_parity \
  native_non_owner_script_overlay_survives_restart -- --ignored
```

实际结果：`0 passed; 0 failed; ... filtered out`，filter 匹配不到任何测试（进程退出码可能仍是 0，但这不是 green）。gate 无法变绿，因为 replacement 不存在。

这不是产品行为失败，但它是 gate 的当前硬失败：`native_gates.rs` 仍要求该 ID 处于 `required_not_migrated`，且 `GATE_REPLACEMENTS` 没有把任何 native 测试链接到它。

### 2.2 失败 F2：restart 后第一次 hover 被丢弃（真实 LSP 可复现）

这是本次探针真正观测到的行为失败，也是 gate “survives restart” 断言会踩到的点。

复现过程（fixture 见 §2.4）：

1. 打开 `pages/index/index.wxml`，hover `name`，得到 `string`。
2. `didOpen models/user.ts` overlay `"OVERLAY"`，hover 得到 `"OVERLAY"`。
3. 找到 server 的 engine child（`<package>/engine/lib/tsc`），发送 `SIGKILL`。
4. 立即发 hover #1：返回 **`null`**；server stderr 打印 `dropping a hover result for an old revision`。
5. 再发 hover #2：返回 **`"OVERLAY"`**。

关键点：

- overlay 没有丢；hover #2 证明新 session 从当前 host view replay 成功。
- 但 hover #1 是“正在失败的旧 session 请求”，adapter 在 `with_session_mode` 里发现 session 死亡/查询失败，推进 epoch、drop session，并把这次结果按 stale 丢弃（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:357-385`）。
- server 的 hover 路径不会自动重试；只有 diagnostics 会在 epoch 变化时 mark dirty 后重试。于是“restart 后第一次 hover 就应看到 overlay”这个旧 gate 语义在当前 native 实现上不成立。
- 旧 Node 测试是 `drop(backend)` 后构造**新** `NodeBackend`，再发 hover；第一次请求就落在新 backend 上，不会经历“旧 session 失败”的丢弃。native LSP 没有显式的“构造新 backend”步骤，所以语义不对等。

结论：**F2 是当前真实的行为红灯**——不是 overlay 丢失，而是“restart 后的首个请求不保证成功”。gate 测试如果只发一次 hover 会失败；如果允许重试，则能通过。

### 2.3 失败 F3：固定 `RequestContext` 时 identity 可能复用旧 session（latent）

这是 adapter 契约层的**潜在**失败，不是本工作树当前 LSP 主路径的红灯。旧测试每次都重建 `context`（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:578-593` 使用 `host.project_revision()`），但 native 侧不少测试/调用会固定 `ProjectRevision::new(host.project_id(), 1)`（例如 ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:660-665`）。

固定 context 时的风险链：

- `build_request_plan` 把 `context.project_revision()` 原样写进 `RequestIdentity`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:856`
- `RequestIdentity` 没有 non-owner 文件集合、没有 `files_hash`、没有 non-owner overlay revision：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:185`
- `ensure_open` 只比较 `open.identity == plan.plan_identity()`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:389`、`:396`。相等就**直接返回**，不会调用 `update_project`。
- 但 `RequestIdentity.view_id` 来自 host 当前 `ViewState.view_revision`，而 `overlay_update` 会通过 `mark_views_dirty` 推进 view revision（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:1854`）。因此当前真实 host 上，non-owner overlay 即使 context 的 `project_revision` 固定，`view_id` 通常也会变，identity 仍然不同，`update_project` 仍会执行。
- 只有当 host 的 `view_id` 也不变（例如 fake `ViewProvider` 返回固定 `PreparedView`、或未来 `dirty_for_source` 返回空且 view revision 不推进）时，才会复现“overlay 已进 `PreparedView.files`、但 session 保持旧 VFS、hover 仍是 disk `string`”。

在 fake `ViewProvider`（固定 `PreparedView.view_id`）下，复现表是：

| 步骤 | host 输入 | 固定 context | `RequestIdentity` | 实际 hover |
| --- | --- | --- | --- | --- |
| 0 | disk `models/user.ts` = `string` | `project_revision=1, view_revision=1, epoch=1` | 初始 | `string` |
| 1 | `open_document(models/user.ts, "OVERLAY", v1)` | 同上 | **不变** | `string`（错误；应为 `"OVERLAY"`） |
| 2 | `close_document(models/user.ts)` | 同上 | **不变** | `string`（碰巧正确） |
| 3 | 重新 `open_document(..., v2)` | 同上 | **不变** | `string`（错误；应为 `"OVERLAY"`） |

真实 host 上第 1/3 步通常会因 `view_id` 变化而更新，所以这张表只用于证明“identity 契约本身不完整”，不能当作当前 LSP 主路径的红灯。


因此 F3 是**契约弱点/测试风险**，不是今天 LSP 主路径的既成事实。它的价值是：不要让 E2E-01 测试依赖“view_id 恰好会变”这一隐式行为；最小加固仍是给 `RequestIdentity` 加显式全输入指纹（见 §4 M1）。

### 2.4 对照：LSP 主路径当前是绿的（但不能替代 gate）

在 `/tmp/opencode/e2e01-probe` 用真实包和真实引擎跑了一个手工 LSP 探针（非提交测试）：

fixture：

```text
app.json
pages/index/index.wxml   = "{{ user.name }}"
pages/index/index.ts     = import type { User } from "../../models/user";
                           Page({ data: { user: null as unknown as User } });
models/user.ts           = export interface User { name: string }
tsconfig.json            = { "compilerOptions": { "strict": true } }
```

实测输出：

| 请求 | 结果 |
| --- | --- |
| disk hover `name` | `string` |
| `didOpen` `models/user.ts` v1，overlay `name: "OVERLAY"` | `"OVERLAY"` |
| `didChange` v2，overlay `name: "OVERLAY2"` | `"OVERLAY2"` |
| `didClose` | `string` |
| 重新 `didOpen` v3，overlay `name: "OVERLAY"` | `"OVERLAY"` |
| kill engine child 后第一次 hover | `null`（F2） |
| kill 后第二次 hover | `"OVERLAY"`（新 session replay 成功） |

多 overlay 探针（两个页面、两个 non-owner 模型）也通过：

| 顺序 | 结果 |
| --- | --- |
| A 页 `aUser.name` + `bUser.name`，两个 overlay 都打开 | `"U1"` / `111` |
| 只改 `models/user.ts`，不 sleep 立即 hover | `"U2"` / `111`（B 没被覆盖） |
| 再改 `models/extra.ts` | `"U2"` / `222` |
| B 页 `cUser.name` | `"U2"`（跨模板看到同一个 non-owner overlay） |
| 连续三轮 A/B 切换 | 两个 overlay 都保持 |

为什么主路径是绿的：

- `ProjectState::overlay_update` 对 accepted script 会 `bump_slot_revision`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:1790`、`:1855`
- `apply_host_update` 调 `workspace.set_project_revision`，把该 project 的**所有**打开文档 stamp 更新：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/diagnostics.rs:384`、` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/workspace.rs:287`
- 所以真实 LSP 请求的 context 每次都会变，`RequestIdentity` 随之变化，`ensure_open` 触发 `update_project`。
- view revision 也会变化，`view_id` 是第二道隐式指纹（见 F3）。
- 重启时 `start_and_open` 用当前 host 的 `PreparedView.files`，overlay 自然被 replay。

这个探针证明**主路径可行**，但不能替代 gate：

- 它不是 committed test，不在 `native_parity` 的 ignored 集合里；
- 它暴露了 F2（first hover dropped），但没有把“重试后成功”写成 gate 断言；
- 它没有断言 `invalidate()` 的 epoch 路径，只用了外部 kill；
- 它没有覆盖 CJK/CRLF、多 owner、stale epoch、source-root 外 overlay 等矩阵项。

### 2.5 失败分类

| ID | 类型 | 今天是否红 | 说明 |
| --- | --- | --- | --- |
| F1 | 结构性 | 是 | 没有 native replacement，gate 无法执行 |
| F2 | restart 首请求 | 是（真实 LSP 探针） | 旧 session 失败后 epoch 推进，hover #1 被丢弃；hover #2 才 replay |
| F3 | 契约/缓存身份 | 潜在 | `RequestIdentity` 无显式 non-owner 指纹，依赖 `view_id` 的隐式变化 |
| F4 | 覆盖/环境 | 是（gate 证据） | 没有 committed gate 断言，也没有固定 package/typings/引擎身份记录 |

---

## 3. 根因分解

### 3.1 输入快照构造：文本是对的，身份是隐式的

链路：

1. `HostViewProvider::prepare` 调 `ProjectHost::analysis`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:113`
2. `ProjectState::analysis` 用 slot 的**当前** `project_revision`/`view_revision` 查缓存并构建 `PreparedView`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:1602`
3. `build_prepared` 调 `slot.project.collect_sources(...)`，overlay 文本优先：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:2877`、` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-project/src/sources.rs:293`
4. `prepared_file` 把 `source_revision` 写进 `PreparedFile`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:3039`
5. `build_request_plan` 把 `view.files` 全量转成 `SourceFile`，owner 用 shadow，其余用 prepared text：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:811-850`

所以 **non-owner overlay 的文本确实在 input 里**。F3 的风险发生在“是否需要更新 session”的判断，而不是文本构造。

### 3.2 会话 project identity：template/owner 为主，non-owner 靠隐式 view_id

`ensure_open` 的比较对象是 `PlanIdentity`：

- `PlanIdentity::Template(RequestIdentity)` / `PlanIdentity::Script(ScriptIdentity)`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:343`
- `RequestIdentity` 字段：project_revision、view_revision、source FileId、source_revision、source_snapshot、script_path、script_revision、script_bytes、view_id、project_root、fragment_count、environment
- `ScriptIdentity` 已有 `files_hash`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:366`、`:1022`
- `RequestIdentity` **没有** `files_hash`

这是不对称的：standalone script plan 有显式的 `files_hash`，template plan 没有。non-owner overlay 只改变“别的文件”，不改变 owner script/template 的 revision；template plan 当前靠 `view_id`（host 当前 `ViewState.view_revision`）作为隐式代理。`overlay_update` 通常会通过 `mark_views_dirty` 推进 view revision，所以 `view_id` 通常也会变；但这是行为耦合，不是显式契约。

`same_open_project` 是另一层判断：

- ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:923`：只比较 project/root/environment。
- 它决定“能否 in-place update”；`open.identity` 决定“是否需要 update”。
- 这两层必须分开：不能为了检测 overlay 变化而把 project/root 变化塞进 `same_open_project`，否则 non-owner 更新会重建无关 project。

### 3.3 overlay 所有权：host 全局表、project 局部表、view owner 三套概念

- `ProjectState.overlays: BTreeMap<uri, (text, revision)>`：全局，按 URI：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:720`
- `Project.overlay`：每个 project 一份；`open_app` 在加载 project 时把全局 overlay 重放进 `Project`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:2318`
- `ViewState.script`：template 的 owner script；non-owner 就是“不是任何 view 的 `script`”的 project script
- `accepts_script_overlay` 的接受条件：`contains_source` + 是脚本扩展名 + 不在 excluded 目录：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-project/src/sources.rs:198`
- `overlay_update` 先写入 `state.overlays`，再判断 accepted：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:1790-1808`
- 不被 accepted 时返回 `HostUpdate::default()`，**不会** `project.set_overlay`，也**不会** bump revision。

因此 non-owner overlay 的失败面有两种不同语义：

1. **应该被分析但 identity 只靠隐式代理**（F3）：overlay 已进 `Project`；如果 `view_id`/project revision 没有推进，session 不会更新。
2. **本来就不在 source tree**（例如 `miniprogramRoot` 外的 `models/user.ts`、`dist/`、`node_modules/`）：overlay 被存进全局表但被分析层拒绝；这是有意的，但必须显式降级，不能伪装成成功。

### 3.4 didChange 顺序与 revision stamping

真实 LSP 顺序（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/diagnostics.rs:438`）：

1. `workspace.change` 应用增量文本
2. `host.change_document(ChangeDocument)` → `overlay_update`
3. `apply_host_update`：
   - `workspace.set_project_revision(revision)`（所有同 project 文档）
   - dirty view 的 `set_view_revision`
   - 立即 schedule WXML diagnostics（script/JSON overlay 不 join 输入 debounce）
4. 对 script 还 schedule `script_diagnostics`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/diagnostics.rs:714`

native 侧**不会**在 didChange 时 `invalidate`；它依赖下一次请求的 plan identity 变化。这样做的好处是避免每次编辑都重启引擎；代价是 identity 必须覆盖所有输入变化。

stale 安全网：

- server 在调度时捕获 `AnalysisStamp`（file/revision/snapshot/view_revision/project_revision/epoch）：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/workspace.rs:185`
- 结果返回时 `workspace.is_current(&stamp)` 不匹配就丢弃；
- adapter 在查询返回后检查 `state.generation != current`，不匹配就 drop session 并返回 `Cancelled`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:357-385`

所以“旧结果不会被发布”这一层是成立的；F2 的问题是 restart 首请求在旧 session 失败时被丢弃，F3 的问题是“新输入是否一定触发 update”依赖隐式 view_id，而不是“旧结果被发布”。

### 3.5 source revision：两套计数器 + `max`

- overlay revision：`state.next_overlay_revision`，每次 `overlay_update` 全局 +1：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:1794`
- disk file version：`state.next_file_version`，每次 save/watch bump：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:2658`
- `build_prepared` 对每个文件取 `max(file_versions[path], source.source_revision)`：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:2880-2886`
- `SourceFile.source_revision` 在无 overlay 时是 1，有 overlay 时是 overlay revision：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-project/src/sources.rs:312-374`

风险：

- 两个计数器独立，`max` 可能让一个高 file version 掩盖后续较低的 overlay revision；`PreparedFile.source_revision` 在一段时间内不变，而文本已经变了。
- 当前 `update_project` 每次 close/reopen 全部文件并发送 full fileSystem（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:2595`），引擎不会仅凭 revision 复用旧内容，所以今天没有观测到丢失。
- 但 adapter 的 identity 只记录 owner 的 `script_revision`，non-owner 的 revision 完全不参与；一旦未来引擎改成按 revision 增量更新，或 host 改成 delta input，就会变成真实丢失。
- 最小方案应把“revision 变化”变成“identity 的完整指纹”，而不是依赖 `max` 的单调性。

### 3.6 restart / epoch

- `NativeSession::open_project` 创建 full VFS + tsconfig：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:729`
- `update_project` 要求同 project/root、revision 前进，然后 `update_changes`（close 旧全部 + open 新全部 + full FS）：`:848`、`:2595`
- `release` 清空 epoch/project/vfs：`:957`
- adapter `start_and_open` 用当前 plan 的 `input`，而 plan 的 `files` 来自当前 host view；所以 overlay replay 不需要单独的 overlay ledger。

结论：**replay 机制已经存在，缺的是“证明它在 gate 场景下发生”的测试、restart 首请求的重试语义，以及“输入变化一定触发 update”的显式身份保证。**

---

## 4. 最小实现方案

### 4.1 不变量

| ID | 不变量 |
| --- | --- |
| I1 | 同一 project/root/type environment 下，任何输入文件文本/revision 变化都必须改变 plan identity |
| I2 | identity 变化只触发 `update_project`，不触发 session restart；只有 project/root/environment 变化或 update 失败才 restart |
| I3 | 每个 template/script plan 的 `files` 必须包含当前 project 的全部 effective sources，non-owner overlay 不能被 owner shadow 覆盖 |
| I4 | 多 template、多 overlay 在同一 session 内互不覆盖；切换 template 只做 in-place update |
| I5 | 旧 epoch / 旧 revision 的结果绝不发布 |
| I6 | restart 后第一个成功进入新 session 的请求必须从当前 host view replay 所有仍打开的 overlay；如果不做生产侧重试，则允许首个旧 session 请求被丢弃 |

### 4.2 修改点（按依赖顺序）

#### M1. `RequestIdentity` 增加完整输入指纹（gate 硬化，必需；不替代 F2 的重试语义）

文件：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs`

- 在 `RequestIdentity`（`:185`）增加 `files_hash: u64`（命名可改为 `input_fingerprint`）。
- 在 `build_request_plan_with_completion` 的 `files` 构造完成之后（`:811-850`）、`identity` 构造之前（`:856`）计算指纹。
- 指纹输入必须是**最终**发给引擎的 `files`：
  - 每个 `SourceFile` 的 path
  - `DocumentRevision`
  - 文本长度 + 文本内容 hash（建议；至少 path+revision）
  - owner script 的 `derived` shadow 文本（包含 completion 的 synthetic suffix）
- 不要只 hash `view.files` 的 revision：owner 的 `derived` 文本可能因 fragment 变化而变，但 `PreparedFile.source_revision` 不变。
- 复用/升级现有的 `input_files_hash`（`:1061`），让 `ScriptIdentity.files_hash` 和 `RequestIdentity.files_hash` 用同一个函数，避免两套语义。
- 如果担心每次请求 O(total bytes) 成本：`build_request_plan` 已经克隆了全部文本（`:811-850`），hash 是同一量级；也可以把 per-file hash 缓存在 `PreparedFile`，plan 只做组合。

为什么这是必要的硬化：

- F2 由 restart 首请求的 epoch 丢弃引起，M1 不能修复它；M1 修复的是 F3 的隐式身份依赖。
- 不改 `ensure_open` 的控制流；identity 相等判断自然从“owner 维度”升级为“全输入维度”。
- 不改 `same_open_project`；non-owner 更新仍然 in-place。
- 不改 native session/protocol；`update_project` 已经是 full FS。

#### M2. owner/non-owner revision 统一为 per-path 单调 revision（建议，E2E-01 非硬依赖）

文件：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs`

- 现状：overlay 和 file version 两套计数器，`prepared_file` 用 `max`（`:2880-2886`）。
- 最小改动一：`bump_file_version` 和 `overlay_update` 共用 `state.next_file_version`，保证同一 path 的 revision 单调且全局唯一。
- 最小改动二：`PreparedFile` 增加 `text_hash`（或 `content_len`），identity 指纹直接使用它；这样即使 revision 相同、文本不同，也不会误判。
- 这一项可以单独提交；如果 E2E-01 只要求行为，M1 已足够。

#### M3. `overlay_update` 的 accepted / stored 语义显式化（建议）

文件：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:1790`

- 现状：先 `state.overlays.insert`，再判断 `accepted`；不接受时返回 default。
- 风险：全局表和 `Project.overlay` 可能不一致；后续 `open_app` replay 时再次判断，行为依赖路径。
- 最小改动：先算 accepted，再决定是否写入全局表；对“stored but not analyzed”的情况返回带原因的 `HostUpdate`/degradation，而不是静默 default。
- 这不影响 E2E-01 的 `models/user.ts`（在 source tree 内），但能避免“overlay 有时出现、有时不出现”的假象。

#### M4. gate 测试的 restart 入口（必需，测试侧）

文件：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_lsp.rs`

- 增加 Linux-only helper：扫描 `/proc/<server>/task/*/children`，找到 `exe` 以 `<package>/engine/lib/tsc` 结尾的 descendant，发送 `SIGKILL`。
- 现有 `assert_no_engine_exec` / `descendants` 已经有一套 `/proc` 扫描（`:332`、`:490`），复用同一套遍历即可。
- 不要在 gate 里 `sleep` 等固定时间；用有界轮询：第一次 hover 允许 `null`（stale drop），随后重试到非空或超时；超时失败并打印 stderr。
- 备选：在 `backend_tsgo/tests.rs` 用 `adapter.invalidate()` 做 in-process 证据。它可以证明 epoch/replay，但**不能**替代 `native_engine_real` 的 gate replacement，因为 `GATE_REPLACEMENTS` 只链接 ignored native gate 测试。

#### M5. 新增 native gate 测试（必需）

文件：` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_engine_real.rs`

测试名：`native_non_owner_script_overlay_survives_restart`

fixture 与旧测试一致：

- `app.json`：`{"pages":["pages/index/index"],"usingComponents":{}}`
- `pages/index/index.wxml`：`{{ user.name }}`
- `pages/index/index.ts`：import `User` + `Page({ data: { user: null as unknown as User } })`
- `models/user.ts`：`export interface User { name: string }`
- `tsconfig.json`：`{"compilerOptions":{"strict":true}}`

流程：

1. `Session::start(&gate)`，`open_wxml(index.wxml, wxml_text, 1)`，等待 diagnostics。
2. hover `name`，断言 disk `string`。
3. `didOpen models/user.ts` v1，overlay `name: "OVERLAY"`；hover 断言 `"OVERLAY"`。
4. `didClose models/user.ts`；hover 断言不含 `OVERLAY`，且为 `string`。
5. 重新 `didOpen` v2；hover 断言 `"OVERLAY"`。
6. `kill_engine_child`；第一次 hover 允许 `null`/空（旧 epoch 丢弃）；第二次 hover 断言 `"OVERLAY"`。
7. `session.assert_no_node_exec()`；`session.shutdown()`；`gate.tripwire().assert_absent()`。

断言边界：

- 必须保留旧 gate 的四点：overlay 可见、close 恢复、reopen 可见、restart 可见。
- 额外增加：第一次 restart 请求不能返回 disk `string`（no stale result）。
- 不要求第一次 restart 请求一定成功；允许它因 epoch 变化被丢弃，但第二次必须成功。
- 不得把 hover 换成 diagnostics，不得接受空数组作为成功，不得跳过缺失引擎/包。

#### M6. Gate 台账/清单更新（必需，但与行为修改分开提交）

文件：

-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_engine_real.rs:41` `GATE_REPLACEMENTS` 增加 `("G-P5-E2E-01", "native_non_owner_script_overlay_survives_restart")`
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_engine_real.rs:83` `IGNORED_GATE_TESTS` 增加新测试路径
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:31` `GATE_GREEN_IDS` 增加 `G-P5-E2E-01`
-  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:52` `REQUIRED_NOT_MIGRATED_IDS` 移除 `G-P5-E2E-01`
- `GATE_GREEN_COUNT` 13 → 14；`REQUIRED_NOT_MIGRATED_COUNT` 10 → 9
- `IGNORED_GATE_COUNT` 仍是 23（这里数的是旧 Node gate，不是 native replacement 数）

这一组改动只能在 required native job 绿 + fail-closed RED 之后做；不能先改计数再补测试。

#### M7. restart 首请求：两种可选语义（先决策，再写测试）

| 方案 | 做法 | 优点 | 风险 |
| --- | --- | --- | --- |
| A. 测试侧重试（最小） | gate 测试在 kill 后允许 hover #1 为 `null`，有界重试直到 hover #2 成功；生产代码不改 | 改动最小；符合当前 adapter 的 stale-drop 语义 | 用户在一次引擎崩溃后会看到一次空 hover；gate 不能声称“首请求透明” |
| B. 生产侧 one-shot retry（更好） | `with_session_mode` 在 `ensure_open` 检测到 dead session 并成功重启后，重跑一次 operation；只有在重试也失败时才推进 epoch/返回错误 | 首请求透明；更接近旧 Node gate 的 `drop + new backend` 语义 | 需要严格保证没有并发旧结果；会改变多个 gate 的时序假设 |

建议：

- E2E-01 的最小实现先用 A，把 gate 跑绿；B 作为单独的生产改进，配 stale/并发测试。
- 如果选 A，gate 必须断言“hover #1 不是 disk `string`”，并断言“hover #2 是 `OVERLAY`”；不能只断言最终成功而忽略中间是否发布了 stale 值。
- 如果选 B，gate 可以要求 hover #1 直接成功，但仍要保留 hover #2 作为幂等性检查。

### 4.3 数据流（实现后）

```text
didOpen/didChange(models/user.ts)
  -> workspace.change / host.open_document / host.change_document
  -> ProjectState.overlays + Project.overlay (accepted script)
  -> slot.project_revision bump + prepared clear
  -> next WXML request:
       context (current project_revision/view_revision/epoch)
       -> HostViewProvider.prepare
       -> PreparedView.files (owner shadow + non-owner overlay text)
       -> SourceFile[] (path, text, DocumentRevision)
       -> input_fingerprint(files)
       -> RequestIdentity { ..., files_hash }
       -> ensure_open:
            identity differs -> same_open_project true -> update_project(full VFS)
            same project/root/env -> no restart
       -> hover/diagnostics on the updated snapshot
```

restart：

```text
Backend::invalidate / engine death
  -> epoch advance + session drop
  -> next request: ensure_open -> state.session None
  -> start_and_open(plan.input(revision))
  -> NativeSession::open_project(full VFS from current host)
  -> overlay replay is implicit in PreparedView.files
  -> old epoch result -> Cancelled; new epoch result -> publish
```

### 4.4 为什么不会“重建无关 project”

- `same_open_project` 只比较 `ProjectKey`、`ProjectRoot`、`TypeEnvironmentIdentity`（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:923`）。
- non-owner overlay 更新不改变这三者；只改变 `files_hash`，因此走 `update_existing`，不 `shutdown`。
- 不同 project/root 或 environment 变化才走 restart 分支（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:418-427`）。
- 多 template 切换时 `ProjectKey`/root/env 相同，只是 `PlanIdentity` 的 `view_revision`/`view_id` 变化，仍然 in-place。

### 4.5 为什么不会“多 overlay 互相覆盖”

- `update_project` 发送的是**新 VFS 的全量**：`update_changes` 先 close 旧 source paths，再 open 新 source paths，并带 `SnapshotFileSystem::full(new.snapshot_files())`（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:2595-2613`）。
- 新 VFS 从 `AnalysisInput.files` 构建，包含当前 project 的全部 effective sources；不是 delta。
- 只要 `PreparedView.files` 完整（`source_complete` 为 true），两个 overlay 都会在 VFS 里。
- 测试要覆盖“改 A 后 B 仍在”“A/B 两页交叉”“同一个 non-owner 被两个 owner 共享”。

### 4.6 为什么 stale 结果会被丢弃

- adapter：查询返回后检查 generation；`invalidate` 或 fatal 会推进 generation，返回 `Cancelled` 并 drop session（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:357-385`）。
- server：`AnalysisStamp` 包含 file/revision/snapshot/view_revision/project_revision/epoch（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/workspace.rs:185`）；`is_current` 不匹配就丢弃。
- 新增 `files_hash` 不改变这条路径；它只保证“新输入会被请求”，不保证“旧结果不被发布”（后者已经成立）。

---

## 5. 测试矩阵

### 5.1 行为矩阵

| ID | 场景 | 操作 | 断言 |
| --- | --- | --- | --- |
| E01-01 | owner overlay | 打开 owner `pages/index/index.ts` 的 overlay，改 `data.user.name` | hover 看到 overlay 值；close 恢复 disk |
| E01-02 | non-owner overlay | 打开 `models/user.ts` overlay | hover 看到 overlay 值；disk 文件不变 |
| E01-03 | close 恢复 | `didClose` non-owner | hover 回到 disk `string`，不残留 overlay |
| E01-04 | reopen | 再次 `didOpen` non-owner v2 | hover 再次看到 overlay |
| E01-05 | 多 overlay 不覆盖 | 同时打开 `models/user.ts` + `models/extra.ts`，只改 user | user 更新；extra 保持自己的 overlay |
| E01-06 | 多模板 | `pages/a/a.wxml` + `pages/b/b.wxml` 共享 `models/user.ts` | 两页都看到同一 overlay；互不污染 |
| E01-07 | 打开顺序 | 先 overlay 后 WXML；先 WXML 后 overlay | 两种顺序首次 hover 都正确 |
| E01-08 | didChange 顺序 | `didOpen` → `didChange` → `didSave` → `didClose` → `didOpen` | 每一步 hover 与 host revision 一致；无 stale |
| E01-09 | restart（LSP） | kill engine child；第一次 hover 允许 stale drop；第二次 hover | 第二次必须 `OVERLAY`；第一次不得是 disk `string` |
| E01-10 | restart（in-process） | `adapter.invalidate()`；旧 epoch 请求；新 epoch 请求 | 旧 epoch `Cancelled`；新 epoch overlay |
| E01-11 | stale epoch | 用旧 `BackendEpoch` 发 hover/definition | `Cancelled`，不启动新 session |
| E01-12 | CJK/CRLF | WXML 前缀 CJK+emoji+CRLF；overlay 文本 CRLF | hover 位置 UTF-16 正确；文本不被换行规范化 |
| E01-13 | source root 外 | `miniprogramRoot` 外的 non-owner overlay | 明确 disk/Unsupported 降级；不能假绿 |
| E01-14 | 无 Node/无泄漏 | 整个 gate | `assert_no_node_exec` + tripwire absent + 无 sandbox 残留 |
| E01-15 | 不重启 | overlay 打开/修改/关闭期间 | engine PID 稳定；不新建 session/sandbox |
| E01-16 | 快速顺序 | didChange 后立即 hover（不 sleep） | hover 要么是新值，要么被丢弃后重试成功；不能是旧值 |

### 5.2 单元测试（不替代 gate）

在 `backend_tsgo/tests.rs` 增加 fake-session 测试：

- `request_identity_includes_non_owner_files`：用固定 `ProjectRevision` **且固定 `view_id`** 的 fake `ViewProvider`，open overlay，断言 `update_project` 被调用第二次。只有固定 `view_id` 才能稳定复现 F3；真实 host 会因 view revision 推进而意外更新，不能作为 RED。
- `non_owner_overlay_update_does_not_restart_session`：记录 `SessionFactory::start` 次数，overlay 变化后仍为 1（script diagnostics 可能引起额外 update，但不应 restart）。
- `stale_epoch_is_cancelled_before_update`：`invalidate` 后旧 epoch 请求返回 `Cancelled`，不调用 `update_project`。
- `completion_plan_identity_differs_from_normal_plan`：确保新增指纹没有把 completion plan 和普通 plan 混同。

### 5.3 真实引擎 gate 命令

包构建使用现有 `native_package` fixture（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_package.rs:336`、`:470`）。环境：

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  WXML_TEST_NATIVE=1 \
  WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
  TSGO_TYPINGS_ROOT=/tmp/opencode/tsgo-typings-provenance/runtime-tree \
  cargo test -p wxml-lsp --test native_parity native_engine_real \
    -- --ignored --test-threads=1
```

说明：

- `TSGO_TYPINGS_ROOT` 必须是 loader 能挂载的 typings package 内容。现有证据使用 `/tmp/opencode/assets-lock-root/typings`（26 文件，其中 20 个 `.d.ts` 被 pin 校验）；本次探针使用 `/tmp/opencode/tsgo-typings-provenance/runtime-tree`（20 文件 / 2,522,752 B，正好是 runtime pin）。
- 引擎必须是 pin 的 `lib/tsc`：SHA-256 `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70`，24,522,912 B。
- 测试必须在 `/tmp/opencode` 下的 disposable package 运行；不能在 repo 内产生临时文件。
- 新增测试后预期是 **14 passed / 0 failed / 36 filtered**；默认（非 ignored）仍是 36 passed / 0 failed / 14 ignored。

fail-closed RED：

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE -u WXML_TEST_NATIVE \
  WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=... TSGO_ASSET_ROOT=... TSGO_TYPINGS_ROOT=... \
  cargo test -p wxml-lsp --test native_parity native_engine_real \
    -- --ignored --test-threads=1
```

预期：14 failed（每个 gate 都因缺少 `WXML_TEST_NATIVE=1` 失败），不能有 skip。

### 5.4 证据字段

每次 gate 记录至少包含：

- 实际 HEAD（注意并发提交）
- 运行时间
- 引擎 SHA-256 / size / version
- typings 文件数 / 总字节 / pin 版本
- `cargo test` 的 passed/failed/filtered 计数
- RED run 的 failed 计数
- package 路径（`/tmp/opencode` 下，非 repo）
- 探针/辅助脚本不写入 repo 的说明

---

## 6. 风险与冲突面

### 6.1 与 script diagnostics routing 的冲突

- `open_overlay`（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/diagnostics.rs:344`）对 project script 会立即 `schedule_script_diagnostics`（`:714`）。
- `build_script_request_plan` 用 `HostViewProvider::prepare_script`；当脚本不是任何 view 的 owner 时，它通过 `view_templates(file)` 选**第一个**可用 template 来获取 project 文件集（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:127-144`）。
- 因此打开 `models/user.ts` 可能把共享 session 从“当前 hover 的 template plan”切到“另一个 template 派生的 script plan”。两者的 `files` 内容应当相同，但 `PlanIdentity` 不同，会多一次 `update_project`。
- 测试不要断言“overlay 更新只触发一次 update”或“engine PID 永远不变”在 script diagnostics 也开启的情况下；应断言**内容正确、没有 restart、没有 stale publication**。如果需要断言 PID 稳定，先关闭 script diagnostics 或使用不触发 script route 的 fixture。

### 6.2 与 completion / optionalTypes 共享文件的冲突

- `RequestIdentity` / `build_request_plan` 同时服务 hover、diagnostics、definition、completion、document symbols。
- completion 的 synthetic plan 故意改变 `fragment_count`、`script_bytes` 和 shadow 文本（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:604-617`、`:707-736`）。新增指纹必须包含**最终 derived 文本**，否则 completion plan 可能和普通 plan 碰撞，导致 completion 复用普通 shadow。
- `optionalTypes` 的 behavior/declared properties 在 ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:505` 的 `collect_script_facts` 中，由 `393ff0d` 修改；本切片不应改 behavior/guard 逻辑，只加 identity 字段。
- 新增指纹会让 completion 和普通请求的 identity 更精确，但也会增加 update 次数；这是正确性优先，测试应允许 update 次数增加，不允许结果错误。

### 6.3 多 template / 多 project

- 同一 project 的多个 template 共享一个 `ProjectKey`/root/env；切换 template 只应 in-place update。
- 不同 project（multi-root）有不同 `ProjectKey`/root；切换会 restart，这是允许的，但 E2E-01 的断言不能跨 project。
- `ProjectState.overlays` 是全局 URI 表；嵌套 app root 可能让同一 URI 被两个 project 路由。E2E-01 不覆盖嵌套 root；如需覆盖，单独开 case。

### 6.4 Gate 断言的不可改边界

以下内容**不能**为了变绿而改：

- ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:493-623` 的旧测试保持 `#[ignore]`、保持四个断言、保持 Node 路径；P6 不删除，P7 才删除。
- 新 native 测试必须覆盖旧测试的四个行为；可以增加 stale/多 overlay 断言，但不能减少。
- 不能把“缺少引擎/包”变成 skip；`WXML_TEST_NATIVE=1` 缺失必须是 fail。
- 不能把 hover 换成 diagnostics，不能把非空结果换成“空数组即成功”，不能接受 disk 值作为 restart 成功。
- 不能修改 `native_gates.rs` 的 23 个旧 gate 集合来绕过 E2E-01；只能把 E2E-01 从 `required_not_migrated` 移到 `gate_green`，且必须有 required native run + fail-closed RED 证据。
- 不能把 `RequestIdentity.files_hash` 作为唯一证据；gate 仍需要真实引擎的 hover 行为。

### 6.5 环境/平台风险

- 只支持 Linux x86-64；`/proc` kill helper 是 Linux-only，macOS/Windows 不声明。
- 引擎/typings pin 必须逐字节验证；不能用不同的 `tsc` 让测试变绿。
- 测试临时目录必须在 `/tmp/opencode` 或 `WXML_TEST_TMPDIR` 指定的 repo 外路径；不能写入 project root。
- 并发会话可能继续推进 HEAD；证据必须记录实际运行的 commit 和二进制 hash，不能只写“HEAD”。

---

## 7. 明确不做

- 不在本切片修改生产代码、测试、Cargo、台账、gate 计数。
- 不新增公开的 restart API；restart 只通过 `invalidate` 或测试侧进程控制。
- 不删除旧 Node 测试；不删除 Node backend；不切换默认 backend。
- 不重构 shadow/behavior/optionalTypes；只在 identity 层加指纹。
- 不把 `models/user.ts` 变成 owner script；不伪造 owner。
- 不把 non-owner overlay 的 disk 读取失败变成空成功。

---

## 8. 待决问题

1. **指纹算法**：path+revision 是否足够，还是必须包含文本 hash？建议至少 path+revision+text length；如果成本可接受，直接 full text hash。
2. **计数器统一**：是否在 E2E-01 同一切片统一 overlay/file-version 计数器？建议单独提交，避免和 gate 测试耦合。
3. **restart 测试入口**：LSP 侧 kill engine child 能直接对应 gate；in-process `invalidate` 更干净但不能作为 `GATE_REPLACEMENTS`。建议两者都做，gate 用前者。
4. **脚本 diagnostics 干扰**：gate fixture 是否暂时不打开 non-owner 脚本的 `didOpen` 之外的 diagnostics？不能禁用路由；只能接受额外的 update，并断言内容/PID 的合理边界。
5. **stale 第一次请求的断言**：允许 `null` 还是要求重试？建议允许 `null`，但要求“不得返回 disk `string`”，第二次必须 overlay；这样既证明旧结果被丢弃，又证明 replay 成功。
6. **native gate 文件位置**：新测试放在 `native_engine_real.rs` 是最小改动；新建文件会改变 `IGNORED_GATE_TESTS`/扫描逻辑，不建议。

---

## 附录 A：手工探针的最小复现记录

探针目录：`/tmp/opencode/e2e01-probe`（不在 repo 内）。

包布局：

```text
/tmp/opencode/e2e01-probe/package/bin/wxml-lsp        # target/debug 二进制副本
/tmp/opencode/e2e01-probe/package/engine/lib/tsc      # pin 引擎
/tmp/opencode/e2e01-probe/package/engine/lib/lib*.d.ts
/tmp/opencode/e2e01-probe/package/typings/miniprogram-api-typings  # 20 files / 2,522,752 B
/tmp/opencode/e2e01-probe/package/manifest.json       # schema v2，packageVersion 0.1.0
```

启动：

```bash
/tmp/opencode/e2e01-probe/package/bin/wxml-lsp --stdio --backend native
# cwd = /tmp/opencode/e2e01-probe/project
# TMPDIR = /tmp/opencode/e2e01-probe/tmp
```

关键输出（已在上文 §2.3 表格列出）：

```text
disk hover:                 string
open-overlay hover:         "OVERLAY"
change-overlay hover:       "OVERLAY2"
close-overlay hover:        string
reopen-overlay hover:       "OVERLAY"
kill engine, hover #1:      null
kill engine, hover #2:      "OVERLAY"
```

多 overlay 输出：

```text
A aUser: "U1"   A bUser: 111
change user -> "U2", bUser 仍为 111
change extra -> bUser 222, aUser 仍为 "U2"
B cUser: "U2"
```

探针不是 gate 证据：它没有提交、没有纳入 `native_parity`、没有 RED run、没有固定证据格式。它只用于确认“主路径可行”和“restart replay 可行”，从而把实现范围收敛到 identity + gate test。

---

## 附录 B：关键代码索引

| 主题 | 位置 |
| --- | --- |
| 旧 gate |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs:493` |
| native gate 清单 |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_engine_real.rs:41` |
| required 集合 |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:52` |
| RequestIdentity |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:185` |
| identity 构造 |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:856` |
| ScriptIdentity.files_hash |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:366` |
| input_files_hash |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:1061` |
| ensure_open |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:389` |
| same_open_project |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:923` |
| start_and_open |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:468` |
| invalidate |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:825` |
| epoch 检查 |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:357` |
| overlay_update |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:1790` |
| close_overlay |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:1865` |
| save_overlay |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:1933` |
| build_prepared |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:2771` |
| prepared_file |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:3039` |
| did_change |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/diagnostics.rs:438` |
| schedule_script_diagnostics |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/diagnostics.rs:714` |
| did_save |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/lifecycle.rs:465` |
| set_project_revision |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/workspace.rs:287` |
| open_project |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:729` |
| update_project |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:848` |
| release |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:957` |
| update_changes |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:2595` |
| accepts_script_overlay |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-project/src/sources.rs:198` |
| collect_sources |  ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-project/src/sources.rs:293` |

---

## 附录 C：一句话结论

`G-P5-E2E-01` 的产品主路径（overlay 传播、close 恢复、restart replay）在当前 native 实现上已经可工作；真正阻塞 gate 的是**没有 native replacement test**，以及 `RequestIdentity` 缺少 non-owner 文件指纹导致固定/过期 context 下会复用旧 session。最小实现是“给 template plan 加全输入指纹 + 一个 LSP 级 restart gate 测试 + 台账单独提升”，不需要改 native session 协议。
