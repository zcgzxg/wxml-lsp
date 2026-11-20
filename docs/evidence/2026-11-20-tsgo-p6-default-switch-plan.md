# P6 默认切换 + P7 去 Node/TS 实施设计（2026-11-20）

## 0. 身份、边界与当前快照

- Worktree：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`。任务给定 HEAD `6466ff3`；读取/写入期间并行会话已把 HEAD 推进到 `4140e9b`（`3220362` event handler hover、`4fd45bd` non-literal registrations、`4140e9b` symbol query split）。
- 性质：**只读设计切片**。本文之外不修改生产代码、测试、Cargo、台账、合同、CI、脚本或 Zed；不 commit；不等同于 P6/P7 完成声明。
- 工作树在读取时还有并行未提交改动（不是本文产生的）：
  - native symbols/overlay replay 的生产代码与测试：`crates/wxml-backend-tsgo/src/script/{references,symbol_facts,mod}.rs`、`symbol_query.rs`/`this_scope.rs`、`crates/wxml-lsp/src/backend_tsgo/{input,mod,tests}.rs`、`tests/native_parity/{native_engine_real,native_gates,native_lsp}.rs`、`native_measure.rs`；
  - perf/stability gate 提升：`tests/native_parity/native_measure.rs`、[2026-11-20-tsgo-p6-perf-stability.md](2026-11-20-tsgo-p6-perf-stability.md)（并行证据）；
  - E2E-20 与 F3-06/09 的并行设计：[2026-11-20-tsgo-e2e20-component-binding-plan.md](2026-11-20-tsgo-e2e20-component-binding-plan.md)、[2026-11-20-tsgo-f3-kill-window-plan.md](2026-11-20-tsgo-f3-kill-window-plan.md)。
- 因此本文区分两类数字：
  - **任务给定 HEAD `6466ff3`**：ledger `36 green / 267 planned / 36 retired`；23 个旧 Node gate 中 **16 `gate_green` / 7 `required_not_migrated`**。
  - **当前工作树（含 `3220362`/`4fd45bd`/`4140e9b` 与未提交 gate 提升）**：gate 提升到 **18 `gate_green` / 5 `required_not_migrated`**；case 计数不变。event hover 与 non-literal 修复已经提交，不再是“未提交生产代码”。
- F3 当前状态按 `docs/reference/tsgo-delivery-contract.md` §9.5：**13 closed / 2 partial / 1 blocked**（F3-06/09 partial，F3-14 blocked）。
- 本文没有运行 `cargo test`、`npm`、`strace`、Zed 或安装/升级/回滚；所有“通过/失败”均引用既有证据，不新增运行事实。
- 行号取自读取时的工作树（含上列未提交改动）；提交前必须按最终 HEAD 重新核对，不能把本文行号当作冻结证据。

**结论先行：**

1. 当前默认入口是 `crates/wxml-lsp/src/main.rs:88` → `serve_real()`（`:182`）→ `NodeBackend::new`（`:184`）。`--backend native` 已经是严格 opt-in（`:196`），但默认 `--stdio` 仍是 Node；原生失败**没有**隐式 Node fallback。
2. 默认切换在代码层已经“可接线”：引擎发现、沙箱、类型资产、schema v2 打包器、v2 安装/升级/回滚、离线缓存都存在；真正缺的是 **release 构建仍走 v1/Node、23 个 gate 未全部 mandatory/reviewed、F3-14/F3-06/F3-09 未关闭、P7 台账 267 planned 未归零**。
3. 不能把“实现 switch”与“关闭 G5/G6/G7”混为一谈。P6-8 可以按本文顺序落地并验证，但默认 flip 的合并条件必须明确：要么 G5 先关闭，要么留下书面 G5 waiver。

---

## 1. 当前默认 Node 的精确入口、回退与测试环境变量

### 1.1 入口链路（当前 HEAD）

| 位置 | 事实 |
| --- | --- |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:88` | `--stdio` 无 `--backend` → `Action::Stdio` → `serve_real()` |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:162` | `--backend node` 也映射到 `Action::Stdio`，与默认完全同路 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:182` | `serve_real()` 构造 `ProjectState` + `NodeBackend` |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:184` | `NodeBackend::new(NodeBackendConfig::from_env(), host)` |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/config.rs:155` | `from_env()` 读取 `WXML_NODE`、`WXML_TYPE_BACKEND`、`WXML_BACKEND_DIR`、`WXML_ANALYZE_TIMEOUT_MS`、`WXML_TEMPLATE_CHECK` |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/config.rs:229` | 默认只探测 executable-relative `backends/typescript/dist/server.js` 或 `wxml-type-backend/server.js`，从不探测 cwd |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/mod.rs:134` | `NodeBackend::new` **惰性**：启动时只记录 `unavailable_reason`，不 spawn |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/process.rs:248` | 第一次类型请求才 `Command::new(node) -- server.js`，`current_dir(backend_dir)` |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:196` | `--backend native` → `serve_native()` → `run_stdio_native()` |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:163` | `run_stdio_native()` 先 `discover_from_current_exe()`，再加载/校验类型资产，再创建 adapter；失败在 `initialize` 之前 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/engine.rs:137` | 原生发现从 `/proc/self/exe` 的 `<package>/bin/wxml-lsp` 推导包根，校验 manifest/`engine/lib/tsc` |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/environment.rs:163` | 类型资产只从 verified install 加载；缺失/篡改/不安全一律失败 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:44` | `pub mod backend_node`（为了测试可见） |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:49` | `pub(crate) mod backend_tsgo`；注释仍写“default `--stdio` path still constructs `backend_node::NodeBackend`” |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:73` | 只 re-export `run_stdio_native` 与四类启动错误 |

### 1.2 当前默认路径的实际行为

| 场景 | 当前行为 |
| --- | --- |
| `--stdio`，安装布局里有 `server.js` + Node | 启动时 LSP 正常；第一次类型请求 spawn `node -- server.js` |
| `--stdio`，没有 `server.js`，但有 Node | 启动正常；类型请求返回 `Unavailable`（`node failed to start`/`node transport failed` 一类 reason），静态 WXML 功能仍工作 |
| `--stdio`，没有 Node、没有 `server.js` | 启动正常；类型请求 `Unavailable`；静态功能工作；不会因为缺 Node 而退出 |
| `--stdio --backend native`，没有安装/资产 | **在 LSP handshake 前失败退出**；stderr 是 path-free 的 `NativeStartupError`；不会构造 Node |
| `--stdio --backend native`，安装/资产损坏 | 失败退出；不降级到 Node，也不启动半可信 session |
| `--stdio --mock` | 进程内 `MockBackend`；无项目 host、无 Node |

关键点：**Node 默认是“启动不失败、类型能力降级”的模型；原生 opt-in 是“启动 fail-closed”的模型。** 两者不是同一个产品语义。P6-8 必须明确选择其中一个作为默认，不能把两者简单对调后不处理静态-only 场景。

### 1.3 测试/开发环境变量

