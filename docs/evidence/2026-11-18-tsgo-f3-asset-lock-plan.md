# tsgo F3-03 资产锁与 provisioning 设计（审计/计划，非完成声明）

- 审计对象：worktree /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
- 任务指定 HEAD：`5541ee0`。审计期间另一会话把 HEAD 推进到 `f7b9ef7`（仅 docs/evidence 提交）；`5541ee0..f7b9ef7` 对 `crates/`、`docs/reference/tsgo-delivery-contract.md`、`assets.lock.json` 无改动，因此本文的源码行号以当前树复核，结论不依赖该 docs 提交。
- 工作树状态：`crates/wxml-backend-tsgo/src/{input,native/session,native/vfs}.rs` 有其它会话的未提交修改；`crates/wxml-delivery` 无未提交修改，仅有未跟踪的 `tests/native_real_upgrade.rs`。本文不以这些并发修改作为交付侧事实。
- 写入边界：只读源码/契约/证据/本地固定资产；只新增本文件；未修改生产代码、测试、Cargo、契约、打包器、安装器、回滚、backend/server、Node；未提交。
- 未下载、未执行任何二进制；只做了只读文件枚举、`sha256sum`、`stat`、`tar -tvzf` 列表。
- 状态：**F3-03 仍为 BLOCKED**。本文只给出可执行、可测试的顺序设计，不关闭任何 F3 ID，不删除 Node，不切换默认后端。

---

## 0. 结论摘要

1. v2 打包器已经存在，并且只固定了 `engine/lib/tsc` 的字节、大小与 `engine/package.json` 的 `gitHead`；其余引擎文件、stdlib、typings 的“期望值”仍来自打包输入本身。见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:652 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1106 )。
2. 仓库**没有** `assets.lock.json`，也**没有** provisioning 命令、下载/解包/逐文件校验管线、typings 的 tarball SRI/SHA-256/逐文件摘要。契约自己声明该文件“当前尚不存在”，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:713 )。
3. typings 存在可复现的 26 vs 23 冲突：上游包 26 文件，打包器 allowlist 只允许 23，manifest/契约要求 26。实测被 allowlist 排除的恰是 `package.json`、`README-en.md`、`typings.json`；其中 `package.json` 还被运行期负向扫描单独禁止。见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1074 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:42 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:84 )。
4. 因此仅靠“打包器计算自己在输入里看到的哈希”不能证明包内容；必须引入一个**独立于打包输入**的仓库文件 `assets.lock.json`，固定上游 tarball 身份与解包后的最终 payload 逐文件身份，再由打包器、provisioning 缓存和测试三方交叉校验。
5. 本文的 lock 设计把“上游归档事实”与“包内 payload 事实”分开记录：`tarball{url,size,integrity,sha256}`、`upstream{fileCount,unpackedSize,stripPrefix}`、`payload{fileCount,totalBytes,treeDigest,files[],bundles[]}`。26 与 23 同时可见，任何一方变化都会显式失败，而不是静默改常量。
6. provisioning 必须是**显式构建期命令**，只接受仓库内 lock；支持 `--from-dir` 与 `--offline`，默认不联网；解包必须使用独立的 npm-tarball 规则（`package/` 前缀、模式归一化、include/exclude 审计），不能复用正式包解包器的“精确 allowlist、拒绝一切额外文件”语义，也不能放宽正式包安装规则。见契约 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:783 )。
7. 打包器必须从“已验证的 provisioning 缓存”组装 `engine/` 与 `typings/`，禁止从任意 `--input` 接受这两棵子树；否则 lock 只是一份未被消费的文档。
8. 安装器不读取仓库 lock（安装时没有仓库）；它继续校验 manifest + SHA256SUMS + 逐文件哈希。lock 身份通过打包器写入 `provenance.json`（`assetsLockSha256`、每个 asset 的 `payloadTreeDigest`、上游 tarball 摘要），安装器在字段存在时重算并拒绝 drift。manifest schema 与 `state.json`/journal 不在本轮改形状。
9. 现在就能做的测试：lock 解析/拒绝矩阵、合成 npm tarball 的安全解包、`--from-dir` 使用本地已存在的 7.1.0-beta 引擎 tarball、缓存命中重校验、打包确定性（引擎真实、typings 合成）。现在不能做的：真实 typings tarball 的 SRI/SHA 验证、干净环境打包运行、网络审计、26/23 合同裁决、发布包 license/provenance 评审。
10. F3-03 的退出条件不是“多写一个 JSON”，而是：真实 typings tarball 被锁定、26/23 被合同裁决并冻结、真实 Rust 包在隔离 Linux 环境通过布局/许可/负向扫描/SHA256SUMS/两次构建同哈希、且证据记录写入。当前一项都不满足，B6 仍为 BLOCKED，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1227 )。

---

## 1. 事实基线（HEAD 可复核）

### 1.1 已存在、可复用的能力

| 能力 | 位置 | 现状 |
| --- | --- | --- |
| v2 manifest 结构与冻结常量 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:33 ) | 引擎版本/gitHead/SRI/SHA-256/size/fileCount、116 stdlib、26 typings 都是编译期常量 |
| manifest 结构校验 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:259 ) | `deny_unknown_fields`；类型资产 bundle 与实际 `files[]` 对账 |
| v2 打包器 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:652 ) | 收集输入树、写 manifest/index/SHA256SUMS/确定性归档、自检 |
| 固定引擎字节与 gitHead | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1151 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1106 ) | 只覆盖 `engine/lib/tsc` 与 `engine/package.json` 的 name/version/gitHead |
| 负向扫描 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:23 ) | 拒绝 `node_modules/`、JS/CJS/MJS、Node 锁文件、非 `engine/package.json` 的 package.json |
| 树校验与 fingerprint | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:42 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:295 ) | 精确文件集合、逐文件哈希/大小/模式、目录集合、`tree_fingerprint` |
| 资产身份复核 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/sri.rs:152 ) | 一次读取同时校验 size/SHA-512 SRI/SHA-256 |
| 正式包安全解包 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/archive.rs:344 ) | 拒绝链接/设备/遍历/重复/大小写冲突/异常模式；预扫描扩展头 |
| 本地/远端 release 获取 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/acquire_v2.rs:73 ) | `--from-dir`、offline fail-closed；v2 归档缓存未实现 |
| 安装事务/回滚/恢复 | ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/native_install.rs:409 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/state.rs:161 ) | state/journal、staged→committed→activated、rollback 重校验 |

这些能力说明：**安装期**的信任链已经基本成形；缺的是**构建期**“这些字节本来应该是什么”的独立锚点。

### 1.2 缺失（F3-03 的直接阻塞）

