# P6-3 LSP Adapter 类型资产接线审计与有序实施计划（审计，非完成声明）

- 审计对象：worktree `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`，HEAD `4ad3459`（`docs(evidence): plan F3 asset lock provisioning`）。
- 工作树事实：未提交 slice 为 `crates/wxml-backend-tsgo/src/{input.rs,native/vfs.rs,native/session.rs}` 的修改，加上未跟踪的 `crates/wxml-backend-tsgo/tests/type_assets.rs`、`crates/wxml-delivery/tests/native_real_upgrade.rs`。本审计按“HEAD + 该未提交 slice”读取，不修改它们。
- 审计边界：只读源码/契约/交付清单/测试；**未修改**生产代码、测试、Cargo、契约、打包器/安装器的默认行为、Node 后端、默认后端选择；未提交。唯一新增文件是本文。
- 状态：**P6-3 未完成**。本 slice 只完成了 native 数据层（DTO/VFS/session 映射），LSP adapter 仍然构造 legacy `AnalysisInput`，没有加载、校验或传递任何类型资产；生产 native 路径仍是 `noLib:true`。

---

## 0. 结论摘要

1. backend slice 已经具备“接收环境”的能力：`AnalysisInput::with_environment`、33 个 `COMPILER_OPTION_KEYS`、`TypeAssets`/`TypeAssetFile`/`AssetPath`、VFS 全量挂载、`MissingAsset` 映射都已落地（锚点见 §1）。legacy `AnalysisInput::new` 被保留且语义未变。
2. **接线缺口是单一的、可定位的**：`crates/wxml-lsp/src/backend_tsgo/input.rs:120-128` 的 `RequestPlan::input` 仍然调用 `AnalysisInput::new(...)`；`RequestPlan`（`:100-116`）没有 options/assets 字段；`RequestIdentity`（`:73-97`）不包含 options digest 或 asset digest；`TsgoBackendAdapter`（`crates/wxml-lsp/src/backend_tsgo/mod.rs:106-112`）没有资产来源。
3. 因此现在任何走 adapter 的请求都会生成 `noLib:true` 的 tsconfig，`wx`/`Page`/`Component`/`getApp` 与标准库类型必然缺失；这与 P6-3 计划 §0.1/§1.2 的诊断一致，且**不会**因为 backend slice 合入而改变。
4. adapter 侧完全不存在资产代码：`crates/wxml-lsp/src/` 内没有任何 `TypeAssets`/`type_assets` 引用；`wxml-lsp` 也没有 `wxml-delivery` 依赖（`crates/wxml-lsp/Cargo.toml:6-17`），`sha2` 只在 dev-dependencies（`:19-21`）。
5. 资产身份还缺一条显式链：`TypeAssets` 只保存 `package_digest` 并只校验其 sha256 形状（`crates/wxml-backend-tsgo/src/input.rs:345-350`），不校验它是否覆盖所挂载的文本。loader 必须用 manifest 的 `typeAssets[].sha256`（由交付侧 `compute_type_asset_bundle` 计算）作为该字段，不能自行对挂载文本重算后比对（原因见 §2.3）。
6. 每次 open/update 都会重发约 6.1 MiB 资产文本（`crates/wxml-backend-tsgo/src/native/session.rs:601-607`、`:703`；`crates/wxml-backend-tsgo/src/native/vfs.rs:307-313`），这是契约 §5.1 的 full 快照要求。P6-3 不应偷偷改成 layer；先计量并在门禁里记录成本，layer 优化单独阻塞（§4.4）。
7. 交付侧仍存在 26 vs 23 typings 文件数的互斥冻结值（`crates/wxml-delivery/src/package.rs:1097-1104` vs `:1326-1334`），因此“真实安装包 loader 端到端通过”仍然 BLOCKED；P6-3 只能先落 fake/显式根测试。
8. 结论：**P6-3 的下一步是 adapter 接线，不是再改 backend**。顺序见 §6；P6-3 完成也不等于默认后端切换（§7）。

---

## 1. 事实基线

### 1.1 backend slice 已具备的能力

