# 类型桥候选验证与 C1 建议（P4，2026-11-15，第 2 轮修订 + 冻结前修订）

## 1. 对象与环境

| 项 | 值 |
| --- | --- |
| TypeScript | `6.0.3`（固定） |
| `miniprogram-api-typings` | `5.3.1`（固定） |
| Node | `v24.22.2` |
| 平台 | `linux 7.2.15+deb14-amd64 (x64)`，AMD Ryzen AI 9 HX 370 w/ Radeon 890M × 24，30.5 GiB |
| 运行规模 | 33 场景 × 2 候选 = 66 次运行 |
| C1 契约 | [type-bridge-c1.md](../reference/type-bridge-c1.md)（C0 仍以 [contracts.md](../reference/contracts.md) 为准） |
| 复跑入口 | ` backends/typescript/` |

本记录是 E2。结论只覆盖本轮样本与机制；不声称完整 WXML 支持，也不外推到跨进程调用。

## 2. 方法

### 2.1 候选

- **A 原位 shadow（唯一推荐）**：options 字面量仍是注册调用的直接实参；页面探针在 options，组件探针在 `methods`；一次插入承载全部片段；`properties` 的 `optionalTypes` 用声明派生覆盖（TS 类型别名 / JS JSDoc typedef）。
- **B 符号桥 + 独立检查文件（淘汰）**：在注册点绑定 options、导出根锚点，片段放生成的 `.ts` 检查文件。代价：改写 `this` 上下文；闭包注册无法锚定；模块化脚本下覆盖类型不可见。

### 2.2 请求/响应

每次运行都构造完整 C1 请求：`viewId`/owner、`projectRoot`、有效 `compilerOptions`、`files[]`、输入 `fragments[]`、`policy`、`limits`；响应包含 `jsCheckApplied`、`registration{kind,count,topLevel}`、`insertions[]`、`mounts[]`（含 `targetLengthUtf16`）、分能力状态。

- 编译选项按项目有效值透传（含 `strictNullChecks`、`noImplicitThis`、`noUncheckedIndexedAccess`、`exactOptionalPropertyTypes`、`jsx`、`lib` 等）；未知字段被拒绝（`unsupported-option`），不会静默丢弃。
- `alias-project` 实测：`compilerOptions` 含 `baseUrl`、`paths={"@models/*":["models/*"]}`（`tsconfig.json` `extends` 解析），文件 `index.ts`/`models/user.ts`，digest `fnv1a:5c76e51d`。
- 选项回归：`noUncheckedIndexedAccess` → `{{ users[0].name }}` 报 2532 并精确映射到 `users[0]`（`44..52`）；`strictNullChecks:false` → 可空访问不再误报；`strict:false`/`noImplicitThis:false` → 探针根为 `any`，整套能力 `Unavailable`（不伪装 `Supported`）。
- 请求驱动：挂载集合来自 `request.fragments`、检查策略来自 `request.policy`（`runRequest` 直接执行请求）。
- 响应组装：`toAnalyzeResponse` 产出 C1 §6.3 形状（逐片段 hover/诊断字节 `range`/定义 `fileId+range`、`insertions`、`mounts`、`moduleKind`、`registration`）。
- 文件身份：`insertions[].file`/`mounts[].file` 与 `files[].fileId`/`scriptFileId` 同一空间（项目相对）；B 的生成检查文件用 synthetic `__wxml_view_0.ts`；`properties` 覆盖类型的定义发布原脚本 `fileId`。
- 分片身份：`request.fragments[].fragmentId` 全链路回显（响应 `fragments[].fragmentId`、`mountId = fragmentId#n`）；同一 `exprText` 可挂载多次；重复 id 返回 `bad-fragment` 而非崩溃。

### 2.3 样本

| 场景 | 覆盖点 |
| --- | --- |
| `page-basic` / `page-export-empty` / `page-cjs` / `page-esm` | 裸 `Page`、`export {}`、CommonJS、相对 `import` |
| `page-closure` | 函数内注册、闭包内 `interface`/`type`、泛型 |
| `component-properties` / `component-module` / `component-js` | `optionalTypes`、模块化组件、JS 组件（JSDoc 覆盖） |
| `component-data-cross-file` | AC-001：组件 `data` 引用跨文件 `User` + `properties` |
| `properties-spread` / `multi-registration` / `methods-nonliteral` | 声明不完整、多注册（第二个注册为 `number`，仍取首个）、`methods` 非字面量 |
| `alias-project` | AC-008：`tsconfig` `extends` + `paths` |
| `unchecked-index` / `nullable-no-strict-null` / `no-implicit-this-off` | 编译选项透传回归（2532 / 无 18047 / Unavailable） |
| `js-jsdoc-pragma` / `js-jsdoc-off` / `js-checkjs` / `js-source-check-off` / `js-source-check-inherit` / `js-bom-check-off` / `js-bom-check-inherit` | JS 策略矩阵（含 BOM+hashbang+源码 pragma） |
| `ts-nocheck` / `js-nocheck` / `js-shebang-bom` | `@ts-nocheck` 优先、BOM+hashbang 注入点 |
| `interface-root` / `interface-root-import` / `data-shorthand` | 类型根降级 |
| `nullable` / `unicode-crlf` / `crlf-script` / `cross-file-overlay` | 接收者诊断、Unicode/CRLF（模板与脚本）、跨文件 overlay |