1. 仓库根没有 `assets.lock.json`（本次 `ls` 确认不存在）。
2. 没有 provisioning 二进制/子命令；`wxml-lsp-package` 只有 v1 与 `native` 两种打包路径，没有 `provision`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/bin/wxml-lsp-package.rs:1 )。
3. 没有**上游资产**的 provisioning 路径：`verify_asset_file` 已用于 release 归档（source.rs 在本地/远端获取后调用 `release.verify_archive`），但没有代码按 lock 校验引擎/typings tarball 的 size/SRI/SHA-256 后再解包；现有测试 harness 只是直接调用该函数，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/real_asset/support.rs:173 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/source.rs:350 )。
4. 没有 typings 的 tarball 身份与逐文件哈希；当前所有测试的 typings 都是合成内容，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_package.rs:146 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/real_asset/support.rs:487 )。
5. 没有“打包输入必须等于 lock”的检查；现有 manifest 的 `files[]` 哈希是从输入现算的，因此输入被替换后 manifest 仍然自洽。见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:961 )。
6. stdlib 只有数量检查（116）与 bundle 摘要由输入计算，没有独立固定值；见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1314 )。
7. 26 vs 23 冲突没有裁决；任一方向的“修复”都会违反另一处冻结值（见 §4.4）。
8. 确定性归档测试是 opt-in 的：没有 `TSGO_NATIVE_ENGINE_DIR` 时直接跳过，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_package.rs:345 )；默认 CI 不证明两次构建同哈希。
9. CI 没有 clean-PATH / 无 Node 的 packaging job；当前 workflow 只有 fmt/clippy/test/doc/deny，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/.github/workflows/ci.yml:1 )。

### 1.3 26 vs 23 的实测事实

在未修改任何文件的前提下，对本地 npm 安装副本 `backends/typescript/node_modules/miniprogram-api-typings` 做了只读枚举与哈希：

