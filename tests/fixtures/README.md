# 共享验收样本

本目录保存 AC-001..AC-013 的正例、反例和运行时场景，是各 crate 共用的唯一验收样本来源。样本全部为本项目自造，不包含第三方代码。

每个静态样本是一棵可直接打开的真实小程序工程（`app.json`、`tsconfig.json`、页面、组件、脚本）。crate 测试把这些目录当作项目根直接读取，不再经过独立的校验 crate 或 Node runner；原 `wxml-fixtures` crate 与 `wave5_acceptance` runner 已随 ADR-0009 删除。

当前被 Rust 测试直接打开的样本是 `component-basic`、`component-bindings`、`events-basic`；其余目录保留为 AC 样本目标与对照。`manifest.json` 记录样本清单与 AC 对应关系，`expected/` 保存 AC 查询与诊断锚点；两者是人工维护的样本记录，当前没有代码加载，因此改动样本时必须人工保持同步。

## 1. 目录结构

```text
tests/fixtures/
├── manifest.json              # 样本清单（schema v2，人工维护）
├── README.md                  # 本文件
└── <case-id>/                 # 静态样本（kind = fixture）
    ├── app.json
    ├── tsconfig.json
    ├── tsconfig.base.json     # 使用 extends 的样本
    ├── pages/<page>/...        # .ts/.js/.wxml/.json
    ├── components/<name>/...   # 组件样本
    ├── templates/*.wxml        # import/include 样本
    └── expected/
        ├── hover.json
        ├── diagnostics.json
        └── definition.json    # 可选，AC-012
```

`runtime` 场景没有目录和磁盘样本，只写在 `manifest.json`。

## 2. manifest v2

以下 schema 是 C0 冻结的清单约定，保留为样本记录格式。原校验器已随 ADR-0009 删除，字段不再被机器强制；修改样本时仍按下表更新，保证清单与 `expected/` 可回溯。

```json
{
  "version": 2,
  "cases": [
    {
      "id": "page-basic",
      "kind": "fixture",
      "acs": ["AC-001", "AC-002"],
      "root": "page-basic",
      "positive": ["{{ title }}", "{{ user.name }}"],
      "negative": ["{{ user.nmae }}"],
      "expected": {
        "hover": "expected/hover.json",
        "diagnostics": "expected/diagnostics.json",
        "definition": "expected/definition.json"
      },
      "coverage": { "edits": "deferred-to-C1" }
    },
    {
      "id": "unsaved-overlay",
      "kind": "runtime",
      "acs": ["AC-004"],
      "scenario": "……",
      "steps": [
        { "action": "didOpen index.wxml", "expect": "diagnostics published" },
        { "action": "didChange index.ts without save", "expect": "hover returns the new type" }
      ]
    }
  ]
}
```

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `version` | 是 | 固定为 `2`；修改 schema 必须递增 |
| `cases[].id` | 是 | 全清单唯一 |
| `cases[].kind` | 是 | `fixture` 或 `runtime` |
| `cases[].acs` | 是 | AC-001..AC-013，且每个编号至少被一个 case 覆盖 |
| `cases[].root` | fixture | 相对本目录的样本根 |
| `cases[].positive` | fixture | 必须与 `hover.json` 的 marker **逐项计数**一致；跨文件重复的 marker 需重复列出 |
| `cases[].negative` | fixture | 必须与 `diagnostics.json` 的 marker 逐项计数一致；详见 §5 |
| `cases[].expectClean` | 可选 | `true` 表示已检查且无诊断：`diagnostics` 必须为空数组、`negative` 必须为空 |
| `cases[].requirePositions` | 可选 | `true` 时每条 hover 查询必须给出 `position`（`unicode` 使用） |
| `cases[].expected.hover` | fixture | 相对 `root` 的路径 |
| `cases[].expected.diagnostics` | fixture | 相对 `root` 的路径 |
| `cases[].expected.definition` | 可选 | 相对 `root` 的路径 |
| `cases[].coverage` | 可选对象 | 记录覆盖范围；AC-012 必须写 `edits` |
| `cases[].scenario` | runtime | 非空、不含 TODO |
| `cases[].steps` | runtime | 至少 2 项，每项 `action`/`expect` 非空且不含 TODO |

所有 `expected` 路径必须位于该 fixture root 内；不得使用绝对路径、`..` 或 symlink 逃逸。

## 3. marker 与 target

### 3.1 marker

