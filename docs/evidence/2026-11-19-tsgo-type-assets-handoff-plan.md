# P6-4b 生产 adapter type-assets handoff 设计（计划，非完成声明）

- 记录日期：2026-11-19
- 对象：worktree `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD：`576c231`（`docs(plan): record native hover gate increment`）
- 锚点取法：`git show 576c231:<path>`。工作树 document-symbol WIP 已修改 `backend_tsgo/mod.rs`、`document_symbol.rs`、`mapping.rs`、`tests.rs` 等，编辑器行号可能偏移；冲突定位以**函数名**为准。
- 边界：只读审计 + 本文。未修改代码/测试/`Cargo.toml`/合同/ADR/计划正文；未运行 `cargo`、未启动引擎、未联网、未提交。

**状态：本切片不关闭 P6-3/F3/P6/G5/G6，不切换默认后端。**
本文只描述“生产 adapter 使用 trusted type assets”的最小接线；不把 `--backend native` 变默认，不删除 Node，不伪造真实包/Zed/F3 验收。

## 目录

1. [结论](#1-结论)
2. [范围与不变量](#2-范围与不变量)
3. [事实基线与固定 pin](#3-事实基线与固定-pin)
4. [接线设计](#4-接线设计)
5. [身份、update 与 restart](#5-身份update-与-restart)
6. [失败映射](#6-失败映射)
7. [manifest 验证缺口](#7-manifest-验证缺口)
8. [EnvironmentDegradation](#8-environmentdegradation)
9. [文件清单与冲突面](#9-文件清单与冲突面)
10. [测试计划 E1–E10 与真实 gate](#10-测试计划-e1e10-与真实-gate)
11. [残留风险](#11-残留风险)
12. [不做的项](#12-不做的项)
13. [本切片完成判定](#13-本切片完成判定)

---

## 1. 结论

1. **只加载一次**：在 `TsgoBackendAdapter::with_install` 中调用 `TypeAssets::load_pinned(install.standard_lib_root, install.typings_root)`，存为 adapter 字段 `Environment::Trusted(Arc<TypeAssets>)`。`run_stdio_native` 只做编排。
2. **身份**：`RequestIdentity` 增加 `environment: Option<TypeEnvironmentIdentity>`，直接复用 backend 类型；不自造 digest、不新增 `sha2` 依赖。
3. **options 变化必须 restart，不能 update**：`update_project` 要求环境身份完全相等，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:2695 )。旧计划“options 变化走 update”与实现冲突。
4. **adapter 必须在 spawn 前拒绝 Trusted→Legacy**：`open_project` 不校验环境，只记录 `None` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:692 、:739 )，不能依赖 backend 兜底。
5. **manifest `typeAssets` 本切片不校验**：运行时资产身份由 crate pin 负责；manifest 交叉验证属于 P6-3-4/F3-03。
6. **degradation 只接 hover/diagnostics/completion/definition**；`document_symbols` 不接（纯语法结构，接了会产生 false `Partial`）。
7. 默认后端仍是 Node：`--stdio` 仍构造 `NodeBackend` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:184 )，`--backend native` 仍是 opt-in。

---

## 2. 范围与不变量

**做**：生产 adapter 在首个请求前完成一次可信资产加载；生产 `RequestPlan` 走 `AnalysisInput::with_environment`；资产/选项变化正确重启；资产失败在启动期 fail closed；把 VFS 环境降级映射到四类查询的 `Partial`。

**不变量**：

- `AnalysisInput::new` 保持 legacy 语义，仅测试/probe seam 使用；生产 adapter 不可能构造出 legacy input。
- 资产只从 `DiscoveredInstall` 的两个根读取，不读 `PATH`/cwd/环境变量/项目 `node_modules`。
- 不启动 Node，不探测 `server.js`。
- `js_check` 仍为 `Inherit`，本切片不改（§12）。

---

## 3. 事实基线与固定 pin

| 事实 | 锚点 |
| --- | --- |
| `NativeStartupError` 仅 Discovery/Sandbox/Server | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:114 ) |
| `run_stdio_native` / `with_install` | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:136 )、:197 |
| adapter 字段 / `new` | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:153 )、:224 |
| `prepare` / `ensure_open` / identity 短路 / spawn | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:252 )、:297、:304、:354 |
| `RequestIdentity` / `RequestPlan` / `input()` | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:74 )、:101、:120 |
| degradation 通道 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:207 ) |
| `DiscoveredInstall` / 根构造 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/engine.rs:59 )、:197 |
| manifest 投影解析但丢弃资产字段 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/engine.rs:499 ) |
| `load_pinned` | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/asset_loader.rs:177 ) |
| `with_environment` / `TypeEnvironmentIdentity` | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:1301 )、:789 |
| VFS 环境 tsconfig / degradation 类型 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:950 )、:63 |
| session 暴露 degradations | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:446 ) |
| `QuerySession` seam | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:60 ) |
| 每 session 新建沙箱 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/sandbox.rs:333 ) |

固定 pin（不得从被测树反推）：

- stdlib：116 文件 / 3,870,196 B / `5e126e7206065b6fff9d651eff3ad66b9413804d3e23d7ecfb4257299af3e85e`
  ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:154 )；
- typings：20 文件 / 2,522,752 B / `821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9`
  ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:166 )；
- 合计 136 / 6,392,948 B ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:143 )；预算 256 文件 ( :110 )、8 MiB ( :116 )。

---

## 4. 接线设计

### 4.1 `Environment`

新增 `crates/wxml-lsp/src/backend_tsgo/environment.rs`：

```text
enum Environment { Trusted(Arc<TypeAssets>), Legacy }
```

- 手写 `Debug`，只打印 `trusted(<file_count> files)` / `legacy`，避免把 136 个文件文本打进日志。
- `is_trusted()`；`Legacy` 只允许测试/旧 seam 构造。
- **不把 `CompilerOptions` 放进 `Environment`**：options 是每个 view/项目的，资产是每个安装的；混在一起会跨项目错误复用。

### 4.2 `with_install` 一次加载

`with_install` 改为返回 `Result<Self, NativeStartupError>`：

```text
1. environment = load_environment(install)?          // NativeStartupError::Environment
2. sessions   = SandboxSessionFactory::new(...)?     // NativeStartupError::Sandbox
3. Self::new_with_environment(pinned, views, sessions, environment)
```

先加载资产再做沙箱：纯只读失败时不会留下临时目录。

`load_environment`：

- 断言 `install.standard_lib_root == install.engine.parent()`，失败为 `EnvironmentLoadError::EngineRootMismatch`（当前恒真，防未来漂移）。
- 调 `TypeAssets::load_pinned(&install.standard_lib_root, &install.typings_root)`。
- **不要**传 `typings/miniprogram-api-typings`：loader 自己进入该子目录 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/asset_loader.rs:240 )。`DiscoveredInstall.typings_root` 已是 `<package>/typings`，二者一致。

新增 `new_with_environment(...)`；现有 `new` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:224 ) 委托为 `Legacy`，所以 `tests.rs` 的 8 处 `new(...)` 不用改。`with_host`(:177) 也保持 `Legacy`。`prepare` 把 `&self.environment` 传给 `build_request_plan`。

### 4.3 `RequestPlan` 与 `input()`

`RequestPlan` 增加 `environment: Environment` 和 `compiler_options: CompilerOptions`。`input()`：

- `Trusted(assets)` → `AnalysisInput::with_environment(..., compiler_options.clone(), Arc::clone(assets))`；
- `Legacy` → 现有 `AnalysisInput::new`。

**没有 `Option` 回退**：`Trusted` 分支不允许在资产缺失时退回 legacy。

### 4.4 缓存与生命周期

- 每个 adapter 进程加载一次。`TypeAssets` 的文件文本是 `Arc<str>`；`RequestPlan` 只克隆 `Arc<TypeAssets>`，每请求不读盘、不 hash。
- 不是每 session 一次：session 会因 epoch/失败重启（`sandbox.rs:333`），资产属于安装代。
- 资产不复制进沙箱；VFS 是内存快照 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:180 )。
- `invalidate` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:527 ) 只丢 session，不丢资产。
- 验证复用：测试用 `Arc::ptr_eq(plan.input(rev)?.type_assets().unwrap(), injected)`。

---

## 5. 身份、update 与 restart

### 5.1 `RequestIdentity.environment`

在 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:74 ) 增加 `environment: Option<TypeEnvironmentIdentity>`：

- `Trusted`：`Some(TypeEnvironmentIdentity::new(&options, assets))` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:805 )；
- `Legacy`：`None`。
- 该类型已含 options digest、两个根、116/20、字节数、两个 bundle digest，且正是 update 比较的同一类型；不要自造字符串。

### 5.2 决策表

| 变化 | 动作 |
| --- | --- |
| identity 全等 | 复用，不发请求 |
| 仅源码/view 变化，environment 相等 | `update_project` |
| **environment 任一字段变化（含 options digest）** | shutdown + `start_and_open` |
| project/root 变化 | shutdown + `start_and_open` |
| update 失败 / fatal / epoch 失效 | shutdown + 重启（现逻辑） |

实现：`same_project` 增加 `open.identity.environment == plan.identity.environment`。不要“先 update 等 backend 拒绝再重启”——多一次协议往返且掩盖意图。

**关键修正**：`validate_environment_transition` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:2695 ) 对 `(Some, Some)` 要求完全相等；options digest 不同即拒绝。因此 tsconfig 变化会重启 session。这是明确取舍：正确性优先、频率低；放宽 backend 身份需 backend owner 单独评审。

### 5.3 杜绝静默降级（四层）

1. **构造层**：`with_install` 是唯一生产构造器；资产失败直接 `NativeStartupError`。`Legacy` 只能由 `new`/`with_host` 产生，`run_stdio_native` 不调用。
2. **Plan 层**：`RequestPlan.environment` 是枚举不是 `Option`；`Trusted` 无回退。
3. **spawn 前 guard**：`ensure_open` 在 `plan.input(revision)?` 之后、`sessions.start()` 之前检查 `Trusted && !input.has_type_environment()` → `BackendError::Unavailable("native type assets are not loaded")`；测试断言 `starts == 0`。
4. **backend 最后一道**：transition 拒绝 → `NativeError::InvalidParams` → `BackendError::Protocol` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:665 )，被 `with_session` 视为 fatal，session drop 且错误返回；不会重试成 legacy。

### 5.4 测试如何构造 env→legacy

- **Plan 级**：同一 `RequestContext`/fake view，分别用 `Trusted(fake_assets)` 与 `Legacy` 调 `build_request_plan`：`identity.environment` 为 `Some/None`，`has_type_environment()` 为 `true/false`。
- **Adapter 级**：`new_with_environment(..., Trusted(fake_assets))` 后，在 `backend_tsgo::environment::tests`（后代模块可访问私有 `ensure_open`/`AdapterState`）直接传 `Legacy` plan：断言 `Unavailable` 且 `starts == 0`。
- **重启而非 update**：fake view 返回不同 `compiler_options`，连续两次请求：`starts == 2`、`updates == 0`。

---

## 6. 失败映射

**两层错误域**：

- 启动层（未进入 `initialize`）：`EngineDiscoveryError`/`EnvironmentLoadError`/`SandboxError` → `NativeStartupError` → stderr + 非零退出；不构造 `NodeBackend`。
- 请求层：`NativeError` → `BackendError`；`diagnostics` 返回语法诊断 + `Unavailable/Partial`。

| 场景 | 触发点 | 结果 | 层 |
| --- | --- | --- | --- |
| 包根缺失/不安全 | discovery | `NotInstalled/PackageUnsafe` → `NativeStartupError::Discovery` | 启动 |
| `engine/lib` 缺失 | discovery | `PackageUnsafe` → `Discovery` | 启动 |
| typings 根缺失/非目录 | `load_environment` | `EnvironmentLoadError::TypingsRootMissing` → `NativeStartupError::Environment` | 启动 |
| pin 不匹配（116/20/字节/digest） | `TypeAssets::new` | `PinMismatch { field }` → `Environment` | 启动 |
| 必需声明缺失 | `TypeAssets::new` | `Incomplete` → `Environment` | 启动 |
| symlink/hardlink/非普通文件 | loader | `UnsafeTree` → `Environment` | 启动 |
| 文件读取中被替换 | loader | `Unreadable`/`PinMismatch` | 启动 |
| manifest `typeAssets` 不匹配 | **不检查** | 见 §7；将来 `ManifestMismatch` → `Discovery` | 启动/未来 |
| 启动后磁盘资产被替换 | 无检测 | 内存快照为准；新进程重新加载 fail closed | — |
| 引擎 digest 在 session start 不符 | `NativeSession::start` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:378 ) | `Unavailable` → `BackendError::Unavailable`，session drop | 请求 |
| VFS 资产撞项目文件/重复 | `map_vfs` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:2714 ) | `InvalidParams`/`Protocol`/`Unavailable(MissingAsset)` | 请求 |
| Trusted adapter + legacy input | adapter guard | `BackendError::Unavailable`，`starts == 0` | 请求前 |
| 父/子资产根重叠 | `TypeAssets::new` | `EnvironmentLoadError::Layout` | 启动 |

`EnvironmentLoadError` 的 `Display` 不得含宿主路径、IO 源错误或 option 值。`TypeAssetError` 许多变体带路径（`MissingRoot{root}`、`HashMismatch{path}` 等），映射时必须丢弃 path，只保留稳定分类/字段名；测试断言 `Display` 不含 `/`。

---

## 7. manifest 验证缺口

### 7.1 本切片不新增 manifest 资产校验

1. 运行时不用 manifest 的 `typeAssets` 加载资产；`load_pinned` 用 crate pin 直接对磁盘重算。manifest 谎报资产也无法放宽实际挂载闭包。
2. P6-4 计划要求 LSP 不复制 `MINIPROGRAM_TYPINGS_*`/`STANDARD_LIB_*` 常量；在 `engine.rs` 再写一套资产校验会越界并漂移。
3. typings 三套摘要（26 上游 / 23 payload `ba3eb8…` / 20 runtime `821c93…`）仍冲突；任何“manifest 摘要 == runtime pin”检查在真实包上都会失败。这是 F3-03。

唯一新增的本地不变量是 §4.2 的根一致性断言；它不是 manifest 校验。

### 7.2 留给 P6-3-4 / F3-03

| 字段 | 现状 | 归属 |
| --- | --- | --- |
| `typeAssets[].name/root/include/version/license` | 解析后丢弃 | P6-3-4（共享 manifest 投影） |
| `typeAssets[miniprogram].fileCount/sha256` | 丢弃；23/26/20 未裁决 | F3-03，禁止本切片比较 |
| `typeAssets[standard-lib].fileCount/sha256` | 未校验 | P6-3-4；本切片由 loader 从磁盘验 |
| `binaries[]`（rust/native-engine/zed） | 丢弃 | P6-4b/c；LSP 不执行这些二进制 |
| `builtAt` | 只当字符串 | P6-4b；对资产无运行时影响 |
| `engine.distributionUrl/distributionSize` | 未校验 | 安装器 provenance，F3 |
| `files[]` 除 engine 外条目 | 未校验 | P6-3-4 / 安装器 |

### 7.3 不改名

**不要改名为 `VerifiedInstall`。** 现在改名会把“engine 已验、资产未验、manifest 资产未交叉验证”说成“全量验证”。保留 `DiscoveredInstall`，只更新文档；若必须体现进度，最多 `EngineVerifiedInstall`。`VerifiedInstall` 留给 P6-3-4 + P6-4b/c 之后。

---

## 8. EnvironmentDegradation

`EnvironmentDegradation` 只在 environment 输入且 `typeRoots`/`baseUrl`/`paths` 指向快照外时产生 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:950 )；pinned 资产本身不产生。字段/原因都是 `&'static str`，天然路径无关。