| 能力 | 精确锚点 | 语义 |
| --- | --- | --- |
| 33 个 compiler option key |  `crates/wxml-backend-tsgo/src/input.rs:58-92` | 与 `crates/wxml-lsp/src/project_host.rs:48-82`、`backends/typescript/src/protocol.ts:217-248` 同集合 |
| 值形状校验（21 bool / 8 string / 3 string[] / paths） |  `crates/wxml-backend-tsgo/src/input.rs:1048-1083`、`:1088-1124` | 未知 key → `UnsupportedCompilerOption`；错形状 → `InvalidCompilerOption` |
| 稳定 options digest |  `crates/wxml-backend-tsgo/src/input.rs:131`、`:1170-1173` | 对 `BTreeMap` 的规范 JSON 做 SHA-256，键序无关 |
| `AssetPath` |  `crates/wxml-backend-tsgo/src/input.rs:169-209` | 绝对、`/`、无 `.`/`..`/空组件/反斜杠/控制字符；不访问磁盘 |
| `TypeAssetFile` |  `crates/wxml-backend-tsgo/src/input.rs:216-299` | 仅 `.d.ts`；size 与 SHA-256 必须与文本一致（`:238-261`） |
| `TypeAssets` |  `crates/wxml-backend-tsgo/src/input.rs:306-467` | 固定根、必需声明、预算、重叠根；纯内存 |
| 预算 |  `crates/wxml-backend-tsgo/src/input.rs:97-106` | 256 文件 / 8 MiB（实际 136 / 6,392,948 B） |
| 环境构造器 |  `crates/wxml-backend-tsgo/src/input.rs:783-799` | 携带 `CompilerOptions + Arc<TypeAssets>`；legacy `new` 保留于 `:761-775` |
| VFS 资产挂载 |  `crates/wxml-backend-tsgo/src/native/vfs.rs:153-186`、`:836-874` | 独立 assets 表 + 独立目录索引；不进 `source_paths()`（`:270-275`） |
| full 快照包含资产 |  `crates/wxml-backend-tsgo/src/native/vfs.rs:300-313` | `snapshot_files()` = 项目文件 + tsconfig + 资产 |
| environment tsconfig |  `crates/wxml-backend-tsgo/src/native/vfs.rs:895-928` | `noEmit:true`；`types` 追加 typings；`typeRoots` 追加 `<package>/typings`；**不输出 `noLib`**；不强制 strict/target/module |
| legacy tsconfig |  `crates/wxml-backend-tsgo/src/native/vfs.rs:876-887` | 仍 `noLib:true, strict:true, module/target esnext` |
| 失败映射 |  `crates/wxml-backend-tsgo/src/native/session.rs:2181-2187` | `VfsError::Asset` → `Unavailable(MissingAsset)`；重复资产 → `Protocol`；撞项目文件 → `InvalidParams` |
| 资产目录边界 |  `crates/wxml-backend-tsgo/src/native/vfs.rs:766-818` | 只索引固定根及其后代，不暴露祖先 |
| 请求预算 |  `crates/wxml-backend-tsgo/src/native/vfs.rs:470-479`、`crates/wxml-backend-tsgo/src/config.rs:66` | upper bound vs 32 MiB |
| 测试 |  `crates/wxml-backend-tsgo/tests/type_assets.rs:99-330`、`:514-622`；`crates/wxml-backend-tsgo/src/native/vfs.rs:1372-1470` | fake + 真实引擎（opt-in） |

### 1.2 adapter 仍然是 legacy

| 事实 | 精确锚点 | 后果 |
| --- | --- | --- |
| `RequestPlan::input` 调用 `AnalysisInput::new` |  `crates/wxml-lsp/src/backend_tsgo/input.rs:120-128` | 无 options、无资产 |
| `RequestPlan` 无环境字段 |  `crates/wxml-lsp/src/backend_tsgo/input.rs:100-116` | adapter 无处存放 `CompilerOptions`/`Arc<TypeAssets>` |
| `RequestIdentity` 无 options/asset 身份 |  `crates/wxml-lsp/src/backend_tsgo/input.rs:73-97` | tsconfig/资产变化可能复用旧快照 |
| adapter 无资产来源 |  `crates/wxml-lsp/src/backend_tsgo/mod.rs:106-112`、`:129-138` | `with_host` 只注入 `SessionOptions` |
| `js_check` 恒为 `Inherit` |  `crates/wxml-lsp/src/backend_tsgo/input.rs:198-204`（`js_check: JsCheckPolicy::Inherit` 在 `:202`） | 与 Node 默认 `Pragma` 不一致（`crates/wxml-lsp/src/backend_node/config.rs:71-101`、`:141`） |
| 只读取 `checkJs` |  `crates/wxml-lsp/src/backend_tsgo/input.rs:193-197` | 其余 32 个 option 从未传给 native |
| adapter 无资产模块 | `crates/wxml-lsp/src/` 全量 grep 无 `TypeAssets`/`type_assets` | 加载器必须新建 |

### 1.3 生产入口仍未选择 native

- `crates/wxml-lsp/src/main.rs:8-12`、`:66`、`:94-100`：`--stdio` 只构造 `NodeBackend`。
- `crates/wxml-lsp/src/lib.rs:46`：`pub(crate) mod backend_tsgo;`，没有 `run_stdio_native()`。
- `crates/wxml-lsp/src/backend_tsgo/mod.rs:59-62`：`#![allow(dead_code, reason = "...no server or config path selects it")]`。

---

## 2. 变更 1：adapter 如何发现/校验资产并构造 `TypeAssets`

### 2.1 输入与禁止项

**唯一输入**：P6-4 的 `VerifiedInstall`（尚未实现）。它至少必须携带（`docs/evidence/2026-11-18-tsgo-native-engine-discovery-plan.md:336-368`）：

- `package_root`（绝对、已 canonicalize、根自身不是 symlink）；
- 已通过 schema v2 校验的 `ManifestV2`；
- `engine`（`engine/lib/tsc`）；
- 可选的 `files[]` → `{sha256,size,mode}` 只读映射。

**禁止**（生产路径）：

- 不读 `PATH`、不执行 `which`、不读 `cwd`、不读 `WXML_*`/`TSGO_*` 环境变量来定位资产（P6-4 计划 `:198-207`）；
- 不读项目 `node_modules`、用户 home、网络、npm（P6-3 计划 §4.1 `:229-236`）；
- 不探测 `backends/typescript/dist/server.js`、`wxml-type-backend/server.js`；
- 不在资产失败时回退 legacy `noLib:true`（P6-3 计划 §8 `:377-382`）。

**唯一的测试旁路**：测试可以像 `crates/wxml-backend-tsgo/tests/type_assets.rs:359-387` 那样从 `TSGO_ASSET_ROOT`/`TSGO_TYPINGS_ROOT` 推导根；该逻辑不得进入 `wxml-lsp` 生产代码。

### 2.2 加载算法（顺序固定）