### 2.4 度量口径

| 指标 | 定义 |
| --- | --- |
| 分能力状态 | `hover` / `diagnostics` / `definition` 各自六态 + reason；根因必须在 reason 中 |
| `perturbsUserTypes` | 原脚本与派生脚本 `this.<member>` 成员类型文本是否变化（只比类型） |
| `userScriptDiagnosticsIntroduced` | 派生脚本新增、且不在片段内的诊断，按 `(code, 映射后 span)` 比较 |
| `suppressedDiagnostics` | 查询文件中未映射到片段原文段的诊断 |
| `insertions`/`mounts` | 响应与证据数据都包含；`replaceLengthUtf16` 表达替换，`targetLengthUtf16` 表达目标长度 |
| 位置 | 结果换算为 WXML UTF-8 字节，断言等于目标 token 的精确范围 |

### 2.5 复现

```bash
$ cd backends/typescript
$ npm ci
$ npm run check          # 0 error
$ npm test               # 208 tests, 208 pass
$ npm run --silent matrix:json
$ npm run evidence       # 脱敏写入 docs/evidence/data/type-bridge-c1/
```

## 3. 实测矩阵

### 3.1 分能力状态（66 次运行）

| 场景 | A hover | A diag | A def | B hover | B diag | B def |
| --- | --- | --- | --- | --- | --- | --- |
| page-basic | supported | supported | supported | supported | supported | supported |
| page-export-empty | supported | supported | supported | supported | supported | supported |
| page-cjs | supported | supported | supported | supported | supported | supported |
| page-esm | supported | supported | supported | supported | supported | supported |
| page-closure | supported | supported | supported | unsupported | unsupported | unsupported |
| component-properties | supported | supported | supported | supported | partial | supported |
| component-module | supported | supported | supported | partial | partial | partial |
| component-js | supported | supported | supported | supported | supported | supported |
| properties-spread | partial | partial | partial | partial | partial | partial |
| multi-registration | partial | partial | partial | partial | partial | partial |
| js-jsdoc-pragma | supported | partial | supported | supported | supported | supported |
| js-jsdoc-off | supported | disabled | supported | supported | disabled | supported |
| js-checkjs | supported | supported | supported | supported | supported | supported |
| ts-nocheck | supported | disabled | supported | supported | disabled | supported |
| js-nocheck | supported | disabled | supported | supported | disabled | supported |
| js-shebang-bom | supported | supported | supported | supported | supported | supported |
| interface-root | unsupported | unsupported | unsupported | unsupported | unsupported | unsupported |
| interface-root-import | unsupported | unsupported | unsupported | unsupported | unsupported | unsupported |
| nullable | supported | supported | supported | supported | supported | supported |
| cross-file-overlay | supported | supported | supported | supported | supported | supported |
| alias-project | supported | supported | supported | supported | supported | supported |
| component-data-cross-file | supported | supported | supported | partial | partial | partial |
| crlf-script | supported | supported | supported | supported | supported | supported |
| data-shorthand | unsupported | unsupported | unsupported | unsupported | unsupported | unsupported |
| js-source-check-off | supported | disabled | supported | supported | disabled | supported |
| js-source-check-inherit | supported | supported | supported | supported | supported | supported |
| methods-nonliteral | partial | partial | partial | supported | supported | supported |
| unchecked-index | supported | supported | supported | supported | supported | supported |
| nullable-no-strict-null | supported | supported | supported | supported | supported | supported |
| no-implicit-this-off | unavailable | unavailable | unavailable | unavailable | unavailable | unavailable |
| js-bom-check-off | supported | disabled | supported | supported | disabled | supported |
| js-bom-check-inherit | supported | supported | supported | supported | supported | supported |
| unicode-crlf | supported | supported | supported | supported | supported | supported |

汇总：A hover 26 / 3 / 3 / 1；
B hover 24 / 4 / 4 / 1。
**A 不是“全部支持”**：`properties-spread`、`multi-registration`、`methods-nonliteral` 为 `Partial`；`interface-root*`、`data-shorthand` 为 `Unsupported`；`no-implicit-this-off` 为 `Unavailable`。

诊断：65/66 次运行逐字节精确；唯一例外是 `component-module / symbol-bridge`（B 的已知失败）。34 次为零期望运行（只证明“不多报”），其余用于证明检出与精确范围。

多注册 reason（A，实测）：`2 page registrations; the probe mounts the first one`。样本中第二个注册的 `title` 是 `number`，hover 仍为 `string`，证明“首个注册、不合并”。

### 3.2 hover

