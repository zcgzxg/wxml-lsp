# Rust + tsgo 迁移 P0：工作区与工具链基线（2026-11-17）

- 记录时间：2026-11-17T21:59:39+08:00
- 执行工作区： /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
- 分支 / HEAD：`tsgo/migration` / `b9616c557e54a06b7d756e516e6dd7c4016664cf`
- 结论：P0 工作区基线 PASS；P2/P3 入口仍被 P1-A/B/C 阻塞（见第 6 节）。

本文件只记录只读盘点，不运行构建、测试或真实 Node 门禁，不修改源码、测试、Cargo、计划或 ADR，不提交。测试执行由其他 worker 负责。

## 0. 判定摘要

| 项 | 结果 | 证据 |
| --- | --- | --- |
| 工作区、分支、HEAD、clean 状态 | PASS | 第 1.1 节 |
| 计划与 ADR-0008 存在、SHA-256 | PASS | 第 1.2 节 |
| workspace / 工具链 / lint / CI 盘点 | PASS | 第 2 节 |
| `Backend` 与 `backend_node` 边界审计 | PASS | 第 3 节 |
| worktree / 分支命名不冲突 | PASS | 第 4 节 |
| 后续 worker 预检清单 | PASS | 第 5 节 |
| ADR 索引一致性 | PARTIAL | 表格含 0008；阅读顺序和局部链接仍停在 0007 |
| P0 dirty 行为增量表 | OPEN | 修复已提交并包含在 BASE，但逐项行为归属表尚未产出；22:04 主工作区又出现 3 个并发 modified 路径 |
| 同 worktree 并发写入 | OPEN | 22:04 出现另一 worker 的 delivery-spike 证据文件；见 B10 |
| P2/P3 入口 | BLOCKED | 依赖 P1-A/P1-B/P1-C 三路 PASS（计划 G1） |
| G5 质量门禁可复现性 | BLOCKED | 无 `deny.toml`；CI 未接 `--locked`、coverage、audit/deny |

## 1. 工作区、基线与文档

### 1.1 Git 状态

实际输出（在 tsgo worktree 内）：

```text
$ pwd
/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
$ git rev-parse --abbrev-ref HEAD
tsgo/migration
$ git rev-parse HEAD
b9616c557e54a06b7d756e516e6dd7c4016664cf
$ git status --porcelain
（0 行）
$ git rev-list --left-right --count master...tsgo/migration
0	1
$ git merge-base --is-ancestor master HEAD && echo "master is ancestor of HEAD"
master is ancestor of HEAD
```

- BASE 是 `b9616c5`，其父提交为 `4d59ca7`；`master` 是 `4d59ca7`，因此 BASE 已包含 master 上已提交的修复。
- 主工作区（只读检查）：HEAD `4d59ca7`；2026-11-17T21:59 首次检查 `git -C /home/zxg/code/rust/wxml-lsp status --porcelain` 为 0 行。
- 2026-11-17T22:04 复核发现主工作区出现 3 个 modified 路径： crates/wxml-lsp/src/backend_node/mod.rs、 crates/wxml-lsp/src/dependencies.rs、 crates/wxml-lsp/src/project_host.rs。这是 main worktree 的并发工作，不属于 BASE；本任务未读取/修改其内容、未纳入本分支。P0 收尾必须处理（见 B6）。
- 同一时间本工作区出现两个 untracked 证据文件：本文件与另一 worker 的  docs/evidence/2026-11-17-tsgo-delivery-spike.md。后者不属于本任务，未读取/修改；说明本 worktree 存在并发写入（见 B10）。
- `4d59ca7` 的提交统计为 `19 files changed, 1606 insertions(+), 563 deletions(-)`；这 19 个文件与计划第 2 节记录的历史 dirty 路径逐项对应（TS 分析/Behavior/compiler/events/protocol、3 个 TS 测试、`backend_node` 的 request/response/tests、completion、hover、metadata、server、component_intelligence、discovery）。结论：历史 19 个 dirty 修复已提交并包含在 BASE；但“dirty 行为增量表”本身仍待 P0 补齐（见 6.6）。

计划要求 P0 记录 HEAD、分支、状态与逐文件 hash，见  docs/plans/2026-11-17-rust-tsgo-migration.md:41 与 :50；本记录在第 1.3 节补记了 19 个已提交修复文件的 BASE 摘要。

### 1.2 计划与 ADR

| 文件 | 大小 | 行数 | SHA-256 |
| --- | ---: | ---: | --- |
|  docs/plans/2026-11-17-rust-tsgo-migration.md | 50.3K | 497 | `3c451c10fb34554c765fcf63042ef286bc686b0ada01098a7c6e5c72c2dfce71` |
|  docs/adr/0008-rust-tsgo-no-node-backend.md | 9.2K | 108 | `e3463cd94f7c8bc8771aece1092562661e2253b37f62e4d0cdd3e5074d4da37e` |

- 两个文件均由 BASE 提交 `b9616c5` 引入；该提交只增加 3 个文件（计划、ADR、ADR 索引 1 行）。
- ADR 状态为 `proposed`，与索引表一致；ADR 自身声明接口与能力验证完成前不升级为 `accepted`，见  docs/adr/0008-rust-tsgo-no-node-backend.md:3 与 :7。
- 复核复读：两次 `sha256sum` 结果一致，说明本次盘点期间未被改动。

