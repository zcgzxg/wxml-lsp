# C1 类型桥契约

**Status**: accepted（2026-11-15，C1.1 增量版；布局/边界仍为第 4 轮）。
**Last Updated**: 2026-11-16（显式 app/project 根边界见 [ADR-0006](../adr/0006-explicit-mini-program-roots.md)）。
冻结动作由 P0 在合并本文件时执行；C0 契约仍以[基础契约](contracts.md)为准，本文件只补充类型桥部分。
证据：[类型桥验证与 C1 建议](../evidence/type-bridge-c1.md)；实验实现见 [backends/typescript](../../backends/typescript/README.md)。

## 0. 范围

已冻结：

1. 布局：候选 A 原位 shadow 唯一；页面/组件探针落点（§1）。
2. 数据根分类与 `properties` 声明派生覆盖（TS 与 JS JSDoc）（§2）。
3. JS 检查策略与 pragma 优先级、`@ts-nocheck`、注入点（§3）。
4. 位置模型：`insertions[]`（含替换）与 `mounts[]`、合成前缀裁剪、定义溯源（§4）。
5. 分能力状态、探针健康门、多注册、类型根降级（§5）。
6. 请求/响应模型：`files[]`、`fragments[]`、`compilerOptions`、`policy`、`viewId`/owner、输入上限（§6）。
7. 消息可见性、ABI 保留名、版本锁定（§7–§9）。

未冻结：反向编辑、组件内 `usingComponents` 建模、多根工作区、发布流水线。P6 完整模板作用域（`wx:for` 元素/索引类型、条件窄化）、补全和 Zed 多语言同步已在 Wave 5 实现，接口见 §6 的 protocol v2 增量。

## 1. 布局 A（唯一）

```text
Page({                                Component({
  __wxml_check_0() {                    methods: {
    const __wxml_root = this.data;        __wxml_check_0() {
    return [ <mount…> ];                    const __wxml_root = this.data;
  },                                        return [ <mount…> ];
  data: { …原文… },                          };
});                                       },
                                        });
```

规则：

1. options 字面量必须仍是 `Page(...)`/`Component(...)` 的直接实参；不得提取成变量、不得新增 `export`、不得改变文件种类。
2. 页面探针是 options 成员；组件探针必须放 `methods`（`Component.Options` 没有 `TCustom` 槽位，直接放 options 会得到 2353）。
3. 组件已有 `methods` 时插入其内部；`methods` 不是对象字面量（简写、展开、调用结果）时**不挂载任何片段**：视图返回 `Partial`，reason 写明 `methods is not an object literal`。
4. 探针方法名 `__wxml_check_<n>`，根绑定固定 `const __wxml_root = this.data;`。
5. 一次插入承载全部片段；不得为每个片段单独插入一段代码。
6. 插入只增不删，唯一的例外是 BOM 替换（§3.4）。

## 2. 数据根与 `properties` 覆盖

### 2.1 数据根

| 形式 | 结果 |
| --- | --- |
| `data: { … }` | `Supported`；根 = `this.data` |
| `data: <表达式>`（变量、泛型调用、`as` 断言） | `Supported`；根类型来自真实推断 |
| 无 `data` 成员 | `Partial`；reason 说明根为空 |
| `data` 成员存在但不是属性赋值（简写/方法） | `Unsupported`；reason 写明“成员存在但非属性赋值” |
| `data: User`（interface/type alias，含 `import type` 别名） | `Unsupported` + `TypeQuality::Unknown`；不发布诊断、不伪造字段 |

类型根判定必须经类型检查器（`2693`/`2749` 或等价符号判定），同文件与导入别名都要覆盖。

### 2.2 `properties` 覆盖

`miniprogram-api-typings@5.3.1` 的 `PropertyToData` 只看 `type`，忽略 `optionalTypes`。契约要求从 `properties` 声明生成覆盖类型：

- 字段类型 = `type` ∪ `optionalTypes`（去重；`null` → `any`）；
- 忽略 `value`/`default`；
- `Array` → `any[]`、`Object` → `Record<string, any>`、`Function` → `(...args: any[]) => any`；
- 字段 JSDoc 复制进覆盖类型，`documentationSource = copied`；
- 未知形状（计算属性、非字面量、无法识别的构造器）→ 该字段不覆盖，视图 `Partial`；
- `properties` 含 `...spread` 或简写成员 → `Partial`，reason 说明“声明不完整，`optionalTypes` 可能欠报”；
- TypeScript 用类型别名 `__WxmlProps_<n>`；JavaScript 用 JSDoc：

```js
/**
 * @typedef {Object} __WxmlProps_0
 * @property {string | number} title 标题，支持字符串或数字
 */
const __wxml_props_0 = /** @type {__WxmlProps_0} */ (raw);
```

覆盖类型成员的定义必须映射回 `properties.<name>` 原节点；无法映射时丢弃并 `Partial`。

## 3. JS 检查策略

### 3.1 优先级（写死）

```text
off > @ts-nocheck > source @ts-check > pragma
```

