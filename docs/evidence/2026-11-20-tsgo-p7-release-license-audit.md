# P7 最终发布产物许可/来源/内容完整性审计（2026-11-20）

## 0. 结论摘要

审计对象是 P7 删除后、tag `p7-removed` 指向的 HEAD `b6c1ced` 构建出的最终
release archive。机械完整性、上游引擎/typings 许可与来源、无 Node/无源码泄漏
均通过；**第三方许可文本覆盖不完整，发布许可门禁 BLOCKED**。

| 维度 | 结果 | 说明 |
| --- | --- | --- |
| 归档机械完整性（文件数/大小/哈希/manifest） | **PASS** | 157 个 manifest 条目逐一匹配；archive SHA-256 与 index SRI 一致 |
| 引擎 Apache-2.0 / typings MIT 与 `assets.lock.json` | **PASS** | 版本、commit、SRI、SHA-256、size、fileCount、license 全部一致 |
| 无 Node / 无 JS / 无后端 / 无测试源码泄漏 | **PASS** | 159 文件全部可归类；无 `.js`、无 `server.js`、无 `node_modules`、无 `backends/` |
| `THIRD_PARTY_LICENSES.md` 覆盖 Rust workspace 依赖 | **FAIL** | 只含 engine/typings 两份上游许可；两个 Rust bin 合计 109 个 runtime 外部 crate、Zed wasm 114 个 crate 均无许可文本/版权声明 |
| 包根首方 LICENSE/NOTICE | **缺失（P2）** | 仓库无任何首方 `LICENSE`/`NOTICE`/`COPYING`，Cargo 也无 `license` 字段 |
| 安装器安装许可文件 | **PASS（仅第三方汇总）** | 安装树保留包根 `THIRD_PARTY_LICENSES.md` 与嵌套 engine/typings 许可 |
| 安装器 verify 校验许可/来源 | **PARTIAL** | 校验存在性/哈希/模式与 provenance 自洽；不解析许可文本、不核对 Cargo 依赖清单、不验 `tsc.sig` |
| **总判定** | **BLOCKED** | 仅对“对外发布/发布候选”成立；若只做内部验证需显式接受 B7/NF-09 的 P1 风险。不阻塞安装、`verify`、LSP/Zed 运行 |
| **最高风险级别** | **P1（HIGH）** | 第三方许可文本缺失，属发布合规阻塞，不是内容损坏 |

一句话：**这个归档是“可安装、可校验、无 Node、字节一致”的产物，但不是“许可材料完整”的发布产物。**
归档本身不包含 `bin/wxml-lsp`、`bin/wxml-lsp-install`、`extension.wasm` 所静态链接的
Rust 依赖许可文本，不能据此关闭交付合同的 B7/NF-09。

## 1. 审计对象与边界

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `b6c1ceda5baa9b55ec1537cff278ece3fc883a9b` |
| Tag | `p7-removed`（与 HEAD 相同） |
| 审计对象 | `/tmp/opencode/p7-post-delete-release-c` |
| Archive | `/tmp/opencode/p7-post-delete-release-c/wxml-lsp-0.1.0-linux-x86_64.tar.gz` |
| Archive SHA-256 | `7a51a3e92b21b829c60f5a4dd5c911b3605c76a5cc09c57dfb7edf491dcd81b7` |
| Archive size | `15,117,270` bytes |
| 审计时间（UTC） | `2026-11-20T02:10Z` 前后 |
| 证据等级 | E1（可复查级；原始产物与命令输出在 `/tmp`，关键哈希与命令已记录） |
| 写操作 | 只新增本证据文件；未改代码/测试/README/ledger/脚本；未 commit |
| 重新构建 | 否。复用既有 A/B/C 构建；A/B/C archive 逐字节相同，见 ` docs/evidence/2026-11-20-tsgo-p7-post-delete-revalidation.md` |

选择 release-c 的原因：该目录是冷 `CARGO_TARGET_DIR` 独立编译产物，且其
`artifact-identity.log` 与 A/B 的 archive SHA-256、两个 Rust bin、engine 二进制、
Zed wasm、manifest 完全一致。A/B/C 的顶层 `SHA256SUMS` 与 `index.json` 亦相同，
因此本报告对 A/B/C 任一目录成立。

