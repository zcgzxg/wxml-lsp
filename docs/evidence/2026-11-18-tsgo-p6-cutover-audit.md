# P6/P7 默认切换与去 Node 审计（2026-11-18）

## 0. 审计身份与边界

- 审计对象：commit `69e13fb125071885b24877c804ed69363fda290a`（用户指定 HEAD）。
- 审计方式：只读源码、台账 JSON、合同/计划与 `git archive` 快照；未运行 `cargo`/`npm` 测试，未运行进程审计，未切换默认后端，未删除 Node，未修改生产代码/测试/Cargo/ADR/计划/合同。
- 审计期间该 worktree 被另一会话推进：开始时 `git log` 顶部为 `69e13fb`，结束时 HEAD 为 `6ab1bc6`（`feat(tsgo): align completion with native contract`）。两者差异只在 `crates/wxml-backend-tsgo/src/native/wire.rs`、`query/completion.rs`、`crates/wxml-lsp/src/backend_tsgo/{completion,mod,tests}.rs`，共 5 个文件。本文行号以 `69e13fb` 快照为准；除 completion 外，路径与行号与当前树一致。
- 审计期间同一 worktree 还存在其他会话的未提交/未跟踪修改（至少 `crates/wxml-backend-tsgo/src/native/{ast,mod,session}.rs`、`src/query/mod.rs`、`document_symbol` 相关文件，以及 `docs/evidence/2026-11-18-tsgo-f3-clean-env.md`）；审计结束时又出现未跟踪的 `scripts/verify-no-node.sh`、`crates/wxml-delivery/tests/no_node_guard.rs`。这些修改不属于本次审计，未被当作证据，本文判断以 `69e13fb` 为准；其中 clean-env 相关文件可能部分覆盖 BLOCK-P6-08，需另行复核。
- 本文是审计，不是 P6/P7 完成声明。所有“当前”陈述均来自 `69e13fb` 源码事实；所有验收状态均未因本次审计而改变。

---

## 1. 简明总结

**当前默认后端仍然是 Node，原生后端没有接线。** `wxml-lsp --stdio` 只构造 `NodeBackend`；`backend_tsgo` 是 `pub(crate)` 的惰性模块，没有 server/config 路径选择它。原生适配器虽然是 `Backend` 实现，但 `main.rs` 无法访问它，且没有生产构造点。

**P6 现在不能开始默认切换，至少被 8 个硬阻塞挡住：**

1. G5 未关闭：台账 `33 green / 270 planned / 36 retired`，16 个 parity 分区 `MIGRATED_CASES` 全空；189 个 planned 用例没有任何 Rust 证据。
2. 原生适配器 `pub(crate)`，`main.rs` 不可见；`with_host` 需要调用方自己给出 `SessionOptions`。
3. 没有引擎发现/校验：没有从 `current/bin/wxml-lsp` 的包根读取 `manifest.json`、定位 `engine/lib/tsc`、校验 SHA-256、创建空 sandbox 的生产代码。
4. 原生 VFS 固定 `noLib:true`、`strict:true`、`module/target esnext`，忽略项目 `compilerOptions`；`AnalysisInput` 只有文件，没有 `typings/`、`lib*.d.ts`、`compilerOptions` 字段。切换后 `wx`/标准库类型会缺失。
5. 没有 schema v2 打包器。`wxml-lsp-package` 只接受 v1/Node 布局；`manifest_v2` 只有读取/校验/`to_json`，没有生产打包构造与写入路径。
6. 安装器的 legacy 只读命令仍会执行 Node：`current` 调 `check_node`，`verify` 调 `verify_install`（内部 `check_node`），`env` 仍输出 `WXML_TYPE_BACKEND`。这与 F3-01/F3-15 的“无 Node 的 current/verify/env”冲突。
7. 旧 Node 安装迁移到原生包后，安装器明确拒绝把 legacy 作为 rollback 目标；首次迁移在 switch 之后、state 写入之前失败时，`previous=None` 使 `restore_previous` 无法切回旧 current。F3-09 的“失败/中断旧 current 不变”与实现不一致。
8. 没有 v2 归档缓存；`--offline` 的原生安装只能 `--from-dir`。没有 clean-PATH + `strace` 进程审计脚本或 CI job；现有证明只有 PATH shim，无法排除绝对路径调用。

**P7 更晚。** 270 个 planned 必须先变成 0（green 或带裁决的 `retired-with-decision`）；23 个 `#[ignore]`、2 个 `semantic_tools` 静默 return、3 个 blocked native edit gates 必须先有真实原生命令；`native_parity` 仍会读取 `backends/typescript/test/**` 和冻结 hash，直接删除 TS 测试会先弄红 parity 门禁。

**已经 Node-free 的部分：** `wxml-backend-tsgo`、`wxml-project`、`wxml-syntax`、`wxml-text`、`wxml-types`、`wxml-fixtures`、`.github/workflows/ci.yml`、Zed wasm 代码（除环境变量转发）本身不启动 Node；`manifest_v2`/`verify_v2` 还主动拒绝 Node 运行时文件。

---

## 2. 默认后端选择链路（精确）

### 2.1 当前链路

| 位置 | 事实 | 判定 |
| --- | --- | --- |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:66` | `--stdio` 无额外参数 → `serve_real()` | 固定入口 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:93-99` | `serve_real()` 只构造 `NodeBackend::new(NodeBackendConfig::from_env(), host)` | **P6 必须改** |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:28-35` | usage 只列 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`/`WXML_ANALYZE_TIMEOUT_MS`/`WXML_TEMPLATE_CHECK` | **P6/P7 必须改** |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:44` | `pub mod backend_node;` | **P6/P7 必须改** |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:46` | `pub(crate) mod backend_tsgo;`，注释写明“deliberately inert” | **P6 必须改** |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/server.rs:413,422,437` | `run_stdio`/`run_stdio_with_host`/`run_with_host` 接收 `Arc<dyn Backend>` | 无选择逻辑；保持 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:42-47` | `SUPPORTED_COMPILER_OPTIONS` 注释写“C1 Node backend accepts” | 仅文档/命名；P6/P7 清理 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:345-370` | `PreparedView` 有 `compiler_options`，但原生适配器没有消费它 | **P6 功能缺口** |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/config.rs:155-217` | `from_env()` 解析 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`/`WXML_ANALYZE_TIMEOUT_MS`/`WXML_TEMPLATE_CHECK` | **P6/P7 删除** |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/config.rs:229-247` | 默认只探测 executable-relative `backends/typescript/dist/server.js` 或 `wxml-type-backend/server.js` | **P6/P7 删除** |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/process.rs:249-260` | `Command::new(&config.node).arg("--").arg(server).current_dir(backend_dir)` | **P6/P7 删除** |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:95` | `pub(crate) struct TsgoBackendAdapter` | **P6 必须公开或提供库内构造器** |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:118-124` | `with_host(host, SessionOptions)`；没有 engine 发现 | **P6 必须补** |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:403-407` | `symbols()` 固定 `SymbolResult::unsupported(SYMBOLS_UNSUPPORTED)` | F1 缺口；P6 需裁决 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:430-452` | hover/diagnostics/definition/completion 全部 `Partial`，理由是 event-handler/component/详情/自动导入未覆盖 | **P6 G5 缺口** |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:194-203` | `checkJs` 从项目读取，但 `js_check: JsCheckPolicy::Inherit` 硬编码 | `WXML_TEMPLATE_CHECK` 未接线 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:79-94` | `SessionOptions` 只有 `executable` + `sandbox_root` | 无 engine/asset 发现 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/supervisor.rs:107-108,507-518` | `env_clear()`，仅保留 `LANG=C`/`LC_ALL=C` | 已是无 Node 环境；保留 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:685-696` | 合成 tsconfig：`noLib:true`、`strict:true`、`module/target esnext`，`files` 只含项目文件 | **P6 必须改** |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:214-263` | `AnalysisInput` 只有 project/root/revision/files | **P6 必须扩展 compilerOptions/typeAssets** |

