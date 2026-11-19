# tsgo 真实 miniprogram-api-typings 5.3.1 资产溯源审计（tarball provenance）

- 审计对象：worktree `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`，审计时 HEAD `1519a7342f9ccac742a57c12021d708bf485d888`（工作树含其它会话的未提交修改，见 §10）。
- 审计日期：2026-11-19。
- 写入边界：只新增本文件。未修改生产代码、测试、Cargo、合同、ADR、计划正文；未创建根 `assets.lock.json`；未修改任何冻结常量。
- 执行边界：只访问 `registry.npmjs.org`；未安装/运行 Node/npm；tarball 与解包树只存在于 `/tmp/opencode/tsgo-typings-provenance`，未写入仓库；未执行 `git add` / `commit` / `clean` / `stash`。
- 状态：**本文是 provenance 证据，不关闭 F3-03**。`assets.lock.json` 未创建、真实打包/安装/Zed/ABI 未验，F3-03 仍为 BLOCKED。

---

## 0. 结论摘要

1. 真实 tarball 已从 npm registry 下载：`449,429` B。本地 SHA-512（base64）为 `2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow==`，与 registry `dist.integrity` **完全一致**；本地 SHA-1 `077ab884d2fc0b526afccdf7ae666cfe69e3370a` 与 `dist.shasum` 一致。
2. 解包树为 **26 文件 / 2,552,052 B**，与 registry `dist.fileCount` / `dist.unpackedSize` 及合同记录完全一致。
3. 26 / 23 / 20 三套 **bundle 摘要**全部与冻结值逐字节一致：`2141aac…` / `ba3eb83…` / `821c93…`。**没有发现数值差异，不需要改任何常量，也不因此标 BLOCKED。**
4. 首次计算并记录三套 **treeDigest**（`verify_v2::tree_fingerprint` 语义）：26 文件 `b89a8bc2…`、23 文件 payload `12a224f8…`、20 文件 runtime `fef4d322…`。这三值此前未被冻结，本文只作为候选证据，不改合同。
5. 安全解包检查全部为空：无 symlink、无 hardlink、无绝对路径、无 `..`、无反斜杠、无非 ASCII 名、无大小写碰撞、无重复项、无未分类项、无 `package/` 之外条目。
6. 真实 tarball 与仓库内两份 fixture 逐文件一致（26 文件 node_modules 副本全同；23 文件测试 fixture 等于 payload 子集）。这消除了“fixture 被本地改动”的疑虑，但 fixture 仍不能替代 tarball provenance。

---

## 1. 采集来源与时间

| 项目 | 值 |
| --- | --- |
| 包名/版本 | `miniprogram-api-typings@5.3.1` |
| 版本元数据 URL | `https://registry.npmjs.org/miniprogram-api-typings/5.3.1` |
| 完整 packument URL | `https://registry.npmjs.org/miniprogram-api-typings` |
| Tarball URL | `https://registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.1.tgz` |
| 采集时间（UTC） | 2026-11-19T12:35:30Z（版本元数据）～ 2026-11-19T12:35:38Z（tarball 完成） |
| 采集时间（本地 +08:00） | 2026-11-19 20:35:30 ～ 20:35:38 |
| 本地目录 | `/tmp/opencode/tsgo-typings-provenance`（不在仓库内） |
| Tarball 本地文件 | `/tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz` |
| 解包树本地路径 | `/tmp/opencode/tsgo-typings-provenance/extracted` |
| 下载方式 | `curl -sS --fail`，HTTP 200，未使用 npm/node |

### 1.1 registry 元数据（`GET /miniprogram-api-typings/5.3.1`）

