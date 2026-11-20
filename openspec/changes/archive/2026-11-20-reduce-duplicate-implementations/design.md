# Design

## Context

见 `proposal.md` 的 Why。当前状态与约束：

- **成员扫描两份**：`crates/wxml-backend-tsgo/src/script/scanner.rs` 与
  `crates/wxml-backend-tsgo/src/shadow/layout.rs` 各自定义 `Script`、`Member`、`MemberKind`
  与 `scan_members`/`member_end`/`parse_member`/`function_keyword_before`/`method_name_before`。
  两侧 `MemberKind` 变体完全相同，但 `parse_member` 行为不同：scanner 识别 `*name()`、`async name()`
  和 `get`/`set` 访问器并保留名称，layout 对 `*` 返回无名 `Method`、对 `async` 落到 `Unknown`。
  除扫描器外，layout 的 `Script` 少 `utf16` 与 `depths` 两个字段。
- **测试临时目录 19 处**：`wxml-project` 的 6 处（`discovery.rs`、`config.rs`、`files.rs`、
  `sources.rs`、`tests/{project,p3b,styles}.rs`）以 `std::env::temp_dir()` 加进程号与
  `AtomicU64` 拼名；`wxml-lsp` 侧（`tests/common/mod.rs`、`cli.rs`、`project_host.rs`、
  `styles_resources.rs`、`full_feature_matrix/support.rs`、`backend_tsgo/{engine,sandbox}.rs`、
  `environment/{gate,fakes}.rs`、`native_lsp_document_symbols/support.rs`）另有各自命名与 `Drop`。
- **LSP 集成测试工具 4 份重复**：`tests/{lsp,component_intelligence,structural_editing,semantic_surface}.rs`
  各有一份 `read_frame` 与 `wait_with_timeout`，实现漂移（帧大小上限、超时后的回收方式不同）。
- **约束**：仓库内不得留临时文件（ADR-0004/0005）；发布路径不新增依赖；`editors/zed/` 是独立
  WASM workspace，根 cargo 命令不覆盖，本变更不涉及。

## Goals / Non-Goals

**Goals:**

- 同类逻辑在仓库中只有一份实现，且删除后仍能通过现有测试说明行为未变。
- 测试代码减少"目录命名 + 清理"这类与业务无关的样板。
- 成员扫描的差异从"两份拷贝各自演化"变成"一份实现 + 明确的策略参数"。

**Non-Goals:**

- 不替换手写 TS lexer，不引入 `tree-sitter`/`swc`/`oxc`；`scan_members` 只做第一层对象成员切分。
- 不引入 `cssparser` 重写样式索引（见 proposal）。
- 不改 shadow 生成算法、handler 分层、协议输出或诊断文本。

## Decisions

### D1：`tempfile` 只作为 dev-dependency，父目录走 `TMPDIR` 或 `/tmp/opencode`

`tempfile` 已在 `Cargo.lock` 中作为传递依赖存在，加为 dev-dependency 不会扩大发布树。所有
`TempDir` 改由 `tempfile::tempdir_in(base)` 创建，`base` 取 `TMPDIR`，缺省 `/tmp/opencode`
（沿用 `tests/common/mod.rs` 现有约定，也满足"不在仓库内产生临时文件"）。

- 替代方案 A：继续自写 `TempDir` + 一个宏收敛。仍要维护 `Drop`、命名冲突与 panic 清理，收益有限。
- 替代方案 B：放入普通 `[dependencies]`。无生产代码需要它，违反"第一个消费者才加入"的约定。

### D2：保留"项目结构搭建"辅助，只委托目录生命周期

`TempProject`（`tests/common/mod.rs`、`full_feature_matrix/support.rs`、`styles_resources.rs`）
继续提供 `write`/`root`/`uri`，内部持有一个 `tempfile::TempDir`。这样测试里不会同时出现
`tempfile` 与旧 API 两套写法，删除的只是目录名生成与 `Drop`。

### D3：LSP 测试工具并入已有的 `tests/common/mod.rs`

`read_frame` 与 `wait_with_timeout` 以 `pub(super)` 放入 `common`，四个测试文件删掉本地副本。
统一版本取现有实现中最严格的一份：

- `read_frame`：带 `MAX_HEADER_BYTES`/`MAX_MESSAGE_BYTES` 上限、`Content-Length` 校验、循环读头部；
- `wait_with_timeout`：超时后 `kill` 再 `wait` 回收，返回 `ExitStatus`。

- 替代方案：引入 `lsp-server`/`tower-lsp` 的测试工具。它们绑定了自己的类型体系，而这里只需
  读裸 JSON-RPC 帧并断言原始字段，依赖成本大于收益。