### 2.2 P6 必须改、legacy 回滚必须留、P7 删除

| 对象 | P6 | legacy 回滚需要保留 | P7 |
| --- | --- | --- | --- |
| `main.rs` 的 Node 构造 | 改为原生适配器；缺失引擎显式失败 | 旧包仍可单独运行 | 删除 Node import/usage |
| `lib.rs` 的 `backend_node` | 从 release 构建排除（feature/test-only 或移到 test-support） | 旧 release 包不受影响 | 删除模块 |
| `backend_node/**` | 不参与 release 构建；测试引用迁移 | 旧包字节保留在 `versions/<legacy>` | 删除 |
| `backend_tsgo` 可见性 | 公开构造器或库内 `run_stdio_native` | 不涉及 | 保留 |
| 引擎发现 | 新增包根/manifest/engine/sandbox 解析 | 不涉及 | 保留 |
| `WXML_NODE` | 不再读取；设置也不得激活任何路径 | 旧二进制仍可读自己的 env | 删除文档/转发 |
| `WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` | 不再读取；不得探测 `server.js` | 旧二进制仍可用 | 删除 |
| 安装器 v1 读取 | 保留只读识别/迁移 | **必须保留**（识别 legacy current） | 保留只读投影；删除 v1 打包/Node 检查 |
| 安装器 legacy rollback | **明确拒绝**（现状） | 不提供自动回滚；只能人工旧包 | 保持拒绝 |
| v1 打包器/Node 包 | 保留旧 release 产物用于回滚 | 旧 tarball + 旧 installer | 删除打包代码 |

**结论：** P6 的“legacy rollback”只能是包级人工回滚（重新安装旧 Node 包并把 `WXML_LSP_BIN` 指回旧二进制），不是新安装器的能力。新安装器对 legacy rollback 明确拒绝，见 `native_install.rs:810-831`。如果业务要求自动回滚到 Node，必须另立 ADR/合同，不能把当前状态说成已支持。

---

## 3. Node/npm/npx 引用清单与分类

分类：

- `P6`：P6 必须改（构建/运行/测试/交付/文档）。
- `P7`：P7 删除。
- `R`：保留（业务 TS/JS/.d.ts/fixture/history/parity 证据）。
- `N`：已经 Node-free。
- `P6/P7`：P6 改，P7 删。

原始 grep 在 `69e13fb` 命中 1667 行、187 个文件（排除 `node_modules`/`target`/`.git`）。下表是可执行清单；历史证据中的叙述性 `node` 提及统一为 `R`，不逐行重复。

### 3.1 LSP / 生产代码

