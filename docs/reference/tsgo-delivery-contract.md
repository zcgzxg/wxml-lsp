# tsgo 无 Node 交付合同（草案）

**状态**：draft（草案），**未接受（not accepted）**，**未实现（not implemented）**。

**日期**：2026-11-17。**合同版本**：`draft-0`。**适用发布**：Linux x86_64、原生引擎 `7.1.0-beta`、manifest/index schema v2。

> **2026-11-17 后续**：维护者已条件性接受 `7.1.0-beta`（gitHead
> `3e9236361fa97a9e6c685f4833ac457ae2d8c50a`、linux-x64 SRI/`lib/tsc` SHA-256 见 §2）作为唯一
> 候选基线。临时 Python spike 已执行一次可执行安装器证明，见
> [P1-C 证据 §11](../evidence/2026-11-17-tsgo-delivery-spike.md)；该证明**不是**生产安装器，
> 不改变本文 draft/未接受状态。当前 F3 进度为 **13 closed / 2 partial / 1 blocked**（F3-14
> blocked，F3-06/09 partial），见 §9.5。

**关系**：[ADR-0007](../adr/0007-local-delivery-trust-model.md)（accepted，信任/离线/原子安装/回滚规则继续有效）；
[ADR-0008](../adr/0008-rust-tsgo-no-node-backend.md)（proposed，迁移方向）；实施计划
[§8～§9](../plans/2026-11-17-rust-tsgo-migration.md)；事实来源
[P1-C 交付试验](../evidence/2026-11-17-tsgo-delivery-spike.md)；F0 常量对齐
[F0 原生合同草案](tsgo-native-contract.md)。

> 本文件是 **F3 交付合同草案**，不是实现完成报告、不是验收证据、也不是发布说明。它不表示 migration、
> 无 Node 验收、安装器、schema v2 或 Zed 冒烟已经通过。F3 验收仍未通过；测试文件存在或默认
> 测试为绿都不等于验收通过。在本文件被维护者接受前，实现不得声称“符合 F3 合同”。

## 1. 状态、范围与权威

### 1.1 状态

- 本文是**拟冻结文本**。字段名、算法和状态机可被评审修改；未接受的条款不是实现依据。
- 本文**未**加入 [docs/README.md](../README.md) 的权威索引；索引更新由文档 owner 在合同接受后单独提交。
- 本文不修改 ADR、计划、源码、Cargo、脚本、CI 或 Zed 扩展；不在本轮产生构建产物。
- 当前 `wxml-delivery` 仍是 schema v1/Node 链路。本文描述的是目标状态，不能引用为现状。

### 1.2 范围

**包含**：

- Linux x86_64 发布包内容、安装根布局；
- manifest v2 / index v2 / state / transaction journal 字段；
- 原生资产的获取、解包、校验和探针；
- install / upgrade / rollback / verify / current / env / list 的语义；
- 旧 schema v1 安装的只读识别与迁移；
- 离线、平台、glibc/ABI 边界；
- 交付安全与信任边界；
- F3 验收测试 ID、未来路径和阻塞项。

**不包含**：

- F0 原生 API 合同与 AST 细节（由 F0 草案冻结，待其自身接受；本文只消费其版本/schema 常量）；
- F1 查询语义、shadow、rename、诊断发布行为；
- F2 测试迁移账本与差分规则；
- 实际 release 产物、CI/构建脚本、Zed 扩展代码；
- macOS、Windows、aarch64 及其他平台的交付承诺。

### 1.3 权威顺序

发生冲突时按以下顺序判断：

1. 有效的 ADR（当前 ADR-0007 accepted；ADR-0008 proposed 仅为方向）；
2. 实施计划 §8～§9（NF-01～NF-12、F3、§8.1 状态机）；
3. 本文件；
4. 证据记录（历史结论及其限制）。

代码事实优先于文档。发现文档与代码不一致时，应更新文档并记录差异，不能保留两套说法。
本文件是**目标合同**，与当前 schema v1 代码不一致是预期状态，不据此宣称代码有缺陷。

### 1.4 术语

| 术语 | 含义 |
| --- | --- |
| native package | 只含 Rust 二进制、原生 `lib/tsc`、类型资产和 Zed 扩展的 schema v2 包 |
| legacy package | 现有 schema v1、包含 Node 后端和 `server.js` 的历史包；只用于只读识别与迁移 |
| engine | 上游 TypeScript 原生发行包解出的 `engine/lib/tsc` 及其同目录标准库 |
| `current` | 安装根下指向 `versions/<version>` 的相对 symlink，是唯一活动入口 |
| staging | 同文件系统的 `tmp/<txn>/tree`，校验和探针通过前不得成为 `current` |
| transaction | 一次 install/upgrade/rollback 的 journal 记录，状态仅 `staged`/`committed`/`activated` |
| provisioning | 构建机上的**显式**原生资产获取步骤；唯一允许由构建流程联网的阶段 |
| SRI | Subresource Integrity 字符串，本合同中固定为 `sha512-<base64>` |
| typeAssets | 随包固定的标准库 `.d.ts` 与 `miniprogram-api-typings` 版本/许可/摘要元数据 |
| preprovisioned | 在无网络或离线安装前已放入缓存/`--from-dir` 的已验证资产 |

本文件中“必须/MUST”为合同要求，“应/SHOULD”为建议但不作为本轮门禁。

### 1.5 生效条件

本文件从 draft 变为 accepted 前，至少满足：

1. F0 草案冻结 `protocolVersion`、`engine.apiSchemaVersion`、`engine.astHeaderBytes`、`--version` 探针前缀和 `--api --async` 握手；
2. `assets.lock.json` 已提交，含精确 URL、版本、SHA-512 SRI、大小、许可和 `extract` allowlist；
3. 维护者已**条件性**接受 `7.1.0-beta` 作为唯一候选基线（2026-11-17）；正式接受仍以 F3
   评审和证据 §11 的限制为准；
4. 安全评审接受 ADR-0007 的“TLS+摘要、无发布者签名”残留风险；
5. 本文 §10 的阻塞项有明确 owner 和退出条件。

ADR-0008 是否从 proposed 升为 accepted 由 ADR 自身门禁决定，不由本文件替代。

## 2. 事实分级

本节区分**已核实的 registry 元数据**、**仓库静态事实**、**P1-A 运行时证据**和**本交付链未验证的声明**。
交付合同只能把前两类当作确定性输入；P1-A 结果只作为独立证据引用；第四类必须留到 F3 验收。

| # | 事实 | 值 / 结论 | 类别 | 限制 |
| --- | --- | --- | --- | --- |
| 1 | 平台包版本 | `@typescript/typescript-linux-x64@7.1.0-beta` | registry 元数据 | 2026-11-17 通过 HTTPS 读取；未下载 tarball |
| 2 | 源码提交 | `gitHead=3e9236361fa97a9e6c685f4833ac457ae2d8c50a` | registry 元数据 | 与 ADR-0008 引用提交一致 |
| 3 | 归档 SRI | `sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==` | registry 元数据 | `dist.integrity` |
| 4 | 许可/平台 | Apache-2.0；`os=["linux"]`、`cpu=["x64"]`；无 `libc` 字段 | registry 元数据 | 无 `libc` 意味着不能从元数据区分 glibc/musl |
| 5 | 归档/解包大小 | packed `10,175,135` B；unpacked `28,450,796` B；`122` 文件 | registry 元数据 | packed 由 1 字节 range 得到；文件数来自发布清单 |
| 6 | 引擎文件 | `lib/tsc` `24,522,912` B；`116` 个 `lib*.d.ts`（含 `lib.d.ts`）共 `3,870,196` B；`LICENSE` `9,197` B；`NOTICE.txt` `46,827` B；`tsc.sig` `481` B | registry 发布清单 | 未下载真实字节；文件大小以清单为准 |
| 7 | npm 入口性质 | `typescript` 包的 `bin/tsc` 是 `#!/usr/bin/env node` launcher；平台包无 `bin/`，原生入口是 `lib/tsc` | registry 小文本文件内容 | 这不是本项目运行时验证 |
| 8 | 类型资产 | `miniprogram-api-typings@5.3.1`：MIT；registry 上游 26 文件、packed `449,429` B、unpacked `2,552,052` B、SRI `sha512-2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow==`；交付 payload 23 文件/`2,548,075` B（`extract.exclude` 排除 `package.json`、`README-en.md`、`typings.json`） | registry 元数据 + 2026-11-19 本地未打包树只读复核 | 26 是上游身份，23 是 manifest 口径；两者都已在根 `assets.lock.json` 冻结，tarball SRI/SHA-256 已复核；F3-03 其余项仍 BLOCKED |
| 9 | 现有 v1 合同 | schema v1 强制 `node` 字段和 `node-backend` artifact；index v1 强制 `nodeMinimumVersion`；`env` 输出 `WXML_TYPE_BACKEND` | 仓库静态事实 | 见 `crates/wxml-delivery/src/manifest.rs`、`bin/wxml-lsp-install.rs` |
| 10 | 现有校验/安装骨架 | 临时区校验 → `rename` 到 `versions/` → 临时 symlink atomic `rename`；OS 文件锁；`WXML_LSP_BIN` 优先 | 仓库静态事实 | 现有测试为 v1/Node 路径，不能替代 F3 |
| 11 | 原生进程能力 | `7.1.0-beta` 在 Linux x64 的 `--api --async`、快照、查询、AST v9/header 64 等 | P1-A 运行时证据（独立记录） | 本交付合同未重跑；F0 必须冻结常量 |
| 12 | `7.0.2` 排除 | 稳定版不具备所需 snapshot/租约/AST protocol v5，不满足架构；明确不作为回退候选 | P1-A 运行时证据 + ADR-0008 决策 | 不是 registry 元数据事实 |
| 13 | 交付运行时声明 | 临时 Python spike 已在 Linux x64 验证 install/verify/upgrade/rollback/离线/无 Node PATH；生产 Rust 安装器、CI、Zed 冒烟、平台矩阵仍未验证 | 2026-11-17 临时证据（非生产） | 见 §10 与证据 §11；不得把 spike 当生产完成 |
| 14 | 引擎资产摘要 | tarball SHA-256 `59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855`、SRI 见 #3、size `10175135`；解包 122 文件/`28450796` B；`lib/tsc` SHA-256 `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70`、`Version 7.1.0-beta` | F0/P1-A + 2026-11-17 spike 复核 | tarball/引擎已复核；`miniprogram-api-typings` 只有已解包 26 文件，SRI 因无 tarball 未复核 |

**不得混用**：

- registry 文件清单/大小/SRI 只能证明“上游包的身份”，不能证明“本机可执行”。
- P1-A 的运行时结果只能证明其记录的 Linux x64 场景，不能证明安装器、包布局、离线、回滚或 Zed。
- 现有 v1 测试通过不能证明 schema v2；现有 Node 路径的“成功”也不是无 Node 成功。

## 3. 发布包与安装布局

### 3.1 包树

Linux x86_64 release 目录（`wxml-lsp-<version>-linux-x86_64/`）必须包含以下结构；标注“可选”的条目可以省略，其余必须存在：

```text
wxml-lsp-<version>-linux-x86_64/
├── bin/
│   ├── wxml-lsp                       0755  Rust LSP，version == packageVersion
│   └── wxml-lsp-install               0755  Rust 安装器，便于离线修复/回滚
├── engine/
│   ├── lib/
│   │   ├── tsc                        0755  原生引擎 7.1.0-beta
│   │   ├── tsc.sig                    0644  随上游保留；运行期是否必需由 F0 探针决定
│   │   └── lib*.d.ts                  0644  ×116，标准库类型声明（含 lib.d.ts）
│   ├── LICENSE                        0644  Apache-2.0 全文
│   ├── NOTICE.txt                     0644  Apache-2.0 NOTICE
│   ├── package.json                   0644  仅作版本/provenance 元数据，不执行
│   └── README.md                      0644  可选
├── typings/
│   └── miniprogram-api-typings/
│       ├── index.d.ts                 0644
│       ├── types/**                   0644
│       ├── LICENSE                    0644  MIT
│       └── README.md / CHANGELOG.md   0644
├── zed-extension/
│   ├── extension.wasm                 0644
│   ├── extension.toml                 0644
│   ├── languages/**                   0644  五语言注册保持
│   └── grammars/wxml.wasm             0644  可选，预置 grammar
├── manifest.json                      0644  schema v2
├── SHA256SUMS                         0644  精确文件集合
├── provenance.json                    0644  NF-09 溯源
└── THIRD_PARTY_LICENSES.md            0644  第三方许可汇总
```

### 3.2 必需、可选与禁止

**必需**：`bin/wxml-lsp`、`bin/wxml-lsp-install`、`engine/lib/tsc`、116 个 `engine/lib/lib*.d.ts`（含 `lib.d.ts`）、
`engine/LICENSE`、`engine/NOTICE.txt`、`engine/package.json`、`engine/lib/tsc.sig`、
`typings/miniprogram-api-typings/index.d.ts` 与 `types/**`、typings `LICENSE`、
`zed-extension/extension.wasm`、`zed-extension/extension.toml`、`zed-extension/languages/**`、
`manifest.json`、`SHA256SUMS`、`provenance.json`、`THIRD_PARTY_LICENSES.md`。

**可选**：`engine/README.md`、`zed-extension/grammars/wxml.wasm`。可选文件一旦存在就必须进入
`files[]` 和 `SHA256SUMS`，不能出现“存在但未哈希”的文件。

**禁止**：

- `node_modules/`、`bin/wxml-type-backend/`、`server.js`、任何运行期 `.js`/`.cjs`/`.mjs`；
- `package-lock.json`、`yarn.lock`、`pnpm-lock.yaml` 等 Node 依赖锁；
- 任何未在 `binaries[]` allowlist 中的可执行文件；
- 包内 symlink、hardlink、FIFO、设备文件、绝对路径、`..` 路径、重复路径、控制字符；
- `engine/package.json` 之外的 `package.json`（除非未来合同明确允许；本轮不允许）。

