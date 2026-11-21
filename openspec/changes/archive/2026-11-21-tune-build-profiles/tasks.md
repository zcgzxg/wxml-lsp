# Tasks

## 1. 根 workspace 的 release profile

- [x] 1.1 在根 `Cargo.toml` 的 `[profile.release]` 显式写出 `opt-level = 3`，新增 `strip = "symbols"`，保留 `lto = "thin"` 与 `codegen-units = 1`；用注释说明 `panic` 保持默认 `unwind` 是因为 scheduler/server/inproc 依赖 `catch_unwind` 做单请求隔离。
- [x] 1.2 新增 `[profile.release.package."*"] codegen-units = 16`，注释说明依赖侧不需要单 CGU（thin LTO 在链接期仍跨 crate 优化）。
- [x] 1.3 新增 `[profile.release.build-override]`（`opt-level = 0`、`codegen-units = 16`），注释写明这是构建时间取舍。
- [x] 1.4 新增 `[profile.profiling]`（`inherits = "release"`、`debug = "line-tables-only"`、`strip = "none"`），并在注释里给出 `cargo build --profile profiling -p wxml-lsp` 的用途。
- [x] 1.5 运行 `cargo build --release -p wxml-lsp`，记录构建耗时与 `ls -l target/release/wxml-lsp` 体积，确认相比 30.1 MB 基线下降且 `file` 显示 `stripped`。

## 2. 目标三元下的 target-cpu

- [x] 2.1 试启 `.cargo/config.toml`（`[target.x86_64-unknown-linux-gnu] rustflags = ["-C", "target-cpu=x86-64-v3"]`），用 `cargo build -v` 确认 native 构建带上该 flag、`cd editors/zed && cargo build --target wasm32-wasip2 --release -v` 不带该 flag（证明按目标三元限定生效，且没有写成会波及 wasm 的 `[build] rustflags`）。
- [x] 2.2 做双向 A/B（`--save-baseline v3` 后 generic 对比，再 `--save-baseline generic` 后 v3 对比）：差异落在运行间噪声内（同配置重复即相差 6%），反向一轮出现 2–4% 回退，结论是不进默认配置；数字见 `design.md` D5。
- [x] 2.3 删除 `.cargo/config.toml`，在 `README.md` 记录自行启用的方式（`RUSTFLAGS="-C target-cpu=native"`）与"不能写成 `[build] rustflags`"的约束；确认 zed workspace 构建仍通过、`git status` 无残留配置文件。

## 3. dev/test 的依赖优化

- [x] 3.1 新增 `[profile.dev.package."*"] opt-level = 2`，注释说明 `test` 继承 `dev`，以及代价（首次构建变慢）与动机（`TSGO_TYPINGS_ROOT` 下的真实引擎用例不再被未优化代码失真）。
- [x] 3.2 运行 `cargo test --workspace --all-features`，确认全绿且断言语义未变（`debug-assertions` 仍为 true）。

## 4. Zed 扩展的 wasm profile

- [x] 4.1 在 `editors/zed/Cargo.toml` 新增体积优先的 `[profile.release]`：`opt-level = "s"`、`lto = true`、`codegen-units = 1`、`strip = "symbols"`；注释说明不加 `panic = "abort"` 的原因与"该组件不是热路径"。
- [x] 4.2 在 `editors/zed` 下运行 `cargo fmt --all -- --check`、`cargo clippy --all-targets --locked -- -D warnings`、`cargo test --locked`、`cargo build --target wasm32-wasip2 --release --locked`，全部通过。

## 5. 文档

- [x] 5.1 在 `README.md` 的构建小节补充：release/bench 的 profile 选择（thin LTO、strip、依赖 16 CGU、build-override）、`x86-64-v3` 的前提与 SIGILL 症状、`RUSTFLAGS=""` 逃生口、`--profile profiling` 的用法；并明确根 profile 不覆盖 `editors/zed/`。
- [x] 5.2 在 `docs/reference/benchmarks.md` §3 补充一段：`bench` 继承 `release`（含 LTO/strip），改动根 profile 后必须重建 baseline 再对比；A/B 时一次只改一项。

## 6. 全量验证

- [x] 6.1 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`cargo doc --workspace --no-deps`、`cargo deny --offline check`，全部通过。
- [x] 6.2 串行跑基准：`cargo bench -p wxml-lsp --bench queries --features bench-support -- --save-baseline <name>`，覆盖 `query/tag`、`query/class`、`query/type` 与 `first-query`；`target-cpu` 按正向/反向两轮归因（结论见 `design.md` D5），采样只落在被忽略的 `target/criterion/`。
- [x] 6.3 确认 `git status --short` 只包含预期文件（`Cargo.toml`、`.cargo/config.toml`、`editors/zed/Cargo.toml`、`README.md`、`docs/reference/benchmarks.md`、openspec 产物），仓库内无临时文件。

## Workflow follow-up

- 归档本 change（`openspec archive tune-build-profiles`）。
- 按 Conventional Commits 分组提交：`build(profile): ...`（根 profile）、`build(cargo): ...`（target-cpu）、`build(zed): ...`（wasm profile）、`docs: ...`。
- 用 `cargo build --release -p wxml-lsp && scripts/install-native.sh --root "$HOME/.local/share/wxml-lsp"` 重新安装本地布局，确认编辑器实际启动与类型查询可用（安装根在仓库外）。
