# tsgo typings 26 vs 23 裁决建议（只读审计，非合同变更）

- 审计对象：worktree /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration，HEAD `f6f175f`。
- 审计日期：2026-11-19（文件名按任务指定为 `2026-11-18`）。
- 工作树状态：`crates/wxml-backend-tsgo/src/{input,native/session,native/vfs}.rs`、`crates/wxml-lsp/src/backend_tsgo/{mod.rs,engine.rs,sandbox.rs}`、`crates/wxml-lsp/src/{lib,main}.rs`、`crates/wxml-lsp/tests/cli.rs` 有其它会话的未提交/未跟踪修改；`crates/wxml-delivery` 无未提交修改。本文不把这些并发修改当作交付侧事实。
- 写入边界：只新增本文件；未修改生产代码、测试、Cargo、契约、打包器、安装器、回滚、backend/server/Node；未提交。
- 执行边界：未下载、未执行任何二进制（含 tsgo/Node/npm），未联网；只做了只读文件枚举、`sha256sum`、`stat`、`grep` 与本地哈希重算。
- 状态：**F3-03 仍为 BLOCKED**。本文给出裁决建议和可执行改动清单，不关闭任何 F3 ID，不删除 Node，不切换默认后端，也不改变任何冻结常量。

---

## 0. 结论摘要

1. 被 allowlist 排除的 3 个文件（`package.json`、`README-en.md`、`typings.json`）**没有一个是运行时声明**，也不是 MIT 许可文本；它们分别只是 npm 包元数据、英文翻译、旧 `typings` 工具元数据。真正承载许可的是保留在 23 文件集合中的 `LICENSE`。
2. 推荐 **Option A：交付 payload 固定为 23 文件**。它精确等于现有打包器 allowlist，不放松 `forbidden_runtime_path`，不把 `package.json` 带进无 Node 安装树。
3. 推荐把 A 落地为“可审计的 A”：`assets.lock.json` 同时记录 `upstream.fileCount=26`（含 3 个被排除文件的逐文件哈希与原因）和 `payload.fileCount=23`，打包器/安装器只消费 payload；这不是第三个安全模型，只是 A 的审计形态。
4. **Option B（保持 26）严格劣于 A**：需要同时改契约、负向扫描、allowlist 和负向测试，并把 Node 包元数据（含 npm scripts/devDependencies）重新带入 payload；运行时、许可、provenance 三项均无收益。
5. 仅把 `MINIPROGRAM_TYPINGS_FILE_COUNT` 从 26 改成 23 **不能**关闭 F3-03：manifest 的 `files[]` 哈希仍由打包输入现算，没有独立锚点。23/26 只解决“内部自相矛盾”，不解决“打包输入可被替换”。
6. 复核提醒：26/23 摘要只有在**字节序排序**（Rust `str::cmp` / `LC_ALL=C`）下才可复现。普通 UTF-8 locale 的 `sort` 会得到 `89d11a…`/`02ddcc…` 这类错误值；测试与脚本不得用 locale 排序生成 bundle。

---

## 1. 事实基线（本次独立复核）

### 1.1 文件集与摘要

对本地 npm 安装副本 `backends/typescript/node_modules/miniprogram-api-typings`（5.3.1）做只读枚举，结果与 P6-3/F3-03 记录一致：

| 口径 | 文件数 | 字节 | bundle 摘要（manifest 算法） |
| --- | ---: | ---: | --- |
| 上游全部文件 | 26 | 2,552,052 | `2141aac47206441f0434a7b519e49da1e8756d1bcdb6c17809a7f5e188a34c42` |
| 打包器 allowlist（= 本建议 payload） | 23 | 2,548,075 | `ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b` |
| 运行时 `.d.ts` 闭包（VFS 实际挂载） | 20 | 2,522,752 | `821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9` |

差异恰好是 3 个文件、3,977 B：`package.json` 1,077 B + `README-en.md` 2,839 B + `typings.json` 61 B。23 文件集合的构成为：`index.d.ts` 44 B + `LICENSE` 1,057 B + `README.md` 2,730 B + `CHANGELOG.md` 21,536 B + `types/**` 19 文件 2,522,708 B。