1. marker 是文件中的**字面量**子串，按 `str::matches` 计数，不是正则。
2. marker 必须在该 `file` 中**恰好出现一次**。
3. 同一表达式重复出现时，用周围文字或完整属性区分（如 `外层组名：{{ item.name }}`）。
4. marker 不得为空；中文、emoji 直接写入，不做编码转换。
5. 同一 marker 可合法出现在两个文件（如 LF/CRLF 变体）；`positive`/`negative` 需按出现次数重复列出，比较时按多重集处理，删除任一条都会不一致。

### 3.2 target

`target` 是类型检查器实际查询/报错的 token，必须是 marker 的子串，并在 marker 内**恰好出现一次**：

| 场景 | target |
| --- | --- |
| hover | 被悬停的标识符/表达式（`user.name`、`state.user.name`） |
| 属性不存在 | 属性名（`nmae`、`age`、`subtitle`） |
| 可空错误 | 可空对象表达式（`maybeUser`） |
| `unknown` 访问 | unknown 表达式（`external`） |
| 组件属性不匹配 | 绑定表达式（`total`、`cardObject`） |
| 事件缺失 | handler 名（`missingHandler`） |

断言诊断时 `Diagnostic.range` 必须**精确等于** target 的字节范围，不得用 `contains` 放宽。hover 查询点取 target 起点。

## 4. `expected/hover.json` v2

```json
{
  "queries": [
    {
      "file": "pages/index/index.wxml",
      "marker": "{{ user.name }}",
      "target": "user.name",
      "position": { "line": 1, "character": 20 },
      "expect": {
        "type": "string",
        "typeKind": "string",
        "documentation": "用户显示名称",
        "status": "supported",
        "quality": "known"
      }
    }
  ]
}
```

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `file` | 是 | 相对 root 的 `.wxml` 文件 |
| `marker` | 是 | 唯一字面量片段 |
| `target` | 是 | 查询/悬停的 token |
| `position` | 可选 | target 起点的零基行号 + UTF-16 列；由 `LineIndex` 计算并精确匹配 |
| `expect.type` | 是 | 展示文本 |
| `expect.typeKind` | 是 | `string`/`number`/`boolean`/`any`/`unknown`/`union`/`object`/`alias`/`missing`/`other` |
| `expect.documentation` | 是 | 字段说明；没有写 `""` |
| `expect.status` | 可选 | 六种 `CapabilityStatus`，缺省 `supported` |
| `expect.quality` | 可选 | `known`/`any`/`unknown`/`missing`，缺省 `known` |
| `expect.rangeTarget` | 可选 | hover 返回范围与 target 不同时使用；本轮未使用 |

`type` 是展示文本；语义比较以 `typeKind` 为准，避免 `typeToString` 空白或顺序差异造成假失败。

## 5. `expected/diagnostics.json` v2

```json
{
  "status": "supported",
  "diagnostics": [
    {
      "file": "pages/index/index.wxml",
      "marker": "{{ user.nmae }}",
      "target": "nmae",
      "severity": "error",
      "messageContains": "nmae",
      "status": "supported"
    }
  ]
}
```

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `diagnostics` | 是 | 诊断数组，可为空 |
| `status` | 可选 | 本次检查的整体状态；缺省视为 `supported` |
| `diagnostics[].file` | 是 | 相对 root 的 `.wxml` 文件 |
| `diagnostics[].marker` | 是 | 唯一字面量片段 |
| `diagnostics[].target` | 是 | 应被精确标记的 token |
| `diagnostics[].severity` | 是 | `error`/`warning`/`information`/`hint` |
| `diagnostics[].messageContains` | 是 | 消息必须包含的片段，大小写敏感 |

query、expect、diagnostic 对象只应出现文档列出的字段；未知字段（如 query 级 `rangeTarget`、`diagnostics[].status`）不应出现（原校验器已删除，该约束由书写约定维持）。结果层的检查状态由 `DiagnosticsResult.status` 表达，单条 `Diagnostic` 没有 status。

- 只有 `status == supported`（或缺省）且 `diagnostics` 为空才表示“干净”，配合 `expectClean: true`。
- 未支持/未知形式使用 `diagnostics: []` + 顶层 `status: "unsupported"`/`"unknown"`，且 `negative: []`。这种“降级空诊断”是允许的，但 `expectClean` 不能与降级状态同时使用。
- 不得用空 marker 或伪造诊断表达降级。

