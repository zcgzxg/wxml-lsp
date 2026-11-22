# ADR-0007: 本地交付与安装信任模型

**Date**: 2026-11-17

**Status**: accepted（安装实现已由 ADR-0009 收敛，类型资产安装由 ADR-0012 收敛；运行时信任边界仍有效）

**Deciders**: 项目维护者

**2026-11-22 校准**：本文早期版本记录过 `install`/`upgrade`/`rollback` 子命令、`--enable-download`/`--from-dir`、index/manifest 哈希链、Node 最低版本、release generator、`engine/lib/tsc` 安装和原子回滚。这些能力均已删除，分别由 [ADR-0009](0009-project-slimming.md)（交付 crate 与清单）和 [ADR-0012](0012-in-process-tsrs-session.md)（进程内引擎）替代，不再属于现行安装边界。以下按当前 `scripts/install-native.sh`、Zed 扩展和运行时发现重写；历史取舍见 ADR-0009 与 tag `pre-slim-20261120`。

## Context

项目只面向本人在 Linux 上使用 Zed，没有 Marketplace 发布、官方下载域名或服务器自动安装服务。上游交付实际只有两种可选方式：

1. 扩展在 wasm 内下载、解包并启动语言服务器；
2. 扩展只接受用户提供的绝对 `WXML_LSP_BIN`，由独立本地脚本取得并验证产物。

把下载器放进 wasm 会增加 Zed API 版本依赖、沙箱能力限制和项目文件触发任意可执行文件的风险；独立脚本可以使用宿主 OS 的 HTTPS、文件校验、权限和暂存目录能力。个人环境不需要多用户安装、版本目录、自动升级或发布者签名链。

## Decision

### 扩展只接受绝对 `WXML_LSP_BIN`

Zed 扩展**不下载、不解包、不安装**语言服务器。它只读取 `WXML_LSP_BIN`，并要求：

- 非空；
- 宿主 OS 的绝对路径（Linux/macOS 以 `/` 开头；Windows 为驱动器绝对路径或 UNC）；
- 不以路径分隔符结尾；
- 不含 `..` 路径组件；
- 指向存在且可执行的普通文件。

无效值直接报错，没有 `PATH`、worktree 或已安装布局探测回退。启动命令固定为 `wxml-lsp --stdio`。

### 安装脚本只有一个覆盖式安装动作

`scripts/install-native.sh` 只接受 `--root`、`--binary`、`--assets` 和 `--help`：

- 默认根为 `${WXML_LSP_HOME:-$HOME/.local/share/wxml-lsp}`；
- 根不能在仓库内，也不能是 `/` 或 `$HOME`；
- `--binary` 默认仓库 `target/release/wxml-lsp`，必须存在且可执行；
- `--assets` 默认仓库 `assets.lock.json`。

脚本没有 `install`/`upgrade`/`rollback` 子命令，不读取项目文件、workspace 配置或 `.zed/settings.json`，也没有发布、上传、tag 或 release 生成代码。

### 资产锁是唯一安装输入

`assets.lock.json`（`schemaVersion: 1`）登记一个或多个资产，每项包含 `name`、`packageRoot`、tarball `url`/`size`/`sha256` 和 `stripPrefix`。脚本只接受：

- `https://` 且不含空白的 URL；
- 正整数 tarball 大小；
- 64 位小写 SHA-256；
- 相对包路径 `packageRoot`，且不能占用 `bin/`；
- 恰好一层目录的 `stripPrefix`。

下载后先校验字节数与 SHA-256，全部通过才解包；解包结果不能为空。当前锁中只有 `miniprogram-api-typings@5.3.1`，安装布局固定为：

```
<root>/bin/wxml-lsp
<root>/typings/miniprogram-api-typings/
```

脚本最后确认 `typings/miniprogram-api-typings/index.d.ts` 存在并安装可执行二进制。旧布局中的 `<root>/engine/` 会被明确忽略；引擎和 TypeScript 标准库随进程内会话提供，不再安装 `engine/lib/tsc`。

