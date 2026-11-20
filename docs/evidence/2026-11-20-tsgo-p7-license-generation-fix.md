# P7 发布许可 P1 修复：确定性 Rust 依赖许可清单（2026-11-20）

## 0. 结论摘要

针对 ` docs/evidence/2026-11-20-tsgo-p7-release-license-audit.md` 的 **P1（`THIRD_PARTY_LICENSES.md` 只有 engine/typings，缺 Rust crate 名称/许可文本）**，本次修复让
`scripts/build-release.sh` 在打包前用 `cargo metadata --format-version 1 --locked --offline`
生成完整、确定性的 Rust 依赖许可清单，并把它与已验证的上游资产许可合并进同一个
`THIRD_PARTY_LICENSES.md`。同时扩展了 `provenance.json`，新增通道级
`release-provenance.json`，并增加生成器与安装器测试。

本轮补齐了上一版遗留的 11 个 Zed wasm crate 许可全文缺口：crate 本地没有许可文件时，
生成器按 Cargo license expression 选取随生成器 vendored 的 SPDX 标准全文，并在清单中
逐 crate 记录 `text_source=local|spdx`，不做静默替换。

| 项 | 结果 |
| --- | --- |
| 两个 Rust 二进制依赖（root workspace） | **124/124** crate 有本地许可文本 |
| Zed wasm 依赖（`editors/zed` workspace） | 88 个 crate，**88 有文本**（77 本地 + 11 vendored SPDX） |
| 去重后第三方 crate | **155**（124 + 88，57 个跨 workspace 重叠） |
| 有许可文本的 crate | **155 / 155**（144 本地 + 11 vendored SPDX） |
| 生成的不同许可文本 | **117** 份（按字节去重；新增 Apache-2.0、MIT、LLVM-exception 三份） |
| `text_source=local` / `text_source=spdx` | **144 / 11**（每行显式记录） |
| 既无 expression 又无文本的 crate | **0**（fail-closed 路径已测试） |
| 首方项目许可 | **BLOCKED owner decision**（仓库仍无 LICENSE / Cargo license 字段） |
| 归档许可清单大小 | `467,493` B（上一版 `448,597` B） |
| 归档 SHA-256 | `793a4f216e2d3ab9b3a707f27720745bd688f2989b02de81a6acd7c5132e4077` |
| 构建可复现 | 生成器两次相同输入字节相同；许可清单只依赖 `SOURCE_DATE_EPOCH` |
| **P1 判定** | **解除**：155 个 crate 均有名称/版本/expression/文本，且文本来源逐行可见 |
| 剩余 owner 决策 | 首方 LICENSE（R2）；`ALLOW_UNLICENSED` 是否允许任何本地构建 |

一句话：**发布包现在能回答“静态链接了哪些 Rust crate、各自什么许可、许可文本在哪、
文本来自 crate 本地还是 vendored SPDX”，155/155 个第三方 crate 都有可分发文本；
首方项目许可仍由 owner 决定，本次没有替 owner 选择。**

## 1. 审计对象与边界

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| 构建时 HEAD | `180fa42b7e41863b18280f1ca8dedfdab9211e35`（共享 worktree，含其他并行未提交改动） |
| 构建输出 E | `/tmp/opencode/p7-license-fix-release-e` |
| 构建缓存 | `/home/zxg/.cache/wxml-lsp-p7-post-delete-c`（已有 provisioned assets cache） |
| `SOURCE_DATE_EPOCH` | `1763683200`（`2025-11-21T00:00:00Z`） |
| 写操作 | 修改 `scripts/build-release.sh`、`crates/wxml-delivery/src/licenses.rs`、`crates/wxml-delivery/src/bin/wxml-lsp-package.rs`、本证据文件；新增 `crates/wxml-delivery/assets/spdx/*.txt`；未 commit |
| 未改 | Node 后端、默认 backend、ledger、`assets.lock.json`、engine/typings 内容、README/ADR/plan |

## 2. 实现

### 2.1 许可生成器（`wxml-delivery::licenses`，生成器版本 2）

