# Zed 标准安装与真实项目验收计划

> **状态：历史归档（Superseded）**。P7 删除已完成；本文仅记录当时计划/证据，不再代表当前仓库状态。最终状态见 docs/plans/2026-11-17-rust-tsgo-migration.md §12.16。

- 状态：已完成（2026-11-16）
- 基线：`master` @ `0ef4190`（Wave 5 合并提交）
- 目标：把“Zed 标准 Dev Extension 安装 + 真实编辑器行为”从文档缺口变成可复核的验收记录
- 约束：不修改用户未提交的 ` tests/fixtures/page-basic/pages/index/index.wxml`；不在项目目录生成临时文件；不推送、不创建 PR

## 0. 执行摘要

本轮不新增语言功能，只验证产品风险最高的交付链路：

1. 在**隔离用户数据目录**中，通过 Zed 命令面板实际执行 `zed: install dev extension`，选择 `editors/zed`，不使用手工 symlink 代替。
2. 用 release `wxml-lsp` 和真实 Node 类型后端，在真实 Zed 中逐项执行 hover、definition、completion、diagnostics，并核对内容而不只是“有弹窗”。
3. 在真实小程序项目中观察语法着色、布局、补全、hover、诊断和交互延迟，记录降级与观感问题。
4. 重启 Zed，确认扩展、语言服务器和 hover 能恢复。
5. 把最小证据写入 `docs/evidence/p8-zed.md` 与 `docs/evidence/data/zed-standard/`，更新 README 的 Zed 状态。

## 1. 验收边界

### 1.1 必须完成

- 标准 UI 安装成功，且满足：
  - `extensions/installed/wxml-lsp` 是指向 `editors/zed` 的 symlink；
  - `extensions/index.json` 中 `wxml-lsp.dev == true`；
  - Zed 日志显示扩展加载、WXML 语言注册、`wxml-lsp --stdio` 启动；
  - 只启动一个 `wxml-lsp` 进程，并启动 Node 类型后端。
- 干净 fixture 实际执行并核对：
  - `title` hover：`string`，文档“页面标题”；
  - `user.name` hover：`string`，文档“用户显示名称”；
  - `user` 或字段 definition 跳到 `index.ts` 对应声明；
  - 标签、属性、事件、Mustache 成员补全至少各一项；
  - 故意制造的未知成员/类型错误显示诊断，修复后消失。
- 真实项目至少两个：
  - TS 项目 `jielong-team-doc/miniprogram/miniprogram`；
  - JS 项目 `ai-mode-demo`；
  - 记录实际打开的 WXML、语法着色、hover/补全/诊断结果和限制。
- 关闭并重启同一隔离实例后，标准安装仍在，hover 仍可用。
- 更新证据文档和 README 状态；保留可复核的截图与日志摘录。

### 1.2 非目标

- 不补 F-08–F-18 的新功能；验收中发现的缺陷只修复阻断安装/核心验收的问题。
- 不证明 Windows/macOS 行为。
- 不验证 Marketplace 发布、自动下载、升级流水线。
- 不把隔离 Xvfb 的观感等同于用户主桌面/Wayland 的最终体验，但必须记录差异。

## 2. 环境与素材

| 项 | 值 |
| --- | --- |
| Zed | 1.28.1 (`e8bc16a`) |
| 隔离显示 | Xvfb `:99`，1600x1000，llvmpipe，无窗口管理器 |
| 用户数据 | `/tmp/opencode/zed-standard/data` |
| 隔离配置 | `/tmp/opencode/zed-standard/config` |
| 项目副本 | `/tmp/opencode/zed-standard/proj`（复制 `tests/fixtures/page-basic-clean`） |
| release server | `<repo>/target/release/wxml-lsp` |
| Node 后端 | `<repo>/backends/typescript/dist/server.js` |
| wasi-sdk | `/tmp/opencode/wasi-sdk-34.0-x86_64-linux` |
| 真实项目 A | `/home/zxg/code/frontend/jielong-team-doc/miniprogram/miniprogram` |
| 真实项目 B | `/home/zxg/code/frontend/ai-mode-demo` |

隔离设置：

```json
{"session":{"trust_all_worktrees":true,"restore_unsaved_buffers":false},"telemetry":{"diagnostics":false,"metrics":false},"autosave":"off"}
```

