# 证据索引与记录规则

更新日期：2026-11-20。

本目录保存支撑设计判断的最小证据。结论、环境、方法、限制和原始数据都在仓库内，不把本机临时目录或外部仓库当作唯一证据来源。

## 1. 证据等级

| 等级 | 含义 | 最低要求 |
| --- | --- | --- |
| E0 | 结论级 | 环境清单、方法摘要、结论和限制；原始运行脚本或输出未保留 |
| E1 | 可复查级 | 原始输出或关键样本已保留，可人工复查；运行器可能依赖未入库的外部程序 |
| E2 | 可复跑级 | 运行脚本、输入样本和原始输出齐全，并写明前置条件和执行方式 |

等级描述的是本仓库保存的材料，不代表实验本身的可信度。任何记录都不能把“当时通过”扩大为“当前已实现”。

## 2. 记录索引

| 日期 | 记录 | 对象 | 等级 | 结论摘要 |
| --- | --- | --- | --- | --- |
| 2026-11-15 | [TypeScript Language Service 机制实验](2026-11-15-typescript-language-service.md) | TypeScript 6.0.2 | E0 | 22 项断言通过，支持 Node 类型后端路线；脚本未保留 |
| 2026-11-15 | [现有 Rust WXML LSP 核验](2026-11-15-existing-wxml-lsp-audit.md) | 0.1.9 | E2 | 基础场景可用，跨文件、联合类型、说明和未保存同步不足 |
| 2026-11-15 | [官方模板分析器核验](2026-11-15-official-template-analyzer-audit.md) | 1.0.0 | E1 | 导出式组件效果完整；裸写法退化为 `any`；未保存导入类型需刷新 |
| 2026-11-15 | [TypeScript 原生语言服务实验](2026-11-15-native-typescript-service.md) | 7.0.2 / 7.1 开发版 | E2 | 虚拟 `file://` 文件可用；自定义 scheme 与显式 `files` 归属存在问题 |
| 2026-11-15 | [类型桥候选验证与 C1 建议](type-bridge-c1.md) | TypeScript 6.0.3 / miniprogram-api-typings 5.3.1 | E2 | 33 场景 × 2 候选：A 26 supported / 3 Partial / 3 Unsupported / 1 Unavailable（208 tests）；B 在闭包、模块组件与 `this` 类型上失败；选项透传、多注册、替换与定义坐标已冻结为 C1 契约 |
| 2026-11-16 | [P7 真实集成证据](p7-integration.md) | `wxml-lsp` + Node 24.22.2 | E1 | ProjectHost/NodeBackend/C1.1 路由落地；G1 真实门禁 `stdio_type_loop` 5 passed、`real_node` 1 passed（当时快照）；Wave 5 已补齐 AC-005/006/008/012 与未保存 JS/TS 同步 |
| 2026-11-16 | [P8 Zed 扩展证据](p8-zed.md) | Zed 1.28.1 / zed_extension_api 0.7.0 | E1 | 显式 `WXML_LSP_BIN` 绝对路径、无 worktree/PATH 回退；host 单测 25 项；平台感知路径校验；wasm debug 7,633,946 B / release 238,792 B；标准 UI `zed: install dev extension` 已实际执行，symlink + `dev=true`，五语言注册；干净 fixture 的 hover/definition/completion/diagnostics 和重启恢复已实测；事件处理函数 hover 与 Ctrl+点击跳转已实测，标签/属性 hover 仍无结果；真实 TS/JS 项目观感已记录；Wave 5 多语言同步见 §12，标准安装见 §13 |
| 2026-11-17 | [W2 集成与 Q1 交付](full-feature-w2.md) | W2 `a441de9` / Q1 `f46dec9` | E1 | W2 固定日志 workspace 1095 passed / 20 ignored；Node 304/305，唯一失败为既有 perf 墙钟断言（高负载超时，单独重跑通过）；IT-01/02/03/05/09/10/11/12 真实 stdio 矩阵通过；Q1 `cargo test -p wxml-delivery` 通过；本地打包/安装/升级/回退存在，下载默认关闭；S/Q2 当时未完成 |
| 2026-11-17 | [W3 语义工具与安全编辑](full-feature-w3.md) | W3 `379b097` / S `dd15bc1` | E1 | S 语义工具合入：references/rename/codeAction/semanticTokens/workspaceSymbol/inlayHint/signatureHelp/codeLens；W3 固定日志 workspace 1251 passed / 0 failed / 21 ignored（45 suites），Node 串行 326/326，`semantic_tools` 41 passed；两轮正式 Rust/安全审核均 CHANGES_REQUESTED，残余项由 `dd15bc1` 修复并由集成探针复验，未开第 3 轮；Q2 当时未完成 |
| 2026-11-17 | [W4 最终验收与发布](full-feature-final.md) | W4 `9b804d4` / Q2 `95ebb21` | E1 | W4 Rust 1283 passed / 0 failed / 23 ignored（49 suites）；Node 串行 333/333；ignored 真实 Node/stdio/wave5/stability/performance 全部通过；性能/稳定性阈值 `pass=true`；覆盖率 lines 87.24% / functions 88.85% / regions 86.71%；cargo audit 0 vulnerabilities、deny advisories ok；Zed workspace 25 passed；最终 release 两次构建 SHA-256 `c09062b16dd6bfea4ee03f99f13665cf4858130aae83eefec9c04cd94d3815ab`，214 files，install/verify/verify-package 通过；Linux Zed 运行时冒烟通过；Q2 R2 Rust/安全 APPROVE；无 marketplace/远端发布/发布者签名 |
| 2026-11-17 | [tsgo 迁移 workspace 基线](2026-11-17-tsgo-workspace-baseline.md) | `tsgo/migration` `b9616c5` | E1 | 记录 worktree、工具链、Backend 边界、单写者与 P2/P3 阻塞；无生产代码改动 |
| 2026-11-17 | [tsgo 迁移测试基线](2026-11-17-tsgo-migration-baseline.md) | Rust workspace / TS 339 用例 | E1 | Rust 1297 pass/0 fail/23 ignored；TS 338/339，唯一失败为全量并行 perf 8325.25ms；isolated 串行 3/3 绿；台账 274 静态/339 展开 |
| 2026-11-17 | [tsgo 原生 API 能力验证](2026-11-17-tsgo-native-api-spike.md) | 7.1.0-beta Linux x64 | E1 | 无 Node `--api --async`、虚拟快照、诊断/类型/符号/文档/补全/引用可用；AST v9 部分解码；无 QuickInfo/Definition/Rename 直接 API；7.0.2 排除；脚本仅在临时目录 |
| 2026-11-17 | [tsgo 无 Node 交付验证](2026-11-17-tsgo-delivery-spike.md) | 7.1.0-beta / miniprogram-api-typings 5.3.1 | E0 | 资产身份、SRI、无 Node 入口与安装/回滚状态机设计；未下载安装，安装器与 clean-env 验收仍待执行 |
| 2026-11-18 | [P6-3 原生类型资产实施计划](2026-11-18-tsgo-native-type-assets-plan.md) | `b0a108f` / 7.1.0-beta | E0 | 只读审计：full VFS 必须挂载 116 个 stdlib 与 20 个 typings `.d.ts`；真实引擎探针可解析 `wx`/`Page`/`Component`/`getApp`；P6-3 未完成，AC-004 旧诊断、TS 6.0.3/7.1.0-beta 偏差和真实资产布局仍阻塞 |
| 2026-11-18 | [P6-3 LSP adapter 类型资产接线计划](2026-11-18-tsgo-adapter-type-assets-plan.md) | `4ad3459` | E0 | 只读审计：backend 已具备 `AnalysisInput::with_environment`，adapter 仍调用 `AnalysisInput::new`，没有 loader/compilerOptions/jsCheck 接线；给出有序接线与回滚步骤 |
| 2026-11-18 | [P6-4 原生引擎发现与可启动性计划](2026-11-18-tsgo-native-engine-discovery-plan.md) | `558beb9` | E0 | P6-4a 可实施、P6-4b/c BLOCKED；默认仍是 Node；本文不声称原生后端可启动 |
| 2026-11-18 | [F1 completion auto-import/逐项文档计划](2026-11-18-tsgo-completion-autoimport-plan.md) | `a17794a` / `5541ee0` | E0 | auto-import 未实现；逐项文档被固定 API 阻塞；F1-21/B-07 仍开放，非空补全仍为 `Partial` |
| 2026-11-18 | [F3-03 资产锁与 provisioning 设计](2026-11-18-tsgo-f3-asset-lock-plan.md) | `5541ee0` / `f7b9ef7` | E0 | 仓库无 `assets.lock.json`；当时 26/23 typings 口径冲突（后由 `6358925` 固定为 payload 23）；F3-03 仍 BLOCKED |
| 2026-11-18 | [F3-02 干净环境/进程审计](2026-11-18-tsgo-f3-clean-env.md) | `8490a92` | E1 | 真实资产安装、干净 PATH、网络/挂载隔离和 fixture 升级回滚 PASS；`execve/execveat` 审计与原生 stdio LSP 会话 BLOCKED，F3-02 仍 BLOCKED |
| 2026-11-18 | [P5 安全审计](2026-11-18-tsgo-p5-security.md) | `d6e8f41` | E1 | 方法白名单、VFS 读边界、进程清理和 AST/句柄预算 PASS；运行目录最小权限第二层与 `--runExternalCode` 全验证 BLOCKED；license 门禁当时 BLOCKED（后续根 `deny.toml` 已补，P5 其余项仍 BLOCKED）；G5 未通过 |
| 2026-11-18 | [P6/P7 默认切换与去 Node 审计](2026-11-18-tsgo-p6-cutover-audit.md) | `69e13fb` | E0 | 默认 `--stdio` 仍构造 `NodeBackend`；记录 8 个硬阻塞；该审计时 P6/P7 未开始，不删除 Node |
| 2026-11-18 | [P7 mandatory gates triage](2026-11-18-tsgo-p7-gates-triage.md) | `8490a92` | E1 | 23 个 `#[ignore]` 在 Node 24 下单跑全通过；0 enable-now / 11 keep-gated / 12 delete-at-P7；`semantic_tools` 2 个静默 return 与可选 `tsc` 分支复现 |
| 2026-11-18 | [Native document symbols DTO 计划](2026-11-18-tsgo-document-symbols-dto-plan.md) | `497380c` | E0 | 推荐独立 `Backend::document_symbols`（Option B）；DTO/decoder 未接 adapter/LSP，脚本路由与 reverse lookup 仍是前置；F1 未关闭 |
| 2026-11-18 | [tsgo typings 26 vs 23 裁决](2026-11-18-tsgo-typings-26-23-decision.md) | `f6f175f` | E1 | 交付 payload 定为 23 文件 / 2,548,075 B / `ba3eb83…`，上游身份保留 26 文件；lock 与真实 tarball 未冻结，F3-03 仍 BLOCKED |
| 2026-11-18/19 | [P6-2 mandatory-gate 转换计划](2026-11-18-tsgo-p6-gate-conversion-plan.md) | `f6f175f` | E0 | 0/23 gate 绿；3 enable-now / 8 keep-gated / 12 delete-at-P7；2 个静默 return + 可选 `tsc` 转为 3 个强制原生 gate；S0–S9 与 no-false-green 规则 |
| 2026-11-19 | [tsgo native hover 与首波 native gate 证据](2026-11-19-tsgo-hover-gates.md) | `953ee1e` / 7.1.0-beta | E0 | `wxml-lsp` lib 668 passed（执行会话记录，未重跑）；真实引擎 adapter hover 1 passed；首波 3 个 native gate 3 passed / 36 filtered（G-P5-E2E-09/12/14）；ledger 仍 `required_not_migrated`；默认仍 Node；记录时 workspace clippy 被并行 loader 的 `unreadable_literal` 阻塞（后续 `ca0b554` 未重跑，不声称已绿） |
| 2026-11-19 | [tsgo native gate 台账提升](2026-11-19-tsgo-native-gate-status.md) | `7bdab96` / 证据 `953ee1e` / 7.1.0-beta | E0 | P6-2 台账 3 `gate_green`（`G-P5-E2E-09/12/14`）/ 20 `required_not_migrated` / 23 total；green 带命令、pinned 引擎/资产身份与 3 passed 证据；3 个仍 `#[ignore]`，默认 CI 不跑；本次复跑被并行 type-assets WIP 阻塞，green 仍锚定 `953ee1e`；P6/G5/G6/F3 未关闭 |
| 2026-11-19 | [F1 event-handler completion 实施计划](2026-11-19-tsgo-event-handler-completion-plan.md) | `bff58d1` / S1 `939831b` | E0 | 设计 only：保留宽松直接事件形式，避免 `bindgetphonenumber` 等回归；缓存键排除 `view_id`；S1 顶层注册选择已提交，S2+ 尚未实现；无 native checker/auto-import/自定义事件；F1/P6/G5 未关闭 |
| 2026-11-19 | [tsgo 测试台账回填与 native gate 复核](2026-11-19-tsgo-ledger-backfill.md) | `9a8b65c` / 7.1.0-beta | E0 | case 级 36 `green` / 267 `planned` / 36 retired（新增 `TS-COMPLETION-0175/0193`、`TS-PROBES-0169`）；`TS-META-0009`、`TS-SERVER-0237` 为 weaker；document symbols 真实引擎 7 passed；3 个既有 `gate_green` 复跑 3 passed / 36 filtered；20 gate 仍 required，无新增 gate；默认 Node/CI/mandatory 未变 |
| 2026-11-19 | [F3 真实 release 验收](2026-11-19-tsgo-f3-real-release.md) | `561b00a` / 7.1.0-beta / 真实 Rust + Zed 产物 | E1 | 真实 lock/tarball 的两次打包逐字节一致（157 文件）、真实包空安装/verify/current/env、N1→N2→N1、离线回滚、recover、archive 篡改拒绝、readelf/ldd/GLIBC_2.39、平台拒绝和进程级 Zed 启动；F3-02/14 blocked；F3-12 只覆盖缓存 install |
| 2026-11-19 | [F3 剩余验收矩阵](2026-11-19-tsgo-f3-acceptance.md) | `0b3b3da` / 真实包 + 受控变异 | E1 | 6 个 opt-in 测试：F3-05 缺资产/探针、F3-10 六类篡改、F3-11 五种恶意归档经真实 `--from-dir`、F3-08/09/15 legacy、F3-06 四窗口 seam、F3-16 journal 注入/并发；F3-06/09 进程级 kill 仍 blocked |
| 2026-11-20 | [F3-02 ptrace exec 审计](2026-11-20-tsgo-f3-exec-audit.md) | `f2952c2` / 7.1.0-beta | E1 | test-only ptrace 记录真实 installer 10 exec、native LSP 3 exec；无 Node 家族/server.js/legacy 变量，全绝对路径；F3-02 closed（Linux x86_64），非 Linux/无 ptrace 权限显式 BLOCKED |
| 2026-11-20 | [P6-2 原生性能与稳定性门禁](2026-11-20-tsgo-p6-perf-stability.md) | `8b6ca14` / 7.1.0-beta release | E1 | 真实 stdio：hover p95 0.822ms、completion 0.648ms、diagnostics 208.075ms/500（含 120ms debounce）、cold start 60.1ms、RSS +11.6%；12-cycle RSS +6.373%；gate 18/23 |
| 2026-11-20 | [E2E-20 组件属性绑定诊断设计](2026-11-20-tsgo-e2e20-component-binding-plan.md) | `4140e9b` / 7.1.0-beta | E0 | `PreparedView`/`ScriptAnalysis` 已有子组件属性类型；需新增 `ShadowLayout` 表达式范围 + `type_query` 的绑定检查，复杂类型 Partial，事件属性排除；尚未实现 |
| 2026-11-20 | [P6 默认切换与 P7 删除设计](2026-11-20-tsgo-p6-default-switch-plan.md) | `4140e9b` / 7.1.0-beta | E0 | 默认 native + HostOnly + 显式 `--backend node`；P7 仍被 5 个 required gate、2 个 silent return、267 planned、旧 TS fixture 与 v1 build 阻塞 |
| 2026-11-20 | [F3-06/09 进程级 kill 窗口设计](2026-11-20-tsgo-f3-kill-window-plan.md) | `4140e9b` / 7.1.0-beta | E0 | 建议 test-only ptrace 在 rename/symlink/fsync 暂停后 SIGKILL；仍需真实 N2、真实 legacy release，contract §6.3 崩溃语义需裁决 |

