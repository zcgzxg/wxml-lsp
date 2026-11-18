# tsgo P5 安全审计证据（2026-11-18）

日期：2026-11-18。状态：P5 安全审计已执行；本文只记录本次只读审计与本机测试结果，不表示 P5/G5 已通过。
审计工作区：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`，HEAD `d6e8f41ee27f9771dc6923f7977d177280a896e6`（`test(tsgo): make backpressure test independent of helper race`）。
审计范围：ADR-0008、`docs/reference/tsgo-native-contract.md` 的安全/FS 条款、`crates/wxml-backend-tsgo/src/native/{vfs,session,supervisor,ast,handles,framing,rpc,protocol}.rs`、`crates/wxml-backend-tsgo/src/query/**`，以及为确认“可达方法”而查看的 `crates/wxml-lsp/src/backend_tsgo/{session,mod}.rs` 边界。
写入范围：本文件是本次唯一仓库写入；未修改生产代码、测试、ADR、合同、server/config；未提交。
证据等级：E1（命令与关键输出已内嵌本文；原始日志在仓库外 `/tmp/opencode/p5-*.log`，不是唯一证据来源）。

## 0. 结论摘要

**整体判定：BLOCKED。** 没有发现 CRITICAL/HIGH 的远程可利用写路径或越界读路径；方法白名单、VFS 读边界、进程环境清理、AST/句柄预算在本次可证明范围内为 PASS。但以下三项使 P5/G5 安全门禁不能判 PASS：

1. **B-02 的第二层“运行目录最小权限”没有在 Rust 层实现或测试。** 方法白名单（第一层）成立；`validate_sandbox_root` 只校验目录存在、为空、非符号链接，未设置/校验只读挂载、目录权限、uid/gid 降权或 seccomp。ADR/合同要求的最小权限目前是部署约定，不是代码控制。
2. **合同 §4.3 要求的 `--runExternalCode` 三重验证只实现了 argv 断言。** 没有进程树断言，也没有插件 canary 测试；源码证明“永不传 flag”成立，但完整验证门禁不成立。
3. **`cargo deny check` 全量命令失败（licenses）。** `cargo audit` 与 `cargo deny check advisories bans sources` 通过；仓库没有 `deny.toml`，全量 `cargo deny --offline check` 以 139 个 license rejection 退出 4。该缺口在 `docs/evidence/README.md:47` 已作为既有基线记录，不是新 CVE，但不能把“cargo deny check 通过”写成事实。

| 审计项 | 判定 | 依据 |
| --- | --- | --- |
| 方法白名单/禁止清单 | **PASS** | 类型化 26 方法枚举；无任意 `call(method, params)`；配置/枚举一致性测试通过 |
| 可达方法写/emit/build/clean/外部代码/插件 | **PASS（方法层）** | session 仅调用 15 个读/生命周期方法；tsconfig 固定合成；无写 API；无 `--runExternalCode` |
| `--runExternalCode` 完整验证 | **PARTIAL** | argv 断言通过；缺进程树断言与插件 canary（合同 §4.3） |
| VFS 路径限制/`useOS`/未注册回调 | **PASS** | 组件边界前缀、`..`/绝对/反斜杠拒绝；不返回 `useOS`；未知回调报错；真实引擎 canary 通过 |
| `writeFile`/`removeFile` noop | **PASS** | 回调返回 `noop`；真实引擎运行后宿主 canary 内容未变、虚拟项目根未创建 |
| `appendFile`/`chtimes` B-02 | **PARTIAL（已接受的残余风险）** | 无回调可注册；白名单使写方法不可达；最小权限第二层未实现/未测试 |
| 进程 env/cwd/stderr/超时/回收 | **PASS（除下述 LOW）** | `env_clear` + `LANG/LC_ALL`；空 cwd；有界 stderr；SIGKILL/reap；超时 poison；测试通过 |
| AST/句柄预算/epoch/陈旧快照 | **PASS** | 64 字节 v9 header、1M 节点/字符串上限、扁平扫描、六元绑定、更新后旧句柄拒绝；测试通过 |
| 依赖安全 | **PASS（advisories/bans/sources）** | `cargo audit` 0 vulnerabilities；deny advisories/bans/sources ok |
| license 门禁 | **BLOCKED** | 无 `deny.toml`，全量 deny licenses 失败 |
| **P5 安全门禁整体** | **BLOCKED** | 上述 1/2/3 未闭环；不是“发现可远程利用漏洞” |

## 1. 方法白名单、禁止清单与外部代码

### 1.1 只有类型化方法名，没有任意 method 入口

`NativeMethod` 由宏生成，唯一变体集就是冻结的 26 个方法；`ALL` 与 `config::ALLOWED_METHODS` 由测试逐项比较。`NativeRequest` 的 `method`/`params` 字段私有，只能经 `NativeRequest::new(NativeMethod, RawPayload)` 构造；`encode_request` 也只接受 `NativeRequest`，不接受字符串方法名。

- 枚举与 wire 名：`crates/wxml-backend-tsgo/src/native/protocol.rs:19`（宏）、`crates/wxml-backend-tsgo/src/native/protocol.rs:45`（26 个变体）
- 请求构造/编码：`crates/wxml-backend-tsgo/src/native/protocol.rs:74`、`crates/wxml-backend-tsgo/src/native/rpc.rs:724`
- 白名单数据：`crates/wxml-backend-tsgo/src/config.rs:135`
- 一致性测试：`crates/wxml-backend-tsgo/src/native/protocol.rs:480`（`typed_methods_match_the_frozen_allowlist`）
- 旧 LanguageService 方法不可达测试：`crates/wxml-backend-tsgo/src/native/protocol.rs:490`

禁止清单（29 项）覆盖 `emit`/`emitToString`/`getJavaScriptEmit`/`getDeclarationEmit`、build/clean、profile/heap、module/config 解析、transpile/format/print、`getCurrentLanguageServerSnapshot` 和 `batchRequests`，见 `crates/wxml-backend-tsgo/src/config.rs:165`。该清单是**数据**，运行时真正的控制是类型化白名单；两者有无交集由 `config::tests::allow_and_deny_lists_are_unique_and_disjoint` 检查（`crates/wxml-backend-tsgo/src/config.rs:487`）。

### 1.2 当前 HEAD 的可达方法集

`NativeSession` 实际构造的方法只有：

`initialize`、`createSnapshot`、`updateSnapshot`、`release`、`getSourceFile`、`getSourceFileNames`、`getSemanticDiagnostics`、`getSyntacticDiagnostics`、`getTypeAtPosition`、`typeToString`、`getSymbolAtPosition`、`getDocumentationComment`、`getJsDocTags`、`getReferencedSymbolsForNode`、`getCompletionsAtPosition`。

这些都是生命周期或只读查询；没有 emit/build/clean/transpile/format/plugin 方法。`createSourceFile`/`retainSourceFile`/`releaseSourceFile`/`getCachedSourceFile`、`getTypeAtLocation`、`getTypeOfSymbol`、`getDeclaredTypeOfSymbol`、`getSymbolAtLocation`、`getSymbolOfNode`、`getDefaultProjectForFile` 虽在白名单中，但本 HEAD 没有公开包装调用（`grep NativeMethod::` 结果只到上述 15 个；§6.1 的白名单测试另行覆盖枚举本身）。

LSP 侧适配器的 `QuerySession` 只暴露 open/update、semantic/syntactic diagnostics、type_query、symbol/documentation、completions，见 `crates/wxml-lsp/src/backend_tsgo/session.rs:54`；且 `wxml-lsp/src/backend_tsgo/mod.rs:1` 说明该适配器不被 `server.rs`/`project_host.rs`/配置默认构造，当前 HEAD 没有远程 LSP 入口直接到达 native 会话。

### 1.3 外部代码与插件

- 启动 argv 固定为 6 个元素（`--cwd <sandbox>` 占两项）：`--api`、`--async`、`--cwd <sandbox>`、`--useCaseSensitiveFileNames=true`、`--callbacks=<固定串>`；没有 `--runExternalCode`、`--pipe`、`--timing`。见 `crates/wxml-backend-tsgo/src/config.rs:361` 和测试 `crates/wxml-backend-tsgo/src/native/supervisor.rs:1729`（断言 argv 不含 `--runExternalCode`、`appendFile`、`chtimes`、`node`）。
- `--version` 探针只传 `--version`，同样清空环境（`crates/wxml-backend-tsgo/src/native/session.rs:1536`）。
- `SpawnSpec::command` 使用 `Command::new` 绝对路径、`current_dir(sandbox)`、`env_clear()`，只注入 `LANG=C`/`LC_ALL=C`；不经过 shell，stdin/stdout/stderr 均为 pipe。见 `crates/wxml-backend-tsgo/src/native/supervisor.rs:507`；环境测试见 `crates/wxml-backend-tsgo/src/native/supervisor.rs:2432`。
- 合成的 `tsconfig.json` 只包含 `noLib`、`strict`、`module`、`target` 和排序后的 `files`，没有 `plugins`、`types`、`typeRoots`、`paths`、`extends`。见 `crates/wxml-backend-tsgo/src/native/vfs.rs:685`。
- `AnalysisInput` 的 `SourceFile` 只接受 `.ts/.mts/.cts/.js/.mjs/.cjs`，不能把 `tsconfig.json`/`package.json` 作为输入文件注入（`crates/wxml-backend-tsgo/src/input.rs:170`）。因此不能通过项目配置启用 tsconfig `plugins`。
- 生产代码中 `--runExternalCode` 只出现在测试的“禁止项”数组（`crates/wxml-backend-tsgo/src/native/supervisor.rs:1745`）；`appendFile`/`chtimes` 只出现在测试/回调拒绝用例，不在 `CALLBACK_NAMES`（`crates/wxml-backend-tsgo/src/config.rs:116`、`crates/wxml-backend-tsgo/src/config.rs:477`）。

**缺口（P5-SEC-02）**：合同 §4.3 要求 `--runExternalCode` 禁止同时具备“argv 断言 + 进程树断言 + 插件 canary 测试”（`docs/reference/tsgo-native-contract.md:289`–`295`）。本 HEAD 只有 argv 断言；没有读取 `/proc`/子进程树，也没有插件 canary。方法层的“不可达”是源码证明，但不能替代这三项门禁。

## 2. VFS：路径限制、回退与回调

### 2.1 路径规范化与根边界

所有回调参数先要求字符串（`crates/wxml-backend-tsgo/src/native/vfs.rs:569`），然后：

1. `validate_absolute` 拒绝非绝对路径、`/` 本身、`//`、空组件、`.`/`..`、反斜杠、控制字符（`crates/wxml-backend-tsgo/src/native/vfs.rs:652`）。
2. `within_root` 用 `strip_prefix(root)` 后要求剩余部分以 `/` 开头，按**路径组件边界**匹配，避免 `/proj` 命中 `/project-evil`（`crates/wxml-backend-tsgo/src/native/vfs.rs:584`）。
3. 不在根内或非法路径按回调类型 fail-closed：`readFile/stat -> missing`，`fileExists/directoryExists -> value:false`，`getAccessibleEntries -> 空 value`，`realpath -> identity`（`crates/wxml-backend-tsgo/src/native/vfs.rs:325`、`crates/wxml-backend-tsgo/src/native/vfs.rs:386`）。
4. 相对路径（`AnalysisInput` 的文件路径）由 `validate_relative` 拒绝空、绝对、反斜杠、控制字符、`//`、`.`/`..`、空组件（`crates/wxml-backend-tsgo/src/native/vfs.rs:624`）。

已有测试覆盖 `../secret.ts`、`//`、尾随 `/`、相对路径、`./`（`crates/wxml-backend-tsgo/src/native/vfs.rs:944`），以及宿主真实文件读取返回 `missing`（`crates/wxml-backend-tsgo/src/native/vfs.rs:773`）。非 UTF-8 路径在 Rust `&str` 与 JSON UTF-8 解码层不可能进入；反斜杠由上述两个校验器拒绝，句柄路径另有 `validate_absolute_path`（`crates/wxml-backend-tsgo/src/native/handles.rs:497`，测试 `crates/wxml-backend-tsgo/src/native/handles.rs:545`）。

### 2.2 不返回 `useOS`，未注册回调不落到宿主

- 生产 VFS 的 8 个回调只返回 `value`/`missing`/`noop`/`identity`，从不返回 `useOS`；源码中没有生产 `useOS` 字符串（仅文档注释，`crates/wxml-backend-tsgo/src/native/vfs.rs:5`、`crates/wxml-backend-tsgo/src/native/vfs.rs:22`）。
- 未知回调返回 `CallbackError::UnknownCallback`（JSON-RPC `-32601`），`fail_closed_answer` 对未知名字返回 `None`，不会伪造空内容（`crates/wxml-backend-tsgo/src/native/vfs.rs:373`、`crates/wxml-backend-tsgo/src/native/vfs.rs:386`）。测试断言 `appendFile` 被拒绝（`crates/wxml-backend-tsgo/src/native/vfs.rs:926`、`crates/wxml-backend-tsgo/src/native/vfs.rs:984`）。
- 未安装 VFS（initialize/createSnapshot 窗口）时使用逐回调 fail-closed 形状，而不是 JSON-RPC error；这是合同 §5.2 的要求，因为读回调 error 会让引擎 panic（`crates/wxml-backend-tsgo/src/native/session.rs:1428`）。

**注意**：这里的“未知回调报错”只处理引擎主动发来的未知名字；对于**没有注册**的读回调，引擎内部会直接走 base OS，而不是发给 Rust。真正的边界是 `CALLBACK_NAMES` 覆盖 callbackfs 的 8 个名字（见 §3.2），以及版本升级时重新审计。

### 2.3 `writeFile`/`removeFile` 是 noop

`writeFile` 校验 `{path}` 形状后返回 `{"kind":"noop"}`，`removeFile` 校验路径字符串后返回 `{"kind":"noop"}`；即使路径在根外也不写、不删（`crates/wxml-backend-tsgo/src/native/vfs.rs:365`、`crates/wxml-backend-tsgo/src/native/vfs.rs:369`）。测试在调用后重新读取虚拟文件，确认内容未变（`crates/wxml-backend-tsgo/src/native/vfs.rs:904`）。启动串使用 `writeFile:noop,removeFile:noop`（`crates/wxml-backend-tsgo/src/config.rs:116`），因此 engine 的写入路径在正常配置下也不会向 Rust 发写回调。

### 2.4 读/stat canary 与真实引擎

- 本地 canary：`assert_confined` 对一个真实宿主文件依次断言 `readFile`/`stat` 为 `missing`、`fileExists`/`directoryExists` 为 false、`getAccessibleEntries` 为空、`realpath` 为 identity；若 canary 在根内则拒绝（`crates/wxml-backend-tsgo/src/native/vfs.rs:410`）。测试见 `crates/wxml-backend-tsgo/src/native/vfs.rs:987`。
- 真实引擎 canary：`real_engine_vertical_slice` 在项目外创建 `host-canary.ts`，通过 `import type { HostCanary } from "<outside>"` 断言诊断码 2307（无法解析），见 `crates/wxml-backend-tsgo/tests/real_engine.rs:241`；运行结束后断言 canary 文件内容未变、虚拟项目根未在宿主创建（`crates/wxml-backend-tsgo/tests/real_engine.rs:433`）。本次实跑通过（§6.2）。
- `stat` 的回调形状：本 HEAD 返回 `{mode,size,mtime}`，`mtime` 为 `1970-01-01T00:00:00Z`（`crates/wxml-backend-tsgo/src/native/vfs.rs:465`、`crates/wxml-backend-tsgo/src/native/vfs.rs:41`）。对照固定上游源码 `callbackfs.go` 的 `Stat` 解码字段为 `mode/size/mtime`，`mtime` 用 `time.RFC3339Nano` 解析，形状一致。但 P2 证据的写回调/读回调捕获表没有 `stat` 的实测帧（`docs/evidence/2026-11-17-tsgo-native-api-spike.md:1083` 只列 callbackfs 名字；B-03 捕获表没有 stat），因此**没有仓库内 engine-level stat canary**；这是 P5-SEC-03（LOW），不是写绕过。

### 2.5 请求/响应预算

- 单请求 32 MiB、单响应 64 MiB、回调 32 MiB、header 8 KiB、JSON 深度 128、容器条目 100,000、单字符串 32 MiB、AST 节点/字符串各 1,000,000（`crates/wxml-backend-tsgo/src/config.rs:62`–`87`，`crates/wxml-backend-tsgo/src/native/framing.rs:80`）。
- 入站帧队列另有 96 MiB 聚合预算，防止 8 个 64 MiB 帧同时排队（`crates/wxml-backend-tsgo/src/native/supervisor.rs:93`、`crates/wxml-backend-tsgo/src/native/supervisor.rs:1306`）；测试见 `crates/wxml-backend-tsgo/src/native/supervisor.rs:1924`、`crates/wxml-backend-tsgo/src/native/supervisor.rs:2012`。
- 快照请求在序列化前用保守上界预检，超 32 MiB 直接 `InputTooLarge`（`crates/wxml-backend-tsgo/src/native/vfs.rs:454`、`crates/wxml-backend-tsgo/src/native/session.rs:1498`）；测试 `crates/wxml-backend-tsgo/src/native/session.rs:1752`。

## 3. B-02 残余风险：`appendFile`/`chtimes`

### 3.1 事实与文档

ADR-0008 记录：固定 7.1.0-beta 的 `callbackfs.go` 只注册 8 个回调；`AppendFile` 与 `Chtimes` 始终委托 base OS FS，源码注释明确 *“always delegates to base (no callback support)”*；任何 `--callbacks=appendFile*`/`chtimes*` 都在启动阶段 panic，因此**不能实现回调级 deny**。见 `docs/adr/0008-rust-tsgo-no-node-backend.md:25`、`docs/adr/0008-rust-tsgo-no-node-backend.md:29`–`31`，以及合同 `docs/reference/tsgo-native-contract.md:341`–`347`、B-02 表 `docs/reference/tsgo-native-contract.md:788`。P2 实测逐项记录在 `docs/evidence/2026-11-17-tsgo-native-api-spike.md:1072`–`1083`。

### 3.2 Rust 层实际控制

| 控制 | 状态 | 证据 |
| --- | --- | --- |
| 方法白名单使 emit/build/clean/append 路径不可达 | **已实现** | 类型化枚举仅 26 方法（§1.1）；session 只调用读/生命周期方法；无 `call(method, params)` 入口 |
| `writeFile:noop`/`removeFile:noop` 启动串 | **已实现** | `crates/wxml-backend-tsgo/src/config.rs:116`；测试 `crates/wxml-backend-tsgo/src/config.rs:477` |
| 回调级 `appendFile`/`chtimes` deny | **不可实现（设计边界）** | 上游 callbackfs 无回调分支；ADR/合同 B-02 已接受 |
| 运行目录最小权限/只读 | **未在 Rust 层实现或测试** | `validate_sandbox_root` 只检查存在、目录、非符号链接、为空（`crates/wxml-backend-tsgo/src/native/supervisor.rs:604`）；supervisor 文档自己写明“should ... does not change its mode”（`crates/wxml-backend-tsgo/src/native/supervisor.rs:17`–`19`） |
| 失败时 OS 拒绝 append/chtimes 的测试 | **缺失** | 合同 §5.3 要求该测试（`docs/reference/tsgo-native-contract.md:345`）；仓库只有 VFS 单元测试断言 `appendFile` 是未知回调（`crates/wxml-backend-tsgo/src/native/vfs.rs:926`），没有启动真实引擎并验证 OS 拒绝的用例 |

### 3.3 残余风险陈述（必须保留）

- 第一层（方法白名单）成立：Rust 不会发送 emit/build/clean/transpile 等方法，也不会发送任意方法名；因此“通过 API 主动调用 append/chtimes”不可达。
- 第二层（最小权限）**不成立**：进程以启动 LSP 的同一 uid/gid 运行，没有只读挂载、目录权限收紧、capability 降权或 seccomp；`--cwd` 只保证是空目录，不保证引擎不可写。若某个白名单方法在引擎内部调用 `AppendFile`/`Chtimes`（例如未来编译器实现变化），写入会按引擎进程的宿主权限发生；Rust 侧没有运行时检测点，因为这两个操作根本不会变成回调。
- 结论：B-02 仍是 **PARTIAL**；维护者已接受“方法白名单 + 运行目录最小权限 + 禁用 emit/external code”的补偿控制，但当前代码只实现了第一项和 `writeFile/removeFile` noop，第二项（最小权限）需要部署/后续实现证明。不能声称“回调级写沙箱已实现”，也不能声称“已证明 append/chtimes 一定不会发生”。

## 4. 进程、环境与生命周期

| 控制 | 状态 | 证据 |
| --- | --- | --- |
| 直接 exec、无 shell | PASS | `Command::new(&self.program)` + `args`（`crates/wxml-backend-tsgo/src/native/supervisor.rs:507`） |
| 环境清空，仅 `LANG=C`/`LC_ALL=C` | PASS | `env_clear` + 固定数组（`crates/wxml-backend-tsgo/src/native/supervisor.rs:107`、`crates/wxml-backend-tsgo/src/native/supervisor.rs:511`）；测试断言 `PATH/HOME/NODE_OPTIONS/NODE_PATH/TS_NODE_PROJECT` 均 absent（`crates/wxml-backend-tsgo/src/native/supervisor.rs:2432`） |
| cwd 为空 sandbox | PASS（目录卫生） | 非符号链接、目录、空目录（`crates/wxml-backend-tsgo/src/native/supervisor.rs:604`）；真实引擎握手回显 `currentDirectory=<sandbox>`（§6.2）；**不校验/不设置 mode**，见 B-02 |
| 可执行文件身份 | PASS | 非符号链接、regular、可执行位、SHA-256 匹配固定身份（`crates/wxml-backend-tsgo/src/native/supervisor.rs:643`）；测试 `crates/wxml-backend-tsgo/src/native/supervisor.rs:2045`、`crates/wxml-backend-tsgo/src/native/supervisor.rs:2074` |
| stderr 独立 drain + 有界环 | PASS | 64 KiB ring、8 KiB tail、单行 8 KiB 截断（`crates/wxml-backend-tsgo/src/native/supervisor.rs:68`–`74`、`crates/wxml-backend-tsgo/src/native/supervisor.rs:337`）；flood/截断测试 `crates/wxml-backend-tsgo/src/native/supervisor.rs:2416` |
| stderr/路径不进入用户错误 | PASS（本层） | `JsonRpcError` 只保留 code，message 截断/清洗但不外露（`crates/wxml-backend-tsgo/src/native/rpc.rs:286`、`crates/wxml-backend-tsgo/src/native/rpc.rs:357`）；`NativeError::EngineError` 只含 code（`crates/wxml-backend-tsgo/src/native/mod.rs:144`）；`stderr_tail` 注明仅内部诊断（`crates/wxml-backend-tsgo/src/native/session.rs:294`） |
| close stdin → wait → SIGKILL → reap | PASS | `shutdown_with_timeout`（`crates/wxml-backend-tsgo/src/native/supervisor.rs:1111`）；测试断言 forced_kill（`crates/wxml-backend-tsgo/src/native/supervisor.rs:2298`） |
| kill-on-drop | PASS | `Drop for ProcessSession` kill/wait（`crates/wxml-backend-tsgo/src/native/supervisor.rs:1175`）；背压实测 writer 线程退出（`crates/wxml-backend-tsgo/src/native/supervisor.rs:2247`） |
| 超时 poison | PASS | `recv_until` 超时 `poison()`（`crates/wxml-backend-tsgo/src/native/supervisor.rs:1000`）；测试 `crates/wxml-backend-tsgo/src/native/supervisor.rs:2192` |
| abandoned IDs 有界 | PASS | 8 in-flight / 64 queued / 64 abandoned（`crates/wxml-backend-tsgo/src/native/rpc.rs:834`）；测试 `crates/wxml-backend-tsgo/src/native/rpc.rs:1534`、`crates/wxml-backend-tsgo/src/native/rpc.rs:1559` |
| 子进程退出检测 | PASS | `try_wait`；`mark_ready` 前复查（`crates/wxml-backend-tsgo/src/native/supervisor.rs:1493`）；测试 `crates/wxml-backend-tsgo/src/native/supervisor.rs:2324`、`crates/wxml-backend-tsgo/src/native/supervisor.rs:2331` |
| stderr tail 路径泄漏 | 仅内部证据 | `stderr_tail` 保留路径/栈文本，文档明确不得作为用户可见错误（`crates/wxml-backend-tsgo/src/native/supervisor.rs:1069`；adapter 未使用） |

**LOW 发现（P5-SEC-04）**：`probe_version` 在 `--version` 探针中用 `read_to_string` 读取子进程 stdout，未设上限（`crates/wxml-backend-tsgo/src/native/session.rs:1575`）。可执行文件已按 SHA-256 固定且位于受信安装树，实际风险低；但“hash 校验 → 按路径 spawn”之间存在 TOCTOU 窗口，且该探针没有像帧读取那样的 8 KiB/64 MiB 预算。

**LOW 发现（P5-SEC-08）**：`DROP`/`shutdown` 只 kill 直接子进程，不杀进程组/后代；`shutdown` 对后代持有 pipe 的线程做有界 detach（`crates/wxml-backend-tsgo/src/native/supervisor.rs:1111`–`1160`），这是有意的有界行为，但在 `--runExternalCode` 被误开启或未来引入外部子进程时会留下后代风险。当前没有外部代码路径。

## 5. AST 与句柄

### 5.1 边界与畸形输入

- v9 header 必须 64 字节，metadata 高字节必须为 9；`string_offsets` 必须等于 64；section 顺序、node table 28 字节对齐、binder 范围均校验（`crates/wxml-backend-tsgo/src/native/ast.rs:160`–`239`）。
- 字符串池/节点表上限各 1,000,000（`crates/wxml-backend-tsgo/src/native/ast.rs:201`、`crates/wxml-backend-tsgo/src/native/ast.rs:217`）；测试覆盖短 payload、错误版本、错误 section、binder、非对齐（`crates/wxml-backend-tsgo/src/native/ast.rs:498`）。
- `find_identifier` 是**扁平扫描** `0..nnodes`，不跟随 `next`/`parent`/`data` 链，因此畸形链接不能造成循环；节点范围用源文本 UTF-16 长度校验（`crates/wxml-backend-tsgo/src/native/ast.rs:257`）；测试覆盖越界、错误 data、错误 string index/range（`crates/wxml-backend-tsgo/src/native/ast.rs:583`、`crates/wxml-backend-tsgo/src/native/ast.rs:619`）。
- `decode_base64` 严格字母表/填充/长度校验，容量按输入长度预留，不 panic（`crates/wxml-backend-tsgo/src/native/ast.rs:381`）；测试 `crates/wxml-backend-tsgo/src/native/ast.rs:635`。
- AST 解码输入受 JSON 单字符串 32 MiB 和响应 64 MiB 预算约束；节点表/字符串表不按 `nnodes` 预分配，避免恶意 header 直接放大内存。未发现无界分配或栈递归。

### 5.2 句柄 epoch 绑定与陈旧快照

- `HandleBinding` 保存 `(session_generation, engine_snapshot_id, normalized_file_path, content_hash_or_node_id, node_index, node_kind)`（`crates/wxml-backend-tsgo/src/native/handles.rs:73`）；`is_current` 比较 generation、snapshot、content hash（`crates/wxml-backend-tsgo/src/native/handles.rs:188`）。
- `bind_handle` 从当前 epoch 和 VFS 文件哈希构造绑定，文件不在快照即拒绝（`crates/wxml-backend-tsgo/src/native/session.rs:1234`）；`validate_handle` 在发送前重新检查（`crates/wxml-backend-tsgo/src/native/session.rs:1268`）。
- `update_project` 要求新 snapshot id 不等于当前 id，否则 poison（`crates/wxml-backend-tsgo/src/native/session.rs:656`）；成功后释放旧 snapshot，旧绑定失效；真实引擎测试断言更新后 `validate_handle` 和 `documentation` 都返回 `StaleHandle`（`crates/wxml-backend-tsgo/tests/real_engine.rs:388`–`397`）。
- `call` 对每个请求携带期望 snapshot，响应后 `ensure_snapshot` 比较当前 epoch；不匹配返回 `StaleSnapshot`（`crates/wxml-backend-tsgo/src/native/session.rs:1339`、`crates/wxml-backend-tsgo/src/native/session.rs:1420`）。测试 `crates/wxml-backend-tsgo/src/native/session.rs:1693`。
- `SymbolReferenceBinding` 另外比较 `contentHash`/`parseOptionsKey`/`nodeId`/symbol id/kind（`crates/wxml-backend-tsgo/src/native/handles.rs:266`），测试 `crates/wxml-backend-tsgo/src/native/handles.rs:612`。
- 句柄/符号的构造函数是 `pub(crate)`，外部不能伪造绑定（`crates/wxml-backend-tsgo/src/native/handles.rs:164`、`crates/wxml-backend-tsgo/src/native/handles.rs:356`）。

`query/**` 只做展示映射，不调用 native、不写文件、不发送原始句柄：`crates/wxml-backend-tsgo/src/query/mod.rs:1`；references 明确标记 `Partial` 且不发布未解码范围（`crates/wxml-backend-tsgo/src/query/references.rs:41`）；diagnostics 对越界/不一致范围丢弃并降级（`crates/wxml-backend-tsgo/src/query/diagnostics.rs:80`）；completion 上限 256（`crates/wxml-backend-tsgo/src/query/completion.rs:70`）。

**LOW 发现（P5-SEC-05）**：diagnostics 的 `text` 只做 8 KiB 截断，没有控制字符清洗（`crates/wxml-backend-tsgo/src/query/diagnostics.rs:227`）；documentation 直接返回引擎 comment（`crates/wxml-backend-tsgo/src/query/documentation.rs:34`）。这不是代码执行或文件写入，但若未来直接进入终端/日志，存在 ANSI/控制字符注入的展示风险；建议在 LSP 发布边界统一清洗。

## 6. 执行的测试与工具

工具链：`cargo 1.100.0 (3acc79c0c 2026-11-05)`；`cargo-audit 0.23.0`；`cargo-deny 0.21.0`。引擎：`/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc`（本地固定 7.1.0-beta，测试内部会校验 SHA-256；未下载、未联网）。

### 6.1 单元测试

命令与结果（工作区根目录执行）：

```text
cargo test -p wxml-backend-tsgo --lib -- --test-threads=1
  => 215 passed; 0 failed; 0 ignored; finished in 0.56s; exit 0

cargo test -p wxml-backend-tsgo --lib native:: -- --test-threads=1
  => 111 passed; 0 failed; 104 filtered out; finished in 0.46s; exit 0

cargo test -p wxml-backend-tsgo --lib config:: -- --test-threads=1
  => 5 passed; 0 failed; 210 filtered out; finished in 0.00s; exit 0
```

关键测试名（均在 111/5 的通过集合中）：

- `config::tests::launch_plan_matches_the_frozen_six_argument_argv`
- `config::tests::callback_data_is_consistent`
- `config::tests::allow_and_deny_lists_are_unique_and_disjoint`
- `native::protocol::tests::typed_methods_match_the_frozen_allowlist`
- `native::protocol::tests::typed_methods_do_not_include_legacy_language_service_names`
- `native::vfs::tests::reads_only_virtual_files_and_never_host_files`
- `native::vfs::tests::writes_are_noops_and_unknown_callbacks_are_errors`
- `native::vfs::tests::traversal_and_relative_paths_fail_closed`
- `native::vfs::tests::startup_canary_rejects_a_real_host_file_outside_the_root`
- `native::supervisor::tests::builds_only_frozen_startup_args`
- `native::supervisor::tests::sandbox_root_validation_is_strict`
- `native::supervisor::tests::clears_the_child_environment_to_lang_and_lc_all`
- `native::supervisor::tests::recv_timeout_poisons_the_session`
- `native::supervisor::tests::explicit_shutdown_closes_stdin_then_kills_and_reaps`
- `native::supervisor::tests::process_exit_is_detected_without_blocking`
- `native::supervisor::tests::drains_stderr_and_surfaces_truncation`
- `native::rpc::tests::abandons_requests_and_discards_late_responses`
- `native::rpc::tests::default_budgets_reject_request_seventy_three`
- `native::ast::tests::rejects_wrong_version_short_payloads_and_section_order`
- `native::handles::tests::binding_matches_only_the_same_epoch_and_content`

原始日志：`/tmp/opencode/p5-lib-tests.log`、`/tmp/opencode/p5-native-tests.log`、`/tmp/opencode/p5-config-tests.log`（仓库外，仅辅助）。

### 6.2 真实引擎测试

```text
TSGO_BIN=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc \
WXML_REQUIRE_REAL_ENGINE=1 \
cargo test -p wxml-backend-tsgo --test real_engine -- --nocapture
  => 1 passed; 0 failed; finished in 1.11s; exit 0
```

关键输出：

```text
initialize: caseSensitivity=1 currentDirectory=/tmp/wxml-tsgo-p4-sandbox-2962201-0
snapshot=2 project=Some(".../tsconfig.json") revision=1
diagnostics: semantic=1 syntactic=1 mapped=2 status=Supported
type: text=Some("User") quality=Known
symbol: name=u declarations=1 reference=true status=Supported
documentation: comment=Some("A documented user.") status=Supported
completions: entries=2 status=Partial ... names=["name", "age"]
updated snapshot=3 revision=2 live=1
shutdown: forced_kill=false detached=false stderr_discarded=0
```

该测试同时覆盖：项目外 canary import 必须报 2307（读边界）、运行后宿主 canary 内容不变、虚拟项目根未创建、更新后旧句柄/文档引用返回 `StaleHandle`（`crates/wxml-backend-tsgo/tests/real_engine.rs:112`、`crates/wxml-backend-tsgo/tests/real_engine.rs:241`、`crates/wxml-backend-tsgo/tests/real_engine.rs:388`、`crates/wxml-backend-tsgo/tests/real_engine.rs:433`）。日志：`/tmp/opencode/p5-real-engine.log`。

### 6.3 依赖与供应链

```text
cargo audit --no-fetch --stale
  => exit 0
  => Loaded 1527 security advisories (from ~/.cargo/advisory-db)
  => Scanning Cargo.lock for vulnerabilities (225 crate dependencies)
  => JSON: vulnerabilities=0, warnings=0
  => advisory DB HEAD 233155b4025b854c68a4b199c0e32d7fd53291a2 (2026-11-17)

cargo deny --offline check advisories bans sources
  => exit 0
  => advisories ok, bans ok, sources ok
  => warning[duplicate]: windows-sys 0.52.0 / 0.61.2（非阻断）

cargo deny --offline check
  => exit 4
  => advisories ok, bans ok, sources ok
  => licenses FAILED: 139 个 rejected
  => WARN: unable to find a config path, falling back to default config
```

- `cargo audit` 使用本地 advisory DB，`--no-fetch` 未联网；0 vulnerabilities。
- `cargo deny` 没有 `deny.toml`（`docs/evidence/README.md:47` 已记录“无 deny.toml，licenses 未评估”），默认配置没有 allowlist，导致所有 license 被拒。**这不是依赖漏洞**，但 `cargo deny check` 全量命令不能记为 PASS；license 门禁为 **BLOCKED**。
- CLI 细节：`cargo-deny 0.21` 要求 `--offline` 位于子命令前；`cargo deny check --offline` 被 CLI 以 exit 2 拒绝（unexpected argument），随后执行上面的 `cargo deny --offline check`。
- 未运行完整 workspace 测试、fmt/clippy、Node oracle、性能/长跑；这些不属于本次安全审计的“相关现有测试”，也不在本文件中冒充通过。

## 7. 发现清单（按严重度）

| ID | 严重度 | 发现 | 证据 | 处置建议 |
| --- | --- | --- | --- | --- |
| P5-SEC-01 | **MEDIUM** | B-02 第二层“运行目录最小权限”未实现/未测试：sandbox 只校验空/非符号链接/目录，不设只读、不降权、不校验 mode；进程与 LSP 同 uid/gid。若白名单方法内部触发 AppendFile/Chtimes，写入按宿主权限发生。 | `crates/wxml-backend-tsgo/src/native/supervisor.rs:17`–`19`、`crates/wxml-backend-tsgo/src/native/supervisor.rs:604`–`630`；`docs/reference/tsgo-native-contract.md:345`；ADR `docs/adr/0008-rust-tsgo-no-node-backend.md:31` | 实现并测试只读 sandbox/权限校验/降权之一，或由维护者明确把该层降为部署责任并补 OS 拒绝测试；在此之前不得声称 B-02 补偿控制完整 |
| P5-SEC-02 | LOW | 合同 §4.3 要求 argv + 进程树 + 插件 canary 三者；当前只有 argv 断言 | 合同 `docs/reference/tsgo-native-contract.md:289`–`295`；测试 `crates/wxml-backend-tsgo/src/native/supervisor.rs:1729` | 增加进程树枚举断言与插件 canary（或明确记录为后续门禁） |
| P5-SEC-03 | LOW | `stat` 已注册且形状与固定上游源码一致，但没有仓库内真实引擎 stat 帧/canary；P2 捕获表未覆盖 stat | `crates/wxml-backend-tsgo/src/native/vfs.rs:465`；`docs/evidence/2026-11-17-tsgo-native-api-spike.md:1083` | 增加一次真实引擎 stat 触发/记录测试，或在 F0 证据中补 stat 帧 |
| P5-SEC-04 | LOW | `probe_version` 无界读取 `--version` stdout；hash→按路径 spawn 存在 TOCTOU 窗口 | `crates/wxml-backend-tsgo/src/native/session.rs:1536`–`1583` | 给探针 stdout 设上限；如需更强保证使用已打开 fd/受信安装树校验 |
| P5-SEC-05 | LOW | diagnostics/documentation 文本只截断，不清洗控制字符；若直接进入终端/日志有展示注入风险 | `crates/wxml-backend-tsgo/src/query/diagnostics.rs:227`；`crates/wxml-backend-tsgo/src/query/documentation.rs:34` | 在 LSP 发布边界统一去除/转义控制字符 |
| P5-SEC-06 | LOW | `cargo deny check` 全量失败（licenses），无 `deny.toml` | 本文件 §6.3；`docs/evidence/README.md:47` | 增加 `deny.toml` allowlist，或把门禁明确限定为 advisories/bans/sources |
| P5-SEC-07 | LOW | 缺 append/chtimes 的 OS 拒绝测试；合同 §5.3 要求 | `docs/reference/tsgo-native-contract.md:345`；仅有 `crates/wxml-backend-tsgo/src/native/vfs.rs:926` | 启动真实引擎并以只读目录/AppendFile 假设触发，断言 OS 拒绝而非写成功 |
| P5-SEC-08 | INFO | `Drop` 的 kill 失败时 `wait` 无超时；只杀直接子进程，不杀后代/进程组 | `crates/wxml-backend-tsgo/src/native/supervisor.rs:1175`–`1193` | 保持为有界 shutdown 的已知限制；若未来启用外部代码，增加进程组处理 |

未发现：CRITICAL、HIGH、`unsafe` 块、生产 `std::fs::write`/`File::create`、任意 method 入口、`useOS` 返回、Node/插件加载路径。

## 8. 门禁判定与关闭条件

**总体：BLOCKED（安全门禁），不是“发现可远程利用漏洞”。**

关闭条件（全部满足后重跑对应证据）：

1. **B-02 第二层**：实现只读 sandbox/权限检查/降权中的至少一项，并增加 append/chtimes OS 拒绝测试；或在 ADR/计划中明确将其降为部署责任并由维护者重新接受，同时保留“不得声称回调级 deny”的表述。
2. **外部代码三重验证**：补进程树断言与插件 canary 测试；保留 argv 不含 `--runExternalCode` 的断言。
3. **license 门禁**：补 `deny.toml`（允许 MIT/Apache-2.0/BSD 等）并让 `cargo deny check` 全量通过，或把项目门禁明确限定为 `advisories bans sources` 并记录 licenses 未评估。
4. **建议但不阻塞**：补 stat 的真实引擎 canary；给 `--version` stdout 设上限；清洗 diagnostics/documentation 控制字符。

## 9. 证据边界

- 本次是只读审计；唯一仓库写入为本文件；未修改生产代码、测试、ADR、合同、server/config；未 commit。
- 未审计：完整 `wxml-lsp` adapter/project host/symbol/shadow/script/delivery、安装器、Node oracle、性能与长跑；这些不能由本文件推断为 PASS。
- `/tmp/opencode` 下的上游源码与日志仅作辅助核对；B-02 的仓库级依据是 ADR/合同/P2 证据，stat 形状另有固定上游源码对照（`callbackfs.go` 的 `Stat` 解码字段 `mode/size/mtime`），但仓库内仍缺 stat 实测帧。
- 真实引擎测试使用固定 7.1.0-beta 本地二进制；测试内部校验 SHA-256，未联网、未下载。缺失工具/依赖时本应记 BLOCKED；本次 `cargo-audit`/`cargo-deny` 均可用，故 license 失败是配置事实而非工具缺失。
- 测试期间工作区出现另一未跟踪文件 `crates/wxml-backend-tsgo/tests/perf_budget.rs`（非本审计创建，未修改/未删除）；本审计未运行该测试。
