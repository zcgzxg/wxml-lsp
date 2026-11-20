# P7 最终 release 构建、安装、许可覆盖与 native/Zed 冒烟复验（2026-11-20）

## 0. 结论摘要

在 worktree `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`、HEAD
`180fa42`（工作树含未提交的 P1 脚本诊断重试与许可证生成修复）上，用固定
`SOURCE_DATE_EPOCH=1763683200` 重新构建、安装并冒烟验证最终 release。

| 维度 | 结果 |
| --- | --- |
| release 构建 + `verify-package` + `SHA256SUMS` | **PASS**（exit 0） |
| 安装 / `verify` / `env` | **PASS**（exit 0，157 files / 42,291,203 B） |
| `THIRD_PARTY_LICENSES.md` 覆盖 Rust 依赖 | **PASS（清单）**：155 个第三方 crate，144 有本地全文，11 仅 expression |
| 安装树与 archive 的许可文件一致 | **PASS**：4/4 SHA-256 相同；`SHA256SUMS` 158/158 通过 |
| 真实 native LSP 冒烟 | **PASS**：WXML 2 条诊断、TS 0 条、hover `string`、仅 `engine/lib/tsc` 子进程 |
| Zed Xvfb 冒烟 | **PASS：实际 16/16 CHECK**（不是 18；以日志为准），无 Node/server.js |
| P1（TS 诊断 inflight + JSON didOpen） | **PASS**：仍发布 `2322`；stderr 有 rescheduling |
| P2（仅 `Page(...)` 的 documentSymbol） | **仍为 known**：`-32803 the script outline is incomplete` |
| **对外发布许可门禁** | **仍 BLOCKED**：首方 LICENSE 缺失（8 个包）+ 11 个 Zed wasm crate 只有 expression、无全文 |

一句话：**这份归档已解决“Rust 依赖清单缺失”的原 P1，安装/运行/无 Node 全部通过；
但对外发布许可材料仍不完整，不能据此关闭 B7/NF-09。**

## 1. 身份、构建窗口与边界

| 项 | 值 |
| --- | --- |
| Worktree | `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration` |
| HEAD | `180fa42b7e41863b18280f1ca8dedfdab9211e35` |
| 构建窗口（本地） | 2026-11-20 10:44～10:46 +0800（UTC 02:44～02:46） |
| 构建输出 | `/tmp/opencode/p7-license-release` |
| 构建缓存 | `/home/zxg/.cache/wxml-lsp-p7-post-delete-c` |
| `SOURCE_DATE_EPOCH` | `1763683200`（`2025-11-21T00:00:00Z`） |
| 构建版本 | `0.1.0`（未传 `--version`，直接从当前工作树编译，包含未提交修复） |
| 本切片写操作 | 只新增本证据文件；未改生产/测试/脚本/README/ledger；未 commit |

构建日志记录了 `warning: the source worktree has uncommitted tracked changes;
provenance records HEAD 180fa42...`，与工作树实际状态一致。

### 1.1 构建后并行写入（范围声明）

本构建完成（archive mtime `10:46:22`）后，同一共享 worktree 被并行会话继续修改：

- `scripts/build-release.sh` 于 `10:48:42` 改为要求 `wxml-lsp-license-generator/2`
  与 `text-coverage missing=0`；
- `crates/wxml-delivery/src/licenses.rs` 于 `10:50:22` 更新，并新增
  `crates/wxml-delivery/assets/`；
- 当前脚本已不是本归档构建时的 v1 路径。

本证据只覆盖 **10:44～10:46 构建出的 v1 产物**。当前工作树用当前脚本重建会得到
不同（generator/2）产物，不在本证据范围内；本证据没有把 v2 的结果混入。

## 2. 构建命令与结果

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
WXML_BUILD_CACHE=/home/zxg/.cache/wxml-lsp-p7-post-delete-c \
SOURCE_DATE_EPOCH=1763683200 \
  bash scripts/build-release.sh --output /tmp/opencode/p7-license-release \
  > /tmp/opencode/p7-license-release-build.log 2>&1
