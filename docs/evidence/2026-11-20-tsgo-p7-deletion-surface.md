# tsgo P7 删除面审计（2026-11-20）

## 0. 结论先行

- 审计对象：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`。
  读取时实际 HEAD 为 `b0f6cc1`（`test(p7): retarget stdio gates to native`）。
  任务文本给出的 `1c89338` 是 ledger 的 `implementation_head` 历史快照，不是当前
  读取 HEAD。并行会话在审计期间提交了 `b9291a0`（real matrix）和 `b0f6cc1`
  （stdio retarget），因此本文以实际读取的 `b0f6cc1` 行号为准；`1c89338` 仅作为
  历史基线引用。
- 性质：**只读审计**。只新增本文件；未修改代码、测试、ledger、freeze JSON、
  README、脚本、CI 或 ADR；未运行 `cargo test`、`cargo clippy`、`cargo fmt`、
  `scripts/native-gates.sh`、`scripts/p7-freeze.sh`；未 commit、未 review。
- 删除面不是“一个 `backend_node` 目录”。完整 P7 删除至少跨 **7 个面**：
  1. 运行时 Node 后端（8 文件 / 7,589 行 / 102 个单元测试）；
  2. 13 个 Rust 测试根文件 / 111 个测试属性（23 个 `#[ignore]`、2 个 silent skip）；
  3. 旧 TypeScript 树（99 tracked + 34 `dist`；34 个 `src`、21+1 个 `test`、
     35 个 `samples`、2 个 `scripts`）；
  4. v1 delivery 写路径（15 个冻结点 + 3 个未冻结生产分支 + 测试 fixture 面）；
  5. `scripts/p7-freeze.sh`、`f3-legacy-real.sh`、`verify-no-node.sh`、
     `native-gates.sh`、Zed 转发和 CI 文案；
  6. oracle/fixture/ledger/gate registry 的冻结与计数；
  7. ADR/README/合同文档状态。
- 机械层面，`old_test_removed` 只要求“Frozen oracle + gate_green + 已链接
  replacement + ledger case 仍在 manifest”；它**不校验 P7-1 的 planned=0**。
  但治理层面 G5/P7-1 仍会阻塞删除；owner 的 C-lite 裁决必须作为书面 waiver
  先行落盘，否则删除无法在评审上自洽。
- 最小安全顺序必须是：**先固定 pre-delete 身份（tag + freeze JSON + oracle
  Frozen 方案 + waiver），再删 12 个 delete gate，再 retarget 11 个 gate，
  再迁移/删除其余 Node 测试，再删 `backend_node`，再删旧 TS 树，再删 v1 写路径，
  最后 full revalidation**。反过来做会同时触发 `native_parity`、`p7-freeze
  --check`、`no_node_guard`、`verify-no-node.sh` 和 delivery 测试的硬失败。
- 两个最容易被漏掉的硬引用：
  - ` crates/wxml-lsp/tests/native_parity/native_gates.rs:99` 的
    `P7_FREEZE_RELATIVE_PATH` 会在测试运行时读取
    ` docs/evidence/data/legacy-p7-freeze.json` 的 `old_ignored_gates.gates[]`。
    这个 JSON 不能简单删除或改名，否则 `native_parity` 自身先红。
  - ` crates/wxml-delivery/tests/no_node_guard.rs:497-518` 和
    ` scripts/verify-no-node.sh:396-402` 都**静态要求 `serve_node()` 仍构造
    `NodeBackend`**。删掉 `serve_node` 后，这两个 guard 会把“已经没有 Node”
    判成 `UNVERIFIED/BLOCKED`，必须与删除同一变更修正。

---

## 1. 审计边界与事实快照

| 项 | 读取值 | 说明 |
| --- | --- | --- |
| 实际 HEAD | `b0f6cc1` | 审计期间由并行会话推进；`git status` 仅剩未跟踪证据文档 |
| 任务基线 HEAD | `1c89338` | ledger `migration_evidence.implementation_head`，历史 |
| 冻结基线 | `75040c2` | ` docs/evidence/data/legacy-p7-freeze.json` 的生成基线；脚本当前 `DEFAULT_REQUESTED_HEAD=befba03` |
| 默认 `--stdio` | native / HostOnly | ` crates/wxml-lsp/src/main.rs:200`；`NotInstalled` 才退化 |
| `--backend node` | 仍存在，deprecated | ` crates/wxml-lsp/src/main.rs:41`、`:206` |
| `backend_node` | 8 文件 / 7,589 行 / 102 测试 | ` crates/wxml-lsp/src/backend_node/**` |
| 旧 TS 树 | 99 tracked，`test` 22、`samples` 35、`src` 34 | ` backends/typescript/**`；另有 34 个未跟踪 `dist` 文件 |
| v1 写路径 | 15 个冻结点 + 3 个未冻结生产分支 + 测试面 | ` docs/evidence/data/legacy-p7-freeze.json` + §6.3 |
| 23 个 gate | 23 `gate_green`，0 `mandatory_green`，0 `old_test_removed` | ` docs/plans/tsgo-test-ledger.json` |
| legacy oracle | 机制已支持 Frozen；当前 live 树仍在 | ` crates/wxml-lsp/tests/native_parity/oracle_manifest.rs:813`；`backends/typescript/test` 存在即走 Live |
| `native-gates.sh` | 35 gates（23 native + 3 edit + 9 stdio） | 工作树已由 `b0f6cc1` 提交；perf/stability retarget 后目标 37 |
| `p7-freeze.sh --check` | 当前已过期（predeletion baseline 记录 exit 2） | 必须先 re-freeze；删除任一冻结路径后也必然失败 |

---

## 2. 删除/修改面总表

| # | 面 | 必须删除 | 必须修改/迁移 | 主要风险 |
| ---: | --- | --- | --- | --- |
| 1 | Node 运行时 | ` crates/wxml-lsp/src/backend_node/**` | `main.rs`、`lib.rs`、CLI 测试、no-node guard | `serve_node` 是唯一生产构造点 |
| 2 | Node 测试 | `real_node.rs`、`wave5_acceptance.rs`、`node_backend.rs`、2 个 `semantic_tools` real-node 测试 | 11 个 retarget；79 个 fake-C1 迁移 + 2 个重写/删除 | `q2_support` 是 q2_scheduler 的传递依赖 |
| 3 | 旧 TS 树 | ` backends/typescript/**` | `real_matrix` samples 补全、typings fallback、文档 | samples 复制当前只有 33/35 |
| 4 | v1 delivery | 15 个写符号 + `acquire.rs`/`cache.rs` | delivery 测试改静态 v1 fixture；保留只读边界 | `rollback` 的 v1 安装分支未冻结 |
| 5 | 脚本 | `f3-legacy-real.sh`（或改纯归档模式） | `p7-freeze.sh`、`verify-no-node.sh`、`native-gates.sh`、`f3-zed-smoke.sh` | `p7-freeze` 是测试运行时依赖 |
| 6 | CI | 无 Node step 可删 | `native-gates` 名称/计数；可选新增 no-Node 静态 guard | CI 默认 job 不跑 23 gate |
| 7 | Zed | 无 Node 转发 | `FORWARDED_ENV` 两个已死变量；README | 变量当前只被 `backend_node` 读取 |
| 8 | Cargo | 无 feature/dep 可删 | 无 | 删除模块不需要 Cargo 变更 |
| 9 | oracle/ledger | 不删 manifest/ledger 文件 | `old_test_removed`、silent gate 清空、gate 注释 | 计数常量与测试互相锁定 |
| 10 | 文档 | 当前面向用户的 Node 指令 | ADR-0008 状态、README/合同/功能基线 | 历史证据必须保留原事实 |

---

## 3. 运行时 Node 后端

### 3.1 必须整目录删除

` crates/wxml-lsp/src/backend_node/` 共 8 个文件：

