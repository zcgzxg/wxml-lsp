# P8 Zed 扩展证据

日期：2026-11-16。基线提交：`a6c7eef`。范围：`editors/zed/**`、本记录和
`docs/evidence/README.md` 索引。未修改根 `Cargo.toml`/`Cargo.lock`、CI、
`.opencode`、ADR、fixtures、其他 crates/backends。实现已随 `36aad34` 提交；
本记录与本地安装状态随后续文档提交更新。
证据等级：E1（关键命令输出摘录见第 8 节；查询验证器为仓库外临时程序，
未入库）。

## 1. 结论摘要

- 已建立可审阅的本地 Zed 扩展：`extension.toml`、独立 Rust crate、WXML 语言
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
- 2026-11-16 **已完成标准 UI Dev Extension 安装**：在隔离 user-data-dir 中通过
  Zed 命令面板实际执行 `zed: install dev extension`，选择 `editors/zed`。安装目录
  是指向源码目录的 symlink，`extensions/index.json` 中 `dev=true`，manifest
  注册 WXML/JavaScript/TypeScript/TSX/JSON 五语言服务。Zed 日志显示编译
  Rust 扩展、编译真实 `tree-sitter-wxml` grammar（wasi-sdk 34）、加载扩展，
  并启动 release `wxml-lsp --stdio` 与 Node 后端。详见第 13 节。
- 干净 fixture 已逐项实测：`title`/`user.name` hover 的类型和 JSDoc 正确；
  definition 跳到 `index.ts`；标签、属性、事件属性、Mustache 成员补全均实际
  返回预期候选；`user.nmae` 诊断出现，撤销后 WXML 诊断清空。
- 真实 TS 项目（`jielong-team-doc/miniprogram/miniprogram`）首次暴露
  “页面用 `Component` 注册”被拒绝的问题；修复后端页面注册回退后，`src` hover
  为 `string`，definition 跳到 `data.src`，标签补全可用。真实 JS 项目
  （`ai-mode-demo`）在无 tsconfig/`allowJs` 时按设计降级为
  `script is not part of the program`，语法着色和布局正常。
- 主 Zed 用户数据目录中的手工 dev symlink/index 只作为历史本地安装记录，不再是
  标准安装的唯一依据。G0/G2 的隔离安装、加载、语言注册、服务器启动、hover/
  definition/completion/diagnostics 和重启恢复已有证据；Windows/macOS 实机、
  连续键入延迟和完整 G0/G2 矩阵仍未验收。

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

当前 `extension.toml` 的扩展 `id` 为 `wxml-lsp`、显示名为 `WXML LSP`，用于
避免与 Marketplace 上已有的 `wxml` 扩展（仅 grammar，可能自动更新覆盖本地
dev 扩展）冲突；该命名不影响本记录的验证结果。

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
| 真实 grammar 构建 | wasi-sdk 34 `clang --target=wasm32-wasip1 -Os -fPIC -shared` | 通过；`editors/zed/grammars/wxml.wasm` 约 23,806 B（gitignored） |
| 扩展 id/name | `editors/zed/extension.toml` | `id = "wxml-lsp"`、`name = "WXML LSP"`；避免 Marketplace 的 `wxml` 扩展自动更新覆盖本地 dev 扩展 |
| 查询语法/节点名 | `tree-sitter 0.25.8` 加载 grammar，样本解析后 `Query::new` | 四份查询全部通过；样本命中：highlights 77、brackets 27、indents 11、injections 5 |
| 路径 helper + manifest 契约单测 | `cargo test --locked` | 25 passed（新增 3 项：五语言注册顺序、language_ids、WXML config 不变）；按 `Os::Windows`/`Os::Linux`/`Os::Mac` 参数化，覆盖盘符、UNC、POSIX、`/C:/...` 的 Windows 拒绝与 POSIX 接受、混合分隔符、尾部分隔符、空格、相对路径、`..`、空值、无 `WXML_LSP_BIN`、无效路径不回退、`which` 异常返回 |
| 格式 | `cargo fmt --all -- --check` | 通过 |
| 编译 | `cargo check --all-targets --locked` | 通过 |
| wasm check | `cargo check --target wasm32-wasip2 --locked` | 通过 |
| Lint | `cargo clippy --all-targets --locked -- -D warnings` | 通过，无告警 |
| wasm debug 构建 | `cargo build --target wasm32-wasip2 --locked` | 通过；`zed_wxml.wasm` 7,633,946 B（7.3 MiB） |
| wasm release 构建 | `cargo build --target wasm32-wasip2 --release --locked` | 通过；`zed_wxml.wasm` 238,792 B（233.2 KiB） |
| D6 真实编辑器 smoke | 隔离 Xvfb + Zed 1.28.1；未保存 `index.ts` 类型在 string/number 间切换时 WXML hover 跟随；`ctrl+s` 触发 `didSave` | 通过（第 12 节；截图与 JSON-RPC 日志在仓库外） |