启动环境：

```bash
export DISPLAY=:99
export ZED_ALLOW_EMULATED_GPU=1
export WXML_LSP_BIN=<repo>/target/release/wxml-lsp
export WXML_TYPE_BACKEND=<repo>/backends/typescript/dist/server.js
export WXML_BACKEND_DIR=<repo>/backends/typescript
export WXML_NODE=/usr/bin/node
export WASI_SDK_PATH=/tmp/opencode/wasi-sdk-34.0-x86_64-linux
```

## 3. 阶段

### Phase 0：基线、构建与隔离环境

1. 确认 `git status` 只有用户 fixture 修改；记录 HEAD。
2. `cargo build -p wxml-lsp --release`；`(cd backends/typescript && npm run build)`。
3. 创建隔离目录，复制 `page-basic-clean` 到 `/tmp/opencode/zed-standard/proj`。
4. 写隔离 settings；启动 Xvfb；确认无其他 Zed 使用该 user-data-dir。

退出条件：release binary、Node dist、fixture 副本、Xvfb 和隔离目录就绪。

### Phase 1：标准 UI Dev Extension 安装

1. 启动隔离 Zed，打开空项目或 fixture 副本。
2. 用 `ctrl-shift-p` 打开命令面板，搜索并执行 `zed: install dev extension`。
3. 在目录选择器输入 `<repo>/editors/zed` 并确认。
4. 等待扩展构建；如果下载 grammar/wasi-sdk，允许使用 `WASI_SDK_PATH` 复用本地 SDK。
5. 验证：
   - 安装目录是 symlink；
   - `index.json` 中 `dev=true`，manifest 含五语言；
   - `Zed.log` 有扩展加载、WXML 注册和 LSP 启动；
   - `ps` 中只有目标 release `wxml-lsp`，且 Node 后端启动。

退出条件：**不靠手工 symlink**，标准 UI 安装成功并可复核。

### Phase 2：干净 fixture 的逐项行为验收

在隔离项目副本中实际执行，不能只读代码或测试结果：

1. 打开 `pages/index/index.wxml`，确认状态栏语言为 `WXML`、语法着色正常。
2. Hover：
   - 鼠标停在 `title`，截图并核对 `(property) title: string` 与“页面标题”；
   - 鼠标停在 `user.name`，截图并核对字段类型与“用户显示名称”。
3. Definition：
   - 对 `user`/`user.name` 执行跳转（优先 `F12`，必要时 `ctrl+click`）；
   - 确认打开 `index.ts` 并落在对应声明，而不是只看到光标移动。
4. Completion：
   - 在空标签处输入 `<vi`，确认 `view`；
   - 在 `<view ` 后确认 `class` 等属性；
   - 在属性名后确认事件/指令补全（如 `bindtap`）；
   - 在 `{{ user.` 后确认成员补全（`name`，以及类型允许的其他字段）。
5. Diagnostics：
   - 临时把 `{{ user.name }}` 改为 `{{ user.nmae }}`，确认错误诊断；
   - 制造一个类型错误（如 `{{ title + 1 }}` 或项目允许的反例），确认内容可读；
   - 改回正确内容，确认旧诊断消失。
6. 保存/未保存：
   - 修改 `index.ts` 的 `name` 类型但不保存，确认 WXML hover 跟随；
   - `ctrl+s` 后确认诊断刷新且不丢失未保存 overlay。

退出条件：每项都有截图或日志证据，且内容与预期一致。

### Phase 3：真实项目观感验收

#### A. TS 项目 `jielong-team-doc/miniprogram/miniprogram`

1. 打开 `pages/index/index.wxml` 和对应 TS。
2. 观察语法着色、缩进、括号、Mustache 注入。
3. 对 `src`/`title`/`userId` 实际 hover，记录类型、文档和降级。
4. 实际触发补全和 definition；记录是否可用、延迟和内容质量。
5. 打开浏览器/项目配置相关文件，确认 wxml-lsp 不干扰其他语言服务。

#### B. JS 项目 `ai-mode-demo`

1. 打开 `pages/home/home.wxml`。
2. 观察复杂 WXML（`wx:for`、事件、自定义组件）的着色和布局。
3. 实际执行 hover、事件属性补全、组件标签补全、诊断。
4. 记录无 `tsconfig`/JS 类型降级是否符合预期，不把预期降级误报为缺陷。

