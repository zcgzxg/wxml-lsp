# 2026-11-17 无 Node 交付与安装试验（P1-C）

日期：2026-11-17。工作树：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`，
分支 `tsgo/migration`，基线 `b9616c5`（工作树 clean）。对象：现有 `wxml-delivery` 交付链、
ADR-0007 信任模型、npm 上的原生 TypeScript 平台包，以及“旧 Node 安装 → 原生安装”的
升级/回滚状态机设计。

本记录是**只读调查 + 设计**，不是实现完成报告。未下载或安装原生二进制、未运行
`node`/`npm`、未执行项目构建或安装器、未修改任何产物；本轮唯一仓库写入是本文件。

## 0. 结论摘要

| 任务 | 状态 | 结论 |
| --- | --- | --- |
| 1 Node/npm 依赖盘点 | ✅ PASS（静态） | 运行时、构建、安装器、测试、Zed、文档均有精确落点；运行时依赖集中在 `backend_node` 与 `build-release.sh`，测试有 23 个 `#[ignore]` Node 门禁 + 2 个静默跳过 |
| 2 现有打包/安装/回滚与 ADR-0007 | ✅ PASS（静态） | 现有链路已具备“临时区校验 → rename → 原子 symlink”骨架；但 schema v1 强制要求 Node 后端，原生包必须升 schema v2 |
| 3 npm registry 事实 | ✅ PASS（元数据） | 平台包 `@typescript/typescript-linux-x64@7.1.0-beta`：Apache-2.0、无 scripts、`os=linux/cpu=x64`、122 文件、解包 28,450,796 B；`gitHead` 与 ADR-0008 调研提交完全一致 |
| 4 无 Node 交付算法 + manifest schema | ✅ PASS（设计）/ ◐ 未实现 | 算法与 schema v2 已定义到字段级；实现属于 D 包 |
| 5 旧 Node → 原生状态机 + 测试 | ◐ PARTIAL | 状态、崩溃窗口、不变量、测试矩阵已定义；现有测试只覆盖旧链路的一部分 |
| 6 无 Node 验收命令 | ◐ PARTIAL | 命令矩阵已给出；本工作树无法执行（无原生资产、无 schema v2 安装器） |
| 7 风险登记 | ✅ PASS（记录） | 12 项具体风险，含 FS 回调、noEmit 非沙箱、ABI、许可与摘要固定 |

**P1-C 门禁建议：PARTIAL PASS（可进入 P2/F3 冻结，但 C 门禁不能关闭）。**
资产身份、许可、平台、摘要与“npm 入口本身是 Node launcher、无 Node 入口是平台包里的
`lib/tsc`”已经核实；缺的是原生资产下载后的真实解包/探针/离线安装证据，以及 schema v2
安装器。二者分别是“另一位 worker 的下载任务”和 D 包实现，不能在本次只读试验中冒充通过。

## 1. 范围、方法与限制

- 阅读：`docs/plans/2026-11-17-rust-tsgo-migration.md` §8～§10、`docs/adr/0008-*.md`、
  `docs/adr/0007-local-delivery-trust-model.md`、交付 crate、CI、Zed 扩展、测试与活跃文档。
- 使用 `rg`/`sed` 做静态行号盘点；`cargo`/`node`/`npm` 均未执行。
- npm 元数据通过 HTTPS 读取 `https://registry.npmjs.org/...`；包内文件清单通过 jsDelivr
  的“已发布包文件清单”API 读取，另读取了已发布的小文本文件（`package.json`、`README.md`、
  `bin/tsc`、`lib/tsc.js`、`lib/getExePath.js`）。**未下载任何 `.tgz`**；`Content-Range`
  只做了 1 字节 range 请求以获得归档总长度。
- 查询时间：`2026-11-17T13:53Z`～`13:59Z`（registry `date` 响应头）。`dist-tags` 可变，
  精确版本元数据不应被当作永久不变。
- 未验证：原生二进制的实际 `--api --async` 行为、AST/schema、lib 解析、glibc 下限、
  安装器真实执行、Zed UI 冒烟。这些都在第 9 节列为后续门禁。
- 本记录**不提议在本轮删除任何 Node 代码、脚本或依赖**；删除属计划 P7，必须在 P5/P6
  全量等价、恢复、安全与无 Node 交付门禁通过后串行执行。本文只设计迁移路径与新包形态。

## 2. Node/npm 依赖清单（精确路径与行号）

分类：**R**=产品运行时；**B**=构建/打包/release；**I**=安装/升级/回退；**T**=测试；
**E**=编辑器扩展转发；**D**=文档/脚本。同一位置可能跨类。

### 2.1 产品运行时（R）

| 位置 | 行 | 依赖内容 |
| --- | --- | --- |
|  crates/wxml-lsp/src/main.rs | 10 | `use wxml_lsp::backend_node::{NodeBackend, NodeBackendConfig}` |
|  crates/wxml-lsp/src/main.rs | 28-36 | usage 记录 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`，并声明只探测可执行文件相对布局 `backends/typescript/dist/server.js` 或 `wxml-type-backend/server.js` |
|  crates/wxml-lsp/src/main.rs | 94-101 | `serve_real()` 构造 `NodeBackend`，即默认产品后端 |
|  crates/wxml-lsp/src/backend_node/config.rs | 26-29 | 两个安装布局常量：`backends/typescript/dist/server.js`、`wxml-type-backend/server.js` |
|  crates/wxml-lsp/src/backend_node/config.rs | 63-68 | `Typings` 固定 `miniprogram-api-typings@5.3.1` |
|  crates/wxml-lsp/src/backend_node/config.rs | 104-145 | `NodeBackendConfig`：`node` 字段 + 默认 `PathBuf::from("node")`（134 行） |
|  crates/wxml-lsp/src/backend_node/config.rs | 166-178 | `WXML_NODE`（命令名或绝对路径） |
|  crates/wxml-lsp/src/backend_node/config.rs | 179-195 | `WXML_TYPE_BACKEND` / `WXML_BACKEND_DIR`（必须绝对路径） |
|  crates/wxml-lsp/src/backend_node/config.rs | 229-257 | 只按 `current_exe()` 相对路径探测 `server.js`，并按 `dist/` 推导 `node_modules` 根 |
|  crates/wxml-lsp/src/backend_node/process.rs | 74 | 文档：真实 `node server.js` 进程，NDJSON |
|  crates/wxml-lsp/src/backend_node/process.rs | 258 | `Command::new(&config.node)` 启动 Node |
|  crates/wxml-lsp/src/backend_node/process.rs | 344 | `[wxml-node]` stderr 日志前缀 |
|  crates/wxml-lsp/src/backend_node/mod.rs | 140 | `StdioNode::start(&factory_config)` |
|  crates/wxml-lsp/src/backend_node/mod.rs | 163 | `BackendId::new("node")` |
|  backends/typescript/src/server.ts | 16, 43, 299-333 | Node 后端入口；注释明确项目根“holds node_modules with TypeScript and typings”；CLI 入口 `node dist/server.js`，用 `process.argv[1]` 解析 |
|  backends/typescript/src/env.ts | 3-6, 12-14, 36-43 | 从 `node_modules/<pkg>/package.json` 读取 `typescript`/`miniprogram-api-typings` 版本并上报 |
|  backends/typescript/src/analysis.ts | 57-58, 274-291 | `backendDir`（含 `node_modules`）+ `typeRoots` 追加 + 读策略 |
|  backends/typescript/src/compiler.ts | 136-160 | 项目 `typeRoots` 与后端 `node_modules` 合并逻辑 |
|  backends/typescript/src/symbols.ts | 44-45, 1117-1135 | 同上，符号查询路径 |

运行时可观察后果：`WXML_NODE` 默认走 `PATH` 找 `node`；缺失时类型能力降级但静态能力保留
（ crates/wxml-lsp/README.md:22-23）。这是一条“隐式 Node 回退”路径，NF-01/NF-07 要求删除。

### 2.2 构建、打包、release（B）

| 位置 | 行 | 依赖内容 |
| --- | --- | --- |
|  scripts/build-release.sh | 12-13 | usage：构建 “Rust language server + installer, the Node type backend …, and the wasm32-wasip2 Zed extension” |
|  scripts/build-release.sh | 24-25 | 默认 `cargo`/`npm` 离线，`--online` 才取依赖 |
|  scripts/build-release.sh | 102-108 | 从 Rust 与 TS 源分别读取协议版本并要求相等 |
|  scripts/build-release.sh | 110-116 | 从 `backends/typescript/package.json` 读 `node` 最低版本与 backend 版本 |
|  scripts/build-release.sh | 151-172 | `require_tool node`、`require_tool npm`、Node 主版本检查（`node -e ...`） |
|  scripts/build-release.sh | 182, 189 | dry-run 打印 `node >=`；步骤 2 为 `npm ci`/`check`/`build`/`prune` |
|  scripts/build-release.sh | 206-216 | `npm --offline` 标志 |
|  scripts/build-release.sh | 224-244 | 复制 TS 源、`npm ci --ignore-scripts`、`npm run check/build`、`npm prune --omit=dev`、删除 `node_modules` 内符号链接、复制 `dist` 与 `node_modules` |
|  scripts/build-release.sh | 271-274 | 给打包器传 `--node-minimum`/`--backend-version` |
|  crates/wxml-delivery/src/package.rs | 31-34 | `BuildOptions.node_minimum` / `backend_version` |
|  crates/wxml-delivery/src/package.rs | 136-137 | 校验 Node 版本与 backend 版本 |
|  crates/wxml-delivery/src/package.rs | 382-399 | staged 树必需项：`bin/wxml-type-backend/server.js`、`node_modules/typescript/package.json`、`node_modules/miniprogram-api-typings/package.json` |
|  crates/wxml-delivery/src/package.rs | 427-435 | 写 `ArtifactKind::NodeBackend` 产物 |
|  crates/wxml-delivery/src/manifest.rs | 23 | `SCHEMA_VERSION = 1` |
|  crates/wxml-delivery/src/manifest.rs | 43-51 | `NodeInfo { minimumVersion, backendVersion }` |
|  crates/wxml-delivery/src/manifest.rs | 85-95, 128-131 | `ArtifactKind::NodeBackend`；`Manifest.node` 必填 |
|  crates/wxml-delivery/src/manifest.rs | 198-213 | schema/协议/Node 版本校验 |
|  crates/wxml-delivery/src/manifest.rs | 322-339 | Node 后端路径必须恰为 `bin/wxml-type-backend/server.js`，协议/版本必须匹配 |
|  crates/wxml-delivery/src/manifest.rs | 354-387 | 必需 `wxml-lsp`，且**恰好一个** Node backend，最多一个 Zed 扩展 |
|  crates/wxml-delivery/src/manifest.rs | 411-453 | `Release.node_minimum_version` 必填并校验 |
|  crates/wxml-delivery/src/bin/wxml-lsp-package.rs | 17-31, 101-102, 122-146 | CLI 帮助与参数：必需 `--node-minimum`，输出 `--backend-version` |
|  .github/workflows/ci.yml | 1-26 | CI 只有 fmt/clippy/test/doc，**没有** Node 安装步骤；因此 Node 门禁在当前 CI 中完全不跑 |

### 2.3 安装/升级/回退（I）

| 位置 | 行 | 依赖内容 |
| --- | --- | --- |
|  crates/wxml-delivery/src/verify.rs | 15-22 | `RuntimeReport.node_version` |
|  crates/wxml-delivery/src/verify.rs | 274-310 | `check_backend` 要求 `server.js`、`node_modules/typescript/package.json`、`node_modules/miniprogram-api-typings/package.json` |
|  crates/wxml-delivery/src/verify.rs | 312-364 | `resolve_node`（`WXML_NODE` 或 `node`）+ `check_node` 执行 `node --version` |
|  crates/wxml-delivery/src/verify.rs | 366-379 | `verify_install` = binary + backend + Node |
|  crates/wxml-delivery/src/install.rs | 305-308 | staged 树上执行 `check_binary` + `check_backend`，通过后才 rename |
|  crates/wxml-delivery/src/install.rs | 393-400 | `same_payload` 比较包含 `left.node == right.node` |
|  crates/wxml-delivery/src/install.rs | 284-342 | `install_tree`：校验 → 处理同版本 → rename 到 `versions/<v>` → `switch` |
|  crates/wxml-delivery/src/install.rs | 350-381 | `switch`：临时 symlink + `rename` 原子替换 `current`（仅 Unix） |
|  crates/wxml-delivery/src/bin/wxml-lsp-install.rs | 28 | `verify` 命令描述为 “Fully verify the active installation and Node.js” |
|  crates/wxml-delivery/src/bin/wxml-lsp-install.rs | 29, 459-486 | `env` 输出 `WXML_LSP_BIN` + `WXML_TYPE_BACKEND`（指向 `wxml-type-backend/server.js`） |
|  crates/wxml-delivery/src/bin/wxml-lsp-install.rs | 376-377 | `versions` 打印 `node >= ...` |
|  crates/wxml-delivery/src/bin/wxml-lsp-install.rs | 405-426 | `current` 调 `check_node`，无 Node 即失败 |
|  crates/wxml-delivery/src/bin/wxml-lsp-install.rs | 428-457 | `verify` 调 `verify_install`（含 Node） |
|  crates/wxml-delivery/src/bin/wxml-lsp-install.rs | 540-613 | 安装后 `check_node` 仅 **warning**，因此 `install` 已能在无 Node 时成功但类型能力降级 |
|  crates/wxml-delivery/tests/installer_flow.rs | 593 | `reports_node_missing_old_and_broken_runtime`（现有 Node 负例） |
|  crates/wxml-delivery/tests/installer_security.rs | 337 | `rejects_manifest_node_metadata_that_differs_from_the_index` |

### 2.4 测试与测试工具（T）

| 位置 | 行 | 依赖内容 |
| --- | --- | --- |
|  crates/wxml-lsp/tests/real_node.rs | 30, 103, 219, 494, 626 | 5 个 `#[ignore]`，注释要求 Node 24 + `dist/server.js`，`WXML_TEST_NODE=1` |
|  crates/wxml-lsp/tests/stdio_type_loop.rs | 499, 601, 653, 701, 775, 831, 942, 1008, 1078 | 9 个 `#[ignore]` Node G1/stdio 门禁；53-54、82-87 为缺失即硬失败 |
|  crates/wxml-lsp/tests/wave5_acceptance.rs | 670, 723, 771, 845, 995, 1029, 1112 | 7 个 `#[ignore]` Wave 5 门禁；121-122、157-162 为硬失败 |
|  crates/wxml-lsp/tests/performance_bench.rs | 781-785, 348, 18-19 | 1 个 `#[ignore]` 性能门禁；`node_version()` 真实执行 `node --version` 写入报告 |
|  crates/wxml-lsp/tests/q2_stability.rs | 202-206, 14-15 | 1 个 `#[ignore]` 稳定性门禁 |
|  crates/wxml-lsp/tests/semantic_tools.rs | 2069-2084, 2568-2573 | 2 个**非 ignore** 的真实 Node 测试：`dist/server.js` 不存在时打印 “skipping” 并 `return`（静默通过） |
|  crates/wxml-lsp/tests/semantic_tools.rs | 11, 24, 429 | 其余用例使用 `NodeBackend` + fake transport（不启动 Node 进程，但编译期绑定 `backend_node` 模块） |
|  crates/wxml-lsp/tests/component_intelligence.rs | 40-42 | 把 `WXML_NODE` 指向不存在路径，断言降级；本身是“无 Node”负例 |
|  crates/wxml-lsp/tests/node_backend.rs | 96, 352 | `BackendEpoch`/`BackendId("node")`，fake transport |
|  backends/typescript/package.json | 7-19, 28-34 | `engines.node >= 24.0.0`；`test: node --test test/*.test.ts`；依赖 `typescript@6.0.3`、`miniprogram-api-typings@5.3.1` |