## 6. 未验证项

| 项 | 原因/状态 |
| --- | --- |
| Zed dev extension 安装 | 本节记录的是早期状态；2026-11-16 后续已在隔离 user-data-dir 中通过命令面板实际执行 `zed: install dev extension`，确认 symlink + `dev=true`、五语言注册、LSP/Node 启动，详见第 13 节 |
| Zed 内语法高亮/括号/缩进/注入 | 早期 smoke 用临时 HTML grammar；最终隔离测试已加载真实 `tree-sitter-wxml` grammar。主 Zed 中 WXML 语法着色可见，括号/缩进/注入观感仍未逐项验收 |
| grammar 的 Zed/wasi-sdk 构建 | 已用 wasi-sdk 34 在 host 侧编译真实 grammar（约 23,806 B）；未在 Zed 内触发自动 grammar 构建 |
| `wxml-lsp` initialize | **已由 Zed 1.28.1 启动**：日志显示 `starting language server process ... args: ["--stdio"]`，工作目录为测试项目页面目录（`.../pages/index`）；未逐项核对 `initializationOptions`/`workspace/configuration` |
| 诊断、悬停、定义 | 诊断已在隔离 smoke 中显示（`user.nmae` 精确下划线）；D6 已确认 WXML hover 内容随未保存 TS 类型变化（第 12 节）；definition 仍由 G1/集成测试覆盖，未做真实编辑器跳转验收 |
| Spike S1 运行时复测 | 已完成：2026-11-16 抓包复测见第 7 节，按键级验证见第 12 节 |
| 多 worktree/远程项目 | 未测 |
| Windows/macOS 实机 | 未测；Windows 路径只用字符串单测覆盖 |

### 6.1 隔离 smoke（非标准安装）

目的：确认 Zed 能加载扩展 wasm、注册 WXML 并启动真实语言服务器；不把它当作标准 UI Dev Extension 验收。

- 早期 smoke 曾把已安装 HTML grammar 改名为 `tree_sitter_wxml`，只证明注册/启动链路，不证明 WXML 语法。
- 最终隔离测试使用 wasi-sdk 34 编译的真实 `BlockLune/tree-sitter-wxml` rev `1cc1761…` grammar，产物为 `editors/zed/grammars/wxml.wasm`，约 23,806 B（gitignored）。在 grammar checkout 中执行：

  ```bash
  $WASI_SDK_PATH/bin/clang --target=wasm32-wasip1 -Os -fPIC -shared \
    -I src src/parser.c src/scanner.c \
    -Wl,--export=tree_sitter_wxml -Wl,--allow-undefined \
    -o <repo>/editors/zed/grammars/wxml.wasm
  ```

- 使用 `zed 1.28.1 --user-data-dir /tmp/opencode/zed-user`，项目 `/tmp/opencode/zed-project`（`page-basic` 样本）；`WXML_LSP_BIN=<repo>/target/release/wxml-lsp`（绝对路径），Node 后端指向本仓库 `backends/typescript/dist/server.js`。
- 扩展 id 改为 `wxml-lsp` 后，不再被 Marketplace 的 `wxml` 扩展覆盖。
- 结果：Zed 状态栏显示 `WXML`；第二次启动日志稳定为 `starting language server process. binary path: <repo>/target/release/wxml-lsp, working directory: .../pages/index, args: ["--stdio"]`，无卸载/重启；`user.nmae` 行出现诊断下划线，状态栏显示 1 条诊断。
- 当时未确认悬停内容；后续 D6 已在隔离 Xvfb 中确认（第 12 节）。定义跳转和标准
  `zed: install dev extension` 流程随后在第 13 节完成验收。
- 结论：**语言服务器启动与诊断链路已在 Zed 内实际跑通**；G0/G2 仍只算部分通过。