| 变量 | 谁读取 | 作用 | 备注 |
| --- | --- | --- | --- |
| `WXML_NODE` | `NodeBackendConfig::from_env` | Node 可执行名或绝对路径 | P6 后仅 `--backend node` 读取 |
| `WXML_TYPE_BACKEND` | 同上 | 绝对 `server.js` 路径 | 相对值会禁用 backend |
| `WXML_BACKEND_DIR` | 同上 | 绝对 backend 目录 | 相对值会禁用 backend |
| `WXML_ANALYZE_TIMEOUT_MS` | 同上 | Node 请求超时，默认 15000 | 原生当前**不读**这个变量 |
| `WXML_TEMPLATE_CHECK` | 同上 | `off`/`pragma`/`inherit`，默认 `pragma` | 原生当前硬编码 `Inherit`，见 `backend_tsgo/input.rs:727` |
| `WXML_TEST_NODE=1` | 23 个旧 gate 的 guard | 允许真实 Node gate 运行 | 仍需 `backends/typescript/dist/server.js` |
| `WXML_TEST_NATIVE=1` | native gate guard | 显式请求 `#[ignore]` native gate | 缺失时 ignored gate 运行会失败，不是 skip |
| `WXML_TEST_NATIVE_PACKAGE` | native gate fixture | 复用已存在的 package root | test-only，生产发现不读 |
| `WXML_REQUIRE_REAL_ENGINE=1` | native gate fixture | 缺 package/engine 时 hard fail | 是 required native job 的 fail-closed 开关 |
| `TSGO_BIN` / `TSGO_ASSET_ROOT` / `TSGO_TYPINGS_ROOT` | native gate fixture | 构造 disposable package | 引擎/资产必须逐字节匹配 pin |
| `WXML_LSP_BIN` | installer / Zed | 显式二进制优先 | server 本身不读 |
| `WXML_REQUIRE_LSP_NATIVE` | `no_node_guard.rs` | 要求 LSP 默认已 native | 当前 `lsp_native_status()` **永远不可能返回 PASS**，见 §3.1 |
| `WXML_REQUIRE_EXEC_AUDIT` | F3-02 ptrace test | 缺 ptrace/非 Linux 时失败关闭 | 只证明 exec 集合 |

### 1.4 默认测试套件中依赖“默认 Node/host”的文件

这些**不在** 23 个 gate 清单里，切换时容易漏：

