# WXML LSP

个人自用的微信小程序 WXML 语言服务器，目标是在 Zed 中得到可追踪到脚本类型的模板体验：悬停显示 `data`/`properties` 字段的真实类型与 JSDoc，定义跳转到声明，类型错误落在 WXML 的准确范围。

实现是 Rust，无 Node 运行时。类型能力来自进程内固定版本的 tsrs 类型引擎（`7.1.0-beta.tsrs4`，内嵌 TypeScript 标准库）和 `miniprogram-api-typings@5.3.1` 类型资产；引擎由 Cargo 精确 pin，typings 由 `assets.lock.json` 固定。

- 形态：本地安装、自用；没有 marketplace 发布、远端 release 或发布者签名。
- 平台：仅在 Linux x86_64 验证；Windows/macOS 未验证。
- 工具链：MSRV 1.100（workspace `Cargo.toml` 的 `rust-version`），edition 2024。
- 决策背景：[ADR 索引](docs/adr/README.md)，重点见 [ADR-0008](docs/adr/0008-rust-tsgo-no-node-backend.md)（native 默认，Node 已移除）和 [ADR-0012](docs/adr/0012-in-process-tsrs-session.md)（进程内类型会话）。

## 架构与分层

```text
Zed 扩展（editors/zed，Rust/WASM）
  │  LSP over stdio
  ▼
wxml-lsp            传输、多项目宿主、请求路由、调度与降级
  ├─ wxml-backend-tsgo   进程内类型会话适配：只读 VFS、shadow、查询、源码映射
  ├─ wxml-types          C0 契约、模板语义、WXS/静态摘要、后端接口
  ├─ wxml-project        URI/路径、overlay、多 app/惰性发现、tsconfig、样式/资源索引
  ├─ wxml-syntax         容错 lexer/parser、Mustache、WXS rawtext
  └─ wxml-text           UTF-8 字节偏移 ↔ LSP UTF-16 位置、行索引
```

依赖方向：`wxml-text` → `wxml-syntax` → `wxml-types` / `wxml-project` → `wxml-backend-tsgo` → `wxml-lsp`。禁止反向依赖，也不允许 `wxml-types → wxml-project`；边界见 [ADR-0001](docs/adr/0001-layered-language-service-architecture.md)。

类型路径：`wxml-lsp` 把 WXML、关联脚本与配置投影成内存快照交给 `wxml-backend-tsgo`，后者在进程内驱动类型会话完成类型查询，再把结果映射回模板的 UTF-16 范围。业务文件不写盘，也不需要修改 `Page({...})`/`Component({...})` 写法。

## 构建

工具链为 stable（`rust-toolchain.toml`，含 clippy/rustfmt），edition 2024，MSRV 为 Rust 1.100。

```bash
cargo build --release
# 产物：target/release/wxml-lsp
```

服务入口：

```bash
wxml-lsp --stdio              # native 默认后端
wxml-lsp --stdio --backend native   # 严格 native：要求已安装类型资产
wxml-lsp --stdio --mock       # 进程内 mock 后端，仅用于测试
wxml-lsp --version | --help
```

单独构建二进制不足以获得类型能力：运行时需要下面安装好的 typings 资产；TypeScript 标准库随二进制内嵌，不需要额外文件。

## 安装

`scripts/install-native.sh` 把 release 二进制与固定类型资产组装成运行时可发现的安装根：

```text
<root>/
├── bin/wxml-lsp
└── typings/miniprogram-api-typings/
```

`assets.lock.json` 是类型资产的固定输入：脚本只下载其中登记的 typings tarball，校验字节数与 SHA-256 后解包到上述位置，再从已安装布局做 fail-closed 检查。引擎没有可执行文件或标准库资产需要安装；旧布局中的 `engine/` 与 `lib/tsc` 会被忽略。安装根必须是完整包，不能只拷贝二进制。

```bash
cargo build --release
root="$HOME/.local/share/wxml-lsp"
scripts/install-native.sh --root "$root"
# 其他选项（--binary、--assets）见 scripts/install-native.sh --help
```

运行时发现规则：`wxml-lsp` 只从自身可执行文件（canonical 后的 `current_exe`）出发，要求可执行文件位于 `<package>/bin/wxml-lsp`，且同根下存在 `typings/miniprogram-api-typings`；不读 `PATH`、不读工作目录，也不再识别 `TSGO_BIN`/`TSGO_ASSET_ROOT` 等引擎覆盖变量。typings 摘要必须与内建 pin 一致，否则在 LSP 握手前 fail closed。安装发现阶段唯一的降级是 `NotInstalled`：从 `target/release/` 直接运行或布局不完整时退化为 `HostOnly`（静态能力可用，类型查询返回 `Unavailable`）。运行期类型会话初始化失败或中毒时，类型能力降级为 `Unavailable`/`SessionPoisoned`，语言服务器其余功能继续工作。

## 在 Zed 中使用

