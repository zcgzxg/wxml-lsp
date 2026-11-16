# W2 集成与 Q1 本地交付证据

更新日期：2026-11-17。

本记录保存 W2（组件/样式/资源/编辑/类型语义）和 Q1（本地交付）在集成分支上的验收快照。它不把“测试通过”扩大为“所有编辑器 UI 和平台均已验证”；S 语义工具、Q2 性能/恢复和 macOS/Windows 实机仍不在本记录范围。

## 1. 基线

| 项 | 值 |
| --- | --- |
| W2 HEAD | `a441de9`（`merge: integrate type semantics for behavior templates and WXS`） |
| Q1 HEAD | `f46dec9`（`merge: integrate local delivery installation and packaging`） |
| 分支 | `feat/full-integration` |
| 已合并工作包 | P、E、C、N、T；随后 Q1 |
| 未合并 | S 语义工具；Q2 性能/恢复与最终 Linux UI 验收 |
| 环境 | Linux x86_64；Cargo 1.100.0；Node 24.22.2；测试期间宿主负载较高 |

W2 合并的提交包括显式多 app 宿主、依赖失效、组件智能、样式/资源索引、结构编辑/格式化、Behavior/template/WXS 静态语义和跨特性 JSON-RPC 矩阵。Q1 增加 `crates/wxml-delivery`、`scripts/build-release.sh` 和 [ADR-0007](../adr/0007-local-delivery-trust-model.md)。

## 2. 测试命令与结果

### 2.1 W2 验收日志

以下结果来自 W2 的固定验收日志；不同运行环境的计数不混用。

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| Rust workspace | `cargo test --workspace` | `1095 passed / 20 ignored` |
| Node 后端 | `cd backends/typescript && npm test` | `304 / 305`；唯一失败是既有 perf 墙钟断言在高负载下超时，单独重跑通过 |
| Zed 扩展 | `cd editors/zed && cargo test --locked` | `25 passed` |
| G1 真实 Node | `WXML_TEST_NODE=1 cargo test -p wxml-lsp --test stdio_type_loop -- --ignored` | `9 passed` |
| G1 真实 Node | `WXML_TEST_NODE=1 cargo test -p wxml-lsp --test real_node -- --ignored` | `4 passed` |
| Wave 5 真实 Node | `env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR WXML_TEST_NODE=1 cargo test -p wxml-lsp --test wave5_acceptance -- --ignored` | `7 passed` |

说明：

- 20 个 ignored 就是 9 + 4 + 7 个真实 Node 门禁；未设置 `WXML_TEST_NODE=1` 或缺少 `backends/typescript/dist/server.js` 时默认不运行。
- `WXML_TYPE_BACKEND` 指向其他 checkout 时，`wave5_acceptance` 会拒绝运行；上面的命令显式清除外部后端变量，确保测试的是当前 worktree 的 `dist`。
- Node 全量测试中的 perf 墙钟断言对宿主负载敏感，单独运行 `perf`/`rewrite` 用例通过；这不改变功能断言结果。

### 2.2 Q1 本地交付

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 交付 crate 测试 | `cargo test -p wxml-delivery` | 通过（安装、升级/回退、缓存、归档校验、构建脚本测试） |
| 构建脚本 dry-run | `scripts/build-release.sh --dry-run --output "$HOME/wxml-release"` | 通过；打印计划，不写文件、不联网、不发布 |
| 格式/静态检查 | `cargo fmt --all -- --check`；`cargo clippy --workspace --all-targets -- -D warnings` | 通过 |
| Node 类型检查 | `cd backends/typescript && npm run check` | 通过 |

Q1 测试覆盖的关键行为：

- `installs_verifies_and_reports_a_path_with_spaces`、`installs_from_a_local_release_directory`：路径含空格和本地目录安装。
- `upgrades_and_rolls_back_to_a_verified_version`、`offline_rollback_can_restore_a_verified_cached_version`：升级、回退和离线缓存。
- `explicit_binary_wins_and_invalid_values_never_download`：`WXML_LSP_BIN` 有效时优先，无效时明确失败且不回退下载。
- `download_requires_explicit_enablement`、`rejects_untrusted_plain_http_sources`、`project_configuration_cannot_select_a_download_source`：下载默认关闭、仅可信 HTTPS/localhost、项目配置不能指定源。
- `verify_does_not_execute_a_tampered_binary`、`rejects_a_bad_archive_hash`、`rejects_a_local_archive_symlink`：哈希/归档/链接校验先于执行。
- `dry_run_does_not_write_and_publish_is_rejected`：`--publish` 等发布参数在构建前失败。

本次没有执行完整的 `scripts/build-release.sh` 全量 release 构建，也没有做 marketplace 上传、远端 tag/release 或发布者签名；`--dry-run` 只验证计划和前置条件。

### 2.3 集成分支合并 Q1 后

合并 Q1 后的全量 Rust 运行以最终验收日志为准。当前已确认一处既有 5 秒墙钟断言在高负载下超时，单独重跑通过；不把该超时解释为功能失败，也不把不同次运行的通过/失败计数混合。Q1 自身的 `cargo test -p wxml-delivery` 在上述运行中通过。

## 3. 交叉特性矩阵

以下用例位于 `crates/wxml-lsp/tests/`，均通过真实 stdio JSON-RPC 驱动，不只检查 JSON 形状。