echo $?   # 0
```

关键日志：

```text
building Rust release binaries (version 0.1.0, linux-x86_64)
   Compiling wxml-lsp v0.1.0 (.../crates/wxml-lsp)
    Finished `release` profile [optimized] target(s) in 58.58s
building Zed extension (wasm32-wasip2)
    Finished `release` profile [optimized] target(s) in 0.13s
warning: editors/zed/grammars/wxml.wasm is absent; the package omits the grammar
licenses: 155 third-party crates (144 with local text, 11 without text, 0 unlicensed), 114 distinct texts
first-party-license: BLOCKED owner decision
verified native package wxml-lsp 0.1.0 (linux-x86_64)
protocol: 2  engine: 7.1.0-beta  files: 157
archive sha256: bd5121ddee1bfaaa1a51f1770d2b8055d218bcbb813b8092106592d464186bb9
BUILD_EXIT=0
```

`wxml-lsp` 在本次构建中重新编译（58.58 s），不是复用旧的 worktree
`target/release/wxml-lsp`。包内二进制来自
`/home/zxg/.cache/wxml-lsp-p7-post-delete-c/cargo-target/release/wxml-lsp`
（mtime `10:46:18`），与包内 `bin/wxml-lsp` 哈希相同（见 §3）。

Zed wasm 由当前 `editors/zed/src/lib.rs`（mtime `09:56`）在 `10:29` 构建进 cache，
构建脚本以 `0.13 s` 复用；包内 `extension.wasm` 与 cache 产物哈希相同
（`ba83770b…d7dd`），不是旧包里的 `488531ea…`。

## 3. 产物身份

| 对象 | 大小 (B) | SHA-256 |
| --- | ---: | --- |
| `wxml-lsp-0.1.0-linux-x86_64.tar.gz` | 15,141,762 | `bd5121ddee1bfaaa1a51f1770d2b8055d218bcbb813b8092106592d464186bb9` |
| `THIRD_PARTY_LICENSES.md` | 448,597 | `118bec0ac6b97f173a754b459fd975bf6fc90e1610f4232837af10683149df72` |
| `bin/wxml-lsp` | 5,539,552 | `c31d9822e937bc622d87fba63b5428bb2e2e65ed9795ca5e7b2e9d87a87348a1` |
| `bin/wxml-lsp-install` | 5,058,736 | `9c80fc2314de44acb590a57881cc86df050f291e981936dec2313b3d6d55cb98` |
| `engine/lib/tsc` | 24,522,912 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| `zed-extension/extension.wasm` | 238,672 | `ba83770b306b440890914cdbd056f0486de016ea1c6da88b2b83eaa18a0d7ddd` |
| `manifest.json` | 32,928 | `e401f7bdd7159d0c3dc36fdbe121a8df70215e7d8dfc8ef8c5821a5d1f37a25c` |
| `provenance.json` | 2,198 | `e0137f922e5e4ebe5049a0e7ee124096ba8065bbf99fdbbbb7d751183d063260` |
| 通道 `release-provenance.json` | 955 | `690340750f44cfb1f3c8586c385cb01b7514940ac0be3922da9368323ef557c8` |

- archive 内 159 个文件，`manifest.files` 157 个，总字节 `42,291,203`。
- `index.json`：`sha256=bd5121dd…186bb9`、`size=15141762`、protocol 2、
  engine `7.1.0-beta`；`verify-package` 重新计算 archive SHA 后一致。
- `provenance.json`：`sourceCommit=180fa42b7e41863b18280f1ca8dedfdab9211e35`、
  `sourceDateEpoch=1763683200`；`release-provenance.json` 的 archive
  SHA/大小/SRI 与 `index.json` 一致。
- archive SHA 与既有 `p7-license-fix-release-c` 逐字节相同
  （`bd5121dd…186bb9`），说明同机同 cache 下可复现。

包内 `version = 0.1.0` 且 `--help` 显示 `native (default)`；`ldd` 只依赖
`libgcc_s` / `libc`，无 libnode。

## 4. 许可覆盖

### 4.1 清单结构

`THIRD_PARTY_LICENSES.md` 448,597 B，包含：

```text
## Upstream assets                  # engine Apache-2.0 + NOTICE，typings MIT
## Rust dependencies
   ## Dependency graphs
   ## Rust dependency inventory    # 155 行
   ## First-party packages          # 8 个首方包，BLOCKED
   ## License texts                 # 114 份去重全文
   ## License texts unavailable ... # 11 个 crate
   ## Reproducibility
