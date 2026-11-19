# P6-3 原生类型资产与 compilerOptions 实施计划（审计，非完成声明）

- 审计对象：worktree `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`，审计开始时 HEAD `b0a108f`。
- 审计期间其他会话把 HEAD 推进到 `5a451df`（`document symbols` 与 evidence 提交）。本次涉及的 `crates/wxml-backend-tsgo/src/input.rs`、`native/vfs.rs`、`crates/wxml-lsp/src/backend_tsgo/input.rs` 未变；`native/session.rs` 的引用行号已在当前树复核，均未漂移。
- 审计边界：只读源码、契约、包布局与固定资产；未修改生产代码、测试、Cargo、契约、打包器、安装器或回滚文件，未提交。唯一新增文件是本文。
- 工作树状态：审计期间存在其他会话的未提交修改（`native/{ast,mod,session}.rs`、`query/mod.rs`、`wxml-delivery` 的 `package.rs`/`native_install.rs`/`error.rs`/`wxml-lsp-package.rs`、`backend_tsgo/mod.rs`，以及未跟踪的 `document_symbol*`、delivery 测试、三份 evidence）。本文以 HEAD 事实为准；涉及 `package.rs` 的现状只作风险提示，不作为 P6-3 的依赖事实。
- 状态：**P6-3 未解决，BLOCKED**。本文只定义可执行的实施顺序、验收门禁与回滚方式。Native 后端仍不是默认后端，P6-3 的完成也不等于 P6/G5 完成。

---

## 0. 结论摘要

1. Native 后端当前只把项目文件放入 `fileSystem.kind="full"`，合成的 `tsconfig.json` 固定 `noLib:true`、`strict:true`、`module/target esnext`；`AnalysisInput` 没有 compilerOptions，也没有 stdlib/typings 资产。因此 `wx`、`Page`、`Component`、`getApp` 与标准库类型都不可用，AC-001..AC-013 无法原样复现。
2. Node 后端不是“隐式拥有类型”：它通过 `PreparedView.compiler_options`、固定的 `types:["miniprogram-api-typings"]`、`typeRoots` 追加、TS 自身 `lib/` 读策略和 `assertEnvironment` 共同建立类型环境。Native 缺少的是同一组选项与资产，而不是某个单独 flag。
3. 本次只读探针确认了一个关键引擎行为：**当 `createSnapshot` 带 `fileSystem.kind="full"` 时，引擎不会调用任何 FS 回调；stdlib/typings 必须作为虚拟文件出现在 `files` 字典里。** 仅把 `noLib` 改为 `false` 不会让类型出现。
4. 在 full VFS 中加入 `<engine>/lib/lib*.d.ts`（116 个）与 `typings/miniprogram-api-typings/**/*.d.ts`（20 个）后，真实引擎能解析 `wx.getSystemInfoSync`、`Page`、`Component`、`getApp`；`wx.` 补全返回 505 项；缺字段诊断是 2339、赋值错误是 2322，未再出现 2304“Cannot find name 'wx'”。
5. 仍然阻塞 P6-3 完成的不是“能否挂载”，而是：真实包资产布局与 26/23 文件数不一致（交付侧活跃工作）、全量资产每次请求/更新的体积与性能、以及真实引擎在 `updateSnapshot` 后保留旧文本诊断（AC-004 未保存更新）的既有问题。
6. 另外存在不可忽略的版本偏差：Node 后端使用 TypeScript `6.0.3`，native 引擎是 `7.1.0-beta`。即使资产与选项完全一致，也不能宣称 AC-001..AC-013 与 Node “逐字节等价”；必须先跑差分矩阵并对差异做裁决。

---

## 1. Node 后端与 native 后端的精确差异

### 1.1 选项链路

| 环节 | Node 后端（当前基线） | Native 后端（当前） | 缺口 |
| --- | --- | --- | --- |
| 有效选项来源 |  `crates/wxml-lsp/src/project_host.rs:345` `PreparedView.compiler_options` | 同一 `PreparedView` | native 只读 `checkJs` |
| 选项过滤 |  `crates/wxml-lsp/src/project_host.rs:48` 33 个 key；未知 key 置 Partial；始终把 `miniprogram-api-typings` 加入 `types`（`:3258-3345`） | 未传入 | 全部 33 个 key 丢失 |
| 请求发送 |  `crates/wxml-lsp/src/backend_node/request.rs:139-165` 发送 `project.compilerOptions`、`project.typings`、`policy.jsCheck` |  `crates/wxml-lsp/src/backend_tsgo/input.rs:193-205` 只读 `checkJs`，`js_check` 固定 `Inherit` | compilerOptions/typings/jsCheck 丢失 |
| 转换为 TS options |  `backends/typescript/src/protocol.ts:1121-1166`；始终 `noEmit:true`；不补 target/module/strict 默认值 |  `crates/wxml-backend-tsgo/src/native/vfs.rs:685-696` 固定 `noLib:true`、`strict:true`、`module/target esnext` | 既缺选项，又强制了与项目无关的选项 |
| TS 程序 |  `backends/typescript/src/analysis.ts:273-292`，`toCompilerOptions` + `typeRoots` 追加 + `defaultReadPolicy` + `assertEnvironment` |  `native/session.rs:586` 只用 `AnalysisInput` 生成 VFS | 无 typeRoots/资产/环境校验 |

### 1.2 资产链路