| 场景 | 候选 | 查询 | 类型 | 质量 | 文档 | 来源 |
| --- | --- | --- | --- | --- | --- | --- |
| page-basic | A | title | `string` | known | 页面标题 | quickinfo |
| page-basic | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| page-basic | A | user.nmae | `any` | missing | — | none |
| page-basic | B | title | `string` | known | 页面标题 | quickinfo |
| page-basic | B | user.name | `string` | known | 用户显示名称 | quickinfo |
| page-basic | B | user.nmae | `any` | missing | — | none |
| page-export-empty | A | title | `string` | known | 页面标题 | quickinfo |
| page-export-empty | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| page-export-empty | A | user.nmae | `any` | missing | — | none |
| page-export-empty | B | title | `string` | known | 页面标题 | quickinfo |
| page-export-empty | B | user.name | `string` | known | 用户显示名称 | quickinfo |
| page-export-empty | B | user.nmae | `any` | missing | — | none |
| page-cjs | A | title | `string` | known | 页面标题 | quickinfo |
| page-cjs | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| page-cjs | A | user.nmae | `any` | missing | — | none |
| page-cjs | B | title | `string` | known | 页面标题 | quickinfo |
| page-cjs | B | user.name | `string` | known | 用户显示名称 | quickinfo |
| page-cjs | B | user.nmae | `any` | missing | — | none |
| page-esm | A | title | `string` | known | 页面标题 | quickinfo |
| page-esm | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| page-esm | A | user.nmae | `any` | missing | — | none |
| page-esm | B | title | `string` | known | 页面标题 | quickinfo |
| page-esm | B | user.name | `string` | known | 用户显示名称 | quickinfo |
| page-esm | B | user.nmae | `any` | missing | — | none |
| page-closure | A | config.title | `string` | known | 页面标题 | quickinfo |
| page-closure | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| component-properties | A | title | `string \| number` | known | 标题，支持字符串或数字 | copied |
| component-properties | A | state | `string` | known | 内部状态 | quickinfo |
| component-properties | A | subtitle | `any` | missing | — | none |
| component-properties | B | title | `string \| number` | known | 标题，支持字符串或数字 | copied |
| component-properties | B | state | `string` | known | 内部状态 | quickinfo |
| component-properties | B | subtitle | `any` | missing | — | none |
| component-module | A | title | `string \| number` | known | 标题，支持字符串或数字 | copied |
| component-module | A | state | `string` | known | 内部状态 | quickinfo |
| component-module | A | subtitle | `any` | missing | — | none |
| component-module | B | title | `any` | any | — | none |
| component-module | B | state | `any` | any | — | none |
| component-module | B | subtitle | `any` | any | — | none |
| component-js | A | title | `string \| number` | known | 标题，支持字符串或数字 | copied |
| component-js | A | state | `string` | known | 内部状态 | quickinfo |
| component-js | A | subtitle | `any` | missing | — | none |
| component-js | B | title | `string \| number` | known | 标题，支持字符串或数字 | copied |
| component-js | B | state | `string` | known | 内部状态 | quickinfo |
| component-js | B | subtitle | `any` | missing | — | none |
| properties-spread | A | title | `string \| number` | known | 标题，支持字符串或数字 | copied |
| properties-spread | A | state | `string` | known | 内部状态 | quickinfo |
| properties-spread | B | title | `string \| number` | known | 标题，支持字符串或数字 | copied |
| properties-spread | B | state | `string` | known | 内部状态 | quickinfo |
| multi-registration | A | title | `string` | known | 页面标题 | quickinfo |
| multi-registration | B | title | `string` | known | 页面标题 | quickinfo |
| js-jsdoc-pragma | A | greeting | `string` | known | 问候语 | quickinfo |
| js-jsdoc-pragma | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| js-jsdoc-pragma | A | user.nmae | `any` | missing | — | none |
| js-jsdoc-pragma | B | greeting | `string` | known | 问候语 | quickinfo |
| js-jsdoc-pragma | B | user.name | `string` | known | 用户显示名称 | quickinfo |
| js-jsdoc-pragma | B | user.nmae | `any` | missing | — | none |
| js-jsdoc-off | A | greeting | `string` | known | 问候语 | quickinfo |
| js-jsdoc-off | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| js-jsdoc-off | B | greeting | `string` | known | 问候语 | quickinfo |
| js-jsdoc-off | B | user.name | `string` | known | 用户显示名称 | quickinfo |
| js-checkjs | A | greeting | `string` | known | 问候语 | quickinfo |
| js-checkjs | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| js-checkjs | A | user.nmae | `any` | missing | — | none |
| js-checkjs | B | greeting | `string` | known | 问候语 | quickinfo |
| js-checkjs | B | user.name | `string` | known | 用户显示名称 | quickinfo |
| js-checkjs | B | user.nmae | `any` | missing | — | none |
| ts-nocheck | A | title | `string` | known | 页面标题 | quickinfo |
| ts-nocheck | B | title | `string` | known | 页面标题 | quickinfo |
| js-nocheck | A | greeting | `string` | known | 问候语 | quickinfo |
| js-nocheck | B | greeting | `string` | known | 问候语 | quickinfo |
| js-shebang-bom | A | title | `string` | known | 页面标题 | quickinfo |
| js-shebang-bom | A | title.missing | `any` | missing | — | none |
| js-shebang-bom | B | title | `string` | known | 页面标题 | quickinfo |
| js-shebang-bom | B | title.missing | `any` | missing | — | none |
| nullable | A | users[0].name | `string` | known | 用户显示名称 | quickinfo |
| nullable | A | maybeUser.name | `User \| null` | known | — | none |
| nullable | A | external.anything | `unknown` | unknown | 外部数据 | quickinfo |
| nullable | B | users[0].name | `string` | known | 用户显示名称 | quickinfo |
| nullable | B | maybeUser.name | `User \| null` | known | — | none |
| nullable | B | external.anything | `unknown` | unknown | 外部数据 | quickinfo |
| cross-file-overlay | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| cross-file-overlay | B | user.name | `string` | known | 用户显示名称 | quickinfo |
| alias-project | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| alias-project | B | user.name | `string` | known | 用户显示名称 | quickinfo |
| component-data-cross-file | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| component-data-cross-file | A | title | `string` | known | 标题 | quickinfo |
| component-data-cross-file | B | user.name | `any` | any | — | none |
| component-data-cross-file | B | title | `any` | any | — | none |
| crlf-script | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| crlf-script | A | user.nmae | `any` | missing | — | none |
| crlf-script | B | user.name | `string` | known | 用户显示名称 | quickinfo |
| crlf-script | B | user.nmae | `any` | missing | — | none |
| js-source-check-off | A | title | `string` | known | 页面标题 | quickinfo |
| js-source-check-off | B | title | `string` | known | 页面标题 | quickinfo |
| js-source-check-inherit | A | title | `string` | known | 页面标题 | quickinfo |
| js-source-check-inherit | A | title.missing | `any` | missing | — | none |
| js-source-check-inherit | B | title | `string` | known | 页面标题 | quickinfo |
| js-source-check-inherit | B | title.missing | `any` | missing | — | none |
| methods-nonliteral | B | title | `string` | known | 标题 | copied |
| unchecked-index | A | users[0].name | `string` | known | 用户显示名称 | quickinfo |
| unchecked-index | B | users[0].name | `string` | known | 用户显示名称 | quickinfo |
| nullable-no-strict-null | A | maybeUser.name | `string` | known | 用户显示名称 | quickinfo |
| nullable-no-strict-null | B | maybeUser.name | `string` | known | 用户显示名称 | quickinfo |
| no-implicit-this-off | A | title | `any` | any | — | none |
| no-implicit-this-off | B | title | `any` | any | — | none |
| js-bom-check-off | A | title | `string` | known | 页面标题 | quickinfo |
| js-bom-check-off | B | title | `string` | known | 页面标题 | quickinfo |
| js-bom-check-inherit | A | title | `string` | known | 页面标题 | quickinfo |
| js-bom-check-inherit | A | title.missing | `any` | missing | — | none |
| js-bom-check-inherit | B | title | `string` | known | 页面标题 | quickinfo |
| js-bom-check-inherit | B | title.missing | `any` | missing | — | none |
| unicode-crlf | A | user.name | `string` | known | 用户显示名称 | quickinfo |
| unicode-crlf | A | 标题 | `string` | known | 标题 | quickinfo |
| unicode-crlf | A | user.nmae | `any` | missing | — | none |
| unicode-crlf | B | user.name | `string` | known | 用户显示名称 | quickinfo |
| unicode-crlf | B | 标题 | `string` | known | 标题 | quickinfo |
| unicode-crlf | B | user.nmae | `any` | missing | — | none |

