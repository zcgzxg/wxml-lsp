# WXML LSP：Rust + tsgo 去 Node 迁移执行计划

日期：2026-11-17（执行状态更新 2026-11-20）。状态：执行中；P0/P1 已执行，P2 契约已落地，P3–P6 完成部分切片；P6-8a 默认 `--stdio` 已切到 native，但 Node 实现未删除、P7 未开始；不是迁移完成报告。
仓库：`/home/zxg/code/rust/wxml-lsp`。执行工作目录：`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`（分支 `tsgo/migration`）。当前记录基线：HEAD `899a89a`（native edit gates，见 §12.12）；§12.1–§12.10 保留各自历史快照。

配套决策草案：`wxml-lsp-adr-0008-rust-tsgo-no-node-draft-2026-11-17.md`。正式计划为本文件，ADR 为 `docs/adr/0008-rust-tsgo-no-node-backend.md`。

## 1. 用户目标、范围与完成定义

原始要求：采用 Rust + tsgo 去除 Node；设计架构并记录 ADR；提供详细计划；将测试用例迁到 Rust，控制回归；通过 `/orchestrate` 并行调度 subagent 执行迁移。

本次选择替代旧全功能计划中“不替换 Node 后端”的限制，其他已经接受的能力、项目根规则、信任模型和交付范围继续有效。现有功能不能因迁移重新变成可选项。计划阶段已完成；从 P0 起在 `tsgo-migration` worktree 中按闸门实施。

最终交付必须同时满足：

1. Rust LSP 使用自己的原生 tsgo 子进程；后端实现代码不再需要 TS/Node 适配器，也不自行实现 TypeScript 类型系统。
2. 产品运行、当前测试、开发构建、CI、release 构建、安装与验证无需 Node/npm。原生引擎/类型资产必须固定版本和摘要，离线模式不自动下载。
3. 保留现有 hover、diagnostics、definition、completion、references/rename、小程序 Page/Component/Behavior、属性/事件、loops/guards、JS/JSDoc、跨文件与未保存更新、项目隔离和故障恢复能力。
4. 旧 TS 源测试、展开用例及关键断言均可追溯到 Rust 测试或有证据的重复项合并；禁止靠减少断言、忽略测试或放宽结果比较通过迁移。
5. 现有 Rust 全量门禁、真实原生门禁、差分矩阵、性能与资源门禁、无 Node 交付和 Linux Zed 人工冒烟通过；最后删除旧实现后再验证一遍。
6. 保留业务 `.ts/.js/.d.ts` 输入、fixture 和历史证据。去 Node 不等于删除 TypeScript 语言支持或 tree-sitter grammar。

本计划通过可检验门禁控制回归，不承诺数学意义上的零回归。尚未实测的原生能力必须写为阻塞项。

### 1.1 与旧执行计划的逐项关系

权威历史文件：`/home/zxg/.opencode/plan/wxml-lsp-full-feature-execution-plan-2026-11-16.md`；本次读取的 SHA-256 为 `41f5d6b81c8a0824d6c9849c19eaaafb6127037477dd90db55e568a9a70036a5`。不覆盖该文件；执行时记录其实际摘要，如有变化先复核差异。

| 旧条款（本次读取行号） | 本计划处置 |
| --- | --- |
| 第 1 节目标，第 14 行：保留 Rust + Node | 仅该运行时选择由 Rust + tsgo 替代；全部已接受业务功能继续有效 |
| 用户要求第 1 项、第 18 行；排除项第 26 行“不替换 Node” | 本次明确撤销该限制；同一条中“不移植 TS 类型引擎为 Rust、不降低要求”继续有效 |
| 第 19/28 行：不做 macOS/Windows 实机，不删除跨平台实现/CI | 继续有效，不将未实测平台写成已验证 |
| 第 20～22 行：独立 worktree、/orchestrate、Rust 复核、集成测试 | 继续有效；复核轮次由本计划收紧为“代码修改完成后 review + 自动修复，最多 1 轮”；第 10 节细化依赖基线与文件所有者 |
| 第 27/29/30 行：不外部发布、不改业务文件、本轮仅计划 | 继续有效；正式文档与实现等待可编辑模式 |
| 第 32 行：包/下载/安装/升级/回退、release dry-run、Linux 本机、性能与回归在范围内 | 继续有效；增加旧 Node 安装迁入原生包的明确状态机 |
| 第 9.5 节工程预算 | 继续有效，见第 5 节 P5；旧测量数据不作为新后端通过证据 |

其余已完成工作、修复意图、测试安全要求和未完成交付均保留，不因本计划重开或删除。

## 2. 已核实基线与工作区保护

- 调研 HEAD：`9c9950189c16f722ada288c9e9f6ad9d37f82cf5`，分支 `master`；该值不能代替执行当天的预检。
- 测试代理在 `2026-11-17 21:01:42 +08:00` 的稳定读取窗口记录 19 个 dirty 路径；这是时间点盘点，非当前工作区的永久状态。本轮未修改仓库文件，并发工作仍在进行，执行前必须重新冻结。
- 变化涉及 `backends/typescript/src/{analysis,behaviors,compiler,events,protocol}.ts`、`test/{analysis,compiler,events}.test.ts`、`backend_node/{request,response,tests}.rs`、completion、hover、metadata、server、component_intelligence 和 project discovery。尤其旧 Node 目录中已有正在修复的语义，不能以删除目录的方式丢弃。
- 既有 `full-*`、`wave5-*` worktree 全部视为其他工作的所有物；迁移另用 `tsgo-*` worktree/分支。
- 现有 ADR 到 0007。只局部替代 0001/0002/0003/0006/0007 的 Node、桥接、传输及安装前置条件，其余决策继续有效；0004 并行交付及 0005 文档治理继续有效。
- 当前后端自行启动 Node，并自行创建 TypeScript LanguageService；没有复用 Zed 的 JS/TS 服务。本次目标也不依赖 Zed 提供共享类型服务。

### P0 必须生成的基线记录

1. 记录 HEAD、分支、staged/unstaged/untracked 路径、逐文件 SHA-256、已有 worktree 和工具版本。补丁及临时日志只写 `/tmp/opencode/<run-id>/`，不可写项目目录。
2. 保护未提交内容；不 reset、restore、clean、stash 或覆盖不属于本轮的更改，不自动提交用户工作。
3. 建立“dirty 行为增量”表：每项修复的需求、旧失败、新断言、目标 Rust 落点。仅保存 diff 不算保留功能。
4. 使用独立集成 worktree，从明确的集成基线开始；将需要的当前行为以隔离补丁/经审核的提交纳入 oracle 与迁移基线。主工作区继续可用。
5. 如源文件或修复在冻结后变化，登记 delta 并补齐对应迁移与测试，不静默沿用旧 hash。

## 3. 目标架构与职责

```text
Zed / 其他 LSP 客户端
          │ LSP + 未保存文档版本
          ▼
Rust wxml-lsp：调度、项目、取消/过期响应屏障、C0 DTO
          │ 既有 wxml_types::Backend
          ▼
Rust 小程序类型适配层
  Page/Component/Behavior / props / events / symbols
  原位 shadow / fragments / loops / guards / 双向 source map
          │ 固定版本的原生能力接口
          ▼
Rust tsgo 客户端
  双向 RPC / 受限 VFS / AST codec / snapshot+lease / 进程生命周期
          │ 首选 --api --async，stdio Content-Length JSON-RPC
          ▼
原生 tsgo：TS/JS AST、绑定、checker、类型关系、诊断与原生查询
```

### 核心规则

- Rust 是工作区/未保存内容的权威来源。tsgo 只能读取已批准快照及固定标准库/小程序声明资产，不得绕过现有项目读边界。
- 原位 shadow 保留原注册调用的上下文、`this`、模块和闭包；synthetic 片段通过源码映射回到真实脚本/WXML，绝不写入业务文件。
- 上层保留 `Backend` 边界；`server.rs`、handlers 和模板 parser 不承接原生 wire 细节。只有确有必要且经契约评审才改共享 DTO。
- 原生进程由 Rust 管理；双向 RPC 的回调必须在等待查询响应时仍可服务，防止持有全局锁等待同一连接而死锁。
- 项目 key、配置、文件集合、版本、依赖指纹、session/epoch 一同决定缓存有效性。远程 AST/type/symbol 句柄不能跨 snapshot 或进程重启复用。
- 超时/取消必须有有界队列、响应丢弃与必要的 kill/reap/restart。不能将后端失败映射为“成功但零结果”，也不能因迁移丢弃已有静态降级能力。
- 单进程 API 是待实测的首选。若为了查询保真度需要同进程 LSP + API 连接，先验证共享项目/快照、读策略和生命周期，再修订 ADR/契约。两个独立原生进程的项目图不视为共享。
- 正常使用安装好的原生资产不需要 Go 工具链；从源码构建引擎属于单独的可重复资产生产过程，不把 Go 编译过程塞进用户启动路径。

### 3.1 拟新增模块与依赖方向

以下路径是实施提案，当前尚未创建。选择一个新 crate 并在内部按职责划分，避免将全部迁移逻辑继续堆入 LSP，也避免过早拆出多个单消费者 crate。

```text
crates/wxml-lsp/src/backend_tsgo.rs       ProjectHost → InputProvider adapter
crates/wxml-backend-tsgo/
  src/lib.rs                            受控公开入口
  src/input.rs                          AnalysisInput / InputProvider / budgets
  src/backend.rs                        impl Backend / 结果缓存
  src/config.rs                         引擎和资产配置
  src/native/{supervisor,framing,rpc,protocol}.rs
  src/native/{ast,handles,strings,vfs}.rs
  src/script/{registration,behavior,properties,events,symbols}.rs
  src/shadow/{expression,rewrite,layout,locals,guards,mapping}.rs
  src/query/{hover,diagnostics,definition,completion,bindings}.rs
  tests/                                真实原生合同与集成测试
```

`wxml-lsp → wxml-backend-tsgo → wxml-types`；后端按实际所需使用现有 text/syntax 工具，但不能反向依赖 `wxml-lsp` 或复制整个 ProjectHost。`InputProvider` trait 与输入 DTO 由新后端拥有，LSP 用薄 adapter 实现。保持现有 types/project 的分层约束；共享 DTO 改动由集成 owner 审核。

### 3.2 快照、句柄、映射与编辑安全

- `AnalysisInput` 是不可变源快照：项目/文件 revision、源文本、编译选项、固定声明资产、模板片段及完整性。
- `DerivedView` 是同一输入的原位 shadow、分段映射、基线 facts、降级原因；不引用随时变动的宿主状态。
- `NativeLease` 仅在后端内部，绑定 engine epoch + snapshot/program/source 身份，不能作为稳定 symbol ID 返回上层。
- 第一版按 owner 串行完成基线分析→提取纯数据 facts→同 session 更新 shadow→重新取得 program handles→查询。跨 snapshot 不复用旧 type/symbol/node；独立语法树节点不得用于 program checker。以后若优化 immutable snapshot/并行 session，另加合同与资源证据。
- 保留原实现对基线/派生 `this` 类型、用户脚本新增诊断和语义漂移的校验，不只比较最后的模板 diagnostics。
- 映射需要组合：WXML UTF-8→expression copy/rewrite→一个或多个 mount→shadow UTF-8↔native/LSP 坐标；原脚本则另有插入/替换映射。现有仅适用于有序、不重叠、等长 copy 的 `SourceMap` 不足以单独表达全部场景。synthetic 文本无真实源范围；generated declaration 导航用显式关系，不冒充 verbatim copy。
- 常态每个 Rust LSP 最多一个 native 子进程，懒启动，多项目使用有界 session/snapshot LRU；记录活跃 program/session/snapshot 数和切换成本。同进程并不保证所有查询共享类型图。
- 延续现有取消语义：排队任务可跳过，运行任务取消后不发布结果，计算占用由剩余 deadline 约束。native 即时取消能力未验证，不写成既有保证；需要下沉 cancel token 时单列合同任务。
- symbols 返回精确原始 token 与 `expected_text`；`complete=false`、动态 key、spread/computed、缺失 Behavior、快照外引用及截断均不能生成编辑。最终路径、版本、范围、重叠、语法及 UTF 转换仍经 LSP 的 EditPlan 校验，不原样透传 native rename edits。
- 当前 TS 后端还可能把 compiler `typeRoots` 纳入允许读取集合；P0 必须核对实际有效读策略，迁移为快照或显式批准的只读声明集合，不能把“仅固定两个目录”误写成已满足的现状。