| 文件 | 行数 | 删除的公开/关键符号 |
| --- | ---: | --- |
| ` crates/wxml-lsp/src/backend_node/mod.rs` | 797 | `NodeBackend`（:107）、`BackendState`（:80）、`impl Backend`（:431）、`Drop`（:787）、re-export（:38-39）、`pub mod {completion,config,process,protocol,request,response}`（:12-17） |
| ` crates/wxml-lsp/src/backend_node/config.rs` | 401 | `NodeBackendConfig`（:106）、`from_env`（:155）、`from_lookup`（:161）、`RequestLimits`（:33）、`Typings`（:54）、`JsCheckMode`（:72）、`DEFAULT_MAX_LINE_BYTES/DEFAULT_TIMEOUT/DEFAULT_RESTART_COOLDOWN`（:17-23）、`SERVER_RELATIVE`（:26） |
| ` crates/wxml-lsp/src/backend_node/process.rs` | 568 | `NodeTransport`（:66）、`NodeTransportError`（:35）、`StdioNode`（:75）、`Command::new(&config.node)`（:258） |
| ` crates/wxml-lsp/src/backend_node/protocol.rs` | 191 | `PROTOCOL_VERSION = 2`（:15）、`ProtocolError`（:19）、`request_line`（:65）、`parse_response`（:80）、`validate_ping`（:119） |
| ` crates/wxml-lsp/src/backend_node/request.rs` | 1,331 | C1 `analyze` 请求构造（fragments/events/locals/guards/components） |
| ` crates/wxml-lsp/src/backend_node/response.rs` | 2,714 | C1 响应解析、definition/completion 上限与截断 |
| ` crates/wxml-lsp/src/backend_node/completion.rs` | 220 | protocol v2 `complete` 解析 |
| ` crates/wxml-lsp/src/backend_node/tests.rs` | 1,367 | 37 个 fake transport 单元测试；连同内联 `#[cfg(test)]` 共 **102** 个测试 |

### 3.2 必须修改的运行时入口

| 位置 | 当前内容 | 删除后 |
| --- | --- | --- |
| ` crates/wxml-lsp/src/main.rs:10` | `use wxml_lsp::backend_node::{NodeBackend, NodeBackendConfig};` | 删除 |
| ` crates/wxml-lsp/src/main.rs:41-52` | `--backend node` 说明与 5 个 Node env | 删除整段；`--backend` 只接受 `native` |
| ` crates/wxml-lsp/src/main.rs:65` | `Action::Node` | 删除 |
| ` crates/wxml-lsp/src/main.rs:74` | `BackendChoice::Node` | 删除 |
| ` crates/wxml-lsp/src/main.rs:101` | `Ok(Action::Node) => serve_node()` | 删除 |
| ` crates/wxml-lsp/src/main.rs:160` | `"node" => BackendChoice::Node` | 删除 |
| ` crates/wxml-lsp/src/main.rs:175` | `(Some(BackendChoice::Node), false)` | 删除 |
| ` crates/wxml-lsp/src/main.rs:206-216` | `serve_node()` 构造 `NodeBackend` | 删除 |
| ` crates/wxml-lsp/src/main.rs:257` | 解析 `--backend node` 的测试 | 改为拒绝 |
| ` crates/wxml-lsp/src/lib.rs:11-13` | 文档仍说 “real Node backend land in later waves” | 改为 native-only 描述 |
| ` crates/wxml-lsp/src/lib.rs:44` | `pub mod backend_node;` | 删除 |
| ` crates/wxml-lsp/src/lib.rs:48` | “legacy Node backend is reachable only through `--backend node`” | 删除/改写 |
| ` crates/wxml-lsp/src/backend_tsgo/mod.rs:12` | `--backend node` 是唯一构造 Node 的路径 | 改为“无 Node 路径” |
| ` crates/wxml-lsp/src/backend_tsgo/mod.rs:227-228` | 注释列 `WXML_NODE/WXML_TYPE_BACKEND/WXML_BACKEND_DIR` | 改为“legacy env 不再读取” |
| ` crates/wxml-lsp/src/project_host.rs:45` | 注释“C1 Node backend accepts” | 改为 native 输入契约 |

### 3.3 不再被读取的 legacy env

删除 `backend_node/config.rs` 后，以下变量在仓库内**没有任何生产读取者**：

| 变量 | 唯一读取点 | 处理 |
| --- | --- | --- |
| `WXML_NODE` | ` crates/wxml-lsp/src/backend_node/config.rs:166` | 删除；文档/测试改为 poison-only |
| `WXML_TYPE_BACKEND` | ` crates/wxml-lsp/src/backend_node/config.rs:179` | 同上 |
| `WXML_BACKEND_DIR` | ` crates/wxml-lsp/src/backend_node/config.rs:185` | 同上 |
| `WXML_ANALYZE_TIMEOUT_MS` | ` crates/wxml-lsp/src/backend_node/config.rs:196` | 同上；**Zed 仍在转发，见 §9** |
| `WXML_TEMPLATE_CHECK` | ` crates/wxml-lsp/src/backend_node/config.rs:203` | 同上；**Zed 仍在转发，见 §9** |

---

## 4. Rust 测试面

### 4.1 13 个受影响测试根文件（111 个测试）

`backend_node` 的 14 条 `use` 行分布在 12 个文件；`stdio_type_loop` 通过 CLI 间接依赖；
`q2_scheduler` 通过 `q2_support` 传递依赖。合计 **13 个测试根文件 / 111 个测试属性**；
下表另列 `q2_support/mod.rs`，它是 helper，不计入 13 个 root。

| 文件 | 测试数 | ignore | 当前 Node 用法 | 建议动作 |
| --- | ---: | ---: | --- | --- |
| ` crates/wxml-lsp/tests/real_node.rs` | 5 | 6 | 真实 `NodeBackend` + `dist/server.js` | **整文件删除**（G-P5-E2E-01..05） |
| ` crates/wxml-lsp/tests/wave5_acceptance.rs` | 7 | 8 | 真实 `NodeBackend` | **整文件删除**（G-P5-E2E-15..21） |
| ` crates/wxml-lsp/tests/node_backend.rs` | 5 | 0 | 纯 fake `NodeTransport` | **整文件删除**；断言已被 native 覆盖 |
| ` crates/wxml-lsp/tests/stdio_type_loop.rs` | 9 | 9 | 已 retarget 到 native package（`b0f6cc1`） | **保留**；确认 `WXML_TEST_NODE` 不再出现 |
| ` crates/wxml-lsp/tests/performance_bench.rs` | 1 | 2 | `NodeBackend` + `SlowTransport` + `node --version` | **原地 retarget**：`start_real_node`（:272）、`SlowTransport`（:557）、`with_transport_factory`（:632）、`node_version`（:347） |
| ` crates/wxml-lsp/tests/q2_stability.rs` | 1 | 1 | `NodeBackend` + `node_children` 扫 `dist/server.js` | **原地 retarget**：`NodeBackend`（:226）、`node_children`（:85）、`kill_node_children`（:136） |
| ` crates/wxml-lsp/tests/semantic_tools.rs` | 41 | 0 | 39 个 fake C1 + 2 个真实 Node | **39 迁移到非 Node 测试后端；2 个删除**；`real_node_paths`（:2074）、silent return（:2099/:2602）、`tsc.exists()`（:2184） |
| ` crates/wxml-lsp/tests/lsp_p7.rs` | 6 | 0 | fake `NodeTransport` | **迁移**到非 Node 测试后端 |
| ` crates/wxml-lsp/tests/full_feature_matrix.rs` | 4 | 0 | fake `FakeNode`（:972） + `NodeBackend`（:1073） | **迁移** |
| ` crates/wxml-lsp/tests/multi_app_projects.rs` | 4 | 0 | fake `NodeBackend`（:288） | **迁移** |
| ` crates/wxml-lsp/tests/zed_sync.rs` | 13 | 0 | fake `NodeBackend`（:334） | **迁移** |
| ` crates/wxml-lsp/tests/q2_recovery.rs` | 12 | 0 | fake C1 + `StdioNode`（:159/:175） | **10 迁移 + 2 个真实子进程协议测试重写/删除** |
| ` crates/wxml-lsp/tests/q2_scheduler.rs` | 3 | 0 | 经 `q2_support` 传递依赖 | **随 `q2_support` 迁移** |
| ` crates/wxml-lsp/tests/q2_support/mod.rs` | 0 | 0 | `NodeBackend`（:188）、`FakeTransport`（:107） | **改造成非 Node 测试后端**，否则 q2_scheduler/q2_recovery 无法编译 |

### 4.2 必须一起删除的 helper/gate