| 字段 | 值 |
| --- | --- |
| `name` / `version` | `miniprogram-api-typings` / `5.3.1` |
| `license` | `MIT` |
| `dist.integrity` | `sha512-2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow==` |
| `dist.shasum` | `077ab884d2fc0b526afccdf7ae666cfe69e3370a` |
| `dist.tarball` | `https://registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.1.tgz` |
| `dist.fileCount` | `26` |
| `dist.unpackedSize` | `2,552,052` |
| `dist.size` | **不存在该字段**；packed 大小以本地实测 `449,429` B 记录 |
| `dist.signatures` | 存在 2 条，`keyid = SHA256:DhQ8wR5APBvFHLF/+Tc+AYvPOdTpcIDqOhxsBHRwC7U`；**本轮未验证** |
| `dist.attestations` | 存在 SLSA provenance URL；**本轮未抓取/未验证** |
| `gitHead` | `e5dc50ccea71e7999a8aa8a942a6f68045e141ca` |
| `_npmVersion` / `_nodeVersion` | `11.19.1` / `24.22.2` |
| `_npmUser.trustedPublisher` | `github`（OIDC 发布）；仅记录，未验证 |
| `main` / `types` | `./index.d.ts` / `./index.d.ts` |

版本元数据与完整 packument 中的 `dist` 字段一致。

### 1.2 registry SRI 与本地 SHA-512 对照

| 口径 | 值 | 结果 |
| --- | --- | --- |
| registry `dist.integrity`（去掉 `sha512-` 前缀） | `2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow==` | — |
| 本地 tarball SHA-512 base64 | `2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow==` | **一致** |
| registry `dist.shasum` | `077ab884d2fc0b526afccdf7ae666cfe69e3370a` | — |
| 本地 SHA-1 | `077ab884d2fc0b526afccdf7ae666cfe69e3370a` | **一致** |

---

## 2. Tarball 字节身份

| 指标 | 值 |
| --- | --- |
| 文件 | `miniprogram-api-typings-5.3.1.tgz`（gzip tar） |
| 字节数 | `449,429` |
| SHA-512（hex） | `d828ab3be0b5cf94662970ea4d448d6eeca8b877f9774fca253b7e42a8b512c89062cf2395886aef25a3a7a11d4dc269bd9ce3b9e7d94027038b161bae022aa3` |
| SHA-512（base64 SRI） | `2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow==` |
| SHA-256 | `e5f8b50fe25b8979432060b915eb1cdfd1c91852e7e3842f3bfca932bdbd4b71` |
| SHA-1 | `077ab884d2fc0b526afccdf7ae666cfe69e3370a` |

合同已记录 packed `449,429` B（ ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:109 )），与本地实测一致。

---

## 3. 解包与安全边界

### 3.1 tar 结构

| 项目 | 值 |
| --- | --- |
| tar 成员总数 | 26 |
| 普通文件数 | 26 |
| 目录条目 | 0（`types/`、`types/wx/` 由解包器按文件祖先创建） |
| 成员前缀 | 全部 `package/`，权威前缀 `package/`（= `NPM_STRIP_PREFIX`） |
| tar 内文件模式 | 全部 `0644`，无任何可执行文件 |
| 解包后目录模式 | `0755`（`types`、`types/wx`） |
| uid/gid | `0/0`，无用户名/组名 |
| tar mtime | 全部 `1985-10-26T08:15:00Z`（npm 打包归一化时间） |
| 解包总字节 | `2,552,052`（与 registry `unpackedSize` 一致） |

### 3.2 安全解包检查

| 检查 | 结果 |
| --- | --- |
| symlink | 0 |
| hardlink | 0 |
| 绝对路径 | 0 |
| `..` 路径分量 | 0 |
| 反斜杠 | 0 |
| 非 ASCII 文件名 | 0 |
| 大小写碰撞（case-fold 重复） | 0 |
| 完全重复路径 | 0 |
| 非普通文件（设备/FIFO/未知类型） | 0 |
| 未分类 tar 类型 | 0 |
| `package/` 前缀之外的成员 | 0 |

解包脚本对每个成员先校验 `isfile()`，再按 `package/` 剥离前缀、逐文件写入 `/tmp`，不处理链接、不调用外部 `tar -x`，因此不存在链接穿越或覆盖风险。除上述 26 个文件外，解包树没有空目录或额外文件。

---

## 4. 26 文件完整清单与 treeDigest

