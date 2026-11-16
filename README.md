# WXML LSP：需求与实施计划

更新日期：2026-11-16。

架构决策于 2026-11-15 整理至 [ADR 索引](docs/adr/README.md)，包含模块边界、类型检查与源码映射、文档同步及并行交付方案。架构边界以有效 ADR 为准；`accepted` 不表示功能已实现，实际进度与证据仍见本文。

本文记录已确认的需求、方案评审结果、实验依据和后续计划。项目用于个人在 Zed 中开发微信小程序；当前已完成 Rust workspace、WXML 语法/项目模型、C0/C1 契约、类型桥（候选 A）、Node 类型后端、模板语义、LSP、Zed 扩展代码及真实 Node 端到端（G1）；剩余主要是 Wave 4 类型能力、Zed 标准 Dev Extension 安装与实机观感验收。

文档入口：[文档索引](docs/README.md) · [WXML 语言功能基线](docs/reference/wxml-language-feature-baseline.md) · [证据索引](docs/evidence/README.md) · [来源与许可证记录](docs/reference/provenance.md)。

## 0. 当前实现与快速开始

### 0.1 实现范围

| 组件 | 位置 | 当前内容 |
| --- | --- | --- |
| Rust workspace | `crates/` | `wxml-text`（UTF-8 字节 ↔ UTF-16 位置、行索引）、`wxml-syntax`（容错 lexer/parser、Mustache Pratt、WXS rawtext、资源上限）、`wxml-project`（URI/路径、overlay、app 发现、tsconfig、sources）、`wxml-types`（C0 契约、模板语义、`wx:for` 作用域、后端接口与 mock）、`wxml-lsp`（JSON-RPC、ProjectHost、Node 后端、hover/diagnostics/definition、静态与后端补全、版本屏障/缓存/超时/重启）、`wxml-fixtures`（schema v2 校验） |
| Node 类型后端 | `backends/typescript/` | C1.1 类型桥；固定 `typescript@6.0.3`、`miniprogram-api-typings@5.3.1`；要求 Node >= 24；内存原位 shadow，不写业务文件 |
| Zed 扩展 | `editors/zed/` | 本地 Dev Extension，id `wxml-lsp`、名称 `WXML LSP`（避免与 Marketplace 上仅 grammar 的 `wxml` 扩展冲突）；WXML 语言配置、tree-sitter 查询、`wasm32-wasip2`；真实 grammar 已用 wasi-sdk 34 构建（约 23,806 B）；从 `WXML_LSP_BIN` 绝对路径启动 `wxml-lsp --stdio` |
| 样本与证据 | `tests/fixtures/`、`docs/evidence/` | AC-001..AC-013 样本与 schema 校验；类型桥、P7/P8 与 G1 证据记录 |

当前状态快照（2026-11-16）：workspace `625 passed / 6 ignored / 26 suites`；`editors/zed` `22 passed`；`wasm32-wasip2` debug `7,633,946 B`、release `238,792 B`。6 个 ignored 是需显式开启的真实 Node 门禁（`WXML_TEST_NODE=1`）。

### 0.2 验证命令

```bash
# Rust workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# Node 类型后端（Node >= 24）
(cd backends/typescript && npm ci && npm run build)

# Zed 扩展（独立 workspace；需要 wasm32-wasip2 target）
(cd editors/zed && cargo fmt --all -- --check)
(cd editors/zed && cargo check --all-targets --locked)
(cd editors/zed && cargo clippy --all-targets --locked -- -D warnings)
(cd editors/zed && cargo test --locked)
(cd editors/zed && cargo build --target wasm32-wasip2 --locked)
(cd editors/zed && cargo build --target wasm32-wasip2 --release --locked)
```

真实 Node 端到端门禁：

```bash
(cd backends/typescript && npm ci && npm run build)
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test stdio_type_loop -- --ignored
WXML_TEST_NODE=1 cargo test -p wxml-lsp --test real_node -- --ignored
```

### 0.3 在 Zed 中运行

1. 构建服务与后端：`cargo build -p wxml-lsp --release`，并在 `backends/typescript` 执行 `npm ci && npm run build`。
2. 在仓库根目录、启动 Zed 前导出绝对路径（必须是**绝对路径**，扩展不探测 worktree、不查 `PATH`）：

   ```bash
   export WXML_LSP_BIN="$PWD/target/release/wxml-lsp"
   export WXML_TYPE_BACKEND="$PWD/backends/typescript/dist/server.js"
   # 可选：覆盖后端工作目录（含 node_modules）
   # export WXML_BACKEND_DIR="$PWD/backends/typescript"
   ```