| 位置 | 原因 |
| --- | --- |
| ` crates/wxml-lsp/tests/common/native_edit.rs`（219 行） | 只为 `semantic_tools` 两个 superseded Node 测试提供 fail-closed guard；两个测试删除后无调用者 |
| ` crates/wxml-lsp/tests/native_parity/native_gates.rs:1056-1279` | `semantic_tools_silent_returns_are_mandatory_unmigrated_gates` 硬要求 ledger 保留 2 个 silent gate |
| ` crates/wxml-lsp/tests/native_parity/native_gates.rs:1281-1338` | `native_edit_guard_honors_both_opt_in_flags` 直接读 `common/native_edit.rs` |
| ` crates/wxml-lsp/tests/native_parity/native_gates.rs:1123-1143` | 扫描 `semantic_tools.rs` 的 `real_node_paths()`/`skipping real Node LSP test` |

### 4.3 `cli.rs` 与 poison-only 测试

| 位置 | 当前 | 删除后 |
| --- | --- | --- |
| ` crates/wxml-lsp/tests/cli.rs:50-52` | 断言 help 含 `--backend` 的 `native`/`node` 两个值和 “deprecated transitional Node backend” | 改为只含 `native` |
| ` crates/wxml-lsp/tests/cli.rs:224-236` | `explicit_node_backend_still_initializes_without_install` | 删除或改为“拒绝 `--backend node`” |
| ` crates/wxml-lsp/tests/component_intelligence.rs:3-5` | 注释说 Node 被故意指向不存在路径 | 改为“无 Node 可启动” |
| ` crates/wxml-lsp/tests/component_intelligence.rs:40` | 设置 `WXML_NODE` | 可保留为 poison，但不再是有效语义 |
| ` crates/wxml-lsp/tests/native_lsp_document_symbols/lsp.rs:343-345` | poison 三个 legacy env | 保留，作为 native 不读 legacy env 的回归 |
| ` crates/wxml-lsp/tests/native_parity/native_lsp.rs:29-32` | scrub 列表含 `WXML_TEMPLATE_CHECK` | 保留；可加入 `WXML_ANALYZE_TIMEOUT_MS` 作为 poison |

### 4.4 测试计数影响

- `b0f6cc1` 的只读基线（见并行证据
  ` docs/evidence/2026-11-20-tsgo-p7-predeletion-baseline.md`）：
  `cargo test --workspace --all-features` = **2813 passed / 0 failed / 87 ignored**；
  `native_parity` = **78 passed / 29 ignored**（组成以该证据的分布为准）。
- 旧 Node gate 不在 `native_parity` 二进制里；删除 12 个 delete gate 后，
  `native_parity` 仍是 **78/29**，workspace ignored 从 87 降到 **75**。
  `cargo test -p wxml-lsp` 的精确 ignored 数在实施时重新记录，不要复用旧
  P7 演练的 49/37（那是 `8a56f27` 时代的组成）。
- `backend_node` 删除会同时移除 102 个单元测试；这是“删代码”而不是“删 gate”，
  不会被 `native_gates` 的计数方程捕获，必须在评审里单独记录覆盖损失。
- `semantic_tools` 的 2 个 silent skip 当前在默认套件里显示 passed（skip body）；
  删除后默认测试数会减 2，且 `silent_skip_gates` 必须从 2 变 0。

---

## 5. 旧 TypeScript 树

### 5.1 删除清单

| 路径 | 数量 | 说明 |
| --- | ---: | --- |
| ` backends/typescript/src/**` | 34 文件 / 11,068 行 | C1 Node 类型桥实现；`server.ts`、`protocol.ts`、`compiler.ts`、`analysis.ts` 等 |
| ` backends/typescript/test/**` | 21 个 `*.test.ts` + `support.ts` = 22 文件 / 6,263 行 | legacy oracle 的 live 源；删除前必须已 Frozen |
| ` backends/typescript/samples/**` | 35 文件 / 6,069 B | 28 个 sample 目录；native 测试主要用 frozen 字符串常量 |
| ` backends/typescript/scripts/**` | `emit-evidence.ts`、`inspect.ts` | Node 证据/检查脚本 |
| ` backends/typescript/package.json` | 1 | `check/test/build/start/matrix/perf/evidence/inspect` 全部 Node 脚本 |
| ` backends/typescript/package-lock.json` | 1 | `typescript@6.0.3`、`miniprogram-api-typings@5.3.1`、`@types/node@24.10.1` |
| ` backends/typescript/tsconfig.json` | 1 | 测试/脚本编译配置 |
| ` backends/typescript/tsconfig.build.json` | 1 | `dist/server.js` 构建 |
| ` backends/typescript/README.md` | 1 | Node 构建/矩阵说明 |
| ` backends/typescript/.gitignore` | 1 | `dist`/`node_modules` |
| ` backends/typescript/dist/**` | 34 未跟踪文件 | 本地构建产物；`p7-freeze` 曾纳入身份，删除后不可 `--check` |

### 5.2 samples 迁移缺口

`crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/samples`（`b9291a0` 已提交）
复制了 **33/35** 个 sample 文件，缺：

- ` alias-project/tsconfig.base.json`
- ` alias-project/tsconfig.json`

`real_matrix.rs:78` 的注释仍写 “copied byte-for-byte”。删除 `backends/typescript/samples`
前必须二选一：

1. 补齐 2 个 tsconfig 并保持 “byte-for-byte” 的事实；或
2. 明确写成“33/35，tsconfig 不参与 native matrix”，并确认没有 native 测试读取它们。

`tests/fixtures` 与 `crates/wxml-lsp/tests/fixtures` 是**保留**的统一 fixture，不是旧 TS 树，
不能随 `backends/typescript` 一起删除。

### 5.3 删除后仍会引用旧树路径的地方

| 位置 | 引用 | 处理 |
| --- | --- | --- |
| ` crates/wxml-lsp/tests/native_parity/oracle_manifest.rs:37` | `LEGACY_TEST_SOURCE_ROOT = "backends/typescript/test"` | 保留常量；Frozen 模式只用于校验，不要求路径存在 |
| ` crates/wxml-lsp/tests/native_parity/fixture_hash.rs:71-82` | ` backends/typescript/test`、`samples` 的 `legacy_archived: true` | 保留；缺失时走 Archived |
| ` crates/wxml-lsp/tests/native_parity/ledger.rs:2114` | `test.__simulated_deleted__` | 保留；模拟删除测试仍有效 |
| ` crates/wxml-lsp/tests/native_parity/real_matrix.rs:78-80` | `samples` fixture 路径 | 改指向已提交 fixture；删除后不得再指旧树 |
| ` crates/wxml-lsp/tests/native_lsp_document_symbols/support.rs:145` | 默认 typings 目录 `backends/typescript/node_modules` | 改为显式 `TSGO_TYPINGS_ROOT` 或测试资产根 |
| ` crates/wxml-backend-tsgo/tests/f1_hover.rs:427` | 默认 typings 根 `../../backends/typescript/node_modules` | 同上 |
| ` crates/wxml-backend-tsgo/tests/asset_loader.rs:821,875,926` | repo typings fixture / `npm ci` 提示 | 改为资产根；删除依赖 `node_modules` 的 ignored 测试或改 opt-in 资产 |
| ` crates/wxml-backend-tsgo/tests/type_assets.rs:634` | 默认 `backends/typescript/node_modules` | 改为显式资产根 |
| ` crates/wxml-lsp/tests/native_parity/real_matrix.rs:1838` | typings fallback `backends/typescript/node_modules` | 改为 `TSGO_TYPINGS_ROOT` |
| ` crates/wxml-lsp/tests/native_parity/fixtures/real_matrix/projection.json:4715` | `"path": "backends/typescript/src/scenarios.ts"` | 这是历史 oracle 的 provenance 字符串，不是运行时文件读取；保留不动 |

> 这些 fallback 当前大多是“目录存在才用，否则要求显式 env”。删除旧树后不会
> 产生新的运行时错误，但会让默认路径失效；必须把测试提示和 CI 文案同步到
> `TSGO_TYPINGS_ROOT`/`TSGO_ASSET_ROOT`，否则干净 checkout 的 opt-in gate 会
> 以误导性的 “run npm ci” 失败。

---

## 6. v1 delivery 写路径

### 6.1 已冻结的 15 个写符号