```

`###` 小节 116 个 = 2 个上游资产 + 114 份去重许可文本。
`serde`、`libc`、`rustls`、`tokio`、`zed_extension_api`、`unicode-ident`
均能在文件中检索到（出现次数分别为 11/5/20/14/3/5）。

### 4.2 数量

| 项 | 数量 |
| --- | ---: |
| 第三方 crate（去重后） | **155** |
| 有本地许可全文 | **144** |
| 仅 expression、无本地全文 | **11** |
| 既无 expression 又无文本 | **0** |
| 去重后的许可文本 | **114** |
| root workspace（`wxml-lsp` + `wxml-delivery`） | 124，**124/124 有全文** |
| `editors/zed` workspace（`zed-wxml`） | 88，**77/88 有全文** |
| 两个 workspace 重叠 crate | 57 |
| 首方包 | 8（全部无 license/Cargo 字段） |

### 4.3 expression-only 的 11 个 crate

全部来自 `editors/zed` 的 `wasm32-wasip2` 图，包内已显式列出，未静默省略：

```text
auditable-serde 0.8.0                  MIT OR Apache-2.0
wasm-encoder 0.227.1                   Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT
wasm-metadata 0.227.1                  同上
wasmparser 0.227.1                     同上
wit-bindgen 0.41.0                     同上
wit-bindgen-core 0.41.0                同上
wit-bindgen-rt 0.41.0                  同上
wit-bindgen-rust 0.41.0                同上
wit-bindgen-rust-macro 0.41.0          同上
wit-component 0.227.1                  同上
wit-parser 0.227.1                     同上
```

首方 8 个包（`wxml-backend-tsgo`、`wxml-delivery`、`wxml-lsp`、`wxml-project`、
`wxml-syntax`、`wxml-text`、`wxml-types`、`zed-wxml`）在清单中被标记为
`first-party-license: BLOCKED owner decision`。仓库根仍无 `LICENSE`/`NOTICE`/
`COPYING`，根 `Cargo.toml` 仍无 `license`/`license-file` 字段；包根亦无首方
`LICENSE`。

## 5. 安装、`verify`、`env` 与许可一致性

```bash
REL=/tmp/opencode/p7-license-release
ROOT=/tmp/opencode/p7-license-release-install
env -u WXML_LSP_BIN "$REL/wxml-lsp-0.1.0-linux-x86_64/bin/wxml-lsp-install" \
  install --root "$ROOT" --from-dir "$REL" --version 0.1.0 --offline
env -u WXML_LSP_BIN "$ROOT/current/bin/wxml-lsp-install" verify --root "$ROOT"
env -u WXML_LSP_BIN "$ROOT/current/bin/wxml-lsp-install" env --root "$ROOT"
```

最终输出：

```text
installed 0.1.0 (native)
engine: typescript 7.1.0-beta
binary: /tmp/opencode/p7-license-release-install/current/bin/wxml-lsp

verified 0.1.0 (native)
engine version: 7.1.0-beta
files: 157  bytes: 42291203
binary: /tmp/opencode/p7-license-release-install/current/bin/wxml-lsp

export WXML_LSP_BIN='/tmp/opencode/p7-license-release-install/current/bin/wxml-lsp'
```

说明：会话环境原本残留 `WXML_LSP_BIN=/home/zxg/code/rust/wxml-lsp/target/release/wxml-lsp`，
首次 `verify` 因该变量走了外部路径；上表和日志均为 `env -u WXML_LSP_BIN` 重跑后的
结果。