- 输入是两个已保存的 `cargo metadata` 文档：
  - root workspace：`--filter-platform x86_64-unknown-linux-gnu`，root package
    `wxml-lsp` 与 `wxml-delivery`（后者提供 `wxml-lsp-install`）；
  - `editors/zed` 独立 workspace：`--filter-platform wasm32-wasip2`，root package
    `zed-wxml`。
- 只沿 `dep_kinds.kind == normal`（或旧格式的空 `dep_kinds`）的边遍历；`dev`/`build`
  依赖不进入清单。
- 每个第三方 crate 记录 `(name, version)`、Cargo `license` 表达式、workspace 归属；
  按 `(name, version)` 去重并排序。
- 文本解析顺序：
  1. 从 `manifest_path` 的父目录收集 `LICENSE*`/`NOTICE*`/`COPYING*`/`UNLICENSE*`
     普通文件（大小写不敏感，跳过空文件、目录、逃逸符号链接）；
  2. `license_file` 声明的文件必须存在，缺失即构建失败；
  3. **只有当 crate 完全没有本地文本时**，才按 license expression 选取
     `crates/wxml-delivery/assets/spdx/` 中 vendored 的 SPDX 标准全文。
  本地文本优先，因此已有 144 个 crate 的字节不变；vendored 文本只覆盖 11 个缺口。
- expression 解析不做“二选一”：
  - `MIT OR Apache-2.0` 同时收录 MIT 与 Apache-2.0；
  - `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` 同时收录
    Apache-2.0、LLVM-exception、MIT；
  - `[LICENSE] WITH [EXCEPTION]` 收录完整许可文本和 exception 文本；
  - 无法识别的 identifier（如 `LicenseRef-Proprietary`）不猜，仍进入
    “unavailable” 缺口清单。
- 每个 crate 行新增 `text source` 列（字段 `text_source`），取值 `local` / `spdx` /
  `none`；文档头新增
  `<!-- text-coverage: third-party=155 local=144 spdx=11 unlicensed=0 missing=0 -->`。
- vendored 文本按内容 SHA-256 与本地文本一起去重；`License texts` 章节对 `spdx`
  条目写明 `Source: spdx canonical …` 和
  `Canonical license text, copyright notices remain with upstream distribution.`。
  不添加任何 crate 专属版权行。
- crate 既无 expression 又无任何文本 → 构建失败；只有显式 `ALLOW_UNLICENSED=1`
  才接受，并在文档中单独警告该清单 local-only、不可发布。
- 有 expression 但本地与 vendored 都没有文本 → **不省略** crate，表格和
  “License texts unavailable in the local cache”章节都显式列出。
- 输出只有 `SOURCE_DATE_EPOCH` 一个时间值；排序全部使用稳定字节序，不依赖 locale、
  `HashMap` 迭代或当前时间。生成器编号升至 `wxml-lsp-license-generator/2`。

### 2.2 vendored SPDX 文本

来源：`https://github.com/spdx/license-list-data/tree/main/text`，构建时已静态落入
仓库，运行时/构建时都不联网。

| 文件 | SHA-256 | 用途 |
| --- | --- | --- |
| ` crates/wxml-delivery/assets/spdx/Apache-2.0.txt` | `074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff` | `Apache-2.0` |
| ` crates/wxml-delivery/assets/spdx/LLVM-exception.txt` | `e34c58338bd89d43e709e226610d8f32b3e3c47f4ad9a99a8dc1d4ac7842488e` | `Apache-2.0 WITH LLVM-exception` 的 exception 部分 |
| ` crates/wxml-delivery/assets/spdx/MIT.txt` | `b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` | `MIT` |
| ` crates/wxml-delivery/assets/spdx/Unicode-3.0.txt` | `f7db81051789b729fea528a63ec4c938fdcb93d9d61d97dc8cc2e9df6d47f2a1` | `Unicode-3.0` |
| ` crates/wxml-delivery/assets/spdx/Zlib.txt` | `bfb1112d49db5b1daecdfef24bd7e2f3ea0bafb33aa67aa0ab51e2bf8407c03d` | `Zlib` |

说明：