### 3.3 诊断

| 场景 | 候选 | 期望 | 观测（WXML 字节） | 裁剪 | 消息 |
| --- | --- | --- | --- | --- | --- |
| page-basic | A | user.nmae | 134..138 (2339) | no | Property 'nmae' does not exist on type '{ name: string; age: number; }'. |
| page-basic | B | user.nmae | 134..138 (2339) | no | Property 'nmae' does not exist on type '{ name: string; age: number; }'. |
| page-export-empty | A | user.nmae | 134..138 (2339) | no | Property 'nmae' does not exist on type '{ name: string; }'. |
| page-export-empty | B | user.nmae | 134..138 (2339) | no | Property 'nmae' does not exist on type '{ name: string; }'. |
| page-cjs | A | user.nmae | 134..138 (2339) | no | Property 'nmae' does not exist on type '{ name: string; }'. |
| page-cjs | B | user.nmae | 134..138 (2339) | no | Property 'nmae' does not exist on type '{ name: string; }'. |
| page-esm | A | user.nmae | 134..138 (2339) | no | Property 'nmae' does not exist on type '{ name: string; }'. |
| page-esm | B | user.nmae | 134..138 (2339) | no | Property 'nmae' does not exist on type '{ name: string; }'. |
| page-closure | A | — | — | no | — |
| page-closure | B | — | — | no | — |
| component-properties | A | subtitle | 130..138 (2339) | no | Property 'subtitle' does not exist on type '{ state: string; }'. |
| component-properties | B | subtitle | 130..138 (2339) | no | Property 'subtitle' does not exist on type '{ state: string; } & declared properties'. |
| component-module | A | subtitle | 130..138 (2339) | no | Property 'subtitle' does not exist on type '{ state: string; }'. |
| component-module | B | subtitle | — | no | — |
| component-js | A | subtitle | 130..138 (2339) | no | Property 'subtitle' does not exist on type '{ state: string; }'. |
| component-js | B | subtitle | 130..138 (2339) | no | Property 'subtitle' does not exist on type '{ state: string; } & declared properties'. |
| properties-spread | A | — | — | no | — |
| properties-spread | B | — | — | no | — |
| multi-registration | A | — | — | no | — |
| multi-registration | B | — | — | no | — |
| js-jsdoc-pragma | A | user.nmae | 140..144 (2339) | no | Property 'nmae' does not exist on type 'User'. |
| js-jsdoc-pragma | B | user.nmae | 140..144 (2339) | no | Property 'nmae' does not exist on type 'User'. |
| js-jsdoc-off | A | — | — | no | — |
| js-jsdoc-off | B | — | — | no | — |
| js-checkjs | A | user.nmae | 140..144 (2339) | no | Property 'nmae' does not exist on type 'User'. |
| js-checkjs | B | user.nmae | 140..144 (2339) | no | Property 'nmae' does not exist on type 'User'. |
| ts-nocheck | A | — | — | no | — |
| ts-nocheck | B | — | — | no | — |
| js-nocheck | A | — | — | no | — |
| js-nocheck | B | — | — | no | — |
| js-shebang-bom | A | title.missing | 91..98 (2339) | no | Property 'missing' does not exist on type 'string'. |
| js-shebang-bom | B | title.missing | 91..98 (2339) | no | Property 'missing' does not exist on type 'string'. |
| interface-root | A | — | — | no | — |
| interface-root | B | — | — | no | — |
| interface-root-import | A | — | — | no | — |
| interface-root-import | B | — | — | no | — |
| nullable | A | maybeUser.name, external.anything | 45..54 (18047), 98..106 (18046) | yes | 'maybeUser' is possibly 'null'. / 'external' is of type 'unknown'. |
| nullable | B | maybeUser.name, external.anything | 45..54 (18047), 98..106 (18046) | yes | 'maybeUser' is possibly 'null'. / 'external' is of type 'unknown'. |
| cross-file-overlay | A | — | — | no | — |
| cross-file-overlay | B | — | — | no | — |
| alias-project | A | — | — | no | — |
| alias-project | B | — | — | no | — |
| component-data-cross-file | A | — | — | no | — |
| component-data-cross-file | B | — | — | no | — |
| crlf-script | A | user.nmae | 95..99 (2339) | no | Property 'nmae' does not exist on type '{ name: string; }'. |
| crlf-script | B | user.nmae | 95..99 (2339) | no | Property 'nmae' does not exist on type '{ name: string; }'. |
| data-shorthand | A | — | — | no | — |
| data-shorthand | B | — | — | no | — |
| js-source-check-off | A | — | — | no | — |
| js-source-check-off | B | — | — | no | — |
| js-source-check-inherit | A | title.missing | 91..98 (2339) | no | Property 'missing' does not exist on type 'string'. |
| js-source-check-inherit | B | title.missing | 91..98 (2339) | no | Property 'missing' does not exist on type 'string'. |
| methods-nonliteral | A | — | — | no | — |
| methods-nonliteral | B | — | — | no | — |
| unchecked-index | A | users[0].name | 44..52 (2532) | yes | Object is possibly 'undefined'. |
| unchecked-index | B | users[0].name | 44..52 (2532) | yes | Object is possibly 'undefined'. |
| nullable-no-strict-null | A | — | — | no | — |
| nullable-no-strict-null | B | — | — | no | — |
| no-implicit-this-off | A | — | — | no | — |
| no-implicit-this-off | B | — | — | no | — |
| js-bom-check-off | A | — | — | no | — |
| js-bom-check-off | B | — | — | no | — |
| js-bom-check-inherit | A | title.missing | 91..98 (2339) | no | Property 'missing' does not exist on type 'string'. |
| js-bom-check-inherit | B | title.missing | 91..98 (2339) | no | Property 'missing' does not exist on type 'string'. |
| unicode-crlf | A | user.nmae | 103..107 (2339) | no | Property 'nmae' does not exist on type '{ name: string; }'. |
| unicode-crlf | B | user.nmae | 103..107 (2339) | no | Property 'nmae' does not exist on type '{ name: string; }'. |