摘要算法在 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:196 )：按完整路径排序后拼接 `"{sha256}  {relative}\n"` 再 SHA-256。排序使用 `str` 比较，即 UTF-8 字节序，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:223 )。本次用 `LC_ALL=C sort` 重算，三个摘要与既有记录逐字节一致；用默认 locale 的 `sort` 会得到不同摘要，这是审计时必须避免的假差异。

### 1.2 三个互不相同的摘要空间（不得混用）

| 摘要 | 值 | 语义/消费者 |
| --- | --- | --- |
| 26 文件上游包摘要 | `2141aac…` | 上游解包树身份；只应在 `assets.lock.json` 的 `upstream.treeDigest`/`upstream.files[]` 中出现 |
| 23 文件 payload 摘要 | `ba3eb83…` | 交付 manifest `typeAssets[miniprogram].sha256` / lock `payload.bundles[].sha256` |
| 20 文件运行时闭包摘要 | `821c93…` | 原生后端 `TypeAssetsPin`；只证明 VFS 挂载的 `.d.ts` 与本地 fixture 一致 |

20 文件摘要在 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:148 ) 固定，代码注释明确它不是交付 manifest 的 26/23 包摘要；`TypeAssets::package_digest()` 也再次声明“这是 20 文件运行时闭包，不是 26 文件包摘要”，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:702 )。因此 **Option A 不改变任何运行时摘要常量**。

### 1.3 当前代码的强制点（冲突是真实且必现的）

| 强制点 | 位置 | 现状 |
| --- | --- | --- |
| manifest 冻结文件数 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:84 ) | `MINIPROGRAM_TYPINGS_FILE_COUNT = 26` |
| manifest 必需 typings 身份 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:583 ) | 要求 `include == ["**"]` 且 `file_count == 26`、`license_files == [typings/.../LICENSE]` |
| 打包器 allowlist | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1074 ) | 只允许 `index.d.ts`、`LICENSE`、`README.md`、`CHANGELOG.md`、`types/**` = 23 |
| 打包器输入负向扫描 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1019 ) | `forbidden_runtime_path` 先拒绝；`is_allowed_native_path` 再不接受即拒绝（不是静默丢弃） |
| `package.json` 运行期禁令 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:42 ) | 仅 `engine/package.json` 可豁免；`typings/.../package.json` 必被拒绝 |
| 打包器计数检查 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1326 ) | 输入实际匹配数必须等于 26 |
| 安装期 bundle 复算 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:120 ) | 从 manifest `files[]` 重新计算，防止声明与实际文件集不一致 |
| 运行期负向扫描 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:110 ) | 安装树中出现 `package.json` 等即拒绝 |

推论：用真实 typings 目录打包时，`package.json` 先被负向扫描拒绝；`README-en.md`/`typings.json` 被 allowlist 拒绝；只放行其中一两个则计数仍不等于 26。因此**当前 v2 打包器无法用真实 5.3.1 目录产出一个自洽包**，这是内部矛盾，不是统计口径差异。

---

## 2. 逐文件用途（26 文件）

### 2.1 允许并进入 payload 的 23 个文件

| 文件/组 | 数量 | 字节 | 用途 | 运行时? | 许可/provenance? |
| --- | ---: | ---: | --- | --- | --- |
| `index.d.ts` | 1 | 44 | 包入口；唯一内容 `/// <reference path="./types/index.d.ts" />` | 是（类型入口） | 否 |
| `types/index.d.ts` | 1 | 41 | 二级入口；引用 `./wx/index.d.ts` | 是 | 否 |
| `types/wx/index.d.ts` | 1 | 6,857 | `WechatMiniprogram` 命名空间与 11 条 `/// <reference>`；文件头含 Tencent 2025 MIT 全文 | 是 | 该文件自带许可文本 |
| `types/wx/lib.wx.*.d.ts` | 17 | 2,515,810 | 全部 API/组件/Page/Component/云/canvas/xr-frame 等声明；其中 `lib.wx.api.d.ts` 1,647,088 B | 是 | 部分文件自带 MIT 头 |
| `LICENSE` | 1 | 1,057 | MIT 许可全文（`Copyright (c) 2019`，未写持有人名） | 否 | **是：唯一完整许可文本** |
| `README.md` | 1 | 2,730 | 中文 README/安装说明 | 否 | 否（仅文档） |
| `CHANGELOG.md` | 1 | 21,536 | 变更日志 | 否 | 否（仅文档） |

