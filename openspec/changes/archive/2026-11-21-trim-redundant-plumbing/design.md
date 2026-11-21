# Design

## Context

动机见 `proposal.md`。相关约束：

- 分层固定（`docs/adr/0001`）：`wxml-backend-tsgo` 只依赖 `wxml-types`，`wxml-lsp` 依赖全部；不得反向依赖。
- Shadow 是显式的 crate 边界：`shadow` 为 `pub mod`，`FragmentSpec` 的字段可由外部构造，渲染端必须继续防御非法名。
- 多 app 路由（`docs/adr/0006`/`0010`）：路径键的访问器应自行解析所属项目；`ProjectHost` 的 trait 默认实现已隐含“路径即可定位 owner”。
- 根 workspace 默认测试不跑真实引擎，因此本次以“输入/布局/提取结果等价”为验收，不声称 native 类型行为已验证。

## Goals / Non-Goals

**Goals:**

- 去掉同一信息的多份拷贝与同一源码的重复扫描，同时保持可观察输出逐年等价。
- 让模板级信息的扩充不再要求无关测试补空字段。
- 让多 app 路径查询只依赖路径本身，不依赖“当前活动项目”这一时序状态。

**Non-Goals:**

- 不重写 shadow 构建流程，不引入缓存层、trait 抽象或通用测试 builder。
- 不删除输入校验、预算上限与截断计数（`MAX_*`、`is_binding_identifier` 过滤、`Utf16Index::offset` 边界检查）。
- 不恢复已删除的交付/迁移脚手架，不改引擎或类型资产。

## Decisions

### D1. WXS 模块名提升到 shadow 请求级

`FragmentSpec.wxs_modules` 改为由构建请求持有（例如 `ShadowRequest.wxs_modules` 或等价的一次性参数），`fragment_spec` 不再 `to_vec()`。`probe_wxs_modules` 的“跨片段并集 + 首现顺序 + 非法名过滤”语义由请求级校验承担；`effective_local_names` 仍把模块名并入每个表达式的可重写根集合。

- 为什么不直接删字段：`FragmentSpec` 是公开类型，删除字段属于跨 crate 契约变更且会让多个手写测试失效；保留字段可用 `#[deprecated]` 过渡，但本变更选择**一次性迁移全部构造点**，保持单一数据来源。
- 为什么不引入 `ShadowOptions` 结构体：一次性引入会扩大 diff；请求级参数与现有 `ShadowRequest` 字段风格一致。
- 保留：`is_binding_identifier` 与 `__wxml_` 保留名过滤仍在写入 shadow 前执行一次，防止外部调用注入 probe 文本。

### D2. 一次 `Script` 扫描服务 `data` 与 `properties`

`properties::build` 内先 `Script::new(file.text())` 一次，把 `&Script` 传给 `build_data`/`build_properties`；`Script::new` 失败时两个 overlay 都走既有的 `*_partial` 降级路径，保持“scan/select 失败返回 Err、overlay 失败降级 Partial”的差异。不把 `Script` 上提到 `analyze_script`：那会改变更多函数的签名与生命周期，收益未经测量。

- 备选：模块级 `OnceCell` 缓存 → 引入隐式状态，且同一请求内 `SourceFile` 版本可能不同，放弃。

### D3. helper 合并位置

- `utf16_len`/`utf16_slice` 收敛到 `native/strings.rs`（UTF-16 位置的唯一归属）；`shadow/mapping.rs` 通过 `pub use` 重导出，保持 `tests/shadow_mapping.rs` 的导入路径。
- `push_reason`/`dedupe` 复用 `script/symbol_facts.rs` 的 `pub(crate)` 实现。
- `cap_signature` 与 `MAX_SIGNATURE_*` 只留一份；`events::MAX_SIGNATURE_LENGTH` 作为重导出保留名字，因为集成测试直接引用它。
- `path_within` 收敛到 `files.rs` 的实现（含 Windows 归一化分支），`resources.rs` 改为调用。

### D4. 不可达兜底的判定标准

只删除**能证明输入域内不可达**的分支：`limit <= text.len()` 且 `0` 必为字符边界；`rsplit('/').next()` 必有值；`len_utf16()` 只可能是 1 或 2。对“转换失败即静默给 0/MAX”的分支，改为可证明不溢出的转换或显式错误，而不是换一个默认值。反之，来自 LSP 客户端、原生引擎 AST、外部 `FragmentSpec` 的输入一律保留校验。

### D5. 样式诊断复用上下文

`style_diagnostics` 已拿到 `(project, …)` 后直接调用 `style_facts_in(&project, &path)`，不再经 `style_facts_for` 二次解析。末尾 `facts.complete && !has_external_class_reference(text)` 中的第二个条件删除，因为 `style_facts_in` 在 `~class`/`^class` 时已把 `complete` 置 false；删除前用现有 style 测试确认两个条件的输入是同一份快照。

### D6. `component_properties` 按路径定位 owner

先尝试 `ensure_view_project(wxml)` 得到所属项目；找不到时返回 `None`，不回退到活动项目。这样与 `view_components`、`style_facts`、`resource_*` 的既有写法和 trait 默认实现的隐含契约一致。

### D7. 测试搭建收敛

新增一个或多个**局部** fixture：在已有最小 `PreparedView`/`FragmentSpec` 构造上只覆盖与断言相关的字段，其余字段用共享默认值。判定标准：以后新增请求级字段时，`semantics/event_definition.rs`、`backend_tsgo/hover/tests.rs` 等无关测试无需修改。禁止引入带大量可选参数的通用 builder。

## Risks / Trade-offs

- **WXS 名称语义回归（部分片段遗漏模块名）** → 请求级 union 必须覆盖普通片段、补全片段与事件片段；用现有 `prepared_view_collects_deduplicated_wxs_module_names` 与补全/事件 shadow 测试锁定。
- **`Script` 复用把一次失败放大成两类 overlay 失败** → 仅把 `Script::new` 的 `Err` 映射到两个既有降级路径，不改 `scan`/`select` 的 `Err` 传播。
- **`MAX_SIGNATURE_LENGTH` 重导出破坏导入路径** → 保留同名 `pub use`，运行 `--test script_events` 验证。
- **`component_properties` 行为变化影响既有测试** → 现有多 app 夹具补一条“同名模板、非活动 app”断言；若无夹具则新增最小两 app 夹具。
- **删除 `has_external_class_reference` 复核后漏掉边界输入** → 仅在能指出 `style_facts_in` 覆盖同一输入时删除；否则保留并只做上下文复用。
- **一次提交过大难以回滚** → 按任务分组提交，每组独立通过 `cargo test -p <crate>`。
- **与 `master` 后续 fix 冲突** → 工作开始前确认 HEAD 与 OpenSpec 状态，冲突时先 rebase 再改。

## Migration Plan

1. 按任务顺序实施：types/兜底 → backend helper 与扫描 → shadow 请求级 WXS → lsp 路由与样式 → 测试搭建。
2. 每组完成后运行对应 crate 的 `cargo test`，最后跑全量 fmt/clippy/test/doc。
3. 回滚：按提交粒度 `git revert`；无数据迁移、无配置迁移。

## Open Questions

无。范围已由抽查证据界定；未测的延迟收益不作为验收条件。