| 文件 | 行 | 符号 | 角色 |
| --- | ---: | --- | --- |
| ` crates/wxml-delivery/src/acquire.rs` | 60 | `acquire` | v1 acquire/cache |
| ` crates/wxml-delivery/src/bin/wxml-lsp-install.rs` | 270 | `install` | schema v1 install 分支 |
| ` crates/wxml-delivery/src/bin/wxml-lsp-install.rs` | 325 | `upgrade` | schema v1 upgrade 分支 |
| ` crates/wxml-delivery/src/bin/wxml-lsp-install.rs` | 854 | `verify_package` | 含 schema v1 verify-package 分支 |
| ` crates/wxml-delivery/src/bin/wxml-lsp-install.rs` | 942 | `install_and_report` | v1 install/verify/switch |
| ` crates/wxml-delivery/src/bin/wxml-lsp-package.rs` | 125 | `run` | 无子命令默认仍构建 v1 |
| ` crates/wxml-delivery/src/bin/wxml-lsp-package.rs` | 179 | `build_package` 调用点 | v1 packager |
| ` crates/wxml-delivery/src/cache.rs` | 176 | `store_verified` | v1 archive cache 写入 |
| ` crates/wxml-delivery/src/install.rs` | 286 | `install_tree` | v1 `versions/current` 写入 |
| ` crates/wxml-delivery/src/manifest.rs` | 632 | `parse_index` | v1 index 解析 |
| ` crates/wxml-delivery/src/package.rs` | 101 | `build_package` | v1 package 入口 |
| ` crates/wxml-delivery/src/package.rs` | 422 | `build_manifest` | v1 manifest 构造 |
| ` crates/wxml-delivery/src/package.rs` | 508 | `update_index` | v1 index.json 写入 |
| ` crates/wxml-delivery/src/verify.rs` | 244 | `check_backend` | 要求 `bin/wxml-type-backend/server.js` |
| ` crates/wxml-delivery/src/verify.rs` | 283 | `verify_archive` | v1 archive 验证 |

### 6.2 必须保留的 3 个边界

| 文件 | 行 | 符号 | 原因 |
| --- | ---: | --- | --- |
| ` crates/wxml-delivery/src/install.rs` | 257 | `verify_installed` | legacy `current`/`verify` 只读校验 |
| ` crates/wxml-delivery/src/manifest_v2.rs` | 1438 | `read_legacy_manifest` | schema v1 只读身份投影 |
| ` crates/wxml-delivery/src/package.rs` | 666 | `build_native_package` | v2 native packager |

### 6.3 冻结清单之外的隐藏写分支

| 位置 | 问题 | 建议 |
| --- | --- | --- |
| ` crates/wxml-delivery/src/bin/wxml-lsp-install.rs:430-444` | `rollback` 在目标未安装时仍走 `acquire` + `install_tree` 的 v1 安装 | `install_tree` 删除后必须同步删除；只保留“已安装 legacy 目标不可回退”的拒绝 |
| ` crates/wxml-delivery/src/bin/wxml-lsp-install.rs:910-932` | `verify_package` 的 schema 1 分支调用 `parse_index`/`verify_archive` | 随 15 个符号删除；schema 1 只保留只读 `current/verify/env/list` |
| ` crates/wxml-delivery/src/bin/wxml-lsp-install.rs:990-1000` | `print_install_result` 调 `check_binary`/`check_backend` | 与 `install_and_report` 一起删除 |
| ` crates/wxml-delivery/src/package.rs:380-402,492-507` | `normalized_mode`/`copy_file` 被 v2 复用（:1248/:1770） | **不能删**；只删 v1 块 39-379、403-491、508-665 |
| ` crates/wxml-delivery/src/manifest.rs:411-459` | `Release`/`Index` 仅 v1 index 使用 | 删除；但 `Manifest`/`ExpectedManifest` 仍需保留给 `verify_tree` 只读 |
| ` crates/wxml-delivery/src/acquire.rs`、` cache.rs` | 整模块只服务 v1 | 删除；`source.rs::fetch_archive`（:160）也随 v1 删除，保留 `fetch_archive_v2`（:264） |

### 6.4 v1 写路径的测试面

| 测试/辅助 | 依赖 | 删除 v1 写路径后的处理 |
| --- | --- | --- |
| ` crates/wxml-delivery/tests/support/mod.rs:18,113,134,160` | `build_package` + `BuildOptions` | 改成 **test-only** 静态 v1 fixture writer；不要把生产 `build_package` 留在 `#[cfg(test)]` |
| ` crates/wxml-delivery/tests/installer_flow.rs`（20 tests） | 24 处 `pack*` | 改为静态 v1 fixture；只保留只读/拒绝语义 |
| ` crates/wxml-delivery/tests/installer_security.rs`（13 tests） | 11 处 `pack*` | 改为静态 v1 index/archive fixture，或迁到 v2 index 安全测试 |
| ` crates/wxml-delivery/tests/packager_cli.rs`（3 tests） | v1 默认 `wxml-lsp-package` | 删除 `builds_a_package_and_rejects_bad_arguments`/`dry_run_writes_no_output`，改为 native CLI 断言 |
| ` crates/wxml-delivery/tests/f3_acceptance/support.rs:1075` | `build_package` 造 legacy fixture | 改为静态 v1 fixture writer |
| ` crates/wxml-delivery/tests/f3_kill_window/real.rs:847` | `build_legacy_release` | 改为静态 fixture；L3/L4 中断窗口不依赖 v1 packager |
| ` crates/wxml-delivery/tests/f3_acceptance/legacy.rs:41` | `build_legacy_release` | 同上 |
| ` crates/wxml-delivery/tests/f3_legacy_real.rs`（2 ignored） | 真实 v1 包 + `--backend node` 冒烟（:435-470） | 保留为历史验收，但改为只读归档包；删除 Node LSP initialize 断言 |
| ` crates/wxml-delivery/tests/native_cli.rs:736-805` | legacy v1 只读 | 保留；这正是要保住的只读边界 |
| ` crates/wxml-delivery/tests/native_delivery.rs:416,526-556` | 直接写 legacy manifest，不用 packager | 保留 |

### 6.5 v1 单元测试

| 文件 | 总测试 | 随 v1 删除/改写 |
| --- | ---: | --- |
| ` crates/wxml-delivery/src/package.rs` | 16 | 约 6 个 v1（`builds_verifies...`、`dry_run...`、`rejects_symlinked...`、`refuses_to_overwrite...`、`archive_and_manifest...`、`manifest_digest...`）；v2 测试保留 |
| ` crates/wxml-delivery/src/verify.rs` | 4 | `check_binary/check_backend` 相关约 3 个；`verify_tree` 保留 |
| ` crates/wxml-delivery/src/install.rs` | 5 | `install_tree`/`missing_backend...` 约 2 个；`explicit_binary`/root 校验保留 |
| ` crates/wxml-delivery/src/manifest.rs` | 10 | `index_upsert`/`rejects_bad_checksum_lines` 等 v1 index 测试删除；`Manifest` 只读解析测试保留 |
| ` crates/wxml-delivery/src/cache.rs` | 4 | 全部随模块删除 |
| ` crates/wxml-delivery/src/bin/wxml-lsp-install.rs` | 2 | 检查是否仍适用；help 文案会变 |

---

## 7. 脚本面

| 脚本 | 当前作用 | 删除/修改 |
| --- | --- | --- |
| ` scripts/p7-freeze.sh` | pre-delete 冻结/校验 | **不能原样保留**：`FROZEN_PATHS`（:123-140）含 `backend_node`、5 个旧测试、`backends/typescript`；`node_dependency_json`（:311-365）、`GATE_ONLY_ENV_JSON`（:506-510）、`REFERENCE_PATHS`（:608-635）都会因删除失败。建议：保留 JSON 的 `old_ignored_gates.gates[]`（`native_gates.rs:99` 运行时依赖），增加 `--check-ref p7-pre-delete` 只校验 tag；post-delete 用新脚本或新 schema |
| ` scripts/f3-legacy-real.sh` | 用生产 v1 packager + `npm ci` 造真实 v1 包，并跑 `--backend node` 冒烟（:190-309） | 删除构建段与 Node 冒烟；改为 `--package` 必填的归档包只读验收，或整体降级为历史证据脚本 |
| ` scripts/verify-no-node.sh` | F3-02 no-Node guard | `LSP_NATIVE` 逻辑（:396-402）当前要求 `main.rs` 含 `NodeBackend::new` 才 PASS；删除后必须改为“没有 NodeBackend 才 PASS”；残余说明（:477）同步 |
| ` scripts/native-gates.sh` | 35 gate runner | 当前 `NATIVE_ENGINE_GATES=23`、`NATIVE_EDIT_GATES=3`、`NATIVE_STDIO_GATES=9`（:53-56）；perf/stability retarget 后应变 37，并补 perf/stability 调用 |
| ` scripts/f3-zed-smoke.sh` | Zed 冒烟 | :173 的 “package --help still documents node as default” 提示删除；:498 的 `--backend node` 负断言保留 |
| ` scripts/build-release.sh` | v2 native release | **保持 Node-free**；无需删除，但 `docs`/README 的 Node 构建指令要改 |
| ` scripts/f3-exec-audit.sh` | execve 审计 | 保留；正控制需要名为 `node` 的二进制 |
| ` scripts/f3-real-release.sh` | 真实 release | 保留；:168 的 `unset WXML_*` 仍可作为 hygiene |
| ` scripts/f3-acceptance.sh` / `f3-kill-window.sh` | delivery 验收 | 保留；legacy fixture 改为静态 v1 包 |

