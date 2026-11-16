# 共享验收样本

本目录保存 AC-001..AC-013 的正例、反例和运行时场景，是各 crate 共用的唯一验收样本来源。样本全部为本项目自造，不包含第三方代码。

结构校验由 `crates/wxml-fixtures/tests/manifest.rs` 执行，入口：

```bash
cargo test -p wxml-fixtures
```

校验器不执行类型分析；它拒绝坏样本，保证后续 runner 拿到的是可定位、可断言的数据。

## 1. 目录结构

```text
tests/fixtures/
├── manifest.json              # 样本清单（schema v2）
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
| `version` | 是 | 固定为 `2`；修改 schema 必须递增并更新校验器 |
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

所有 `expected` 路径必须位于该 fixture root 内；校验器拒绝绝对路径、`..` 和 symlink 逃逸。

## 3. marker 与 target

### 3.1 marker

1. marker 是文件中的**字面量**子串，按 `str::matches` 计数，不是正则。
2. marker 必须在该 `file` 中**恰好出现一次**。
3. 同一表达式重复出现时，用周围文字或完整属性区分（如 `外层组名：{{ item.name }}`）。
4. marker 不得为空；中文、emoji 直接写入，不做编码转换。
5. 同一 marker 可合法出现在两个文件（如 LF/CRLF 变体）；`positive`/`negative` 需按出现次数重复列出，校验器按多重集比较，删除任一条都会失败。

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

runner 对诊断必须断言 `Diagnostic.range` **精确等于** target 的字节范围，不得用 `contains` 放宽。hover 查询点取 target 起点。

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

`type` 是展示文本；后续 runner 以 `typeKind` 做语义比较，避免 `typeToString` 空白或顺序差异造成假失败。

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

query、expect、diagnostic 对象只允许文档列出的字段；未知字段（如 query 级 `rangeTarget`、`diagnostics[].status`）会被校验器拒绝。结果层的检查状态由 `DiagnosticsResult.status` 表达，单条 `Diagnostic` 没有 status。

- 只有 `status == supported`（或缺省）且 `diagnostics` 为空才表示“干净”，配合 `expectClean: true`。
- 未支持/未知形式使用 `diagnostics: []` + 顶层 `status: "unsupported"`/`"unknown"`，且 `negative: []`。校验器允许这种“降级空诊断”，但 `expectClean` 不能与降级状态同时使用。
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
- 只要求 definition；反向编辑/重命名在 C1/G4 前不实现，AC-012 用 `coverage.edits = "deferred-to-C1"` 记录。

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
| AC-012 | `component-bindings` | 组件属性绑定、事件绑定、definition |
| AC-013 | `component-basic`、`component-properties` | `properties` 的 `type`/`optionalTypes`/JSDoc，未声明属性不伪造 |

### 7.1 运行时验收（WT-B 样本）

§7 的编号对应描述的是样本目标；结构校验只执行 `cargo test -p wxml-fixtures`，不做类型分析。`loop-scope`、`conditional-union`、`component-bindings`、`multi-view`、`alias-project` 的类型行为由真实 Node runner 执行：

```bash
(cd backends/typescript && npm ci && npm run build)
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test wave5_acceptance -- --ignored
```

| 样本 | runner 覆盖的行为 |
| --- | --- |
| `loop-scope` | 8 条 hover：外层 `itemIndex` 为 number，`item.name`/`item.title` 为 string；内层默认 `index` 为 number、`item.name` 为 string；退出内层后恢复 `item.title`；`groupMap` 的 `index` 为 string；循环外 `{{ item }}` 在 `item` 上报错；`itemIndex` hover 不带文档 |
| `conditional-union` | 3 条 hover 验证 `wx:elif`/`wx:else` 与真值窄化；`state.user.age` 在 `age` 上报错；守卫外 `maybeUser` 报 null 错误，守卫内通过 |
| `component-bindings` | 2 条属性绑定 hover（`pageTitle` string、`total` number）；`title="{{ total }}"` 在 `total` 上报 number 不兼容；事件 marker 属于 WT-C，runner 明确跳过 |
| `multi-view` | 7 条 hover 分别命中 card/badge/a/b 自己的声明与 JSDoc；`card.wxml` 的 `user.nmae` 只在该文件报错；同时打开两个页面、card 组件和两个脚本时互不污染 |
| `alias-project` | `@models/*` 别名下 `user.name` hover 为 string；`user.nmae` 在 `nmae` 报错；definition 跨别名解析到 `models/user.ts` 的 `name` |

该 runner 被 `#[ignore]` 保护，默认门禁只编译不执行；它只覆盖 WT-B 的 AC-005/006/008/012，不覆盖补全与事件处理函数。

延期断言（P6/P7 runner，不是当前校验器）：

- AC-007 的“不重复发布 JS/TS 主服务已报告的诊断”；
- AC-004 的 `properties` overlay 变体（当前 runtime 场景只描述 `data` 路径）。

manifest 的 `coverage` 只记录覆盖范围，不代表对应断言已经执行。

## 8. 换行约定

- 普通 `.wxml` 使用 LF，不得包含 `\r`。
- 以 `-crlf.wxml` 结尾的文件必须是 CRLF：每个 `\n` 前都是 `\r`，且至少包含一个 `\r\n`。
- 根 `.gitattributes` 用 `tests/fixtures/**/*.wxml text eol=lf` 保证普通样本在 Windows 检出仍为 LF，再用 `tests/fixtures/**/*-crlf.wxml -text` 保持 CRLF 字节精确；校验器同时检查文件字节，不依赖 Git 配置。

## 9. 新增样本

1. 在 `tests/fixtures/` 下新建与 case id 相同的目录。
2. 至少提供 `app.json`、`tsconfig.json`、模板与脚本、`expected/hover.json`、`expected/diagnostics.json`。
3. 每个正例和反例都要有唯一 marker 和明确 target；不要用“存在任意 hover”代替具体断言。
4. 在 `manifest.json` 增加 case，`positive`/`negative` 与 expected 的 marker 集合完全一致。
5. 运行 `cargo test -p wxml-fixtures`，再运行全量测试。
6. 新增外部来源样本前先更新 `docs/reference/provenance.md`；默认只接受自造样本。
