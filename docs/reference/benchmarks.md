# 查询基准

更新日期：2026-11-20。

本文件说明 `wxml-lsp` 的查询基准（Criterion `queries` bench）测什么、如何运行、结果如何解读。它是**本地基线工具**，不是验收门禁，也不定义延迟预算。查询语义和协议契约仍以[基础契约](contracts.md)、[原生类型后端合同](tsgo-native-contract.md)和[模板类型诊断策略](type-diagnostics.md)为准；样本约定见[共享验收样本](../../tests/fixtures/README.md)。

## 1. 测量内容

基准按**查询路径**组织，而不是按内部函数组织。每个用例固定输入（fixture 文件、字节偏移、请求种类、缓存状态），组名为 `query/<路径>/<变体>`，可直接用 `--bench` 过滤。

| 组 | 用例 | 走的路径 |
| --- | --- | --- |
| `query/type` | `root`、`member`、`local` | 根变量、成员访问、`wx:for` 模板局部变量的 hover，经进程内类型会话查询 |
| `query/tag` | `builtin`、`component`、`attribute`、`value` | 内置标签、自定义组件标签、属性名、属性值的 hover，走静态 metadata |
| `query/class` | `extract`、`site`、`hover`、`completion`、`definition` | 静态类名提取、指定偏移的类名识别（纯 WXML 解析），以及 class hover/补全/定义（样式索引） |
| `first-query` | `type`、`tag`、`class` | 完整建栈后执行一次同类查询，见 §5 |

只看解析层的低噪声参考点时使用 `query/class/extract` 与 `query/class/site`；它们不依赖类型会话和样式索引。

## 2. 测量边界

每个样本都在进程内起一个真实 LSP 服务：

- 用 `UnixStream::pair()` 建立一对本地套接字，把服务端交给 `run_with_host(server_read, server_write, backend, host)`，host 使用生产 `ProjectState`。
- 客户端按 `Content-Length` 帧发送 `initialize`、`initialized`、`didOpen`，随后对同一会话重复发送目标请求。
- 计时区间是“写入请求帧到读出对应响应帧”。

因此一个样本**包含** JSON-RPC 编解码与分帧、scheduler 线程切换、workspace 查找、shadow 映射，以及类型会话或样式查询；**不包含**服务进程启动和编辑器客户端自身的开销，类型会话初始化只计入 §5 的冷样本。

不采用子进程启动已安装的 `wxml-lsp --stdio`：额外的进程与管道噪声会淹没亚毫秒路径，也要求先构建并安装完整布局。也不直接调用内部 helper：那会绕过 scheduler 和 shadow，测到的不是用户实际等待的路径。

## 3. 运行方式

```bash
cargo bench -p wxml-lsp --bench queries --features bench-support
```

该 bench 目标设置了 `required-features = ["bench-support"]`，所以必须显式启用该 feature；只写 `--bench queries` 会被 cargo 拒绝并提示缺少 feature。该 feature 默认关闭，只暴露 bench 侧入口，关闭时生产可见面不变。`cargo clippy --all-targets --all-features` 会编译 bench 目标，用于防止 bench 代码腐烂。

按组过滤（`--` 之后的参数传给 Criterion）：

```bash
cargo bench -p wxml-lsp --bench queries -- "query/class"
cargo bench -p wxml-lsp --bench queries -- "query/type"
cargo bench -p wxml-lsp --bench queries -- "first-query"
```

Criterion 的采样产物写入 `target/criterion/`；对比同一台机器上的改动时使用它的 baseline 参数（如 `--save-baseline` / `--baseline`），不要在项目目录另建结果目录。

`cargo bench` 使用 `bench` profile，它继承 `release`（含 thin LTO、`strip`、依赖 16 CGU、`x86-64-v3`），因此基准数字反映交付二进制的形态。改动根 `Cargo.toml` 的 profile 或 `.cargo/config.toml` 的 `rustflags` 后，旧 baseline 不再可比：先重建 baseline 再对比，并且一次只改一项（`codegen-units` 与 `target-cpu` 分别归因），否则无法区分来源。

## 4. 类型组与 typings 要求