## 3. 当前门禁基线（2026-11-17，W4 最终）

以下是 W4 最终快照，不替代各记录的原始结论。W2、W3、Q1 的计数只作历史，不与 W4 混用。Rust + tsgo 迁移的 P0/P1 基线见 §3.1，P6 实施状态见 §3.2；该迁移尚未完成，不能把本节 W4 的 Node 结果当作原生后端结果。

| 项 | 命令 | 当前结果 |
| --- | --- | --- |
| Rust workspace（W4 最终） | `cargo test --workspace -- --test-threads=4` | 1283 passed / 0 failed / 23 ignored（49 suites，含 doctests） |
| ignored 门禁（W4 最终） | `WXML_TEST_NODE=1 cargo test ... -- --ignored` | 23/23 passed：performance 1、q2_stability 1（strict 10 轮 + RSS）、real_node 5、stdio 9、wave5 7 |
| Node 后端串行（W4 最终） | `cd backends/typescript && node --test --test-concurrency=1 test/*.test.ts` | 333 / 333 |
| Node 矩阵/性能 CLI（W4 最终） | `npm run matrix:json`；`npm run perf:json` | 通过（66 个 matrix run） |
| 覆盖率（W4 最终） | `cargo llvm-cov ...` | lines 87.24% / functions 88.85% / regions 86.71%；3 个纯墙钟断言为插桩跳过，阈值未改 |
| 安全（W4 最终） | `cargo audit`；`cargo deny check advisories` | 0 vulnerabilities；advisories ok；无 `deny.toml`，licenses 未评估 |
| Zed 扩展 workspace（W4 最终） | fmt / test / clippy / WASI build | 25 passed；其余通过 |
| 最终 release（W4 最终） | `scripts/build-release.sh --output <repo 外目录>` 两次 | SHA-256 `c09062b16dd6bfea4ee03f99f13665cf4858130aae83eefec9c04cd94d3815ab`；8,696,984 B；protocol 2；214 files |
| 安装与验证（W4 最终） | `wxml-lsp-install install/verify`；`verify-package` | 通过；安装根 `/tmp/opencode/final-wxml-root3` |
| Linux Zed 运行时冒烟（W4 最终） | 隔离 Xvfb + 手工安装发布包 | 通过；不是标准 UI 重装，见 [W4 证据](full-feature-final.md) §6 |