规模：21 个 TS 测试文件、34 个 TS 源文件（28 个顶层 + 6 个 `layouts/`，12,631 行 src /
6,263 行 test）。23 个 ignored
测试全部以 `WXML_TEST_NODE=1` 为门槛（与 `docs/evidence/README.md` §3 的 W4 计数一致）。

### 2.5 Zed 扩展（E）

| 位置 | 行 | 依赖内容 |
| --- | --- | --- |
|  editors/zed/src/lib.rs | 19-25 | `FORWARDED_ENV` 转发 `WXML_TYPE_BACKEND`、`WXML_BACKEND_DIR`、`WXML_NODE`、`WXML_ANALYZE_TIMEOUT_MS`、`WXML_TEMPLATE_CHECK` |
|  editors/zed/src/lib.rs | 46-57 | `language_server_command`：解析 `WXML_LSP_BIN`，启动 `--stdio`，注入转发环境 |
|  editors/zed/src/lib.rs | 684-699 | 单测断言只转发文档化变量（含 `WXML_NODE`） |
|  editors/zed/README.md | 45-69, 107, 131-135, 183-185 | 文档要求先 `npm ci && npm run build`，并导出/转发 Node 变量 |
|  editors/zed/extension.toml | 12-24 | grammar pin 与五语言注册；不含 Node 字段（保留） |

### 2.6 文档、样本与开发脚本（D）

| 位置 | 行 | 说明 |
| --- | --- | --- |
|  README.md | 34-38, 53-58 | 快速开始要求 Node >= 24 与 `npm ci/check/build/test` |
|  README.md | 70-76, 92-94 | 手工构建后端、`WXML_NODE`/`WXML_BACKEND_DIR` 表 |
|  README.md | 179, 187, 211-216 | release 构建默认 cargo/npm 离线；产物绑定 “Node 最低版本”；安装器依赖系统 Node |
|  README.md | 220-235 | W4 发布记录：`node >= 24.0.0`、214 files、Linux Zed 冒烟“release LSP 与 Node 启动” |
|  crates/wxml-lsp/README.md | 5, 10, 22-23, 34-36, 51, 60, 80-94, 111, 117, 125 | 适配器/环境变量/真实 Node 测试说明 |
|  crates/wxml-delivery/README.md | 9, 32-34, 46, 65, 100, 103, 112, 116 | 布局含 `server.js`/`node_modules`；manifest 记录 Node 版本；排错写 Node |
|  docs/reference/type-diagnostics.md | 132, 212 | 补全经 Node；验证命令 `npm ci && npm run build` |
|  docs/reference/type-bridge-c1.md | 391, 407, 411, 416 | Node ≥ 24、`ts.sys` harness、NDJSON 后端、构建命令 |
|  docs/reference/wxml-language-feature-baseline.md | 49-51, 60, 95-98, 217 | 能力/性能/稳定性以 Node 后端为基线 |
|  docs/reference/contracts.md | 212 | “Rust–Node 私有协议”列为未冻结项 |
|  docs/reference/provenance.md | 30 | TypeScript 6.0.2 许可证记录 |
|  tests/fixtures/README.md | 230-233 | 样本类型分析前置 `npm ci && npm run build` |
|  docs/evidence/README.md | §3、§4 | 历史 Node 计数与证据规则；历史记录保留，不能当当前命令 |

### 2.7 汇总

- **真正阻塞“无 Node 运行”的只有三处**： `backend_node`（启动 node + NDJSON）、
  `build-release.sh`（npm 构建后端）、manifest/installer（schema v1 强制 Node 后端与 Node 版本）。
- **CI 已经无 Node**，但也因此从未运行 23 个 ignored 门禁；这与 NF-03 的“必跑 native job”相反。
- **测试的隐式依赖**比进程依赖更广：12 个 Rust 测试文件编译期引用 `NodeBackend`，
  其中 `semantic_tools` 的两个真实测试会在缺少 `dist/server.js` 时静默跳过。
- **文档与 Zed 转发**是“重新激活 Node”的软路径：即使二进制删掉 Node，`WXML_NODE` 仍可能让旧布局生效；
  NF-07/NF-12 要求一并删除。

## 3. 现有交付实现与 ADR-0007 审查

### 3.1 当前布局与命令

现有产物（`crates/wxml-delivery/README.md:28-44`，并由本机历史 release
`/tmp/opencode/final-release-3/.../manifest.json` 复核）：

```text
wxml-lsp-<version>-<platform>/
  bin/wxml-lsp
  bin/wxml-lsp-install
  bin/wxml-type-backend/server.js
  bin/wxml-type-backend/node_modules/{typescript,miniprogram-api-typings}/...
  zed-extension/extension.wasm
  zed-extension/extension.toml
  zed-extension/languages/...
  manifest.json
  SHA256SUMS
```

命令：`install` / `upgrade` / `rollback` / `versions` / `list` / `current` / `verify` / `env` /
`verify-package`（ crates/wxml-delivery/src/bin/wxml-lsp-install.rs:18-51）。下载默认关闭，
`--enable-download` 才可联系远端；`--from-dir` 是显式离线源；`--offline` 只用缓存或本地目录。

### 3.2 校验链与安全边界（ADR-0007 的实际实现）

1. 源策略：仅 HTTPS；HTTP 只允许 loopback fixture；拒绝凭据/query/fragment/`..`/跨源 redirect
   （ crates/wxml-delivery/src/source.rs:37-78, 231-237）。
2. 归档：索引里的 `sha256` + `size`，下载后重新哈希并比对（ source.rs:159-227）。
3. 解包：只允许普通文件/目录；拒绝绝对路径、`..`、反斜杠、控制字符、超长路径、重复路径、
   文件/目录同名、符号链接/硬链接/FIFO/设备、setuid/setgid/sticky、组/其他可写、非 `0644/0755`
   （ crates/wxml-delivery/src/archive.rs:344-381, 431-545；`prescan_tar` 在 157-223）。
4. 树校验：manifest 结构 + `SHA256SUMS` 精确集合 + 逐文件 SHA-256/大小/模式 + 无多余文件/链接
   （ crates/wxml-delivery/src/verify.rs:30-99）。
5. 运行检查：staged 树上执行 `bin/wxml-lsp --version`（10 s 超时）与 Node 后端文件存在性
   （ verify.rs:173-310）；失败不切换。
6. 原子性：同文件系统 `tmp/` 暂存 → `rename` 到 `versions/<v>` → 临时 symlink `rename` 成 `current`
   （ install.rs:284-342, 350-381）；独占 OS 文件锁（ install.rs:145-160）。
7. 环境优先级：`WXML_LSP_BIN` 一旦设置即优先；无效值硬失败，绝不回退（ install.rs:423-486）。
8. 信任残留：TLS + 索引/归档 SHA-256 + manifest 哈希，**无发布者签名**；ADR-0007
   “Residual Risk” 明确接受，正式发布前应加 detached 签名。

ADR-0007 的“安装前执行 staged `--version` 与 Node 后端依赖检查”在原生方案中应替换为：
staged **原生引擎版本/schema 探针** + **类型资产完整性**检查，仍然在 rename 与 switch 之前。