### 必须先验证的原生协议事实

调研对象为 Microsoft TypeScript 官方源码提交 `3e9236361fa97a9e6c685f4833ac457ae2d8c50a`；这只是候选源码，尚未选择生产发行物。

- 异步 API 使用带 Content-Length 的 JSON-RPC，不能沿用旧 NDJSON parser。
- JSON 响应仍可能包含 base64 二进制 AST。AST 的 schema、节点种类表、结构化数据、字符串池、source file/lease/handle 都要冻结并验证，不能假设直接反序列化 JSON 就能遍历 AST。
- 对每个字段确认坐标单位。AST 位置、API 位置、LSP UTF-16 与 Rust UTF-8 不得共用无单位整数；字符串池字节偏移和 JS 含孤立 surrogate 的字符串单独处理。
- 原生 API 不是旧 `ts.LanguageService` 的同名替代。hover 文档/展示、重载、alias、定义与安全引用需要明确实现和逐项验证。
- 未注册 FS 回调和显式 useOS 可能读真实磁盘；写入 no-op/noEmit 不构成 OS 沙箱。只允许已审查的查询方法，不启用 external code、emit/build/clean。

已通过官方固定提交源码复核：`api/options.ts` 的 `--api/--async` 参数、`ipc/protocol_jsonrpc.go` 的 Content-Length、`api/encoder/encoder.go` 的二进制格式（该调研提交的 protocol version 为 9）及 `api/callbackfs.go` 的回退行为。运行时仍须按选定发行物重新确认，不能把源码版本数字当作跨版本稳定协议。

## 4. 冻结契约与共享文件责任

在并行实现前，由集成负责人维护唯一版本的四份契约。实现 agent 只能提出变更请求，不能单方面改变共享 DTO 或规范。

| 契约 | 必须冻结的内容 | 验收方式 |
| --- | --- | --- |
| F0 原生兼容 | 固定 `7.1.0-beta`（gitHead、平台资产、SRI/sha256）、CLI 参数、API 方法白名单/帧上限、AST protocol v9/header 64、错误类别；`7.0.2` 明确排除 | executable `--version` + 摘要 + schema 探针；版本/协议不符明确失败；任何 emit/build/clean/profile 方法必须拒绝 |
| F1 Backend 语义 | C0 DTO、snapshot/epoch/handle 绑定、`closeFiles+openFiles` 更新、取消/超时、错误、UTF/URI、map、缓存与 invalidation；QuickInfo/Definition/Rename 的 Rust 组合与保守拒绝规则；hover 文档/重载、补全详情缺失的明确降级 | 合同测试、旧响应/句柄失效测试、跨快照错误解析注入、写方法拒绝与 FS fail-closed 测试 |
| F2 测试基线 | 源测试 ID、展开参数、关键断言、fixture hash、oracle revision、允许归一化字段 | 全量映射账本与 comparator 故障注入 |
| F3 交付 | 二进制/类型资产布局、定位/env、manifest/index schema、离线及旧包策略 | 构建/包/安装/升级/回滚合同测试 |

集成负责人单写：根 `Cargo.toml`/`Cargo.lock`、新 crate 清单、`crates/wxml-lsp/{Cargo.toml,src/lib.rs,src/main.rs}`、共享 `wxml-types`、`server.rs`/`project_host.rs` 如需变更、共享测试工具、`.gitattributes`。

交付负责人单写 `wxml-delivery/src/manifest.rs` 及 schema；CI/构建负责人单写 `scripts/build-release.sh` 和 `.github/workflows/ci.yml`；Zed 负责人单写 `editors/zed/src/lib.rs`；文档负责人单写 README/ADR 索引。一个文件只能有一个当前 owner。

## 5. 执行阶段与门禁

```text
P0 冻结当前行为与工作区
 ├─ P1-A 原生 API/AST/FS/查询保真度试验
 ├─ P1-B 测试逐项盘点、oracle 与回归比较器设计
 └─ P1-C 无 Node 资产、交付 schema 与离线路径试验
              │ 三路通过
              ▼
P2 ADR + F0/F1/F2/F3 冻结 + 安全评审
              ▼
P3 客户端/会话 ║ 语义基础与映射 ║ Rust 测试支架 ║ 交付基础
              ▼
P4 按依赖并行迁移完整语义切片，每片 TDD/差分/评审
              ▼
P5 全量等价、恢复、安全、性能与无 Node 候选验证
              ▼
P6 默认切换 + 候选 release/安装/回滚/Linux Zed 验收
              ▼
P7 串行删除旧 Node 实现与当前 TS 测试，再执行全部门禁
```

### P0：可重放基线

按第 2 节保护工作区，补齐当前 Rust/Node 基线的实际执行记录。未运行的历史统计不能写成此次通过。输出源测试 manifest、dirty delta、fixture 字节 hash、工具/原生资产信息及 oracle 配置。无法重现的旧测试必须先诊断，不能算迁移成功。

退出门禁 G0：当前行为和未提交修复有明确归属；独立集成基线可重放；所有后续 agent 使用同一个 BASE。

### P1：三路只做最小能力验证

**A 原生能力（P1-A 已完成，有条件通过）**：先验证稳定版，实测 `7.0.2` 不满足 `createSnapshot`/租约/`createSourceFile`，AST protocol v5（header 44）与候选线不兼容；唯一可行候选为 `7.1.0-beta`，AST protocol v9（header 64），已固定 gitHead 与 linux-x64 SRI。独立无 Node 客户端原型已完成启动、版本/schema 检查、Content-Length、双向 FS 回调、虚拟项目 snapshot、AST 关键字段解码、类型/符号/文档/补全/引用查询。未验证项：完整 Page/Component 原位 shadow、Behavior/props/loops/guards 组合、跨文件 alias、保守 rename、完整 AST msgpack/children/binder 解码。实测风险：无 QuickInfo/Definition/Rename 直接 API；node handle 跨快照可静默错位；`emit` 默认可写宿主磁盘；未保存更新需 `closeFiles+openFiles`。证据：`docs/evidence/2026-11-17-tsgo-native-api-spike.md`。

**B 测试与 oracle**：执行完整旧 TS 套件，记录直接测试/参数化展开/执行通过/跳过分别的数量；梳理每条关键断言和 fixture。历史 Node 后端在隔离基线运行，仅用于过渡期对比。原实现与预期都为空不得自动视为覆盖。

**C 资产与交付**：用无 npm 的 Rust 资产工具验证一个 Linux 当前支持平台的包获取/解包/摘要/许可/标准库与小程序声明挂载，证明预置资产下离线构建和启动可行；设计 schema 升级、安装失败不切 current 及原生包间回滚。验证目标 ABI，不能只看 OS/CPU 名称。

退出门禁 G1：三路全部完成并有证据。任何必需查询、AST、读策略或离线资产缺口都阻塞 P2/P3；先解决或明确修订架构，不借减少功能继续。

**G1 实际状态（2026-11-17，未关闭）**：

- A：有条件 PROCEED。`7.1.0-beta` 是唯一满足快照/虚拟 FS/查询/AST 的版本；`7.0.2` 明确失败。hover/definition/rename 无直接 API，需 F1 组合规则；handle 跨快照错位、`emit` 写盘、更新语义已列为必测风险。证据：`docs/evidence/2026-11-17-tsgo-native-api-spike.md`。
- B：P0 Rust 基线绿（1297 pass/0 fail/23 ignored，36.5s；fmt/check/clippy 通过）；TS 套件 339 个展开用例，338 pass/1 fail。唯一失败是 `perf.test.ts` 全量并行下冷启动 8325.25 ms；同一测试串行隔离 3/3 绿（1058.25/1005.64/675.60 ms），已裁决采用专用串行 perf gate，阈值与测试内容不变，全量并行结果保留为环境记录。台账 274 静态/339 展开，37 个原 blocked 决策已关闭：36 个 Candidate B/symbol-bridge 为 `obsolete`，1 个 `TS-META-0027` 为 F0 version pin `rewrite`；原位 shadow 用例仍强制。证据：`docs/evidence/2026-11-17-tsgo-migration-baseline.md`、`docs/plans/2026-11-17-tsgo-test-ledger.md`、`docs/plans/tsgo-test-ledger.json`。
- C：PARTIAL PASS（可执行 spike）。Node 依赖面、原生入口 `lib/tsc`、7.1.0-beta/7.0.2 资产结构与 manifest v2 状态机已确认；`/tmp/opencode/tsgo-delivery-spike` 已实测校验、原子安装、失败保持旧版本、N1→N2 升级、回滚、离线安装和无 Node PATH，但生产 Rust 安装器、CI/Zed、平台矩阵与断电持久性仍未完成。证据：`docs/evidence/2026-11-17-tsgo-delivery-spike.md`。
- B-02：PARTIAL（负结果）。`appendFile`/`chtimes` 无合法回调名，源码明确始终走 base OS FS；维护者 2026-11-18 接受补偿控制（方法白名单 + 运行目录最小权限 + 禁用 emit/external code），不得声称回调级 deny。
- 结论：P2 入口证据已收口，可启动 P3 的 native client/FS/AST 骨架；F1 查询与语义切片仍受 B-05–B-09/B-11/B-13 阻塞。A 为有条件通过，B 的测试处置与 perf gate 已裁决，C 的可执行安装器方案已由 spike 证明但生产实现转入 P3-D/F3。维护者已于 2026-11-17 有条件接受固定 integrity 的 7.1.0-beta，并于 2026-11-18 接受 B-02 补偿控制；不得把 G1-C 说成全绿。

### P2：落地 ADR、接口与测试契约

将 ADR-0008 和本计划写入正式 docs 目录，更新 `docs/adr/README.md`、`docs/README.md`，对既有 ADR 只加局部替代链接。冻结 F0～F3、模块路径与 owner、资产 lock、错误码/能力与版本策略。安全 reviewer 审查 RPC 输入、路径/符号链接、原生 FS 回退、写入口和外部执行边界。

退出门禁 G2：架构与合同通过；原型转为可保留的合同测试；每个并行包有唯一 writable scope、依赖、输入/输出和验收命令。

### P3：可测基础骨架

可在 F0～F3 冻结后并行：

1. 原生客户端：进程、帧、双向回调、消息路由、有界输入、AST/句柄、取消、超时和回收。
2. 语义基础：注册定位、文本/位置工具、原位 shadow、fragments/locals/guards、映射和类型资产快照。
3. Rust 测试设施：fixture runner、断言映射表、差分器、fake native peer、AST/位置性质测试、真实原生测试准备。
4. 交付基础：原生资产 lock、包布局、manifest/index schema、校验与离线安装。最多同时推进三个实现 worker，按 CPU/内存顺序排队第四个包。

退出门禁 G3：骨架能通过真实原生端到端的一个完整正例及负例；fake 测试不能代替真实连接。共享 API、依赖和锁文件由集成负责人合并。

G3 通过后，集成负责人记录 `FOUNDATION_BASE=<已合入 N/S/T 及相关交付基础的 commit>` 与合同摘要。P4 必须从此基础或更新的已验收 `INTEGRATION_BASE` 派生；不从仅含空接口的 CONTRACT_BASE 直接开始语义实现。

### P4：语义与测试同批迁移

每个切片必须同时交付实现、对应 Rust 测试、源测试映射、差分与相邻功能结果。先编译通过但语义失败的 RED，再最小 GREEN，再重构/评审；不能只提供编译失败的“RED”。