3. 标准安装动作：在 Zed 命令面板执行 `zed: install dev extension`，选择 `editors/zed`；扩展 id 为 `wxml-lsp`，名称 `WXML LSP`。
4. 详细步骤、WASI/grammar 构建和已知限制见 [Zed 扩展 README](editors/zed/README.md)。

`WXML_LSP_BIN` 缺失或无效时，扩展返回错误且不会启动 LSP，也没有 worktree 或 `PATH` 回退。

本机主 Zed 用户数据目录还放入过手工 dev symlink/index（`extensions/installed/wxml-lsp -> editors/zed`）作为等价安装；主 Zed 已重启并实际加载扩展，打开 `tests/fixtures/page-basic` 后启动了 `wxml-lsp --stdio` 与 Node 后端，`user.nmae` 显示红色诊断。这不等于标准 UI 安装。

环境变量约束（由 `wxml-lsp --help` 与扩展转发逻辑共同决定）：

| 变量 | 约束 |
| --- | --- |
| `WXML_LSP_BIN` | Zed 扩展唯一支持的服务器来源；必须为宿主 OS 绝对路径，且存在、可执行；无回退 |
| `WXML_NODE` | Node 可执行文件名或绝对路径；默认 `node` |
| `WXML_TYPE_BACKEND` | `server.js` 的绝对路径；相对值被服务端拒绝 |
| `WXML_BACKEND_DIR` | 后端工作目录的绝对路径（含 `node_modules`），默认由 `dist/` 推导；不用于选择 `server.js` |
| `WXML_ANALYZE_TIMEOUT_MS` | 单请求超时；默认 `15000` |
| `WXML_TEMPLATE_CHECK` | `off` / `pragma` / `inherit`；默认 `pragma` |

未设置 `WXML_TYPE_BACKEND` 时，服务只探测可执行文件相对的安装布局，不读取当前工作目录；`WXML_BACKEND_DIR` 只改后端工作目录，不能替代服务器路径。类型能力会降级。

### 0.4 已知限制

- Wave 4 尚未完成：完整 `wx:for` 元素类型、条件窄化、多视图/项目别名、反向编辑等对应 AC-005/006/008/012；含 locals 的片段当前为 `Partial`，元素类型按 `any` 处理。
- Zed 扩展已完成代码评审、host 测试、wasm 构建和真实 `tree-sitter-wxml` grammar 构建（wasi-sdk 34，约 23,806 B）；早期隔离 smoke 用临时 HTML grammar，最终隔离测试已加载真实 grammar、注册 WXML、启动真实 LSP、显示 `user.nmae` 诊断。本机主 Zed 已通过手工 dev symlink/index 重启加载扩展，在 `tests/fixtures/page-basic` 中启动 `wxml-lsp --stdio` 与 Node 后端，并显示 `user.nmae` 红色诊断。**标准 UI `zed: install dev extension` 安装、hover/definition 内容与真实 grammar 查询观感尚未确认**；未做 Windows/macOS 实机验证。
- G0/G2 仅部分通过；G1 真实 Node 端到端通过（5 + 1 个显式门禁测试）。KDE EIS 权限弹窗阻断了自动化鼠标/键盘验证。
- 未发布：扩展 id 为 `wxml-lsp`、名称 `WXML LSP`（避免与 Marketplace 上仅 grammar 的 `wxml` 扩展冲突）；没有 marketplace 发布、服务器自动下载或安装流水线。
- Zed 只把 WXML buffer 交给 `wxml-lsp`；未保存的 JS/TS 修改不会实时反映到 WXML，保存后的下一次分析才会读取磁盘内容。

## 1. 项目目标

以本项目的[WXML 语言功能基线](docs/reference/wxml-language-feature-baseline.md)为通用语言能力参考，开发独立维护、自用的 Zed WXML 扩展，并增加可靠的模板类型支持。

最重要的需求是：WXML 引用的数据能够追踪到所属页面或组件的实际类型；鼠标悬停时显示字段类型及说明；出现类型错误时，在 WXML 中准确标记对应位置。

WXML 的页面/组件数据根只能来自脚本中声明的字段：页面来自 `data`，自定义组件来自 `data` 与 `properties`；`methods`（组件）或页面选项上的函数只作为事件处理函数来源。WXML 不会把单独定义的 `interface` 或模型文件当作数据源；`interface`/`type` 只有被 `data`/`properties` 字段引用时才参与类型和文档查询。

例如组件这样声明：

```ts
Component({
  properties: {
    /** 外部传入的标题 */
    title: {
      type: String,
      value: '',
    },
  },
  data: {
    // WXML 数据从 data 中引用
    user: {
      /** 用户显示名称 */
      name: '',
    },
  },
  methods: {
    // WXML 事件所引用的方法
    onTap() {},
  },
});
```