补充事实：20 个 `.d.ts` 中只有 12 个包含 MIT 许可全文，11 个含 `Copyright (c) 2025 Tencent`；`glass-easel.*`、`lib.wx.skyline.d.ts` 以及两个纯 `/// <reference>` 入口没有许可头。因此 **`LICENSE` 不是“可有可无的 README 类文件”，而是 MIT 合规的必要文件**；但它已经在 23 集合内，Option A 保留它。

### 2.2 被排除的 3 个文件

| 文件 | 字节 | SHA-256 | 精确内容/用途 | 运行时需要? | 许可/provenance 需要? |
| --- | ---: | --- | --- | --- | --- |
| `package.json` | 1,077 | `7dc13675e2fed5daeb67ab90fe3a26d458883e7b6d1ce542d2b8048c40b6d1fc` | npm 元数据：`name`、`version=5.3.1`、`main/types=./index.d.ts`、`scripts{test,tsd,eslint}`、`repository`、`license=MIT`、`devDependencies`（含 `typescript ^5.2.2 <5.4.0`）、`files[]` | 否 | 否：`license` 字段不是许可文本；版本/许可已由 manifest 常量与 lock 记录 |
| `README-en.md` | 2,839 | `0b239cf2de187501d8dbed123992ee91a026f431dd6892483af4ebfcebbc8de8` | `README.md` 的英文翻译；含 `npm install miniprogram-api-typings` 与贡献者列表 | 否 | 否：翻译文本，不含完整许可 |
| `typings.json` | 61 | `0f11c8a7d974fcd773983554a9cc1fc69ce8adc71057d5613c6668e0a736653e` | 旧 `typings` 工具元数据：`{"name":"miniprogram-api-typings","main":"index.d.ts"}` | 否 | 否：旧工具遗留，TypeScript 5/7 不读取 |

引用关系核查：所有 `.d.ts` 中不存在对 `package.json`、`typings.json`、`README*`、`CHANGELOG.md` 的 `/// <reference>`/`import`/`require`。入口闭包是 `index.d.ts → types/index.d.ts → types/wx/index.d.ts → 17 个 lib.wx.*`，共 20 个 `.d.ts`。`lib.wx.api.d.ts` 里出现的 `app.json` 只是文档注释/字符串说明，不是文件引用。

---

## 3. 影响分析：运行时、安全、许可、provenance

### 3.1 运行时：3 个文件都进不了 VFS

- 原生后端的资产类型只接受 `.d.ts`：`TypeAssetFile::new` 对非 `.d.ts` 直接返回 `UnsupportedFile`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:306 )。
- VFS 只挂载 `TypeAssets::files()`，并在挂载边界再次检查根目录与重复，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:888 )。
- 期望的挂载集合是 20 个 `.d.ts`、2,522,752 B，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:95 )；`source_paths()` 明确排除资产与 `tsconfig.json`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:322 )。
- 因此：即使 Option A 的安装树里没有 `package.json`，`types:["miniprogram-api-typings"]` 的解析仍走 `<typings_root>/miniprogram-api-typings/index.d.ts`。P6-3 探针已记录“缺 `package.json` 仍回退到 `index.d.ts`”，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:98 )；该探针是本次审计未重跑的外部证据，必须由真实引擎门禁再证明一次（见 §7）。

### 3.2 安全：把 3 个文件带进 payload 的后果

| 文件 | 直接执行风险 | 解析/工具链触发 | 结论 |
| --- | --- | --- | --- |
| `package.json` | 无 install 生命周期脚本（无 pre/postinstall），但 `scripts.test/tsd/eslint` 可被 `npm test` 触发，`devDependencies` 会在 `npm install` 时联网拉包 | 当前 Rust 代码只解析 `engine/package.json`（( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1106 )），不会解析 typings 的；未来若 VFS/loader 为 `types` 解析读取它，会新增 `types`/`typings`/`main`/`exports` 重定向面 | 无收益、新增 package-manager 与解析面；应继续禁止 |
| `README-en.md` | 无（纯 Markdown） | 含 `npm install` 与外部徽章链接；若被渲染可能产生外部请求/诱导用户安装 Node 包 | 惰性文本，风险低，但无任何交付收益 |
| `typings.json` | 无 | 旧 `typings` 工具可能读取 `main`；TS 不读 | 惰性 JSON，无收益 |