### 6.2 主 Zed 手工 dev 安装状态（2026-11-16）

- `~/.local/share/zed/extensions/installed/wxml-lsp` 是指向
  `<repo>/editors/zed` 的 dev symlink；`index.json` 含 `wxml-lsp`
  （`dev=true`）和 WXML 语言条目。
- `~/.bashrc` 已导出 `WXML_LSP_BIN`、`WXML_TYPE_BACKEND`、`WXML_BACKEND_DIR`
  的绝对路径。
- 主 Zed 已于 2026-11-16 05:35 重启；Zed 日志显示扩展加载、`wxml-lsp`
  以 `--stdio` 启动；打开 `tests/fixtures/page-basic` 后还启动了
  `node -- backends/typescript/dist/server.js`。
- 主 Zed 中 `user.nmae` 显示红色诊断下划线和文件树错误标记，WXML 语法着色可见。
- 这不是标准 UI `zed: install dev extension` 流程；标准流程已在第 13 节完成。
- hover 内容已在 D6（第 12 节）确认；definition 内容、诊断刷新时序和服务重启
  恢复随后在第 13 节完成真实编辑器验收。

## 7. Spike S1/D1：多语言共享进程与通知转发（2026-11-16 运行时复测）

### 7.1 方法

- 隔离实例：`zed --foreground --user-data-dir /tmp/opencode/wave5/zed-spike/data -n ...`，
  不复用主 Zed 用户数据；settings 开启 `trust_all_worktrees`、关闭自动保存与遥测。
- 扩展副本：`/tmp/opencode/wave5/zed-spike/ext`（`languages = ["WXML", "JavaScript",
  "TypeScript", "TSX", "JSON"]`，language_ids 为 `wxml`/`javascript`/`typescript`/
  `typescriptreact`/`json`）。
- 抓包代理：`/tmp/opencode/wxml-lsp-log.py`（转发到本 worktree
  `target/debug/wxml-lsp`，把 JSON-RPC 方法、URI、languageId 记录到
  `/tmp/opencode/wave5/zed-spike/rpc.log`），并在 `initialized` 后注入一次
  `client/registerCapability` 以验证客户端支持。
- 同一代理进程内观察到下列消息（`pgrep` 去重确认只有一个服务器进程）。所有
  临时文件都在仓库外，未写入项目目录。

### 7.2 结论

1. 一个 `[language_servers.wxml-lsp]` 配置下的多语言只启动一个进程；WXML 与 JS
   的 `didOpen` 进入同一进程（`lang=wxml`、`lang=javascript`）。
2. Zed 1.28.1 转发关联语言的 `didOpen`（带正确 languageId）；`didChange`/
   `didSave`/`didClose` 走同一 buffer 注册链路（源码 `lsp_store.rs` 已核对；
   按键级运行时验证见第 12 节 D6）。
3. initialize 客户端能力：`workspace.didChangeWatchedFiles.dynamicRegistration=true`、
   `relativePatternSupport=true`、`textDocument.synchronization.dynamicRegistration=true`、
   `didSave=true`。
4. 客户端接受服务端 `client/registerCapability`（响应无错误），并实际投递
   `workspace/didChangeWatchedFiles`（外部修改 `utils/other.js`、
   `pages/index/index.json` 均以 `type: 2` 到达）。
5. `workspace/didChangeConfiguration` 通知也会到达。
6. 结论：D1 通过，继续 D2-D6。

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
  只需保持 `--stdio`。扩展 id 使用 `wxml-lsp`，避免 Marketplace 的 `wxml`
  扩展自动更新覆盖本地 dev 安装。
- 数据位置：实现和最小配置在 `editors/zed/`；关键命令输出摘录在本记录第 8 节；
  查询验证使用仓库外临时程序，不作为长期证据来源。

## 11. 风险与后续

- `Worktree::which` 对绝对路径的支持依赖 Zed 实现细节；已在 1.28.1 源码中核对，
  但未来版本可能变化。失败时会给出清晰错误，用户可检查 `WXML_LSP_BIN`。
- grammar 为小型第三方仓库，长期维护和 ABI 升级不在本仓库控制内；rev 已固定，
  升级需重新跑查询验证。真实 grammar 已用 wasi-sdk 34 编译并被标准 UI 安装
  流程加载；渲染观感只在隔离 Xvfb 中验收，Windows/macOS 未验收。