### 3.4 定义

| 场景 | 候选 | 查询 | kind | 源字节范围 |
| --- | --- | --- | --- | --- |
| page-basic | A | title | script | 45..50 |
| page-basic | A | user.name | script | 136..140 |
| page-basic | B | title | script | 45..50 |
| page-basic | B | user.name | script | 136..140 |
| page-export-empty | A | title | script | 45..50 |
| page-export-empty | A | user.name | script | 136..140 |
| page-export-empty | B | title | script | 45..50 |
| page-export-empty | B | user.name | script | 136..140 |
| page-cjs | A | title | script | 233..238 |
| page-cjs | A | user.name | script | 324..328 |
| page-cjs | B | title | script | 233..238 |
| page-cjs | B | user.name | script | 324..328 |
| page-esm | A | title | script | 92..97 |
| page-esm | A | user.name | script | 183..187 |
| page-esm | B | title | script | 92..97 |
| page-esm | B | user.name | script | 183..187 |
| page-closure | A | config.title | script | 208..213 |
| page-closure | A | user.name | script | 98..102 |
| component-properties | A | title | generated | 77..82 |
| component-properties | A | state | script | 252..257 |
| component-properties | B | title | generated | 77..82 |
| component-properties | B | state | script | 252..257 |
| component-module | A | title | generated | 77..82 |
| component-module | A | state | script | 181..186 |
| component-js | A | title | generated | 77..82 |
| component-js | A | state | script | 181..186 |
| component-js | B | title | generated | 77..82 |
| component-js | B | state | script | 181..186 |
| properties-spread | A | title | generated | 167..172 |
| properties-spread | A | state | script | 271..276 |
| properties-spread | B | title | generated | 167..172 |
| properties-spread | B | state | script | 271..276 |
| multi-registration | A | title | script | 45..50 |
| multi-registration | B | title | script | 45..50 |
| js-jsdoc-pragma | A | greeting | script | 265..273 |
| js-jsdoc-pragma | A | user.name | script | 52..56 |
| js-jsdoc-pragma | B | greeting | script | 265..273 |
| js-jsdoc-pragma | B | user.name | script | 52..56 |
| js-jsdoc-off | A | greeting | script | 265..273 |
| js-jsdoc-off | A | user.name | script | 52..56 |
| js-jsdoc-off | B | greeting | script | 265..273 |
| js-jsdoc-off | B | user.name | script | 52..56 |
| js-checkjs | A | greeting | script | 265..273 |
| js-checkjs | A | user.name | script | 52..56 |
| js-checkjs | B | greeting | script | 265..273 |
| js-checkjs | B | user.name | script | 52..56 |
| ts-nocheck | A | title | script | 60..65 |
| ts-nocheck | B | title | script | 60..65 |
| js-nocheck | A | greeting | script | 57..65 |
| js-nocheck | B | greeting | script | 57..65 |
| js-shebang-bom | A | title | script | 68..73 |
| js-shebang-bom | B | title | script | 68..73 |
| nullable | A | users[0].name | script | 47..51 |
| nullable | A | maybeUser.name | script | 85..94 |
| nullable | A | external.anything | script | 145..153 |
| nullable | B | users[0].name | script | 47..51 |
| nullable | B | maybeUser.name | script | 85..94 |
| nullable | B | external.anything | script | 145..153 |
| cross-file-overlay | A | user.name | external | 54..58 |
| cross-file-overlay | B | user.name | external | 54..58 |
| alias-project | A | user.name | external | 54..58 |
| alias-project | B | user.name | external | 54..58 |
| component-data-cross-file | A | user.name | external | 54..58 |
| component-data-cross-file | A | title | script | 89..94 |
| crlf-script | A | user.name | script | 90..94 |
| crlf-script | B | user.name | script | 90..94 |
| js-source-check-off | A | title | script | 58..63 |
| js-source-check-off | B | title | script | 58..63 |
| js-source-check-inherit | A | title | script | 58..63 |
| js-source-check-inherit | B | title | script | 58..63 |
| methods-nonliteral | B | title | generated | 92..97 |
| unchecked-index | A | users[0].name | script | 85..90 |
| unchecked-index | B | users[0].name | script | 85..90 |
| nullable-no-strict-null | A | maybeUser.name | script | 47..51 |
| nullable-no-strict-null | B | maybeUser.name | script | 47..51 |
| js-bom-check-off | A | title | script | 81..86 |
| js-bom-check-off | B | title | script | 81..86 |
| js-bom-check-inherit | A | title | script | 81..86 |
| js-bom-check-inherit | B | title | script | 81..86 |
| unicode-crlf | A | user.name | script | 131..135 |
| unicode-crlf | A | 标题 | script | 39..45 |
| unicode-crlf | B | user.name | script | 131..135 |
| unicode-crlf | B | 标题 | script | 39..45 |