1. 构建或安装好 native 包（见上）。
2. 启动 Zed 前导出服务器绝对路径（必须是绝对路径；扩展不探测 worktree、不查 `PATH`）：

   ```bash
   export WXML_LSP_BIN="$HOME/.local/share/wxml-lsp/bin/wxml-lsp"
   ```

3. 命令面板执行 `zed: install dev extension`，选择 `editors/zed`；扩展 id 为 `wxml-lsp`，名称为 `WXML LSP`。

`WXML_LSP_BIN` 缺失、相对路径或不可执行时，扩展返回错误且不启动服务器，没有回退。扩展注册 WXML、JavaScript、TypeScript、TSX、JSON（不含 JSX），同一进程接收关联文档的 `didOpen`/`didChange`/`didSave`，未保存的脚本修改会反映到模板查询。

### 初始化选项

写入工作区 `.zed/settings.json` 的 `lsp.wxml-lsp.initialization_options`。路径都相对 LSP workspace root。

默认**不需要配置 `apps`**：LSP 从当前打开的 WXML 向上寻找最近的 `app.json` 和独立最近的 `tsconfig.json`，并从根 `package.json#workspaces` / `pnpm-workspace.yaml` 挂载工作区包。省略 `initializationOptions`、`null`、`{}` 或 `{ "apps": [] }` 均进入该零配置模式。worktree 因此不需要复制 `.zed/settings.json`。

需要固定多 app 边界时，可以显式列出：

```json
{
  "lsp": {
    "wxml-lsp": {
      "initialization_options": {
        "apps": [
          { "root": "apps/miniprogram/src", "projectRoot": "apps/miniprogram" },
          { "root": "apps/sub-merchant/src", "projectRoot": "apps/sub-merchant" }
        ],
        "wxml": {
          "globalStyles": ["styles/theme.scss"],
          "styleDiagnostics": { "resources": true, "classes": false }
        },
        "snippetsPath": ".wxml-lsp/snippets.json",
        "semanticTools": { "inlayHints": true, "codeLens": true }
      }
    }
  }
}
```

| 选项 | 作用 |
| --- | --- |
| `apps` | 可选。省略时启用零配置惰性发现；显式提供时，每项 `root` 指向 `app.json` 所在目录，可选 `projectRoot`、`tsconfig`（默认 `<projectRoot>/tsconfig.json`）和 `dependencies`（工作区包目录，加入类型快照并按 `package.json` exports 生成 `paths`）。非法配置返回 `-32602`，不静默回退。 |
| `wxml.globalStyles` | 额外全局样式入口；`app.wxss`/`app.scss` 仍按 `styleIsolation` 自动参与。 |
| `wxml.styleDiagnostics` | `resources` 默认开启；`classes` 默认关闭，只对可证明的静态 token 生效。 |
| `snippetsPath` | 可选；省略时读取 `<workspace>/.wxml-lsp/snippets.json`，修改后经 watched-file 更新。 |
| `semanticTools` | 关闭后不再广告 inlayHint / codeLens；其余语义工具没有独立开关。 |

零配置模式从当前 WXML 向上找最近 `app.json`，并独立寻找最近 `tsconfig.json`；页面/组件不要求出现在 `app.json` 中，只要存在同 basename 的 `.ts`/`.js` 即可建立惰性视图。`app.json` 仍是路由、全局 `usingComponents` 和页面配置的增强来源，但不是 hover 的前置条件。

工作区包源码从根 `package.json#workspaces` 或 `pnpm-workspace.yaml` 发现，只挂载当前小程序包 `dependencies` / `devDependencies` / `peerDependencies` 中列出的同名工作区包；只读取包清单和源码，不读取 `node_modules`，也不推断第三方包图。显式 `dependencies` 配置仍可用于固定边界。详见 [ADR-0010](docs/adr/0010-zero-config-lazy-discovery.md)、[ADR-0006](docs/adr/0006-explicit-mini-program-roots.md) 和 [Zed 扩展说明](editors/zed/README.md)。

配置只在启动时消费，修改后需重启 LSP。当前最近 `tsconfig.json` 选择不展开 solution-style `references`；复杂项目建议使用显式 `apps`。

## 能力摘要

| 类别 | 内容 |
| --- | --- |
| 类型体验 | hover（类型 + JSDoc）、definition、模板表达式成员补全、事件处理函数补全、诊断；覆盖 `data`、组件 `properties`、跨文件 `interface`/`type`、JS + JSDoc。 |
| 静态语言功能 | 标签/属性/属性值/`wx:` 指令补全，组件与属性事实，class 与资源路径补全/导航/诊断，documentSymbol/outline，folding、selection range、linked editing。 |
| 格式化 | 全量与 range 保真格式化：只规范开始标签内部空白，解析失败时零编辑。 |
| 语义工具 | references、prepareRename/rename（安全 `EditPlan`）、codeAction（quickfix）、semanticTokens、workspaceSymbol、inlayHint、signatureHelp、codeLens。 |
| 编辑一致性 | overlay 优先于磁盘；诊断 debounce（默认 120 ms，可配 0–5000）、同文档合并、有界队列、排队取消、timeout 丢弃迟到结果；类型会话不可恢复时显式降级，不重启进程。 |