历史计数（不与 W4 混合）：

| 记录 | 命令 | 结果 |
| --- | --- | --- |
| W3 Rust | `cargo test --workspace -- --test-threads=4` | 1251 passed / 0 failed / 21 ignored（45 suites） |
| W3 Node | `node --test --test-concurrency=1 test/*.test.ts` | 326 / 326 |
| W2 Rust / Node / Zed | `cargo test --workspace`；`npm test`；`cargo test --locked` | 1095 / 20 ignored；304 / 305（唯一失败为墙钟）；25 passed |
| Q1 本地交付 | `cargo test -p wxml-delivery` | 通过 |

W4 的 23 个 ignored 是：`performance_bench` 1、`q2_stability` 1、`real_node` 5、
`stdio_type_loop` 9、`wave5_acceptance` 7；全部在 `test-logs/final-rust-ignored.log` 中通过。
性能/稳定性 JSON 的 `git_head` 是 `5990126`（clean）；之后 `7433194`/`9b804d4` 只改 Node
发布入口/模块解析，不改调度/恢复或性能路径。最终 release 在这两个修复后重建，并做了 Linux
Zed 运行时冒烟。

未计入通过：Windows/macOS 实机、真实主桌面 Wayland 观感、标准 UI `zed: install dev
extension` 在最终版本上的重跑、大项目中/大语料性能、完整 G0/G2 矩阵、marketplace/远端
release/发布者签名。W4 的 Linux Zed 冒烟是手工安装最终发布包的运行时验证，不冒充标准 UI
安装；标准安装历史证据仍见 [p8-zed.md](p8-zed.md) 第 13 节。

