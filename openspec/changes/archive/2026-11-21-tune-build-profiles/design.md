# Design

## Context

动机见 `proposal.md`。影响设计取舍的既有事实：

- 根 workspace 有 188 个依赖，其中 30 个 `tsrs_*` crate 构成完整的 TypeScript
  编译/检查链路，是 release 构建时间的主体；它们在运行期是进程内类型会话的热路径。
- 当前 `target/release/wxml-lsp` 为 30.1 MB，未 strip（`.strtab` 4.1 MB、`.symtab` 0.9 MB）。
  `tsrs_ast::identifier::SOURCE_TEXTS`（16 MiB）等大数组位于 BSS，不占文件体积。
- 失败模型要求单请求 panic 只失败该请求：`crates/wxml-lsp/src/scheduler.rs:422`、
  `crates/wxml-lsp/src/server.rs:613/623`、`crates/wxml-backend-tsgo/src/native/inproc.rs:383`
  使用 `catch_unwind`。因此 `panic = "abort"` 不可用。
- `editors/zed/` 是独立 workspace（`[workspace]` 且不在根 members 中），
  Zed 用 `cargo build --target wasm32-wasip2 --release --locked` 构建；根 profile 对它无效。
- 基准（`cargo bench -p wxml-lsp --bench queries --features bench-support`）的 `bench`
  profile 继承 `release`，因此基准数字天然反映交付二进制的优化级别。
- 仓库禁止临时文件与 `docs/evidence`（ADR-0004/0005）：基准对比结论只出现在对话/提交说明。

## Goals / Non-Goals

**Goals:**

- 让 "依赖不因单 CGU 而拖慢构建" 与 "最终二进制仍是 LTO 优化产物" 同时成立。
- 让交付二进制的运行时性能拿到本机指令集这一层收益，且不波及 wasm 目标。
- 让 `perf` 可用（有独立 profiling profile），而交付产物保持精简。
- 让 `dev`/`test` 下的进程内类型引擎可用（真实引擎用例不再因未优化而失真）。

**Non-Goals:**

- 不切换 `lto = "fat"`，不引入 PGO，不引入 `-Zbuild-std` 或 nightly 依赖。
- 不改 `panic`、`overflow-checks`、`debug-assertions` 的默认语义（后者在 release 已关闭）。
- 不调优 `editors/zed/grammars/` 的 tree-sitter WASM（由 Zed 以 clang `-Os` 构建，非本仓库可控）。
- 不写 ADR：本变更不改变分层、契约或模块边界，属工具链配置。

## Decisions

### D1. 保留 `lto = "thin"`，只显式写出 `opt-level = 3`

Cargo 文档明确 thin LTO "takes substantially less time to run while still achieving
performance gains similar to fat"。fat 的边际收益在个位数百分比以下，代价是链接时间
和内存翻倍，而且 `bench` 继承 `release`，fat 会直接拖慢基准迭代。没有 A/B 数据前不改。

- 备选：`lto = "fat"` + `[profile.bench] lto = "thin"` 保持迭代速度。放弃原因是那会让
  bench 数字与交付二进制产生系统性偏差，失去"基准即交付态"的性质。

### D2. 依赖 `codegen-units` 回到 16

`[profile.release] codegen-units = 1` 默认作用于整图。tsrs 链路被压成单 CGU 后并行代码
生成消失，是 release 冷构建时间的主要来源；而 thin LTO 在链接期仍会跨 crate 合并与内联，
依赖的 1 CGU 对成品几乎无增益。用 `[profile.release.package."*"] codegen-units = 16`
只把依赖还原为默认并行度，workspace 成员仍保持 1 CGU。

- 备选：对 `tsrs_*` 逐个列出配置。放弃原因：cargo 的 package 表只支持精确名与 `"*"`，
  逐项维护 30 个包名随 pin 变化而腐化，收益与 `"*"` 相同。
- 风险对冲：若发现 thin LTO 在更多 CGU 下链接时间反而上升，回退这一行即可，不影响其它决策。

### D3. `build-override` 用 `opt-level = 0`

构建脚本与 proc-macro 只在编译期运行，`opt-level = 3` 与 `codegen-units = 1` 让它们的
编译成本显著上升（`serde`/`syn` 一类 derive 尤其明显），而运行期收益只体现为构建脚本
几毫秒的执行差异。`[profile.release.build-override]` 设为 `opt-level = 0`、
`codegen-units = 16`，并在注释里写明这是构建时间取舍而非运行取舍。

### D4. `strip = "symbols"`，符号需求交给 `[profile.profiling]`

交付二进制不再保留符号表（约省 5 MB），冷启动页入更少。仓库里没有任何 backtrace
处理代码（全仓无 `RUST_BACKTRACE`/`backtrace` 引用），因此丢失符号不影响现有诊断路径。
需要 `perf`/flamegraph 时用独立 profile：

```toml
[profile.profiling]
inherits = "release"
debug = "line-tables-only"
strip = "none"
```

- 备选：`strip = "debuginfo"`。在该仓库等价于不 strip（release 默认 `debug = false`），
  拿不到体积收益，放弃。
- 备选：始终 `strip = "none"` 保留符号。放弃原因：为偶发 profiling 常驻 5 MB 且拖慢页入。

### D5. `target-cpu` 不进入默认配置（A/B 后否决）

解析、JSON、哈希、类型图遍历都是 CPU 密集路径，`target-cpu` 本是预期收益最大的一项。
在本机（Criterion `queries` bench，`bench` profile 继承 `release`）做双向 A/B 后否决：