## 6. `expected/definition.json` v2

```json
{
  "queries": [
    {
      "file": "pages/index/index.wxml",
      "marker": "bind:tap=\"onCardTap\"",
      "target": "onCardTap",
      "expect": { "file": "pages/index/index.ts", "target": "onCardTap" }
    },
    {
      "file": "pages/index/index.wxml",
      "marker": "bindtap=\"missingHandler\"",
      "target": "missingHandler",
      "expect": null
    }
  ]
}
```

- `file` 可为 `.wxml`/`.ts`/`.js`；marker 唯一。
- `expect == null` 表示“没有定义/不能安全映射时不得返回位置”。
- 正例的 `expect.file` 必须在 root 内、可读，`expect.target` 在目标文件中唯一出现。
- 只要求 definition；反向编辑/重命名至今未实现，AC-012 用 `coverage.edits = "deferred-to-C1"` 记录（字段名保留为历史）。

## 7. 验收编号对应

| AC | 样本 | 关注点 |
| --- | --- | --- |
| AC-001 | `page-basic`、`component-basic`、`cross-file-type` | Page `data`、Component `data`/`properties`、跨文件类型与 JSDoc |
| AC-002 | `page-basic`、`page-basic-clean` | 错误字段精确标记；修复后诊断清空 |
| AC-003 | `unicode` | 中文、emoji、LF/CRLF 的 UTF-16 位置 |
| AC-004 | `unsaved-overlay`（runtime） | 未保存的类型和 JSDoc 更新 |
| AC-005 | `loop-scope` | 嵌套 `wx:for`、默认/自定义 item/index、遮蔽、对象遍历、退出后不泄漏 |
| AC-006 | `conditional-union` | `wx:if/elif/else`、可空与判别联合 |
| AC-007 | `js-jsdoc` | JS JSDoc、`checkJs: false`、不重复发布 JS 诊断 |
| AC-008 | `alias-project`、`multi-view` | `extends` + 路径别名；同脚本多视图与组件隔离 |
| AC-009 | `stale-request`（runtime） | 旧结果不发布、后端退出与世代恢复 |
| AC-010 | `zed-multi-doc`（runtime）、`multi-view` | Zed 多文档实例归属与主服务并存 |
| AC-011 | `any-unknown`、`unsupported-forms`、`interface-root` | `any`/`unknown`、未支持形式、interface 根不伪造类型 |
| AC-012 | `component-bindings`、`events-basic` | 组件属性绑定、事件绑定、definition |
| AC-013 | `component-basic`、`component-properties` | `properties` 的 `type`/`optionalTypes`/JSDoc，未声明属性不伪造 |

### 7.1 运行时验收（WT-B 样本）

§7 的编号对应描述的是样本目标。原 `wave5_acceptance` Node runner 与 `backends/typescript` 已随 P7 删除，下面两条命令已不可执行，仅作为历史记录保留：

```bash
(cd backends/typescript && npm ci && npm run build)
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test wave5_acceptance -- --ignored
```

| 样本 | 原 runner 断言的行为（样本目标） |
| --- | --- |
| `loop-scope` | 8 条 hover：外层 `itemIndex` 为 number，`item.name`/`item.title` 为 string；内层默认 `index` 为 number、`item.name` 为 string；退出内层后恢复 `item.title`；`groupMap` 的 `index` 为 string；循环外 `{{ item }}` 在 `item` 上报错；`itemIndex` hover 不带文档 |
| `conditional-union` | 3 条 hover 验证 `wx:elif`/`wx:else` 与真值窄化；`state.user.age` 在 `age` 上报错；守卫外 `maybeUser` 报 null 错误，守卫内通过 |
| `component-bindings` | 2 条属性绑定 hover（`pageTitle` string、`total` number）；`title="{{ total }}"` 在 `total` 上报 number 不兼容。事件 marker（`bind:tap="onCardTap"` definition、`bindtap="missingHandler"` 诊断）归 WT-C：WT-B 的断言按 marker 前缀过滤事件项，事件两项由同一测试的 WT-C 断言覆盖 |
| `multi-view` | 7 条 hover 分别命中 card/badge/a/b 自己的声明与 JSDoc；`card.wxml` 的 `user.nmae` 只在该文件报错；同时打开两个页面、card 组件和两个脚本时互不污染 |
| `alias-project` | `@models/*` 别名下 `user.name` hover 为 string；`user.nmae` 在 `nmae` 报错；definition 跨别名解析到 `models/user.ts` 的 `name` |