| ID | 测试 | 覆盖 | 仍有限制 |
| --- | --- | --- | --- |
| IT-01 | `full_feature_matrix.rs::it_01_legacy_page_and_component_queries_survive_new_capabilities` | 新 capability 全开时，既有 Page/Component hover、definition、completion、diagnostics 分派不被覆盖 | 仍是既有样本子集，不等于全部历史写法 |
| IT-02 | `full_feature_matrix.rs::it_02_component_resolution_is_shared_by_completion_hover_and_definition` | page/component 自身与 app `usingComponents`、属性 hover、组件 definition 共用同一解析结果；本地覆盖优先 | 插件/远程组件仅在有本地可信元数据时建模 |
| IT-03 | `full_feature_matrix.rs::it_03_picker_mode_completion_and_diagnostics_stay_in_sync` | `picker` mode 改变后补全与条件诊断同步更新；不重复已存在属性 | 条件规则目前只覆盖 `picker`、`input` 等已登记表项 |
| IT-05 | `full_feature_matrix.rs::it_05_two_explicit_nested_apps_do_not_cross_roots` | 两个显式嵌套 app 各自使用 tsconfig、路径和样式，查询不串根 | 单 workspace 内多 app；不是完整多 workspace |
| IT-09 | `full_feature_matrix_editing.rs::it_09_style_classes_follow_imports_globals_and_ignore_comments` | WXSS/SCSS `@import`/全局样式驱动 class 补全、hover、definition；注释 selector 不算定义 | 动态插值、mixin/`@extend` 生成类不猜测；截断时标记不完整 |
| IT-10 | `full_feature_matrix_editing.rs::it_10_document_links_and_resource_diagnostics_track_disk_changes` | 资源补全/documentLink/definition/存在性诊断随新增、删除和 watched-file 更新；远程值不变成可点击的本地目标 | 仅本地 `app.json` 根内资源；不访问远程 URL，不处理插件包内未落盘资源 |
| IT-11 | `full_feature_matrix_editing.rs::it_11_formatting_is_lossless_idempotent_and_reparses` | document/range formatting 保留 CRLF、中文、emoji、Mustache、WXS rawtext，幂等且应用后可重新解析 | 只规范开始标签内部空白；不重排文本节点或整份缩进 |
| IT-12 | `full_feature_matrix_editing.rs::it_12_closing_tag_completion_linked_editing_and_rename_prep` | `</` 栈补全、标签 snippets、linkedEditingRange 对嵌套同名兄弟和自闭合元素精确 | `rename`/`prepareRename` 未实现；这里的“rename prep”只是客户端可用的 linked ranges |

未覆盖或未完成：IT-04/06/07/08 中依赖 S 的引用/重命名/反向编辑部分、IT-13（semanticTokens）、IT-14/15/18（Q2 性能/恢复）。IT-17 的本地打包/安装/回退子集由 Q1 覆盖，但完整 release dry-run、下载/校验矩阵和版本升级场景不在本记录执行。它们不能因本记录中的相邻用例通过而视为完成。

## 4. 能力边界

### 4.1 已实现

- 显式 `initializationOptions.apps` 多 app；`root`/`projectRoot` 分层；单 workspace 最长目录匹配；非法配置 `-32602`；无自动扫描/根热更新（ADR-0006）。
- 每个 app 独立 `projectId`、tsconfig、overlay、依赖索引、缓存和重启身份；已知依赖失效，未知来源只刷新所属 app；共享文件 active/唯一 owner，多 owner 无 active 时降级。
- 组件智能：页面/组件自身/app `usingComponents`、本地覆盖、组件 props/事件/静态 hover、组件与属性 definition、`picker`/`input` mode 条件、插件/包降级。
- 样式/资源：WXSS/SCSS/CSS class、`@import`/`@use`/`@forward`、app 样式和显式全局样式、`styleIsolation`、class 补全/hover/definition、资源补全/documentLink/definition/存在性诊断，以及有界扫描。
- 编辑/结构：保真 document/range formatting、标签 snippets、linked editing、folding、selection range、documentSymbol、Zed outline；内置/工作区 snippets。
- 类型语义：静态 Behavior 图、共享 data/properties、template/import/include、WXS 静态子集、JS/JSDoc、guards/loops、未保存跨文件 overlay、复杂 props/`required`/`optionalTypes`。

### 4.2 明确降级或未实现

- 动态 template `is`、动态 WXS 导出、动态 Behavior 工厂/无法解析的导入、未知 spread、动态 class 插值和无法证明的 SCSS 生成内容：给出原因或保持不完整，不猜测。
- 插件/远程组件缺少本地可信元数据时只给降级说明；不下载、不执行包源码。
- 模板体不按每次调用实例做完整类型检查；WXS 是静态子集，不执行源码，也不冒充完整 JS 标准库。
- S 语义工具（references、prepareRename/rename、codeAction、semanticTokens、workspaceSymbol、inlayHint、signatureHelp、codeLens）未实现。
- Q2 性能/取消/恢复完善、大项目预算、最终 Linux UI 验收未完成。
- Q1 已实现本地打包/安装/升级/回退和可选显式 HTTPS 下载，但没有 marketplace、远端 release、自动更新或发布者签名；信任链为 TLS + checksum，TLS/CA/本地 bundle 被攻破时仍可自洽。详见 ADR-0007。
- 没有 macOS/Windows 实机验证；Zed 的部分 UI 交互（连续键入延迟、跨文件未保存传播、所有编辑预览/撤销）未逐项验收。

## 5. 相关记录

- [ADR-0006：显式小程序根与类型项目根](../adr/0006-explicit-mini-program-roots.md)
- [ADR-0007：本地交付与安装信任模型](../adr/0007-local-delivery-trust-model.md)
- [wxml-delivery README](../../crates/wxml-delivery/README.md)
- [P8 Zed 扩展证据](p8-zed.md)
- [WXML 语言功能基线](../reference/wxml-language-feature-baseline.md)