审计脚本与中间输出只写在 `/tmp/opencode/p7-license-audit/`，未进入仓库。

## 2. 归档结构、文件数与哈希

### 2.1 顶层内容

Archive 根目录就是包根（tar 内无额外的 `wxml-lsp-0.1.0-linux-x86_64/` 前缀）：

```text
SHA256SUMS                      0644  16,194 B
THIRD_PARTY_LICENSES.md         0644  57,789 B
bin/wxml-lsp                    0755   5,539,136 B
bin/wxml-lsp-install            0755   5,058,560 B
engine/                         122 files
typings/miniprogram-api-typings/ 23 files
zed-extension/                   8 files
manifest.json                   0644  32,927 B
provenance.json                 0644   2,009 B
```

`index.json` **不在 archive 内**，它是 release 通道目录级文件，与 archive 并列：

```text
/tmp/opencode/p7-post-delete-release-c/
├── SHA256SUMS
├── index.json
├── wxml-lsp-0.1.0-linux-x86_64/
├── wxml-lsp-0.1.0-linux-x86_64.tar.gz
└── wxml-lsp-0.1.0-linux-x86_64.tar.gz.sha256
```

按本审计要求的六个文件逐项对照：

| 文件 | archive 内 | 位置 / 说明 |
| --- | --- | --- |
| `THIRD_PARTY_LICENSES.md` | **是** | 包根，57,789 B |
| `LICENSE` | **部分** | `engine/LICENSE`、`typings/miniprogram-api-typings/LICENSE`；**无包根首方 LICENSE** |
| `NOTICE` | **部分** | `engine/NOTICE.txt`；**无包根首方 NOTICE** |
| `provenance.json` | **是** | 包根，2,009 B |
| `manifest.json` | **是** | 包根，32,927 B |
| `index.json` | **否** | release 通道目录，不在 archive；`verify-package` 从 `--from-dir` 读取 |

这是设计行为：安装器从 `--from-dir`/`--source` 读取 `index.json`，archive 只承载
manifest 与 payload；`verify-package` 用 index 的版本/平台/协议与 archive 内
manifest 交叉校验。

### 2.2 文件计数与字节数

| 分组 | 文件数 | 说明 |
| --- | ---: | --- |
| `bin/` | 2 | `wxml-lsp`、`wxml-lsp-install` |
| `engine/` | 122 | 116 个 `lib*.d.ts` + `lib/tsc` + `lib/tsc.sig` + `LICENSE` + `NOTICE.txt` + `README.md` + `package.json` |
| `typings/miniprogram-api-typings/` | 23 | 20 个 `.d.ts` + `LICENSE` + `README.md` + `CHANGELOG.md` |
| `zed-extension/` | 8 | `extension.toml`、`extension.wasm`、5 个 `.scm` + `config.toml` |
| 包元数据 | 4 | `THIRD_PARTY_LICENSES.md`、`provenance.json`、`manifest.json`、`SHA256SUMS` |
| **合计** | **159** | `manifest.files` = 157，另加 `manifest.json` 与 `SHA256SUMS` |

`manifest.files` 总字节：`41,899,646` bytes。archive 内 159 个条目与包目录
159 个文件一一对应，无目录项、无隐藏文件、无额外文件。

### 2.3 逐文件校验

使用脚本对包目录、`manifest.json`、archive 三层交叉核对：

| 检查 | 结果 |
| --- | --- |
| `manifest.files[]` 条目的 SHA-256 / size / mode 与磁盘文件 | **157/157 匹配，0 mismatch** |
| archive 解包后每个 payload 的 SHA-256 与 manifest | **0 mismatch** |
| archive 文件集合 vs 包目录 | 完全一致（159 = 159） |
| manifest 列出但 archive 缺失的文件 | 0 |
| archive 多出且不在 manifest 的文件 | 仅 `manifest.json`、`SHA256SUMS`（自引用元数据，按合同不入 `files[]`） |
| 包根 `SHA256SUMS` 行数 | 158 = `files[]` 157 + `manifest.json` |
| 包根 `SHA256SUMS` 逐行复核 | **158/158 通过**，格式为规范排序 |
| 通道 `SHA256SUMS` / `.tar.gz.sha256` | 均指向同一个 archive SHA-256，复核通过 |

### 2.4 关键哈希