### 1.3 历史 dirty 修复文件在 BASE 的 SHA-256

`4d59ca7` 的 19 个文件在 BASE 工作区的实际摘要（用于后续核对修复没有被静默替换）：

```text
ddbe7223dde391591ce503a739a1bb1735cd63043049f7af807df37dfd3eb19b  backends/typescript/src/analysis.ts
89b8b6eda65f4bb23816a6651d3e1d76ef62597d7cb9acfa56e28c675ad2a171  backends/typescript/src/behaviors.ts
63b10db783b73bea628edb0628279b44c7d4bd7f138fe28d1c668c988ae450e5  backends/typescript/src/compiler.ts
74a8c5a4a077002260773e2c9c4bafb5df54b304229fb6775e952073ecfaf260  backends/typescript/src/events.ts
1a61247f8793da4dc778c2ccdf2dbc8a5cae328d0b242571ef7c8c89fd168c0e  backends/typescript/src/protocol.ts
5a62a6f19e94bbafbec498206b91107c63acbf2443e1261a56e2e86ff3546531  backends/typescript/test/analysis.test.ts
87cde2394a04a2d44cf7ca2bf8f3d9fd9869d157af0b72cd391f2bab6af18619  backends/typescript/test/compiler.test.ts
3c81432ae8617fa417942dee15d4e23d93875e19b63fa0ca1902fb22ca211445  backends/typescript/test/events.test.ts
18f278f250a2003ab9f025b44b23592557ef9f8b708cb7ca60182fb70aa6a6b7  crates/wxml-lsp/src/backend_node/request.rs
c38a196c9fd24f652a441dcee9e298338d684d9c581e32ff3fe7feece50c2bc0  crates/wxml-lsp/src/backend_node/response.rs
c33f8b7e49283bc0cc85e02fb5270c8a64d6a92677dac1605456f909c55100a3  crates/wxml-lsp/src/backend_node/tests.rs
411a6a0320e2d186b937f6630f95041a3b46d9cff3bd6c6b4e4864afeea7277f  crates/wxml-lsp/src/completion/catalog.rs
88ea9e6385a39fac86bbf059ec1c8cf54d287809a2cda205f7e3fada11544959  crates/wxml-lsp/src/completion/static_items.rs
65f740d8457fbf1b07ea58931e4fd5faea38b784c77a4f5e8dc2267962d38f4a  crates/wxml-lsp/src/handlers/hover.rs
905771d48fffb5be08e59155b3512d5ea7eba8d1cd3c3fd07c8dc208e9305ae5  crates/wxml-lsp/src/metadata/docs.rs
6a7c8750c8df19767c49103b8ca06ce3a6c4f4572a63a34e48c38744f3b955b0  crates/wxml-lsp/src/metadata/tests.rs
476a4fec677e479981e5a5e02433ebc1c60c6c929855f8a9187f7f5ec4e9295f  crates/wxml-lsp/src/server.rs
0151795d494fba393f2be25c3ab4f486b276fb9594e4b1a81bef314ce1facd56  crates/wxml-lsp/tests/component_intelligence.rs
5611a542d8ecbf75e72b9d34f7d07e283b8c93d4d565f1e69bddd5b001b543e9  crates/wxml-project/src/discovery.rs
```

## 2. 仓库与工具链表面

### 2.1 Workspace Cargo

关键配置，见  Cargo.toml：

- 第 2 行：`resolver = "3"`。
- 第 3 行：`members = ["crates/*"]`。新增  crates/wxml-backend-tsgo/Cargo.toml 会被自动纳入 workspace；不需要改 members 数组，但根 Cargo.toml 与新 crate 清单仍由集成负责人单写（计划 :147）。
- 第 7 行：`edition = "2024"`；第 8 行：`rust-version = "1.85"`；第 9 行：`publish = false`。
- 第 33–35 行：release 使用 `lto = "thin"`、`codegen-units = 1`。
- 第 37–43 行 rust lints：`unsafe_code = "deny"`、`unused_qualifications = "deny"`、`missing_docs = "warn"`、`missing_debug_implementations = "warn"`、`unreachable_pub = "warn"`。
- 第 45–60 行 clippy lints：`all = "deny"`；`pedantic`/`nursery`/`cargo` 为 warn；`unwrap_used`/`expect_used`/`todo`/`unimplemented` 为 warn；`dbg_macro = "deny"`。
- 所有现有 crate 的 Cargo.toml 都以 `[lints] workspace = true` 继承。新 crate 必须同样声明，否则不会继承上述 lint。
- `Cargo.lock` 已跟踪；`cargo metadata --locked --no-deps` 通过，说明当前 lockfile 与清单一致。
- `cargo fmt --all --check` 通过（只读检查，未改文件）。

### 2.2 现有 crate 依赖边界

根注释声明的目标分层，见  Cargo.toml:12–18：

```text
wxml-lsp      -> wxml-types, wxml-project, wxml-syntax, wxml-text
wxml-types    -> wxml-syntax, wxml-text
wxml-project  -> wxml-text
wxml-syntax   -> wxml-text
wxml-text     -> (none)
```

实际清单：