传递路径：

1. `wxml-backend-tsgo/src/native/vfs.rs` 增加纯函数 `environment_degradation_summary(&[EnvironmentDegradation]) -> Option<String>`，输出如 `the native type environment is partial: typeRoots (...); paths (...)`；不包含用户值（`baseUrl` 可能含宿主路径）。
2. LSP `QuerySession` 增加**带默认实现**的 `environment_degradation(&self) -> Option<String> { None }`；`LiveQuerySession`/`SandboxedSession` 转发真实值。默认实现让 `tests.rs` 的 fake 不用改。
3. `AdapterState` 增加 `environment_reason: Option<String>`；`ensure_open` 成功后刷新，失败/重启/失效时清空。degradation 属于“当前打开的 session 的 VFS”，不是 plan 静态属性。
4. adapter 用现有 `merge_reason` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:540 ) 合并 `plan.degradation` 与 `environment_reason`。

| 查询 | 是否接 | 接法 |
| --- | --- | --- |
| `hover` | 是 | `resolved_status` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/hover.rs:100 ) |
| `diagnostics` | 是 | `merge_status` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/diagnostics.rs:136 ) |
| `completion` | 是 | `merge_status` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/completion.rs:129 ) |
| `definition` | 是 | `mark_partial` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/definition.rs:119 ) |
| `document_symbols` | **否** | 纯语法结构；接了会产生 false `Partial`，只传 `plan.degradation` |
| `symbols` | 否 | 已显式 `Unsupported` |