### 3.3 最终无 Node 包必须包含的内容

| 包内路径 | 来源 | 许可 | 必需性 |
| --- | --- | --- | --- |
| `bin/wxml-lsp` (0755) | 本项目 Rust | 项目许可 | 必需 |
| `bin/wxml-lsp-install` (0755) | 本项目 Rust | 项目许可 | 必需（便于离线修复/回滚） |
| `engine/lib/tsc` (0755) | `@typescript/typescript-linux-x64@7.1.0-beta` 的 `lib/tsc` | Apache-2.0 | 必需 |
| `engine/lib/lib.*.d.ts` (0644) | 同一包 `lib/`，116 个文件 | Apache-2.0 | 必需（默认标准库） |
| `engine/lib/tsc.sig` (0644) | 同一包 `lib/tsc.sig` | Apache-2.0 | 随包保留；运行期是否必需**待探针** |
| `engine/LICENSE` | 同一包 | Apache-2.0 全文 | 必需 |
| `engine/NOTICE.txt` | 同一包 | Apache-2.0 NOTICE | 必需 |
| `engine/package.json` | 同一包 | — | 建议保留（provenance/版本） |
| `engine/README.md` | 同一包 | — | 可选 |
| `typings/miniprogram-api-typings/index.d.ts` + `types/**` + `LICENSE`/README | `miniprogram-api-typings@5.3.1`（26 文件） | MIT | 必需 |
| `zed-extension/extension.wasm` + `extension.toml` + `languages/**` | 本项目 Zed 扩展 | 项目许可 | 必需 |
| `zed-extension/grammars/wxml.wasm` | 预置 grammar（可选） | 上游许可 | 可选 |
| `manifest.json` / `SHA256SUMS` | 打包器生成 | — | 必需 |
| `THIRD_PARTY_LICENSES.md` / `provenance.json` | 打包器生成 | — | 建议（NF-09） |

明确**不应**出现：`node_modules/`、运行时 `*.js`/`*.cjs`/`*.mjs`、`package-lock.json`
（Node 依赖锁）、`bin/wxml-type-backend/`。`engine/package.json` 仅作元数据保留。

### 3.4 平台宣称与 Linux-only 门禁

- 平台标签：`host_platform()` 支持 `linux`/`macos`/`windows` × `x86_64`/`aarch64`/`arm`
  （ crates/wxml-delivery/src/paths.rs:20-42）。
- 但 release 构建只允许 Linux，且仅 x86_64/aarch64（ scripts/build-release.sh:118-127）。
- 安装器 `switch` 与非 Unix 模式：非 Unix 直接 `UnsupportedPlatform`
  （ install.rs:350-381；`set_mode` 同样在 manifest.rs:763-777）。
- 文档明确“只在 Linux 验证，依赖 Unix 原子 symlink 与系统 Node.js；Windows/macOS 未验证”
  （ crates/wxml-delivery/README.md:116；README.md:216）。
- 现有验收证据全部是 Linux：W4 release `linux-x86_64`、Linux Xvfb Zed 冒烟
  （ docs/evidence/README.md §3）。

结论：**最终 no-Node 验收只能宣称 Linux x86_64**；aarch64 需要真实构建与运行证据，
macOS/Windows 不在本轮范围。原生平台包虽然列出 21 个平台，但本项目没有对应验收。

### 3.5 对 no-Node 的缺口（按文件）

| 缺口 | 位置 | 影响 |
| --- | --- | --- |
| schema v1 强制 Node 后端 |  manifest.rs:354-387 | 原生包无法通过现有校验 |
| `Release.nodeMinimumVersion` 必填 |  manifest.rs:411-453 | 索引 schema 必须升 v2 |
| 安装器执行 `node --version` |  verify.rs:312-364 | 无 Node 时 `verify`/`current` 必失败 |
| `env` 导出 `WXML_TYPE_BACKEND` |  wxml-lsp-install.rs:459-486 | 引导 Zed 重新使用 Node 后端 |
| 后端探测 `server.js` |  config.rs:26-29, 229-241 | 残留 JS 文件可被当作后端执行 |
| `WXML_NODE` 默认 PATH 查找 |  config.rs:134, 166-178 | 隐式 Node 回退 |
| 打包器要求 `node_modules/*/package.json` |  package.rs:382-399 | 无法生成无 Node 包 |
| 构建脚本要求 node/npm |  scripts/build-release.sh:151-172, 224-244 | 无法无 Node 构建 |
| 无事务日志/目录 fsync |  install.rs:284-342 | 崩溃窗口可恢复但缺显式记录；断电持久性未证明 |

## 4. npm registry 事实（HTTPS 元数据，未下载 tarball）

### 4.1 主表：`@typescript/typescript-linux-x64@7.1.0-beta`

| 字段 | 值 |
| --- | --- |
| package/version | `@typescript/typescript-linux-x64` / `7.1.0-beta` |
| dist-tags（该包） | `rc=7.0.1-rc`、`latest=7.0.2`、`beta=7.1.0-beta`、`next=7.1.0-dev.20261117.1` |
| tarball | `https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz` |
| `dist.integrity` (SRI) | `sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==` |
| `dist.shasum` | `5a8fbfcace1010b8c8a3b38df93c0444d3fef05b` |
| 归档大小（packed） | `10,175,135` B（1 字节 range 的 `Content-Range: bytes 0-0/10175135`） |
| 解包大小 | `28,450,796` B；`fileCount=122` |
| license | `Apache-2.0` |
| os / cpu / libc | `["linux"]` / `["x64"]` / **无 `libc` 字段** |
| engines | `{"node": ">=16.20.0"}`（仅元数据；本项目不使用 npm） |
| scripts | **无**（该版本元数据无 `scripts`；无 install/postinstall 生命周期脚本） |
| `bin` | **无**（不被 npm 链接成命令） |
| 其他 | `type: module`、`preferUnplugged: true`、`exports` 仅 `./package.json`、`files: ["lib","NOTICE.txt"]` |
| `gitHead` | `3e9236361fa97a9e6c685f4833ac457ae2d8c50a`（与 ADR-0008/计划引用的调研提交一致） |
| registry 签名 | `dist.signatures` 存在（keyid `SHA256:DhQ8wR5APBvFHLF/+Tc+AYvPOdTpcIDqOhxsBHRwC7U`），本项目不校验 |

包内文件（jsDelivr 已发布包清单，122 项）：

- `/lib/tsc` — 24,522,912 B（原生可执行文件）
- `/lib/tsc.sig` — 481 B
- `/lib/lib.*.d.ts` — **116 个**，合计 3,870,196 B（`lib.es5`、`lib.dom` 2,399,570 B、ES2015–ES2026、`lib.esnext.*`、`webworker` 等）
- `/LICENSE` 9,197 B、`/NOTICE.txt` 46,827 B、`/package.json` 1,045 B、`/README.md` 138 B

关键点：**包内没有 `bin/`，也没有 JS 启动器**；原生入口就是 `lib/tsc`，标准库 d.ts 与它同目录。

### 4.2 其他平台包（`typescript@7.1.0-beta` 的 optionalDependencies，共 21 个）

`aix-ppc64`、`android-arm64`、`darwin-arm64`、`darwin-x64`、`freebsd-arm64`、`freebsd-x64`、
`linux-arm`、`linux-arm64`、`linux-loong64`、`linux-mips64el`、`linux-ppc64`、`linux-riscv64`、
`linux-s390x`、`linux-x64`、`netbsd-arm64`、`netbsd-x64`、`openbsd-arm64`、`openbsd-x64`、
`sunos-x64`、`win32-arm64`、`win32-x64`。实际核验元数据的 6 个（integrity 仅显示首尾，
完整值以 registry 为准）：

| 包 | integrity (sha512-) | os/cpu | 解包 B | 文件数 |
| --- | --- | --- | ---: | ---: |
| typescript-linux-x64 | `kcD51ihf…AgFQ==` | linux/x64 | 28,450,796 | 122 |
| typescript-linux-arm64 | `hNqQBgQ5…iHiWw==` | linux/arm64 | 27,127,796 | 122 |
| typescript-darwin-arm64 | `q7mxblcy…BuNdw==` | darwin/arm64 | 28,172,009 | 121 |
| typescript-darwin-x64 | `D5EYqG5W…KWWAA==` | darwin/x64 | 29,375,951 | 121 |
| typescript-win32-x64 | `UKcHvbyb…J3rw==` | win32/x64 | 28,841,163 | 121 |
| typescript-win32-arm64 | `lJbo71IE…bsvQ==` | win32/arm64 | 27,203,795 | 121 |

所有已核验平台包均为 `Apache-2.0`、`engines.node >= 16.20.0`、无 `scripts`、无 `libc` 字段。
`libc` 缺失意味着 **musl/glibc 不能从包元数据区分**，必须运行探针 + ABI 检查（见 R6）。

### 4.3 稳定版对照：`@typescript/typescript-linux-x64@7.0.2`

- `latest` 稳定版；`gitHead=2bd066d87f5bafd315be9f40889d0a60b9e58e0b`；
  integrity `sha512-EYdf2cNg7rgCWJnxCdJ+F3V39O8ihb37eHAu1LK8oAFizgTQbPOK7zHHXbPt8rX24COqODXeI3sIf0fCXG7H/A==`；
  解包 27,945,990 B，114 文件；`lib/tsc` 24,101,026 B，108 个 d.ts，同样有 `LICENSE`/`NOTICE.txt`/`tsc.sig`。
- 说明：**稳定版已存在同构无 Node 入口**，符合计划“先验证稳定版、不默认选 beta”。
  P1-A 应同时探测 `7.0.2` 与 `7.1.0-beta`，再决定 F0 固定版本。

### 4.4 `miniprogram-api-typings@5.3.1`（运行时类型资产，MIT）

| 字段 | 值 |
| --- | --- |
| tarball | `https://registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.1.tgz` |
| integrity | `sha512-2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow==` |
| packed / unpacked | 449,429 B / 2,552,052 B |
| files | 26（`index.d.ts`、`types/**`、`LICENSE`、`CHANGELOG.md`、README） |
| license | `MIT` |
| scripts | 有 `tsd`/`eslint`（仅开发；安装时不会执行，且本项目不 `npm install`） |

### 4.5 关键发现：npm 的 `typescript` 入口本身就是 Node launcher

`typescript@7.1.0-beta`（unscoped meta 包，558 文件）的 `bin/tsc` 只有 44 B：

```js
#!/usr/bin/env node
import "../lib/tsc.js";
```

`lib/tsc.js`（609 B）用 `node:child_process` 的 `execFileSync`/`process.execve` 启动
`getExePath()` 解析出的平台包 `lib/tsc`；`lib/getExePath.js` 按
`@typescript/<base>-<platform>-<arch>` 解析并校验 `pkg.bin` 恰有一个 `tsc`/`tsgo` 项。
因此：

1. 走 `npm install typescript` + `tsc` 入口**必然需要 Node**（`process.execve` 分支要求
   Node ≥22.15，旧版本退回 `execFileSync`，但两条路径都在 Node 内）；
2. 无 Node 的正确入口是直接执行平台包的 `lib/tsc`，参数原样透传（即
   `engine/lib/tsc --api --async`）；
3. 这也解释了为什么平台包不带 `bin`、只带 `lib/tsc` 与 d.ts。

