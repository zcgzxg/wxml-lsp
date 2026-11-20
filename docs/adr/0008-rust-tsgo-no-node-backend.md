# ADR-0008: Rust + tsgo 类型后端，移除 Node 依赖

**Date**: 2026-11-17（2026-11-20 owner scope waiver 与最终验收口径）
**Status**: accepted (native default; Node removed)
**P7 删除状态**: removed（2026-11-20）；Node/旧 TS/v1 已删除并通过删除后验收
**Deciders**: 项目维护者；planner、architect、Rust 测试及交付盘点参与设计

本文记录已采用的迁移方向。2026-11-20 owner 明确选择 **C-lite（owner-scoped functional waiver）**，原话：**“不需要严格按照原来的功能写测试。功能合理即可”**。据此本 ADR 最终关闭为 `accepted`，验收口径固定为 **native engine + native gates + F3 + 合理行为测试**，状态附注 **native default; Node removed**。`accepted` 接受的是架构方向、默认后端和功能验收口径，删除已落地；仍不表示 G7 的逐条 parity。

本 ADR 明确**不声称旧 TS/Node 测试逐条等价**，也不把 `reviewed` / `waived` / `retired-with-decision` 当作 `green` 或“已迁移”。功能验收以 native engine、native gates、F3 交付合同和合理行为测试为准；旧 TS/Node 的逐条对照保留为历史证据和残余风险登记。台账终态与 waiver 依据见 `docs/evidence/2026-11-20-tsgo-p7-closeout-ledger.md`、`docs/evidence/2026-11-20-tsgo-p7-scope-decision.md` 和 `docs/evidence/2026-11-20-tsgo-p7-ledger-waiver-policy.md`。

2026-11-20 删除已完成并写入 `removed`：`backend_node`、`backends/typescript`、12 个 delete gate、v1 写路径和 legacy Node env 已删除；默认 `--stdio` 为 native，只有 `NotInstalled` 降级 `HostOnly`，损坏安装 fail closed，已无 `--backend node`。删除后验收：fmt/check/clippy/rustdoc 通过；native gates **40/40**（23 engine + 3 edit + 9 stdio + 1 perf + 1 stability + 3 real_matrix；`WXML_STABILITY_ALLOW_DIRTY=1` 仅用于 dirty 工作树本地证据）；release build/install/Zed smoke **5/5**，无 Node/server.js；F3 **16/0/0**；ledger **109/0/130/29/71**（reviewed/waived/retired 非 green）；matrix **22 exact/11 blocked**。workspace 为 2587 passed/1 failed/73 ignored，唯一失败是既有 `type_assets::fake_full_resend_ten_updates_stays_within_budget` debug p95 221–233ms >200ms，release 10/10 通过；未放宽预算。

## Context

现有 Rust LSP 自行启动 Node 后端，后端自行创建 TypeScript LanguageService，并未复用 Zed 的 JavaScript/TypeScript 服务。小程序上下文、原位 shadow、Behavior、属性、事件、源码映射和安全符号规则已经形成项目能力，迁移必须保留其可观察行为及正在修改的修复。用户决定采用 Rust + tsgo，去除产品运行、构建、当前测试及发布流程对 Node/npm 的要求，并将后端 TypeScript 测试迁入 Rust。2026-11-20 owner 将“逐条保留可观察行为”的验收口径收窄为**功能合理即可**（见 Decision 的 C-lite 裁决），不再要求旧测试逐条 parity。

原生 TypeScript 提供进程协议和类型查询，但不能将旧 LanguageService 的 API 与原生 API 当作相同接口。当前源码调研只支持将原生 API 作为候选；尚未通过本项目完整 shadow、查询呈现、读写边界及恢复验证。

## Decision

采用 **Rust 语义适配器 + 独立的原生 tsgo 子进程**。Rust 拥有 WXML/小程序语义、项目快照、shadow 生成、源码映射、查询编排和 C0 DTO；tsgo 拥有 JavaScript/TypeScript 解析、绑定、类型检查及类型关系计算。不自行实现 TypeScript 类型系统，不保留 Node 适配层，也不假设共享编辑器的类型服务。