路径均为剥离 `package/` 后的包内相对路径；`payload` 列表示是否进入 23 文件交付 payload，`runtime` 列表示是否进入 20 文件运行时 `.d.ts` 闭包。

| # | path | size (B) | mode | payload | runtime | sha256 |
| ---: | --- | ---: | --- | --- | --- | --- |
| 1 | `CHANGELOG.md` | 21,536 | `0644` | ✓ | — | `9db64dd244b206ced5bebc8510cbc85d907c62fdd7284fc05bb4e7108cdf1ffe` |
| 2 | `LICENSE` | 1,057 | `0644` | ✓ | — | `feb87a2e0c305de3464cc44077da5393c52d8ca6362d37427157d04ec6f4510d` |
| 3 | `README-en.md` | 2,839 | `0644` | — | — | `0b239cf2de187501d8dbed123992ee91a026f431dd6892483af4ebfcebbc8de8` |
| 4 | `README.md` | 2,730 | `0644` | ✓ | — | `a007523387900b26edfcb4c2782e5098c9987e4df6797e66712a689348c8e4e4` |
| 5 | `index.d.ts` | 44 | `0644` | ✓ | ✓ | `a32d71d6bbc9ae904afbc1ef595d14c6f8d7c43ef1226f6718a0a0ce5069efe7` |
| 6 | `package.json` | 1,077 | `0644` | — | — | `7dc13675e2fed5daeb67ab90fe3a26d458883e7b6d1ce542d2b8048c40b6d1fc` |
| 7 | `types/index.d.ts` | 41 | `0644` | ✓ | ✓ | `84cf183ef256e672127308875b5c47e79a544752a855fbf4f19658ce196243ce` |
| 8 | `types/wx/index.d.ts` | 6,857 | `0644` | ✓ | ✓ | `49532ed4fe0524596f661ae263cdc276808d762dd83d57f4ec21c6c6293c35f5` |
| 9 | `types/wx/lib.wx.api.d.ts` | 1,647,088 | `0644` | ✓ | ✓ | `047ea9ccf4b4b17be816730fb0bcf7152a4205f33b15c5d80a691163cad4971f` |
| 10 | `types/wx/lib.wx.app.d.ts` | 17,080 | `0644` | ✓ | ✓ | `c40dcd000a486036a44891d958a11ef0741fa0bd8d20d139317d0f068428eea3` |
| 11 | `types/wx/lib.wx.behavior.d.ts` | 4,898 | `0644` | ✓ | ✓ | `f3f3c278d9642cbbccc165ade38ae2f6561f4ecf9cc75adfdeccad00899d15f7` |
| 12 | `types/wx/lib.wx.canvas.d.ts` | 92,635 | `0644` | ✓ | ✓ | `53c407508e86a2b5b96d7f23e42b4ed98f8e77fe633b2ce3e6eb5530beb14b99` |
| 13 | `types/wx/lib.wx.cloud.d.ts` | 36,872 | `0644` | ✓ | ✓ | `634a9199f013bf94db2b46255be0353b79a17183301ffcc7e779a3a5970775f7` |
| 14 | `types/wx/lib.wx.component.d.ts` | 45,742 | `0644` | ✓ | ✓ | `f38432d3a7a038bc596686fdc59277cbcfd51152dc0faa74ee4fdb3313f6aaa1` |
| 15 | `types/wx/lib.wx.event.d.ts` | 43,010 | `0644` | ✓ | ✓ | `654fdffdde4ea10190d4b8a3649430bab0ea1504e10454c64c11ed6bfd65bb9f` |
| 16 | `types/wx/lib.wx.glass-easel.behavior.d.ts` | 17,430 | `0644` | ✓ | ✓ | `d95971a5c51ad1bdb3a5d50be12816a146fe0230fa92ff81b1750c104886c835` |
| 17 | `types/wx/lib.wx.glass-easel.component.d.ts` | 27,837 | `0644` | ✓ | ✓ | `ffd211c0b5d92c4992b4e16e621ec52c06bfa293fc96494bd015d313f3a7a16f` |
| 18 | `types/wx/lib.wx.glass-easel.d.ts` | 244 | `0644` | ✓ | ✓ | `bd11d66b1ae14ec48884bd3c0b86a03fbe931ae9f66ea5307b5ff858fad3fc4b` |
| 19 | `types/wx/lib.wx.glass-easel.trait-behavior.d.ts` | 628 | `0644` | ✓ | ✓ | `aeb5b0ef37b4382b015fd1fa61b67bd2cb40b3a49441869c3d9f8263ae62fd34` |
| 20 | `types/wx/lib.wx.glass-easel.utils.d.ts` | 17,804 | `0644` | ✓ | ✓ | `c4b93dc0728056834cbe0265c422c93d7881a2dabd928fed4b4f0d3483b02b57` |
| 21 | `types/wx/lib.wx.page.d.ts` | 15,350 | `0644` | ✓ | ✓ | `6a1412877844af91b28b09f7ef3648537a4813e87de6d4a661f73e059c08b856` |
| 22 | `types/wx/lib.wx.phys3D.d.ts` | 12,901 | `0644` | ✓ | ✓ | `bb8555efa0b2ddde9cd79dcb99620d0fa7cdeacd2fecddcc98552ff072576bd4` |
| 23 | `types/wx/lib.wx.skyline.d.ts` | 1,261 | `0644` | ✓ | ✓ | `f1a4f078bcde0b8f0ea2d6c533c688bd408198a8595f499f110cd8e9ca837c27` |
| 24 | `types/wx/lib.wx.wasm.d.ts` | 6,068 | `0644` | ✓ | ✓ | `08bd1b2abbf69a600f0561aaa70fd65b6ff730db6a7f7e7e4964a94c9c5b068c` |
| 25 | `types/wx/lib.wx.xr-frame.d.ts` | 528,962 | `0644` | ✓ | ✓ | `d307154fdf3aae1e3c5c79e812a73e91cc80bcc037684dfdc3aab9abe2a229f4` |
| 26 | `typings.json` | 61 | `0644` | — | — | `0f11c8a7d974fcd773983554a9cc1fc69ce8adc71057d5613c6668e0a736653e` |