残留缺口：`typeRoots` 在项目根内但目录未被 VFS 挂载时，VFS 只做前缀判断 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:1048 )，仍可能得到 2688；关闭它属于 P6-3D/T5，本切片不制造假 `Supported`。

---

## 9. 文件清单与冲突面

### 9.1 生产改动

| 文件 | 改动 | 冲突风险 |
| --- | --- | --- |
| `backend_tsgo/environment.rs`（新增） | `Environment`、`EnvironmentLoadError`、`load_environment`、测试 | 无 |
| `backend_tsgo/input.rs` | `RequestIdentity.environment`、`RequestPlan.environment/compiler_options`、`input()` 分支、`build_request_plan(..., &Environment)`、options 防御过滤 | 无（不在 WIP 列表） |
| `backend_tsgo/session.rs` | `QuerySession::environment_degradation` 默认方法 + `LiveQuerySession` 转发 | 无 |
| `backend_tsgo/sandbox.rs` | `SandboxedSession` 转发 | 无 |
| `native/vfs.rs`（backend） | `environment_degradation_summary` + 单测 | 无（不是 `shadow/mapping.rs`） |
| `backend_tsgo/mod.rs` | 字段、构造器、`prepare`、`with_session` 委托、`ensure_open`、guard、diagnostics | **有**，见 §9.2 |
| `wxml-lsp/src/lib.rs` | 重新导出 `EnvironmentLoadError`（:69 同处） | 无 |
| `backend_tsgo/engine.rs` | 仅测试调整（fake 包无资产、真实 gate 需 typings） | 无 |
| `wxml-lsp/src/main.rs` | 仅 usage 文案“Type assets are not wired yet”（:31） | 可选；建议单独提交 |