| 资产 | Node 后端 | Native 后端 | 缺口 |
| --- | --- | --- | --- |
| TypeScript 标准库 | TS 引擎自身 `ts.getDefaultLibFilePath`；读策略允许 `node_modules/typescript/lib`（`backends/typescript/src/compiler.ts:180-187`） | 合成 tsconfig `noLib:true`，VFS 只含项目文件 | 116 个 `lib*.d.ts` 未挂载 |
| miniprogram typings | `withPinnedTypings` + `defaultReadPolicy` + `assertEnvironment`（`compiler.ts:148-208`）；固定 5.3.1，MIT | 无 `types`/`typeRoots`，无文件 | 20 个 `.d.ts` 未挂载，`wx`/`Page`/`Component`/`getApp` 缺失 |
| 项目 typeRoots | `analysis.ts:274-284` 追加 backend `node_modules` 并把项目 typeRoots 加入读策略 | 未消费 | 项目 typeRoots 语义丢失 |
| 资产身份 | `project.typings.package/version` 校验；`assertEnvironment` 读 `package.json` | 无 | 无版本/哈希/许可证校验 |
| 缺失资产 | `EnvironmentError` → 请求 `Unavailable`，不返回干净结果 | `noLib:true` 让缺失看起来像“没有标准库”，诊断仍可能被当成 Supported | 无 fail-closed |

### 1.3 为什么“只改 noLib:false”不够

本审计用固定引擎 `7.1.0-beta` 做了只读探针（脚本与结果在 `/tmp/opencode/tsgo-spike/p63_*.py`，不属于仓库证据）：

- `fileSystem.kind="full"` + 仅项目文件：引擎 **0 次** FS 回调；`wx`/`Page`/`Component`/`getApp` 全部 2304。
- `fileSystem.kind="full"` + 项目文件 + 全部 `lib*.d.ts` + typings `.d.ts`：仍然 **0 次** 回调，但 `wx` 类型为 `Wx`、`SystemInfo`、`Page`/`Component` 为 `Constructor`、`getApp` 为 `GetApp`，`wx.` 补全 505 项；诊断只剩预期的 2322/2339。
- 不加 `fileSystem`、只用回调：引擎会读取 73 次 `readFile`、30 次 `fileExists`、12 次 `directoryExists`、1 次 `realpath`（其中 51 次 lib、20 次 typings），也能得到同样结果。但这违反契约 §5.1 的 full VFS 要求，且漏注册回调会回退 OS。
- `fileSystem.kind="layer"` + 项目文件 + 回调资产：也能工作，但契约明确禁止用 layer 叠加宿主 FS（`docs/reference/tsgo-native-contract.md:301`）。

结论：P6-3 的主路径必须是 **full VFS 内加入 stdlib/typings 文本**；回调资产只能作为研究/回退，不能作为生产契约。

---

## 2. 固定资产审计（当前可测事实）

### 2.1 miniprogram-api-typings

来源：`backends/typescript/node_modules/miniprogram-api-typings`，`package.json` 版本 `5.3.1`，许可证 MIT。

| 项 | 值 |
| --- | --- |
| 包内文件总数 | 26 |
| 包内总字节 | 2,552,052 |
| 运行时 `.d.ts` 文件 | 20 |
| 运行时 `.d.ts` 字节 | 2,522,752 |
| 交付 allowlist 允许的文件（`index.d.ts`、`LICENSE`、`README.md`、`CHANGELOG.md`、`types/**`） | 23；2,548,075 字节 |
| 全 26 文件 bundle 摘要（manifest 算法：排序后的 `sha256  relative\n`） | `2141aac47206441f0434a7b519e49da1e8756d1bcdb6c17809a7f5e188a34c42` |
| 23 文件 allowlist bundle 摘要 | `ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b` |
| 版本/许可证常量 |  `crates/wxml-delivery/src/manifest_v2.rs:76-84`：`5.3.1`、`MIT`、`typings/miniprogram-api-typings`、26 文件 |

**已知不一致（交付侧，P6-3 不修改）：** 当前未提交的 `crates/wxml-delivery/src/package.rs` 的 `is_allowed_native_path` 只允许 23 个 typings 文件（`:888-895`），但 `native_typings` 仍要求 26 个（`:1119-1127`）。HEAD 的 `package.rs` 没有 v2 打包路径。P6-3 只能消费“已冻结且一致的 manifest”；在交付 owner 修正 26/23 之前，真实包测试必须视为 **BLOCKED**，不能静默改常量或改 allowlist。

### 2.2 TypeScript 标准库（tsgo）

来源：固定引擎包 `@typescript/typescript-linux-x64@7.1.0-beta`，许可证 Apache-2.0。

| 项 | 值 |
| --- | --- |
| `lib*.d.ts` 文件数（含 `lib.d.ts`） | 116 |
| `lib*.d.ts` 总字节 | 3,870,196 |
| bundle 摘要（manifest 算法） | `5e126e7206065b6fff9d651eff3ad66b9413804d3e23d7ecfb4257299af3e85e` |
| `lib/tsc` SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| `lib/tsc.sig` SHA-256 | `e7daebc35e86ac9e30905031e6e83a3b5ea3d1b34b6618ab87c0ec0fc541f107` |
| 上游 tarball | SHA-256 `59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855`，10,175,135 字节，122 文件 |
| 固定常量 |  `crates/wxml-delivery/src/manifest_v2.rs:41-84`；`crates/wxml-backend-tsgo/src/config.rs:24-43` |

### 2.3 必须挂载的最小集合