该 runner 当时被 `#[ignore]` 保护，默认门禁只编译不执行；它覆盖 WT-B 的 AC-005/006/008/012，以及 WT-C 在 `component-bindings` 与 `events-basic` 中的事件诊断、definition 和事件值补全。这些行为现在由 native 引擎测试按需覆盖（如 `crates/wxml-lsp/src/backend_tsgo/tests.rs` 中的 events-basic / component-bindings 用例，以及 `crates/wxml-backend-tsgo/tests/real_engine.rs` 的 opt-in 真实引擎用例）。

### 7.2 事件处理函数样本（WT-C）

`events-basic` 是一个自包含小程序工程，专门覆盖事件处理函数的诊断、definition 与补全；组件属性绑定仍由 `component-bindings` 负责，两者同属 AC-012。

```text
events-basic/
├── app.json                          # pages: index / multi / nonliteral
├── tsconfig.json                     # strict + ES2020（与 component-bindings 一致）
├── pages/index/index.{json,ts,wxml}  # Page：方法简写/函数属性/箭头属性
├── pages/multi/multi.{ts,wxml}       # 两次 Page(...) 注册
├── pages/nonliteral/nonliteral.{ts,wxml}  # Page(options) 非字面量注册
├── components/widget/widget.{json,ts,wxml} # Component methods
└── expected/{hover,diagnostics,definition}.json
```

| 维度 | 绑定 / 位置 | 期望 |
| --- | --- | --- |
| Page 方法简写 | `bindtap="onShorthandTap"` | definition → `pages/index/index.ts` |
| Page 函数属性 | `catchtap="onFunctionTap"` | definition → `pages/index/index.ts` |
| Page 箭头属性 | `bind:tap="onArrowTap"` | definition → `pages/index/index.ts` |
| capture-bind | `capture-bind:tap="onCaptureTap"` | definition → `pages/index/index.ts` |
| capture-catch | `capture-catch:tap="onCaptureCatchTap"` | definition → `pages/index/index.ts` |
| mut-bind | `mut-bind:tap="onMutTap"` | definition → `pages/index/index.ts` |
| catch | `catch:tap="onCatchTap"` | definition → `pages/index/index.ts` |
| 缺失处理函数 | `bindtap="missingHandler"` | Error 诊断（target 精确为 `missingHandler`，消息含同名），definition 为 `null` |
| 动态绑定 | `bindtap="{{dynamicHandler}}"` | 无诊断；不做 definition 断言（表达式本身可解析，避免把“未支持”误判成“找不到”） |
| 空绑定 | `bindtap=""` | 无诊断；无 token，不做 definition 断言 |
| 非标识符 | `bindtap="this.onTap"` | 无诊断；不做 definition 断言 |
| Component 方法 | `bindtap="onWidgetTap"` | definition → `components/widget/widget.ts` |
| Component 箭头属性 | `bindtap="onWidgetArrow"` | definition → `components/widget/widget.ts` |
| Component 缺失处理函数 | `bindtap="missingWidgetHandler"` | Error 诊断（消息含同名），definition 为 `null` |
| 多注册降级 | `pages/multi`：`onSecondTap` 只在第二个 `Page(...)` | Partial，无缺失诊断，definition 为 `null` |
| 非字面量降级 | `pages/nonliteral`：`Page(options)` 内的 `onNonLiteralTap` | `Unsupported`（沿用既有视图策略），无缺失诊断，definition 为 `null` |

`hover.json` 的 3 条正例是 data/properties 锚点（`label`、`title`、`firstLabel`），用于确认样本工程本身可分析；本轮不承诺事件处理函数自身的 hover。

`pages/index/index.wxml` 同时包含动态、空、非标识符三种未支持事件形式；这些片段被记录但不产生诊断，也不会单独降级状态。测试不要断言该文件的整体 `status`，只断言具体诊断与缺失噪声。

`diagnostics.json` 只能表达“某处必须有诊断”，不能表达“某处必须没有”，因此恢复自动断言时需要补上文件级检查：