**不改**：`wxml-backend-tsgo/src/input.rs`、`asset_loader.rs`、`native/session.rs`、`wxml-delivery/*`、`Cargo.toml`。

### 9.2 `backend_tsgo/mod.rs` 最小冲突面

document-symbol WIP 已改 `mod.rs`（`SYMBOLS_UNSUPPORTED` 文档，HEAD :147 附近）。为避免重叠：

- **改**：字段/构造器（HEAD 约 153–238）、`prepare`(:252)、`with_session`(:256)、`ensure_open`(:297)、`start_and_open`(:354)、`AdapterState`/`OpenProject`(:580/:598)、`hover`/`diagnostics`/`definition`/`completion` 调用点（:386/:404/:453/:467）。
- **不改**：`symbols`/`document_symbols`（:496–525）。保留 `with_session` 原签名，内部委托给新的 `with_session_environment(..., |session, reason| ...)`；`document_symbols` 继续调 `with_session`，调用点字节级不变。
- **不改**：`SYMBOLS_UNSUPPORTED` 文案（并行会话刚改过）。

---

## 10. 测试计划 E1–E10 与真实 gate

> 全部是计划，未运行。fake 资产集：116 stdlib `.d.ts` + 20 typings `.d.ts`（含 `lib.d.ts`、`index.d.ts`、`types/index.d.ts`），用 `TypeAssetsPin::new("5.3.1", 116, <fake bytes>, <fake digest>, 20, <fake bytes>, <fake digest>)`。**不伪造生产摘要**，只断言计数、identity 传播和 Arc 复用。

