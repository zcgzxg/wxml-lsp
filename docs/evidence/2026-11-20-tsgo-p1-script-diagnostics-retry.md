# P1：脚本诊断在无关 JSON didOpen 后重试

日期：2026-11-20
HEAD：`b6c1ced`（工作树含最终收尾修复）
状态：P1 已修复；P2 保留为已知限制

## 缺陷

功能 smoke 发现：脚本首次 `textDocument/publishDiagnostics` 仍在计算时，客户端立即
`didOpen` 一个 JSON 文件，脚本诊断结果被静默丢弃且不再发布；等待 20 秒仍无通知，
之后编辑脚本才恢复。日志：

```text
[wxml-lsp] dropping script diagnostics for an old revision
```

## 根因

`crates/wxml-lsp/src/handlers/diagnostics.rs` 的 `script_diagnostics_done` 用
`workspace.is_current(&done.stamp)` 判断结果是否可发布。`workspace.is_current`
同时比较 project revision、文档 revision、view revision 与文本快照
（`crates/wxml-lsp/src/workspace.rs:462`）。任何 JSON `didOpen` 都会更新项目
revision（`apply_host_update`），使仍在计算中的脚本诊断 stamp 过期，原实现直接
`return Vec::new()`，没有重新调度。

## 修复

- 脚本结果先取出 `script_hash`；stamp 过期时再检查同一文件当前文本 hash。
- 文本未变则用最新 project revision 重新调度脚本诊断，`attempt` 有界递增，
  上限 `MAX_SCRIPT_DIAGNOSTICS_RETRIES = 3`，避免连续打开文件造成无限循环。
- 文本已变或超过重试上限仍丢弃，绝不把旧结果当成干净结果发布。
- 新增 `ScriptHost` JSON 路由/project revision 推进测试，以及真实 LSP 场景回归。

## 验证

- `cargo test -p wxml-lsp --lib`：851 passed。
- `cargo test -p wxml-lsp`：1098 passed / 40 ignored。
- `cargo clippy -p wxml-lsp --lib -- -D warnings`：No issues。
- 真实安装布局（复制 0.1.0 后替换当前 debug 二进制）LSP 探针：
  - 先 `didOpen` `pages/index/index.ts`，立即 `didOpen` `pages/index/index.json`；
  - 10 秒内收到 `publishDiagnostics` for `index.ts`（0 diagnostics，clean 空集）；
  - stderr 出现 `rescheduling script diagnostics after an unrelated workspace update`；
  - shutdown 正常。

复现脚本：`/tmp/opencode/p1_probe.py`；安装副本：`/tmp/opencode/p1-install`。

## P2（未修复，已记录）

仅含 `Page(...)`/`Component(...)`、没有顶层声明的脚本，`documentSymbol` 返回
`-32803 the script outline is incomplete` 且 `result=null`。这是当前语法轮廓
walkers 不遍历 Page/Component 对象字面量的诚实限制；未伪造空/完整符号。保留为
已知限制，后续可改成语义合法的 partial outline。