| 对象 | SHA-256 |
| --- | --- |
| `wxml-lsp-0.1.0-linux-x86_64.tar.gz` | `7a51a3e92b21b829c60f5a4dd5c911b3605c76a5cc09c57dfb7edf491dcd81b7` |
| `index.json` | `9ee5620d12f5d6bda8ab87099d9a1d14b2b61756fc32cf63b4c34ecd070e850b` |
| 通道 `SHA256SUMS` | `9136d76c47ca48cae14ac99e20c6f6069e93b06c5329a76d60b6092fdf7f3363` |
| 包根 `SHA256SUMS` | `815fcc1fef43ce567b147b25d18d6dda45727dde72a62482e0ef47a044d44db0` |
| `manifest.json` | `2a6cddc2969df9177b13718d16a21809c0ce6d050f28a35f199ac91cd5555677` |
| `provenance.json` | `b3001987c54845fd42f1c19484f78bdc4ccf9442f73bb25b78612764826addf7` |
| `THIRD_PARTY_LICENSES.md` | `cad23bf2b8e5944a21db316673fbd7c90ddbb6a03be233b0d80c23c13c9bc3b7` |
| `bin/wxml-lsp` | `aa8aa03b40d3445a96405079cc6ab72c8a0a342ed5292306374d9e7540fe77e2` |
| `bin/wxml-lsp-install` | `cc8070fad9a863fb6576a453d2db00af04657ca3a414c53f156bb62b574b45f5` |
| `engine/lib/tsc` | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| `zed-extension/extension.wasm` | `488531eaec969281fa295803e2d3cd857493bd9e26f56adc1c21a71bad0aeb1f` |
| `typings` bundle | `ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b` |
| engine standard-lib bundle | `5e126e7206065b6fff9d651eff3ad66b9413804d3e23d7ecfb4257299af3e85e` |

`index.json` 与 archive 的关系：

- `size` = `15,117,270`，与实际 `stat` 一致；
- `sha256` = `7a51a3e9…dcd81b7`，与实际一致；
- `archiveIntegrity`（SRI）=
  `sha512-iXV/I1efxcpjt2jnBeQx+WnlbvPEEiJqVTj0jXVF8ocUsaIC/BDPoIGlGrHhGPp8ciV2CyGFJTM6OEjL3dIJ7w==`，
  重新计算 SHA-512 后一致；
- `engineVersion`、`engineSourceCommit`、`engineDistribution`、`engineIntegrity`、
  `engineApiSchemaVersion`（9）、`engineAstHeaderBytes`（64）、`minInstallerSchema`（2）
  与 `manifest.json` 完全一致。

### 2.5 可复现性与产物身份

`scripts/build-release.sh` 使用固定 `SOURCE_DATE_EPOCH=1763683200`
（`2025-11-21T00:00:00Z`）。A/B/C 三次构建（含冷 target）archive 逐字节相同，
说明**同机、同工具链、同资产输入下打包可复现**。

但 archive 内没有记录 `wxml-lsp` 自身的源码 commit / 构建环境摘要；包内
`bin/wxml-lsp` 的“来自哪个 HEAD”只能依靠包外证据
（ ` docs/evidence/2026-11-20-tsgo-p7-post-delete-revalidation.md` ）绑定。
交付合同 F3-03 行记录的是更早一次审计的产物限制：该二进制由含并发未提交
改动的工作树构建，不是任一 HEAD 的逐字节重建，见
` docs/reference/tsgo-delivery-contract.md:1366`。本次 P7 复验的 A/B/C 是在
干净 HEAD `b6c1ced` 基线上重新构建（含冷 target），archive 逐字节相同，因此
本归档与 HEAD 的关联由该复验证据承担，而不是由 archive 内字段自描述。

## 3. 许可与来源核对

### 3.1 引擎（TypeScript native）

