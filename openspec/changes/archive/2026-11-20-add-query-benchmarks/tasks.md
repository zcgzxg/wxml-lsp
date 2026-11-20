# Tasks

## 1. bench 骨架（可并行：A）

- [x] 1.1 `crates/wxml-lsp/Cargo.toml` 增加 `criterion` dev-dependency、
      `[[bench]] name = "queries"`（`harness = false`, `required-features = ["bench-support"]`）
      与 `[features] bench-support = []`。
- [x] 1.2 新增 `crates/wxml-lsp/src/bench_support.rs`，仅在 `bench-support` feature 下编译：
      `BenchBackend { backend: Arc<dyn Backend>, host: Arc<dyn ProjectHost> }`（两者共享同一 host）、
      `backend_from_install(root) -> Result<BenchBackend, String>`、
      `host_only_backend() -> BenchBackend`、
      `class_tokens(text) -> Vec<(u32, u32, String)>`、
      `class_site_at(text, offset) -> Option<(String, u32, u32)>`。
      全部 `#[doc(hidden)]` 且有文档注释。
- [x] 1.3 `lib.rs` 在 feature 下挂载 `bench_support`；`backend_tsgo/engine.rs` 的
      `discover_at` 放宽到 `#[cfg(any(test, feature = "bench-support"))]`。
- [x] 1.4 确认 feature 关闭时无可见面变化：`cargo check -p wxml-lsp`。

## 2. bench fixture（可并行：B，独立于 1）

- [x] 2.1 新增真实小程序工程 `tests/fixtures/bench-queries/`：
      `app.json`、`app.wxss`、`tsconfig.json`、`pages/index/index.{wxml,ts,wxss}`、
      `components/outer-card/outer-card.{wxml,ts,json,wxss}`。
- [x] 2.2 WXML 提供唯一锚点：根变量、成员访问、`wx:for` 局部变量、内置标签、自定义组件、
      属性名、枚举属性值、静态 class、混合 mustache class。TS 提供明确类型
      （`string`/接口成员），WXSS 提供 `@import`、全局与页面规则、足够数量的类。
- [x] 2.3 在 `tests/fixtures/README.md` 记录该样本用途与锚点约定（不改动 AC manifest 语义）。
- [x] 2.4 确认 fixture 可被现有发现逻辑读取：写一个临时验证脚本放在 `/tmp/opencode`，
      不在仓库内留文件。

## 3. bench harness 与用例（依赖 1、2：C）

- [x] 3.1 `crates/wxml-lsp/benches/queries.rs` 作为入口，子模块：
      `session.rs`（UnixStream 对 + `run_with_host` + JSON-RPC 客户端）、
      `scenario.rs`（锚点/期望/offset 解析）、`groups.rs`（type/tag/class/first-query）。
- [x] 3.2 `session.rs`：`initialize`（`rootUri` 指向 fixture、`diagnostics.debounceMs = 0`）、
      `initialized`、`didOpen`、等待首轮诊断结束、请求/响应配对、`shutdown`。
- [x] 3.3 `scenario.rs`：按唯一文本锚点解析偏移（断言唯一且非 ASCII 边界安全），
      每个 case 声明请求种类、期望结果片段与失败信息。
- [x] 3.4 热用例按矩阵实现：
      `query/type/{root,member,local}`、`query/tag/{builtin,component,attribute,value}`、
      `query/class/{extract,site,hover,completion,definition}`。
- [x] 3.5 冷用例：`first-query/{type,tag,class}`，每轮新建 backend/host/server 与索引，
      `sample_size(10)`；类型组只在引擎可用时注册。
- [x] 3.6 正确性前置：每个 case 计时前断言期望结果；空结果/降级/`Unavailable` 直接失败。
- [x] 3.7 引擎门槛：读取 `WXML_BENCH_INSTALL_ROOT`（默认 `$HOME/.local/share/wxml-lsp`），
      缺失时打印 SKIP 与安装提示并跳过类型组；`WXML_REQUIRE_REAL_ENGINE=1` 时硬失败。
- [x] 3.8 bench 输出记录 fixture 规模（行数、类数量）与模式（热/冷），满足 ADR-0004
      对规模与冷热状态的要求。

## 4. 文档（可并行：D，依赖 3 的组名约定）

- [x] 4.1 新增 `docs/reference/benchmarks.md`：运行方式、环境变量、测量边界、
      冷/热解释、可比较范围、硬件与文件规模说明、不得当 CI 门禁使用。
- [x] 4.2 在 `docs/README.md` 的 reference 索引加入该文档链接（若存在索引）。

## 5. 验证（主代理汇总验收）

- [x] 5.1 `cargo fmt --all -- --check`
- [x] 5.2 `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [x] 5.3 `cargo test --workspace --all-features`
- [x] 5.4 `cargo doc --workspace --no-deps`
- [x] 5.5 `cargo bench -p wxml-lsp --bench queries --no-run`
- [x] 5.6 实跑静态组与 class 组；若本机已安装原生布局，实跑类型组与冷组，
      否则记录 SKIP 原因（不得声称类型性能已验证）。
- [x] 5.7 `openspec validate add-query-benchmarks --strict`