## 3.1 Rust + tsgo 迁移 P0/P1（2026-11-17）

| 项 | 命令/范围 | 结果 |
| --- | --- | --- |
| Rust 基线 | `cargo test --workspace --all-features` | 1297 passed / 0 failed / 23 ignored（36.5s） |
| Rust 静态质量 | fmt / check / clippy `-D warnings` | 通过 |
| TS 基线 | 全量 339 展开用例 | 338 passed / 1 failed（perf 并行冷启动 8325.25ms） |
| TS perf 隔离 | `node --test test/perf.test.ts` 串行 3 次 | 3/3 通过（1058.25 / 1005.64 / 675.60ms） |
| 迁移台账 | 21 个测试文件 + support | 274 静态 / 339 展开；direct 240、rewrite 52、split 11、obsolete 36、blocked 0 |
| 原生能力 | 7.1.0-beta `--api --async` | 有条件通过；AST v9 部分解码；完整 shadow/查询组合待后续 gate |
| 无 Node 交付 | 资产/安装设计 | PARTIAL；未安装、未 clean-env 验收 |

G1 尚未关闭；7.1.0-beta 的接受与 integrity 固定、可执行安装器和 F0–F3 契约冻结是 P3 前置。

## 3.2 Rust + tsgo 迁移 P6 实施状态（2026-11-19，HEAD `6358925`）