- MIT 文本保留 SPDX 的 `<year> <copyright holders>` 占位符，Apache-2.0 保留
  appendix 的 `[yyyy] [name of copyright owner]` 占位符；Unicode-3.0 文本中的
  `Copyright © 1991-2023 Unicode, Inc.` 是 SPDX 标准文本自身的一部分。
- 没有把任何 crate 的名称、作者或年份写进文本；没有伪造版权行。
- 这些文本按 expression 提供，不声称是某个 crate 上游包的逐字节副本；清单中明确
  标注 `not copied from the upstream package`。

### 2.3 `THIRD_PARTY_LICENSES.md` 结构

最终文件包含：

1. `## Upstream assets`：保留原有 engine/typings 许可全文（2 个资产、3 个许可文件）；
2. `## Rust dependencies`：生成器输出，含
   - `## Dependency graphs`：每个 workspace 的 target、root package、crate 数；
   - `## Rust dependency inventory`：155 行 crate 清单（名称/版本/expression/workspace/
     文本数/`local|spdx|none`）；
   - `## First-party packages`：8 个首方包与
     `first-party-license: BLOCKED owner decision`；
   - `## License texts`：117 份去重许可全文，`spdx` 条目带来源说明；
   - `## License texts unavailable in the local cache`：本轮为 `None.`；
   - `## Reproducibility`：generator id/version、source commit、`SOURCE_DATE_EPOCH`。

doc 头部记录了：

```text
<!-- generated-by: wxml-lsp-license-generator/2 -->
<!-- source-commit: 180fa42b7e41863b18280f1ca8dedfdab9211e35 -->
<!-- source-date-epoch: 1763683200 -->
<!-- source-date: 2025-11-21T00:00:00Z -->
<!-- text-coverage: third-party=155 local=144 spdx=11 unlicensed=0 missing=0 -->
```

构建脚本对 generator marker 和 `text-coverage` marker 做 fail-closed 检查；在
`ALLOW_UNLICENSED=0` 的发布构建中，`missing` 必须为 0。

### 2.4 `provenance.json` 与通道级 `release-provenance.json`

`provenance.json`（archive 内）新增：

- `sourceCommit`：构建时 `git rev-parse HEAD`；
- `sourceDateEpoch`：本次构建使用的 epoch；
- `archivePayloadDigest`：按 `path NUL sha256 LF`、对 `manifest.files[]` 中除
  `provenance.json` 外的全部 payload 排序计算；消费者可用 manifest 重算。

`release-provenance.json`（**archive 外，release 通道目录**）新增并记录：

- `sourceCommit`、`sourceDateEpoch`、`builtAt`；
- `archive.sha256` / `archive.size` / `archive.integrity`（SRI）；
- `payload.archivePayloadDigest`、`payload.manifestSha256`、`payload.provenanceSha256`。

原因：archive 哈希只有在 archive 写完后才知道，把 archive SHA-256 放进 archive 内部
会自引用。该文件与 `index.json` 同级，不伪造“archive 内已有该字段”。脚本帮助文本
和 README 已注明这一位置语义。

### 2.5 packager allowlist 与安装器

- `THIRD_PARTY_LICENSES.md` 原本就在 `validate_native_input_files` 必需列表与
  `is_allowed_native_path` allowlist 中，因此**不需要新增包内路径**；本次只是把同一
  路径的内容从 448 KB 扩展到 467 KB。
- `release-provenance.json` 位于 archive 外（通道目录），不经过 packager allowlist，
  与 `index.json` 同一处理方式。
- 安装器仍然整树原子安装 `versions/<version>/`；新增测试断言
  `THIRD_PARTY_LICENSES.md` 与 `engine/LICENSE` 安装后字节不变。
- 实际安装验证：安装树中的 `THIRD_PARTY_LICENSES.md` SHA-256 与 release 内完全一致
  （见 §4）。

## 3. 首方项目许可

- 仓库根、各 crate、`editors/zed` 仍无 `LICENSE`/`NOTICE`/`COPYING`；
- 根 `Cargo.toml` 的 `[workspace.package]` 仍无 `license` / `license-file`；
- 生成清单已显式写出 8 个首方包（`wxml-backend-tsgo`、`wxml-delivery`、`wxml-lsp`、
  `wxml-project`、`wxml-syntax`、`wxml-text`、`wxml-types`、`zed-wxml`）并标记：
  `first-party-license: BLOCKED owner decision`。