- `pages/index/index.wxml` 恰好 1 条诊断，即只有 `missingHandler`；`bindtap="{{dynamicHandler}}"`、`bindtap=""`、`bindtap="this.onTap"` 必须不产生诊断（按诊断范围与这些片段不重叠逐项检查）。
- `components/widget/widget.wxml` 恰好 1 条诊断，即只有 `missingWidgetHandler`。
- `pages/multi/multi.wxml`（`bindtap="onSecondTap"`）与 `pages/nonliteral/nonliteral.wxml`（`bindtap="onNonLiteralTap"`）没有任何诊断：多注册按 `Partial`、非字面量根按既有策略 `Unsupported`，均不报缺失。
- `definition.json` 的 `expect: null` 表示 locations 为空；“找不到”与“后端不可用”仍按 `CapabilityStatus` 区分。对 `expect: null` 可以接受 `supported`/`partial`/`unsupported`，所以动态、空、非标识符绑定不写 definition 期望，改用上面的诊断缺失断言。

延期断言（当前无自动执行者）：

- AC-007 的“不重复发布 JS/TS 主服务已报告的诊断”；
- AC-004 的 `properties` overlay 变体（当前 runtime 场景只描述 `data` 路径）。

manifest 的 `coverage` 只记录覆盖范围，不代表对应断言已经执行。

## 8. 换行约定

- 普通 `.wxml` 使用 LF，不得包含 `\r`。
- 以 `-crlf.wxml` 结尾的文件必须是 CRLF：每个 `\n` 前都是 `\r`，且至少包含一个 `\r\n`。
- 根 `.gitattributes` 用 `tests/fixtures/**/*.wxml text eol=lf` 保证普通样本在 Windows 检出仍为 LF，再用 `tests/fixtures/**/*-crlf.wxml -text` 保持 CRLF 字节精确；行尾约束依赖 Git 配置与样本字节本身，没有独立校验器再检查。

## 9. 新增样本

1. 在 `tests/fixtures/` 下新建与 case id 相同的目录。
2. 至少提供 `app.json`、`tsconfig.json`、模板与脚本、`expected/hover.json`、`expected/diagnostics.json`。
3. 每个正例和反例都要有唯一 marker 和明确 target；不要用“存在任意 hover”代替具体断言。
4. 在 `manifest.json` 增加 case，`positive`/`negative` 与 expected 的 marker 集合完全一致。
5. 运行 `cargo test -p wxml-lsp --all-features`；需要真实类型时按 `crates/wxml-backend-tsgo/tests/real_engine.rs` 的说明设置 `TSGO_TYPINGS_ROOT`（必要时设 `WXML_REQUIRE_REAL_ENGINE=1`）。
6. 新增外部来源样本前先更新 `docs/reference/provenance.md`；默认只接受自造样本。

## 10. 查询基准样本（bench-queries）

`bench-queries/` 不是 AC 样本，而是 `crates/wxml-lsp/benches` 的查询基准工程：真实小程序目录，含 `app.json`（全局 `usingComponents`）、`app.wxss`（`@import "styles/theme.wxss"`）、`tsconfig.json`、`pages/index/index.{wxml,ts,wxss}`（`index.wxss` 再 `@import "index-extra.wxss"`）与 `components/outer-card/outer-card.{json,ts,wxml,wxss}`。它不进入 `manifest.json`，不改变 AC 覆盖关系。

bench 按**唯一字面量子串**定位（与 §3.1 的 marker 规则一致），当前锚点：

| 组 | 文件 | 唯一锚点 |
| --- | --- | --- |
| `query/type/root` | `pages/index/index.wxml` | `{{ title }}` |
| `query/type/member` | `pages/index/index.wxml` | `{{ user.name }}` |
| `query/type/local` | `pages/index/index.wxml` | `{{ item.label }}`（同块含 `wx:for="{{ rows }}" wx:for-item="item"`） |
| `query/tag/builtin` | `pages/index/index.wxml` | `<view class="card">` |
| `query/tag/component` | `pages/index/index.wxml` | `<outer-card title="基准卡片">` |
| `query/tag/attribute` | `pages/index/index.wxml` | `<view class="page-shell"`（悬停 `class`） |
| `query/tag/value` | `pages/index/index.wxml` | `<input type="text" />` |
| `query/class/hover` | `pages/index/index.wxml` | `class="card"` |
| `query/class/site` | `pages/index/index.wxml` | `class="card {{extra}}"` |