| crate | 关键依赖 | 位置 |
| --- | --- | --- |
| wxml-text | 无 |  crates/wxml-text/Cargo.toml |
| wxml-syntax | wxml-text；dev proptest |  crates/wxml-syntax/Cargo.toml:10 |
| wxml-project | wxml-text + serde/serde_json/thiserror |  crates/wxml-project/Cargo.toml:10–13 |
| wxml-types | wxml-syntax, wxml-text + serde/thiserror |  crates/wxml-types/Cargo.toml:10–13 |
| wxml-lsp | wxml-project, wxml-syntax, wxml-text, wxml-types + serde/serde_json/thiserror；dev proptest |  crates/wxml-lsp/Cargo.toml:10–19 |
| wxml-delivery | 无 wxml-* path 依赖；flate2/fs2/reqwest/serde/serde_json/sha2/tar/thiserror |  crates/wxml-delivery/Cargo.toml:10–17 |
| wxml-fixtures | 仅 dev-deps：serde_json、wxml-text；`publish = false` |  crates/wxml-fixtures/Cargo.toml |

计划规定的目标方向：`wxml-lsp → wxml-backend-tsgo → wxml-types`；新后端可使用现有 text/syntax 工具，但不能反向依赖 `wxml-lsp` 或复制整个 `ProjectHost`，见  docs/plans/2026-11-17-rust-tsgo-migration.md:109。

### 2.3 Toolchain

实际输出：

```text
rustc 1.100.0 (0ef1bc386 2026-11-09)
cargo 1.100.0 (3acc79c0c 2026-11-05)
stable-x86_64-unknown-linux-gnu (overridden by '.../tsgo-migration/rust-toolchain.toml')
node v24.22.2
npm 11.19.1
cargo-llvm-cov 0.9.1
cargo-audit 0.23.0
cargo-deny 0.21.0
cargo-nextest: absent
```

-  rust-toolchain.toml：`channel = "stable"`、`components = ["clippy", "rustfmt"]`、`profile = "minimal"`。channel 未固定 patch 版本，CI 也使用 `dtolnay/rust-toolchain@stable`，可重复性弱于“固定工具链”，属于后续 G5 需冻结项。
-  clippy.toml：`msrv = "1.85"`，测试内允许 unwrap/expect/panic/indexing，域名词汇白名单包含 WXML/WXSS/TypeScript/miniprogram 等。
-  rustfmt.toml：`edition = "2024"`、`max_width = 100`、Unix newline。
- 已安装 `wasm32-wasip2` target（`rustup component list --installed`），满足 `scripts/build-release.sh` 对 Zed wasm 的前置检查。
- Node/npm 当前存在，但最终 NF-01/NF-02 要求在无 Node 环境验证；本记录不把“当前有 Node”当作迁移依赖。

机器资源（用于编译并发建议）：

```text
nproc: 24
内存: 30 GiB total / 11 GiB available（swap 16 GiB free）
/tmp: tmpfs 16G / 7.7G free
/: 290G free
worktree/target: 3.6G（已存在，gitignored）
CARGO_TARGET_DIR / CARGO_BUILD_JOBS / RUSTFLAGS: 未设置
```

### 2.4 CI、覆盖率、deny 与构建脚本

 `.github/workflows/ci.yml` 当前只有 4 个步骤：

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo doc --workspace --no-deps
```

- CI 没有 setup-node / npm 步骤；因此 CI 不构建 Node 后端，也不运行 23 个 `#[ignore]` 真实 Node 门禁。
- CI 四条命令都未带 `--locked`，与计划第 7 节要求的 `--locked` 命令不一致，见  docs/plans/2026-11-17-rust-tsgo-migration.md:344–356。
- CI 没有 coverage、`cargo audit`、`cargo deny`、原生资产准备或 no-node guard 步骤。
- 仓库无 `deny.toml`（`ls deny.toml .deny.toml` 均不存在）；`cargo deny check` 的许可证/来源策略尚未冻结。ADR/证据历史中的 “deny advisories ok” 不等于当前策略文件存在。
- `cargo-llvm-cov` 已安装且可执行，但未接入 CI。
-  `.gitignore` 只忽略 `/target` 与 `/.codegraph/`；没有其他产物目录规则。
-  `.gitattributes` 固定 `tests/fixtures/**/*.wxml` 为 LF，CRLF fixture 与 `backends/typescript/samples/crlf-script/index.ts` 保持 byte-exact。
- `scripts/build-release.sh` 当前强依赖 Node/npm：
  - 第 157–158 行 `require_tool node` / `require_tool npm`；
  - 第 104–108 行比较 Rust 与 Node 的 `PROTOCOL_VERSION`；
  - 第 224–241 行执行 `npm ci`、`npm run check`、`npm run build`、`npm prune` 并复制 `dist`/`node_modules`。
- Node 后端自身固定 `node >= 24.0.0`、`typescript 6.0.3`、`miniprogram-api-typings 5.3.1`，见  backends/typescript/package.json:8、:29–30。

## 3. `Backend` 与 `backend_node` 边界审计

### 3.1 新后端必须保留的 C0 契约

`Backend` trait 定义见  crates/wxml-types/src/backend.rs:343。其文档明确：同步阻塞边界；调用方负责专用线程/`spawn_blocking` 与读超时；实现不得无限阻塞；查询来自多个任务，因此必须 `Send + Sync` 并自行同步（:335–342）。