| 轮次 | 配置顺序 | 观察 |
| --- | --- | --- |
| A | 先 `x86-64-v3` 存基线，再 generic 对比 | generic 快 1–8%，`query/type/root`、`member` p<0.05 |
| B | 先 generic 存基线，再 `x86-64-v3` 对比 | 中性居多，`query/tag/value`、`class/completion`、`first-query/type`、`first-query/class` 回退 2–4% |
| 噪声 | generic 连测两次 | `query/type/root` 225.9 µs vs 240.3 µs（±6%），`query/class` 稳定在 ±2% |

结论：该负载是标量指针追逐与分配为主，AVX2 代码生成没有可复现收益，落点全在噪声里；
反向一轮还出现回退，说明是代码布局的运气而非稳定优化。**默认配置不写入 `target-cpu`**，
把 SSE/AVX2 带来的可移植性代价（老机器 SIGILL）留给愿意自测的人。

- 保留的约束：任何自行启用的写法都必须按目标三元或 `RUSTFLAGS` 指定，不能写成
  `[build] rustflags`，否则 `wasm32-wasip2` 的 Zed 扩展构建会失败（wasm32 只接受
  `generic`/`mvp` 一类 CPU 名）。README 记录 `RUSTFLAGS="-C target-cpu=native"` 的用法。
- 备选：默认 `x86-64-v3`。否决，理由如上，且它把 SIGILL 风险转嫁给所有使用者。
- 备选：继续测 `lto = "fat"`。未做，`benches` 的噪声幅度（±6%）已大于 fat 的预期收益上限，
  用现有工具无法给出可信结论，列为非目标。

### D6. `dev`/`test` 依赖用 `opt-level = 2`

`test` profile 继承 `dev`，所以只写 `[profile.dev.package."*"]` 即同时覆盖两者。动机是
`TSGO_TYPINGS_ROOT` 下运行真实引擎用例时，未优化的 `tsrs_*` 慢一到两个数量级，容易把
"未优化"误读为"实现慢"。`debug-assertions` 仍随 `dev` 为 true，不改变断言语义。

- 代价：`cargo clippy`/`cargo test` 首次构建依赖变慢。CI 依赖 `Swatinem/rust-cache`
  缓存依赖产物，且依赖很少变动，接受该代价；若后续不可接受，删除该段即回到默认。
- 备选：只优化少数 tsrs 包。放弃原因同 D2（package 表无前缀匹配），且热路径横跨
  parser/binder/checker/compiler 多个 crate。

### D7. Zed 扩展走体积优先，且与性能 profile 完全隔离

`editors/zed/src/lib.rs` 只在启动期构造 `Command` 与 workspace 配置，不是热路径；
WASM 组件由 Wasmtime/Cranelift 在加载时重新编译，且 PGO 数据在 wasm 上采集不到
（`profiler_builtins` 未随 wasm `rust-std` 分发，实测 `E0463`）。因此：

```toml
[profile.release]
opt-level = "s"
lto = true
codegen-units = 1
strip = "symbols"
```

不加 `panic = "abort"`：体积收益有限，而它会改变扩展内任何 panic 的表现（宿主 trap 掉
整次调用），与原生侧的隔离语义不一致，没必要为几百 KB 引入行为差异。

### D8. 验证策略

profile 改动不会产生可断言的输出差异，只能靠构建产物与既有门禁验证：

1. 全量门禁（与 CI 一致）：fmt / clippy / test / doc / deny。
2. 体积：`ls -l target/release/wxml-lsp`，与 30.1 MB 基线对比。
3. 运行时：`cargo bench -p wxml-lsp --bench queries --features bench-support`
   先 `--save-baseline pre-profile`，改后 `--baseline pre-profile` 对比；
   `target-cpu` 与 `codegen-units` 各自单独一轮，避免混淆归因。基准结论只写进提交说明。
4. wasm：`cd editors/zed && cargo build --target wasm32-wasip2 --release --locked`，
   并确认根 `.cargo/config.toml` 的 flag 没有波及该构建。

## Risks / Trade-offs

- **`target-cpu=x86-64-v3` 在无 AVX2 的机器上 SIGILL** → 仅支持 Linux x86_64 的既有定位；
  README 写明症状与 `RUSTFLAGS=""` 逃生口。
- **`[build] rustflags` 误写导致 wasm 构建失败** → 设计固定为目标三元写法，并用 wasm
  构建作为验收项 6。
- **`strip = "symbols"` 后无法定位线上 panic** → 现有代码不采集 backtrace；需要时用
  `--profile profiling` 重建。
- **依赖 `codegen-units = 16` 反而拉长 thin LTO 链接** → 单行回退；先测冷构建与增量构建时间。
- **`dev` 依赖 opt-level 2 让 CI 首次构建变慢** → rust-cache 缓存；不可接受时删该段。
- **`.cargo/config.toml` 影响所有 Linux x86_64 构建（含 CI）** → GitHub runner 满足 v3；
  本地老机器用环境变量覆盖。
- **wasm profile 改动无 CI 覆盖** → 任务里显式执行 zed workspace 构建；失败即回退该段。

## Migration Plan

1. 按任务顺序落地：根 profile → `.cargo/config.toml` → dev profile → zed profile → 文档。
2. 每组独立提交，各自通过对应验证；`target-cpu` 单独提交，便于单点回滚。
3. 回滚：删除对应 profile 段/配置文件即可，无数据、无协议、无资产迁移。

## Open Questions

无。`lto = "fat"` 与 PGO 的收益需要独立测量，已明确列为非目标，不影响本变更的落地。