| 项 | 实测 | 对应冻结/允许值 |
| --- | ---: | --- |
| 上游包文件总数 | 26 | manifest/契约要求 26，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:84 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:333 ) |
| 打包器 allowlist 接受的文件数 | 23 | `is_allowed_native_path`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1095 ) |
| 被 allowlist 排除的文件 | `package.json`、`README-en.md`、`typings.json` | 无配置项可覆盖 |
| 其中被负向扫描再次禁止的 | `package.json` | 见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:42 ) |
| 运行时 `.d.ts`（index + types/**.d.ts） | 20 / 2,522,752 B | P6-3 计划记录 20，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:39 ) |
| 全 26 文件 bundle 摘要 | `2141aac47206441f0434a7b519e49da1e8756d1bcdb6c17809a7f5e188a34c42` | 本次重算与 P6-3 记录一致，见同文件 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:70 ) |
| 23 文件 allowlist bundle 摘要 | `ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b` | 同上 |
| 全包字节 | 2,552,052 B | 契约 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:192 ) |

结论：**26 与 23 不是“统计口径差异”，而是两个互斥的冻结值**。任何 lock 都必须同时记录 `upstream.fileCount=26` 与 `payload.fileCount`（23 或 26，由合同裁决），否则冲突会继续被隐藏。

### 1.4 本轮可用的本地固定资产（未下载）

| 资产 | 路径 | 实测 |
| --- | --- | --- |
| 引擎 tarball | /tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz | SHA-256 `59128685…1855`、10,175,135 B、122 条目、顶层 `package/`；条目模式实测为 `0644/0755`、uid/gid 0、固定时间戳 |
| 解包后的引擎 | /tmp/opencode/tsgo-spike/extract/7.1.0-beta/package | `lib/tsc` 24,522,912 B、SHA-256 `a183b195…faaa70`；`lib*.d.ts` 116 个、3,870,196 B、bundle `5e126e72…3e85e` |
| typings 本地副本 | /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/backends/typescript/node_modules/miniprogram-api-typings | 26 文件；该目录被 `.gitignore` 忽略（`git ls-files` 为 0），**不能作为 provenance**，只能作为本地 fixture |

注意：契约示例给 typings 的 `integrity` 是 `sha512-2Cir…`、`size` 449,429，但本地没有该 tarball，无法重算；该值只能视为“registry 元数据，未在本次复核”，不能写进已提交 lock 的已验字段。

---

## 2. `assets.lock.json` schema

### 2.1 定位与不变量

- 文件位置：仓库根 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/assets.lock.json（新建；当前不存在）。
- 唯一消费者：构建机 provisioning 与打包器。安装器/运行时**不得**读取它，见契约 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:697 )。
- 只记录**不可变事实**；不写下载时间、主机名、绝对路径、临时目录、工具 stderr。时间戳/下载源写入缓存 `provenance.json`，不进入 lock（否则 lock 不确定）。
- lock 变更必须独立提交，附带新 SRI/大小/许可、F0/F1 复跑结果、release 重新生成，见契约 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1139 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1275 )。
- 解析使用 `serde(deny_unknown_fields)`，大小上限复用 `MAX_INDEX_BYTES`（1 MiB），见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest.rs:41 )。

### 2.2 JSON 形状（冻结字段形状；`files[]` 为节选，未验证值用占位标记）

```json
{
  "schemaVersion": 1,
  "assets": [
    {
      "name": "typescript-native-engine",
      "kind": "npm-platform-tarball",
      "npmPackage": "@typescript/typescript-linux-x64",
      "version": "7.1.0-beta",
      "sourceCommit": "3e9236361fa97a9e6c685f4833ac457ae2d8c50a",
      "os": ["linux"],
      "cpu": ["x64"],
      "provenance": {
        "registry": "https://registry.npmjs.org",
        "distSha1": "<40 hex; registry cross-check only, never a security digest>",
        "publishedAt": "<RFC3339; informational>"
      },
      "license": "Apache-2.0",
      "licenseFiles": ["engine/LICENSE", "engine/NOTICE.txt"],
      "packageRoot": "engine",
      "tarball": {
        "url": "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz",
        "size": 10175135,
        "integrity": "sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==",
        "sha256": "59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855"
      },
      "upstream": {
        "fileCount": 122,
        "unpackedSize": 28450796,
        "stripPrefix": "package/",
        "treeDigest": "<由 upstream.files[] 重算>",
        "files": [
          {"path": "lib/tsc", "sha256": "a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70", "size": 24522912, "mode": "0755"}
        ]
      },
      "extract": {
        "include": ["lib/**", "LICENSE", "NOTICE.txt", "package.json", "README.md"],
        "exclude": [],
        "executables": ["lib/tsc"]
      },
      "payload": {
        "fileCount": 122,
        "totalBytes": 28450796,
        "treeDigest": "<由下方 files[] 重算，不手填>",
        "files": [
          {"path": "engine/lib/tsc", "sha256": "a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70", "size": 24522912, "mode": "0755"},
          {"path": "engine/LICENSE", "sha256": "<64 hex>", "size": 9197, "mode": "0644"},
          {"path": "engine/NOTICE.txt", "sha256": "<64 hex>", "size": 46827, "mode": "0644"},
          {"path": "engine/package.json", "sha256": "<64 hex>", "size": 1045, "mode": "0644"},
          {"path": "engine/README.md", "sha256": "<64 hex>", "size": 138, "mode": "0644"},
          {"path": "engine/lib/lib.d.ts", "sha256": "<64 hex>", "size": 953, "mode": "0644"}
        ],
        "bundles": [
          {
            "name": "typescript-standard-lib",
            "root": "engine/lib",
            "include": ["lib*.d.ts"],
            "fileCount": 116,
            "totalBytes": 3870196,
            "sha256": "5e126e7206065b6fff9d651eff3ad66b9413804d3e23d7ecfb4257299af3e85e"
          }
        ]
      }
    },
    {
      "name": "miniprogram-api-typings",
      "kind": "npm-tarball",
      "npmPackage": "miniprogram-api-typings",
      "version": "5.3.1",
      "os": ["any"],
      "cpu": ["any"],
      "provenance": {
        "registry": "https://registry.npmjs.org",
        "distSha1": "<40 hex; registry cross-check only>",
        "publishedAt": "<RFC3339; informational>"
      },
      "license": "MIT",
      "licenseFiles": ["typings/miniprogram-api-typings/LICENSE"],
      "packageRoot": "typings/miniprogram-api-typings",
      "tarball": {
        "url": "https://registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.1.tgz",
        "size": 449429,
        "integrity": "sha512-2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow==",
        "sha256": "<MEASURED-64-HEX>"
      },
      "upstream": {
        "fileCount": 26,
        "unpackedSize": 2552052,
        "stripPrefix": "package/",
        "treeDigest": "<由 upstream.files[] 重算>",
        "files": [
          {"path": "package.json", "sha256": "<64 hex>", "size": 1234, "mode": "0644"},
          {"path": "README-en.md", "sha256": "<64 hex>", "size": 1234, "mode": "0644"},
          {"path": "typings.json", "sha256": "<64 hex>", "size": 1234, "mode": "0644"},
          {"path": "index.d.ts", "sha256": "<64 hex>", "size": 1234, "mode": "0644"}
        ]
      },
      "extract": {
        "include": ["index.d.ts", "types/**", "LICENSE", "README.md", "CHANGELOG.md"],
        "exclude": [
          {"path": "package.json", "reason": "Node package metadata; forbidden by runtime payload policy"},
          {"path": "README-en.md", "reason": "not part of the frozen payload allowlist"},
          {"path": "typings.json", "reason": "original-package metadata; not needed at runtime"}
        ],
        "executables": []
      },
      "payload": {
        "fileCount": 23,
        "totalBytes": 2548075,
        "treeDigest": "<重算>",
        "files": [
          {"path": "typings/miniprogram-api-typings/index.d.ts", "sha256": "<64 hex>", "size": 1234, "mode": "0644"}
        ],
        "bundles": [
          {
            "name": "miniprogram-api-typings",
            "root": "typings/miniprogram-api-typings",
            "include": ["**"],
            "fileCount": 23,
            "totalBytes": 2548075,
            "sha256": "ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b"
          }
        ]
      }
    }
  ]
}
```

字段说明：

| 字段 | 必需 | 约束/语义 |
| --- | --- | --- |
| `schemaVersion` | 是 | 必须为 `1`；其它值 fail-closed，不复用 manifest 的 `schemaVersion=2` |
| `assets[].name` | 是 | 唯一；必须恰好包含 `typescript-native-engine`、`miniprogram-api-typings` |
| `kind` | 是 | `npm-platform-tarball` 或 `npm-tarball`；不允许 `local-dir`/`git` |
| `npmPackage` | 是 | 精确 package 名，必须与 `engine.distribution`/契约表一致 |
| `version` | 是 | 严格 semver；引擎必须等于 `ENGINE_VERSION`，typings 必须等于 `MINIPROGRAM_TYPINGS_VERSION` |
| `sourceCommit` | 引擎必需 | 40 位小写 hex；必须等于 `ENGINE_SOURCE_COMMIT` |
| `os`/`cpu` | 是 | 引擎必须 `linux`/`x64`；typings 为 `any`/`any`；不匹配在任何 I/O 前 `UnsupportedPlatform` |
| `provenance{registry,distSha1,publishedAt}` | 是 | 只用于 registry 元数据交叉核对；SHA-1 不得作为安全摘要，也不得参与信任判断 |
| `license` | 是 | SPDX：`Apache-2.0` 或 `MIT` |
| `licenseFiles` | 是 | 必须存在于 `payload.files[]`，且随包分发 |
| `packageRoot` | 是 | 最终包内根；必须等于 manifest 的 asset root |
| `tarball.url` | 是 | 精确 `https://registry.npmjs.org/...`；禁止 userinfo/query/fragment/端口/重定向到跨源 |
| `tarball.size` | 是 | >0 且 ≤ `MAX_ARCHIVE_BYTES`；必须等于实测 |
| `tarball.integrity` | 是 | `sha512-<base64>`；必须等于实测 SRI；等价于 manifest 的 `engine.distributionIntegrity` |
| `tarball.sha256` | 是 | 64 位小写 hex；必须等于实测；禁止全 0 占位 |
| `upstream.fileCount/unpackedSize` | 是 | 解包后的**上游全部文件**（引擎 122；typings 26）；必须等于 `upstream.files[]` 的数量/总和 |
| `upstream.files[]` | 是 | stripPrefix 之后的**完整**上游列表（含被 payload 排除的文件；示例为节选）；`{path,sha256,size,mode}`；mode 为归一化后的模式；逐文件固定 |
| `upstream.treeDigest` | 是 | 对 `upstream.files[]` 按 `tree_fingerprint` 重算：根为 `""`；先所有 `D 755 <dir>\n`（目录=文件路径祖先闭包、不含根，按 UTF-8 字节序），再所有 `F <octal mode> <sha256> <path>\n`（按路径字节序），最后 SHA-256 |
| `extract.include` | 是 | 相对 strip 后的 glob；`*` 不跨目录、`**` 递归；复用 `validate_glob_pattern` |
| `extract.exclude[]` | 是 | 数组元素为 `{path, reason}`；显式列出被丢弃的上游文件及原因；`include` 与 `exclude` 必须覆盖上游全部条目，**不允许静默丢弃** |
| `extract.executables` | 是 | 仅这些路径解包为 `0755`；其余一律 `0644` |
| `payload.fileCount/totalBytes` | 是 | 必须等于 `files[]` 的长度/总和；必须等于 manifest `typeAssets[].fileCount` |
| `payload.treeDigest` | 是 | 与 `upstream.treeDigest` 完全相同的 `tree_fingerprint` 算法，但根为 `packageRoot`（先 strip 精确前缀），目录同样取祖先闭包且固定 `0755`；行拼接 helper 与 `verify_v2::tree_fingerprint` 共用，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:303 ) |
| `payload.files[].path` | 是 | 包相对（= `packageRoot/` + 上游相对路径）、`/` 分隔、无 `..`/绝对/反斜杠/控制字符；唯一且大小写折叠唯一 |
| `payload.files[].sha256/size/mode` | 是 | 逐文件固定；mode 仅 `0644/0755`；大小 >0 |
| `payload.bundles[]` | 是 | 每个 manifest `typeAssets[]` 必须有同名 bundle；`fileCount/sha256` 必须与 `files[]` 重算一致（算法见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:191 )） |

### 2.3 规范化与 lock 身份

- 规范形式：结构体字段顺序（serde 声明顺序）、2 空格 pretty JSON、UTF-8、LF、末尾单个 LF；数组按 `path`/`name` 排序。
- `lockSha256 = SHA-256(规范化字节)`。它**不写入 lock 自身**（避免自引用），由 provisioning 写入缓存 `provenance.json`，由打包器写入包内 `provenance.json`。
- 复核方式：`lockSha256` 始终对**重序列化后的规范字节**计算，原文空白/key 顺序不参与身份；测试断言 `parse -> serialize` 幂等。语义变化一定改变身份。
- 禁止把 `builtAt`、`fetchedAt`、主机名、绝对路径、环境变量写进 lock。

### 2.4 结构性校验（在读取任何 tarball 之前）

1. `schemaVersion == 1`；`assets` 恰好两个、名称唯一。
2. 所有字符串路径走 `validate_relative_path`；预算复用 `MAX_PATH_BYTES/COMPONENTS/COMPONENT_BYTES`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/paths.rs:8 )。
3. `tarball.url` 必须精确匹配冻结 URL 字符串，不做宽松解析；这与 manifest 的 `distributionUrl` 必须相等，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:330 )。
4. 引擎字段逐项等于 `manifest_v2` 常量：版本、gitHead、distribution、SRI、SHA-256、size、fileCount、unpackedSize；任一不等 → `InvalidPackage("assets lock ... does not match the frozen engine")`。
5. typings 字段逐项等于常量：版本 `5.3.1`、license `MIT`、root `typings/miniprogram-api-typings`；`upstream.fileCount == 26`；`payload.fileCount` 必须等于契约最终冻结值（见 §4.4，当前未裁决）。
6. `upstream.files[]` 的数量/字节总和必须等于 `upstream.fileCount/unpackedSize`；`upstream.treeDigest` 必须重算相等；payload 的每个文件都必须在 `upstream.files[]` 中有同字节记录（按 `packageRoot` 映射后比较 sha/size）。
7. 每个 `payload.files[]` 必须位于 `packageRoot/` 下；`bundles[].root` 必须是 `payload` 的前缀；bundle 的 include 不得逃出 root。
8. `payload.bundles[]` 必须按 `compute_type_asset_bundle` 重算并相等；`payload.treeDigest` 必须按 §2.2 重算并相等。
9. 禁止占位：`tarball.sha256` 全 0、`files[].sha256` 全 0、`size == 0`、`fileCount == 0` 一律 `InvalidPackage`。契约已明确禁止保留占位值，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:754 )。
10. 引擎 payload 必须包含 `engine/lib/tsc` 且 sha/size 等于固定值；必须包含 `engine/LICENSE`、`engine/NOTICE.txt`、`engine/package.json`；typings payload 必须包含 `index.d.ts`、`LICENSE`、至少一个 `types/**`（与 manifest 必需项一致，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:736 )）。

---

## 3. provisioning 管线

### 3.1 命令与输入

新增二进制或子命令（二选一，推荐后者以减少发布物）：

```text
wxml-lsp-package provision --lock <repo>/assets.lock.json --cache <dir> [--from-dir <dir>] [--offline] [--allow-download]
wxml-lsp-package provision --lock <repo>/assets.lock.json --cache <dir> --print-plan
```

- `--lock` 必填，且必须位于仓库根（拒绝任意用户路径；用 canonicalize + 前缀检查）。
- `--cache` 必填，绝对路径、目录不存在则创建、权限 0700；不得位于安装根 `versions/` 内。
- `--from-dir`：本地 tarball 目录；文件名必须严格等于 lock 中 URL 的最后一段（例如 `typescript-linux-x64-7.1.0-beta.tgz`）。拒绝 symlink/硬链接/设备/目录替换；canonicalize 后必须仍在目录内。
- `--offline`：只读缓存与 `--from-dir`；缺失即失败，不回退网络。
- `--allow-download`：唯一允许联网的开关；没有它时即使 lock 有 URL 也失败。
- `--print-plan`：只输出每个 asset 的来源/缓存键/是否命中，不写文件；用于 CI 审计。

### 3.2 有序步骤（每个 asset 独立执行）

| 步骤 | 行为 | 失败结果 |
| --- | --- | --- |
| P0 | 读取 lock（≤1 MiB），解析并执行 §2.4 全部校验 | `InvalidPackage`/`SizeLimit`；不创建 cache |
| P1 | `os/cpu` 与本机比较（`host_platform`），引擎必须 `linux-x64` | `UnsupportedPlatform`；不创建 cache |
| P2 | 用 lock 已声明的 `payload.treeDigest` + `lockSha256` 计算缓存键，先查缓存 | 命中 → P9；未命中继续 |
| P3 | 解析来源：`--from-dir` → （仅 `--allow-download`）HTTPS | 无来源：`InvalidArgument("... --from-dir ...")`/`InstallState` |
| P4 | 取 tarball 到 `<cache>/tmp/<nonce>/<name>.tgz`（目录 0700，文件 `create_new`） | 写失败：`Io`，删 tmp |
| P5 | **解包前**校验 size/SRI/SHA-256，复用 `verify_asset_file` | 任一不符：`Integrity`；删 tmp，不创建 cache 条目 |
| P6 | 用 npm-tarball 规则解包到 `<cache>/tmp/<nonce>/<name>/` | `UnsafeArchive`/`SizeLimit`；删 tmp |
| P7 | 先按 `upstream.files[]` 校验全部解包字节；再按 `payload.files[]` 组装 payload（只复制 lock 列出的路径；`exclude` 仅用于审计），逐文件校验 sha/size/mode | `Integrity`；删 tmp |
| P8 | 重算 treeDigest/bundles/total；与 lock 全等；在 tmp 内写 `provenance.json` + `SHA256SUMS` + `lock.json`；fsync；原子 `rename` 整个目录到 `assets/<name>/<treeDigest>.<lockSha256[..16]>/` | 任一不符：`Integrity`，删 tmp |
| P9 | 缓存命中：读 `provenance.json`，要求 `lockSha256` 相等；重新逐文件哈希/大小/模式；重算 treeDigest/bundles | 不等：`InstallState("assets cache entry belongs to a different lock")` 或 `Integrity`；fail closed，不复用 |

关键点：

- **先验后解包**：P5 必须在 P6 之前；`verify_asset_file` 一次读取同时算 SRI 与 SHA-256，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/sri.rs:152 )。
- **缓存键同时绑定 payload 与 lock**：`<treeDigest>.<lockSha256[..16]>`。同一个 tarball 因 allowlist/归一化变化会产生不同 payload；lock 任何语义变化也会产生新键。这样缓存条目可以保持不可变，不需要原地覆盖。`provenance.lockSha256` 仍作为命中时的二次校验。
- **缓存命中不信任目录名**：P9 必须重新哈希；契约明确要求“命中后必须重新校验”，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:217 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:768 )。
- **无子进程/无 Node**：provisioning 模块不得使用 `std::process::Command`；测试在 tripwire PATH 下运行并断言 marker 未创建。frozen 资产只由 Rust 代码读写。
- **原子性**：所有写入先在 `<cache>/tmp/<nonce>/` 完成，再 `rename`；失败删除 tmp；不允许半成品出现在 `assets/`。
- **并发**：多个 provision 进程可同时运行；`create_new` + 唯一 nonce；失败者丢弃自己的 tmp；`rename` 目标已存在时按“另一进程已产出同一 payload”处理，但仍执行 P9 复核。

### 3.3 npm-tarball 解包规则（独立实现，不得放宽正式包）

现有 `extract_tar_gz` 不能直接用于 npm 包，原因有三：

1. 它不接受顶层 `package/` 前缀的“映射”，会把内容解到 `destination/package/...`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/archive.rs:412 )。
2. 它只接受 `0644/0755`，其它模式直接拒绝；npm 包理论上可能带 `0664`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/archive.rs:588 )。
3. `extract_tar_gz_allowlisted` 要求归档文件集合**精确等于** allowlist；provisioning 需要“include 之外的条目被记录并丢弃”，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/archive.rs:455 )。

因此新增 `extract_npm_tar_gz(archive, dest, strip_prefix, include, exclude, executables)`，安全规则与正式包保持一致：

- 顶层只允许唯一 `package/`；出现第二个顶层条目、绝对路径、盘符、UNC、`..`、`.`、空组件、反斜杠、NUL/控制字符、超长路径 → `UnsafeArchive`。
- 拒绝 symlink、hardlink、FIFO、socket、字符/块设备、setuid/setgid/sticky。
- 拒绝重复路径、文件/目录同名、大小写折叠冲突。
- 预算复用：归档 ≤512 MiB、单文件 ≤128 MiB、解包总量 ≤1 GiB、条目数 ≤2×8192、PAX 扩展头 ≤64 KiB；见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/archive.rs:117 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest.rs:31 )。
- 模式归一化：目录一律 `0755`；`executables` 中的文件写 `0755`，其余文件写 `0644`；原始 mode 只用于安全拒绝（setuid 等），不进入 payload。这样 payload 的 `treeDigest` 与安装期 `verify_tree_v2` 对目录模式的断言一致（目录 0755，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:107 )）。
- 解包后枚举上游文件全集，断言 `include ∪ exclude == 上游全集`；任何未分类条目 → `InvalidPackage("assets lock does not classify ...")`，不静默丢弃。
- 解包目标必须不存在；失败删除整棵临时目录。

**与正式包的关系**：provisioning 解包器只用于构建机缓存；正式包安装仍走 `extract_tar_gz_allowlisted` 与 `verify_tree_v2`。契约明确禁止把宽松规则用于正式包，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:794 )。

### 3.4 网络策略（只对 `--allow-download`）

- 只允许 `https://`；禁止 `http://`（测试 loopback 例外，见契约 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:704 )）。
- 拒绝 userinfo、query、fragment、`..` 路径段、反斜杠、控制字符、非默认端口。
- 只允许同源 redirect，最多 3 次；跨源失败；不携带凭据。
- 不支持 `--insecure`、代理凭据；不解析 `dist-tags`/`latest`；不读取环境变量决定源。
- 下载使用流式哈希 + 大小上限；先写 tmp；任何失败删除 tmp，不进入缓存。
- 默认 CI/本地测试**不触发**该路径；所有测试使用 `--offline`/`--from-dir`。

### 3.5 缓存布局

```text
<cache>/
├── assets/
│   ├── typescript-native-engine/
│   │   └── <payloadTreeDigest>.<lockSha256[..16]>/
│   │       ├── payload/engine/...          # 最终包内字节
│   │       ├── provenance.json             # lockSha256、tarball、实测、来源、时间
│   │       ├── lock.json                   # 提交的 lock 原字节
│   │       └── SHA256SUMS                  # 仅 payload
│   └── miniprogram-api-typings/
│       └── <payloadTreeDigest>.<lockSha256[..16]>/...
└── tmp/
    └── <nonce>/                            # 0700；失败/崩溃后由下一次 provisioning 清理
```

- `provenance.json` 至少包含：`schemaVersion`、`lockSha256`、`assetName`、`tarball{url,size,integrity,sha256}`、`payloadTreeDigest`、`payloadFileCount/totalBytes`、`downloadedAt`（仅缓存）、`sourceKind`（`cache|from-dir|https`）、`toolVersion`。
- `SHA256SUMS` 覆盖 `payload/` 下所有文件；集合精确等于 lock 的 `payload.files[]`。
- 缓存条目不可变；损坏时删除/隔离并重新 provisioning，绝不原地修补。

---

## 4. 打包器/安装器如何校验并拒绝 drift

### 4.1 打包器（构建期，唯一 lock 消费者）

新增参数（推荐）：

```text
wxml-lsp-package native \
  --input <stage-without-engine-and-typings> \
  --assets-lock <repo>/assets.lock.json \
  --assets-cache <cache> \
  --output <dir> ...
```

行为顺序（在任何输出目录创建之前）：

1. 解析 lock，计算 `lockSha256`，执行§2.4。
2. 对每个 asset 执行 provisioning 的 P9 复核（缓存必须已存在；打包器**不下载**）。缺失 → `InstallState("asset cache is missing; run provision first")`。
3. `--input` 中出现 `engine/` 或 `typings/miniprogram-api-typings/` → `InvalidArgument("engine/typings must come from the verified asset cache")`；防止“lock 写的是一套、打包输入是另一套”。
4. 在私有临时目录组装最终树：`--input` 的其他文件 + 缓存 `payload/`；逐文件按 lock 校验 sha/size/mode。
5. 走现有 `collect_native_files` / `build_native_manifest`；在写 manifest 前断言：
   - `engine.binary` 的 sha/size 等于 lock；
   - 每个 `typeAssets[]` 的 `fileCount/sha256/totalBytes` 等于 lock bundle；
   - `engine`/`typings` 根下的文件集合精确等于 lock `payload.files[]`；不得有额外文件；
   - `provenance.json` 写入 `assetsLockSha256`、每个 asset 的 `payloadTreeDigest`、`tarball` 摘要。
6. 仅当全部通过才进入现有写出路径 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:683 )；失败时不产生 output/stage。

这样修复了当前的核心缺口：当前 `build_native_package` 只固定 `lib/tsc` 与 gitHead，`engine/lib/lib.es2020.d.ts`、`LICENSE`、typings 内容完全由输入决定，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:652 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1106 )。

### 4.2 安装器（安装期，不读仓库 lock）

- 安装器继续执行 `verify_tree_v2`：manifest、SHA256SUMS 精确集合、逐文件哈希/大小/模式、负向扫描、bundle 重算，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:42 )。
- 新增（字段存在时）对 `provenance.json` 的检查：
  - `assetsLockSha256` 必须是 64 hex（仅形状，不声称外部可信）；
  - 每个 asset 的 `payloadTreeDigest` 必须能对包内对应子树重算并相等；
  - `tarball.integrity/sha256/size` 必须与 manifest `engine.distribution*` 一致（引擎）。
- 该检查不改变 manifest schema，也不要求安装器联网/读仓库；旧包没有这些字段时跳过，保持向后兼容。
- 失败一律 `Integrity`，在激活前拒绝；`current` 不变。

### 4.3 错误/状态映射（供测试断言固定子串）

| 场景 | 变体 | 固定子串/说明 |
| --- | --- | --- |
| lock 缺失/超大 | `Io`/`SizeLimit` | 文件不存在；`assets lock` |
| lock schema≠1 / 未知字段 | `InvalidPackage` | `unsupported assets lock schema` / `assets lock is malformed` |
| lock 与编译常量不一致 | `InvalidPackage` | `assets lock does not match the frozen` |
| URL 非 https/带凭据/query | `UntrustedSource` | `assets lock tarball URL` |
| os/cpu 不匹配 | `UnsupportedPlatform` | 在任何 I/O 前 |
| tarball 大小超上限 | `SizeLimit` | `asset archive exceeds` |
| SRI 不符 | `Integrity` | `asset SHA-512 SRI does not match`（复用 sri.rs） |
| SHA-256 不符 | `Integrity` | `asset SHA-256 does not match` |
| size 不符 | `Integrity` | `asset size ... does not match` |
| 解包越界/链接/设备 | `UnsafeArchive` | 沿用 archive.rs 文案 |
| 解包预算超限 | `SizeLimit` | 沿用 archive.rs 文案 |
| payload 缺失/额外/哈希/模式 | `Integrity` | `payload ...` |
| treeDigest/bundle 不符 | `Integrity` | `payload tree digest` / `type asset bundle` |
| lock vs 打包输入不符 | `Integrity` | `packaged bytes do not match assets.lock.json` |
| 缓存属于不同 lock | `InstallState` | `assets cache entry belongs to a different lock` |
| 离线且无缓存/来源 | `InvalidArgument`/`InstallState` | 必须含 `--from-dir`（对齐 acquire_v2） |

错误变体现有集合见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/error.rs:13 )；不新增 `NodeFallback`/`Skip` 类变体，不允许把失败降级为 warning。

### 4.4 26 vs 23：必须先裁决，禁止静默改

**事实**：

- manifest 常量 `MINIPROGRAM_TYPINGS_FILE_COUNT = 26`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:84 )。
- 打包器 allowlist 允许 23，且 `package.json` 被 `forbidden_runtime_path` 再次拒绝，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1095 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:42 )。
- 契约要求 26，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:333 )。
- 因此：用真实 typings 目录打包，当前会失败：`package.json` 先被 `forbidden_runtime_path` 拒绝，`README-en.md`/`typings.json` 被 allowlist 拒绝；要满足 26 必须同时放行全部 3 个（包括运行期禁止的 `package.json`），只放行一部分则计数仍不等。

**两个可选裁决（必须由合同 owner 单独提交，不由实现者选择）**：

| 选项 | 内容 | 需要改动的冻结面 | 风险/前置 |
| --- | --- | --- | --- |
| A（推荐） | payload 固定 23：`index.d.ts`、`LICENSE`、`README.md`、`CHANGELOG.md`、`types/**`；`package.json`/`README-en.md`/`typings.json` 只在上游存在，不进入包 | 契约 §4.1 typings 行（fileCount 26→23）、`MINIPROGRAM_TYPINGS_FILE_COUNT`、manifest 示例、P6-3 的 26 假设 | 需要 F1/P6 确认 `types` 解析不依赖 `package.json`；P6-3 探针已记录缺失 package.json 仍可回退到 `index.d.ts`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:98 ) |
| B | 保持 26：把 `typings/**/package.json` 从运行期禁止中豁免，并把 `README-en.md`、`typings.json` 加入 allowlist | 契约 + `forbidden_runtime_path` + allowlist + 负向扫描测试 | 需要安全评审；会把 Node 包元数据重新带入 payload，削弱“无 Node 运行时”的负向扫描 |

无论选哪个，lock 必须同时写 `upstream.fileCount: 26` 与 `payload.fileCount`，并列出 `extract.exclude[]` 及原因。这样“26/23”从隐藏冲突变成可审计决策。

**在裁决前**：provisioning/打包器对真实 typings 必须 fail closed，测试应断言错误（例如 `payload fileCount 23 does not match the frozen contract 26`），不能 `#[ignore]`、不能跳过、不能只改常量。