rename 只接受 `documentChanges`，打开文件带客户端 version、关闭文件重读后以 `version: null` 返回，失败零编辑，服务端不写盘。自定义组件 tag rename、动态访问、跨视图冲突等场景保守拒绝。诊断策略见 [类型诊断策略](docs/reference/type-diagnostics.md)，通用行为见 [WXML 语言功能基线](docs/reference/wxml-language-feature-baseline.md)。

## 仓库结构

```text
Cargo.toml                 # workspace、共享 lint、依赖版本
crates/
├── wxml-text              # 字节偏移 ↔ UTF-16、行索引
├── wxml-syntax            # lexer/parser、Mustache、源码映射
├── wxml-project           # 项目/页面/组件、usingComponents、tsconfig、样式/资源索引
├── wxml-types             # C0 契约、模板语义、WXS/静态摘要、后端接口与 mock
├── wxml-backend-tsgo      # 进程内类型会话适配（native/script/shadow/query）
└── wxml-lsp               # LSP 二进制
editors/zed/               # Zed Dev Extension（独立 workspace）
scripts/install-native.sh  # 组装 native 安装根
tests/fixtures/            # AC-001..AC-013 验收样本
docs/adr/                  # 架构决策记录
docs/reference/            # 契约、诊断策略、功能基线、来源记录
openspec/                  # spec-driven 变更流程（changes/、archive/、specs/）
assets.lock.json           # typings 类型资产的固定输入
```

## 检查与测试

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo doc --workspace --no-deps
cargo deny --offline check   # 首次需先 cargo deny fetch db
```

Zed 扩展是独立 workspace，需要 `wasm32-wasip2` target：

```bash
cd editors/zed
cargo fmt --all -- --check
cargo check --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --target wasm32-wasip2 --locked
```

CI（`.github/workflows/ci.yml`）只有 `check` 作业，执行 fmt、clippy（`-D warnings`）、test、doc 和 cargo-deny。

默认 `cargo test` 不需要真实类型引擎：真实类型用例在未设置门禁时跳过。要运行进程内真实类型门禁，提供 typings 根并要求缺失即失败：

```bash
TSGO_TYPINGS_ROOT="$HOME/.local/share/wxml-lsp/typings/miniprogram-api-typings" \
WXML_REQUIRE_REAL_ENGINE=1 \
cargo test --workspace --all-features
```

两个变量名称沿用历史门禁，但语义已是“进程内引擎 + 外部 typings”：不会启动外部引擎，也没有 `TSGO_BIN`/`TSGO_ASSET_ROOT` 覆盖入口。

## 已知限制

- 默认零配置惰性发现，也支持单个 LSP workspace 内的显式多 app；不支持完整多 workspace，`apps`/`projectRoot` 或 tsconfig 变更后需重启。最近 `tsconfig.json` 选择暂不展开 solution-style `references`。
- 惰性视图只对已经打开或查询过的 WXML 建立；`view_templates` 和部分语义工具不会枚举尚未打开的模板。
- 模板体不做逐次实例化的完整类型检查；WXS 仅支持静态子集且从不执行。
- 插件/远程组件缺少本地可信元数据时降级，不伪造属性；动态 template/`is`、动态 WXS 导出不猜测。
- 性能与稳定性只在单个小 fixture 上验证，没有大项目语料；真实主桌面 Wayland 与连续键入延迟未验收。
- 类型引擎在语言服务器进程内运行：单请求 panic 以该请求失败返回，但会使类型会话中毒、类型能力降级；OOM、abort、栈溢出会终止整个进程；运行中的类型计算不可取消，超时只是放弃等待。
- 会话中毒后类型能力持续不可用，必须重启语言服务器才能恢复；不自动重建会话。
- 没有发布者签名、自动更新或跨平台安装器；信任模型见 [ADR-0007](docs/adr/0007-local-delivery-trust-model.md)。

## 文档

- [文档索引](docs/README.md)
- [ADR 索引](docs/adr/README.md)：0001 分层、0002 类型检查与源码映射、0003 文档同步、0004 交付闸门、0005 文档自包含、0006 显式根、0007 交付信任、0008 native 后端、0009 瘦身、0010 零配置惰性发现、0012 进程内类型会话
- [C0 基础契约](docs/reference/contracts.md)、[模板类型诊断策略](docs/reference/type-diagnostics.md)
- [原生类型后端合同](docs/reference/tsgo-native-contract.md)（tsrs 进程内会话口径）
- [来源与许可证记录](docs/reference/provenance.md)
- 变更流程：[`openspec/`](openspec/)（活跃变更在 `changes/`，归档在 `changes/archive/`；`specs/` 尚未沉淀能力规格）
