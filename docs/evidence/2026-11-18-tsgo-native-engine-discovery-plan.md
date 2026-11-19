# P6-4 原生 tsgo 引擎发现与可启动性实施计划（审计，非完成声明）

## 0. 审计身份、边界与状态

- 审计对象：worktree `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`，HEAD `558beb9`。
- 审计方式：只读源码、合同、交付代码、现有测试与 `git show HEAD:...` 快照；未运行 `cargo`、`npm`、`strace`、Zed，未启动真实引擎。除本文外未新增/修改任何文件；未 commit。
- 工作树被其他会话持续推进：审计开始时 `git status --short` 显示 `crates/wxml-delivery/src/{bin/wxml-lsp-package.rs,error.rs,native_install.rs,package.rs}`、`docs/reference/tsgo-delivery-contract.md` 已修改，`crates/wxml-delivery/tests/{native_package.rs,native_rollback.rs}` 未跟踪；审计结束前又出现 `crates/wxml-backend-tsgo/src/{input.rs,native/session.rs,native/vfs.rs}` 的修改。本文所有行号以 **HEAD `558beb9`** 为准；关键文件用 `git show HEAD:...` 核对，未提交改动不作为 P6-4 的证据。
- 状态：**P6-4 BLOCKED / PARTIAL**。
  - P6-4a（引擎发现 + 沙箱 + opt-in 选择器）：可实施，但当前**未实施**，因此不能声称可启动。
  - P6-4b（带 stdlib/typings 的真实启动）：**BLOCKED**，依赖 P6-3 的 `AnalysisInput`/VFS/资产 loader。
  - P6-4c（真实安装包 + no-Node 进程审计 + Zed 冒烟）：**BLOCKED**，依赖交付 v2 真实包、F3-01/02/14/15 和 P6-8/C3 的 Zed 选择能力。
  - 默认后端切换、Node 删除、G6、G5 归零：不属于 P6-4；仍是 P6-8/P7 的范围。
- 未执行任何测试；本文所有“通过/失败”均不是本次审计产生的结果。
- 路径约定：正文/表格中的代码锚点默认写绝对路径；§4/§6 的文件列为了可读性使用相对路径，绝对锚点见 §10。

**明确不声称：** 不切换默认后端；不删除 Node；不关闭任何 F3 ID；不声称 AC-001..AC-013 已通过；不声称 Zed 原生冒烟已完成；不声称 Windows/macOS/aarch64 支持。

---

## 1. 简明结论

**当前安装的 `wxml-lsp` 不能启动原生 tsgo 引擎，缺口是一条完整链路，而不是单个函数：**

1. `--stdio` 只构造 `NodeBackend`，没有任何配置/参数路径选择原生后端：
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:10`、
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:56-77`、
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:93-99`。
2. 原生适配器存在但不可被 bin 构造：`backend_tsgo` 是 `pub(crate)`，且模块显式标注 inert：
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:44-46`、
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:59-62`、
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:95-100`、
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:129-138`。
3. **没有引擎发现**：没有从运行中的可执行文件定位包根、读取 `manifest.json`、解析 `engine.binary`、校验 `engine/lib/tsc` 的生产代码。LSP 内唯一的 `current_exe` 是 Node 后端对 `server.js` 的探测：
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/config.rs:229-241`。
4. **没有 sandbox 创建**：`SessionOptions` 只接收调用方给的 `executable` + `sandbox_root`；`EngineSupervisor` 只校验“存在、空、非 symlink”，不创建、不清理：
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:80-96`、
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/supervisor.rs:604-630`。
5. **没有类型资产 handoff**：`AnalysisInput` 只有 project/root/revision/files；VFS 合成 tsconfig 固定 `noLib:true`；适配器只读 `checkJs`，没有 compilerOptions/typings：
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:212-294`、
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:685-696`、
   `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:193-205`。

**P6-4 的最小可交付顺序是：**（a）发现 →（b）沙箱 →（c）opt-in 选择器 →（d）失败模式 →（e）类型资产 handoff（依赖 P6-3）→（f）真实安装包/进程审计/Zed 冒烟。前 4 步可以在不改变默认 `NodeBackend` 的前提下落地；没有 P6-3 和交付 v2 真实包，P6-4 不能声称“从安装包启动并具备 `wx`/标准库类型”。

---

## 2. 当前事实与源码锚点

### 2.1 默认后端与入口

| 事实 | 锚点 |
| --- | --- |
| `--stdio` 无额外参数 → `serve_real()` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:66` |
| `serve_real()` 只构造 `NodeBackend::new(NodeBackendConfig::from_env(), host)` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:93-99` |
| usage 只列 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`/`WXML_ANALYZE_TIMEOUT_MS`/`WXML_TEMPLATE_CHECK` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:15-36` |
| 参数解析只接受一个 `extra`，当前只识别 `--mock` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:56-77` |
| `backend_node` 是 `pub mod`，`backend_tsgo` 是 `pub(crate) mod`，注释写明惰性 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:44-46` |
| `run_stdio`/`run_stdio_with_host`/`run_with_host` 接收 `Arc<dyn Backend>`，本身没有选择逻辑 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/server.rs:413-447` |

### 2.2 原生适配器与引擎 pin

| 事实 | 锚点 |
| --- | --- |
| 适配器 `TsgoBackendAdapter` 是 `pub(crate)`，字段含 `PinnedEngine`、view/session seams | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:95-112` |
| 生产构造器 `with_host(host, SessionOptions)`；调用方必须自己给出可执行路径和 sandbox | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:129-138` |
| 适配器内部 `PinnedEngine::pinned()`，但不做包根/manifest 发现 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:133-137` |
| 生产 session factory 直接 `NativeSession::start(self.options.clone())` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:108-125` |
| `NativeSession::start` 依次校验 pin SHA、`engine/lib/tsc --version`、沙箱 canary、spawn、handshake、schema probe | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:296-329` |
| `probe_version` 在可执行文件目录运行 `--version`，`env_clear` + `LANG/LC_ALL` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1935-1981` |
| `confinement_self_check` 用引擎可执行文件作为 canary，证明 VFS 不能读到沙箱外真实文件 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1923-1933` |
| `PinnedEngine` 冻结版本、发行包、`engine/lib/tsc`、可执行文件 SHA-256、AST v9/header 64 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/config.rs:19-45`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/config.rs:197-234` |
| `launch_plan` 校验绝对、无 `.`/`..`、无控制字符的 sandbox，并生成冻结 argv | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/config.rs:356-375` |
| 适配器 `capabilities()` 固定 `Partial`；`symbols()` 固定 `Unsupported` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:414-419`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:441-465` |

### 2.3 沙箱与进程监督

| 事实 | 锚点 |
| --- | --- |
| `SessionOptions` 只有 `executable` + `sandbox_root` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:78-96` |
| `EngineSupervisor::spawn_with_executable` 要求 sandbox 已存在、空、非 symlink、非空目录失败 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/supervisor.rs:604-630`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/supervisor.rs:1454-1462` |
| 子进程 `env_clear`，仅 `LANG=C`/`LC_ALL=C`，cwd 为 sandbox | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/supervisor.rs:107-108`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/supervisor.rs:507-519` |
| `verify_executable` 拒绝 symlink/非普通文件/无可执行位，并流式校验 SHA-256 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/supervisor.rs:638-685` |
| `NativeSession` 存储 sandbox 路径；`shutdown` 释放 snapshot 后关闭进程 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:361-365`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:852-858` |

### 2.4 交付安装树与 manifest 身份

| 事实 | 锚点 |
| --- | --- |
| `InstallRoot` 包含 `versions/`、`cache/`、`current`；`current_version` 只接受相对 `versions/<v>` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/install.rs:17-40`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/install.rs:196-249` |
| `CurrentInstall` 区分 None/Legacy/Native；`inspect_current` 只读，不执行旧包 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/native_install.rs:275-305`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/native_install.rs:346-386`（HEAD 行号） |
| manifest v2 固定 `engine.version=7.1.0-beta`、`engine.binary=engine/lib/tsc`、protocol `2`、API schema `9`、AST header `64` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:41`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:60-86`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:330-414` |
| `ManifestV2` 的 `engine`/`type_assets`/`binaries`/`files` 是公开字段；`validate` 校验全部身份 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:200-221`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:266-273` |
| `read_manifest_v2` 只读 `manifest.json`；`read_limited` 拒绝非普通文件、超 4 MiB | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:856-860`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest.rs:571-599` |
| `verify_tree_v2` 精确校验 `files[] ∪ {manifest.json, SHA256SUMS}`、模式、摘要、类型资产 bundle、Node 运行时负向规则 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:42-135` |
| `check_native_engine` 校验引擎普通文件、可执行位、大小、manifest SHA-256 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:350-382` |
| `state.json` 记录 active/previous/engine 身份；`validate_against` 要求 current/active/manifest 一致 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/state.rs:41-91`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/state.rs:262-318` |
| 安装/回滚只会在失败清理时删除 staging；`remove_empty_versions` 只删除空 `versions/`，不裁剪已安装版本 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/native_install.rs:1262-1268`（HEAD 行号） |

### 2.5 合同要求（P6-4 的目标，不是现状）

| 要求 | 锚点 |
| --- | --- |
| LSP 从 `current/bin/wxml-lsp` 启动时，以 `/proc/self/exe` 真实路径定位 `versions/<v>/`，读同目录 `manifest.json`，再解析 `engine.binary`；不得 PATH/cwd 搜索 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:221-223` |
| `WXML_LSP_BIN` 必须指向安装树内 `bin/wxml-lsp` 或测试构造的同构包根；裸二进制缺 manifest/engine 必须显式失败，不得猜路径或回退 Node | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:224-227` |
| 启动引擎前按 F0-03 校验 `engine/lib/tsc` 为普通文件、可执行、非 symlink、大小/SHA-256 匹配 manifest | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:226-227` |
| runtime 只执行 manifest 的 `engine/lib/tsc`；不查 PATH/cwd、不执行 JS | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:699` |
| 解析 `engine.binary` 后必须 canonicalize，并确认仍在包根内；不得 symlink 逃逸 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1093-1094` |
| 不提供引擎覆盖环境变量；产品只执行 `<current>/manifest.json` 的 `engine.binary` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1110-1114` |
| 引擎唯一启动 argv：`--api --async --cwd <sandbox> --useCaseSensitiveFileNames=true --callbacks=...`；cwd 是后端拥有的空沙箱，不是项目根 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:140-161` |