| 挂载类别 | 包内路径 | 虚拟绝对路径 | 文件数/字节 |
| --- | --- | --- | --- |
| 标准库 | `<package>/engine/lib/lib*.d.ts` | `<package>/engine/lib/...` | 116 / 3,870,196 |
| typings 声明 | `<package>/typings/miniprogram-api-typings/index.d.ts`、`types/**/*.d.ts` | `<package>/typings/miniprogram-api-typings/...` | 20 / 2,522,752 |
| 合计 | | | 136 / 6,392,948（≈6.10 MiB） |

`package.json`、`LICENSE`、`README*`、`CHANGELOG.md` 不挂载到 VFS；它们由包清单校验并随包分发。探针证明缺失 `package.json` 时 `types:["miniprogram-api-typings"]` 仍能回退到 `index.d.ts`；若未来引擎开始强制读 `package.json`，必须按缺失资产 fail-closed，不能读项目磁盘。

---

## 3. 目标设计（精确）

### 3.1 数据模型

在 `crates/wxml-backend-tsgo` 内新增/扩展（不依赖 `wxml-delivery`，不做磁盘 I/O）：

```text
CompilerOptions
  map: BTreeMap<String, serde_json::Value>   // 仅 33 个允许 key，值形状已校验
  digest: String                             // 规范化 JSON 的 SHA-256

AssetPath
  String                                     // 绝对、/ 分隔、无 . / .. / 控制字符 / 反斜杠

TypeAssetFile
  path: AssetPath
  text: Arc<str>
  sha256: String
  size: u64

TypeAssets
  standard_lib_root: String                  // <package>/engine/lib
  typings_root: String                       // <package>/typings
  files: Vec<TypeAssetFile>                  // 只含 .d.ts
  file_count: u32
  total_bytes: u64
  package_digest: String                     // 来自 manifest typeAssets 条目
```

`AnalysisInput` 增加：

```text
compiler_options: CompilerOptions
type_assets: Arc<TypeAssets>
```

保持 `AnalysisInput::new` 的旧签名只用于无资产探针/旧测试，并明确标注“legacy, no type environment”；新增生产构造器（建议名 `AnalysisInput::with_environment`），要求 `CompilerOptions + Arc<TypeAssets>`。生产适配器不得调用 legacy 构造器。

`TypeAssets` 以 `Arc` 共享；`VirtualFile`/`snapshot_files` 的文本改用 `Arc<str>`（或等价共享文本），避免每个请求复制 6.1 MiB。

### 3.2 VFS 挂载

修改 `crates/wxml-backend-tsgo/src/native/vfs.rs`：

1. `VirtualFileSystem` 增加独立 `assets: BTreeMap<String, VirtualFile>`；项目文件表仍受 `root` 约束。
2. `from_analysis_input`：
   - 项目文件 + 合成 `tsconfig.json` 走现有 `build`；
   - 把 `input.type_assets().files()` 插入 `assets`；每个资产路径必须是绝对路径、位于声明的 `standard_lib_root` 或 `typings_root` 下、按路径组件边界匹配、不得与项目文件/tsconfig 重名；
   - 资产目录索引单独维护（供 `fileExists`/`directoryExists`/`getAccessibleEntries`/`stat` 使用），不进入项目根目录表。
3. `snapshot_files()` 返回 `files` + `assets`（full VFS 必须包含资产文本）；`source_paths()`/`absolute_paths()` 仍只返回项目文件（`openFiles` 不应打开资产）。
4. `readFile`/`fileExists`/`stat` 先查项目表再查资产表；`getAccessibleEntries` 对资产根返回组件边界内的直接子项；`realpath` 仍为 `identity`；永不返回 `useOS`。
5. `request_size_upper_bound()` 纳入资产路径、文本和 URI；超过 32 MiB 请求预算时在构建 JSON 前返回 `InputTooLarge`。
6. `from_root_and_files` 保留 handshake/probe 的旧行为（`noLib:true`、`strict:true`、`module/target esnext`），不要让它偷偷加载资产；production 只走 `from_analysis_input`。

### 3.3 `tsconfig.json` 生成

`tsconfig_text` 改为 `tsconfig_text(relative_paths, options) -> Result<String, VfsError>`：

```json
{
  "compilerOptions": {
    "noEmit": true,
    "types": ["<project types...>", "miniprogram-api-typings"],
    "typeRoots": ["<project roots...>", "<package>/typings"],
    "...": "仅传入 PreparedView 中存在的 33 个 key"
  },
  "files": ["<sorted project-relative files>"]
}
```

规则：

- 始终 `noEmit:true`（对齐 `protocol.ts:1121-1123`）。
- 始终把 `miniprogram-api-typings` 放入 `types`；保留项目其他 `types` 的顺序（`project_host.rs:3258-3345` 已经保证，native 再做一次防御性检查）。
- `typeRoots`：
  - 项目有 `typeRoots`：保留项目根，最后追加 `<package>/typings`（对齐 Node `withPinnedTypings` 追加 backend `node_modules` 的语义；native 的 currentDirectory 是项目根，所以必须显式追加）。
  - 项目无 `typeRoots`：显式设置为 `["<package>/typings"]`。这是 Node “cwd=backendDir，默认从 backendDir/node_modules 解析 typings”的等价物；不是新发明的编译选项。
  - 项目 `typeRoots` 位于项目根之外且不在虚拟快照/资产根内：不要读磁盘；从选项移除并在 `RequestPlan.degradation` 记录 `Partial` 原因。
