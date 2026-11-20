# ADR-0006: 显式小程序根与类型项目根

**Date**: 2026-11-16

**Status**: accepted（显式 `apps` 模式；省略 `apps` 时的默认行为由 [ADR-0010](0010-zero-config-lazy-discovery.md) 替代）

**Deciders**: 项目维护者

**2026-11-17 后续**：[ADR-0008](0008-rust-tsgo-no-node-backend.md)（2026-11-20 已在 owner scope waiver 下最终关闭为 accepted，native 默认；P7 删除完成，Node removed）调整后端资产路径配置；本文的小程序根与类型项目根边界继续有效。

**2026-11-20 后续**：新增 `apps[].tsconfig`（显式 tsconfig 文件，默认 `projectRoot/tsconfig.json`）与 `apps[].dependencies`（显式工作区包目录）。依赖源码进入类型快照，`package.json#exports` 转为 TypeScript `paths`；仍不读取 `node_modules`、不推断包图。依赖目录与 tsconfig 文件必须留在 `projectRoot` 内，每 app 最多 64 个依赖，`package.json` 最多 256 KiB / 512 个 exports 项。

**2026-11-20 后续（默认模式）**：[ADR-0010](0010-zero-config-lazy-discovery.md) 将**省略 `apps` 时**的默认行为改为零配置惰性发现：从当前 WXML 向上找最近 `app.json` 和独立最近的 `tsconfig.json`，并自动挂载工作区包。本文的显式 `apps` 配置仍是覆盖模式，其路径校验、隔离和“不扫描子目录”约束在该模式下继续有效；下文“取消扫描”仅指显式模式，不再约束默认模式。

## Context

工作区根、小程序 `app.json` 目录和类型项目 `tsconfig.json` 目录可能分层，同一仓库也可能包含多个小程序。自动扫描子 `app.json`、向上寻找最近 tsconfig，以及按生成目录名称忽略候选，会让项目归属依赖目录布局和扫描规则，难以解释或复现。需要由用户明确 app 与类型项目边界，同时保留工作区根就是单个小程序的零配置用法。本记录补充/替代 [ADR-0001](0001-layered-language-service-architecture.md) 与 [ADR-0003](0003-versioned-document-synchronization.md) 中的根发现、配置选择及生效时机，不废弃它们的历史背景、分层或同步原则。

## Decision

### 初始化契约与路径

采用 `initialize.initializationOptions`：

```json
{
  "apps": [
    { "root": "apps/miniprogram/src", "projectRoot": "apps/miniprogram" },
    { "root": "apps/sub-merchant/src", "projectRoot": "apps/sub-merchant" }
  ]
}
```

每项 `root: string` 必填，指向 `app.json` 所在目录；`projectRoot?: string` 是类型后端项目目录，默认等于 `root`；`tsconfig?: string` 显式指定配置，默认 `projectRoot/tsconfig.json`；`dependencies?: string[]` 列出加入类型快照的工作区包。这些路径都相对 **LSP workspace root**：`rootUri`，或首个 `workspaceFolders` 根；不相对进程当前目录或 `.zed/`。`projectRoot` 是类型后端的项目目录基准。Rust 使用显式 tsconfig 与相对 app 路径构造项目事实，向类型后端提供快照和有效编译选项。

本文以下 Decision 描述**显式 `apps` 模式**。省略 `initializationOptions`、使用 `null`、`{}` 或 `{ "apps": [] }` 时，不再等价于 `{ "apps": [{ "root": "." }] }`，而是进入 [ADR-0010](0010-zero-config-lazy-discovery.md) 的零配置惰性发现模式；只有显式给出 `apps` 时，才应用本文的根边界与“不扫描子目录”规则。

初始化校验必须满足：

- `root`、`projectRoot`、显式 `tsconfig` 和 dependencies 均为已存在的相对路径，禁止绝对路径及 `..` 路径分量。
- canonical 后满足 `workspace ⊇ projectRoot ⊇ root`，且显式 `tsconfig` 与 dependencies 留在 `projectRoot` 内，不得通过 symlink 越出边界。
- canonical app root 不得重复；最多 64 个 app、每 app 最多 64 个 dependencies，每个路径最多 4096 UTF-8 bytes。
- 非法配置使 `initialize` 返回 `-32602`，不截取有效子集或静默 fallback。合法目录内的 `app.json` 缺失或内容错误，仍由现有 project issues 报告，不混为初始化配置错误。

显式模式下取消扫描子 `app.json` 和向上寻找最近 tsconfig；生成目录不再按名称自动忽略，也不会自动发现为 app。需要使用的源码或生成小程序目录必须由配置选中。

### 路由、隔离与共享文件