`class="card"` 同时是内置标签锚点；`.card` 在 `app.wxss` 与 `pages/index/index.wxss` 各声明一次（带注释），`query/class/definition` 期望命中页面 WXSS。`card`、`card-title`、`card-footer`、`card-body`、`card-header`、`card-kicker`、`card-meta`、`card-highlight`、`card-caption`、`card-extra` 共享 `car` 前缀，供补全用例在光标前缀处取值。样式索引合计 33 个不同类名（含 partial；`app.wxss`/`index.wxss`/`theme.wxss` 本体为 31 个），`styles/theme.wxss` 与 `pages/index/index-extra.wxss` 用于覆盖 `@import` 链。

修改该样本时必须同步 bench 锚点；本目录不提供 `expected/`，正确性断言写在 bench 场景内。

## 11. 组件类型声明样本（components-typings）

`components-typings/` 是一棵自包含的小程序工程，专门覆盖「第三方组件类型声明」这条链路：与 `tsconfig.json` 同级的 `components.d.ts` 被发现 → 声明事实覆盖脚本推断 → 属性绑定、事件名与 hover 使用声明。它不使用 `node_modules`，也不携带第三方代码：`app.json` 里的 `van-button` 指向 `/miniprogram_npm/@vant/weapp/button/index`，该目录在样本中**故意不存在**，因此该组件没有任何本地脚本，属性与事件事实只能来自声明文件。

```text
components-typings/
├── app.json                          # pages + 全局 usingComponents（van-button → /miniprogram_npm/...）
├── tsconfig.json                     # 常规 include（**/*.ts），不参与声明发现
├── components.d.ts                   # 与 tsconfig.json 同级：WxmlComponentMap → VanButtonProps
└── pages/index/index.{json,ts,wxml}  # Page + 人工样例
```

启用与关闭只靠文件本身：

- **启用**：在项目 `tsconfig.json` 同级新增 `components.d.ts`。
- **关闭**：删除该文件；不需要改 tsconfig，也不需要在 tsconfig 里写 `files`/`include`/`paths` 指向它。
- 其他位置（如 `types/components.d.ts`）的同名文件不生效，这也是本样本把声明放在根目录的原因。

`components.d.ts` 的约定：

- 入口为 `declare global { interface WxmlComponentMap { 'van-button': VanButtonProps } }`，文件以 `export {}` 结束，保持模块写法。
- `VanButtonProps.props` 含字符串字面量联合 `type?: 'primary' | 'default' | 'warning'`、可选 boolean `loading?: boolean`、必填 `text: string`。
- `VanButtonProps.events` 含 `change: { detail: { index: number } }` 与 `input: { detail: { value: string } }`（后者用于事件名补全）。
- 文件末尾的 `interface OrphanButtonProps` 未被 `WxmlComponentMap` 引用，是「未被引用的接口不生效」的人工样例。

`pages/index/index.wxml` 的人工样例（每个片段是唯一字面量，可直接作 marker）：

| 位置 | 片段 | 期望 |
| --- | --- | --- |
| 正例 | `type="primary" text="{{ buttonText }}" loading="{{ loading }}" bind:change="onButtonChange"` | 无诊断；`buttonText` 为 string、`loading` 为 boolean，与声明兼容；`change` 是已声明事件 |
| 反例 1（未声明属性） | `round="{{ true }}"` | 在 `round` 上报未声明属性；声明里没有该属性 |
| 反例 2（绑定类型不匹配） | `text="{{ loading }}"` | 在绑定表达式 `{{ loading }}` 上报类型不匹配（`string` ← `boolean`），范围落在表达式而非属性名 |
| 反例 3（未声明事件名） | `bind:chaneg="onButtonChange"` | 在 `chaneg` token 上报 `Warning`；声明只有 `change`/`input` |

`pages/index/index.ts` 的 `onButtonChange` 形参显式标注 `WechatMiniprogram.CustomEvent<{ index: number }>`，与 `change` 的 detail 结构一致，因此不产生「handler 参数不兼容」诊断；事件名拼写错误的片段复用它，避免同时触发「处理函数缺失」。

约定与限制：

- 全部文件为 LF，无尾随空格；`.wxml` 由 `.gitattributes` 的 `tests/fixtures/**/*.wxml text eol=lf` 保护。
- 本样本不提供 `expected/`，也尚未登记到 `manifest.json`；期望行为以 crate 内的组件声明/事件测试断言为准，人工核对时按上表执行。
- 不要为它添加 `node_modules`、`miniprogram_npm` 或其它第三方代码；需要新的声明样例时直接扩充同级 `components.d.ts`（一个映射表可以登记多个标签）。
