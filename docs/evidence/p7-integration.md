# P7 真实集成证据

日期：2026-11-15。范围：`crates/wxml-lsp/**`（P7-1..P7-4）。未修改 P3/P5/P6、根
Cargo/lock、CI、fixtures、`.opencode` 或 ADR。

## 1. 实现

| 阶段 | 内容 |
| --- | --- |
| P7-1 | `ProjectHost`/`ProjectState`：`rootUri`、真实 `FileId`、page/component 视图、脚本/JSON overlay、WXML snapshot、`ViewRevision`、C1 compilerOptions 过滤、`PreparedView`/syntax diagnostics |
| P7-2 | `NodeBackend`：`NodeTransport`、`StdioNode`（NDJSON、ping 校验、8 MiB 行上限、超时 kill、stderr 前缀日志）、缓存、epoch、失败重启 |
| P7-3 | C1.1 请求/响应：target/locals、wxmlRange、诊断范围校验、syntax 保留、hover target/quality、definition fileId→URI、revision 校验 |
| P7-4 | `--mock` 保留、默认 `--stdio` 接 `ProjectState`+`NodeBackend`、环境变量、`run_stdio_with_host`/`run_with_host`、测试与本文档 |

`run_stdio(backend)` 保持兼容，内部使用 `NullHost`；现有 LSP 骨架测试改用
`--stdio --mock`。

## 2. 覆盖矩阵（规格 §8 的 16 项）

| # | 项目 | 测试 |
| --- | --- | --- |
| 1 | URI 往返/拒绝 | `tests/project_host.rs` `routes_and_file_ids_are_real_and_stable` |
| 2 | 真实 FileId / project id 不泄露路径 | 同上、`root_and_uri_helpers_do_not_expose_host_paths_in_ids` |
| 3 | 脚本 overlay | `script_overlays_are_visible_and_mark_views_dirty` |
| 4 | 无项目降级 | `no_project_degrades_explicitly`、`tests/node_backend.rs::no_project_view_degrades_without_starting_node` |
| 5 | 未知 owner/无脚本 | `malformed_and_unknown_templates_still_produce_syntax_diagnostics`、`node_failure_keeps_syntax_and_advances_the_epoch` |
| 6 | compilerOptions 过滤/baseUrl/types | `prepares_files_fragments_and_filtered_compiler_options`、`filters_unknown_compiler_options_and_merges_typings` |
| 7 | 零片段不启动 Node | `zero_fragments_do_not_start_node` |
| 8 | 一次 analyze 服务三能力 | `one_analysis_serves_all_three_capabilities`、`tests/node_backend.rs` 同名测试 |
| 9 | 缓存/epoch/崩溃重启 | `node_failure_keeps_syntax_and_advances_the_epoch`、`a_second_revision_requests_a_new_analysis` |
| 10 | ping 校验 | `backend_node::protocol::tests::validates_ping_version` |
| 11 | NDJSON 行/id 匹配 | `backend_node::protocol::tests::parses_results_and_rejects_mismatched_ids`、`builds_ndjson_requests_with_string_ids` |
| 12 | hover target/quality/range | `hover_only_answers_inside_the_target_token`、`tests/lsp_p7.rs` |
| 13 | 诊断精确范围 + 语法保留 | `diagnostics_keep_syntax_and_drop_unattributable_or_invalid_ranges`、`syntax_is_preserved_when_node_fails`、`tests/lsp_p7.rs` |
| 14 | 跨文件 definition URI | `definition_maps_file_ids_through_the_host`、`tests/lsp_p7.rs` |
| 15 | 真实 Node 冒烟 / G1 | `tests/real_node.rs`、`tests/stdio_type_loop.rs`（`#[ignore]`，`WXML_TEST_NODE=1`） |
| 16 | locals Partial / dropped definition / revision 屏障 | `locals_make_every_capability_partial`、`dropped_definition_is_partial_and_has_no_location`、`revision_mismatch_invalidates_the_cache_and_reports_protocol` |

## 3. 验证命令

```bash
cargo fmt -p wxml-lsp -- --check
cargo clippy -p wxml-lsp --all-targets -- -D warnings
cargo test -p wxml-lsp
RUSTDOCFLAGS="-D warnings" cargo doc -p wxml-lsp --no-deps
```

结果：见交付报告；真实 Node 冒烟与 G1 验收需先 `npm ci && npm run build`，命令见第 6 节。

## 4. 安全修复（第 2 轮 security review）