`engine/package.json` 只作为元数据保留，安装器和 LSP 不得执行其中的 `scripts`，也不得据此启动 Node。

**关键文件大小（registry 事实，发布时以 manifest/checksums 为准）**：

| 文件 | 大小 | 说明 |
| --- | ---: | --- |
| `engine/lib/tsc` | 24,522,912 B | 原生 ELF，`0755` |
| `engine/lib/lib*.d.ts` | 3,870,196 B | 116 个文件合计（含 `lib.d.ts`） |
| `engine/LICENSE` | 9,197 B | Apache-2.0 |
| `engine/NOTICE.txt` | 46,827 B | 必须随包 |
| `engine/lib/tsc.sig` | 481 B | 用途待 F0 确认，先随包保留 |
| `engine/package.json` | 1,045 B | 元数据 |
| `engine/README.md` | 138 B | 可选 |
| `typings/miniprogram-api-typings`（payload） | 2,548,075 B | 23 文件，MIT；manifest `fileCount` 与 bundle 摘要以此为准 |
| `typings/miniprogram-api-typings`（上游解包树） | 2,552,052 B | 26 文件；含被排除的 `package.json`、`README-en.md`、`typings.json`，只用于 lock/provenance 审计 |

### 3.3 安装根布局

```text
<root>/
├── current -> versions/<active>          相对 symlink；唯一活动入口
├── versions/
│   ├── <active>/                        精确等于 §3.1 包树
│   └── <previous>/                      已验证的原生历史版本
├── cache/
│   └── <version>/archive.tar.gz         可选；命中后必须重新校验
├── tmp/
│   └── <txn>/tree/                      暂存区；0700；失败/崩溃后由下一次 mutating 命令清理
├── txn/
│   └── <txn>.json                       事务 journal；仅 staged/committed/activated
├── state.json                           活动/previous/engine 身份
└── .lock                                OS 文件锁（实现细节；只读命令不创建）
```

规则：

- `versions/<v>` 是**不可变**发布树；安装后不得原地修改。任何修复都必须走新事务/重新安装。
- `current` 必须是指向 `versions/<v>` 的**相对** symlink；禁止绝对路径、`..`、多级路径或指向
  `versions/` 之外。解析后必须仍在 `<root>/versions/` 内。
- `cache/<v>/archive.tar.gz` 只缓存归档，不作为可信来源；每次命中重新校验 SRI/SHA-256/大小。
- `tmp/<txn>/`、`txn/<txn>.json`、`state.json` 必须与 `versions/` 同文件系统，以支持 `rename`。
- `list`/`current`/`verify`/`env`/`versions` 不得创建 root、不得 chmod 已有目录、不得创建锁文件、
  不得联网。
- LSP 从 `current/bin/wxml-lsp` 启动时，以 `/proc/self/exe` 解析出的真实路径定位包根
  （`versions/<v>/`），读取同目录 `manifest.json`，再解析 `engine.binary`；不得使用 PATH/cwd
  自动搜索，不得探测 `server.js`。
- 因此 `WXML_LSP_BIN` 必须指向安装树内的 `bin/wxml-lsp`（或测试构造的同构包根）；裸二进制
  缺少 manifest/engine 时必须显式失败，不得猜测引擎路径或回退 Node。
- LSP 启动引擎前必须按 F0-03 校验 `engine/lib/tsc` 为常规文件、可执行、非 symlink、大小与
  SHA-256 匹配 manifest；失败显式报错，不启动引擎、不回退 Node。

> 这是相对 ADR-0007 的**收紧**：`WXML_LSP_BIN` 仍是唯一入口且仍优先，但目标必须位于有效包根，
> 否则无法在不查 PATH/不启动 Node 的前提下定位 manifest 与 `engine.binary`。若维护者不接受该
> 收紧，必须在 F3 前修订 ADR-0008/本契约，不能由实现自行放宽。

### 3.4 归档命名与确定性

- 归档名：`wxml-lsp-<version>-<platform>.tar.gz`；同目录 sidecar `wxml-lsp-<version>-<platform>.tar.gz.sha256`。
- 归档为确定性 tar.gz：条目按路径排序、`mtime=0`、`uid/gid=0`、`uname/gname` 为空、模式固定；
  同一输入两次构建必须产生相同 SHA-256（F3-03）。
- `index.json` 与归档同级；`archive` 字段只能是文件名，不含 `/`、`..`、URL 或 query。
- 打包器必须在写 archive 前生成完整 `files[]`/`SHA256SUMS`，写完后用 `verify-package` 自检。

### 3.5 各阶段 Node 禁令

| 阶段 | 允许 | 必须禁止 |
| --- | --- | --- |
| provisioning | 显式 `provision` 命令、HTTPS、系统 tar/gzip/cargo | `node`/`npm`/`npx`/`pnpm`/`yarn`；隐式下载 |
| build/package | `cargo`、`rustc`、`cc`、`tar`、`gzip`、`sha256sum` | npm install/build、Node 脚本、`setup-node` |
| test | `cargo test`、真实原生资产、无 Node PATH | Node 测试进程、用 PATH shim 冒充通过 |
| install/upgrade/rollback | 打包的 Rust 安装器 | Node、npm、任何 JS 运行时 |
| runtime | Rust LSP + `engine/lib/tsc` + d.ts | Node、`server.js`、`node_modules`、JS 插件 |
| Zed | wasm 扩展 + `WXML_LSP_BIN` | 扩展内下载、Node 启动、Node 环境转发 |
| 用户项目 | 已安装的二进制/类型资产 | 在用户项目运行 npm/Node |

`WXML_TEST_NODE=1` 不再是门禁条件；缺少原生资产必须让相关测试**失败**，不能静默 skip。

迁移期在**隔离历史基线**环境运行旧 Node 后端生成 oracle，是 ADR-0008 允许的临时比较手段；它
不属于本交付链，不得进入本仓库构建、最终测试、发布包或运行路径。本合同的“无 Node 测试”指最终
门禁测试必须在无 Node PATH 下完成；oracle 产物只能作为独立的历史输入，不能作为安装/运行依赖。

## 4. Manifest v2 与 Index v2

### 4.1 Manifest v2 字段

Manifest 使用 `serde(rename_all = "camelCase", deny_unknown_fields)`；任何未声明字段必须拒绝。
文件上限 4 MiB。所有路径为包相对、`/` 分隔、无 `..`/绝对/反斜杠/控制字符。

| 字段 | 类型 | 必需 | 约束 |
| --- | --- | --- | --- |
| `schemaVersion` | integer | 是 | 必须为 `2`；其他值 fail-closed |
| `package` | string | 是 | 必须为 `wxml-lsp` |
| `packageVersion` | string | 是 | semver；必须等于 release 版本 |
| `platform` | string | 是 | 本发布必须为 `linux-x86_64` |
| `protocolVersion` | integer | 是 | Rust↔engine RPC/帧协议；本轮支持 `2`（F0 草案） |
| `builtAt` | string | 是 | UTC RFC 3339 |
| `engine` | object | 是 | 见下 |
| `typeAssets` | array | 是 | 至少标准库与 miniprogram 两项 |
| `binaries` | array | 是 | 见下 |
| `files` | array | 是 | 全部 payload 文件；不含 `manifest.json`/`SHA256SUMS` |
| `engine.name` | string | 是 | `typescript` |
| `engine.version` | string | 是 | 精确 `7.1.0-beta`；禁止浮动版本 |
| `engine.sourceCommit` | string | 是 | 40 位小写 hex；`gitHead` |
| `engine.distribution` | string | 是 | `@typescript/typescript-linux-x64` |
| `engine.distributionUrl` | string | 是 | 固定 HTTPS tarball URL，**仅 provenance**；安装器不得直接请求 |
| `engine.distributionIntegrity` | string | 是 | `sha512-...`；必须等于 `assets.lock.json` 的 SRI |
| `engine.distributionSha256` | string | 是 | 上游 tarball SHA-256；不得用 SHA-1 `dist.shasum` 代替 |
| `engine.distributionSize` | integer | 是 | packed 字节数；必须等于 lock |
| `engine.distributionFileCount` | integer | 是 | 上游 tarball 文件数；本轮 `122` |
| `engine.distributionUnpackedSize` | integer | 是 | 上游 tarball 解包字节数；本轮 `28450796` |
| `engine.apiSchemaVersion` | integer | 是 | AST/API schema；F0 草案冻结为 `9` |
| `engine.astHeaderBytes` | integer | 是 | AST header 字节数；F0 草案冻结为 `64` |
| `engine.license` | string | 是 | `Apache-2.0` |
| `engine.licenseFiles` | array | 是 | 必须包含 `engine/LICENSE`、`engine/NOTICE.txt` |
| `engine.binary` | string | 是 | **nativeEngine 路径**；本轮固定 `engine/lib/tsc` |
| `engine.probe.args` | array | 是 | 本轮固定 `["--version"]` |
| `engine.probe.expectPrefix` | string | 是 | F0 草案冻结为 `"Version "`；与 `engine.version` 拼接后必须精确等于实际输出 |
| `typeAssets[].name` | string | 是 | 见下 |
| `typeAssets[].version` | string | 是 | 精确版本 |
| `typeAssets[].license` | string | 是 | `Apache-2.0` 或 `MIT` |
| `typeAssets[].root` | string | 是 | 包相对目录 |
| `typeAssets[].include` | array | 是 | 相对 root 的 glob；`*` 不跨目录，`**` 递归 |
| `typeAssets[].fileCount` | integer | 是 | 匹配文件数，必须与实际一致 |
| `typeAssets[].sha256` | string | 是 | 匹配文件的 bundle digest，见 §4.5 |
| `typeAssets[].licenseFiles` | array | 是 | 许可文件必须存在并进入 `files[]` |
| `binaries[].kind` | string | 是 | `rust`、`native-engine`、`zed-extension`；**不得**出现 `node-backend` |
| `binaries[].name` | string | 是 | 稳定短名 |
| `binaries[].path` | string | 是 | 包相对路径；必须在 `files[]` 中且哈希一致 |
| `binaries[].version` | string | 是 | `rust` 等于 `packageVersion`；`native-engine` 等于 `engine.version`；Zed 等于 `extension.toml` 版本 |
| `binaries[].protocolVersion` | integer | 否 | 仅 `native-engine` 必填，且等于顶层 `protocolVersion` |
| `binaries[].sha256` | string | 是 | 必须等于同路径 `files[]` 的 SHA-256 |
| `files[].path` | string | 是 | 包相对、唯一、大小写敏感 |
| `files[].sha256` | string | 是 | 64 位小写 hex |
| `files[].size` | integer | 是 | `1..=128 MiB` |
| `files[].mode` | string | 是 | 仅 `0644` 或 `0755` |

`engine.binary` 就是本合同的 nativeEngine 路径；**不另设** `nativeEngine` 字段，避免双重真相。
`binaries[]` 中 `kind=native-engine` 的 `path` 必须等于 `engine.binary`，`sha256` 必须与
`files[]` 同路径一致。

Artifact URL/SRI 的归属：上游 engine tarball 的 **URL** 是 `engine.distributionUrl`，其 **SRI** 是
`engine.distributionIntegrity`；release 归档的 URL 由用户显式 `--source` 基址 + `releases[].archive`
派生（index 不存绝对 URL），其 **SRI** 是 `releases[].archiveIntegrity`，另有 `sha256`/`size`。
`engine.distributionUrl` 只用于 provenance，安装器不得据此发起请求。

`engine.distributionSha256`、`distributionFileCount`、`distributionUnpackedSize`、`apiSchemaVersion`、
`astHeaderBytes` 的值来自 F0 草案的 P1-A 资产证据（AST schema 9/header 64；tarball SHA-256
`59128685…`）。本交付链尚未独立重跑该下载/解包，见 §10 B4；安装时仍以 manifest 与真实文件哈希
比对为准。

**必需 typeAssets**：

| name | version | license | root | include | fileCount |
| --- | --- | --- | --- | --- | ---: |
| `typescript-standard-lib` | `7.1.0-beta` | `Apache-2.0` | `engine/lib` | `["lib*.d.ts"]` | 116 |
| `miniprogram-api-typings` | `5.3.1` | `MIT` | `typings/miniprogram-api-typings` | `["**"]` | 23 |

`typeAssets[].licenseFiles`：标准库为 `engine/LICENSE`、`engine/NOTICE.txt`；
miniprogram 为 `typings/miniprogram-api-typings/LICENSE`。

typings 口径：交付 payload 固定 23 文件/`2,548,075` B，bundle 摘要（按字节序排序的
`<sha256>  <relative>` 行再 SHA-256）为
`ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b`；上游 `5.3.1` 树为
26 文件/`2,552,052` B/`2141aac47206441f0434a7b519e49da1e8756d1bcdb6c17809a7f5e188a34c42`。
`package.json`、`README-en.md`、`typings.json` 不属于 payload：三者都由 `extract.exclude` 排除。
打包器、manifest 校验和安装树校验现在使用**精确的 23 路径 allowlist**（拒绝排除文件、大小写
变体和同数量替换），并重算 payload 的 `fileCount`/总字节/bundle 摘要，与上述三个冻结值比较；
`package.json` 仍同时被运行期负向扫描拒绝。上述 23/26 值是本地未打包树的复算候选值，不是 tarball
provenance；`assets.lock.json` 已按真实 tarball 冻结（F3-03 其余项仍 BLOCKED）。真实 fixture 一致性测试
见 `crates/wxml-delivery/tests/typings_payload.rs`（`#[ignore]`，需要
`WXML_TYPINGS_FIXTURE_DIR`），它不构成 provenance 或 F3-03 关闭证据。

注意：`lib.d.ts` 是基础声明，`lib.*.d.ts` 只匹配 115 个文件，会漏掉它；标准库 include 必须用
`lib*.d.ts` 并断言 `fileCount == 116`。2026-11-17 spike 已实测该差异（见证据 §11）。

**必需 binaries**：