证据：jsDelivr 上 `typescript@7.1.0-beta` 的 `bin/tsc`、`lib/tsc.js`、`lib/getExePath.js`
原文（小文本，未下载归档）；文件大小/路径由同一包清单核对。

## 5. 无 Node 交付算法（设计）

原则：**构建期显式 provisioning，安装期纯离线、纯哈希校验；运行期只执行 manifest 声明的
原生引擎，不查 PATH、不查 cwd、不执行 JS。**

### 5.1 阶段 A：显式 provisioning（构建机，唯一允许联网的步骤）

新增 `wxml-asset-provision`（或 `wxml-lsp-package provision`），输入是**仓库内提交的**
`assets.lock.json`（见 5.5），输出是内容寻址缓存 `<cache>/assets/<sha256>/`：

1. 只接受 `https://`；拒绝凭据/query/fragment/`..`/跨源 redirect（复用 `source.rs` 策略）。
2. `GET` tarball → 计算 **SHA-512** 与 SRI `sha512-<base64>` 比对 `assets.lock.json.integrity`；
   同时计算 SHA-256 存档（供 manifest/index 使用）。
3. 用**宽松于正式包**的 npm-tar 解包器解到 `<cache>/assets/tmp-<nonce>`：
   - npm 标准打包布局是顶层唯一 `package/`（下载后必须确认）；只允许 `package/` 下的路径；
   - 拒绝绝对路径、`..`、反斜杠、控制字符、重复路径、symlink/hardlink/FIFO/设备；
   - 限制文件数、单文件、总解压量、扩展头（沿用 `archive.rs` 的预算，但**模式归一化**：
     npm 归档可能不是 `0644/0755`，provisioning 阶段归一化为 `0644`，仅 `lib/tsc` 为 `0755`）；
   - `include` allowlist：`lib/**`、`LICENSE`、`NOTICE.txt`、`package.json`、`README.md`；
     其余文件丢弃但记录数量。
4. 生成 `provenance.json`：包名、版本、`gitHead`、`dist.integrity`、`dist.shasum`、
   归档 SHA-256/大小、解包文件数与总字节、每个文件的 SHA-256/大小/模式、下载时间与源 URL。
5. 原子 rename 到 `<cache>/assets/<sha256>/`；缓存命中时**重新逐文件校验**，不信任目录名。
6. 离线模式 `--offline` 只读缓存；缺失即失败（fail-closed），绝不隐式下载。

`miniprogram-api-typings` 走同一流程（MIT，26 文件），只保留 `index.d.ts`、`types/**`、
`LICENSE`、README/CHANGELOG。

### 5.2 阶段 B：打包

1. 从缓存复制：
   - `engine/` ← 平台包的 `lib/` + `LICENSE` + `NOTICE.txt` + `package.json` (+ README)
   - `typings/miniprogram-api-typings/` ← MIT 包
2. 构建 Rust `wxml-lsp`、`wxml-lsp-install` 与 Zed `extension.wasm`（纯 cargo，无 npm）。
3. 生成 `provenance.json` / `THIRD_PARTY_LICENSES.md`。
4. 调用 `wxml-lsp-package`：逐文件 SHA-256/大小/模式；manifest schema v2；`SHA256SUMS`；
   确定性 tar.gz（mtime=0、条目按路径排序；现有实现见 package.rs:230-242，
   W4 已证明两次构建 SHA-256 一致）。
5. 打包后自检：`verify-package` + `sha256sum -c`（无 Node）。
6. **负向自检**：扫描输出树，若出现 `node_modules/`、`*.js`/`*.cjs`/`*.mjs`、`bin/wxml-type-backend/`
   或可执行文件不在 allowlist，构建失败。

### 5.3 阶段 C：安装事务（无 Node）

```text
lock(root)
  read state.json（可选）→ previous = current
  acquire（--offline 只读缓存/本地源；否则显式 --enable-download）
  download archive → sha256(index) + size
  extract to root/tmp/<txn>/tree（严格解包，拒绝链接/遍历/设备/重复/超大）
  verify_tree(tree)（manifest v2 + SHA256SUMS 精确集合 + 逐文件哈希/模式）
  probe_native(tree)  ← 在 staged 树上执行，绝不执行已安装旧版
      engine/lib/tsc --version（10 s 超时、env_remove NODE_*/TS_NODE_*、cwd=staged engine）
      （可选）--api --async 握手/schema 探针，由 F0 冻结
  check_type_assets(tree)（lib.es2020.d.ts 等基准文件 + miniprogram index.d.ts + 许可文件）
  write txn journal {op, target, previous, state:"staged"} + fsync
  rename tree → versions/<target> + fsync(versions)
  journal state="committed"
  switch current（tmp symlink + rename） + fsync(root)
  journal state="activated"; 更新 state.json（active/previous/engine 身份）
  （可选）对 current 再跑一次 --version；失败则 switch(previous) 并报错
  cleanup txn/staging
unlock
```

关键改动：**原生探针在 rename 前**（现有 `check_binary` 已如此，见 install.rs:305-308），
因此“切换前必须验证”继续保持；`state.json` + journal 为原子性提供可恢复记录。

### 5.4 manifest schema v2（JSON）

```json
{
  "schemaVersion": 2,
  "package": "wxml-lsp",
  "packageVersion": "0.2.0",
  "platform": "linux-x86_64",
  "protocolVersion": 2,
  "builtAt": "2026-11-17T00:00:00Z",
  "engine": {
    "name": "typescript",
    "version": "7.1.0-beta",
    "sourceCommit": "3e9236361fa97a9e6c685f4833ac457ae2d8c50a",
    "distribution": "@typescript/typescript-linux-x64",
    "distributionIntegrity": "sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==",
    "apiSchemaVersion": "<F0 冻结值：调研提交源码为 9，运行时须重验>",
    "license": "Apache-2.0",
    "licenseFiles": ["engine/LICENSE", "engine/NOTICE.txt"],
    "binary": "engine/lib/tsc",
    "probe": { "args": ["--version"], "expectPrefix": "Version " }
  },
  "typeAssets": [
    {
      "name": "typescript-standard-lib",
      "version": "7.1.0-beta",
      "license": "Apache-2.0",
      "root": "engine/lib",
      "fileCount": 116,
      "sha256": "<sorted path+sha256 bundle digest>"
    },
    {
      "name": "miniprogram-api-typings",
      "version": "5.3.1",
      "license": "MIT",
      "root": "typings/miniprogram-api-typings",
      "integrity": "sha512-2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow==",
      "fileCount": 26,
      "sha256": "<bundle digest>"
    }
  ],
  "binaries": [
    { "kind": "rust", "name": "wxml-lsp", "path": "bin/wxml-lsp", "version": "0.2.0", "sha256": "…" },
    { "kind": "rust", "name": "wxml-lsp-install", "path": "bin/wxml-lsp-install", "version": "0.2.0", "sha256": "…" },
    { "kind": "native-engine", "name": "tsgo", "path": "engine/lib/tsc", "version": "7.1.0-beta", "protocolVersion": 2, "sha256": "…" },
    { "kind": "zed-extension", "name": "zed-extension", "path": "zed-extension/extension.wasm", "version": "0.0.1", "sha256": "…" }
  ],
  "files": [
    { "path": "bin/wxml-lsp", "sha256": "…", "size": 123456, "mode": "0755" }
  ]
}
```

约束（`deny_unknown_fields` 保持）：
- **删除** `node`；新增 `engine` 与 `typeAssets`；`ArtifactKind` 增加 `native-engine`，
  删除 `node-backend`（旧包由受限 legacy 读取器处理，不进 v2 校验）。
- `engine.binary` 必须位于 `engine/` 前缀下；v2 校验要求 `binaries[kind=native-engine].path == engine.binary`
  且 SHA-256 与 `files[]` 一致。
- `protocolVersion` 表示**引擎 RPC 协议**；`engine.apiSchemaVersion` 表示 AST/API schema（F0）；
  两者独立，不能互相替代。
- `files[]` 必须包含引擎、全部 d.ts、许可文件与 typings；`SHA256SUMS` 精确等于
  `files[] ∪ {manifest.json}`。
- 打包器额外要求 `engine/lib/lib.es2020.d.ts`、`engine/lib/lib.es5.d.ts`、
  `typings/miniprogram-api-typings/index.d.ts`、`engine/LICENSE`、`engine/NOTICE.txt` 存在。

示例中的 `engine.apiSchemaVersion`、`engine.probe.expectPrefix`、`protocolVersion` 是待 P1-A
实测/冻结的值，不是本试验已经验证的事实；冻结后写入 `assets.lock.json` 与 manifest。

索引 schema v2（`index.json`）：`Release` 用 `engineVersion`、`engineSourceCommit`、
`engineProtocolVersion`、`engineDistribution`、`engineIntegrity`、`minInstallerSchema` 替换
`nodeMinimumVersion`；旧安装器读到 schema 2 会因 `schemaVersion != 1` 拒绝（ manifest.rs:485-490），
这是期望的 fail-closed。

### 5.5 `assets.lock.json`（仓库内，构建期唯一事实源）

```json
{
  "schemaVersion": 1,
  "assets": [
    {
      "name": "typescript-native-engine",
      "npmPackage": "@typescript/typescript-linux-x64",
      "version": "7.1.0-beta",
      "sourceCommit": "3e9236361fa97a9e6c685f4833ac457ae2d8c50a",
      "tarball": "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz",
      "integrity": "sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==",
      "size": 10175135,
      "unpackedSize": 28450796,
      "fileCount": 122,
      "license": "Apache-2.0",
      "os": ["linux"],
      "cpu": ["x64"],
      "extract": {
        "stripPrefix": "package/",
        "include": ["lib/**", "LICENSE", "NOTICE.txt", "package.json", "README.md"],
        "executables": ["lib/tsc"]
      }
    },
    {
      "name": "miniprogram-api-typings",
      "npmPackage": "miniprogram-api-typings",
      "version": "5.3.1",
      "tarball": "https://registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.1.tgz",
      "integrity": "sha512-2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow==",
      "size": 449429,
      "unpackedSize": 2552052,
      "fileCount": 26,
      "license": "MIT",
      "extract": {
        "stripPrefix": "package/",
        "include": ["index.d.ts", "types/**", "LICENSE", "README.md", "CHANGELOG.md"],
        "executables": []
      }
    }
  ]
}
```

`assets.lock.json` 的变更必须作为独立、经过 P1-A 能力验证与许可复核的提交；禁止运行时解析
`dist-tags` 或“最新版”。这样即使 registry 元数据被篡改，内容仍被 SRI 与 lock 双重固定。

### 5.6 安全解包（复用与扩展）

正式包解包继续使用 `archive.rs` 的规则（拒绝链接/设备/遍历/重复/超大/异常模式，
`prescan_tar` 先扫描扩展头）。新增两点：

1. **npm tarball 专用** provisioning 解包器：允许 `package/` 前缀、归一化模式、
   `include` allowlist；不得把该宽松规则用于正式包安装。
2. **路径 allowlist 再校验**：正式包安装后，`verify_tree` 已保证集合精确；新增“不得有 JS”
   检查（按扩展名），避免引擎包/构建产物夹带 Node 启动器。
3. **SHA-256 allowlist**：安装树中只允许 `manifest.files[] ∪ {manifest.json, SHA256SUMS}`；
   任何额外路径、缺失路径、大小/模式/摘要不符立即失败（现有 `verify.rs:30-99` 已实现，
   原生包只需扩展必需文件集）。

### 5.7 原生探针（staged，失败不切换）

