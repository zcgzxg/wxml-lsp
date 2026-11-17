# W4 最终验收与发布证据

更新日期：2026-11-17。

本记录保存 W4 在集成分支上的最终验收快照，覆盖 Q2 调度/恢复、最终发布包和 Linux Zed 运行时
冒烟。它是 W2、W3 之后的最新状态，但不改写这些历史记录中的计数和结论；不同运行的计数不混用。
本记录不把“最终 release 已构建”扩大为 marketplace 发布、远端 release 或发布者签名完成。

## 1. 基线

| 项 | 值 |
| --- | --- |
| 分支 | `feat/full-integration` |
| W4 最终代码 HEAD | `9b804d4`（`fix: resolve Node backend modules in packaged layout`） |
| Q2 合并提交 | `95ebb21`（`merge: integrate performance and recovery hardening`） |
| Q2 审核对象 | `1909e62`；集成侧测试脚手架 `42a82ba`；性能噪声底 `5990126` |
| 发布运行时修复 | `7433194`（symlink 安装入口）、`9b804d4`（打包布局 module 解析） |
| W3 文档 | `d7d1073`（`docs: record W3 semantic tools and safe edits`） |
| Q1 基线 | `f46dec9`，本记录不改变其结论 |
| 环境 | Linux x86_64；Rust/Cargo 1.100.0；Node 24.22.2；npm 11.19.1；AMD Ryzen AI 9 HX 370 / 24 核 / 30 GiB；Debian sid，kernel 7.2.15 |

W3 时，集成分支已包含 P、E、C、N、T、W2、Q1 和 S。W4 合入 Q2，随后做最终验收、发布包
重建和 Linux 运行时冒烟；没有新增 marketplace、远端 release/tag 或发布者签名。

主工作区 `tests/fixtures/page-basic/pages/index/index.wxml` 的 SHA-256 仍为
`8e1e5bfbef41c79ae7c11ff426eb3b7d7d6de3bc426ee84359a65d92a19b0b26`，未修改。该文件在
主工作区有未提交改动，本记录不把它复制或回滚到集成工作区。

## 2. 本轮变更

### 2.1 Q2 调度与恢复

- 诊断 debounce 默认 120 ms，可通过 `initializationOptions.wxml.diagnostics.debounceMs`
  设置（0–5000 ms）；打开、保存和配置变更绕过 debounce。
- 单后台 worker，两类有界队列：交互请求上限 128，诊断队列上限 8；每连续 8 个交互任务后
  至少让一个诊断任务前进，避免连续 hover 饿死诊断。
- 同一文档的诊断按版本合并；快速输入只保留最新版本，旧 revision 的结果不发布。
- `$/cancelRequest` 对排队中的请求立即响应；已开始的同步 TypeScript 调用不可中断，只能等待
  后端返回或超时，运行中占用单独测量。
- 后端 timeout、crash、协议版本不匹配时降级并报告，静态能力保持可用；恢复后重新建立请求。
- 根移除/重新加入时重放已打开的 WXML snapshot 和 script/JSON/style overlay，重新解析 FileId；
  资源删除/恢复、页面从 `app.json` 移除/恢复、关闭/重开文档都有恢复测试。
- Node 项目池 key 使用递归稳定序列化，保留嵌套 `paths`，不同 alias 不错误复用项目。
- 诊断重试最多 8 次；队列满时明确拒绝，不无限增长。

### 2.2 发布运行时修复

`7433194` 让 Node 后端通过 `realpath` 识别 symlink 安装入口；`9b804d4` 让打包布局从
`bin/wxml-type-backend` 正确解析 `node_modules/typescript`。两处都只改 Node 发布入口/模块
解析和对应测试，不改调度、恢复或性能测量路径。

## 3. 评审状态