| 文件（绝对路径） | 行锚点 | 内容 | 分类 |
| --- | --- | --- | --- |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs` | 10, 28-35, 93-99 | NodeBackend import、usage、`serve_real` | P6 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs` | 11-12, 44, 46 | “real Node backend”、`pub mod backend_node`、惰性 tsgo | P6/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/completion.rs` | 全文（220 行） | C1 completion 适配 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/config.rs` | 6-7, 26, 29, 151-217, 229-247 | Node 路径/env/安装布局 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/mod.rs` | 10, 39, 107, 131-140, 431, 777 | `NodeBackend`/`StdioNode`/capabilities | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/process.rs` | 37-55, 74-80, 98-174, 249-260 | 启动 `node -- server.js`、stderr/stdout 线程 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/protocol.rs` | 全文（191 行） | C1 NDJSON 协议 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/request.rs` | 全文（1331 行） | C1 请求构造 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/response.rs` | 全文（2714 行） | C1 响应解析 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/tests.rs` | 全文（1322 行） | C1 单测 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs` | 42-47, 3215-3218 | “C1 Node backend”注释、typeRoots 解析 | P6 文档；P7 清理 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/*` | 全目录 | 原生适配器；无 Node 进程 | N（但 P6 要接线） |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/**` | 全 crate | 原生 engine/shadow/query；无 Node 进程 | N |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-project/**`、`wxml-syntax/**`、`wxml-text/**`、`wxml-types/**`、`wxml-fixtures/**` | 全 crate | 无 Node 进程 | N |

### 3.2 交付 / 安装器 / 打包

| 文件（绝对路径） | 行锚点 | 内容 | 分类 |
| --- | --- | --- | --- |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest.rs` | 23, 26-28, 43-50, 85-95, 119, 411-425 | v1 schema、`NodeInfo`、`NodeBackend` artifact、index `nodeMinimumVersion` | P7（先拆共享 helper） |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs` | 24-28, 921-959 | v2 schema + legacy 只读投影 | R（P6/P7 保留） |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify.rs` | 247-277, 280-330, 339-... | `check_backend`/`check_node`/`verify_install` | P7（保留 `output_with_timeout` 等共享） |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/install.rs` | 257-274, 276-... | v1 `verify_installed`/`install_tree`；`InstallRoot` 本身共享 | P7 删 v1 方法，保留 root/lock/switch |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs` | 382-468 | 强制 `server.js`/`node_modules`，v1 manifest | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/bin/wxml-lsp-package.rs` | 15-20, 29-31 | 必需 v1 布局、`--node-minimum` | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/bin/wxml-lsp-install.rs` | 36-48, 267-311, 313-363, 365-453, 702-711, 750-770, 802-818 | CLI 分派；legacy `current`/`verify` 执行 Node；legacy `env` 导出 `WXML_TYPE_BACKEND` | P6 改 legacy 只读；P7 收窄 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/acquire.rs` | 全文（288 行） | v1 归档获取 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/cache.rs` | 全文（465 行） | v1 缓存 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/source.rs` | v1/v2 共用 | `Source` + `fetch_archive_v2` | R/改 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/native_install.rs` | 273-387, 675-695, 810-831, 998-1101, 1186-1202, 1305-1344 | v1 识别/迁移/native rollback/recovery | R（P6/P7 保留） |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs` | 3-5, 111-118 | v2 allowlist，拒绝 Node 运行时 | N/R |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs` | 24-46 | `forbidden_runtime_path` | N/R |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/README.md` | 9, 22-24, 32-34, 46, 65, 100-103, 112, 125 | Node 构建/包布局/安装/排错 | P6/P7 |

### 3.3 脚本 / CI / release

| 文件（绝对路径） | 行锚点 | 内容 | 分类 |
| --- | --- | --- | --- |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/scripts/build-release.sh` | 24, 103-112, 157-171, 182-194, 224-244, 266-272 | 要求 `node`/`npm`，构建 `dist/server.js` + `node_modules`，打包 v1 | P6（新增原生路径）；P7 删 Node 步骤 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/.github/workflows/ci.yml` | 1-31 | 仅 cargo fmt/clippy/test/doc/deny | N；P6 需新增 clean-PATH native job |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/Cargo.toml` | workspace | 无 Node 依赖 | N |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/package.json` | 1-40 | `node >=24`、`node --test`、`tsc`、`server.js` | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/package-lock.json` | 全文 | Node 锁文件 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/tsconfig.json`、`tsconfig.build.json` | 全文 | Node 构建配置 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/test/**` | 22 文件 | 旧 TS 测试/oracle | R until P7；P7 删除/归档 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/scripts/**` | 2 文件 | Node 证据/检查脚本 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/src/**` | 39 文件 | 旧 Node 后端实现 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/README.md` | 全文 | Node 后端说明 | P7/历史 |

### 3.4 Zed 扩展

| 文件（绝对路径） | 行锚点 | 内容 | 分类 |
| --- | --- | --- | --- |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/src/lib.rs` | 18-25, 684-698 | `FORWARDED_ENV` 含 `WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`/`WXML_NODE` | P6 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/README.md` | 31, 56-70, 107, 133-135, 183-187, 221, 362 | Node >=24、server.js、backend dir、Node 进程冒烟 | P6/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/extension.toml` | 全文 | 无 Node 依赖 | N |

### 3.5 活跃文档 / 样本说明

| 文件（绝对路径） | 行锚点 | 内容 | 分类 |
| --- | --- | --- | --- |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/README.md` | 18, 22, 34-38, 56-62, 70-77, 92-98, 216, 225-228, 508-511, 516 | Node 后端、npm 命令、env 表、安装器 Node 依赖、包内 server.js | P6/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/README.md` | 10-15, 22-23, 34-45, 51-52, 60, 80-94 | Node 后端/安装布局/真实 Node 测试 | P6/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/README.md` | 9, 22-24, 32-34, 46, 65, 100-103, 112, 125 | Node 构建/包/安装 | P6/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/README.md` | 31, 33 | ADR-0008 proposed；F0/F1/F2/F3 draft | P6/P7 文档状态 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/adr/0008-rust-tsgo-no-node-backend.md` | 4 | ADR 状态 proposed | P6 仅在 G6 后改 accepted |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/2026-11-17-rust-tsgo-migration.md` | 239-251 | P6/P7 定义与 G6/G7 | P6/P7 执行依据 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/2026-11-19-tsgo-remaining-ledger-plan.md` | 11, 37, 56-74 | 270/189/55/6/17/3 与冻结 fixture 树 | P7 依据 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md` | 3, 13-14, 532-620, 718-780 | F0/F1 draft、查询组合、F1 测试全 planned | P6/P7 合同状态 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md` | 3, 21, 84, 221-232, 873-960, 1149-1188 | F3 draft、包根/manifest/engine、状态机、F3-01..16 planned | P6/P7 目标合同 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-test-contract.md` | 3, 18-23, 31-38 | F2 draft、33/270/36、fixture hash | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/type-bridge-c1.md` | 411-416 | `server.js`/npm 构建 | P7 标历史 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/type-diagnostics.md` | 212 | npm 构建命令 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/wxml-language-feature-baseline.md` | 60 | Node 测试作为证据 | P7 更新证据来源 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/tests/fixtures/README.md` | 233-236 | `npm ci && npm run build` + Node runner | P6/P7 |

### 3.6 样本与业务 TS/JS

| 文件/目录（绝对路径） | 行锚点 | 内容 | 分类 |
| --- | --- | --- | --- |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/samples/**` | 35 文件 | 旧 scenario 样本；P7 移到统一 fixture，保留 hash/CRLF/许可 | R/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/samples/js-shebang-bom/index.js` | 1 | `#!/usr/bin/env node` 是 fixture 内容，不是运行时 | R |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/samples/js-bom-shebang-check/index.js` | 1 | 同上 | R |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/tests/fixtures/**` | 136 文件 | AC-001..AC-013，hash 冻结 | R（不得新增） |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/fixtures/**` | 43 文件 | 组件/项目 fixture，hash 冻结 | R（不得新增） |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/fixtures/component-intelligence/miniprogram_npm/**` | 3-4 | `miniprogram_npm` 业务样本 | R |

### 3.7 Rust 测试中的 Node 引用

| 文件（绝对路径） | 行锚点 | 内容 | 分类 |
| --- | --- | --- | --- |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/node_backend.rs` | 10, 128-139 | 直接测 `NodeBackend` | P6 迁移/P7 删 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/real_node.rs` | 18, 30-34, 70-76, 494, 626 | 真实 Node 门禁 5 个 | P6 原生替换/P7 删 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/stdio_type_loop.rs` | 14, 48-82, 499-1078 | 真实 Node stdio 9 个 | P6/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/wave5_acceptance.rs` | 6, 26, 121-157, 670-1112 | 真实 Node 验收 7 个 | P6/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/q2_stability.rs` | 14-39, 202-231 | 稳定性门禁，`dist/server.js` | P6/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/performance_bench.rs` | 18-41, 83-87, 277-282, 781 | 性能门禁，NodeBackend | P6/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/semantic_tools.rs` | 24, 2069-2084, 2168-2194, 2568-2571 | fake + 2 个真实 Node 静默 return + 可选 tsc | P6/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/q2_recovery.rs` | 28-29, 152-173, 671-672 | `StdioNode`/协议错误 | P6/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/q2_support/mod.rs` | 25, 188-189 | fake Node transport | P6 迁移/P7 删 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/full_feature_matrix.rs` | 28, 1073-1074 | fake Node transport | P6 迁移/P7 删 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/lsp_p7.rs` | 23, 212-213, 360-361 | fake Node transport | P6 迁移/P7 删 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/multi_app_projects.rs` | 26, 288-289 | fake Node transport | P6 迁移/P7 删 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/zed_sync.rs` | 24, 334-335 | fake Node transport | P6 迁移/P7 删 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/component_intelligence.rs` | 40-42 | 设置 `WXML_NODE` 为不存在路径 | P6 改/P7 删 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/installer_flow.rs` | 604-614 | v1 Node 安装/离线 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/installer_security.rs` | 全文 | v1 安全用例，部分可复用 | P6/P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/packager_cli.rs` | 全文 | v1 打包器 | P7 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_cli.rs` | 371-396, 411-466 | PATH shim（node/npm/npx/pnpm/yarn） | P6 保留/加强 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_delivery.rs` | 341-360, 616-638, 827-960 | legacy 读取/迁移/rollback 拒绝 | R/P6 保留 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/support/mod.rs` | 361 | `env_remove("WXML_NODE")` | R |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/**` | 全目录 | 无 Node 进程，但 pin 旧 TS 树/hash | R/P7 改 |

### 3.8 历史证据 / 历史计划

| 文件/目录（绝对路径） | 内容 | 分类 |
| --- | --- | --- |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/**` | P1-A/P1-C/P5/P7/P8/W2/W3/W4 历史 Node 证据 | R（保留原事实，不改写） |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/2026-11-16-wave5-parallel-delivery.md` | 历史并行交付计划，含 npm 命令 | R（P7 标注历史身份） |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/2026-11-16-zed-standard-acceptance.md` | 历史 Zed 验收，含 npm 构建 | R（P7 标注历史身份） |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/2026-11-17-tsgo-test-ledger.md` | 迁移台账摘要，含 33/270/36 | R/P7 更新 |

---

## 4. 交付安装器 v1/v2 与旧包兼容状态机

### 4.1 精确行为

| 场景 | 当前行为 | 代码 | 判定 |
| --- | --- | --- | --- |
| 旧 Node 包是 current | `inspect_current` 只读 manifest schema → `CurrentInstall::Legacy`；不执行旧 binary/server.js | `native_install.rs:346-386`；`manifest_v2.rs:921-959` | 正确；保留 |
| legacy current + `state.json` | 拒绝（legacy 不应有 state） | `native_install.rs:736-766` | 正确 |
| 安装 v1 包到 native current 之上 | `reject_v1_over_native` 明确拒绝 | `bin/wxml-lsp-install.rs:267-311`、`535-545` | 正确；P6 保留 |
| legacy current + 安装 v2 包 | `plan_operation` → `Install`, `previous=None`, outcome `Migrated`；旧树字节不变 | `native_install.rs:687-689`；测试 `native_delivery.rs:917-940` | 成功路径正确；rollback 见下 |
| 迁移失败（probe/verify/rename 前） | 旧 current/旧树不变；tmp 清理；无 state | `native_delivery.rs:885-915` | 正确 |
| 迁移在 switch 后失败/中断 | `previous=None` → `restore_previous` 无法切回；current 已是 native target，保留 committed journal | `native_install.rs:1305-1318`；`recover_committed` 1033-1058 | **F3-09 冲突；P6 blocker** |
| 原生 N1 → N2 成功 | 校验/探针后 switch；`state.previous=N1` | `native_install.rs:690-698` | 正确 |
| 原生 N1 → N2 失败（rename 后/switch 前） | N1 current/字节不变；N2 保留为 committed，不自动激活 | 测试 `native_delivery.rs:674-718` | 正确 |
| 原生 N1 → N2 switch 后失败 | `restore_previous(N1)` 重新校验并切回 | `native_install.rs:1305-1344` | 正确 |
| rollback 到 native N1 | 要求 current 是 native、state 存在、目标 schema v2、完整校验/探针；切换并写 `state.previous=N2` | `native_install.rs:810-848`；测试 `native_delivery.rs:966-1000` | 正确 |
| rollback 到 legacy | 明确拒绝：“legacy Node package is not a Node-free rollback target” | `native_install.rs:810-831` | 设计如此；P6 不得宣称 legacy rollback |
| 离线 native 安装/升级 | 没有 v2 归档缓存；`--offline` + 远端源失败；只能 `--from-dir` | `acquire_v2.rs:8-10, 60-62`；`native_cli.rs:710-731` | **F3-12 部分 BLOCKED** |
| 离线 native rollback | 目标必须已安装；重新完整校验/探针；不联网 | `native_install.rs:810-848`；测试 `native_delivery.rs:966-1000` | 正确 |
| 并发 install/upgrade/rollback | OS 文件锁串行化；journal 清理 | `install.rs` lock；`native_install.rs:420-...` | 正确 |
| 崩溃恢复 `staged` | target 不存在 → 清 tmp/journal；target 存在 → 标 committed，不自动激活 | `recover_staged` 998-1032 | 正确 |
| 崩溃恢复 `committed` | target 保留；current 不指向它时不激活；指向它时重新校验并补 state | `recover_committed` 1033-1058 | 正确 |
| 崩溃恢复 `activated` | current 必须指向 target；state 有效则幂等删 journal；否则补 state | `recover_activated` 1059-1083 | 正确 |
| current/verify/env 在 legacy current | `current` 执行 `node --version`；`verify` 执行 `verify_install`（含 Node）；`env` 输出 `WXML_TYPE_BACKEND` | `bin/wxml-lsp-install.rs:702-711, 750-770, 802-818`；`verify.rs:280-330` | **P6 blocker（F3-01/15）** |
| `list` 在 legacy current | 只读 schema，标 `(legacy, read-only)` | `bin/wxml-lsp-install.rs:650-670` | 正确；保留 |

### 4.2 P6 交付阻塞

1. **无 v2 打包器**：`package.rs` 只生成 v1；`manifest_v2` 没有 builder。F3-03 未实现。
2. **无真实资产 provisioning**：没有 `assets.lock.json`、没有 Rust 下载/解包/逐文件校验/`readelf`/glibc 记录。F3-03/F3-13 未实现。
3. **无 v2 缓存**：离线升级/安装必须显式 `--from-dir`；F3-12 不能按“缓存命中”通过。
4. **legacy 只读命令仍执行 Node**：F3-01/F3-15 直接失败。
5. **迁移 switch 后无法恢复 legacy current**：F3-09 的“失败/中断旧 current 不变”不成立；需要合同裁决或实现前滚恢复（补 state）并把“switch 后不可回 legacy”写成明确事实。
6. **没有 clean-PATH/strace 证据**：`native_install.rs:19` 自己标注 F3-01/F3-02 BLOCKED。

**不得声称 F3-01～F3-16 已关闭。** 合同 §9.4 与 §10.1 仍把它们标为 planned/partial/blocked；本次审计没有运行任何 F3 验收。

---

## 5. 无 Node 运行时不变量的现状与证明方案

### 5.1 当前缺口

| 缺口 | 证据 | 影响 |
| --- | --- | --- |
| 默认 `wxml-lsp` 启动 Node | `main.rs:93-99` | 最直接的 Node 运行时 |
| 引擎发现不存在 | `backend_tsgo/mod.rs:118` 只收 `SessionOptions`；没有 `/proc/self/exe`/manifest/engine 解析 | P6 无法启动真实引擎 |
| `WXML_NODE` 仍被解析 | `backend_node/config.rs:166-177` | 旧路径；P6 若仍编译则可能被激活 |
| Zed 转发 legacy env | `editors/zed/src/lib.rs:18-25` | 用户设置 `WXML_NODE` 会被传给新 server |
| legacy 只读命令执行 Node | `bin/wxml-lsp-install.rs:702-711` 等 | 违反 F3-01/15 |
| 没有进程审计 | 只有 `native_cli.rs:371-396` PATH shim；`native_install.rs:19` 明确 F3-02 BLOCKED | 不能排除绝对路径 `node` 调用 |
| 构建仍要求 Node/npm | `build-release.sh:157-171` | P6 无法在无 Node 环境产出候选包 |
| 类型资产未挂载 | `vfs.rs:685-696` `noLib:true`；`input.rs:214-263` 无 assets | 引擎缺失标准库/`wx` 类型；不是“无 Node”问题，但会让默认切换不可用 |

**没有隐藏 Node fallback 的部分：** `wxml-backend-tsgo` 不 spawn Node；`supervisor.rs:507-518` `env_clear`；`backend_tsgo` 的失败是 `Unavailable`/`Partial`/`Unsupported`，不是 Node 回退。`manifest_v2`/`verify_v2` 还会拒绝 `node_modules`、`server.js`、`.js/.cjs/.mjs`、Node 锁文件。

### 5.2 证明方案（P6 必须执行）

1. **clean PATH**：PATH 只含 `wxml-lsp`、`wxml-lsp-install`、`engine/lib/tsc` 所需目录；放置 `node`/`npm`/`npx`/`pnpm`/`yarn` tripwire shim（现有 `native_cli.rs:371-396` 可复用）。
2. **execve 审计**：`strace -f -e trace=execve,execveat` 包住 `install`/`upgrade`/`rollback`/`verify`/`current`/`env` 和一个真实 `wxml-lsp --stdio` 会话；断言 exec 集合 ⊆ `{wxml-lsp-install, wxml-lsp, engine/lib/tsc, 测试驱动}`。PATH shim 不能替代此步，因为绝对路径调用绕过 shim。
3. **进程树**：会话运行中读 `/proc/<pid>/task/*/children` 或 `pgrep -P`，断言没有 `node` 子进程；安装器结束后断言无孤儿。
4. **legacy env 反向测试**：设置 `WXML_NODE=/tripwire/node`、`WXML_TYPE_BACKEND=/tripwire/server.js`、`WXML_BACKEND_DIR=/tripwire`，断言 tripwire 未执行、后端选择不变（F3-15）。
5. **包负向扫描**：`verify_tree_v2` + `forbidden_runtime_path`；`SHA256SUMS` 精确集合；两次构建归档 SHA-256 一致（F3-03）。
6. **离线/网络**：`unshare -rn` 或不可达代理；记录 DNS/连接尝试为零（F3-12）。
7. **Zed 冒烟**：真实安装包 + `WXML_LSP_BIN` 启动 Zed；`pgrep` 显示 `wxml-lsp --stdio`，无 `node`；五语言注册、hover/definition/completion/诊断（F3-14）。

**截至 `69e13fb`：以上 1 只有 PATH shim 的库级测试；2-7 未执行，均为 BLOCKED。**

---

## 6. P7 前置门禁

### 6.1 台账冻结值

| 指标 | 值 | 来源 |
| --- | ---: | --- |
| 总展开用例 | 339 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json` |
| `green` | 33 | 同上；`ledger.rs:23` |
| `planned` | 270 | 同上；`ledger.rs:25` |
| `retired-with-decision` | 36 | 同上；`ledger.rs:27` |
| 无 Rust 证据的 planned | 189 | `migration_evidence.remaining_planned_without_evidence` |
| `MIGRATED_CASES` | 16 个分区全空 | `native_parity/*.rs` |
| `#[ignore]` Rust 测试 | 23 | `native_gates.rs:178-190` |
| `semantic_tools` 静默 return | 2 | `semantic_tools.rs:2082-2084, 2569-2571` |
| blocked native edit gates | 3 | `native_parity/symbols_edits.rs:14-31` |
| 可选真实引擎测试（会 skip） | 5 | 2 个 adapter + 2 个 backend + 1 个 perf |