- 切片 A：原位 shadow、名称 rewrite、fragments/locals、嵌套循环、条件守卫与基础 diagnostics。
- 切片 B：Page/Component 注册、data/properties、Behavior 合成、属性/事件绑定、跨文件依赖与增量失效。
- 切片 C：hover/文档、diagnostics 清理/重发、definition、completion 与 fallback，精确映射到原文件。
- 切片 D：symbol 提取、references、保守 rename、`setData`/observer key、字符串/注释排除、不完整结果拒绝编辑。

切片 A/B 的基础接口先冻结；C/D 可依赖其固定快照并行，但同一 AST/mapper/query 公共文件不能并发修改。每片代码写完立即 code review，Rust 专项复核所有权、边界、错误与资源管理。

退出门禁 G4：全量迁移账本已覆盖、无未归属断言；脏工作区行为增量也已纳入；新引擎与旧基线差异均已解释并获得符合需求的断言。

### P5：综合回归与质量门禁

运行第 7、8 节矩阵。重复相同序列验证结果稳定；覆盖配置变更、跨项目、未保存导入、依赖删除/恢复、引擎退出/卡死/损坏消息、请求取消、旧版本响应、长时间内存与进程回收。

性能使用当前冻结基线同机同 fixture/版本/冷热条件，记录端到端延迟 p50/p95/p99、总 CPU/RSS、进程数、队列、长跑增长；沿用既有阈值，新增预算在 P2 先冻结。不得拿编译器全量构建加速率当作本项目请求性能证明，也不得用旧小样本记录替代本次测量。

沿用既有全功能计划第 9.5 节的工程目标（不是已测结果或用户 SLA）：静态补全/结构查询 p95 ≤ 50 ms；热态语义 hover/completion/definition p95 ≤ 200 ms；停止输入到新诊断 p95 ≤ 500 ms；可中断队列任务收到取消到退出 p95 ≤ 100 ms。每组固定语料至少 200 次；冷启动与突发编辑另外统计。已有路径 p95 不得回退超过 10%，Rust + 后端合计稳态 RSS 不得回退超过 15%；10 轮打开/关闭/编辑后不得持续增长。不可中断的原生调用占用单列，不能以丢弃响应代替计算终止。若基线本来不达标，列出瓶颈和修复工作项，不静默降低目标。

退出门禁 G5：功能/安全/恢复与资源预算全部通过；当前测试不依赖旧 Node oracle。CRITICAL/HIGH 必须清零。MEDIUM 仅限不影响验收契约的维护性问题可记录处置理由；功能差异、编辑安全、FS 越界、必跑测试失败、覆盖率或性能门禁失败，无论严重度标签都不能豁免。代码修改完成后执行 1 轮 review + 自动修复；修复后只做针对性验证，不再开启第 2 轮完整 review。仍有 CRITICAL/HIGH、功能/安全/测试/覆盖率/性能门禁失败时标记 BLOCKED 并报告；轮数上限不产生自动通过。

### P6：默认切换与无 Node 候选交付

在测试集成中切换默认后端，只使用签核的原生版本。移除实际构建/运行/测试对旧后端的引用，旧源码暂只作为不参与构建的待删除材料。验证新包、安装器、Zed 启动、旧 Node 安装迁入原生包、离线升级和原生版本间回滚，执行第 8.1 节全部状态转换。每项 NF 门禁必须在 Node 不可用的独立环境验证。

退出门禁 G6：完整候选包在当前 Linux 支持环境通过；旧 TS 测试已无运行职责；旧实现删除授权范围已清楚且用户改动均已迁移。无支持证据的平台不能标为正式支持。

### P7：最后删除、重新验证与交接

串行删除旧 `backend_node`、旧 TS 实现/当前 TS 测试/Node 构建脚本和包锁；将需要的 samples 移至统一 fixture，保留 hash、CRLF 和许可证。删除必须是可单独审阅的变更，不夹带行为重写；先确保目录中的用户工作均已处置。

重新执行全量门禁、两次确定性 release 构建、安装/升级/回滚和 Linux Zed smoke，检查活跃引用清零。新增证据及 provenance；更新 README 当前状态、阶段 6、验证命令和支持平台。历史计划/证据/ADR 保留原事实并明确其历史身份。

退出门禁 G7：仓库、包和实际进程树均符合完成定义。最终报告列出迁移用例/断言数量、差异处置、工具/引擎/提交/资产摘要、测试结果、平台验证范围和已知限制。

## 6. 测试迁移规则

### 6.1 迁移账本是唯一进度来源

正式落点建议 `tests/backend-parity/cases.json` 与对应 Rust 校验器；P2 根据现有测试组织冻结路径。每个条目至少包含：

`source_file`、`source_test_id`、`case_id`、参数展开、行为说明、fixture 路径及 hash、关键断言、Rust 测试位置、oracle revision、dirty delta ID、状态、结果/证据。

允许状态：待迁移、已写 RED、已 GREEN、已差分、已集成通过；重复项合并须指向覆盖同一断言的 Rust 测试并解释。与旧 Node 协议专有实现相关的测试，迁为相同失败模式的原生合同测试，不简单删除。历史证据脚本的路径/脱敏/确定性断言也要迁移。

必须区分静态 `test()` 声明数、循环/参数化展开数、实际执行数、通过/失败/skip/ignore 数。过去记录的“333”不能代替执行当天盘点。默认 Rust 测试通过也不等于真实原生门禁运行。

### 6.1.1 本轮静态盘点及迁移分区

盘点时间 `2026-11-17 21:01:42 +08:00`，HEAD 同第 2 节；所读测试内容在该读取窗口稳定。以下全是静态计数，**没有运行通过结果**，P0/P1-B 必须刷新。

| 当前 TS 文件（均位于 backends/typescript/test） | 源码 test 调用数 | 必须迁移的行为 |
| --- | ---: | --- |
| analysis.test.ts | 23 | 查询编排、快照、quality、behavior spread |
| compiler.test.ts | 6 | 项目池、文件/完整 options 身份、固定 typings |
| completion.test.ts | 9 | 根/成员/事件、替换范围、排序与降级 |
| componentBindings.test.ts | 28 | props union/结构/optionalTypes、Behavior、逐 tag 诊断 |
| definitions.test.ts | 5 | 原始声明、导入、.d.ts、overlay 回映 |
| events.test.ts | 23 | handler 签名/文档、跨文件别名、精确范围与不完整性 |
| evidence.test.ts | 3 | 证据路径、脱敏与产物约束 |
| guards.test.ts | 16 | 条件收窄、循环交错、JS 策略 |
| layouts.test.ts | 12 | 原位视图、properties overlay、上下文保真 |
| locals.test.ts | 12 | 嵌套/遮蔽、数组/tuple/Record/any、预算 |
| mapping.test.ts | 16 | 范围、copy/rewrite/mount、UTF 与生成来源 |
| matrix.test.ts | 13 | 33 scenario × 2 layout 来源身份与断言 |
| messages.test.ts | 5 | 有界公开消息与无内部路径泄漏 |
| meta.test.ts | 2 | 固定版本、能力与契约 |
| overlay.test.ts | 5 | 未保存 owner/依赖、回退与失效 |
| perf.test.ts | 1 | 冷启动、hover、diagnostics 采样语义 |
| probes.test.ts | 10 | pragma、probe 健康、用户诊断不干扰、读边界 |
| protocol.test.ts | 30 | 输入/预算/重复 ID/身份/版本验证 |
| rewrite.test.ts | 24 | 字符串/注释/key 不改写、locals/global、escape 拒绝 |
| server.test.ts | 9 | 传输、恰好一次响应、错误与 EOF 恢复 |
| symbols.test.ts | 21 | 引用、setData/observer、完整性与编辑拒绝 |
| 合计 | 274 | 21 个测试文件，另有 support.ts |

274 个调用＝273 个字面标题＋1 个动态标题调用；后者展开 33×2＝66 项，实际注册总数 `274−1+66＝339`。矩阵文件 `12+66＝78` 项。内部循环（例如 10 个 LOCAL_CASES）还要分子用例 ID，不能仅按注册数量迁移。

Rust workspace 原始 `#[test]` 属性 1,322 个（src 948/74 文件、tests 374/32 文件），编译后 1,319 个（3 个 `#[cfg(windows)]` 在 Linux 不编译），包含 13 个 proptest 定义；不含 doctest、bench、workspace 外 Zed 测试，也不是执行数。其中 23 个显式 ignored 均在 LSP 集成层，必须逐项安排必跑原生命令。旧 backend_node 有 100 个单元测试属性；已明确耦合的 13 个集成根文件有 111 个测试属性，不能删模块时丢弃。这两个集合不是所有受影响测试的上限。

夹具盘点：`tests/fixtures` 136 文件（manifest 19 案例：16 实体＋unsaved-overlay/stale-request/zed-multi-doc 3 运行案例）；`crates/wxml-lsp/tests/fixtures` 43 文件；旧 samples 35 文件/28 目录，对应 33 scenario。合计 214 个磁盘文件不含内联/生成输入。树摘要算法（v1）为 regular files only，按 UTF-8 相对路径字节排序，累加 `relative_path_utf8 + NUL + raw_bytes + NUL`：

```text
backends/typescript/test       630a16af41623c78f19f47bcaba1a7f1282201ac27b4414115ac0b3764afc32e
backends/typescript/samples    00a7fa3d6962da50581c396f73c13f7ffb043efae060a0801b373928726e5c1f
tests/fixtures                7c9ddedda18c70bc217af6b3bbd23dd22a33be9c885276cbd587adc0596f04a9
crates/wxml-lsp/tests/fixtures  834eea4defcb7565c4b84e91a173079cf3be85201d44fb4a7a6d0a32af3da073
```

拟建公共入口为 `crates/wxml-lsp/tests/native_parity.rs`，分区位于 `tests/native_parity/{hover_docs,definitions,completion,loops,guards_js,components,behaviors,events,project_overlay,symbols_edits,scenario_matrix,protocol_limits,messages_evidence,shadow_rewrite,mapping_utf16,perf_budget}.rs`；纯 codec/emitter/map 测试置于实际生产模块旁。新后端 crate 的真实 native 合同测试与公共 LSP 语义矩阵分层，不能互相代替。

台账在第 6.1 节字段外还必须包含旧测试体 hash、完整标题/参数 ID、每条 assertion ID/原始 span、Rust 全限定测试 ID/子用例、比较类型（semantic_exact/safety_exact/contract_translated/mechanism_replaced/native_new_risk）、负例、RED/GREEN 原始证据、reviewer 和 native 身份。只有已审阅且门禁通过的断言可关闭；blocked 和机制替代均不可直接记“不适用”。

dirty 必保增量至少包括：Behavior spread 展开后仍为 boolean/Supported；别名 Behavior handler 定义回到原文件并保留中文文档，别名缺失为 Partial 而非伪造 missing-handler；自定义 typeRoots 不挤掉固定声明；已具备有效内容的 hover 不追加降级文案；中文元数据/官方链接/枚举、unknown attribute 路由及 root component 回退。P0 按实际 diff 补齐，不以该简表代替全部增量。

现有 semantic_tools 中“缺 dist/server.js 直接 return”的真实后端路径，以及“缺 tsc 可不做编辑后类型检查”的路径必须改为 native 必备门禁；不能保留静默成功。批准的 WorkspaceEdit 应用到隔离快照后重新 parse/typecheck；拒绝编辑同时断言错误、无编辑且文件未变。

### 6.2 差分允许与禁止

