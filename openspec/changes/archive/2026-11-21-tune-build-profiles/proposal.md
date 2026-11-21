# Proposal

## Why

根 workspace 的 `[profile.release]` 只有 `lto = "thin"` 与 `codegen-units = 1` 两项，
其中 `codegen-units = 1` 会作用于全部 188 个依赖（含 30 个 `tsrs_*` crate）：构建时间
和内存代价显著，而最终代码质量在 LTO 阶段已被合并，依赖侧的单个 CGU 基本不带来运行
收益。同时三类可观的杠杆没有被使用：release 二进制未 strip（30.1 MB，其中 `.strtab`
4.1 MB、`.symtab` 0.9 MB）、没有为目标三元设置 `-C target-cpu`（这是解析/类型检查这类
CPU 密集负载最大的一项剩余收益）、`dev`/`test` profile 下进程内类型引擎依赖完全未优化
（`TSGO_TYPINGS_ROOT` 跑真实引擎用例时慢一到两个数量级）。此外需要把范围写清楚：
`editors/zed/` 是独立 workspace，根 profile 对其无效，且它们是 `wasm32-wasip2` 目标，
`target-cpu`/PGO 这类本机代码优化手段不适用。

## What Changes

- 根 `Cargo.toml` 的 `[profile.release]`：显式写出 `opt-level = 3`，新增 `strip = "symbols"`；
  保留 `lto = "thin"`（未 A/B 前不改 fat）。
- 新增 `[profile.release.package."*"] codegen-units = 16`，让依赖回到并行代码生成。
- 新增 `[profile.release.build-override]`（`opt-level = 0`、`codegen-units = 16`），
  构建脚本与 proc-macro 不再按 release 级优化。
- 新增 `[profile.dev.package."*"] opt-level = 2`，只影响 `dev`/`test`（`test` 继承 `dev`），
  让进程内类型引擎在本地验证与真实引擎用例中可用。
- 新增 `[profile.profiling]`（`inherits = "release"`、`debug = "line-tables-only"`、
  `strip = "none"`），供 `perf`/flamegraph 使用，不污染交付产物。
- `editors/zed/Cargo.toml` 增加体积优先的 `[profile.release]`（`opt-level = "s"`、
  `lto = true`、`codegen-units = 1`、`strip = "symbols"`）；该组件不是热路径，
  不追求指令级最优。
- **不**写入默认 `-C target-cpu`：双向 A/B 显示差异落在噪声内并出现 2–4% 回退，
  只在 README 记录自行启用的方式。
- 文档：README 构建小节说明 profile 决策、`target-cpu` 的实测结论与 opt-in 写法；
  `docs/reference/benchmarks.md` 说明基准继承 release profile，改 profile 后需重建基线。

不改变任何语言功能、LSP 协议行为或引擎/资产 pin。`panic = "abort"` 明确不做：
`scheduler.rs`、`server.rs`、`native/inproc.rs` 依赖 `catch_unwind` 做单请求 panic 隔离。

## Capabilities

### New Capabilities

无。本变更只调整构建配置与文档，不引入新的系统行为。

### Modified Capabilities

无。`query-benchmarks` 的测量口径（按查询路径独立计时、预热与首次查询分开、结果正确性
优先）不因 profile 变化而改变；基准仍是本地工具而非门禁。

> 说明：`openspec/changes/tune-build-profiles/.openspec.yaml` 设置 `skip_specs: true`。
> 构建 profile 属于工具链配置，不产生可观察的 capability 需求变化。

## Impact

- 代码：根 `Cargo.toml`、`editors/zed/Cargo.toml`，以及 `README.md`、
  `docs/reference/benchmarks.md`。不触碰 `crates/` 下任何源码，也不新增
  `.cargo/config.toml`（`target-cpu` 的 A/B 结论是"不进默认配置"）。
- 构建产物：release 二进制体积下降（实测 30.1 MB → 29.0 MB，约 -2.5 MB；`.strtab` 大部分
  已在 LTO 阶段合并，故降幅小于符号表原始大小）；release 冷构建时间下降
  （依赖侧恢复并行代码生成）；`cargo test` 首次构建变慢、运行变快。
- 可移植性：默认构建不引入任何架构假设，仍可在无 AVX2 的机器上构建与运行；
  自行开启 `target-cpu` 的用法写在 README，并保留"不能写成 `[build] rustflags`"的约束。
- 依赖与资产：无新增依赖，`Cargo.lock`、`deny.toml`、`assets.lock.json`、引擎 pin 均不变。
- CI：`cargo fmt/clippy/test/doc/deny` 命令不变；没有按目标三元的 rustflags，CI 与本地
  构建参数保持一致。
- 未纳入：`lto = "fat"`、PGO、`wasm-opt`、默认 `target-cpu`。基准的运行间噪声（±6%）
  大于 fat 的预期收益上限；PGO 在原生侧需长期维护流水线，在 wasm 侧连 profile 都采不到
  （`profiler_builtins` 未随 wasm `rust-std` 分发，实测 E0463）。