### 2.6 Zed 与构建/CI

| 事实 | 锚点 |
| --- | --- |
| Zed 只接受绝对 `WXML_LSP_BIN`，不探测 worktree/PATH | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/src/lib.rs:80-113` |
| Zed 仍转发 `WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`/`WXML_NODE` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/src/lib.rs:18-25`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/src/lib.rs:231-238` |
| Zed 启动参数固定只有 `--stdio`，没有选择原生后端的参数 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/src/lib.rs:53-57` |
| `build-release.sh` 仍要求 Node/npm、构建 `dist/server.js` + `node_modules`、打包 v1 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/scripts/build-release.sh:151-172`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/scripts/build-release.sh:218-244`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/scripts/build-release.sh:265-283` |
| CI 只有 fmt/clippy/test/doc/deny，没有 clean-PATH 原生 job | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/.github/workflows/ci.yml:10-34` |

### 2.7 现有门禁的真实覆盖

| 事实 | 锚点 |
| --- | --- |
| `lsp_native_status` 只静态 grep `main.rs` 是否还有 `NodeBackend::new`，不运行 LSP | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/no_node_guard.rs:468-490` |
| 即使 `WXML_REQUIRE_LSP_NATIVE=1`，当前也只断言静态状态；并打印 `F3-02: BLOCKED` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/no_node_guard.rs:851-863` |
| `verify-no-node.sh` 同样静态 grep，并明确残留“没有 execve/strace、没有原生 stdio LSP 会话” | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/scripts/verify-no-node.sh:356-369`、`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/scripts/verify-no-node.sh:432-438` |
| 真实资产 harness 已能安装真实引擎并不用 PATH Node，但**不启动 LSP** | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/real_asset.rs:182-225` |
| `cli.rs` 只测 `--version`/`--help`/未知参数，没有后端选择测试 | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/cli.rs:1-61` |
| LSP 源码中没有 `/proc/self/exe`/`manifest.json`/`engine/lib/tsc` 的发现代码；grep 只命中 Node 的 `current_exe` | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/config.rs:229-241` |

---

## 3. 目标设计（精确）

### 3.1 引擎发现：从运行中的可执行文件定位包根

**接口（建议）**

新增 `crates/wxml-lsp/src/backend_tsgo/engine.rs`：

```rust
pub(crate) struct VerifiedInstall {
    pub(crate) package_root: PathBuf,
    pub(crate) manifest: ManifestV2, // 来自共享 manifest crate，见 4-1
    pub(crate) engine: PathBuf,      // <package>/engine/lib/tsc
    pub(crate) standard_lib_root: PathBuf, // <package>/engine/lib
    pub(crate) typings_root: PathBuf,      // <package>/typings/miniprogram-api-typings
}

pub(crate) fn discover_from_current_exe() -> Result<VerifiedInstall, EngineDiscoveryError>;

#[cfg(test)]
pub(crate) fn discover_at(executable: &Path) -> Result<VerifiedInstall, EngineDiscoveryError>;
```

生产路径必须只调用 `discover_from_current_exe()`；`discover_at()` 只给测试构造同构包根使用，不进入生产配置。

**精确步骤**

1. **取运行路径**：调用 `std::env::current_exe()`。
   - Linux 上 Rust 通过读取 `/proc/self/exe` 实现；返回的是当前进程可执行文件的路径，不是 `argv[0]`。
   - 如果文件已被替换/删除，内核可能返回带 `" (deleted)"` 后缀的路径；Rust 不会自动去掉。此时下一步 canonicalize 会失败，必须 **fail closed**，不得 stripping 后缀、不得猜路径。
   - **禁止**使用 `argv[0]`、`PATH`、`std::env::current_dir()` 或任何 `WXML_*` 变量来定位包根。
2. **canonicalize**：`std::fs::canonicalize(&exe)`。
   - 解析 `current` 符号链接以及所有父目录符号链接；结果是绝对、无 `.`/`..` 的真实路径。
   - `current` 是设计允许的安装 symlink；canonicalize 后应落到 `versions/<v>/bin/wxml-lsp`。
   - 失败（NotFound、PermissionDenied、deleted）→ `EngineDiscoveryError::UnresolvableExecutable`；不启动引擎、不回退 Node。
3. **校验可执行文件位置**：
   - `exe.file_name() == "wxml-lsp"`；否则 `NotInstalled`。
   - `exe.parent().file_name() == "bin"`；否则 `NotInstalled`。
   - `package_root = exe.parent().parent()`；只向上取一级，**不搜索父目录**。
   - 这样 `target/debug/wxml-lsp`、`/usr/local/bin/wxml-lsp`、裸拷贝都会显式失败，而不是在父目录碰运气找 `manifest.json`。
4. **校验包根**：
   - `package_root` 必须是绝对路径、不是 `/`、`symlink_metadata(package_root)` 是真实目录且不是 symlink。
   - `package_root/bin`、`package_root/engine`、`package_root/engine/lib` 必须是真实目录；`typings/` 在 P6-3 资产加载时再校验（发现阶段可以只确认 `package_root`/`bin`/`engine`）。