- 标准 UI 安装、重启加载、真实 LSP/Node 启动、hover、definition、completion 和
  diagnostics 已在隔离 Xvfb 中通过；Windows/macOS、真实主桌面 Wayland 和连续
  键入延迟仍未验收。
- 平台判断依赖 `zed::current_platform()`；host 单测显式传入 `Os`，但 Windows/
  macOS 实机行为仍未验证。
- P8 代码门禁不要求构建 TypeScript 后端；本地安装已构建
  `backends/typescript/dist/server.js` 供 Zed 使用，但这不属于 P8 门禁证据。
  无 `WXML_TYPE_BACKEND` 时服务降级为语法能力，不能据此判断类型功能。
- S1/D1 已在 Zed 1.28.1 上抓包复测（第 7 节），D6 又做了按键级验证（第 12 节）；
  Zed 升级后应重新确认协议行为。
- `didSave` 依赖服务端在 `initialize` 声明 `textDocumentSync.save`；本次已补
  `save.includeText=false`，升级 Zed 或调整初始化能力后需回归。
- watched files 依赖客户端动态注册；1.28.1 支持并已验证，不支持时回退到 `didSave`。
- 纯 `.js` 页面在 tsconfig 未开 `allowJs` 时后端拒绝类型检查（既有行为）；本次
  smoke 使用 `.ts` 页面脚本。连续键入时诊断队列可能滞后数十秒，粘贴/低频编辑无明显延迟。
- 本记录只构成 G0/G2 的部分证据（扩展加载、语言注册、服务器启动和诊断链路），
  不构成完整 G0/G2 验收，也不替代 P7 的 `wxml-lsp` 集成证据。

## 12. D6：真实 Zed 多文档未保存修改 smoke（2026-11-16）

结论：通过。五语言注册只启动一个 `wxml-lsp` 进程；未保存的 `index.ts` 字段类型在
`string`/`number` 间切换时，WXML `hover` 同步变化；`ctrl+s` 到达 `didSave` 后刷新
WXML 诊断；无关 `utils/other.ts` 的未保存修改不触发 WXML 重新分析。证据截图在
`/tmp/opencode/wave5/zed-d6/`（`e2-unsaved-string.png`、`e5-number-final.png`、
`e4-unrelated.png`），原始 JSON-RPC 日志为
`/tmp/opencode/wave5/zed-spike/rpc.log`；均为仓库外临时证据。

### 12.1 环境与启动

- Zed 1.28.1（`e8bc16a`），隔离 Xvfb `:99`（1600x1000，X11 + llvmpipe，无窗口
  管理器），用户数据目录 `/tmp/opencode/wave5/zed-d6/data`；不接触主 Zed 桌面。
- 扩展：`/tmp/opencode/wave5/zed-d6/ext`（本 worktree 最终五语言 `extension.toml`、
  release wasm 238,792 B、真实 WXML grammar）。启动参数：
  `ZED_ALLOW_EMULATED_GPU=1 zed --foreground --user-data-dir ... -n proj
  index.wxml index.ts utils/other.ts index.json`。
- 项目：`app.json`、`pages/index/index.{wxml,ts,json}`、`utils/other.ts`、
  `components/card/*`、`tsconfig.json`（baseUrl + `@/*`）。`index.wxml` 为
  `<view class="page">{{user.name}}<card title="t"></card></view>`。
- 服务器仍通过 `WXML_LSP_BIN` 指向的代理启动（代理转发到本 worktree
  `target/debug/wxml-lsp`，只记录 JSON-RPC，不修改消息；抓包注入已关闭）。

### 12.2 注册与多文档归属

同一进程的日志摘录：

```text
C>S initialize ... didChangeWatchedFiles={"dynamicRegistration": true, "relativePatternSupport": true}
                      sync={"dynamicRegistration": true, "didSave": true} root=.../proj
S>C client/registerCapability {"registrations":[{"id":"wxml-lsp-watch",
      "method":"workspace/didChangeWatchedFiles","registerOptions":{"watchers":[
      {"globPattern":"**/*.wxml","kind":7}, ... {"globPattern":"**/*.wxss","kind":7}]}}]}
C>S <response id='wxml-lsp-watch' err=False>
C>S textDocument/didOpen .../pages/index/index.wxml lang=wxml version=0 len=63
C>S textDocument/didOpen .../pages/index/index.json lang=json version=0 len=58
C>S textDocument/didOpen .../utils/other.ts lang=typescript version=0 len=41
C>S textDocument/didOpen .../pages/index/index.ts lang=typescript version=0 len=50
```