本节记录已提交切片的当前状态，不替代各证据记录，也不把库级测试当作真实资产、CLI 或 Zed 验收。工作树中存在未提交的 P6-3 adapter/document-symbol 接线 WIP（不属于 HEAD `6358925`）；这些改动不是本节事实，也不作为通过证据。

| 提交 | 切片 | 已提交内容 | 当前可证明 | 仍未关闭 |
| --- | --- | --- | --- | --- |
| `2c6d39e` | P6-3 后端类型资产 | `AnalysisInput::with_environment`、33 个 compiler option key、`TypeAssets`/`TypeAssetsPin`、full VFS 资产表、116 stdlib + 20 typings `.d.ts`、environment tsconfig；`crates/wxml-backend-tsgo/tests/type_assets.rs` 10 个测试 | 库/单元与真实引擎层：`type_assets` 真实门禁记录为 10/10 passed（其中 2 个为 opt-in 真实引擎用例；本轮未重跑）。可解析 `wx`/`Page`/`Component`/`getApp`，缺资产 fail-closed | adapter 仍调用 `AnalysisInput::new`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:121 ），`js_check` 仍为 `Inherit`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:202 ）；可信 pin loader、F3-03、AC-004 未保存更新旧诊断、TS 6.0.3 vs 7.1.0-beta 偏差、adapter 降级均未关闭 |
| `abf0046` | P6-4a 引擎发现/沙箱/opt-in | `engine.rs` 从安装树发现并校验 `manifest.json`/`engine/lib/tsc`；`sandbox.rs` 每 session 新建 `0700` 沙箱；`wxml-lsp --stdio --backend native`；默认 `--stdio` 仍为 `NodeBackend` | CLI/库层：`--backend native` 失败闭合，未验证包时不回退 Node；默认路径未改变 | fd/`openat` 沙箱加固仍 BLOCKED，存在路径校验到 spawn 的残余 TOCTOU；P6-4b 真实资产启动、P6-4c 真实包/无 Node 进程审计、默认切换均未完成 |
| `31d6d29` | F1 hover 证据与 Markdown 组合 | 三段式 Markdown 顺序、checked-empty/`Unsupported`/`Partial` 区分、声明范围双坐标；`crates/wxml-backend-tsgo/tests/f1_hover.rs` 11 个测试 | 库/单元层：9 个纯组合测试；2 个真实引擎测试仅在 `TSGO_BIN` 可用时运行，`WXML_REQUIRE_REAL_ENGINE=1` 才转为硬门禁 | 无 `getQuickInfoAtPosition`；QuickInfo display parts、逐 overload 文档/分段仍 BLOCKED；组件缺失等降级仍需 adapter 合同 |
| `6eb6f26` | Native document symbols DTO | `wxml-types` 新增 `DocumentSymbol*` DTO 与默认 `Backend::document_symbols`；`wxml-backend-tsgo` 解码 v9 document symbols，保留 UTF-16/UTF-8 范围与 gap/truncation | 库/单元层：DTO 默认 `Unsupported`、decoder 与映射测试；不会把空结果伪装成 supported | adapter 与 LSP 路由未接线；脚本文档的 view/reverse lookup、snapshot 身份与 `DocumentRoute::Script` 仍是前置；`structure.rs` 的 WXML outline 不变 |
| `6358925` | typings payload 身份强制 | 23 文件 allowlist、`2,548,075 B`、bundle 摘要 `ba3eb83…`、精确 payload 路径、入库的 5.3.1 fixture；manifest/verify/打包器重算并比较 | 交付库/单元层：hermetic fixture 下的打包/清单/安装树负向测试；`typings_payload.rs` 仍是对真实 5.3.1 树的 opt-in 一致性检查 | 真实上游 tarball 的 SRI/SHA-256/size、provisioning、`assets.lock.json`、clean-env/ABI、包级验收未完成；**F3-03/B6 仍 BLOCKED**，不关闭任何 F3 ID |
| `0e7c672` | P6-2 gate 转换计划 | 23 个 `#[ignore]` 的逐项处置、3 个 enable-now/8 个 keep-gated/12 个 delete-at-P7、2 个静默 return + 可选 `tsc` → 3 个强制原生 gate、S0–S9 | 计划层：gate 状态与 case 状态分离，定义 `required_not_migrated → replacement_authored → replacement_red → gate_green → mandatory_green → replacement_reviewed → old_test_removed → closed` | 0/23 gate 绿；23 ignored + 2 个静默 return（另有可选 `tsc` 分支）仍是 gate debt；未删除旧测试、未改 `#[ignore]`、未切换默认后端 |

