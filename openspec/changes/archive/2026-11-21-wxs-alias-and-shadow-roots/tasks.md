# Tasks

## 1. WXS 别名导出签名

- [x] 1.1 `crates/wxml-types/src/wxs.rs::classify_initializer`：初始值为单个标识符时继承 `local_by_name` 的 kind/type/signature。
- [x] 1.2 增加单元测试：函数别名保留参数名与返回类型、值别名仍为 Value、简写导出不变、未命中别名保守降级。
- [x] 1.3 `cargo test -p wxml-types` 与 `cargo clippy -p wxml-types --all-targets --all-features -- -D warnings`。

## 2. shadow 中的 WXS 模块根

- [x] 2.1 `PreparedView` 增加 `wxs_modules`，从模板静态 WXS 块收集合法模块名（去重、过滤 shadow 保留名）；更新所有构造点与测试字面量。
- [x] 2.2 `FragmentSpec` 增加 `wxs_modules`，`fragment_spec` 填充；`FragmentSpec::new` 与现有字面量补默认值。
- [x] 2.3 `effective_local_names` 并入模块名；probe 顶部按并集声明一次 `const <name>: any = null as any;`，不增加 unresolved。
- [x] 2.4 保持 `fragment_eligibility` 只按 `bindings` 判断自由根；补充“仅 WXS 模块根仍挂载”的测试。
- [x] 2.5 测试：WXS 模块调用不产生缺失属性诊断、混合 data 错误仍报告、`wx:for` 同名遮蔽、保留名过滤、无重复声明。
- [x] 2.6 `cargo test -p wxml-backend-tsgo`、`cargo test -p wxml-lsp` 与 clippy。

## 3. 集成验证

- [x] 3.1 真实引擎测试：WXS 模块调用无 `Property 'm1'` 诊断；别名导出 hover 含参数名。
- [x] 3.2 真实项目 stdio 复现：`apps/miniprogram/src/appointment/pages/create-appointment/index.wxml` 第 432 行 `m1` 无红线；`m1.getLimitSettingText` hover 显示 `(attendLimitSetting: any, userAppointLimit: any) => string`。
- [x] 3.3 全量 `cargo fmt --all -- --check`、clippy workspace、`cargo test --workspace --all-features`、`cargo doc --workspace --no-deps`、`cargo deny --offline check`。

## 4. 收尾

- [x] 4.1 `openspec validate wxs-alias-and-shadow-roots --strict`。
- [x] 4.2 归档并提交（Conventional Commits，例如 `fix(wxs): 修复模板 WXS 模块误报并保留别名函数签名`）。