| kind | name | path | version |
| --- | --- | --- | --- |
| `rust` | `wxml-lsp` | `bin/wxml-lsp` | `packageVersion` |
| `rust` | `wxml-lsp-install` | `bin/wxml-lsp-install` | `packageVersion` |
| `native-engine` | `tsgo` | `engine/lib/tsc` | `engine.version` |
| `zed-extension` | `zed-extension` | `zed-extension/extension.wasm` | `extension.toml` 版本 |

强制校验：

- `bin/wxml-lsp` 必须存在、`0755`、`version == packageVersion`；
- `bin/wxml-lsp-install` 必须存在、`0755`、版本一致；
- 恰好一个 `native-engine`，路径/版本/协议/SHA-256 与 `engine` 一致；
- 至多一个 `zed-extension`，路径固定；
- `files[]` 必须包含所有 `binaries[]`、所有 typeAssets 匹配文件、所有 licenseFiles、引擎文件；
- 除 `manifest.json` 和 `SHA256SUMS` 外，包内不得有 `files[]` 之外的文件。

### 4.2 Manifest 示例

以下示例冻结**字段形状**。`engine` 的版本/gitHead/SRI/tarball SHA-256/大小/文件数使用
2026-11-17 F0/P1-A 记录的固定值；`files[]`/`binaries[]` 中的 `aaaa…`/`bbbb…`/`cccc…` 等 64 位
hex 是结构占位值，**不是真实摘要**，真实发布必须由打包器计算。`files[]` 为节选，完整 manifest
必须列出全部文件。

```json
{
  "schemaVersion": 2,
  "package": "wxml-lsp",
  "packageVersion": "0.2.0",
  "platform": "linux-x86_64",
  "protocolVersion": 2,
  "builtAt": "2026-11-17T00:00:00Z",
  "engine": {
    "name": "typescript",
    "version": "7.1.0-beta",
    "sourceCommit": "3e9236361fa97a9e6c685f4833ac457ae2d8c50a",
    "distribution": "@typescript/typescript-linux-x64",
    "distributionUrl": "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz",
    "distributionIntegrity": "sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==",
    "distributionSha256": "59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855",
    "distributionSize": 10175135,
    "distributionFileCount": 122,
    "distributionUnpackedSize": 28450796,
    "apiSchemaVersion": 9,
    "astHeaderBytes": 64,
    "license": "Apache-2.0",
    "licenseFiles": ["engine/LICENSE", "engine/NOTICE.txt"],
    "binary": "engine/lib/tsc",
    "probe": {
      "args": ["--version"],
      "expectPrefix": "Version "
    }
  },
  "typeAssets": [
    {
      "name": "typescript-standard-lib",
      "version": "7.1.0-beta",
      "license": "Apache-2.0",
      "root": "engine/lib",
      "include": ["lib*.d.ts"],
      "fileCount": 116,
      "sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
      "licenseFiles": ["engine/LICENSE", "engine/NOTICE.txt"]
    },
    {
      "name": "miniprogram-api-typings",
      "version": "5.3.1",
      "license": "MIT",
      "root": "typings/miniprogram-api-typings",
      "include": ["**"],
      "fileCount": 23,
      "sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
      "licenseFiles": ["typings/miniprogram-api-typings/LICENSE"]
    }
  ],
  "binaries": [
    {
      "kind": "rust",
      "name": "wxml-lsp",
      "path": "bin/wxml-lsp",
      "version": "0.2.0",
      "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    },
    {
      "kind": "rust",
      "name": "wxml-lsp-install",
      "path": "bin/wxml-lsp-install",
      "version": "0.2.0",
      "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    },
    {
      "kind": "native-engine",
      "name": "tsgo",
      "path": "engine/lib/tsc",
      "version": "7.1.0-beta",
      "protocolVersion": 2,
      "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
    },
    {
      "kind": "zed-extension",
      "name": "zed-extension",
      "path": "zed-extension/extension.wasm",
      "version": "0.0.1",
      "sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
    }
  ],
  "files": [
    {
      "path": "bin/wxml-lsp",
      "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "size": 12345678,
      "mode": "0755"
    },
    {
      "path": "bin/wxml-lsp-install",
      "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "size": 1234567,
      "mode": "0755"
    },
    {
      "path": "engine/lib/tsc",
      "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
      "size": 24522912,
      "mode": "0755"
    },
    {
      "path": "engine/LICENSE",
      "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
      "size": 9197,
      "mode": "0644"
    },
    {
      "path": "engine/NOTICE.txt",
      "sha256": "2222222222222222222222222222222222222222222222222222222222222222",
      "size": 46827,
      "mode": "0644"
    },
    {
      "path": "typings/miniprogram-api-typings/index.d.ts",
      "sha256": "3333333333333333333333333333333333333333333333333333333333333333",
      "size": 1000,
      "mode": "0644"
    }
  ]
}
```

### 4.3 Index v2 字段与示例

Index 最大 1 MiB，`deny_unknown_fields`。`archive` 只存**文件名**；release 归档 URL 由用户显式
提供的 `--source` 基址与 `archive` 拼接。**index 不得包含绝对下载 URL**，避免索引把安装导向
任意主机；`--from-dir` 是显式离线源。

| 字段 | 类型 | 必需 | 约束 |
| --- | --- | --- | --- |
| `schemaVersion` | integer | 是 | 必须为 `2` |
| `package` | string | 是 | `wxml-lsp` |
| `releases` | array | 是 | 至少一项；`version+platform` 唯一 |
| `releases[].version` | string | 是 | semver |
| `releases[].platform` | string | 是 | `linux-x86_64` |
| `releases[].protocolVersion` | integer | 是 | Rust↔engine RPC/帧；与 manifest 一致 |
| `releases[].engineVersion` | string | 是 | 精确 `7.1.0-beta` |
| `releases[].engineSourceCommit` | string | 是 | 40 位 hex |
| `releases[].engineProtocolVersion` | integer | 是 | 与 manifest `protocolVersion` 一致 |
| `releases[].engineApiSchemaVersion` | integer | 是 | 与 manifest `engine.apiSchemaVersion` 一致 |
| `releases[].engineAstHeaderBytes` | integer | 是 | 与 manifest `engine.astHeaderBytes` 一致；本轮 `64` |
| `releases[].engineDistribution` | string | 是 | 平台包名 |
| `releases[].engineIntegrity` | string | 是 | `sha512-...`；与 manifest/lock 一致 |
| `releases[].minInstallerSchema` | integer | 是 | 必须 `<= 2`，否则显式拒绝 |
| `releases[].archive` | string | 是 | 文件名；无 `/`、`..`、URL、query |
| `releases[].archiveIntegrity` | string | 是 | release 归档的 `sha512-...` |
| `releases[].sha256` | string | 是 | release 归档 SHA-256 |
| `releases[].size` | integer | 是 | `1..=512 MiB` |

```json
{
  "schemaVersion": 2,
  "package": "wxml-lsp",
  "releases": [
    {
      "version": "0.2.0",
      "platform": "linux-x86_64",
      "protocolVersion": 2,
      "engineVersion": "7.1.0-beta",
      "engineSourceCommit": "3e9236361fa97a9e6c685f4833ac457ae2d8c50a",
      "engineProtocolVersion": 2,
      "engineApiSchemaVersion": 9,
      "engineAstHeaderBytes": 64,
      "engineDistribution": "@typescript/typescript-linux-x64",
      "engineIntegrity": "sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==",
      "minInstallerSchema": 2,
      "archive": "wxml-lsp-0.2.0-linux-x86_64.tar.gz",
      "archiveIntegrity": "sha512-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
      "sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
      "size": 150000000
    }
  ]
}
```

`archiveIntegrity` 示例为全零 base64 占位，**不是真实值**。安装器必须在解包前同时校验
`archiveIntegrity`、`sha256` 和 `size`；任一不符立即失败，且不得进入解包。

### 4.4 `state.json` 与 transaction journal

`state.json`（`0600` 或 `0644`，原子写入）必须记录：

| 字段 | 类型 | 约束 |
| --- | --- | --- |
| `schemaVersion` | integer | `2` |
| `active` | string | 必须等于 `current` 指向的 `versions/<v>`；必须是已验证 schema v2 |
| `previous` | string \| null | 只允许 `null` 或已验证 schema v2；**禁止** legacy/schema v1 |
| `engine` | object | `name`/`version`/`sourceCommit`/`protocolVersion`/`apiSchemaVersion`/`astHeaderBytes`，必须与 active manifest 一致 |
| `lastTransaction` | object \| null | 最近一次成功激活的事务；字段见下 |

`txn/<id>.json`：

| 字段 | 类型 | 约束 |
| --- | --- | --- |
| `schemaVersion` | integer | `2` |
| `id` | string | `[A-Za-z0-9._-]+`；与文件名一致 |
| `op` | string | `install`、`upgrade`、`rollback` |
| `target` | string | 目标版本 |
| `previous` | string \| null | 操作前 active；legacy 时为 `null` |
| `state` | string | 仅 `staged`、`committed`、`activated` |
| `startedAt` | string | RFC 3339 UTC |
| `updatedAt` | string | RFC 3339 UTC |

`state.json.lastTransaction` 使用同一字段集合，但以 `finishedAt` 表示完成时间；`txn/<id>.json`
使用 `updatedAt` 表示最近一次状态写入时间。两者都必须与 journal 的 `id`/`op`/`state`/`target`/
`previous` 一致。

```json
{
  "schemaVersion": 2,
  "active": "0.2.0",
  "previous": "0.1.0",
  "engine": {
    "name": "typescript",
    "version": "7.1.0-beta",
    "sourceCommit": "3e9236361fa97a9e6c685f4833ac457ae2d8c50a",
    "protocolVersion": 2,
    "apiSchemaVersion": 9,
    "astHeaderBytes": 64
  },
  "lastTransaction": {
    "id": "txn-20261117T000000Z-0001",
    "op": "upgrade",
    "state": "activated",
    "target": "0.2.0",
    "previous": "0.1.0",
    "startedAt": "2026-11-17T00:00:00Z",
    "finishedAt": "2026-11-17T00:00:10Z"
  }
}
```

```json
{
  "schemaVersion": 2,
  "id": "txn-20261117T000000Z-0001",
  "op": "upgrade",
  "target": "0.2.0",
  "previous": "0.1.0",
  "state": "committed",
  "startedAt": "2026-11-17T00:00:00Z",
  "updatedAt": "2026-11-17T00:00:05Z"
}
```

首次原生安装或 legacy 迁移成功后 `previous` 必须为 `null`，不能把旧 Node 版本写成
`previous`；旧版本只在 `list` 中以 `legacy` 标记出现。`state.json` 的 `active` 与 `current`
不一致时，只读命令报 `InstallState`，mutating 命令按 §6.3 恢复，不得静默改写。

### 4.5 校验和格式

- `SHA256SUMS`：每行 `<64 位小写 hex><两个空格><包相对路径>\n`；路径按 UTF-8 字节升序；
  集合精确等于 `files[] ∪ {manifest.json}`；不包含 `SHA256SUMS` 自身；无 CRLF、无注释、无 `./`。
- `sha256sum -c SHA256SUMS` 必须在包根成功。
- typeAssets bundle digest：对 `root` + `include` 匹配的文件，按包相对路径排序，拼接
  `<sha256><两个空格><root 相对路径>\n`（UTF-8，无 BOM），再取 SHA-256；`fileCount` 必须等于
  匹配数。该 digest 写入 `typeAssets[].sha256`，并与 `files[]` 的逐文件哈希独立校验。
- 归档 SRI：`sha512-<标准 base64>`；release 归档由 `archiveIntegrity` 固定；上游 tarball 由
  `assets.lock.json` 的 `integrity` 固定。
- 所有摘要为小写 hex；SRI 为大小写敏感的 base64；不允许省略 padding。

### 4.6 schema v1 → v2 迁移

| v1 字段/行为 | v2 处理 | 迁移规则 |
| --- | --- | --- |
| `schemaVersion: 1` | `schemaVersion: 2` | 旧安装器读到 2 必须 fail-closed（现有行为保持） |
| `node.minimumVersion` | 删除 | 不再要求系统 Node；v2 manifest 含 `node` 视为未知字段并拒绝 |
| `node.backendVersion` | 删除 | 不再有 Node 后端版本 |
| `protocolVersion` | 保留 | 语义改为引擎 RPC 协议；本轮支持 `2` |
| `binaries[].kind: node-backend` | 删除 | 新增 `native-engine`；旧包只由 legacy 读取器识别 |
| 无 `engine` | 新增 `engine` | 版本/gitHead/发行包 URL/SRI/tarball SHA-256/文件数/API schema/AST header/许可/nativeEngine 路径 |
| 无 `typeAssets` | 新增 `typeAssets` | 标准库与 miniprogram 的版本/许可/摘要 |
| `files[]` | 保留 | 字段不变；新增引擎、d.ts、typings、许可、provenance |
| index `nodeMinimumVersion` | 删除 | 新增 engine 字段、`minInstallerSchema`、`archiveIntegrity` |
| 无 state/journal | 新增 | v1 root 可无 `state.json`；首次原生安装后创建 |
| `env` 输出 `WXML_TYPE_BACKEND` | 删除 | 只输出 `WXML_LSP_BIN`；Zed 删除三个转发变量 |

**受限 legacy 读取器**：

- 只读取 `current` 的 symlink 目标和 `versions/<v>/manifest.json` 的**最小投影**
  （`schemaVersion`、`package`、`packageVersion`、`platform`）；不执行旧二进制、不启动 Node、
  不解析/执行旧 `server.js`、不加载 `node_modules`。
- 仍校验路径约束（相对、无 `..`、在 `versions/` 内、非 symlink 逃逸）；读取大小上限。
- `schemaVersion != 1`、缺字段、未知 package、路径逃逸 → 明确拒绝；不得回退到其他版本。
- legacy 目录**只读保留**，不得删除、重命名、chmod 或写入；不得作为 rollback 目标。

**迁移事务**：