### 运行时发现 fail closed

`wxml-lsp` 从 canonical 后的 `current_exe()` 出发，要求自身位于 `<package>/bin/wxml-lsp` 且同根存在 `typings/miniprogram-api-typings`；不读 `PATH`、当前工作目录或 `TSRS_BIN`/`TSRS_ASSET_ROOT`。typings 版本、文件数、字节数和摘要必须与代码内 pin（`5.3.1`）一致，否则在 LSP 握手前失败。

发现不到完整布局时唯一降级是 `NotInstalled` → `HostOnly`：静态 WXML 能力可用，类型查询返回 `Unavailable`。这不算“安装成功”，也不能据此声称类型行为已验证。

### 行为边界

重复运行脚本会覆盖同一布局；它不维护 `versions/<version>`、`current`、归档缓存或旧版本副本，也不提供原子升级、自动回滚或后台更新。失败后需要修复输入并重新运行；旧安装目录必须重新安装，不能原地复用。

## Residual Risk: TLS + Asset Lock, No Publisher Signature

信任根是 **HTTPS 传输 + 仓库内 `assets.lock.json` 的 URL/大小/SHA-256**。它不提供发布者身份证明：

- 能改动仓库、控制 HTTPS 源或攻破 CA 的攻击者，可以同时替换资产锁和字节，使二者仍然自洽；
- 没有发布者签名、公钥 pin、透明日志或证书固定；
- 安装器只能证明“字节与这份资产锁一致”，不能证明“来自可信发布者”。

本轮明确接受该风险，不实现 publisher signature。若未来需要发布者身份，应新增 ADR，并把签名校验、密钥分发和失败语义一起设计；不得只给单个资产加签名就宣称供应链完整。

## Alternatives Considered

| 方案 | 优点 | 不采用原因 |
| --- | --- | --- |
| 扩展 wasm 内下载并启动服务器 | 用户零 CLI 操作 | Zed API/沙箱限制；项目文件或环境可触发下载；难以复用宿主文件权限与校验工具 |
| 仅手工 `cargo build` + 导出 `WXML_LSP_BIN` | 实现最简单 | 没有固定 typings 布局和资产校验，运行时发现会退化为 `HostOnly` |
| 依赖系统包管理器/市场自动更新 | 用户界面一致 | 未发布、无官方源，且会把信任交给未审计渠道 |
| 引入完整签名发布链 | 强发布者身份 | 当前无发布密钥/域名；先显式记录 TLS + 资产锁的残留风险 |

## Consequences

- 用户必须运行安装脚本并自行把 `<root>/bin/wxml-lsp` 导出为 `WXML_LSP_BIN`；这是有意的安全边界。
- 脚本依赖 `curl`、`tar`、`python3`、`sha256sum`、`mktemp`、`install` 和 `realpath`；Linux 已验证，macOS/Windows 未验证。它不依赖 Node。
- 每次资产变更都必须同时更新 `assets.lock.json` 与代码内的 typings pin/摘要；任一处不匹配都会 fail closed。
- 没有自动更新、原子升级或回滚；需要旧版本时从 git tag 重建并重新安装。
- 未实现发布者签名是明确的遗留风险，不能把 TLS + SHA-256 描述成完整供应链安全。

## References

- [ADR-0009 项目瘦身](0009-project-slimming.md)
- [ADR-0012 进程内 tsrs 类型引擎会话](0012-in-process-tsrs-session.md)
- [安装脚本](../../scripts/install-native.sh)
- [资产锁](../../assets.lock.json)
- [项目 README](../../README.md)
- [ADR-0001 分层语言服务](0001-layered-language-service-architecture.md)
- [ADR-0003 版本化文档同步](0003-versioned-document-synchronization.md)
- [ADR-0006 显式小程序根](0006-explicit-mini-program-roots.md)