189 个无证据用例的分布（来自 2026-11-19 backlog plan）：

| 分区 | none |
| --- | ---: |
| `hover_docs` | 7 |
| `definitions` | 0 |
| `completion` | 6 |
| `symbols_edits` | 21 |
| `project_overlay` | 17 |
| `shadow_rewrite` | 1 |
| `mapping_utf16` | 7 |
| `loops` | 7 |
| `guards_js` | 15 |
| `components` | 10 |
| `behaviors` | 4 |
| `events` | 3 |
| `protocol_limits` | 40 |
| `messages_evidence` | 8 |
| `scenario_matrix` | 43 |
| `perf_budget` | 0 |
| **合计** | **189** |

### 6.2 必须关闭的 P7 门禁

1. **台账 `planned == 0`**：270 个必须逐项变 `green` 或带证据/裁决的 `retired-with-decision`。189 个无证据项不能静默删除。F2 合同明确“不得用旧测试已删除代替新测试通过”（`tsgo-test-contract.md:49-51`）。
2. **解除 skeleton 冻结**：`native_parity.rs:154-168` 断言所有 `MIGRATED_CASES` 为空；`native_parity.rs:298-305` 要求 `#[test]` 与列表精确匹配。任何真实 parity case 落地前必须改这个门禁。
3. **23 个 `#[ignore]` 全部原生化**：`native_gates.rs:1-7, 178-190`；ledger 里 `status=required_not_migrated`。分拆：performance 1、q2_stability 1、real_node 5、stdio_type_loop 9、wave5_acceptance 7。
4. **2 个静默 return 原生化**：G-P5-E2E-22/23；替换为 `symbols_edits.rs` 的 3 个 blocked gates（组件属性编辑 reparse/typecheck、跨命名空间/动态 rename 拒绝）。`semantic_tools.rs:2168-2194` 的可选 `tsc` 分支也必须变成原生 post-edit 检查，不能保留 `tsc not installed; skipped`。
5. **5 个真实引擎测试变强制**：`backend_tsgo/tests.rs:2209-2221, 2275-2288`、`wxml-backend-tsgo/tests/real_engine.rs:112-126, 469-480`、`perf_budget.rs:402-406` 在无 `TSGO_BIN` 时 `return`，只有 `WXML_REQUIRE_REAL_ENGINE=1` 才失败。P7 的 clean-env 门禁必须设置该变量，或把这些测试改成显式 hard gate。
6. **旧测试树引用解耦**：`native_parity/ledger.rs:1425-1445` 逐个读取 `backends/typescript/test/*.test.ts`，`:1569-1578` 读取 `test/support.ts`；`fixture_hash.rs:49-70` 把 `backends/typescript/test` 与 `samples` 作为冻结树。P7 删除 TS 测试前必须先：
   - 把旧测试字节归档到只读证据树（保留 hash/许可），并更新 ledger 路径；或
   - 把 ledger 改为只校验记录的 `body_sha256`/文件清单，不再读旧树；同时更新 `FROZEN_TREES`。
   否则 P7 删除会直接弄红 `cargo test -p wxml-lsp --test native_parity`。