1. 识别旧 `current`（只读），记录旧树 digest（文件集合 + 每文件 SHA-256 + 模式）和 `current` 目标；
2. 按 §5 获取/校验 v2 包；staged 探针通过后 `rename` 到 `versions/<v2>`；
3. 原子切换 `current` 到 v2；写 `state.json`（`active=v2`、`previous=null`）；
4. 成功后旧目录保留为非活动历史材料；`list` 标记 `legacy`；
5. switch 前任一步失败/中断：旧 `current` 和旧树字节不变；不写指向未验证版本的 state；不安装
   Node。switch 后失败：对已验证 target 前滚补 state；旧树字节不变；永不恢复/执行 Node。

### 4.7 限制与验证

沿用现有交付预算，并在 v2 明确：

| 项 | 上限 |
| --- | ---: |
| release 归档 | 512 MiB |
| `index.json` | 1 MiB |
| `manifest.json` | 4 MiB |
| 单文件 | 128 MiB |
| 解包总量 | 1 GiB |
| 文件数 | 8192 |
| 路径总字节 | 4096 |
| 路径组件数 | 32 |
| 单组件字节 | 255 |
| `state.json` / journal | 64 KiB |

验证规则：路径唯一、大小写敏感；版本为合法 semver；`platform` 精确匹配；SHA-256 为 64 位
小写 hex；SRI/base64 合法；`files[]` 不得重复；`binaries[]` 路径必须存在于 `files[]`；
`SHA256SUMS` 精确集合；未知字段拒绝；manifest/index 中不得出现 URL 作为下载源（`engine.distributionUrl`
仅 provenance，不参与获取）。

## 5. 获取、解包与校验算法

### 5.1 两个信任边界

```text
[构建机] provisioning（唯一允许联网的构建步骤）
  assets.lock.json --HTTPS--> tarball --SRI sha512--> 安全解包 --逐文件 sha256--> 内容寻址缓存

[用户机] install/upgrade/rollback（显式命令）
  source/index --选择精确版本--> archive --SRI + sha256 + size--> 安全解包
    --verify tree + 探针--> fsync/journal --rename--> 原子切换 current
```

- provisioning 的输入只能是仓库内提交的 `assets.lock.json`；不得解析 `dist-tags`/`latest`。
- install 的输入只能是用户显式 `--source`/`--from-dir`/缓存；不得读取项目文件、workspace 配置、
  `.zed/settings.json` 或环境变量来决定下载源。
- runtime 只执行 manifest 声明的 `engine/lib/tsc`；不查 PATH、不查 cwd、不执行 JS。

### 5.2 源策略

- 远端只接受 `https://`；`http://localhost`、`http://127.0.0.1`、`http://[::1]` 仅测试 fixture。
- 拒绝 userinfo、query、fragment、`..` 路径段、反斜杠、控制字符、非 HTTPS 默认端口（测试
  loopback 除外）。
- redirect 仅允许同源，最多 3 次；跨源 redirect 明确失败；不得携带凭据。
- `--from-dir` 必须是绝对路径；归档必须是普通文件；拒绝 symlink/硬链接/设备/目录替换。
- 不允许 `--insecure`、代理凭据、或从 index 读取绝对 URL。

### 5.3 构建机 provisioning（`assets.lock.json`）

`assets.lock.json` 已冻结，见仓库根 `assets.lock.json`，至少包含：

```json
{
  "schemaVersion": 1,
  "assets": [
    {
      "name": "typescript-native-engine",
      "npmPackage": "@typescript/typescript-linux-x64",
      "version": "7.1.0-beta",
      "sourceCommit": "3e9236361fa97a9e6c685f4833ac457ae2d8c50a",
      "tarball": "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz",
      "integrity": "sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==",
      "sha256": "59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855",
      "size": 10175135,
      "license": "Apache-2.0",
      "extract": {
        "stripPrefix": "package/",
        "include": ["lib/**", "LICENSE", "NOTICE.txt", "package.json", "README.md"],
        "executables": ["lib/tsc"]
      }
    },
    {
      "name": "miniprogram-api-typings",
      "npmPackage": "miniprogram-api-typings",
      "version": "5.3.1",
      "tarball": "https://registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.1.tgz",
      "integrity": "sha512-2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow==",
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "size": 449429,
      "license": "MIT",
      "upstream": {
        "fileCount": 26,
        "unpackedSize": 2552052,
        "treeDigest": "<tree_fingerprint of upstream.files[]; not the 2141aac bundle digest>"
      },
      "extract": {
        "stripPrefix": "package/",
        "include": ["index.d.ts", "types/**", "LICENSE", "README.md", "CHANGELOG.md"],
        "exclude": [
          {
            "path": "package.json",
            "reason": "npm package metadata; runtime policy forbids non-engine package.json"
          },
          {
            "path": "README-en.md",
            "reason": "English translation; not part of the frozen payload allowlist"
          },
          {
            "path": "typings.json",
            "reason": "legacy typings-tool metadata; not read by TypeScript 5/7"
          }
        ],
        "executables": []
      },
      "payload": {
        "fileCount": 23,
        "totalBytes": 2548075,
        "bundles": [
          {
            "name": "miniprogram-api-typings",
            "sha256": "ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b"
          }
        ]
      }
    }
  ]
}
```

`miniprogram-api-typings` 示例中的 tarball `sha256` 全 0 仍是结构占位，**不是真实摘要**；
provisioning 必须计算并写入真实 tarball SHA-256，发布前不得保留占位值。`upstream.treeDigest`
与 `payload.bundles[].sha256` 是本地未打包 5.3.1 树的只读复算候选值，不是 tarball provenance；
它们只有在真实 tarball 的 SRI/SHA-256/size 与逐文件清单核对后才可写入 lock。打包器/安装器只消费
`payload` 与 `extract.include`，不读取 `upstream.files[]`。

`miniprogram-api-typings` 的 3 个 `extract.exclude` 路径不进入 payload，也不进入 manifest
`files[]`/`typeAssets[].fileCount`；其中 `package.json` 若出现在输入树或安装树，还会被
`forbidden_runtime_path` 拒绝。

算法：

1. 只读取 lock；对每个 asset 校验 `https`、无 query/fragment/`..`、版本精确、SRI 格式合法、
   `sha256` 为 64 位小写 hex。
2. `GET` tarball，限制大小；计算 SHA-512 与 `integrity` 比较，计算 SHA-256 与 `sha256` 比较，
   并核对 `size`；随后把 SHA-256/大小写入 provenance。
3. 解包到 `<cache>/assets/tmp-<nonce>`：只允许顶层 `package/`；`stripPrefix=package/`；拒绝绝对路径、
   `..`、反斜杠、控制字符、重复路径、symlink/hardlink/FIFO/设备；限制文件数/单文件/总量/扩展头；
   模式归一化（默认 `0644`，仅 `executables` 为 `0755`）；`include` 之外的文件丢弃并记录数量，
   `extract.exclude` 必须逐条匹配并记录原因。
4. 逐文件 SHA-256/大小/模式写入 `provenance.json`，包括包名、版本、`gitHead`、SRI、`dist.shasum`、
   归档 SHA-256/大小、文件数/总字节、下载源和下载时间。
5. 原子 `rename` 到 `<cache>/assets/<sha256>/`；缓存命中时**重新逐文件校验**，不信任目录名。
6. `--offline` 只读缓存；缺失/损坏 fail-closed，绝不隐式下载。

### 5.4 用户机获取

1. 解析 `--source`/`--from-dir`；读取 `index.json`（上限 1 MiB，schema 2，`package=wxml-lsp`）。
2. 选择**精确** `version` + `platform`；拒绝多义/缺失；检查：
   `protocolVersion`、`engineVersion`、`engineSourceCommit`、`engineProtocolVersion`、
   `engineApiSchemaVersion`、`engineAstHeaderBytes`、`minInstallerSchema <= 2`。
3. 归档来源：缓存命中则重新校验；否则仅当显式 `--enable-download` 才 HTTPS 获取；`--offline`
   时只用缓存/`--from-dir`。
4. 下载后先计算 `archiveIntegrity`（SHA-512 SRI）并比对，再计算 SHA-256/大小比对 index；
   任一失败在解包前终止；不得把归档写入缓存/版本目录。
5. 解包前 `prescan_tar` 检查扩展头/条目预算；解包到 `tmp/<txn>/tree`。

### 5.5 安全解包

只允许普通文件和目录。必须拒绝：

- 绝对路径、盘符、UNC、`..`、`.`、空组件、反斜杠、NUL/控制字符、超长路径；
- symlink、hardlink、FIFO、socket、字符/块设备、setuid/setgid/sticky；
- 重复路径、文件/目录同名、大小写冲突（在大小写不敏感文件系统上预先拒绝）；
- 组/其他可写、非 `0644`/`0755` 的模式；
- 超过 §4.7 的文件数/单文件/总量/扩展头预算；
- `include` allowlist 之外的文件（正式包不允许“丢弃”）。

npm tarball 的 provisioning 解包器可以使用更宽松的 `package/` 前缀和模式归一化，但**不得**用于
正式 release 包安装；正式包只接受 §3.1 的精确树。

### 5.6 树校验（sha256 allowlist）

按以下顺序，任一失败即拒绝：

1. `manifest.json` 大小/JSON/schema 2/`deny_unknown_fields`；
2. `ExpectedManifest`：`packageVersion`、`platform`、`protocolVersion` 与请求一致；
3. `files[]` 路径安全、唯一、模式仅 `0644/0755`、大小上限；
4. 实际树精确集合 = `files[] ∪ {manifest.json, SHA256SUMS}`；无额外文件/目录/链接；
5. 逐文件 SHA-256、大小、模式匹配；
6. `SHA256SUMS` 精确等于 `files[] ∪ {manifest.json}` 的重新计算值；
7. `binaries[]` 与 `files[]` 的路径/SHA-256 一致；`engine.binary`、native-engine、typeAssets 一致；
8. 必需文件存在：`engine/lib/lib.es5.d.ts`、`engine/lib/lib.es2020.d.ts`、typings `index.d.ts`、
   `engine/LICENSE`、`engine/NOTICE.txt`、typings `LICENSE`；
9. 负向扫描：禁止 `node_modules/`、`bin/wxml-type-backend/`、`*.js`/`*.cjs`/`*.mjs`、
   `package-lock.json`/`yarn.lock`/`pnpm-lock.yaml`；可执行文件仅 allowlist 三个；
10. `provenance.json`、`THIRD_PARTY_LICENSES.md` 存在且已被 `files[]` 覆盖。

### 5.7 探针

探针必须在 **staged 树**上执行，绝不对已安装旧版执行；失败不切换。

1. **Rust 二进制**：`<staged>/bin/wxml-lsp --version` 与
   `<staged>/bin/wxml-lsp-install --version`，各 10 秒超时；输出必须分别为
   `wxml-lsp <packageVersion>` 与 `wxml-lsp-install <packageVersion>`；退出码 0；
   非 UTF-8/超时/非零 → `Runtime`。
2. **原生引擎版本**：`<staged>/engine/lib/tsc --version`，10 秒超时；`cwd` 为
   `<staged>/engine`；`env_clear` 后仅保留 `LANG=C`/`LC_ALL=C`（以及运行所需的 loader 变量，
   如果有，必须记录）；移除 `NODE_OPTIONS`、`NODE_PATH`、`TS_NODE_PROJECT` 等；stdout 去尾随
   空白后必须精确等于 `engine.probe.expectPrefix + engine.version`（本轮
   `Version 7.1.0-beta`），不能只做宽松前缀匹配。
3. **API/schema 握手**：按 F0 草案冻结的帧格式启动 `engine/lib/tsc --api --async`，`--cwd` 使用
   staged 树之外的**空临时目录**（不得是项目目录/引擎目录），发送初始化与 F0 的最小能力探针；
   `initialize` 结果形状必须符合 F0（`caseSensitivity == 1`，且不出现
   `useCaseSensitiveFileNames`），必需方法必须存在。注意 `protocolVersion` 是 Rust 侧帧/路由
   合同版本，engine 的 `initialize` **不返回协议版本**；安装器只校验 manifest/index/二进制身份
   的 `protocolVersion` 一致。随后用最小虚拟文件解码 AST header：长度
   `engine.astHeaderBytes`、metadata 版本 `9`、字段自洽；然后关闭进程。不得发送项目查询、不得
   打开项目文件、不得启用 `--runExternalCode`、不得调用 emit/build/clean/plugin 方法。
   超时/非零/方法缺失/header 不符 → 拒绝。
4. **类型资产**：标准库基准文件与 typings `index.d.ts` 存在；typeAssets bundle digest 匹配；
   许可文件存在。
5. **可选 `tsc.sig`**：本轮只要求随包保留并哈希；F0 明确它**未验签**，不得声称已签名验证；
   是否运行期必需由 F0 对照实验决定，不能在本合同中假定必需或可删。

探针进程不得继承 `WXML_NODE`、`WXML_TYPE_BACKEND`、`WXML_BACKEND_DIR`，不得读取用户项目
`tsconfig`/plugins。超时/失败按 F0 的关闭顺序处理：close stdin → 等待 → SIGKILL → reap，不能
只发 SIGTERM 或留下孤儿进程。

### 5.8 原子暂存与提交

1. `lock(root)`（OS 文件锁，mutating 命令独占）；
2. 读取 `state.json`（可选）与 `current`；记录 `previous`；
3. 获取/校验/解包/探针全部在 `tmp/<txn>/tree`；
4. 写 journal `{state:"staged"}` 并 `fsync(tmp)`/`fsync(root)`；
5. `rename(tmp/<txn>/tree, versions/<target>)`；`fsync(versions)`；
6. 写 journal `{state:"committed"}` 并 `fsync(txn)`；
7. 在 root 创建临时 symlink `current.tmp-<txn>` → `versions/<target>`，`rename` 为 `current`；
   `fsync(root)`；