退出条件：每个项目至少 1 张关键截图和 1 段结论；问题分为“阻断”“体验问题”“已知降级”。

### Phase 4：重启与服务恢复

1. 关闭隔离 Zed，确认 `wxml-lsp`/Node 子进程退出。
2. 用同一 user-data-dir 重新启动，打开同一项目。
3. 确认扩展仍为标准 dev 安装、LSP 重新启动、`title` hover 仍返回正确内容。
4. 记录启动耗时和任何重复进程/端口/扩展冲突。

退出条件：重启后标准安装与核心 hover 均恢复。

### Phase 5：证据、文档与提交

1. 将最小截图放入 `docs/evidence/data/zed-standard/`（安装状态、hover、definition、completion、diagnostics、真实项目、重启）。
2. 在 `docs/evidence/p8-zed.md` 新增“标准 UI 安装与逐项验收”章节，包含：
   - 命令与版本；
   - symlink/index 证据；
   - hover/definition/completion/diagnostics 的实际内容；
   - 两个真实项目的观感结论；
   - 重启结果；
   - 仍未验证项和新的限制。
3. 更新 `docs/evidence/README.md` 的 P8 索引和当前门禁。
4. 更新 README 中“标准 UI 安装未确认”“hover/definition 观感未确认”等状态；只写实际证据支持的结论。
5. 运行 `git diff --check`；如改动代码则补跑相应测试。只暂存文档与证据，不暂存用户 fixture。
6. 提交文档（不 push、不创建 PR）。

## 4. 风险与应对

| 风险 | 应对 |
| --- | --- |
| 无窗口管理器导致命令面板/文件选择器无法聚焦 | xdotool 按窗口 id 聚焦；每次关键操作后截图确认 |
| 标准安装需要网络或 wasi-sdk | 设置 `WASI_SDK_PATH` 复用本地 SDK；若仍失败，保留失败日志并修复环境，不用手工 symlink 冒充 |
| 扩展构建使用旧缓存 | 使用全新 user-data-dir；安装后核对 symlink 指向源码目录 |
| 主桌面 Zed 干扰 | 全程 `DISPLAY=:99` + 独立 user-data-dir；不触碰主 Zed 进程 |
| fixture 被用户修改 | 只使用 `/tmp` 副本；不修改仓库 fixture |
| Node 后端未构建或路径错误 | 启动前检查绝对路径和 `node --version`；日志确认后端请求 |
| hover 只证明弹窗存在 | 每项核对实际类型/文档文本并截图 |
| 真实项目配置差异导致降级 | 明确区分“预期降级”和“产品缺陷” |

## 5. 最终验收清单

- [x] 标准 UI 安装成功，symlink + `dev=true`
- [x] 扩展加载，WXML 注册，单一 LSP + Node 后端
- [x] `title` hover 内容正确
- [x] `user.name` hover 内容正确
- [x] definition 实际跳转到 `index.ts`
- [x] 标签、属性、事件、Mustache 成员补全均实际执行
- [x] 未知成员诊断出现；撤销修改后 WXML 诊断清空
- [x] 未保存 TS 修改影响 WXML hover，保存后刷新（Wave 5 D6 + 本轮复验）
- [x] TS 真实项目观感记录
- [x] JS 真实项目观感记录
- [x] 重启后扩展与 hover 恢复
- [x] 证据、P8、README 更新
- [x] 用户 fixture 未被提交/覆盖

## 6. 执行结果（2026-11-16）

### 6.1 标准 UI 安装

在隔离 user-data-dir 中实际打开命令面板并执行 `zed: install dev extension`。由于
隔离 X11 没有桌面 portal，使用仓库外的私有 session bus 假 portal 只替代“目录选择”
对话框，返回 `<repo>/editors/zed`；扩展编译、加载和安装仍由 Zed 自己完成。

结果：

- Zed 日志显示编译 Rust 扩展、编译 `tree-sitter-wxml` grammar、使用
  `WASI_SDK_PATH` 的 wasi-sdk、`finished compiling extension ... in 12.25s`、
  `extensions updated. loading 1`。
- `<user-data>/extensions/installed/wxml-lsp` 是指向 `<repo>/editors/zed` 的
  symlink；`extensions/index.json` 中 `dev=true`。