| 方法 | 行 | 语义要求 |
| --- | ---: | --- |
| `epoch()` |  crates/wxml-types/src/backend.rs:345 | 返回将回答请求的 backend generation；替换实例或失效必须前进 |
| `hover()` | :348 | 返回 `HoverResult`：contents、markup、原始模板 range、CapabilityStatus、TypeQuality |
| `diagnostics()` | :351 | 返回原始模板坐标的 diagnostics + 检查状态 |
| `definition()` | :357 | 默认 `Unsupported`；空结果 + `Supported` 才表示“已检查且无定义” |
| `completion()` | :371 | 默认 `Unsupported`；只覆盖表达式成员/事件 handler，静态模板补全不走后端 |
| `symbols()` | :387 | 默认 `Unsupported`；必须区分“完整空结果”和“未实现” |
| `invalidate()` | :402 | 缓存型后端 MUST 覆盖：清空旧项目缓存并推进 epoch，使迟到结果被发布屏障丢弃；默认 no-op 只适用于无状态后端 |
| `capabilities()` | :407 | 默认全 unsupported；新后端必须显式 opt in |

必须保留的结果/状态语义：

- `BackendError`：`Unavailable` / `Protocol` / `Timeout` / `Cancelled` / `Internal`，见  crates/wxml-types/src/backend.rs:33。不得把后端失败映射为“成功但零结果”。
- `CapabilityStatus` 六态：`Supported` / `Partial` / `Unsupported` / `Disabled` / `Unavailable` / `Unknown`，见  crates/wxml-types/src/status.rs:30。空 diagnostics 只有在 `Supported` 时才表示 clean，见  crates/wxml-types/src/backend.rs:140。
- `HoverResult`  :89、`DefinitionResult`  :171、`DiagnosticsResult`  :140、`CompletionResult`  :289；`SymbolResult`（含 `complete=false` 时禁止生成编辑）见  crates/wxml-types/src/symbols.rs:176。
- `BackendEpoch` 由 `BackendId + u64` 组成，见  crates/wxml-types/src/contract.rs:327；`RequestContext` 绑定 project/view revision、epoch 与不可变 `SourceSnapshot`，见  crates/wxml-types/src/context.rs:111。
- 坐标：对外是 `wxml_text` 的 UTF-8 字节 `SourceRange`；native/LSP 的 UTF-16 与字符串池偏移必须在适配层完成转换，不能把无单位整数直接透传（计划 :130）。

### 3.2 当前 Node 适配器

入口与实现：

- `NodeBackend` 结构： crates/wxml-lsp/src/backend_node/mod.rs:107。
- 构造：`:134` `NodeBackend::new` 使用 `StdioNode::start`；`:148` `with_transport_factory` 供测试/嵌入方注入 transport。
- 分析编排与 LRU 缓存：`:187` `analyze`；缓存上限 `MAX_CACHED_ANALYSES = 8`，见 :66；缓存键包含 file、source/view/project revision、generation、epoch。
- `Backend` 实现：`invalidate` :432、`epoch` :445、`hover` :449、`diagnostics` :477、`definition` :513、`completion` :539、`symbols` :649、`capabilities` :777。
- `Drop` 关闭 transport：:787。
- 诊断降级：`diagnostics` 在类型后端失败时仍返回已解析的语法诊断并标记 `Unavailable`（:477–511）；这是必须保留的“静态降级”行为。
- 失败/重启策略：连续失败后进入 cooldown，默认 2 秒（ config.rs:23），`invalidate` 不重置失败计数（mod.rs:432–443）。

模块划分与 P7 可删除性：

| 模块 | 行数 | 当前职责 | 迁移/删除判断 |
| --- | ---: | --- | --- |
|  crates/wxml-lsp/src/backend_node/mod.rs | 797 | NodeBackend 编排、缓存、epoch、cooldown、C0 方法实现 | 语义需迁移；结构可删除 |
|  crates/wxml-lsp/src/backend_node/config.rs | 401 | Node 路径/环境变量、超时、typings、JS 检查策略 | Node 专属；`WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`/`WXML_ANALYZE_TIMEOUT_MS`/`WXML_TEMPLATE_CHECK` 需替换 |
|  crates/wxml-lsp/src/backend_node/process.rs | 568 | `NodeTransport` trait、`StdioNode` 启动 `node`、读写线程、shutdown | Node 专属；可整体删除 |
|  crates/wxml-lsp/src/backend_node/protocol.rs | 191 | NDJSON JSON-RPC 帧、`PROTOCOL_VERSION = 2` | Node 专属；native 是 Content-Length JSON-RPC，不能复用 |
|  crates/wxml-lsp/src/backend_node/request.rs | 1331 | 从 `PreparedView` 构建 C1 `analyze` JSON（fragments/events/locals/guards/components） | 输入语义需迁移，wire 结构删除 |
|  crates/wxml-lsp/src/backend_node/response.rs | 2714 | 解析 C1 响应并映射到 C0 DTO；definition/completion 上限与截断规则 | 映射/安全规则需迁移，wire 依赖删除 |
|  crates/wxml-lsp/src/backend_node/completion.rs | 220 | 解析 protocol v2 `complete` 响应 | 逻辑可重写；替换为 native 查询 |
|  crates/wxml-lsp/src/backend_node/tests.rs | 1322 | Node fake transport 单元测试 | 随实现替换；断言需迁入 Rust 新测试 |