**26 文件 treeDigest**：`b89a8bc2d00e14d9b03719328093435dc8153ada5f79f2e7fb9f720b160caaed`。

- 目录行 2 条：`types`、`types/wx`。
- 文件行 26 条（按 UTF-8 字节序）。
- 与解包树按真实磁盘模式计算的 `verify_v2::tree_fingerprint` 结果相同（见 §6.4）。

---

## 5. 23 文件 payload 与 20 文件 runtime

### 5.1 排除的 3 个文件

| path | size (B) | sha256 | 排除原因 |
| --- | ---: | --- | --- |
| `README-en.md` | 2,839 | `0b239cf2de187501d8dbed123992ee91a026f431dd6892483af4ebfcebbc8de8` | 英文翻译，非运行时声明 |
| `package.json` | 1,077 | `7dc13675e2fed5daeb67ab90fe3a26d458883e7b6d1ce542d2b8048c40b6d1fc` | npm 包元数据；运行期负向扫描也拒绝它 |
| `typings.json` | 61 | `0f11c8a7d974fcd773983554a9cc1fc69ce8adc71057d5613c6668e0a736653e` | 旧 `typings` 工具元数据 |

3 文件合计 `3,977` B；`2,552,052 - 3,977 = 2,548,075`。

### 5.2 payload（23 文件）

```
CHANGELOG.md
LICENSE
README.md
index.d.ts
types/index.d.ts
types/wx/index.d.ts
types/wx/lib.wx.api.d.ts
types/wx/lib.wx.app.d.ts
types/wx/lib.wx.behavior.d.ts
types/wx/lib.wx.canvas.d.ts
types/wx/lib.wx.cloud.d.ts
types/wx/lib.wx.component.d.ts
types/wx/lib.wx.event.d.ts
types/wx/lib.wx.glass-easel.behavior.d.ts
types/wx/lib.wx.glass-easel.component.d.ts
types/wx/lib.wx.glass-easel.d.ts
types/wx/lib.wx.glass-easel.trait-behavior.d.ts
types/wx/lib.wx.glass-easel.utils.d.ts
types/wx/lib.wx.page.d.ts
types/wx/lib.wx.phys3D.d.ts
types/wx/lib.wx.skyline.d.ts
types/wx/lib.wx.wasm.d.ts
types/wx/lib.wx.xr-frame.d.ts
```