当前门禁口径：

> 本节是 2026-11-19 `6358925` 快照；F3 的 2026-11-20 审计状态见 §3.3。

| 项 | 状态 |
| --- | --- |
| F3-01～F3-16 | 13 closed / 2 partial / 1 blocked（2026-11-20 §3.3）；F3-14 BLOCKED，F3-06/09 partial；root lock/package cache 已落（`assets.lock.json`）；F3 整体验收仍未全部关闭 |
| P6-3 | 部分：backend 数据层与 adapter trusted type-assets handoff 已接线（`caf045d`）；P6-3 整体未关闭 |
| P6-4 | P6-4a opt-in 切片已落；P6-4b/c BLOCKED |
| G5 / G6 | 均未关闭；G7 不在当前范围 |
| 默认后端 | 仍为 Node；`--backend native` 只是 opt-in |
| ignored / silent | 26 个 `#[ignore]`（23 个旧 gate + 3 个 native replacement）；`semantic_tools` 2 个静默 return；可选 `tsc` 分支仍在 |
| native gate 台账 | 3 `gate_green`（`G-P5-E2E-09/12/14`）/ 20 `required_not_migrated` / 23 total；3 个仍 `#[ignore]`，只在 opt-in native job 绿，不是默认 CI 绿，也不是 `mandatory_green` |
| 迁移台账 | 36 `green` / 267 `planned` / 36 `retired-with-decision`；P6 回填新增 3 个 case 级 `green`（`TS-COMPLETION-0175/0193`、`TS-PROBES-0169`），仍无 `reviewed`/`closed` |
| 平台 | 仅 Linux x86_64 有证据；aarch64/macOS/Windows/musl 未验证 |