即：一个进程同时持有 WXML/JSON/TS 文档；客户端接受服务端动态注册；
`workspace/didChangeConfiguration` 也会到达（仅记录）。

### 12.3 未保存类型修改 → WXML hover

1. 初始磁盘 `index.ts` 为 `name: 42`，WXML hover 为 `(property) name: number`。
2. 在 `index.ts` 全选粘贴 `Page({ data: { user: { name: 'Alice', age: 30 } } });`，
   **不保存**。日志只有 `didChange ... version=1 changes=1`（一次编辑事件，无
   `didSave`）；WXML 诊断重新发布 `count=0`；切到 WXML hover 显示
   `(property) name: string`（截图 `e2-unsaved-string.png`，`index.ts` 标签带未保存圆点）。
3. `ctrl+s`：日志出现 `workspace/didChangeWatchedFiles(index.ts type 2)` 与
   `textDocument/didSave uri=...index.ts hasText=False len=0`，1.6 s 后
   `publishDiagnostics ...index.wxml count=0`（磁盘 54 B，截图 `e3-saved.png`）。
4. 再次在 `index.ts` 不保存改为 `name: 42`（`didChange version=3`），WXML hover
   变为 `(property) name: number`（截图 `e5-number-final.png`）。

这验证了 AC-004（未保存 overlay 进入 WXML 分析）和 AC-010 的多文档部分
（WXML + 两个 TS + JSON 共享一个服务器实例）。

### 12.4 didSave 能力缺口与修复

第一次 D6（修复前）按 `ctrl+s` 只看到 `workspace/didChangeWatchedFiles`，没有
`didSave`。对照 Zed 1.28.1 源码 `lsp_store.rs::did_save_include_text_for_buffer`：
客户端只有在服务端 `initialize` 声明 `textDocumentSync.save` 时才发送 `didSave`。
修复：`initialize_result.textDocumentSync` 增加 `"save": {"includeText": false}`，
并新增初始化结果断言（`server.rs`、`tests/lsp.rs`）。修复后第 12.3 节第 3 步
首次看到 `didSave`。`includeText=false` 时处理器以当前打开 buffer 文本为准
（仅在 buffer 不存在时才使用 `params.text`）。保存同时触发的 watched-files
事件会跳过仍打开的 URI，因此 `didSave` 与 watched-files 双到不会互相覆盖，
也不会把未保存内容回退成磁盘文本。

### 12.5 无关脚本隔离

向 `utils/other.ts` 不保存粘贴 `export const unrelated = { flag: false };`
后，最终二进制记录到一次 `didChange`（`version=1`）和一次
`index.wxml publishDiagnostics count=0`，WXML hover 仍为
`(property) name: number`（截图 `e6-unrelated.png`）。类型后端把项目内所有脚本
作为同一程序，因此非归属脚本的变更会保守地重跑全部视图诊断；诊断内容不受
污染，未保存的 `index.ts` overlay 仍优先于磁盘文本。

### 12.6 JSON 保存与 rediscovery

保存 `index.json`（先 `{}`，后 `{"usingComponents":{"card":"../../components/card/card"}}`）
均产生 `didSave` 和随后的 `index.wxml publishDiagnostics count=0`，说明配置 JSON
保存会触发项目 rediscovery/视图重建。该样本没有未知组件诊断，无法用诊断差证明
`usingComponents` 生效；界面左下角 `1` 是 Zed 自带 TypeScript 服务的
`Cannot find name 'Page'`，不是 `wxml-lsp` 诊断。

### 12.7 限制与观察

- 该 fixture 使用 `.ts` 页面脚本；纯 `.js` 页面在 tsconfig 未开 `allowJs` 时后端
  报 `script is not part of the program`（既有后端行为，不属于本次改动）。
- 连续键入 45 个字符会产生 45 次 `didChange`，诊断队列约 26–35 s 才稳定；粘贴
  （单次 `didChange`）响应即时。真实键入的响应时间仍需单独观测。