1. 校验 `package_root`：`symlink_metadata` 必须是目录且非 symlink；`canonicalize` 一次，之后所有拼接都必须以该 canonical root 为前缀（组件边界，见契约 `docs/reference/tsgo-native-contract.md:359`）。
2. 读 `<package_root>/manifest.json`：必须 schema v2；v1 直接拒绝（`crates/wxml-delivery/src/manifest_v2.rs:856-881`）。
3. 身份 pin（§2.4）：`manifest.package_version == wxml_lsp::version()`；`engine.version == PinnedEngine::pinned().version()`；`files["engine/lib/tsc"].sha256 == PinnedEngine::pinned().executable_sha256()`。
4. 取两个 `type_assets` 条目并校验冻结身份：
   - `typescript-standard-lib`：`root == "engine/lib"`、`include == ["lib*.d.ts"]`、`file_count == 116`、`version == 7.1.0-beta`（`crates/wxml-delivery/src/manifest_v2.rs:553-574`）；
   - `miniprogram-api-typings`：`root == "typings/miniprogram-api-typings"`、`include == ["**"]`、`version == 5.3.1`、`license == MIT`、`file_count` 等于 manifest 常量（`crates/wxml-delivery/src/manifest_v2.rs:575-594`）。
5. 用**共享**的 bundle 计算函数从 `manifest.files[]` 求匹配集合与文件数：`compute_type_asset_bundle`（`crates/wxml-delivery/src/type_assets.rs:196-236`，排序键为相对路径，行格式 `<sha256>  <relpath>\n`）。**不得**在 LSP 里复制一份 glob 实现。
6. 逐文件校验：
   - `symlink_metadata`：必须是普通文件、不是 symlink；`len() == entry.size`；
   - 按声明长度读取，UTF-8 解码失败即失败；
   - `AssetPath::new(<package_root>/<relpath>)`（`crates/wxml-backend-tsgo/src/input.rs:179-186`）；
   - `TypeAssetFile::new(path, text, entry.sha256, entry.size)`（`:231-268`）——它内部重算 size 与 SHA-256，篡改在构造函数处失败。
7. 只保留 `.d.ts`（`TypeAssetFile::new` 已强制，`:238-242`）。`package.json`、`LICENSE`、`README*`、`CHANGELOG.md`、`typings.json` **不能**也不应进入 `TypeAssets`。
8. `TypeAssets::new(<root>/engine/lib, <root>/typings, typings.version, typings.sha256, files)`（`:332-412`）。
9. 预算复核：`file_count <= 256`、`total_bytes <= 8 MiB`（`:359-390`）；实际应为 136 / 6,392,948。
10. 交叉校验：`<root>/engine/lib == executable.parent()`。引擎默认库来自可执行文件同目录的 `lib/`（`docs/reference/tsgo-native-contract.md:366`），所以资产根必须与引擎安装位置对齐，否则“能启动但库是别人的”。
11. 返回 `Arc<TypeAssets>`，在 `NativeSession::start` **之前**完成；失败即终止，不 spawn。

### 2.3 `TypeAssets` 精确构造与两个 digest 的区别

```text
// 绝对路径（不是仓库真实代码，仅构造说明）
TypeAssets::new(
    AssetPath::new(format!("{package_root}/engine/lib")),   // standard_lib_root
    AssetPath::new(format!("{package_root}/typings")),      // typings_root（package 的父目录）
    typings_asset.version,                                  // "5.3.1"
    typings_asset.sha256,                                   // manifest typeAssets[].sha256
    files,                                                  // Vec<TypeAssetFile>，136 条
)
```

关键区别（必须写进实现注释与评审）：

- `TypeAssets.package_digest` 是 **manifest 的 bundle digest**，覆盖 26 个 typings payload 文件；
- 挂载到 VFS 的只有 20 个 `.d.ts`；
- 因此**不能**对挂载文本重算 digest 再与 `package_digest` 比较——两者文件集不同（26 vs 20），必然不等。正确做法：
  - 逐文件校验 `files[]` 的 sha/size（第 6 步，`TypeAssetFile::new`）；
  - 整包 digest 由 manifest 校验链保证（`crates/wxml-delivery/src/manifest_v2.rs:534-546`、`crates/wxml-delivery/src/verify_v2.rs:118-130`）。
- `TypeAssets::new` 本身不校验 `package_digest` 覆盖内容，只校验 sha256 形状（`crates/wxml-backend-tsgo/src/input.rs:345-350`）。这是本 slice 的一个已知弱点，必须由 loader 的 manifest 校验补齐。

### 2.4 身份 pin（缺一不可）

| pin | 期望值 | 校验点 |
| --- | --- | --- |
| 引擎版本 | `7.1.0-beta` | `crates/wxml-backend-tsgo/src/config.rs:19-20`、`:262-280`；manifest `crates/wxml-delivery/src/manifest_v2.rs:41` |
| 引擎可执行 SHA-256 | `a183b195…faaa70` | `crates/wxml-backend-tsgo/src/config.rs:42-44`；`crates/wxml-backend-tsgo/src/native/session.rs:302-307` 启动时再校验 |
| 标准库根 | `engine/lib`（= `tsc` 的父目录） | 契约 `docs/reference/tsgo-native-contract.md:366`；loader 第 10 步 |
| 标准库文件数 | 116 | `crates/wxml-delivery/src/manifest_v2.rs:70-74` |
| typings 版本/许可/根 | `5.3.1` / `MIT` / `typings/miniprogram-api-typings` | `crates/wxml-delivery/src/manifest_v2.rs:76-84` |
| typings bundle digest | `typeAssets[].sha256` | `crates/wxml-delivery/src/manifest_v2.rs:157-158` |
| 挂载集预算 | 136 / 6,392,948 B（上限 256 / 8 MiB） | `crates/wxml-backend-tsgo/src/input.rs:97-106` |

### 2.5 失败关闭点