| 指标 | 值 |
| --- | ---: |
| 文件数 | 23 |
| 总字节 | 2,548,075 |
| treeDigest | `12a224f865880487f63b78d35f5e81835b1542c2bdb3a1e43bad0bc0eda7facb` |
| bundle digest | `ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b` |

构成校验：`index.d.ts` 44 + `LICENSE` 1,057 + `README.md` 2,730 + `CHANGELOG.md` 21,536 + `types/**` 19 文件 2,522,708 = `2,548,075` B。

### 5.3 runtime 闭包（20 个 `.d.ts`）

| # | path | size (B) |
| ---: | --- | ---: |
| 1 | `index.d.ts` | 44 |
| 2 | `types/index.d.ts` | 41 |
| 3 | `types/wx/index.d.ts` | 6,857 |
| 4 | `types/wx/lib.wx.api.d.ts` | 1,647,088 |
| 5 | `types/wx/lib.wx.app.d.ts` | 17,080 |
| 6 | `types/wx/lib.wx.behavior.d.ts` | 4,898 |
| 7 | `types/wx/lib.wx.canvas.d.ts` | 92,635 |
| 8 | `types/wx/lib.wx.cloud.d.ts` | 36,872 |
| 9 | `types/wx/lib.wx.component.d.ts` | 45,742 |
| 10 | `types/wx/lib.wx.event.d.ts` | 43,010 |
| 11 | `types/wx/lib.wx.glass-easel.behavior.d.ts` | 17,430 |
| 12 | `types/wx/lib.wx.glass-easel.component.d.ts` | 27,837 |
| 13 | `types/wx/lib.wx.glass-easel.d.ts` | 244 |
| 14 | `types/wx/lib.wx.glass-easel.trait-behavior.d.ts` | 628 |
| 15 | `types/wx/lib.wx.glass-easel.utils.d.ts` | 17,804 |
| 16 | `types/wx/lib.wx.page.d.ts` | 15,350 |
| 17 | `types/wx/lib.wx.phys3D.d.ts` | 12,901 |
| 18 | `types/wx/lib.wx.skyline.d.ts` | 1,261 |
| 19 | `types/wx/lib.wx.wasm.d.ts` | 6,068 |
| 20 | `types/wx/lib.wx.xr-frame.d.ts` | 528,962 |

| 指标 | 值 |
| --- | ---: |
| 文件数 | 20 |
| 总字节 | 2,522,752 |
| treeDigest | `fef4d32218621debfa7723724b1a98bcb7097646d65a2397a793342ea79b858b` |
| bundle digest | `821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9` |

runtime 的 `.d.ts` 文件集合 = payload 中所有以 `.d.ts` 结尾的文件；不包含 `LICENSE`、`README.md`、`CHANGELOG.md`。

---

## 6. 摘要算法与重算

### 6.1 bundle digest

按 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:198 ) 的实现：对匹配文件按完整路径排序（Rust `str::cmp`，即 UTF-8 字节序），拼接 `"{sha256}  {relative}\n"`，再对整个字节串做 SHA-256。不是 locale 排序；用非 C locale 的 `sort` 会得到不同结果。

### 6.2 treeDigest 语义

按 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:335 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/assets_lock/digest.rs:34 )：