| 字段 | `assets.lock.json` | `manifest.json` | `provenance.json` | 一致 |
| --- | --- | --- | --- | --- |
| 名称 | `typescript-native-engine` | `typescript` / `native-engine` | `typescript` | 是 |
| 版本 | `7.1.0-beta` | `7.1.0-beta` | `7.1.0-beta` | 是 |
| commit | `3e9236361fa97a9e6c685f4833ac457ae2d8c50a` | 同 | 同 | 是 |
| 分发包 | `@typescript/typescript-linux-x64` | 同 | 同 | 是 |
| SRI | `sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==` | 同 | 同 | 是 |
| tarball SHA-256 | `59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855` | 同 | 同 | 是 |
| size | `10,175,135` | `10,175,135` | `10,175,135` | 是 |
| fileCount | `122` | `122` | `122` | 是 |
| unpacked size | `28,450,796` | `28,450,796` | `28,450,796` | 是 |
| license | `Apache-2.0` | `Apache-2.0` | `Apache-2.0` | 是 |
| licenseFiles | `engine/LICENSE`、`engine/NOTICE.txt` | 同 | 同 | 是 |

包内 `engine/LICENSE`（9,197 B）与 `engine/NOTICE.txt`（46,827 B）的 SHA-256
分别是 `a7d00bfd…54a47`、`317cd4d9…4d33`，与 `assets.lock.json` payload 和
`manifest.files[]` 均一致。`engine/NOTICE.txt` 保留了 Microsoft 及 DefinitelyTyped /
Unicode / W3C / WHATWG / Khronos 等上游 notices，未被改写。

`engine/lib/tsc` 是 24,522,912 B、0755，SHA-256 `a183b195…aaa70`，与
`manifest.binaries[]`、`provenance.engine.binary`、实际文件一致。上游 tarball 的
`tsc.sig` 也随包保留（481 B），但安装器不验证该 PGP 签名（见 §5）。

### 3.2 miniprogram typings

| 字段 | `assets.lock.json` | `manifest.typeAssets[]` | `provenance.json` | 一致 |
| --- | --- | --- | --- | --- |
| 版本 | `5.3.1` | `5.3.1` | `5.3.1` | 是 |
| license | `MIT` | `MIT` | `MIT` | 是 |
| licenseFiles | `typings/miniprogram-api-typings/LICENSE` | 同 | 同 | 是 |
| fileCount | `23` | `23` | `23` | 是 |
| bundle SHA-256 | `ba3eb834…bab46b`（bundles[0]） | `ba3eb834…bab46b` | `payloadTreeDigest=12a224f8…` | 是（tree vs bundle 语义不同，均与 lock 对应字段一致） |
| totalBytes | `2,548,075` | — | `2,548,075` | 是 |

`typings/miniprogram-api-typings/LICENSE` 是 MIT 全文（1,057 B），SHA-256
`feb87a2e0c305de3464cc44077da5393c52d8ca6362d37427157d04ec6f4510d`，与 lock
payload 一致；版权行 `Copyright (c) 2019` 保留（上游原文即无持有人名称）。

`index.json` 与 `manifest.typeAssets[]` 的 bundle digest 由 `verify-package`
重新计算并通过；`miniprogram-api-typings` 还受 `manifest_v2.rs` 的冻结常量
`MINIPROGRAM_TYPINGS_BUNDLE_DIGEST` 约束。

### 3.3 `THIRD_PARTY_LICENSES.md` 的实际覆盖

文件 57,789 B，只有 3 个二级章节：

```text
## miniprogram-api-typings      # MIT 全文
## typescript-native-engine    # Apache-2.0 全文 + NOTICE.txt 全文
## Rust dependencies            # 仅一段说明，无 crate 名称、无许可文本
```

“Rust dependencies” 章节全文只有：

```text
The two Rust binaries are built from this workspace and `Cargo.lock`.
Their transitive dependency licenses are limited by `deny.toml` to
permissive SPDX identifiers (MIT, Apache-2.0, BSD-3-Clause, ISC,
Unicode-3.0, Zlib, ...). Run `cargo deny --offline check` for the
authoritative dependency graph.
```

对包内文本做关键词核对：`serde`、`libc`、`rustls`、`tokio`、`unicode-ident`、
`zed_extension_api` 的出现次数均为 0（`hyper` 仅在 engine NOTICE 的普通文本中
出现 1 次，不是 Rust crate 许可章节）。也就是说：

- **两个 Rust 二进制**：`cargo tree -e normal` 的 runtime 图并集有 109 个外部 crate；
  其中 43 个是单一许可（MIT / BSD-3-Clause / ISC / Apache-2.0 / Unicode-3.0 /
  CDLA-Permissive-2.0 等），其余是双许可/三许可，但无论选哪个许可，都需要随
  二进制分发相应许可文本与版权声明。