---

## 8. CI 面

`.github/workflows/ci.yml` 没有 Node/npm 安装或 `setup-node` step。删除面只有文案和计数：

| 行 | 当前 | 修改 |
| ---: | --- | --- |
| ` .github/workflows/ci.yml:188` | `Run the 26 native gates` | 改为最终 gate 数（perf/stability retarget 后 37；当前脚本是 35） |
| ` .github/workflows/ci.yml:9` | `native_gates` 描述 | 保持 opt-in；可补充“real engine package on runner” |
| ` .github/workflows/ci.yml:30` | `cargo test --workspace --all-features` | 删除 Node 测试后仍保留；建议新增 `git grep` 静态 guard |
| ` .github/workflows/ci.yml:54-120` | `native-package` no-Node PATH | 保留；这是 release 的 no-Node 证据 |
| ` .github/workflows/ci.yml:156-207` | `native-gates` job | 保留；计数/名称同步 |

建议新增（非删除）一个轻量 CI step：

```bash
! git grep -nE 'backend_node|WXML_TEST_NODE|dist/server\.js|--backend[ =]node' -- \
  ':!docs/evidence/**' ':!docs/plans/**' ':!docs/adr/**'
```

注意：历史证据/计划/ADR 必须排除，否则 owner 要求保留的历史事实会被误判。

---

## 9. Zed 面

Zed 扩展已经**不转发**三个 Node 变量，但仍转发两个只被 Node 后端读取的变量：

| 位置 | 当前 | 修改 |
| --- | --- | --- |
| ` editors/zed/src/lib.rs:23` | `FORWARDED_ENV = ["WXML_ANALYZE_TIMEOUT_MS", "WXML_TEMPLATE_CHECK"]` | 删除两者，或改为空数组；native 当前不读取它们 |
| ` editors/zed/src/lib.rs:683-710` | `forwards_only_timeout_and_template_check` / `never_forwards_legacy_node_variables` | 改为“不转发任何 legacy/Node 变量”；保留 negative 断言 |
| ` editors/zed/README.md:48-54` | “local Node-backed development setup” + `npm ci` | 删除 |
| ` editors/zed/README.md:58-61` | 只转发两个 tuning 变量 | 改为“只转发 WXML_LSP_BIN；不转发任何 Node 变量” |
| ` editors/zed/README.md:172-173` | 两个变量表 | 删除或标注“native 不读取” |
| ` editors/zed/README.md:175` | “full semantics in main.rs” | 指向新的 native 说明 |

---

## 10. Cargo / features / cfg

| 项 | 结论 |
| --- | --- |
| Cargo features | 无 Node 相关 feature；`wxml-lsp` 无 `[features]` |
| Cargo deps | `backend_node` 只用 `std`/`serde_json`/`thiserror`；删除模块不需要改 `Cargo.toml` |
| workspace members | ` backends/typescript` 不在 Cargo workspace；删除不影响 workspace |
| `cfg` | Node 测试文件有 `#![cfg(unix)]`（`lsp_p7.rs:15`、`node_backend.rs` 等）；删除文件即可。`#[cfg(target_os = "linux")]` 的 native perf/stability 保留 |
| test targets | 删除 2 个 integration 文件后，integration target 数减少；没有脚本断言精确 target 数 |

---

## 11. oracle / fixture / ledger / gate registry（隐藏引用）

### 11.1 必须保持可解析的冻结文件

| 文件 | 读取者 | 约束 |
| --- | --- | --- |
| ` docs/evidence/data/legacy-p7-freeze.json` | `native_gates.rs:99` `P7_FREEZE_RELATIVE_PATH`、`load_p7_actions()` | 必须继续含 `old_ignored_gates.gates[]` 和每个 gate 的 `p7_action`；不能改名/删除。删除后可把 `blocking_state.deletion_allowed` 改为 true，但 `--check` 的精确比较会失败，需同步脚本 |
| ` docs/plans/tsgo-legacy-oracle-manifest.json` | `oracle_manifest.rs:194`、`ledger.rs`、`fixture_hash.rs` | 不要重新生成；`FROZEN_MANIFEST_SHA256`（:45）和 `ORACLE_BASELINE_HEAD` 是硬 pin |
| ` docs/plans/tsgo-test-ledger.json` | `ledger.rs` 计数、`native_gates.rs` | 删 12 个 gate 加 `old_test_removed: true`；silent gates 清空；不要改 case counts 来“凑”P7-1 |

### 11.2 必须修改的 gate registry

| 位置 | 内容 | 删除后 |
| --- | --- | --- |
| ` crates/wxml-lsp/tests/native_parity/native_gates.rs:93` | `IGNORED_GATE_COUNT = 23` | **保持 23**；它是 ledger 条目数，不是“仍在树里的旧测试数” |
| ` crates/wxml-lsp/tests/native_parity/native_gates.rs:95-97` | `P7_DELETE_GATE_COUNT=12`、`P7_RETARGET_GATE_COUNT=11` | 保持；freeze JSON 仍是权威 |
| ` crates/wxml-lsp/tests/native_parity/native_gates.rs:105-133` | `P7_DELETE_GATE_IDS` / `P7_RETARGET_GATE_IDS` | 保持；用于拒绝 retarget gate 被删 |
| ` crates/wxml-lsp/tests/native_parity/native_gates.rs:1056` | `semantic_tools_silent_returns...` | **删除**；ledger `silent_skip_gates` 改 0 |
| ` crates/wxml-lsp/tests/native_parity/native_gates.rs:1281` | `native_edit_guard_honors_both_opt_in_flags` | **删除**；`common/native_edit.rs` 删除 |
| ` crates/wxml-lsp/tests/native_parity/native_engine_real.rs:56,63,127` | `NATIVE_GATE_FILE`、`GATE_REPLACEMENTS`、`IGNORED_GATE_TESTS` | 保持；23 个 native replacement 不随旧测试删除 |
| ` crates/wxml-lsp/tests/native_parity/native_gates.rs:532` | `old_test_removed` 要求 Frozen（反向测试在 `:1474`） | 保持；这正是 Phase 1 必须先做 Frozen 的原因 |
| ` crates/wxml-lsp/tests/native_parity/ledger.rs:105,119` | `ALLOWED_STATUSES`、`ALLOWED_RETIREMENT_SCOPES` | 若 owner 走 C-lite/coverage waiver，必须新增 scope 并改 G5；否则不要动 |

### 11.3 ledger 字段

| 字段 | 当前 | 删除后 |
| --- | --- | --- |
| `.ignored_rust_tests[]` | 23 条，`old_test_removed` 全缺省 false | 12 条 delete 加 `true`；11 条 retarget 保持 false |
| `.silent_skip_gates[]` | 2 条（G-P5-E2E-22/23） | 变 0；删除 `native_gates` 的对应 gate |
| `.silent_skips.rust_silent_skips[]` | 3 条（含 tsc 分支） | 变 0 |
| `.counts.statuses` | green/planned/retired | 删除测试不改 case 计数；只有 waiver 才改 |
| `.migration_evidence` | implementation_head 等 | 删除完成后回填新 head；不要伪造 planned=0 |
| `.v1_delivery.write_paths` | 15 条 | 不适用（该字段在 freeze JSON，不在 ledger） |

---

## 12. release packaging

