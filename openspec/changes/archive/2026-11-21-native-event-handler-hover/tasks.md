# Tasks

## 1. Shadow 成员片段

- [x] 1.1 在 `wxml-backend-tsgo::shadow` 的片段计划中增加成员片段模式：挂载 `this.<name>`，`this.` 为合成段，仅名称段映射回 WXML；验证 `cargo test -p wxml-backend-tsgo shadow` 通过，并新增断言 derived 文本与目标偏移的单元测试
- [x] 1.2 为成员片段补充映射测试：handler 名中的每个 UTF-16 位置可映射回原 WXML 范围，片段外位置不映射；验证 `cargo test -p wxml-backend-tsgo` 通过
- [x] 1.3 更新 shadow 模块文档注释，说明成员片段与表达式片段的差异；验证 `cargo doc -p wxml-backend-tsgo --no-deps` 无新增 warning

## 2. 按点 shadow 计划

- [x] 2.1 在 `crates/wxml-lsp/src/backend_tsgo/input.rs` 增加按 offset 构建的计划：仅当 offset 落在 `PreparedView.events` 的某个 `handler_range` 内时追加一个成员片段；验证单元测试覆盖“命中 handler / 未命中 / 动态 handler”三种情况
- [x] 2.2 在 `TsgoBackendAdapter` 中让 hover/definition 使用按点计划，其余查询保持原计划；验证 `cargo test -p wxml-lsp backend_tsgo::hover` 与 definition 相关测试通过
- [x] 2.3 更新 `capabilities()` 的 hover/definition 说明，移除“事件 handler 由静态层提供”的旧描述；验证 `cargo test -p wxml-lsp` 中 capability 断言通过且 `grep` 不再出现旧文案

## 3. 路由与清理

- [x] 3.1 从 `handlers/hover.rs` 移除 `local_event_hover` 与 `local_event_definition` 路由和实现，让事件值进入 shadow 查询；验证事件 handler hover/definition 的单元测试按新行为通过
- [x] 3.2 删除仅被静态事件 hover/definition 使用的 API（`resolve`、`resolve_handler`、`event_site`、`EventHandlerFacts`、`EventHandlerResolution`、`event_hover_contents` 及相关常量/测试）；`EventValueSite` 保留给事件诊断使用；验证 `cargo clippy -p wxml-lsp --all-targets --all-features -- -D warnings` 无未使用代码告警
- [x] 3.3 确认静态 `analyze`、`event_handler_sites` 与 `TsconfigPaths` resolver 仍服务事件诊断和 handler 补全；验证 `cargo test -p wxml-lsp semantics::event_definition` 与 `handlers::diagnostics` 通过

## 4. 集成验证

- [x] 4.1 增加真实引擎测试：`Component` 通过 tsconfig 别名引入 behavior 时 hover 含类型与 JSDoc、definition 指向 behavior 声明；相对路径 behavior 同样通过；验证在设置 `TSGO_BIN`/`TSGO_TYPINGS_ROOT`（或安装原生布局）后测试通过
- [x] 4.2 增加真实引擎测试：`Page` 自身方法可 hover/definition；`Page` 仅由 behavior 提供 handler 时 hover/definition 为空；验证同一引擎环境下测试通过
- [x] 4.3 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`、`cargo doc --workspace --no-deps`，确认全绿（默认特性 2331 passed；设置真实引擎环境时 `backend_tsgo::environment::gate` 的 3 个 `ProjectMismatch` 失败在 HEAD 上同样存在，与本变更无关）
- [x] 4.4 用 stdio 客户端对 `apps/miniprogram/src/checkin/pages/other-record/index.wxml` 复测 `onBackTap` hover 与 definition，确认非空且指向 `BasePage.ts`；验证脚本放在仓库外（hover 含 `导航栏上的后退或Home按钮点击时触发`，definition 指向 `apps/miniprogram/src/behaviors/BasePage.ts`）

## Workflow follow-up

- 归档 change：`openspec archive native-event-handler-hover`，并验证归档后的主 spec 包含新的处理函数 hover requirement。
- 提交 git：使用 Conventional Commits（例如 `feat(hover): 事件处理函数 hover 改用原生类型查询`），包含实现、测试与 OpenSpec 归档结果。