```rust
let mut cmd = Command::new(staged_engine);
cmd.arg("--version")
   .current_dir(staged_engine_dir)
   .env_clear()                      // 只保留必要项（如 LANG=C）
   .env_remove("NODE_OPTIONS")
   .env_remove("NODE_PATH")
   .env_remove("TS_NODE_PROJECT");
let out = output_with_timeout(&mut cmd, Duration::from_secs(10))?;
```

要求：
- stdout 前缀与 manifest `engine.probe.expectPrefix` 匹配，且版本 == manifest `engine.version`；
- 非零退出/超时/输出非 UTF-8 → `DeliveryError::Runtime`，删除 staged 树，`current` 不变；
- 版本探针通过后，P1-A 冻结的 `--api --async` 握手（Content-Length JSON-RPC、schema 版本）
  作为第二探针；未通过同样拒绝激活；
- 探针**不得**读取项目目录（cwd 为 staged engine 目录），不得加载项目 `tsconfig`/plugin。

### 5.8 回滚

`rollback --to <v>`：
1. 目标必须是已安装目录且 manifest schema == 2；schema 1 目录**明确拒绝**（“legacy Node package
   is not a Node-free target”），不删除、不改写；
2. 完整 `verify_tree`（逐文件哈希/模式/集合）+ `check_native`（引擎存在/可执行/版本匹配）；
3. 可选 staged 探针（对已安装目录执行，仍不改变 current）；
4. 记录 journal `{op:"rollback", target, previous, state:"staged"}` → `switch(target)` →
   `state:"activated"`；
5. 探针/切换失败 → 若 `previous` 存在则 `switch(previous)`，报错；绝不激活 Node 包。

## 6. 旧 Node → 原生升级/回滚状态机

### 6.1 状态与转换

```text
S0 无安装
S1 旧 Node current（schema1，Node 运行）
S2 原生 current N1（schema2）
S3 原生 N1 + 已 staged N2（未提交）
S4 原生 N2（已 committed，未 activated）
S5 原生 N2 activated
S6 回滚目标 N1 已验证（staged）
```

| 起始 | 动作 | 结果 | 不变量 |
| --- | --- | --- | --- |
| S0 | install native | 校验/探针通过 → 原子激活 | 失败不留 current |
| S1 | install native | **不执行旧二进制/Node**；旧目录只读识别；新包 staged 校验通过后一次 switch | 失败时旧树与 `current` 字节不变 |
| S1 | install native 失败/中断 | 报“尚未迁移”；旧环境是否还能跑取决于它原来的 Node，不算新包通过 | 不安装 Node、不修改旧树 |
| S1 | rollback/activate legacy | 明确拒绝（schema1 非 Node-free） | 不改 current |
| S2 | upgrade 到 N2 | 先 staged 校验/探针，后 switch | N2 失败时 N1 不变 |
| S2 | upgrade 中断（rename 后未 switch） | N2 目录存在但未激活；N1 仍 current | 混合资产不可见 |
| S4 | 恢复/继续 | 重新校验 N2；通过才 switch；不通过保持 N1 | 不出现半安装当前版本 |
| S5 | rollback N1 | 重新完整校验 N1 → switch | 离线也必须满足完整性 |
| S5 | 无可用原生回退 | 显式错误 + 静态能力降级 | 不自动激活 Node 包 |
| 任意 | 未知 schema/篡改/错误平台 | 执行前拒绝 | 已验证安装不受影响 |

`state.json`（示例）：

```json
{
  "schemaVersion": 2,
  "active": "0.2.0",
  "previous": "0.1.0",
  "engine": { "name": "typescript", "version": "7.1.0-beta", "sourceCommit": "3e92363…", "protocolVersion": 2 },
  "lastTransaction": { "id": "txn-…", "op": "upgrade", "state": "activated", "startedAt": "…", "finishedAt": "…" }
}
```

`txn/<id>.json` 只用 `staged|committed|activated` 三态；恢复规则固定：
- `staged` → 删除 `tmp/<id>`，current 不动；
- `committed` → 目标目录已 rename，current 若仍指 previous 则**不自动激活**；由显式
  `install`/`upgrade`/`rollback` 重新校验后再切换（只读命令不改变安装状态）；
- `activated` → 幂等确认 current 指向 target；不一致则报 `InstallState`，要求显式回滚。

### 6.2 不变量（必须由测试证明）

- **I1**：`current` 只在 staged 树完成 manifest/SHA-256/资产/原生探针之后改变。
- **I2**：任一步失败或中断，旧安装 `versions/<old>` 与 `current` **字节不变**（目录内文件哈希集合不变）。
- **I3**：N1→N2 失败保持 N1；N2 目录不得成为 current；不得残留混合资产（引擎/typings/二进制必须同一 manifest）。
- **I4**：rollback 只接受 schema2 且完整校验通过的目标；**永不**重新激活 schema1/Node 包。
- **I5**：任何路径都不执行 `node`/`npm`/`npx`/`pnpm`/`yarn`；不查 `PATH` 找引擎；
  `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` 被删除或忽略（推荐删除）。
- **I6**：离线缺缓存/缓存损坏 → 明确失败，current 不变，不发网络请求。
- **I7**：同一 root 的并发 mutating 命令由 OS 文件锁串行化；只读命令不加锁、不创建 root。
- **I8**：包内不得存在 Node 运行时文件（`node_modules/`、`*.js` 运行时），安装器不得执行它们。

### 6.3 崩溃窗口（现有实现 vs 目标）

| 窗口 | 现有实现 | 目标 |
| --- | --- | --- |
| 解包/校验中 | `tmp/` 目录；`acquire` 失败移除临时目录（ acquire.rs:123-126） | 保持；崩溃残留由下次 mutating 命令清理 |
| rename 前 | staged 树完整校验；失败删除 | 保持 + journal `staged` |
| rename 后 switch 前 | 版本目录已存在、current 仍旧；`install` 再次同版本会走 `AlreadyInstalled`（ install.rs:317-330） | 保持；journal `committed`；不自动激活 |
| switch 中 | symlink 临时名 + rename，原子 | 保持 + journal `activated` |
| switch 后报告探针失败 | 切回 previous（ wxml-lsp-install.rs:556-567） | 保留为额外保险；主探针已前移 |
| 目录持久性 | 无目录 fsync | 增加 `fsync(dir)`（versions、root、tmp） |

### 6.4 测试矩阵

单元（`crates/wxml-delivery`，确定性）：

| 测试 | 断言 |
| --- | --- |
| `rejects_native_engine_hash_mismatch_before_activation` | 无 `versions/<v>`、无 `current`、staged 清理 |
| `rejects_missing_standard_lib_or_miniprogram_typings` | 错误明确；current 不变 |
| `rejects_engine_version_mismatch_or_probe_timeout` | `DeliveryError::Runtime`；旧 current 不变 |
| `rejects_manifest_v1_for_rollback_activation` | schema1 目标明确拒绝；目录字节不变 |
| `accepts_manifest_v1_for_read_only_migration` | 能读旧 manifest/current；不执行旧二进制 |
| `recovery_staged_txn_only_removes_tmp` | current 与 versions 不变 |
| `recovery_committed_txn_does_not_mix_assets` | 引擎/typings/二进制来自同一 manifest |
| `rejects_js_payload_and_node_modules` | 打包器/校验器都拒绝 |
| `sri_mismatch_rejects_asset_before_extract` | 归档未解包、缓存未写入 |
| `npm_tarball_rejects_traversal_symlink_hardlink_duplicate` | 逐类恶意样本 |

集成（`crates/wxml-delivery/tests/installer_flow.rs`，进程级）：

| 测试 | 断言 |
| --- | --- |
| `legacy_node_install_to_native_failure_keeps_old_bytes` | 旧树逐文件 SHA-256 集合与 current 完全不变 |
| `legacy_node_install_to_native_success_switches_once` | 新 current；旧目录保留且未被改写；全过程无 node 执行 |
| `native_upgrade_failure_keeps_n1`（扩展 `failed_runtime_check_keeps_the_previous_active_version`， installer_flow.rs:520） | current 仍 N1；N2 未安装；`tmp/` 空 |
| `native_rollback_reverifies_n1_offline`（扩展 installer_flow.rs:199, 456） | 篡改 N1 后回滚失败且 current 仍 N2 |
| `crash_between_rename_and_switch_is_recoverable` | 预置 `versions/N2` + journal `committed`，current=N1；下一次 install/verify 不产生混合状态 |
| `no_node_shim_is_ever_executed` | PATH 全部替换为记录调用的 shim（node/npm/npx/pnpm/yarn）；日志为空 |
| `upgrade_refuses_downgrade_and_same_version_rebuild`（已有， installer_flow.rs:549） | 保持 |
| `concurrent_installs_are_serialized`（已有， installer_flow.rs:284） | 保持 |

进程级崩溃注入建议：单测直接驱动状态机并在每个转换后模拟“进程消失”；集成层再补
“在若干时间点 `SIGKILL` 安装器”的随机化测试作为补充（不作为唯一证据，避免时序不稳定）。

## 7. 无 Node 验收命令（clean environment）

### 7.1 环境构造

```bash
# 目录中只放构建/安装需要的工具，明确不放 node/npm/npx/pnpm/yarn
NONODE=/tmp/opencode/nonode-bin
mkdir -p "$NONODE"
for t in bash sh cargo rustc cc gcc ld ar make pkg-config tar gzip sha256sum realpath uname date awk sed grep find; do
  p=$(command -v "$t") && ln -sf "$p" "$NONODE/$t"
done
export PATH="$NONODE"
for t in node npm npx pnpm yarn; do ! command -v "$t" >/dev/null || { echo "unexpected $t"; exit 1; }; done
```

工具清单按本机实际安装补齐（缺少的 `cc`/`pkg-config`/`make` 等按目标发行版替换，但不得加入 node）。

环境隔离：`HOME`、`XDG_DATA_HOME`、`CARGO_HOME`、`CARGO_TARGET_DIR` 都指向
`/tmp/opencode/<run>`；`HTTPS_PROXY=http://127.0.0.1:9` 或 `unshare -rn` 用于离线证明。

### 7.2 命令矩阵（映射 NF-01～NF-12）

下面是 schema v2 安装器落地后的**目标命令**，不是本工作树今天可执行的命令。