`forbidden_runtime_path` 是词法防线，不是内容扫描：它按路径/扩展名禁止 `node_modules/`、JS/CJS/MJS、Node 锁文件和非 `engine/package.json` 的 `package.json`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:24 )。为 typings 单独开 `package.json` 例外，会把一个“路径形状”重新合法化，后续任何 vendor 目录都可援引同一例外；而契约当前明确写“`engine/package.json` 之外的 `package.json` 本轮不允许”，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:177 )。

### 3.3 许可

- MIT 义务是“在副本或实质部分中保留版权声明与许可声明”。`LICENSE` 是完整文本，Option A 保留它；`package.json` 的 `"license": "MIT"` 字段与 README 徽章都不能替代许可文本。契约要求随包 `LICENSE` 并保留版权声明，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1094 )。
- `deny.toml` 的 MIT allowlist 只覆盖 Cargo 依赖图，不覆盖 npm 类型资产，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/deny.toml:42 )。因此 typings 的许可评审必须由 `THIRD_PARTY_LICENSES.md`/`provenance.json`/lock 承担，不能引用 `cargo deny` 作为证据。
- 排除 3 个文件不会造成许可缺失；排除 `LICENSE` 才会。Option A 不受影响。

### 3.4 Provenance

本地 `backends/typescript/node_modules/miniprogram-api-typings` 被 `backends/typescript/.gitignore` 的 `node_modules/` 忽略，`git ls-files` 为 0，**不能作为上游 provenance**，只能作为本地 fixture。上游身份必须由 `assets.lock.json` 的 tarball SRI/SHA-256/size + 完整 26 文件 `upstream.files[]` 固定；本地副本只能用于一致性回归。`package.json` 里的版本/仓库字段不构成可审计锚点，因为整个目录都可能被本地修改。

---

## 4. 选项比较

### 4.1 Option A：payload = 23（推荐）

内容：`index.d.ts`、`LICENSE`、`README.md`、`CHANGELOG.md`、`types/**`；`package.json`、`README-en.md`、`typings.json` 仅存在于上游，不进入包。

| 维度 | 评价 |
| --- | --- |
| 与现有 allowlist | 完全一致，无需改 `is_allowed_native_path` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1095 ) |
| 与契约 negative list | 一致，不需要给 `package.json` 开口子 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:177 ) |
| 与契约 lock 示例 | 一致：示例 `extract.include` 已经是 23 文件集合 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:746 ) |
| 运行时 | 不改变 20 个 `.d.ts`、2,522,752 B、`821c93…`；`PINNED_TYPINGS_*` 全不变 |
| 许可 | `LICENSE` 保留，满足 MIT |
| 安全 | 不引入 npm 元数据；负向扫描保持完整 |
| 代价 | 改 1 个常量、契约 3 处数字、6 个合成 fixture；旧 26 包（尚不存在）不再被新校验器接受 |

### 4.2 Option B：保持 26（需安全评审，不建议）

需要同时改：

1. `forbidden_runtime_path` 为 `typings/miniprogram-api-typings/package.json` 开精确例外，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:42 )。
2. allowlist 增加 `README-en.md`、`typings.json`、`package.json`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1095 )。
3. 契约第 177 行、§4.1 表、lock 示例、负向测试同步修改。
4. 安全评审：package-manager 触发面、未来 TS `types` 解析读 `package.json` 的重定向面。

收益：无（运行时仍只挂载 20 个 `.d.ts`；许可仍靠 `LICENSE`；provenance 仍靠 lock）。风险：把 Node 包元数据重新带入“无 Node”发布物，削弱词法负向扫描，且为后续同类豁免制造先例。**严格劣于 A。**

### 4.3 Option C：A + 显式 lock 审计（推荐的落地形态）

不是第三个安全模型，而是 A 的审计实现：