| 取值 | 行为 |
| --- | --- |
| `off` | 真正关闭模板诊断：不查询、不发布；若源码含 `@ts-check`，派生视图在**源码 pragma 之后**插入 `// @ts-nocheck` 中和（TypeScript 以最后一条 pragma 为准） |
| `@ts-nocheck`（源码，`inherit`/`pragma` 下） | 诊断 `Disabled` |
| 源码 `@ts-check` | 仅在 `inherit`/`pragma` 下生效（视为 `inherit`，不重复注入）；`off` 下被中和 |
| `pragma` | 在 hashbang 之后、首条语句之前注入 `// @ts-check` |
| `inherit` | 使用项目 `checkJs`；`checkJs:true` 时不注入 |

响应必须回传**实际生效值** `jsCheckApplied`，且状态与发布集合必须一致：`Disabled` ⇒ 发布集合为空。

### 3.2 范围与发布

1. 检查是文件级的；只发布映射到片段原文段的诊断，其余计入 `suppressed`。
2. 无关 JS 错误（例如 `@ts-check` 带出的 2322）不得重复发布。
3. `@ts-nocheck` 下 hover 仍可返回类型，但不得发布诊断。

### 3.3 注入点

- `// @ts-check`：BOM 之后（若存在）、hashbang 行之后、首条语句之前。
- 中和用 `// @ts-nocheck`：**前导注释块之后**（最后一条 pragma 生效）。
- BOM 与 hashbang 同时存在且需要注入时，派生视图丢弃 BOM（记录为 1 字符替换，§4.1）。

## 4. 位置模型

```text
WXML 原文 UTF-8 字节
  ↕ Rust 模板映射
片段文本 UTF-8 字节（含合成前缀）
  ↕ Node 插入映射（insertions[]）
派生文件绝对 UTF-16
```

### 4.1 `insertions[]`

```jsonc
{
  "insertionId": "probe",
  "file": "pages/index/index.ts",
  "kind": "probe",              // probe|pragma|anchor|capture|check-file|props-type|edit
  "startUtf16": 6,
  "lengthUtf16": 203,           // 插入文本长度
  "replaceLengthUtf16": 0,      // 被替换掉的原始字符数（BOM 为 1）
  "viewRevision": 7,            // 契约字段：由 P5 帧提供，C1 透传
  "backendFileVersion": 3       // 契约字段：由 P5 帧提供，C1 透传
}
```

- BOM 丢弃必须记为 `kind:"edit"`、`lengthUtf16:0`、`replaceLengthUtf16:1`，不得记成 0 长插入。
- 每个插入独立记录；偏移计入此前全部插入与替换。
- `insertions[].file` 与 `files[].fileId`/`scriptFileId` 使用**同一身份空间**（相对所选 app 的 `projectRoot`、`/` 分隔）；不在 `files[]` 中的生成文件使用文档化 synthetic fileId（例如 `__wxml_view_0.ts`）。

### 4.2 `mounts[]`

```jsonc
{
  "fragmentId": "user.name",
  "mountId": "user.name#0",
  "insertionId": "probe",
  "insertionId": "probe",
  "startUtf16": 96,
  "targetStartUtf16": 108,
  "targetLengthUtf16": 4,
  "segments": [
    { "startUtf16": 96, "lengthUtf16": 12, "synthetic": true },
    { "startUtf16": 108, "lengthUtf16": 9, "source": { "start": 150, "end": 159 } }
  ]
}
```

- 片段 = 合成前缀段 + 原文段；合成段必须显式标注。
- 同一表达式可挂载多次，每次挂载独立返回；结果按挂载点映射。
- `fragmentId` 来自请求 `fragments[].fragmentId`，是响应中唯一的分片身份；`mountId` 以 `${fragmentId}#${n}` 生成。同一 `exprText` 允许出现多次（不同 `fragmentId`/`wxmlRange`），不得按表达式文本去重或拒绝；重复 `fragmentId` 返回 `bad-fragment`。
- `targetLengthUtf16` 是必填字段（目标标识符的 UTF-16 长度），本轮探针逐次断言等于 `Buffer.byteLength(target)` 的 UTF-16 换算。

### 4.3 裁剪与丢弃

| 情形 | 行为 |
| --- | --- |
| span 完全落在合成段 | `unattributable`，丢弃并计入覆盖不足 |
| span 起点在合成前缀、终点在原文段 | 裁剪到原文段起点 |
| span 跨越合成间隙 | 优先裁剪到 `target`（若 target 完整落在 span 内），否则裁剪到与 span 重叠最大的原文段；无原文重叠 → `unattributable` |
| span 落在挂载内但完全无法归属 | 丢弃并计入 `unmappedDiagnostics`；该请求 `diagnostics` 至少 `Partial` |
| 映射后越界 / 非字符边界 | `unattributable`，丢弃；禁止 clamp 到 `0..len` |
| target 不在诊断 span 内 | 不使用 target，改取与 span 重叠最大的原文段；不得裁到 span 外的 token |
| 同一 `(code, start, end)` 重复 | 去重后只发布一次 |