| 记录 | 对象 | 结论 |
| --- | --- | --- |
| `reviews/full-semantic-tools-r1.md` | S `411e4c1` | `CHANGES_REQUESTED`（8 BLOCKING / 9 MAJOR / 13 MINOR） |
| `reviews/full-semantic-tools-r2.md` | S `d08844f` | `CHANGES_REQUESTED`（最终正式轮；3 BLOCKING / 3 MAJOR） |
| `reviews/full-semantic-tools-security-r1.md` | S `411e4c1` | `CHANGES_REQUESTED`（5 HIGH） |
| `reviews/full-semantic-tools-security-r2.md` | S `d08844f` | `CHANGES_REQUESTED`（1 HIGH / 1 MEDIUM） |
| `reviews/full-performance-recovery-r1.md` | Q2 `31c37ed` | `CHANGES_REQUESTED`（4 BLOCKING/HIGH、2 MAJOR） |
| `reviews/full-performance-recovery-r2.md` | Q2 `1909e62` | **APPROVE（代码与验收行为）** |
| `reviews/full-performance-recovery-security-r1.md` | Q2 `31c37ed` | `CHANGES_REQUESTED`（1 HIGH、3 MEDIUM） |
| `reviews/full-performance-recovery-security-r2.md` | Q2 `1909e62` | **APPROVE（安全）** |

S 的第 2 轮 `CHANGES_REQUESTED` 残余项由 `dd15bc1` 修复，并由集成侧探针复验；按用户设定的
审核轮次上限，没有开启第 3 轮正式审核，因此 S 仍不标记为 `APPROVED`。Q2 的 R2 Rust 与安全
均为 `APPROVE`；R2 提出的 2 个证据/测试脚手架 MAJOR 已在最终门禁前处理
（`42a82ba`/`5990126` 和最终 manifest），没有开启第 3 轮正式审核。

## 4. 最终测试

### 4.1 Rust

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 格式 | `cargo fmt --all -- --check` | 通过 |
| 检查 | `cargo check --workspace --all-targets` | 通过 |
| Clippy | `cargo clippy --workspace --all-targets -- -D warnings` | 通过 |
| Workspace 测试 | `cargo test --workspace -- --test-threads=4` | `1283 passed / 0 failed / 23 ignored`（49 suites，含 doctests） |
| 文档测试 | `cargo test --workspace --doc` | 通过 |
| Rustdoc | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | 通过 |

23 个 ignored 的最终门禁全部通过（`test-logs/final-rust-ignored.log`）：

| 门禁 | 结果 |
| --- | --- |
| `performance_bench` | 1 passed（214.54 s） |
| `q2_stability` strict 10 轮 + RSS baseline | 1 passed（20.81 s） |
| `real_node` | 5 passed |
| `stdio_type_loop` | 9 passed |
| `wave5_acceptance` | 7 passed |

### 4.2 Node 后端

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 安装/检查/构建 | `npm ci && npm run check && npm run build` | 通过；`npm ci` 0 vulnerabilities |
| 全量串行测试 | `node --test --test-concurrency=1 test/*.test.ts` | `333 passed / 0 failed` |
| 矩阵 | `npm run matrix:json` | 通过（66 个 run） |
| 性能 CLI | `npm run perf:json` | 通过 |
| 发布相关测试 | `node --test test/server.test.ts` | 9 passed；覆盖 source/dist/packaged backend dir 与 symlink entry |

### 4.3 性能与稳定性

性能最终 JSON（`perf/final-rust-perf.json`）：

| 指标 | 最终值 |
| --- | --- |
| static completion p95 | 0.178 ms |
| semantic completion p95 | 39.659 ms |
| diagnostics stop p95 | 164.894 ms |
| queued cancel p95 | 0.674 ms |
| single TS call occupancy p95 | 402.126 ms |
| parent RSS | 10,460 KiB |

阈值 `thresholds.pass=true`。稳定性 `q2_stability` strict 10 轮通过，`thresholds.pass=true`，
1 个 Node 进程 / 7 线程；Node RSS 比 1.0309，Rust RSS 比 0.9928，均在阈值内
（`test-logs/final-stability.json`）。