- `assets.lock.json` 的 `upstream.files[]` 列全 26 文件（含 3 个被排除文件的哈希/大小/模式），`upstream.fileCount=26`、`upstream.unpackedSize=2,552,052`；
- `extract.exclude[]` 精确列 3 条 `{path, reason}`；`extract.include` 为 23 文件集合；
- `payload.fileCount=23`、`payload.totalBytes=2,548,075`、`payload.bundles[0].sha256=ba3eb83…`；
- 打包器/安装器只消费 payload，不读取 lock；lock 变更独立提交。

F3-03 计划已给出同一形状，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-f3-asset-lock-plan.md:200 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-f3-asset-lock-plan.md:219 )。建议采用 C 作为 A 的审计细节，而不是新增 payload 口径。

---

## 5. 推荐与精确改动（本轮不实施）

**裁决建议：Option A（payload 23），按 Option C 记录上游 26。** 由合同 owner 单独提交；实现者不得在本轮静默修改常量。

### 5.1 必须改的代码/契约（按提交顺序）

| 步骤 | 文件:行 | 当前 | 目标 | 备注 |
| --- | --- | --- | --- | --- |
| 1 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:333 ) | typings `fileCount=26` | `23` | 同时标注 `upstream=26` |
| 2 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:412 ) | manifest 示例 `"fileCount": 26` | `23` | 字段形状不变 |
| 3 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:192 ) | `2,552,052 B / 26 文件` | `payload 2,548,075 B / 23 文件；upstream 2,552,052 B / 26 文件` | 保留 registry 事实 |
| 4 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:84 ) | `MINIPROGRAM_TYPINGS_FILE_COUNT: u64 = 26` | `23` | 注释写明 payload 23/upstream 26 |
| 5 | 6 个合成 fixture | `for index in 0..24` | `0..21` | 见 §5.3 |
| 6 | `assets.lock.json`（尚不存在） | — | upstream 26 + payload 23 | 见 §5.2；依赖 tarball 实测，当前 BLOCKED |

契约第 109/115 行的 registry 元数据（`26 文件`、SRI `sha512-2CirO…`、449,429 B）应保留，但必须标注为 **upstream**，不能删成 23；第 746 行的 lock `extract.include` 已经正确，不需要改。

### 5.2 `assets.lock.json` 的 typings 条目（形状；值待实测）

| 字段 | 值/来源 |
| --- | --- |
| `upstream.fileCount` | `26` |
| `upstream.unpackedSize` | `2552052` |
| `upstream.files[]` | 26 条，逐文件 `{path,sha256,size,mode}`；含 3 个被排除文件 |
| `extract.include` | `["index.d.ts","types/**","LICENSE","README.md","CHANGELOG.md"]` |
| `extract.exclude[]` | 3 条：`package.json`（Node 元数据；运行期策略禁止）、`README-en.md`（英文翻译，不在冻结 allowlist）、`typings.json`（旧工具元数据，运行期不需要） |
| `payload.fileCount` | `23` |
| `payload.totalBytes` | `2548075` |
| `payload.bundles[0].sha256` | `ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b`（必须由工具重算，不手填） |
| `payload.treeDigest` | 按 `tree_fingerprint` 算法重算，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:303 ) |

注意：manifest_v2 中**没有** typings 的编译期 bundle 摘要常量；摘要由 `compute_type_asset_bundle` 在打包时计算（( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1327 )），安装时再复算（( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:121 )）。因此“hash 变更”只发生在 lock/provenance/manifest 的运行期数据，不是新增代码常量。

### 5.3 明确**不要**改的内容

| 项 | 位置 | 原因 |
| --- | --- | --- |
| `is_allowed_native_path` 的 typings 分支 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1095 ) | 已经是 23，正是目标 |
| `forbidden_runtime_path` | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:24 ) | 保持 `package.json` 禁令 |
| `typings.include == ["**"]` | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:586 ) | 本次不改 schema 语义；额外文件仍会改变 count/digest |
| `PINNED_TYPINGS_FILE_COUNT=20` | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:122 ) | 运行时闭包不变 |
| `PINNED_TYPINGS_BYTES=2522752` | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:125 ) | 运行时闭包不变 |
| `PINNED_TYPINGS_BUNDLE_DIGEST=821c93…` | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:157 ) | 20 文件闭包不变 |
| 版本/许可/root 常量 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:78 ) | 5.3.1 / MIT / `typings/miniprogram-api-typings` 不变 |