- loader 在 adapter 构造/启动阶段失败 ⇒ `run_stdio_native()` 在 LSP `initialize` 之前返回错误并非零退出（P6-4 计划 `:326-334`）。
- 请求阶段绝不允许 `Option<TypeAssets>` 回退到 legacy：生产 `RequestPlan` 持有 `Arc<TypeAssets>`，`None` 只允许来自测试 seam（§6 的 P6-3B/C）。

### 2.6 依赖选择（阻塞点）

`wxml-lsp` 当前不依赖 `wxml-delivery`（`crates/wxml-lsp/Cargo.toml:6-17`），而 `wxml-delivery` 的默认依赖含 `reqwest/tar/flate2/fs2`（`crates/wxml-delivery/Cargo.toml:9-17`）。可选路线：

- **推荐**：给 `wxml-delivery` 增加 `runtime-manifest` feature（只暴露 `manifest_v2` + `type_assets` 的纯函数），LSP 以 `default-features = false` 依赖；P6-4 计划 `:211-221` 明确要求不得把完整 delivery 拉进服务器。
- 备选：新建共享 `wxml-assets`/`wxml-manifest` crate。
- 临时备选（不推荐）：在 LSP 内写最小 manifest 读取器；必须用契约测试对齐 `ManifestV2::validate`，否则 schema 漂移风险高于收益。

**该决策未落地前，真实安装包 loader 视为 BLOCKED**（§7）。

---

## 3. 变更 2：compilerOptions 映射

### 3.1 数据来源（adapter 不读 tsconfig）

- 项目 tsconfig 由 `wxml-project`/`project_host` 读取、`extends` 展平、key 过滤后得到 `PreparedView.compiler_options`（`crates/wxml-lsp/src/project_host.rs:345-372`，字段在 `:353`；构建在 `:2901-2911`）。
- Node 生产路径发送同一份 map（`crates/wxml-lsp/src/backend_node/request.rs:139-150`）。
- **adapter 只消费 `view.compiler_options`**，不得再读 `tsconfig.json`、不得读 `extends` 文件、不得读 `node_modules`。

### 3.2 精确的 33 个 key（native 必须一对一）

来自 `COMPILER_OPTION_KEYS`（`crates/wxml-backend-tsgo/src/input.rs:58-92`），分组与校验规则：

- 21 个 boolean（`:1048-1070`）：`strict`, `strictNullChecks`, `strictFunctionTypes`, `strictBindCallApply`, `strictPropertyInitialization`, `noImplicitAny`, `noImplicitThis`, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, `strictBuiltinIteratorReturn`, `useUnknownInCatchVariables`, `allowJs`, `checkJs`, `esModuleInterop`, `allowSyntheticDefaultImports`, `resolveJsonModule`, `isolatedModules`, `verbatimModuleSyntax`, `forceConsistentCasingInFileNames`, `skipLibCheck`, `experimentalDecorators`
- 8 个 string（`:1072-1081`）：`target`, `module`, `moduleResolution`, `jsx`, `moduleDetection`, `baseUrl`, `rootDir`, `outDir`
- 3 个 string[]（`:1083`）：`lib`, `types`, `typeRoots`
- 1 个 object（`:1113-1124`）：`paths`（每个 value 必须是 string[]）

合计 33。native 只额外做三件事（`crates/wxml-backend-tsgo/src/native/vfs.rs:895-928`）：`noEmit:true`、`types` 确保含 `miniprogram-api-typings`、`typeRoots` 追加 `<package>/typings`。其余 key 原样输出（`:917-921`）。**不得**输出 `noLib`、不得强制 `strict/target/module/moduleResolution/skipLibCheck/lib` 默认值。

### 3.3 映射规则（adapter 侧）

1. 把 `view.compiler_options`（`serde_json::Map`）转 `BTreeMap<String, Value>`；先按 `COMPILER_OPTION_KEYS` 过滤：
   - 未知 key：丢弃并 `merge_reason` 记录 `Partial`（不得整请求失败）；
   - 错形状：同上（`project_host` 已经过滤，这里是防御性第二道）。
2. `types` pin：保留项目顺序，缺失时把 `miniprogram-api-typings` 追加到末尾（对齐 `crates/wxml-lsp/src/project_host.rs:3316-3352` 与 `backends/typescript/src/compiler.ts:148-165`）；VFS 会幂等再追加一次（`crates/wxml-backend-tsgo/src/native/vfs.rs:903-908`）。
3. `typeRoots` 追加语义：项目根在前，`<package>/typings` 在最后（`crates/wxml-backend-tsgo/src/native/vfs.rs:910-915`）。项目没有 `typeRoots` 时，VFS 结果就是 `["<package>/typings"]`，等价于 Node “backend `node_modules` 提供 pinned typings”的效果（`backends/typescript/src/analysis.ts:274-284`）。
4. `baseUrl`：必须是项目根内的绝对规范路径（`project_host` 已在 `:3280-3310` 处理绝对/`..` 拒绝）；adapter 若发现不在项目根内，丢弃并 `Partial`。
5. `paths`：原样转发；其解析依赖 `baseUrl`，因此 `baseUrl` 被丢弃时 `paths` 也必须一并丢弃并 `Partial`（否则会解析到宿主 cwd）。
6. `checkJs` + `js_check`：
   - `checkJs` 已在 `crates/wxml-lsp/src/backend_tsgo/input.rs:193-197` 读取并传给 shadow；
   - `WXML_TEMPLATE_CHECK` 的 `off|pragma|inherit` 必须在**配置层**映射为 `JsCheckPolicy`（`crates/wxml-backend-tsgo/src/shadow/layout.rs:109-116`），不能在 adapter 里读环境变量；当前恒 `Inherit`（`crates/wxml-lsp/src/backend_tsgo/input.rs:202`）。