7. **samples 迁移**：35 个 `backends/typescript/samples` 文件必须移到统一 fixture，保留 byte hash、CRLF、BOM、shebang 和许可；`tests/fixtures`（136）与 `crates/wxml-lsp/tests/fixtures`（43）是 hash-pinned，不得偷偷加入。
8. **F3 交付门禁**：F3-01..16 全部 planned；至少 F3-01/02/03/06/07/08/09/12/13/14/15/16 要在 P7 前关闭。当前 B1/B2/B4/B5/B7/B9 PARTIAL，B3/B6 BLOCKED，B10 OPEN；不得写成已关闭。

   F3 ID 对照（全部未关闭）：F3-01 clean-PATH install/verify/current/env；F3-02 exec 进程审计；F3-03 包内容/确定性；F3-04 空安装；F3-05 安装失败；F3-06 N1→N2 失败；F3-07 离线回滚；F3-08 v1 rollback 拒绝；F3-09 legacy→native；F3-10 篡改检测；F3-11 恶意归档；F3-12 离线 fail-closed；F3-13 平台/ABI；F3-14 打包 Zed 冒烟；F3-15 legacy 变量/入口；F3-16 崩溃/并发。
9. **平台门禁**：仅 Linux x86_64；Windows/UNC/非 UTF-8 根在 `backend_tsgo/mod.rs:36-41` 明确拒绝；F3-13 未测。P7 不得宣称跨平台。
10. **Zed 打包冒烟**：F3-14 未执行；现有 W4 冒烟是 v1 Node 包，不是原生包。