---

## 5. CI / clean-PATH / 网络审计与测试计划

### 5.1 现在（不下载、不联网）就能执行

> 下表中的新增文件/测试路径是**计划路径，尚未存在**；现有路径可按行号打开。

| 测试 | 位置（计划） | 输入 | 断言 |
| --- | --- | --- | --- |
| lock 结构/拒绝矩阵 | crates/wxml-delivery/src/assets_lock.rs（新增，`#[cfg(test)]`） | 合成 JSON | schema、未知字段、重复路径、大小写冲突、坏 hex/SRI、占位 0、预算 |
| lock↔常量 drift | crates/wxml-delivery/tests/assets_lock.rs（新增） | 合成 lock | 每个引擎/typings 常量单独漂移都失败；26/23 冲突显式失败 |
| 合成 npm tarball 解包 | crates/wxml-delivery/tests/provisioning.rs（新增） | 测试内用 `tar` crate 构造 `package/` 归档 | 见 §5.2 |
| 本地引擎 tarball 的 `--from-dir` | crates/wxml-delivery/tests/provisioning.rs（新增，opt-in） | /tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz | size/SRI/SHA-256/122 条目/顶层 `package/`/逐文件树摘要 |
| 缓存命中重校验/损坏 | 同上 | 合成 tarball + 篡改缓存 | 一字节/缺文件/多文件/改 mode 全部失败 |
| 打包确定性（引擎真实、typings 合成） | crates/wxml-delivery/tests/native_package.rs（extend） | `TSGO_NATIVE_ENGINE_DIR=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package` | 两次构建归档 SHA-256 相同；manifest/index/SHA256SUMS 一致 |
| 打包 lock 输入拒绝 | 同上 | 带 `engine/` 的 `--input` | `InvalidArgument`，不写输出 |
| 安装器拒绝 lock 派生 drift | crates/wxml-delivery/tests/installer_security.rs（extend） | 由 lock 构建的包 + 篡改 typings/引擎/许可 | 激活前拒绝，`current` 不变 |