性能与稳定性 JSON 记录的 `git_head` 是 `5990126`（clean）；之后只有 `7433194`/`9b804d4`
两个 Node 发布运行时修复，没有改调度/恢复路径或性能测试。因此这些数字绑定 `5990126` 的
调度/恢复代码，最终发布包在 `9b804d4` 重建并冒烟。该 benchmark 仍使用单个小 fixture
（1 页 / 1 script），不是大项目语料。

### 4.4 覆盖率

`coverage/final-summary.json`：

| 指标 | 覆盖率 |
| --- | --- |
| lines | 87.24%（41,808 / 47,924） |
| functions | 88.85%（4,081 / 4,593） |
| regions | 86.71%（48,286 / 55,685） |

为插桩跳过 3 个纯墙钟断言：`semantic_tools::loop_rename...`、
`discovery::caps_total...`、`component_script_budget_is_bounded`。覆盖阈值未改。

### 4.5 安全与依赖

- `cargo audit`：0 vulnerabilities。
- `cargo deny check advisories`：advisories ok。
- 仓库没有 `deny.toml`；licenses 未评估，不写成 licenses 通过。

### 4.6 Zed 扩展（独立 workspace）

`cargo fmt --all -- --check`、`cargo test --locked`（25 passed）、
`cargo clippy --all-targets --locked -- -D warnings` 和 WASI release 构建通过。

## 5. 最终发布包

最终发布包在 `7433194`/`9b804d4` 两个 Node 运行时修复后重建两次（`a3`/`b3`），归档 SHA-256
完全一致：

| 项 | 值 |
| --- | --- |
| 版本 | `0.1.0` |
| 归档 SHA-256 | `c09062b16dd6bfea4ee03f99f13665cf4858130aae83eefec9c04cd94d3815ab` |
| 大小 | 8,696,984 bytes |
| protocol | 2 |
| Node 最低版本 | 24.0.0 |
| 文件数 | 214 |
| 安装根 | `/tmp/opencode/final-wxml-root3` |

包内包含 `zed-extension/grammars/wxml.wasm`、`zed-extension/extension.wasm`、
`bin/wxml-lsp`、`bin/wxml-type-backend/server.js` 及 Node backend 依赖。构建过程临时把
`editors/zed/grammars/wxml.wasm` 复制到 gitignored 目录，构建后已清理；包内 grammar 仍存在。

`install`、`verify`、`verify-package` 均通过：

```text
verified package wxml-lsp 0.1.0 (linux-x86_64)
protocol: 2  node: >= 24.0.0  files: 214
verified 0.1.0
binary version: 0.1.0
node: v24.22.2 (minimum 24.0.0)
protocol: 2
```

没有上传、tag、远端 release 或发布者签名；release 只在 Linux x86_64 验证。

## 6. Linux Zed 运行时冒烟

在隔离 Xvfb `:99`、隔离 `HOME`/`XDG`/user-data 中，把最终发布包的 `zed-extension` 手工安装
到隔离数据目录，打开 `tests/fixtures/page-basic`：

- release `wxml-lsp` 和 Node backend 启动。
- `{{ title }}` hover 显示 `(property) title: string` 和“页面标题”。
- definition 跳到 `tests/fixtures/page-basic/pages/index/index.ts` 的 `data.title`。
- 成员补全显示 `title`/`user`。
- 诊断显示 `nmae` 错误，状态栏计数 1。

截图（仓库外证据，未入库）：

- `zed/zed-proj-loaded.png`
- `zed/zed-hover-title4.png`
- `zed/zed-def-title.png`
- `zed/zed-completion-member.png`

这是对**最终 release 包**的运行时冒烟，不是重新执行标准 UI `zed: install dev extension`。
标准 Dev Extension 安装的历史证据仍是 [P8 第 13 节](p8-zed.md)，不能写成 W4 重跑。

## 7. 已做、未做与限制

### 7.1 已做