合计 7,544 行。除 `mod.rs` 的编排规则和 `response.rs` 的映射/安全规则外，其余 Node wire/process 逻辑不进入新后端。

Node 专属的 TypeScript 实现位于  backends/typescript/（`src/` 含 analysis/behaviors/compiler/events/protocol/rewrite/symbols/shadow 相关模块，`test/` 22 个文件，`samples/` 与 package 文件）。计划要求先迁移语义和测试，再在 P7 删除，见  docs/plans/2026-11-17-rust-tsgo-migration.md:239、:404–410。

### 3.3 新后端输入与读边界

当前 `NodeBackend` 直接持有 `Arc<dyn ProjectHost>`，并从 `PreparedView` 取输入。新 crate 不能依赖 `wxml-lsp`，因此计划要求由 `wxml-lsp/src/backend_tsgo.rs` 提供 `ProjectHost → InputProvider` 的薄适配器（计划 :95、:109）。

必须保留的输入字段，见  crates/wxml-lsp/src/project_host.rs:345：

- `view_id`、`target`（无 owner/script 时为空）；
- `files`、`compiler_options`（有效编译选项）；
- `fragments`、`components`、`events`（均按源顺序且有上限）；
- `project_root`（只用于本地读边界，绝不发给 LSP 客户端）；
- `syntax_diagnostics`、`status_hint`（类型后端不可用时仍要发布）；
- `source_complete`（false 时 symbols 永不完整）。

当前 `ProjectHost` trait 见  crates/wxml-lsp/src/project_host.rs:373。计划还要求 P0 核对当前 TS 后端实际 `typeRoots` 读策略，并把“只读固定两个目录”当作待验证假设而非既成事实（计划 :122）。

### 3.4 构造点、导出与耦合测试

- 二进制构造： crates/wxml-lsp/src/main.rs:10 导入 `NodeBackend`，:96–97 用 `NodeBackendConfig::from_env()` 构造。
- 模块导出： crates/wxml-lsp/src/lib.rs:44 `pub mod backend_node`。
- 生命周期失效调用： crates/wxml-lsp/src/handlers/lifecycle.rs:753 `self.backend.invalidate()`；该调用应保持 backend-neutral。
- `server.rs` 只持有 `Arc<dyn Backend>`，不承接 Node wire 细节（计划 :403）。
- 直接导入 `backend_node` 的集成测试有 12 个文件：`full_feature_matrix.rs`、`lsp_p7.rs`、`multi_app_projects.rs`、`node_backend.rs`、`performance_bench.rs`、`q2_recovery.rs`、`q2_stability.rs`、`q2_support/mod.rs`、`real_node.rs`、`semantic_tools.rs`、`wave5_acceptance.rs`、`zed_sync.rs`。删模块前必须逐项迁移其断言，不能以删除文件代替迁移。

### 3.5 Node 路径是否仍被测试/构建要求

是，当前仍被要求：

- 23 个 `#[ignore]` 真实 Node/stdio 门禁全部要求 `WXML_TEST_NODE=1` 和已构建的 `dist/server.js`：`real_node.rs` 5、`stdio_type_loop.rs` 9、`wave5_acceptance.rs` 7、`performance_bench.rs` 1、`q2_stability.rs` 1。分布与 W4 证据的 23 个 ignored 一致。
- 2 个未标记 ignored 的 `semantic_tools.rs` 测试在缺少 `dist/server.js` 时直接 `return` 并打印 skip： crates/wxml-lsp/tests/semantic_tools.rs:2083 与 :2570。这会在无 Node 时静默通过，计划 :305 要求改为 native 必备门禁。
- `scripts/build-release.sh` 的 release 构建强依赖 Node/npm（:157–158、:224–241）。
- `wxml-delivery` 的 manifest 仍要求 `node.minimum_version` / `node.backend_version`，产物布局含 `bin/wxml-type-backend/server.js` 与 `node_modules`，见  crates/wxml-delivery/src/manifest.rs:43–49、:128–131；README 也写明安装仍需 Node。
- CI 本身不要求 Node，因为它跳过上述 ignored 门禁且不运行 release 构建；这不能被当作“测试已无 Node 路径”。

因此 P2/P3 只能新增 native 路径，不能删除或替换现有 Node 路径；删除属于 P6/P7，且须先通过 G5/G6（计划 :233–243）。

## 4. Worktree 与分支盘点

`git worktree list` 实际结果（16 个）：