5. **读取 manifest**：
   - `package_root/manifest.json`：`symlink_metadata` 必须是普通文件、不是 symlink；大小 ≤ 4 MiB（`MAX_MANIFEST_BYTES`）；Unix 模式精确 `0644`。
   - 使用共享 manifest reader（见 4-1）调用 `read_manifest_v2(package_root, ExpectedManifestV2 { version: Some(wxml_lsp::version()), platform: Some("linux-x86_64"), protocol_version: Some(2) })`。
   - 该 reader 的 `validate_engine` 已冻结以下身份（不要复制常量）：
     - `schemaVersion == 2`、`package == "wxml-lsp"`、`platform == "linux-x86_64"`；
     - `protocolVersion == 2`；
     - `engine.name == "typescript"`、`engine.version == "7.1.0-beta"`、`engine.sourceCommit == 3e9236...c50a`；
     - `engine.distribution == "@typescript/typescript-linux-x64"`、SRI、tarball SHA-256、大小/文件数；
     - `engine.apiSchemaVersion == 9`、`engine.astHeaderBytes == 64`；
     - `engine.license == "Apache-2.0"`、`engine.licenseFiles` 含 `engine/LICENSE`/`engine/NOTICE.txt`；
     - `engine.binary == "engine/lib/tsc"`；
     - `engine.probe.args == ["--version"]`、`engine.probe.expectPrefix == "Version "`；
     - `typeAssets` 含 `typescript-standard-lib`（`engine/lib`，`lib*.d.ts`，116 文件）与 `miniprogram-api-typings`（`typings/miniprogram-api-typings`，26 文件）；
     - 恰好一个 `native-engine` artifact，路径 `engine/lib/tsc`。
   - 以上来自 `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:41`、`:60-86`、`:200-221`、`:330-414`、`:485-596`、`:599-638`。
   - `manifest.package_version == wxml_lsp::version()` 作为纵深防御（防止把别的 release 的 manifest 指给当前二进制）；失败 → `ManifestMismatch`。
6. **解析并校验引擎**：
   - `engine_relative = manifest.engine.binary`，必须是包相对、`/` 分隔、无 `..`/绝对/反斜杠/控制字符；本版本精确要求 `"engine/lib/tsc"`。
   - `engine_path = package_root.join(&engine_relative)`。
   - 先 `symlink_metadata(engine_path)`：必须是普通文件，**不是 symlink**；Unix 模式精确 `0755`；`metadata.len()` 等于 `manifest.file(engine_relative).size`。
   - `canonicalize(engine_path)`；结果必须 `starts_with(package_root)` 且不等于 `package_root`；否则 `EngineEscapesPackage`。
   - 计算一次 SHA-256（或复用共享 `check_native_engine`/`verify_engine_executable`）；必须同时等于：
     - `manifest.file(engine_relative).sha256`（manifest 身份）；
     - `PinnedEngine::pinned().executable_sha256()`（编译期 pin；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/config.rs:43-45`）。
   - 注意：`NativeSession::start` 还会再校验一次 pin SHA（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:296-300`）。P6-4 接受启动阶段一次额外 hash（24 MB 左右），换取“服务器进入 initialize 之前就失败”；后续可用已验证 token 优化，但不得削弱 session 自身的校验。
7. **不做的事**：
   - 不读 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`/`WXML_TSGO_ENGINE`/`TSGO_BIN`；
   - 不探测 `backends/typescript/dist/server.js`、`wxml-type-backend/server.js`；
   - 不查 `PATH`、不查 cwd、不执行 `which`；
   - 不联网、不下载、不读取项目根或用户 home 来补资产。
8. **返回**：`VerifiedInstall` 供 session 构造和 P6-3 资产 loader 使用；发现阶段不启动进程、不创建 sandbox。

**manifest 身份与 `engine/lib/tsc` 的单一事实源**

- `PinnedEngine` 是编译期运行时 pin（`config.rs:19-45`、`:197-234`）。
- `manifest_v2` 是交付/安装期 schema（`manifest_v2.rs:41-91`、`:330-414`）。
- 两者必须一致；P6-4 不在 LSP 里复制版本/SHA/文件数常量，而是：
  - 用共享 manifest reader 校验 manifest；
  - 用 `PinnedEngine::pinned()` 校验引擎可执行文件；
  - 加一个测试断言两者一致（见 6.1）。
- 如果维护者拒绝新建共享 crate，最低要求是给 `wxml-delivery` 增加 `runtime-manifest` feature，并让 LSP 以 `default-features = false` 依赖它；**不得**直接把完整 `wxml-delivery` 加入 LSP，因为它会带入 `reqwest`/`tar`/`flate2`/`fs2`（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/Cargo.toml:9-17`），扩大服务器运行时攻击面。

**symlink 策略**

- 允许：安装根的 `current` 相对 symlink 指向 `versions/<v>`；这是唯一预期 symlink，由 `/proc/self/exe` + canonicalize 解析。
- 拒绝：`manifest.json`、`SHA256SUMS`、`engine/lib/tsc`、`bin/wxml-lsp` 是 symlink；`bin`/`engine`/`engine/lib`/`typings` 是 symlink；`engine.binary` 通过 symlink 逃出包根。
- 对 `engine_path` 必须先 `symlink_metadata` 再 `canonicalize`，不能只 `canonicalize` 后检查，否则会跟随 symlink 后才判断。
- 测试构造的同构包根可以放在任意临时目录，但仍必须满足 `bin/wxml-lsp`、`manifest.json`、`engine/lib/tsc` 的形状；不得为测试放宽生产 symlink 规则。

**权限**

- `manifest.json`：`0644`。
- `engine/lib/tsc`：`0755`（manifest `validate_engine` 已要求 `engine_file.mode == "0755"`，`manifest_v2.rs:408-414`）。
- `bin/`、`engine/`、`engine/lib/`、`package_root`：`0755`；不接受 group/other 可写（安装器 `verify_v2` 已对目录/文件做模式校验，`verify_v2.rs:107-109`、`:187-207`）。
- 拒绝 setuid/setgid/sticky 位；拒绝非普通文件（FIFO/device/socket）。

### 3.2 沙箱：创建、生命周期与 supervisor/session 的边界

**接口（建议）**

新增 `crates/wxml-lsp/src/backend_tsgo/sandbox.rs`：

```rust
pub(crate) struct SandboxDir {
    path: PathBuf,
    base: PathBuf,
}

impl SandboxDir {
    pub(crate) fn create() -> Result<Self, SandboxError>;
    pub(crate) fn path(&self) -> &Path;
}

impl Drop for SandboxDir { /* 只删除自己创建的空目录 */ }
```

**创建规则**

1. 基目录：`std::env::temp_dir()`（标准 OS 临时目录；它只决定沙箱位置，**不参与引擎发现**，不能改变引擎身份）。不要用安装根、`versions/<v>`、`<root>/tmp`、项目根或 LSP 的 cwd。
2. 名称：`wxml-lsp-<pid>-<nanos>-<atomic counter>`；每个 session 唯一。
3. 使用 `std::fs::create_dir(&path)`，**不要** `create_dir_all`：
   - 已存在（文件/symlink/目录）→ 失败；不重用、不清空、不 chmod 别人的目录。
4. 创建后：
   - `set_permissions(0o700)`；
   - `symlink_metadata` 确认是目录、不是 symlink；
   - `canonicalize` 确认父目录是预期 base（防止 base 本身被 symlink 替换）；
   - 读一次目录，确认空（`create_dir` 已保证，但防御性检查）。
5. 失败清理：创建后任一步失败，best-effort `remove_dir`；不递归删除未知内容。

**生命周期与 supervisor/session**