- Q2 调度/恢复合并；R2 Rust 与安全均 `APPROVE`。
- W4 Rust/Node 全量、ignored 真实门禁、性能、稳定性、覆盖率、audit/deny 和 Zed workspace 检查通过。
- 最终 release 两次构建 bit-identical；install/verify/verify-package 通过。
- Linux Zed 运行时冒烟使用最终 release 包通过。
- S 第 2 轮残余修复并由集成探针复验；没有宣称第 3 轮正式审核或 `APPROVED`。

### 7.2 未做与限制

- 没有 marketplace 上传、远端 release/tag、自动更新或发布者签名；信任链仍是 TLS + checksum，
  见 [ADR-0007](../adr/0007-local-delivery-trust-model.md)。
- 没有在最终版本上重新执行标准 Zed UI `zed: install dev extension`；§6 是手工安装发布包的
  运行时冒烟。
- 仍是单 workspace 内的显式多 app；不支持完整多 workspace、多根热加或跨 app 自动依赖图。
- TypeScript 同步调用不可中断；运行中请求只能等待后端返回或超时，排队请求可立即取消。
- 性能/稳定性 benchmark 使用单个小 fixture，不是中/大项目语料；大项目缓存、扫描和 RSS 仍需
  单独验证。
- 发布包只在 Linux x86_64 验证；没有 Windows/macOS 实机，也没有真实主桌面 Wayland 观感。
- 没有跨文件事务、原子多文件提交或 undo；S 的 tag rename、静态 include caller 覆盖、动态访问、
  共享 importedData 跨脚本和硬链接检测仍是明确边界。
- 本记录没有重新验证 Windows/macOS、真实 15 s Node 超时、完整 G0/G2 矩阵或标准 UI 安装。

## 8. 证据文件路径

以下是仓库外证据的路径或名称，未复制进仓库；本记录的数字来自这些文件。外部证据根目录
（未入库）为 `/tmp/opencode/wxml-full/20261116T143701Z-577f530`，下表路径相对该目录。

| 内容 | 路径/文件 |
| --- | --- |
| W4 Rust 全量 | `test-logs/final-rust-test.log` |
| W4 ignored 门禁 | `test-logs/final-rust-ignored.log` |
| 稳定性 JSON | `test-logs/final-stability.json` |
| 性能 JSON | `perf/final-rust-perf.json` |
| 覆盖率 | `coverage/final-summary.json` |
| 格式/检查/Clippy/Rustdoc | `test-logs/final-fmt.log`、`final-check.log`、`final-clippy.log`、`final-rustdoc.log`、`final-rust-doc-test.log` |
| Node 最终测试 | `test-logs/final-node-test-final.log`、`final-node-ci.log`、`final-node-check.log`、`final-node-build.log` |
| Node 发布布局 | `test-logs/final-node-server-layout.log`、`final-node-server-symlink.log` |
| 矩阵/性能 CLI | `perf/final-matrix.json`、`perf/final-perf.json` |
| 安全 | `test-logs/cargo-audit-final.json`、`cargo-deny-advisories-final.log` |
| Zed workspace | `test-logs/final-zed.log` |
| Release 构建 | `test-logs/final-release-a3.log`、`final-release-b3.log`；`final-release-a3/`、`final-release-b3/` |
| 安装/验证 | `test-logs/final-install3.log`、`final-verify3.log`、`final-verify-package3.log` |
| 发布清单 | `manifests/full-integration.md` |
| Zed 运行时截图 | `zed/zed-proj-loaded.png`、`zed-hover-title4.png`、`zed-def-title.png`、`zed-completion-member.png` |

## 9. 相关记录

- [W3 语义工具与安全编辑](full-feature-w3.md)
- [W2 集成与 Q1 交付](full-feature-w2.md)
- [WXML 语言功能基线](../reference/wxml-language-feature-baseline.md)
- [项目 README](../../README.md)
- [证据索引](README.md)
- [P8 Zed 扩展证据](p8-zed.md)
- [ADR-0007：本地交付与安装信任模型](../adr/0007-local-delivery-trust-model.md)
