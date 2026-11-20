# Design

## Context

见 `proposal.md` 的 Why。设计只依赖已存在的结构：`wxml_lsp::run_with_host` 是公开入口，
生产 `ProjectState` 是公开 host，`run_with_host` 接受 `Arc<dyn Backend>`；而具体 backend
（`TsgoBackendAdapter`）和安装发现都在 `pub(crate)` 内。`criterion 0.8.2` 已在 workspace
声明并有 `crates/wxml-text/benches/line_index.rs` 先例。

约束：ADR-0009 删除过性能门禁，`docs/README.md` 要求不重建；ADR-0004 要求性能数据注明
硬件、文件规模与冷热状态。真实类型查询必须走已安装的 pinned 引擎与 typings。

## Goals / Non-Goals

**Goals:**

- 以真实 LSP 路径（handler → workspace → scheduler → shadow → 引擎/样式索引）测量每条
  查询路径，不让 bench 复制被测逻辑。
- 结果可校验：先证明查询命中预期结果，再计时。
- 冷/热分离，默认手动运行，可作为本地基线与回归参考。

**Non-Goals:**

- 不建立 CI 耗时门禁、不冻结延迟预算。
- 不测诊断（pull/push）、格式化、rename 等写路径，也不测编辑器进程与真实客户端 IPC。
- 不为 bench 重构生产查询代码，不把 `pub(crate)` 大面积公开。

## Decisions

### D1. Criterion，单 bench 目标，手动运行

复用 workspace `criterion = "0.8.2"`，在 `crates/wxml-lsp` 增加
`[[bench]] name = "queries", harness = false, required-features = ["bench-support"]`。
`cargo bench -p wxml-lsp --bench queries` 即可运行，`-- "query/class"` 过滤。
不加入 CI 门禁；但 `cargo clippy --all-targets --all-features` 会编译 bench，防止
bench 代码腐烂。

备选：`perf_budget.rs` 风格的 opt-in p95 测试（现有 `wxml-backend-tsgo/tests/perf_budget.rs`）。
不采用：它测 TS 输入层而非 WXML 全链路，且需要自造统计与采样睡板。

### D2. 测量边界：进程内真实 LSP，Unix socket 对

bench 用 `UnixStream::pair()` 起 `run_with_host(server_read, server_write, backend, host)`，
客户端按 Content-Length 帧发送 `initialize` / `initialized` / `didOpen`，随后对同一会话
重复请求。计时区间是“写入请求帧到读出对应响应帧”，包含 JSON-RPC 编解码、scheduler 线程
切换、workspace 查找、shadow 映射与引擎/样式查询，不包含进程启动和操作系统 IPC。

备选：子进程启动已安装的 `wxml-lsp --stdio`。不采用：额外的进程与管道噪声会淹没亚毫秒
路径，且要求先构建并安装完整布局。备选：直接调用内部 helper。不采用：`pub(crate)` 面
太窄，会迫使大范围公开，且绕过 scheduler/shadow，测不到用户实际等待。

### D3. 唯一的 bench 侧公开面：`bench-support` feature

`crates/wxml-lsp` 新增 feature `bench-support`（默认关闭），提供 `src/bench_support.rs`：

- `pub struct BenchBackend { backend: Arc<dyn Backend>, host: Arc<dyn ProjectHost> }`：两者
  必须共享同一个 host，adapter 与 `run_with_host` 用同一个实例，避免视图与会话错位。
- `pub fn backend_from_install(root: &Path) -> Result<BenchBackend, String>`：用
  `<root>/bin/wxml-lsp` 走 `engine::discover_at` + `TsgoBackendAdapter::with_install`。
- `pub fn host_only_backend() -> BenchBackend`：`without_engine`，供不依赖引擎的组。
- `pub fn class_tokens(text: &str) -> Vec<(u32, u32, String)>`、
  `pub fn class_site_at(text: &str, offset: u32) -> Option<(String, u32, u32)>`：
  纯解析路径，LSP 没有对应方法。

同时把 `engine::discover_at` 的 `#[cfg(test)]` 放宽为
`#[cfg(any(test, feature = "bench-support"))]`。feature 关闭时生产可见面完全不变。

