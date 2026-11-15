# P8 Zed 扩展证据

日期：2026-11-16。基线提交：`a6c7eef`。范围：`editors/zed/**`、本记录和
`docs/evidence/README.md` 索引。未修改根 `Cargo.toml`/`Cargo.lock`、CI、
`.opencode`、ADR、fixtures、其他 crates/backends；未 commit。
证据等级：E1（关键命令输出摘录见第 8 节；查询验证器为仓库外临时程序，
未入库）。

## 1. 结论摘要

- 已建立可审阅的 Zed 扩展骨架：`extension.toml`、独立 Rust crate、WXML 语言
  配置和四类 tree-sitter 查询、README。
- 语言服务器只接受显式 `WXML_LSP_BIN` 绝对路径；不探测 worktree、不查
  `PATH`、不执行项目脚本、不做 shell 拼接。未设置或无效时直接报错，不回退。
  `Worktree::shell_env` 可能包含已 `direnv allow` 的 `.envrc` 导出；扩展只校验
  最终环境变量，不解析 `.envrc`。允许 `.envrc` 本身已执行项目代码。
- 路径判断完全使用字符串 helper，不依赖 `wasm32-wasip2` 的 POSIX
  `std::path`；支持 POSIX、Windows 盘符和 UNC 路径。
- 扩展 crate 使用独立 `[workspace]`，通过 `zed_extension_api 0.7.0` 编译。
- 已在本机完成 host 侧 `fmt`/`check`/`clippy`/`test`、`wasm32-wasip2`
  debug/release 构建，并验证 `extension.toml`、语言 `config.toml` 可解析，
  四份查询可在 `tree-sitter-wxml` 真实语法树上编译并命中样本。
- **未**在 Zed 内安装/运行 dev extension，**未**做 `wxml-lsp`
  initialize/didOpen/诊断/悬停/定义端到端运行。本记录不声称任何 G0/G2 通过。

## 2. 环境

| 项 | 值 |
| --- | --- |
| OS | Linux（本机） |
| Zed | `Zed 1.28.1 e8bc16acee5ae1937600d571d441c4b5d7b0d288` |
| rustc | `1.100.0 (0ef1bc386 2026-11-09)` |
| cargo | `1.100.0 (3acc79c0b 2026-11-05)` |
| 已安装 target | `x86_64-unknown-linux-gnu`、`wasm32-wasip2` |
| Zed 扩展 API | `zed_extension_api 0.7.0`（crates.io，发布日 2025-09-12） |
| grammar | `BlockLune/tree-sitter-wxml` rev `1cc1761c8c11cacab5516dbe64e20ee689aa4428` |
| 查询验证器 | `tree-sitter 0.25.8` + 上述 grammar 的 Rust binding |
| Node（仅类型后端需要） | `v24.22.2` |

## 3. 实现结构

```text
editors/zed/
├── .gitignore                 # /target, /extension.wasm, /grammars/
├── Cargo.toml                 # 独立 [workspace]，crate-type = cdylib
├── Cargo.lock
├── extension.toml             # id/schema_version/grammar/language_server
├── README.md
├── src/lib.rs                 # Extension trait + 字符串路径解析
└── languages/wxml/
    ├── config.toml            # name/grammar/path_suffixes/comments/brackets
    ├── highlights.scm
    ├── brackets.scm
    ├── indents.scm
    └── injections.scm
```

当前 Zed manifest 通过 `languages/` 目录发现语言，`ExtensionManifest.languages`
是路径数组，因此没有在 `extension.toml` 中使用旧式的 `[languages.wxml]`
表；`name`/`grammar`/`path_suffixes` 等字段位于
`languages/wxml/config.toml`。`[language_servers.wxml-lsp]` 不写 `name`，
因为 Zed 1.28.1 的 manifest 结构没有这个字段，会被忽略。

## 4. 二进制解析（HIGH-1/HIGH-2 + 平台感知校验）

`editors/zed/src/lib.rs` 只读取 `WXML_LSP_BIN`，并用
`zed::current_platform().0` 选择 host 路径规则：

| host OS | 接受 | 拒绝 |
| --- | --- | --- |
| Windows | 盘符绝对 `C:\...`/`C:/...`；UNC `\\server\share...`/`//server/share...` | POSIX `/...`（包括 `/C:/...`）、`C:`、`C:foo`、缺少 server/share 的 UNC |
| Linux/macOS | `/...`（`/C:/...` 按普通 POSIX 路径接受） | 盘符路径、UNC `\\...`/`//server/share...`、相对路径 |