- `SandboxDir` 必须在 `NativeSession::start` 之前创建，因为 `EngineSupervisor::spawn_with_executable` 会校验它存在、空、非 symlink（`supervisor.rs:604-630`）；`NativeSession::start` 还会用 sandbox 根做 `confinement_self_check`（`session.rs:1923-1933`）。
- `SessionOptions` 继续只携带路径（`session.rs:80-96`），不改后端 API；sandbox 的所有权放在 LSP 侧：
  - `NativeSessionFactory::start` 每次创建新的 `SandboxDir`；
  - `LiveQuerySession { session, sandbox }`（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:127-129`）持有 guard；
  - `LiveQuerySession::shutdown` 先 `session.shutdown()`（释放 snapshot、关闭子进程，`session.rs:852-858`），再 drop guard 删除空目录。
- 这样：
  - 一个 adapter 同时只有一个 session（`backend_tsgo/mod.rs:467-483` 的 `AdapterState`），不会在同一 sandbox 里并发两个引擎；
  - 多个 LSP 进程各有唯一 sandbox；
  - session 故障重启时换新 sandbox，不复用可能被上一次进程污染的目录。
- `EngineSupervisor` 只负责“校验 + spawn + 生命周期”，不负责创建/删除目录；这与现有职责一致（`supervisor.rs:1420-1462`）。
- **禁止**把 sandbox 放在 `<install_root>/tmp` 或 `<install_root>/versions/<v>`：前者是安装事务临时区，需要安装锁；后者是不可变发布树（`docs/reference/tsgo-delivery-contract.md:213-219`）。运行时写这些位置会破坏安装器不变量。
- 崩溃（SIGKILL）可能留下 `temp_dir()/wxml-lsp-*` 空目录；这是可接受的残留，下一次启动使用新名字。不要扫描并删除其他用户的目录；如要清理，只允许清理同一 uid、同一 base、名字匹配且年龄超过阈值的目录，并必须在独立评审中做。

### 3.3 opt-in 选择器：不改变默认 `NodeBackend`

**默认路径保持原样**

- `serve_real()`（`main.rs:93-99`）及其 `NodeBackend` 构造**不改**。
- `--stdio` 无额外参数仍然是 Node 默认；这是 P6-8 之前的产品行为。
- 在 v2 原生包中，`backends/typescript/dist/server.js`/`wxml-type-backend/server.js` 不存在，因此默认 Node 路径会显式 `Unavailable`；这正是为什么 P6-4 必须提供 opt-in 原生入口，而 P6-8 必须先把默认切到原生，才能发布/使用 v2 包。

**新增 opt-in 入口（建议）**

- CLI：`wxml-lsp --stdio --backend native`。
  - `--backend node` 或不写：走现有 `serve_real()`（默认不变）。
  - `--backend native`：走新的 `serve_native()`。
  - 未知值/重复参数：错误退出，不猜。
- 参数解析位置：`main.rs:56-77` 当前只读一个 `extra`，需要扩展为严格解析 `--stdio [--backend native|node] [--mock]`；`--mock` 与 `--backend` 互斥或明确拒绝。
- 不新增 `WXML_LSP_BACKEND` 之类的环境变量选择器；P6-8 再决定是否保留 CLI。环境变量只用于既有的 `WXML_ANALYZE_TIMEOUT_MS`/`WXML_TEMPLATE_CHECK`（P6-8 映射到原生），不用于引擎路径。

**库内 seam（保持 `backend_tsgo` 私有）**

推荐在 `crates/wxml-lsp/src/lib.rs` 增加一个小的公开函数，而不是 `pub mod backend_tsgo`：

```rust
pub fn run_stdio_native() -> Result<ServerOutcome, NativeStartupError>;
```

实现放在 `crates/wxml-lsp/src/backend_tsgo/mod.rs` 或新的 `native_start.rs`：

1. `let host = Arc::new(ProjectState::new());`
2. `let install = engine::discover_from_current_exe()?;`
3. `let sandbox = SandboxDir::create()?;`
4. `let options = SessionOptions::new(install.engine.clone(), sandbox.path());`
5. `let adapter = TsgoBackendAdapter::with_install(host.clone(), install, options, sandbox);`
6. `run_stdio_with_host(Arc::new(adapter), host)`。

`TsgoBackendAdapter` 保持 `pub(crate)`；`backend_tsgo` 保持 `pub(crate)`；`main.rs` 只需要 `wxml_lsp::run_stdio_native()`。这样：
- 不把 `PinnedEngine`/`SessionOptions`/内部 session seams 暴露到 crate 外；
- 不改变 `run_stdio`/`run_stdio_with_host` 的通用性；
- 测试仍可用现有的 fake adapter/seam（`backend_tsgo/session.rs:57-125`）。

**native 未安装/启动失败时的行为**

- `discover_from_current_exe()` 失败：`run_stdio_native()` 在进入 LSP `initialize` 之前返回 `NativeStartupError`；`main.rs` 打印到 stderr 并以非零退出。
- **不得**在 native 选择失败时构造 `NodeBackend`、探测 `server.js` 或启动 Node；这是 fail-closed 的核心。
- 不得启动一个“空但 Supported”的服务器：`Backend` 的 `capabilities()` 仍返回 `Partial`（`backend_tsgo/mod.rs:441-465`），但只在引擎已成功发现/启动后才可达；启动失败根本进不到请求阶段。
- 运行时失败：
  - `NativeError::Unavailable/Protocol/Timeout` 已经映射为 `BackendError::Unavailable/Protocol/Timeout`（`backend_tsgo/mod.rs:552-581`），并触发 session drop/epoch advance（`backend_tsgo/mod.rs:174-213`）。
  - `diagnostics()` 在输入/session 不可用时返回 `CapabilityStatus::Unavailable` + 语法诊断，而不是“干净”结果（`backend_tsgo/mod.rs:322-368`）。
  - `symbols()`/rename 保持显式 `Unsupported`（`backend_tsgo/mod.rs:100`、`:414-419`）；不得改成空 `Supported`。
- 如果 `WXML_REQUIRE_REAL_ENGINE=1` 或后续的 `WXML_REQUIRE_LSP_NATIVE=1` 被设置，缺失/失败必须是硬失败，不得 skip。

**P6-8 才做的事（本文不做）**

- 把 `--stdio` 默认改成 native；删除/移出 `backend_node`；移除 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` 的读取与 usage；Zed 停止转发 legacy env 并选择 native；把 Node/fake transport 测试迁移或移除；`build-release.sh`/CI 去 Node；更新 README/ADR；G5 归零；G6 默认切换门禁。
- P6-4 只提供 opt-in；不得在文档/提交里声称“默认已切换”。

### 3.4 类型资产 handoff（依赖 P6-3，只定义接口，不重复实现）

P6-3 的当前计划（`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:11-17`、`:102-207`、`:320-335`）已经定义了正确方向。P6-4 只做“把已验证安装交给 P6-3”，不实现第二套 loader。

**P6-4 提供给 P6-3 的接口**

`VerifiedInstall`（3.1）必须携带：
- `package_root`；
- `manifest: ManifestV2`（共享 crate 类型）；
- `engine`；
- `standard_lib_root = package_root/engine/lib`；
- `typings_root = package_root/typings/miniprogram-api-typings`；
- 可选：`manifest.files[]` 的 path→{sha256,size,mode} 只读映射；P6-3 用它逐文件校验，避免再次解析 manifest。

**P6-3 必须提供的接口（P6-4 只调用）**

- `wxml-backend-tsgo/src/native/type_assets.rs`（新增）：
  - `TypeAssets::load(package_root: &Path, manifest: &ManifestV2) -> Result<Arc<TypeAssets>, AssetError>`；
  - 校验 `typeAssets[]` 的 `root`/`include`/`file_count`/`sha256` 与 `files[]` 逐文件摘要；
  - 只读 `engine/lib/lib*.d.ts` 与 `typings/miniprogram-api-typings/**`；拒绝 symlink、越界路径、Node 运行时文件；
  - 不读项目 `node_modules`、用户 home、PATH、网络、npm。