| # | 测试 | 断言 |
| --- | --- | --- |
| E1 | `trusted_plans_share_one_asset_arc` | 两次 `plan.input()` 的 `type_assets()` 与注入 `Arc` `ptr_eq`；`has_type_environment()` 为 true |
| E2 | `trusted_plan_never_builds_legacy_input` | `input.type_assets().is_some()`；`identity.environment.is_some()` |
| E3 | `request_identity_tracks_options_and_assets` | 同 revisions、不同 options digest → `identity !=`；不同资产 → `!=`；完全相同 → `==` |
| E4 | `trusted_adapter_rejects_a_legacy_plan_before_start` | `Unavailable`；fake factory `starts == 0` |
| E5 | `environment_change_restarts_instead_of_updating` | 两次请求不同 options → `starts == 2`、`updates == 0` |
| E6 | `source_change_updates_without_reopening` | 同环境、仅 view revision 变化 → `starts == 1`、`updates == 1` |
| E7 | `environment_degradation_marks_hover_partial_but_not_document_symbols` | fake session 返回固定 reason；hover `Partial` 含该 reason；document_symbols 不含 |
| E8 | `load_environment_rejects_missing_typings_root` | `TypingsRootMissing`；`Display` 不含 `/` |
| E9 | `load_environment_maps_pin_and_tree_failures_without_paths` | test-only `load_environment_with_pin`：缺 `lib.d.ts` → `Incomplete`；多一个 `.d.ts` → `PinMismatch{field}`；symlink → `UnsafeTree`；根不一致 → `EngineRootMismatch`；Display 均无宿主路径 |
| E10 | `with_install_fails_before_creating_a_session` | 空 typings 的 fake install → `NativeStartupError::Environment`；无 session 启动 |