- 不输出 `noLib`（默认 false）。只有 legacy probe 的 `from_root_and_files` 继续输出 `noLib:true`。
- 不输出 `strict`/`target`/`module`/`moduleResolution`/`skipLibCheck`/`lib` 的默认值；Node 生产路径没有这些默认值。`target`/`module` 等只在 `PreparedView.compiler_options` 存在时原样输出（值已由 `project_host` 规范化）。
- `baseUrl` 是 `project_host` 解析后的绝对项目内路径；`paths` 原样；`extends` 已由 `wxml-project` 展平，不在 native 再读。
- 输出使用稳定的 key 排序（`BTreeMap`/`serde_json` 默认排序）以便 digest 稳定；不得把 `Map` 的插入顺序带进缓存键。

### 3.4 exact compilerOptions 集合

从 Node 生产链路追踪得到，native 应接受并转发以下 33 个 key（来自 `project_host.rs:48-82` / `protocol.ts:238-270`）：

```text
strict, strictNullChecks, strictFunctionTypes, strictBindCallApply,
strictPropertyInitialization, noImplicitAny, noImplicitThis,
noUncheckedIndexedAccess, exactOptionalPropertyTypes,
strictBuiltinIteratorReturn, useUnknownInCatchVariables,
allowJs, checkJs, esModuleInterop, allowSyntheticDefaultImports,
resolveJsonModule, isolatedModules, verbatimModuleSyntax,
forceConsistentCasingInFileNames, skipLibCheck,
target, module, moduleResolution, jsx, moduleDetection, lib,
baseUrl, paths, types, typeRoots, rootDir, experimentalDecorators, outDir
```

Native 额外只做三件事：

1. `noEmit:true`（Node 固定）。
2. `types` 确保含 `miniprogram-api-typings`（Node/project_host 固定）。
3. `typeRoots` 追加 `<package>/typings`（Node 通过 cwd + backend node_modules 达到的等价结果）。

不添加 `noLib`、不强制 `strict`、不强制 `target/module/moduleResolution`、不默认 `skipLibCheck`、不默认 `lib`。`jsx` 即使传入也不会生效，因为 `Language::from_path` 明确拒绝 `.tsx/.jsx`；保留转发以保持选项一致性。

### 3.5 mini-program shadow 行为

- `shadow` 文本生成不变；本次只改变类型环境。
- `JsCheckPolicy` 当前硬编码 `Inherit`（`crates/wxml-lsp/src/backend_tsgo/input.rs:202`），而 Node 默认 `Pragma`（`backend_node/config.rs:141`）。P6-3 必须把 `WXML_TEMPLATE_CHECK` 的 `off|pragma|inherit` 映射为 `JsCheckPolicy`，并把 `checkJs` 同时传入 `ShadowRequest` 与 tsconfig。否则 JS 模板（AC-007）仍会与 Node 不一致。
- 该映射可以复用 Node 的 `JsCheckMode` 语义，但不要在 native 里读 `WXML_*` 环境变量；由 P6-C2 的配置层统一注入。

### 3.6 更新路径

当前 `NativeSession::update_project`（`native/session.rs:665-780`）每次发送 `fileSystem.full` + close/open project/files。加入资产后每次更新会重复发送约 6.1 MiB。

计划：

1. 先保持 full 更新以确保正确性，记录 create/update 的请求字节、耗时和 RSS。
2. 若 full 更新超出预算或 p95 不可接受，改用“初始 full + 后续 layer 只含变更项目文件”，并在同一 `updateSnapshot` 中 close/open 项目与文件。探针表明 layer 更新后资产仍可解析、`model` 类型会变为新值；但旧文本诊断仍在（见 §5），所以该优化必须在 AC-004 诊断问题解决后单独验收。
3. 不得为了性能只挂载“本次可能用到的 lib”；默认库闭包由引擎决定，必须全量挂载。

---

## 4. 资产加载与安全

### 4.1 加载位置

建议新增 `crates/wxml-lsp/src/backend_tsgo/assets.rs`（或等价的 engine 发现模块）：

- 从已验证的包根读取 `manifest.json`，取 `typeAssets` 的 `typescript-standard-lib` 与 `miniprogram-api-typings` 条目，以及 `files[]` 的逐文件 SHA-256/大小。
- 固定根：`<package>/engine/lib`、`<package>/typings/miniprogram-api-typings`；不得读取项目 `node_modules`、用户 home、PATH、网络或 npm。
- 读取一次，构造 `Arc<TypeAssets>`；`wxml-lsp` 启动/会话创建时注入。`wxml-backend-tsgo` 只接收已验证文本，不做磁盘 I/O。
- 依赖 P6-A5 的包根/manifest 发现；A5 未完成时，真实包测试只能用显式测试根，生产路径保持不可用。

### 4.2 安全检查清单