- `wxml-backend-tsgo/src/input.rs`：
  - 保留 `AnalysisInput::new` 作为 legacy/no-asset 构造器（`input.rs:212-294`）；
  - 新增 `AnalysisInput::with_environment(project, root, revision, files, CompilerOptions, Arc<TypeAssets>)`；
  - 生产路径禁止使用 legacy 构造器。
- `wxml-backend-tsgo/src/native/vfs.rs`：
  - `from_analysis_input` 把 `type_assets.files()` 挂到 `fileSystem.files`；
  - `tsconfig_text` 不再固定 `noLib:true`（当前 `vfs.rs:685-696`），改为 `noLib:false`/不写 `noLib`，写 `types`/`typeRoots`/项目 compilerOptions；
  - 资产路径必须按路径组件边界位于 `standard_lib_root`/`typings_root` 下。
- `wxml-lsp/src/backend_tsgo/input.rs`：
  - 把 `view.compiler_options`（`project_host.rs:345-370`）传给 `AnalysisInput`；
  - 把 `WXML_TEMPLATE_CHECK` 映射为 `JsCheckPolicy`，而不是固定 `Inherit`（当前 `backend_tsgo/input.rs:193-205`）；
  - `RequestIdentity` 增加 options/assets digest，防止旧结果跨资产版本。
- `wxml-lsp/src/backend_tsgo/mod.rs`：
  - `with_install(host, install, options, sandbox)` 把 `VerifiedInstall` 存起来；每次 session 启动时把 `TypeAssets` 传给 `AnalysisInput`。

**依赖顺序与不可越界**

- P6-4a（发现/沙箱/选择器）可以先于 P6-3 落地；但它只能用 fake/no-asset 测试，**不得**声称真实启动可用。
- P6-4b（资产 handoff）必须等待 P6-3 的 3-1/3-4/3-5/3-6（计划 `:326-331`），以及交付侧 typings 26/23 文件数一致（计划 `:74`、`:335`）。
- P6-4 不实现资产扫描、不复制 `MINIPROGRAM_TYPINGS_*`/`STANDARD_LIB_*` 常量、不读项目 `node_modules`；这些是 P6-3/交付的唯一职责。
- 真实包 AC 门禁在 P6-3 + 交付一致前保持 BLOCKED；P6-4 不得用“引擎能启动”掩盖“`wx`/标准库类型缺失”。

---

## 4. 有序实施清单

> 顺序按依赖排列；每项独立提交/评审。P6-4 不修改默认后端；任一项 revert 都应回到 Node 默认。

### 4-1. 先定 manifest 共享边界（前置）

| 项 | 内容 |
| --- | --- |
| 变更 | 新建轻量 crate `crates/wxml-native-manifest`（建议名），只含 `serde`/`serde_json`/`sha2`/`thiserror`；把 `manifest_v2`、`type_assets`、`verify_v2` 的 manifest/engine 校验、`FileEntry`/`read_limited`/`sha256_file` 移入或重导出。`wxml-delivery`、`wxml-lsp`、`wxml-backend-tsgo` 依赖它。 |
| 备选 | 给 `wxml-delivery` 加 `runtime-manifest` feature，把 `reqwest`/`tar`/`flate2`/`fs2` 设为 optional；LSP 以 `default-features=false` 依赖。 |
| 拒绝 | 直接把完整 `wxml-delivery` 加入 LSP（会带入网络/归档/锁依赖，`crates/wxml-delivery/Cargo.toml:9-17`），或在 LSP 复制一份 manifest schema/常量。 |
| 验收 | `cargo tree -p wxml-lsp` 不出现 `reqwest`/`tar`/`flate2`（若采用新 crate）；manifest 单测仍绿；`PinnedEngine` 与 manifest 常量一致性测试。 |

### 4-2. 实现引擎发现（P6-4a）

| 项 | 内容 |
| --- | --- |
| 文件 | 新增 `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/engine.rs`；在 `backend_tsgo/mod.rs` 挂 `mod engine;`。 |
| 内容 | 3.1 的 `VerifiedInstall`/`EngineDiscoveryError`/`discover_from_current_exe`/`discover_at`；复用共享 `read_manifest_v2`/`check_native_engine`/`PinnedEngine`。 |
| 验收 | 单测覆盖 6.1；不启动进程；不读 PATH/cwd/env；无 Node 依赖。 |
| 回滚 | 删除新模块；`main.rs`/`lib.rs` 未接线时无行为变化。 |

### 4-3. 实现 sandbox 与 session 生命周期（P6-4a）

| 项 | 内容 |
| --- | --- |
| 文件 | 新增 `crates/wxml-lsp/src/backend_tsgo/sandbox.rs`；修改 `crates/wxml-lsp/src/backend_tsgo/session.rs:108-195`，让 `NativeSessionFactory::start` 创建 `SandboxDir` 并由 `LiveQuerySession` 持有。 |
| 内容 | 3.2 的 `create_dir`/0700/空/非 symlink/唯一/cleanup；不写安装根/事务临时区。 |
| 验收 | 6.1 沙箱单测；`EngineSupervisor` 的 `SandboxNotEmpty`/`SandboxSymlink` 不被触发；shutdown 后目录删除。 |
| 回滚 | 恢复固定 `SessionOptions`；sandbox 单测删除。 |

### 4-4. 实现 opt-in 选择器（P6-4a）

| 项 | 内容 |
| --- | --- |
| 文件 | `crates/wxml-lsp/src/main.rs:15-36`（usage）、`:56-77`（解析）、`:93-99`（保持 Node）；`crates/wxml-lsp/src/lib.rs`（新增 `pub use native_start::run_stdio_native;`）；新增 `crates/wxml-lsp/src/backend_tsgo/native_start.rs`（或直接在 `mod.rs` 内实现）。 |
| 内容 | `--stdio --backend native`；默认 `--stdio`/`--backend node` 仍 Node；`NativeStartupError` 公开、不含绝对路径；`backend_tsgo` 保持 `pub(crate)`。 |
| 验收 | 6.1 CLI/解析单测；`--backend native` 在无安装时非零退出且不启动 Node；`--stdio --mock` 不变；`--help` 说明 opt-in。 |
| 回滚 | 删除 flag 和公开函数；默认路径未动。 |

### 4-5. 失败映射与 capability 收口（P6-4a）

| 项 | 内容 |
| --- | --- |
| 文件 | `crates/wxml-lsp/src/backend_tsgo/mod.rs:552-581`（错误映射）；`:322-368`（diagnostics Unavailable）；`:414-419`/`:441-465`（Unsupported/Partial）。 |
| 内容 | 新增 `EngineDiscoveryError` → `NativeStartupError`/`NativeError` 的稳定映射；不泄露绝对路径；缺引擎/坏引擎/版本/AST 失败均 fail-closed；不引入 Node fallback。 |
| 验收 | 单测断言错误类别；`capabilities()` 不因 native 未安装而变成空 `Supported`。 |
| 回滚 | 映射层可单独 revert；默认 Node 不变。 |

### 4-6. 类型资产 handoff（P6-4b，依赖 P6-3）