解析步骤：

1. `WXML_LSP_BIN` 未设置：返回错误，提示在启动 Zed 前导出绝对路径；不探测
   `<worktree>/target/...`，不查 `PATH`。
2. 空值、host 不匹配的路径形状、尾部分隔符、含 `..` 段：返回错误，不做
   `trim()`。尾部分隔符按 host 判断：Windows 拒绝 `/` 和 `\`，POSIX 只拒绝 `/`。
3. 用 Zed host 的 `Worktree::which(absolute_path)` 验证存在且可执行；`None`
   直接返回错误，不回退。
4. `which` 返回的路径再次用同一 host 校验（包括 Windows 的 `/C:/...` 拒绝）。
5. 启动参数只有 `--stdio`。扩展额外转发 shell 中存在的
   `WXML_TYPE_BACKEND`、`WXML_BACKEND_DIR`、`WXML_NODE`、
   `WXML_ANALYZE_TIMEOUT_MS`、`WXML_TEMPLATE_CHECK`；这不是脱敏边界，
   Zed 仍会合并完整 shell 环境。

路径 helper 均为纯字符串操作，不引入 `std::path`：

| helper | 行为 |
| --- | --- |
| `is_absolute_host_path(raw, os)` | Windows 分支接受盘符/UNC；Linux/macOS 分支接受 `/...` 且拒绝 UNC |
| `is_windows_drive_absolute` | `[A-Za-z]:[\\/]`，拒绝 `C:` 和 `C:foo` |
| `is_unc_path` | 接受 `\\\\server\\share...`/`//server/share...`，要求 server 和 share 两段；兼容 `\\?\C:\...` |
| `ends_with_separator(raw, os)` | Windows 检查 `/`、`\`；POSIX 只检查 `/` |
| `has_parent_dir` | 同时按 `/` 和 `\` 切分，任一段为 `..` 即拒绝；不 trim |
| `display_name` | 错误/日志只显示最后一段，不暴露完整绝对路径 |
| `resolve_binary(shell_env, os, lookup)` | `WXML_LSP_BIN` 缺失/无效/不可执行时只报错，不回退 |

`WXML_LSP_BIN` 无效时不会尝试 worktree 内 `target/release/wxml-lsp` 或
`target/debug/wxml-lsp`。这是与旧实现的关键差异，消除了“打开不可信项目即执行
项目内二进制”的路径。

错误和日志中的二进制名使用 basename；worktree 根路径不再参与解析。host 单测
显式传入 `Os`，不依赖运行测试的真实 host（`current_platform()` 在 host 测试
中会返回 Linux）。

## 5. 已验证项

| 项 | 方法 | 结果 |
| --- | --- | --- |
| API 签名 | docs.rs `zed_extension_api 0.7.0` 源码 | `language_server_command(&mut self, &LanguageServerId, &Worktree) -> Result<Command>`；`Command { command, args, env }`；`Worktree::which(&str) -> Option<String>` |
| API 版本兼容 | Zed `v1.28.1` 的 `wasm_host/wit.rs` | stable 支持 `0.6.0..=0.7.0`，选用 0.7.0 |
| 平台识别 | `zed_extension_api 0.7.0` 根导出与 docs.rs | `zed::Os::{Mac,Linux,Windows}`、`zed::current_platform() -> (Os, Architecture)` 可用 |
| manifest 字段 | Zed `v1.28.1` 的 `ExtensionManifest`/`GrammarManifestEntry` | `id`/`name`/`version`/`schema_version` 必需；`grammar.rev` 可写，`commit` 是别名；`languages` 自动发现 |
| TOML 解析 | Python 3 `tomllib` 读取三份 TOML | 全部通过 |
| grammar 可获取 | `git clone --depth 1`，`git rev-parse HEAD` | HEAD 与写入的 rev 一致；含 `src/parser.c`（ABI 15）、`src/scanner.c`、`src/node-types.json` |
| 查询语法/节点名 | `tree-sitter 0.25.8` 加载 grammar，样本解析后 `Query::new` | 四份查询全部通过；样本命中：highlights 77、brackets 27、indents 11、injections 5 |
| 路径 helper 单测 | `cargo test --locked` | 22 passed；按 `Os::Windows`/`Os::Linux`/`Os::Mac` 参数化，覆盖盘符、UNC、POSIX、`/C:/...` 的 Windows 拒绝与 POSIX 接受、混合分隔符、尾部分隔符、空格、相对路径、`..`、空值、无 `WXML_LSP_BIN`、无效路径不回退、`which` 异常返回 |
| 格式 | `cargo fmt --all -- --check` | 通过 |
| 编译 | `cargo check --all-targets --locked` | 通过 |
| wasm check | `cargo check --target wasm32-wasip2 --locked` | 通过 |
| Lint | `cargo clippy --all-targets --locked -- -D warnings` | 通过，无告警 |
| wasm debug 构建 | `cargo build --target wasm32-wasip2 --locked` | 通过；`zed_wxml.wasm` 7,633,946 B（7.3 MiB） |
| wasm release 构建 | `cargo build --target wasm32-wasip2 --release --locked` | 通过；`zed_wxml.wasm` 238,792 B（233.2 KiB） |

## 6. 未验证项

| 项 | 原因/状态 |
| --- | --- |
| Zed dev extension 安装 | 未通过 GUI 执行 `zed: install dev extension`；**2026-11-16 做过隔离用户目录的手动 smoke**：把 `extension.wasm`、语言配置和一个临时 grammar 放入 `--user-data-dir` 的 installed 目录，详见 §6.1。标准 Dev Extension 安装仍未完成 |
| Zed 内语法高亮/括号/缩进/注入 | 临时 grammar 只用于让语言注册成功，查询未按真实 WXML 语法验证；高亮/括号/缩进观感未验收 |
| grammar 的 Zed/wasi-sdk 构建 | 未在 Zed 内触发真实 `tree-sitter-wxml` 编译；只验证了 grammar 仓库、ABI 和查询 |
| `wxml-lsp` initialize | **已由 Zed 1.28.1 启动**：日志显示 `starting language server process ... args: ["--stdio"]`，工作目录为测试项目根；未逐项核对 `initializationOptions`/`workspace/configuration` |
| 诊断、悬停、定义 | 诊断已在隔离 smoke 中显示（`user.nmae` 精确下划线）；悬停/定义未完成可视化确认，依赖 G1 端到端测试 |
| Spike S1 运行时复测 | 本节结论来自 Zed 1.28.1 的 buffer 注册行为，本次未做运行时抓包复测 |
| 多 worktree/远程项目 | 未测 |
| Windows/macOS 实机 | 未测；Windows 路径只用字符串单测覆盖 |

### 6.1 隔离 smoke（非标准安装）

目的：只确认 Zed 能加载扩展 wasm、注册 WXML 并启动真实语言服务器，不把它当作标准 Dev Extension 验收。

- 使用 `zed 1.28.1 --user-data-dir /tmp/opencode/zed-user`，项目 `/tmp/opencode/zed-project`（`page-basic` 样本）；`WXML_LSP_BIN=target/release/wxml-lsp`，Node 后端指向本仓库 `backends/typescript/dist/server.js`。
- 由于未走 GUI Dev Extension 安装，手动把 release `zed_wxml.wasm`、`languages/wxml/` 和 `grammars/wxml.wasm` 放入隔离数据目录。`tree-sitter-wxml` 未在 Zed 内编译；临时 grammar 使用已安装 HTML grammar 并仅把导出名替换为 `tree_sitter_wxml`，**只能证明注册/启动链路，不证明 WXML 语法**。
- 结果：Zed 状态栏显示 `WXML`；日志出现 `starting language server process. binary path: .../target/release/wxml-lsp, working directory: /tmp/opencode/zed-project, args: ["--stdio"]`；`user.nmae` 行出现诊断下划线，状态栏显示 1 条诊断。
- 未确认：悬停内容、定义跳转、真实 grammar 高亮、标准 `zed: install dev extension` 流程。KDE 的 EIS 合成输入权限弹窗阻断了自动化鼠标/键盘，未继续强行操作。
- 结论：**语言服务器启动与诊断链路已在 Zed 内实际跑通**；G0/G2 仍只算部分通过。

## 7. Spike S1 结论与复测清单

结论（Zed 1.28.1）：Zed 只把 WXML buffer 注册给 `wxml-lsp`；
JavaScript/TypeScript/JSON 的 `didOpen`/`didChange` 不会转发给该服务器。
因此未保存的 JS 修改不会实时反映到 WXML 分析；保存后，下一次分析重新收集
脚本时会读取磁盘上的新内容并生效。不存在“必须等到 project reload”的限制。

本结论未做运行时抓包复测。后续升级 Zed 后可用以下清单复核：

- [ ] `wxml-lsp` 是否收到 `page.js` 的 `textDocument/didOpen`
      （预期：否）。
- [ ] 修改 `page.js` 但不保存时，是否收到 `didChange`（预期：否）。
- [ ] 保存 `page.js` 后，下一次 WXML 分析是否看到新内容（预期：是）。
- [ ] `page.json` 的 `didOpen`/`didChange` 是否转发（预期：否）。
- [ ] `workspace/configuration` 请求是否得到 Zed 响应。
- [ ] `initializationOptions = {}` 是否被接受，日志是否有协议错误。
- [ ] JS/TS 主语言服务是否仍正常工作（不应被本扩展影响）。

## 8. 关键输出摘录

以下为本次运行的原始输出摘录（路径已替换为占位符）：

```text
$ cargo fmt --all -- --check
# no output, exit 0