### 6.3 F1 查询缺口（P6/P7 共同阻塞）

- **hover**：无 `getQuickInfoAtPosition`；只能 type + docs + JSDoc 组合，无 display parts/链接/逐 overload 文档（`tsgo-native-contract.md:533`、§9.8）。
- **definition**：无直接 API；只覆盖 expression-member；external `.d.ts`、generated members、event-handler/component 目标不可表示或未覆盖（§9.5；`backend_tsgo/mod.rs:442-445`）。
- **completion**：无 `getCompletionEntryDetails`、无 `textEdit`、`includeSymbol:false` 关闭自动导入；只有 expression-member；`.`/EOF/未闭合表达式未实现（§9.7；`backend_tsgo/mod.rs:447-450`；`backend_tsgo/mod.rs:383`）。
- **rename**：无 `getRenameInfo`/`getEditsForRename`/`rename`；adapter `symbols` 固定 Unsupported；只剩 Rust 静态语义的保守 rename（§9.9；`backend_tsgo/mod.rs:403-407`）。
- **references**：alias/target 完整性未冻结；空引用默认 `Partial`，不能当“零引用”（§9.6）。
- **diagnostics**：只有 semantic+syntactic；event-handler/component 诊断未覆盖；`getProgramDiagnostics` 不能当 clean 证明（§9.3）。
- **cancellation/perf**：running cancel 无 token；static completion p95 无路径；cold start 未纳入原生采样（2026-11-19 plan WP10）。
- **平台**：Windows drive/UNC、非 UTF-8 root 拒绝（`backend_tsgo/mod.rs:36-41`）。

### 6.4 类型资产 / compiler options 缺口（本次审计新增的 P6 硬阻塞）

- `AnalysisInput` 只有 project/root/revision/files（`wxml-backend-tsgo/src/input.rs:214-263`）。
- VFS 合成 tsconfig 固定 `noLib:true`（`native/vfs.rs:685-696`）。
- `PreparedView.compiler_options` 没有进入原生请求；`backend_tsgo/input.rs:194-203` 只读 `checkJs`，`js_check` 固定 `Inherit`。
- 包内 `typings/miniprogram-api-typings` 与 `engine/lib/lib*.d.ts` 没有任何运行期挂载代码；`vfs.rs` 只挂项目文件。
- 结论：即使 P6 只切换 `main.rs`，原生引擎也没有标准库/`wx` 类型，AC-001..AC-013 的类型行为无法成立。P6 必须扩展 `AnalysisInput`/VFS/tsconfig，或在切换前明确降级并重写验收，不得静默接受。

---

## 7. P6 有序变更清单

> 顺序按依赖排列。A 是 G5 关闭项，必须先于默认切换；B 是交付项；C 是切换与收尾。每项都必须有独立提交/评审，不得与行为重写混在一起。

### P6-A：G5 关闭（默认切换前）