| 文件 | 用法 | 切换后的风险 |
| --- | --- | --- |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/lsp.rs:866` | `custom_components_complete_from_using_components` 用 `--stdio` 默认后端 | 严格 native 启动会失败；host-only 默认可保留静态补全 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/multi_app_projects.rs:508` | 两个 `stdio_binary_*` 测试用默认 `--stdio`，断言 WXML 语法诊断 | 严格 native 启动会失败；host-only 可保留 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/component_intelligence.rs:38` | 默认 `--stdio`，并设置不存在的 `WXML_NODE`/`WXML_TYPE_BACKEND` 以强制 backend 不可用 | 严格 native 会失败；host-only 可保留原意 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/structural_editing.rs:42` | `--stdio --mock` | 不受影响 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/lsp.rs` 其余 | `--stdio --mock` | 不受影响 |

**这是推荐 host-only 默认回退的直接原因**：否则 P6-8 会先弄红默认测试套件，并让“只有静态能力”的用户无法启动。

---

## 2. 切换后 `--stdio` 无 Node 的行为（本文建议）

### 2.1 三种启动模式

| 模式 | 选择方式 | 行为 |
| --- | --- | --- |
| **Native（默认）** | `--stdio`，无 `--backend` | 先尝试原生 discovery/assets。成功 → 原生引擎；`NotInstalled` → **HostOnly**；其他 discovery/asset 错误 → fail closed |
| **Native（严格）** | `--stdio --backend native` | 任何 discovery/asset/sandbox 错误都在 `initialize` 前失败；不降级 |
| **Node（过渡）** | `--stdio --backend node` | 保持当前 Node 行为；标记 deprecated，P7-3 删除 |
| **Mock** | `--stdio --mock` | 进程内 mock；无 host、无引擎 |

**HostOnly 不是 Node fallback。** 它使用 `TsgoBackendAdapter` 的 legacy environment + 一个永远返回 `Unavailable` 的 session factory：不启动任何引擎进程，不读 `WXML_NODE`，不探测 `server.js`，只提供：

- 静态 WXML：语法诊断、格式化、结构、事件绑定诊断/定义、事件补全、symbols（Rust script facts）；
- 类型能力：`Unavailable`/`Unsupported`，带稳定 reason；不会伪造空成功。

HostOnly 不读取 `WXML_LSP_BIN`；F3-15 的“无效 `WXML_LSP_BIN` hard error”是安装器/Zed 的行为，不在 server 的 HostOnly 路径里。

### 2.2 切换后的行为矩阵

| 场景 | 默认 `--stdio` | `--backend native` | `--backend node` |
| --- | --- | --- | --- |
| v2 包安装完整，无 Node | 原生引擎；无 Node 进程 | 同左 | 不适用（无 `server.js`，类型 `Unavailable`） |
| 无安装、无 Node | HostOnly；静态功能可用；类型 `Unavailable` | 启动失败 | 启动正常；类型 `Unavailable` |
| 无安装、有 Node | HostOnly；**不会**使用 Node | 启动失败 | 使用 Node（过渡） |
| 安装存在但 manifest/engine/资产被篡改 | **启动失败**（不降级） | 启动失败 | 使用 Node（如果 `server.js` 存在） |
| 安装完整 + 设置 `WXML_NODE`/`WXML_TYPE_BACKEND` poison | 原生引擎；legacy 变量被忽略 | 同左 | legacy 变量生效 |
| `cargo run -- --stdio`（target/debug） | HostOnly（exe 不在 `<pkg>/bin`） | 启动失败 | 使用 Node（若可用） |
| Zed 用 v2 包 + `WXML_LSP_BIN` | 原生引擎 | 同左 | 不推荐 |

### 2.3 如何阻止隐式 Node fallback

1. **代码结构**：`serve_default()` 只调用 `run_stdio_native_default()`；`serve_native()` 只调用 `run_stdio_native()`；`NodeBackend` 只允许出现在 `serve_node()` 和 `backend_node` 内部。
2. **类型/错误**：`NativeStartupError` 不包含 `NodeBackend` 分支；HostOnly 也不构造 `NodeBackend`。
3. **source gate**：在 `native_gates.rs` 或 `no_node_guard.rs` 增加扫描：`main.rs` 的 `Action::Native`/default 分支不得出现 `NodeBackend`；`backend_tsgo/**` 不得引用 `backend_node`。
4. **运行时 tripwire**：PATH 放 `node`/`npm`/`npx`/`pnpm`/`yarn` shim；设置 legacy env poison；启动默认 `--stdio`（v2 包和 HostOnly 各一次），断言 shim 未被调用、`/proc` 无 node 子进程。
5. **安装器**：`verify_v2`/`type_assets::forbidden_runtime_path` 继续拒绝 `node_modules`、`server.js`、JS 文件；不因为默认切换而放宽。

### 2.4 为什么不在默认路径“native 失败后尝试 Node”

- 违反 F3-15/I5/I8 的“无 Node fallback”原则；
- 会把“安装损坏”变成“静默用 Node”，用户无法知道自己在跑哪个后端；
- 让 exec 审计无法给出确定集合；
- 让 P7 删除 Node 后行为再次变化。

HostOnly 是唯一的降级方向，且只在 `NotInstalled` 时发生；`PackageUnsafe`/`ManifestUnavailable`/`ManifestMismatch`/`Engine*`/`Environment*` 全部 fail closed。

---

## 3. 分步方案：P6-8 默认 native

### 3.1 P6-8a：默认选择器与 HostOnly

| # | 变更 | 文件 | 验收 |
| --- | --- | --- | --- |
| 8a-1 | `Action` 增加 `Node`；`--stdio` 默认 → `Default`；`--backend node` → `Node`；`--backend native` → `Native` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:88`、`:161` | `main.rs` 单测：默认 native、显式 node、显式 native、`--mock` 互斥 |
| 8a-2 | 新增 `serve_default()`：`run_stdio_native_default()` 成功 → 原生；`EngineDiscoveryError::NotInstalled` → `run_stdio_host_only()`；其他错误 → `Err` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:182` | 默认无安装时 LSP 可 initialize；显式 `--backend native` 仍失败 |
| 8a-3 | 新增 `run_stdio_host_only()` 与 `TsgoBackendAdapter::without_engine(host)`；`HostOnlySessionFactory` 永远返回 `BackendError::Unavailable("native engine is not installed")` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:163`、`.../session.rs` | host-only 下 diagnostics 仍合并 `plan.syntax_diagnostics`；hover/definition/completion 返回 `Unavailable`；symbols/event 仍工作 |
| 8a-4 | `lib.rs` 导出 `run_stdio_host_only`；更新模块注释，删除“default `--stdio` 仍是 Node” | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:44`、`:49`、`:73` | `cargo doc` 无 stale 陈述 |
| 8a-5 | usage 文本：`--backend native|node`，native 默认，node deprecated；legacy env 只列在 node 段 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:16` | `--help` 快照/测试更新 |
| 8a-6 | `serve_native()` 保持严格；`serve_node()` 保留 `NodeBackend::new` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:196` | `native_without_install_fails_closed` 继续绿 |
| 8a-7 | 更新 `no_node_guard::lsp_native_status()`：默认 native/HostOnly 且无 `NodeBackend::new` 才 PASS；`WXML_REQUIRE_LSP_NATIVE=1` 变真门禁 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/no_node_guard.rs:477`、`:970` | `WXML_REQUIRE_LSP_NATIVE=1` 可在 P6-8 后通过 |

**HostOnly 的边界：**

- 只在 `EngineDiscoveryError::NotInstalled` 触发；
- 不创建 sandbox、不加载资产、不 spawn 进程；
- `capabilities()` 必须把类型能力标为 `Unavailable`/`Partial`，不能标 `Supported`；
- `crate::server::log` 打印一次 path-free 警告，不写 stdout；
- 不读取 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`。

### 3.2 P6-8b：默认切换门禁

| Gate | 内容 | 证据 |
| --- | --- | --- |
| G-DEFAULT-01 | v2 包 + 默认 `--stdio` → `initialize`、hover、definition、completion、diagnostics、shutdown 全部走原生 | `native_engine_real` 或新 `default_native_stdio` 测试；`pgrep` 无 node |
| G-DEFAULT-02 | 无安装 + 无 Node → HostOnly 可 initialize；静态诊断/格式化/事件可用；类型查询 `Unavailable`；无 node exec | `cli.rs` + PATH tripwire |
| G-DEFAULT-03 | 损坏安装（engine hash/manifest/资产）→ 默认启动失败；不降级 HostOnly/Node | `cli.rs` 单点变异 fixture |
| G-DEFAULT-04 | legacy env poison（`WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`）→ 默认后端不变、tripwire 未执行 | `cli.rs`/`no_node_guard` |
| G-DEFAULT-05 | `--backend node` 仍可用（过渡），且不会在缺 `server.js` 时变成 native | `cli.rs` |
| G-DEFAULT-06 | `cargo test --workspace` 在无 Node PATH 下通过 | clean-env CI job |

### 3.3 P6-8c：release 构建与 v2 包默认

这一项实际上属于 P6-B7，但必须与默认切换同批验收，否则“默认 native”只存在于源码，不在发布物：

| # | 变更 | 文件 | 验收 |
| --- | --- | --- | --- |
| 8c-1 | 新增 native release 路径：`provision --from-dir` → 准备 `bin/` + `zed-extension/` → `wxml-lsp-package native` → `verify-package` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/scripts/build-release.sh:157`、`:224`、`:266` | 无 node/npm；两次构建 archive 同 SHA-256 |
| 8c-2 | 删除 Node 前置：`require_tool node/npm`、`npm ci/build/prune`、Rust/Node protocol 交叉校验、`--node-minimum` | 同上 `:157-171`、`:224-244`、`:266-277` | `command -v node` 不存在时完整构建 |
| 8c-3 | 平台固定 `linux-x86_64`；非 x86_64 直接失败（v2 packager 只支持该平台） | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/engine.rs:35`；`wxml-lsp-package` native | aarch64 不产出 native 包 |
| 8c-4 | 生成/校验 `THIRD_PARTY_LICENSES.md`（当前仓库根没有这个文件，v2 packager 强制要求） | 新增 release staging 逻辑 | `verify-package` 通过；许可文件 hash 入 manifest |
| 8c-5 | staging 只放 native allowlist：`bin/wxml-lsp`、`bin/wxml-lsp-install`、`zed-extension/extension.wasm`、`extension.toml`、`languages/**`、`THIRD_PARTY_LICENSES.md`；不要放 `initialization-options.schema.json` 除非 verify 允许 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:807` | 包文件集合精确；无 `node_modules`/JS/`server.js` |
| 8c-6 | `wxml-lsp-package` 默认子命令改为 native；v1 移到 `legacy`；`native` 保留别名 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/bin/wxml-lsp-package.rs:166` | `packager_cli.rs` v1 用例改调 `legacy`，不删除 |

### 3.4 P6-9a：环境变量映射

| # | 变更 | 文件 | 验收 |
| --- | --- | --- | --- |
| 9a-1 | `WXML_ANALYZE_TIMEOUT_MS` → native `FrameLimits::request_deadline_ms`；默认 15000；0/非法/越界 → 默认并告警 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/framing.rs:74`；`SessionOptions`/`NativeSessionFactory` | 设置 1500 时请求按 1500ms 截止；0 不产生 0ms deadline |
| 9a-2 | `WXML_TEMPLATE_CHECK` → `JsCheckPolicy`；默认 **`Pragma`**（当前硬编码 `Inherit`） | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:727` | off/pragma/inherit 三态测试；默认与旧 Node 行为一致 |
| 9a-3 | `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` 只在 `--backend node` 下读取；默认/HostOnly 设置这些变量不改变行为 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/config.rs:155` | poison 测试（G-DEFAULT-04） |
| 9a-4 | 无效 `WXML_ANALYZE_TIMEOUT_MS`/`WXML_TEMPLATE_CHECK` 不导致启动失败；不产生路径/值泄漏 | 同上 | CLI 测试 |

### 3.5 P6-9b：Zed

| # | 变更 | 文件 | 验收 |
| --- | --- | --- | --- |
| 9b-1 | 扩展继续只传 `--stdio`；默认切换后自动选 native，不需要 `--backend native` | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/src/lib.rs:53` | F3-14 进程级 smoke：`pgrep` 显示包内 `wxml-lsp --stdio`，无 node |
| 9b-2 | 保持 `FORWARDED_ENV = [WXML_ANALYZE_TIMEOUT_MS, WXML_TEMPLATE_CHECK]`；不恢复三个 legacy 变量 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/src/lib.rs:23` | 回归测试仍断言 legacy 不在列表 |
| 9b-3 | `WXML_LSP_BIN` 必须指向包内 `bin/wxml-lsp`；README 写清 v2 安装路径 | ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/README.md` | 新用户按文档可启动 |
| 9b-4 | 标准 Dev Extension + 五语言注册 + hover/definition/completion/诊断的 UI 证据 | F3-14 | 截图/日志 + reviewer；不能用 stdio 测试代替 |

### 3.6 P6-9c：文档

| 文档 | 需要删除/改写的内容 |
| --- | --- |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/README.md` | Node 后端、`npm` 构建、Node 安装器、包内 `server.js`、`WXML_NODE` 默认说明 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/README.md` | “real Node backend”、Node 安装布局、真实 Node 测试命令 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/README.md` | Node 包布局、`npm` 构建、v1 安装排错 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/README.md` | Node >= 24、`server.js`、`WXML_BACKEND_DIR`、Node 冒烟 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/README.md` | ADR-0008 状态、F3 索引；只在 G6 后改 accepted |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/adr/0008-rust-tsgo-no-node-backend.md` | `proposed` → `accepted` 只能在 G6 证据齐备后 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:42` | “C1 Node backend accepts” 注释；改为 native/共享语义 |

**顺序要求**：代码切换（P6-8）和文档（P6-9c）分开提交；文档不得先声称“默认 native”而代码仍 Node。

---

## 4. 测试与 CI 迁移

### 4.1 先纠正计数

任务里的“36 ignored”与源码不符，必须按以下三类分开：

| 指标 | 值 | 来源 |
| --- | ---: | --- |
| 旧 Node gate `#[ignore]` | **23** | `real_node` 5 + `wave5` 7 + `stdio_type_loop` 9 + perf 1 + stability 1 |
| native replacement `#[ignore]` | **18**（工作树） | `native_engine_real.rs` |
| `crates/wxml-lsp/tests` 全部 `#[ignore]` | **41** | 23 + 18 |
| ledger `retired-with-decision` | **36** | 不是 ignored 测试 |
| ledger `planned` | **267**（184 无 Rust 证据） | `tsgo-test-ledger.json` |
| silent return / 可选 tsc | **2 + 1** | `semantic_tools.rs:2081/2169/2569` |

“36”是 retired case 数，不是 ignored 测试数；把两者混写会低估 23 个旧 gate 的冻结范围。

### 4.2 23 个旧 gate 状态（HEAD 16/7；工作树 18/5）

| Gate | 旧测试 | 分类 | native replacement | 当前 |
| --- | --- | --- | --- | --- |
| G-P5-E2E-01 | `real_node.rs:493` `non_owner_script_overlay_propagates_and_survives_a_node_restart` | delete-at-P7 | 需新增 `native_non_owner_script_overlay_survives_restart` | **required**；`304aa20` 已有 overlay replay + `files_hash`，缺 gate test/promotion |
| G-P5-E2E-02 | `real_node.rs:219` `real_node_event_handler_diagnostics_definition_and_completion` | delete-at-P7 | 需新增 event-handler hover gate | **required**；`3220362` 已提交 event hover，缺 gate test |
| G-P5-E2E-03 | `real_node.rs:103` | delete-at-P7 | `native_expression_and_event_completion_smoke` | green |
| G-P5-E2E-04 | `real_node.rs:30` | delete-at-P7 | `native_hover_and_diagnostics_smoke` | green |
| G-P5-E2E-05 | `real_node.rs:626` `real_node_symbols_smoke` | delete-at-P7 | 需新增 `native_symbols_smoke` | **required**；`304aa20` + `4140e9b` 已有 symbols，缺 LSP gate test/promotion |
| G-P5-E2E-06 | `stdio_type_loop.rs:499` | keep-gated-native | `native_ac001_page_data_and_jsdoc_hover` | green |
| G-P5-E2E-07 | `stdio_type_loop.rs:601` | keep-gated-native | `native_ac002_field_typo_diagnostic_and_fix_clear` | green |
| G-P5-E2E-08 | `stdio_type_loop.rs:653` | keep-gated-native | `native_ac003_unicode_and_crlf_positions` | green |
| G-P5-E2E-09 | `stdio_type_loop.rs:775` | keep-gated-native | `native_ac011_unknown_owner_degrades_honestly` | green |
| G-P5-E2E-10 | `stdio_type_loop.rs:701` | keep-gated-native | `native_ac013_component_properties_hover` | green |
| G-P5-E2E-11 | `stdio_type_loop.rs:942` | keep-gated-native | `native_component_behavior_data_and_properties` | green |
| G-P5-E2E-12 | `stdio_type_loop.rs:831` | keep-gated-native | `native_static_wxs_and_template_semantics_work_over_stdio` | green |
| G-P5-E2E-13 | `stdio_type_loop.rs:1078` | keep-gated-native | `native_static_wxs_answers_survive_an_unavailable_type_backend` | green |
| G-P5-E2E-14 | `stdio_type_loop.rs:1008` | keep-gated-native | `native_unsaved_wxs_overlay_updates_static_hover_and_completion` | green |
| G-P5-E2E-15 | `wave5_acceptance.rs:670` | delete-at-P7 | `native_ac005_loop_scope_typed_locals` | green |
| G-P5-E2E-16 | `wave5_acceptance.rs:723` | delete-at-P7 | `native_ac006_conditional_union_narrowing` | green |
| G-P5-E2E-17 | `wave5_acceptance.rs:1112` | delete-at-P7 | `native_ac008_alias_project_hover_and_definition` | green |
| G-P5-E2E-18 | `wave5_acceptance.rs:995` | delete-at-P7 | `native_ac008_multi_view_fixture_isolation` | green |
| G-P5-E2E-19 | `wave5_acceptance.rs:1029` | delete-at-P7 | `native_ac008_simultaneous_views_do_not_pollute_each_other` | green |
| G-P5-E2E-20 | `wave5_acceptance.rs:771` `ac012_component_property_bindings` | delete-at-P7 | 需新增 component-binding diagnostics gate；设计见 [2026-11-20-tsgo-e2e20-component-binding-plan.md](2026-11-20-tsgo-e2e20-component-binding-plan.md) | **required；唯一还没有已实现 replacement 的 gate** |
| G-P5-E2E-21 | `wave5_acceptance.rs:845` `ac012_event_handlers_basic` | delete-at-P7 | 需新增 non-literal `Unsupported` gate | **required**；`4fd45bd` 已提交 non-literal 状态修复，缺 gate test |
| G-P5-PERF-01 | `performance_bench.rs:781` | keep-gated-native | `native_q2_performance_benchmark` | green（工作树） |
| G-P5-STABILITY-01 | `q2_stability.rs:202` | keep-gated-native | `native_ten_round_lifecycle_stability` | green（工作树） |

**冻结校验**：`native_gates.rs:37`/`:63`/`:71` 要求 green 集合精确、required 5 个、总数 23；任何提升必须同步改 `GATE_GREEN_IDS`/`REQUIRED_NOT_MIGRATED_IDS`/计数，否则默认 `cargo test` 先红。

### 4.3 12 个 `real_node`/`wave5` 删除清单

只有这 12 个是“delete-at-P7”；`stdio_type_loop` 9 个、perf、stability 是 keep/retarget，不要一起删。

| # | 文件:函数 | Gate | 删除前置 |
| ---: | --- | --- | --- |
| 1 | `real_node.rs::real_node_hover_and_diagnostics_smoke` | E2E-04 | 已 green；需 mandatory + reviewed |
| 2 | `real_node.rs::real_node_expression_and_event_completion_smoke` | E2E-03 | 同上 |
| 3 | `real_node.rs::real_node_event_handler_diagnostics_definition_and_completion` | E2E-02 | **required**：event hover gate |
| 4 | `real_node.rs::non_owner_script_overlay_propagates_and_survives_a_node_restart` | E2E-01 | **required**：restart gate |
| 5 | `real_node.rs::real_node_symbols_smoke` | E2E-05 | **required**：symbols gate |
| 6 | `wave5_acceptance.rs::ac005_loop_scope_typed_locals` | E2E-15 | 已 green |
| 7 | `wave5_acceptance.rs::ac006_conditional_union_narrowing` | E2E-16 | 已 green |
| 8 | `wave5_acceptance.rs::ac012_component_property_bindings` | E2E-20 | **required：无 replacement** |
| 9 | `wave5_acceptance.rs::ac012_event_handlers_basic` | E2E-21 | **required**：non-literal gate |
| 10 | `wave5_acceptance.rs::ac008_multi_view_fixture_isolation` | E2E-18 | 已 green |
| 11 | `wave5_acceptance.rs::ac008_simultaneous_views_do_not_pollute_each_other` | E2E-19 | 已 green |
| 12 | `wave5_acceptance.rs::ac008_alias_project_hover_and_definition` | E2E-17 | 已 green |

**结论**：12 个里 7 个已 green，5 个（01/02/05/20/21）未完成；其中 01/02/05/21 的实现已在 HEAD（或 `3220362`/`4fd45bd`），缺 gate test 与 promotion；**E2E-20 是唯一还没有已实现 replacement 的 gate**，其并行设计见 [2026-11-20-tsgo-e2e20-component-binding-plan.md](2026-11-20-tsgo-e2e20-component-binding-plan.md)。在 E2E-20 关闭前，`wave5_acceptance.rs` 不能整体删除。

### 4.4 11 个 keep-gated-native 的 retarget

| 文件 | 数量 | 要做的事 |
| --- | ---: | --- |
| `stdio_type_loop.rs` | 9 | 把 `backend_paths()` 从 `WXML_TEST_NODE` + `dist/server.js` 改为 native package fixture（`WXML_TEST_NATIVE`/`WXML_TEST_NATIVE_PACKAGE`）；保留测试名与公开 stdio 边界断言 |
| `performance_bench.rs` | 1 | 从 `start_real_node` 改为 `--backend native` + `native_measure`；保留 p50/p95/p99、RSS、冷启动采样；Linux-only |
| `q2_stability.rs` | 1 | 从 `NodeBackend` 改为 native package；保留 10+ 轮 open/update/restart/RSS 检查 |

这些测试的 native replacement 已经存在（green），但旧测试体仍构造 Node；P7 删除 Node 前必须 retarget 或删除。**推荐 retarget 而不是删除**，因为它们覆盖公开 `--stdio` 边界和产品质量预算，和 `native_engine_real` 的 gate 断言不是同一层。

### 4.5 2 个 silent return + 可选 tsc

| 位置 | 当前行为 | 目标 |
| --- | --- | --- |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/semantic_tools.rs:2081` | `real_node_paths()` 返回 `None` 时打印 skip 并 `return`；测试在默认套件算 passed | 替换为 `native_component_property_edit_applies_and_reparses`；缺 engine 必须 panic，不得 skip |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/semantic_tools.rs:2169` | `tsc` 不存在时打印 `tsc not installed; skipped` 并继续 | 用 native post-edit reparse/typecheck；不得有“跳过”分支 |
| ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/semantic_tools.rs:2569` | 同样的 `Option`/early return | 替换为 `native_rejects_cross_namespace_and_dynamic_edits`；`-32803`、无 edit、源不变必须断言 |

对应 3 个 `BLOCKED_NATIVE_GATES` 在 ` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/symbols_edits.rs:18`；`MIGRATED_CASES` 目前仍为空（`:15`），`native_gates.rs:713` 还要求它为空。**先实现 replacement 并变绿，再改冻结列表，最后才删旧测试**。

### 4.6 13 个直接依赖 `backend_node` 的测试文件（另有 1 个间接）

| # | 文件 | 处理 |
| ---: | --- | --- |
| 1 | `tests/node_backend.rs` | P7-3 删除（Node C1 协议单测；native 协议已有自己的测试） |
| 2 | `tests/real_node.rs` | P7-3 删除 5 个 gate（按 §4.3） |
| 3 | `tests/wave5_acceptance.rs` | P7-3 删除 7 个 gate |
| 4 | `tests/stdio_type_loop.rs` | retarget 9 个（§4.4） |
| 5 | `tests/q2_stability.rs` | retarget（§4.4） |
| 6 | `tests/performance_bench.rs` | retarget（§4.4） |
| 7 | `tests/semantic_tools.rs` | 替换 2 个 silent + tsc（§4.5） |
| 8 | `tests/q2_recovery.rs` | 改为 native protocol 错误/恢复测试，或删除（native supervisor/framing 已有单测） |
| 9 | `tests/q2_support/mod.rs` | fake Node transport → fake native session/backend |
| 10 | `tests/q2_scheduler.rs` | 通过 `q2_support` 使用 fake；改为 fake native |
| 11 | `tests/full_feature_matrix.rs` | fake transport → fake native |
| 12 | `tests/lsp_p7.rs` | fake transport → fake native |
| 13 | `tests/multi_app_projects.rs` | fake transport + 2 个默认 stdio 测试；改为 fake native / HostOnly 默认 |
| 14 | `tests/zed_sync.rs` | fake transport → fake native |

（表里 14 行是因为 `q2_scheduler` 通过 `q2_support` 间接依赖；直接 import 的文件是 13 个。）

### 4.7 归档与“不得提前删”

| 对象 | 归档方式 | 删除条件 |
| --- | --- | --- |
| 12 个 `real_node`/`wave5` 测试 | `git tag p7-pre-delete`；`docs/evidence/data/node-gates.json` 记录 path、test name、gate id、blob sha、`#[ignore]` reason | 每个 gate `mandatory_green` + `replacement_reviewed` + `old_test_removed` |
| 13 个 fake/Node 测试文件 | 同上；记录每个测试名到 replacement | 同一 ladder |
| `backends/typescript/test/**` | 逐文件 SHA-256 + 每个 case 的 `body_sha256` 写入只读 archive manifest；`ledger.rs` 改为读 manifest，而不是读 live tree | P7-2 完成、`cargo test -p wxml-lsp --test native_parity` 仍绿 |
| `backends/typescript/samples/**`（35 文件） | 移到统一 fixture；保留 byte hash、CRLF/BOM/shebang、许可 | P7-4；不得偷偷加入 `tests/fixtures` 的 hash-pinned 树 |
| 旧 TS 实现/构建 | 不归档到 `src/`；只保留 git tag + archive manifest | P7-4 |

**F2 规则**：不得用“旧测试已删除”代替“新测试通过”。任何删除提交必须先有 replacement 的 green + RED 证据；`native_parity` 的冻结列表必须与删除同一提交更新，不能先删后补。

### 4.8 CI 迁移

当前 `ci.yml` 只有 fmt/clippy/test/doc/deny（` /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/.github/workflows/ci.yml:24`），没有 native job、没有 clean-PATH、没有 pinned asset 缓存。P6 需要新增：

| Job | 内容 | 失败条件 |
| --- | --- | --- |
| `check` | 保留现有 fmt/clippy/test/doc/deny | 任一失败 |
| `native-required`（Linux x86_64） | 预置 pinned engine/typings cache（key = `assets.lock.json` hash）；`WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1` 跑 `native_engine_real -- --ignored --test-threads=1`；跑 `native_parity` 默认套件 | 缺资产/引擎、任何 gate 失败、出现 skip |
| `clean-env` | PATH 不含 node/npm/npx/yarn/pnpm；`cargo test --workspace`；v2 包默认 `--stdio` smoke | 任何 node exec；启动失败 |
| `delivery` | `wxml-lsp-package provision --from-dir` + `native` + `install --from-dir` + `verify` + `rollback`；`f3_exec_audit` | 包/安装/回滚/exec 任一失败 |
| `zed-smoke`（可选/自托管） | 真实包 + `WXML_LSP_BIN` + 隔离 Zed；`pgrep` 无 node；五语言/查询 UI 证据 | F3-14 条件未满足 |

**缓存约束**：

- engine tarball 24MB，不能 check-in；CI 缓存必须按 `assets.lock.json` 的 SHA-256 做 key，每次运行重新校验 size/SHA-256/SRI；
- `wxml-lsp-package provision` 不接受 `--allow-download`；CI 只能在 setup 阶段用受控下载或预置 cache，然后 `--from-dir`/`--offline`；
- PR 不写共享 cache；缓存未命中时 fail closed，不跳过 gate。

**测试资产**：`crates/wxml-delivery/tests/fixtures/miniprogram-api-typings-5.3.1` 已经在仓库内（23 文件 / 20 `.d.ts` / 2,522,752 B），可直接作为 `TSGO_TYPINGS_ROOT`，P7 不需要依赖 `backends/typescript/node_modules`。这是 P7-4 能删除 TS 树的关键前提。

---

## 5. 发布、安装与回滚

### 5.0 F3 当前状态（13 closed / 2 partial / 1 blocked）

| 状态 | ID | 对 P6/P7 的含义 |
| --- | --- | --- |
| closed（13） | F3-01/02/03/04/05/07/08/10/11/12/13/15/16 | 可作为既有交付/进程/包/回滚证据；P6 不重复关闭，只做回归 |
| partial（2） | F3-06、F3-09 | F3-06 缺真实第二版本 + 进程级 SIGKILL；F3-09 缺 pre/post-switch 进程级 kill + 真实 legacy 包；**P7-5 删除 v1 packager 前必须关闭 F3-09** |
| blocked（1） | F3-14 | 缺 Zed UI/五语言/查询证据；默认 `--stdio` 仍是 Node；P6-8 后才能做 |

**不要把 closed 当成 P6/G6 已通过**：F3 只覆盖交付合同；G6 还要求默认切换、旧测试无运行职责、Zed 冒烟与候选包验收。

### 5.1 v2 package 默认 native

| 层面 | 当前 | 目标 |
| --- | --- | --- |
| `wxml-lsp-package` | 无子命令 = v1；`native` = v2 | 无子命令 = v2 native；`legacy` = v1；`native` 保留别名 |
| `build-release.sh` | Node/npm + v1 packager | provision + v2 native packager + `verify-package` |
| `index.json` | v1（`nodeMinimumVersion`） | v2（`protocol_version`/`engine_version`/`platform`） |
| 安装器 | 已按 index `schemaVersion` 分派 | 保持；v2 为默认发布物 |
| 包内容 | `bin/wxml-type-backend/**` | `engine/lib/tsc` + `typings/miniprogram-api-typings` + 两个 Rust bin + Zed wasm |

### 5.2 安装/升级/回滚语义（保持不变，P6 只验收）

- `install --version V --from-dir DIR`：v2 先 `verify_tree_v2`、engine probe、activation binaries，再原子 switch。
- `upgrade`：v2 按 index 最新版；N1→N2 失败在 switch 前保持 N1；switch 后 state 已写则前滚（F3-06 partial 的已知边界）。
- `rollback --to V`：只接受已安装的 native V；重新校验/probe 后 switch；`state.previous` 记录上一个 native。
- legacy v1 current：`list/current/verify/env` 只读；`install` v2 迁移；**rollback 到 legacy 明确拒绝**（`native_install.rs:888`：“legacy Node package is not a Node-free rollback target”）。
- legacy→native 迁移 `previous=None`（`native_install.rs:753`）：switch 后失败只能前滚，不能自动切回 legacy。这是设计事实，必须在 release notes 写明。

### 5.3 F3-14 Zed UI

F3-14 仍 blocked：当前 Zed smoke 只证明进程启动、无 Node 子进程；默认 `--stdio` 仍是 Node，原生需要 `--backend native`。P6-8 后：

1. 用真实 v2 包安装；
2. Zed 设置 `WXML_LSP_BIN=<root>/current/bin/wxml-lsp`；
3. 启动隔离 Zed，确认五语言注册；
4. 实际 hover/definition/completion/诊断（含未保存 WXML/WXS）；
5. `pgrep`/`/proc` 断言只有 `wxml-lsp` + `engine/lib/tsc`，无 node；
6. 保存截图或 Zed LSP 日志 + reviewer。

如果 CI 没有显示环境，F3-14 只能由自托管 runner 或人工证据关闭；不得用 stdio 测试替代 UI 证据。

### 5.4 F3-06/F3-09 partial 的取舍

| 项 | 现有 | 关闭所需 | 建议 |
| --- | --- | --- | --- |
| F3-06 | 真实 N1→N2→N1、四窗口 `InstallHooks`、fixture 前滚；缺真实第二版本 + 进程级 SIGKILL | 两个真实 release（不同 commit/二进制）；test-only ptrace 在系统调用边界暂停真实 CLI + 外部 `SIGKILL`；`recover` 后状态断言 | **P6 结束前关闭**；否则 G6 只能声明“fixture/seam 级” |
| F3-09 | 真实 N1 + v1 fixture 成功迁移；缺精确 pre/post-switch 进程级 kill、真实 legacy 发布包 | 用生产 v1 packager 产出 legacy 包（合同允许 fixture 标注）；test-only ptrace 精确窗口 + 外部 `SIGKILL`；前滚补 state；旧树字节不变 | **P7-5 删除 v1 packager 之前必须关闭**；否则以后无法再测 |

> 进程级 kill 的机制选择见 [2026-11-20-tsgo-f3-kill-window-plan.md](2026-11-20-tsgo-f3-kill-window-plan.md)：**journal 轮询只能采样，不能关闭**；推荐 test-only ptrace 在 `switch`/`state` 系统调用边界暂停真实 `wxml-lsp-install`，再由父进程发送 `SIGKILL`。生产 failpoint 不作为关闭机制。

如果维护者选择不关闭，必须在 G6/F3 记录“accepted risk”，并明确：
- 不声称 F3-06/09 closed；
- 不声称 switch 后崩溃窗口完全安全；
- P7 删除 v1 packager 后不再提供 legacy migration 的真实资产证据。

### 5.5 回滚只回原生包

- 自动回滚：只支持 native→native（`state.previous`）。
- legacy→native 迁移后：`previous=None`，安装器拒绝 legacy rollback；需要人工重装旧包并把 `WXML_LSP_BIN` 指回旧二进制。
- 发布说明必须写：**升级到 v2 后没有自动回 Node 的路径**；这不是缺陷，是 F3-08 的设计。
- 可选的审计增强：在 `state.json` 记录 `migratedFrom`（只读信息，不作为 rollback target），便于支持人员判断来源；不改变状态机。

### 5.6 平台

- v2 packager 固定 `linux-x86_64`（`engine.rs:35`）；`build-release.sh` 必须对 aarch64 直接失败，不得回退 v1 Node 包。
- Windows/UNC、macOS、musl 无证据；默认 native 后这些平台源码构建会 fail closed；不发布、不承诺。
- 如果仍要发布 aarch64，只能继续发 v1 Node 包，与“去 Node”目标冲突；建议停止该平台的 native 发布，直到有证据。

---

## 6. P7 删除顺序（P7-1..P7-7）

### 6.0 新增 P7-0：预删除冻结

在删除任何东西之前，先做一个**只读/测试基建**提交：

1. `git tag p7-pre-delete`；
2. 生成 `docs/evidence/data/legacy-node-tests.json`（12 个 gate + 13 个文件 + 每个测试的 blob sha/name/gate id）；
3. 生成 `docs/evidence/data/legacy-ts-tests.json`（`backends/typescript/test/**` 逐文件 sha256 + case `body_sha256`）；
4. 把 `native_parity/ledger.rs:1429`/`:1571` 从“读 live TS 树”改为“读 archive manifest”；
5. 把 `native_parity/fixture_hash.rs:49` 的 `FROZEN_TREES` 改为 archive manifest hash；
6. 把 `native_gates.rs` 的 23-gate 冻结改成支持 `old_test_removed`（允许旧 `#[ignore]` 消失，但 replacement 必须仍在）。

**验收**：`cargo test -p wxml-lsp --test native_parity` 仍绿；没有删除任何生产/测试代码。没有这一步，P7-3 会先弄红 `native_parity`。

### 6.1 P7-1：台账归零

- 267 planned → 0：184 无证据 + 57 weaker + 6 structural + 17 blocked + 3 near-exact。
- 逐项 green 或带裁决的 `retired-with-decision`；不得批量改状态；不得用删除测试代替证据。
- 23 个 gate 全部 `mandatory_green` + `replacement_reviewed`；2 个 silent return 被替换；可选 tsc 分支删除。
- 这是最大的 P7 blocker，也是 G5/P7-1 的核心；需要独立的 10 个 WP（见 [2026-11-19-tsgo-remaining-ledger-plan.md](../plans/2026-11-19-tsgo-remaining-ledger-plan.md)）。

### 6.2 P7-2：归档旧 TS oracle

- 把 `backends/typescript/test/**` 字节归档到只读证据树（或 archive manifest），更新 ledger 路径/hash；
- 更新 `fixture_hash` 的 `backends/typescript/test`/`samples` 记录；
- 保留 `tests/fixtures` 136 与 `crates/wxml-lsp/tests/fixtures` 43 的 hash 不变；
- 35 个 samples 移到统一 fixture，保留 CRLF/BOM/shebang/许可。

**验收**：`cargo test -p wxml-lsp --test native_parity` 绿；旧字节/hash/许可可审计。

### 6.3 P7-3：删除 `backend_node` 与 Node 测试

前置：P7-1（23 gates `mandatory_green` + `replacement_reviewed`；`closed` 在本步骤删除旧测试后才成立）、P7-2、P6-8（默认 native/HostOnly）。

| 删除 | 说明 |
| --- | --- |
| `crates/wxml-lsp/src/backend_node/**` | 含 config/process/protocol/request/response/tests |
| `crates/wxml-lsp/src/lib.rs:44` | `pub mod backend_node` |
| `main.rs` 的 `--backend node`/`serve_node`/`NodeBackend` import | P6 过渡结束 |
| `tests/node_backend.rs` | C1 Node 单测 |
| `tests/real_node.rs`、`tests/wave5_acceptance.rs` | 12 个 delete-at-P7（每个已 replacement） |
| `tests/q2_recovery.rs` 的 Node 协议错误 | 改为 native 或删除 |
| 其余 6 个 fake Node transport 测试 | `full_feature_matrix`/`lsp_p7`/`multi_app_projects`/`q2_recovery`/`q2_support`/`zed_sync`；改为 fake native session/backend，或删除重复覆盖 |

**不能删**：`tests/stdio_type_loop.rs`（已 retarget）、`performance_bench.rs`/`q2_stability.rs`（已 retarget）、`semantic_tools.rs` 的 native replacement 测试。

**验收**：`cargo test --workspace` 无 Node；`git grep backend_node` 活跃引用清零；exec 审计无 node/server.js；`native_parity` 绿。

### 6.4 P7-4：删除旧 TS 实现/测试/构建

前置：P7-2（archive）、P7-1（相关 case 已裁决）、P7-3。

删除：
- `backends/typescript/src/**`（39 文件）；
- `backends/typescript/test/**`（22 文件，已 archive）；
- `backends/typescript/scripts/**`；
- `package.json`、`package-lock.json`、`tsconfig*.json`；
- `backends/typescript/README.md` 标历史或删除；
- `tests/fixtures/README.md:233` 的 `npm ci && npm run build` 说明。

保留：
- `backends/typescript/samples/**` 只在已移到统一 fixture 后删除；
- 旧证据文档中的 Node 叙述标历史，不改写事实。

**验收**：`git grep` 活跃路径无 `backends/typescript` 构建/运行指令；35 samples byte-exact；`cargo test --workspace` 与 native job 绿。

### 6.5 P7-5：删除 v1 打包/Node 包写入路径

前置：F3-08/F3-09 真实 legacy 证据已关闭（否则删除后无法再测）。

删除：
- `wxml-lsp-package` 的 v1 默认路径（保留 `legacy` 直到本提交）；
- `package.rs` 的 `build_package`/v1 manifest 构造（先拆共享 helper）；
- `acquire.rs`/`cache.rs` 的 v1 路径；
- `install.rs` 的 v1 `install_tree` **写路径**；`verify_installed` 是只读校验，必须保留给 legacy `verify`；
- `verify.rs` 的 `check_backend`（Node 后端检查）；
- `verify-package` 的 v1 分支。

**同时收窄**：
- legacy current 的 v1 `install`/`upgrade`/`rollback` 命令改为明确拒绝或只读提示，不再执行 `acquire`/`install_tree`/`check_backend`；否则“legacy 只读”不成立。
- 若业务仍需要一个“人工装旧包”的入口，应放在发布说明/脚本层，不放在无 Node 安装器里。

**必须保留**：
- `manifest_v2` 的 legacy 只读投影（`read_legacy_manifest`）；
- `inspect_current` 的 `CurrentInstall::Legacy`；
- `list/current/verify/env` 的 legacy 只读分支；
- `rollback` 对 legacy 的明确拒绝；
- `manifest.rs` 的共享 helper（sha256/atomic_write/safe_join 等）；
- `verify.rs` 的 `output_with_timeout`/`binary_version` 等共享函数。

**验收**：`cargo test -p wxml-delivery` 绿；legacy current 仍可 `list/current/verify/env`；`rollback --to legacy` 仍拒绝；`git grep build_package` 无生产调用。

### 6.6 P7-6：文档/CI/Zed 清理

- README、crates/*/README、Zed README、docs/README：删除 Node 构建/运行说明；
- 历史计划/证据/ADR 保留原事实并标历史身份；
- CI 删除 v1/Node job，保留 native-required/clean-env/delivery；
- Zed 文档写 v2 安装 + `WXML_LSP_BIN`；不恢复 legacy env。