- 允许归一化：临时路径根、非语义 ID 的一一对应、明确无顺序语义的集合排序；保留 ID 引用和版本先后关系，规则写入 F2。不得排序/去重掩盖漏项或重复项。
- 禁止抹去：hover 类型/文档/重载、诊断 code/severity/range/message 的合同部分、definition URI/range、completion 关键顺序与替换范围、references/rename 编辑目标和不完整标志。
- TypeScript 版本升级导致的合理诊断差异按用例记录旧/新、需求依据和独立预期，不能整批接受快照或删除失败断言。
- comparator 自身必须检测人为注入的缺失条目、范围偏移、丢失文档、不安全编辑和两端都错误为空；保留不依赖旧实现的规范断言。
- 迁移期 oracle 环境可使用 Node，但终态门禁只运行 Rust runner + 固定原生引擎；不能将 Node 留作测试生成器或必需的 schema 生成器。

### 6.3 真实引擎门禁

真实原生测试不得因为找不到引擎而成功返回或默默 skip。CI 必须显式准备并校验原生资产后执行；资产缺失/不兼容直接失败。快速纯单元层可使用 fake，但其报告不能冒充真实引擎结果。确需独立 feature/profile 的慢测必须由明确的必跑命令与 CI job 覆盖。

旧 fixture 的 messageContains 不能独立充当等价门禁；保留原断言，并补精确 code/range/语义内容。原生历史实验中的 included、显式 files、custom scheme、项目归属、未保存依赖与诊断修复也迁为 Rust 自动测试。并发/恢复优先用 channel/barrier/可控时钟，不用 sleep 碰时序。

### 6.4 新原生风险的独立测试

- 坐标分阶段验证：ASCII/CJK/emoji/组合字符 × LF/CRLF/BOM/hashbang × token 前/内/后/EOF；AST pos/end、pool 字节位置、Rust source range、LSP line/character 分别验证后再端到端，防止两次错误换算抵消。
- 孤立 surrogate 用原始字节或 Vec<u16> 构造，不先经过 Rust String/lossy 解码；保真 wire 值与不可转换为 source range 的错误策略写入 F0/F1，不替换为 U+FFFD 后编辑。
- codec 截断、越界、溢出、非法 tag/引用、错误类型、无效 WTF-8、循环/深度/输出预算：typed error、无 panic、有界分配；畸形输入性质测试覆盖规定的后续会话行为。
- v1 查询→v2 更新→v1 迟到、release 后访问、跨项目/session、重启后旧句柄：不得发布旧结果或修改错误源，清理有界。
- native FS callback 与 response 交错、嵌套读取、取消/超时/重启中回调：无锁等待死锁、ID 配对、每请求一次响应、快照与读边界正确。

## 7. 必测矩阵与命令

| 层次 | 必测内容 | 回归判据 |
| --- | --- | --- |
| 纯语义 | 注册/alias、rewrite、loops/guards、locals、props/Behavior/event、symbols | 对应源断言全部到 Rust，正反例和安全拒绝完整 |
| 文本/codec | UTF-8/UTF-16、非 BMP、CRLF、转义、WTF-8、AST 索引/长度/版本/循环 | 精确范围，无 panic/越界/损失性字符串替换；性质测试 |
| RPC | 分帧/粘包/短读、Content-Length、大小/深度、ID、callback 交错、错误/EOF/stderr | 有界资源，错误明确，无等待死锁、无 stdout 污染 |
| 会话 | snapshot 更新/释放、lease、句柄失效、epoch、缓存和项目隔离 | 不复用过期类型/符号；重启和 cancel 不发布旧结果 |
| 真实查询 | JS/JSDoc、泛型/overload、alias、imports、tsconfig/extends/path、未保存依赖 | hover/diag/def/completion/reference/edit 逐项符合 F2 |
| 小程序完整 E2E | Page/Component/Behavior、data/props/events、模板 loop/guard、跨 app | shadow/原脚本/WXML 映射及编辑安全与原合同等价 |
| 恢复/压力 | 崩溃、卡死、大消息、反复编辑、退出、依赖删除/恢复 | 有界队列/RSS/时延，无孤儿进程，无跨项目泄漏 |
| 文件安全 | 配置/导入/包/声明、symlink/逃逸、所有 FS 回调/OS 回退/写入口 | 不超出现有权限，不写业务文件；拒绝非法包和路径 |
| 交付 | 摘要/许可、schema、离线、恶意压缩包、原子 current、升级/回滚 | 无 Node、无隐式下载、失败不改变已验证安装 |
| 编辑器 | Linux Zed 未保存脚本/WXML/配置、查询、rename 与恢复 | 实际 UI 验证，保留必要证据，不把 stdio 测试称为 UI smoke |

每个实现包至少执行相关测试；集成后执行：

```sh
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo test --workspace --doc --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo llvm-cov --workspace --all-features --fail-under-lines 80
cargo audit
cargo deny check
```

具体 native E2E、差分、资产验证、no-node guard 的命令由 P2 冻结在本计划与 CI；不得仅写未来命令而不给实现。执行固定 lockfile；完成 provisioning 后用 Cargo 离线模式重跑无 Node 门禁。`cargo deny` 当前策略文件需先由集成/安全负责人建立；审计数据库/依赖/原生资产预置后验证离线流程。命令缺失、工具未安装或数据库过期必须报告，不计为通过。

覆盖率要求：workspace 与迁移新增/修改 Rust 代码均 ≥80%；codec、输入验证、路径/读边界的安全契约分支及拒绝路径必须全部命中。安全敏感模块目标为 100% 行覆盖，未命中逐行说明并补测，不得通过排除文件抬高数字。报告分别列出整体、变更代码和关键模块，不能用整体均值掩盖新实现缺测。

## 8. 无 Node 交付验收 NF-01～NF-12

| ID | 必须通过的验收 |
| --- | --- |
| NF-01 | 产品只启动受验证的原生资产；无 node/npm/npx/pnpm/yarn 子进程，无隐藏 Node fallback；缺失引擎显式报错/可观察降级 |
| NF-02 | 在无 Node 环境完成 Rust 构建、单元/集成/文档/真实原生测试；不再要求 `WXML_TEST_NODE=1` |
| NF-03 | CI 无 setup-node/npm 步骤；原生资产预置校验后执行必跑 native job，缓存 miss 明确失败 |
| NF-04 | release dry-run 与完整构建无需 Node；离线缓存完整时可 `--offline`；无 npm prune/build |
| NF-05 | manifest/index schema 升版，移除 Node 最低版本/NodeBackend 等新包必需字段，加入引擎和类型资产兼容信息 |
| NF-06 | install/verify/current/env/verify-package/upgrade/rollback 无 Node；失败不得切换 current；env 保留 `WXML_LSP_BIN` 单入口 |
| NF-07 | Zed 移除 `WXML_NODE/WXML_TYPE_BACKEND/WXML_BACKEND_DIR` 转发，只凭已验证安装布局找到引擎；五语言注册继续保留 |
| NF-08 | 启动/build script/测试不隐式下载；远端资产获取只在显式 provisioning/下载命令中发生，离线模式 fail-closed |
| NF-09 | 固定发行版本、源码提交、API/schema、平台和摘要；包 SRI/二进制 SHA-256、类型资产及许可证、SBOM/provenance 可追溯 |
| NF-10 | 旧 Node 安装→首个原生包、新原生包→升级→回滚，以及损坏包/缓存、错误平台、离线缺缓存均按第 8.1 节处理；保留原子切换和完整校验 |
| NF-11 | 业务 TS/JS/.d.ts、samples、CRLF/Unicode 字节保护及历史证据保留；vendored grammar 不因含 Node 绑定文件被误删 |
| NF-12 | 活跃 README/CI/命令/契约不再要求 Node；历史 ADR/证据保留但不可作为当前命令；静态引用检查与进程树验证一致 |

仅靠 PATH shim 不足以排除绝对路径调用。最终用未安装 Node 的独立环境，结合子进程追踪/审计验证；shim 用作辅助失败检测。预置 Rust/Cargo、wasi-sdk/grammar 及原生资产，记录网络开关与缓存命中条件，避免将“无需 Node”混同为“无需任何工具或预置资产”。

### 8.1 旧安装迁移与失败/回退状态机

F3 必须冻结旧 schema 的受限读取器及以下状态机。读取旧 manifest/current 不执行旧二进制，不要求 Node；旧路径仍按信任与路径约束校验。旧 schema 仅用于识别、保留与迁入，不恢复旧 NodeRuntime 前置条件为新包要求。

| 初始状态/动作 | 预期结果与必测断言 |
| --- | --- |
| 空安装→安装原生包 | 完整校验 engine/资产/ABI，暂存区完成原生健康检查后原子切 current；缺缓存/损坏/中断不产生已激活的半成品 |
| 旧 Node 包 current→准备迁移 | 无 Node 也能识别并保留旧包；显式 provisioning 或预置资产，构建新 schema 安装；不调用旧服务、不自动修改 Zed 用户配置 |
| 旧 current→新包验证失败/中断 | 原 current 与旧包字节保持不变，明确“尚未迁移”；旧环境是否仍可运行依赖它原来的 Node，这不是新包无 Node 验收成功，也不偷偷恢复/安装 Node |
| 旧 current→新包验证成功 | 一次原子提交激活原生安装；新 env/verify/Zed 启动只使用新布局；旧包作为非活动历史材料保留；整个迁移事务在无 Node 环境测试 |
| 已激活原生 N1→N2 升级失败 | 验证前不动 N1；中断恢复/事务日志按已冻结状态机恢复到最后验证过的原生 current，不出现混合资产 |
| 原生 N2→显式 rollback N1 | 重新校验原生 N1 的完整资产/版本/摘要再原子切换，离线也必须满足资产完整性 |
| 原生运行故障且无可用原生回退 | 可观察错误/静态能力降级；不自动激活历史 Node 包。保留诊断与人工恢复指引，不把故障当空结果 |
| 新安装器请求激活/rollback 到旧 Node 包 | 明确拒绝：旧 schema 非 Node-free 目标；不改变原生 current。不删除旧材料；确需恢复历史版时由维护者显式使用旧版工具，此举不属于新版本无 Node 保证 |
| 篡改旧 current/旧路径逃逸、未知 schema、恶意包 | 在执行或切换前拒绝；保护已验证安装，错误不泄漏不必要路径/数据 |

P1-C 证明旧→新的可行路径；P6/P7 用真实本地包重跑全部转换，含 Node 已卸载、离线缓存命中/缺失、进程在提交前后被终止和多个安装事务冲突。跨 schema 读取器/事务日志由 D 单写，测试验证旧材料不被覆盖、current 不悬空且原生资产来自同一发布身份。

## 9. 交付与文档具体落点

| 范围 | 修改或迁移目标 |
| --- | --- |
| 入口 | `crates/wxml-lsp/src/{lib,main}.rs`，替换 backend 构造/usage/env；`server.rs` 保持 backend-neutral |
| 旧适配 | `backend_node/{config,process,protocol,request,response,completion,mod,tests}.rs`，先迁移语义/资源规则再删除 |
| Rust 测试 | `node_backend`、`real_node`、`stdio_type_loop`、`wave5_acceptance`、`performance_bench`、`q2_*`、`semantic_tools`、`lsp_p7`、`zed_sync`、`multi_app_projects`、`full_feature_matrix`、`component_intelligence` 及共享工具逐项去 Node |
| 安装格式 | `wxml-delivery/src/{manifest,package,verify,install,cache,acquire,source,error}.rs` 和 `bin/{wxml-lsp-install,wxml-lsp-package}.rs`；相应 installer/security/packager 测试 |
| 构建/CI | `scripts/build-release.sh`、`.github/workflows/ci.yml`；移除 npm/TS构建/Node 最低版本检查；原生资产校验和 no-node guard |
| Zed | `editors/zed/src/lib.rs`、`editors/zed/README.md`；保留 grammar pin/注册，更新启动/env |
| 固定类型资产 | 小程序 `miniprogram-api-typings` 声明及原生标准库按固定版本/许可提供，不在用户项目运行 npm |
| 样本 | `backends/typescript/samples/**` → 统一 fixture，保留内容 hash；同步 `.gitattributes` 与 fixture README |
| 文档 | ADR-0008、ADR 索引/局部链接、`docs/README.md`、项目/各 crate/Zed README、contracts/type-bridge/diagnostics/baseline/provenance、全新迁移证据 |