```bash
# NF-04/NF-02：构建与测试
cargo build --workspace --locked --offline
cargo test --workspace --all-features --locked --offline
cargo test --workspace --doc --locked --offline
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check

# NF-04：release dry-run 与完整构建（资产已 provisioning 到缓存）
scripts/build-release.sh --dry-run --output /tmp/opencode/release
scripts/build-release.sh --output /tmp/opencode/release --offline

# NF-06：安装/校验/回滚/环境（全部 --offline）
PKG=/tmp/opencode/release/wxml-lsp-0.2.0-linux-x86_64
INST="$PKG/bin/wxml-lsp-install"
"$INST" install   --root /tmp/opencode/root --from-dir /tmp/opencode/release --version 0.2.0 --offline
"$INST" verify    --root /tmp/opencode/root
"$INST" current   --root /tmp/opencode/root
"$INST" env       --root /tmp/opencode/root
"$INST" verify-package --from-dir /tmp/opencode/release --version 0.2.0 --offline
"$INST" rollback  --root /tmp/opencode/root --to 0.1.0 --offline

# NF-05/NF-09：布局、许可、摘要
find "$PKG" -type f | sort
(cd "$PKG" && sha256sum -c SHA256SUMS)
! find "$PKG" \( -name node_modules -o -name '*.js' -o -name '*.cjs' -o -name '*.mjs' \) | grep .
test -f "$PKG/engine/LICENSE" && test -f "$PKG/engine/NOTICE.txt"
test -f "$PKG/typings/miniprogram-api-typings/LICENSE"
test -x "$PKG/engine/lib/tsc"

# NF-01/NF-07：进程审计（安装器）
strace -f -e trace=execve,execveat -o /tmp/opencode/install.exec \
  "$INST" install --root /tmp/opencode/root --from-dir /tmp/opencode/release --version 0.2.0 --offline
! rg '"/(node|npm|npx|pnpm|yarn)"' /tmp/opencode/install.exec

# NF-01：产品运行审计（stdio 会话）
# 由 P1-A/P3 的真实 stdio 测试驱动发送 initialize/shutdown 的 Content-Length 帧；
# 这里只示范如何对驱动进程做 exec 审计。整段必须在无 Node 的 PATH 下运行。
strace -f -e trace=execve,execveat -o /tmp/opencode/lsp.exec \
  <native-stdio-driver> /tmp/opencode/root/current/bin/wxml-lsp
! rg '"/(node|npm|npx|pnpm|yarn)"' /tmp/opencode/lsp.exec
rg '"engine/lib/tsc"|"/root/current/bin/wxml-lsp"' /tmp/opencode/lsp.exec

# NF-07：Zed 启动（Linux，隔离 HOME/XDG/DISPLAY）
env -i HOME=/tmp/opencode/zedhome XDG_DATA_HOME=/tmp/opencode/zeddata \
  DISPLAY=:99 PATH="$NONODE" WXML_LSP_BIN=/tmp/opencode/root/current/bin/wxml-lsp \
  zed --foreground /path/to/tests/fixtures/page-basic
pgrep -af 'wxml-lsp --stdio'
! pgrep -af '(^|/)(node|npm|npx)( |$)'

# ABI/glibc
ldd "$PKG/engine/lib/tsc"
readelf --version-info "$PKG/engine/lib/tsc" | rg -o 'GLIBC_[0-9.]+' | sort -Vu | tail -1
readelf -h "$PKG/engine/lib/tsc" | rg 'Class|Machine|Type'
```

说明：
- `strace` 不是所有环境都有；没有时用“PATH shim 记录调用”作为辅助（ADR/计划 §8 已说明 shim
  不能排除绝对路径调用，因此 shim 只作失败检测，不能单独证明）。
- 进程审计按**进程树**（安装器/LSP 及其子进程）判定，不能因为系统其它进程有 node 就误报。
- `env -i` 下 Zed 可能需要 `XDG_RUNTIME_DIR`、DBus 等；这些不是 Node，按实际补齐并记录。
- `--api --async` 握手/查询矩阵属于 P1-A，不在 C 的命令里冒充。

### 7.3 本工作树可证明 / 不可证明

| 项 | 状态 |
| --- | --- |
| 静态依赖盘点、schema/状态机/命令设计 | ✅ 本文可复核 |
| registry 元数据、SRI、许可、包内文件清单 | ✅ 已核实（未下载 tarball） |
| 原生资产真实解包、`lib/tsc --version`、`--api` 探针 | ⛔ BLOCKED（资产下载归另一 worker；本环境无该二进制） |
| schema v2 安装器、事务日志、无 Node 安装 | ⛔ BLOCKED（D 包未实现） |
| glibc/ABI、musl、aarch64 | ⛔ BLOCKED（无资产；仅有元数据） |
| Linux Zed UI 冒烟 | ⛔ BLOCKED（需要构建扩展 + 图形环境 + 新安装包） |
| 离线 fail-closed、进程审计 | ◐ 命令已定义；需在真实包上执行 |

## 8. 风险登记

| ID | 风险 | 证据/位置 | 处置（P2 前必须冻结） |
| --- | --- | --- | --- |
| R1 | **FS 回调回退到 OS 文件系统**：未注册的回调可能读真实磁盘；`--api` 不是沙箱 | ADR-0008 “文件访问”；计划 §3 “未注册 FS 回调…可能读真实磁盘” | 注册全部读回调并走显式 allowlist；对每个回调做访问轨迹测试；禁止 `--useOS`/external code |
| R2 | **`noEmit` 不是沙箱**：写入口/emit/build/clean 仍可能写盘 | ADR-0008 “noEmit 和禁止 writeFile 不等于 OS 沙箱” | 不启用 emit/build/clean/external plugins；写回调返回显式错误并记录，不静默 no-op |
| R3 | **AppendFile/Chtimes 等写回调**语义不明 | 计划 §3 要求验证所有可达入口 | 用真实引擎探测每个回调；返回错误而非假装成功；测试断言无项目文件被写 |
| R4 | **external plugins / tsconfig `plugins` / `extends`** 可加载仓库外代码或文件 | 计划 §3、§6.4 | 禁止 plugins；`extends` 限制在快照内；配置固定 `types`/`typeRoots`；测试恶意 tsconfig |
| R5 | **lib.d.ts 许可与来源**：Apache-2.0 需 LICENSE+NOTICE；版本/文件可能变化 | §4.1；`engine/LICENSE` 9,197 B、`NOTICE.txt` 46,827 B | 逐文件哈希 + 随包许可 + `provenance.json`；升级必须重新生成并评审 |
| R6 | **ABI/glibc/musl**：包元数据无 `libc`，二进制动态链接；Rust 二进制也绑定构建机 glibc | §4.1；本机 glibc 2.43 | 在最低支持发行版上 `readelf`/`ldd` + 真实运行；musl 不宣称支持；记录 `GLIBC_*` 上限 |
| R7 | **摘要固定/供应链**：只有 TLS+SHA-256，无发布者签名；依赖 registry 元数据 | ADR-0007 残留风险；§5.5 | 仓库内 `assets.lock.json` 固定 URL+SRI+size；provisioning 只认 lock；未来加 detached 签名 |
| R8 | **隐式 Node 回退**：`WXML_NODE`/`WXML_TYPE_BACKEND`/`server.js` 探测/`PATH` 查找 | §2.1、§2.5 | 删除三个变量转发；删除 `default_server_path` 的 JS 布局；只允许 manifest 声明的引擎 |
| R9 | **npm tarball 解包细节**：`package/` 前缀、模式可能是 0664、PAX 扩展头 | §5.1；现有解包器只接受 0644/0755（ archive.rs:539-543） | 独立 provisioning 解包器 + allowlist + 模式归一化；恶意样本测试 |
| R10 | **回归面**：引擎 7.x 诊断/类型结果与 TS 6.0.3 不同；fixture/索引可能与旧版不一致 | 计划 §6.2；`miniprogram-api-typings` 仍 5.3.1 | 先跑 P1-A/P1-B；逐例差分；不得批量覆盖 oracle；引擎/资产升级单独提交 |
| R11 | **包体积/回滚保留**：引擎 28.4 MB + typings 2.5 MB；多版本占用磁盘 | §4.1/§4.4 | manifest/index 记录大小；文档给出 `versions/` 清理策略；不自动删除回滚目标 |
| R12 | **`tsc.sig` 用途未知**：481 B，是否运行期必需未验证 | §4.1 | 随包保留并哈希；P1-A 删除/保留对照实验后再决定是否作为必需项 |

## 9. P1-C 判定与建议

### 9.1 判定

- **PASS**：依赖盘点、现有交付/ADR-0007 审查、registry 元数据与许可、manifest/状态机/命令设计。
- **PARTIAL**：状态机与验收只有设计和既有测试映射；没有 schema v2 实现。
- **BLOCKED**：原生资产真实下载/解包/探针、无 Node 安装、ABI、Zed 冒烟（都不是本只读任务可完成的）。

因此 **G1 的 P1-C 不能标记为完成**；但“资产身份与无 Node 入口是否可行”这一关键未知已消除：
npm 的 `typescript` 入口是 Node launcher，平台包的 `lib/tsc` 才是可无 Node 直接执行的引擎；
其 `gitHead` 与 ADR 引用的源码提交一致，许可/摘要/平台/文件清单齐全。

### 9.2 对 P2/F3 的具体建议（按优先级）

1. **固定资产决策**：优先评估 `@typescript/typescript-linux-x64@7.0.2`（stable）与
   `7.1.0-beta`；由 P1-A 的 `--api/--async`、lib 解析、AST/schema 结果决定。禁止浮动版本。
2. **schema v2 先落地**：`SCHEMA_VERSION=2`、删除 `node`、新增 `engine`/`typeAssets`、
   `ArtifactKind::NativeEngine`、索引 v2；旧安装器对 schema2 必须 fail-closed（已具备）。
3. **受限 legacy 读取器**：只读 schema1 的 `current`/manifest，用于迁移识别；不得执行旧二进制、
   不得 rollback 激活 schema1。
4. **安装器改造**：`check_backend` → `check_native` + `check_type_assets`；删除 `check_node`；
   `env` 不再输出 `WXML_TYPE_BACKEND`；新增 `state.json`/`txn` 日志与目录 fsync。
5. **构建链改造**：新增 `wxml-asset-provision`（或 `package provision`）与 `assets.lock.json`；
   `build-release.sh` 删除 node/npm 前置与 npm 步骤；CI 增加 no-node guard 与 native 必跑 job。