新增 `wxml-backend-tsgo` crate，内部划分 native、script、shadow、query 模块；`wxml-lsp` 仅提供 ProjectHost 输入适配。依赖保持单向，底层后端不依赖 LSP，现有 `Backend` 为对上边界。常态每个 Rust LSP 最多管理一个原生子进程，多项目通过有界 session/snapshot 管理。

首选接口是同一个原生进程的 `--api --async` 双向 stdio。冻结版本后实现 Rust JSON-RPC 客户端、受限文件系统回调、必要的二进制 AST 解码、snapshot/lease/handle 生命周期与类型查询。保留现有 `wxml_types::Backend` 对上层的边界。

P1-A 实测确认：原生 API 没有旧 LanguageService 的 QuickInfo、DefinitionAtPosition、Rename、SignatureHelp 等直接方法。hover 由类型字符串、文档注释与 JSDoc 组合；definition 由 `getReferencedSymbolsForNode` 加 AST 解码得到；rename 仅允许 Rust 侧基于完整符号/引用证据的保守编辑，证据不完整时拒绝，不得伪造或降级为不安全编辑。补全的详情/自动导入需 `prepareAutoImports`，其缺失部分必须在 F1 明确为可见降级还是阻塞项。若组合规则不能保持现有可观察语义，先阻塞迁移并修订本 ADR；只有验证同进程 LSP + API 连接确实共享快照且满足同一文件访问策略，才可作为明确批准的替代实现。不得默认改成两个独立类型图。

原生 node handle 不携带快照身份，跨 `updateSnapshot` 可能静默解析到新 AST 的同序号节点。Rust 必须把每个 handle 绑定 `(snapshot id, file path, content hash/node id)`，跨 epoch 一律拒绝，不能缓存到下一快照。未保存内容更新必须使用同一次 `updateSnapshot` 的 `closeFiles` + `openFiles`；仅 layer/full/notification 不会刷新已打开文件。原生 API 暴露 `emit` 等写方法且实测可写宿主磁盘，客户端只允许白名单查询方法，启动时设置 `writeFile:noop`、`removeFile:noop`，读回调 fail-closed，永不启用 `--runExternalCode`；`noEmit`/no-op 不构成 OS 沙箱。2026-11-17 P2 追加探针确认 `appendFile`/`chtimes` 没有可注册的 deny 回调，源码中始终走 base OS FS；因此写阻断不能只依赖回调，必须同时依赖“写方法不可达”的白名单和运行目录最小权限。该残余风险为 B-02 PARTIAL；维护者已于 2026-11-18 显式接受“方法白名单 + 运行目录最小权限 + 禁用 emit/external code”的补偿控制，P3 原生骨架可据此启动，但不得声称已实现回调级 deny。

### B-02：`appendFile`/`chtimes` 是设计边界还是临时缺陷

维护者于 2026-11-18 询问：这两项无法注册回调，是 tsgo 临时未实现的功能/缺陷，还是有意设计？固定版本 7.1.0-beta 的结论是：**当前 API 的有意能力边界**，不是本项目误用或偶发崩溃。`callbackfs.go` 只注册 `readFile`、`fileExists`、`directoryExists`、`getAccessibleEntries`、`realpath`、`stat`、`writeFile`、`removeFile` 八个回调；`AppendFile` 与 `Chtimes` 始终委托 base OS FS，源码注释明确写着 *“always delegates to base (no callback support)”*。实测 `appendFile`、`appendFile:error`、`appendFile:noop`、`chtimes` 及对应变体均在启动阶段以 `panic: unknown callback name` 失败；证据见 `docs/evidence/2026-11-17-tsgo-native-api-spike.md` §15（第 1072–1083 行）及 `docs/reference/tsgo-native-contract.md` 第 788 行。