安装树与 archive 的许可文件逐字节一致：

| 文件 | 两侧 SHA-256 |
| --- | --- |
| `THIRD_PARTY_LICENSES.md` | `118bec0ac6b97f173a754b459fd975bf6fc90e1610f4232837af10683149df72` |
| `engine/LICENSE` | `a7d00bfd54525bc694b6e32f64c7ebcf5e6b7ae3657be5cc12767bce74654a47` |
| `engine/NOTICE.txt` | `317cd4d9d8a4de4971222bd232bc45c010424bf0dae61be830e49532e62e4d33` |
| `typings/miniprogram-api-typings/LICENSE` | `feb87a2e0c305de3464cc44077da5393c52d8ca6362d37427157d04ec6f4510d` |

安装树内 `sha256sum -c SHA256SUMS`：**158/158 行成功，exit 0**。
`verify-package --from-dir`：`verified native package ... files: 157`，
archive SHA `bd5121dd…186bb9`，exit 0。

## 6. 真实 native LSP 冒烟

对安装树 `versions/0.1.0/bin/wxml-lsp --stdio` 运行最小探针
（`/tmp/opencode/p7_final_native_smoke.py`）：initialize → didOpen
`pages/index/index.wxml` + `pages/index/index.ts` → 等待诊断 → hover
`pageTitle` → shutdown。

```text
INIT_OK True
PUBLISH index.wxml 2
PUBLISH index.ts 0
CHILD ... /tmp/opencode/p7-license-release-install/versions/0.1.0/engine/lib/tsc
  .../engine/lib/tsc --api --async ...
EXIT_CODE 0
WXML_PUBLISHED True
TS_PUBLISHED True
```

同一 fixture 的定位探针（`p7_final_hover_p2.py`）在 `pageTitle` token 内请求 hover：

```text
PUBLISH index.wxml 2
PUBLISH index.ts 0
HOVER {"contents":{"kind":"markdown","value":"```ts\nstring\n```\n\n页面标题"}}
P2_DOCUMENT_SYMBOL {"error":{"code":-32803,"message":"the script outline is incomplete"}}
WXML_DOCUMENT_SYMBOL result: view + card ...
EXIT_CODE 0
```

说明：

- 首次 hover 探针落在 `{{` 后的空白字符上返回 `null`，按 token 内字符位置重试后
  返回 `string` + JSDoc，属于探针定位问题，不是产品失败。
- 会话运行期间轮询子进程，**只看到 `engine/lib/tsc`**；没有 Node、`server.js`、
  `backends/`。`ldd` 仅 `libgcc_s`/`libc`。
- 安装树扫描：`*.js`/`*.mjs`/`*.cjs`/`*.tsx`/非 `.d.ts` 的 `.ts` = 0；
  `server.js` 路径 = 0；`node_modules` = 0；`backends` = 0；159 个文件与
  archive 一致。
- 可执行文件仅 `bin/wxml-lsp`、`bin/wxml-lsp-install`、`engine/lib/tsc`。

## 7. Zed Xvfb 冒烟

```bash
cd /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration
env -u WXML_LSP_BIN -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR \
  bash scripts/f3-zed-smoke.sh \
    --package /tmp/opencode/p7-license-release-install/versions/0.1.0 \
    --grammar /tmp/opencode/f3-zed-smoke/data/extensions/installed/wxml-lsp/grammars/wxml.wasm \
    --direct --output /tmp/opencode/p7-final-zed-smoke
echo $?   # 0
```

**实际 CHECK 数：16 个，16 PASS / 0 FAIL**（逐行来自日志，不沿用 18/18）：