7. 不输出 `noLib`（VFS 对 environment 输入已不输出，`crates/wxml-backend-tsgo/src/native/vfs.rs:1436` 断言其不存在）。

### 3.4 项目 `typeRoots` 在快照外：显式 Partial，绝不读盘

现状：`project_host::resolve_type_roots` 用 `candidate.is_dir()` 做宿主磁盘探测（`crates/wxml-lsp/src/project_host.rs:3213-3256`，判定在 `:3244-3249`）。这会把“真实存在但不在快照里的目录”保留下来，native 无法表达。

adapter 的判定算法（纯内存）：

1. 允许根 = 项目根 + 已挂载资产根（`engine/lib`、`typings`）。
2. 项目 `typeRoots` 条目：转成规范绝对路径；若不在项目根内 ⇒ 丢弃 + `Partial("typeRoots outside the virtual snapshot")`。
3. 若在项目根内，但快照里没有任何挂载文件以该目录为前缀（组件边界）⇒ 丢弃 + `Partial`。
4. 被丢弃的条目不得写入 `CompilerOptions`；`types` 中依赖它而无法解析的条目保留（引擎会给出 2688/2304 一类诊断），但结果状态必须是 `Partial`，不能是 `Supported`。
5. `RequestPlan.degradation` 是既有通道（`crates/wxml-lsp/src/backend_tsgo/input.rs:207-242`），各 feature 已把它合并成 `Partial`（`crates/wxml-lsp/src/backend_tsgo/completion.rs:129-137`、`crates/wxml-lsp/src/backend_tsgo/hover.rs:110-120`、`crates/wxml-lsp/src/backend_tsgo/diagnostics.rs:136-144`、`crates/wxml-lsp/src/backend_tsgo/definition.rs:119-127`）。

### 3.5 与 Node 的已知语义差异（必须记录，不得静默）

- Node 在项目**没有** `typeRoots` 时保持未设置（`backends/typescript/src/compiler.ts:154-162`），把解析交给 TypeScript 默认规则；native 无条件写 `typeRoots:["<package>/typings"]`，会**替换**默认解析。
- 影响：使用项目 `@types` 包（`types:["node"]` 等）的项目，native 可能报 2688。这是“显式化 pinned typings”的代价，不是本 slice 引入的 bug。
- 处理：作为已知差异记录；在 `TS-COMPILER-0132` 的 native 版本用例里显式断言“pinned typings 仍可解析”，同时把 `@types` 项目行为标为待裁决（`docs/plans/2026-11-19-tsgo-remaining-ledger-plan.md:587-601` 的合同变更类）。

---

## 4. 变更 3：会话复用身份

### 4.1 现有身份

`RequestIdentity`（`crates/wxml-lsp/src/backend_tsgo/input.rs:73-97`）已含 project/view/source revisions、script path/revision/bytes、`view_id`、`project_root`、`fragment_count`；`ensure_open` 在身份完全相等时直接复用（`crates/wxml-lsp/src/backend_tsgo/mod.rs:221-225`）。

### 4.2 新增字段（两项，足够覆盖）

```
// crates/wxml-lsp/src/backend_tsgo/input.rs
pub(crate) options_digest: String,          // CompilerOptions::digest()
pub(crate) asset_identity: Option<String>,  // None = legacy/test；Some = 生产
```

`asset_identity` 建议用**不新增哈希依赖**的组合键：

```
format!("{}:{}:{}:{}:{}",
    package_digest, file_count, total_bytes,
    standard_lib_root, typings_root)
```

理由：`TypeAssets` 已有 `package_digest/file_count/total_bytes/roots`（`crates/wxml-backend-tsgo/src/input.rs:432-466`），且 `package_digest` 已是内容摘要；无需为 6.1 MiB 再做一次 SHA-256，也避免把 `sha2` 加进 `wxml-lsp` 生产依赖。`project_revision` 已经在身份里（`:76`）。

### 4.3 决策表（`ensure_open` 扩展）

| 变化 | 动作 | 依据 |
| --- | --- | --- |
| identity 完全相等 | 复用，不发请求 | 现有 `crates/wxml-lsp/src/backend_tsgo/mod.rs:221-225` |
| 仅源码/视图 revision 变化 | `update_project` | 现有 `crates/wxml-lsp/src/backend_tsgo/mod.rs:232-249` |
| 仅 `options_digest` 变化（tsconfig 编辑） | `update_project`（同一 project/root，tsconfig 路径不变） | `crates/wxml-backend-tsgo/src/native/session.rs:699-703` 在同一次 update 里 `closeProject/openProject` |
| `asset_identity` 变化（换安装包/重新加载） | **shutdown + `start_and_open`** | 不同资产代的默认库/typings 不应在同一引擎进程内混用；资产变化是低频事件 |
| project 或 root 变化 | shutdown + `start_and_open` | 现有 `crates/wxml-lsp/src/backend_tsgo/mod.rs:232-234`、`:251-256` |
| update 失败 / fatal / epoch 失效 | shutdown + 重启 | 现有 `crates/wxml-lsp/src/backend_tsgo/mod.rs:187-208`、`:250-256` |

实现要点：`OpenProject`（`crates/wxml-lsp/src/backend_tsgo/mod.rs:485-499`）增加 `asset_identity: Option<String>`；`same_project` 条件扩展为“project/root 相同 **且** asset identity 相同”。

### 4.4 6.1 MiB 成本：现状、计量点、优化边界