- **Zed extension wasm**：`editors/zed` 是独立 workspace，`cargo tree -e normal`
  有 114 个外部 crate，包含 `zed_extension_api`（Apache-2.0）、`unicode-3.0`
  系列、`wasmparser`、`wit-bindgen` 等；这些依赖静态编进 `extension.wasm`，
  同样没有随包许可文本。
- 根 `deny.toml` / CI 的 `cargo deny --offline check` 只分析根 workspace；
  `editors/zed` 因声明了独立 `[workspace]`，不在该门禁的依赖图中。即使把
  `cargo deny` 的 “licenses ok” 当作 Rust 依赖的策略证据，它也不是随包许可
  文本，且覆盖不到 Zed wasm。

> 口径对齐：` docs/evidence/2026-11-20-tsgo-p7-final-security-audit.md` 的
> “Rust 依赖漏洞 / 许可 / 来源 PASS” 指 `cargo deny` 的四项策略检查
> （advisories/bans/licenses/sources）通过；本报告的 FAIL 指**随包许可文本
> 覆盖**，两者不矛盾。策略门禁通过不等于收件人拿到了 MIT/Apache/BSD 等
> 许可文本与版权声明。

交付合同 §8.4（ ` docs/reference/tsgo-delivery-contract.md:1159` ）写明：

> 本项目/Rust 二进制许可：按仓库现有许可；`THIRD_PARTY_LICENSES.md` 汇总。
>
> 许可或来源无法复核时不得发布；不得用“registry 有 license 字段”代替随包许可文本。

而 B7（ ` docs/reference/tsgo-delivery-contract.md:1431` ）仍是 **PARTIAL**，
退出条件为“许可文件、provenance、SBOM 评审通过”。本归档的
`THIRD_PARTY_LICENSES.md` 只是构建期聚合说明，不能作为 Rust 依赖许可已覆盖
的证据。**这是本报告判为 BLOCKED 的直接原因。**

### 3.4 首方（本项目）许可

- 仓库根、各 crate、`editors/zed` 均无 `LICENSE`、`NOTICE`、`COPYING`；
  `git log --all -- LICENSE LICENSE.md` 无结果。
- 根 `Cargo.toml` 的 `[workspace.package]` 没有 `license` / `license-file` 字段；
  `deny.toml` 的 `[licenses.private] ignore = true` 明确跳过 workspace 成员的
  许可检查。
- 包根只有 `THIRD_PARTY_LICENSES.md`，没有首方 `LICENSE`。

这意味着收到归档的人没有获得任何首方使用许可声明。若项目是内部/私有交付，
可由组织政策覆盖；若对外发布，这是独立的发布阻塞项（P2）。

### 3.5 `provenance.json` 与交付合同的差距

`provenance.json` 2,009 B，包含：

```text
schemaVersion, package, packageVersion
assetsLockSha256 = ce33fa960aa3eaf433419e2baeb6709587783ec11450642b6b1188c597725456
engine { name, version, sourceCommit, distribution {integrity,sha256,size,fileCount,unpackedSize,measurement}, binary {path,sha256,size,mode,measurement} }
assets[{name,version,license,packageRoot,fileCount,totalBytes,payloadTreeDigest,tarball{integrity,sha256,size}}]
```

`assetsLockSha256` 与实际 `assets.lock.json` 的 SHA-256 完全一致；engine/typings
的 payloadTreeDigest 与 lock 一致。这部分 PASS。

但交付合同 §8.4 要求 provenance 包含“上游包名/版本/gitHead/SRI/shasum、tarball URL、
归档 SHA-256/大小、文件数/总字节、逐文件 SHA-256/大小/模式、构建时间、构建环境摘要”。
实际文件缺少：

- 归档 SHA-256 / 大小（在 `index.json` 与通道 `SHA256SUMS`，不在 provenance）；
- tarball URL（在 `manifest.engine.distributionUrl`，不在 provenance）；
- 逐文件 SHA-256 / 大小 / 模式（在 `manifest.files[]`，不在 provenance）；
- 构建时间 `builtAt`（在 `manifest.json`）；
- 构建环境摘要、`wxml-lsp` 自身 commit。

