# Zed 标准安装验收数据

日期：2026-11-16。环境：Zed 1.28.1（`e8bc16a`），隔离 Xvfb `:99`，1600x1000，
llvmpipe，无窗口管理器；隔离 user-data-dir；release `wxml-lsp` + 真实 Node 后端。

| 文件 | 内容 |
| --- | --- |
| `install-log.txt` | 标准 UI 安装触发的扩展/grammar 编译与加载日志（路径已清洗） |
| `extension-index-excerpt.json` | 安装后 `extensions/index.json` 中 `wxml-lsp` 与 WXML 语言注册的摘录；`dev=true`，server languages 为 WXML/JavaScript/TypeScript/TSX/JSON |
| `install-success.png` | 命令面板 `zed: install dev extension` 完成后的界面 |
| `hover-title.png` | 干净 fixture：`{{ title }}` hover 显示 `string` 和“页面标题” |
| `hover-user-name3.png` | 干净 fixture：`{{ user.name }}` hover 显示 `string` 和“用户显示名称” |
| `definition-title.png` | 干净 fixture：definition 跳转到 `index.ts` 的 `data.title` |
| `completion-tag.png` | 标签补全：`<vi` 返回 `video`/`view` 等 |
| `comp-attr.png` | 属性补全：`<view ` 返回 `class`/`id`/`style`/`wx:*` 等 |
| `comp-event.png` | 事件属性补全：`<view bindt` 返回 `bindtap`/`bindtouchend` 等 |
| `comp-member2.png` | Mustache 成员补全：`{{ user.` 返回 `name` |
| `diag-hover-nmae2.png` | 干净 fixture 改为 `user.nmae` 后，hover 显示成员不存在诊断和 `type quality: missing` |
| `real2-hover-src-inner.png` | 真实 TS 项目：`src` hover 显示 `(property) src: string` |
| `real2-def-src.png` | 真实 TS 项目：`src` definition 跳到 `index.ts` 的 `data.src` |
| `real2-completion-tag2.png` | 真实 TS 项目：`<vi` 标签补全 |
| `ai2-home-open.png` | 真实 JS 项目：复杂 WXML 的语法着色与布局 |
| `ai2-hover-title.png` | 真实 JS 项目：无 tsconfig/allowJs 时 hover 无结果（预期降级） |

截图中的绝对路径和业务项目内容仅用于验收，不作为长期唯一证据；结论和限制见
[P8 Zed 扩展证据](../../p8-zed.md) 第 13 节。