`query/type/*` 和 `first-query/type` 需要已安装的 typings 布局；类型引擎在进程内运行，不需要引擎可执行文件，TypeScript 标准库随二进制内嵌。基准读取 `WXML_BENCH_INSTALL_ROOT`，默认 `$HOME/.local/share/wxml-lsp`，并要求该根目录下存在：

```text
<root>/bin/wxml-lsp
<root>/typings/miniprogram-api-typings
```

`query/tag/*` 与 `query/class/*` 不依赖该布局；`first-query/tag`、`first-query/class` 同样不需要 typings。

- 布局缺失时，类型组打印 `SKIP` 与安装提示，不产生伪造数据；其余组照常运行。
- 设置 `WXML_REQUIRE_REAL_ENGINE=1` 后，缺失布局从跳过改为硬失败，适合在确知已安装的机器上做完整基线。
- 布局由 [`scripts/install-native.sh`](../../scripts/install-native.sh) 按 `assets.lock.json` 下载并校验 typings 后创建；不下载或安装引擎。安装根必须位于仓库之外。直接运行 `target/debug` 或 `target/release` 下的二进制不算安装布局。

没有实际运行类型组时，不得声称类型路径的性能已验证。

## 5. 热查询与首次查询

- **热（`query/*`）**：后端、host、server、会话与索引全部就绪后重复查询同一位置，是主指标。`didOpen` 之后先把 `diagnostics.debounceMs` 设为 `0` 并等待首轮诊断结束，避免诊断任务与查询争抢 worker。
- **冷（`first-query/*`）**：每次迭代新建 backend + host + server，执行 `initialize` + `didOpen` + 一次查询后关闭，`sample_size(10)`。因此冷样本**包含** pinned typings 校验、进程内类型会话初始化（`initialize` 与能力探针）和样式索引的首次构建。

冷、热样本测的不是同一个区间，**不要相互比较**，也不要拼接两者的数字。冷样本用于观察“打开项目后第一次查询”的成本量级；热样本用于观察稳态路径的相对变化。

## 6. 正确性前置校验

每个用例在计时前先执行一次查询并断言预期结果，断言失败直接失败，不产出该用例的耗时：

- 变量类型：hover 返回具体类型文本（如 `string`），状态不得是降级或不可用。
- 标签信息：hover 文档包含预期的内置标签/组件描述/属性说明片段。
- class：提取与识别返回预期类名；补全包含预期候选；定义指向 fixture 的 `index.wxss`。

这样可避免“查询返回空结果所以很快”被记成有效数据；降级和 `Unavailable` 一律按失败处理。

## 7. 结果解读

按 [ADR-0004](../adr/0004-parallel-delivery-and-validation-gates.md) 的要求，记录或引用基准数字时必须同时说明：

- **硬件**：CPU 型号与运行环境（共享 CI 机器上的数字不作基线）。
- **样本规模**：fixture 的 WXML/WXSS/TS 行数、样式类数量与查询位置；bench 输出会打印这些规模信息。
- **冷热状态**：该数字来自 `query/*` 还是 `first-query/*`。

比较时只看同一台机器、同一 fixture、同一模式下的相对变化（Criterion 报告的中位数与区间），不在机器之间搬运绝对值。本基准**不设固定延迟预算**，也不加入 CI 耗时门禁：ADR-0009 已删除迁移期的性能门禁，本工具不恢复它，只提供手工运行的本地基线与回归参考。

## 8. 样本工程

基准读取 `tests/fixtures/bench-queries/` 下的真实小程序工程（`app.json`、`app.wxss`、`tsconfig.json`、`pages/index/*`、`components/outer-card/*`），覆盖根变量、成员访问、`wx:for` 局部变量、内置标签、自定义组件、属性名与属性值、静态与混合 mustache class，以及带 `@import` 链的 WXSS。样本用途与目录约定记录在[共享验收样本](../../tests/fixtures/README.md)；它不是 AC manifest 的一部分，不改变 AC 语义。

用例用**唯一文本锚点**定位查询偏移，锚点由 bench 代码（`crates/wxml-lsp/benches/`）声明和维护：锚点在对应文件中必须唯一，且偏移落在合法字符边界上。修改 fixture 文本时需要同步更新 bench 侧的锚点与期望结果；基准不在仓库内生成临时工程。