| 路径 | 分支 | HEAD |
| --- | --- | --- |
| /home/zxg/code/rust/wxml-lsp | master | 4d59ca7 |
| /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration | tsgo/migration | b9616c5 |
| /home/zxg/code/rust/wxml-lsp-worktrees/full-components | feat/full-components | 0c88899 |
| /home/zxg/code/rust/wxml-lsp-worktrees/full-editing | feat/full-editing | 69b70d2 |
| /home/zxg/code/rust/wxml-lsp-worktrees/full-integration | feat/full-integration | eedef06 |
| /home/zxg/code/rust/wxml-lsp-worktrees/full-local-delivery | feat/full-local-delivery | 6eb43ff |
| /home/zxg/code/rust/wxml-lsp-worktrees/full-performance-local | feat/full-performance-local | 42a82ba |
| /home/zxg/code/rust/wxml-lsp-worktrees/full-projects | feat/full-projects | 50786dd |
| /home/zxg/code/rust/wxml-lsp-worktrees/full-semantic-tools | feat/full-semantic-tools | dd15bc1 |
| /home/zxg/code/rust/wxml-lsp-worktrees/full-styles | feat/full-styles | 856c1ac |
| /home/zxg/code/rust/wxml-lsp-worktrees/full-type-semantics | feat/full-type-semantics | 36bba6d |
| /home/zxg/code/rust/wxml-lsp-worktrees/wave5-completion | feat/wave5-completion | 4fb185c |
| /home/zxg/code/rust/wxml-lsp-worktrees/wave5-contracts | feat/wave5-contracts | 26c2766 |
| /home/zxg/code/rust/wxml-lsp-worktrees/wave5-events | feat/wave5-events | 98bc529 |
| /home/zxg/code/rust/wxml-lsp-worktrees/wave5-templates | feat/wave5-template-types | dc8c7eb |
| /home/zxg/code/rust/wxml-lsp-worktrees/wave5-zed-sync | feat/wave5-zed-sync | cad3c76 |

未 checkout 的分支还有：`integration-pre-c`、`integration-pre-n`、`integration-pre-q1`、`integration-pre-q2`、`integration-pre-s`、`integration-pre-t`、`integration/wave5`。

命名冲突检查：

- 分支前缀实际是 `feat/full-*`、`feat/wave5-*`，不存在裸 `full-*` / `wave5-*` 分支；worktree 目录名是 `full-*` / `wave5-*`。
- 唯一 `tsgo` 名称是本工作区 `tsgo-migration` 与分支 `tsgo/migration`；没有其他 `tsgo-*` 分支或 worktree。
- 与 `full-*`、`wave5-*`、`tsgo-*` 均不冲突。
- 主工作区以 21:59 的 clean 状态为初始基线；22:04 复核出现 3 个 modified（`backend_node/mod.rs`、`dependencies.rs`、`project_host.rs`），由 main worktree 的并发工作产生，不属于本工作区，未读取/修改/纳入 BASE。
- `git remote -v` 为空：当前仓库没有配置远端；未来 codeup/PR 操作需要另行配置，这不影响 P0，但会影响交付/PR 阶段。

## 5. 后续 worker 预检清单

### 5.1 固定输入

```text
WORKTREE=/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
BASE=b9616c557e54a06b7d756e516e6dd7c4016664cf
BRANCH=tsgo/migration
PLAN_SHA256=3c451c10fb34554c765fcf63042ef286bc686b0ada01098a7c6e5c72c2dfce71
ADR0008_SHA256=e3463cd94f7c8bc8771aece1092562661e2253b37f62e4d0cdd3e5074d4da37e
```

任务书必须带实际输入 commit；不得从仅含空接口的 CONTRACT_BASE 直接开始语义实现（计划 :206、:424）。

### 5.2 独立 target 与日志

推荐：

```sh
export CARGO_TARGET_DIR=/tmp/opencode/tsgo-migration/<pkg>
export CARGO_BUILD_JOBS=4
mkdir -p "$CARGO_TARGET_DIR"
```

`<pkg>` 示例：`native`、`shadow`、`parity`、`delivery`、`queries`、`symbols`。日志也放 `/tmp/opencode/tsgo-migration/<pkg>/logs/`，不得在项目目录创建临时文件。

注意：

- 当前 `/tmp` 是 tmpfs，16G 总量、盘点时仅 7.7G free；已有 worktree `target/` 为 3.6G。一个完整 workspace target 可能继续增长，三个并行 target 有填满 tmpfs 的风险。
- 预检建议：P3 最多同时 2 个编译 worker（`CARGO_BUILD_JOBS=4` 各 4 个 rustc）；计划允许最多 3 个实现 worker（:424），启动第三个前必须重新检查 `df -h /tmp` 与 `free -g`，不要静默改用仓库内 `target/`。
- 不要共享 worktree 现有的 `target/`：会产生 Cargo 锁竞争，也无法隔离各包产物。
- 若确实需要第三个 target 或 /tmp 容量不足，由集成负责人明确批准改为磁盘目录并记录；不要自行变更计划。

### 5.3 单写文件

按计划第 4 节与 10.1 节（ docs/plans/2026-11-17-rust-tsgo-migration.md:147–149、:432–445）：

| 所有者 | 单写范围 |
| --- | --- |
| 集成/合同 I | 根  Cargo.toml、 Cargo.lock、新 crate 清单； crates/wxml-lsp/Cargo.toml、 crates/wxml-lsp/src/lib.rs、 crates/wxml-lsp/src/main.rs；共享  crates/wxml-types； crates/wxml-lsp/src/server.rs 与  crates/wxml-lsp/src/project_host.rs（如需变更）；共享测试工具； .gitattributes |
| 交付 D |  crates/wxml-delivery/src/manifest.rs 及 schema |
| 构建/编辑器 C |  scripts/build-release.sh、 .github/workflows/ci.yml、 editors/zed/src/lib.rs |
| 文档 R | 根 README、 docs/README.md、 docs/adr/README.md、活跃 reference/evidence |
| 测试 T | parity 账本最终合并（各包只写自己的测试文件与交接记录） |