### 3.5 insertions / mounts

共 153 条插入、117 个挂载点。每条插入记录 `startUtf16/lengthUtf16/replaceLengthUtf16`；每个挂载点含 `targetStartUtf16/targetLengthUtf16` 与 `segments[]`（`startUtf16/lengthUtf16`）。示例：

| 场景 | 候选 | insertion | kind | start | len | replace |
| --- | --- | --- | --- | --- | --- | --- |
| page-basic | A | probe | probe | 6 | 197 | 0 |
| page-basic | B | opts-decl | capture | 0 | 19 | 0 |
| page-basic | B | capture | capture | 24 | 16 | 0 |
| page-basic | B | anchor | anchor | 256 | 43 | 0 |
| page-basic | B | check-file | check-file | 0 | 194 | 0 |
| js-shebang-bom | A | bom | edit | 0 | 0 | 1 |
| js-shebang-bom | A | pragma | pragma | 20 | 13 | 0 |
| js-shebang-bom | A | probe | probe | 39 | 168 | 0 |
| js-shebang-bom | B | opts-decl | capture | 21 | 19 | 0 |
| js-shebang-bom | B | capture | capture | 45 | 16 | 0 |
| js-shebang-bom | B | anchor | anchor | 115 | 43 | 0 |
| js-shebang-bom | B | check-file | check-file | 0 | 169 | 0 |
| unchecked-index | A | probe | probe | 58 | 139 | 0 |
| unchecked-index | B | opts-decl | capture | 52 | 19 | 0 |
| unchecked-index | B | capture | capture | 76 | 16 | 0 |
| unchecked-index | B | anchor | anchor | 138 | 43 | 0 |
| unchecked-index | B | check-file | check-file | 0 | 144 | 0 |

### 3.6 非干扰与新增脚本诊断

| 场景 | 候选 | `perturbsUserTypes` | `userScriptDiagnosticsIntroduced` | suppressed |
| --- | --- | --- | --- | --- |
| page-basic / component-properties / component-module / component-js | B | **是** | 2339 / 2304（部分场景） | 0 |
| component-data-cross-file | B | 否 | 2304（模块作用域） | 0 |
| js-jsdoc-pragma | A | 否 | 2322（`@ts-check` 带出的无关错误） | 1 |
| 其余 | A / B | 否 | — | 0 |

A 在全部场景 `perturbsUserTypes = false`；`@ts-check` 的副作用单独计为 `userScriptDiagnosticsIntroduced`。

### 3.7 性能粗测

