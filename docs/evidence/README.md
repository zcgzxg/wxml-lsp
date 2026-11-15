# 证据索引与记录规则

更新日期：2026-11-16。

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
| 2026-11-16 | [P7 真实集成证据](p7-integration.md) | `wxml-lsp` + Node 24.22.2 | E1 | ProjectHost/NodeBackend/C1.1 路由落地；G1 真实门禁 `stdio_type_loop` 5 passed、`real_node` 1 passed；Wave 4（AC-005/006/008/012）与未保存 JS 同步延期 |
| 2026-11-16 | [P8 Zed 扩展证据](p8-zed.md) | Zed 1.28.1 / zed_extension_api 0.7.0 | E1 | 显式 `WXML_LSP_BIN` 绝对路径、无 worktree/PATH 回退；host 单测 22 项；平台感知路径校验；wasm debug 7,633,946 B / release 238,792 B；隔离用户目录 smoke 注册 WXML、启动真实 LSP、显示 `user.nmae` 诊断；标准安装与 hover/定义可视化未确认 |

## 3. 当前门禁基线（2026-11-16）

以下是当前仓库的快照，不替代各记录的原始结论：

| 项 | 命令 | 当前结果 |
| --- | --- | --- |
| Rust workspace | `cargo test --workspace` | 625 passed / 6 ignored / 26 suites |
| Zed 扩展 | `cd editors/zed && cargo test --locked` | 22 passed（1 suite） |
| Zed wasm (debug) | `cargo build --target wasm32-wasip2 --locked` | 7,633,946 B |
| Zed wasm (release) | `cargo build --target wasm32-wasip2 --release --locked` | 238,792 B |
| G1 真实 Node | `WXML_TEST_NODE=1 cargo test -p wxml-lsp --test stdio_type_loop -- --ignored` | 5 passed |
| G1 真实 Node | `WXML_TEST_NODE=1 cargo test -p wxml-lsp --test real_node -- --ignored` | 1 passed |

6 个 ignored 就是上述 G1 门禁（5 + 1），默认 `cargo test` 不运行；设置
`WXML_TEST_NODE=1` 后仍要求 `backends/typescript/dist/server.js` 已构建。

未计入通过：标准 Zed Dev Extension 安装、真实 grammar 渲染、hover/definition
可视化、Windows/macOS 实机、G0/G2 完整验收。G0/G2 目前只算部分通过。

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
└── data/
    ├── typescript-language-service/
    ├── existing-wxml-lsp-audit/
    ├── official-template-analyzer-audit/
    ├── native-typescript-service/
    └── type-bridge-c1/
```