因此迁移不以“等待上游补回调”为前提，也不声称实现回调级写沙箱。采用方案 1 的补偿控制：方法白名单使写/append/chtimes 路径不可达，引擎以最小权限运行，禁止 emit/external code；若任何白名单方法实际触发 `AppendFile`/`Chtimes`，按 F0 阻塞缺陷处理，而不是当作可接受行为。该结论只对固定的 7.1.0-beta 成立；升级必须重新探测回调集合，不能把当前行为视为永久 API 承诺。

采用精确的原生发行版本、源码提交、API/AST schema 及平台资产摘要。P1-A 实测稳定版 `7.0.2` 不具备 `createSnapshot`/租约/`createSourceFile`，AST protocol v5（header 44）与候选线不兼容，不能作为本架构基础；当前唯一满足快照、虚拟 FS、查询与 AST 要求的候选是 `7.1.0-beta`（gitHead `3e9236361fa97a9e6c685f4833ac457ae2d8c50a`，AST protocol v9/header 64）。维护者已于 2026-11-17 有条件接受 beta 作为唯一候选基线并固定 integrity。该段是 2026-11-17 的 proposed 条件；2026-11-20 已按下方 waiver 升为 accepted，默认后端已为 native，删除状态见“最终验收范围（C-lite，accepted）”。`7.0.2` 不作为回退候选。不使用 `main`、`next` 或浮动版本。版本、协议与类型资产更新作为一个经过测试的变更交付；升级必须重跑 F0/F1 合同测试与 AST schema 探针。旧 Node 安装可在无 Node 条件下由受限旧 schema 读取器识别并迁入原生包；验证成功前保持旧 current 不变，原生版本之间支持校验后的回退，不自动回退激活 Node 包。

现有 TS 测试按源测试、展开用例、关键断言逐项登记并迁入 Rust。迁移期允许在隔离的历史基线环境运行旧 Node 后端生成 oracle；最终构建、测试、安装包及运行过程不得依赖它。删除旧实现前必须完成交付、恢复、安全、性能及无 Node 验收；删除后再验一次。2026-11-20 owner waiver 后，**逐条等价不再是关闭条件**：台账以 **109 green / 0 planned / 130 reviewed / 29 waived / 71 retired**（339 总数）收口，`green` 仅表示 `parity=exact` 且有真实 Rust 证据，`reviewed` / `waived` / `retired-with-decision` 均非 green。删除后仍须重跑 native 全量门禁与交付验收，但不再要求旧 TS 断言逐条重现。

### 最终验收范围（C-lite，accepted）

| 项 | 裁决 | 当前状态/证据 |
| --- | --- | --- |
| 旧 TS/Node 逐条 parity | 不再要求 | 台账 `109 green / 0 planned / 130 reviewed / 29 waived / 71 retired`；`exact=109`、`never_green=38` |
| 功能验收 | **native engine + native gates + F3 + 合理行为测试** | native gates **40/40**（23 engine + 3 edit + 9 stdio + 1 perf + 1 stability + 3 real_matrix，opt-in required job）；F3 **16 closed / 0 partial / 0 blocked**；matrix **22 exact / 11 blocked**（33 in-place） |
| 最终验收清单 | workspace、native gates、release/install、Zed smoke、无 Node 进程、ADR/README 口径 | `docs/evidence/2026-11-20-tsgo-p7-final-docs.md`；**已验收**，结果与残余风险已填写 |
| 默认后端 | native | `--stdio` 默认 native；仅 `EngineDiscoveryError::NotInstalled` 降级 `HostOnly`，损坏安装 fail closed |
| 删除范围（P7） | Node 运行时、旧 TS 实现/测试、12 个 delete gate、v1 写路径、legacy Node env | 冻结范围见 `docs/evidence/2026-11-20-tsgo-p7-deletion-surface.md` 与 `docs/evidence/2026-11-20-tsgo-p7-predelete-freeze.md`；删除演练见 `docs/evidence/2026-11-20-tsgo-p7-delete-dryrun.md` |
| retarget 状态 | 11 个 retarget gate 全部 native-only；不标记 `old_test_removed` | 9 个 stdio 在 `b0f6cc1`，perf/stability 在 `3e24c44`；5 项 stability BLOCKED 见 `docs/evidence/2026-11-20-tsgo-p7-retarget-native-tests.md` |
| 显式保留 | 业务 `.ts/.js/.d.ts` 与 JSDoc、WXS/WXML 语义、grammar、fixtures、typings/`assets.lock.json`、schema v1 只读识别 | 去 Node 不等于删除 TypeScript 语言支持；`verify_installed`、`read_legacy_manifest`、`build_native_package`、`list/current/verify/env` 只读边界保留 |
| 已完成 | 实际删除、12 个 delete gate、11 个 retarget、删除后全量重验 | 2026-11-20 删除提交与验收证据；G7 逐条 parity 不声称 |
| `removed` 写入条件 | 已满足：删除提交落地 + 删除后验收清单通过 | 2026-11-20 写入 `removed`；残余风险见最终验收清单 |

