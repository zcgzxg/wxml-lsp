# WXML LSP：需求与实施计划

更新日期：2026-11-17（W4）；Rust + tsgo 迁移、P7 与 F-02 状态补充：2026-11-20。

架构决策于 2026-11-15 整理至 [ADR 索引](docs/adr/README.md)，包含模块边界、类型检查与源码映射、文档同步及并行交付方案；2026-11-16 的 [ADR-0006](docs/adr/0006-explicit-mini-program-roots.md) 补充显式小程序根配置；2026-11-20 的 [ADR-0008](docs/adr/0008-rust-tsgo-no-node-backend.md) 在 owner scope waiver 下最终关闭为 `accepted (native default; Node removed)`。架构边界以有效 ADR 为准；删除已完成；实际进度与证据仍见本文。

本文记录已确认的需求、方案评审结果、实验依据和后续计划。项目用于个人在 Zed 中开发微信小程序。以下“截至 W4（2026-11-17，`9b804d4`）”段落是 **Node 后端历史快照**：当时 Rust workspace、WXML 语法/项目模型、C0/C1 契约、Node 类型后端、模板类型语义、组件/样式/资源智能、结构编辑与保真格式化、显式多 app、依赖失效、未保存 overlay、版本屏障与重启恢复均已接入；S 语义工具（references/rename/codeAction/semanticTokens/workspaceSymbol/inlayHint/signatureHelp/codeLens）已合入；Q2 调度/恢复、最终 release 和 Linux Zed 运行时冒烟已完成。Q2 的 Rust 与安全 R2 均 `APPROVE`；S 第 2 轮 `CHANGES_REQUESTED` 的残余已由 `dd15bc1` 修复并由集成侧复验，按用户上限没有第 3 轮正式审核，不写成 `APPROVED`。Q1 本地交付（本地打包、验证安装、升级/回退、可选显式 HTTPS 下载）已合入 `f46dec9`。当前没有 marketplace、远端 release 或发布者签名；Windows/macOS 实机、真实主桌面 Wayland 交互和完整多 workspace 仍未验收。**当前 native 状态见 §0.1 与 §10，不能把本段 Node 结果当作当前后端验收。**

文档入口：[文档索引](docs/README.md) · [WXML 语言功能基线](docs/reference/wxml-language-feature-baseline.md) · [W4 最终验收证据](docs/evidence/full-feature-final.md) · [W3 语义工具证据](docs/evidence/full-feature-w3.md) · [W2 集成证据](docs/evidence/full-feature-w2.md) · [证据索引](docs/evidence/README.md) · [来源与许可证记录](docs/reference/provenance.md)。

## 0. 当前实现与快速开始

### 0.1 实现范围

| 组件 | 位置 | 当前内容 |
| --- | --- | --- |
| Rust workspace | `crates/` | `wxml-text`（UTF-8 字节 ↔ UTF-16 位置、行索引）、`wxml-syntax`（容错 lexer/parser、Mustache Pratt、WXS rawtext、资源上限）、`wxml-project`（URI/路径、overlay、显式多 app、tsconfig、依赖/样式/资源索引）、`wxml-types`（C0 契约、模板语义、`wx:for`/guards、Behavior 声明事实、template/import/include 与 WXS 静态摘要、脚本符号事实、后端接口与 mock）、`wxml-backend-tsgo`（原生 tsgo 适配：native/script/shadow/query）、`wxml-lsp`（JSON-RPC、多项目宿主、native 默认后端、hover/definition/diagnostics/completion、格式/结构/链接、语义工具与安全编辑、Q2 调度/取消/恢复、版本屏障/缓存/超时/重启）、`wxml-delivery`（本地打包/验证安装器）、`wxml-fixtures`（schema v2 校验） |
| native 类型后端（默认） | `crates/wxml-backend-tsgo/`、固定资产 | Rust 语义适配器 + 独立 tsgo 子进程；固定 `7.1.0-beta` 引擎与 `miniprogram-api-typings@5.3.1` 类型资产；原位 shadow、快照/租约、C0 DTO、查询呈现与保守编辑；默认 `--stdio`，仅引擎未安装时 HostOnly |
| 旧 Node 类型后端（历史，已删除） | `backends/typescript/`、`crates/wxml-lsp/src/backend_node/`、`crates/wxml-delivery/` 的 v1 写路径（均已删除） | 仅保留历史证据；当前无 Node/server.js 运行路径，显式 `--backend node` 被拒绝；业务 TS/JS/.d.ts 输入仍保留 |
| Zed 扩展 | `editors/zed/` | 本地 Dev Extension，id `wxml-lsp`、名称 `WXML LSP`（避免与 Marketplace 上仅 grammar 的 `wxml` 扩展冲突）；WXML 语言配置、tree-sitter 查询与 outline、`wasm32-wasip2`；真实 grammar 已用 wasi-sdk 34 构建（约 23,806 B）；从 `WXML_LSP_BIN` 绝对路径启动 `wxml-lsp --stdio` |
| 样本与证据 | `tests/fixtures/`、`docs/evidence/` | AC-001..AC-013 样本与 schema 校验；类型桥、P8、G1、[W2 集成证据](docs/evidence/full-feature-w2.md)、[W3 语义工具证据](docs/evidence/full-feature-w3.md)、[W4 最终验收证据](docs/evidence/full-feature-final.md) 与 P7 删除/最终验收证据；Node 时代的 [P7 真实集成](docs/evidence/p7-integration.md) 仅作历史 |

W4 历史快照（2026-11-17，`9b804d4`；Node 后端，非当前验收）：W4 固定日志为 Rust workspace `1283 passed / 0 failed / 23 ignored`（49 suites，含 doctests），Node 串行 `333 / 333`；23 个 ignored 门禁（performance 1、q2_stability 1、real_node 5、stdio 9、wave5 7）全部通过；覆盖率 lines 87.24% / functions 88.85% / regions 86.71%；`cargo audit` 0 vulnerabilities、`cargo deny` advisories ok。最终 release 两次构建 SHA-256 均为 `c09062b16dd6bfea4ee03f99f13665cf4858130aae83eefec9c04cd94d3815ab`（8,696,984 B、protocol 2、214 files），install/verify/verify-package 通过；Linux Zed 运行时冒烟使用最终发布包，hover/definition/成员补全/诊断均通过。W3 的 `1251 / 21`、Node `326 / 326` 与 W2 的 `1095 / 20`、Node `304 / 305`、Zed `25 passed` 保持各自历史记录，不与 W4 混用。Q1 `f46dec9` 的 `cargo test -p wxml-delivery` 结果也保持原记录。W4 的 Linux 冒烟是手工安装发布包，不是标准 UI 重跑；标准 Dev Extension 安装仍引用 [P8 第 13 节](docs/evidence/p8-zed.md) 的历史证据。以上 Node 命令和计数只用于历史追溯，**不是当前后端验收**。

Rust + tsgo 迁移状态（2026-11-20，P7 完成）：F3 交付合同 **16 closed / 0 partial / 0 blocked**；测试迁移台账终态 **109 `green` / 0 `planned` / 130 `reviewed` / 29 `waived` / 71 `retired-with-decision`**（339 总数）；native gates **40/40**（23 engine + 3 edit + 9 stdio + 1 perf + 1 stability + 3 real_matrix，opt-in required job）；真实 matrix **22 exact / 11 blocked**。默认 `--stdio` 已是 native（引擎未安装时退化为 `HostOnly`，损坏安装 fail closed），显式 `--backend node` 被拒绝。2026-11-20 owner 选择 **C-lite functional waiver**（原话：“不需要严格按照原来的功能写测试。功能合理即可”）：**不声称旧 TypeScript/Node 测试逐条 parity**，`reviewed`/`waived`/`retired` 均非 green；功能验收以 native engine、native gates、F3 和合理行为测试为准。P7 删除已完成（删除提交 **`c4ddf32`**；`p7-removed` 指向 `b6c1ced`，只是旧 tag，不含本轮验收修复）：`backend_node`、旧 TS 实现/测试树、12 个 delete gate、v1 写路径和 legacy Node env 已删除；11 个 retarget gate 保留 native-only；业务 TS/JS/.d.ts 输入、WXS/WXML 语义和 fixtures 保留。删除后 release/install/Zed smoke 通过（Zed **16/16**、截图 9 张），无 Node/server.js；F-02 的 debug perf 假失败修复已落地（release 200 ms 预算未放宽），**2592 passed / 0 failed / 73 ignored 待 `p7-final-verification` 追加**（不是 tag 事实；[F-02 记录](docs/evidence/2026-11-20-tsgo-f02-debug-perf-fix.md)）。functional smoke 为 **P1 fixed / P2 known / PASS-with-followup**（P1 由 `180fa42` 修复）；stability 为 **4 implemented / 1 blocked**（ledger 未回填）。最终验收清单见 [P7 最终文档与验收清单](docs/evidence/2026-11-20-tsgo-p7-final-docs.md)（**PASS-with-followup**）；**第三方许可文本 P1 已解除**（155/155 有文本：144 本地 + 11 vendored SPDX），首方 LICENSE 仍 **BLOCKED owner decision/需 owner 决策**，内部安装可用。详见 [ADR-0008](docs/adr/0008-rust-tsgo-no-node-backend.md)、[P7 收口台账](docs/evidence/2026-11-20-tsgo-p7-closeout-ledger.md)、[P7 范围裁决](docs/evidence/2026-11-20-tsgo-p7-scope-decision.md) 与[迁移计划 §12.16](docs/plans/2026-11-17-rust-tsgo-migration.md)。