### 5.2 `provisioning.rs` 的具体用例与 fixtures

fixtures（全部在测试内构造，落盘到 `WXML_TEST_TMPDIR` 或 `/tmp/opencode`，不进入仓库）：

1. `npm_tarball(files, prefix="package/", modes)`：用 `tar::Builder` 生成 gzip；包含正常文件、`0664`、目录、PAX 头。
2. 恶意变体：绝对路径、`..`、symlink、hardlink、FIFO、字符设备、重复路径、大小写冲突、超长路径、超预算 PAX/大小。
3. lock 变体：正确 lock、错误 SRI、错误 SHA-256、错误 size、错误 treeDigest、错误 bundle、占位 0、缺 `exclude`、多余 `exclude`。
4. 缓存变体：正确条目、翻转一字节、删一个文件、加一个文件、改 mode、替换为 symlink、`provenance.lockSha256` 不匹配。

用例名（建议）：

- `offline_without_cache_or_from_dir_fails_closed`
- `from_dir_verifies_sri_size_and_sha256_before_extraction`
- `sri_sha256_or_size_mismatch_leaves_no_cache_entry`
- `extraction_strips_package_prefix_and_normalizes_modes`
- `extraction_rejects_traversal_links_devices_duplicates_and_budget`
- `unclassified_upstream_files_fail_instead_of_being_dropped`
- `payload_file_count_tree_digest_and_bundles_match_lock`
- `cache_hit_rehashes_every_file_and_rejects_lock_drift`
- `cache_eviction_and_reprovision_are_deterministic`
- `provisioning_never_executes_node_or_network`（`--offline` + tripwire PATH）