8. 写 `state.json`（active/previous/engine/lastTransaction）原子替换；`fsync(root)`；
9. 写 journal `{state:"activated"}`；清理 `tmp/<txn>` 与 journal；
10. `unlock`。

任何一步失败：按 §5.9/§6.3 清理；`current` 不变，除非第 7 步已经原子完成。第 7 步后的
失败必须显式报告：仅当 `state.json` 仍指向已验证的 native `previous` 时切回（不重写 state）；
若新 state 已写入，则对已验证 target 前滚补 state/journal；legacy/Node current 永不恢复。

### 5.9 失败清理

- 解包/校验/探针失败：删除 `tmp/<txn>`；不创建 `versions/<target>`；`current` 不变；缓存中
  已验证归档可保留（下次重新校验），未验证归档必须删除。
- `rename` 前失败：journal 若已写 `staged`，下次 mutating 命令删除 tmp；只读命令不清理。
- `rename` 后、switch 前失败：保留 `versions/<target>` 与 journal `committed`；**不自动激活**；
  由显式 install/upgrade/rollback 重新校验后再 switch。
- switch 后失败：若 `state.json` 仍指向已验证 native `previous`，切回且不重写 state；若 state
  已指向 target，不切回，重新校验 target 后前滚补 state/journal；legacy 迁移只前滚，永不恢复
  Node。无法协调时返回 typed `SwitchedButNotDurable`（含 recovery）或 `InstallState` 诊断，
  不启动 Node、不删除版本目录。
- 失败事务不得留下 `state.json` 指向未验证版本；若 `state.json` 原本不存在，失败后也不得创建
  指向未验证版本的 state。

## 6. 安装/升级/回滚状态机

### 6.1 状态

| 状态 | 含义 |
| --- | --- |
| `S0` | 空安装（无 root/current） |
| `S1` | legacy current：schema v1，可能仍依赖旧 Node |
| `S2` | 原生 current `N1`（schema v2，已验证） |
| `S3` | 原生 `N1` + 已验证 staged `N2`（journal `staged`） |
| `S4` | 原生 `N2` 已 rename 到 `versions/`，未激活（journal `committed`） |
| `S5` | 原生 `N2` 已激活（journal `activated`） |
| `S6` | rollback 目标 `N1` 已验证/暂存 |
| `SX` | 不一致/损坏：`current` 悬空、state 与 current 不符、未知 schema、篡改 |

### 6.2 转换

| 起始 | 动作 | 结果 | 必须保持 |
| --- | --- | --- | --- |
| `S0` | install native 成功 | 校验/探针通过 → 原子激活 | 失败不留 current/半成品 |
| `S0` | install native 失败/中断 | 明确错误 | 无 `current`；tmp 清理 |
| `S1` | install native 成功 | 只读识别旧包；staged 校验通过后一次 switch | 旧树与旧 `current` 在切换前字节不变；全过程无 Node |
| `S1` | install native 失败/中断（switch 前） | 报“尚未迁移” | 旧 `current` 与旧树字节不变；不安装 Node、不修改旧树 |
| `S1` | install native 失败/中断（switch 后） | 已验证 target 前滚并补 state；返回显式恢复状态 | 旧树字节不变；`current` 为已验证 target；永不恢复/执行 Node |
| `S1` | activate/rollback legacy | **明确拒绝** | `current` 不变；不执行旧二进制 |
| `S2` | upgrade `N1→N2` 成功 | staged 校验/探针 → switch | `state.previous=N1`；资产同属一个 manifest |
| `S2` | upgrade `N1→N2` 失败/中断（switch 前） | 保留 `N1` | `current=N1`；N2 不得成为 current；无混合资产 |
| `S2` | upgrade `N1→N2` 失败/中断（switch 后） | state 仍指 `N1` 则切回；state 已指 `N2` 则前滚补 state | `current` 为已验证 `N1` 或 `N2`；不得出现 current/state 分裂；无混合资产 |
| `S3` | 恢复 | 删除 tmp，或重新校验后继续 | `current` 不变；不自动激活 |
| `S4` | 恢复/继续 | 重新校验 N2；通过才显式 switch；不通过保持 N1 | 不出现半安装 current |
| `S5` | rollback `N2→N1` 成功 | 重新完整校验/探针后 switch | 离线也满足完整性；`state.previous=N2` |
| `S5` | rollback 目标损坏/失败 | 拒绝 | `current=N2`；目标目录不改写 |
| `S5` | 无可用原生回退 | 显式错误/静态能力降级 | 不自动激活 Node 包 |
| 任意 | 未知 schema/篡改/错误平台 | 执行前拒绝 | 已验证安装不受影响 |
| 任意 | 并发 mutating | OS 文件锁串行化 | 不产生混合资产/悬空 current |

`upgrade` 默认拒绝降级；降级只能通过显式 `rollback --to`。同版本同内容为幂等 no-op；
同版本不同内容不得静默覆盖，必须显式错误。

### 6.3 中断/崩溃窗口

| 窗口 | 恢复规则 |
| --- | --- |
| 获取/解包/校验中 | 仅 tmp 变化；下次 mutating 清理 tmp；current 不动 |
| journal `staged` 后、rename 前 | 删除 tmp；current 不动；不自动激活 |
| rename 后、switch 前（`committed`） | 目标目录保留；current 仍 previous；**不自动激活**；显式命令重新校验后再切换 |
| switch 中 | symlink 临时名 + `rename` 原子；要么 previous 要么 target；按 journal/current 协调 |
| switch 后、state 写入前 | 若 state 仍指 previous 且 previous 已验证：切回且不重写 state；若 state 已指 target：不切回，重新校验后前滚；其余报 `InstallState`/`SwitchedButNotDurable` |
| current 为已验证 native，state 指向另一版本 | 有 committed/activated journal 且 target 可验证时，重新校验 current/target 后前滚到 target；否则报错；恢复幂等 |
| switch 后 state 写入失败 | 返回 typed `SwitchedButNotDurable`，不泄漏裸 I/O；按 state/journal 协调 |
| state 写入后、cleanup 前 | 幂等清理；重复命令不改变 active |
| journal `activated` 但 current != target | `InstallState` 错误；要求显式 rollback/install，不静默修正 |
| 断电/fsync 失败 | 按 I/O 错误报告；断电持久性未证明（见 §10） |

恢复规则固定：

- `staged` → 删除 `tmp/<txn>`，`current` 不动；
- `committed` → 保留 `versions/<target>`，**不自动激活**；下次显式 mutating 命令重新校验。
  例外：若 `state.json` 已指向 target 且 current 是另一个已验证 native，recovery 重新校验
  current/target 后前滚到 target，修复 current/state 分裂；
- `activated` → 若 current 指向 target，幂等确认并补 state；否则报 `InstallState`。

### 6.4 不变量

- **I1**：`current` 只在 staged 树完成 manifest/SHA-256/资产/原生探针之后改变。
- **I2**：switch 前任一步失败或中断，旧 `versions/<old>` 与 `current` **字节不变**（文件集合、
  每文件 SHA-256、模式、current 目标均不变）。switch 后失败只允许：state 仍指 previous 时切回
  已验证 native previous（不重写 state），或对已验证 native target 前滚补 state；legacy 树字节
  不变且永不恢复/执行。
- **I3**：`N1→N2` 在 switch 前失败保持 `N1`，N2 不得成为 current；switch 后失败只能在 state
  仍指 `N1` 时切回 `N1`，或 state 已指 `N2` 时前滚 `N2`；不得出现引擎/typings/二进制来自不同
  manifest 的混合资产。
- **I4**：rollback 只接受 schema v2 且完整校验/探针通过的目标；**永不**重新激活 schema v1/Node 包。
- **I5**：任何路径都不执行 `node`/`npm`/`npx`/`pnpm`/`yarn`；不查 PATH 找引擎；
  `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` 不参与激活。
- **I6**：离线缺缓存/缓存损坏 → 明确失败；`current` 不变；不发网络请求。
- **I7**：同一 root 的并发 mutating 命令由 OS 文件锁串行化；只读命令不加锁、不创建 root。
- **I8**：包内不得存在 Node 运行时文件（`node_modules/`、运行期 JS）；安装器不得执行它们。
- **I9**：`current` 必须始终指向存在的、已验证的 `versions/<v>`；不得悬空；发现不一致时
  只读命令报错，mutating 命令按 §6.3 恢复。
- **I10**：`versions/<v>` 不可原地修改；激活前必须重新校验；不得用缓存/目录名代替内容哈希。

### 6.5 命令语义

| 命令 | 语义 |
| --- | --- |
| `install --root <p> --version <v> (--source <https> --enable-download \| --from-dir <abs> \| --offline)` | 安装精确版本并激活；active 为 v2 且目标更低时拒绝；同版本不同内容拒绝 |
| `upgrade --root <p> (--source ... \| --from-dir ... \| --offline)` | 选择大于 active 的版本；失败保持 active；默认拒绝降级 |
| `rollback --root <p> --to <v>` | 目标必须已安装、schema v2、完整校验/探针通过；离线也必须满足完整性 |
| `verify --root <p>` | 校验 current 树 + 原生探针 + typeAssets；不联网、不切换 |
| `current --root <p>` | 打印版本/platform/engine 身份；不执行探针、不联网 |
| `list` / `versions` | 列出原生与 legacy；legacy 标记只读；不执行、不修改 |
| `env` | 仅打印 `export WXML_LSP_BIN=<root>/current/bin/wxml-lsp`；不再输出 `WXML_TYPE_BACKEND` |
| `verify-package --from-dir <abs> --version <v>` | 只校验 release 目录/归档；不安装、不切换 |

`WXML_LSP_BIN` 优先级：有效时 `install`/`upgrade`/`rollback` 必须跳过（不下载、不覆盖）并明确
报告该路径优先，`env` 只打印该路径；无效时 hard error，绝不回退到已安装版本或网络。
`WXML_TYPE_BACKEND` 不再是激活入口；设置它不得改变后端选择。

若 `rollback --to` 的目标未安装，必须明确失败；本轮不提供“降级 install”，需要维护者显式恢复
目标版本目录或重新构建该版本，不得用删除/覆盖当前 native 版本的方式绕过。

### 6.6 legacy 迁移专项

- 读取 legacy 不要求 Node；不得执行旧 `bin/wxml-lsp` 或旧 `server.js`。
- 旧 current 在 switch 前保持不变；switch 前失败报“尚未迁移”，旧树字节不变。
- switch 后失败：对已验证 native target 前滚补 state；永不恢复/执行旧 Node；旧树字节不变。
- 迁移成功只切换一次 current；旧目录保留为非活动历史材料，`state.previous=null`。
- 迁移不修改 Zed 用户配置或 `.zed/settings.json`；安装器只打印稳定路径，由用户显式设置
  `WXML_LSP_BIN`。
- 新安装器请求激活/rollback 到 legacy：明确拒绝（“legacy Node package is not a Node-free target”），
  不改变 current，不删除旧材料。
- 旧环境是否还能运行取决于它原来的 Node；这不是新包无 Node 验收成功，不得据此宣称通过。

### 6.7 无自动 Node 回退

- 原生引擎缺失/损坏/探针失败：显式错误或可观察的静态能力降级；不得启动 Node、不得探测
  `server.js`、不得查找 PATH 中的 `node`。
- 没有可用原生回退时：保留诊断和人工恢复指引；不自动激活历史 Node 包。
- `WXML_NODE` 不再作为默认 PATH 查找；`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` 不再转发；
  Zed 只通过 `WXML_LSP_BIN` 启动。
- 任何“Node fallback”都视为违反 I5/I8，F3-02/F3-15 必须失败。

## 7. 离线、平台与 ABI

### 7.1 离线矩阵

| 命令/阶段 | 网络 | 允许来源 |
| --- | --- | --- |
| `install`/`upgrade`/`rollback` 无 `--offline` | 仅显式 `--enable-download` | HTTPS source 或 `--from-dir` |
| 同上 + `--offline` | **禁止** | 缓存或 `--from-dir`；缺失即失败 |
| `list`/`versions`/`current`/`verify`/`env` | **禁止** | 本地安装根 |
| `verify-package` | 仅 `--from-dir` 时禁止网络 | 本地 release 目录/归档 |
| 构建/测试 | 依赖已 provisioning；`cargo --offline` | 本地 Cargo/资产缓存 |
| LSP 启动/运行 | **禁止** | 已安装 `current` + manifest |
| Zed 扩展 | **禁止** | `WXML_LSP_BIN` |

离线验证必须证明没有 DNS/连接尝试，而不只是“命令成功”。推荐 `unshare -rn` 或
`HTTPS_PROXY=http://127.0.0.1:9` 加连接审计；缺失工具时报告 blocked，不能算通过。

### 7.2 预置资产

- 构建机：`assets.lock.json` + `<cache>/assets/<sha256>/`；缓存命中重新逐文件校验。
- CI：原生资产/cargo 缓存预置；cache miss 必须显式失败，不得在测试 job 隐式下载。
- 用户机：release 目录（`index.json` + archive + sidecar）可整体作为 `--from-dir`；离线安装
  也必须完成完整校验/探针。
- 启动/构建/测试不得隐式下载；远端获取只在显式 provisioning/下载命令中发生。

### 7.3 平台支持

- 本发布只支持 **`linux-x86_64`**，且仅 glibc。
- `platform` 必须精确等于 `linux-x86_64`；`host_platform()` 不匹配时在任何下载/解包/切换前
  返回 `UnsupportedPlatform`。
- aarch64：即使代码可构建，也没有本合同的发布/验收证据；本轮不支持。
- macOS/Windows：不支持；不发布对应包，不宣称兼容。
- 其他 Linux 架构（arm、riscv64、s390x 等）：不支持。
- 不允许通过 qemu/Rosetta/兼容层“碰巧运行”来声称支持；host 架构不符即拒绝。

### 7.4 glibc/ABI 边界

- 平台只接受 glibc 用户态；musl 不在本轮支持范围。上游平台包元数据**没有 `libc` 字段**，
  不能据此区分 glibc/musl。
