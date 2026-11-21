# ADR-0010: 零配置惰性小程序发现与工作区包挂载

**Date**: 2026-11-20

**Status**: accepted（2026-11-20 已实现并通过真实工作区探针）

**Deciders**: 项目维护者

**局部替代**：[ADR-0006](0006-explicit-mini-program-roots.md) 的显式 `apps` 契约、路径校验和显式模式下的“不扫描/不向上查找”规则继续有效；其省略 `apps` 时默认 `root: "."` 的行为由本文替代。本文新增默认零配置模式，并补充工作区包源码进入类型快照的规则。ADR-0001、0002、0003 的分层、类型桥和同步原则不变。

## Context

ADR-0006 要求通过 `initializationOptions.apps` 显式声明小程序根和类型项目根，以避免目录扫描带来的不确定性。实际使用中出现了三类无法由显式配置低成本覆盖的情况：

1. 构建期生成页面路由。`app.json` 只保留源码中可静态列出的页面，生成式页面（例如 `feedback/pages/feedback-detail/index`）不在其中，LSP 因找不到 view 而无法提供类型 hover。
2. 同一仓库的多个 worktree 需要同步 `.zed/settings.json`。worktree 中没有该文件时，默认 `{ "root": "." }` 指向仓库根，找不到 `app.json`，所有模板都退化为无类型。
3. monorepo 工作区包。小程序通过 `node_modules/jl-api -> packages/api` 引用工作区包，而类型快照的边界是 `projectRoot`；包源码未进入快照时，tsrs 无法解析其中的枚举和类型。

同时，Zed 扩展已经能通过 `Worktree.root-path` 获得工作区根，LSP 也已从 `initialize.rootUri` / `workspaceFolders` 获得同一根目录。继续要求用户重复声明根目录，收益低于维护成本。

## Decision

### 默认模式：零配置惰性发现

省略 `initializationOptions`、使用 `null`、`{}` 或 `{ "apps": [] }` 时，进入 **lazy workspace mode**：

- 不合成默认 app，不在启动时加载项目；
- 记录 canonical workspace root；
- 第一次遇到文档 URI 或项目路径时，从该路径向上发现所属项目；
- `is_project_open` 在 lazy mode 下为 true，但项目集合可以暂时为空。

显式配置 `apps` 时仍使用 ADR-0006 的显式模式，不执行向上发现；显式模式是覆盖和逃生口。

### 从当前 WXML 向上发现

对 `apps/miniprogram/src/feedback/pages/feedback-detail/index.wxml` 这类路径：

1. 从文件所在目录向上到 workspace root，寻找最近的 `app.json`；
2. 同一次向上遍历中独立寻找最近的 `tsconfig.json`，它可以在 `app.json` 之上；
3. 项目身份目录按以下优先级确定：
   - `tsconfig.json` 所在目录；
   - `app.json` 所在目录；
   - 最近的 `package.json` 所在目录；
   - 文件所在目录或 workspace root。
4. `projectRoot` 固定为 workspace root；`app.json` 与 `tsconfig.json` 保存为 workspace 相对路径。若向上没有找到 `app.json`，再尝试身份目录下的 `app.json` 和 `身份目录/src/app.json`；仍未找到时保留默认路径，由 discovery/tsconfig 加载报告缺失，不阻止视图降级。
5. 若 `app.json` 缺失，不把它当作“不是页面”的判据；hover 只要求存在相邻的 owner script。

`app.json` 仍是页面路由、全局 `usingComponents` 和页面配置的来源，但降级为**可选增强**：存在时参与发现，不存在时不影响相邻脚本驱动的 hover。

### 惰性视图

首次打开或查询某个 WXML 时：

- 若 discovery 已给出页面/组件视图，直接使用；
- 否则尝试同目录、同 basename 的 `.ts` 或 `.js`；
- 通过 `registration_style_hints` 判断 `Page` / `Component`，无法判断时按 `Component` 处理；
- 相邻 JSON 提供本地 `usingComponents`，最近的 `app.json` 提供全局组件；
- 插入标记为 `lazy` 的 `ViewState`，并保留到后续 rediscovery；discovery 找到正式视图时以正式视图为准。

因此，生成式页面无需出现在 `app.json`，也无需新增配置即可获得类型 hover。

### 工作区包

工作区包不从 `node_modules` 读取，而是从工作区清单发现：

- 根 `package.json#workspaces`（数组或 `{ "packages": [...] }`）；
- 或 `pnpm-workspace.yaml` 的 `packages:` 列表；
- 支持精确目录路径和单个整段 `*`（`*` 必须是完整路径段，可后接 `/子路径`，例如 `packages/*/src`）；不支持段内前后缀或 `**`；pnpm-workspace 的 `!` 取反项忽略；
- 对当前小程序包的 `dependencies` / `devDependencies` / `peerDependencies` 中同名的工作区包，加入 `source_roots`；
- 读取包 `package.json#exports`，把 `"."` / `"./x"` 映射为包名 / `包名/x` 的 TypeScript `paths`，目标为 workspace 相对源文件路径；
- 不读取第三方 `node_modules` 内容，不推断包图，不执行 npm/pnpm 解析。

显式 `apps[].dependencies` 继续作为显式模式的等价配置；lazy mode 自动完成同样的源码挂载和别名生成。