- open：`open_project` → `VirtualFileSystem::from_analysis_input` → `snapshot_files()`（`crates/wxml-backend-tsgo/src/native/vfs.rs:307-313`）把每个文件 `to_owned()` 成 `String`，包含 136 个资产；`SnapshotFileSystem::full` 持有 `BTreeMap<String,String>`（`crates/wxml-backend-tsgo/src/native/wire.rs:51-59`）；`create_changes`（`crates/wxml-backend-tsgo/src/native/session.rs:1672-1680`）把它放进 `createSnapshot`。
- update：`update_project` 同样构造 full FS（`crates/wxml-backend-tsgo/src/native/session.rs:703`），每次编辑都重发全部资产。
- 预检：`request_size_upper_bound()`（`crates/wxml-backend-tsgo/src/native/vfs.rs:470-479`）计入资产路径、文本、URI、JSON 转义；上限 32 MiB（`crates/wxml-backend-tsgo/src/config.rs:66`）。
- 结论：**本设计无法避免重发**；契约要求 full（`docs/reference/tsgo-native-contract.md:301`、`:396-398`），不得为性能退回 layer/回调。
- P6-3 必须做的：在 adapter 记录 open/update 的 `request_size_upper_bound`、耗时、RSS（可用现有 seam 包一层或直接打点），并在门禁里给出 p95；不复用旧的“只测正确性”。
- 允许的后续优化（全部 BLOCKED，需单独评审）：`SnapshotFileSystem` 改 `Arc<str>` 避免克隆（native owner）；layer 更新 + 预挂资产（需契约修订 + AC-004 关闭；P6-3 计划 §3.6 `:215-224`、§5.4 `:309-316`）。

---

## 5. 变更 4：失败/状态映射与测试

### 5.1 映射表（目标行为）

| 场景 | 触发点 | 最终状态 | 说明 |
| --- | --- | --- | --- |
| 安装树缺 manifest/asset | loader 第 2/4 步 | `BackendError::Unavailable`，不 spawn | 启动期失败，先于 LSP initialize |
| 单文件 size/hash 不符 | `TypeAssetFile::new`（`crates/wxml-backend-tsgo/src/input.rs:250-260`） | `TypeAssets` 构造失败 → `Unavailable` | 错误信息不含宿主绝对路径（loader 负责 redact） |
| 必需声明缺失（`lib.d.ts`/`index.d.ts`/`types/index.d.ts`） | `TypeAssets::new`（`crates/wxml-backend-tsgo/src/input.rs:392-400`） | `Unavailable` | 不降级为 noLib |
| VFS 资产越界/重复/撞项目文件 | `crates/wxml-backend-tsgo/src/native/vfs.rs:845-869` | `Unavailable(MissingAsset)` / `Protocol` / `InvalidParams`（`crates/wxml-backend-tsgo/src/native/session.rs:2181-2187`） | 引擎启动后也不返回干净结果 |
| 项目 `typeRoots` 在快照外 | adapter §3.4 | 结果 `Partial` + reason | 不读盘，不发宿主路径 |
| legacy 无资产输入 | `crates/wxml-lsp/src/backend_tsgo/input.rs:120-128` | 保持 `noLib:true` | 仅测试/probe；生产构造器不得产生 |
| 生产 plan 缺资产 | P6-3C | `Unavailable("native type assets are not loaded")` 且 session `starts == 0` | fail-closed 的最后一道 |
| `WXML_REQUIRE_REAL_ENGINE=1` 但缺引擎/资产根 | 测试 gate | 硬失败（panic），不得 skip | 现有模式见 `crates/wxml-backend-tsgo/tests/type_assets.rs:389-433` |

### 5.2 测试矩阵（精确文件与目标）

| # | 场景 | 现有覆盖 | 需要新增（精确文件/函数） |
| --- | --- | --- | --- |
| T1 | 33 key 接受/拒绝、digest 稳定 |  `crates/wxml-backend-tsgo/src/input.rs:1389-1421`；`crates/wxml-backend-tsgo/tests/type_assets.rs:99-124` | adapter 转发：`crates/wxml-lsp/src/backend_tsgo/tests.rs::production_plan_forwards_all_33_compiler_options` |
| T2 | 资产挂载、目录边界、宿主文件不可读 |  `crates/wxml-backend-tsgo/src/native/vfs.rs:1372-1400`、`:1518-1560`；`crates/wxml-backend-tsgo/tests/type_assets.rs:257-289` | adapter 断言 `snapshot_files`/`asset_paths`：`crates/wxml-lsp/src/backend_tsgo/tests.rs::production_plan_mounts_the_pinned_assets` |
| T3 | 缺失资产 fail-closed |  `crates/wxml-backend-tsgo/src/native/session.rs:2199-2224`；`crates/wxml-backend-tsgo/tests/type_assets.rs:608-622` | `crates/wxml-lsp/src/backend_tsgo/tests.rs::missing_assets_fail_closed_before_spawn`（断言 `FakeLog.starts == 0`，`FakeLog` 在 `crates/wxml-lsp/src/backend_tsgo/tests.rs:340-357`） |
| T4 | hash mismatch |  `crates/wxml-backend-tsgo/src/input.rs:1489-1558` | loader 级：`crates/wxml-lsp/src/backend_tsgo/assets.rs` 内 `#[cfg(test)] mod tests::hash_mismatch_is_rejected_before_the_session_starts` |
| T5 | 外部 `typeRoots` → Partial |  `crates/wxml-lsp/src/project_host.rs:3639-3665`（Node 侧） | adapter：`crates/wxml-lsp/src/backend_tsgo/tests.rs::external_type_roots_are_dropped_and_reported_partial`；vfs：扩展 `crates/wxml-backend-tsgo/src/native/vfs.rs:1403-1453` 增加项目 `typeRoots` 前置断言 |
| T6 | legacy `noLib` 回归 |  `crates/wxml-backend-tsgo/src/native/vfs.rs:1337-1351`、`:1456-1470` | adapter：`crates/wxml-lsp/src/backend_tsgo/tests.rs::legacy_adapter_still_opens_asset_free_input`（`has_type_environment() == false`） |
| T7 | 真实引擎 `wx`/`Page`/`Component`/`getApp` |  `crates/wxml-backend-tsgo/tests/type_assets.rs:514-600` | adapter 级真实引擎：`crates/wxml-lsp/src/backend_tsgo/tests.rs::optional_real_engine_type_environment_smoke`（`TSGO_BIN` + `TSGO_ASSET_ROOT`） |
| T8 | required-mode 硬失败 |  `crates/wxml-backend-tsgo/tests/type_assets.rs:389-433`；`crates/wxml-backend-tsgo/tests/real_engine.rs:113-130` | adapter 变体：同文件，缺 `TSGO_ASSET_ROOT` 时 `WXML_REQUIRE_REAL_ENGINE=1` 下 panic |
| T9 | 会话身份：options/asset 变化触发 update/reopen |  `crates/wxml-lsp/src/backend_tsgo/tests.rs` 现有 update 用例（`FakeLog.updates`，`crates/wxml-lsp/src/backend_tsgo/tests.rs:340-357`） | `crates/wxml-lsp/src/backend_tsgo/tests.rs::options_change_updates_the_snapshot_and_asset_change_reopens` |
| T10 | ledger `TS-COMPILER-0132` | 无（`crates/wxml-lsp/tests/native_parity/project_overlay.rs` 仍是 13 行 blocked 骨架） | `crates/wxml-lsp/tests/native_parity/project_overlay.rs::case_compiler_0132_project_typeroots_still_load_the_pinned_mini_program_typings`（ledger 目标名见 `docs/plans/tsgo-test-ledger.json` 的 `case_id=TS-COMPILER-0132`） |