- 上游 `engine/lib/tsc` 按 F0/P1-A 是**静态链接 Go 二进制，无 glibc 动态依赖**；本项目 Rust
  二进制仍按 glibc 目标构建。glibc 下限约束的是 Rust 二进制的动态依赖，不是引擎的 Go 运行时。
- 打包时必须分别记录 Rust 二进制与引擎的 `readelf -h` Class/Machine/Type、`ldd` 结果、
  `readelf --version-info` 的最高 `GLIBC_x.y` 符号版本；引擎的 `ldd` 可能显示
  “not a dynamic executable”，必须如实记录，不能据此推断 Rust 二进制也静态。
- **Rust 二进制的 glibc 下限尚未测量**。在 F3-13 产出真实数据前，本文件不声明具体 glibc
  版本；release README 必须写“glibc 下限未验证”而不是给出猜测值。
- Rust 二进制按 workspace 的 `rust-toolchain.toml`/`Cargo.lock` 在目标机或兼容构建机构建；
  不使用 musl target，不静态链接来掩盖 ABI 差异。
- ABI 为 x86-64 SysV；不支持 i386/multilib 或 32 位用户态。
- 若 Rust 二进制的 loader/glibc 不满足：显式 `Runtime`/`UnsupportedPlatform` 错误，保留
  `current` 不变；不得降级为 JS/Node 后端，不得静默返回空结果。

### 7.5 不支持平台的显式失败

- `install`/`upgrade`/`rollback`：在任何 root 写入/下载前失败，错误包含“unsupported platform”
  和实际 `os-arch`；不创建 root、不写缓存。
- `verify-package`：拒绝 platform 不匹配的包；不执行其中的二进制。
- 运行时：`wxml-lsp` 发现引擎不可执行/loader 失败时显式报错；Zed 显示错误，不启动 Node。
- 错误信息不得把“不支持”伪装成“尚未安装”或“无诊断”。

## 8. 安全与信任

### 8.1 信任根与残留风险

- 信任根：TLS + 仓库内 `assets.lock.json` 的 SRI/大小 + release index 的 SHA-256/SRI +
  manifest 的逐文件 SHA-256 + 路径/集合 allowlist。
- **没有发布者签名**。攻破 HTTPS 源、CA 或恶意 `--from-dir` bundle 的攻击者可以同时提供自洽的
  index/manifest/archive。ADR-0007 明确接受该残留风险；正式发布前应增加 detached 签名
  （如 ed25519/minisign）并新增 ADR，不能静默改变信任根。
- 本轮 release 只在可信本地/CI 生成，只从可信 HTTPS 入口或本地 `--from-dir` 安装。

### 8.2 固定来源

- 上游资产：`assets.lock.json` 固定 URL、版本、gitHead、SRI、大小、许可、extract allowlist；
  禁止 `dist-tags`、`latest`、`next`、浮动版本。
- release：index 固定每个版本的 platform + archive SHA-256/SRI + 引擎身份；安装器不得查询
  dist-tags/latest，也不得选择 index/`--from-dir` 之外的版本。`upgrade` 只能从用户显式提供的
  来源中选择版本，不能隐式跨源。
- 下载源由用户显式 CLI 提供；项目文件、workspace 配置、`.zed/settings.json` 和环境变量不能
  指定待执行下载源。
- 升级引擎/typeAssets 必须作为独立、经过 F0/F1 与许可复核的变更；不得在安装时解析远端元数据。

### 8.3 校验和 allowlist

- 安装树只允许 `files[] ∪ {manifest.json, SHA256SUMS}`；任何额外路径、缺失路径、大小/模式/
  摘要不符立即失败。
- `binaries[]` 只允许 `wxml-lsp`、`wxml-lsp-install`、native-engine、Zed extension；可执行
  文件仅 `bin/wxml-lsp`、`bin/wxml-lsp-install`、`engine/lib/tsc`。
- 禁止 `.js`/`.cjs`/`.mjs`、`node_modules/`、`bin/wxml-type-backend/`、Node lockfile。
- 归档 SRI、SHA-256、大小三重校验；typeAssets bundle digest 与逐文件哈希独立校验。

### 8.4 许可与 provenance

- 引擎 Apache-2.0：必须随包提供 `engine/LICENSE`、`engine/NOTICE.txt`；不得删除/改写。
- `miniprogram-api-typings` MIT：随包 `LICENSE`；保留版权声明。`package.json`、`README-en.md`、
  `typings.json` 不进入 payload：它们分别是 npm 元数据、英文翻译和旧工具元数据，均不含完整 MIT
  许可文本；许可义务由随包 `LICENSE` 承担。
- 本项目/Rust 二进制许可：按仓库现有许可；`THIRD_PARTY_LICENSES.md` 汇总。
- `provenance.json` 必须包含：上游包名/版本/gitHead/SRI/shasum、tarball URL、归档 SHA-256/大小、
  文件数/总字节、逐文件 SHA-256/大小/模式、构建时间、构建环境摘要。
- SBOM 至少由 `Cargo.lock`、`provenance.json` 和 `THIRD_PARTY_LICENSES.md` 追溯 Rust 依赖、
  引擎与类型资产；若引入正式 SBOM 生成器，作为独立变更，不在本轮假装已完成。
- 许可或来源无法复核时不得发布；不得用“registry 有 license 字段”代替随包许可文本。

### 8.5 路径与 symlink 约束

- 包内不允许 symlink/hardlink；tar 解包拒绝链接、设备、FIFO、重复、`..`、绝对路径。
- `current` 只允许相对 `versions/<v>`；解析后必须在 `<root>/versions/` 内；不得指向 root 外。
- `versions/<v>` 名称只允许合法 semver；不得含 `/`、`..`、控制字符。
- LSP 解析 `engine.binary` 后必须 canonicalize，并确认仍在包根内；不得通过 symlink 逃逸。
- 不引入 PATH/cwd 自动搜索；配置只允许已验证安装相对路径或显式绝对路径（`WXML_LSP_BIN`）。
- 安装根/缓存目录使用最小权限；staging `0700`；不使用全局可写临时目录存放可执行文件。

### 8.6 无 Node 环境与 legacy 变量

- 安装器/LSP 不得执行 `node`/`npm`/`npx`/`pnpm`/`yarn`；不得通过 PATH 查找引擎。
- 探针使用 `env_clear`，移除 `NODE_OPTIONS`、`NODE_PATH`、`TS_NODE_PROJECT` 等；只保留稳定
  输出所需的 `LANG`/`LC_ALL`。
- `env` 只输出 `WXML_LSP_BIN`；不得输出 `WXML_TYPE_BACKEND`、`WXML_NODE`、`WXML_BACKEND_DIR`。
- Zed 扩展删除上述三个变量的转发；五语言注册保持；扩展不下载、不解包、不安装。
- `WXML_TYPE_BACKEND` **不得**触发 legacy/Node 激活；即使外部设置，也不改变后端选择、不启动
  Node。`WXML_NODE`/`WXML_BACKEND_DIR` 同理。
- `WXML_LSP_BIN` 必须是绝对路径、存在的普通可执行文件；拒绝相对路径、`..`、目录、缺失或
  不可执行文件。有效值优先：install/upgrade/rollback 跳过安装/下载/覆盖并明确报告；无效值
  hard error，不回退。

### 8.7 引擎覆盖变量

本发布**不提供**引擎覆盖环境变量（不新增 `WXML_TSGO_ENGINE` 等）。产品只执行
`<current>/manifest.json` 的 `engine.binary`；没有 PATH/cwd 搜索。开发/测试通过构造安装根或
直接调用后端 crate 注入显式路径，不进入产品环境。未来若需要覆盖变量，必须单独冻结名称、
绝对路径、版本/摘要校验和禁用条件，并重新做安全评审。

### 8.8 错误与日志

- 错误可包含包相对路径、版本、platform、摘要前缀；不得输出完整环境变量、文件内容、凭据或
  不必要的绝对路径。
- 远端错误只显示源 host/状态码，不回显 query/凭据；不得把 TLS/摘要失败降级为警告。
- 进程 stdout 只用于稳定输出（版本/路径/状态）；日志走 stderr；不得用 stdout 污染 LSP/JSON-RPC。
- 探针输出有界；超时/非 UTF-8/异常长度均失败，不把截断内容当版本。

### 8.9 升级审查

引擎/类型资产升级必须：

1. 更新 `assets.lock.json`（新版本/gitHead/SRI/大小/许可）；
2. 重跑 F0 协议/AST schema 探针与 F1 能力合同；
3. 重新生成 manifest/index/provenance/许可；
4. 重新执行 §9 的 F3 测试（至少离线/回滚/篡改/clean-env）；
5. 作为独立提交评审，不得与业务语义修改混在一起。

## 9. 验收测试 F3-xx

### 9.1 环境

- Linux x86_64；`PATH` 只含构建/安装所需工具，**明确不含** `node`/`npm`/`npx`/`pnpm`/`yarn`。
- 预置 Rust 工具链、Cargo 缓存、原生资产缓存；`HOME`/`XDG_DATA_HOME`/`CARGO_HOME`/
  `CARGO_TARGET_DIR` 隔离到临时 run 目录。
- 离线证明：`unshare -rn` 或不可达代理；记录网络开关与缓存命中条件。
- 进程审计：`strace -f -e trace=execve,execveat` 或等价机制；PATH shim 只作辅助，不能单独证明
  （绝对路径调用可能绕过 shim）。
- 缺少 `strace`/原生资产/构建工具时，测试标记 **BLOCKED**，不得算通过或 skip。

### 9.2 测试矩阵

2026-11-20 逐项审计后，F3 整体仍未通过：16 项中 **8 closed / 6 partial / 2 blocked**（见 §9.5）。
真实资产、包级安装/回滚、ABI 与部分篡改/恶意归档已有实测；`strace` 进程审计、Zed UI、
进程级 kill、缓存升级/回滚、Zed legacy 变量转发和若干边界用例仍缺。下表中的测试文件多数已存在
（早期 `new` 标注是历史计划），但存在不等于通过；`extend` 的文件仍需新增/保留 schema v2 断言。
状态以 §9.5 和证据为准，不能把“文件存在”或“默认测试绿”当作 F3 关闭。

| ID | 验收内容 | 未来测试路径 | NF | 现有覆盖（不可替代） |
| --- | --- | --- | --- | --- |
| F3-01 | 无 node/npm 的干净 PATH 下完成 `install`/`verify`/`current`/`env`；`env` 只输出 `WXML_LSP_BIN`；全程无网络 | `crates/wxml-delivery/tests/native_delivery.rs`（new）`clean_path_install_without_node` | NF-01/02/06 | 无；现有 `verify` 会检查 Node |
| F3-02 | 安装器/LSP 进程审计：`install`/`upgrade`/`rollback` 与 stdio LSP 会话均无 node/npm 子进程；只 exec `wxml-lsp-install`/`wxml-lsp`/`engine/lib/tsc`（及测试驱动） | `crates/wxml-delivery/tests/no_node_guard.rs`（new）+ `scripts/verify-no-node.sh`（new） | NF-01/07 | 无；shim 不能排除绝对路径 |
| F3-03 | 包内容/布局/许可/负向扫描：无 `node_modules`/JS/`wxml-type-backend`；`SHA256SUMS` 通过；两次构建归档 SHA-256 一致 | `crates/wxml-delivery/tests/native_package.rs`（new） | NF-05/09/11 | `packager_cli.rs` 仅 v1 |
| F3-04 | 空安装成功：精确树、原子 current、state.json、tmp 为空、verify 通过 | `crates/wxml-delivery/tests/native_delivery.rs`（new）`installs_native_package_atomically` | NF-06/10 | `installer_flow.rs` 为 v1/Node |
| F3-05 | 安装失败：归档哈希/SRI 不符、缺标准库/typings、引擎探针失败；不产生 current/versions，tmp 清理 | `crates/wxml-delivery/tests/native_delivery.rs`（new）`failed_install_leaves_no_current` | NF-06/10 | `failed_runtime_check...` 仅 v1 |
| F3-06 | `N1→N2` 升级失败/中断：switch 前 N1 digest/current 不变、N2 不激活；switch 后 state 已写则 N2 前滚，state 仍指 N1 则切回 N1；无混合资产 | `crates/wxml-delivery/tests/native_delivery.rs`（new）`upgrade_failure_keeps_n1` + `crates/wxml-delivery/tests/native_rollback.rs`（new）`native_after_state_write_rolls_forward_without_switching_back` | NF-10 | `installer_flow.rs` 部分 v1 |
| F3-07 | 离线回滚到 N1：重新完整校验/探针后切换；current=N1；state.previous=N2；不联网 | `crates/wxml-delivery/tests/native_delivery.rs`（new）`offline_rollback_reverifies_n1` | NF-06/10 | `offline_rollback...` 仅 v1 |
| F3-08 | schema v1 回滚目标明确拒绝；v1 目录字节不变；原生 current 不变；不执行 Node | `crates/wxml-delivery/tests/native_delivery.rs`（new）`rollback_rejects_schema_v1` | NF-07/10 | 无 |
| F3-09 | legacy current → 原生：成功一次切换、旧树字节不变、legacy 标记；switch 前失败/中断旧 current 与旧树不变且无 native state；switch 后失败对已验证 target 前滚补 state，永不恢复/执行 Node | `crates/wxml-delivery/tests/native_rollback.rs`（new）+ `crates/wxml-delivery/tests/native_delivery.rs`（new）`legacy_migration_succeeds_once_and_keeps_legacy_read_only` | NF-10 | 无 |
| F3-10 | 篡改检测：archive SRI/SHA-256、manifest、逐文件哈希、额外/缺失文件、许可文件；全部在激活前拒绝，current 不变 | `crates/wxml-delivery/tests/installer_security.rs`（extend）+ `crates/wxml-delivery/tests/provisioning.rs`（new） | NF-09/10 | `rejects_a_bad_archive_hash` 等仅 v1 |
| F3-11 | 恶意归档：绝对路径、`..`、symlink、hardlink、FIFO/设备、重复、超长/超大；拒绝且不越界解包 | `crates/wxml-delivery/tests/installer_security.rs`（extend）`archive_rejects_traversal_links_and_budget` | NF-09/10 | 仅部分 PAX/redirect 用例 |
| F3-12 | 离线 fail-closed：无缓存安装失败且无网络；有效缓存可安装/升级/回滚；损坏缓存失败；代理 sink 无连接 | `crates/wxml-delivery/tests/native_delivery.rs`（new）+ `scripts/verify-no-node.sh`（new） | NF-04/08 | `offline_install...` 仅 v1 |
| F3-13 | 平台/ABI：错误 platform/arch 在写入前拒绝；记录 `readelf`/`ldd`/`GLIBC_*`；musl 明确不支持；glibc 下限写入证据 | `crates/wxml-delivery/tests/native_delivery.rs`（new）`rejects_unsupported_platform` + `docs/evidence/<run>-tsgo-delivery-acceptance.md`（new） | NF-05/09 | 无 |
| F3-14 | 打包 Zed 冒烟：真实安装包 + `WXML_LSP_BIN` 启动 Zed；`pgrep` 显示 `wxml-lsp --stdio`；无 node；五语言注册；查询/未保存脚本可用 | `scripts/zed-smoke.sh`（new）+ `docs/evidence/<run>-tsgo-packaged-zed-smoke.md`（new） | NF-01/07/12 | 无；现有 stdio 测试不是 UI smoke |
| F3-15 | legacy 变量/入口：设置 `WXML_TYPE_BACKEND`/`WXML_NODE`/`WXML_BACKEND_DIR` 不激活 Node；Zed 不转发；有效 `WXML_LSP_BIN` 阻止 install/upgrade/rollback；无效值 hard error | `crates/wxml-delivery/tests/installer_flow.rs`（extend）+ `editors/zed/src/lib.rs` 测试（extend） | NF-07/12 | `explicit_binary_wins...` 仅部分 |
| F3-16 | 崩溃/并发：各 journal 状态注入进程消失；`staged` 只清 tmp、`committed` 不自动激活、`activated` 幂等；并发 install 串行；current 始终有效 | `crates/wxml-delivery/tests/installer_flow.rs`（extend）+ `crates/wxml-delivery/tests/native_delivery.rs`（new）`crash_recovery_and_lock` | NF-10 | `concurrent_installs...` 仅 v1 |