### D4：成员扫描合并到 `script/scanner.rs`，阴影侧复用

`Member`/`MemberKind`/`scan_members`/`member_end`/`parse_member`/`function_keyword_before`/
`method_name_before` 保留 `script/scanner.rs` 这一份，`shadow/layout.rs` 删除副本并 `use` 之；
layout 私有的 `Member::key_range` 与 scanner 的 `to_range` 合并为一个实现。

分类语义以 scanner 为准（更完整）：阴影侧此前落到 `Unknown` 的 `async name()`/`*name()`/
`get name()` 成员会得到正确 kind 与名称。

- 验收方式：先在测试里保留旧 `parse_member`（`#[cfg(test)]` 一份冻结副本），对全部 fixture 的
  成员列表做新旧对比，输出差异集合；确认差异只出现在"更准确"的方向后，更新受影响的期望并删除旧副本。
  若出现无法解释的映射变化，退回到"一份实现 + `Mode` 参数"（保留 layout 旧分类）。
- 替代方案：直接复制 scanner 的语义先跑测试。缺少差异可视化，失败时难以判断是回归还是改进。

### D5：逐字相同的小函数收敛到既有归属

- `export_target` → 保留 `crates/wxml-lsp/src/configuration.rs` 的 `pub(crate)` 版本
  （已有 `depth > 4` 语义与测试），`workspace_packages.rs` 引用它。
- `hex_lower` → 保留 `crates/wxml-backend-tsgo/src/input.rs` 的 `pub(crate)` 版本，
  `native/supervisor.rs` 与 `native/vfs.rs` 引用它。
- 替代方案：为十几行引入 `hex` crate。增加依赖而删除量不变，且 `hex_lower` 有意避免额外依赖
  （`input.rs` 注释已说明）。

## Risks / Trade-offs

- [成员分类变化改变 shadow 映射] → 先做新旧对比测试，把差异收敛成明确列表；无法解释的变化退回 `Mode` 参数。
- [删除临时目录时仍有引擎子进程持有句柄] → 涉及子进程的测试先 `kill` + `wait` 再 drop；必要时用
  `TempDir::close()` 并忽略错误，避免清理失败影响断言。
- [并发测试目录名冲突] → 交给 `tempfile` 的随机后缀，删除现有 `AtomicU64` 计数器。
- [统一 `read_frame` 后错误文案变化导致断言失败] → 按测试文件逐个替换并单独跑该 binary，不一次性全改。

## Migration Plan

四个步骤各自独立可验证，每步结束都要 `fmt` + `clippy -D warnings` + 全量 `test` 绿色：

1. D1 + D2：测试临时目录（改动面最大但语义最弱）。
2. D3：LSP 测试工具。
3. D5：两个小函数。
4. D4：成员扫描（风险最高，放最后，单独一轮 review）。

全部为工作区内重构，回滚即还原步骤对应的文件；无数据迁移、无发布物变化。

## 实施记录（D4 细节）

- 共享语法放在 `crates/wxml-backend-tsgo/src/script/members.rs`：`Member`/`MemberKind`、
  `scan_members`/`member_end`/`parse_member`、`object_bounds`/`member_last`、
  `function_keyword_before`/`method_name_before`、`doc_before`/`unquote`。
- 两侧的 `Script` 各自保留词法器、`utf16`/`depths` 表与错误类型；共享代码只通过
  `Members` trait 取 token（`kind_at`/`span_at`/`text_at`/`close_at`/`punct_at`/`ident_at`），
  因此不需要把 layout 的 `kind()` 改成 `Option<TokenKind>`，也没有引入 `Mode` 参数。
- `script/registration.rs` 的 `function_keyword_before`/`method_name_before` 是第三份副本，
  一并改为复用 `members`。
- 语义以 scanner 为准，layout 侧由此获得三处更准确的行为：
  - `*name() {}` 保留成员名并按 `Method` 处理（原为无名 `Method`）；
  - `async name() {}` 按 `Method` 处理（原为 `Unknown`，名称为 `async`）；
  - `get name()`/`set name(v)` 的 key 范围指向属性名（原指向 `get`/`set`），
    `value_start` 相应后移一位。
- `unquote` 采用 scanner 版本，layout 现在也能解码 `\u`/`\x` 转义；
  `doc_before` 采用带 4096 字节回看上限的版本，避免长脚本上的 O(prefix) 重扫。
- 冻结基线对比测试（临时）确认除上述四类形状外，新旧实现逐字段一致；
  删除冻结副本后保留 `layout_and_scanner_member_views_agree`，比较两套 `Script`
  视图对同一文本产出的成员列，防止两个 token 视图再次分叉。