`engine.rs` 测试调整：

- `with_install_builds_a_managed_adapter_from_a_discovered_install` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/engine.rs:792 )：fake 包只有空 typings，改为断言 `NativeStartupError::Environment`。
- `optional_real_engine_starts_a_native_session_from_a_discovered_install` (:1185)：必须同时有 `TSGO_ASSET_ROOT`（或 `TSGO_TYPINGS_ROOT`）提供真实 typings；`WXML_REQUIRE_REAL_ENGINE=1` 时缺任何一项硬失败，否则打印 skip。

### 10.1 真实 gate

```bash
TSGO_ASSET_ROOT=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package \
TSGO_BIN=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc \
WXML_REQUIRE_REAL_ENGINE=1 \
cargo test -p wxml-lsp --lib backend_tsgo::environment::tests::optional_real_install_loads_pinned_assets -- --nocapture
```

断言：stdlib 116 / 3,870,196 B / `5e126e7206065b6fff9d651eff3ad66b9413804d3e23d7ecfb4257299af3e85e`；typings 20 / 2,522,752 B / `821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9`；合计 136 / 6,392,948 B；随后 `with_install` 成功并 `start_and_stop_session_for_test()` 成功。

`TSGO_BIN` 在本仓库约定里指向 npm 布局 `<package>/lib/tsc`；安装布局是 `<package>/engine/lib/tsc`。gate helper 必须同时接受两种根（参考 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/asset_loader.rs:930 )），typings 同时接受 `<...>/typings` 与 `<...>/typings/miniprogram-api-typings`。