资产解包必须拒绝绝对路径、`..`、symlink/hardlink 逃逸、重复覆盖和超大解压；只允许批准的必要文件。配置只允许已验证安装相对路径或显式绝对路径，不引入 PATH/cwd 自动搜索。若提供引擎覆盖变量，名称、路径/版本/摘要校验及禁用条件在 F3 单独冻结。

## 10. `/orchestrate` 并行调度与审查

本轮已按现有 `.opencode/commands/orchestrate.md` 工作流实际发起四个只读 subagent：planner 统筹 DAG；architect 设计接口/模块；rust-tdd-guide 盘点测试与门禁；explore 盘点交付/配置/文档。此处不声称已启动迁移实现。

切换可编辑模式后，由集成负责人运行下一轮编排：

1. P0 确定 BASE、`/tmp/opencode/<run-id>`、受保护更改与每包 scope，保存合同 hash。
2. P1 三路并行得出 PASS/BLOCKED，主线程做决策；不可跨过失败门禁。
3. P2 单写共享合同/模块入口/清单，产生 CONTRACT_BASE。
4. P3 从 CONTRACT_BASE 创建独立 `tsgo-*` worktree；合并并通过 G3 后记录 FOUNDATION_BASE。P4 首批从 FOUNDATION_BASE 派生，后续从该批依赖已通过测试的 INTEGRATION_BASE 派生；任务书记录实际输入 commit，不跨过缺失依赖或在 worker 私自拼接共享接口。最多三个实现 worker、最多两个重编译，`CARGO_BUILD_JOBS=4` 为起点，实际按资源缩减。每个 worker 独立 `/tmp/opencode/<run-id>/<package>/target` 和日志。
5. 每个任务书包含：精确输入 commit/hash、只读依赖、唯一可写路径、不可修改路径、输出接口、待迁用例 ID、命令、结果格式和完成门禁。发现共享需求先交合同 owner，不能抢改。
6. agent 写完后立即 code-reviewer；Rust 语义由 rust-reviewer 复核；RPC/FS/包处理由 rust-security-reviewer 审查。reviewer 只审查其范围，由 owner 在**同 1 轮**内自动修复并跑针对性验证；不开启第 2 轮完整 review。
7. 集成负责人按依赖逐包合并、检查冲突和 dirty delta，重新运行相邻集成测试；不要将“每包分别通过”当作组合后通过。
8. P5～P7 集成主线负责，最后删除和发布候选验收串行。未授权发布或发送消息不在本次任务范围内。

每次交接只交：实现变更及基线、契约版本、迁移用例/断言、测试命令与结果、已知差异/风险。架构/测试/交付各维护一个权威产物，避免多个 agent 重复维护冲突的真相。

### 10.1 实施工作包及唯一写入范围

| 包 | 建议 agent / worktree | 前置 | 唯一写入范围 | 交接与验收 |
| --- | --- | --- | --- | --- |
| I 集成/合同 | 主线程 + planner；`tsgo-integration` | P0 | 共享清单/入口、`input.rs/backend.rs/config.rs/lib.rs`、LSP adapter、正式合同/ADR 索引 | CONTRACT_BASE、编译骨架、错误/epoch/资源合同 |
| N 原生客户端 | Rust 实现 agent；`tsgo-native` | F0/F1 | 新 crate `native/*` 与专属 codec/RPC/生命周期测试 | Rust 原型、真实引擎合同、恶意帧/句柄/重启结果 |
| S shadow | Rust 实现 agent；`tsgo-shadow` | 输入/AST 视图/mapper 合同 | `shadow/*` 与对应单元测试 | 原位布局/多 mount/loops/guards、正反映射及性质测试 |
| B 脚本语义 | Rust 实现 agent；`tsgo-script` | 原生 AST/checker + S 基础 | `script/{registration,behavior,properties}.rs`、`query/bindings.rs` 及专属测试 | Page/Component/Behavior/props 及 assignability 等价证据 |
| Q 查询呈现 | Rust 实现 agent；`tsgo-queries` | 原生会话 + shadow/script 合同 | `query/{hover,diagnostics,definition,completion}.rs` 及专属测试 | 类型/文档/重载/质量、精确范围、diagnostics 发布合同 |
| Y 符号与事件 | Rust 实现 agent；`tsgo-symbols` | script facts + mapper 合同 | `script/{symbols,events}.rs` 及专属测试 | setData/observer、expected_text、complete=false 拒绝编辑 |
| T 测试基础 | rust-tdd-guide；`tsgo-parity` | F2 | 共用 Rust fixture runner、账本校验器、独立差分集成测试 | 源测试/断言映射、comparator 故障注入；不抢改各包单元测试 |
| D 交付 | Rust 实现 agent；`tsgo-delivery` | F3 | `wxml-delivery/src/*` 与其 tests、资产 lock/获取校验工具 | schema、平台/摘要/许可、离线安装和原子升级/回滚 |
| C 构建/编辑器 | Rust 实现 agent；`tsgo-local-delivery` | D 包 schema/布局 + 环境合同 | `scripts/build-release.sh`、CI、Zed src/tests | no-node 构建/进程守卫、包检查、扩展入口；交付 README 由文档 owner 统一 |
| R 文档证据 | doc-updater；`tsgo-docs` | 各阶段实证 | 活跃 README/reference/evidence；ADR/索引变更交集成 owner | 历史与当前分离、provenance、命令可运行、证据无私有源码 |

本表是任务池，不是同时开启十个 agent。P3 优先 N/S/T；D 由空闲名额接续。N/S 合同可用后，P4 调度 B/Q/Y，Q 等待其所需 facts 已集成；D 完成 schema 后 C 才修改发布脚本。I/R 与 reviewers 主要只读汇总，不抢占三个实现名额；需要实质修改时同样登记 owner 和资源占用。

业务 owner 必须迁移本包测试并按用例 ID 更新交接记录，账本最终由 T 单写合并。新 crate 的 `mod` 导出由 I 更新；公共 AST visitor、文本单位、映射 DTO 由 I 指定 N/S 其中一名 owner，不能因位于不同 worktree 而重复发明。主线程对所有共享改动保持整合审阅，不把 worktree 隔离当作接口兼容证据。

### 10.2 可直接发给实施 agent 的任务模板

```text
目标：<单一工作包及用户可见行为>
基线：BASE=<commit>，CONTRACT_BASE=<commit>，FOUNDATION_BASE/INTEGRATION_BASE=<适用commit>，F0/F1/F2/F3=<hash>
工作区：<tsgo-* worktree>；临时/target/logs：/tmp/opencode/<run-id>/<包>/
可写：<本表唯一范围>；只读依赖：<文件/合同>；禁止触碰：<共享及用户变动路径>
输入：<fixture/case ID/dirty delta ID>；输出：<接口、行为、错误、完整性>
步骤：先写可编译且语义失败的 Rust RED，运行；最小 GREEN；差分；相邻回归；重构。
门禁：<本包命令 + 真实native要求>；缺失原生资产必须失败，不能 skip。
安全：不扩读权限、不写业务文件、不启用 external code、不跨 epoch 复用句柄。
交接：实现 diff/commit，迁移断言和结果，合同变化请求，未通过项与证据。
停点：发现契约不够或必须改共享文件时返回具体请求，由 owner 先更新合同。
```

## 11. 风险、停止条件与后续动作

| 风险 | 在何时消除或阻塞 |
| --- | --- |
| API 无法完整替代现有 LanguageService 查询 | P1 验证独立矩阵；缺失能力阻塞架构，不降规格 |
| AST/句柄/schema 变动 | P1 验证 + F0 固定；生成/校验工具用 Rust，升级重新跑协议合同 |
| OS FS 回退与写入口 | P1 访问轨迹/P2 安全评审；回调非沙箱，必要隔离单独明确 |
| 工作区修复落在被删除目录 | P0 delta + P4 映射 + P7 前核对；未迁移禁止删除 |
| 测试数量虚高或漏掉循环展开 | P1-B 实际展开盘点 + 断言账本 + comparator 反向测试 |
| 引擎升级掩盖结果改变 | 差异逐例评审，保留独立规范预期，不批量覆盖 oracle |
| 并行冲突/资源争用 | 单文件 owner、合同基线、独立 target、受限 worker/编译并发 |
| 发布 ABI/离线/旧 schema 回归 | P1-C/P6/P7 真实包验证；不扩大未经验证的平台宣称 |

当前状态（HEAD `899a89a`）：P6-8a 的默认 `--stdio` native、`NotInstalled` → HostOnly、损坏安装 fail-closed 和显式 `--backend node` 已由 `5020ae7` 提交，no-Node guard 的默认分支静态检查由 `7f9116a` 更新；全 workspace `cargo test` 在 `7f9116a` 的执行会话记录为 `CARGO_EXIT=0`。23 个旧 Node gate 的 native replacement 已全部 `gate_green`（0 required），但 replacement 仍 `#[ignore]`，不等于 `mandatory_green`；`899a89a` 的 3 个 native edit gate 尚未提升 ledger。下一步：P7-0 预删除冻结、P7-1 将 255 planned 归零并完成 gate review、F3-06/09 收口（F3-14 已由 `5020ae7` 默认路径关闭）；P7 删除（`backend_node`、旧 TS 树、v1 写路径）仍未开始，不得先删后补。详见 §12.11–§12.12。

## 12. 执行状态、证据与边界

正式计划与 ADR-0008 已提交到迁移 worktree 的 `b9616c5`。P0/P1 已执行；P2 契约已落盘；P3–P6 只完成下列切片。§12.1–§12.10 记录各自历史快照（§12.1–§12.3 为 `6358925`，§12.5 为 `0a245b9`，§12.6–§12.10 为 2026-11-20 gate 波次）。迁移未完成：默认 `--stdio` 已于 `5020ae7` 切到 native（`NotInstalled` 时 HostOnly），但 Node 实现保留为显式 `--backend node`，P7 删除未开始。

### 12.1 P0/P1 证据（历史）

P0/P1 的原始记录不变：

- `docs/evidence/2026-11-17-tsgo-workspace-baseline.md`：workspace/工具链/Backend 边界基线。
- `docs/evidence/2026-11-17-tsgo-migration-baseline.md`：Rust/TS 基线、339 个展开 TS 用例、perf 串行复跑。
- `docs/evidence/2026-11-17-tsgo-native-api-spike.md`：原生 `--api --async`、虚拟快照、查询、AST v9、UTF、句柄/写边界实测。
- `docs/evidence/2026-11-17-tsgo-delivery-spike.md`：无 Node 资产、安装/回滚状态机与平台事实。
- `docs/plans/2026-11-17-tsgo-test-ledger.md` 与 `tsgo-test-ledger.json`：逐用例迁移台账、fixture v1 hash、ignored/静默跳过映射。

G1 仍未关闭；7.1.0-beta 仅由维护者有条件接受并固定 integrity。P2 的 F0–F3 文档已落盘，但合同整体冻结、F3 验收与 P5 安全门禁均未关闭。

### 12.2 P2–P6 已提交切片

“证据层级”只说明该提交能证明什么；任何库/单元结果都不自动升级为 LSP adapter、CLI、真实包或 Zed 验收。下表只列 P6 入口以来需要记录的最新切片；P3/P4 的历史证据与逐用例状态仍以台账和 `docs/reference/tsgo-*` 为准。