| 项 | 状态 | P7 动作 |
| --- | --- | --- |
| ` scripts/build-release.sh` | 已只产 schema v2 / Node-free | 保持；不要回退到 v1 |
| ` crates/wxml-delivery/src/bin/wxml-lsp-package.rs` | 默认无子命令仍是 v1 | 删除 v1 分支；只保留 `provision`/`native`，未知/缺失子命令失败 |
| ` crates/wxml-delivery/src/bin/wxml-lsp-install.rs` | `install`/`upgrade`/`rollback`/`verify-package` 仍有 v1 分支 | 删除写分支；保留 `list/current/verify/env` 只读与“legacy 不是 rollback 目标”的拒绝 |
| ` crates/wxml-delivery/src/package.rs` | v1 块 39-665 + v2 块 666+ | 只删 v1 块；保留 `copy_file`/`normalized_mode` |
| ` crates/wxml-delivery/src/acquire.rs`/`cache.rs` | 纯 v1 | 删除模块与 `lib.rs:20,24` |
| ` crates/wxml-delivery/src/verify.rs` | `verify_tree` 保留；`check_binary/check_backend/verify_archive` v1 | 删除 v1 函数；保留只读 `verify_tree` |
| ` crates/wxml-delivery/src/manifest.rs` | `Manifest`/`ExpectedManifest` 供只读；`Index`/`Release`/`parse_index` 供 v1 index | 删除 index 类型/解析；保留 manifest 只读解析 |
| ` crates/wxml-delivery/tests/*` | v1 fixture builder 大量使用 `build_package` | 改静态 v1 fixture writer；不要保留生产写路径 |
| ` scripts/f3-legacy-real.sh` | 依赖 v1 packager + npm + node | 改归档包只读，或降级为历史脚本 |
| ` assets.lock.json`、` crates/wxml-delivery/tests/fixtures/miniprogram-api-typings-5.3.1` | native 引擎/typings pin，不是 Node 后端 | **保留**；不得随旧 TS 树删除 |
| ` docs/evidence/data/type-bridge-c1/matrix.json`、` perf.json` | TypeScript 6.0.3 历史 oracle | **保留**；只作为历史投影，不参与 native 运行 |

---

## 13. 文档与 ADR 状态

### 13.1 必须改状态

| 文件 | 当前 | 删除完成后 |
| --- | --- | --- |
| ` docs/adr/0008-rust-tsgo-no-node-backend.md:4` | `Status: proposed` | `accepted`（仅在 G5/G6/G7 关闭后）；若走 C-lite，明确写 “scope waiver, not full parity” |
| ` docs/adr/0008-rust-tsgo-no-node-backend.md:106-112` | “`--backend node` 保留、P7 未开始” | 改为“Node 已删除；P7-1 以 waiver 收口/未收口” |
| ` docs/adr/README.md:18,20` | 0008 行 `proposed` + “不得写成 accepted” | 改 `accepted`；删除过期的“不得”句 |
| ` docs/adr/README.md:30` | 维护约定仍写 “当前状态为 proposed” | 同步 |
| ` docs/README.md:31` | “0008 仍为 proposed” | 同步 |
| ` docs/reference/type-bridge-c1.md:3` | C1 `accepted`，但 native 不实现 | 改为 `superseded/historical`；明确只保留为历史 Node 桥契约 |
| ` docs/reference/tsgo-test-contract.md:150-156,171-184` | 23 个 ignored + 2 个 silent return 必须保留 | 改为“删除完成后为 0 个 silent；23 个 gate 中 12 个 removed、11 个 retarget” |
| ` docs/reference/tsgo-delivery-contract.md:1248,1328-1330,1362` | F3-09 依赖真实 v1 packager/Node 冒烟 | 加历史说明：v1 写路径已删除，复现需 `p7-pre-delete`/`e4fed4a` 归档包 |
| ` docs/reference/wxml-language-feature-baseline.md:50,60-61,94-96` | 链接 `real_node.rs`/`stdio_type_loop.rs`/`backends/typescript/test`，“Node 重启重放” | 改链到 native gates；删 Node 描述 |
| ` docs/reference/type-diagnostics.md:212-218` | `npm ci` + `wave5_acceptance` | 改 native 命令 |
| ` tests/fixtures/README.md:233-240` | `npm ci` + `wave5_acceptance` | 改 native 命令 |
| ` docs/plans/2026-11-17-rust-tsgo-migration.md:245-247,487,800,817` | P7 未开始/删除阻塞 | 标记历史；新增 P7 完成快照 |
| ` docs/plans/2026-11-17-tsgo-test-ledger.md:66,213-243` | 23 ignored + 2 silent + 旧 gate 表 | 标注 removed/retarget；保留历史行 |
| ` docs/evidence/README.md:70,85-86,99-100,121` | 索引仍指向 Node 证据 | 加 P7 删除证据行；历史行保留但标注 historical |

### 13.2 当前面向用户的 README（任务要求不改，但删除实施时必须改）

| 文件 | 行 | 内容 |
| --- | ---: | --- |
| ` README.md` | 24 | 状态说“P7 删除未开始，Node 尚未移除” |
| ` README.md` | 18 | 目录表“Node 类型后端” |
| ` README.md` | 37-40 | `npm ci && npm run check && npm run build && npm test` |
| ` README.md` | 58-64 | 5 条 `WXML_TEST_NODE` 真机命令 |
| ` README.md` | 72-74 | “构建服务与后端”Node 步骤 |
| ` README.md` | 79-81 | `WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` |
| ` README.md` | 96-102 | 5 个 Node env 表 |
| ` README.md` | 514-524 | 目录树仍列 `backends/typescript` 和 “Node 后端” |
| ` crates/wxml-lsp/README.md` | 3,8-12,22-29,34-38,51-52,83-94 | C1.1 Node adapter、Node 运行/环境/测试 |
| ` crates/wxml-delivery/README.md` | 9,96,110,119-120,132 | v1 写路径/legacy 说明 |
| ` editors/zed/README.md` | 48-54,58-61,172-173 | Node 开发设置与两个死变量 |

> 历史证据（`docs/evidence/**`）和已冻结计划（`docs/plans/2026-11-16-*`）保留原事实，
> 只加历史标识；不要把历史记录改写成“从未有 Node”。

---

## 14. 隐藏引用速查（按文件）

| 文件:行 | 引用 | 删除前必须处理 |
| --- | --- | --- |
| ` crates/wxml-lsp/src/lib.rs:44,48` | `pub mod backend_node` / Node 注释 | 删除模块时同步 |
| ` crates/wxml-lsp/src/main.rs:10,41,100,160,175,206` | Node import/分支/serve_node | 删除 |
| ` crates/wxml-lsp/src/backend_tsgo/mod.rs:12,227` | Node 路径/legacy env 注释 | 改写 |
| ` crates/wxml-lsp/src/project_host.rs:45` | C1 Node backend 注释 | 改写 |
| ` crates/wxml-lsp/tests/cli.rs:50-52,224` | help/`--backend node` 正向测试 | 改/删 |
| ` crates/wxml-lsp/tests/native_parity/native_gates.rs:93-133,1056,1281` | gate 计数、P7 id、silent gate、native_edit guard | 部分删除 |
| ` crates/wxml-lsp/tests/native_parity/native_engine_real.rs:56,63,127` | gate registry | 保持 |
| ` crates/wxml-lsp/tests/native_parity/oracle_manifest.rs:37,45,813` | legacy root/pin/Frozen | 保持 |
| ` crates/wxml-lsp/tests/native_parity/fixture_hash.rs:71-82` | archived legacy trees | 保持 |
| ` crates/wxml-lsp/tests/native_parity/ledger.rs:2114` | simulated deletion | 保持 |
| ` crates/wxml-lsp/tests/native_parity/real_matrix.rs:78-80,1838` | samples fixture / typings fallback | 改路径 |
| ` crates/wxml-lsp/tests/native_lsp_document_symbols/support.rs:145,390,429` | typings fallback | 改路径 |
| ` crates/wxml-backend-tsgo/tests/f1_hover.rs:427` | typings fallback | 改路径 |
| ` crates/wxml-backend-tsgo/tests/asset_loader.rs:821,875,926` | node_modules fixture | 改路径/删 ignored 测试 |
| ` crates/wxml-backend-tsgo/tests/type_assets.rs:634` | node_modules fallback | 改路径 |
| ` scripts/p7-freeze.sh:123-140,311-365,486-510,525-534,608-635` | 冻结路径/gate/env/v1 锚点 | 重写或分模式 |
| ` scripts/verify-no-node.sh:396-402,477` | 要求 `serve_node` 存在 | 反转为“无 Node 才 PASS” |
| ` scripts/native-gates.sh:53-56` | 23+3+9=35 | retarget 后 37 |
| ` scripts/f3-legacy-real.sh:190-309` | npm/node/v1 packager | 改归档模式 |
| ` scripts/f3-zed-smoke.sh:173` | “node (default)”提示 | 删除 |
| ` .github/workflows/ci.yml:188` | “26 native gates” | 改最终数 |
| ` editors/zed/src/lib.rs:23,683-710` | 两个死变量转发 | 删除/改测试 |
| ` crates/wxml-delivery/tests/no_node_guard.rs:497-518` | 要求 `serve_node` 存在 | 反转为“没有 NodeBackend 才 PASS” |
| ` crates/wxml-delivery/tests/support/mod.rs:18,113,134,160` | `build_package` | 改静态 v1 fixture |
| ` crates/wxml-delivery/tests/f3_acceptance/support.rs:1075` | `build_package` | 改静态 fixture |
| ` crates/wxml-delivery/tests/f3_kill_window/real.rs:847` | `build_legacy_release` | 改静态 fixture |
| ` crates/wxml-delivery/src/lib.rs:20,24` | `pub mod acquire/cache` | 删除模块 |
| ` crates/wxml-delivery/src/bin/wxml-lsp-install.rs:430-444` | `rollback` v1 安装 | 随 `install_tree` 删除 |