### 10.2 不要新造 CLI 测试

`tests/cli.rs::native_without_install_fails_closed` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/cli.rs:64 ) 仍有效。不要伪造“资产坏掉的安装包”CLI 测试——`discover_from_current_exe` 固定要求真实引擎 digest；资产失败由 E8–E10 + 真实 gate 覆盖。

---

## 11. 残留风险

1. **options 变化会重启 session**：tsconfig 编辑后的首次请求多一次 spawn；正确性优先。
2. **启动后磁盘资产被替换不会被运行中进程发现**：内存快照为准，保证进程内资产代稳定；新进程重新加载并 fail closed。
3. **manifest `typeAssets` 未交叉验证**：说谎不会被 runtime 发现，但也不会削弱 runtime pin；F3-03 关闭前不能声称交付链完整。
4. **`typeRoots` 在项目根内但未挂载**：可能 2688，degradation 不记录（§8）。
5. **`js_check` 仍为 `Inherit`** ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:202 )：compilerOptions 只做了转发 + 防御过滤，`WXML_TEMPLATE_CHECK` 映射仍属 P6-3D。
6. **真实 gate 未运行**：本文不产生 P6-4b/F3/P6 通过声明。
7. **行号漂移**：document-symbol WIP 正在改 `mod.rs`/`tests.rs`，以函数名为准。

---

## 12. 不做的项

- 不切换默认后端；`--stdio` 仍构造 `NodeBackend`；不删除/修改 Node。
- 不改 `server.rs`、`scheduler.rs`、`structure.rs`。
- 不改 `backend_tsgo/document_symbol.rs`、`mapping.rs`、`tests.rs`。
- 不改 delivery asset lock、`assets.lock.json`、packager/installer、`wxml-delivery`。
- 不改 `backend-tsgo` 的 `input.rs`/`asset_loader.rs`/`native/session.rs`。
- 不固定 `js_check`，不实现 `WXML_TEMPLATE_CHECK → JsCheckPolicy`（P6-3D）。
- 不做 manifest `typeAssets`/`binaries`/`builtAt` 校验（P6-3-4/F3-03）。
- 不比较 23/26/20 三套 typings 摘要。
- 不伪造真实包、Zed、F3、P6、G5、G6 验收；不声称默认已切换。

---

## 13. 本切片完成判定

1. 生产 `run_stdio_native` 路径只可能得到 `Environment::Trusted`；`AnalysisInput.has_type_environment() == true`。
2. E1–E10 全绿；`cargo test -p wxml-lsp --lib backend_tsgo` 通过。
3. 资产缺失/不匹配时进程在 `initialize` 前非零退出；无 Node 探测、无 legacy 回退。
4. options/资产变化触发 restart，源码变化仍走 update。
5. out-of-VFS `typeRoots/baseUrl/paths` 时四类查询返回 `Partial` 且 reason 路径无关；document_symbols 不受影响。
6. 默认后端、Node、交付/安装器、`assets.lock.json` 均无改动。

**本切片完成也不等于**：P6-3 完成、P6-4b 真实包验收、F3-03 关闭、P6/G5/G6 通过、默认后端切换。真实 gate 未运行前，只能描述为“设计 + fake 测试计划”。