- [ ] 包根 `canonicalize` 一次；根自身不是 symlink。
- [ ] 每个资产路径的每个组件 `symlink_metadata` 检查，拒绝 symlink；拒绝 `nlink != 1`（避免硬链接指向包外 inode）。
- [ ] 只接受普通文件；打开后比较 `dev/ino/size` 与打开前的 `symlink_metadata`，再读取；读取上限 = 声明 size，超出即失败。
- [ ] 路径必须是绝对、`/` 分隔、无空组件/`.`/`..`/反斜杠/控制字符；必须按组件边界位于固定根下（`/engine/lib-evil` 不得匹配 `/engine/lib`）。
- [ ] 逐文件 SHA-256 与 manifest `files[]` 比对；bundle 摘要与 `typeAssets[]` 比对；版本/许可证与常量比对。
- [ ] 缺失/哈希不符/数量不符：`Unavailable(MissingAsset)`，且在 spawn 引擎前失败；不得降级为 `noLib:true` 或干净诊断。
- [ ] 资产上限：文件数 ≤ 256，文本 ≤ 8 MiB（当前 136 / 6,392,948 字节）；超限 fail-closed。
- [ ] 许可证/来源：`engine/LICENSE`、`engine/NOTICE.txt`、`typings/miniprogram-api-typings/LICENSE`、`THIRD_PARTY_LICENSES.md` 必须随包并校验；不挂载到 VFS。
- [ ] 不启动 Node/npm/npx，不发起网络；不把 `useOS` 返回给引擎；full VFS 下回调零调用是预期，但 VFS 仍保留 fail-closed 答案。
- [ ] 启动 canary 继续证明项目根外的真实宿主文件对 `readFile`/`stat` 返回 `missing`。

### 4.3 预算

| 预算 | 值 | 依据 |
| --- | --- | --- |
| 挂载资产文件数 | 136（116 lib + 20 typings） | 实测 |
| 挂载资产字节 | 6,392,948 | 实测 |
| 资产硬上限 | 256 文件 / 8 MiB | 预留余量，防止包被替换 |
| 单请求上限 | 32 MiB（现有 `MAX_REQUEST_BODY_BYTES`） |  `crates/wxml-backend-tsgo/src/config.rs:66` |
| 输入项目源上限 | 48 MiB（现有 `MAX_INPUT_BYTES`） |  `crates/wxml-backend-tsgo/src/input.rs:37`；资产单独计量 |

---

## 5. 测试计划（现状与 BLOCKED 边界）

### 5.1 现在就能通过/不能通过的

| 测试 | 现在 | 说明 |
| --- | --- | --- |
| `cargo test -p wxml-backend-tsgo --lib native::vfs` 现有用例 | PASS | 只覆盖项目文件、路径、canary；不覆盖资产 |
| `TSGO_BIN=... cargo test -p wxml-backend-tsgo --test real_engine` 现有用例 | PASS | 现有测试用项目内 `globals.d.ts` 伪造 `Page`，不依赖真实 typings |
| `cargo test -p wxml-lsp --lib backend_tsgo` 现有 fake 用例 | PASS | fake session 不启动引擎，不验证类型资产 |
| 真实 `wx`/`Page`/`Component`/`getApp` 类型 | **BLOCKED** | 没有资产挂载；现有 real-engine hover 只验证 `string` |
| AC-001/002/005/006/007/008/011/012/013 原生真实引擎对照 | **BLOCKED** | 依赖 P6-3 + G5 用例迁移；且 Node 6.0.3 vs native 7.1.0-beta 有版本偏差 |
| AC-003 CJK/emoji 位置 | **BLOCKED**（引擎探针已验证 CJK 标识符可得到 `string`，但仓库内没有原生真实用例） | 需新增真实引擎测试并映射回 UTF-8 |
| AC-004 未保存更新 | **BLOCKED** | 探针显示新快照仍保留旧文本诊断（见 5.3），需先定位引擎/更新语义 |
| 缺资产 fail-closed | **BLOCKED** | 还没有 loader 与 `MissingAsset` 路径 |
| 项目外部 `typeRoots` | **BLOCKED** | 当前 Node 读磁盘；native 没有可表达的虚拟资产 |

### 5.2 Fake VFS 测试（实现后必须全部通过）

文件建议：`crates/wxml-backend-tsgo/tests/type_assets.rs`，以及 `native/vfs.rs` 的 `#[cfg(test)]`：

1. `mounts_standard_lib_and_typings_as_virtual_files`：小规模 synthetic `TypeAssets`，断言 `snapshot_files()` 同时包含项目文件、`tsconfig.json`、`<engine>/lib/lib.d.ts`、`<typings>/miniprogram-api-typings/index.d.ts`；`source_paths()` 不含资产。
2. `tsconfig_uses_prepared_options_without_forcing_defaults`：`strict:false`、`target:ES2020`、`module:CommonJS`、`moduleResolution:node10`、`skipLibCheck:true`、`lib:["es2020"]`、`allowJs:true`、`checkJs:false`；断言 `noLib` 不存在、`noEmit:true`、`types` 含 typings、`typeRoots` 末尾为 `<package>/typings`。
3. `legacy_probe_keeps_no_lib`：`from_root_and_files` 仍输出 `noLib:true`，且不加载资产。
4. `rejects_asset_outside_fixed_root`、`rejects_asset_traversal`、`rejects_duplicate_asset_path`、`rejects_asset_project_collision`。
5. `rejects_oversized_asset_set`、`rejects_unknown_asset_hash`（如果 `TypeAssets` 在 native 侧重验）。
6. `request_bound_includes_assets`：构造接近上限的资产，断言 `request_size_upper_bound()` 包含资产且在 32 MiB 预检处拒绝。
7. `asset_directories_answer_component_confined_entries`：`getAccessibleEntries(<package>/engine/lib)` 只返回该目录直接子项，不返回 `/`、项目根或宿主目录。
8. `runtime_callbacks_never_use_os`：在 full VFS 下即使引擎不调用回调，`answer` 对资产/越界路径仍保持 fail-closed 形状。