| 提交 | 切片 | 已提交内容 | 证据层级 | 不能证明 |
| --- | --- | --- | --- | --- |
| `0e7c672` | P6-2 gate 转换计划 | 23 个 `#[ignore]` 的逐项处置、3 个 enable-now / 8 个 keep-gated / 12 个 delete-at-P7、2 个静默 return + 可选 `tsc` → 3 个强制原生 gate、S0–S9 | 计划（无运行） | 0/23 gate 绿；未删除旧测试、未修改 `#[ignore]`、未切换默认后端 |
| `2c6d39e` | P6-3 后端类型资产 | `AnalysisInput::with_environment`、33 个 compiler option key、`TypeAssets`/`TypeAssetsPin`、full VFS 资产表、116 stdlib + 20 typings `.d.ts`、environment tsconfig；`crates/wxml-backend-tsgo/tests/type_assets.rs` 10 个测试 | 库/单元 + opt-in 真实引擎；`type_assets` 真实门禁记录为 10/10 passed（含 2 个 opt-in 真实引擎用例；本轮未重跑） | adapter 仍调用 `AnalysisInput::new`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:121 ），`js_check` 仍为 `Inherit`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:202 ）；未证明 LSP/CLI/Zed 或真实包；可信 pin loader、F3-03、AC-004、TS 6.0.3 vs 7.1.0-beta 偏差仍开放 |
| `abf0046` | P6-4a 引擎发现/沙箱/opt-in | `engine.rs` 从安装树发现并校验 `manifest.json`/`engine/lib/tsc`；`sandbox.rs` 每 session 新建 `0700` 沙箱；`wxml-lsp --stdio --backend native`；默认 `--stdio` 仍构造 `NodeBackend` | CLI/库层 | P6-4b 真实资产启动、P6-4c 真实包/无 Node 进程审计、默认切换均未完成；fd/`openat` 沙箱加固仍 BLOCKED，残留路径校验到 spawn 的 TOCTOU |
| `31d6d29` | F1 hover 证据与 Markdown 组合 | 三段式 Markdown 顺序、checked-empty/`Unsupported`/`Partial` 区分、声明范围双坐标；`crates/wxml-backend-tsgo/tests/f1_hover.rs` 11 个测试 | 库/单元；2 个真实引擎测试为 opt-in，`WXML_REQUIRE_REAL_ENGINE=1` 才转硬门禁 | 无 `getQuickInfoAtPosition`；QuickInfo display parts、逐 overload 文档/分段仍 BLOCKED；未证明 adapter 端到端 hover/降级 |
| `6eb6f26` | Native document symbols DTO | `wxml-types` 新增 `DocumentSymbol*` DTO 与默认 `Backend::document_symbols`；`wxml-backend-tsgo` 解码 v9 document symbols，保留 UTF-16/UTF-8 范围与 gap/truncation | 库/单元 | adapter/LSP 路由未接线；脚本文档的 view/reverse lookup、snapshot 身份与 `DocumentRoute::Script` 仍是前置；WXML `structure.rs` outline 不变 |
| `6358925` | typings payload 身份强制 | 23 文件 allowlist、`2,548,075 B`、bundle 摘要 `ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b`、精确 payload 路径、入库的 5.3.1 fixture；manifest/verify/打包器重算并比较 | 交付库/单元（入库 fixture）；`typings_payload.rs` 为 opt-in 真实树检查 | 真实上游 tarball 的 SRI/SHA-256/size、provisioning、`assets.lock.json`、clean-env/ABI、包级验收未完成；F3-03/B6 仍 BLOCKED，不关闭任何 F3 ID |

### 12.3 当前门禁与下一步（2026-11-19 快照；默认切换后的状态见 §12.11）

当前门禁事实：

> **2026-11-19 后续（P6 回填，HEAD `9a8b65c`）**：trusted type-assets handoff 已由
> `caf045d` 接线，event completion 已由 `4371386` 落地，F3 root lock/package cache 已由
> `791bf33`/`e442f5d` 落地。台账已更新为 36 `green` / 267 `planned` / 36 retired；
> 3 个 `gate_green` 在带 `TSGO_TYPINGS_ROOT` 的 opt-in native job 复跑 3 passed，
> 其余 20 个 gate 仍 required。下列 P6-3/F3/台账条目保留为 2026-11-19 早先快照；
> 详情见 `docs/evidence/2026-11-19-tsgo-ledger-backfill.md`。默认 Node、默认 CI、
> `mandatory_green`、P6/G5/G6/F3 均未因此改变。

- **Gate 债**：23 个 `#[ignore]` 全部仍为 gate debt；`semantic_tools` 的 2 个静默 return 与可选 `tsc` 分支仍在。按 `0e7c672` 计划，它们只有在替代测试 `mandatory_green` 且 review 后才能删除/关闭。
- **P6-3**：backend 数据层已落，但 adapter 尚未接线资产、compilerOptions 或 `js_check`。可信 pin loader 与 `assets.lock.json` 缺失；AC-004 未保存更新旧诊断、TS 版本偏差、adapter 降级均未关闭。
- **P6-4**：仅 opt-in `--backend native` 与引擎发现/沙箱切片；P6-4b/c、真实包启动、无 Node 进程审计和默认切换均未完成。
- **F1 hover / document symbols**：hover 只有 backend 组合与 opt-in 真实引擎证据；QuickInfo/overload 文档缺口 BLOCKED。document symbols 只有 DTO/decoder，adapter 与 LSP 路由未接线。
- **F3**（2026-11-19 快照；2026-11-20 审计见 §12.7）：F3-01～F3-16 无一项关闭；F3-02、F3-03 明确 BLOCKED，B6 仍 BLOCKED。`6358925` 固定的是 payload 身份，不是上游 tarball/provisioning/lock 身份。
- **台账**：33 `green` / 270 `planned` / 36 `retired-with-decision`；本轮没有新增 `green`、`reviewed` 或 `closed`，不得把计划中的 case 标记为通过。余下 270 个 planned 的排期见 [2026-11-19 剩余台账计划](2026-11-19-tsgo-remaining-ledger-plan.md)；该计划本身不提升任何 case 状态。
- **默认后端与平台**：默认仍是 Node，Node 未删除；G5/G6 未关闭；原生后端只有 Linux x86_64 证据，aarch64/macOS/Windows/musl 未验证，也没有 Zed 原生冒烟。

下一步按依赖顺序：

1. **P6-3 adapter handoff**：`VerifiedInstall` → `TypeAssets` → `AnalysisInput::with_environment`；透传 compilerOptions，修正 `js_check`，接入 adapter diagnostics/hover；补 loader 的缺失/篡改/symlink/预算测试。
2. **F3-03 可信资产链**：真实 typings tarball 的 SRI/SHA-256/size、provisioning、`assets.lock.json`，再由打包器/安装器交叉校验；构建真实 v2 包并跑 clean-env/ABI 验收。
3. **P6-4 残余**：fd/`openat` 沙箱加固；真实资产启动（P6-4b）；真实包 + `strace`/PATH tripwire 的无 Node 进程审计（P6-4c）。4a 的 opt-in 结果不能替代 4c。
4. **F1 查询缺口**：按合同决定 QuickInfo display parts/逐 overload 文档的实现或显式 `Partial`；不得伪造缺失能力。
5. **Document symbols 路由**：补脚本 → view 反向查找与 snapshot 身份，接 adapter/`DocumentRoute::Script`；WXML outline 保持静态。
6. **P6-2 gate 转换**：按 S0–S8 逐项落地；23 个 `#[ignore]` 在替代测试 `mandatory_green` 前不得移除；2 个静默 return + 可选 `tsc` 必须变成 3 个强制原生 gate，并加负向 fail-closed 测试。
7. **P6-8/P7**：只有在 G5/G6 关闭、真实包/安装/回滚/Zed 验收完成后，才切换默认后端和删除 Node；本轮不执行。

### 12.4 明确不声称（§12.1–§12.3 快照；默认切换后的边界见 §12.11）

- 不声称任何 F3 验收 ID 已关闭（2026-11-19 快照；2026-11-20 的 8 closed / 6 partial / 2 blocked 见 §12.7）；`6358925` 只固定 typings payload 身份，F3-03/B6 当时仍 BLOCKED。
- 不声称 P6-3、P6-4、G5 或 G6 已关闭；不声称 23 个 ignored gate 或 2 个静默 return 已迁移。
- 不声称默认后端已切换或 Node 已删除。
- 不把库/单元测试、入库 fixture 或 opt-in 真实引擎测试当作真实包、CLI 或 Zed 验收。
- 不把 `planned` ledger case 标记为 `green`、`reviewed` 或 `closed`。
- 不声称 Linux x86_64 之外的平台支持，也不声称原生 Zed 冒烟已完成。

### 12.5 2026-11-19 增量（HEAD `0a245b9`）

本节只追加 `6358925` 之后的六个提交和对应运行事实；§12.1–§12.3 保留原快照，§12.4 的不声称边界继续适用。迁移未完成，默认后端仍是 Node。

#### 12.5.1 新增已提交切片

| 提交 | 切片 | 已提交内容 | 证据层级 | 不能证明 |
| --- | --- | --- | --- | --- |
| `e303e1d` | native hover evidence 加固 | `native/session.rs`、`query/hover.rs`、`query/type_info.rs`、`query/symbol.rs`、`native/handles.rs`；`f1_hover.rs` 断言加强 | 代码切片；未单独运行 | 单独不证明真实引擎 hover；QuickInfo/overload 文档仍缺 |
| `1e35fe8` | `Partial` capability 分发 | `handlers/mod.rs` 新增 `backend_available`，hover/definition 不再把 `Partial` 当 unavailable；新增 574 行 capability gate 测试 | lib 单元（计入 668） | 只证明 handler 分发；不证明真实引擎、stdio 或能力合同变化 |
| `a032a1f` | trusted type-asset loader | 新增 `asset_loader.rs`；`input.rs` 增加 `TypeAssets`/`TypeAssetsPin`/`with_environment`；新增 loader 测试 | 库/单元 | adapter 生产请求未接线；无真实 tarball/`assets.lock.json`；Windows/UNC 未验证 |
| `953ee1e` | adapter 消费 native hover evidence | `backend_tsgo/hover.rs` 改为消费单个 `HoverEvidence` 并发布后端组合 Markdown；sandbox/session 配套；`tsgo-native-contract.md` 补状态合成规则 | lib + opt-in 真实引擎 smoke | 不是 LSP stdio/CLI/Zed；未加载 miniprogram typings；completion/definition 未覆盖 |
| `ca0b554` | loader 收敛 | `asset_loader.rs` 重构与加固；`input.rs` 调整；loader 测试补充 | 库/单元 + loader 运行（见 §12.5.2） | 未完成生产 handoff；不关闭 F3-03/B6 |
| `0a245b9` | hover/gate evidence 文档 | 新增 `docs/evidence/2026-11-19-tsgo-hover-gates.md` 并在 evidence README 建索引 | 文档（E0） | 只记录既有运行，不新增运行事实 |

#### 12.5.2 运行事实

| 范围 | 结果 | 边界 |
| --- | --- | --- |
| `cargo test -p wxml-lsp --lib` | 668 passed / 0 failed（在 `953ee1e` 工作树记录） | 仅 lib 单元；包含无 `TSGO_BIN` 时直接返回的 opt-in smoke，不能证明真实引擎 |
| 真实引擎 adapter hover smoke | 1 passed / 0 failed | pinned 7.1.0-beta、`WXML_REQUIRE_REAL_ENGINE=1`；lib 级 adapter，不是 stdio/CLI |
| 首波 native gate `G-P5-E2E-09/12/14` | 3 passed / 0 failed / 36 filtered | 仍 `#[ignore]`；`36 filtered` 未运行；当轮 ledger 仍 `required_not_migrated`（后续 2026-11-19 提升见 §12.6） |
| loader 验证 | `asset_loader` 19 passed / 4 ignored；显式 fixture gate 4 passed；`wxml-backend-tsgo` 542 passed / 4 ignored；clippy 无问题 | 执行会话记录，本次未重跑，stdout 未入库；clippy 范围不外推为 workspace 全绿 |

上述结果只在各自范围内成立，均为执行会话记录；本次状态更新未重跑 `cargo`，原始 stdout 未入库，不合并为 workspace、CLI、真实包或 Zed 全绿。