模板中：

```xml
<view bindtap="onTap">{{ title }} {{ user.name }}</view>
```

悬停 `title`、`name` 应显示实际类型和字段说明，例如：

```ts
(property) title: string
(property) user.name: string
```

同时显示“外部传入的标题”“用户显示名称”。对于 `{{ user.nmae }}`，应标记不存在的字段 `nmae`，并给出可理解的错误说明。

`data`/`properties` 字段可以引用跨文件 `interface`/`type` 作为类型；此时类型、JSDoc 和诊断沿该声明查询，但模板数据根仍是脚本中的 `data`/`properties`，不是独立 interface。页面使用 `Page({ data })` 时没有 `properties`，组件则可将两者同时作为模板数据来源。

## 2. 已确认的范围与偏好

### 2.1 用户明确要求

- 目标编辑器为 Zed，目标场景为微信小程序 WXML 开发。
- 可以独立维护和安装自用扩展，不以公开发布为前提。
- 不把市场审核、与现有扩展功能重叠、向上游提交 PR 作为实施前提。
- 使用 Rust 承担扩展或语言服务的主要实现；完整 TypeScript 类型能力可以通过复用类型引擎获得。
- WXML 数据引用必须能够展示类型和说明，并标记类型错误。

### 2.2 明确不需要

- Vue / Pug 内嵌模板支持。
- Emmet。
- 一键创建组件及其命令、输入框、右键菜单。
- JS 变量任意自定义颜色或装饰样式。
- 自动修改 VS Code 配置。
- 围绕扩展市场发布或上游 PR 设计工作流。

因此，上述功能在 Zed 中的 API 限制不构成本项目的主要风险，也不安排对应的兼容或替代实现。

### 2.3 实施范围的默认选择

首阶段聚焦原生微信小程序的 `.wxml` 与相关 JS/TS、JSON、样式文件。原插件中为 Vue/Pug 或其他模板方言服务的兼容层不作为首阶段目标。

“全部逻辑只能使用 Rust”“单进程”“无 Node”“单一二进制分发”均未被确认为硬性要求。当前优先级是类型准确性、说明完整性和真实编辑体验。

## 3. 功能需求

### 3.1 第一优先级：模板类型体验

页面/组件数据根由脚本声明决定：页面为 `data`，自定义组件为 `data` 与 `properties`。`interface`/`type` 只有作为这些字段的类型来源时才参与；`methods`/页面函数只用于事件处理，不是模板数据。

| 编号 | 需求 | 验收要点 |
| --- | --- | --- |
| T1 | 关联所属页面或组件 | 普通 `Page({...})`、`Component({...})` 写法不要求改业务源码，不强制增加导出或辅助方法 |
| T2 | 字段类型与说明 | `user.name`、`title` 能追踪到 `data`/`properties` 的实际声明；展示类型、字段 JSDoc；支持字段引用跨文件类型 |
| T3 | 类型错误标注 | 不存在字段、非法类型操作等错误映射到 WXML 中的准确范围 |
| T4 | 编辑期间一致性 | WXML、关联脚本及导入类型尚未保存时，查询结果能随编辑更新 |
| T5 | 模板作用域 | 正确处理 `wx:for` 的默认和自定义变量、嵌套遮蔽及作用域退出 |
| T6 | 条件与联合类型 | 结合模板语义处理 `wx:if/elif/else`、可空类型与类型窄化，减少误报 |
| T7 | 项目类型上下文 | 尊重适用的 `tsconfig`、模块解析和路径别名；追踪类型别名、继承、泛型等完整 TS 类型关系 |
| T8 | JS 类型来源 | 支持具备 JSDoc 类型的 JS 页面和组件，不把所有 JS 数据退化为 `any` |
| T9 | 字符位置正确 | 中文、emoji、换行等场景中的 hover 与错误位置正确，统一处理 UTF-16 与字节偏移 |

类型后端能够识别某项语义，不代表模板转换层已经支持它；T5、T6、T7 等都需要独立验收。

### 3.2 后续类型增强

- 自定义组件的 `properties`、页面或组件 `data` 共同构成模板上下文；`data`/`properties` 字段引用的 `interface`/`type` 只提供类型和文档，不作为独立数据根。
- 根据属性的实际声明类型，检查绑定表达式与目标组件属性的兼容性。
- 保留字段的可选性、联合类型、文档及声明位置，支持成员补全和定义跳转。
- 对缺少类型信息的 `any`、裸 `Object` 等保持诚实：不能凭空生成字段说明或声称严格校验通过。
- 模板运行时转换、可空访问等规则需要形成明确的诊断策略，避免照搬 TS 规则产生误报。

