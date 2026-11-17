# 证据索引与记录规则

更新日期：2026-11-17。

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

## 3. 当前门禁基线（2026-11-17，W4 最终）

以下是 W4 最终快照，不替代各记录的原始结论。W2、W3、Q1 的计数只作历史，不与 W4 混用。

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
└── data/
    ├── typescript-language-service/
    ├── existing-wxml-lsp-audit/
    ├── official-template-analyzer-audit/
    ├── native-typescript-service/
    ├── type-bridge-c1/
    ├── w3/
    └── zed-standard/
```