### 5.4 与现有测试 fixture 的冲突

现有合成 fixture 均为 `1 index + 1 LICENSE + 24 types = 26`：

- ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_cli.rs:138 )
- ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_delivery.rs:210 )
- ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_package.rs:167 )
- ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/no_node_guard.rs:572 )
- ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_rollback.rs:222 )
- ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/real_asset/support.rs:498 )

改为 `0..21` 后得到 `1 + 1 + 21 = 23`。这些测试都用常量 `MINIPROGRAM_TYPINGS_FILE_COUNT` 断言，因此常量与 fixture 必须同一提交修改；否则全红。

---

## 6. 迁移与发布影响

1. **manifest 身份变化**：新包 `typeAssets[miniprogram].fileCount=23`、`sha256=ba3eb83…`；新校验器按常量 23 拒绝 26 包，旧校验器按 26 拒绝 23 包。语义变化必须递增合同版本并记录 hash，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1272 )；不得在 worker 内私改。
2. **归档/index**：release 归档的 SHA-256/SRI/size 与 index 记录会变化；`ReleaseV2` 不包含 typeAssets 字段（( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/index_v2.rs:133 )），所以 index 只在归档摘要上变化，不需要 schema 变更。
3. **兼容性**：v2 原生包尚未真实发布（B6 BLOCKED，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1225 )），不存在需要迁移的线上 26 包。若未来已发布，必须按“新增 manifest/index 字段必须同时更新 schema/打包器/校验器/测试/迁移说明”的规则处理，见同文件变更控制段。
4. **F3-03 仍未关闭**：A 只消除内部矛盾；真实 tarball SRI/SHA、lock、clean-env 打包、真实许可/provenance 评审仍缺。
5. **本地 fixture 不是 provenance**：不要把 `backends/typescript/node_modules/miniprogram-api-typings` 的 23/26 摘要直接写成已验 lock 字段；它们只能作为“候选值/回归 fixture”，lock 的 tarball 字段必须来自实测 tarball。

---

## 7. 测试矩阵（建议；路径为计划/现有）

| # | 场景 | 输入 | 断言 | 参考 |
| --- | --- | --- | --- | --- |
| T1 | 拒绝 `package.json` | 输入树含 `typings/miniprogram-api-typings/package.json` | `InvalidPackage`，文案含 `Node runtime file`/`only engine/package.json metadata is allowed` | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1019 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:42 ) |
| T2 | 拒绝 `README-en.md` | 输入树含该文件 | `InvalidPackage`，文案含 `not part of the schema v2 package layout` | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1024 ) |
| T3 | 拒绝 `typings.json` | 输入树含该文件 | 同 T2 | 同上 |
| T4 | 接受 23 文件集合 | 合成或真实 23 文件 | 打包成功；manifest `fileCount=23`、bundle 摘要与重算一致 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1326 ) |
| T5 | manifest 层拒绝 | manifest `files[]` 含 typings `package.json` | `InvalidPackage`，含 forbidden reason | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:469 ) |
| T6 | 安装树拒绝额外文件 | 已验包树多出 `package.json`/`README-en.md`/`typings.json` | `Integrity`（精确文件集合）或 `InvalidPackage`（负向扫描） | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:93 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:110 ) |
| T7 | 计数漂移 | 22 或 24 文件 | `must contain exactly 23 files`/manifest 不一致 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1328 ) |
| T8 | 摘要稳定性 | 同一 23 文件两次计算 | `ba3eb83…` 稳定；字节序排序；与 locale 无关 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:223 ) |
| T9 | 三个摘要空间隔离 | 26 / 23 / 20 文件 | 分别得到 `2141aac…`/`ba3eb83…`/`821c93…`；互不替代 | §1.2 |
| T10 | 运行时只挂载 `.d.ts` | 含 metadata 的完整安装树 | `TypeAssetFile` 拒绝非 `.d.ts`；`snapshot_files` 只含 20 个 `.d.ts`；`source_paths` 不含资产 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:306 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:888 ) |
| T11 | 真实引擎（opt-in） | `TSGO_BIN` + 20 个 `.d.ts` | `types:["miniprogram-api-typings"]` 无 `package.json` 仍能解析 `wx`/`Page`/`Component`/`getApp`；无 2304 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/type_assets.rs:788 )；无 `TSGO_BIN` 时显式跳过，不是门禁证据 |
| T12 | 真实包（opt-in，BLOCKED） | provisioning cache 中的 23 文件 payload | 打包/安装/校验全绿；两次构建归档同哈希 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_package.rs:345 ) |
| T13 | 无 Node | PATH 仅构建工具 + tripwire shim | 打包/安装/校验不触发 node/npm；marker 未创建 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/no_node_guard.rs:769 ) |