`unmappedDiagnostics` 不新增响应字段；计数与原因只写入 `states.diagnostics.reason`（`Disabled` 时保留 `Disabled`）。

### 4.4 定义跳转

- 目标在原脚本 → 经 shadow 映射回原脚本字节；
- 目标在生成声明（覆盖类型成员、JSDoc typedef）→ 映射回 `properties.<name>` 原节点；
- 目标在快照内其它文件（`models.ts`、`.d.ts`）→ 返回该 `FileId` + 范围；
- 目标在派生文件（查询文件或检查文件）自身、快照外、或无法映射 → **丢弃**，定义能力 `Partial`，不得返回派生路径。

## 5. 状态

### 5.1 分能力状态

| 情形 | hover | diagnostics | definition |
| --- | --- | --- | --- |
| 正常 | `Supported` | `Supported` | `Supported` |
| `jsCheck:off` / `@ts-nocheck` | `Supported` | `Disabled`（发布集合为空） | `Supported` |
| `pragma` 带出无关脚本诊断 | `Supported` | `Partial`（说明已抑制） | `Supported` |
| 探针根为 `any`/缺失 | `Partial`/`Unavailable` | 同左 | `Partial`/`Unavailable` |
| 诊断与插入相交 | `Partial` | `Partial`/`Unavailable` | `Partial` |
| 模块分类翻转 | `Partial` | `Partial` | `Partial` |
| 覆盖字段定义无法映射 | `Supported` | `Supported` | `Partial` |
| `data` 为类型引用 | `Unsupported` | `Unsupported` | `Unsupported` |
| `data` 简写/非属性赋值 | `Unsupported` | `Unsupported` | `Unsupported` |
| 多注册 | `Partial` | `Partial` | `Partial` |
| `properties` 不完整 | `Partial` | `Partial` | `Partial` |
| `methods` 非字面量 | `Partial` | `Partial` | `Partial` |
| typed 模板 locals / 条件窄化（Wave 5） | `Supported` | `Supported` | `Supported`（复杂守卫/无法映射时 `Partial`） |
| 挂载内诊断完全无法映射到 WXML | `Supported` | `Partial`/`Disabled` | `Supported` |

降级 reason 必须包含根因；诊断“期望未命中”只能**追加**在根因之后，不能覆盖根因。

### 5.2 多注册（与 C0 策略一致）

采用实现现状：**`Partial` + 候选计数 + 使用首个注册 + 不合并**。

```text
reason = "<n> <page|component> registrations; the probe mounts the first one"
```

- `n` 是本次注册种类（`Page` 或 `Component`）的候选数；
- 不使用后续注册的 `data`/`properties` 合并；
- 注册数 > 1 时 hover/diagnostics/definition 全部 `Partial`；
- 该策略与[类型诊断策略](type-diagnostics.md) §10 的多候选行一致。

页面视图的 owner kind 为 `page` 时优先匹配 `Page` 注册；若脚本没有任何 `Page`
注册、但有 `Component` 注册（真实小程序允许用 `Component` 注册页面路由），则回退到
首个 `Component`，并把响应中的 `registration.kind` 置为 `component`。组件视图仍只
匹配 `Component`。回退后数据根按组件的 `data`/`properties` 处理。

### 5.3 探针健康门

构建后、发布前必须检查：

1. 记录的 helper 偏移处存在 TypeScript 符号（**按精确偏移**，不得按名字搜索）；
2. 根绑定 hover 质量不是 `any`/`missing`；
3. 没有诊断与插入区间相交（片段内部诊断除外）；
4. 派生前后模块分类未翻转。

任一失败 → `Partial`（根缺失时 `Unavailable`），不得因诊断被抑制就报 `Supported`。

## 6. 请求 / 响应

### 6.1 请求（Rust → Node）

```jsonc
{
  "protocolVersion": 2,
  "requestId": "pages/index:view-7",
  "analysisStamp": { "snapshotId": 3, "projectRevision": 5, "viewRevision": 7, "backendEpoch": 2 },
  "view": {
    "viewId": "pages/index/index.wxml:7",
    "owner": { "kind": "page", "fileId": "pages/index/index.ts" }
  },
  "project": {
    "projectRoot": "/work/miniapp",
    "compilerOptions": {
      "strict": true, "strictNullChecks": true, "strictFunctionTypes": true,
      "strictBindCallApply": true, "strictPropertyInitialization": true,
      "noImplicitAny": true, "noImplicitThis": true, "noUncheckedIndexedAccess": true,
      "exactOptionalPropertyTypes": false, "useUnknownInCatchVariables": true,
      "allowJs": true, "checkJs": false, "esModuleInterop": false,
      "resolveJsonModule": false, "isolatedModules": false, "verbatimModuleSyntax": false,
      "target": "ES2022", "module": "ESNext", "moduleResolution": "bundler",
      "jsx": "preserve", "moduleDetection": "auto", "lib": ["ES2022", "DOM"],
      "baseUrl": ".", "paths": { "@models/*": ["models/*"] },
      "types": ["miniprogram-api-typings"]
    },
    "typings": { "package": "miniprogram-api-typings", "version": "5.3.1" }
  },
  "scriptFileId": "pages/index/index.ts",
  "files": [
    { "fileId": "pages/index/index.ts", "path": "pages/index/index.ts", "kind": "ts", "text": "…", "sourceRevision": 12 },
    { "fileId": "models/user.ts", "path": "models/user.ts", "kind": "ts", "text": "…", "sourceRevision": 4 }
  ],
  "fragments": [
    { "fragmentId": "f0", "exprText": "user.name", "target": "name",
      "locals": [], "wxmlRange": { "start": 24, "end": 33 },
      "fragmentRevision": 1, "root": "page" }
  ],
  "policy": { "jsCheck": "inherit", "propertiesOverlay": "declaration" },
  "limits": { "maxFiles": 2048, "maxTotalBytes": 16777216, "maxFragments": 2048 }
}
```