文档 URI 按最长 app 目录匹配，稳定 `projectId` 隔离各 app 的状态。后端 `fileId`/文件相对路径基于所选 app 的 `projectRoot`，app 资源解析仍以 `root` 为准。app 目录外、`projectRoot` 内的共享文件保留 active owner，或选择唯一 owner；多个 owner 且没有 active owner 时不猜测。

这是同一 LSP workspace root 内的显式多 app 配置，不承诺完整多 workspace、跨 app 依赖图或共享文件的自动依赖归属推断。

### Zed 与生效时机

Zed 在工作区 `.zed/settings.json` 的 `lsp.wxml-lsp.initialization_options` 下配置上述内层对象。[完整可复制示例](../../editors/zed/README.md#mini-program-roots-initialization-options) 展示仓库内两个小程序分别使用独立源码目录和 TypeScript 项目目录的布局；示例不作为真实 Zed 验收证据。

[JSON Schema](../../editors/zed/initialization-options.schema.json) 仅描述 `initialization_options` 内层值，不能替换 Zed 整个 settings schema；文件系统存在性、canonical 包含、symlink 与路径字节上限仍由服务端校验。

配置只在启动时消费，修改后重启 LSP；没有根热更新。[Zed 官方配置说明](https://zed.dev/docs/configuring-languages#possible-configuration-options) 同样将初始化选项定义为启动时发送、修改后重启服务才能重新应用。

## Alternatives Considered

以下替代方案评估仅针对显式 `apps` 模式；省略 `apps` 的默认发现方式已在 [ADR-0010](0010-zero-config-lazy-discovery.md) 重新评估。

| 方案 | 优点 | 代价及不采用原因 |
| --- | --- | --- |
| 显式模式下也扫描嵌套 `app.json`，向上选最近 tsconfig | 嵌套项目初次打开较省配置 | 目录预算、生成物和近邻配置影响归属；不能明确表达 app 与类型项目边界 |
| 只允许 workspace 根作为 app 与类型项目根 | 契约最小，零配置清晰 | 无法在同一 workspace 表达多 app 或分层 tsconfig；当时保留为默认值，现由 ADR-0010 的 lazy 模式替代 |
| 从小程序工程配置或生成目录名称推断 | 可复用部分构建工具约定 | 工具约定不统一，仍会引入隐式发现与目录过滤规则 |
| 运行时热更新根配置 | 编辑设置后无需重启 | 涉及 owner、overlay、项目身份、后端缓存及旧结果迁移；本次以重启建立清晰边界 |

## Consequences

### Positive

- 根选择可配置、可复现，不再依赖扫描顺序、目录深度或生成目录名称。
- app 与类型项目可分层，最长目录匹配和稳定项目身份明确隔离边界。
- 配置错误在初始化时暴露，路径包含校验限制工作区与项目边界。

### Negative

- 嵌套 app 与分层 tsconfig 需要显式配置；移动目录时需同步修改设置并重启 LSP。
- 多 owner 共享文件可能保持歧义，需要已有 active owner；不为跨 app 依赖自动建图。
- 配置 Schema 无法代替文件系统校验；有效目录下的 `app.json` 问题仍需检查 project issues。

## Migration

1. 截至 [ADR-0010](0010-zero-config-lazy-discovery.md)，省略 `apps` 已默认进入 lazy mode；只有需要固定边界时才使用本文的显式模式。
2. 从 lazy mode 切到显式模式时，把每个 `app.json` 目录列入 `apps[].root`，并为需要独立类型配置的 app 填写 `projectRoot` / `tsconfig`。
3. 需要使用生成目录时显式配置它，不再依赖自动发现或按名称忽略。所有目录必须存在并满足 canonical 包含关系；重复 app root 或越界配置应修正，不能依赖 fallback。
4. 保存 Zed 设置后重启 LSP。配置失败检查 `initialize -32602`；初始化配置有效但 app 清单缺失或错误时检查既有 project issues。

## Validation

验收应覆盖四种默认输入、分层/双 app 配置、最长目录匹配与稳定隔离、共享文件 active/唯一/歧义归属、非法路径与 symlink/重复/数量/字节上限，以及错误配置不 fallback、修改后重启生效。文档和 Schema 示例检查不等同这些服务行为测试，更不等同真实 Zed 多 app 验收；本记录不新增已执行的运行证据。

## References

- [项目快速开始](../../README.md#在-zed-中使用)
- [Zed 配置指南](../../editors/zed/README.md#mini-program-roots-initialization-options)
- [C1 类型桥：请求与项目路径边界](../reference/type-bridge-c1.md#61-请求rust--node)
- [ADR-0001：分层语言服务与模块依赖边界](0001-layered-language-service-architecture.md)
- [ADR-0003：版本化文档同步与 Zed 接入](0003-versioned-document-synchronization.md)