证据层级说明：

- **库/单元**：`type_assets`、`f1_hover` 纯测试、document-symbol DTO/decoder 测试只证明 Rust 数据层与映射，不证明 LSP adapter、CLI 或编辑器行为。
- **真实资产/CLI**：F3-01/02 的安装/干净 PATH/网络隔离与 ptrace exec 审计是真实资产级证据（`f2952c2`，Linux x86_64）；其他交付测试多为入库 fixture。
- **Zed/UI**：原生后端没有 Zed UI/查询冒烟证据；不得引用历史 Node/W4 的 Zed 结果代替。

## 3.3 F3 逐项验收状态（2026-11-20 审计 + ptrace 更新，HEAD `f2952c2`）

本节只记录 2026-11-20 对 F3 证据的审计结论，权威表和关闭条件在
` docs/reference/tsgo-delivery-contract.md` §9.5。证据运行锚点为 `561b00a`（real release）、
`0b3b3da`（acceptance）、`50e4a21`（partial gaps）和 `f2952c2`（ptrace exec audit）；
`crates/wxml-delivery/src` 在 `561b00a..f2952c2` 无变化。包内 `bin/wxml-lsp` 是 2026-11-19
23:21 从含并发未提交改动的工作树构建，不是任一 HEAD 的逐字节重建；F3-03/04 只声明包/安装
事实，不声明该二进制可从 HEAD 复现。