T1–T3 已由 `native_package.rs` 的 packager 用例和 `native_delivery.rs` 的 hand-crafted manifest 用例覆盖；T5/T6/T7 同时覆盖 manifest/安装树/计数漂移。`typings_payload.rs` 仍是 opt-in 的真实树一致性检查。T10/T11 依赖未跟踪的后端 WIP 测试，**不是本交付变更的门禁证据**；本轮只更新引用，不据此关闭任何 F3 ID。

补充：工作树里未跟踪的 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/type_assets.rs:60 ) 仍按旧的 10 参数 `TypeAssets::new` 调用，而当前签名是 4 参数（( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:562 )），属于其它会话的 WIP，不能作为门禁证据。

---

## 8. 仍为 BLOCKED、无法从本地证据裁决的事项

1. **真实上游 tarball 身份**：本地没有 `miniprogram-api-typings-5.3.1.tgz`；`node_modules` 被 gitignore 且可能被本地修改。tarball 的 SRI/SHA-256/size、完整 26 文件 `upstream.files[]`、`upstream.treeDigest` 无法在本轮冻结。契约里的 `sha512-2CirO…`/449,429 B 只是 registry 元数据，未复核。
2. **未来引擎是否强制读 `package.json`**：P6-3 只记录了“缺 package.json 仍回退 index.d.ts”的探针结果（( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:98 )）。本轮未重跑真实引擎；若未来 TS/tsgo 改变解析，必须按缺失资产 fail-closed，而不是把 package.json 加回 payload。
3. **Option B 的安全评审**：需要合同 owner + 安全评审对“把 npm 包元数据带入无 Node 发布物”的接受决定；本地证据不能替代该裁决。
4. **合同 owner 授权**：26 是冻结合同值，worker 不能自行改。F3-03 计划明确“必须由合同 owner 单独提交，不由实现者选择”，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-f3-asset-lock-plan.md:446 )；合同变更控制见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1272 )。
5. **真实包级验收**：clean-env 打包、真实 Rust 发布包、Zed 冒烟、安装/回滚/恢复、index/归档摘要均未执行；B6 仍 BLOCKED，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1225 )。
6. **许可/provenance/SBOM 评审**：B7 仅 PARTIAL；本轮只做了本地许可文本核查，未做发布包评审。
7. **文档索引**：`docs/evidence/README.md` 的索引更新不在本任务写入边界内（B10 OPEN），需文档 owner 另行处理。

---

## 9. 关键证据索引

- 冻结文件数： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:84 )
- typings 身份校验： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:583 )
- 打包器 allowlist： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1074 )
- 打包器输入门禁： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1019 )
- 打包器计数： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1326 )
- 运行期负向扫描： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:24 )
- bundle 算法/字节序排序： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:196 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:223 )
- 安装树精确集合： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:93 )
- 安装期 bundle 复算： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:120 )
- tree fingerprint： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:303 )
- 运行时 20 文件引脚： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:122 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:157 )
- `.d.ts` 限制： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:306 )
- VFS 挂载： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:888 )
- 合同必需/禁止： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:160 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:177 )
- 合同 typeAssets 表/示例： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:333 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:412 )
- 合同 lock 示例（已含 23）： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:746 )
- 许可条款： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1094 )；Cargo 许可： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/deny.toml:42 )
- 26/23/20 摘要与文件用途：本文 §1–§2；P6-3 记录见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:69 )；F3-03 冲突记录见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-f3-asset-lock-plan.md:60 )