1. 对每个文件路径，收集其所有 `/` 祖先目录（祖先闭包）；根 `""` 不生成目录行。本文树根是剥掉 npm `package/` 前缀后的包根，因此目录只有 `types`、`types/wx`。
2. 目录按路径 UTF-8 字节序排序，序列化为 `D <octal mode> <dir>\n`；目录模式固定 `0755`（`canonical_tree_digest` 语义）。即使 tar 没有目录条目，祖先目录也**必须纳入**。
3. 文件按路径 UTF-8 字节序排序，序列化为 `F <octal mode> <sha256> <path>\n`；文件模式为 npm 归一化后的 `0644`。本 tarball 没有可执行文件，因此没有 `0755` 文件行。
4. 对上述文本做 SHA-256。

26 文件 preimage 共 28 行（2 个目录 + 26 个文件），开头为：

```
D 755 types
D 755 types/wx
F 644 9db64dd244b206ced5bebc8510cbc85d907c62fdd7284fc05bb4e7108cdf1ffe CHANGELOG.md
F 644 feb87a2e0c305de3464cc44077da5393c52d8ca6362d37427157d04ec6f4510d LICENSE
...
```

payload preimage 为 25 行（2 + 23），runtime preimage 为 22 行（2 + 20）。三份 preimage 均由前述规则生成，并分别用 `sha256sum` 独立复核：

| 口径 | preimage 行数 | treeDigest |
| --- | ---: | --- |
| 26 文件上游 | 28 | `b89a8bc2d00e14d9b03719328093435dc8153ada5f79f2e7fb9f720b160caaed` |
| 23 文件 payload | 25 | `12a224f865880487f63b78d35f5e81835b1542c2bdb3a1e43bad0bc0eda7facb` |
| 20 文件 runtime | 22 | `fef4d32218621debfa7723724b1a98bcb7097646d65a2397a793342ea79b858b` |

### 6.3 与磁盘 tree fingerprint 的一致性

把 tarball 解包到 `/tmp` 后，按真实磁盘模式（目录 0755、文件 0644）调用 `verify_v2::tree_fingerprint`，26 文件树得到 `b89a8bc2…`，与按 lock 语义（祖先闭包 + 固定 0755）算出的值相同。23/20 子树分别得到 `12a224f8…` / `fef4d322…`。这说明：

- tar 内没有隐藏的目录模式差异或可执行位；
- 本文的 ancestor-closure 计算与 Rust 磁盘遍历语义一致。

### 6.4 Rust 实现交叉验证

1. 用现有 opt-in 测试读取真实解包树（未修改测试）：
   `WXML_TYPINGS_FIXTURE_DIR=/tmp/opencode/tsgo-typings-provenance/extracted cargo test -p wxml-delivery --test typings_payload -- --ignored --nocapture`
   输出：
   `typings fixture: upstream=26 files/2552052 B/2141aac47206441f0434a7b519e49da1e8756d1bcdb6c17809a7f5e188a34c42; payload=23 files/2548075 B/ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b; runtime=20 files/2522752 B/821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9`
   该测试还断言 payload 路径集合精确等于 `MINIPROGRAM_TYPINGS_PAYLOAD_PATHS`，并检查 20 个运行时声明不引用 3 个被排除文件。
2. 用 `/tmp` 下的临时 Rust probe 直接调用 `wxml_delivery::verify_v2::tree_fingerprint`（path 依赖当前 worktree crate），对 26/23/20 三棵树分别得到 `b89a8bc2…` / `12a224f8…` / `fef4d322…`，与 §6.2 一致。

---

## 7. 与现有冻结值对比