| # | 变更 | 文件:行 | Owner | 依赖 | 验收门禁 | 回滚 |
| --- | --- | --- | --- | --- | --- | --- |
| A1 | 解除 parity skeleton 冻结：允许 `MIGRATED_CASES` 非空，`#[test]` 与列表精确匹配 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity.rs:154-168, 298-305`；16 个 `native_parity/*.rs:13` | 测试/集成 owner | 无 | `cargo test -p wxml-lsp --test native_parity`；每个新增 case 有 RED/GREEN 证据 | 保留 Node 默认；gate 变更可单独 revert |
| A2 | 迁移/裁决 270 planned：189 无证据 + 55 weaker + 6 structural + 17 blocked + 3 near-exact | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/2026-11-19-tsgo-remaining-ledger-plan.md:56-74` | 测试/集成 owner | A1 | `planned==0`；`green` 或 `retired-with-decision` 均有证据；F2 §3-4 | 不删旧测试；保持 Node 默认 |
| A3 | 23 个 `#[ignore]` + 2 个 silent return + 3 个 blocked edit gates 原生化 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/native_gates.rs:1-7, 178-190`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/semantic_tools.rs:2082-2084, 2569-2571`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/symbols_edits.rs:14-31` | 测试/集成 owner | A1/A2 | 23→0、2→0；编辑后 reparse/typecheck；跨命名空间/动态 rename -32803 且无 edit | 旧测试保留；Node 默认 |
| A4 | 原生类型资产与 compilerOptions：扩展 `AnalysisInput`（compilerOptions/typeRoots/typings/lib 文件），VFS 生成 `noLib:false` + 资产文件，映射 `WXML_TEMPLATE_CHECK` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:214-263`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:685-696`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:194-203`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:345-370` | 原生后端 owner | 无 | AC-001..AC-013 原生精确对照；标准库/`wx` 类型可用；F1-15/16/21/22 | 保持 Node 默认；新字段可 feature-gate |
| A5 | 原生引擎发现/校验/沙箱：从 `/proc/self/exe` 包根读 `manifest.json`，取 `engine.binary`，校验 size/SHA-256/non-symlink，创建空 sandbox；缺失显式失败 | 新增 `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/engine.rs`（建议）；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:118`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:79-94` | 原生后端/交付 owner | A4；manifest 解析归属需先定 | F3-02；无 PATH/cwd 探测；engine hash/版本精确；无 Node fallback | 保持 Node 默认；旧包可用 |
| A6 | 公开原生选择点：`backend_tsgo` 对 bin 可见，提供 `run_stdio_native` 或公开构造器；移除 `dead_code` allow | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:46`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:52-54, 95, 118`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:93-99` | 集成 owner | A5 | `main.rs` 能构造原生 backend；无 Node import | 保留 Node `serve_real` 分支到切换 commit 前 |
| A7 | F1 查询缺口裁决：symbols/rename/component/event/外部 `.d.ts` 要么实现，要么在能力/合同里显式降级并有测试 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:403-407, 430-452`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:532-620` | 原生后端 owner | A4 | G5：CRITICAL/HIGH 清零；无“空+Supported”掩盖缺证 | Node 默认 |

### P6-B：交付与安装

| # | 变更 | 文件:行 | Owner | 依赖 | 验收门禁 | 回滚 |
| --- | --- | --- | --- | --- | --- | --- |
| B1 | 实现 schema v2 打包器（manifest/index/provenance/license/deterministic tar），不要求 Node | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:382-468`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/bin/wxml-lsp-package.rs:15-20`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:259-273` | 交付 owner | A4/A5 | F3-03；两次构建同 SHA-256；无 `node_modules`/JS/`server.js`；`SHA256SUMS` 精确 | 保留 v1 打包器/旧 release |
| B2 | 原生资产 provisioning：`assets.lock.json`、Rust 下载/解包/逐文件校验、`engine/lib/tsc` SHA、typings、许可证、`readelf`/glibc 记录 | 新增 `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/assets.lock.json`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/acquire_v2.rs` 扩展、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/provisioning.rs` | 交付/构建 owner | B1 | F3-03/13；真实 7.1.0-beta 资产；离线缓存命中/缺失 | 旧 release 可继续用 |
| B3 | v2 归档缓存或明确离线范围：实现 `cache/<version>/archive.tar.gz` 重新校验；否则将 F3-12 的“缓存命中”降为显式 `--from-dir` 并获批 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/acquire_v2.rs:8-10, 60-62`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/cache.rs` | 交付 owner | B1 | F3-12；离线安装/升级/回滚；损坏缓存失败；无网络 | 保留 `--from-dir` 路径 |
| B4 | legacy 只读命令去 Node：`current` 不执行 `check_node`；`verify` 不执行 `verify_install`；`env` 只输出 `WXML_LSP_BIN`；legacy 仅 `list`/inspect | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/bin/wxml-lsp-install.rs:702-711, 750-770, 802-818`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify.rs:247-330` | 交付 owner | B1 | F3-01/15；clean PATH 下 current/verify/env 成功；tripwire 不触发；env 无 `WXML_TYPE_BACKEND` | 旧 installer 仍可人工回滚 |
| B5 | legacy 迁移与 native rollback 语义收口：明确 switch 后不可回 legacy；实现前滚恢复/补 state；文档写清“legacy 仅只读、不自动回滚” | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/native_install.rs:687-689, 810-831, 1305-1318`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:873-960` | 交付/集成 owner | B4 | F3-08/09；迁移成功一次切换、旧树字节不变；失败/中断有明确恢复矩阵；无 Node | 旧包人工重装 |
| B6 | 安装器/打包 CLI 的 v2 分派、`--from-dir`、`WXML_LSP_BIN` 优先级、`recover` 全链路真实资产验收 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/bin/wxml-lsp-install.rs:267-363, 476-530`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_cli.rs:403-526` | 交付 owner | B1-B5 | F3-04/05/06/07/08/09/12/16；真实包；崩溃注入；并发 | 保留 v1 CLI 分支 |
| B7 | 构建脚本原生路径：去掉 `require_tool node/npm`、`npm ci/build/prune`、`wxml-type-backend`；改为 provision + v2 package | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/scripts/build-release.sh:155-171, 224-244, 266-272` | 构建/CI owner | B1/B2 | 无 Node 环境两次确定性构建；`verify-package` 通过 | 旧脚本/旧 release 保留 |
| B8 | CI clean-env + 进程审计：新增 job 只装 Rust/原生资产；`strace` exec 审计；PATH tripwire；无 Node 构建/测试 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/.github/workflows/ci.yml:1-31`；新增 `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/scripts/verify-no-node.sh` | CI/安全 owner | B4/B7 | F3-01/02/03/12；exec 集合；无网络；无 Node | 保留原 CI job |

### P6-C：默认切换与收尾

| # | 变更 | 文件:行 | Owner | 依赖 | 验收门禁 | 回滚 |
| --- | --- | --- | --- | --- | --- | --- |
| C1 | 默认 `--stdio` 构造原生 backend；NodeBackend 不再参与 release 构建（feature/test-only 或 test-support） | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:93-99`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:44`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/Cargo.toml` | 集成 owner | A5/A6/B1-B7 | G6；默认原生；无 Node 进程；旧 TS 测试无运行职责 | 上一 native 版本；或人工旧包 + `WXML_LSP_BIN` |
| C2 | env/config 收口：`WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` 不再读取/激活；`WXML_ANALYZE_TIMEOUT_MS` 映射原生 deadline；`WXML_TEMPLATE_CHECK` 映射 `JsCheckPolicy` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:28-35`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/config.rs:155-217`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:194-203` | 集成 owner | C1 | F3-15；设置 legacy env 不改变后端；无 Node；配置文档一致 | 无自动回滚；legacy env 永久失效 |
| C3 | Zed 停止转发 legacy env；`WXML_LSP_BIN` 必须指向包内 `bin/wxml-lsp`；更新 README | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/src/lib.rs:18-25, 684-698`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/README.md:31, 56-70, 183-187` | Zed owner | C1 | F3-15；Zed 不转发 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`；标准 Dev Extension 安装 | 上一扩展版本 |
| C4 | 测试隔离：把 fake C1 transport 移到 test-support，迁移 12 个 `backend_node` import 和 1 个真实 Node stdio 测试文件（共 13 个 Node 测试文件）；真实 Node 测试不再参与默认 `cargo test`；旧测试只保留为待删材料 | 13 个文件见 §3.7；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/Cargo.toml` | 测试/集成 owner | C1/A3 | G6；`cargo test --workspace` 无 Node；旧测试无运行职责 | 旧测试从 tag 恢复 |
| C5 | 文档状态：README/LSP/delivery/Zed 改为原生安装；ADR-0008 在 G6 后 accepted；F0/F1/F2/F3 仍按各自状态 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/README.md:18-22, 34-38, 56-77, 92-98, 216, 225-228`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/README.md:34-45`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/README.md:9-125`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/README.md:31-70`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/README.md:31-33`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/adr/0008...:4` | 文档 owner | C1-C4 | G6；文档无“默认 Node”陈述；历史证据不改写 | 文档 revert；不影响运行 |

---

## 8. P7 有序变更清单

| # | 变更 | 文件:行 | Owner | 依赖 | 验收门禁 | 回滚 |
| --- | --- | --- | --- | --- | --- | --- |
| P7-1 | 台账归零：270→0；按 green/retired 结果更新 16 个 `MIGRATED_CASES`；23 ignore/2 silent/3 blocked gates 关闭 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/plans/tsgo-test-ledger.json`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity.rs:154-305`；`native_gates.rs`；`symbols_edits.rs` | 测试/集成 owner | P6-A1..A3 | G7；无 `planned`；所有旧断言有原生替代或裁决 | 不删除旧测试；保留 tag |
| P7-2 | 归档旧 TS oracle：把 `backends/typescript/test/**` 字节移到只读证据树，更新 ledger 路径/hash；或改为只校验记录 hash | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/ledger.rs:1425-1445, 1569-1578`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity/fixture_hash.rs:49-70`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-test-contract.md:31-38` | 测试/文档 owner | P7-1 | `cargo test -p wxml-lsp --test native_parity` 仍绿；旧字节/hash/许可可审计 | 恢复归档树 |
| P7-3 | 删除 `backend_node` 模块及 13 个 Node 测试文件引用（12 个 import + stdio）；fake transport 移到 test-support/原生 fake | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/**`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:44`；13 文件见 §3.7 | 集成 owner | P7-1/P7-2/C4 | G7；仓库无 `backend_node` 活跃引用；`cargo test` 绿 | git revert/tag |
| P7-4 | 删除旧 TS 实现/测试/Node 构建：`backends/typescript/{src,test,scripts,package.json,package-lock.json,tsconfig*.json}`；samples 移到统一 fixture | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/**`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/tests/fixtures/README.md:233-236` | 后端/文档 owner | P7-2；samples hash 核对 | 仓库无 Node 构建/测试；35 samples byte-exact；`git grep` 活跃引用清零 | tag/归档 |
| P7-5 | 删除 v1 打包/Node 包代码：`package.rs`、`wxml-lsp-package` v1 布局、`acquire.rs`/`cache.rs` v1、`verify.rs` Node 检查；先拆共享 helper | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:382-468`；`bin/wxml-lsp-package.rs:15-20`；`manifest.rs:23-95`；`install.rs:257-...`；`verify.rs:247-330` | 交付 owner | P7-1/B5 | 仓库无 v1 写入路径；legacy 只读投影仍可识别/迁移（如保留） | tag/旧 release |
| P7-6 | 清理活跃文档/CI/Zed 的 Node 指令；历史计划/证据/ADR 保留并标历史身份 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/README.md`、`crates/*/README.md`、`editors/zed/README.md`、`docs/README.md`、`docs/reference/*`、`docs/evidence/**` | 文档 owner | P7-3..P7-5 | `git grep` 活跃路径无 Node 构建/运行指令；历史证据仍可追溯 | 文档 revert |
| P7-7 | 最终复验：全量门禁、两次确定性 release、install/upgrade/rollback、Linux Zed smoke、clean-env exec 审计、provenance/许可 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/scripts/build-release.sh`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/.github/workflows/ci.yml`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-p6-cutover-audit.md` 后续证据 | 集成/交付/安全 owner | P7-1..P7-6 | G7；包/进程树/仓库均无 Node；最终报告列用例/断言/差异/平台限制 | 保留上一 release/tag |

---

## 9. 剩余 BLOCKED 项（不得声称已解决）

| ID | 阻塞 | 证据 | 退出条件 |
| --- | --- | --- | --- |
| BLOCK-P6-01 | G5 未关闭；270 planned，16 分区空 | `ledger.rs:21-27`；`native_parity/*.rs:13` | P6-A1..A3 |
| BLOCK-P6-02 | 原生适配器不可被 bin 构造；无 engine 发现 | `backend_tsgo/mod.rs:46,95,118` | P6-A5/A6 |
| BLOCK-P6-03 | `noLib:true`、无 typings/compilerOptions | `vfs.rs:685-696`；`input.rs:214-263`；`backend_tsgo/input.rs:194-203` | P6-A4 |
| BLOCK-P6-04 | 无 v2 打包器/真实资产 provisioning | `package.rs:382-468`；`manifest_v2.rs:259-273` | P6-B1/B2 |
| BLOCK-P6-05 | 无 v2 缓存；离线仅 `--from-dir` | `acquire_v2.rs:8-10, 60-62` | P6-B3 或获批范围 |
| BLOCK-P6-06 | legacy `current`/`verify` 执行 Node；legacy `env` 导出 `WXML_TYPE_BACKEND` | `bin/wxml-lsp-install.rs:702-711, 750-770, 802-818` | P6-B4 |
| BLOCK-P6-07 | 迁移 switch 后无法恢复 legacy current | `native_install.rs:687-689, 1305-1318` | P6-B5 合同裁决/实现 |
| BLOCK-P6-08 | 截至 `69e13fb` 无 clean-PATH/strace 进程审计；审计结束时出现未跟踪 `scripts/verify-no-node.sh`/`no_node_guard.rs`，未验证 | `native_install.rs:19`；`native_cli.rs:371-396` | P6-B8 + 复核并行 clean-env 工作 |
| BLOCK-P6-09 | F1 hover/definition/completion/rename/references/component/event 缺口 | `tsgo-native-contract.md:532-620`；`backend_tsgo/mod.rs:403-407,430-452` | P6-A7 |
| BLOCK-P6-10 | 构建仍要求 Node/npm | `build-release.sh:157-171, 224-244` | P6-B7 |
| BLOCK-P7-01 | 189 个无证据 planned；270 未归零 | `tsgo-test-ledger.json`；`2026-11-19` plan §2 | P7-1 |
| BLOCK-P7-02 | 23 ignore + 2 silent + 3 blocked gates 未原生化 | `native_gates.rs:1-7,178-190`；`semantic_tools.rs:2082-2084,2569-2571`；`symbols_edits.rs:14-31` | P7-1 |
| BLOCK-P7-03 | parity 仍读旧 TS 树/冻结 hash | `ledger.rs:1425-1445,1569-1578`；`fixture_hash.rs:49-70` | P7-2 |
| BLOCK-P7-04 | v1 打包/Node 包代码与 v2 共享 helper 未拆 | `manifest.rs:23-95`；`package.rs:382-468`；`install.rs:257-...`；`verify.rs:247-330` | P7-5 |
| BLOCK-P7-05 | 仅 Linux x86_64；Windows/UNC/非 UTF-8 根拒绝 | `backend_tsgo/mod.rs:36-41`；F3-13 planned | 保持不支持或另立平台门禁 |
| BLOCK-P7-06 | F3-01..16 全部 planned；B3/B6 BLOCKED；B1/B2/B4/B5/B7/B9 PARTIAL；B10 OPEN | `tsgo-delivery-contract.md:1149-1210` | P6-B1..B8 + F3 验收 |
| BLOCK-P7-07 | Zed 标准 UI/原生包冒烟未做；W4 是 v1 Node 包冒烟 | `docs/evidence/full-feature-final.md`；F3-14 | P7-7 |
| BLOCK-P7-08 | 无发布者签名；仅 TLS+摘要 | ADR-0007；B8 accepted risk | 接受风险或新增签名 ADR |

**明确不声称：**

- 不声称 P6 可以开始默认切换。
- 不声称 P7 可以删除 `backend_node` 或 Node 打包。
- 不声称 F3-01～F3-16 中任何一项已关闭。
- 不声称 23 个 ignored tests 或 2 个 silent returns 已迁移。
- 不声称原生后端在真实引擎上通过了 AC-001..AC-013；本次审计没有运行测试。
- 不声称 legacy Node 包可以自动 rollback；当前实现明确拒绝。
- 不声称 Windows/macOS/aarch64 支持。

---

## 10. 附：审计证据与未执行项

- 读取的冻结值：`docs/plans/tsgo-test-ledger.json` 的 `counts`、`migration_evidence`、`ignored_rust_tests`、`silent_skip_gates`；`ledger.rs:21-27`；`native_gates.rs:178-190`；`fixture_hash.rs:49-70`。
- 读取的合同状态：F0/F1 `draft / not accepted`；F2 `draft`；F3 `draft / not accepted / not implemented`；ADR-0008 `proposed`。
- 未执行：`cargo test`、`cargo build`、`npm ci/test/build`、`strace`、Zed smoke、真实引擎查询、安装/升级/回滚。所有“通过/失败”均未由本次审计产生。
- 未产生：生产代码、测试、Cargo、ADR、计划、合同修改；未 commit；未删除 Node；未切换默认后端。
- 行号说明：本文以 `69e13fb` 快照为准；当前 worktree 已推进到 `6ab1bc6`，仅 completion 相关 5 文件有差异；引用这些文件时需重新核对行号。