| 项 | 修复 | 测试 |
| --- | --- | --- |
| C1 工作区 RCE | 默认 server 只按 `current_exe()` 相对布局解析；`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` 必须绝对，相对值 fail-closed；`WXML_NODE` 只接受名称或绝对路径；backend_dir 从绝对 server 推导；命令加 `--`；文档说明 env 为受信配置 | `config::cwd_backend_is_never_used_as_a_default`、`absolute_environment_paths_are_accepted`、`relative_environment_paths_fail_closed`、`executable_relative_layout_is_probed`、`process::command_uses_end_of_options_and_absolute_paths` |
| H1 stdout 无界 | `sync_channel(4)` + 8 MiB 行上限；reader 背压 | `stdout_channel_is_bounded`、`stdout_reader_backpressures_large_output` |
| H2 definition 无界 | 32 个 locations 上限 + `(file, range)` 去重 + Partial；服务端每请求缓存 LineIndex | `definition_locations_are_deduplicated_and_capped`、`duplicate_definition_locations_are_dropped_and_marked_partial` |
| M1 写阻塞 | 写操作在 helper 线程执行，读写共用 deadline；超时 kill | `write_timeout_does_not_block_on_a_full_pipe` |
| M2 日志放大 | Node error message 在构造 `ProtocolError` 时用 512B 安全截断 | `protocol::node_error_messages_are_bounded` |
| M3 响应条目无界 | 解析期计数：diagnostics/fragments 超限截断并 Partial；insertions 超限拒绝 | `response::oversized_diagnostics_are_truncated_not_rejected`、`response::many_fragments_with_diagnostics_are_truncated_not_rejected`、`response::oversized_insertion_array_is_rejected` |
| M4 Markdown 注入 | 动态围栏 + documentation Markdown 转义 + hover note 转义 | `hover_escapes_markdown_and_uses_a_safe_fence`、`handlers::markdown_escape_disables_links_images_and_html`、`markdown_status_notes_escape_links_and_images` |
| M5 重启风暴 | 连续 3 次失败进入 2 s 冷却 | `repeated_failures_enter_a_cooldown` |
| M6 JSON rediscover / 读锁 | 记录延期（见下） | — |

## 5. 聚焦 MEDIUM 修复（第 3 轮）

| 项 | 修复 | 测试 | 实测（debug） |
| --- | --- | --- | --- |
| M1 Partial definition 被丢弃 | `definition_done`：`Partial` 且有 locations → 返回数组；Supported 空 → `[]`；Partial 空/Unsupported/Unavailable/Disabled → `null` | `tests/lsp_p7.rs::partial_definition_result_is_returned_as_locations`（40 条 → 32 条数组）、`duplicate_definition_result_is_deduplicated_but_not_null`（300 条重复 → 1 条数组）、`empty_definition_results_respect_supported_and_partial_status`（Supported 空 → `[]`；Partial 空 → `null`） | — |
| M-A definition 索引预算 | `MAX_DEFINITION_FILES=8`、`MAX_DEFINITION_INDEX_BYTES=64 MiB`，按 `text.len() + 8*newline_count` 估算；超预算跳过并记 `unmappable`，不无限缓存 | `definition_file_count_is_capped_at_eight`（32 个小文件 → 8）、`definition_index_budget_limits_distinct_target_files`（32×2 MiB 换行密集） | 修复前 32 locations / 4.2 s / 665 MiB RSS；修复后 3 locations / 1.2 s / 125 MiB RSS；预算耗尽后不再读取新目标 |
| M-B 响应解析峰值 | `parse_line` 直接反序列化到 `AnalyzeResponse`（无中间 `Value` 树）；计数 visitor 在解析中截断 diagnostics/fragments；`truncated` 使三能力 `Partial`；行上限 20 MiB → 8 MiB | `response::large_diagnostics_line_is_truncated_without_a_value_tree`、`response::many_fragments_with_diagnostics_are_truncated_not_rejected`、`response::legal_large_response_is_not_rejected`、`backend_node::truncated_response_is_partial_and_does_not_restart`、`process::default_line_cap_rejects_a_twelve_mebibyte_line` | 40k 诊断/约 4.9 MiB 行：经中间 `Value` 树峰值 47 MiB → 直接反序列化 15 MiB；12 MiB 行在传输层直接 `LineTooLarge` |
| LOW stderr 排空 | 超长 stderr 行后继续排空到行边界，仅首次记录 | `process::stderr_drains_after_an_oversized_line` | — |
| LOW baseUrl 逃逸 | 绝对或含 `..` 的 `baseUrl` 丢弃并标记 partial compiler options | `project_host::tests::rejects_absolute_or_parent_base_url` | — |

