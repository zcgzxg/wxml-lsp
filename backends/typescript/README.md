# TypeScript 类型桥后端（P5/P6）

本目录是 [C1 类型桥契约](../../docs/reference/type-bridge-c1.md) 的 P5/P6 Node 类型后端。
它用固定的 TypeScript Language Service 在内存派生视图里执行 C1 请求，并保留两种候选的对照 harness：

| 候选 | 做法 | 结论 |
| --- | --- | --- |
| A：原位 shadow | 在原脚本 options 内插入探针成员，片段用 `this.data` 作为根 | **唯一布局**，写入 C1 契约 |
| B：符号桥 + 独立检查文件 | 在注册点绑定 options、导出根锚点，片段放在生成的 `.ts` 检查文件 | 淘汰（改写 `this`、闭包不可锚定、模块化组件失效） |

两种候选都不写业务文件：脚本与检查文件只存在于内存文件系统里（`src/compiler.ts`），
`samples/**` 始终不被修改，探针不执行任何用户代码。

## 环境

- Node `>=24`（原生 TypeScript 类型擦除直接运行 `.ts`，无编译产物）
- `typescript@6.0.3`（固定，运行时依赖，JS 语言服务实现）
- `miniprogram-api-typings@5.3.1`（固定，运行时依赖，真实 `Page`/`Component` 类型）
- `@types/node@24.10.1`（仅构建/测试，devDependency）

运行时依赖为 `typescript` 与 `miniprogram-api-typings`（`npm ci --omit=dev` 后 `dist/server.js` 可启动）；锁文件在 `package-lock.json`。仓库不提交 `node_modules`/`dist`/`*.tsbuildinfo`。

## 命令

```bash
npm ci             # 安装锁定依赖
npm run check      # tsc --noEmit
npm test           # node --test：208 条断言
npm run build      # tsc -p tsconfig.build.json → dist/server.js
npm start          # node dist/server.js（NDJSON JSON-RPC over stdio）
npm run matrix     # 33 场景 × 2 候选的对照矩阵（Markdown）
npm run --silent matrix:json
npm run evidence   # 刷新 docs/evidence/data/type-bridge-c1/{matrix,perf}.json（先校验目录在仓库内，再脱敏）
npm run --silent perf:json
npm run inspect -- component-properties in-place-shadow   # 派生文件 / 插入 / 挂载 / 原始诊断
```

`matrix`/`perf` 只写 stdout；`evidence` 是唯一会写仓库的脚本，输出固定为证据数据目录。

## 请求 / 响应模型

`src/protocol.ts` 固化 C1 请求与响应，`src/run.ts` 用同一模型执行：

- 请求：`protocolVersion`、`requestId`、`analysisStamp`、`view{viewId,owner}`、`project{projectRoot,compilerOptions,typings}`、
  `scriptFileId`、`files[]`、`fragments[]`（Rust 侧输入，C1.1 支持可选 `target`/`locals`）、`policy{jsCheck,propertiesOverlay}`、`limits`。
- 请求身份：`files[].fileId` 必须等于项目相对的 `files[].path`（同一身份空间，拒绝绝对路径/`..`/NUL），`view.owner.fileId` 必须等于 `scriptFileId`；响应里的 `insertions[].file`/`mounts[].file`/定义 `fileId` 都复用该 id，生成文件用 synthetic id。
- `fragments[].wxmlRange.end - start` 必须精确等于 `exprText` 的 UTF-8 字节长度；`target` 省略时取表达式最右侧值标识符的**原始偏移**（`a.a` 指向第二个 `a`，字符串/注释内不算）。
- 表达式必须精确解析为单个 `const __wxml_expr = (…);` 包装；多余语句、括号逃逸、`satisfies`/TypeNode/`new` 表达式 → `bad-fragment`。表达式遍历深度 >256 或节点 >10,000 同样 `bad-fragment`，不会以 `RangeError`/`-32603` 结束。
- 错误消息中的表达式截断到 120 字符并附总长度；`locals` 可以覆盖 `Math` 等全局内建名。
- 挂载时重写**每个自由根标识符**（`!title`、`(title)`、`title + user.name`、`users[0].name` 等），跳过成员名、对象 key、字符串/模板文本、注释、关键字/字面量、全局内建和 `locals`；对象简写 `{ title }` 展开为 `{ title: __wxml_root.title }`，无法安全挂载的形状返回 `bad-fragment`。
- 响应：`jsCheckApplied`、`registration{kind,count,topLevel}`、`states{hover,diagnostics,definition}`、
  `insertions[]`（含 `replaceLengthUtf16`）、`mounts[]`（含 `targetLengthUtf16`）、逐片段结果（诊断只发布 WXML 字节 `range`，定义发布 `fileId + range`；`insertions[].file`/`mounts[].file` 与 `files[].fileId` 同一身份空间，生成文件用 synthetic fileId；`fragmentId` 按请求回显，`mountId = fragmentId#n`）、`suppressed`、`moduleKind`；`run.ts` 的 `toAnalyzeResponse` 负责组装。