### 9.3 证据要求

每个 F3 通过项必须记录：

- 输入 commit、package 版本、archive SHA-256/SRI、manifest SHA-256、引擎版本/gitHead；
- 执行命令、PATH、环境隔离、网络开关、缓存命中；
- 安装前后 old tree digest（文件集合 + 每文件 SHA-256 + 模式）与 `current` 目标；
- 进程审计原始日志（execve/execveat）与网络审计；
- 失败注入点、journal 状态、恢复结果；
- 平台/glibc/ABI 的 `readelf`/`ldd` 输出；
- Zed 冒烟的实际 UI 证据（截图/日志），不能只用 stdio 测试代替。

每个 PASS 还必须标注证据类型：真实包/真实 CLI、真实包 + 单点受控变异、fixture、库级 seam，
或真实资产上的测试注入。fixture/seam/单点变异不得表述为“真实发布缺陷”或“进程级中断”。

### 9.4 状态

以下条目按日期保留历史结论；同一 ID 的当前判定以 §9.5 为准，不要把早于 2026-11-20 的
“仍为 BLOCKED/未关闭”当作当前状态。

- 2026-11-17 初始状态：F3-01～F3-16 **planned，未实现**（生产 Rust 交付链）。
- 2026-11-19：F3-09 的 fixture 级迁移/回滚/前滚/恢复窗口测试位于
  `crates/wxml-delivery/tests/native_rollback.rs`；F3-09 仍为 **planned**，真实资产、CLI/clean-env
  进程审计和包级验收未完成。
- 2026-11-19：F3-03 的 L5/L6 已接线：`wxml-lsp-package native` 现在要求
  `--assets-lock <repo>/assets.lock.json` 与 `--cache <provisioned-cache>`，拒绝输入树提供
  `engine/`/`typings/`，只消费 cache 中已逐文件复核（sha256/size/mode/treeDigest/bundle）的
  payload；manifest `files[]`/`typeAssets[]` 来自 lock 的同一事实源，`engine/lib/tsc` 仍核对固定
  SHA-256/size。生成的 `provenance.json` 记录 `assetsLockSha256` 与逐 asset
  `payloadTreeDigest`，安装器的 `verify_tree_v2`（install/verify/upgrade/rollback/recover 共用）
  在激活前对已安装树重算并比较。**F3-03 仍为 BLOCKED**：真实发布包、clean-env/B6 和包级验收
  未完成；F3-04/05/06/07 获得新的 fixture/opt-in 证据，但仍未真实资产验收，保持 planned。
- 2026-11-19：`wxml-lsp-install` 已按 index `schemaVersion` 分派 schema v1/v2，并新增 `recover`；v2 的
  `install`/`verify`/`upgrade`/`rollback`/`recover`、`--from-dir` 获取、`WXML_LSP_BIN` 优先级和精确版本
  探针已有 fixture 级集成测试。该实现**不关闭任何 F3 ID**：真实 `7.1.0-beta` 资产、clean-env/`strace`、
  网络审计、包级/Zed 冒烟和 ABI 证据仍缺；v2 归档缓存尚未实现，v2 离线安装必须显式 `--from-dir`。