- 冷启动：约 0.2–0.5 s（同机波动 2–3×）。
- 热 hover：p50 亚毫秒；整文件语义诊断约 0.05 ms。
- 样本量：page-basic 3 片段 × 20 轮；wide-40 40 × 5。逐次数据见 ` docs/evidence/data/type-bridge-c1/perf.json`，不构成基准。

## 4. 逐项验证（规格 §4）

| # | 要求 | 结果 |
| --- | --- | --- |
| 1 | 页面 `data` 嵌套对象类型与 JSDoc | A/B 返回 `string` + `用户显示名称`；`user.nmae` 精确落在 `nmae` |
| 2 | 无导出 / `export {}` / CommonJS / ESM | A 四种写法通过且不改变模块分类；B 在模块文件上暴露覆盖类型不可见问题 |
| 3 | 闭包内局部类型、别名、interface、泛型 | A 通过；B 无法锚定函数内注册 |
| 4 | 组件 `properties`（含 `optionalTypes`） | A/B 都需要声明派生覆盖；A 支持 TS 与 JS（JSDoc typedef） |
| 5 | 未保存 overlay | 脚本与跨文件模型 overlay 生效、磁盘不变；`properties` overlay 延期（C1 §10） |
| 6 | 诊断精确范围 | 笔误只标字段；可空/unknown 标在接收者；2532 标在 `users[0]`；消息不含内部符号 |
| 7 | 标识符映射 | 片段与定义都映射回源字节；生成成员映射回 `properties.<name>`；派生路径不发布 |
| 8 | 性能 | 冷启动 0.2–0.5 s，热 hover 亚毫秒 |

## 5. 关键发现

### 5.1 `optionalTypes` 需声明派生覆盖

`miniprogram-api-typings@5.3.1` 的 `PropertyToData` 只看 `type`。两个候选都从声明生成覆盖类型：TS 用类型别名，JS 用 `@typedef`/`@property`；字段文档来源标为 `copied`，定义映射回 `properties.<name>`。

### 5.2 探针落点

`Page` options 接受探针成员；`Component` options 拒绝（2353）；组件探针必须放 `methods`。`methods` 非字面量时不挂载片段，视图 `Partial`（`methods-nonliteral`）。

### 5.3 候选 B 的代价

- 改写 `this` 上下文：`page-basic` 的 `this.setData` 变 `any`；`component-properties`/`component-module` 出现 2339 假阳性；`component-js` 的 `this.setData` 退化为 `any`（该 JS 文件不检查，因此不报 2339；它发布的 2339 是 `subtitle` 模板诊断）。
- 闭包注册 `Unsupported`；模块化组件/组件跨文件数据新增 `2304`，根退化为 `any`。

### 5.4 多注册与 C0 对齐

采用 `Partial` + 候选计数 + 首个注册 + 不合并，reason 为 `2 page registrations; the probe mounts the first one`；[类型诊断策略](../reference/type-diagnostics.md) 的多候选行已同步为同一策略；第二个注册类型不同也不影响结果。

### 5.5 `jsCheck=off` 必须真正关闭

源码 `// @ts-check` + `policy=off` 时，派生视图在源码 pragma **之后**插入 `// @ts-nocheck` 中和（TypeScript 以最后一条 pragma 为准），不查询、不发布诊断：`jsCheckApplied=off`、`diagnostics=disabled`、发布集合为空（`js-source-check-off`）。同一源码在 `inherit` 下正常发布 2339（`js-source-check-inherit`）。

### 5.6 位置、替换与定义

- `insertions[]` 含 `replaceLengthUtf16`；BOM 丢弃记为 `kind:"edit"`、`0` 长插入 + `1` 字符替换。
- 可空/unknown 诊断裁剪后精确覆盖 `maybeUser`（45..54）与 `external`（98..106）。
- Unicode/CRLF：模板 CRLF 与脚本 CRLF（`crlf-script`）都按字节断言。
- 定义分类：`script` / `generated` / `external`；指向派生文件的目标丢弃并 `Partial`。
- 输入上限（512 文件 / 8 MiB / 2048 片段）在解析前校验。

### 5.7 编译选项必须完整透传

只传 8 个选项会让派生视图按不同规则检查：`noUncheckedIndexedAccess` 被丢弃时 `users[0].name` 会假 `Supported` 且无诊断；`strictNullChecks:false` 被忽略时会假报 18047。本轮把选项扩为近完整集合并拒绝未知字段；三个回归场景（`unchecked-index` 2532、`nullable-no-strict-null` 无 18047、`no-implicit-this-off` `Unavailable`）进入矩阵。

### 5.8 BOM + hashbang 与 pragma

BOM 在 hashbang 之前会让 TypeScript 误解析 shebang，源码 pragma 因此不在前导注释块内。派生视图在**任何 pragma 需要生效**（注入 `@ts-check`、源码 `@ts-check`、源码 `@ts-nocheck`）时丢弃 BOM，记为 1 字符替换。两个回归场景：
`js-bom-check-off`（`checkJs:true` + policy off）发布为空且 `disabled`；`js-bom-check-inherit`（源码 `@ts-check` + inherit）发布 2339。

### 5.9 响应坐标