### 5.3 真实引擎测试（实现后）

建议在 `crates/wxml-backend-tsgo/tests/real_engine.rs` 与 `crates/wxml-lsp/src/backend_tsgo/tests.rs` 增加，统一由 `TSGO_BIN` 控制；资产根从安装布局推导（`TSGO_BIN` = `<package>/engine/lib/tsc` → package root = 上两级），或测试专用 `TSGO_ASSET_ROOT`。真实包必须用 `WXML_REQUIRE_REAL_ENGINE=1` 使缺失成为硬失败。

1. `real_engine_wx_page_component_getapp`：
   - `wx.getSystemInfoSync` hover/type → `Wx`，返回值 → `SystemInfo`；
   - `Page`/`Component` → `Constructor`；`Page({data:{count:1}})` 的 `this.data.count` → `number`；
   - `Component` 的 `this.data.n` → `number`；
   - `getApp` → `GetApp`；
   - `wx.` completion 至少包含 `getSystemInfoSync`，且总项数接近 505（不要硬编码 505，断言关键项）。
2. `real_engine_missing_member_diagnostics`：`info.notAField` → 2339；`const bad:number=info.model` → 2322；不得出现 2304 `wx`/`Page`/`Component`/`getApp`。
3. `real_engine_missing_asset_fails_closed`：删除一个 typings `.d.ts` 或篡改哈希 → `Unavailable(MissingAsset)`；不得 spawn 后返回干净诊断。
4. `real_engine_cjk_positions`：`const 标题: string = info.model;` 的 hover 为 `string`；WXML 中文/emoji 表达式映射回原始 UTF-8 范围；断言 UTF-16 位置与原始字节范围一致。
5. `real_engine_unsaved_update`：修改 `const model: string` → `number`，close/open 项目+文件并 `updateSnapshot`；断言新快照只有 `bad` 的 2322，不再有旧的 `model` 2322。**当前探针失败**：新旧诊断同时存在，见下。
6. `real_engine_project_type_roots`：项目内 `typeRoots` + `types:["outer"]` 的 `Outer.X` 可解析；项目外/未入快照的 typeRoots → `Partial`，不得读磁盘。
7. `real_engine_js_pragma`：JS `Page` 在 `WXML_TEMPLATE_CHECK=pragma` 下产生模板诊断；`off` 下为 `Disabled`/`Partial` 而不是干净 `Supported`；`checkJs:true` 与 `@ts-nocheck` 行为对齐 Node。

### 5.4 本次探针的未保存更新结果（必须记录为风险）

在 `/tmp/opencode/tsgo-spike/p63_e2.py`：

- 初始文本 `const model: string = info.model;` 与新文本 `const model: number = info.model;`。
- `updateSnapshot` 同时 `closeProjects/openProjects/closeFiles/openFiles`，`fileSystem.kind="full"`，并已 `release` 旧 snapshot。
- 新 snapshot 的 `getSemanticDiagnostics` 仍返回 **旧行 1 的 2322** 和新行 2 的 2322；`getTypeAtPosition` 在 `model` 上已返回 `number`。
- 这不是 P6-3 引入的；它阻挡 AC-004 的“未保存更新”验收。P6-3 的测试应把它标为 BLOCKED，并由 native/F1 owner 决定是引擎 bug、更新语义还是诊断缓存问题；不得用“新类型正确”掩盖旧诊断。

---

## 6. 有序文件级变更清单

> 所有条目都是计划，未实施。顺序按依赖排列；每项独立提交/评审。P6-3 不修改 `wxml-delivery/src/package.rs`、`native_install.rs`、rollback/active symbols 文件。