### 0.2 验证命令

```bash
# Rust workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=4
cargo test --workspace --doc
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# 真实 native gates（opt-in required job；不是默认 CI，也不是 mandatory_green）
# --package 指向已安装/已解包的 schema v2 native package（含 engine/lib/tsc 与 typings）
scripts/native-gates.sh --package "$HOME/.local/share/wxml-lsp/versions/<version>"

# 本地交付（Q1）
cargo test -p wxml-delivery
scripts/build-release.sh --dry-run --output "$HOME/wxml-release"

# Zed 扩展（独立 workspace；需要 wasm32-wasip2 target）
(cd editors/zed && cargo fmt --all -- --check)
(cd editors/zed && cargo check --all-targets --locked)
(cd editors/zed && cargo clippy --all-targets --locked -- -D warnings)
(cd editors/zed && cargo test --locked)
(cd editors/zed && cargo build --target wasm32-wasip2 --locked)
(cd editors/zed && cargo build --target wasm32-wasip2 --release --locked)
```

历史 Node 命令（**P7 已删除；仅用于追溯 `p7-pre-delete` 及更早证据；当前不可运行，不再是验收命令**）：

```bash
# 以下命令依赖 backends/typescript、Node >= 24 和 dist/server.js，P7 后删除
(cd backends/typescript && npm ci && npm run check && npm run build && npm test)
(cd backends/typescript && node --test --test-concurrency=1 test/*.test.ts)
(cd backends/typescript && npm run matrix:json && npm run perf:json)
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test stdio_type_loop -- --ignored
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test real_node -- --ignored
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR WXML_TEST_NODE=1 \
  cargo test -p wxml-lsp --test wave5_acceptance -- --ignored
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test q2_stability -- --ignored
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test performance_bench -- --ignored --nocapture
```

当前性能/稳定性以 native gate runner 的 opt-in job 为准；W4 的 Node 性能数字和 23 个 ignored 门禁只作为历史证据，见[W4 证据](docs/evidence/full-feature-final.md)。Node/旧 TS/v1 删除后的 workspace、native gates **40/40**、release/install、Zed smoke、无 Node 进程与文档口径已按 [P7 最终验收清单](docs/evidence/2026-11-20-tsgo-p7-final-docs.md) 执行并记录；残余风险见清单。

### 0.3 在 Zed 中运行

默认 `--stdio` 自 `5020ae7` 起走 native（只有引擎未安装时为 `HostOnly`，损坏安装 fail closed）。当前推荐路径是安装 schema v2 native 包，用 `WXML_LSP_BIN` 指向安装根的 `current/bin/wxml-lsp`；**不再需要 Node/npm，也不使用 `WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR`**。P7 删除已完成：显式 `--backend node` 被拒绝，5 个 legacy Node 变量不再读取（设置无效果）；它们只保留在历史证据中，不是推荐或验收路径。

1. 构建 native 服务：`cargo build -p wxml-lsp --release`；再按本节「本地打包、安装、升级与回退」构建或安装 schema v2 包，或复用已有安装。
2. 在仓库根目录、启动 Zed 前导出绝对路径（必须是**绝对路径**，扩展不探测 worktree、不查 `PATH`）：

   ```bash
   export WXML_LSP_BIN="$HOME/.local/share/wxml-lsp/current/bin/wxml-lsp"
   ```

3. 标准安装动作：在 Zed 命令面板执行 `zed: install dev extension`，选择 `editors/zed`；扩展 id 为 `wxml-lsp`，名称 `WXML LSP`。
4. 详细步骤、WASI/grammar 构建和已知限制见 [Zed 扩展 README](editors/zed/README.md)。

`WXML_LSP_BIN` 缺失或无效时，扩展返回错误且不会启动 LSP，也没有 worktree 或 `PATH` 回退。

> 历史记录（2026-11-16）：本机主 Zed 曾用 dev symlink/index（`extensions/installed/wxml-lsp -> editors/zed`）加载扩展，并通过 `WXML_TYPE_BACKEND` 启动 Node 后端；该路径已在 P7 删除，仅保留为[P8 证据](docs/evidence/p8-zed.md)与 [W4 证据](docs/evidence/full-feature-final.md)的历史事实，不是当前安装说明。

环境变量约束（native 路径）：

| 变量 | 约束 |
| --- | --- |
| `WXML_LSP_BIN` | Zed 扩展唯一支持的服务器来源；必须为宿主 OS 绝对路径，且存在、可执行；无回退 |
| `WXML_NODE`、`WXML_TYPE_BACKEND`、`WXML_BACKEND_DIR`、`WXML_ANALYZE_TIMEOUT_MS`、`WXML_TEMPLATE_CHECK` | 旧 Node 后端变量（P7 已删除）：native 不读取，设置无效果；仅保留在历史证据中，不再是配置项 |

native 默认路径只按已验证安装布局发现引擎和类型资产；缺引擎时退化为 `HostOnly`（模板语法/格式/结构/事件 facts 可用，类型查询 `Unavailable`），manifest/引擎/类型资产损坏或不匹配则在 LSP handshake 前 fail closed，不回退 Node。原生包的构建与安装见本节「本地打包、安装、升级与回退」。

#### 小程序根与类型项目根

服务通过 `initialize.initializationOptions` 接收 `{ "apps": [{ "root": "." }] }`。省略该选项、传 `null`、`{}` 或 `{ "apps": [] }`，都等价于单 app `{ "root": "." }`。每个 app 的 `root` 必填，指向 `app.json` 所在目录；可选 `projectRoot` 指向类型后端项目及 `tsconfig.json` 所在目录，默认等于 `root`，与 `WXML_BACKEND_DIR` 无关。

Zed 中写入工作区的 `.zed/settings.json`，位置是 `lsp.wxml-lsp.initialization_options`。例如以仓库根为 LSP workspace root 时，`apps/miniprogram` 和 `apps/sub-merchant` 两个小程序可使用以下配置：

```json
{
  "lsp": {
    "wxml-lsp": {
      "initialization_options": {
        "apps": [
          {
            "root": "apps/miniprogram/src",
            "projectRoot": "apps/miniprogram"
          },
          {
            "root": "apps/sub-merchant/src",
            "projectRoot": "apps/sub-merchant"
          }
        ]
      }
    }
  }
}
```

两个路径都相对 **LSP workspace root**（`rootUri`，或首个 `workspaceFolders` 根），不是 `.zed/` 目录；上例分别使用 `apps/miniprogram/src/app.json` 与 `apps/miniprogram/tsconfig.json`，以及另一 app 的对应文件。路径必须是工作区内已存在的相对目录，不允许绝对路径、`..` 或 symlink 越界；canonical 后须满足 `workspace ⊇ projectRoot ⊇ root`。重复 root 报错；最多 64 个 app，每个路径最多 4096 UTF-8 bytes。

不再扫描子目录发现 `app.json`，不再向上推断最近的 `tsconfig.json`，生成目录也不再按名称自动忽略或自动发现为 app。嵌套小程序、分层 tsconfig 和需要使用的生成目录都应明确配置。非法配置使 `initialize` 返回 `-32602`，不静默回退；目录有效但 `app.json` 缺失或内容错误，仍走现有 project issues。