**不声称**：本表不是 parity 报告，`reviewed`/`waived`/`retired` 不是 green；F3 16/0/0 只覆盖交付合同。native gates 是 opt-in required job，不是默认 CI 绿，也不是 `mandatory_green`。平台证据仍只有 Linux x86_64；G7 逐条 parity 未覆盖。

## Alternatives Considered

### 保留 TS 适配器，通过 Node 调用 tsgo

- **Pros**: 初期改动较少，可直接使用官方 TS 客户端。
- **Cons**: 同时需要 Node 和原生引擎，小程序语义及测试仍留在 TS。
- **Why not**: 不满足本次去 Node 和测试迁 Rust 的明确目标。

### Rust 只转发标准 LSP 请求

- **Pros**: 常规 hover、definition、completion 的接入较直观。
- **Cons**: 不足以直接表达现有 AST/checker、属性赋值关系、Behavior 合成及安全符号规则；还存在虚拟文件和配置归属问题。
- **Why not**: 在完整等价性证据缺失时，不能用普通 TS 文件验证代替小程序语义验收。

### Rust 直接链接或 fork Go 类型引擎

- **Pros**: 理论上可控制更深层接口或形成更紧的打包形式。
- **Cons**: 增加 FFI、ABI、Go runtime、跨平台链接和上游维护成本。
- **Why not**: 当前没有此需求；先使用进程边界隔离故障并复用上游原生发行物。未来若能力验证确实要求上游改动，单独决策。

### 用 Rust 重写 TypeScript 类型引擎

- **Pros**: 单语言实现。
- **Cons**: 需要重做跨文件解析、绑定、推断、JSDoc、泛型、条件类型和 TypeScript 兼容行为。
- **Why not**: 与迁移现有能力的目标不相称，回归面不可接受。

### 共享 Zed 已运行的 JavaScript/TypeScript 服务

- **Pros**: 可能降低重复项目状态开销。
- **Cons**: 当前项目没有共享会话协议；编辑器生命周期、未保存文档、shadow 所有权和快照隔离均需额外协作。
- **Why not**: 不把编辑器未提供的跨服务协作当作后端前提；本方案由 Rust 管理自己的原生进程。

## Consequences

### Positive

- 产品后端实现统一到 Rust，使用上游原生类型引擎；最终无 Node/npm 前置条件。
- 业务语义、缓存失效、路径策略和测试归属清晰，保留可替换 Backend 边界。
- 发布包可固定原生引擎和类型资产，沿用摘要校验、离线安装及原子升级。

### Negative

- 需要迁移现有 TS 语义实现与测试，不能仅改一个依赖或启动命令。
- Rust 需要维护原生协议/AST schema 的适配、查询呈现及远程对象生命周期。
- 多平台资产和原生协议兼容性成为发布职责；原生检查更快不等于端到端请求必然更快。

### Risks