M6 仍延期：JSON overlay 全量 `rediscover`、`analysis` 跨 `collect_sources` 持读锁、
definition 协议线程读盘；代码注释、README 与本节均保留说明。

## 6. G1 真实端到端验收（2026-11-16，base `a6c7eef`）

真实 `wxml-lsp --stdio` 二进制 + 真实 `backends/typescript/dist/server.js`，通过
`Content-Length` JSON-RPC 管道驱动。测试文件 `crates/wxml-lsp/tests/stdio_type_loop.rs`，
共享临时项目 helper 为 `crates/wxml-lsp/tests/common/mod.rs`。

```bash
(cd backends/typescript && npm ci && npm run build)
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test stdio_type_loop -- --ignored --nocapture
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test real_node -- --ignored
```

| AC | 测试 | 结果 | 关键断言 |
| --- | --- | --- | --- |
| AC-001 页面 data + JSDoc | `ac001_page_data_and_jsdoc_hover` | PASS | `user.name` hover 含 `string` + `用户显示名`；`title` hover 含 `string` + `页面标题`；hover range 8..12 / 19..24；无诊断 |
| AC-002 字段笔误 + 修复清除 | `ac002_field_typo_diagnostic_and_fix_clear` | PASS | 唯一 error 精确落在 `nmae` 0:8..0:12，消息 `Property 'nmae' does not exist`；`didChange` 后诊断 `[]`；无 severity-2 状态诊断 |
| AC-003 中文/emoji/CRLF | `ac003_unicode_and_crlf_positions` | PASS | 第 0 行 `nmae` 13..17（emoji 计 2），第 1 行 12..16（CRLF）；两条 error；无状态诊断 |
| AC-013 组件 properties | `ac013_component_properties_hover` | PASS | 组件 WXML `{{ title }}` hover `(property) title: string`，range 9..14；P5 当前对 properties-only 组件报 severity-2 Partial（`registration has no data member`），测试显式断言该 reason，不伪 Supported |
| AC-011 未知 owner 降级 | `ac011_unknown_owner_degrades_honestly` | PASS | 发布 `type checking is unsupported: template is not a discovered page or component`；hover `result = null`；无伪造类型诊断 |

实测（Node 24.22.2，debug）：

```
WXML_TEST_NODE=1 ... --test stdio_type_loop -- --ignored --nocapture  → 5 passed (1.82 s)
WXML_TEST_NODE=1 ... --test real_node -- --ignored --nocapture        → 1 passed (0.90 s)
```

门禁行为：`#[ignore]` 表示必须显式请求真实门禁。`--ignored` 但未设置
`WXML_TEST_NODE=1` 时 5 个 G1 测试全部 FAIL（`set WXML_TEST_NODE=1 to run the
real Node G1 gate`），`real_node` 同样 FAIL；设置 env 但缺少 `dist/server.js`
或 backend 目录时也直接 FAIL。因此不会出现“5 passed (0.00s)”的假绿。

前置条件：Node 24、`backends/typescript/{node_modules,dist}`（gitignored）。测试后
可删除这些构建产物；真实门禁需重新 `npm ci && npm run build`。

## 7. 限制/延期

- `wx:for` locals 仅传名字，Node 以 `any` 绑定；含 locals 的片段三能力均为
  `Partial`（C1.1）。AC-005/006 延期。
- 非 target 标识符 hover、补全、反向编辑、增量解析/LS、watcher、多 workspace
  根、debounce 延期。
- `ProjectState` 以 8 B/行起点估算内存预算；精确 RSS 不作承诺。
- 定义目标在项目外或 `uri_for` 失败时丢弃，不返回伪造位置。超过 M-A 预算的
  目标同样丢弃；LSP definition 没有 status 字段，返回的是可映射子集。
- 真实 Node 测试依赖本机 Node 24 与已构建的 P5 `dist/server.js`。
- M6：JSON overlay 每次完整 `rediscover`；`analysis` 在 `collect_sources` 期间
  持有读锁；definition 目标文件仍在协议 worker 线程读取。SourceSet 缓存/缩短
  锁粒度延期。
- 通过 overlay 新建原本缺失的页面脚本不会自动重建 P3 视图；下一次 JSON 变更
  或重启可恢复。
- 请求写入/读取有 15 s 默认 deadline；单 worker 仍串行，阻塞调用只能在 deadline
  或 shutdown 的 2 s detach 时被终止。
- definition target 读取仍在请求线程完成；M-A 只限制单请求的文件数/字节数，
  未改为异步 I/O。
- definition 映射超预算时只记录日志并返回子集；LSP 协议没有逐结果状态字段，
  客户端无法看到 `Partial`。