这些信息在包内的其他文件中存在，因此**不是证据缺失，而是 provenance 单文件
不满足合同字段清单**。按合同字面属于 P2/P3 偏差；若评审以“包内可追溯”为准，
可降级为 P3。

## 4. 禁用内容与源码泄漏扫描

对包目录和 archive 同时扫描：

| 检查 | 结果 |
| --- | --- |
| `*.js` / `*.mjs` / `*.cjs` / `*.jsx` / `*.tsx` | **0** |
| `*.ts` 且非 `*.d.ts`（测试/业务源码） | **0** |
| `server.js` | **0** |
| `node_modules/` | **0** |
| `backends/`、`backends/typescript` | **0** |
| `package-lock.json` / `yarn.lock` / `pnpm-lock.yaml` | **0** |
| 业务 fixtures（`.wxml` / `.wxss` / `.wxs`） | **0** |
| 可执行文件 | 仅 `bin/wxml-lsp`、`bin/wxml-lsp-install`、`engine/lib/tsc`（均为 0755，符合 allowlist） |

包内 136 个 `.d.ts` 是刻意随包的类型资产：

- `engine/lib/lib*.d.ts` 116 个：TypeScript 标准库声明，供 native engine 使用；
- `typings/miniprogram-api-typings/**` 20 个 `.d.ts`：微信小程序 API 类型，
  来自锁定的 `miniprogram-api-typings@5.3.1`。

它们不是测试源码，也不是业务 fixtures；没有发现 `.ts` 实现文件、WXML 工程文件
或测试用例泄漏。159 个文件已按 §2.2 完整归类，无“用途不明”文件。

唯一需要说明的元数据是 `engine/package.json`：它保留上游的
`"type": "module"` 与 `"engines": {"node": ">=16.20.0"}`。这是上游 npm 包的
版本/provenance 元数据，包内没有 JS 入口，安装器也不执行它；但它可能让简单的
“含 node 字样”扫描产生误报。属于 P3 信息项，不是 Node 运行时泄漏。

## 5. 安装器对许可/来源的处理

### 5.1 安装布局

对 `/tmp/opencode/p7-post-delete-install` 的实际安装树核对：

```text
versions/0.1.0/
├── THIRD_PARTY_LICENSES.md      0644  与 release 包内同哈希
├── engine/LICENSE               0644
├── engine/NOTICE.txt            0644
├── typings/miniprogram-api-typings/LICENSE  0644
├── manifest.json
├── SHA256SUMS
├── provenance.json
├── bin/wxml-lsp
├── bin/wxml-lsp-install
└── ...
```

安装器把验证后的整棵树原子重命名到 `versions/<version>/`，因此包根
`THIRD_PARTY_LICENSES.md` 与嵌套的 engine/typings 许可都会保留；`current`
指向该版本。包根**没有**首方 `LICENSE`（与 release 包一致）。

### 5.2 `verify` / `verify-package` 实际校验什么

native 路径调用 `verify_tree_v2`
（ ` crates/wxml-delivery/src/verify_v2.rs:53` ），安装器 `verify` 走
`verify_native`（ ` crates/wxml-delivery/src/native_install.rs:1057` ）：

| 校验项 | 是否执行 | 依据 |
| --- | --- | --- |
| manifest schema 2、identity、`deny_unknown_fields` | 是 | `read_manifest_v2` |
| 文件集合精确 = `files[] ∪ {manifest.json, SHA256SUMS}` | 是 | `verify_tree_v2` |
| 每文件 SHA-256 / size / mode | 是 | `verify_payload_file` |
| 目录集合与 0755 模式 | 是 | `collect_tree` |
| 禁止运行期 `.js` / `node_modules` / `server.js` 等 | 是 | `forbidden_runtime_path` |
| typeAssets bundle digest 重算 | 是 | `compute_type_asset_bundle` |
| `THIRD_PARTY_LICENSES.md` 存在且 mode 0644 | 是 | `validate_required_payload`（ ` crates/wxml-delivery/src/manifest_v2.rs:1080` ） |
| `provenance.json` 存在且 mode 0644 | 是 | 同上 |
| `engine/LICENSE`、`engine/NOTICE.txt`、typings `LICENSE` 存在 | 是 | `validate_required_payload` |
| provenance 的 `assetsLockSha256` / packageRoot / fileCount / totalBytes / treeDigest 与 `files[]` 自洽 | 是 | `verify_provenance_lock_identity`（ ` crates/wxml-delivery/src/verify_v2.rs:167` ） |
| provenance 是否对应真实仓库 `assets.lock.json` | **否**（lock 不随包） | 只校验自洽；真实性依赖包外索引/签名 |
| 解析 `THIRD_PARTY_LICENSES.md` 内容、核对 Rust crate 覆盖 | **否** | 未实现 |
| 对照 `cargo metadata` / `cargo deny` 的许可证清单 | **否** | 未实现 |
| 校验 `engine/lib/tsc.sig` PGP 签名 | **否** | 未实现 |
| 首方 LICENSE 存在性 | **否** | 合同布局未要求 |