- **查询保真度**：先验证文档、重载、alias、引用、rename 及范围映射；缺项阻塞并修订设计，不删除断言。
- **坐标与 AST**：逐字段区分 UTF-8 字节、UTF-16 code unit、字符串池偏移；校验大小、索引、版本、句柄和生命周期，加入性质测试及畸形输入测试。
- **文件访问**：未配置的回调可能使用 OS 文件系统；`noEmit` 和禁止 writeFile 不等于 OS 沙箱。验证所有可达读入口、配置/包/类型解析、符号链接及写入口，只开放查询方法，不启用 external code。任何超出现有权限的行为都阻塞切换。
- **旧响应污染**：请求绑定项目代次、文档版本、依赖指纹和进程 session；重启、失效、取消后不能发布旧 diagnostics 或 WorkspaceEdit。
- **未提交改动丢失**：冻结 HEAD + 工作区指纹及行为增量；旧目录内未提交修复未迁移并通过测试前禁止删除。

## Validation

详细门禁以实施计划为准，至少包含：

1. 固定版本的单进程能力试验：只读 VFS、原位 shadow、查询结果、AST/句柄及生命周期完整通过。
2. 每个旧测试和关键断言都有 Rust 落点、显式 `reviewed`/`waived` 登记或 `retired-with-decision` 裁决；差分比较不得抹去类型、文档、范围、诊断、编辑目标或安全标志。2026-11-20 owner waiver 后，本条不再要求逐条 exact parity，但非 green 状态必须逐 case 有 owner/date/scope/evidence/replacement 或残余风险说明。
3. 原有 Rust 全量测试、真实原生 E2E、状态恢复、UTF/CRLF、未保存跨文件更新及保守 rename 规则通过；缺少原生引擎必须使原生门禁失败，不能静默 skip。
4. 在 Node 不可用且进程调用受监测的环境，构建、测试、发布、安装、升级、回滚和 Linux Zed 冒烟通过。
5. 新 manifest/index schema、固定原生资产、类型声明许可证和摘要验证通过；离线模式不能隐式下载。
6. 旧 Node 默认入口、实现和当前测试删除后重新执行完整门禁；保留业务 TS/JS、`.d.ts`、fixture、字节规则和历史证据。
7. 删除完成后的最终验收按 `docs/evidence/2026-11-20-tsgo-p7-final-docs.md` 执行：workspace、native gates 40/40、release/install、Zed smoke、无 Node 进程和 ADR/README 口径已全部通过，并已写入 `removed`。

截至 2026-11-17，P1-A 已实测原生进程、虚拟快照、类型/符号/文档/补全/引用、AST 关键字段、UTF-16 坐标和进程控制；稳定版 7.0.2 已排除，7.1.0-beta 为唯一候选。完整 shadow、Behavior/props/loops/guards、保守 rename、完整 AST 解码、无 Node 安装器和 clean-env 验收仍**待执行**。现有原生实验和旧后端测试记录只能作为阶段性证据。

截至 2026-11-20 的删除后验证状态（**accepted；native default；Node removed**）：