| # | 变更 | 文件（精确锚点） | Owner | 依赖 | 验收门禁 | 回滚 |
| --- | --- | --- | --- | --- | --- | --- |
| 3-1 | 定义 `CompilerOptions`、`AssetPath`、`TypeAssetFile`、`TypeAssets`、预算与 digest；扩展 `AnalysisInput`（保留 legacy 构造器） |  `crates/wxml-backend-tsgo/src/input.rs:212-294`；新增 `crates/wxml-backend-tsgo/src/native/type_assets.rs`；`crates/wxml-backend-tsgo/src/native/mod.rs:27-37` | Native backend | 无 | `cargo test -p wxml-backend-tsgo --lib`；新类型单测；未知 key/越界路径/超预算拒绝；现有测试仍绿 | 纯新增类型 + legacy 构造器保留；revert 不影响默认后端 |
| 3-2 | VFS 挂载资产、生成 options tsconfig、full snapshot 包含资产、请求预算包含资产 |  `crates/wxml-backend-tsgo/src/native/vfs.rs:126-149, 294-299, 454-463, 685-696` | Native backend | 3-1 | `cargo test -p wxml-backend-tsgo --lib native::vfs`；fake 资产测试 5.2 全绿；legacy probe 仍 `noLib:true` | `from_analysis_input` 可在无 assets 时回退旧 tsconfig；不得在生产回退 |
| 3-3 | Session open/update 消费 `AnalysisInput` 环境；资产身份校验；保持 probe legacy；记录 full 更新字节/耗时 |  `crates/wxml-backend-tsgo/src/native/session.rs:586-650, 665-780, 1666-1674` | Native session | 3-2 | 现有 real-engine 测试通过；full 更新预算通过；无资产时 `MissingAsset` 不进入 session | 保持 Node 默认；session 路径可 revert |
| 3-4 | 新增 LSP 资产 loader：manifest、固定根、symlink/hash/许可证、`Arc<TypeAssets>`、`MissingAsset` | 新增 `crates/wxml-lsp/src/backend_tsgo/assets.rs`；建议与 P6-A5 `engine.rs` 共用 package root/manifest 访问 | LSP/delivery/security | 3-1；P6-A5 的 package root/manifest accessor | loader 单测：正常、缺失、篡改、symlink、硬链接、越界、超预算；不启动 Node/网络 | 不启用 native 后端；loader 可单独 revert |
| 3-5 | `build_request_plan` 传递 compilerOptions/assets；`RequestIdentity` 加 options/assets digest；`js_check` 映射；项目 typeRoots 降级 |  `crates/wxml-lsp/src/backend_tsgo/input.rs:74-128, 193-205, 267-341`；`crates/wxml-lsp/src/backend_tsgo/mod.rs` 构造器 | LSP adapter | 3-1/3-4 | fake adapter：options 原样进入 `AnalysisInput`；缺资产 `Unavailable`；typeRoots 越界 `Partial`；无 `noLib` | 保持 `TsgoBackendAdapter` opt-in；Node 默认不变 |
| 3-6 | Adapter 构造时注入 `Arc<TypeAssets>`（来自 3-4/A5）；生产构造器禁止 legacy 输入 |  `crates/wxml-lsp/src/backend_tsgo/mod.rs:118-150`；`crates/wxml-lsp/src/backend_tsgo/session.rs` | LSP adapter | 3-4/3-5 | 真实引擎 adapter 测试 5.3 的 1/2/4 通过；`missing asset` 不 spawn | 不切换默认 `main.rs`；adapter 可单独移除 |
| 3-7 | 新增/扩展测试：fake VFS、loader、真实引擎 wx/Page/Component/getApp、CJK、缺资产；更新测试保持 BLOCKED 标记 |  `crates/wxml-backend-tsgo/tests/type_assets.rs`（新增）；`crates/wxml-backend-tsgo/tests/real_engine.rs:112-...`；`crates/wxml-lsp/src/backend_tsgo/tests.rs:2498-...` | Test/native | 3-1..3-6 | `cargo test` 全绿；`WXML_REQUIRE_REAL_ENGINE=1` + 固定资产；每个新用例 RED/GREEN 证据 | 测试可单独 revert；不影响生产 |
| 3-8 | Evidence/AC 映射：记录 136/6.1 MiB 资产、真实引擎结果、版本差异和 BLOCKED 项；不更新 G5 计数 | 本文 + 后续 evidence；`docs/plans/tsgo-test-ledger.json` 由 parity owner 更新 | Docs/parity | 3-7 | 文档明确“P6-3 未完成”；不得把 BLOCKED 写成 green | 文档 revert |
| 3-9 | 契约修订（单独评审）：full VFS 资产文件、`AnalysisInput` 字段、full→layer 更新语义、typeRoots 越界降级 |  `docs/reference/tsgo-native-contract.md:301, 359-379` | Contract owner | 3-1/3-2/3-3 | 契约测试/评审通过；实现与契约一致 | 不修改契约则不得实现 layer 优化 |
| 3-10 | 交付依赖（不由 P6-3 修改）：统一 typings 26 文件与 allowlist 23 的冲突；v2 包实际包含 116 lib + 26 typings + 许可证 |  `crates/wxml-delivery/src/package.rs`（活跃未提交）；`manifest_v2.rs:41-84` | Delivery owner | B1/B2 | 真实包 `typeAssets` 与 `files[]` 可被 loader 完整校验；两次构建同 hash | 旧 release/Node 默认继续可用 |

---

## 7. 验收门禁（命令级）

```bash
# 1. 编译/lint
cargo fmt --check
cargo clippy --all-targets -- -D warnings

# 2. Native DTO/VFS（不需要引擎）
cargo test -p wxml-backend-tsgo --lib native::vfs
cargo test -p wxml-backend-tsgo --test type_assets

# 3. Fake adapter（不需要引擎）
cargo test -p wxml-lsp --lib backend_tsgo

# 4. 真实引擎（固定 7.1.0-beta + 固定包根）
TSGO_BIN=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc \
TSGO_ASSET_ROOT=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package \
WXML_REQUIRE_REAL_ENGINE=1 \
cargo test -p wxml-backend-tsgo --test real_engine -- --nocapture

TSGO_BIN=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc \
TSGO_ASSET_ROOT=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package \
WXML_REQUIRE_REAL_ENGINE=1 \
cargo test -p wxml-lsp --lib backend_tsgo::tests::optional_real_engine -- --nocapture

# 5. 工作区
cargo test --workspace
```

门禁判定：

- 3-1/3-2 合并后，现有 `cargo test` 必须全绿，且没有生产路径选择 legacy 输入。
- 3-4/3-5/3-6 合并后，真实引擎必须同时满足：`wx`/`Page`/`Component`/`getApp` 无 2304；缺字段 2339；赋值错误 2322；`wx.` 补全含关键 API；缺资产 fail-closed。
- AC-004 不在 P6-3 的门禁内；在 5.4 的旧诊断问题关闭前，`real_engine_unsaved_update` 保持 `#[ignore]`/BLOCKED，不得计入 green。
- AC 与 Node 的逐项等价必须另开差分任务；P6-3 只能证明“类型环境可用”，不能证明 AC 全绿。