6. **Zed 改造**：`FORWARDED_ENV` 删除 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`；
   只保留超时/模板开关；仍以 `WXML_LSP_BIN` 为唯一入口。
7. **测试迁移**：两个 `semantic_tools` 静默跳过改为硬门禁或迁移；23 个 ignored 门禁在
   native 资产就绪后变为必跑；新增本文 §6.4/§5.6 的测试。
8. **许可/证据**：随包 `LICENSE`/`NOTICE`/MIT 许可 + `provenance.json` + `THIRD_PARTY_LICENSES.md`；
   证据记录引用 registry URL、`gitHead`、SRI 与文件数。

### 9.3 交给其它 worker 的最小验证（不重叠本任务）

- 资产 worker：下载 `@typescript/typescript-linux-x64@7.1.0-beta`（或 7.0.2）→ 校验 SRI →
  按 §5.1 解包 → 记录 `lib/tsc --version`、`--api --async` 握手、`ldd`/`readelf`、
  `lib/*.d.ts` 数量/哈希、`tsc.sig` 删除对照。
- D 包：实现 schema v2 + provisioning + 安装器改造 + §6.4 测试。
- C 包：在资产 worker 的缓存上跑 §7.2 命令，产出 clean-env 日志。
- P6/P7：用真实本地包重跑旧→新/升级/回滚、Node 未安装、离线命中/缺失、提交前后 kill、
  并发事务，并做 Linux Zed 冒烟。

## 10. 证据来源与复现

### 10.1 registry 查询（HTTPS，可在任意机器重放）

```bash
curl -fsS -H 'Accept: application/vnd.npm.install-v1+json' \
  'https://registry.npmjs.org/@typescript%2ftypescript-linux-x64/7.1.0-beta'
curl -fsS 'https://registry.npmjs.org/@typescript%2ftypescript-linux-x64'
curl -fsS 'https://registry.npmjs.org/@typescript%2ftypescript/7.1.0-beta'
curl -fsS 'https://registry.npmjs.org/@typescript%2ftypescript-linux-x64/7.0.2'
curl -fsS 'https://registry.npmjs.org/miniprogram-api-typings/5.3.1'

# 文件清单（已发布包，不含归档字节）
curl -fsS 'https://data.jsdelivr.com/v1/packages/npm/@typescript/typescript-linux-x64@7.1.0-beta?structure=flat'
curl -fsS 'https://data.jsdelivr.com/v1/packages/npm/typescript@7.1.0-beta?structure=flat'

# 打包大小（1 字节 range，不下载归档）
curl -s -r 0-0 -D - -o /dev/null \
  'https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz'
```

### 10.2 仓库内证据

- 现有发布 manifest 实例（本机历史产物，**不在仓库内、不作为唯一证据**）：
  `/tmp/opencode/final-release-3/wxml-lsp-0.1.0-linux-x86_64/manifest.json`
  （`schemaVersion=1`、`node.minimumVersion=24.0.0`、211 files、`node-backend` 产物）。
  仓库内可复核的等价定义： ` crates/wxml-delivery/src/manifest.rs:116-138`（Manifest 字段）、
   ` crates/wxml-delivery/src/package.rs:382-399`（staged 必需文件）。
- ADR-0007 信任模型： ` docs/adr/0007-local-delivery-trust-model.md`（默认关闭下载、HTTPS、
  原子安装、`WXML_LSP_BIN` 优先、无发布者签名）。
- 计划 §8/§8.1：旧安装迁移、失败/回退状态机与 NF-01～NF-12。
- ADR-0008：运行时/构建/测试/发布去 Node、`lib.d.ts`/typings 许可与摘要、旧安装迁移。

### 10.3 本记录的等级与限制

等级：**E1（可复查）**。结论可由本节命令与仓库行号复核；缺原始解包/运行输出，
未达到 E2。所有“PASS”仅指静态调查或元数据核验，不代表实现/门禁已通过。
`docs/evidence/README.md` 的索引登记由文档 owner（R 包）统一处理，本轮按只读约束未修改。

## 11. 2026-11-17 追加：可执行安装器 spike（临时证明）

本节是**追加的临时证明**，不改变 §0～§10 的历史结论。§10.3 的 E1 等级仍适用于 §0～§10；
§11 虽然包含原始 spike 输出，但它只是临时证明，不能把整份记录升级为 E2 或生产验收。它不是
生产安装器、不是 F3 实现、不是 release 产物。工作目录固定为 `/tmp/opencode/tsgo-delivery-spike`；原生资产复用
`/tmp/opencode/tsgo-spike` 中**已存在**的 7.1.0-beta 解包与 tarball，**未重新下载**；未运行
`node`/`npm`；未修改生产源码、Cargo、脚本、CI、Zed、计划或 ADR。仓库写入仅本证据与
[F3 交付合同](../reference/tsgo-delivery-contract.md)。

### 11.1 结论摘要

| 项 | 结果 |
| --- | --- |
| 临时 Python 安装器（manifest v2/index/state/journal/provision/install/verify/rollback） | ✅ PASS（Linux x64 本地） |
| 真实 7.1.0-beta tarball SRI/SHA-256/size/122 文件/`lib/tsc` 复核 | ✅ PASS |
| fresh install、verify、atomic current | ✅ PASS |
| failed checksum / tampered archive 保持旧 current 与旧树 | ✅ PASS |
| interrupted upgrade（commit 后退出 137）保持 previous；recover 不自动激活 | ✅ PASS |
| native N1→N2 upgrade；rollback N2→N1 | ✅ PASS |
| legacy Node current 迁移（旧树不变）；rollback 到 legacy 明确拒绝 | ✅ PASS |
| clean PATH 无 node/npm；fake `/usr/bin/node`/npm shim 未被调用 | ✅ PASS（受限审计，见 11.6） |
| offline install/verify（`unshare -rn`，网络不可达） | ✅ PASS |
| unsafe tar（绝对路径/`..`/symlink/hardlink/FIFO/坏模式/重复） | ✅ PASS（7/7 拒绝） |
| 版本/`--version` mismatch 拒绝 | ✅ PASS |
| `miniprogram-api-typings` SRI | ◐ **NOT VERIFIED**（无 tarball；只验 26 文件/版本/许可/包内 digest） |
| F0 `--api --async` / AST 握手由安装器消费 | ⛔ **NOT RUN**（本次只做 `--version` 探针） |
| 生产 Rust 安装器 / Rust build/test/CI / Zed 冒烟 | ⛔ **BLOCKED** |
| 平台矩阵 / Rust glibc / musl / aarch64 | ⛔ **BLOCKED** |
| 许可/provenance 发布评审 | ◐ **PARTIAL**（spike 已打包并 hash，未评审） |
| 断电/fsync 持久性 | ⛔ **BLOCKED**（只测进程退出，不测断电） |

**G1-C 判定：仍不能关闭。** 本次只把“可执行安装器存在性”从 BLOCKED 降为
**PARTIAL PASS（临时证明）**；生产实现、clean-env Rust 门禁、平台/ABI、Zed、许可评审仍未满足。
详见 §11.8。

### 11.2 边界与不可替代性

- 安装器是 **Python 3.14.10** 临时脚本，不是 Rust `wxml-delivery`；包内 `bin/wxml-lsp` 是
  打印固定版本号的 shell placeholder，不是真实 Rust LSP。任何“安装成功”只证明流程与校验，
  不证明真实 LSP/原生 API。
- 只使用 `--from-dir` 本地源；spike **没有实现网络下载**。因此离线测试证明“本地安装不依赖
  网络”，不能证明生产 HTTPS 获取实现。
- 进程审计使用 `bwrap --unshare-net` + 替换 `/usr/bin/node` 和 npm 入口为“记录并退出 99”的
  shim；系统未安装 `strace`，因此这不是完整 execve 审计。clean PATH 与源码审查作为补充。
- 原生引擎 `lib/tsc` 是上游静态 Go 二进制；Rust LSP 的 glibc 下限未测，因为本次没有构建
  Rust 二进制。
- 类型资产 `miniprogram-api-typings` 来自仓库内已解包的 5.3.1（26 文件、MIT），没有对应
  tarball，故无法重算 `sha512-2Cir…` SRI；只验证版本、文件数、字节数、许可和包内 bundle digest。
- 没有修改生产安装器；本文的 B1/B2/B4/B7/B9 状态更新只描述 spike 的实际输出。

### 11.3 复用资产的复核

复用的 tarball：`/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz`。
已解包目录：`/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package`。

| 项 | 实测值 | 与维护者/F0 值 |
| --- | --- | --- |
| tarball size | `10,175,135` B | 一致 |
| tarball SHA-256 | `59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855` | 一致 |
| tarball SRI | `sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==` | 一致 |
| 解包文件数 | `122` | 与 registry 清单一致 |
| 解包总字节 | `28,450,796` | 一致 |
| `lib/tsc` 大小 | `24,522,912` B | 一致 |
| `lib/tsc` SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` | 一致 |
| `lib/tsc --version` | `Version 7.1.0-beta`（exit 0） | 一致 |
| `lib/*.d.ts` | `116` 个，含基础 `lib.d.ts` | 发现 include 陷阱：`lib.*.d.ts` 只匹配 115 |
| `miniprogram-api-typings` | `5.3.1`、MIT、`26` 文件、`2,552,052` B | 版本/文件数/字节一致；**SRI 未复核** |
| 引擎链接 | `ldd` 输出“not a dynamic executable”；`readelf` Class `ELF64`、Type `EXEC`、Machine `x86-64` | 静态 Go；无 GLIBC version info |

复核命令（`$PKG=dist/wxml-lsp-0.1.0-linux-x86_64`）：

```sh
LC_ALL=C ldd "$PKG/engine/lib/tsc"          # exit 1, "not a dynamic executable"
LC_ALL=C readelf -h "$PKG/engine/lib/tsc"   # Class ELF64, Type EXEC, Machine x86-64
LC_ALL=C readelf --version-info "$PKG/engine/lib/tsc"  # no version information
```

`lib.d.ts` 是基础声明：`find .../lib -name 'lib*.d.ts'` 为 116，`-name 'lib.*.d.ts'` 为 115。
仓库合同已据此把标准库 include 从 `lib.*.d.ts` 修正为 `lib*.d.ts`，并断言 `fileCount == 116`。
这是本次 spike 直接发现并修正的合同缺陷。

### 11.4 临时实现物

只在 `/tmp/opencode/tsgo-delivery-spike`：

| 路径 | 作用 |
| --- | --- |
| `spike.py` | Python 安装器/校验器/打包器；manifest v2、index v2、state/journal、safe tar、probe |
| `run_tests.sh` | 临时测试驱动；记录命令、exit code、digest、审计 |
| `clean-path/{python3,sh,env}` | 无 node/npm 的 PATH |
| `dist/` | 临时 release 0.1.0、0.2.0、0.3.0 与 index |
| `dist-bad-archive/` | 篡改 0.2.0 归档，用于 checksum 失败测试 |
| `root-main/`、`root-crash/`、`root-legacy/`、`root-refuse/`、`root-tamper/`、`root-offline/`、`root-audit/` | 各测试安装根 |
| `logs/run-20261117T154936Z.log` | 完整命令/输出/exit code 日志（主证据） |
| `audit/exec.log` | fake node/npm shim 日志；本次为 0 字节 |

临时包不是可发布产物：`bin/wxml-lsp` 是 shell 脚本；`zed-extension/extension.wasm` 是文本占位；
`provenance.json` 明确标记 `"spike": true`。manifest 的 `typeAssets`、`files[]`、`SHA256SUMS`
由 spike 按合同字段生成并校验。

### 11.5 命令与结果

变量：`SPIKE=/tmp/opencode/tsgo-delivery-spike`，`CLEAN=$SPIKE/clean-path`，
`PY=/usr/bin/python3`。所有 `run_clean` 命令均为：

```sh
# clean PATH check
env -i PATH="$CLEAN" /bin/sh -c 'command -v node || true; command -v npm || true'

# ordinary clean-env run
env -i PATH="$CLEAN" HOME="$HOME" LANG=C LC_ALL=C "$PY" "$SPIKE/spike.py" <subcommand...>

# network-disabled run
unshare -rn env -i PATH="$CLEAN" HOME="$HOME" LANG=C LC_ALL=C "$PY" "$SPIKE/spike.py" \
  install --root "$SPIKE/root-offline" --from-dir "$SPIKE/dist" --version 0.1.0 --offline

# process audit run (fake /usr/bin/node + npm log and exit 99)
bwrap --unshare-net --ro-bind / / --dev /dev --proc /proc \
  --bind "$SPIKE" "$SPIKE" \
  --bind "$SPIKE/audit/fake-node" /usr/bin/node \
  --bind "$SPIKE/audit/fake-npm" /usr/lib/node_modules/npm/bin/npm-cli.js \
  -- /usr/bin/env -i PATH="$CLEAN" HOME="$HOME" LANG=C LC_ALL=C \
  SPIKE_EXEC_LOG="$SPIKE/audit/exec.log" \
  "$PY" "$SPIKE/spike.py" install --root "$SPIKE/root-audit" \
  --from-dir "$SPIKE/dist" --version 0.1.0 --offline

# network-namespace probe
unshare -rn /usr/bin/python3 -c \
  'import socket; socket.create_connection(("1.1.1.1", 80), 2)'
# -> OSError: [Errno 101] Network is unreachable
```

`CLEAN` 只含 `python3`、`sh`、`env` 三个 symlink；`command -v node` 与 `command -v npm` 均为空。
完整日志：`logs/run-20261117T154936Z.log`。主测试结果：

| # | 命令（摘要） | exit | 结果 |
| --- | --- | ---: | --- |
| 1 | `spike.py provision --dist "$SPIKE/dist"` | 0 | 122 文件/28,450,796 B；三个临时包与 index |
| 2 | `spike.py install --root root-main --from-dir dist --version 0.1.0 --offline` | 0 | current=`versions/0.1.0` |
| 3 | `spike.py verify --root root-main` | 0 | manifest/files/SHA256SUMS/engine probe 全通过 |
| 4 | `install ... --version 0.2.0 --index index-bad-sri.json` | 1 | `archive SRI mismatch` |
| 5 | `install ... --version 0.2.0 --index index-bad-sha.json` | 1 | `archive sha256 mismatch` |
| 6 | `install ... --from-dir dist-bad-archive --version 0.2.0` | 1 | 篡改归档 `archive sha256 mismatch` |
| 7 | `install ... 0.2.0 --crash-after-commit` | **137** | 模拟 commit 后进程消失 |
| 8 | `spike.py recover --root root-main` | 0 | `committed -> kept target 0.2.0; not auto-activated` |
| 9 | `install ... 0.2.0 --offline`（resume） | 0 | current=`0.2.0`；state.previous=`0.1.0` |
| 10 | `install ... 0.3.0 --offline` | 1 | placeholder 打印 `wxml-lsp 0.2.9`，version mismatch |
| 11 | `spike.py rollback --root root-main --to 0.1.0` | 0 | current=`0.1.0`；state.previous=`0.2.0` |
| 12 | `install ... 0.2.0 --offline` | 0 | current=`0.2.0` |
| 13 | `verify`（篡改 `lib.es2020.d.ts`） | 1 | `size mismatch` |
| 14 | `verify`（额外 `extra.txt`） | 1 | `file set mismatch` |
| 15 | `make-legacy --root root-legacy` + `install ... 0.1.0` | 0 | legacy current → native；旧树 digest 不变 |
| 16 | `rollback --root root-refuse --to 0.0.9` | 1 | `legacy Node package is not a Node-free target` |
| 17 | `unshare -rn` + `install ... 0.1.0 --offline` | 0 | 无网络命名空间安装成功 |
| 18 | `unshare -rn` + `verify` | 0 | 离线 verify 成功 |
| 19 | `unshare -rn` + `install`（空 `from-dir`） | 1 | `missing JSON file: .../index.json` |
| 20 | bwrap fake node/npm + `install`/`install`/`rollback` | 0/0/0 | `audit/exec.log` 0 字节 |
| 21 | `spike.py test-archives` | 0 | 7/7 恶意条目拒绝 |
| 22 | `sha256sum -c SHA256SUMS`（0.1.0 包根） | 0 | 156 条全部成功 |

**归档/清单摘要**：

| 产物 | size | SHA-256 | SRI |
| --- | ---: | --- | --- |
| `wxml-lsp-0.1.0-linux-x86_64.tar.gz` | 10,517,620 | `950cbf617d9c0d83cfe0c43c84c14310abc254a269036106e250fb7250e04f4e` | `sha512-ZFZrRaArMVLRQXd5y3RefboRjkG/pjT6xUF/4umnmrPN9bJhbMCz1RZgTxZScNXkDpU1uDB/a828J6bVc/1HRQ==` |
| `wxml-lsp-0.2.0-linux-x86_64.tar.gz` | 10,517,600 | `30817627671ed987c1454ba25db2ed60df130d703e474ce03bd0c11dcdf470a7` | `sha512-NJERf4GCpc6v6BIxgiPGHQ2GizHK1K6VfQguQtMOlO1x4vn29JP2OLvxhFgvzohl3ckCz6JWcH6YwNfTnU6DpA==` |
| `wxml-lsp-0.3.0-linux-x86_64.tar.gz`（故意坏版本） | 10,517,623 | `06da50c66ca3c516543d134c8b8675b8e95f42c6bf3ff6711c0d410deb97ead7` | `sha512-TCbgjgCG7X/EfLFhVROouJ5l001uSBHWNTrL4MucrF2Z6BovObOr6BGxs6zLj7FE/i3CLXsD1FhfoqUJFy1tfA==` |
| `0.1.0/manifest.json` | 32,520 | `c4f3b4dafb3cc9dbb632e02af13f4518adb6ed054b4708d1ae2908f951ab1204` | — |
| `0.1.0/SHA256SUMS` | 15,978 | `d2afda41e8bc5fee269269cc7830df51e3724cf424c78fd988d52c5eed947395` | — |

包内 `files[]` 为 155 项；`SHA256SUMS` 为 156 行（155 + `manifest.json`）。标准库 116 个
`.d.ts`，typings 26 个文件，均与 manifest 的 `fileCount`/bundle digest 一致。

### 11.6 状态机观测

| 场景 | journal / state | current | 旧树 digest |
| --- | --- | --- | --- |
| fresh install V1 | `staged` → `committed` → `activated`；state active `0.1.0`，previous `null` | `versions/0.1.0` | — |
| failed checksum（3 次） | 未创建新 journal；state/current 不变 | `versions/0.1.0` | `728c3959cce6e63a6c1da04f53caeb0803486543edad61f0614aa00ec2fd9b07`（前后一致，157 项） |
| interrupted upgrade V2 | journal `committed`，target `0.2.0`，previous `0.1.0`；进程 `os._exit(137)` 在 switch 前 | `versions/0.1.0` | `728c3959…`（前后一致） |
| `recover` | `committed -> kept target 0.2.0; not auto-activated`；tmp 清理 | `versions/0.1.0` | 不变 |
| explicit resume V2 | 重新校验现有 `0.2.0` → switch；state active `0.2.0`，previous `0.1.0` | `versions/0.2.0` | V1 `728c3959…` 不变 |
| version mismatch V3 | probe 失败；未写 `staged`/`committed`；tmp 清理 | `versions/0.2.0` | V2 `f90c81b65ac617d9c4c60f80b992e1b643b6e5fbee9221872be7c3d9831780ce`（前后一致） |
| rollback N2→N1 | `committed` → `activated`；state active `0.1.0`，previous `0.2.0` | `versions/0.1.0` | V2 不变 |
| legacy migration | 旧 `current` 只读识别；`staged` → `committed` → `activated`；state active `0.1.0`，previous `null` | `versions/0.1.0` | legacy `a23de21aaf35dc47e278cca96045ec7268f9107c01eebc8125b89a4ae0debf78`（前后一致，4 项） |
| legacy rollback refusal | 未创建 journal；schema v1 在 verify/switch 前拒绝 | `versions/0.1.0` | legacy `a23de21a…`、native `728c3959…` 均不变 |

`root-crash` 的 journal 原文（commit 后、switch 前）：

```json
{
  "schemaVersion": 2,
  "id": "txn-20261117T155149Z-2161662-158",
  "op": "install",
  "target": "0.2.0",
  "previous": "0.1.0",
  "state": "committed",
  "startedAt": "2026-11-17T15:51:49Z",
  "updatedAt": "2026-11-17T15:51:49Z"
}
```

`root-crash` 的 current 仍为 `versions/0.1.0`；`recover` 输出
`committed -> kept target 0.2.0; not auto-activated`。这证明“commit 后中断不自动激活”，
但不证明断电持久性。

### 11.7 P1-C 测试判定

| P1-C 测试 | 判定 | 依据/限制 |
| --- | --- | --- |
| fresh install | ✅ PASS | 0.1.0 安装、verify、atomic current、state/journal |
| failed checksum keeps old current byte-identical | ✅ PASS | 坏 SRI / 坏 SHA / 篡改归档均 exit 1；V1 digest 前后一致 |
| interrupted/failed upgrade keeps previous | ✅ PASS | exit 137；current/旧树不变；recover 不自动激活 |
| native N1→N2 upgrade | ✅ PASS | 0.1.0→0.2.0；state.previous=`0.1.0` |
| rollback to N1 | ✅ PASS | 0.2.0→0.1.0；重新校验；state.previous=`0.2.0` |
| refusal to reactivate legacy Node package | ✅ PASS | `rollback --to 0.0.9` exit 1；current/legacy/native digest 均不变 |
| legacy migration | ✅ PASS | legacy current → native；旧树 digest 不变；previous=null |
| no Node process spawned | ✅ PASS（受限） | clean PATH 无 node/npm；fake `/usr/bin/node`+npm shim 0 调用；无 `strace`，非完整 execve 审计 |
| offline install/verify | ✅ PASS | `unshare -rn` 内安装/verify 成功；网络探测 `OSError [Errno 101] Network is unreachable` |
| unsafe tar rejection | ✅ PASS | absolute/`..`/symlink/hardlink/FIFO/0777/duplicate 共 7/7 拒绝 |
| version/`--version` mismatch | ✅ PASS | 0.3.0 包打印 0.2.9，exit 1，V2 current 不变 |
| tamper detection | ✅ PASS | 改 d.ts 或加 extra 文件，verify exit 1 |
| engine SRI/SHA-256/file count | ✅ PASS | tarball/122 文件/28,450,796 B/`lib/tsc` 全部一致 |
| miniprogram typings SRI | ◐ NOT VERIFIED | 无 tarball；只验 26 文件/2,552,052 B/5.3.1/MIT/bundle digest |
| F0 API/AST handshake | ⛔ NOT RUN | 安装器只做 `--version` 探针；未发送 `initialize`/验证 AST header |
| production Rust installer | ⛔ BLOCKED | 未改 `crates/wxml-delivery`；Python spike 不可替代 |
| Rust build/test/CI clean-env | ⛔ BLOCKED | 未运行 `cargo`/CI |
| packaged Zed smoke | ⛔ BLOCKED | 无真实包/图形环境 |
| glibc/ABI/musl/aarch64 | ⛔ BLOCKED | 只记录引擎静态 ELF；Rust glibc 未测 |
| power-loss/fsync durability | ⛔ BLOCKED | 只测进程退出；未断电/未证明目录持久性 |
| license/provenance review | ◐ PARTIAL | spike 包内许可与 provenance 已生成并 hash；未做发布评审 |

### 11.8 G1-C 判定与限制

**G1-C 仍不能关闭。** 结论是 **PARTIAL PASS（临时证明）**，不是 PASS：

1. 生产 `wxml-delivery` 的 Rust schema v2 安装器、provisioning、state/journal、legacy 读取器
   仍未实现（B1）。
2. clean-env 只覆盖 Python spike；Rust build/test/release/CI/Zed 未在无 Node 环境运行（B2/B6）。
3. 平台矩阵只测 Linux x86_64；Rust glibc 下限、musl、aarch64、macOS/Windows 未测（B3）。
4. F0 `--api --async`/AST 握手未由安装器执行；`tsc.sig` 仍未验签；miniprogram SRI 未复核
   （B4/B5）。
5. 许可/provenance 只是 spike 打包，未做发布评审（B7）。
6. 无发布者签名（B8，ADR-0007 残留风险）；断电持久性未测（B9）。
7. 本次 Python spike 不是生产代码，不能作为 F3-01～F3-16 的实现证据；这些测试仍是 planned。

可关闭的未知：**“manifest v2 + index + state/journal + 原子 current + checksum/失败/回滚/
legacy 拒绝 + 无 Node 本地路径”在 Linux x64 上可实现**，且已用真实 7.1.0-beta 引擎字节
（SRI/SHA/122 文件/`--version`）验证。该未知从 BLOCKED 降为 PASS（临时），但 G1-C 门禁仍为
OPEN。