配套断言：
- `upstream.treeDigest`/`payload.treeDigest` 与 `verify_v2::tree_fingerprint` 共用同一行拼接 helper；upstream 根为 `""`，payload 根为 `packageRoot`；
- 固定 lock 的 golden `treeDigest`，并用临时目录物化文件后与 `verify_v2::tree_fingerprint` 逐字节比较；
- 两次 provision 到不同 cache 得到相同 payload digest；
- 缓存命中不写 lock、不写输出目录；
- 所有错误在 `assets/` 下不留条目。

### 5.3 现在**不能**执行（必须等待资产/裁决/环境）

1. 真实 typings tarball 的 SRI/SHA-256/解包/逐文件哈希：本地没有 tarball，只有被 gitignore 的 npm 副本；不能把该副本当作上游 provenance。
2. 真实 23/26 最终 payload 的打包：取决于 §4.4 合同裁决。
3. 用真实 typings 的完整包在隔离环境做 F3-03 验收：缺裁决、缺 lock、缺 clean-env 打包。
4. 网络路径（下载/redirect/跨源拒绝）：本环境不联网；只能用 loopback fixture 或 mock，不能声称真实 registry 行为已验证。
5. 组装/安装一个“真实 Rust 发布包”：当前 `real_asset.rs` 明确只把 `engine/` 来自真实 tarball，`bin/`、`typings/` 是 fixture，且不关闭任何 F3 ID，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/real_asset.rs:21 )。