实测 `verify-package` 输出：

```text
$ /tmp/opencode/p7-post-delete-install/current/bin/wxml-lsp-install \
    verify-package --from-dir /tmp/opencode/p7-post-delete-release-c \
    --version 0.1.0 --offline
verified native package wxml-lsp 0.1.0 (linux-x86_64)
protocol: 2  engine: 7.1.0-beta  files: 157
archive sha256: 7a51a3e92b21b829c60f5a4dd5c911b3605c76a5cc09c57dfb7edf491dcd81b7
exit=0
```

安装树 `verify` 输出（历史日志
`/tmp/opencode/p7-post-delete-logs/install-verify.log`）：

```text
verified 0.1.0 (native)
engine version: 7.1.0-beta
files: 157  bytes: 41899646
VERIFY_EXIT=0
```

结论：**安装器校验了“文件内容与元数据一致”，没有校验“许可文本是否覆盖所有
随包代码”**。这是设计边界，不是本次归档独有的缺陷；但它意味着不能把
`verify` 通过当作许可合规通过。

## 6. 风险清单

| ID | 级别 | 发现 | 证据 | 处置建议 |
| --- | --- | --- | --- | --- |
| R1 | **P1（HIGH）** | `THIRD_PARTY_LICENSES.md` 不含 Rust 依赖许可文本/版权声明；两个 bin 合计 109 个 runtime 外部 crate、Zed wasm 114 个 crate 均未覆盖 | §3.3；`cargo tree`；`THIRD_PARTY_LICENSES.md` 仅 3 章节；合同 §8.4/B7 | 发布前加入完整 Rust 第三方许可包（如 `cargo about`/`cargo-deny` 导出 + 各 crate LICENSE/NOTICE），重新打包并更新 manifest/SHA256SUMS；或书面接受内部使用风险并保持 B7/NF-09 打开 |
| R2 | **P2** | 包根无首方 `LICENSE`/`NOTICE`，仓库与 Cargo 元数据均无 license 声明 | §3.4；`find`/`git log`/`Cargo.toml` | 增加首方 LICENSE（或专有声明）并随包；在 `Cargo.toml` 增加 `license` 字段 |
| R3 | **P2** | `provenance.json` 缺合同要求的归档 SHA/大小、tarball URL、逐文件哈希、构建时间、构建环境摘要、`wxml-lsp` commit | §3.5；合同 §8.4 | 扩展 packager 生成字段；或在合同中将 manifest/index 明确定义为这些字段的权威载体并修订措辞 |
| R4 | **P3** | 无发布者签名；`assetsLockSha256` 只做自洽校验，lock 不随包；`tsc.sig` 未验证 | §5.2；ADR-0007/B8 | 保持 B8 accepted risk；如需来源证明，增加签名/attestation 与 `tsc.sig` 验证 |
| R5 | **P3** | archive 不记录构建 commit/工具链，来源绑定依赖包外证据；包内二进制未做独立机器重建 | §2.5；合同 F3-03 行 | 在 provenance 写入 `sourceCommit`/toolchain/`builtAt`，或随通道发布 attestation |
| R6 | **P3** | `engine/package.json` 含 `engines.node` 与 `type: module`，可能触发误报 | §4 | 保留上游原样（推荐）或在扫描规则中加白名单并注释用途 |

风险等级说明：P1 为发布合规阻塞；P2 为需要在发布前决策/补文档的缺口；
P3 为已接受或可通过说明消解的残余风险。**最高风险为 P1。**

## 7. 复现命令（只读）

