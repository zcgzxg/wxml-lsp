# Proposal

## Why

小修复的提交经常新增数百行，但其中真正改变运行逻辑的只有一百多行；更贵的是改一个行为会牵连无关模块和无关测试。抽查最近提交与源码后，确认了一批可证的重复与不可达兜底：模板级 WXS 模块名被复制进每个表达式片段、一次脚本分析对同一份源码做两次扫描、同类 UTF-16/字符串 helper 各写两份、以及多项目入口用“当前活动 app”解析另一个 app 的路径。同一批代码里也有必须保留的输入校验与预算截断，本变更不碰它们。

## What Changes

- 将模板级 WXS 模块名从“每个 `FragmentSpec` 各带一份”改为“一次 shadow 请求持有一份”，片段只保留自身局部变量、`wx:for` 绑定与守卫；渲染端保留对非法名/保留名的校验。
- 让 `properties::build` 的 `data` 与 `properties` 两个提取步骤复用一次 `Script` 扫描，不再各调用一次 `Script::new(file.text())`。
- 合并后端内重复 helper：`utf16_len` 两份、`utf16_slice` 两处、`push_reason`、`dedupe`、`cap_signature`；保留既有公开名字与测试引用的常量。
- 删除不可达兜底：`summarize_wxs` 的双层默认值、`basename` 的 `rsplit(...).next().unwrap_or`、`i32::try_from(delta).unwrap_or(0)` 这类会静默产出错误偏移的降级。
- 复用 `style_diagnostics` 已取得的样式上下文，去掉重复的 `style_context_in`/`style_facts_for` 解析，并删除可证的重复外部类名检查。
- `ProjectState::component_properties` 按传入 `wxml` 定位所属项目，而不是使用当前活动项目；两个 app 存在同名模板时不再串用组件表。
- 删除 `wxml-project` 中零调用、语义不同的 `Project::is_style_path`；合并两份 `path_within`。
- 收敛测试搭建：无关测试不再为新增模板级字段填空值，复用已有构造方法/最小夹具；不新增通用 builder，不削减能证明行为的断言。

不改变用户可见的语言功能。`component_properties` 的修正只影响此前未被生产路径使用的多 app 潜伏路径。

## Capabilities

### New Capabilities

无。本变更是纯内部重构与不可达分支清理，不改变任何已登记 capability 的需求或场景。

### Modified Capabilities

无。

> 说明：`openspec/changes/trim-redundant-plumbing/.openspec.yaml` 设置 `skip_specs: true`。多 app 路由由 ADR-0006/0010 约束，属架构不变量而非 capability 需求；本次只让实现符合既有契约。

## Impact

- 代码：`crates/wxml-types/src/wxs.rs`、`crates/wxml-backend-tsgo/src/{shadow,native,script}`、`crates/wxml-lsp/src/{project_host.rs,backend_tsgo}`、`crates/wxml-project/src/{styles.rs,resources.rs}`，以及相关测试与测试夹具。
- 公开 API：`FragmentSpec` 与 shadow 构建入口的字段形态变化属内部 crate 契约；若必须保留兼容，则保留字段但不再逐片段复制。`Project::is_style_path` 无调用者，直接删除。
- 依赖与资产：无新增依赖，`assets.lock.json` 与引擎 pin 不变。
- 验证：根 workspace 的 fmt/clippy/test/doc；涉及模板 shadow 的用例需在默认测试下保持全绿，真实引擎行为不在本变更中断言。