- `files[]` 是本次分析的**完整**输入（含未保存 overlay、`models.ts`、`.d.ts`）；Node 不得对项目文件回退磁盘读取。
- `compilerOptions` 是 P3 解析后的**有效值**，覆盖 `strictNullChecks`、`noImplicitThis`、`noUncheckedIndexedAccess`、`exactOptionalPropertyTypes`、`jsx`、`lib`、`target`、`module`、`moduleResolution`、`typeRoots`、`experimentalDecorators`、`outDir` 等影响类型检查的开关；`tsconfig` 的 `extends`/`paths` 已在 P3 完成。`lib` 短名在 Node 侧规范化为 TypeScript 的 `lib.<name>.d.ts`；`typeRoots` 由 Rust 解析为绝对目录后传给 Node，只用于读取声明文件。按 [ADR-0006](../adr/0006-explicit-mini-program-roots.md)，app 根由初始化配置 `apps[].root` 指定，类型项目及 `tsconfig.json` 目录由 `apps[].projectRoot` 指定（默认 `root`）；不再扫描子 `app.json`、向上寻找最近 tsconfig，或按生成目录名称自动忽略/发现 app。
- 初始化配置的两个路径均相对 LSP workspace root（`rootUri` 或首个 `workspaceFolders` 根），不是 `.zed` 目录。省略、`null`、`{}`、`apps:[]` 均采用单 app `{root:"."}`；每项 `root` 必填。目录必须存在，canonical 后满足 `workspace ⊇ projectRoot ⊇ root`；绝对路径、`..`、symlink 越界、重复 root、超过 64 apps 或每路径超过 4096 UTF-8 bytes 均使 `initialize` 返回 `-32602`，不得静默 fallback。有效目录内的 `app.json` 缺失或内容错误仍走现有 project issues。配置仅启动时消费，修改后须重启 LSP，无根热更新；[Zed 示例与内层 JSON Schema](../../editors/zed/README.md#mini-program-roots-initialization-options) 另见配置指南。
- 上例 `project.projectRoot` 是所选类型项目根的 canonical **绝对目录**；初始化路径转换发生在 Rust 侧。`files[].path`、`fileId`、`scriptFileId` 等项目相对身份均以该 `projectRoot` 为基准，而不是 app 根；例如 `root:"src"`、`projectRoot:"."` 时，脚本路径为 `src/pages/index/index.ts`。文档 URI 按最长 app 目录匹配，以稳定 `projectId` 隔离；app 外、`projectRoot` 内的共享文件保留 active 或唯一 owner，多个 owner 且无 active 时不猜测。这不承诺完整多 workspace 或跨 app 依赖图支持。
- Node 只做**枚举名 → TypeScript 枚举**的转换与已知字段透传，不覆盖、不推断；**未知字段直接拒绝**（`RequestError.unsupported-option`），不得静默丢弃（静默丢弃会让模板按不同规则检查，产生假阴性/假阳性）。
- `strict:false` 或 `noImplicitThis:false` 会让 `this.data` 退化为 `any`：派生视图必须由探针健康门报 `Unavailable`（本契约不通过强制 `noImplicitThis` 掩盖项目选项）。
- `fragments[]` 是 Rust 侧的挂载输入；`fragmentId` 是稳定身份，响应逐片段结果按它回显（不得用表达式文本代替）。同一 `exprText` 可有多个 `fragmentId`。
- **C1.1 增量字段（可选，向后兼容）**：`target` 是默认 hover 与诊断的目标标识符（必须是 `exprText` 中按完整标识符边界可定位的 token；缺省为表达式的右值标识符，例如 `user.nmae` → `nmae`、`user` → `user`）；hover 现在通过 `hovers[]` 覆盖表达式中的每个标识符，`target` 仍作为兼容回退。`locals` 是模板作用域绑定名（如 `wx:for` 的 `item`/`index`）：其中的根标识符**不加** `__wxml_root.`/`__wxml_props_*` 前缀。Node 在挂载时必须按 TypeScript 表达式语义重写**每个自由根标识符**，不能只处理表达式首个 token：括号、一元、二元、条件、索引、调用和对象字面量都要正确处理；属性名（`.name`、`{ key: … }` 的 key）、字符串/注释、局部绑定和全局内建名不得加前缀。无法安全挂载的形状必须返回结构化 `bad-fragment`，不得发布假诊断或假 `Supported`。`target` 找不到、`locals` 重复/为空/非标识符/保留名 → `bad-fragment`。`locals` 可以合法覆盖 `Math` 等全局内建名，此时不得按全局名跳过。默认 target 按**最右值标识符的原始偏移**定位（`a.a` 指向第二个 `a`、`user.user` 指向第二个 `user`），不得用 `indexOf` 取首个同名 token。包装必须精确覆盖整个 `exprText`（恰好一个 `const __wxml_expr = (…);` 语句，且括号内 AST 结束位置等于 `exprText.length`），多余语句/括号逃逸 → `bad-fragment`。`satisfies`、任意 TypeNode、`new` 表达式等不安全形状 → `bad-fragment`。表达式遍历深度 > 256 或节点数 > 10,000 → `bad-fragment`，不得以 `RangeError`/`-32603` 结束。
- **Wave 5 结构化模板作用域**：protocol v2 的 `bindings[]` 携带完整嵌套链（外层到内层，每层 item/index），`sourceExpr`/`sourceRange` 指向产生绑定的 `wx:for` 表达式；Node 解析数组/元组/Record 的元素与索引类型，无法解析时降级 `any` 并报 `Partial`。`locals[]` 保留为兼容字段，由 bindings 名称派生。`guards[]` 携带 `wx:if/elif/else` 链和选中分支，Node 用合成 `if/else` 块完成窄化；复杂或不可解析守卫降级 `Partial`，不得伪造类型。反向编辑仍未实现。
- 容器字段（`viewId`/`owner`/`analysisStamp`）参与日志、缓存键与发布屏障。

### 6.2 输入上限

在解析与语言服务之前校验，超限返回 `Unavailable`（`request rejected (<code>)`），不得部分处理：

| 上限 | 默认值 | 服务端上限 |
| --- | --- | --- |
| `maxFiles` | 2048 | 2048 |
| `maxTotalBytes` | 16 MiB（`files[].text` + `fragments[].exprText` UTF-8） | 16 MiB（同左） |
| `maxFragments` | 2048 | 4096 |

- `limits` 允许缺省或只带部分字段：缺省字段取默认值，**不能因为缺字段而关闭检查**。
- 生效上限取 `min(请求值, 服务端上限)`：调用方只能收紧、不能放宽。
- 非正整数、非整数或未知枚举值分别返回 `bad-limit` / `bad-option`；未知字段返回 `unsupported-option`；`protocolVersion` 不匹配返回 `bad-protocol`。

### 6.3 响应（Node → Rust）

```jsonc
{
  "protocolVersion": 2,
  "requestId": "pages/index:view-7",
  "jsCheckApplied": "inherit",
  "registration": { "kind": "page", "count": 1, "topLevel": true },
  "states": {
    "hover": { "status": "supported" },
    "diagnostics": { "status": "partial", "reason": "…" },
    "definition": { "status": "supported" }
  },
  "insertions": [
    { "insertionId": "probe", "file": "pages/index/index.ts", "kind": "probe",
      "startUtf16": 8, "lengthUtf16": 120, "replaceLengthUtf16": 0,
      "viewRevision": 7, "backendFileVersion": 3 }
  ],
  "mounts": [ { "fragmentId": "f0", "mountId": "f0#0", "insertionId": "probe", "startUtf16": 40,
                "targetStartUtf16": 52, "targetLengthUtf16": 4,
                "segments": [ { "startUtf16": 40, "lengthUtf16": 12, "synthetic": true },
                              { "startUtf16": 52, "lengthUtf16": 9, "source": { "start": 150, "end": 159 } } ] } ],
  "fragments": [
    { "fragmentId": "f0",
      "hover": { "typeText": "string", "quality": "known", "documentation": "…", "documentationSource": "quickinfo" },
      "hovers": [
        { "range": { "start": 124, "end": 128 }, "typeText": "(property) user: { name: string; }", "quality": "known", "documentation": "…", "documentationSource": "quickinfo" },
        { "range": { "start": 129, "end": 133 }, "typeText": "string", "quality": "known", "documentation": "…", "documentationSource": "quickinfo" }
      ],
      "diagnostics": [ { "code": 2339, "range": { "start": 134, "end": 138 }, "message": "…", "attribution": "source" } ],
      "definition": { "kind": "script", "fileId": "pages/index/index.ts", "range": { "start": 136, "end": 140 } } }
  ],
  "suppressed": { "count": 1, "details": [ { "code": 2322, "message": "…" } ] },
  "moduleKind": { "before": "script", "after": "script" }
}
```

- **逐标识符 hover（C1.2，可选）**：`fragments[].hovers[]` 为同一表达式中每个可映射标识符返回一个 hover 候选，`range` 是 WXML UTF-8 字节范围。Rust 侧按请求光标位置选择包含该位置的候选并原样发布其 `range`；`fragments[].hover` 保留为默认 `target` 的兼容结果，旧 Node 后端没有 `hovers[]` 时仍按 `targetRange` 回退。每个表达式最多 32 个候选，单次响应最多 4096 个。
- **诊断范围**：`range` 是 WXML UTF-8 字节（唯一发布坐标）；派生文件里的 `spanUtf16` 属于内部实现，不出现在响应契约中。
- **定义范围**：`fileId` + `range`（目标源文件的 UTF-8 字节）；`startUtf16`/`lengthUtf16` 不发布。生成声明必须已映射回原脚本/`properties` 节点；无法映射时 `kind="dropped"`。
- `fragments[]` 与 `moduleKind` 由 **Node 适配层组装**（本仓库 `toAnalyzeResponse`），Rust 侧直接消费，不再二次拼装。
- `insertions[].file`/`mounts[].file` 与 `definition.fileId` 都使用 `files[].fileId` 身份空间；生成文件用 synthetic fileId，生成声明的定义发布原文件 fileId。
- 声明与发布：`Disabled` ⇒ `diagnostics` 为空；`Partial`/`Unsupported` 不得展示为“检查通过”。
- `registration.count` 与 `topLevel` 用于 `Partial` 归因；`topLevel=false` 时 Node 不得声称可锚定。
- `viewRevision`/`backendFileVersion` 由 P5 帧提供并校验；C1 只要求透传与日志一致，不在本层递增。

### 6.4 Node 后端 stdio 协议（P5）

传输为 **NDJSON**：一行一个 JSON-RPC 2.0 对象，stdout 只写协议行，日志写 stderr。
单行上限 **20 MiB**（大于请求 `maxTotalBytes` 16 MiB，为 JSON 转义/结构开销留余量），超限返回 `-32600`（`data.code = "bad-limit"`）并丢弃该行，不缓冲、不退出。`maxTotalBytes` 计入 `files[].text` 与 `fragments[].exprText` 的 UTF-8 字节；行上限仍是最终硬界。

| method | params | result |
| --- | --- | --- |
| `ping` | — | `{ "ok": true, "protocolVersion": 2, "backendVersion": "0.1.0" }` |
| `analyze` | C1 `AnalyzeRequest` | C1 `AnalyzeResponse` |
| `shutdown` | — | `{ "ok": true }`，随后进程退出；之后不再处理新请求 |

协议 v2 在 `analyze` 上预留可选输入：`fragments[].bindings`、`fragments[].guards`、顶层 `components` 与 `events`；缺少这些字段时行为与 v1 相同，响应中的 `eventResults`/`components` 当前被 Rust 侧忽略。`complete` 方法在 v2 中保留给补全 worktree，Phase 0 的 Node 服务端不处理它。

错误码：parse error `-32700`、invalid request `-32600`、method not found `-32601`、
invalid params `-32602`（`data.code` 为 `bad-protocol`/`bad-params`/`bad-limit`/`bad-option`/
`unsupported-option`/`bad-fragment`/`too-many-*`）、internal error `-32603`。
请求级 `requestId` 与 JSON-RPC `id` 都回显；一个坏请求不得终止进程；请求顺序处理即可。

## 7. 消息与 `code`

- 用户可见消息去掉 `__wxml_*`、`__WxmlProps_*`、`WechatMiniprogram.*`、`FilterUnknownType<…>`、`PropertyOptionToData<…>` 等内部符号；
- 重复交叉字面量去重、空白压缩、上限 200 字符（`…` 结尾）；
- `code` 只用于内部日志/测试；`rawMessage` 仅内部保留；
- hover 文档来源回传：`quickinfo` / `symbol` / `copied` / `none`；覆盖类型字段固定 `copied`。

## 8. ABI 保留名

| 名称 | 形态 | 说明 |
| --- | --- | --- |
| `__wxml_check_<n>` | 方法 | 探针；页面放 options，组件放 `methods` |
| `__wxml_root` | 值 | `const __wxml_root = this.data;` |
| `__WxmlProps_<n>` | 类型别名 / JSDoc typedef | 声明派生属性覆盖 |
| `__wxml_props_<n>` | 值 | 覆盖类型实例，只用于 hover/诊断 |
| `__wxml_props_raw_<n>` | 值 | JS 覆盖的 `unknown` 中转值 |

覆盖限制（不是缺陷）：`noImplicitThis:false`（即使 `strict:true`）会让 `this.data` 退化为 `any`，派生视图只能返回 `Unavailable`；本契约不通过强制 `noImplicitThis` 掩盖项目选项。

保留名不得出现在补全候选、定义结果或用户消息中。

## 9. 版本与断言

- TypeScript `6.0.3`、`miniprogram-api-typings` `5.3.1`、Node ≥ 24，均有断言测试。
- 必须覆盖：探针落点（含 2353 反例）、`optionalTypes` 覆盖（TS/JS）、JS 策略优先级与 `off` 中和、`@ts-nocheck`、pragma 注入点、位置替换（BOM）、合成前缀裁剪、定义回原脚本、模块翻转门、输入上限、读取白名单。
- 升级依赖需重跑矩阵并新增证据记录。

## 10. 延期与未决

| 项 | 状态 |
| --- | --- |
| AC-004 `properties` 未保存 overlay | 已测 `data` 跨文件 overlay；`properties` overlay 的未保存同步延期 |
| AC-007 runner 断言（JS 策略组合逐项） | 已测 `off`/`pragma`/`@ts-nocheck`/`checkJs:true`；完整 runner 断言延期 |
| AC-008 路径别名 / 多视图 | `alias-project` 已验证 `extends` + `paths` 的 hover 与 definition；`multi-view` 已验证不同页面/组件脚本的视图隔离与交错请求互不污染（当前发现模型是 1 WXML ↔ 1 脚本，同一脚本映射多个视图不在模型内，未验证） |
| 多注册/封装注册合并规则 | `Partial` + 计数 + 首个注册（§5.2） |
| `data` 简写/`data: importedData` | 简写为 `Unsupported`；imported 值按表达式处理 |
| 补全替换范围 / 反向编辑 | `deferred-to-C1`（AC-012） |
| 增量类型服务缓存 | 未验证；本轮每次重建 |
| 编译选项全集 | 已覆盖 §6.1 列出的字段并拒绝未知字段；`lib`/`jsx`/`strictBuiltinIteratorReturn` 透传有断言，未逐项验证语义；TypeScript 升级新增 strict 家族项需补测试 |
| harness 读取 tsconfig | `src/run.ts` 用 `ts.sys` 读取样例 tsconfig，模拟 P3；生产 Node 只能接收请求里的 `compilerOptions`，不得读 tsconfig |
| 请求声明上限 | 已加服务端上限（§6.2），调用方只能收紧 |
| 分片身份 | `fragmentId` 全链路携带；`mountId` 由其派生；重复 id → `bad-fragment` |
| 模板 locals / 守卫 | 协议 v2 携带 `bindings`（item/index 名称、角色、源表达式与范围）和 `guards`；数组/元组/`ReadonlyArray` 取元素类型、`Record`/对象取值类型与键类型，`wx:if/elif/else` 由 TypeScript 控制流窄化；`any`/`unknown`/无法解析时降级而不误报 |
| stdio 后端 | `src/server.ts` / `dist/server.js`；NDJSON JSON-RPC；20 MiB 行上限；`npm run build` 产出 `dist/server.js` |

验证（WT-B，2026-11-16）：

```bash
(cd backends/typescript && npm ci && npm run build)
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test wave5_acceptance -- --ignored
```

该 runner 覆盖 AC-005 `wx:for` locals 类型化、AC-006 条件窄化、AC-008 多视图隔离与 `@models/*` 别名 hover/definition、AC-012 组件属性绑定诊断；不含补全与事件处理函数（分别由 WT-A/WT-C 验证）。它被 `#[ignore]` 保护，常规门禁只编译不执行，因此“已验证”指上述显式真机门禁，而不是 `cargo test` 默认路径。

## 11. 变更记录

| 日期 | 变更 |
| --- | --- |
| 2026-11-15 | 第 1 轮评审修订：候选 B 移出契约；分能力状态；`insertions[]`/`mounts[]`；声明派生覆盖；检查策略；读取边界；消息清洗；定义溯源 |
| 2026-11-15 | 第 2 轮修订：请求/响应模型补齐（`fragments[]`、`compilerOptions`、`viewId`/owner、`registration`、输入上限）；多注册策略与 C0 对齐；`replaceLengthUtf16` 与 BOM 替换；`off` 中和源码 `@ts-check`；模块翻转门；定义指向派生文件时丢弃；`methods` 非字面量不挂载片段；`data` 简写 `Unsupported` |
| 2026-11-15 | 第 3 轮修订：`compilerOptions` 扩为近完整选项集并拒绝未知字段（`strictNullChecks`/`noImplicitThis`/`noUncheckedIndexedAccess`/`exactOptionalPropertyTypes`/`jsx`/`lib` 等）；`strict:false`/`noImplicitThis:false` 明确 `Unavailable`；响应诊断补 WXML 字节 `range`、定义改为 `fileId + range`、`targetLengthUtf16` 落地、`segments` 统一 `startUtf16/lengthUtf16`、`fragments[]`/`moduleKind` 入契约；AC-008 收窄 |
| 2026-11-15 | 第 4 轮修订：BOM 在“注入或源码 pragma 需要生效”时一律丢弃（含 `inherit`/`off` 中和）；`limits` 合并默认值并受服务端上限约束（`bad-limit`）；枚举值与 `protocolVersion` 校验（`bad-option`/`bad-protocol`）；挂载集合与策略改由请求驱动（`runRequest`）；`toAnalyzeResponse` 产出 C1 §6.3 响应 |
| 2026-11-15 | 冻结前补充：`insertions[].file`/`mounts[].file` 统一到 `files[].fileId` 身份空间（生成文件用 synthetic fileId）；生成声明的定义发布原文件 `fileId`；`lib` 透传断言 |
| 2026-11-15 | **C1.1 增量**：`RequestFragment` 增加可选 `target`/`locals`；`target` 默认为右值标识符，`locals` 中的根标识符不加 `__wxml_root.`/`__wxml_props_*` 前缀；Wave 2 `locals` 以 `any` 绑定，含 locals 的片段三能力必须 `Partial`；完整 `wx:for` 元素类型（`locals.from`/`iteration`）延期 C1.2/Wave 4。`analyze(request)` 成为真实后端入口；新增 NDJSON JSON-RPC stdio 服务（`ping`/`analyze`/`shutdown`）。 |
| 2026-11-15 | P5 HIGH 修复：表达式发射改为递增 source/mounted 游标（不再 `slice(0,from)`/每片 reduce），roots 用 Set。性能回归固定为 9k roots、padding 在 roots 之前的最坏排列，阈值 1 MiB <300 ms、4 MiB <600 ms（旧 O(n²) 同排列约 4.2 s / 15.6 s）。越界/非字符/小数边界严格 `unattributable`，target 不在 span 内改用最大重叠段。错误消息截断到 120 字符 + 总长度；`locals` 可覆盖全局名；`maxTotalBytes` 计入 fragments；深度从根起算；heredoc 包装精确校验。 |
| 2026-11-15 | P5 第 1 轮修复：`insertions[]` 填 `viewRevision`/`backendFileVersion`；`locals` 根偏移与 dropped definition 状态修正；`target` 默认右值标识符；`locals` 保留字校验；运行时依赖移入 `dependencies`；stdio 行上限 20 MiB（请求上限仍 16 MiB）；`ping` 回传协议/后端版本。第 2 轮补充：Node 必须重写表达式中的每个自由根标识符（括号/一元/二元/条件/索引），不能只前缀首个 token；`wxmlRange` 长度必须等于 `exprText` UTF-8 字节数 |
| 2026-11-15 | P5 MEDIUM 修复：默认 target 使用最右值 token 的原始偏移（重复名 `a.a`/`user.user`）；包装只接受精确覆盖整个表达式的单个 ParenthesizedExpression（拒绝 `); (` 注入）；跨合成间隙诊断裁剪到 target/最大重叠原文段，无法归属时 diagnostics 至少 `Partial`；拒绝 `satisfies`/TypeNode/`new`；表达式深度/节点上限并捕获 RangeError |
| 2026-11-16 | **WT-B（Wave 5）**：`wx:for` locals 升级为 `bindings` 并解析元素/索引类型，`wx:if/elif/else` 增加 `guards` 窄化；删除“template locals are bound as any”降级；新增 `tests/fixtures/{loop-scope,conditional-union,component-bindings,multi-view,alias-project}` 的 `wave5_acceptance` 真机 runner；多视图隔离（不同脚本、交错请求）与 `@models/*` 别名 definition 验证，详见 §10 验证说明 |
| 2026-11-16 | **Zed 验收修复**：页面视图没有 `Page` 注册时回退到首个 `Component` 注册，并把 `registration.kind` 置为 `component`；真实 TS 项目 `jielong-team-doc` 的 `Component` 页面已验证 hover/definition，Node 单测覆盖 `Component` 注册页面 |
| 2026-11-16 | **hover 与真实项目修复（历史行为）**：analyze 响应的 `fragments[].hovers[]` 为表达式中每个标识符返回 hover 候选，Rust 按光标选择；当时在工作区根无 `app.json` 时自动发现嵌套 app 根（深度 6、4096 目录、跳过依赖/构建目录），该发现策略后由 [ADR-0006](../adr/0006-explicit-mini-program-roots.md) 替代；声明属性全部走 `properties` overlay，不再依赖 `hasOptionalTypes`；源文件预算提升到 2048 / 16 MiB；待处理任务队列 32 → 128，避免连续 hover 报 Busy |
| 2026-11-16 | **tsconfig 与编辑后 hover 修复（历史行为）**：app.json 与 tsconfig 可分层（`open_with_app_json` + `source_root`），当时从 app 根向上找最近的 tsconfig，该推断策略后由 [ADR-0006](../adr/0006-explicit-mini-program-roots.md) 的显式 `projectRoot` 替代；绝对 `usingComponents` 仍按小程序根解析；`target`/`module`/`moduleResolution`/`jsx`/`moduleDetection` 大小写归一化，`lib` 短名转 `lib.<name>.d.ts`，支持 `typeRoots`/`experimentalDecorators`/`outDir`；每次 `didChange` 取消同一文档旧的 diagnostics 任务，避免快速输入后 hover 排在多个过期分析后 |
| 2026-11-16 | **显式小程序根配置**：[ADR-0006](../adr/0006-explicit-mini-program-roots.md) 固定 `initialize.initializationOptions.apps[]` 的 `root`/`projectRoot`、默认单 app、路径包含与资源上限、`-32602` 失败和重启生效规则；取消自动 app/最近 tsconfig 推断，明确最长 app 路由、稳定项目隔离及共享文件歧义边界；不将本次文档更新作为真实 Zed 多 app 验收证据 |