---

## 8. 回滚

- P6-3 不改变默认后端：`main.rs` 仍只构造 Node 后端；native 适配器保持 opt-in。任一提交 revert 都回到 Node 默认。
- 3-1 保留 `AnalysisInput::new` legacy 构造器与 `from_root_and_files` 的 `noLib:true` 行为；revert 3-2/3-3 后旧 real-engine 测试仍可运行。
- 生产路径不得用 `noLib:true` 作为“资产缺失回退”；loader 失败必须是 `Unavailable(MissingAsset)`，否则会重新制造 AC 不可复现的静默降级。
- 3-10 的交付改动独立于 P6-3；若交付未就绪，P6-3 只能合并 fake/loader 测试，真实包 AC 门禁保持 BLOCKED。

---

## 9. 风险与未决问题

| 风险 | 等级 | 说明 | 缓解 |
| --- | --- | --- | --- |
| Node TS 6.0.3 vs native 7.1.0-beta 行为偏差 | **HIGH** | 类型推断、lib 检查、诊断文本可能不同；不能宣称 AC 精确等价 | 先跑同一 fixtures 的差分矩阵；差异按 `retired-with-decision` 或另行裁决；不要静默改期望 |
| full VFS 每次约 6.1 MiB 资产 | **HIGH** | 初次快照可接受；每次 `updateSnapshot` 全量重发会伤延迟/RSS | 先测 full；再评估 layer 更新；必要时缓存/共享 `Arc<str>`；设 p95/RSS 门禁 |
| update 后旧诊断残留 | **HIGH** | 探针：close/open 项目+文件并 release 旧 snapshot 后，新快照仍返回旧 2322；AC-004 直接阻塞 | 由 F1/native owner 复现并定位；在修复前把 AC-004 标 BLOCKED |
| 交付 26 vs 23 typings 文件不一致 | **HIGH** | 当前未提交 `package.rs` allowlist 只允许 23，manifest 要求 26；真实包可能无法通过 loader 校验 | 交付 owner 统一常量/allowlist/staging；P6-3 不自行改打包器 |
| 引擎 full VFS 不调用 FS 回调 | **MEDIUM** | 任何“靠回调提供 lib/typings”的设计在本引擎版本上不成立 | 资产必须进入 `files`；保留回调 fail-closed 作为纵深防御 |
| 项目外部 `typeRoots`/`node_modules` | **MEDIUM** | Node 读磁盘，native 不能；可能改变 AC-008 等结果 | 只接受虚拟快照内 typeRoots；越界显式 `Partial`，不读磁盘 |
| typings 5.3.1 与 tsgo 7.1 的 lib 检查 | **MEDIUM** | `skipLibCheck` 默认 false 时可能出现新 lib 诊断 | 真实引擎全量 fixture 跑一遍；必要时由项目配置决定，不默认 skipLibCheck |
| `noLib:true` legacy 路径被误用 | **MEDIUM** | 现有 `AnalysisInput::new` 仍可生成无资产输入 | 生产构造器/类型层禁止 legacy；加测试断言生产 plan 必含 assets |
| 资产摘要/版本在 lsp 与 delivery 重复 | **MEDIUM** | 两处常量漂移会造成误判 | 以 manifest/v2 `typeAssets` 为唯一来源；native 只校验，不复制版本常量；P6-A5 统一 package root/manifest |
| CJK/UTF-16 映射 | **MEDIUM** | 资产不改变位置语义，但 AC-003 必须用真实引擎重新证明 | 5.3 的 CJK 用例；UTF-16↔UTF-8 双向往返断言 |
| `jsCheck` 仍硬编码 `Inherit` | **MEDIUM** | JS 模板诊断与 Node 默认 `Pragma` 不一致，影响 AC-007 | 3-5 映射 `WXML_TEMPLATE_CHECK`；补真实引擎 JS 用例 |

---

## 10. 证据附录（本次只读探针）

- 引擎：`/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc`，`7.1.0-beta`；`lib/tsc` SHA-256 `a183b195...faaa70`。
- 资产：`backends/typescript/node_modules/miniprogram-api-typings` 5.3.1（MIT）；`engine/lib/lib*.d.ts` 116 个。
- 探针脚本（不在仓库内）：
  - `/tmp/opencode/tsgo-spike/p63_e2.py`：full VFS + assets 的 wx/Page/Component/getApp、诊断、update。
  - `/tmp/opencode/tsgo-spike/p63_mode.py`：C（synthetic full assets）、D（callbacks-only）、E（openProjects full assets）对比。
  - `/tmp/opencode/tsgo-spike/p63_probe.py`、`p63_variants.py`、`p63_synth.py`、`p63_isolate.py`：排除 fileSystem/回调/选项组合。
- 关键结果：
  - full + assets：0 回调；`wx`=`Wx`，`info`=`SystemInfo`，`Page`/`Component`=`Constructor`，`getApp`=`GetApp`；`wx.` 505 项；诊断 2322/2339。
  - full 无 assets：`wx`/`Page`/`Component`/`getApp` 均 2304；`xs[0]` 退化为 `{}`。
  - callbacks-only：73 `readFile`、30 `fileExists`、12 `directoryExists`、1 `realpath`；51 lib + 20 typings 读取。
  - update：新快照仍保留旧行 1 的 2322（见 §5.4）。

这些探针是设计证据，不是仓库测试，也不改变任何 AC/ledger 状态。