| 项 | 内容 |
| --- | --- |
| 文件 | `crates/wxml-backend-tsgo/src/native/type_assets.rs`（P6-3 新增）；`input.rs:212-294`；`native/vfs.rs:685-696`；`crates/wxml-lsp/src/backend_tsgo/input.rs:193-205`、`:308-341`；`crates/wxml-lsp/src/backend_tsgo/mod.rs:129-138`。 |
| 内容 | 3.4 的接口；`VerifiedInstall` → `TypeAssets::load` → `AnalysisInput::with_environment` → VFS/tsconfig；`JsCheckPolicy` 映射。 |
| 依赖 | P6-3 3-1/3-4/3-5/3-6；交付 typings 26/23 一致；不得在本项实现第二套 loader。 |
| 验收 | P6-3 的 fake VFS/真实引擎测试；P6-4 只加“从 `VerifiedInstall` 传入”的测试。 |
| 回滚 | 保留 `AnalysisInput::new` legacy；revert 后回到 `noLib:true` 的旧行为（仅非默认/测试）。 |

### 4-7. 真实安装包与进程审计（P6-4c）

| 项 | 内容 |
| --- | --- |
| 文件 | `crates/wxml-delivery/tests/real_asset.rs`（扩展，见 6.2）；`crates/wxml-delivery/tests/no_node_guard.rs:468-490`、`:851-863`；`scripts/verify-no-node.sh:356-369`、`:432-438`；`.github/workflows/ci.yml`（新增 job）。 |
| 内容 | 安装真实 v2 包 → 启动 `bin/wxml-lsp --backend native --stdio` → initialize/didOpen/hover/diagnostics → 断言无 Node 子进程/无 PATH Node；`strace -f -e trace=execve,execveat` 审计。 |
| 依赖 | 交付 v2 真实包（HEAD 的 `package.rs` 没有 v2 路径；当前未提交改动不算证据）；F3-01/02/14/15；真实资产。 |
| 验收 | 6.2；在真实包/真实引擎可用前保持 BLOCKED。 |

### 4-8. Zed 冒烟与文档（P6-4c/P6-8 边界）

| 项 | 内容 |
| --- | --- |
| 文件 | `editors/zed/src/lib.rs:18-25`、`:53-57`；`editors/zed/README.md`；`docs/evidence/2026-11-18-tsgo-native-engine-discovery-plan.md` 后续证据。 |
| 内容 | P6-4 只记录手动直接启动的冒烟命令；真正的 Zed 选择原生需要扩展传 `--backend native` 或新配置，属于 P6-8/C3。不得把 `WXML_LSP_BIN` 指向包装脚本当作首次 Zed 冒烟。 |
| 验收 | P6-4 内只能做“直接二进制 + 真实包”的冒烟；Zed UI/进程树冒烟保持 BLOCKED。 |

---

## 5. 失败模式与 fail-closed 矩阵

| 失败 | 检测点 | 结果 | 不得做 |
| --- | --- | --- | --- |
| `/proc/self/exe` 缺失/deleted/canonicalize 失败 | `current_exe` + `canonicalize` | `NativeStartupError::UnresolvableExecutable`，非零退出 | 不 strip `" (deleted)"`；不退回 `argv[0]`/PATH/cwd |
| 二进制不在 `<pkg>/bin/wxml-lsp` | 文件名/父目录检查 | `NotInstalled`，非零退出 | 不向上搜索 `manifest.json`；不猜包根 |
| manifest 缺失/非普通文件/symlink/超 4 MiB/模式错 | `symlink_metadata` + `read_limited`/共享 reader | `ManifestUnavailable`/`ManifestMismatch`，非零退出 | 不读项目/用户 home 的 manifest；不回退 Node |
| manifest 是 v1/未知 schema/未知字段 | 共享 `read_manifest_v2`/`detect_schema` | `ManifestMismatch`，非零退出 | 不把 v1 当 native；不忽略未知字段 |
| manifest 版本/平台/protocol/engine 身份不符 | `validate_engine`/`ExpectedManifestV2` | `ManifestMismatch`，非零退出 | 不“尽力兼容”；不动态放宽白名单 |
| `engine/lib/tsc` 缺失 | `symlink_metadata`/`check_native_engine` | `EngineMissing`，非零退出 | 不探测 `server.js`；不回退 Node |
| 引擎是 symlink/非普通文件/无执行位/模式非 0755 | `symlink_metadata` + mode | `EngineUnsafe`，非零退出 | 不跟随 symlink；不 chmod 用户文件 |
| 引擎大小/manifest SHA/`PinnedEngine` SHA 不符 | 一次 hash + 两次比较 | `EngineArtifactMismatch`，非零退出 | 不启动损坏引擎；不接受“先跑再报错” |
| 引擎 canonicalize 后逃出包根 | `canonicalize` + `starts_with(package_root)` | `EngineEscapesPackage`，非零退出 | 不把包外可执行文件当引擎 |
| `engine/lib/tsc --version` 不精确等于 `Version 7.1.0-beta` | `probe_version`（`session.rs:1935-1981`） | `Unavailable(ArtifactMismatch)` | 不 strip 版本前缀；不接受浮动版本 |
| initialize 形状/`caseSensitive` 不符 | `handshake`（`session.rs:376-387`） | `Unavailable(HandshakeFailed)` | 不继续发项目内容 |
| schema probe 缺方法/AST v9/header 64 不符 | `probe_schema`（`session.rs:389-577`） | `Unavailable(ApiMismatch)`/`Protocol` | 不进入“降级但继续”；不返回空成功 |
| sandbox 不存在/非目录/symlink/非空 | `EngineSupervisor::validate_sandbox_root`（`supervisor.rs:604-630`） | `SandboxMissing`/`SandboxSymlink`/`SandboxNotEmpty` | 不清空/复用/覆盖已有目录；不写安装根 |
| sandbox 创建失败/权限不足 | `SandboxDir::create` | `SandboxUnavailable`，非零退出 | 不退到安装根 tmp；不使用 cwd |
| 同一 adapter 并发 session | `AdapterState` 单 session + `EngineSupervisor::AlreadyRunning` | 第二个 start 失败/被串行化 | 不共享可变 session；不在同一 sandbox 并发引擎 |
| 两个 LSP 进程并发 | 每进程唯一 sandbox | 各自独立 | 不共享固定路径；不跨进程复用 engine 进程 |
| 升级/回滚时 LSP 正在运行 | 运行中进程持有旧 `versions/<v>` 路径；`versions` 不裁剪（`native_install.rs:1262-1268`） | 旧 session 继续；下次启动重新发现 `current`/新包 | 不在 session 中途切到新 `current`；不混合旧 engine/新 assets |
| 升级/回滚删除了旧版本（外部/异常） | 新 session start 的 `symlink_metadata`/hash | `EngineMissing`/`EngineArtifactMismatch`，fail-closed | 不猜测其他 versions；不回退 Node |
| handle/snapshot 过期 | `StaleHandle`/`StaleSnapshot` | `BackendError::Cancelled`（`backend_tsgo/mod.rs:552-581`） | 不把过期结果当当前结果 |
| 请求超时 | `NativeError::Timeout` | drop session + epoch advance（`backend_tsgo/mod.rs:187-212`） | 不自动重启并复用旧 sandbox；不回退 Node |
| 缺失 stdlib/typings 资产 | P6-3 `TypeAssets`/`MissingAsset`（`native/mod.rs:61-62`） | `Unavailable(MissingAsset)` | 不进入“干净诊断”；不读 OS/项目补资产 |
| 旧 env 设置 | 原生路径不读 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` | 忽略，不改变引擎/后端 | 不激活 Node；不回退 `server.js` |

**统一 fail-closed 规则：**

1. 任何发现/校验失败都发生在 `initialize` 之前或当前请求返回 `Unavailable`/`Protocol`，不产生“空但 Supported”。
2. 原生路径永不构造 `NodeBackend`、永不执行 `node`/`npm`/`npx`/`server.js`。
3. 原生路径不查 PATH/cwd，不读引擎覆盖 env；只执行 manifest 声明的 `engine.binary`。
4. 不在安装根/事务临时区写 sandbox；不在 `versions/<v>` 写任何运行期文件。
5. 缺少必需资产/引擎时失败，不 skip（`WXML_REQUIRE_REAL_ENGINE=1`/`WXML_REQUIRE_LSP_NATIVE=1` 必须硬失败）。

---

## 6. 测试与门禁

> 当前所有门禁都**未运行**；以下是 P6-4 实施后必须新增/扩展的精确目标。P6-4 的审计结论不因这些计划而改变。

### 6.1 单元/CLI 测试（不需要真实引擎）

| 目标文件 | 用例 | 断言 |
| --- | --- | --- |
| `crates/wxml-lsp/src/backend_tsgo/engine.rs`（新增 `#[cfg(test)]`） | `discovers_package_root_from_bin_executable` | 临时同构包根：`bin/wxml-lsp` + `manifest.json` + `engine/lib/tsc` → `package_root`/`engine` 正确 |
| 同上 | `rejects_manifest_symlink`、`rejects_engine_symlink`、`rejects_bin_symlink` | `symlink_metadata` 路径，不跟随；错误类别稳定 |
| 同上 | `rejects_missing_manifest`、`rejects_v1_manifest`、`rejects_unknown_field` | 共享 reader 拒绝；不回退 Node |
| 同上 | `rejects_wrong_platform_protocol_engine_version` | `ManifestMismatch` |
| 同上 | `rejects_engine_hash_size_mode_mismatch` | `EngineArtifactMismatch`；不 spawn |
| 同上 | `rejects_engine_escaping_package` | symlink/`..`/absolute → `EngineEscapesPackage` |
| 同上 | `rejects_executable_not_under_bin`、`rejects_deleted_executable` | `NotInstalled`/`UnresolvableExecutable` |
| 同上 | `ignores_path_cwd_and_legacy_env` | 设置 `WXML_NODE`/`WXML_TYPE_BACKEND`/恶意 cwd/PATH 后结果不变 |
| 同上 | `pinned_engine_matches_manifest_constants` | `PinnedEngine` 与共享 manifest 常量/`engine.binary` 一致 |
| `crates/wxml-lsp/src/backend_tsgo/sandbox.rs`（新增） | `creates_private_empty_non_symlink_directory` | 0700、空、非 symlink、父目录 base |
| 同上 | `unique_per_call`、`cleans_up_on_drop` | 两次不同路径；drop 后不存在 |
| 同上 | `rejects_existing_file_symlink_nonempty` | 不删除/不覆盖 |
| 同上 | `never_writes_install_root_or_versions` | 安装根/`versions/` fingerprint 不变 |
| `crates/wxml-lsp/src/main.rs`（新增 `#[cfg(test)]`）或 `crates/wxml-lsp/tests/cli.rs` | `parses_stdio_backend_native`、`default_is_node`、`rejects_unknown_backend` | 解析层单测；不需要启动 Node |
| `crates/wxml-lsp/tests/cli.rs`（扩展） | `native_without_install_fails_closed` | `--stdio --backend native` 非零退出；stderr 不含 `node` 调用；不挂起 |
| `crates/wxml-lsp/src/backend_tsgo/tests.rs`（现有） | 现有 fake session 用例 | 保持绿；`capabilities`/`Unsupported` 不被改动 |