### 3.3 通用 WXML 语言功能

以下为需要保留或逐步补齐的功能摘要；完整行为、数据来源和验收要求见[WXML 语言功能基线](docs/reference/wxml-language-feature-baseline.md)。先完成模板类型闭环，再按实际使用价值实施。

- 标签名、属性名、属性值、事件及 `wx:` 指令补全。
- 根据组件与已有属性筛选候选项，例如 `picker` 的不同 `mode`。
- 根据 `usingComponents` 解析自定义组件，并提供组件和属性补全。
- 模板表达式成员补全、事件处理函数补全。
- 同名样式及全局样式中的 class 补全和导航。
- Hover、定义跳转、资源路径链接。
- 闭合标签补全、标签配对高亮、基础编辑支持。
- 内置或用户 snippets。
- WXML 格式化；实施前先确定保真规则和测试样本。
- 配置、文件依赖、缓存及更新处理。

历史调研对象使用了多套语言配置和多个格式化后端；这些兼容项不自动成为本项目必须完整复刻的要求。

### 3.4 补全（Wave 5）

补全分两条路径，目录与边界以[功能基线 §3.8](docs/reference/wxml-language-feature-baseline.md#38-补全目录现状wave-5)为准：

- **静态、同步（不启动 Node）**：`<` 后的标签（基础组件 + 当前页面 `usingComponents`）；`</` 后仅当前未闭合元素栈内的标签；开始标签内的属性名（标签属性 + `id`/`class`/`style`/`hidden` + `wx:` 指令 + 常用事件绑定）；引号内枚举属性值（如 `button type`、`picker mode`、`input type`、`image mode`、`navigator open-type`）；事件属性名（`bindtap`、`catchtap`、`bind:tap`、`capture-bind:tap`、`mut-bind:tap` 等）。
- **依赖类型后端**：`{{ user.na }}` 的表达式成员补全复用 C1 虚拟文件布局与 TypeScript `getCompletionsAtPosition`；`bindtap="on"` 枚举 Page 顶层方法与 Component `methods`。后端不可用、超时或不支持时静默降级为**空列表**，不报协议错误。
- **替换范围**：仅替换已输入 token——标签名/属性名替换 token，引号内的值只替换值前缀（引号保留），未加引号的值插入带引号的字面量；不做整行替换。
- **不产生补全**：纯文本、HTML 注释、`<wxs>` 元素体、Mustache 字符串字面量内。
- **能力声明**：`completionProvider` 无条件声明，`resolveProvider: false`，触发字符 `<`、空格、`:`、`"`、`'`、`.`、`{`、`=`；`isIncomplete: false`，单次最多 256 项；参数非法返回 `-32602`。

自定义组件目前只在页面 WXML 中列出（页面 `usingComponents` 与 `app.json` 合并，本地同名优先）；组件自身 WXML 中的自定义组件、组件属性名尚未参与补全。目录是受限子集，不宣称完整小程序覆盖。

## 4. 方案评审结论

### 4.1 保留的判断

- Zed 侧采用轻量 Rust/WASM 扩展，负责注册语言及启动外部语言服务。
- 补全、Hover、跳转、诊断等语言智能通过 LSP 提供。
- 可以复用已有 WXML grammar、查询、组件资料与相关实现经验。
- 自用时先从本地路径启动服务，直接安装 Dev Extension，无须建设公开发布和自动下载流水线。

### 4.2 修正的判断

- 不再建议把向现有 Zed WXML 扩展提交 PR 作为主路线；可以独立维护自己的扩展。
- 不再用 Vue/Pug、Emmet、创建组件、自定义颜色等无关能力衡量功能损失。
- 不把“移植原插件约几千行代码”当作核心工作量依据。
- 撤回此前“覆盖约 80%”“几周完成”等粗略估算；完整类型需求需要依据真实验收结果重新评估。
- Rust 静态提取字段适合基础元数据，不能替代完整 TypeScript 类型引擎。
- 不通过执行工作区 JS 的方式提取组件属性；原插件的 `new Function` 路线不采用。

### 4.3 候选方案比较

| 方案 | 可复用价值 | 结论 |
| --- | --- | --- |
| 逐段翻译历史插件实现 | 补全规则、数据结构、部分路径和样式处理经验 | 无法直接解决新增类型需求，不作为整体架构起点 |
| 直接使用已核验的 Rust WXML LSP | WXML 解析、基础 LSP、部分作用域处理；见[核验记录](docs/evidence/2026-11-15-existing-wxml-lsp-audit.md) | 简化的类型模型无法满足准确类型、文档和精确诊断需求 |
| 适配官方模板分析器 | 模板检查视图、位置映射、组件资料与类型集成经验；见[核验记录](docs/evidence/2026-11-15-official-template-analyzer-audit.md) | 值得复用，但不能只启动其 Rust 二进制就认为类型能力齐备 |
| Rust 模板服务 + 完整 TypeScript 后端 | 类型引擎成熟，Rust 专注模板和编辑器集成 | 当前推荐路线 |

## 5. 推荐架构与待验证决策

```text
Zed 本地 Dev Extension（Rust / WASM）
    │ LSP
    ▼
Rust WXML Language Server
    ├─ WXML 解析与编辑上下文
    ├─ 项目、页面和组件定位
    ├─ usingComponents 与基础组件元数据
    ├─ 模板作用域与检查视图生成
    ├─ WXML ↔ 检查视图的源码映射
    ├─ 文档覆盖层、依赖图、缓存与失效处理
    ├─ 补全、Hover、跳转、诊断的路由与结果转换
    └─ 可替换的 TypeScript 类型后端
         ├─ 首选验证：Node + TypeScript Language Service
         └─ 后续候选：TypeScript 原生语言服务
```

### 5.1 内存检查视图

将 WXML 表达式置于 TypeScript 能检查的内存视图中，保留与原始 WXML 的精确映射。复用组件或页面的实际类型上下文（模板数据根为脚本中的 `data`，自定义组件还包括 `properties`），查询类型、JSDoc 和诊断，再映射回模板。

检查视图、辅助符号和探针方法只存在于语言服务内存中，不写回业务源码，不要求用户改变普通页面或组件写法。实际转换方式应通过原型验证后确定，不提前锁定为某一种代码注入形式。

### 5.2 类型后端选择

优先用 Node + TypeScript Language Service 完成端到端验证，原因是本次实验已经验证其在普通小程序上下文上的类型与文档机制，内存文件和项目管理也可直接控制。

后端通过清晰的接口隔离，避免模板解析、源码映射与某一版本的 TypeScript API 强绑定。若后续希望去除 Node，先让原生 TypeScript 后端通过同一套验收用例，再决定是否切换。

本次实验中的 TypeScript 版本仅表示已测试版本，不代表推荐追随的最新版本。TS 6 时代的 JavaScript Compiler/Language Service API 与原生后端的 LSP 接入方式不同，不能假设直接替换。

### 5.3 未保存文档同步

只把服务挂到 WXML 或只监听磁盘，无法保证 JS/TS 未保存修改即时反映到模板。

计划让同一 WXML 服务也接收相关 JavaScript、TypeScript 文档的打开、修改、关闭事件；不替代用户已有的 JS/TS 主服务，不重复发布无关的脚本诊断。内存覆盖层优先于磁盘内容，并在依赖变化时刷新关联 WXML。

还需在 Zed 中验证：服务器实例复用、同一项目文档归属、多服务并存以及更新顺序。

### 5.4 其他技术选择

- WXML 主解析优先评估已有 Tree-sitter grammar，避免为高亮和分析长期维护两套不同的完整语法。
- Mustache 表达式必须按语法处理，不能用简单正则承担完整类型检查和改写。
- WXML、生成文本、LSP 三者之间统一管理字节与 UTF-16 映射。
- WXSS/SCSS 索引可评估 `raffia`；具体依赖版本在实施时核实。
- WXML 格式化后置；必须保留文本空白、表达式与 `<wxs>` 的语义，解析失败时不修改文件。
- 历史调研中的组件元数据只能作为线索；实际采用的数据必须在本项目登记来源、版本与许可证，并在复用前重新核验（见[来源与许可证记录](docs/reference/provenance.md)）。
- crate 职责和依赖边界见 [ADR-0001](docs/adr/0001-layered-language-service-architecture.md)；LSP 框架、grammar 固定版本和具体 API 在实施接入时核实，不作为已完成事项。

### 5.5 当前工程结构

```text
Cargo.toml                    # workspace、共享 lint 与依赖版本
crates/
├── wxml-text                 # UTF-8 字节偏移 ↔ LSP UTF-16 位置、行索引
├── wxml-syntax               # WXML 词法/语法、Mustache 表达式、源码映射
├── wxml-project              # 项目、页面/组件、usingComponents、tsconfig
├── wxml-types                # C0 契约、请求上下文、模板语义、后端接口与 mock
├── wxml-fixtures             # 共享验收样本的结构校验器
└── wxml-lsp                  # LSP 传输、ProjectHost、Node 后端与请求路由（二进制）
backends/typescript           # C1.1 Node 类型后端
editors/zed                   # 本地 Zed Dev Extension（独立 workspace）
tests/fixtures                # AC-001..AC-013 样本
docs/                         # 参考、ADR、证据
```

目标依赖按 [ADR-0001](docs/adr/0001-layered-language-service-architecture.md) 统一：`wxml-lsp` 依赖各功能层，`wxml-types` 允许依赖 `wxml-syntax`，各层按需依赖 `wxml-text`；`wxml-project` 独立于 syntax/types，项目事实由 LSP 适配为 types 输入。禁止反向依赖和 `types → project`。这取代原文“禁止所有横向依赖”的表述；依赖声明在首次使用时接入。当前 workspace 已包含上表六个 crate；`wxml-types` 落地 C0 契约、请求上下文、后端接口、mock 与模板语义；`wxml-lsp` 已接入真实 `ProjectHost`、Node 后端与 LSP 路由。C1 类型桥和 Node 后端位于 `backends/typescript/`，Zed 扩展位于 `editors/zed/`；`wxml-fixtures` 只校验共享样本结构。

## 6. 已有验证与证据边界

以下是隔离实验的结论摘要。完整环境、方法、限制和最小原始数据见[证据索引](docs/evidence/README.md)；实验在仓库外运行，临时目录不再作为证据来源，也不能把实验结论描述为已实现功能。

### 6.1 TypeScript Language Service 机制实验

实验版本：`typescript@6.0.2`、`miniprogram-api-typings@5.3.1`。

两组实验共 22 项断言通过，覆盖跨文件类型和 JSDoc、字段错误、可空与条件窄化、循环元素、组件属性、JS JSDoc、未保存类型修改，以及包含中文和 emoji 的 WXML 位置映射。

限制：WXML 映射实验只覆盖受限表达式样本，不是完整模板转换器；未覆盖完整项目、所有组件写法、完整 Behavior 合并或 Zed 端到端行为。实验验证的是类型后端机制；模板数据根规则见 §3.1，独立 interface 不作为模板根。实验期间没有修改用户业务项目。原始探针未保留，记录等级为 E0；详见[实验记录](docs/evidence/2026-11-15-typescript-language-service.md)。

### 6.2 现有 Rust WXML LSP 核验

核验对象：现有 Rust WXML 语言服务 0.1.9，提交 `84425dad66de0991e0cdf24e9c86842dc8209cdd`。

实际运行 33 个独立场景、66 次 hover 请求；该版本的 112 项自带测试通过。发现的缺口包括字段说明缺失、跨文件和联合类型支持不足、条件窄化不完整、属性绑定缺少类型兼容性检查，以及部分诊断位置与未保存修改处理错误。

因此可以借鉴其基础设施，不能把现有类型分析层当作本项目的准确类型来源。自带测试通过，不代表通过本项目需求验收。完整结果和限制见[核验记录](docs/evidence/2026-11-15-existing-wxml-lsp-audit.md)，原始数据见[证据数据](docs/evidence/data/existing-wxml-lsp-audit/)。

### 6.3 官方模板分析器核验

源码和保存的实验结果表明，类型能力涉及客户端侧 TypeScript 集成，不能只连接其 Rust LSP 就认为能力完整。

使用小程序 backend 配置的测试中，导出组件样本可以获得字段类型、说明和错误；裸 `Component(...)` / `Page(...)` 样本退化为 `any`。未保存的导入类型更新还存在需要 WXML 刷新才生效的问题。这些是所测配置和样本的结果，不代表所有版本与写法都具有完全相同的行为。完整结果、握手要求和限制见[核验记录](docs/evidence/2026-11-15-official-template-analyzer-audit.md)，原始数据见[证据数据](docs/evidence/data/official-template-analyzer-audit/)。

### 6.4 TypeScript 原生语言服务实验

保存的实验分别使用原生服务 7.0.2 与 7.1 开发版。普通、未落盘的虚拟 TS 文件能够获得 hover 类型、JSDoc 和诊断，修复表达式后诊断清除，未保存的依赖说明修改也能更新。

虚拟 `file://` 文档可解析同目录导入；测试中的自定义 URI scheme 无法正确解析相对模块。显式 `files` 配置下，虚拟文件还出现未归入预期配置项目的情况。

限制：这只验证普通虚拟 TS 文件，不等于已验证完整的小程序检查视图、所有 `tsconfig` 行为或 Zed 集成。完整结果、复跑方式和限制见[实验记录](docs/evidence/2026-11-15-native-typescript-service.md)，原始数据见[证据数据](docs/evidence/data/native-typescript-service/)。

## 7. 实施计划

当前已完成 P0–P8 与 G1：多 crate 的 Rust workspace、C0/C1 契约、WXML 语法与项目模型、类型桥（候选 A）、Node 类型后端、模板语义、LSP、Zed 扩展代码及真实 Node 端到端。Zed 扩展已通过独立评审，但标准 Dev Extension 安装、真实 grammar 渲染和 hover/definition 可视化尚未验证。

### 阶段 1：固定样本和验收规则

- [x] 选择具有代表性的 Page、Component、`data`/`properties`、跨文件类型引用和 JS JSDoc 样本。
- [x] 覆盖 `data`、`properties`、跨文件类型、路径别名、联合类型、循环与条件。
- [x] 明确字段说明优先级，以及缺失类型信息时的展示方式。
- [x] 明确严格空值、模板运行时转换及绑定属性兼容性的诊断策略。
- [x] 建立可重复运行的正例与反例，避免只检查“能返回 hover”。

产出：需求到测试样本的对应关系，以及第一版诊断策略。

状态：C0/C1 已冻结；样本、契约与类型分析 runner 已落地。样本和契约本身不是用户功能，实际能力见 §10。

### 阶段 2：最小类型闭环

- [x] 建立 Rust 服务与 TypeScript 后端的最小通信接口。
- [x] 识别普通 Page / Component 及其 `data`/`properties`，不要求修改业务写法。
- [x] 生成内存检查视图，建立表达式源码映射。
- [x] 对 `user.name`、`title` 返回准确类型和字段说明。
- [x] 对 `user.nmae` 返回精确到字段的诊断范围。
- [x] 覆盖中文、emoji、CRLF 和边界位置。

产出：可通过 stdio 测试的最小语言服务；此时仍不宣称支持完整 WXML。

状态（2026-11-16）：上述六项已在 LSP/Node 层由 G1 真实端到端覆盖（AC-001/002/003/013）；Zed 实机观感仍见阶段 3。

### 阶段 3：Zed 本地端到端验证

- [ ] 创建自用 Dev Extension，从明确的本地路径启动服务。
- [ ] 在真实 WXML 文档中验证 hover 和错误标注。
- [ ] 让相关 JS/TS 文档变更进入同一服务的内存覆盖层。
- [ ] 保留已有 JS/TS 服务，验证并存行为与实例归属。
- [ ] 验证未保存的 `data`/`properties` 字段类型和说明（含其引用的跨文件类型）、脚本及模板修改能够正确传播。
- [ ] 记录响应时间、诊断误报、错误消失和服务重启后的恢复情况。

产出：可本地安装、可评审的最小验证版及实际测试记录。

状态（2026-11-16）：扩展代码、host 测试、wasm 构建和真实 grammar 构建完成；隔离用户目录 smoke 已加载真实 grammar、启动真实 `wxml-lsp` 并显示 `user.nmae` 诊断；主 Zed 已放入手工 dev symlink/index 并重启加载，`page-basic` 窗口中启动了 `wxml-lsp` 与 Node 后端，`user.nmae` 显示红色诊断和语法着色。标准 UI 安装、hover/definition 内容、未保存 JS 同步和并存行为仍未验收，因此上述项不勾选。

### 阶段 4：完整模板作用域与项目类型

- [ ] 完善表达式语法、重写和源码映射，移除原型阶段的受限假设。
- [ ] 支持 `wx:for-item/index`、嵌套遮蔽及作用域恢复。
- [ ] 支持条件分支、可空和判别联合的适用窄化规则。
- [ ] 完善 `tsconfig` 项目归属、路径别名、导入、依赖图和缓存失效。
- [ ] 补充符合项目实际写法的 Behavior、共享数据和复杂类型样本。
- [ ] 增加成员补全、定义跳转和自定义组件属性绑定检查。

产出：围绕实际小程序项目验收的模板类型功能。

状态（2026-11-16）：`wx:for` 仅传递 locals 名称并以 `any` 绑定；条件窄化、多视图归属、补全和反向编辑未实现；definition 已支持可映射目标。AC-005/006/008/012 仍未完成。

### 阶段 5：补齐常用 WXML 功能

- [ ] 内置标签、属性、值、指令和事件补全及文档。
- [ ] `usingComponents` 解析和自定义组件信息。
- [ ] 事件函数、class、资源路径的补全与导航。
- [ ] 闭合标签、标签配对和 snippets。
- [ ] 按需求补充样式索引和格式化，不引入无关兼容层。

产出：日常可用的自用 WXML 工具。

状态（2026-11-16）：基础 hover、diagnostics 和 definition 已可用；补全、事件/class/资源导航、snippets、格式化和样式索引尚未实现。

### 阶段 6：优化与可选后端替换

- [ ] 根据实测性能处理后台任务、取消、诊断防抖及大项目缓存。
- [ ] 评估原生 TypeScript 后端能否通过完全相同的类型和项目验收用例。
- [ ] 只有在兼容性、收益和维护成本明确后，决定是否去除 Node。
- [ ] 补齐本地构建、安装、升级和排错说明。

状态（2026-11-16）：已实现版本屏障、缓存、请求超时、后端失败重启与响应上限等基础防护；原生 TypeScript 后端替换、去 Node 决策、发布流水线和完整安装/升级说明仍未完成。

公开发布、自动下载和多平台发布流水线不在当前计划中；只有实际使用需要时再增加。

## 8. 第一阶段验收清单

以下清单指首个可在 Zed 中评审的类型版本，预计在上述阶段 3 完成时验收；不表示仅完成阶段 1 的样本整理即可满足。

- [ ] 普通 `Page({...})` 的 `data`、`Component({...})` 的 `data`/`properties` 无需改写即可关联模板数据。
- [ ] 悬停 `user.name`、`title` 得到实际类型及该字段的说明。
- [ ] 不存在字段被准确标记，合法表达式不被误报。
- [ ] 中文、emoji、不同换行格式下位置正确。
- [ ] 未保存的 `data`/`properties` 字段类型和说明（含引用的跨文件类型）变更能够更新到模板。
- [ ] 错误修复后旧诊断及时清除。
- [ ] 最小循环与条件样本中的类型正确；尚不支持的情况明确记录。
- [ ] 虚拟文件与辅助代码不污染业务源码或磁盘项目。
- [ ] 与已有 JS/TS 服务并存，不重复发布无关脚本诊断。
- [ ] 文档明确区分已支持、降级与待实现情况。

状态（2026-11-16）：G1 在 LSP 层通过 AC-001/002/003/011/013，覆盖上表前四项以及“错误修复后旧诊断清除”；虚拟文件不写业务源码也已有测试。未保存 JS 同步、循环/条件类型和 Zed 内可评审仍未完成，因此不整体勾选。

## 9. 需求与来源索引

本项目需求不依赖外部项目或文件。通用 WXML 语言功能的需求已整理为[WXML 语言功能基线](docs/reference/wxml-language-feature-baseline.md)；实验结论、方法和最小数据见[证据索引](docs/evidence/README.md)。

调研对象的版本、提交和许可证集中记录在[来源与许可证记录](docs/reference/provenance.md)。该记录仅用于归属、合规和复查，不作为需求来源；外部仓库或临时目录不能作为本项目验收的依据。

## 10. 当前状态

快照：2026-11-16。P0–P8 与 G1 已实现并通过代码门禁；CRLF 样本由 `.gitattributes` 保护。文档与本地 Zed 安装状态见后续提交。

- 已完成：C0/C1 契约；`wxml-text`、`wxml-syntax`（容错 lexer/parser、表达式 Pratt、WXS rawtext、资源上限）、`wxml-project`（URI/路径、overlay、app 发现、tsconfig、sources）、`wxml-types`（类型契约、模板语义、`wx:for` 作用域/事件/不支持形式）、`wxml-lsp`（JSON-RPC、真实 Project/FileId、Node 后端、hover/diagnostics/definition、版本屏障/缓存/超时/重启）、Node 类型桥与后端、P8 Zed 扩展；G1 真实 Node 端到端 5 + 1 个显式门禁测试。
- 验证：workspace `625 passed / 6 ignored / 26 suites`；`editors/zed` `22 passed`；wasm debug `7,633,946 B`、release `238,792 B`；真实 grammar 约 `23,806 B`（gitignored）。
- 部分完成：Zed 扩展代码、wasm 构建和真实 `tree-sitter-wxml` grammar 构建通过；隔离 smoke 注册 WXML、启动真实 LSP 并显示精确诊断；主 Zed 已通过手工 dev symlink/index 重启加载，`page-basic` 窗口启动 `wxml-lsp` 与 Node 后端，显示 `user.nmae` 红色诊断和语法着色。标准 UI `zed: install dev extension`、hover/definition 内容、真实 grammar 查询观感、Windows/macOS 实机、G0/G2 完整验收仍未完成。
- 未完成：完整 `wx:for` 元素类型、条件窄化、多视图/项目别名、补全/反向编辑等 Wave 4+ 能力（AC-005/006/008/012）；未保存 JS/TS 实时同步；发布/自动下载流水线。
- 后续工作的优先级：先在 Zed 中完成标准安装与日常观感验收，再补 Wave 4 类型能力。