```bash
REL=/tmp/opencode/p7-post-delete-release-c
PKG=$REL/wxml-lsp-0.1.0-linux-x86_64

# 1) archive 与通道校验
cd "$REL" && sha256sum -c SHA256SUMS
sha256sum -c wxml-lsp-0.1.0-linux-x86_64.tar.gz.sha256

# 2) archive 文件清单 / 权限 / 可执行文件
tar -tzf wxml-lsp-0.1.0-linux-x86_64.tar.gz | wc -l
tar -tvzf wxml-lsp-0.1.0-linux-x86_64.tar.gz | grep -E 'bin/|tsc$|LICENSE|NOTICE|provenance|manifest'

# 3) manifest 逐文件核对（脚本在仓库外）
python3 /tmp/opencode/p7-license-audit/verify_archive.py "$REL"

# 4) 禁用内容
find "$PKG" -type f \( -name '*.js' -o -name '*.mjs' -o -name '*.cjs' \
  -o -name '*.jsx' -o -name '*.tsx' -o -name '*.ts' \) ! -name '*.d.ts'
find "$PKG" \( -iname '*server.js*' -o -iname '*node_modules*' -o -path '*backends*' \)

# 5) 许可覆盖
grep -n '^## ' "$PKG/THIRD_PARTY_LICENSES.md"
for n in serde libc rustls tokio zed_extension_api; do grep -ci "$n" "$PKG/THIRD_PARTY_LICENSES.md"; done

# 6) 依赖图与许可证门禁（根 workspace）
cargo tree --offline -p wxml-lsp -e normal --prefix none --format '{p}|{l}' | sort -u | wc -l
cargo tree --offline -p wxml-delivery -e normal --prefix none --format '{p}|{l}' | sort -u | wc -l
cargo tree --offline --manifest-path editors/zed/Cargo.toml -e normal --prefix none --format '{p}|{l}' | sort -u | wc -l
cargo deny --offline check licenses

# 7) 安装器交叉校验
/tmp/opencode/p7-post-delete-install/current/bin/wxml-lsp-install \
  verify-package --from-dir "$REL" --version 0.1.0 --offline
```

审计脚本输出只在 `/tmp/opencode/p7-license-audit/`；未在仓库内产生临时文件。

## 8. 最终判定

- **内容完整性：PASS。** 157 个 manifest 条目、159 个 archive 文件、大小、模式、
  SHA-256、index SRI 全部一致；无缺失、无多余、无源码/Node 泄漏。
- **上游资产许可与来源：PASS。** 引擎 Apache-2.0（LICENSE + NOTICE）与 typings
  MIT（LICENSE）随包且与 `assets.lock.json` 的版本/SRI/SHA-256/size/fileCount
  一致；`provenance.json` 的资产身份与 lock 自洽。
- **Rust workspace 依赖许可覆盖：FAIL（P1）。** `THIRD_PARTY_LICENSES.md` 只聚合
  两份上游资产许可，不含两个 Rust bin（合计 109 个 runtime 外部 crate）和 Zed wasm
  （114 个 crate）的许可文本/版权声明；根 `deny.toml`/CI 也覆盖不到独立的
  `editors/zed` workspace。
- **首方许可：缺失（P2）。** 包根无 `LICENSE`/`NOTICE`，仓库与 Cargo 元数据
  均未声明许可。
- **安装器：PARTIAL。** 把 `THIRD_PARTY_LICENSES.md` 与嵌套许可安装到包根，
  `verify`/`verify-package` 校验存在性/哈希/模式/provenance 自洽；不校验许可
  覆盖范围、不解析 Cargo 依赖清单、不验 `tsc.sig`。

**总判定：BLOCKED（对外发布/发布候选）。最高风险 P1（HIGH）。**
**不阻塞：安装、`verify`/`verify-package`、native LSP 与 Zed 运行。**

关闭条件（任一）：

1. 生成并随包发布 Rust/Zed 依赖的完整第三方许可清单与文本，重新打包、
   更新 `manifest.json`/`SHA256SUMS`/`index.json` 并复验；同时补齐首方
   LICENSE（或明确的专有许可声明）；或
2. 明确将该归档限定为内部验证用途，书面接受 B7/NF-09 与 P1 风险，并在
   发布说明中声明“许可材料未完成评审”，不得声称许可门禁已通过。