`crates/wxml-lsp/src/backend_node/*` 是待删除旧实现；除 P7 的单一删除 owner 外，P2–P6 不应并发修改。任何共享接口需求先交合同 owner，不能在 worker 内私自改共享 DTO（计划 :425、:449）。

### 5.4 基线命令

每个实现包至少运行与自身相关的测试；集成后按计划 :344–356：

```sh
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo test --workspace --doc --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo llvm-cov --workspace --all-features --fail-under-lines 80
cargo audit
cargo deny check
```

已知缺口：`cargo deny check` 当前没有 deny.toml；`cargo llvm-cov` 工具存在但 CI 未接；CI 四条命令均未加 `--locked`。这些是 G5 前必须补齐的项，不是本 P0 的通过项。

## 6. 阻塞 P2/P3 的未决基线问题

### 6.1 B1 原生发行物身份、托管与批准（硬阻塞）

- 需要确定：精确 tsgo 发行版本、源码提交、API/AST schema、平台资产、每个资产的 SHA-256、许可证与托管位置/批准人。
- 计划把“固定候选版本、先验证稳定版、不默认 beta”列为 P1-A/P1-C 门禁，见  docs/plans/2026-11-17-rust-tsgo-migration.md:181–187；F0 冻结要求见 :142；NF-09 见 :374。
- 当前仓库只有源码调研提交 `3e9236361fa97a9e6c685f4833ac457ae2d8c50a` 与历史 LSP 实验，没有已批准的原生二进制、摘要或资产 lock。ADR-0008 明确不得把调研用 `7.1.0-beta` 自动视为生产选择（ docs/adr/0008-rust-tsgo-no-node-backend.md:15–25）。
- `wxml-delivery` 已有显式下载门控、缓存、校验和离线安装框架（ crates/wxml-delivery/src/acquire.rs:1–33、 :60–70），但没有 native engine 资产 schema/lock；P1-C 需验证可复用性。
- 影响：F0/F3 无法冻结，P2 合同与 P3 交付基础不能开工。

### 6.2 B2 Rust 原生协议客户端可行性（硬阻塞）

- 首选接口是同一原生进程的 `--api --async` 双向 stdio、Content-Length JSON-RPC，并可能包含 base64 二进制 AST；计划要求验证 FS 回调、snapshot/lease/handle 生命周期、hover 文档/重载、definition、completion、references/rename 保真度，见  docs/plans/2026-11-17-rust-tsgo-migration.md:124–134、:181。
- 当前仓库没有任何 Rust 原生协议客户端原型；现有  crates/wxml-lsp/src/backend_node/protocol.rs 是 NDJSON、`PROTOCOL_VERSION = 2`，不能复用为 native framing（:1、:15）。
- 历史证据  docs/evidence/2026-11-15-native-typescript-service.md 使用 `tsc --lsp --stdio` 的 LSP 路径，并未验证目标 API/AST/callback 组合；ADR 也明确源码调研不等于二进制实测（ docs/adr/0008-rust-tsgo-no-node-backend.md:103–108）。
- 若单进程 API 无法覆盖查询保真度，ADR 要求先阻塞并修订，不得默认拆成两个独立类型图（ADR :15–25）。
- 影响：F0/F1 无法冻结，P2/P3 的原生客户端与查询层不能进入实现。

### 6.3 B3 现有 Node 路径仍是测试/发布前置（约束，不是删除许可）

- 23 个 ignored 真实门禁与 2 个 `semantic_tools` 静默 skip 仍以 `dist/server.js` 为条件；`scripts/build-release.sh` 仍 `require_tool node/npm` 并执行 npm 构建。
- 因此 P2/P3 的任务书必须明确“不删除、不静默跳过旧 Node 路径”；新增 native 测试必须可独立运行，缺失 native 资产应失败而不是 skip（计划 :315–319、:366–377）。
- 删除旧实现的唯一时机是 P7，且需 G5/G6 通过并完成全量等价、恢复、安全、性能与无 Node 交付验收（计划 :237–243）。

### 6.4 B4 质量门禁未接线（G5 阻塞）

- 无  deny.toml：`cargo deny check` 的许可证、来源、重复版本策略没有冻结；`cargo audit` 可执行但 CI 不跑。
- 无 CI coverage、无 `--locked`、无 native 资产校验 job；计划要求的 80% 覆盖率与安全命令目前只能手工执行。
- 需要集成/安全负责人在 P2 建立策略文件，并在 P5 前接入 CI；否则 G5 不能按计划命令复现。

### 6.5 B5 文档索引与局部链接未完成（P2 文档门禁）