- 诊断只发布 WXML 字节 `range`；派生文件 `spanUtf16` 是内部实现。
- 定义发布 `{kind, fileId, range}`（目标源文件字节），不发布派生文件坐标。
- `targetLengthUtf16` 与 `segments[]`（`startUtf16/lengthUtf16`）在 `matrix.json` 中逐挂载点断言。
- `insertions[].file`/`mounts[].file` 与请求的 `files[].fileId` 同一身份空间；`alias-project` 为 `index.ts`（不是 `samples/alias-project/index.ts`），定义为 `models/user.ts`。

## 6. 结论与推荐

1. 候选 A 是唯一布局：26/33 个场景 hover 完全支持，其余为按设计的 `Partial`/`Unsupported`/`Unavailable`；**不是完整支持**。
2. 在共同支持的场景内，A/B 的类型、文档与诊断精度等价；差异在覆盖域与副作用。
3. B 淘汰：改写用户代码类型（4 个场景）、闭包不可锚定、模块化覆盖类型不可见。
4. 编译选项、多注册、JS 策略、请求/响应、位置替换与定义溯源都已写入 [C1 契约](../reference/type-bridge-c1.md)。

## 7. 风险与未决

- 覆盖类型依赖声明形状；未知形状 `Partial`。
- 覆盖字段文档是复制文本；定义可回原节点，但 `{@link}` 等语义失效。
- A 会让 `Page.Instance` 的 `TCustom` 多出 `__wxml_check_0`；hover `this` 类型文本需过滤。
- `@ts-check` 的过滤依赖片段归属；需保持样本。
- 编译选项已覆盖常用类型开关并拒绝未知字段，但未逐项验证语义（如 `lib` 组合）。
- 增量缓存、真实项目规模、AC-004 `properties` overlay、AC-007 runner 断言、AC-008 同脚本多视图延期（C1 §10）。

## 8. P5 实现说明（Wave 2）

本记录第 1–7 节是 P4 的候选验证结论；Wave 2 的 P5 在同一 harness 上实现了生产入口。P5 增加了 insertion 的 `viewRevision`/`backendFileVersion` 等契约字段，因此 `matrix.json` 已按 `npm run evidence` 重新生成；归一化后 `matrix:json` 与入库数据 0 diff（577,049 B）：

- `src/analysis.ts` 的 `analyze(request)` 只使用请求字段（`files[]`/`scriptFileId`/`compilerOptions`/`fragments[]`/`policy`/`view`），不读项目源文件或 tsconfig；项目文件缺失返回 `Unavailable`。
- C1.1：`RequestFragment` 支持可选 `target`/`locals`；`locals` 中的根标识符不加根前缀（Node 以 `any` 绑定，类型由 P6 后续提供）。
- `src/server.ts` 提供 NDJSON JSON-RPC 2.0 stdio（`ping`/`analyze`/`shutdown`，20 MiB 行上限，错误码映射）。
- C1.1 第 2 轮：表达式按 TS 解析结果重写**每个自由根标识符**（一元/括号/二元/条件/索引/调用/可选链），成员名、对象 key、字符串/模板文本、注释、全局内建与 `locals` 不前缀；无法安全挂载的形状返回 `bad-fragment`。`wxmlRange` 长度必须等于 `exprText` UTF-8 字节数；挂载保存逐段 source 映射。
- P5 MEDIUM 轮：默认 target 使用最右值 token 的原始偏移（`a.a`→第二个 `a`），包装只接受精确覆盖整个表达式的单个 ParenthesizedExpression（拒绝 `title); (user` 注入）；跨合成间隙诊断裁剪到 target/最大重叠原文段，无法归属时 diagnostics 至少 `Partial`；`satisfies`/TypeNode/`new` 返回 `bad-fragment`；表达式深度 >256 或节点 >10,000 返回 `bad-fragment` 并捕获 RangeError。
- P5 HIGH 轮：`rewriteExpression` 发射改为递增 source/mounted 游标、roots Set、shorthand 复用 source 区间。性能回归使用 9k roots 且 **padding 在 roots 之前** 的最坏排列（旧 `slice(0, from)` 会复制整段 padding）；阈值 1 MiB <300 ms、4 MiB <600 ms。本机完整测试并行时新实现约 26–66 ms / 46–113 ms；同排列旧实现约 4.2 s / 15.6 s。映射越界/非字符边界不再 clamp；target 不在 span 内取最大重叠原文段。错误消息截断 120 字符 + 总长度；`locals` 覆盖全局名；`maxTotalBytes` 计入 `fragments[].exprText`；深度从 root 起算。
- 请求身份：`files[].fileId === path`（项目相对），`view.owner.fileId === scriptFileId`；`compilerOptions` 的 `lib`/`types`/`typeRoots`/`paths`/`baseUrl`/`rootDir` 与布尔开关校验形状，`propertiesOverlay:"off"` 显式 `unsupported-option`。
- 测试总数 208；`npm run build` 产出 `dist/server.js`；`npm ci --omit=dev` 后服务可启动（typescript/typings 为运行时依赖）。

## 9. 数据与复现

- harness：` backends/typescript/`
- 场景表：` backends/typescript/src/scenarios.ts`
- 断言：` backends/typescript/test/*.test.ts`（208 条）
- 原始矩阵：` docs/evidence/data/type-bridge-c1/matrix.json`（66 次运行，含请求摘要、状态、hover、诊断、定义、insertions/mounts）
- 原始性能：` docs/evidence/data/type-bridge-c1/perf.json`
- 刷新：` npm run evidence`（写入前校验目录在仓库布局内，并替换 `<backend-root>`/`<repo-root>`）
