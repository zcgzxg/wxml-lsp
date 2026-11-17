# W3 最小原始数据

本目录保存 W3 固定日志的关键行和校验值，用于人工复查。完整日志位于审核/集成工作区的
`test-logs/`，没有复制进仓库；下面记录的命令、输出行和 SHA-256 足以定位同一份材料。

> W4 最终验收（Q2、最终 release 与 Linux Zed 运行时冒烟）见
> [W4 最终验收与发布证据](../../full-feature-final.md)。本文件的 W3 数字保持原样，不随 W4 改写。

## 1. 材料校验值

| 材料 | SHA-256 |
| --- | --- |
| `reviews/full-semantic-tools-r1.md` | `86d129d2fe5a52c7fa53ca533a4fe737cd7f032d3861ef559c9bc27a79a3a2ba` |
| `reviews/full-semantic-tools-r2.md` | `be77e46b26a340a88d0a6d145b93983931e6d811104a2be030f24a60fd00a650` |
| `reviews/full-semantic-tools-security-r1.md` | `b5f21289e86dffdf7ea3a3ac0908dd07ef8b2dc5f8edb3269f70364f7265b28c` |
| `reviews/full-semantic-tools-security-r2.md` | `636f7d9ff4a50e3365dad7a586d9a9dd4bc7d5d37637c2524a2d85b74c687ad4` |
| `test-logs/integration-s-rust.log` | `3b4a5957a4c5a3ee45d861e00ad9752f1bbaf9139bacb88f5bedaa4c74ec753f` |
| `test-logs/integration-s-node-test-serial.log` | `bd7c8cfa7d5d323c81dd37d7b4773e017a08212208c3002250f9f5c6aae3f0da` |
| `test-logs/integration-s-node-test.log`（并行对照） | `6dbe5059bb9652dd86e6ed20b2ee0d26cc891ca87983d8b5fa13d4961bd363bf` |
| `test-logs/env-baseline.txt` | `c961bb0f579895e574da829487bef8c85ae6613e0be44f8be7798b97040cc696` |

运行环境（`env-baseline.txt`）：Linux x86_64；Rust/Cargo 1.100.0；Node 24.22.2；npm 11.19.1。
`env-baseline.txt` 记录的是会话开始时的 cwd/HEAD，不是每次测试的 HEAD；W3 结果按
集成分支合并 S 后的工作树记录，不把该文件里的 `head=577f530` 当作测试对象 HEAD。

## 2. Rust workspace（W3 固定日志）

命令：`cargo test --workspace -- --test-threads=4`（日志文件 `integration-s-rust.log`）。

汇总关键行：

```text
suite count: 45
passed:      1251
failed:      0
ignored:     21
```

21 个 ignored 的分布：

```text
tests/stdio_type_loop.rs   9 ignored
tests/real_node.rs         5 ignored
tests/wave5_acceptance.rs  7 ignored
```

`semantic_tools` 套件：

```text
running 41 tests
test result: ok. 41 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.11s
```

其中 `real_node_lsp_component_property_edit_applies_and_parses` 与
`real_node_lsp_rejects_cross_namespace_and_dynamic_edits` 在
`backends/typescript/dist/server.js` 存在时实际运行；该日志中没有
`skipping real Node LSP test` 行。

## 3. Node 后端（W3 固定日志）

串行命令：`cd backends/typescript && node --test --test-concurrency=1 test/*.test.ts`
（日志文件 `integration-s-node-test-serial.log`）。

```text
tests 326
suites 0
pass 326
fail 0
cancelled 0
skipped 0
todo 0
duration_ms 470534.995795
```

并行对照（日志文件 `integration-s-node-test.log`，默认 `npm test` 并发）：

```text
tests 326
pass 324
fail 2
duration_ms 203721.151696
```

两处失败都是宿主负载下的墙钟断言，不是功能断言：

```text
rough timings for cold start, hover and diagnostics (13373.194094ms)
  AssertionError: cold start 11959.45 ms
analyze returns the C1 response for a page (15039.143517ms)
  Error: timeout waiting for server response
```

W3 采用串行结果 `326/326`；并行计数只作为负载敏感性对照，不与串行结果混用。

## 4. 正式评审结论（摘要）

| 记录 | 审核对象 | 结论 |
| --- | --- | --- |
| `full-semantic-tools-r1.md` | S `411e4c1` | `CHANGES_REQUESTED`；8 BLOCKING / 9 MAJOR / 13 MINOR |
| `full-semantic-tools-r2.md` | S `d08844f` | `CHANGES_REQUESTED`；3 BLOCKING（S-B1/S-B8/S-M1 残留）/ 3 MAJOR；最终正式轮，不提出第 3 轮流程 |
| `full-semantic-tools-security-r1.md` | S `411e4c1` | `CHANGES_REQUESTED`；5 HIGH（2 项主线程冻结、3 项静默不完整/错误编辑） |
| `full-semantic-tools-security-r2.md` | S `d08844f` | `CHANGES_REQUESTED`；1 HIGH（`<import>` 模板解析重复读盘，6.3–30.9s 冻结）/ 1 MEDIUM |

`dd15bc1` 提交信息声明修复 security R2 H-1/M-1 和 Rust R2 残余项，并新增对应回归测试。
按用户设定的审核轮次上限，没有开启第 3 轮正式审核；残余项由集成侧探针复验，复验覆盖的
回归用例在 `integration-s-rust.log` 中通过：

```text
code_lens_with_shared_import_is_bounded ... ok
workspace_symbol_with_many_imports_is_bounded ... ok
loop_rename_is_linear_for_many_directives ... ok
loop_rename_stays_bounded_after_the_fragment_budget ... ok
component_property_rename_rejects_component_data_conflict ... ok
data_rename_rejects_component_property_conflict ... ok
oversized_sibling_script_makes_source_snapshot_incomplete ... ok
real_node_lsp_rejects_cross_namespace_and_dynamic_edits ... ok
component_code_lens_uses_current_document_anchor_for_includes ... ok
```

这不构成“第 3 轮审核通过”或“正式审核 APPROVED”的声明；只说明已记录的具体残余项有
可复跑的回归测试并在该日志中通过。

## 5. 墙钟敏感性

`semantic_tools` 中的 8000 条 `wx:for` 重命名用例带 1 秒墙钟上限。并行套件运行时曾出现
`1.10s > 1s` 失败；单独运行该测试为 0.68–0.85s 通过。阈值没有修改，W3 固定日志中的
`loop_rename_stays_bounded_after_the_fragment_budget` 通过。该断言只说明线性预算行为，
不应当作为大项目性能验收或 Q2 性能结论。