- 首方包**不参与** vendored SPDX 回退：首方许可不是第三方 expression 缺失问题，
  不能由生成器代选。

本次**没有**替 owner 选择许可证，也没有创建首方 LICENSE 文件。

## 4. 验证

### 4.1 静态检查与测试

```text
$ bash -n scripts/build-release.sh
（退出 0）

$ rustfmt --edition 2024 crates/wxml-delivery/src/licenses.rs \
      crates/wxml-delivery/src/bin/wxml-lsp-package.rs
（通过；未格式化其他包，避免触碰共享 worktree 的既有改动）

$ cargo check -p wxml-delivery --all-targets --offline
Finished dev profile

$ cargo clippy -p wxml-delivery --all-targets --offline -- -D warnings
No issues found

$ cargo test -p wxml-delivery --lib --offline licenses::
9 passed

$ cargo test -p wxml-delivery --offline
312 passed, 29 ignored
```

针对性单测覆盖：

- 相同 metadata/epoch 两次生成字节相同；
- 两份相同许可文本只输出一次；
- 本地文本优先于 vendored SPDX 文本（`text_source=local`）；
- 无本地文本的 `MIT OR Apache-2.0` / `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`
  分别得到 2 / 3 份 vendored 文本，且 `text_source=spdx`；
- 11 个 Zed wasm crate 的真实名称/版本/expression 全部得到 `text_source=spdx`，
  `missing=0`；
- 无 expression 无文本仍 fail-closed；`allow_unlicensed` 时显式警告；
- 有 expression 但本地和 vendored 都不匹配（`LicenseRef-Proprietary`）仍被列入
  unavailable，不静默省略；
- `license_file` 声明缺失时报错；
- 空 root package 列表报错；
- `archivePayloadDigest` 的规范化顺序与前像格式。

### 4.2 完整 release 构建（E，最终源码）

```bash
WXML_BUILD_CACHE=/home/zxg/.cache/wxml-lsp-p7-post-delete-c \
SOURCE_DATE_EPOCH=1763683200 \
  bash scripts/build-release.sh --output /tmp/opencode/p7-license-fix-release-e
```

生成器输出：

```text
licenses: 155 third-party crates (155 with text: 144 local, 11 vendored SPDX; 0 without text, 0 unlicensed), 117 distinct texts
first-party-license: BLOCKED owner decision
```

构建结果：

| 项 | 值 |
| --- | --- |
| Archive SHA-256 | `793a4f216e2d3ab9b3a707f27720745bd688f2989b02de81a6acd7c5132e4077` |
| Archive size | `15,143,353` B（上一版 144/155 时 `15,141,762` B） |
| archive 条目 | 159（不变） |
| `manifest.files` | 157（不变） |
| `THIRD_PARTY_LICENSES.md` | `467,493` B，SHA-256 `d7ef55139a47df155a9bd95126de18b8c7c6370675afbda2e3a8b00dbab7583c` |
| `provenance.json` SHA-256 | `6ffdfdb0c6ce97f392ab1fd89e908539a806216f1706555a1dad5ee6a8a5185a` |
| `release-provenance.json` SHA-256 | `dd37e3d4e0e7173d5459ca25ad0241ae61efcb7e579bcbf7bebc2380f4b2154b` |
| `archivePayloadDigest` | `cbad03d393e5e0b6557f91ae27b995d4b69d55f14f297d4c560c9c806fa565d7` |
| `manifest.json` SHA-256 | `d93b54698dccfaaf6a2bc866f83fdcd2a90e0d7fef94fcb8f893f32c499d0ec0` |
| 构建内校验 | `verify-package`、`SHA256SUMS`、`text-coverage missing=0` 全部通过 |

本次只重建一次（E），未做第二次逐字节对比；生成器确定性由单测覆盖，且清单内容只
依赖 `cargo metadata`、vendored 文本字节和 `SOURCE_DATE_EPOCH`。

### 4.3 安装与安装器校验