- 2026-11-19：B4 legacy 只读命令去 Node。schema v1 current 的 `current`/`verify`/`env` 只读
  manifest 身份投影或做字节哈希，不再读取 `WXML_NODE`、不解析 `server.js`、不执行
  `node --version`、不执行包内旧二进制或 Node 后端；`env` 只输出 `WXML_LSP_BIN`。
  `verify::check_node`/`resolve_node`/`verify_install` 已删除，v1 安装后的 Node 版本告警也移除。
  证据：`crates/wxml-delivery/tests/native_cli.rs` 的 legacy install/current/verify/env/migrate
  shim 流程、`installer_flow.rs` 的 legacy poison-env 用例，以及 `no_node_guard.rs` 的进程树
  采样（`/proc/<pid>/task/*/children`，无 Node 子进程）。`scripts/verify-no-node.sh --legacy`
  增加了 legacy 只读模式和 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` poison。
  **这些证据不关闭 F3-01/F3-08/F3-09/F3-12/F3-15**：没有 `strace -f -e trace=execve,execveat`
  的绝对路径审计、没有真实 legacy 发布包、clean-env/CI 与 Zed 冒烟；v2 无归档缓存，离线仍只接受
  显式 `--from-dir`；schema v1 的 install/upgrade/rollback 仍保留为只读迁移前的历史路径，属于 P7
  收窄范围。
- 2026-11-19：新增根 `deny.toml`，`cargo deny --offline check` 的 advisories/bans/licenses/sources
  全部通过（P5-SEC-06 的 license 门禁条件）。Rust 依赖许可门禁关闭；B7 的发布包
  许可/provenance/SBOM 评审仍未关闭。
- 2026-11-19：typings 交付口径按
  [26 vs 23 裁决证据](../evidence/2026-11-18-tsgo-typings-26-23-decision.md) 明确为**payload 23
  文件**（对齐现有 allowlist），上游身份仍记录 26 文件；`package.json`、`README-en.md`、
  `typings.json` 由 `extract.exclude` 排除。`MINIPROGRAM_TYPINGS_FILE_COUNT` 与合成 fixture 同步为
  23，真实引擎的 20 文件 `.d.ts` 引脚不变。随后新增冻结常量 `MINIPROGRAM_TYPINGS_TOTAL_BYTES`
  和 `MINIPROGRAM_TYPINGS_BUNDLE_DIGEST`、精确 23 路径 allowlist，并在打包器、manifest 校验和
  `verify_v2` 中强制重算/比较；check-in 的 5.3.1 payload fixture 支撑 hermetic 打包/安装测试，
  `tests/typings_payload.rs` 仍为 opt-in 的真实树一致性检查。**这些改动不关闭 F3-03/B6**：真实
  上游 tarball 的 SRI/SHA-256/size、provisioning/lock、clean-env/ABI 和包级验收仍缺。
- 2026-11-17 临时 Python spike 执行了其中一部分（fresh install、checksum 失败、中断升级、
  N1→N2、rollback、legacy 拒绝、离线、进程审计、恶意 tar、版本探针）；这是**临时证明**，
  不是 F3-01～F3-16 的实现，见 [证据 §11](../evidence/2026-11-17-tsgo-delivery-spike.md)。
- 任何“通过”必须附真实命令与日志；仅有设计/命令文本不算通过。
- 缺失原生资产、`strace`、Zed 图形环境或离线能力时，对应项为 **BLOCKED**，不得标记 pass。
- 2026-11-20：按 [F3 real release 证据](../evidence/2026-11-19-tsgo-f3-real-release.md)、
  [F3 剩余验收矩阵](../evidence/2026-11-19-tsgo-f3-acceptance.md) 和
  [clean-env 证据](../evidence/2026-11-18-tsgo-f3-clean-env.md) 逐项复核，结果见 §9.5。
  本次复核不修改任何历史证据；`closed` 只表示合同列出的条件已有可复查证据，不表示发布、
  签名、其他平台或 Zed UI 已完成。

### 9.5 逐项状态（2026-11-20 审计）

判定口径：

- `closed`：§9.2 的全部子条件都有可复查证据，且证据来自真实包/真实 CLI，或该条件本身就是
  对 legacy/fixture/journal 状态的处理；fixture/seam/单点变异在“关键证据”列标注。
- `partial`：已有 PASS 证据，但至少一个子条件只有 fixture/seam/单点变异，或尚未执行。
- `blocked`：缺少合同要求的机制/环境（`strace`、Zed UI、生产 failpoint），不能用辅助证据替代。

`partial` 行不是“没有证据”；它们已有下表所列 PASS 证据，但仍有合同子条件未满足，不能按
`PASS` 字样关闭。审计基线：证据运行 HEAD 为 `561b00a`（real release）和 `0b3b3da`
（acceptance）；本表在 `0ca4375` 工作树复核。`crates/wxml-delivery/src` 在
`561b00a..0ca4375` 无变化；包内 `bin/wxml-lsp` 是 2026-11-19 23:21 从含并发未提交改动的
工作树构建，不是任一 HEAD 的逐字节重建，F3-03/04 的包/安装结论不声明该二进制的来源可复现。

| ID | 结果 | 关键证据（文件 + 测试/命令） | 仍缺 |
| --- | --- | --- | --- |
| F3-01 | **closed** | ` crates/wxml-delivery/tests/f3_real_release/runtime.rs` 的 `real_release_install_verify_current_env_without_node`（干净 PATH、`--offline`、`env` 仅一行）；` scripts/f3-real-release.sh --network-isolate`；旧 clean-env 的 ` crates/wxml-delivery/tests/real_asset.rs` `installs_pinned_real_engine_without_path_node`（PATH 无 node 名） | 无合同项。网络是 netns/死代理级证明，不是 `connect` 审计（F3-02 范围）。 |
| F3-02 | **closed** | ` crates/wxml-delivery/tests/f3_exec_audit.rs` + `tests/f3_exec_audit/**` + `scripts/f3-exec-audit.sh`：test-only ptrace（`PTRACE_TRACEME`+`PTRACE_O_TRACEEXEC/FORK/VFORK/CLONE/EXITKILL`）记录真实 installer/LSP 的 execve；真实运行 installer 10 exec、native LSP 3 exec，无 node/npm/npx/yarn/pnpm/bun、无 `server.js`、无 `WXML_TYPE_BACKEND`，全部绝对路径；日志 `/tmp/opencode/f3-exec-audit-evidence/`。 | 无合同项。合规平台为 Linux x86_64；非 Linux/无 ptrace 权限时显式 BLOCKED，不能替代成 PASS。 |
| F3-03 | **closed** | ` crates/wxml-delivery/tests/f3_real_release/package.rs` 的 `two_real_release_builds_are_byte_identical` 与 `pinned_real_assets_provision_from_the_frozen_lock`；两次构建 archive/manifest/index/SHA256SUMS 逐字节一致；157 文件、无 `node_modules`/JS/`wxml-type-backend`、`SHA256SUMS` 与许可文件齐全 | 无合同项。包内 `bin/wxml-lsp` 非 HEAD 可复现构建（见审计基线）；无签名/发布通道（非 F3-03 条件）。 |
| F3-04 | **closed** | ` crates/wxml-delivery/tests/f3_real_release/runtime.rs` 的 `real_release_install_verify_current_env_without_node`：空根安装、`verify_tree_v2`、原子 `current`、`state.json`、空 `tmp/`、`verify` 通过 | 无合同项；同样受 dirty-tree `bin/wxml-lsp` 说明限制。 |
| F3-05 | **closed** | ` crates/wxml-delivery/tests/f3_acceptance/package_guards.rs` 的 `f3_05_missing_assets_and_probe_failure_fail_before_activation`（缺 stdlib、缺 typings、探针输出错误三例）；archive SRI 由 ` crates/wxml-delivery/tests/f3_real_release/runtime.rs` 的 `tampered_real_release_archive_fails_before_activation` 覆盖 | 无合同项。三例是真实包 + 单点受控变异，不是已发布缺陷包；archive SRI 只在空根验证。 |
| F3-06 | **partial** | ` crates/wxml-delivery/tests/f3_real_release/runtime.rs` 的 `real_release_upgrade_rollback_and_recover_without_node`（真实 N1→N2→N1、N1 树不变）；` crates/wxml-delivery/tests/f3_acceptance/transactions.rs` 的 `f3_06_real_upgrade_and_rollback_failpoint_windows`（`InstallHooks` 四窗口）；` crates/wxml-delivery/tests/native_rollback.rs` 的 `native_after_state_write_rolls_forward_without_switching_back`（fixture） | 第二个真实 release（N2 是 N1 生产二进制同长度版本常量重标）；生产 failpoint seam 下的精确进程级 `SIGKILL`（switch 前/后），当前 `wxml-lsp-install` 只暴露 `NoHooks`。 |
| F3-07 | **closed** | ` crates/wxml-delivery/tests/f3_real_release/runtime.rs` 的 `real_release_upgrade_rollback_and_recover_without_node`：`rollback --to 0.1.0 --offline`，`current=0.1.0`、`state.previous=0.2.0`，N1/N2 指纹不变；` crates/wxml-delivery/tests/native_delivery.rs` 的 `rollback_refuses_legacy_and_tampered_native_targets`（fixture 负例） | 无合同项。真实包只跑正向 rollback；“拒绝篡改目标”的完整校验负例是 fixture。 |
| F3-08 | **closed** | ` crates/wxml-delivery/tests/f3_acceptance/legacy.rs` 的 `f3_08_09_15_legacy_migration_inert_env_and_v1_rollback`：`rollback --to 0.0.9` 拒绝，v1 树/current/state 不变，tripwire 未触发 | 无合同项。v1 包是生产 v1 packager 生成的 fixture，不是历史发布包（已标注）。 |
| F3-09 | **partial** | ` crates/wxml-delivery/tests/f3_acceptance/legacy.rs` 的 `f3_08_09_15_legacy_migration_inert_env_and_v1_rollback`：真实 N1 + v1 fixture 的成功迁移、缺 typings 失败、`AfterRename`/`AfterSwitch` seam | 精确 pre/post-switch 进程 `SIGKILL`（无生产 seam）；真实 legacy 发布包；seam 是库级 `InstallHooks`，不是 CLI 进程中断。 |
| F3-10 | **closed** | ` crates/wxml-delivery/tests/f3_real_release/runtime.rs` 的 `tampered_real_release_archive_fails_before_activation`；` crates/wxml-delivery/tests/f3_acceptance/package_guards.rs` 的 `f3_10_manifest_file_license_extra_missing_tamper_fails_before_activation`（manifest/逐文件/许可/额外/缺失/缺失许可六类） | 无合同项。六类是真实包 + 单点受控变异；各变体重算 index SRI，失败发生在树校验。 |
| F3-11 | **closed** | ` crates/wxml-delivery/tests/f3_acceptance/package_guards.rs:267` 的 `f3_11_malicious_real_archive_shapes_fail_before_activation`：15 类真实 `install --from-dir` 恶意归档（绝对路径、FIFO/字符/块设备、重复项、GNU long-name >4096 B、PAX 扩展预算 128 KiB > 64 KiB、PAX size 覆盖、PAX 绝对路径、PAX 超长路径、traversal/symlink/hardlink/malformed PAX/per-entry budget）全部在激活前拒绝，无 current/versions/state。 | 无合同项。归档构造是测试侧受控输入，不是已发布恶意包。 |
| F3-12 | **closed** | ` crates/wxml-delivery/tests/f3_acceptance/cache.rs:34` 的 `f3_12_real_cache_driven_upgrade_rollback_and_corrupt_fail_closed`：真实 N1 安装 + 缓存、N2 缓存预热、离线缓存驱动升级/回滚、损坏 archive/index/缺 index fail closed、proxy sink 0 连接；` crates/wxml-delivery/tests/f3_real_release/runtime.rs` 的 `real_release_offline_cache_reinstall_and_fail_closed` 覆盖重装/空缓存。 | 无合同项。内核 `connect` 级审计属于 F3-02；本项只要求网络命名空间/代理 sink。 |
| F3-13 | **closed** | ` crates/wxml-delivery/tests/f3_real_release/abi.rs` 的 `real_release_abi_and_platform_evidence`：`readelf`/`ldd`/`file`、GLIBC_2.39 vs 宿主 2.43、静态引擎、`linux-aarch64` 在输出写入前拒绝；证据 §7 | 无合同项。musl 明确不支持且未生成 artifact（合同不要求）；glibc 下限只在证据文档记录，未写入包 manifest。 |
| F3-14 | **blocked** | ` scripts/f3-real-release.sh` 的 Zed smoke 段；` docs/evidence/2026-11-19-tsgo-f3-real-release.md` §8：Zed 启动打包的 `wxml-lsp --stdio`，`ps` 无 Node 子进程 | 默认 `--stdio` 仍是 Node（原生需 `--backend native`）；无 UI 查询/未保存脚本/五语言注册的截图或诊断；无真实打包 Zed UI 冒烟。 |
| F3-15 | **closed** | ` crates/wxml-delivery/tests/f3_acceptance/legacy.rs:441` 的 `f3_15_explicit_binary_precedence_for_install_upgrade_rollback`（真实 N1/N2 上 `WXML_LSP_BIN` 对 install/upgrade/rollback 的优先级，state/tree 不变；无效值 hard error 无 fallback）；` legacy.rs:28` 的 legacy 只读/`env` 仅一行/native 树无 JS/backend；` crates/wxml-delivery/tests/native_cli.rs` 的优先级/hard error 用例；` editors/zed/src/lib.rs:23` 已删除三个 legacy 转发变量，`:704` 有回归测试。 | 无合同项。`WXML_LSP_BIN` 是唯一保留入口；旧 Node 变量不再由 Zed 注入。 |
| F3-16 | **closed** | ` crates/wxml-delivery/tests/f3_acceptance/transactions.rs:204` 的 `f3_16_journal_recovery_concurrent_install_and_corrupt_active_rollback`：staged/committed/activated journal 注入、两并发 install、损坏 active rollback；第二次 `recover` 幂等（state bytes/inode 不变、journal-only 删除不重写 state）；`support.rs:371` 的 `assert_current_verified` 在 11 个关键点检查相对 `versions/<v>`、containment 与整树校验。 | 无合同项。断电/持续监测仍不在 F3-16 承诺内，已明确标注。 |

对 `2026-11-19-tsgo-f3-real-release.md` 的“可按合同评审关闭”逐项复核：F3-03/04/07/13 维持
`closed`。后续 `50e4a21`/`040898c` 补上了缓存驱动 upgrade/rollback、恶意归档完整形态、Zed
legacy 变量移除、activated 幂等与 `WXML_LSP_BIN` 升级/回滚优先级，因此 F3-11/12/15/16
升为 `closed`。`f2952c2` 的 ptrace exec audit 补齐 F3-02 的 execve 证据，因此升为
**13 closed / 2 partial / 1 blocked**：F3-06/09 仍缺真实第二版本与进程级 SIGKILL，F3-14
缺 Zed UI。F3-05/10 的单点变异性质保留并已标注，不作为“真实发布缺陷”证据。

## 10. 未决阻塞与不承诺事项

### 10.1 阻塞项

| ID | 2026-11-17 状态 | 阻塞 | 影响 | 退出条件 |
| --- | --- | --- | --- | --- |
| B1 | **PARTIAL**（2026-11-20：F3 13 closed / 2 partial / 1 blocked） | v2 库与 `wxml-lsp-install` 的 install/verify/upgrade/rollback/recover、本地 `--from-dir` 获取、state/journal、legacy 读取器已实现；F3-01/02/03/04/05/07/08/10/11/12/13/15/16 已按证据关闭；F3-14 blocked，F3-06/09 partial；Zed UI、生产 failpoint 与真实第二版本仍缺 | 生产 native 安装/升级/回滚仍未全部验收 | D 包 Rust 实现并关闭 F3-01～F3-12；fixture 不能替代真实资产 |
| B2 | **PARTIAL**（spike clean-env） | 无 Node PATH、offline、ptrace exec 审计已在 spike 上执行；Rust build/test/CI/Zed 未在 clean-env 运行；F3-01/02 已关闭 | 生产无 Node 门禁未关闭 | Rust 构建/测试/发布/CI/Zed 在无 Node 环境通过 F3-01/02/12 |
| B3 | **BLOCKED**（2026-11-20：Linux x86_64 ABI 已测） | 平台矩阵未测试；F3-13 已记录 Linux x86_64 的 `readelf`/`ldd`/GLIBC_2.39，其他平台仍无证据 | 只能宣称 Linux x86_64；aarch64/macOS/Windows/musl 无证据 | F3-13 记录 Rust glibc/ABI；其他平台保持“不支持” |
| B4 | **PARTIAL**（2026-11-20：资产/ABI 证据已补） | 真实 tarball 的 SRI/SHA-256/size、122 文件、`lib/tsc` SHA/`--version` 已复核；F3-13 已记录 Rust glibc/ABI；F0 API/AST 握手、`tsc.sig` 用途仍未验证 | 资产身份与 ABI 可用，但安装器未消费真实 API/AST 探针 | F0/F3 安装器消费可重放 SRI/解包/探针/`readelf` 证据 |
| B5 | **PARTIAL**（维护者条件性接受） | 7.1.0-beta 身份已条件性接受；F0 API/AST 合同仍是 draft，`apiSchemaVersion`/协议/探针期望值未正式冻结 | F0 合同未接受 | F0 记录冻结值并写入 lock/manifest，F3 评审接受 |
| B6 | **PARTIAL**（2026-11-20：F3-03 已关闭，F3-14 blocked） | 真实 Rust 包已构建并完成两次构建同哈希、安装、离线回滚、ABI 与 F3-03 负向扫描；Zed 只完成进程级启动，默认 `--stdio` 仍是 Node，无 UI/查询证据 | 包级验收已大部分完成，但无 UI/Zed 验收 | F3-03/14 在隔离 Linux 环境通过真实 Rust 包 |
| B7 | **PARTIAL**（Rust 依赖许可门禁已通过） | `deny.toml` 的许可 allowlist 与 `cargo deny --offline check` 已通过 advisories/bans/sources/licenses；spike 已把 LICENSE/NOTICE/MIT 许可、`provenance.json`、`THIRD_PARTY_LICENSES.md` 打入包并 hash；发布包许可/provenance/SBOM 评审未做 | NF-09 不可关闭 | 许可文件、provenance、SBOM 评审通过 |
| B8 | **ACCEPTED RISK** | 无发布者签名 | 只能证明“内容与元数据一致”，不能证明来源身份 | 接受 ADR-0007 残留风险，或新增签名 ADR |
| B9 | **PARTIAL**（进程中断；2026-11-20：F3-16 partial） | F3-16 的 staged/committed/activated journal 注入、并发 install、损坏 active rollback 已有真实包证据；`activated` 幂等未测，断电/目录 fsync 持久性未测 | 不能宣称断电或全部崩溃窗口安全 | F3-16 补 activated 幂等与断电测试；未通过前只宣称“journal 注入与 commit 后进程中断可恢复” |
| B10 | **OPEN** | 本文件未加入 docs 索引 | 读者可能不知道它是草案 | 合同接受后由文档 owner 更新索引 |

### 10.2 明确不承诺

- 不承诺当前仓库已经无 Node；当前仍是 schema v1/Node 链路。
- 不承诺 F3 合同已接受、F3 测试已实现或通过。
- 不承诺 Linux x86_64 以外的平台；不承诺 musl、aarch64、macOS、Windows。
- 不承诺具体 glibc 下限；未测量前不得写猜测值。
- 不承诺发布者签名、透明日志、证书固定或完整供应链安全。
- 不承诺断电/文件系统崩溃下的原子性；只承诺进程失败/中断下的状态机与校验后切换。
- 不承诺性能、启动时延或引擎查询结果；这些属于 F0/F1。
- 不承诺临时 Python spike 等于生产安装器；其证明范围仅 Linux x86_64 的受限本地路径。

### 10.3 F3 退出门禁

F3 合同可标记 accepted（不是实现完成）需：

1. 本文 §1.5 条件满足；
2. §4 字段/§5 算法/§6 状态机无未决 TBD（F0 常量除外，必须标明 owner/日期）；
3. §9 每个 F3 ID 有唯一 owner（在实施任务书登记）和未来路径；
4. §10 阻塞项有退出条件，不能以“后续再说”关闭；
5. 安全评审接受 §8 的信任边界和残留风险。

实现完成（G3/F3 测试通过）还需 §9 全部通过；两者不能混为一谈。

### 10.4 G1-C 判定（2026-11-17）

**G1-C 仍不能关闭。** 临时 Python spike 消除了“是否可实现 manifest v2/原子安装/回滚/无 Node
本地流程”的关键未知，但：

- 生产 Rust 安装器与 `wxml-delivery` 未改（B1）；
- Rust build/test/release/CI/Zed 未在 clean-env 跑（B2/B6）；
- 平台矩阵、Rust glibc/ABI、musl 未测（B3）；
- F0 API/AST 握手、`tsc.sig`、miniprogram SRI 未验证（B4/B5）；
- 许可/provenance 未经发布评审（B7）；
- 断电持久性未测（B9）。

因此只能把“可执行安装器存在性”从 BLOCKED 降为 **PARTIAL PASS（临时证明）**；G1-C 仍是
**OPEN**，不能以 spike 关闭。详见证据 §11。

## 11. 变更控制

- 本文件版本从 `draft-0` 开始；接受后任何字段/算法/状态机变更必须递增合同版本并记录 hash，
  作为 `CONTRACT_BASE` 的一部分。
- `assets.lock.json` 变更必须独立提交，附新 SRI/大小/许可、F0/F1 复跑结果和 release 重新生成。
- 新增 manifest/index 字段必须同时更新 schema 版本、打包器、校验器、测试和迁移说明；不得利用
  `deny_unknown_fields` 之外的方式静默扩展。
- 信任根变更（签名、证书固定、发布域名）必须新增 ADR 并更新安装器/manifest/index/测试；
  不得在 F3 内静默改变。
- 删除 legacy 读取器/旧材料属于 P7，必须等 P5/P6 等价、恢复、安全和 no-Node 门禁通过后执行；
  本文件不授权删除。
- 发现合同与代码/证据冲突时，先记录冲突并停止实现，由合同 owner 裁决；不得在 worker 内私自
  修改共享合同或伪造通过证据。
