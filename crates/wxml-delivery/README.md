# wxml-delivery

`wxml-delivery` 是 WXML LSP 的本地 release 打包器与独立安装器。它只构建/安装本机产物，**不发布、不上传、不创建远端 tag/release**。Zed 扩展本身仍只接受 `WXML_LSP_BIN` 绝对路径；本 crate 负责生成可信、可验证的安装产物，并由用户把稳定路径交给 Zed。

## 组件

- `wxml-lsp-package`：从 staged 目录生成版本化目录、`.tar.gz`、`.sha256`、`SHA256SUMS`、`index.json` 和 `manifest.json`。
- `wxml-lsp-install`：从 HTTPS/localhost fixture/本地目录安装、升级、回退、校验和查询版本。
- `scripts/build-release.sh`：构建 Rust、Node 后端和 wasm32-wasip2 Zed 扩展，并调用打包器。默认离线，不访问外网。

## 构建

```bash
CARGO_TARGET_DIR=/home/zxg/.cache/wxml-targets/full-local-delivery \
WXML_BUILD_CACHE=/home/zxg/.cache/wxml-targets/full-local-delivery/build-cache \
  scripts/build-release.sh --output /tmp/opencode/final-release
```

- `--output` 必须是仓库外目录；仓库内路径会被拒绝。
- `--dry-run` 只打印计划和前置条件，不写文件。
- `--publish`、`--tag`、`--release`、`--upload` 在参数解析阶段直接失败。
- 默认 `cargo`/`npm` 离线；需要取依赖时必须显式 `--online`。
- `SOURCE_DATE_EPOCH=<unix-seconds>` 会固定 `builtAt` 和 tar mtime，可用于验证两次构建的归档 SHA-256 一致。
- `npm ci` 使用 `--ignore-scripts`，避免执行锁定依赖的安装生命周期脚本。

产物布局：

```text
<output>/wxml-lsp-<version>-<platform>/
  bin/wxml-lsp
  bin/wxml-lsp-install
  bin/wxml-type-backend/server.js
  bin/wxml-type-backend/package.json
  bin/wxml-type-backend/node_modules/...
  zed-extension/extension.wasm
  zed-extension/extension.toml
  zed-extension/languages/...
  manifest.json
  SHA256SUMS
<output>/wxml-lsp-<version>-<platform>.tar.gz
<output>/wxml-lsp-<version>-<platform>.tar.gz.sha256
<output>/index.json
<output>/SHA256SUMS
```

`manifest.json` 记录包版本、平台、协议版本、Node 最低版本、后端版本、构建时间、Rust/Node/Zed 产物路径及每个文件的 SHA-256、大小和显式权限（`0644`/`0755`）。`index.json` 记录归档名、归档 SHA-256、大小、协议和 Node 最低版本。

## 安装

```bash
unset WXML_LSP_BIN   # 确保安装器选择安装根；设置后会优先于安装状态

release=/tmp/opencode/final-release
wxml-lsp-install="$release/wxml-lsp-0.1.0-linux-x86_64/bin/wxml-lsp-install"

"$wxml-lsp-install" install \
  --root "$HOME/.local/share/wxml-lsp" \
  --from-dir "$release" \
  --version 0.1.0 \
  --offline

export WXML_LSP_BIN="$HOME/.local/share/wxml-lsp/current/bin/wxml-lsp"
```

`current` 是原子替换的 symlink，指向 `versions/<version>`；升级/回退后 `WXML_LSP_BIN` 路径不变。安装器在切换 `current` **之前**校验 staged 树哈希、执行 `bin/wxml-lsp --version` 并检查 Node 后端依赖；失败不会激活新版本。

### `WXML_LSP_BIN` 优先

- 已设置且有效：安装/升级/回退直接跳过，只打印该绝对路径；不会下载或覆盖。
- 已设置但无效（空、相对、含 `..`、不存在、非普通文件、不可执行、UNC 形状）：立即报错，绝不回退到安装器或下载。
- 未设置：才进入 `--root` 安装状态。

## 升级、回退与查询

```bash
wxml-lsp-install upgrade --root ... --source https://example.invalid/releases/ --enable-download
wxml-lsp-install versions --source https://example.invalid/releases/ --enable-download
wxml-lsp-install list --root ...
wxml-lsp-install current --root ...
wxml-lsp-install verify --root ... [--version 0.1.0]
wxml-lsp-install env --root ...
wxml-lsp-install rollback --root ... --to 0.1.0 [--offline]
wxml-lsp-install verify-package --from-dir /path/to/release --version 0.1.0 --offline
```