### 源码预算

源码预算按项目边界选择：`projectRoot` 等于 workspace root 时使用 `SourceLimits::workspace()`：

- 最多 8192 个源码文件；
- 总字节上限 64 MiB；
- 单文件上限 16 MiB；
- 仍排除 `node_modules`、`dist`、构建产物等目录，不跟随 symlink 目录。

`projectRoot` 不是 workspace root 时使用 2048 文件 / 16 MiB 的默认预算。lazy mode 以及显式指定 `projectRoot: "."` 都属于前一种情况。

## Alternatives Considered

| 方案 | 优点 | 代价及不采用原因 |
| --- | --- | --- |
| 保持 ADR-0006 的纯显式配置 | 边界最确定，启动不扫描 | 生成式页面、worktree 和多包 monorepo 都需要重复配置；用户实际已遇到这些失败 |
| 让类型引擎直接读取宿主文件系统和 `node_modules` | 零配置，模块解析最完整 | 破坏快照确定性、未保存缓冲区一致性和“引擎不读项目目录”的安全边界；缓存与版本不可复现 |
| 启动时递归扫描所有 `app.json` 与 `tsconfig.json` | 首次查询快 | 大仓库启动成本高，生成目录和构建产物会产生误发现；与惰性查询相比没有明显收益 |
| 仅按 WXML 相邻脚本推导，不读取 `app.json`/`tsconfig.json` | 实现最小 | 失去路由、全局组件、别名和项目编译选项；`@/` 等路径无法解析 |

## Consequences

### Positive

- worktree 不需要同步 `.zed/settings.json`；
- 生成式页面自动获得类型 hover；
- 工作区包通过显式清单进入快照，不需要手写 `paths`；
- 显式 `apps` 仍可精确控制边界，兼容既有配置和测试；
- 类型引擎继续只看到虚拟快照，安全边界不变。

### Negative

- 首次打开 WXML 时才建立项目，首查询承担项目打开和源码收集成本；
- workspace 预算高于单 app，内存和扫描范围更大；
- 最近 `tsconfig.json` 选择暂不展开 solution-style `references`；复杂项目仍建议使用显式 `apps`；
- 多个项目共享 workspace root 时，`projectId` 由身份目录区分，`mark_shared_config_stale` 需按 tsconfig 路径判断，而不是按 project root；
- `view_templates` / 语义工具只能看到已经惰性建立的视图，未打开的 WXML 不在枚举结果中。

## Migration

1. 删除 `.zed/settings.json` 中的 `lsp.wxml-lsp.initialization_options` 即可进入 lazy mode；
2. 需要固定边界或使用 solution-style tsconfig 的项目继续保留显式 `apps`；
3. 显式模式的 `tsconfig` / `dependencies` 配置保持不变；
4. 修改根或 tsconfig 后仍需重启 LSP；lazy mode 没有根热更新。

## Validation

验收覆盖：

- 无 `initializationOptions` 时打开嵌套 WXML，能找到最近 `app.json` 和独立最近的 `tsconfig.json`；
- `app.json` 未声明的页面通过相邻脚本建立 lazy view；
- 无相邻脚本时返回 syntax-only，并给出 `template has no adjacent page or component script`；
- 工作区包源码进入 source roots，`exports` 生成别名，`jl-api` 枚举可 hover；
- 显式 `apps` 的既有行为、路径包含校验和错误码不变；
- workspace 源码预算与排除目录生效；
- 真实 jielong4 主 checkout 和 worktree 在无 `.zed/settings.json` 时通过 `QuestionTypeEnum`、`feedback-detail`、`right-fx-input` hover 探针。

## 实现与真实工作区验证（2026-11-20）

本决策已落地到 `wxml-lsp` 的 lazy workspace mode，并用真实 jielong4 工作区验证：

- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过；
- `cargo test --workspace --all-features` 为 2168 passed / 3 ignored；
- `cargo doc --workspace --no-deps` 与 `cargo deny --offline check` 通过；
- 主 checkout 与 `placid-strand` worktree 都使用**不含 `initializationOptions`** 的 `initialize`：
  - `apps/miniprogram/src/exam-online/pages/edit-question/index.wxml` 的 `QuestionTypeEnum` 返回 `typeof QuestionTypeEnum`；
  - `apps/miniprogram/src/feedback/pages/feedback-detail/index.wxml` 的 `Thread` 返回 `ThreadDetail`，`Thread.GroupId` / `Thread.Subject` 返回 `number` / `string` 及字段文档；
  - `right-fx-input` 返回 `` `right-fx-input` — WXSS class ``，不存在的类名对照返回 `null`。

已知但不阻塞本决策的残留：工作区包的第三方依赖不在快照内，tsrs 会报告部分 `typeRoots`/模块解析降级；模板 hover 的目标行为不受影响。

## References

- [ADR-0006：显式小程序根与类型项目根](0006-explicit-mini-program-roots.md)
- [ADR-0012：进程内 tsrs 类型引擎会话](0012-in-process-tsrs-session.md)（原 C1 类型桥与 Node 后端已删除，历史见 tag `pre-slim-20261120`）
- [Zed 初始化选项](../../editors/zed/README.md#mini-program-roots-initialization-options)