### 6.7 P7-7：最终复验

- `cargo fmt`/`clippy --all-targets -D warnings`/`cargo test --workspace`；
- native-required 23/23；
- 两次确定性 release，archive SHA-256 一致；
- install/upgrade/rollback/migrate/recover；
- clean-PATH + ptrace exec 审计；
- Linux Zed smoke（F3-14）；
- provenance/许可/SBOM；
- 最终报告：迁移用例/断言数、差异处置、工具/引擎/提交/资产摘要、测试结果、平台范围、已知限制。

### 6.8 P7 依赖表

| 步骤 | 依赖 | 可并行 | 阻塞 |
| --- | --- | --- | --- |
| P7-0 | 无 | 是（P6-8 前） | 无 |
| P7-1 | P7-0 | 与 P6-8 实现并行 | 267 planned |
| P7-2 | P7-0 | 是 | TS 树 hash |
| P7-3 | P7-1/P7-2/P6-8 | 否 | 23 gates、2 silent、13 文件 |
| P7-4 | P7-2/P7-1 | 否 | samples 迁移 |
| P7-5 | F3-08/09 | 否 | legacy 证据、共享 helper |
| P7-6 | P7-3/4/5 | 否 | 文档 |
| P7-7 | 全部 | 否 | 全量 |

---