| 口径 | 冻结值（合同/代码/测试） | 真实 tarball 实测 | 结果 |
| --- | --- | --- | --- |
| 上游文件数 | 26（ ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:92 )） | 26 | **一致** |
| 上游解包字节 | 2,552,052（ ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:94 )） | 2,552,052 | **一致** |
| 上游 bundle 摘要 | `2141aac47206441f0434a7b519e49da1e8756d1bcdb6c17809a7f5e188a34c42` | `2141aac47206441f0434a7b519e49da1e8756d1bcdb6c17809a7f5e188a34c42` | **一致** |
| payload 文件数 | 23（ ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:90 )） | 23 | **一致** |
| payload 字节 | 2,548,075（ ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:96 )） | 2,548,075 | **一致** |
| payload bundle 摘要 | `ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b`（ ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:102 )） | 同值 | **一致** |
| runtime 文件数 | 20（ ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/assets_lock/mod.rs:81 )） | 20 | **一致** |
| runtime 字节 | 2,522,752（ ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/assets_lock/mod.rs:83 )） | 2,522,752 | **一致** |
| runtime bundle 摘要 | `821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9`（ ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/assets_lock/mod.rs:88 )） | 同值 | **一致** |
| registry `fileCount` | 26 | 26 | **一致** |
| registry `unpackedSize` | 2,552,052 | 2,552,052 | **一致** |
| packed 大小 | 449,429（ ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:109 )） | 449,429 | **一致** |
| tarball SRI | `sha512-2CirO+…`（合同 §2 事实表第 8 行） | 同值 | **一致** |

结论：**没有 26/23/20 数值差异，也没有 SRI/size 差异；不需要修改任何合同常量，不因数值不一致标 BLOCKED。** 本文新增的 treeDigest 不是现有冻结值，仅作为未来 `assets.lock.json` 的候选记录。

合同 §2 事实表第 8 行仍写着“tarball SRI/SHA-256 仍未复核”，§4 typings 段仍写着“上述 23/26 值是本地未打包树的复算候选值，不是 tarball provenance”（ ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:346 )）。本轮已用真实 tarball 补齐该缺口；按写入边界本文**未修改合同**，合同文本与 lock 状态由合同 owner 后续处理，F3-03 不因本文关闭。

---

## 8. 许可与包元数据

| 项目 | 值 |
| --- | --- |
| `LICENSE` 文件 | 存在，`1,057` B，SHA-256 `feb87a2e0c305de3464cc44077da5393c52d8ca6362d37427157d04ec6f4510d` |
| `LICENSE` 首行 | `MIT License` |
| registry `license` | `MIT` |
| `package.json` `license` | `MIT` |
| `package.json` `name` / `version` | `miniprogram-api-typings` / `5.3.1` |
| `package.json` `main` / `types` | `./index.d.ts` / `./index.d.ts` |
| `package.json` `scripts` | 只有 `tsd`/`test`/`eslint` 开发脚本；无 `preinstall`/`install`/`postinstall` |
| `typings.json`（被排除） | `{"name":"miniprogram-api-typings","main":"index.d.ts"}`，61 B |
| `gitHead` | `e5dc50ccea71e7999a8aa8a942a6f68045e141ca` |

许可文件进入 23 文件 payload，并在合同 `typeAssets[].licenseFiles` 中固定为 `typings/miniprogram-api-typings/LICENSE`；本次真实 tarball 确认该文件存在且为 MIT 文本。

---

## 9. 与仓库内 fixture 的交叉核对（非 provenance）

| 对照对象 | 文件数 | 结果 |
| --- | ---: | --- |
| `backends/typescript/node_modules/miniprogram-api-typings` | 26 | 与 tarball 解包树 26 个文件逐一 SHA-256 相同 |
| `crates/wxml-delivery/tests/fixtures/miniprogram-api-typings-5.3.1` | 23 | 与 payload 23 文件逐一 SHA-256 相同，恰好缺少 3 个被排除文件 |

`diff` 对比基于 `find -type f` + `sha256sum` 排序输出，两份 fixture 均无差异。该结果说明仓库 fixture 当前未被本地改动、且与真实 registry tarball 字节一致；但 fixture 是仓库内副本，**不能替代 registry provenance**，也不能单独关闭 F3-03。

---

## 10. 残余风险与 BLOCKED