```text
$ wxml-lsp-install verify-package --from-dir /tmp/opencode/p7-license-fix-release-e \
    --version 0.1.0 --offline
verified native package wxml-lsp 0.1.0 (linux-x86_64)
protocol: 2  engine: 7.1.0-beta  files: 157
archive sha256: 793a4f21…2e4077

$ wxml-lsp-install install --root /tmp/opencode/p7-license-fix-install-e \
    --from-dir /tmp/opencode/p7-license-fix-release-e --version 0.1.0 --offline
installed 0.1.0 (native)

$ sha256sum .../versions/0.1.0/THIRD_PARTY_LICENSES.md \
            .../p7-license-fix-release-e/wxml-lsp-0.1.0-linux-x86_64/THIRD_PARTY_LICENSES.md
d7ef5513…ab7583c  （两者相同）

$ wxml-lsp-install verify --root /tmp/opencode/p7-license-fix-install-e
verified 0.1.0 (native)
engine version: 7.1.0-beta
files: 157  bytes: 42310099
```

### 4.4 包内关键词抽查

`THIRD_PARTY_LICENSES.md` 中 `serde`、`libc`、`rustls`、`tokio`、
`zed_extension_api`、`unicode-ident` 均出现；155 行 inventory、117 个 `###` 许可文本
小节（另 2 个 `###` 是上游资产），其中 11 行 `text source` 为 `spdx`、144 行为
`local`。`grep -c '^| `'` 为 155，`grep -c '| spdx |'` 为 11，`grep -c '| local |'`
为 144。

## 5. 11 个 Zed wasm crate 的处理

这些 crate 的 registry 包目录中确实没有 `LICENSE*`/`NOTICE*`/`COPYING*`/`UNLICENSE*`
文件。本轮不下载、不伪造上游文件，而是按 expression 提供 vendored SPDX 标准文本：

| crate | version | license expression | 文本数 | text_source |
| --- | --- | --- | ---: | --- |
| `auditable-serde` | 0.8.0 | `MIT OR Apache-2.0` | 2 | `spdx` |
| `wasm-encoder` | 0.227.1 | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | 3 | `spdx` |
| `wasm-metadata` | 0.227.1 | 同上 | 3 | `spdx` |
| `wasmparser` | 0.227.1 | 同上 | 3 | `spdx` |
| `wit-bindgen` | 0.41.0 | 同上 | 3 | `spdx` |
| `wit-bindgen-core` | 0.41.0 | 同上 | 3 | `spdx` |
| `wit-bindgen-rt` | 0.41.0 | 同上 | 3 | `spdx` |
| `wit-bindgen-rust` | 0.41.0 | 同上 | 3 | `spdx` |
| `wit-bindgen-rust-macro` | 0.41.0 | 同上 | 3 | `spdx` |
| `wit-component` | 0.227.1 | 同上 | 3 | `spdx` |
| `wit-parser` | 0.227.1 | 同上 | 3 | `spdx` |

说明：

- 这 11 个 crate 在 inventory 表格中都显式出现，`text source` 为 `spdx`；
- 文中 `Source: spdx canonical …` 标明“按 expression 提供、不是上游文件逐字节副本”；
- `Canonical license text, copyright notices remain with upstream distribution.`
  明确版权声明仍由上游分发承担；
- 若未来出现本地和 vendored 都覆盖不到的 expression，仍会进入 unavailable 章节，
  不会被静默当作已包含。

注意：root workspace 的 **124/124** crate 都有本地文本。审计旧报告中的 109 个是
`cargo tree -e normal` 口径；`cargo metadata` resolve 还会包含 Cargo.lock 中未激活的
optional crate（如 `quinn`/`rand`/`tinyvec` 系列 15 个），因此 124 是保守的超集，
不会漏报。

## 6. 风险与判定