## 7. 风险与冲突

| # | 风险 | 影响 | 缓解 |
| ---: | --- | --- | --- |
| 1 | 默认切换后 Node 用户的 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` 静默失效 | 用户以为还在用旧 backend | release notes + `--backend node` 过渡 + poison 测试 + 启动 warning |
| 2 | 旧 v1 包迁移后不能自动回滚 | 升级失败时只能人工重装 | 文档写明；可选 `migratedFrom` 审计字段；native N1→N2 仍有 rollback |
| 3 | `WXML_TEMPLATE_CHECK` 默认从 Node 的 `pragma` 变成 native 的 `inherit` | JS 诊断行为差异 | P6-9 修正为 `Pragma`；三态测试 |
| 4 | `WXML_ANALYZE_TIMEOUT_MS` 在 native 不生效 | 用户配置被忽略 | P6-9 映射到 `FrameLimits`；无效值告警 |
| 5 | CI 缓存 engine/typings 被污染 | gate 假绿/供应链风险 | cache key 含 `assets.lock.json` hash；每次重算 SHA/SRI；PR 不写共享 cache |
| 6 | Zed 扩展旧版本转发 legacy env | 无效配置但无实际后端切换 | 040898c 已移除；README 要求新版本；回归测试保留 |
| 7 | Windows/macOS/aarch64 无 native 包 | 默认 native 后这些平台不可用 | 只发 linux-x86_64；build-release 显式失败；不承诺 |
| 8 | 开发 shell 全局 `WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` 干扰 gate | `wave5` 曾因此 7/7 假红 | required job `env -u`；文档；poison 测试 |
| 9 | `native_parity` 仍读旧 TS 树/hash | 提前删 TS 会先红 | P7-0/P7-2 archive + ledger 解耦；不得先删后补 |
| 10 | `no_node_guard::lsp_native_status()` 永远不返回 PASS | `WXML_REQUIRE_LSP_NATIVE=1` 永远失败 | P6-8 重写为“默认无 NodeBackend”检查 |
| 11 | `build-release.sh` 仍要求 Node/npm | 无 Node 环境无法产出候选包 | P6-8c native release 路径 |
| 12 | `E2E-20` 无 replacement | `wave5` 不能整体删除 | 实现 component binding diagnostics，或单独保留该测试并记录 waiver |
| 13 | F3-06/09 partial | 崩溃窗口/legacy 迁移无进程级证据 | P6 结束前用 test-only ptrace 精确窗口 + 外部 SIGKILL；F3-09 在 P7-5 前关闭 |
| 14 | 267 planned 未归零 | P7 删除违反 F2 | P7-1 独立程序；不得用删除代替 |
| 15 | HostOnly 被误认为“原生引擎通过” | 假绿 | capabilities 标 `Unavailable`；exec 审计区分；只允许 `NotInstalled` |
| 16 | v1 只读命令在 P7-5 被误删 | 旧安装无法识别/迁移 | 明确 keep 列表；测试 legacy current 的 current/verify/env/list |

---

## 8. 可执行的 gate/commit 顺序

### 8.1 两条轨道

- **Track A（严格按计划）**：G5 关闭（ledger planned=0）→ P6-8 默认 flip → G6 → P7-3..P7-7。
- **Track B（建议的工程轨道，需要书面 waiver）**：23 个 gate 全部 mandatory/reviewed + 2 silent 替换 + 默认套件 Node-free → 默认 flip → P6 发布验收 → P7-1 继续归零 → P7 删除。

如果维护者不接受 waiver，只能走 Track A；这意味着 **267 planned 是 P6-8 的前置**，而不是 P7 的收尾。

### 8.2 Commit/gate 表

| # | 提交 | 内容 | 退出 gate | 回滚 |
| ---: | --- | --- | --- | --- |
| 0 | `docs: plan P6 default switch` | 本文 | 无行为变化 | 文档 revert |
| 1 | `feat(tsgo): finish native gates` | 基于已提交的 `304aa20`/`3220362`/`4fd45bd`/`4140e9b`，补 01/02/05/21 的 LSP gate test；落 perf/stability 未提交提升；promote 到 22/23 | `native_parity` 默认绿；required native job 22 passed/0 failed；fail-closed RED | 保留 Node 默认；只 revert gate |
| 2 | `feat(tsgo): component binding diagnostics` | E2E-20 + gate test；promote 23/23 | required native 23/0；`wave5` 可进入删除评估 | 只 revert gate/诊断 |
| 3 | `test(tsgo): make native gates mandatory` | required CI job；`no_node_guard` PASS 定义；2 silent 替换；`native_gates` 冻结列表更新 | CI native-required 23/0；默认 `cargo test` 无 Node；`WXML_REQUIRE_LSP_NATIVE=1` 可过 | 保留旧 tests；CI job 可单独 disable |
| 4 | `test(tsgo): archive pre-delete state` | P7-0 tag/archive/ledger-hash 解耦 | `native_parity` 绿；无删除 | tag/文件 revert |
| 5 | `feat(lsp): default native` | P6-8a/8b；HostOnly；`--backend node` deprecated；default gate | G-DEFAULT-01..06；clean-env `cargo test` | 回到 Node 默认（本提交可单独 revert） |
| 6 | `feat(tsgo): map native env` | P6-9a timeout/js_check | 三态/超时测试；poison 测试 | 恢复默认常量 |
| 7 | `build(release): native v2 package` | P6-8c + packager 默认 native | 两次构建同 SHA；`verify-package`；无 node/npm | 旧脚本/旧 release |
| 8 | `docs(zed): native install` | P6-9b/c；F3-14 UI | Zed smoke + UI 证据 | 文档/扩展版本 |
| 9 | `test(delivery): close F3-06/09` | 真实 N2、test-only ptrace 精确 SIGKILL、legacy fixture | F3-06/09 closed 或 accepted risk | 证据 revert |
| 10 | `chore(p7): ledger zero` | P7-1 | planned=0；23 gates reviewed | 不删测试 |
| 11 | `refactor(lsp): remove backend_node` | P7-3 | `cargo test` 无 Node；`git grep` 清零 | tag |
| 12 | `refactor(backend): remove TS tree` | P7-4 | samples byte-exact；无活跃引用 | tag |
| 13 | `refactor(delivery): remove v1 write` | P7-5（保留 legacy 只读） | legacy current 仍可 current/verify/env/list；rollback 拒绝 | tag |
| 14 | `docs(ci): remove Node` | P7-6 | CI 无 Node job；文档无 Node 指令 | 文档 revert |
| 15 | `release: final verification` | P7-7 | G7 全量 | 上一 tag |

### 8.3 最小可执行前置（如果只能做三件事）

1. **先补 01/02/05/21 + E2E-20 的 native gate test 并 promote**，否则 12 个删除没有合法依据。
2. **把 required native CI job 跑起来**（pinned engine + repo typings fixture），否则 `mandatory_green` 永远不成立。
3. **改 `build-release.sh` 到 v2 native**，否则默认切换只存在于 `cargo run`，发布包仍是 Node/v1。

---

## 9. 核心结论与必须先完成的 blocker

### 9.1 核心结论

1. 默认 Node 入口非常集中：`main.rs:88` → `serve_real`（`:182`）→ `NodeBackend::new`（`:184`）；`--backend native` 已经是严格的、无 Node fallback 的 opt-in（`:196`）。
2. 原生链路的关键部件（engine discovery、sandbox、type assets、compilerOptions、v2 package/install/upgrade/rollback/offline cache）大多已经存在；F3 为 13/2/1。缺的不是“从零实现”，而是**默认选择器、release 构建、gate 强制化、P7 台账/归档**。
3. P6-8 推荐“默认 native + `NotInstalled` 时 HostOnly + 显式 `--backend node` 过渡”，而不是“严格 native 直接替换”。这保留静态-only 场景和默认测试，同时绝不隐式回 Node。
4. P7 删除目前**不能开始**：23 个 gate 只有 18 green（HEAD 16）、5 required；2 个 silent return 仍在；E2E-20 无 replacement；267 planned 未归零；`native_parity` 仍读旧 TS 树；`build-release.sh` 仍 Node/v1。
5. 12 个 `real_node`/`wave5` 删除里，7 个已 green，5 个未完成；E2E-20 是唯一还没有已实现 replacement 的 gate，但已有可关闭的并行设计（`2026-11-20-tsgo-e2e20-component-binding-plan.md`），必须先落地并 promote。

### 9.2 必须先完成的 blocker（按顺序）

| # | Blocker | 退出条件 |
| ---: | --- | --- |
| B1 | G5/ledger：267 planned（184 无证据） | planned=0 或书面 G5 waiver；Track A 下是 P6-8 前置 |
| B2 | 5 个 required gate + 2 silent return | 01/02/05/20/21 全部 gate_green；G-P5-E2E-22/23 换成 native edit gates |
| B3 | required native CI + `mandatory_green` | pinned engine/typings cache；23/0；无 skip；`no_node_guard` PASS |
| B4 | release 构建仍是 v1/Node | `build-release.sh` native v2；两次同 SHA；无 node/npm |
| B5 | 默认选择器/HostOnly | P6-8 提交；G-DEFAULT-01..06 |
| B6 | `native_parity` 旧 TS 树/hash | P7-0 archive + ledger 解耦 |
| B7 | F3-14 | 真实包 + Zed UI + 无 node 证据 |
| B8 | F3-06/09 | 真实 N2 + journal SIGKILL；F3-09 在 P7-5 前关闭 |
| B9 | P7 删除顺序 | P7-1→P7-7；不得先删后补 |

### 9.3 明确不声称

- 不声称 P6、G5、G6、G7 已关闭。
- 不声称默认后端已经切换；当前 HEAD 仍是 Node 默认。
- 不声称 23 个 gate 已全部迁移；HEAD 是 16 green/7 required，工作树是 18/5。
- 不声称 F3-01..16 全部关闭；当前是 13/2/1。
- 不声称 legacy→native 可以自动回滚；实现明确拒绝。
- 不声称 Windows/macOS/aarch64 支持。
- 不声称 HostOnly 是“原生引擎验证通过”；它只是无类型引擎的静态模式。