备选：在 `wxml-backend-tsgo` 直接实现 `Backend`。不采用：会复制 adapter、shadow 与
presentation 逻辑，测到的是副本而不是生产代码。

### D4. 专用 fixture 工程

新增 `tests/fixtures/bench-queries/`：真实小程序项目，含 `app.json`、`app.wxss`、
`tsconfig.json`、`pages/index/index.{wxml,ts,wxss}`，以及一个自定义组件
`components/outer-card/*`。WXML 同时包含根变量、成员访问、`wx:for` 局部变量、内置标签、
自定义组件标签、属性名/属性值、静态与混合 class 属性；WXSS 含 import 链与多条规则。
bench 直接读取该目录，不在仓库内生成任何临时工程。

备选：复用 `tests/fixtures/page-basic`。不采用：没有 `.wxss`，class 组无法覆盖样式索引。
备选：运行时在 `/tmp` 造工程。不采用：anchor、规模和内容不可复现，也不便于审阅。

### D5. 冷/热两种模式

- **热**：后端、host、server、会话与索引全部就绪，重复查询同一位置（主指标）。
- **冷**：每次迭代新建 backend + host + server，`initialize` + `didOpen` + 一次查询后
  关闭；`sample_size(10)`。冷模式包含 pinned 资产校验、引擎进程 handshake 与样式索引
  首次构建，文档中明确说明。

`didOpen` 后先把 `initializationOptions.wxml.diagnostics.debounceMs` 设为 `0`，等待首轮
诊断完成再开始采样，避免诊断任务与查询抢单 worker。

### D6. 正确性前置校验

每个 case 在 `bench_function` 之外先执行一次查询并断言预期结果：

- 变量类型：hover 结果包含预期类型文本（如 `string`），且状态不是降级/不可用。
- 标签信息：hover 文档包含预期文档片段（内置标签、组件描述、属性说明）。
- class：返回的标记文本包含预期类名；补全包含预期候选；定义指向 `index.wxss`。

断言失败直接 `panic!`（带 case 名与脱敏后的结果摘要），不产出该 case 的耗时。类型组在
无引擎时整体打印 SKIP 与安装提示；`WXML_REQUIRE_REAL_ENGINE=1` 时改为硬失败。

### D7. 组名与用例矩阵

组名固定为 `query/<路径>/<变体>`，便于 `--bench` 过滤与 baseline 比较。首批矩阵：

| 组 | 用例 |
| --- | --- |
| `query/type` | `root`、`member`、`local`（`wx:for` 局部变量） |
| `query/tag` | `builtin`、`component`、`attribute`、`value` |
| `query/class` | `extract`、`site`、`hover`、`completion`、`definition` |
| `first-query` | `type`、`tag`、`class`（完整建栈 + 一次查询） |

引擎环境：`WXML_BENCH_INSTALL_ROOT`，默认 `$HOME/.local/share/wxml-lsp`；静态组不需要它。

## Risks / Trade-offs

- 亚毫秒路径易受噪声影响 → Criterion 默认 warm-up + 报告中位数/区间；不设阈值，比较时
  看同一机器的相对变化。
- 冷组重复校验 24 MiB 引擎资产会压低样本量下的稳定性 → 固定 `sample_size(10)`，在文档
  注明“冷启动包含资产校验”，不与热数据比较。
- fixture 规模有限，不代表超大小程序 → 在 bench 输出与文档记录文件行数、类数量与查询
  位置；后续可加规模变体而不改结构。
- `bench-support` feature 可能与生产代码漂移 → CI 的 `clippy --all-features` 编译 bench
  目标；运行仍是手动。
- 引擎 session 成本可能主导类型 hover 的绝对值 → 这是真实用户成本，保留；另用
  `query/class/extract|site` 提供解析层的低噪声参考点。

## Migration Plan

纯新增，无数据或协议迁移。回滚即还原本 change 的文件；不涉及发布流程。

## Open Questions

- 是否需要在后续 change 中把组扩展到 `definition`/`references`/`semanticTokens`。当前
  spec 声明的三组已满足需求，扩展不改变本设计。