- manifest 的 server languages 为 WXML/JavaScript/TypeScript/TSX/JSON；WXML
  语言由 `languages/wxml` 注册。
- 启动后只有一个 release `wxml-lsp --stdio` 进程，并启动
  `node backends/typescript/dist/server.js`。
- 关闭并用同一 user-data-dir 重启后，扩展仍加载，`title` hover 仍正确。

证据：`docs/evidence/data/zed-standard/install-log.txt`、
`extension-index-excerpt.json`、`install-success.png`；完整记录见
`docs/evidence/p8-zed.md` 第 13 节。

### 6.2 干净 fixture 逐项行为

项目副本来自 `tests/fixtures/page-basic-clean`，实际在 Zed 中执行：

| 操作 | 实际结果 |
| --- | --- |
| hover `{{ title }}` | `(property) title: string` + “页面标题” |
| hover `{{ user.name }}` | `(property) name: string` + “用户显示名称” |
| definition `title` | 跳到 `pages/index/index.ts` 的 `data.title`（行 4，列 10） |
| 标签补全 `<vi` | `video`、`view`、`web-view`、`cover-view`、`picker-view`、`scroll-view`、`movable-view`、`navigator`、`navigation-bar`、`cover-image` 等 |
| 属性补全 `<view ` | `class`、`hidden`、`id`、`style`、`wx:elif`、`wx:else`、`wx:for`、`wx:for-index` 等 |
| 事件属性补全 `<view bindt` | `bindtap`、`bindtouchend`、`bindtouchmove`、`bindtouchstart`、`bind:tap`、`bindinput`、`bindreset`、`bindsubmit` 等 |
| Mustache 成员补全 `{{ user.` | `name` |
| 未知成员 `user.nmae` | hover 显示 `Property 'nmae' does not exist on type '{ name: string; }'` 和 `type quality: missing`；状态栏 WXML 诊断计数为 1 |
| 撤销错误 | 文件恢复为干净 fixture，WXML 诊断清空；未保存临时编辑 |

截图见 `docs/evidence/data/zed-standard/`。成员补全首次触发时后端仍在分析，第二次
`ctrl+space` 才返回 `name`；其余补全即时。

### 6.3 真实 TS 项目

项目：`jielong-team-doc/miniprogram/miniprogram`，页面脚本使用
`Component({ data: { src, title, userId }, ... })` 注册。

第一次打开时状态栏显示
`type checking is unsupported: no page registration with an object literal argument`。
确认是后端只接受页面路由的 `Page` 注册。已修复：

- ` backends/typescript/src/analysis.ts`：页面视图优先匹配 `Page`；没有 `Page`
  时回退到 `Component` 注册，并把实际注册种类写入响应。
- ` backends/typescript/test/analysis.test.ts`：新增页面用 `Component` 注册的
  单元测试。

修复后实测：

- 状态栏不再出现 unsupported。
- hover `src`：`(property) src: string`。
- definition `src`：跳到 `index.ts` 的 `data.src`。
- 标签补全 `<vi`：`video`、`view`。

所有临时编辑已撤销，真实项目工作区保持原状。

### 6.4 真实 JS 项目

项目：`ai-mode-demo`，无 `tsconfig.json`/`jsconfig.json`，页面为 JS。

- 状态栏显示 `type checking is unavailable: script is not part of the program`。
- hover `{{title}}` 无结果；这是未启用 `allowJs`/无项目配置时的既有降级，不是
  本轮回归。
- 复杂 WXML 的语法着色、缩进、Mustache 注入布局正常。

### 6.5 残留风险

- 真实 TS 项目只覆盖 `Component` 注册的页面；`Page`、普通组件和跨文件未保存
  类型传播仍由既有测试覆盖，未在本轮逐项重做。
- 真实 JS 项目无项目配置，类型能力按设计降级；开启 `allowJs` 的 JS 项目未验收。
- 隔离 Xvfb 无窗口管理器，观感不等同于用户主桌面 Wayland。
- 反复制造损坏 WXML/脚本时曾观察到 `server is busy`、Node CPU 升高和
  `wxml-lsp` 进程退出；全新重启后恢复，尚未根因定位。本轮正常验收未复现。
- 连续键入的诊断/补全延迟仍需单独测量；Windows/macOS 实机未验收。