- `upgrade` 默认拒绝降级；降级必须显式 `rollback --to`。同版本但内容不同不会静默保留旧字节，会报 “already installed with different contents”。
- `rollback` 优先验证已安装版本；未安装时可从显式本地源或已验证缓存安装指定版本。
- `list/current/verify/env` 是只读命令：不会创建 root，不会 chmod 已有目录；不存在时返回“未安装”类错误。
- `verify` 先完整校验 manifest、SHA-256 和文件集合，再执行包内 `--version`；被篡改的二进制不会被执行。
- 路径含空格无需额外转义；所有路径参数走 argv，不经过 shell。`env` 输出使用 shell quoting。

## 缓存与离线

缓存位于 `<root>/cache/`，只保存**完整验证通过**的归档。离线安装/回退只使用已验证缓存或显式 `--from-dir` 本地源，不访问网络。缓存归档被截断或改动时会删除并在线重取；离线时明确失败。缓存没有自动回收策略，长期升级需手动清理 `<root>/cache/`。

## 安全边界

- 下载默认关闭；远端源必须显式 `--enable-download`。仅接受 HTTPS，或测试用 `localhost`/`127.0.0.1`/`[::1]` HTTP；拒绝凭据、query、fragment、`..` 和跨源 redirect。
- 项目/编辑器配置（`.zed/settings.json`、项目内 JSON）不能选择下载源；安装器不读取它们。
- 归档必须通过：归档 SHA-256、manifest 版本/平台/协议/Node 元数据、逐文件 SHA-256/大小/权限、`SHA256SUMS` 精确集合。
- 解包拒绝绝对路径、`..`、反斜杠、空组件、控制字符、超长路径、符号/硬链接、FIFO、设备文件、setuid/setgid/sticky、组/其他可写；解压前先流式预扫描 tar 头，限制扩展头和总解压预算，恶意 PAX/GNU 扩展头不会导致无界 RSS 或先写盘。
- 安装使用独占 OS 文件锁、`tmp/` 暂存、`rename` 进入 `versions/<version>`、最后原子替换 `current`；失败清理暂存，不留下半安装。只读命令不创建锁文件。
- 运行时 `--version`/`node --version` 有 10 秒超时；FIFO/非普通文件不会被当成 manifest、index 或哈希输入。

### 已知信任残留风险

当前校验链的信任根是 **TLS 连接 + index/归档 SHA-256**。它证明“下载内容与同一份 index 元数据一致”，不证明发布者身份：若配置的 HTTPS 源被攻破、CA 误签，或用户信任了恶意本地 bundle，攻击者可以同时提供 index 和归档。当前**没有**发布者签名、公钥 pin 或透明日志；这是明确接受的残留风险，后续可给 index 增加 detached 签名或 minisign。详见 [ADR-0007](../../docs/adr/0007-local-delivery-trust-model.md)。

## 排错

- **`WXML_LSP_BIN is set but invalid`**：取消设置或指向绝对、存在、可执行的文件；安装器不会偷偷改用已安装版本。
- **Node.js not found / older**：安装仍需 Node >= manifest 的 `node.minimumVersion` 才启用类型功能；设置 `WXML_NODE` 为绝对路径或安装新 Node。`install` 对 Node 缺失只告警，`verify`/`current` 会失败。
- **checksum/hash mismatch**：产物被改动或缓存损坏；在线重装，或重新用 `scripts/build-release.sh` 生成 release 后 `verify-package`。
- **path contains spaces**：直接传路径即可；不要手工拼 shell 字符串。
- **`current` 不可用**：`wxml-lsp-install rollback --to <previous>`；`list` 查看已安装版本。
- **Windows/macOS**：本包只在 Linux 验证；安装器的 `current` 原子 symlink 语义为 Unix 实现。

## Zed 接入

Zed 扩展仍按 `editors/zed` 的既有规则只读取 `WXML_LSP_BIN`。标准流程：

1. 构建/安装本 crate 的 release。
2. `export WXML_LSP_BIN="<root>/current/bin/wxml-lsp"`。
3. 在 Zed 命令面板执行 `zed: install dev extension` 选择 `editors/zed`，或使用 release 里的 `zed-extension` 目录。
4. 安装包自动探测同目录 `bin/wxml-type-backend/server.js`；也可显式设置 `WXML_TYPE_BACKEND`。

最终 release 必须用合并后的代码重新运行 `scripts/build-release.sh`；`manifest.json`/`index.json`/`SHA256SUMS` 绑定具体字节，旧 release 不能用于新 HEAD 的验收。