- 非页面脚本（如 `utils/other.ts`）的变更会让全部视图重跑诊断（保守策略）：
  诊断内容不变，但会产生一次额外 `publishDiagnostics`。
- 标准 UI `zed: install dev extension` 已于第 13 节完成；与编辑器内置 JS/TS 服务的
  显式归属/重启用例仍未验收，本 smoke 只证明两者可同时运行（内置 TS 诊断未被抑制）。

## 13. 标准 UI 安装与真实项目验收（2026-11-16）

结论：通过。已在隔离 user-data-dir 中通过命令面板实际执行
`zed: install dev extension`，确认标准 dev 安装、真实 grammar 编译、五语言注册、
release LSP + Node 后端启动；干净 fixture 和真实 TS/JS 项目的 hover、definition、
completion、diagnostics 已在 Zed 中逐项执行并核对内容。验收中发现并修复了
“页面路由使用 `Component` 注册”被后端拒绝的问题。

### 13.1 环境与隔离

| 项 | 值 |
| --- | --- |
| Zed | 1.28.1（`e8bc16a`） |
| 显示 | Xvfb `:99`，1600x1000，llvmpipe，无窗口管理器 |
| 用户数据 | `<tmp>/zed-standard/data`（隔离） |
| HOME / XDG_CONFIG_HOME | `<tmp>/zed-standard/home`（隔离，避免用户 `.bashrc` 覆盖路径） |
| 项目副本 | `<tmp>/zed-standard/proj`（`page-basic-clean` 副本） |
| 事件样本 | `<tmp>/zed-standard/events-proj`（`events-basic` 副本） |
| 语言服务器 | release `wxml-lsp --stdio` |
| 类型后端 | `backends/typescript/dist/server.js`，Node 24 |
| 真实项目 A | `jielong-team-doc/miniprogram/miniprogram`（TS，`Component` 注册页面） |
| 真实项目 B | `ai-mode-demo`（JS，无 tsconfig） |

隔离环境使用 `env -u WAYLAND_DISPLAY XDG_SESSION_TYPE=x11 DISPLAY=:99`，避免
Zed 继承主桌面的 Wayland 连接。命令面板的目录选择使用仓库外私有 session bus
上的假 portal，仅替代 XDG 文件选择对话框并固定返回 `<repo>/editors/zed`；
扩展编译、安装和加载均由 Zed 完成，没有手工创建 symlink。

### 13.2 标准安装证据

- Zed 日志：`compiling Rust extension`、`compiling grammar wxml`、
  `using wasi-sdk from WASI_SDK_PATH`、`finished compiling extension ... in
  12.25s`、`extensions updated. loading 1`。
- `<user-data>/extensions/installed/wxml-lsp` 是指向 `<repo>/editors/zed` 的
  symlink；`extensions/index.json` 中 `dev=true`。
- manifest languages 为 `languages/wxml`；server languages 为
  WXML/JavaScript/TypeScript/TSX/JSON。
- 进程：一个 `target/release/wxml-lsp --stdio`，一个
  `node backends/typescript/dist/server.js`。
- 关闭并用同一 user-data-dir 重启后，扩展仍为标准 dev 安装，LSP 重新启动，
  `title` hover 仍返回 `string` 和“页面标题”。
- 证据：`docs/evidence/data/zed-standard/install-log.txt`、
  `extension-index-excerpt.json`、`install-success.png`。

### 13.3 干净 fixture 逐项结果

| 操作 | 实际结果 |
| --- | --- |
| hover `{{ title }}` | `(property) title: string` + “页面标题” |
| hover `{{ user.name }}` | `(property) name: string` + “用户显示名称” |
| definition `title` | 打开 `pages/index/index.ts`，落在 `data.title`（行 4，列 10），面包屑 `data > title` |
| 标签补全 `<vi` | `video`、`view`、`web-view`、`cover-view`、`picker-view`、`scroll-view`、`movable-view`、`navigator`、`navigation-bar`、`cover-image` 等 |
| 属性补全 `<view ` | `class`、`hidden`、`id`、`style`、`wx:elif`、`wx:else`、`wx:for`、`wx:for-index` 等 |
| 事件属性补全 `<view bindt` | `bindtap`、`bindtouchend`、`bindtouchmove`、`bindtouchstart`、`bind:tap`、`bindinput`、`bindreset`、`bindsubmit` 等 |
| 成员补全 `{{ user.` | `name` |
| 未知成员 `user.nmae` | hover 显示 `Property 'nmae' does not exist on type '{ name: string; }'` 和 `type quality: missing`；状态栏 WXML 诊断计数为 1 |
| 撤销错误 | 文件恢复为干净 fixture，WXML 诊断清空 |