- **默认入口**：`5020ae7` 已把默认 `--stdio` 切到 native。验证安装存在时使用固定引擎；只有 `EngineDiscoveryError::NotInstalled` 降级 `HostOnly`，manifest/引擎/类型资产缺失、损坏或不匹配一律在 LSP handshake 前 fail closed。`--backend native` 仍是严格选择器，`--backend node` 已随 P7 删除；显式 Node 路径不再存在。
- **门禁**：23 个旧 Node gate 的 native replacement 已全部 `gate_green`、`required_not_migrated` 为 0；`scripts/native-gates.sh` 的最终 native job 为 **40/40**（23 engine + 3 edit + 9 stdio + 1 perf + 1 stability + 3 real_matrix）。它们仍是 opt-in required job 结果，不是默认 CI 绿，也不是 `mandatory_green`；23 个旧 gate 在源码中仍为 `#[ignore]`，12 个 delete / 11 个 retarget 的 P7 动作已冻结。
- **编辑安全与测试迁移**：3 个 native edit gate（`G-P5-E2E-22/23` 等）已实现；11 个 retarget gate 已全部 native-only：9 个 stdio 在 `b0f6cc1` 完成，`performance_bench`/`q2_stability` 在 `3e24c44` 改为 native-only 薄壳，旧 `#[ignore]` 测试名与文件坐标保留，不标记 `old_test_removed`。台账在 `2ce4626` 按 C-lite 收口为 **109 `green` / 0 `planned` / 130 `reviewed` / 29 `waived` / 71 `retired-with-decision`**（339 总数），其中 `exact=109`、`blocked=31`、`never_green=38`。`reviewed`/`waived`/`retired` 均非 green，`gate_green` 与 case `green` 仍是两个状态轴。
- **交付**：F3 状态为 16 closed / 0 partial / 0 blocked。F3-06 已由 `fb6047e` 的真实 N1 `0.1.0` / N2 `0.2.0` 两次独立构建、安装/升级/回退/中断窗口关闭；F3-09 已由真实 v1 legacy 包（`e4fed4a`）关闭；F3-14 已由 `5020ae7` 默认 `--stdio` 重新构建的真实 v2 包 + 真实 Zed 默认路径冒烟关闭（Xvfb，16/16；无 wrapper、无 Node）。F3 合同收口不等于迁移完成。
- **删除与功能覆盖**：P7-0 预删除冻结已在 `8bda306` 落盘并创建 `p7-pre-delete` tag；P7-1 台账按 owner waiver 归零；删除范围覆盖 `backend_node` 8 文件/102 单元测试、旧 TS 99 tracked 文件 + 34 个 dist、12 个 delete gate、15 个 v1 写路径和 5 个 legacy env，并保留业务 TS/JS/.d.ts、WXS/WXML、fixtures、grammar、typings/`assets.lock.json`、schema v1 只读识别与 3 个交付边界。真实 matrix 为 **22 exact / 11 blocked**（33 in-place；另有 4 个顶层 exact / 6 个 blocked），且 `4035bb7` 已把 scenario source 归档为 fixture，删除 `backends/typescript` 后默认 matrix 不依赖旧树。删除已完成：12 个 delete gate、旧 TS 树、`backend_node`、v1 写路径和 legacy Node env 均已移除；11 个 retarget gate 保留 native-only。

因此本 ADR 于 2026-11-20 在 owner scope waiver 下关闭为 **accepted**，验收口径固定为 **native engine + native gates + F3 + 合理行为测试**，并写入 **Node removed / P7 removed**。删除后验收已完成，残余风险（Windows/macOS 实机、大项目语料、G7 逐条 parity、debug perf 已知 flake）保留在最终验收清单与 waiver 证据中；不声称旧 TS 逐条 parity。

## 与既有 ADR 的关系

- ADR-0001：仅替代 Node 首选运行时/进程模型，保留 crate 分层和可替换后端。
- ADR-0002：仅替代类型桥实现归属；保留原位 shadow、模板语义及映射原则。
- ADR-0003：替代 Rust–Node NDJSON 协议；保留版本化同步、快照一致性和发布屏障。
- ADR-0006：更新后端资产路径配置说明，保留小程序根与类型项目根的区分。
- ADR-0007：替代 Node 安装前置条件和对应包字段；保留信任、离线、原子安装与回滚规则。
- ADR-0004/0005 继续有效。原 ADR 局部增加本记录链接，不整体标记失效，不改写历史证据。

## References