---

## 15. 最小安全删除顺序

> 原则：**identity first, tests second, code third, delivery fourth, docs last**。
> 每个阶段单独 commit；共享工作树里不要与并行矩阵/retarget 改动混提。

### Phase 0 — 固定身份与 waiver（不删除）

**动作**

1. 先把并行 retarget 收口：stdio 已在 `b0f6cc1`；perf/stability 的 retarget 必须先
   单独提交并通过，再开始删除。
2. **先 re-freeze**：当前 `docs/evidence/data/legacy-p7-freeze.json` 的
   `requested_head` 仍是 `befba03`，而 `b9291a0`/`b0f6cc1` 已改冻结路径，
   `scripts/p7-freeze.sh --check` 当前 exit 2。用
   `P7_REQUESTED_HEAD=b0f6cc1 scripts/p7-freeze.sh --output <repo外路径>` 生成，
   审阅后更新 JSON 并单独提交；重复到 `--check` 通过。
3. 在 re-freeze commit 上打 tag：`git tag p7-pre-delete`。
4. 在 tag 上跑 `scripts/p7-freeze.sh --check`，保存输出为 pre-delete 证据。
5. 记录 owner 的 C-lite waiver：在
   `docs/evidence/2026-11-20-tsgo-p7-scope-decision.md` 之外落一份可引用的
   `retirement_decision`/scope 记录；若使用 coverage waiver，扩展
   `ledger.rs:119` 的 `ALLOWED_RETIREMENT_SCOPES` 和
   `docs/reference/tsgo-test-contract.md` 的 G5 文案。
   并行会话另写了一份未提交的
   `docs/evidence/2026-11-20-tsgo-p7-ledger-waiver-policy.md`（B+ 四终态
   提案）；无论采用 B+ 还是 C-lite，只要意图让 `planned=0`，都必须在
   Phase 2 之前先改 `ALLOWED_STATUSES`、counts、G5 和 freeze JSON，
   因为 `native_parity` 会锁定 ledger 形状；本删除顺序本身不依赖某一种
   waiver 形状。
6. 决定 `p7-freeze.sh` 的去向：保留 `legacy-p7-freeze.json` 的
   `old_ignored_gates.gates[]`（`native_gates.rs` 运行时依赖），给脚本增加
   `--check-ref p7-pre-delete`；post-delete 不再用 `--check`。

**验证**

```bash
# 1) 在提交前验证新 freeze JSON
scripts/p7-freeze.sh --check
cargo test -p wxml-lsp --test native_parity   # 期望 78 passed / 29 ignored
# 2) 提交 re-freeze 后打 tag
git tag p7-pre-delete
```

**回滚点**：`p7-pre-delete` tag；Phase 0 不改生产代码，只改 freeze 证据/waiver 文案，
回滚就是丢弃 re-freeze commit 与 tag。

### Phase 1 — oracle 物理 Frozen

**动作**

1. 在仓库外备份 `backends/typescript/test`（`git archive p7-pre-delete --
   backends/typescript/test` 或 tar 到 `/tmp/opencode`）。
2. `git rm -r backends/typescript/test`。
3. 不要改 `docs/plans/tsgo-legacy-oracle-manifest.json`；`ledger.rs`/
   `fixture_hash.rs` 会走 `LegacySourceMode::Frozen`。

**验证**

```bash
cargo test -p wxml-lsp --test native_parity   # 仍应 78 passed / 29 ignored
```

**回滚点**：`git restore --source=p7-pre-delete -- backends/typescript/test`。

### Phase 2 — 删除 12 个 delete gate（测试 + ledger 同 commit）

**动作**

1. `git rm crates/wxml-lsp/tests/real_node.rs crates/wxml-lsp/tests/wave5_acceptance.rs`。
2. 在 `docs/plans/tsgo-test-ledger.json` 给 12 个 gate 加
   `"old_test_removed": true`（G-P5-E2E-01..05、15..21）。
3. 更新 `native_gates.rs` 顶部 “No gate is removed” 注释与 `IGNORED_GATE_COUNT`
   注释；**不要改 23 这个数**。

**验证**

```bash
cargo test -p wxml-lsp --test native_parity
cargo test -p wxml-lsp
```

**回滚点**：Phase 2 commit 整体 revert（ledger flag 与删除必须原子）。

### Phase 3 — retarget 11 个 gate

**动作**

1. 确认 stdio 9 个已在 `b0f6cc1` retarget（本阶段只做 perf/stability）。
2. `performance_bench.rs`：去掉 `NodeBackend`/`SlowTransport`/`node --version`；
   保留 cancel/occupancy 断言，改用 native session 或本地慢后端。
3. `q2_stability.rs`：去掉 `NodeBackend`/`node_children`；保留 10 轮 server
   生命周期、RSS、关闭/重开断言。
4. `scripts/native-gates.sh`：35 → 37，补 perf/stability 调用。
5. 更新 `scripts/p7-freeze.sh` 的 `node_dependency_json` 和 freeze JSON。
6. ledger 的 `old_reason`/`proposed_command` 同步为 native 命令。

**验证**

```bash
cargo test -p wxml-lsp --test performance_bench -- --ignored --test-threads=1
cargo test -p wxml-lsp --test q2_stability -- --ignored --test-threads=1
scripts/native-gates.sh --package /tmp/opencode/tsgo-native-pkg
```

**回滚点**：Phase 3 commit；旧 Node 测试此时仍全部存在，可随时 revert。

### Phase 4 — 迁移/删除其余 Node 测试

**动作**

1. `node_backend.rs` 整文件删除（纯 C1 fake）。
2. `q2_support/mod.rs` 换成非 Node 测试后端；`q2_scheduler.rs`、
   `q2_recovery.rs` 跟着迁移；`q2_recovery` 的 2 个 `StdioNode` 真实子进程测试
   改写为 native supervisor 测试或删除。
3. `semantic_tools.rs`：删除 2 个 real-node 测试；39 个 fake-C1 测试迁移到
   `MockBackend` 或新的非 Node fake；删除 `common/native_edit.rs`；
   同步删除 `native_gates.rs:1056/1281` 两个 gate。
4. `lsp_p7.rs`、`full_feature_matrix.rs`、`multi_app_projects.rs`、`zed_sync.rs`
   把 `NodeBackend` fake transport 换成非 Node 测试后端。
5. ledger：`silent_skip_gates=[]`、`silent_skips.rust_silent_skips=[]`。

**验证**

```bash
env -u WXML_TEST_NODE -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  cargo test -p wxml-lsp
git grep -n 'backend_node' -- crates/wxml-lsp/tests
```

**回滚点**：Phase 4 commit；此时 `backend_node` 仍可编译，revert 即可恢复。