### 5.4 CI 要求

1. 新增 `native-assets` job（clean PATH，无 Node/npm）：
   - `cargo test -p wxml-delivery --test assets_lock --test provisioning --test native_package --test installer_security --test native_delivery`
   - `WXML_REQUIRE_ASSET_LOCK=1`：lock 缺失/未裁决时硬失败，不跳过。
   - `PATH` 只含构建工具与 tripwire shim；断言 tripwire marker 未创建。
2. 新增 `provision-assets` 手动 workflow（唯一允许联网）：
   - 从 lock 下载 → 校验 SRI/SHA-256/size → 解包 → 逐文件校验 → 上传 cache artifact。
   - 产出 `lock.candidate.json`、`provenance.json`、缓存摘要；人工评审后独立提交 lock。
   - 该 workflow 不进入普通 push/PR 的阻塞路径，但 lock 变更 PR 必须附其运行结果。
3. 网络审计：
   - 优先 `unshare -rn`；不可用时用 `HTTPS_PROXY=http://127.0.0.1:9`/`HTTP_PROXY` 作为 sink，并断言 provisioning 只走本地路径。
   - 记录“测试没有发起连接”的方式；不得把 PATH shim 当作进程/网络审计（合同 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1157 )；clean-env 证据同样指出 shim 不能排除绝对路径，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-f3-clean-env.md:63 )）。
4. 确定性：
   - 默认 CI 没有 10 MB 引擎 tarball 时，确定性测试会跳过；必须把“跳过”视为未证明，而不是通过。
   - 推荐在 `native-assets` job 中预置 7.1.0-beta tarball（CI cache/artifact，不提交仓库），设置 `TSGO_NATIVE_ENGINE_DIR`/`TSGO_NATIVE_TARBALL`，并设 `WXML_REQUIRE_REAL_ENGINE=1` 使缺失硬失败。
5. 文档/契约门禁：
   - lock 变更必须同时更新契约表、manifest 常量、打包器 allowlist、测试期望；CI 的 `assets_lock` 测试覆盖这四项的一致性。
   - 禁止在 provisioning 代码中出现 `latest`/`dist-tags`/环境变量选源；可用静态 grep gate（测试内读源码字符串）辅助，但不作为唯一证据。

### 5.5 建议命令（实现后）

```bash
# 1. 结构/拒绝（无网络、无引擎）
cargo test -p wxml-delivery --test assets_lock --test provisioning

# 2. 合成 tarball 的 from-dir/offline（无网络）
cargo test -p wxml-delivery --test provisioning -- --nocapture

# 3. 本地引擎真实字节 + 合成 typings 的确定性
TSGO_NATIVE_ENGINE_DIR=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package \
WXML_REQUIRE_REAL_ENGINE=1 \
cargo test -p wxml-delivery --test native_package -- --nocapture

# 4. 现有真实资产安装回归（不下载）
TSGO_NATIVE_TARBALL=/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz \
WXML_REQUIRE_REAL_ENGINE=1 \
cargo test -p wxml-delivery --test real_asset -- --nocapture

# 5. 全量交付测试
cargo test -p wxml-delivery
```

---

## 6. 迁移/回滚/缓存/两版本共存

### 6.1 lock 身份放入哪里

| 位置 | 现状 | 本次设计 |
| --- | --- | --- |
| manifest | `engine.distribution{Url,Integrity,Sha256,Size,FileCount,UnpackedSize}` 与 `typeAssets[].{fileCount,sha256}` 已存在，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:103 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:142 ) | 不改 schema；lock 的对应字段必须与 manifest 常量相等，打包器断言；不新增 `assetsLock` 字段（`deny_unknown_fields` 会破坏兼容） |
| `provenance.json`（payload） | 已有 schemaVersion 1、package/version、engine 与 distribution（`pinned-expected`），见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1233 ) | 新增 `assetsLockSha256`、每个 asset 的 `payloadTreeDigest`、`tarball` 摘要；仍是普通 payload 文件，被 `files[]`/SHA256SUMS 覆盖 |
| `state.json` | `StateEngine{name,version,sourceCommit,protocol,apiSchema,astHeader}`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/state.rs:41 ) | 本轮不改形状；引擎版本/gitHead 已绑定。若未来要记录 payload digest，需要 state schema 3 + 迁移规则，单独合同评审 |
| journal | `op,target,previous,state,startedAt`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/state.rs:141 ) | 本轮不改；目标树不可变且恢复时重新 `verify_tree_v2`，lock 身份通过 `versions/<v>/manifest.json` + `provenance.json` 传递 |

### 6.2 缓存失效

- 缓存键 = `<payloadTreeDigest>.<lockSha256[..16]>`；lock 变化（即使 payload 未变）→ 新键；旧条目不会被新 lock 读取。
- 旧键条目是可丢弃缓存，可由下一次 provisioning 清理；不得原地覆盖或修补。
- P9 仍校验 `provenance.lockSha256` 与目录名一致，防御目录被手工改名/复制。
- 已安装版本不引用缓存：`acquire_v2` 解包到 `tmp` 后 rename 到 `versions/`，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/acquire_v2.rs:103 )；缓存删除/损坏不影响已安装版本。
- 缓存损坏 → fail closed；不能让打包器“回退到本地 `--input` 里的 engine/typings”。
- 缓存清理不得删除 `versions/`；两者不在同一目录层级（`cache/` vs `versions/`）。

### 6.3 两版本共存与回滚