```text
F3-14-ZED-CHECK: PASS - Zed started the packaged default server (--stdio, no --backend)
F3-14-ZED-CHECK: PASS - ps shows wxml-lsp --stdio exactly (no --backend argument)
F3-14-ZED-CHECK: PASS - no Node process or server.js in Zed's process tree
F3-14-ZED-CHECK: PASS - WXML hover shows the inferred string type
F3-14-ZED-CHECK: PASS - WXML diagnostics show the parser error
F3-14-ZED-CHECK: PASS - Zed shows WXML for index.wxml
F3-14-ZED-CHECK: PASS - Zed shows JavaScript for util.js
F3-14-ZED-CHECK: PASS - Zed shows TypeScript for index.ts
F3-14-ZED-CHECK: PASS - Zed shows TSX for component.tsx
F3-14-ZED-CHECK: PASS - Zed shows JSON for config.json
F3-14-ZED-CHECK: PASS - WXML hover reflects the unsaved script edit (number)
F3-14-ZED-CHECK: PASS - unsaved buffer stayed in memory; index.ts hash is unchanged
F3-14-ZED-CHECK: PASS - exactly one wxml-lsp process remains after all five languages
F3-14-ZED-CHECK: PASS - Zed log records the language server start
F3-14-ZED-CHECK: PASS - Zed passed only --stdio; the package selected native
F3-14-ZED-CHECK: PASS - no vtsls/eslint/Node language server in the Zed log
```

进程树（`logs/zed-tree-final.txt`）：

```text
wxml-lsp  .../versions/0.1.0/bin/wxml-lsp --stdio
tsc       .../versions/0.1.0/engine/lib/tsc --api --async ...
```

无 Node/server.js/vtsls。Zed `1.29.2`；package identity：
`wxml_lsp_sha256=c31d9822…348a1`、`engine_sha256=a183b195…aaa70`、
`extension_wasm_sha256=ba83770b…d7dd`、`grammar_sha256=05bd00d1…6aab9ad`。
Zed 日志只记录 `args: ["--stdio"]`。

边界：该 release 包本身不含 grammar（build log 明示
`editors/zed/grammars/wxml.wasm is absent`），smoke 使用既有已构建的 pinned
grammar 注入隔离 data dir；这不影响 Zed→`wxml-lsp` 默认 native 路径的结论。

## 8. P1：TS 诊断 inflight 时 didOpen JSON

**修复后复验（更强场景）**：打开带类型错误的 TS overlay
（末尾追加 `const brokenValue: number = 'x';`），立即 `didOpen` 无关的
`pages/index/index.json`，等待诊断。

```text
PUBLISH index.ts 1
P1_TS_PUBLISHED True
P1_TS_CODES ["2322"]
P1_TS_MESSAGES ["Type 'string' is not assignable to type 'number'."]
EXIT_CODE 0
STDERR_TAIL ... [wxml-lsp] rescheduling script diagnostics after an unrelated workspace update
```

另一次 clean 脚本场景（真实 `index.ts`，无错误）同样在 JSON `didOpen` 后收到
`index.ts` 的 0 条诊断发布空集。

**判定：P1 PASS。** 脚本诊断不再因无关 JSON didOpen 静默丢弃；stderr 出现预期的
rescheduling 日志，最终发布的是新 revision 的结果。

## 9. P2：仅 `Page(...)` 的 documentSymbol

在 `index.wxml` 与 `index.ts` 都打开后请求脚本 outline：

```text
P2_DOCUMENT_SYMBOL {"error":{"code":-32803,"message":"the script outline is incomplete"}}
STDERR: [wxml-lsp] script document symbols are partial with no symbols: 1 skipped, truncated=false
```

同一 fixture 的 WXML outline 正常返回 `view` + `card`。这是已记录的语法轮廓
walker 限制，不是本次回归；**P2 仍为 known**。

## 10. 对外发布许可判定

| 检查 | 结果 |
| --- | --- |
| 原 P1：“清单只有 engine/typings、无 Rust crate 名称/文本” | **解除**：155 个 crate 全部有名称/版本/expression；144 有全文；0 静默省略 |
| root workspace 静态链接许可全文 | **PASS**：124/124 |
| `editors/zed` wasm 许可全文 | **PARTIAL**：77/88；11 个只有 expression |
| 首方 LICENSE/NOTICE 与 Cargo license 字段 | **缺失**：8 个首方包、包根、Cargo metadata 均无 |
| 安装器 `verify` 是否校验许可覆盖 | **否**（设计边界）：只校验存在性/哈希/模式/provenance 自洽 |
| **对外发布/发布候选许可门禁** | **仍 BLOCKED** |