### 6.2 真实安装/真实引擎测试

| 目标文件 | 用例 | 依赖/门禁 |
| --- | --- | --- |
| `crates/wxml-delivery/tests/real_asset.rs`（扩展 `installs_pinned_real_engine_without_path_node` 之后） | `installed_lsp_serves_native_stdio_without_node`：安装真实 v2 包，启动 `<root>/current/bin/wxml-lsp --backend native --stdio`，发 `initialize` + `didOpen` + `hover`/`diagnostics`，断言有引擎响应、无 Node 子进程 | `TSGO_ASSET`/`TSGO_BIN` + `WXML_REQUIRE_REAL_ENGINE=1`；真实 v2 包；clean PATH |
| `crates/wxml-lsp/tests/native_install.rs`（新增，可选） | `adapter_starts_from_verified_install`：用 `TSGO_BIN` 构造同构包根，调用 `discover_at` + `run_stdio_native` 的库级路径 | 真实引擎；`WXML_REQUIRE_REAL_ENGINE=1` |
| `crates/wxml-backend-tsgo/tests/type_assets.rs`（P6-3 新增） | P6-3 的 stdlib/typings fake/真实引擎用例 | P6-3；不是 P6-4 的重复实现 |
| `crates/wxml-lsp/tests/native_parity/*`（现有） | 不改 skeleton；P6-4 不更新 `MIGRATED_CASES` | G5 仍归 P6-A/P7 |

**说明：** 跨 crate 复用 `real_asset.rs` 的 fixture builder 需要把测试支持提升为 dev-only 共享模块；当前不建议在 P6-4 为测试复制一套真实包构造器。优先在 `crates/wxml-delivery/tests/real_asset.rs` 内扩展（delivery crate 可以直接 spawn 已安装的 `wxml-lsp` 路径，不需要依赖 LSP crate）。

### 6.3 no-Node 进程审计

| 目标文件 | 变更 | 当前状态/不能关闭的原因 |
| --- | --- | --- |
| `crates/wxml-delivery/tests/no_node_guard.rs:468-490` | 把 `lsp_native_status` 从静态 grep 改成“安装真实包 + 启动 `wxml-lsp --backend native --stdio` + 交互 + 断言 tripwire 未触发” | 当前只静态 grep `main.rs`；没有原生 stdio 会话 |
| `crates/wxml-delivery/tests/no_node_guard.rs:851-863` | `WXML_REQUIRE_LSP_NATIVE=1` 时对真实 LSP 会话硬失败；移除“F3-02 BLOCKED”或改为真实覆盖 | 当前明确打印 BLOCKED |
| `scripts/verify-no-node.sh:356-369` | 替换 `LSP_NATIVE` 静态检查为真实会话 | 当前只 grep `NodeBackend::new` |
| `scripts/verify-no-node.sh:432-438` | 移除“没有 execve/strace、没有原生 stdio LSP”的残留说明，改为实际执行 | 当前是已知残留 |
| `.github/workflows/ci.yml`（新增 job） | clean-PATH job：只装 Rust + 原生资产；`strace -f -e trace=execve,execveat` 包住 install/verify/current/env + `wxml-lsp --backend native --stdio`；断言 exec 集合 ⊆ `{wxml-lsp-install, wxml-lsp, engine/lib/tsc, 测试驱动}` | 当前 CI 没有该 job；F3-01/02 仍未关闭 |

**PATH shim 不能替代 strace**：`native_cli.rs:371-396` / `no_node_guard.rs` 的 tripwire 只能捕获 PATH 解析，不能排除绝对路径 `node`；F3-02 仍需要 execve 审计。

### 6.4 Zed 冒烟

| 目标 | 状态 | 说明 |
| --- | --- | --- |
| 手动直接启动 `wxml-lsp --backend native --stdio`（真实包 + 真实引擎） | P6-4 可做（但本次未运行） | 不是 Zed UI 冒烟 |
| Zed 扩展选择原生 | **BLOCKED** | `editors/zed/src/lib.rs:53-57` 固定 `--stdio`，没有 `--backend native`；`WXML_LSP_BIN` 只能指二进制，不能传 flag |
| Zed 标准安装/五语言/hover/definition/completion/诊断冒烟（F3-14） | **BLOCKED** | 需要 P6-8/C3 扩展选择原生 + 真实 v2 包；现有 W4 冒烟是 v1 Node 包 |
| 用包装脚本给 Zed 追加 `--backend native` | 不接受为冒烟证据 | 不是产品路径；会掩盖扩展未选择原生的事实 |