- `versions/<v>` 自包含完整 `engine/` + `typings/`；禁止跨版本 symlink/共享资产（会破坏不可变性与离线回滚）。
- N1/N2 可由不同 lock 构建；每个 manifest/provenance 记录自己的资产身份；回滚只验证目标自己的树，见 `verify_native` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/native_install.rs:1057 )。
- 同版本不同内容必须拒绝：现有 `same_version_with_different_content_is_rejected` 已覆盖，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_delivery.rs:960 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/native_install.rs:714 )；因此“换 lock 后重建同版本”会被拒绝，必须升版本。
- legacy 迁移不变：第一次原生安装 `previous=null`，legacy 只读标记，不执行 Node，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/state.rs:183 ) 与契约 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:612 )。
- 回滚目标必须是已验证的 v2；v1 目录字节不变且拒绝，见现有测试 `rollback_refuses_legacy_and_tampered_native_targets` ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_delivery.rs:827 )。

---

## 7. 仍然 BLOCKED、不能声明的内容

1. **F3-03 未关闭**：无 `assets.lock.json`、无 provisioning 实现、无 typings 真实哈希、无真实发布包、无两次真实构建同哈希的默认 CI 证据。合同仍把它列为 planned，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1170 ) 与 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1199 )。
2. **B6 仍 BLOCKED**：release 包/Zed 冒烟未构建运行；spike 包是 Python/占位产物，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1227 )。
3. **26 vs 23 未裁决**：在合同 owner 明确选择前，真实 typings 包无法既满足 manifest 26 又满足 allowlist 23；不得声称“打包器已支持真实 typings”。
4. **真实 typings 的 provenance 未知**：本地只有 `.gitignore` 的 npm 安装副本；tarball SRI/SHA-256 未重算。不能用该副本替代上游证据。
5. **clean-PATH/网络审计未执行**：本次没有运行 provisioning 或打包；没有 `unshare`/代理 sink 证据；PATH shim 不能替代进程/网络审计。
6. **F3-13 未触及**：平台/ABI/glibc/musl 仍无新证据，见 ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1180 )。
7. **没有 Node 删除/默认切换**：默认后端仍是 Node；`backend_tsgo` 未接线；F3-15 未关闭。
8. **不能把本文件的 JSON 示例当作已实现 schema**：字段形状是计划；实现前需合同评审，尤其是 `payload`/`exclude`/`provenance` 的字段名。
9. **不能声称许可评审完成**：引擎 Apache-2.0、typings MIT 的随包文件与 `THIRD_PARTY_LICENSES.md` 仍需真实包构建后评审（B7）。
10. **不能声称 lock 提供发布者身份**：它只固定内容摘要；无签名，仍是 ADR-0007 的残留风险。

---

## 8. 有序实施清单（每项独立评审/提交）

| # | 变更 | 精确文件/锚点 | 依赖 | 验收门禁 | 回滚 |
| --- | --- | --- | --- | --- | --- |
| L1 | 新增 lock 数据模型、解析、规范化、常量对账；无 I/O | 新增 crates/wxml-delivery/src/assets_lock.rs；crates/wxml-delivery/src/lib.rs（导出）；crates/wxml-delivery/tests/assets_lock.rs | 无 | `cargo test -p wxml-delivery --test assets_lock`；占位/drift/未知字段拒绝 | 纯新增；不接线即无行为变化 |
| L2 | npm-tarball 解包器 + 安全规则 + 预算 | 新增 crates/wxml-delivery/src/asset_extract.rs；复用 archive.rs 的路径/预算检查 | L1 | `provisioning` 的安全用例全绿；正式包解包测试不变 | 删除模块；正式包路径不动 |
| L3 | provisioning 命令：from-dir/offline/cache/provenance/逐文件校验 | 新增 crates/wxml-delivery/src/provision.rs；crates/wxml-delivery/src/bin/wxml-lsp-package.rs 子命令；crates/wxml-delivery/tests/provisioning.rs | L1/L2 | 不联网即可跑：from-dir、缓存命中/损坏、lock drift、无 Node tripwire | 命令可移除；不影响现有打包 |
| L4 | 下载路径 + 源策略 + 大小/SRI/SHA 先验后解包 | provision.rs；复用 source.rs 策略与 sri.rs | L3 | loopback fixture 或 mock；离线默认仍不触发；无跨源/凭据 | 关闭 `--allow-download` 即回到 L3 |
| L5 | 打包器消费 lock/cache；拒绝 `--input` 携带 engine/typings；provenance 增字段 | package.rs:652-721、1233-1287；bin/wxml-lsp-package.rs | L3 | 真实引擎 + 合成 typings 的确定性；lock/输入 drift 拒绝；不产生部分输出 | `--assets-lock` 可选时回退旧路径（但默认要求） |
| L6 | 安装器校验 provenance 的 treeDigest/lock 摘要（字段存在时） | verify_v2.rs:42-135 或 native_install.rs:1057 | L5 | 篡改 typings/engine 在激活前拒绝；旧包仍可安装 | 检查可单独 revert；manifest schema 不变 |
| L7 | 26/23 合同裁决 + 真实 typings lock 条目 | docs/reference/tsgo-delivery-contract.md:328-339；manifest_v2.rs:84；package.rs:1095；assets.lock.json | L1-L5；合同 owner | 裁决文档 + lock + 常量 + allowlist 同一次评审；真实 tarball SRI/SHA/逐文件哈希 | 不合并即保持 BLOCKED |
| L8 | F3-03 真实包验收证据 | 真实 Rust 包 + 隔离 Linux；docs/evidence/<run>-tsgo-delivery-acceptance.md | L7 | 布局/许可/负向扫描/SHA256SUMS/两次构建同 SHA-256/clean PATH | 证据可撤回；不改变代码 |

实施顺序的理由：L1-L4 可以在**不裁决 26/23、不下载 typings**的情况下完成并用合成 fixture 测试；L5 把 lock 变成实际约束；L7 是唯一能把 F3-03 从 BLOCKED 推进到可验收的前置；L8 才是验收运行。

---

## 9. 引用核对（本文关键行）

- F3-03 验收内容与路径： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1170 )
- F3-13 平台/ABI： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1180 )
- 证据要求（archive/manifest/tree digest/进程/网络）： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1185 )
- `assets.lock.json` 最小设计： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:711 )
- provisioning 算法： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:757 )
- 安全解包： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:783 )
- 树校验： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:797 )
- 确定性归档： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:233 )
- 升级审查/lock 独立提交： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1139 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-delivery-contract.md:1275 )
- 冻结常量： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:41 )
- typings 计数常量： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/manifest_v2.rs:84 )
- 打包器 allowlist： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/package.rs:1074 )
- 负向扫描： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:23 )
- bundle 算法： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/type_assets.rs:191 )
- 树 fingerprint： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/verify_v2.rs:303 )
- SRI/size/SHA 一次校验： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/sri.rs:152 )
- 正式包解包/模式： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/archive.rs:344 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/archive.rs:588 )
- state/journal 形状： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/state.rs:41 )、( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/state.rs:141 )
- 安装事务/fingerprint： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/src/native_install.rs:675 )
- 现有确定性测试（opt-in）： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/native_package.rs:344 )
- 现有真实资产 harness 的边界： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-delivery/tests/real_asset.rs:21 )
- clean-env 已证明/未证明： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-f3-clean-env.md:17 )
- P6-3 的 26/23 风险记录： ( /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-18-tsgo-native-type-assets-plan.md:74 )

本文不改变任何 AC/ledger/F3 状态；F3-03 仍是 **BLOCKED**。
