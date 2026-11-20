# Proposal

## Why

目前没有任何可重复的方式测量 WXML 查询路径的耗时：`crates/wxml-text` 只有一个
`line_index` bench，hover、completion、class 样式查询等真正影响编辑器体验的路径
既没有基线，也没有回归手段。ADR-0009 删除了迁移期的性能门禁后，这部分完全空
白，改动 `handlers/hover`、`handlers/styles` 或 native backend 时无法判断响应是变
快还是变慢。

## What Changes

- 在 `wxml-lsp` crate 增加 Criterion 0.8 bench 目标（复用 workspace 已有的
  `criterion` 依赖和 `crates/wxml-text/benches/line_index.rs` 先例），按查询路径
  而不是按内部函数组织测量。
- 首批覆盖三组、每条路径独立计时：
  - **变量类型**：WXML 中根变量、成员访问、模板局部变量（`wx:for`/`wx:if`）的
    hover，走 native tsgo 类型路径。
  - **标签信息**：内置标签、自定义组件标签、属性名、属性值的 hover，走静态
    metadata 路径。
  - **class**：静态类名提取、指定偏移的类名识别、class hover、class 补全、
    class 定义跳转，其中提取与识别走 WXML 解析，hover/补全/定义走样式索引。
- 每个 case 固定输入（fixture 文件、字节偏移、请求种类、缓存状态），组名采用
  `query/<路径>/<变体>`，可用 `--bench` 过滤，输出可直接与 Criterion baseline
  比较。
- 双模式测量：**预热查询**为主指标（项目与会话已就绪、索引已建），**首次查询**
  单独报告（新会话；明确包含发现/建索引/打开文档成本）。
- 结果正确性前置校验：变量类型必须返回具体类型、标签 hover 必须命中预期文档、
  class 查询必须命中预期类名；返回空结果、降级或 `Unavailable` 视为失败，绝不
  把“没有答案”记成快。
- 真实引擎门槛：类型路径需要 `TSGO_BIN` 与 `TSGO_TYPINGS_ROOT`；缺失时打印
  跳过原因并跳过该组，不产生伪造数据。
- 首版只做手工运行入口（`cargo bench -p wxml-lsp --bench queries`），不加入
  耗时敏感的 CI 门禁；fixture 复用 `tests/fixtures/` 现有项目，运行产物写到仓库
  外。

## Capabilities

### New Capabilities

- `query-benchmarks`: 按查询路径组织的 WXML 查询耗时基准，定义测量范围、缓存
  模式、正确性前置条件和真实引擎门槛。

### Modified Capabilities

（无：纯新增测量工具，不改变现有服务器行为。）

## Impact

- `crates/wxml-lsp/Cargo.toml`：新增 `criterion` dev-dependency 与 `[[bench]]` 目标。
- 新增 `crates/wxml-lsp/benches/`：harness、场景表、fixture 定位与结果校验。
- 可能需要一个小小的 bench 可见入口（例如按 feature 门控的查询驱动函数），首选
  直接走公开的 LSP 入口，不为此扩大生产 `pub(crate)` 面。
- 不改动 LSP 协议行为；新增 `docs/reference/` 说明如何运行与解读基线。