### 5.3 真实引擎断言（不可放宽）

- `wx.getSystemInfoSync` 的接收者 → `Wx`，返回值 → `SystemInfo`；
- `Page`/`Component` → `Constructor`；`Page({data:{count:1}})` 的 `this.data.count` → `number`；
- `getApp` → `GetApp`；
- `wx.` 补全包含 `getSystemInfoSync`（不要硬编码 505）；
- 诊断不得出现 2304；缺字段是 2339；赋值错误是 2322；
- 失败模式：缺一个 typings `.d.ts` → `Unavailable`，且没有 spawn 后的“干净”诊断。

---

## 6. 有序变更清单（P6-3A..P6-3H）

> 全部是计划，未实施。顺序按依赖排列，每项独立评审/提交；不得越过 P6-4/交付的阻塞项。

| # | 变更 | 精确锚点 | 依赖 | 验收门禁 | 回滚 |
| --- | --- | --- | --- | --- | --- |
| P6-3A | 新建 loader：`package_root + ManifestV2 → Arc<TypeAssets>`；含 symlink/size/hash/UTF-8/预算/身份 pin；测试用显式根 | 新增 `crates/wxml-lsp/src/backend_tsgo/assets.rs`；参考 `crates/wxml-delivery/src/type_assets.rs:196-236`、`crates/wxml-delivery/src/manifest_v2.rs:485-594` | §2.6 依赖决策（可先用 fake manifest） | `cargo test -p wxml-lsp --lib backend_tsgo::assets`；缺文件/篡改/越界/symlink/超预算全拒绝 | 纯新增模块，不改变任何默认路径 |
| P6-3B | adapter 注入环境：新增 `with_environment(host, options, assets)`；`new` 保留为测试 legacy seam；字段存 `Arc<TypeAssets>` |  `crates/wxml-lsp/src/backend_tsgo/mod.rs:106-112`、`:129-154` | P6-3A | fake 测试可构造生产 adapter；legacy 构造器行为不变 | 保留 `new`，revert 后回到现状 |
| P6-3C | `RequestPlan`/`input()` 分支：生产走 `with_environment`，缺资产 `Unavailable`；legacy 分支仅测试 |  `crates/wxml-lsp/src/backend_tsgo/input.rs:100-128` | P6-3B | `cargo test -p wxml-lsp --lib backend_tsgo`；`missing_assets_fail_closed_before_spawn` | 单函数回退到 `AnalysisInput::new` 即恢复旧行为 |
| P6-3D | options 映射：33 key 过滤 + `CompilerOptions::new` + `types` pin + `typeRoots` 快照内过滤 + `baseUrl/paths` 连带 + `js_check` 注入 |  `crates/wxml-lsp/src/backend_tsgo/input.rs:193-205`、`:207-242`、`:267-306`；`crates/wxml-lsp/src/backend_node/config.rs:93-101`（`JsCheckMode::from_value`；native 需在配置层建立等价映射，当前无） | P6-3C | `production_plan_forwards_all_33_compiler_options`；`external_type_roots_are_dropped_and_reported_partial` | 保留 legacy 空 options 分支 |
| P6-3E | 身份扩展：`options_digest`/`asset_identity`；`OpenProject` 存 asset 身份；资产变化 reopen |  `crates/wxml-lsp/src/backend_tsgo/input.rs:73-97`、`:313-325`；`crates/wxml-lsp/src/backend_tsgo/mod.rs:215-257`、`:485-499` | P6-3C/D | `options_change_updates_the_snapshot_and_asset_change_reopens`；现有 epoch/update 用例全绿 | 字段可删，不影响 backend |
| P6-3F | 成本计量：记录 open/update 的十六进制/字节/耗时 |  `crates/wxml-backend-tsgo/src/native/vfs.rs:470-479`（bound）；adapter `crates/wxml-lsp/src/backend_tsgo/mod.rs:272-293` | P6-3B | 门禁输出 `request_size_upper_bound`、elapsed、RSS | 仅日志/指标 |
| P6-3G | 测试补齐：T3..T10；真实引擎 opt-in；`WXML_REQUIRE_REAL_ENGINE` 硬失败 |  `crates/wxml-lsp/src/backend_tsgo/tests.rs`；`crates/wxml-lsp/tests/native_parity/project_overlay.rs` | P6-3A..F | 见 §5.2；`cargo test -p wxml-lsp --lib backend_tsgo`；真实引擎命令见 §6.1 | 测试可单独 revert |
| P6-3H | 文档/账本映射：记录 136/6.1 MiB、身份 pin、BLOCKED 项；**不得**把 `TS-COMPILER-0132` 标 green | 本文；`docs/plans/tsgo-test-ledger.json`（由 parity owner 更新） | P6-3G | 文档明确“P6-3 未完成” | 文档 revert |