配置仅在启动时消费，修改后需重启 LSP，没有根热更新；这与 [Zed 官方初始化选项说明](https://zed.dev/docs/configuring-languages#possible-configuration-options) 一致。[JSON Schema](editors/zed/initialization-options.schema.json) 仅覆盖 `initialization_options` 的内层值，不能替换整个 Zed settings schema。路由、共享文件归属和迁移细节见 [Zed 配置指南](editors/zed/README.md#mini-program-roots-initialization-options) 与 [ADR-0006](docs/adr/0006-explicit-mini-program-roots.md)；这些配置示例不构成真实 Zed 多 app 验收证据。

#### 样式与 snippets 配置

样式和片段选项与 `apps` 放在同一个 `initialization_options` 对象中。路径均相对 LSP workspace root；非法或越界路径会被记录并忽略，不会静默改成别的文件。

```json
{
  "lsp": {
    "wxml-lsp": {
      "initialization_options": {
        "wxml": {
          "globalStyles": ["styles/theme.scss"],
          "styleDiagnostics": { "resources": true, "classes": true }
        },
        "snippetsPath": ".wxml-lsp/snippets.json"
      }
    }
  }
}
```

- `wxml.globalStyles`：额外的全局样式入口，显式配置后参与 class 索引；`app.wxss`/`app.scss` 仍按页面/组件的 `styleIsolation` 自动参与。不会猜测固定文件名。
- `wxml.styleDiagnostics.resources`：资源路径存在性诊断，默认开启；`classes`：静态 class 缺失诊断，默认关闭。仅对可证明的静态 token 生效，动态插值、mixin/`@extend` 生成和预算截断不伪造结果。
- `snippetsPath`：可选的工作区相对路径或绝对路径；省略时读取 `<workspace>/.wxml-lsp/snippets.json`。文件支持 `enabled`、`builtin` 覆盖开关和 `snippets[]`（`id`/`label`/`prefix`/`scope`/`description`/`body`），用户条目按 `id` 或 `label` 覆盖同名内置片段；修改后通过 watched-file 更新，不需要重启。服务端只读取该显式文件，不读取 Zed 私有用户目录。

#### 语义工具开关

inlay hint 和 code lens 默认开启，可在同一个 `initialization_options` 中用 `semanticTools` 关闭；关闭后服务端不再广告对应能力。

```json
{
  "lsp": {
    "wxml-lsp": {
      "initialization_options": {
        "semanticTools": { "inlayHints": false, "codeLens": false }
      }
    }
  }
}
```

references、rename、codeAction、semanticTokens、workspaceSymbol 和 signatureHelp 没有独立开关；它们在 `initialize` 中声明，并遵守各自的覆盖/预算边界。客户端未声明 `WorkspaceEdit.documentChanges` 时，rename 和 codeAction 不会广告。

#### 本地打包、安装、升级与回退（Q1）

Q1 已合入本地交付工具，但**没有 marketplace 发布、远端 release 或自动更新**；下载只在用户显式执行安装/升级命令并给出 `--enable-download` 时可用。Zed 扩展本身仍不下载、不解包、不安装服务器；它只接受宿主 OS 的绝对 `WXML_LSP_BIN`。独立安装器负责生成和验证产物，再把稳定的 `current/bin/wxml-lsp` 路径交给扩展。

构建（输出目录必须在仓库外；默认 cargo 离线，显式 `--online` 只允许 cargo 取依赖；asset 只来自已校验缓存或 `--from-dir`）：

```bash
# 首次或缓存未命中：--from-dir 提供 assets.lock.json 中精确文件名的两个 tarball
scripts/build-release.sh --output "$HOME/wxml-release" --from-dir /path/to/locked-tarballs
# 只检查前置条件，不写文件、不联网
scripts/build-release.sh --dry-run --output "$HOME/wxml-release"
```

`build-release.sh` 只生成 schema v2 原生包：不执行 node/npm/npx/pnpm/yarn/bun，也不从 `backends/typescript` 复制 JavaScript。脚本拒绝 `--publish`、`--tag`、`--release`、`--upload`，只生成本地版本化目录、`.tar.gz`、checksum、`index.json`、`manifest.json`、`provenance.json` 和 `THIRD_PARTY_LICENSES.md`。产物绑定当前 HEAD 的 Rust/native engine 协议版本、`linux-x86_64` 平台和逐文件哈希；旧 release 不能用于新 HEAD 验收。完整命令与布局见 [wxml-delivery README](crates/wxml-delivery/README.md)。

安装、升级和回退：

```bash
unset WXML_LSP_BIN   # 设置后安装器会优先使用该路径，不读写安装根
release="$HOME/wxml-release"
installer="$release/wxml-lsp-0.1.0-linux-x86_64/bin/wxml-lsp-install"
"$installer" install \
  --root "$HOME/.local/share/wxml-lsp" \
  --from-dir "$release" \
  --version 0.1.0 \
  --offline

export WXML_LSP_BIN="$HOME/.local/share/wxml-lsp/current/bin/wxml-lsp"

"$installer" upgrade  --root "$HOME/.local/share/wxml-lsp" --source https://example.invalid/releases/ --enable-download
"$installer" rollback --root "$HOME/.local/share/wxml-lsp" --to 0.1.0
"$installer" list    --root "$HOME/.local/share/wxml-lsp"
"$installer" current --root "$HOME/.local/share/wxml-lsp"
"$installer" verify  --root "$HOME/.local/share/wxml-lsp"
"$installer" env     --root "$HOME/.local/share/wxml-lsp"
```

- `install`/`upgrade`/`rollback` 先在 `tmp/` 解包并完成归档、manifest、逐文件哈希和权限校验，再原子替换 `current` symlink；失败保留原活动版本。schema v2 另做引擎/typeAssets 探针；schema v1 只做只读后端文件存在性校验，不执行 `node` 或 `node --version`。`upgrade` 默认拒绝降级，降级必须显式 `rollback --to`。
- 下载默认关闭；远端源必须同时给出 `--enable-download`。仅接受 HTTPS；只有测试 fixture 允许 `localhost`/`127.0.0.1`/`[::1]` 的 HTTP。拒绝凭据、query、fragment、`..`、跨源 redirect 和路径/链接逃逸。项目文件、`.zed/settings.json` 不能指定下载源。
- `WXML_LSP_BIN` 已设置且有效时，安装/升级/回退跳过，不下载、不覆盖；已设置但无效（空、相对、含 `..`、不存在、非普通文件、不可执行）立即报错，绝不回退到已安装版本或网络。
- `list`、`current`、`verify`、`env` 是只读命令，不创建安装根或锁文件。`env` 只输出 `WXML_LSP_BIN`；schema v1 的 `current`/`verify`/`env` 不执行旧二进制、不探测 Node。缓存只保存完整验证通过的归档；离线安装/回退只使用已验证缓存或显式 `--from-dir`。
- 当前信任链是 **TLS + index/归档 SHA-256 + manifest 文件哈希**，没有发布者签名、公钥 pin 或透明日志。它能证明内容与同一份元数据一致，不能证明发布者身份；被攻破的 HTTPS 源、误签 CA 或恶意本地 bundle 仍可自洽。该残留风险已明确接受，正式发布前应增加 detached 签名或 minisign，详见 [ADR-0007](docs/adr/0007-local-delivery-trust-model.md)。
- 安装器目前只在 Linux 验证，依赖 Unix 原子 symlink；安装器本身不执行 Node，schema v1 只读命令也不执行或探测 Node。Windows/macOS 未验证。S 语义工具已在 W3 合入，Q2 调度/恢复已在 W4 通过；完整多 workspace、标准 UI 最终重跑和跨平台验收仍不在本记录范围。

#### 最终 release 与 Linux Zed 运行时冒烟（W4，历史）

以下 W4 包是 **Node 后端历史产物**，不是 P7 后的 native release 验收；P7 删除后发布包只含 Rust 二进制、原生引擎/类型资产和 Zed 扩展，不含 `server.js`。保留此节用于追溯最终 W4 快照与 P8 安装证据。

W4 在 `7433194`/`9b804d4` 两个 Node 发布运行时修复后重建最终包两次，归档 SHA-256 完全一致：

```text
sha256  c09062b16dd6bfea4ee03f99f13665cf4858130aae83eefec9c04cd94d3815ab
size    8,696,984 bytes
protocol 2   files 214   platform linux-x86_64   node >= 24.0.0
```

包内包含 `bin/wxml-lsp`、`bin/wxml-type-backend/server.js` 及依赖、`zed-extension/extension.wasm`
和 `zed-extension/grammars/wxml.wasm`。`install`、`verify`、`verify-package` 通过；安装根为
`/tmp/opencode/final-wxml-root3`。构建时临时复制到 gitignored 的 grammar 后已清理。

Linux Zed 运行时冒烟在隔离 Xvfb `:99`、隔离 `HOME`/`XDG`/user-data 中手工安装发布包
`zed-extension`，打开 `tests/fixtures/page-basic`：release LSP 与 Node 启动，hover 显示
`(property) title: string` 和“页面标题”，definition 跳到 `index.ts` 的 `data.title`，成员补全
显示 `title`/`user`，诊断显示 `nmae` 错误且状态栏计数 1。**这不是标准 UI
`zed: install dev extension` 重跑**；标准安装历史证据仍见 [P8 第 13 节](docs/evidence/p8-zed.md)，
截图和原始路径见 [W4 证据](docs/evidence/full-feature-final.md) §6。没有 marketplace 上传、
远端 release/tag 或发布者签名。

### 0.4 已知限制

- W2 已完成显式单 workspace 多 app、依赖失效、隔离 tsconfig、overlay/版本屏障/重启恢复；组件智能（page/component 自身/app `usingComponents`、局部覆盖、props/事件/静态 hover、组件与属性 definition、picker/input mode 条件、插件/包降级）；WXSS/SCSS class 与本地资源补全/导航/链接/诊断；保真格式化、range formatting、linked editing、folding、selection range、documentSymbol/outline、内置/用户 snippets；Behavior/共享 data/props、template/import/include、WXS 静态子集、guards/loops、JS/JSDoc 与未保存跨文件 overlay。多 app 按文档 URI 的最长 app 目录匹配，以稳定 `projectId` 隔离；app 外、`projectRoot` 内的共享文件保留 active owner 或唯一 owner，多个 owner 且无 active 时不猜测。
- 当前仍是**单 LSP workspace 内的显式多 app**：不支持完整多 workspace、根配置热更新或跨 app 自动依赖图；修改 `apps`/`projectRoot` 后需重启 LSP。插件/远程组件只有在本地存在可信元数据时才建模，否则明确降级；动态 template/is、动态 WXS 导出、动态 Behavior 和无法证明的 SCSS 生成类不猜测。模板体仍不按每次调用实例做完整类型检查，WXS 是静态子集且从不执行。格式化只规范开始标签内部空白，不重排文本节点或整份文档缩进。
- S 语义工具已合入：references、prepareRename/rename、codeAction、semanticTokens、workspaceSymbol、inlayHint、signatureHelp、codeLens；rename 使用安全 `EditPlan`，只接受 `documentChanges`，打开文件带客户端 version、关闭文件重读并以 `version: null` 返回，失败零编辑，服务端不写盘。保守拒绝自定义组件 tag rename、静态 include 导致的 caller 覆盖不完整、动态访问、共享 importedData 跨脚本、快照截断/超限、跨视图冲突和文件含 `<template name>` 时的页面 data rename；未落盘 WXML fail-closed；没有跨文件事务/undo；硬链接未检测。
- Q2 调度/恢复已合入：诊断 debounce（默认 120 ms，可配 0–5000）、同文档合并、交互队列 128 / 诊断队列 8、每 8 个交互任务让诊断前进、排队请求可立即取消、timeout/crash/协议不匹配降级并恢复、根移除/重加重放 overlay。单 worker 执行同步 TypeScript 调用，运行中请求不可中断；性能/稳定性门禁使用单个小 fixture，不是大项目语料。大项目语料调优、真实编辑器 UI 和完整多根/多 workspace 仍不在当前验证范围。
- 发布包仅在 Linux x86_64 验证；没有 marketplace、远端 release、自动更新或发布者签名。W4 的 Node 包和 Linux Zed 运行时冒烟是历史事实，native 包的 F3 验收见 [F3 合同](docs/reference/tsgo-delivery-contract.md)；标准 UI 安装在最终版本上没有重跑。
- 源文件预算为 2048 个文件 / 16 MiB，连续 hover 的待处理队列上限为 128；样式索引、资源扫描和 WXS/template 分析各有独立上限，截断时会标记不完整而不是伪造完整结果。
- Zed 扩展已完成代码评审、host 测试、wasm 构建和真实 `tree-sitter-wxml` grammar 构建（wasi-sdk 34，约 23,806 B）。2026-11-16 已在隔离 user-data-dir 中通过命令面板实际执行 `zed: install dev extension`：安装目录为指向 `editors/zed` 的 symlink，`index.json` 中 `dev=true`，扩展加载并启动 release `wxml-lsp --stdio`（W4 历史为 Node 后端）。干净 fixture 的 `title`/`user.name` hover、definition、标签/属性/事件/成员补全、未知成员诊断和重启恢复均已实际执行并核对内容；真实 TS/JS 项目观感已记录。**W4 已对最终发布包做 Linux Zed 运行时冒烟，但没有重跑标准 UI 安装；Windows/macOS 实机、真实主桌面 Wayland 观感和连续键入延迟仍未验收**；证据见 [P8 第 13 节](docs/evidence/p8-zed.md) 与 [W4 证据](docs/evidence/full-feature-final.md)。
- G0/G2 已在隔离 Xvfb 中完成标准安装、扩展加载、五语言注册、LSP 启动（W4 历史为 Node 后端）、hover/definition/completion/diagnostics 和重启恢复；Windows/macOS、真实主桌面 Wayland 观感和连续键入延迟未验收，完整 G0/G2 矩阵仍不整体勾选。W4 的 ignored 门禁 `stdio_type_loop` 9、`real_node` 5、`wave5_acceptance` 7 全部通过；标准 UI 没有在最终版本重跑。上述 Node 门禁是历史记录，P7 后由 native gates 替代。
- Q1 已提供本地打包与安装器，但未发布：扩展 id 为 `wxml-lsp`、名称 `WXML LSP`（避免与 Marketplace 上仅 grammar 的 `wxml` 扩展冲突）；没有 marketplace 发布、远端 release/tag、服务器自动下载或自动安装。安装器下载默认关闭，只有显式 `--enable-download` 才访问 HTTPS 源；当前信任链没有发布者签名，详见 [ADR-0007](docs/adr/0007-local-delivery-trust-model.md)。
- Zed 扩展已注册 WXML、JavaScript、TypeScript、TSX、JSON（不含 JSX），同一 `wxml-lsp` 进程接收关联文档的 `didOpen`/`didChange`/`didSave`。2026-11-16 隔离 Xvfb smoke 已验证：未保存的 TS 字段类型修改会实时反映到 WXML hover；保存后 `didSave` 刷新诊断；无关 TS 脚本只触发一次诊断刷新、不改变 WXML 类型结果；服务端动态注册并接收 `workspace/didChangeWatchedFiles`。标准 UI 安装、definition 跳转、服务重启恢复和真实项目观感已在隔离 Xvfb 中验收；Windows/macOS 和连续键入延迟仍未验收。

## 1. 项目目标

以本项目的[WXML 语言功能基线](docs/reference/wxml-language-feature-baseline.md)为通用语言能力参考，开发独立维护、自用的 Zed WXML 扩展，并增加可靠的模板类型支持。

最重要的需求是：WXML 引用的数据能够追踪到所属页面或组件的实际类型；鼠标悬停时显示字段类型及说明；出现类型错误时，在 WXML 中准确标记对应位置。

WXML 的页面/组件数据根只能来自脚本中声明的字段：页面来自 `data`，自定义组件来自 `data` 与 `properties`；`methods`（组件）或页面选项上的函数只作为事件处理函数来源。WXML 不会把单独定义的 `interface` 或模型文件当作数据源；`interface`/`type` 只有被 `data`/`properties` 字段引用时才参与类型和文档查询。

例如组件这样声明：

```ts
Component({
  properties: {
    /** 外部传入的标题 */
    title: {
      type: String,
      value: '',
    },
  },
  data: {
    // WXML 数据从 data 中引用
    user: {
      /** 用户显示名称 */
      name: '',
    },
  },
  methods: {
    // WXML 事件所引用的方法
    onTap() {},
  },
});
```

模板中：

```xml
<view bindtap="onTap">{{ title }} {{ user.name }}</view>
```

悬停 `title`、`name` 应显示实际类型和字段说明，例如：

```ts
(property) title: string
(property) user.name: string
```

同时显示“外部传入的标题”“用户显示名称”。对于 `{{ user.nmae }}`，应标记不存在的字段 `nmae`，并给出可理解的错误说明。

`data`/`properties` 字段可以引用跨文件 `interface`/`type` 作为类型；此时类型、JSDoc 和诊断沿该声明查询，但模板数据根仍是脚本中的 `data`/`properties`，不是独立 interface。页面使用 `Page({ data })` 时没有 `properties`，组件则可将两者同时作为模板数据来源。

## 2. 已确认的范围与偏好

### 2.1 用户明确要求

- 目标编辑器为 Zed，目标场景为微信小程序 WXML 开发。
- 可以独立维护和安装自用扩展，不以公开发布为前提。
- 不把市场审核、与现有扩展功能重叠、向上游提交 PR 作为实施前提。
- 使用 Rust 承担扩展或语言服务的主要实现；完整 TypeScript 类型能力可以通过复用类型引擎获得。
- WXML 数据引用必须能够展示类型和说明，并标记类型错误。

### 2.2 明确不需要

- Vue / Pug 内嵌模板支持。
- Emmet。
- 一键创建组件及其命令、输入框、右键菜单。
- JS 变量任意自定义颜色或装饰样式。
- 自动修改 VS Code 配置。
- 围绕扩展市场发布或上游 PR 设计工作流。

因此，上述功能在 Zed 中的 API 限制不构成本项目的主要风险，也不安排对应的兼容或替代实现。

### 2.3 实施范围的默认选择

首阶段聚焦原生微信小程序的 `.wxml` 与相关 JS/TS、JSON、样式文件。原插件中为 Vue/Pug 或其他模板方言服务的兼容层不作为首阶段目标。

“全部逻辑只能使用 Rust”“单进程”“单一二进制分发”仍不是硬性要求；**去 Node 已由用户于 2026-11-17 明确选择**，并由 [ADR-0008](docs/adr/0008-rust-tsgo-no-node-backend.md) 于 2026-11-20 在 owner scope waiver 下 accepted。当前优先级仍是类型准确性、说明完整性和真实编辑体验；去 Node 不等于删除 TypeScript 语言支持。

## 3. 功能需求

### 3.1 第一优先级：模板类型体验

页面/组件数据根由脚本声明决定：页面为 `data`，自定义组件为 `data` 与 `properties`。`interface`/`type` 只有作为这些字段的类型来源时才参与；`methods`/页面函数只用于事件处理，不是模板数据。

| 编号 | 需求 | 验收要点 |
| --- | --- | --- |
| T1 | 关联所属页面或组件 | 普通 `Page({...})`、`Component({...})` 写法不要求改业务源码，不强制增加导出或辅助方法 |
| T2 | 字段类型与说明 | `user.name`、`title` 能追踪到 `data`/`properties` 的实际声明；展示类型、字段 JSDoc；支持字段引用跨文件类型 |
| T3 | 类型错误标注 | 不存在字段、非法类型操作等错误映射到 WXML 中的准确范围 |
| T4 | 编辑期间一致性 | WXML、关联脚本及导入类型尚未保存时，查询结果能随编辑更新 |
| T5 | 模板作用域 | 正确处理 `wx:for` 的默认和自定义变量、嵌套遮蔽及作用域退出 |
| T6 | 条件与联合类型 | 结合模板语义处理 `wx:if/elif/else`、可空类型与类型窄化，减少误报 |
| T7 | 项目类型上下文 | 尊重适用的 `tsconfig`、模块解析和路径别名；追踪类型别名、继承、泛型等完整 TS 类型关系 |
| T8 | JS 类型来源 | 支持具备 JSDoc 类型的 JS 页面和组件，不把所有 JS 数据退化为 `any` |
| T9 | 字符位置正确 | 中文、emoji、换行等场景中的 hover 与错误位置正确，统一处理 UTF-16 与字节偏移 |

类型后端能够识别某项语义，不代表模板转换层已经支持它；T5、T6、T7 等都需要独立验收。

### 3.2 后续类型增强

- 自定义组件的 `properties`、页面或组件 `data` 共同构成模板上下文；`data`/`properties` 字段引用的 `interface`/`type` 只提供类型和文档，不作为独立数据根。
- 根据属性的实际声明类型，检查绑定表达式与目标组件属性的兼容性。
- 保留字段的可选性、联合类型、文档及声明位置，支持成员补全和定义跳转。
- 对缺少类型信息的 `any`、裸 `Object` 等保持诚实：不能凭空生成字段说明或声称严格校验通过。
- 模板运行时转换、可空访问等规则需要形成明确的诊断策略，避免照搬 TS 规则产生误报。

### 3.3 通用 WXML 语言功能

以下为需要保留或逐步补齐的功能摘要；完整行为、数据来源和验收要求见[WXML 语言功能基线](docs/reference/wxml-language-feature-baseline.md)。先完成模板类型闭环，再按实际使用价值实施。

- 标签名、属性名、属性值、事件及 `wx:` 指令补全。
- 根据组件与已有属性筛选候选项，例如 `picker` 的不同 `mode`。
- 根据 `usingComponents` 解析自定义组件，并提供组件和属性补全。
- 模板表达式成员补全、事件处理函数补全。
- 同名样式及全局样式中的 class 补全和导航。
- Hover、定义跳转、资源路径链接。
- 引用查找与安全重命名（W3；含保守拒绝边界）。
- 语义 token、workspace symbol、inlay hint、signature help、code lens（W3）。
- 诊断 debounce/coalescing、有界交互/诊断队列、排队取消、timeout/crash/协议恢复和根移除/重加 overlay 重放（Q2/W4）。
- 最终本地 release 包和 Linux Zed 运行时冒烟（W4；不是 marketplace 发布）。
- 闭合标签补全、标签配对高亮、基础编辑支持。
- 内置或用户 snippets。
- WXML 保真格式化；保留文本、表达式与 `<wxs>` 语义，解析失败时不修改文件。
- 配置、文件依赖、缓存及更新处理。

历史调研对象使用了多套语言配置和多个格式化后端；这些兼容项不自动成为本项目必须完整复刻的要求。

### 3.4 补全（W2）

补全分两条路径，目录与边界以[功能基线 §3.8](docs/reference/wxml-language-feature-baseline.md#38-补全目录现状w2)为准：

- **静态、同步（不启动 Node）**：`<` 后的标签（基础组件 + 页面/组件自身及 app `usingComponents`）；`</` 后仅当前未闭合元素栈内的标签；开始标签内的属性名（标签属性 + `id`/`class`/`style`/`hidden` + `wx:` 指令 + 常用事件绑定）；引号内枚举属性值（如 `button type`、`picker mode`、`input type`、`image mode`、`navigator open-type`）；事件属性名（`bindtap`、`catchtap`、`bind:tap`、`capture-bind:tap`、`mut-bind:tap` 等）；`class` 的静态 token 补全和资源属性路径补全。
- **条件与组件事实**：`picker`/`input` 的 mode 规则由数据表驱动，补全和诊断使用同一规则；组件属性名、类型、JSDoc、事件和定义来自已解析的本地组件事实，页面/组件本地 `usingComponents` 覆盖 app 全局表。插件/远程组件缺少本地可信元数据时只给降级说明，不伪造属性。
- **依赖类型后端**：`{{ user.na }}` 的表达式成员补全复用 C1 虚拟文件布局与 TypeScript `getCompletionsAtPosition`；`bindtap="on"` 枚举 Page 顶层方法与 Component `methods`。后端不可用、超时或不支持时，表达式/事件补全降级为**空列表**，静态补全、结构和样式能力仍可用。
- **替换范围**：仅替换已输入 token——标签名/属性名替换 token，引号内的值只替换值前缀（引号保留），未加引号的值插入带引号的字面量；snippets 只在客户端声明 `snippetSupport` 且插入位置安全时提供，否则回退普通候选。
- **不产生补全**：纯文本、HTML 注释、`<wxs>` 元素体、Mustache 字符串字面量内。
- **能力声明**：`completionProvider` 无条件声明，`resolveProvider: false`，触发字符 `<`、空格、`:`、`"`、`'`、`.`、`{`、`=`；`isIncomplete: false`，单次最多 256 项；参数非法返回 `-32602`。

目录仍是手写的受限子集，不宣称完整小程序组件覆盖；未收录的标签/属性仍可正常出现在模板中，只是没有静态候选。

### 3.5 语义工具与安全编辑（S/W3–W4）

S 在 W3 合入，能力声明只覆盖有真实 handler 和失败安全测试的工具。逐项状态和证据见[功能基线 §2.2](docs/reference/wxml-language-feature-baseline.md#22-s-语义工具状态w3)，完整验收快照见 [W3 证据](docs/evidence/full-feature-w3.md) 和 [W4 最终证据](docs/evidence/full-feature-final.md)。两轮正式 Rust/安全审核结论均为 `CHANGES_REQUESTED`；`dd15bc1` 修复第 2 轮残留项并由集成探针复验，按用户上限没有开启第 3 轮正式审核，因此不宣称 `APPROVED`。W4 没有再改 S 语义，最终 release 冒烟覆盖了 hover/definition/成员补全/诊断。

| 工具 | 行为 | 边界 |
| --- | --- | --- |
| references | data、事件、组件属性、template 和 class 的静态引用；`wxml.references` 命令返回 `locations` + `coverage` | 目标 WXML 必须打开；不完整时普通请求返回空，命令返回 `partial`/`unsupported` |
| prepareRename / rename | data、组件 `properties`、事件 handler、loop 绑定、class、template name/import；安全 `EditPlan` | 自定义组件 tag rename、静态 include 导致的 caller 覆盖不完整、动态访问、共享 importedData、截断/超限、跨视图冲突保守拒绝 |
| codeAction | 字段拼写 quickfix 和创建事件 handler；Component `methods` 逗号安全 | 只有 `quickfix` |
| semanticTokens/full | UTF-16；结构、Mustache、WXS token；CRLF/emoji | 只处理可完整解析的模板，超限返回空 token 流 |
| workspace/symbol | 多 app + imported template 的 template/组件符号 | 索引不完整时返回空并写日志 |
| inlayHint / signatureHelp | 静态 WXS/表达式类型提示、WXS 静态子集签名 | 不执行 WXS；可关闭 inlayHint |
| codeLens | template 调用和组件引用计数 | 默认开启，可关闭；有上限，不完整时明确标记 |

rename 的 `EditPlan` 只使用 `documentChanges`：打开文件带客户端 version，关闭文件重读并以 `version: null` 返回；校验 expected 原文、UTF-16 范围、去重、不重叠、可写路径和文件身份；失败零编辑；服务端不写盘。没有跨文件事务、原子提交或 undo，硬链接也未检测。

### 3.6 性能与恢复（Q2/W4）

Q2 的 Rust 与安全 R2 均为 `APPROVE`；最终验收快照见 [W4 证据](docs/evidence/full-feature-final.md)，边界见[基线 §2.3/§3.10](docs/reference/wxml-language-feature-baseline.md#23-q2-调度性能与恢复状态w4)。

- **调度**：单后台 worker；交互队列上限 128、诊断队列 8，每 8 个交互任务让诊断前进；同文档诊断按 revision 合并，旧结果不发布。
- **debounce**：默认 120 ms，`wxml.diagnostics.debounceMs` 可设 0–5000；打开、保存和配置变更立即调度。
- **取消**：排队请求立即返回；运行中的同步 TypeScript 调用不可中断，只能等待后端返回或超时，运行占用单独测量。
- **恢复**：timeout/crash/协议不匹配降级并保留静态能力；根移除/重加重放 WXML snapshot 和 script/JSON/style overlay；资源/页面增删、关闭/重开有测试。
- **最终门禁**：static p95 0.178 ms、semantic 39.659 ms、diagnostics stop 164.894 ms、queued cancel 0.674 ms、single TS 402.126 ms、parent RSS 10,460 KiB；strict 10 轮 + RSS baseline `thresholds.pass=true`。
- **限制**：性能/稳定性使用单个小 fixture，不是大项目语料；仍是单 workspace 显式多 app，不支持多根热加；真实编辑器 UI、标准 UI 重装和跨平台未在 Q2 范围。

## 4. 方案评审结论

### 4.1 保留的判断

- Zed 侧采用轻量 Rust/WASM 扩展，负责注册语言及启动外部语言服务。
- 补全、Hover、跳转、诊断等语言智能通过 LSP 提供。
- 可以复用已有 WXML grammar、查询、组件资料与相关实现经验。
- 自用时先从本地路径启动服务，直接安装 Dev Extension，无须建设公开发布和自动下载流水线。

### 4.2 修正的判断

- 不再建议把向现有 Zed WXML 扩展提交 PR 作为主路线；可以独立维护自己的扩展。
- 不再用 Vue/Pug、Emmet、创建组件、自定义颜色等无关能力衡量功能损失。
- 不把“移植原插件约几千行代码”当作核心工作量依据。
- 撤回此前“覆盖约 80%”“几周完成”等粗略估算；完整类型需求需要依据真实验收结果重新评估。
- Rust 静态提取字段适合基础元数据，不能替代完整 TypeScript 类型引擎。
- 不通过执行工作区 JS 的方式提取组件属性；原插件的 `new Function` 路线不采用。

### 4.3 候选方案比较

| 方案 | 可复用价值 | 结论 |
| --- | --- | --- |
| 逐段翻译历史插件实现 | 补全规则、数据结构、部分路径和样式处理经验 | 无法直接解决新增类型需求，不作为整体架构起点 |
| 直接使用已核验的 Rust WXML LSP | WXML 解析、基础 LSP、部分作用域处理；见[核验记录](docs/evidence/2026-11-15-existing-wxml-lsp-audit.md) | 简化的类型模型无法满足准确类型、文档和精确诊断需求 |
| 适配官方模板分析器 | 模板检查视图、位置映射、组件资料与类型集成经验；见[核验记录](docs/evidence/2026-11-15-official-template-analyzer-audit.md) | 值得复用，但不能只启动其 Rust 二进制就认为类型能力齐备 |
| Rust 模板服务 + 完整 TypeScript 后端 | 类型引擎成熟，Rust 专注模板和编辑器集成 | 当前推荐路线 |

## 5. 推荐架构与待验证决策

```text
Zed 本地 Dev Extension（Rust / WASM）
    │ LSP
    ▼
Rust WXML Language Server
    ├─ WXML 解析与编辑上下文
    ├─ 项目、页面和组件定位
    ├─ usingComponents 与基础组件元数据
    ├─ 模板作用域与检查视图生成
    ├─ WXML ↔ 检查视图的源码映射
    ├─ 文档覆盖层、依赖图、缓存与失效处理
    ├─ 补全、Hover、跳转、诊断的路由与结果转换
    └─ 可替换的 TypeScript 类型后端
         ├─ 默认：Rust tsgo 适配器 + 固定原生引擎（`wxml-backend-tsgo`）
         └─ 历史：Node + TypeScript Language Service（已删除，仅保留 p7-pre-delete 证据）
```

### 5.1 内存检查视图

将 WXML 表达式置于 TypeScript 能检查的内存视图中，保留与原始 WXML 的精确映射。复用组件或页面的实际类型上下文（模板数据根为脚本中的 `data`，自定义组件还包括 `properties`），查询类型、JSDoc 和诊断，再映射回模板。

检查视图、辅助符号和探针方法只存在于语言服务内存中，不写回业务源码，不要求用户改变普通页面或组件写法。实际转换方式应通过原型验证后确定，不提前锁定为某一种代码注入形式。

### 5.2 类型后端选择

2026-11-17 的端到端验证优先使用 Node + TypeScript Language Service；该比较结论已由 [ADR-0008](docs/adr/0008-rust-tsgo-no-node-backend.md) 取代：现在采用 **Rust 语义适配器 + 独立原生 tsgo 子进程**，默认 `--stdio` 为 native。Node 后端已删除，仅保留历史基线和 `p7-pre-delete` 证据，不再是运行路径。

后端通过清晰的 `Backend` 接口隔离，避免模板解析、源码映射与某一版本的 TypeScript API 强绑定。迁移已按同一套验收思路推进：先由 native engine/gates/F3 和合理行为测试证明功能，再删除旧实现；不要求旧 TS 断言逐条重现（2026-11-20 owner scope waiver）。

本次实验中的 TypeScript 版本仅表示已测试版本，不代表推荐追随的最新版本。TS 6 时代的 JavaScript Compiler/Language Service API 与原生后端的 LSP 接入方式不同，不能假设直接替换。

### 5.3 未保存文档同步

只把服务挂到 WXML 或只监听磁盘，无法保证 JS/TS 未保存修改即时反映到模板。

计划让同一 WXML 服务也接收相关 JavaScript、TypeScript 文档的打开、修改、关闭事件；不替代用户已有的 JS/TS 主服务，不重复发布无关的脚本诊断。内存覆盖层优先于磁盘内容，并在依赖变化时刷新关联 WXML。

2026-11-16 已在 Zed 1.28.1 验证：服务器实例复用（一个进程持有 WXML/JSON/TS 多文档）、同一项目文档归属、与内置 JS/TS 服务并存、`didSave` 与 watched-files 的更新顺序；详见 [p8-zed.md](docs/evidence/p8-zed.md) 第 7、12 节。

### 5.4 其他技术选择

- WXML 主解析优先评估已有 Tree-sitter grammar，避免为高亮和分析长期维护两套不同的完整语法。
- Mustache 表达式必须按语法处理，不能用简单正则承担完整类型检查和改写。
- WXML、生成文本、LSP 三者之间统一管理字节与 UTF-16 映射。
- WXSS/SCSS 索引由项目内自带的保守解析器实现（`crates/wxml-project/src/styles/`），没有引入 `raffia` 或外部 CSS 解析进程；只记录可证明的静态 selector，动态插值和生成类标记为不完整。
- WXML 格式化已按保真规则实现：只改开始标签内部空白，保留文本空白、表达式与 `<wxs>` 的语义；解析失败或截断时返回零编辑。
- 历史调研中的组件元数据只能作为线索；实际采用的数据必须在本项目登记来源、版本与许可证，并在复用前重新核验（见[来源与许可证记录](docs/reference/provenance.md)）。
- crate 职责和依赖边界见 [ADR-0001](docs/adr/0001-layered-language-service-architecture.md)；LSP 框架、grammar 固定版本和具体 API 在实施接入时核实，不作为已完成事项。

### 5.5 当前工程结构

```text
Cargo.toml                    # workspace、共享 lint 与依赖版本
crates/
├── wxml-text                 # UTF-8 字节偏移 ↔ LSP UTF-16 位置、行索引
├── wxml-syntax               # WXML 词法/语法、Mustache 表达式、源码映射
├── wxml-project              # 项目、页面/组件、usingComponents、tsconfig
├── wxml-types                # C0 契约、请求上下文、模板语义、WXS/静态摘要、后端接口与 mock
├── wxml-backend-tsgo         # native tsgo 适配：native/script/shadow/query
├── wxml-fixtures             # 共享验收样本的结构校验器
├── wxml-delivery             # 本地 release 打包器与验证安装器（Q1/F3）
└── wxml-lsp                  # LSP 传输、多项目宿主、native 默认后端与请求路由（二进制）
editors/zed                   # 本地 Zed Dev Extension（独立 workspace）
scripts/build-release.sh      # 本地构建 Node-free v2 native 产物；不发布
tests/fixtures                # AC-001..AC-013 样本
docs/                         # 参考、ADR、证据
```

目标依赖按 [ADR-0001](docs/adr/0001-layered-language-service-architecture.md) 统一：`wxml-lsp` 依赖各功能层，`wxml-types` 允许依赖 `wxml-syntax`，各层按需依赖 `wxml-text`；`wxml-project` 独立于 syntax/types，项目事实由 LSP 适配为 types 输入；`wxml-backend-tsgo` 位于 `wxml-lsp → wxml-backend-tsgo → wxml-types` 之间。禁止反向依赖和 `types → project`。当前 workspace 已包含上表八个 crate；`wxml-types` 落地 C0 契约、请求上下文、后端接口、mock 与模板语义；`wxml-lsp` 已接入真实 `ProjectHost`、native 默认后端与 LSP 路由；C1 类型桥与 Node 后端已在 P7 删除（`backends/typescript/` 不再存在，仅保留历史证据）；Zed 扩展位于 `editors/zed/`；`wxml-fixtures` 只校验共享样本结构。业务 `.ts/.js/.d.ts` 输入、WXS/WXML 语义、fixtures 和 tree-sitter grammar 不随 Node 删除。

## 6. 已有验证与证据边界

以下是隔离实验的结论摘要。完整环境、方法、限制和最小原始数据见[证据索引](docs/evidence/README.md)；实验在仓库外运行，临时目录不再作为证据来源，也不能把实验结论描述为已实现功能。

### 6.1 TypeScript Language Service 机制实验

实验版本：`typescript@6.0.2`、`miniprogram-api-typings@5.3.1`。

两组实验共 22 项断言通过，覆盖跨文件类型和 JSDoc、字段错误、可空与条件窄化、循环元素、组件属性、JS JSDoc、未保存类型修改，以及包含中文和 emoji 的 WXML 位置映射。

限制：WXML 映射实验只覆盖受限表达式样本，不是完整模板转换器；未覆盖完整项目、所有组件写法、完整 Behavior 合并或 Zed 端到端行为。实验验证的是类型后端机制；模板数据根规则见 §3.1，独立 interface 不作为模板根。实验期间没有修改用户业务项目。原始探针未保留，记录等级为 E0；详见[实验记录](docs/evidence/2026-11-15-typescript-language-service.md)。

### 6.2 现有 Rust WXML LSP 核验

核验对象：现有 Rust WXML 语言服务 0.1.9，提交 `84425dad66de0991e0cdf24e9c86842dc8209cdd`。

实际运行 33 个独立场景、66 次 hover 请求；该版本的 112 项自带测试通过。发现的缺口包括字段说明缺失、跨文件和联合类型支持不足、条件窄化不完整、属性绑定缺少类型兼容性检查，以及部分诊断位置与未保存修改处理错误。

因此可以借鉴其基础设施，不能把现有类型分析层当作本项目的准确类型来源。自带测试通过，不代表通过本项目需求验收。完整结果和限制见[核验记录](docs/evidence/2026-11-15-existing-wxml-lsp-audit.md)，原始数据见[证据数据](docs/evidence/data/existing-wxml-lsp-audit/)。

### 6.3 官方模板分析器核验

源码和保存的实验结果表明，类型能力涉及客户端侧 TypeScript 集成，不能只连接其 Rust LSP 就认为能力完整。

使用小程序 backend 配置的测试中，导出组件样本可以获得字段类型、说明和错误；裸 `Component(...)` / `Page(...)` 样本退化为 `any`。未保存的导入类型更新还存在需要 WXML 刷新才生效的问题。这些是所测配置和样本的结果，不代表所有版本与写法都具有完全相同的行为。完整结果、握手要求和限制见[核验记录](docs/evidence/2026-11-15-official-template-analyzer-audit.md)，原始数据见[证据数据](docs/evidence/data/official-template-analyzer-audit/)。

### 6.4 TypeScript 原生语言服务实验

保存的实验分别使用原生服务 7.0.2 与 7.1 开发版。普通、未落盘的虚拟 TS 文件能够获得 hover 类型、JSDoc 和诊断，修复表达式后诊断清除，未保存的依赖说明修改也能更新。

虚拟 `file://` 文档可解析同目录导入；测试中的自定义 URI scheme 无法正确解析相对模块。显式 `files` 配置下，虚拟文件还出现未归入预期配置项目的情况。

限制：这只验证普通虚拟 TS 文件，不等于已验证完整的小程序检查视图、所有 `tsconfig` 行为或 Zed 集成。完整结果、复跑方式和限制见[实验记录](docs/evidence/2026-11-15-native-typescript-service.md)，原始数据见[证据数据](docs/evidence/data/native-typescript-service/)。

## 7. 实施计划

当前已完成 P0–P8、G1、W2/W3/W4、Q1/Q2 和 Rust + tsgo 迁移的 P0–P7：多 crate 的 Rust workspace、C0/C1 契约、WXML 语法与项目模型、模板语义、LSP、Zed 扩展代码、native tsgo 默认后端、F3 本地交付与真实 native gates。P7 删除已完成（2026-11-20，删除提交 `c4ddf32`；`p7-removed` 为旧 tag）：删除范围已冻结、retarget 已完成，旧 Node 后端、旧 TS 实现/测试和 v1 写路径均已删除，删除后复验通过（验收为 PASS-with-followup，见 §0.1）。Zed 扩展已通过独立评审，标准 Dev Extension 安装、真实 grammar 渲染和 hover/definition 可视化的历史 Node 证据见 [P8 证据](docs/evidence/p8-zed.md)；当前 native 验收见 §0.1、§10 与 F3 合同。

### 阶段 1：固定样本和验收规则

- [x] 选择具有代表性的 Page、Component、`data`/`properties`、跨文件类型引用和 JS JSDoc 样本。
- [x] 覆盖 `data`、`properties`、跨文件类型、路径别名、联合类型、循环与条件。
- [x] 明确字段说明优先级，以及缺失类型信息时的展示方式。
- [x] 明确严格空值、模板运行时转换及绑定属性兼容性的诊断策略。
- [x] 建立可重复运行的正例与反例，避免只检查“能返回 hover”。

产出：需求到测试样本的对应关系，以及第一版诊断策略。

状态：C0/C1 已冻结；样本、契约与类型分析 runner 已落地。样本和契约本身不是用户功能，实际能力见 §10。

### 阶段 2：最小类型闭环

- [x] 建立 Rust 服务与 TypeScript 后端的最小通信接口。
- [x] 识别普通 Page / Component 及其 `data`/`properties`，不要求修改业务写法。
- [x] 生成内存检查视图，建立表达式源码映射。
- [x] 对 `user.name`、`title` 返回准确类型和字段说明。
- [x] 对 `user.nmae` 返回精确到字段的诊断范围。
- [x] 覆盖中文、emoji、CRLF 和边界位置。

产出：可通过 stdio 测试的最小语言服务；此时仍不宣称支持完整 WXML。

状态（2026-11-16，历史 Node 后端）：上述六项已在 LSP/Node 层由 G1 真实端到端覆盖（AC-001/002/003/013）；Zed 实机观感仍见阶段 3。

### 阶段 3：Zed 本地端到端验证

- [x] 创建自用 Dev Extension，从明确的本地路径启动服务。
- [x] 在真实 WXML 文档中验证 hover 和错误标注。
- [x] 让相关 JS/TS 文档变更进入同一服务的内存覆盖层。
- [x] 保留已有 JS/TS 服务，验证并存行为与实例归属。
- [ ] 验证未保存的 `data`/`properties` 字段类型和说明（含其引用的跨文件类型）、脚本及模板修改能够正确传播。
- [ ] 记录响应时间、诊断误报、错误消失和服务重启后的恢复情况。

产出：可本地安装、可评审的最小验证版及实际测试记录。

状态（2026-11-16）：扩展代码、host 测试、wasm 构建和真实 grammar 构建完成；标准 UI `zed: install dev extension` 已在隔离 Xvfb 中实际执行，确认 symlink + `dev=true`、五语言注册、release LSP/Node 启动。干净 fixture 的 `title`/`user.name` hover、definition、标签/属性/事件/成员补全、未知成员诊断和重启恢复已逐项执行；真实 TS 项目（`Component` 注册页面）和真实 JS 项目（无 tsconfig 降级）已记录，见 [p8-zed.md](docs/evidence/p8-zed.md) 第 13 节。未保存 `data` 字段类型已验收；跨文件未保存类型传播、连续键入的响应时间、Windows/macOS 实机仍未验收，因此第 5、6 项不勾选。W4 另外对最终 release 包做了 Linux Zed 运行时冒烟（hover/definition/成员补全/诊断），但这是手工安装发布包，不是标准 UI 重装，见 [W4 证据](docs/evidence/full-feature-final.md)。

### 阶段 4：完整模板作用域与项目类型

- [ ] 完善表达式语法、重写和源码映射，移除原型阶段的受限假设。
- [x] 支持 `wx:for-item/index`、嵌套遮蔽及作用域恢复。
- [x] 支持条件分支、可空和判别联合的适用窄化规则。
- [x] 完善 `tsconfig` 项目归属、路径别名、导入、依赖图和缓存失效（在显式 app 边界内）。
- [x] 补充符合项目实际写法的 Behavior、共享数据和复杂属性/guards 样本。
- [x] 增加成员补全、定义跳转和自定义组件属性绑定检查。

产出：围绕实际小程序项目验收的模板类型功能。

状态（2026-11-17）：`wx:for` 已支持嵌套 typed locals、元素/索引类型和作用域恢复；条件窄化、多视图/路径别名、组件属性绑定、成员补全和事件链路已实现。W2 增加静态 Behavior/共享 data/props、复杂 props（`optionalTypes`/`required`/真实联合）、guards/loops、JS/JSDoc、template/import/include 与 WXS 静态摘要；未保存跨文件 overlay 与 Node 重启重放已有 LSP 测试。AC-005/006/008/012 已完成。反向编辑、模板体逐调用实例类型检查，以及表达式重写/源码映射的剩余受限假设仍未完成。

### 阶段 5：补齐常用 WXML 功能

- [x] 内置标签、属性、值、指令和事件补全及文档。
- [x] `usingComponents` 解析和自定义组件信息。
- [x] 事件函数、class、资源路径的补全与导航。
- [x] 闭合标签补全、标签配对（linked editing）和 snippets。
- [x] references、安全 rename、quickfix、semanticTokens、workspaceSymbol、inlayHint、signatureHelp、codeLens（W3，带保守拒绝）。
- [x] 按需求补充样式索引和格式化，不引入无关兼容层。

产出：日常可用的自用 WXML 工具。

状态（2026-11-17）：F-01–F-07 补全、静态组件/标签/属性/事件 hover、基础 diagnostics/definition、事件处理函数诊断/definition/补全已完成。W2 增加 class/WXSS/SCSS 索引、资源补全/documentLink/definition/存在性诊断、内置与工作区 snippets、保真 document/range formatting、linked editing、folding、selection range、documentSymbol/outline。W3 合入 S 的 references、prepareRename/rename、codeAction、semanticTokens、workspaceSymbol、inlayHint、signatureHelp 和 codeLens；tag rename 仍保守拒绝，linked editing 继续负责标签配对。

### 阶段 6：优化与可选后端替换

- [x] 根据实测性能处理后台任务、取消、诊断防抖及大项目缓存（Q2）。
- [x] 评估原生 TypeScript 后端能否通过功能验收（ADR-0008；以 native engine/gates/F3/合理行为测试为准，不要求旧 TS 逐条 parity）。
- [x] 完成去除 Node 的 P7 删除（2026-11-20；Node 运行时、旧 TS、12 个 delete gate 与 v1 写路径已删除，11 个 retarget gate 为 native-only）。
- [x] 补齐本地构建、安装、升级、回退和排错说明（Q1）。
- [x] 最终本地 release 与 Linux Zed 运行时冒烟（W4；Node 历史包，native 包见 F3）。

状态（2026-11-20）：Q2 调度/恢复在 W4 已通过；本地交付已合入并演进为 schema v2 native 包。原生 tsgo 已为默认 `--stdio` 后端；F3 为 16/0/0，native gates 40/40，台账终态 109/0/130/29/71。P7 删除已完成（删除提交 `c4ddf32`），Node/旧 TS/v1 已移除；删除后 release/install/Zed smoke 通过（Zed 16/16），无 Node 进程，F-02 修复已落地、2592/0/73 待 p7-final-verification 追加（不是 tag 事实）。functional smoke P1 fixed / P2 known；stability 4 implemented / 1 blocked；第三方许可 155/155，首方 LICENSE BLOCKED、对外发布 BLOCKED（内部安装可用）。W4 的 Node 性能/发布数字只作为历史证据。未实现 marketplace/远端发布和发布者签名；大项目语料、标准 UI 最终重跑和跨平台验收仍未完成。

公开发布、自动下载和多平台发布流水线不在当前计划中；Q1 的下载器仅在用户显式 `--enable-download` 时读取 HTTPS 源，不构成 marketplace 发布或后台自动更新。

## 8. 第一阶段验收清单

以下清单指首个可在 Zed 中评审的类型版本，预计在上述阶段 3 完成时验收；不表示仅完成阶段 1 的样本整理即可满足。

- [x] 普通 `Page({...})` 的 `data`、`Component({...})` 的 `data`/`properties` 无需改写即可关联模板数据。
- [x] 悬停 `user.name`、`title` 得到实际类型及该字段的说明。
- [x] 不存在字段被准确标记，合法表达式不被误报。
- [x] 中文、emoji、不同换行格式下位置正确。
- [ ] 未保存的 `data`/`properties` 字段类型和说明（含引用的跨文件类型）变更能够更新到模板。
- [x] 错误修复后旧诊断及时清除。
- [x] 最小循环与条件样本中的类型正确；尚不支持的情况明确记录。
- [x] 虚拟文件与辅助代码不污染业务源码或磁盘项目。
- [x] 与已有 JS/TS 服务并存，不重复发布无关脚本诊断。
- [x] 文档明确区分已支持、降级与待实现情况。

状态（2026-11-17）：G1 在 LSP 层通过 AC-001/002/003/011/013，AC-005/006/008/012 已完成；W2 增加跨文件未保存 overlay、Behavior/共享类型、WXS/template 静态语义和 Node 重启重放测试。标准 UI 安装、干净 fixture 的 hover/definition/completion/diagnostics、真实 TS/JS 项目和重启恢复已在隔离 Zed 中验收；未保存 TS `data` 字段类型同步已验证，保存后诊断刷新。W4 的最终 release 包也完成了 Linux Zed 运行时冒烟，但仍是手工安装发布包，不是标准 UI 重装。Zed UI 中的跨文件未保存类型传播、连续键入响应时间、Windows/macOS 实机仍未验收，因此第 5 项和整体清单不勾选。

## 9. 需求与来源索引

本项目需求不依赖外部项目或文件。通用 WXML 语言功能的需求已整理为[WXML 语言功能基线](docs/reference/wxml-language-feature-baseline.md)；实验结论、方法和最小数据见[证据索引](docs/evidence/README.md)。

调研对象的版本、提交和许可证集中记录在[来源与许可证记录](docs/reference/provenance.md)。该记录仅用于归属、合规和复查，不作为需求来源；外部仓库或临时目录不能作为本项目验收的依据。

## 10. 当前状态

快照：2026-11-20（迁移收口 `2ce4626`、预删除冻结 `8bda306`、pre-delete 记录 `4dd62b1`、retarget 完成 `3e24c44`、matrix 源归档 `4035bb7`、P7 删除完成 `c4ddf32`；旧 tag `p7-removed`=`b6c1ced`）。当前默认后端为 **native tsgo**；Node 后端、旧 TS 实现/测试和 v1 写路径已删除，ADR-0008 写 **Node removed**。

- 已完成（native）：`wxml-backend-tsgo` 适配器、固定 `7.1.0-beta` 引擎与 typings、默认 `--stdio` native/HostOnly、损坏安装 fail closed、真实 native gates 40/40、F3 交付合同 16/0/0、真实 matrix 22 exact/11 blocked（scenario source 已归档 fixture），以及 Q2 调度/取消/恢复、语义工具和本地 schema v2 交付。
- 迁移台账：**109 `green` / 0 `planned` / 130 `reviewed` / 29 `waived` / 71 `retired-with-decision`**（339 总数）。只有 `parity=exact` 计入 `green`；`reviewed`/`waived`/`retired` 是非 green 终态，不计作 parity。依据是 2026-11-20 owner C-lite waiver 和 [P7 收口台账](docs/evidence/2026-11-20-tsgo-p7-closeout-ledger.md)。
- P7 范围与状态：`crates/wxml-lsp/src/backend_node/**`（8 文件/102 单元测试）、`backends/typescript/**`（99 tracked + 34 dist；含 12 个 delete gate）、15 个 v1 delivery 写路径和 5 个 legacy Node env；23 个旧 gate 中 12 个 delete、11 个 retarget。**11 个 retarget 已全部 native-only**（9 个 stdio + perf/stability，`3e24c44`），不标记 `old_test_removed`；stability 后续经 scenario 证据收敛为 **4 implemented / 1 blocked**（外部 RSS baseline），ledger 未回填，见 [stability 证据](docs/evidence/2026-11-20-tsgo-stability-scenarios.md)。删除已完成（删除提交 `c4ddf32`；`p7-removed`/`b6c1ced` 只是旧 tag）；冻结范围与 `p7-pre-delete` tag 保留在历史，当前无 `--backend node`。
- 显式保留：业务 `.ts/.js/.d.ts` 输入与 JSDoc、WXS/WXML 语义、tree-sitter grammar、统一 fixtures、typings/`assets.lock.json`、schema v1 只读识别（`list/current/verify/env`、`verify_installed`、`read_legacy_manifest`）、`build_native_package` 与历史证据。**去 Node 不等于删除 TypeScript 语言支持。**
- 验证口径：native engine、native gates、F3 合同和合理行为测试；不声称旧 TypeScript/Node 断言逐条 parity，`reviewed`/`waived` 也不写成“已迁移”。native gates 是 opt-in required job，不是默认 CI 绿或 `mandatory_green`。删除后的最终验收已按 [P7 最终文档与验收清单](docs/evidence/2026-11-20-tsgo-p7-final-docs.md) 执行并记录。
- 删除后验证：F-02 修复已落地，**2592 passed / 0 failed / 73 ignored 待 `p7-final-verification` 追加**（release 预算 200 ms 未放宽；不是 tag 事实）；release/install/Zed smoke 通过（Zed 16/16）；无 Node/server.js 进程；functional P1 fixed / P2 known；stability 4/1；第三方许可 155/155，首方 LICENSE BLOCKED、对外发布 BLOCKED（内部安装可用）。
- 未完成/限制：23 个旧 gate 的 mandatory/review（native gates 40/40 是 opt-in required job）、G5/G6/G7、完整多 workspace/多根热加、大项目语料、Windows/macOS 实机、真实主桌面 Wayland、标准 UI 在最终版本上的重跑、marketplace/远端发布/发布者签名。平台证据仍只有 Linux x86_64。

### 10.1 历史快照（W4，2026-11-17；Node 后端）

以下记录保留 W4 Node 后端的原始事实，不作为当前验收：

- 已完成：C0/C1 契约；`wxml-text`、`wxml-syntax`（容错 lexer/parser、表达式 Pratt、WXS rawtext、资源上限）、`wxml-project`（URI/路径、overlay、显式多 app、tsconfig、sources、styles/resources）、`wxml-types`（类型契约、`wx:for`/guards、Behavior、template/WXS 静态语义、脚本符号事实、后端接口与 mock）、`wxml-lsp`（JSON-RPC、多项目宿主、Node 后端、hover/diagnostics/definition/completion、格式/结构/链接、语义工具与安全编辑、Q2 调度/取消/恢复、版本屏障/缓存/超时/重启）、Node 类型桥与后端、P8 Zed 扩展、Q1 本地打包/安装器、S 语义工具、Q2 调度/恢复、最终本地 release。
- 验证（W4 最终日志）：Rust workspace `1283 passed / 0 failed / 23 ignored`（49 suites）；Node 串行 `333 / 333`；23 个 ignored 门禁全部通过；性能 static p95 0.178 ms、semantic 39.659 ms、diagnostics stop 164.894 ms、queued cancel 0.674 ms、single TS 402.126 ms、parent RSS 10,460 KiB；strict 10 轮 + RSS baseline `pass`；覆盖率 lines 87.24% / functions 88.85% / regions 86.71%；`cargo audit` 0 vulnerabilities、`cargo deny` advisories ok。W3 的 `1251 / 21`、Node `326 / 326` 与 W2 的 `1095 / 20`、Node `304 / 305`、Zed `25 passed` 保持历史记录，不与 W4 混用。
- 发布（W4）：最终 release 两次构建 SHA-256 均为 `c09062b16dd6bfea4ee03f99f13665cf4858130aae83eefec9c04cd94d3815ab`（8,696,984 B、protocol 2、214 files），install/verify/verify-package 通过；Linux Zed 运行时冒烟使用最终发布包，hover `(property) title: string` + “页面标题”、definition、成员补全和 `nmae` 诊断均通过。这是手工安装发布包，不是标准 UI 重跑；标准 UI 历史证据仍见 [P8 第 13 节](docs/evidence/p8-zed.md)。
- 未完成/限制：完整多 workspace/多根热加、大项目语料、Windows/macOS 实机、真实主桌面 Wayland、标准 UI 在最终版本上的重跑、marketplace/远端发布/发布者签名。S 的 tag rename、静态 include 的 caller 覆盖、动态访问、共享 importedData 跨脚本、跨文件事务/undo 和硬链接检测仍是明确边界；TypeScript 同步调用不可中断，发布包仅在 Linux 验证。