### Phase 5 — 删除 `backend_node` 运行时

**动作**

1. `git rm -r crates/wxml-lsp/src/backend_node`。
2. `main.rs`：删除 import、`Action::Node`、`BackendChoice::Node`、`serve_node`、
   help/env 段和 parse 测试。
3. `lib.rs`：删除 `pub mod backend_node` 与 Node 注释。
4. `cli.rs`：help 断言与 explicit-node 测试改/删。
5. ` crates/wxml-delivery/tests/no_node_guard.rs:497-518`：改为
   “main.rs 不存在 NodeBackend 才 PASS”。
6. `scripts/verify-no-node.sh:396-402`：同样反转；:477 残余说明删除。
7. `backend_tsgo/mod.rs`/`project_host.rs` 注释改写。

**验证**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
git grep -n 'backend_node\|NodeBackend\|serve_node' -- crates
scripts/verify-no-node.sh --installer <installer> --root <root>
```

**回滚点**：Phase 5 commit；删除前 `backend_node` 已无测试调用者。

### Phase 6 — 删除旧 TypeScript 树

**动作**

1. 补齐 `real_matrix` samples 的 2 个 tsconfig，或改写 “byte-for-byte” 声明。
2. 把 native 测试的 typings fallback 改为 `TSGO_TYPINGS_ROOT`/`TSGO_ASSET_ROOT`。
3. `git rm -r backends/typescript`；本地删除未跟踪 `backends/typescript/dist`。
4. 更新 `docs/reference/*`、`tests/fixtures/README.md`、README 中的 npm 指令。

**验证**

```bash
cargo test --workspace --all-features
cargo test -p wxml-backend-tsgo --test asset_loader
git grep -n 'backends/typescript' -- ':!docs/evidence/**' ':!docs/plans/**' ':!docs/adr/**'
```

**回滚点**：`p7-pre-delete` tag；旧树可从 tag 恢复。

### Phase 7 — 删除 v1 delivery 写路径

**动作**

1. 删除 `acquire.rs`、`cache.rs` 及 `lib.rs:20,24`。
2. 删除 `package.rs` v1 块（39-379、403-491、508-665），保留
   `normalized_mode`/`copy_file`/`build_native_package`。
3. 删除 `install.rs::install_tree`；保留 `verify_installed`/`switch`。
4. 删除 `verify.rs::{check_binary,check_backend,verify_archive}`；保留 `verify_tree`。
5. 删除 `manifest.rs::{Index,Release,parse_index}`；保留 `Manifest` 只读解析。
6. `wxml-lsp-package` 只接受 `provision|native`；`wxml-lsp-install` 删除
   `install/upgrade/rollback/verify-package` 的 v1 分支。
7. delivery 测试：`support::pack*`、`f3_acceptance::build_legacy_release`、
   `f3_kill_window` legacy fixture、`packager_cli` v1 用例改为静态 v1 fixture
   或删除；`f3_legacy_real` 改为归档包只读验收。
8. `scripts/f3-legacy-real.sh` 改为 `--package` 必填、无 npm/node。

**验证**

```bash
cargo test -p wxml-delivery
cargo test --workspace --all-features
scripts/build-release.sh --dry-run --output /tmp/opencode/p7-release-dryrun
scripts/f3-acceptance.sh --skip-build   # 若有预置资产
```

**回滚点**：Phase 7 commit；v1 写路径删除前，先确保只读 legacy 测试仍覆盖
`current/verify/env/list` 和 rollback 拒绝。

### Phase 8 — 文档/ADR/CI + full revalidation

**动作**

1. ADR-0008 → accepted；`docs/adr/README.md`、`docs/README.md` 同步。
2. `type-bridge-c1.md` 标 historical/superseded。
3. README/crates README/Zed README/合同/功能基线/测试契约更新。
4. CI `native-gates` 文案改最终数；可选加 no-Node 静态 guard。
5. `p7-freeze` 历史模式与 post-delete 证据落盘。
6. 更新 `docs/evidence/README.md` 索引（若 owner 允许；本次审计不改）。

**验证（完整）**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo doc --workspace --no-deps
cargo test -p wxml-lsp --test native_parity
scripts/native-gates.sh --package /tmp/opencode/tsgo-native-pkg
scripts/build-release.sh --output /tmp/opencode/p7-release-a
scripts/build-release.sh --output /tmp/opencode/p7-release-b
cmp /tmp/opencode/p7-release-a/index.json /tmp/opencode/p7-release-b/index.json
git grep -nE 'backend_node|WXML_TEST_NODE|--backend[ =]node' -- \
  ':!docs/evidence/**' ':!docs/plans/**' ':!docs/adr/**'
```

**回滚点**：`p7-pre-delete`；每个 Phase 单独 commit，最终验收失败时按 Phase revert。

---

## 16. 停止条件

出现以下任一情况，停止删除并回到上一阶段：

1. `native_parity` 在 Phase 1 后不是 78 passed / 29 ignored（oracle 没有真正 Frozen）。
2. 12 个 delete gate 的 `old_test_removed` 与文件删除不在同一 commit，导致
   “still carries the ignore attribute” 或 “is missing”。
3. 11 个 retarget gate 中任何一个被标记 removed
   （` crates/wxml-lsp/tests/native_parity/native_gates.rs:1572`
   `retarget_cannot_be_marked_removed` 会硬失败）。
4. `backend_node` 仍有活跃引用（`git grep backend_node` 非空）就删除模块。
5. 旧 TS 树删除后，`real_matrix`/`asset_loader`/`f1_hover`/`type_assets` 仍默认读
   `backends/typescript/node_modules`，且没有 `TSGO_TYPINGS_ROOT` 替代。
6. `p7-freeze --check` 在 post-delete HEAD 上被当作 gate 使用（它只对 pre-delete tag 有效）。
7. v1 写路径删除后，`list/current/verify/env` 的 legacy 只读测试不再通过。
8. `scripts/build-release.sh` 出现 Node 执行、JS 文件或 `node_modules`。
9. ADR-0008 在 G5/G6/G7 未关闭、waiver 未书面化时被写成 `accepted`。

---

## 17. 本文件不声称

1. 不声称 P7 删除已完成；本文只给出删除面与顺序。
2. 不声称 P7-1 已归零；`planned` 仍存在，C-lite 只是 owner-scoped waiver。
3. 不声称 23 个 gate 已 `mandatory_green`；它们仍是 opt-in `gate_green`。
4. 不声称 `backend_node` 可以现在删除；必须先完成 Phase 1–4。
5. 不声称旧 TS 树可以现在删除；`samples` 复制仍有 2 个文件缺口，typings fallback 未改。
6. 不声称 v1 写路径可以只删 15 个符号；`rollback`/`verify-package`/`print_install_result`
   还有未冻结分支。
7. 不声称 `p7-freeze.sh --check` 在删除后仍可通过；它必须改成历史 tag 模式或新 schema。
8. 不声称本文件修改了任何代码/测试/ledger/README/脚本/CI/ADR；本文件只新增本文件。
9. 未运行任何测试；所有行号来自 `b0f6cc1` 的只读读取。

---

## 18. 只读复核记录

```text
git rev-parse --short HEAD                         -> b0f6cc1
git log --oneline -5                               -> b0f6cc1, b9291a0, befba03, 1c89338, 74669c3
git status --porcelain                             -> 仅未跟踪 evidence 文档
wc -l crates/wxml-lsp/src/backend_node/*.rs        -> 7,589
grep -c '#\[test\]' backend_node/*                 -> 102
for tests/*.rs (Node imports)                      -> 13 roots / 111 tests
git ls-files backends/typescript                   -> 99 tracked
git ls-files backends/typescript/test              -> 22
git ls-files backends/typescript/samples           -> 35
find real_matrix/samples -type f                   -> 33（缺 2 个 tsconfig）
jq v1_delivery.write_paths                         -> 15 delete / 3 keep
jq old_ignored_gates.gates                         -> 12 delete / 11 retarget
grep P7_FREEZE_RELATIVE_PATH native_gates.rs       -> 运行时读取 freeze JSON
grep WXML_ANALYZE_TIMEOUT_MS/WXML_TEMPLATE_CHECK   -> 仅 backend_node 读取，Zed 仍转发
grep NodeBackend::new no_node_guard.rs/verify-no-node.sh -> 静态要求 serve_node 存在
```