### 6.1 命令级门禁

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings

# 数据层（无需引擎）
cargo test -p wxml-backend-tsgo --lib native::vfs
cargo test -p wxml-backend-tsgo --test type_assets

# adapter fake（无需引擎）
cargo test -p wxml-lsp --lib backend_tsgo

# 真实引擎（固定 7.1.0-beta + 显式资产根；required 模式）
TSGO_BIN=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc \
TSGO_ASSET_ROOT=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package \
WXML_REQUIRE_REAL_ENGINE=1 \
cargo test -p wxml-lsp --lib backend_tsgo::tests::optional_real_engine_type_environment_smoke -- --nocapture
```

判定：P6-3B/C 合入后，任何生产构造路径都不得再出现 `has_type_environment() == false`；P6-3D 合入后，外部 `typeRoots` 必须是 `Partial` 且不读盘；P6-3E 合入后，资产变更必须 reopen。

---

## 7. 仍 BLOCKED（不属于 P6-3 adapter 可解范围）

| 阻塞项 | 事实/影响 | 证据锚点 | 解除条件 |
| --- | --- | --- | --- |
| 交付 typings 26 vs 23 | `is_allowed_native_path` 只允许 23（`index.d.ts`、`LICENSE`、`README.md`、`CHANGELOG.md`、`types/**`）；`native_typings` 要求 26 |  `crates/wxml-delivery/src/package.rs:1097-1104`、`:1326-1334`；`crates/wxml-delivery/src/manifest_v2.rs:84`；`docs/evidence/2026-11-18-tsgo-f3-asset-lock-plan.md:58-73` | delivery owner 裁决并冻结一个值；P6-3 不自行改常量/allowlist |
| 真实包 loader 端到端 | 无法同时满足“26 文件 bundle digest”与“只挂 20 个 `.d.ts`”的重算校验（§2.3） | 同上；`crates/wxml-delivery/src/verify_v2.rs:118-130` | 交付统一后，用真实包跑 T3/T4/T7 |
| AC-004 未保存更新旧诊断 | 探针显示 `updateSnapshot` 后旧文本诊断仍保留 |  `docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:309-316`；ledger `TS-OVERLAY-0055` | native/F1 owner 定位并修复；在该用例上 `#[ignore]`/BLOCKED |
| TS 版本偏差 6.0.3 vs 7.1.0-beta | 不能宣称 AC 与 Node 逐字节等价 |  `docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:18`、`:390` | 差分矩阵 + 差异裁决 |
| native LSP 引擎发现（P6-4a） | `wxml-lsp` 没有 `VerifiedInstall`/package root/manifest 访问；生产 loader 无输入 |  `docs/evidence/2026-11-18-tsgo-native-engine-discovery-plan.md:8-12`、`:198-213`、`:312-318`；`crates/wxml-lsp/Cargo.toml:6-17` | P6-4a 落地并暴露 `package_root + manifest` |
| `wxml-delivery` 依赖形态 | 直接把 delivery 拉进 LSP 会带 `reqwest/tar/flate2/fs2` |  `crates/wxml-delivery/Cargo.toml:9-17`；P6-4 计划 `:211-221` | 新增 `runtime-manifest` feature 或共享 crate |
| 默认后端切换 | `--stdio` 仍只构造 Node；`backend_tsgo` 是 `pub(crate)` |  `crates/wxml-lsp/src/main.rs:66`、`:94-100`；`crates/wxml-lsp/src/lib.rs:46`；P6-4 计划 `:330-334` | P6-8；不属 P6-3 |
| layer/资产增量更新 | 契约要求 full；旧诊断问题未解 |  `docs/reference/tsgo-native-contract.md:301`、`:396-398`；P6-3 计划 `:215-224` | 契约修订 + AC-004 关闭后单独验收 |

---

## 8. 非目标与不变量（本次审计不改变）

- 不切换默认后端；不新增 `--backend native`；不删 Node。
- 不修改 `crates/wxml-delivery/src/package.rs`、`native_install.rs`、rollback/active 符号文件。
- 不在生产路径读 `PATH`/`cwd`/环境变量来定位引擎或资产；测试的 `TSGO_*` 只存在于 `#[cfg(test)]`/`tests/`。
- 不把“缺失资产”降级成 `noLib:true` 或干净诊断；不允许 Node 回退。
- 不伪造 `wx`/`Page`/`Component`/`getApp` 的声明；必须来自通过 manifest 校验的资产。
- 不把 P6-3 的完成等同于 G5/P6/G6 或 AC 全绿；本审计不产生任何 green 声明。