#### 12.5.3 当前边界与下一步

- **默认后端**：默认 `--stdio` 仍构造 `NodeBackend`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/main.rs:184 ），Node 未删除；`--backend native` 仍是 opt-in。F3、P6、G5、G6 均未关闭。
- **P6-3-4 trusted pin**：生产接线仍 BLOCKED；adapter 的 `RequestPlan::input` 仍调用 `AnalysisInput::new`（ /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:121 ），未消费 trusted loader。`assets.lock.json`、真实 tarball 身份与 F3-03/B6 仍未闭环。
- **台账**：本轮 gate 运行未自动晋级 ledger；`G-P5-E2E-09/12/14` 仍为 `required_not_migrated`，关联 case 仍为 `planned`，没有新增 case 级 `green`。（`0a245b9` 快照；2026-11-19 的 3/20/23 台账提升见 §12.6。）
- **document symbols**：`27bec99`（`e303e1d` 的父提交）已把脚本 document symbols 接入 LSP；§12.3 的“adapter 与 LSP 路由未接线”是 `6358925` 快照。当前仍有未提交的 scheduler/HIGH 修复，未完成、未提交。
- **未提交 WIP**：记录时工作树包含 document-symbol scheduler/HIGH 修复（`scheduler.rs`、`server.rs`、`structure.rs`、`backend_tsgo/document_symbol.rs` 等）和 F3 asset-lock L1 review-fix（`crates/wxml-delivery/src/assets_lock*`、`verify_v2.rs` 等）。两者都不在 HEAD `0a245b9`，不得写成已完成或已提交。

下一步按依赖顺序：

1. **P6-3 production asset handoff**：完成设计/实现，`VerifiedInstall` → trusted `TypeAssets` → `AnalysisInput::with_environment`，透传 compilerOptions、修正 `js_check`，再接入 adapter diagnostics/hover。
2. **document-symbol fix review**：审查未提交的 scheduler/HIGH 修复，必要时补测试后再提交。
3. **asset-lock L1 fix review/commit**：审查并提交 F3 asset-lock L1 修复。
4. **F3-03 L2**：在 L1 提交后继续真实 tarball 的 SRI/SHA-256/size、provisioning、`assets.lock.json` 与打包器/安装器交叉校验；不据此关闭 F3-03。
5. **P6-4b/4c**：真实资产启动；真实包 + `strace`/PATH tripwire 的无 Node 进程审计。
6. **P6/P7 默认切换与 Node 删除**：最后执行；前置是 G5/G6 与真实包/安装/回滚/Zed 验收完成。

### 12.6 2026-11-19 native gate 台账提升（P6-2）

本节记录 §12.5 之后的状态变化；§12.5 仍是 `0a245b9` 快照，其中“ledger 仍 `required_not_migrated`”只描述当轮。本次未改生产代码、Node、默认后端、`Cargo.toml` 或任何 `#[ignore]` 属性，也未删除 gate。

| 项 | 状态 |
| --- | --- |
| `gate_green` | 3：`G-P5-E2E-09`、`G-P5-E2E-12`、`G-P5-E2E-14` |
| `required_not_migrated` | 20 |
| 合计 | 23（不增不减） |
| 运行证据 | `3 passed / 0 failed / 36 filtered`；运行锚点 `953ee1e`；台账记录锚点 `7bdab96` |
| fail-closed RED | `0 passed / 3 failed / 36 filtered`（不带 `WXML_TEST_NATIVE=1`，各 gate panic，不是 skip） |
| 触发命令 | `WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine cargo test -p wxml-lsp --test native_parity native_engine_real -- --ignored --test-threads=1` |
| 引擎身份 | TypeScript `7.1.0-beta`，AST v9，`lib/tsc` SHA-256 `a183b195…`，size 24,522,912 B |
| 资产身份 | `TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine` 的 `engine/lib` 116 个 `lib*.d.ts`；未使用 miniprogram typings payload |
| case 计数 | 仍为 339 total / 33 `green` / 270 `planned` / 36 `retired-with-decision`；未新增 case 级 `green` |

边界：

- 3 个 replacement 仍 `#[ignore]`；默认 `cargo test` 不运行它们，`gate_green` 是 opt-in required native job 状态，不是默认 CI 绿，也不是 `mandatory_green`。
- 其余 20 个 gate 仍未验证；P6-3/P6-4、G5、G6、F3 均未关闭；默认后端仍是 Node。
- 本次复跑被并行未提交的 trusted type-assets WIP 阻塞（native gate 启动报 `native type assets are unavailable: a required type declaration is missing`），因此绿色结论仍只锚定 `953ee1e` 的既有运行，不是当前工作树结果。
- 详细证据与限制见 /home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/evidence/2026-11-19-tsgo-native-gate-status.md 。

### 12.7 2026-11-20 F3 逐项验收审计

本节只记录 F3 的验收状态，不改变 P6/G5/G6，也不改变默认后端。权威表和关闭条件见
` docs/reference/tsgo-delivery-contract.md` §9.5；证据索引见
` docs/evidence/README.md` §3.3。

| 结果 | ID | 说明 |
| --- | --- | --- |
| **closed（8）** | F3-01、F3-03、F3-04、F3-05、F3-07、F3-08、F3-10、F3-13 | 真实包/CLI、两次构建同哈希、安装/回滚、ABI 和篡改/恶意归档已覆盖合同子条件；fixture/单点变异已标注 |
| **partial（6）** | F3-06、F3-09、F3-11、F3-12、F3-15、F3-16 | 有 PASS 证据，但进程级 kill/真实 legacy 包、恶意归档形态、缓存 upgrade/rollback、Zed 变量转发、`activated` 幂等仍缺 |
| **blocked（2）** | F3-02、F3-14 | 无 `strace`/等价 `execve` 审计；无 Zed UI/查询证据且默认 `--stdio` 仍 Node |

F3 整体仍未通过：`partial` 不是“没有证据”，而是合同子条件未全部满足，不能按 PASS 字样关闭。
本审计不修改历史证据、不 commit、不删除 Node、不切换默认后端。

### 12.8 2026-11-20 native gate 台账提升（P6-2 第三波）

本节记录 `814ca5e` 之后、尚未提交的 gate 台账切片；§12.6/§12.7 的历史快照不变。
本次未改生产代码、Node、默认后端、`Cargo.toml` 或任何 `#[ignore]` 属性，也未删除 gate。

| 项 | 状态 |
| --- | --- |
| `gate_green` | 16：原 13 个 + `G-P5-E2E-03`、`G-P5-E2E-11`、`G-P5-E2E-16` |
| `required_not_migrated` | 7：`G-P5-PERF-01`、`G-P5-STABILITY-01`、`G-P5-E2E-01`、`G-P5-E2E-02`、`G-P5-E2E-05`、`G-P5-E2E-20`、`G-P5-E2E-21` |
| 合计 | 23（不增不减） |
| 运行证据 | `16 passed / 0 failed / 36 filtered`；运行锚点 `814ca5e` |
| fail-closed RED | `0 passed / 16 failed / 36 filtered`（不带 `WXML_TEST_NATIVE=1`，各 gate panic，不是 skip） |
| 触发命令 | `WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine TSGO_TYPINGS_ROOT=/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/miniprogram-api-typings cargo test -p wxml-lsp --test native_parity native_engine_real -- --ignored --test-threads=1` |
| 引擎身份 | TypeScript `7.1.0-beta`，AST v9，`lib/tsc` SHA-256 `a183b195…`，size 24,522,912 B |
| 资产身份 | `engine/lib` 116 个 `lib*.d.ts`；`miniprogram-api-typings` 5.3.1（26 文件 / 20 个 `.d.ts`） |
| 默认校验 | `cargo test -p wxml-lsp --test native_parity`：36 passed / 0 failed / 16 ignored |
| case 计数 | 仍为 339 total / 36 `green` / 267 `planned` / 36 `retired-with-decision`；未新增 case 级 `green` |

新增绿色 gate 的依据：`814ca5e` 的 root/trailing-dot completion，`393ff0d` 的
`optionalTypes` 联合与 guard 精确范围，`0ca4375`/`647dd8d` 的本地 event diagnostics/definition。
剩余 7 个 gate 的精确阻塞（event hover 缺失、component property binding 无诊断、
non-literal 页面 `Unavailable` 与旧 `Unsupported` 不同、symbols unsupported、无 native
perf/stability/restart 基线）见
` docs/evidence/2026-11-20-tsgo-p6-native-gates-wave3.md` §4。

边界：16 个 replacement 仍 `#[ignore]`；默认 `cargo test` 不运行它们，`gate_green`
不是默认 CI 绿，也不是 `mandatory_green`；P6/P7、G5/G6/F3 未关闭，默认后端仍是 Node。

### 12.9 2026-11-20 native gate 台账提升（P6-2 第四波）

本节记录 `8ccebbf` 之后、尚未提交的 gate 台账切片；§12.6/§12.7/§12.8 的历史快照不变。
本次未改生产代码、Node、默认后端、`Cargo.toml` 或任何 `#[ignore]` 属性，也未删除 gate。

| 项 | 状态 |
| --- | --- |
| `gate_green` | 22：原 18 个 + `G-P5-E2E-01`、`G-P5-E2E-02`、`G-P5-E2E-05`、`G-P5-E2E-21` |
| `required_not_migrated` | 1：`G-P5-E2E-20`（组件属性绑定诊断未实现） |
| 合计 | 23（不增不减） |
| 运行证据 | `22 passed / 0 failed / 36 filtered`（`--release`，含 PERF）；运行锚点 `8ccebbf`；57.88 s |
| fail-closed RED | `0 passed / 22 failed / 36 filtered`（不带 `WXML_TEST_NATIVE=1`，22 个 gate 全部 panic，不是 skip） |
| 触发命令 | `WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine TSGO_TYPINGS_ROOT=/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/miniprogram-api-typings cargo test -p wxml-lsp --test native_parity --release native_engine_real -- --ignored --test-threads=1` |
| 引擎身份 | TypeScript `7.1.0-beta`，AST v9，`lib/tsc` SHA-256 `a183b195…`，size 24,522,912 B |
| 资产身份 | `engine/lib` 116 个 `lib*.d.ts`；`miniprogram-api-typings` 5.3.1（26 文件 / 20 个 `.d.ts`） |
| 默认校验 | `cargo test -p wxml-lsp --test native_parity`：36 passed / 0 failed / 22 ignored |
| case 计数 | 仍为 339 total / 36 `green` / 267 `planned` / 36 `retired-with-decision`；未新增 case 级 `green` |

新增绿色 gate 的依据：`304aa20` 的 non-owner overlay replay（内容指纹 + 一次死会话重试）与
Rust script symbols，`3220362` 的 event-handler hover，`4fd45bd` 的非字面量注册
`Unsupported` 分类。E2E-21 的 `Partial`/`Unsupported` 在 LSP 上以唯一一条零宽状态
warning 出现；gate 断言零 error + 对应 warning，不把两者混同。

边界：22 个 replacement 仍 `#[ignore]`；默认 `cargo test` 不运行它们，`gate_green`
不是默认 CI 绿，也不是 `mandatory_green`；P6/P7、G5/G6/F3 未关闭，默认后端仍是 Node。

### 12.10 2026-11-20 native gate 台账提升（P6-2 第五波，gate 全部关闭）

本节记录 `04fa0fe` 之后、尚未提交的 gate 台账切片；§12.6–§12.9 的历史快照不变。
本次未改生产代码、Node、默认后端、`Cargo.toml` 或任何 `#[ignore]` 属性，也未删除 gate。

