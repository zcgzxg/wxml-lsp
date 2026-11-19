# Rust + tsgo 迁移测试契约（F2）

日期：2026-11-17。状态：draft；P2 评审通过前不是最终门禁。

本契约只定义“如何证明迁移没有回归”，不表示任何用例已经迁移。逐用例的唯一事实源是
[`docs/plans/tsgo-test-ledger.json`](../plans/tsgo-test-ledger.json)，审阅摘要见
[`docs/plans/2026-11-17-tsgo-test-ledger.md`](../plans/2026-11-17-tsgo-test-ledger.md)。
原生引擎与查询契约见 [`tsgo-native-contract.md`](tsgo-native-contract.md)。

## 1. 范围与计数

| 项 | 冻结值 |
| --- | ---: |
| TS 测试文件 | 21 + `support.ts` |
| 静态 `test()` 声明 | 274（273 字面 + 1 动态） |
| 展开注册 | 339 |
| `direct` | 240 |
| `rewrite` | 52 |
| `split` | 11 |
| `obsolete` | 36（Candidate B/symbol-bridge 内部布局） |
| `blocked-decision` | 0 |
| `green` | 44（真实 Rust 测试证据，未独立 review） |
| `planned` | 259 |
| `retired-with-decision` | 36 |

计数在 wave-5 回填（2026-11-20）后为 `green` 44 / `planned` 259；P6 回填（2026-11-19）后为 36/267，P5/P5.1 的历史快照见台账 §14。

`obsolete` 不等于“未迁移的通过项”。36 个 Candidate B 用例只作为历史 A/B 证据保留；生产 LSP
不传 `layoutId`，原位 shadow 用例仍全部强制。`TS-META-0027` 由 F0 引擎/类型资产 pin 测试替代。

## 2. Oracle 与 fixture 冻结

- 测试源码树（`backends/typescript/test`）：`630a16af41623c78f19f47bcaba1a7f1282201ac27b4414115ac0b3764afc32e`
- samples：`00a7fa3d6962da50581c396f73c13f7ffb043efae060a0801b373928726e5c1f`
- `tests/fixtures`：`7c9ddedda18c70bc217af6b3bbd23dd22a33be9c885276cbd587adc0596f04a9`
- `crates/wxml-lsp/tests/fixtures`：`834eea4defcb7565c4b84e91a173079cf3be85201d44fb4a7a6d0a32af3da073`

算法（v1）：regular files only，拒绝 symlink；按 UTF-8 相对路径字节排序；对每个文件累加
`relative_path_utf8 + NUL + raw_bytes + NUL` 后取 SHA-256。任何 fixture 变更必须同时更新台账与 hash，
不得只改 hash 掩盖内容变化。

行尾（M5）：v1 hash 基于 LF checkout 的原始字节；CRLF checkout 会改变 hash。本地和 CI 必须保持 LF
（`git config core.autocrlf false`，仓库 `.gitattributes` 约束），不得通过改 hash 掩盖行尾变化。

## 3. 逐用例记录

每个 case 至少保留：`case_id`、旧文件/标题/行、`body_sha256`、完整展开标题、参数子 ID、断言摘要、
覆盖维度、fixture、目标 Rust 全限定测试 ID、比较类型、RED/GREEN 原始证据、reviewer、原生身份
（engine version/gitHead/AST version/artifact hash）。`status` 只允许：

`planned` → `red` → `green` → `reviewed` → `closed`；或 `planned` → `retired-with-decision`。

不得把 `planned` 批量改成 `green`；不得用“旧测试已删除”代替新测试通过。任何 `obsolete` 必须有
集成负责人裁决与代码证据。

## 4. 差分比较规则

允许归一化：

- 临时根路径与工作区路径前缀的一一对应；
- 请求 ID、进程 ID、时间戳、明确的非语义序号；
- 明确声明为无序集合的引用/诊断/补全列表。

禁止归一化：

- 诊断 code、severity、range、message 的可观察语义；
- hover 的类型字符串、文档、JSDoc、来源链接；
- definition/reference/rename 的目标文件、range、symbol 身份与编辑安全条件；
- completion 的 label、kind、insertText、排序等级；
- 空结果与“查询失败/引擎缺失”的区分。

比较器必须有反向自测：注入缺失结果、range 偏移、文档丢失、越界编辑、双空结果，均必须失败。
双空结果不得自动 PASS。

## 5. 原生执行门禁

- 缺少原生引擎、资产 hash 不符、协议版本不符：测试必须失败，不得 skip/return。
- `semantic_tools` 中缺少 `dist/server.js` 的两个静默 return 必须改为原生强制门禁：
  - `native_component_property_edit_applies_and_reparses`
  - `native_rejects_cross_namespace_and_dynamic_edits`
- 23 个现有 `#[ignore]` 必须逐项映射为必跑原生命令；在最终无 Node 候选构建中不得保持 ignored。
- 性能测试权威门禁是串行 `perf.test.ts`；全量并行结果只作环境记录。阈值 8000 ms 与测试内容不变。
- 原生不可用的平台必须显式失败并给出平台/资产原因，不得回退 Node。

## 6. 测试分区

公共入口：`crates/wxml-lsp/tests/native_parity.rs`。

分区：`hover_docs`、`definitions`、`completion`、`loops`、`guards_js`、`components`、`behaviors`、
`events`、`project_overlay`、`symbols_edits`、`scenario_matrix`、`protocol_limits`、`messages_evidence`、
`shadow_rewrite`、`mapping_utf16`、`perf_budget`。每个文件单一 owner；纯 codec/map/rewrite 单元测试放在
生产模块旁的 `#[cfg(test)]`，不得用 LSP 矩阵代替底层合同测试。

## 7. 必跑矩阵

1. 全 workspace `cargo fmt --check`、`cargo check --workspace --all-features`、
   `cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。
2. 原生 parity 全分区 + 23 个原 ignored 门禁。
3. 坐标矩阵：ASCII/CJK/emoji/BOM/CRLF；API/AST UTF-16、字符串池 UTF-8/WTF-8、孤立 surrogate。
4. codec/协议：畸形长度、越界、循环、未知 kind、超大 frame、快照 v1→v2 迟到、lease/epoch、stale handle。
5. 恢复：子进程崩溃、超时、重启、未保存更新、旧结果丢弃。
6. FS/安全：缺失回调 fail-closed、路径逃逸、symlink、写/emit 拒绝、`--runExternalCode` 禁止。
7. 性能：串行冷启动/热查询/诊断/取消，200 样本，现有 p95 回退 ≤10%，RSS ≤15%。
8. 无 Node：PATH 无 node/npm、进程审计、`--offline`、打包布局、Linux Zed 冒烟。

## 8. 退出条件

F2 冻结需要：原生契约 F0/F1 已评审；测试分区 owner 已指定；比较器反向自测通过；fixture v1 hash
可复现；36 个 `obsolete` 与 1 个 `rewrite` 有裁决记录。G5 通过需要全部 `direct/rewrite/split` 用例
为 `green/reviewed`，没有真实引擎缺失导致的 skip，且 23 个 ignored 与 2 个静默 return 已转为强制门禁。