- ADR 索引表已包含 0008 且状态 `proposed`，见  docs/adr/README.md:18；但阅读顺序仍写“按 0001 → 0007 阅读”（:22）， docs/README.md:30 也只写 0001 → 0007，目录树 :86 仍为 `0001-*.md ... 0007-*.md`。
- 既有 ADR（0001/0002/0003/0006/0007）尚未添加指向 ADR-0008 的局部替代链接，尽管 ADR-0008 的“与既有 ADR 的关系”声明会局部增加链接（ docs/adr/0008-rust-tsgo-no-node-backend.md:94–101）。
- 根 README 仍把 Node 类型后端作为当前组件和命令。P2 文档 owner 需在正式合入时更新索引、链接与活跃命令；这属于 G2 文档门禁，不阻塞 P3 代码骨架，但阻塞 P2 完成。
- 本文件尚未加入  docs/evidence/README.md 的证据索引；该索引由文档 owner 维护，本任务无写权限。

### 6.6 B6 P0 dirty 行为增量表与并发 dirty（P4 前置）

- 本工作区已 clean，19 个历史 dirty 文件全部落在 `4d59ca7` 且该提交是 BASE 的祖先；这只证明“没有未提交代码”，不等于计划第 2 节要求的“每项修复 → 需求 → 旧失败 → 新断言 → Rust 落点”行为表已完成（计划 :54、:303）。
- 22:04 主工作区出现 3 个并发 modified 路径（ crates/wxml-lsp/src/backend_node/mod.rs、 crates/wxml-lsp/src/dependencies.rs、 crates/wxml-lsp/src/project_host.rs），不在 BASE 中。需先确认它们是待迁移修复还是临时编辑；按计划 :52–56 保护并登记 delta，不得 reset/restore/clean/stash 或静默覆盖。
- 需要在 P0 收尾时基于 `4d59ca7` 的 19 个文件与上述并发变更逐项补齐；P4 开始前必须确认没有未归属断言，否则 G4 不通过。
- 本文件不代替该表。

### 6.7 B7 工具链/lock 可重复性（G5 前置）

-  rust-toolchain.toml 使用浮动 `stable`，CI 使用 `dtolnay/rust-toolchain@stable`；`Cargo.lock` 已跟踪，但 CI 未使用 `--locked`。
- 计划要求固定 lockfile、两次确定性 release 与固定摘要（:344–358、:374）。需在 G5 前冻结工具链 patch 或记录实际版本，并让 CI 使用 `--locked`。

### 6.8 B8 编译资源（操作性风险）

- 24 vCPU / 30 GiB RAM，盘点时 11 GiB available；`/tmp` tmpfs 仅 7.7 GiB free，worktree 现有 `target/` 已 3.6 GiB。
- 建议 P3 先并行 2 个编译 worker，各自独立 `/tmp/opencode/tsgo-migration/<pkg>`，`CARGO_BUILD_JOBS=4`；启动第三个前复核磁盘/内存。此项不是架构阻塞，但会导致构建失败或抢占。

### 6.9 B9 无远端（信息项）

- `git remote -v` 为空，当前分支无 upstream（`git rev-parse @{u}` 报“尚未给分支设置上游”）。不影响 P0/P2/P3 本地实现；未来 PR/合入前需要配置 codeup 远端与评审人。

### 6.10 B10 同一 worktree 并发写入（操作性风险）

- 22:04 本 worktree 除本文件外出现  docs/evidence/2026-11-17-tsgo-delivery-spike.md，说明已有另一个交付 spike worker 在此 worktree 写入；本任务未读取/修改该文件。
- 后续 worker 不得执行 `git add .`、`git commit -a`、`git clean`、`git restore` 或 `git stash`；提交前逐文件确认 owner。`git status` 不再为空是预期现象。
- P3 起应按计划使用独立 `tsgo-*` worktree（:424）；本 worktree 只适合 P0/P1 证据汇总，不适合多个实现包共享。

## 7. 交接

- 本文件是本任务唯一仓库写入；main worktree 的 3 个 modified 与本 worktree 的  docs/evidence/2026-11-17-tsgo-delivery-spike.md 均由其他并发工作产生，本任务未触碰。后续 worker 应把本文件作为只读基线，不要修改。
- 可直接信任：BASE、分支、本工作区 clean 状态、计划/ADR hash、workspace 成员与 lint、`Backend` 行号、`backend_node` 行号、worktree/分支清单、23 个 Node ignored 门禁与 2 个静默 skip 的事实。
- 不得直接开始 P2/P3：先完成 P1-A（原生 API/AST/FS/查询保真）、P1-B（测试/oracle 盘点）、P1-C（无 Node 资产与离线交付），三者 PASS 后按 G1/G2 进入 P2；硬阻塞是 B1、B2。
- P2/P3 期间不得删除、替换或静默跳过现有 Node 路径；Node 删除只在 P7 执行。
- 下一步建议顺序：
  1. 由集成负责人补齐 B6 行为增量表并冻结 BASE；
  2. 由 P1-A 产出原生协议/资产探针证据，回答 B1/B2；
  3. P1-B 刷新 TS 测试静态/展开计数并建立 oracle；
  4. P1-C 验证本地资产获取、摘要、许可与离线路径；
  5. 三者 PASS 后再冻结 F0–F3 与 P3 任务书。
- 交接前复核：计划/ADR hash 未变、本任务新增文件仅为  docs/evidence/2026-11-17-tsgo-workspace-baseline.md；本 worktree 的 delivery-spike 与 main worktree 的 3 个 modified 属并发工作，状态按 1.1/6.10 记录；不得对并发变更执行 reset/restore/clean/stash。