| 项 | 状态 |
| --- | --- |
| `gate_green` | 23：原 22 个 + `G-P5-E2E-20` |
| `required_not_migrated` | 0 |
| 合计 | 23（不增不减） |
| 运行证据 | `23 passed / 0 failed / 36 filtered`（`--release`，含 PERF）；运行锚点 `04fa0fe`；65.84 s |
| fail-closed RED | `0 passed / 23 failed / 36 filtered`（不带 `WXML_TEST_NATIVE=1`，23 个 gate 全部 panic，不是 skip） |
| 触发命令 | `WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine TSGO_TYPINGS_ROOT=/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/miniprogram-api-typings cargo test -p wxml-lsp --test native_parity --release native_engine_real -- --ignored --test-threads=1` |
| 引擎身份 | TypeScript `7.1.0-beta`，AST v9，`lib/tsc` SHA-256 `a183b195…`，size 24,522,912 B |
| 资产身份 | `engine/lib` 116 个 `lib*.d.ts`；`miniprogram-api-typings` 5.3.1（26 文件 / 20 个 `.d.ts`） |
| 默认校验 | `cargo test -p wxml-lsp --test native_parity`：36 passed / 0 failed / 23 ignored |
| case 计数 | 仍为 339 total / 36 `green` / 267 `planned` / 36 `retired-with-decision`；未新增 case 级 `green` |

新增绿色 gate 的依据：`04fa0fe` 的组件属性绑定诊断（shadow 表达式范围 + `type_query`，
复杂类型保持 `Partial`，事件属性排除）。`G-P5-E2E-20` 的 native 测试
`native_ac012_component_property_bindings` 复用 `component-bindings` fixture，严格断言
2 个 hover（`pageTitle` `string` + `页面标题`、`total` `number` + `卡片总数`）、1 个属性
error（`title="{{ total }}"` 的 `total` 范围，message 含 `number`）、1 个事件 error
（`missingHandler`）、2 个事件 definition（`onCardTap` 精确声明 token、缺失 handler 为
checked `null`）；该 fixture 完全可解析，页面不得出现状态 warning。

边界：23 个 replacement 仍 `#[ignore]`；默认 `cargo test` 不运行它们，`gate_green`
不是默认 CI 绿，也不是 `mandatory_green`；两个 `semantic_tools` 静默 return、267 个
planned case、P6/P7、G5/G6/F3 均未关闭，默认后端仍是 Node。

### 12.11 2026-11-20 P6-8a 默认 native 与 HostOnly（`5020ae7`、`7f9116a`）

本节记录 `04fa0fe`/`2f072a6` gate 波次之后的默认入口切换；§12.6–§12.10 的历史快照不变。
本次提交没有删除 Node、没有修改 Cargo、没有重建 release 包，也没有把任何 gate 提升为
`mandatory_green`。

| 项 | 已提交事实 | 证据/边界 |
| --- | --- | --- |
| 默认选择器 | `--stdio` 无 `--backend` 映射到 `Action::Default` → `serve_default()` → `run_stdio_native_default()`；`--backend native` 仍是严格 `serve_native()`；`--backend node` 映射到 `serve_node()` | ` crates/wxml-lsp/src/main.rs:100`、`:176`、`:200`、`:206`、`:220` |
| `NotInstalled` | 只有 `EngineDiscoveryError::NotInstalled` 降级到 `run_stdio_host_only()`；HostOnly 不发现引擎、不加载类型资产、不创建 sandbox、不读 `WXML_NODE`/`WXML_TYPE_BACKEND`/`WXML_BACKEND_DIR` | ` crates/wxml-lsp/src/backend_tsgo/mod.rs:208`、`:235`、`:342`；` crates/wxml-lsp/tests/cli.rs:109` |
| 损坏安装 | manifest 缺失/非法 JSON、引擎缺失、引擎摘要不匹配和资产错误均在 LSP handshake 前 fail closed，不降级 HostOnly 或 Node | ` crates/wxml-lsp/tests/cli.rs:259`、`:294`、`:311`、`:319`；严格 native 的回退拒绝见 `:77` |
| Node 过渡 | `--backend node` 保留并标记 deprecated；`serve_node()` 仍是唯一构造 `NodeBackend` 的生产路径 | ` crates/wxml-lsp/src/main.rs:206`；` crates/wxml-delivery/tests/no_node_guard.rs:477` |
| HostOnly 能力 | 模板语法/格式化/结构/事件 facts/script symbol facts 可用；类型查询 `Unavailable`（reason `native engine is not installed`），capabilities 为 `Partial`，不伪造空成功 | ` crates/wxml-lsp/src/backend_tsgo/mod.rs:151`、`:1084`；` crates/wxml-lsp/tests/cli.rs:109` 的 hover/status 断言 |
| no-Node guard | `7f9116a` 将 `lsp_native_status()` 改为检查 `serve_default()` 不含 `NodeBackend::new`，且 `serve_node()` 仍保留；`WXML_REQUIRE_LSP_NATIVE=1` 可得到静态 PASS | ` crates/wxml-delivery/tests/no_node_guard.rs:477`、`:524`、`:1003`；只是源码检查，不运行 live LSP，不关闭 F3-02 的 `execve` 审计 |

`--help` 同步改为 `native (default)`，Node 环境变量说明只保留在 `--backend node` 段
（` crates/wxml-lsp/src/main.rs:25`、`:36`）。

运行事实：

| 验证 | 结果 | 范围与边界 |
| --- | --- | --- |
| 全 workspace `cargo test` @ `7f9116a` | `CARGO_EXIT=0` | 执行会话记录；本轮未重跑，stdout 未入库；该运行早于 `899a89a` 的 edit gates，不等于默认 CI 或 G6 |
| `cargo test -p wxml-lsp --test native_parity` @ `899a89a` | 37 passed / 0 failed / 26 ignored | 默认套件；26 ignored = 23 个旧 gate replacement + 3 个 native edit gate |
| required native job（23 gate，`--release`） | 23 passed / 0 failed / 36 filtered，65.84 s；fail-closed RED 0 passed / 23 failed | 运行锚点 `04fa0fe`，台账提交 `2f072a6`；`--ignored` opt-in，不是默认 CI |
| required native edit job | 3 passed / 0 failed / 60 filtered | 提交 `899a89a`（运行记录 validation HEAD `7f9116a`）；见 §12.12 |
| ledger @ `0bf666b` | 44 `green` / 259 `planned` / 36 `retired-with-decision`，339 总数 | case 轴；gate 变绿不自动提升 case，`gate_green` 也不是 `mandatory_green` |

边界：

- 23 个旧 Node gate 的 native replacement 已全部 `gate_green`、`required_not_migrated` 为 0；
  它们仍全部带 `#[ignore]`，只有 opt-in required native job 证据，不能当作默认 CI 绿。
- 默认切换只在源码层提交；F3-14 使用的 P6-7 包构建于 `8ccebbf`，其 `--help` 仍写
  `node (default)`；`b095258` 的已提交证据没有用 P6-8a 重新构建包或录制默认
  `--stdio` 的 Zed UI 证据。
- F3 的已提交证据仍为 13 closed / 2 partial / 1 blocked（F3-06/09 partial，F3-14 blocked；
  §12.7 的 8/6/2 是 2026-11-20 早先审计快照，后续验收以 F3 合同 §9.5 为准）；
  G5/G6/G7 未关闭；Node 实现、旧 TS 测试和 `semantic_tools` 默认 Node 路径均未删除。
- HostOnly 不是“原生引擎验证通过”；平台证据仍只有 Linux x86_64。

F3 相邻提交（不改变 13/2/1）：`28c5e18` 已提交 F3-06/09 的 test-only ptrace +
外部 `SIGKILL` 精确窗口，F3-06 的 U1/U3/U5/U6 与 F3-09 的 L3/L4 均 PASS，但
F3-06 仍缺真实第二版本、F3-09 仍缺真实 legacy 发布包，故保持 partial；
`b095258` 的 F3-14 opt-in wrapper Zed UI 冒烟 16/16 PASS，但包构建于 `8ccebbf`
且显式追加 `--backend native`，默认路径包冒烟未录制，F3-14 仍 blocked。

### 12.12 2026-11-20 P7 native edit gates 与删除状态（`899a89a`）

本节记录 P7 相关 surface 的 native replacement；§12.6–§12.11 的 gate/默认切换快照不变。
本次没有删除旧测试、没有提升 ledger gate、没有开始 P7-0..P7-7 的任何删除。

| 项 | 状态 |
| --- | --- |
| `G-P5-E2E-22` | `native_component_property_edit_applies_and_reparses` 在 `component-bindings` fixture 上重命名组件属性 `title`：三个 `documentChanges`（`card.ts` 声明、`card.wxml` 的 `{{ title }}`、页面两个 `title=`），断言精确范围/`newText`、应用后模板 reparse、页面仍发布冻结的两个 error 且属性错误改指 `heading`，证明 native 检查使用应用后的 overlay；磁盘文件不变 |
| `G-P5-E2E-22`（post-edit） | `assert_native_edit_reparses_and_typechecks` 是独立 gate：构造精确 `count -> total` 编辑并应用后要求 native script check 清洁，再用真实 `string`/`number` mismatch 正控制产生真实错误；无 `tsc` 分支、无 Node 子进程 |
| `G-P5-E2E-23` | `native_rejects_cross_namespace_and_dynamic_edits` 覆盖 6 个拒绝场景（组件 property↔data、跨 script `setData`、解构 shorthand、动态 `bindtap`、非字面量 `Page(options)` 等），每个必须 `-32803` 拒绝或 `Partial` 且零编辑，源文件逐字节不变；正控制先做真实 `alpha -> beta` 重命名 |
| 冻结清单 | `MIGRATED_CASES` 为上述 3 个测试，`BLOCKED_NATIVE_GATES` 为空；`native_gates` 分类为 23 个旧 gate + 23 个 native engine gate + 3 个 native edit gate = 49 个 `#[ignore]` |
| 运行 | required edit job 3 passed / 0 failed / 60 filtered，12.83 s；缺失 package 时每个 gate 单独 0 passed / 1 failed / exit 101；`semantic_tools real_node_lsp_` 在 native required 模式下 2 passed / 0 failed / 39 filtered |
| 旧路径 | `semantic_tools` 的默认行为未变：无 native flag 时仍打印 skip 并 `return`，可选 `tsc` 分支仍打印 skip；设置 `WXML_TEST_NATIVE=1`/`WXML_REQUIRE_REAL_ENGINE=1` 后缺 package、缺 `server.js` 或缺 `tsc` 改为 panic |

证据锚点：` crates/wxml-lsp/tests/native_parity/symbols_edits.rs:33`、`:44`、`:51`；
` crates/wxml-lsp/tests/semantic_tools.rs:2086`、`:2215`、`:2589`；
` crates/wxml-lsp/tests/common/native_edit.rs`；运行记录见
[2026-11-20 P6-2 native edit gates](../evidence/2026-11-20-tsgo-p6-edit-gates.md)。

仍未完成（BLOCKED）：

- ledger 的 `silent_skip_gates` `G-P5-E2E-22`/`G-P5-E2E-23` 仍是
  `required_not_migrated`；3 个 edit gate 没有 promotion/review，不是
  `mandatory_green`。
- 23 个旧 gate 仍全部 `#[ignore]`，`semantic_tools` 的两个默认 Node 路径和可选
  `tsc` 分支仍在，旧 `real_node`/`wave5`/`stdio_type_loop` 测试未 retarget 或删除。
- ledger 为 48 `green` / 255 `planned` / 36 `retired-with-decision`（wave-6 组件绑定跟进后）；P7-1 归零未开始。
- P7-0 预删除冻结未开始：没有 `p7-pre-delete` tag，没有
  `docs/evidence/data/legacy-{node,ts}-tests.json`；`ledger.rs` 和
  `fixture_hash.rs` 仍读取 live `backends/typescript/test` 树
  （` crates/wxml-lsp/tests/native_parity/ledger.rs:1435`、
  ` crates/wxml-lsp/tests/native_parity/fixture_hash.rs:50`）。
- P7-3..P7-7 没有任何删除提交；`backend_node`、旧 TS 源码/测试、v1 packager 写路径都还在。