| ID | 级别 | 状态 | 说明 |
| --- | --- | --- | --- |
| P1-Rust 清单缺失 | **P1 → 解除** | 已修复 | 155 个 crate 均有名称/版本/expression；155 个有文本；0 个静默省略 |
| P1-wasm 文本缺口 | **P1 → 解除** | 已修复 | 11 个 Zed wasm crate 通过 vendored SPDX 文本补齐，逐行记录 `text_source=spdx` |
| 首方 LICENSE | **P2（剩余，owner）** | BLOCKED | 仓库与 Cargo 均未声明；本次不替 owner 选择许可证 |
| 归档 SHA 不在 provenance | **P2 → 解除** | 已修复 | 通道级 `release-provenance.json` 记录 archive SHA-256/大小/SRI，并明确在 archive 外 |
| 构建 commit/时间不可见 | **P3 → 解除** | 已修复 | `provenance.json` 的 `sourceCommit`/`sourceDateEpoch` + 通道文件 |

**是否解除 P1：**

- 对审计原句“`THIRD_PARTY_LICENSES.md` 只含 engine/typings、无 Rust crate 名称/许可文本”
  而言：**解除**。清单覆盖两个 Rust bin 与 Zed wasm 的全部 155 个第三方 crate。
- 对“发布收件人拿到每个 crate 的许可全文”而言：**本轮解除**。155/155 都有文本；
  11 个 registry 包本身不带许可文件的 crate 使用 vendored SPDX 标准文本，并在包内
  标明来源与版权归属。
- 首方项目许可证仍是 owner 决策，不由本生成器代选；该项保持 BLOCKED。

## 7. 复现命令

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration

# 生成器单测
cargo test -p wxml-delivery --lib --offline licenses::

# 完整构建（使用既有 cache 与固定 epoch）
WXML_BUILD_CACHE=/home/zxg/.cache/wxml-lsp-p7-post-delete-c \
SOURCE_DATE_EPOCH=1763683200 \
  bash scripts/build-release.sh --output /tmp/opencode/p7-license-fix-release-e

# 检查许可覆盖
PKG=/tmp/opencode/p7-license-fix-release-e/wxml-lsp-0.1.0-linux-x86_64
grep -c '^| `' "$PKG/THIRD_PARTY_LICENSES.md"          # 155
grep -c '| spdx |' "$PKG/THIRD_PARTY_LICENSES.md"      # 11
grep -c '| local |' "$PKG/THIRD_PARTY_LICENSES.md"     # 144
grep '^<!-- text-coverage:' "$PKG/THIRD_PARTY_LICENSES.md"
grep '^## ' "$PKG/THIRD_PARTY_LICENSES.md"
jq '{sourceCommit,sourceDateEpoch,archivePayloadDigest}' "$PKG/provenance.json"
jq . /tmp/opencode/p7-license-fix-release-e/release-provenance.json

# 安装与校验
env -u WXML_LSP_BIN "$PKG/bin/wxml-lsp-install" install \
  --root /tmp/opencode/p7-license-fix-install-e \
  --from-dir /tmp/opencode/p7-license-fix-release-e --version 0.1.0 --offline
env -u WXML_LSP_BIN "$PKG/bin/wxml-lsp-install" verify \
  --root /tmp/opencode/p7-license-fix-install-e
```

## 8. 变更文件

-  ` scripts/build-release.sh`（生成器 marker 升到 `/2`，新增 `text-coverage` fail-closed 检查）
-  ` crates/wxml-delivery/src/licenses.rs`（本地优先 + vendored SPDX 回退、`text_source`、`text-coverage`）
-  ` crates/wxml-delivery/src/bin/wxml-lsp-package.rs`（`licenses --help` 与统计输出）
-  ` crates/wxml-delivery/assets/spdx/Apache-2.0.txt`（新增）
-  ` crates/wxml-delivery/assets/spdx/LLVM-exception.txt`（新增）
-  ` crates/wxml-delivery/assets/spdx/MIT.txt`（新增）
-  ` crates/wxml-delivery/assets/spdx/Unicode-3.0.txt`（新增）
-  ` crates/wxml-delivery/assets/spdx/Zlib.txt`（新增）
-  ` crates/wxml-delivery/src/lib.rs`
-  ` crates/wxml-delivery/src/package.rs`
-  ` crates/wxml-delivery/tests/native_delivery.rs`
-  ` crates/wxml-delivery/tests/native_package.rs`
-  ` crates/wxml-delivery/README.md`
-  ` docs/evidence/2026-11-20-tsgo-p7-license-generation-fix.md`（本文件）

未 commit。