成员补全首次触发时类型后端仍在分析，第二次 `ctrl+space` 才返回 `name`；其余
补全即时。截图：`hover-title.png`、`hover-user-name3.png`、
`definition-title.png`、`completion-tag.png`、`comp-attr.png`、
`comp-event.png`、`comp-member2.png`、`diag-hover-nmae2.png`。

#### 13.3.1 事件处理函数 hover 与跳转（补充）

在 `events-basic` 项目副本中实际执行：

| 操作 | 实际结果 |
| --- | --- |
| hover 事件处理函数 `onShorthandTap` | 显示方法签名 `onShorthandTap()` |
| Ctrl+点击 `onShorthandTap` | 打开 `pages/index/index.ts`，落在第 8 行方法定义，面包屑 `pages/index/index.ts > onShorthandTap()` |
| hover 标签 `<button>` | 无结果；标签 hover 尚未实现 |
| hover 属性 `bindtap` | 无结果；属性 hover 尚未实现 |

截图：`event-handler-hover.png`、`event-handler-definition.png`、
`tag-hover-none.png`、`attribute-hover-none.png`。标签/属性 hover 的缺口已在
README 的 F-08 未完成项中记录，不在本轮验收中伪造结果。

### 13.4 真实 TS 项目：Component 注册页面的修复

项目页面脚本是：

```ts
Component({ data: { src: '', title: '', userId: '' }, ... })
```

第一次打开 WXML 时状态栏显示
`type checking is unsupported: no page registration with an object literal argument`，
hover/definition 均无结果。原因是后端按页面视图过滤注册时只接受 `Page`，
而该真实项目使用 `Component` 注册页面路由。

修复：

- ` backends/typescript/src/analysis.ts`：页面视图优先匹配 `Page`；没有
  `Page` 时回退到 `Component` 注册，并把实际注册种类写入响应，后续数据根、
  hover、diagnostics、definition 都按组件根处理。
- ` backends/typescript/test/analysis.test.ts`：新增页面用 `Component` 注册的
  单元测试，断言 `registration.kind === "component"`、`count === 1`、
  `title` hover 为 `string`。

修复后实测：

- 状态栏不再出现 unsupported。
- hover `src`：`(property) src: string`。
- definition `src`：跳到 `index.ts` 的 `data.src`（行 4）。
- 标签补全 `<vi`：`video`、`view`。
- 所有临时编辑已撤销，真实项目没有留下修改。

截图：`real2-hover-src-inner.png`、`real2-def-src.png`、
`real2-completion-tag2.png`。

### 13.5 真实 JS 项目

项目 `ai-mode-demo` 没有 `tsconfig.json`/`jsconfig.json`，页面为 JS。打开
`pages/home/home.wxml` 后：

- 状态栏显示 `type checking is unavailable: script is not part of the program`。
- hover `{{title}}` 无结果；这是未启用 `allowJs`/无项目配置时的既有降级，不是本轮
  回归。
- 复杂 `wx:for`、事件和自定义组件的语法着色、缩进、Mustache 注入布局正常。

截图：`ai2-home-open.png`、`ai2-hover-title.png`。

### 13.6 限制与观察

- 真实 TS 项目只覆盖 `Component` 注册的页面；普通 `Page`、组件和跨文件未保存
  类型传播仍由既有测试覆盖，本轮未逐项重做。
- 真实 JS 项目无项目配置，类型能力按设计降级；开启 `allowJs` 的 JS 项目未验收。
- Xvfb 无窗口管理器，观感不等同于用户主桌面 Wayland；Windows/macOS 未验收。
- 连续键入的诊断/补全延迟未单独测量；此前 D6 观察到 45 次连续 `didChange`
  后诊断约 26–35 s 稳定，粘贴响应即时。
- 反复制造损坏 WXML/脚本时曾观察到 `server is busy`、Node CPU 升高和
  `wxml-lsp` 进程退出；全新重启后恢复，尚未根因定位。本轮正常验收未复现。
- 与 Zed 内置 JS/TS 服务并存已确认；显式归属/重启用例仍未单独验收。