关闭对外发布许可门禁至少需要：

1. 为 8 个首方包增加明确许可（首方 `LICENSE` 或专有声明，并写入 Cargo metadata）；
2. 对 11 个只有 expression 的 Zed wasm crate，补齐上游许可全文（vendor/下载后
   重生成）或由 owner 书面接受 expression-only；
3. 重新打包，更新 `manifest.json`/`SHA256SUMS`/`index.json` 并复验。

**不阻塞**：安装、`verify`/`verify-package`、native LSP、Zed 运行、内部验证。
若只做内部验证，仍需显式接受 B7/NF-09 与上述许可缺口，不能声称许可门禁已通过。

## 11. 复现命令（只读复核）

```bash
REL=/tmp/opencode/p7-license-release
PKG=$REL/wxml-lsp-0.1.0-linux-x86_64
ROOT=/tmp/opencode/p7-license-release-install

sha256sum "$REL/wxml-lsp-0.1.0-linux-x86_64.tar.gz" \
          "$PKG/THIRD_PARTY_LICENSES.md" "$PKG/bin/wxml-lsp"
(cd "$REL" && sha256sum -c SHA256SUMS)
(cd "$ROOT/versions/0.1.0" && sha256sum -c SHA256SUMS)

grep -c '^| `' "$PKG/THIRD_PARTY_LICENSES.md"          # 155
grep -c '^### `' "$PKG/THIRD_PARTY_LICENSES.md"        # 114 份文本
sed -n '/^## Rust dependency inventory/,/^## First-party/p' "$PKG/THIRD_PARTY_LICENSES.md"
sed -n '/^## License texts unavailable/,/^## Reproducibility/p' "$PKG/THIRD_PARTY_LICENSES.md"

env -u WXML_LSP_BIN "$ROOT/current/bin/wxml-lsp-install" verify --root "$ROOT"
env -u WXML_LSP_BIN "$ROOT/current/bin/wxml-lsp-install" env --root "$ROOT"

find "$PKG" -type f \( -name '*.js' -o -name 'server.js' -o -name 'node_modules' \) | wc -l
```

日志与探针（全部在 `/tmp`，未进入仓库）：

- `/tmp/opencode/p7-license-release-build.log`
- `/tmp/opencode/p7-license-release-logs/{install,verify,env,verify-package,channel-sha}.log`
- `/tmp/opencode/p7-license-release-logs/{native-smoke,hover-p2,p1-broken,p1-p2}.log`
- `/tmp/opencode/p7-license-release-logs/zed-smoke.log`
- `/tmp/opencode/p7-final-zed-smoke/`
- `/tmp/opencode/p7_final_*.py`

## 12. 最终判定

- **构建/归档/安装/校验：PASS。** archive SHA `bd5121dd…186bb9`、15,141,762 B；
  安装树 157 files / 42,291,203 B；许可文件与 archive 逐字节一致。
- **Rust 许可清单：PASS；全文覆盖 144/155。** 原 P1（无 Rust 清单）解除；
  11 个 Zed wasm crate 仅 expression 且已显式列出。
- **Native + Zed 冒烟：PASS。** 真实 LSP 仅 exec `engine/lib/tsc`；Zed 实际
  **16/16** CHECK 全过，无 Node/server.js。
- **P1 功能回归：PASS。** JSON didOpen 后脚本 `2322` 仍发布；stderr rescheduling。
- **P2：known。** Page-only outline `-32803` 保持，不伪造结果。
- **对外发布许可：仍 BLOCKED。** 首方许可缺失 + 11 个 expression-only 文本缺口；
  不阻塞安装、校验、native LSP 与 Zed 运行。
