# 未闭合成员表达式补全（2026-11-20）

## 1. 边界

- 任务基线 HEAD `64df115`；记录期间并行会话把工作树推进到 `94c8a66`。
- 只修改 `crates/wxml-lsp/src/backend_tsgo/{completion,mod,tests}.rs`、台账
  JSON/Markdown 和本证据文档；未修改默认 backend、Node、delivery、Cargo 或
  `input.rs`；未提交。
- 未放宽任何 TS oracle 断言。

## 2. 行为

| 输入（cursor 在 typed prefix 末尾） | 结果 |
| --- | --- |
| `{{ user.` | 挂载 `user`，synthetic `.` 后查询成员，replace 空 range |
| `{{ user.na`（未闭合） | 挂载 `user`，synthetic `.` 后查询成员，replace 精确覆盖 `na` |
| `{{ user.na }}`（闭合） | 走原有 mounted fragment 路径，replace 精确覆盖 `na` |
| `{{ .` | `Unsupported`，无 session |
| `{{ user. `（dot 后空格） | `Unsupported`，无 session |
| `{{ user.\n` | `Unsupported`，无 session |
| `{{ user.name.` | `Unsupported`，不合成成员链 |
| `{{ item.` | `Unsupported`，local root 不可解析 |

未闭合 mustache 不会出现在 host fragments 中；completion-only shadow 只挂载 bare
data root，因此 synthetic query 不改变任何原始模板字节。published replacement
range 由原始 text 的 byte offset 计算，query position 由 derived shadow 的 UTF-16
offset 计算。

## 3. Rust 证据

| Case / 行为 | Rust test |
| --- | --- |
| `TS-COMPLETION-0104` bare-root mount + synthetic query |  `crates/wxml-lsp/src/backend_tsgo/tests.rs:4457` |
| `TS-COMPLETION-0104` CJK/CRLF/emoji UTF-16 exact |  `crates/wxml-lsp/src/backend_tsgo/tests.rs:4531` |
| 未闭合 host syntax diagnostic 保留 |  `crates/wxml-lsp/src/backend_tsgo/tests.rs:4595` |
| 不安全形状显式 Unsupported |  `crates/wxml-lsp/src/backend_tsgo/tests.rs:4620` |
| `{{ user.na }}` 回归 |  `crates/wxml-lsp/src/backend_tsgo/tests.rs:1744` |
| `{{ user. }}` 回归 |  `crates/wxml-lsp/src/backend_tsgo/tests.rs:4793` |
| 未闭合 `{{ user.` 回归 |  `crates/wxml-lsp/src/backend_tsgo/tests.rs:4418` |

真实引擎 opt-in：`optional_real_engine_trailing_dot_completion_smoke` 同时覆盖
`{{ user. }}` 与未闭合 `{{ user.na`，断言 `name`/`age` 标签和精确 replace range。

## 4. 与 TS oracle 的差异

`TS-COMPLETION-0104` 的 TS 断言：

1. `response.state.status === "supported"`
2. `name` 必须出现
3. `name.replace === { start: 15, end: 17 }`

native 已满足 2 和 3（含 CJK/CRLF/emoji 的等价场景），但非空 native completion
仍必带 `Partial`：pinned completion API 没有 per-entry documentation，也没有
auto-import origin/import edit。该状态差是既有架构边界，不能在不放宽/伪造断言的
前提下消除，因此 `TS-COMPLETION-0104` 保持 `weaker` / `planned`，不提升 green。
replace range 与成员集合没有差异。

## 5. 验证

```sh
cargo fmt --all -- --check
cargo check -p wxml-lsp --all-targets
cargo clippy -p wxml-lsp --all-targets -- -D warnings
cargo test -p wxml-lsp --lib backend_tsgo::
cargo test -p wxml-lsp --lib completion
TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  cargo test -p wxml-lsp --lib optional_real_engine_trailing_dot_completion_smoke
cargo test -p wxml-lsp --test native_parity
jq empty docs/plans/tsgo-test-ledger.json
git diff --check
```

结果：fmt/check/clippy 通过；`backend_tsgo::` 306 passed；`completion` 177 passed；
真实引擎 trailing-dot smoke 的 2 个输入均通过；`native_parity` 37 passed / 26 ignored；
`jq empty` 与 `git diff --check` 通过。