### 6.5 本轮可直接运行的门禁（实施后）

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p wxml-lsp --lib
cargo test -p wxml-lsp --test cli
cargo test -p wxml-delivery --test no_node_guard -- --nocapture
WXML_REQUIRE_REAL_ENGINE=1 TSGO_BIN=<pinned>/lib/tsc cargo test -p wxml-backend-tsgo --test real_engine
WXML_REQUIRE_REAL_ENGINE=1 TSGO_ASSET=<pinned.tgz> cargo test -p wxml-delivery --test real_asset -- --nocapture
scripts/verify-no-node.sh --installer <installed>/bin/wxml-lsp-install
```

**当前不能声称这些命令通过**：本次审计没有运行它们；真实资产/真实包/CI job 也尚未存在。

---

## 7. P6-4 BLOCKED / PARTIAL 明细

| ID | 阻塞 | 证据 | 退出条件 |
| --- | --- | --- | --- |
| P6-4-B01 | 没有引擎发现代码 | `crates/wxml-lsp/src/main.rs:93-99`；LSP 内无 `/proc/self/exe`/`manifest.json`/`engine/lib/tsc` 发现 | 4-2 落地 + 6.1 单测 |
| P6-4-B02 | 没有 sandbox 创建/清理 | `native/session.rs:78-96`；`native/supervisor.rs:604-630` 只校验 | 4-3 落地 + 6.1 沙箱测试 |
| P6-4-B03 | 没有 opt-in 选择器 | `main.rs:56-77`、`:93-99`；`lib.rs:44-46` | 4-4 落地 + CLI 测试 |
| P6-4-B04 | 没有类型资产 handoff | `input.rs:212-294`；`native/vfs.rs:685-696`；`backend_tsgo/input.rs:193-205` | P6-3 + 4-6 |
| P6-4-B05 | HEAD 没有 v2 打包器；真实包不可用 | `git show HEAD:crates/wxml-delivery/src/package.rs` 无 `ManifestV2`/v2 路径；P6-3 计划 `docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:74`、`:335` | 交付 B1/B2 + 未提交工作评审 |
| P6-4-B06 | 没有原生 LSP no-Node 进程审计 | `no_node_guard.rs:468-490`、`:851-863`；`verify-no-node.sh:356-369`、`:432-438`；F3-02 BLOCKED | 4-7 + strace/CI |
| P6-4-B07 | Zed 不能选择原生 | `editors/zed/src/lib.rs:53-57` | P6-8/C3 或单独的 Zed 选择器变更 |
| P6-4-B08 | 默认切换/Node 删除/G6 不属于 P6-4 | `main.rs:93-99`；`lib.rs:44`；`backend_node/config.rs:155-207` | P6-8 |
| P6-4-B09 | G5/AC-001..AC-013 未关闭 | `docs/plans/tsgo-test-ledger.json`；P6 cutover audit §6.1 | P6-A/P7；P6-4 不更新 ledger |
| P6-4-B10 | F3-01/02/15 未关闭 | `docs/reference/tsgo-delivery-contract.md` F3 表；`no_node_guard.rs:861-862` | 4-7 + legacy env 收口（P6-8） |

**结论：** P6-4 整体是 **BLOCKED/PARTIAL**。可以立即实施并单独评审的是 4-1～4-5（发现/沙箱/选择器/失败映射）；4-6（资产）被 P6-3 阻塞；4-7～4-8（真实包/进程审计/Zed）被交付 v2、F3 和 P6-8/C3 阻塞。不得把 P6-4 的任何计划项写成“已完成”或“F3 已关闭”。

---

## 8. 明确不声称

- 不声称默认后端已切换；`--stdio` 仍构造 `NodeBackend`（`main.rs:93-99`）。
- 不声称 Node 已删除；`backend_node` 仍编译、仍可被默认路径使用（`lib.rs:44`；`backend_node/config.rs:155-207`）。
- 不声称 F3-01/F3-02/F3-14/F3-15 或任何 F3 ID 已关闭；现有门禁明确打印 BLOCKED（`no_node_guard.rs:851-863`、`verify-no-node.sh:432-438`）。
- 不声称 AC-001..AC-013 原生通过；P6-3 仍 BLOCKED（`docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:7`、`:11-17`）。
- 不声称 G5 归零或 `MIGRATED_CASES` 已更新；P6-4 不碰 parity skeleton。
- 不声称 Zed 原生冒烟已完成；扩展仍固定 `--stdio`（`editors/zed/src/lib.rs:53-57`）。
- 不声称 Windows/macOS/aarch64 支持；仅 Linux x86_64 合同（`docs/reference/tsgo-delivery-contract.md:1011-1019`）。
- 不声称本轮运行了任何测试、strace、Zed 或真实引擎。

---

## 9. 回滚

- 4-1～4-5 都是新增模块/opt-in flag；revert 后 `main.rs` 的默认 Node 路径不变，不需要安装树/state 迁移。
- 4-3 的 sandbox guard 只影响新 native 路径；revert 后不残留运行时状态（已删除的空目录/临时目录除外）。
- 4-6 必须保留 `AnalysisInput::new` legacy 构造器（P6-3 计划 `:138`、`:380`）；revert 后旧 no-asset 路径仍可用于测试，但生产 native 不得使用。
- 4-7/4-8 的测试/CI 可单独 revert；不得删除旧 Node 包或旧 release。
- 任何阶段都不得删除 `backend_node`、旧 TS 测试或 legacy 包；这些是 P6-8/P7 的范围。

---

## 10. 关键锚点速查（绝对路径）

- 默认入口：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:10`、`:15-36`、`:56-77`、`:93-99`。
- 惰性原生模块：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/lib.rs:44-46`。
- 适配器与构造器：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:59-62`、`:95-112`、`:129-138`、`:414-419`、`:441-465`。
- 生产 session factory：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:108-125`、`:127-195`。
- `SessionOptions`/start/probe：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:78-96`、`:296-329`、`:1923-1981`。
- sandbox 校验/spawn：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/supervisor.rs:107-108`、`:507-519`、`:604-630`、`:638-685`、`:1454-1462`。
- 引擎 pin/argv：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/config.rs:19-45`、`:197-234`、`:356-375`。
- Node env/探测：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/config.rs:155-207`、`:229-241`。
- `AnalysisInput`：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:212-294`。
- VFS tsconfig：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:685-696`。
- 适配器 compilerOptions/checkJs：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:193-205`、`:308-341`。
- manifest v2：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:41`、`:60-86`、`:200-221`、`:266-273`、`:330-414`、`:485-596`、`:856-860`。
- verify v2/engine：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:42-135`、`:350-382`。
- install root/current：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/install.rs:17-40`、`:196-249`。
- current/install/rollback：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/native_install.rs:146-175`、`:275-305`、`:346-386`、`:778-795`、`:1262-1268`（HEAD 行号）。
- state 身份：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/state.rs:41-91`、`:262-318`。
- 合同：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:221-227`、`:699`、`:1093-1094`、`:1110-1114`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:140-161`。
- Zed：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/editors/zed/src/lib.rs:18-25`、`:53-57`、`:80-113`、`:231-238`。
- 构建/CI：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/scripts/build-release.sh:151-172`、`:218-244`、`:265-283`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/.github/workflows/ci.yml:10-34`。
- 现有门禁：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/no_node_guard.rs:468-490`、`:851-863`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/scripts/verify-no-node.sh:356-369`、`:432-438`；`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/real_asset.rs:182-225`。
- P6-3 依赖：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:7`、`:11-17`、`:102-207`、`:320-335`、`:377-382`。

---

**最终状态：P6-4 BLOCKED / PARTIAL。** 本文只定义实施顺序与门禁，不代表任何代码/测试/交付项已完成；默认后端仍为 Node，Node 未删除，F3 ID 未关闭。
