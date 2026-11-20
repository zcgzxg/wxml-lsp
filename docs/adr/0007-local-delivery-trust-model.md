# ADR-0007: 本地交付与安装信任模型

**Date**: 2026-11-17

**Status**: accepted

**Deciders**: 项目维护者

**2026-11-17 后续**：[ADR-0008](0008-rust-tsgo-no-node-backend.md)（2026-11-20 已在 owner scope waiver 下最终关闭为 accepted，native 默认；P7 删除完成，Node removed）替代 Node 安装前置与相关包字段；本文的信任、离线、原子安装与回滚规则继续有效。

**2026-11-20 后续**：安装机制已由 [ADR-0009](0009-project-slimming.md) 替代。`wxml-delivery` 及其 manifest v2、`versions/<version>` + `current` 布局已删除；安装改由 `scripts/install-native.sh` 完成，布局为 `<root>/bin/wxml-lsp`、`<root>/engine/lib/tsc`、`<root>/typings/`。本文保留的运行时信任边界、引擎/类型资产 pin 和 fail-closed 原则仍然有效；原子升级、回滚、归档缓存和许可证生成不再是当前能力。

## Context

项目只面向本人在 Linux 上使用 Zed，当前没有 Marketplace 发布、官方下载域名或服务器自动安装服务。Zed 扩展存在两种可选的上游交付方式：

1. 扩展在 wasm 内下载、解包并启动语言服务器；
2. 扩展只接受用户提供的绝对 `WXML_LSP_BIN`，由独立本地安装器取得并验证产物。

直接把下载器放进 wasm 扩展会增加 Zed API 版本依赖、沙箱能力限制和项目文件触发任意可执行文件的风险；而一个独立 CLI 可以使用宿主 OS 的 HTTPS、文件锁、原子 rename、权限和进程检查能力。计划 §5.9 还要求默认下载关闭、可信 HTTPS/localhost、`WXML_LSP_BIN` 优先、离线/回退可用，并且远端发布步骤显式关闭。

## Decision

### 扩展边界

Zed 扩展**不下载、不解包、不安装**语言服务器。它只接受宿主 OS 的绝对 `WXML_LSP_BIN`，校验绝对路径、`..`、存在性和可执行性，然后启动 `wxml-lsp --stdio`。没有 worktree/PATH 回退；无效值直接报错。安装脚本通过打印稳定的 `<root>/bin/wxml-lsp` 路径把已验证产物交给扩展。

### 默认关闭远程获取

安装器只有显式 `install`/`upgrade`/`rollback` 命令才读写安装根；远端源还必须显式 `--enable-download`。本地 `--from-dir` 是显式离线源。项目文件、workspace 配置、`.zed/settings.json` 均不能指定待执行下载源；安装器不读取它们。

### 可信源

远端源仅接受 HTTPS，测试 fixture 才允许 `http://localhost`、`127.0.0.1` 或 `[::1]`。拒绝凭据、query、fragment、`..` 路径段和跨源 redirect；下载内容必须匹配 index 中的归档 SHA-256、大小和 manifest 的包版本、平台、协议、Node 最低版本，逐文件 SHA-256、大小和权限必须匹配。解包只允许普通文件和目录，拒绝链接、FIFO、设备文件、路径逃逸、超大扩展头和超预算数据。

### `WXML_LSP_BIN` 优先

环境变量一旦设置即优先于安装状态：

- 有效：安装/升级/回退不执行，不下载、不覆盖；用户显式配置的路径就是唯一运行来源。
- 无效：立即明确报错，绝不静默回退到已安装版本或网络。

这与扩展既有行为一致，也避免 Zed 在用户已经指定二进制时运行另一份未预期版本。

### 原子安装与只读命令

当前安装脚本先下载到临时目录并校验大小与 SHA-256，再解包到 `<root>` 的暂存位置，最后安装 `bin/wxml-lsp`、`engine/lib/tsc` 和 `typings/`。它不再维护 `versions/<version>`、`current`、原子升级或回滚；重复运行会覆盖同一布局。失败时不会留下不可用的 `bin/wxml-lsp`，但也不提供旧版本自动恢复。

`upgrade` 默认拒绝降级；降级只能通过显式 `rollback --to`。同版本不同内容不会被静默当作已安装，而是要求用户明确处理。

### 远端发布关闭

构建脚本只生成本地版本化目录、归档、checksum 和 index；没有上传、tag、远端 release 或 Marketplace 发布代码。`--publish`/`--tag`/`--release`/`--upload` 在参数解析阶段失败，默认 cargo/npm 离线。

## Residual Risk: TLS + Checksum, No Publisher Signature

当前校验链的信任根是 **TLS 连接 + index/归档 SHA-256 + manifest 文件哈希**。它不提供发布者身份证明：

- 控制配置的 HTTPS 源、攻破 CA、或者让用户信任恶意 `--from-dir` bundle 的攻击者，可以同时提供自洽的 index、manifest 和归档。
- 当前没有签名、公钥 pin、透明日志或证书固定。
- 因此安装器只能证明“内容与同一份元数据一致”，不能证明“来自可信发布者”。

本轮明确接受该残留风险，不实现 publisher signature。正式发布前应优先为 `index.json` 增加 detached 签名（如 ed25519/minisign），公钥编译进安装器或由可信用户配置 pin；在此之前，只在可信本地/CI 环境生成 release，并只从可信 HTTPS 入口安装。

## Alternatives Considered

| 方案 | 优点 | 不采用原因 |
| --- | --- | --- |
| 扩展 wasm 内下载并启动服务器 | 用户零 CLI 操作 | Zed API/沙箱限制；项目文件/环境触发下载面；难以实现宿主文件锁、原子 rename 和权限校验 |
| 仅手工 `cargo build` + `export WXML_LSP_BIN` | 实现最简单 | 无可复现 release、checksum、版本/回退/离线语义，不满足 AC-21 |
| 依赖系统包管理器/市场自动更新 | 用户界面一致 | 未发布、无官方源，且会把信任交给未审计渠道 |
| 引入完整签名发布链 | 强发布者身份 | 当前无发布密钥/域名；本轮先完成 TLS+checksum 的显式接受和文档化 |

## Consequences

- 用户必须显式运行安装/升级/回退，并自行把 `WXML_LSP_BIN` 导出给 Zed；这是有意的安全边界。
- 安装器是一个 shell 脚本，依赖 `curl`、`tar`、`python3` 和 Linux/Unix 语义；Windows/macOS 未验证。它不依赖 Node。
- 每次合并后必须重建 release；`assets.lock.json` 的资产版本、大小和 SHA-256 与具体字节绑定。
- 未实现发布者签名是明确的遗留风险，不能把 TLS+checksum 宣称为完整供应链安全。
- 未来若引入签名，需新增 ADR 并更新安装器、manifest/index schema 和测试；不得静默改变信任根。

## References

- [ADR-0009 项目瘦身](0009-project-slimming.md)
- [安装脚本](../../scripts/install-native.sh)
- [ADR-0001 分层语言服务](0001-layered-language-service-architecture.md)
- [ADR-0003 版本化文档同步](0003-versioned-document-synchronization.md)
- [ADR-0006 显式小程序根](0006-explicit-mini-program-roots.md)