- `analyze(request)` 是生产入口：只读请求中的 `files[]`，不读项目源文件/tsconfig；缺少脚本 → `Unavailable`。
- `runRequest(request, ...)` 是证据 harness：在 `analyze` 之上附加场景期望；挂载集合来自 `request.fragments`，检查策略来自 `request.policy`。
- `src/server.ts`：NDJSON JSON-RPC 2.0；`ping`/`analyze`/`shutdown`；**20 MiB** 行上限（高于请求 16 MiB 总量上限）；stdout 只写协议，通知（无 `id`）不回复。
- `compilerOptions` 由项目层解析并**完整透传**（`strictNullChecks`、`noImplicitThis`、`noUncheckedIndexedAccess`、`jsx`、`lib` 等）；未知字段拒绝；`strict:false`/`noImplicitThis:false` 时探针根为 `any`，状态 `Unavailable`。
  `compiler.ts` 不再硬编码选项；输入上限（512 文件 / 8 MiB / 2048 片段）在解析前校验；`maxTotalBytes` 同时计入 `files[].text` 与 `fragments[].exprText`。

## 目录

```text
backends/typescript/
├── src/
│   ├── protocol.ts       C1 请求/响应类型、选项转换、输入上限
│   ├── analysis.ts       analyze(request)：真实后端入口（不依赖场景）
│   ├── server.ts         NDJSON JSON-RPC 2.0 stdio 服务（ping/analyze/shutdown）
│   ├── compiler.ts       内存 LanguageService host + 读取白名单 + 环境 fail fast
│   ├── ast.ts            Page/Component 注册发现、data/properties 声明、spread 检测
│   ├── dataroot.ts       基于 checker/2693 的数据根分类（含 import type 别名）
│   ├── fragments.ts      WXML 表达式 → 挂载片段（逐段 source 映射）
│   ├── rewrite.ts        自由根标识符重写（TS 解析器，locals/全局/成员名跳过）
│   ├── expression.ts     标识符扫描、target 定位、locals 绑定名校验
│   ├── text.ts           插入/替换装配、UTF-16/字节映射、合成前缀裁剪
│   ├── version.ts        `backendVersion` 共享常量
│   ├── propsOverlay.ts   properties 覆盖：TS 类型别名 / JS JSDoc typedef
│   ├── probes.ts         pragma 扫描（最后一条生效）、探针健康门、模块翻转检查
│   ├── messages.ts       用户可见消息清洗
│   ├── queries.ts        hover / diagnostics / definition 与 TypeQuality
│   ├── run.ts            请求构建、执行、状态合成、定义溯源、证据切片
│   ├── evidence.ts       证据脱敏与目录守卫
│   ├── perf.ts           粗测
│   ├── layouts/          候选 A、B、locals 声明与共享上下文
│   ├── scenarios.ts      33 个场景
│   ├── report.ts         矩阵输出（表格转义）
│   └── cli.ts            命令入口
├── samples/              最小样本项目（闭包、模块组件、JS 组件、spread、多注册、别名、pragma、CRLF、Unicode）
├── scripts/inspect.ts    人工复查入口
├── scripts/emit-evidence.ts  刷新证据数据（目录守卫 + 脱敏）
└── test/                 208 条断言
```

## 度量口径

- **状态**：`hover` / `diagnostics` / `definition` 分别给出六态状态与 reason，根因优先于症状。
- **类型**：`getQuickInfoAtPosition` 的类型部分；`quality = known|any|unknown|missing`。
- **文档**：quick info → symbol 回落；覆盖类型字段标记 `copied`，并用定义位置判定来源。
- **诊断**：只发布落在片段原文段的诊断；起点在合成前缀的接收者诊断裁剪到原文起点；跨合成间隙的诊断裁剪到 target（仅当 target 完整在 span 内）或重叠最大的原文操作数；越界/非字符边界不 clamp；完全无法归属的挂载内诊断使 diagnostics 至少 `Partial`，计数只在 reason 中给出（不新增响应字段）；
  `off`/`@ts-nocheck` 下不查询、不发布；其余错误计入 `suppressedDiagnostics`。
- **非干扰**：`perturbsUserTypes` 比较 `this.<member>` 成员类型；`userScriptDiagnosticsIntroduced`
  按 `(code, 映射后 span)` 比较派生脚本新增诊断。
- **探针健康**：精确偏移上的 helper 符号、根 hover 非 `any`、无诊断与插入相交、模块分类未翻转。
- **位置**：结果统一换算成 WXML UTF-8 字节，断言等于目标 token 的精确范围。
- **性能回归**：9k roots、padding 在 roots 之前的最坏排列；1 MiB <300 ms、4 MiB <600 ms（旧 O(n²) 同排列约 4.2 s / 15.6 s）。

## 已知限制

- 实验不实现 WXML 解析：表达式来自场景表，位置由 `{{ expr }}` 唯一匹配；真实解析属于 P2。
- 数据根分类基于 checker 诊断（2693/2749）与同文件语法判定；C1 要求 Node 保持同等覆盖。
- `properties` 覆盖只覆盖构造器形态（String/Number/Boolean/Array/Object/Function/null）；未知形状 `Partial`。
- 每次运行重建语言服务，未验证增量缓存；性能数字只用于量级判断。
- `properties` 未保存 overlay 与 AC-007 runner 断言延期（见 C1 §10）。

## 证据

- [类型桥验证与 C1 建议](../../docs/evidence/type-bridge-c1.md)
- [C1 类型桥契约](../../docs/reference/type-bridge-c1.md)
- 原始矩阵：[matrix.json](../../docs/evidence/data/type-bridge-c1/matrix.json)
- 性能原始数据：[perf.json](../../docs/evidence/data/type-bridge-c1/perf.json)