1. **根 `assets.lock.json` 未创建**（任务约束）。`upstream.files[]`、`upstream.treeDigest`、payload/runtime bundle 仍未被 lock 工具冻结；本文只是候选证据。F3-03 **仍 BLOCKED**。
2. **真实打包未验证**：尚未用该 tarball 走完 `wxml-lsp-package` 的完整 extract→allowlist→manifest→archive 流程；23 路径 allowlist、manifest 复算、archive 摘要与安装树校验未在该真实输入上端到端执行。
3. **真实安装/回滚/升级未验证**：未用由真实 tarball 打出的发布包执行 `wxml-lsp-install`、事务回滚、恢复或升级；clean-env 安装未跑。
4. **Zed 扩展与 ABI 未验证**：Zed extension WASM 未构建/加载；native engine 的协议版本、ABI、真实 tsgo 运行未验证。
5. **registry 签名/attestation 未验证**：只做了字节哈希绑定；`dist.signatures` 与 SLSA provenance 未验证（需要 npm/sigstore 工具链，超出本轮边界）。
6. **单一来源与传输信任**：只从一个 registry URL 下载一次，未对比镜像，未做 TLS 证书固定/透明度审计；HTTPS 与 DNS 信任是隐含前提。
7. **时间点风险**：npm 版本理论上不可变，但本轮只记录 2026-11-19 的观测；安装器仍必须只信 lock，不得在安装时重新解析 registry。
8. **`package.json` 解析风险**：该文件被排除且运行期负向扫描拒绝；未来若 TS/tsgo 改为强制读取 `package.json`，需重新评估，不能把元数据加回 payload。
9. **npm pack 可复现性未验证**：未验证 gzip 层能否逐字节重现；本文只证明“下载到的字节”与 registry 元数据一致。
10. **工作树并发修改**：审计时 `crates/wxml-delivery/src/lib.rs`、`crates/wxml-delivery/src/bin/wxml-lsp-package.rs`、`crates/wxml-lsp/**` 等有其它会话未提交修改；本文引用的是审计时 worktree 内容，不声称等于 `origin` HEAD。`verify_v2::tree_fingerprint` 与冻结常量文件未被本审计修改。

---

## 11. 复现步骤

```bash
mkdir -p /tmp/opencode/tsgo-typings-provenance
cd /tmp/opencode/tsgo-typings-provenance

curl -sS --fail -o registry-5.3.1.json \
  https://registry.npmjs.org/miniprogram-api-typings/5.3.1
curl -sS --fail -o miniprogram-api-typings-5.3.1.tgz \
  https://registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.1.tgz

sha256sum miniprogram-api-typings-5.3.1.tgz
sha512sum miniprogram-api-typings-5.3.1.tgz
openssl dgst -sha512 -binary miniprogram-api-typings-5.3.1.tgz | openssl base64 -A; echo
sha1sum miniprogram-api-typings-5.3.1.tgz
```

解包与清单用 Python 标准库 `tarfile` 逐成员校验 `isfile()` 后写入 `/tmp`，再按 §6 规则重算 bundle/tree 摘要。未使用 `npm`、`node` 或外部 `tar -x`。

交叉验证（可选，只读现有测试）：

```bash
WXML_TYPINGS_FIXTURE_DIR=/tmp/opencode/tsgo-typings-provenance/extracted \
  cargo test -p wxml-delivery --test typings_payload -- --ignored --nocapture
```

---

## 12. 证据索引

- bundle 算法： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:198 )
- tree fingerprint preimage： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:335 )
- tree fingerprint 磁盘遍历： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:369 )
- lock 侧 treeDigest（祖先闭包 + 0755）： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/assets_lock/digest.rs:34 )
- 23 文件冻结常量： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:90 )
- 26 文件/字节冻结常量： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:92 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:94 )
- payload 摘要常量： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:102 )
- 3 个排除路径： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:105 )
- 23 路径 allowlist： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:112 )
- runtime 20 文件/字节/摘要常量： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/assets_lock/mod.rs:81 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/assets_lock/mod.rs:83 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/assets_lock/mod.rs:88 )
- 原生后端 runtime pin： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:131 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:134 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:166 )
- 真实 fixture 一致性测试： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/typings_payload.rs:41 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/typings_payload.rs:46 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/typings_payload.rs:50 )
- 合同 typings 口径与“不是 tarball provenance”声明： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:339 )
- 合同 packed 大小/SRI 记录： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:109 )