| 结果 | ID |
| --- | --- |
| **closed（13）** | F3-01、F3-02、F3-03、F3-04、F3-05、F3-07、F3-08、F3-10、F3-11、F3-12、F3-13、F3-15、F3-16 |
| **partial（2）** | F3-06（N2 版本重标；进程级 kill 无生产 seam）、F3-09（精确 SIGKILL 无 seam；真实 legacy 包缺） |
| **blocked（1）** | F3-14（默认 `--stdio` 仍 Node；无 Zed UI/查询证据） |

F3-05/10 是“真实包 + 单点受控变异”，F3-08/09/15 的 legacy 包是生产 v1 packager 生成的
fixture，F3-06/09 的中断窗口是库级 `InstallHooks` seam，F3-16 的 journal 由测试注入；这些
性质已在合同 §9.5 逐项标注，不作为真实发布缺陷或进程级中断证据。F3-11/12/15/16 的关闭依据
是 `50e4a21` 的真实 installer 归档形态、缓存 upgrade/rollback、Zed 变量移除与 activated
幂等测试；F3-02 的关闭依据是 `f2952c2` 的真实 installer/native LSP ptrace exec 审计，
Linux x86_64 以外平台仍不声明。F3-14 不因这些测试关闭。

## 4. 维护规则

1. 每条记录至少包含：日期、对象与版本、环境、方法、结果、限制、对项目决策的影响、数据位置。
2. 结论必须能从保存的数据复核。没有原始材料的结论标为 E0，并明确缺失内容。
3. 原始数据中的本机绝对路径替换为 `<probe-root>` 等占位符；不保留用户名、临时目录或业务项目路径。
4. 不复制被核验对象的完整仓库、构建产物或依赖目录。只保留复现结论所需的脚本、样本、输出和版本清单。
5. 外部测试通过只说明该对象当时的状态，不能代替本项目的验收矩阵。
6. 新实验优先在仓库外运行；只有结论和最小数据进入本目录。禁止把临时目录作为链接目标或唯一证据。
7. 记录是历史快照。对象升级或结论变化时新增记录，不静默改写旧结果；确需更正时注明日期和原因。

## 5. 数据目录

```text
docs/evidence/
├── README.md
├── 2026-11-15-typescript-language-service.md
├── 2026-11-15-existing-wxml-lsp-audit.md
├── 2026-11-15-official-template-analyzer-audit.md
├── 2026-11-15-native-typescript-service.md
├── type-bridge-c1.md
├── p7-integration.md
├── p8-zed.md
├── full-feature-w2.md
├── full-feature-w3.md
├── full-feature-final.md
├── 2026-11-17-tsgo-*.md          # 迁移 workspace/测试基线、原生 API、交付 spike
├── 2026-11-18-tsgo-*.md          # P5/P6/P7 计划、F3 资产/clean-env、hover/symbols/typings
└── data/
    ├── typescript-language-service/
    ├── existing-wxml-lsp-audit/
    ├── official-template-analyzer-audit/
    ├── native-typescript-service/
    ├── type-bridge-c1/
    ├── w3/
    └── zed-standard/
```