- 本轮用户明确选择 Rust + tsgo、去 Node、测试迁 Rust、通过 `/orchestrate` 并行迁移。
- `docs/adr/0001-*.md`、`0002-*.md`、`0003-*.md`、`0004-*.md`、`0005-*.md`、`0006-*.md`、`0007-*.md`。
- `docs/evidence/2026-11-15-native-typescript-service.md`：普通虚拟 TS 文件历史试验，不是完整小程序验收。
- `docs/evidence/2026-11-17-tsgo-native-api-spike.md`：7.1.0-beta Linux x64 无 Node 实测，包含虚拟快照、查询、AST、UTF、handle、写路径与 7.0.2 不兼容证据。
- `docs/evidence/2026-11-17-tsgo-migration-baseline.md`、`docs/evidence/2026-11-17-tsgo-delivery-spike.md`：P0 测试基线及 P1-C 交付盘点。
- `docs/evidence/2026-11-20-tsgo-p6-default-switch-plan.md`：P6-8 默认选择器、HostOnly、P7 删除顺序与 blocker 设计（只读）。
- `docs/evidence/2026-11-20-tsgo-p6-edit-gates.md`：3 个 native edit gate 与 required job 运行记录。
- `docs/evidence/2026-11-20-tsgo-f3-06-real-n2.md`：真实 N1 `0.1.0` / N2 `0.2.0` 双构建、安装/升级/回退/中断窗口与 F3-06 关闭依据。
- `docs/evidence/2026-11-20-tsgo-f3-14-zed-smoke.md`：默认 `--stdio` 真实包的 Zed UI 冒烟；F3-14 关闭依据。
- `docs/evidence/2026-11-20-tsgo-p7-gate-removal-readiness.md`：`old_test_removed` fail-closed 语义与 P7 删除就绪矩阵。
- `docs/evidence/2026-11-20-tsgo-ledger-backfill-wave9.md`：台账 wave-9 回填历史快照。
- `docs/evidence/2026-11-20-tsgo-p7-scope-decision.md`：2026-11-20 owner C-lite 裁决、128 green 上限与方案 A/B/C 对照。
- `docs/evidence/2026-11-20-tsgo-p7-closeout-ledger.md`：台账终态 **109 green / 0 planned / 130 reviewed / 29 waived / 71 retired** 与 33 个退休、29 个 waiver 的逐组依据。
- `docs/evidence/2026-11-20-tsgo-p7-ledger-waiver-policy.md`：`reviewed`/`waived`/`retired-with-decision` 的状态语义与不伪造 green 规则。
- `docs/evidence/2026-11-20-tsgo-p7-deletion-surface.md`：P7 删除面（Node/旧 TS/12 delete gate/v1 写路径）与最小安全顺序。
- `docs/evidence/2026-11-20-tsgo-p7-delete-dryrun.md`：12 个 delete gate 删除演练与 Frozen oracle 前置。
- `docs/evidence/2026-11-20-tsgo-p7-retarget-native-tests.md`：9 个 stdio + perf/stability 共 11 个 retarget gate 的 native-only 薄壳、预算与 5 项 BLOCKED。
- `docs/evidence/2026-11-20-tsgo-real-matrix-archived-source.md`：`scenarios.ts` 归档 fixture，`backends/typescript` 缺失时 matrix 仍通过。
- `docs/evidence/2026-11-20-tsgo-p7-final-docs.md`：删除后最终验收清单（workspace/native gates/release/Zed/no-Node/文档口径），已验收。
- `docs/evidence/2026-11-20-tsgo-p7-predeletion-baseline.md`：删除前 native 基线（35/35 gates、workspace 2813 passed）与 freeze 状态。
- `docs/evidence/2026-11-20-tsgo-p7-predelete-freeze.md`：`8bda306` 冻结身份、`p7-pre-delete` tag 与 `deletion_allowed=false` 条件。
- `docs/evidence/2026-11-20-tsgo-native-matrix-implementation.md`：真实 matrix **22 exact / 11 blocked** 的修正依据。
- Microsoft TypeScript 官方源码，调研提交 `3e9236361fa97a9e6c685f4833ac457ae2d8c50a`：`packages/typescript/src/api/async/client.ts`、`api/options.ts`、`api/proto.generated.ts`、`tsc/internal/ipc/protocol_jsonrpc.go`、`tsc/internal/api/encoder/encoder.go`、`tsc/internal/api/callbackfs.go`。源码调研与本次二进制实测相互独立；未实测的字段不得当成已验证。