$ cargo check --all-targets --locked
cargo check (1 crates compiled)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s

$ cargo clippy --all-targets --locked -- -D warnings
cargo clippy: No issues found

$ cargo test --locked
cargo test: 22 passed (1 suite, 0.00s)

$ cargo check --target wasm32-wasip2 --locked
cargo check (1 crates compiled)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s

$ cargo build --target wasm32-wasip2 --locked
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s

$ cargo build --target wasm32-wasip2 --release --locked
Finished `release` profile [optimized] target(s) in 0.39s
```

```text
debug/zed_wxml.wasm    7,633,946 bytes
release/zed_wxml.wasm    238,792 bytes
```

```text
OK highlights.scm: 77 matches, 9 captures
OK brackets.scm: 27 matches, 2 captures
OK indents.scm: 11 matches, 3 captures
OK injections.scm: 5 matches, 1 captures
```

评审摘要中的 release 体积为 237.1K；本次复测为 238,792 B（233.2 KiB）。
以本次构建产物为准，差异来自平台校验代码增加；构建时间/工具链也可能有影响。

## 9. 复现命令

```bash
cd editors/zed
CARGO_TARGET_DIR=/tmp/opencode/zed-ext-target cargo fmt --all -- --check
CARGO_TARGET_DIR=/tmp/opencode/zed-ext-target cargo check --all-targets --locked
CARGO_TARGET_DIR=/tmp/opencode/zed-ext-target cargo clippy --all-targets --locked -- -D warnings
CARGO_TARGET_DIR=/tmp/opencode/zed-ext-target cargo test --locked
CARGO_TARGET_DIR=/tmp/opencode/zed-ext-target cargo check --target wasm32-wasip2 --locked
CARGO_TARGET_DIR=/tmp/opencode/zed-ext-target cargo build --target wasm32-wasip2 --locked
CARGO_TARGET_DIR=/tmp/opencode/zed-ext-target cargo build --target wasm32-wasip2 --release --locked
```

查询验证在仓库外 `/tmp/opencode/query-validate` 使用 `tree-sitter 0.25.8` 与
grammar 路径依赖执行；未把验证器或依赖复制进仓库。

## 10. 对项目决策的影响与数据位置

- 影响：编辑器侧采用“显式绝对路径、无自动回退”的接入方式，牺牲少量便利性，
  换取“不可信项目不能触发项目内二进制执行”的边界；`wxml-lsp` 的命令行契约
  只需保持 `--stdio`。
- 数据位置：实现和最小配置在 `editors/zed/`；关键命令输出摘录在本记录第 8 节；
  查询验证使用仓库外临时程序，不作为长期证据来源。

## 11. 风险与后续

- `Worktree::which` 对绝对路径的支持依赖 Zed 实现细节；已在 1.28.1 源码中核对，
  但未来版本可能变化。失败时会给出清晰错误，用户可检查 `WXML_LSP_BIN`。
- grammar 为小型第三方仓库，长期维护和 ABI 升级不在本仓库控制内；rev 已固定，
  升级需重新跑查询验证。
- 未做 Zed 内运行验证，语言服务器生命周期、日志可见性和查询实际渲染仍需
  手工验收。
- 平台判断依赖 `zed::current_platform()`；host 单测显式传入 `Os`，但 Windows/
  macOS 实机行为仍未验证。
- TypeScript 后端未在本任务中构建；无 `WXML_TYPE_BACKEND` 时服务降级为语法
  能力，不能据此判断类型功能。
- S1 运行时行为尚未抓包复测；Zed 升级后应重新确认。
- 本记录不构成 G0/G2 证据，也不替代 P7 的 `wxml-lsp` 集成证据。
