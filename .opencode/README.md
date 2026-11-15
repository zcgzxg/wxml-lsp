# ECC for OpenCode（Rust-only 项目级安装）

- 上游：https://github.com/affaan-m/ecc
- 版本：v2.2.3（commit `d2ef268`）
- 许可证：MIT
- 安装范围：仅当前项目，不修改全局 `~/.config/opencode`
- 语言范围：只保留 Rust 相关内容；Go、Java、Kotlin、C/C++、C#、F#、PHP、Python、Ruby、Swift、Dart、ArkTS/HarmonyOS、Perl、JavaScript/TypeScript 及其 Web 工具链不安装

## 目录结构

```
.opencode/
├── opencode.json          # 仅显式声明 instructions，其余由 OpenCode 自动发现
├── instructions/
│   ├── INSTRUCTIONS.md    # ECC 核心规则（已改写为 Rust 版本）
│   └── rust/              # Rust 专属规则：coding-style/hooks/patterns/security/testing
├── agents/                # 11 个 subagent（V2 markdown）
├── commands/              # 17 个斜杠命令
├── skills/                # 26 个技能（Rust + 语言无关工作流）
└── README.md              # 本文件
```

OpenCode 会自动发现 `.opencode/agents/`、`.opencode/commands/`、`.opencode/skills/`；`opencode.json` 只负责加载 instructions。`instructions` 路径按**项目根目录**解析，因此配置中写成 `.opencode/instructions/...`，而不是相对配置文件的 `instructions/...`。

## 已安装内容

### Agents（11）

通用工作流：

- `planner` — 实现计划
- `architect` — 系统设计
- `code-reviewer` — 代码评审
- `doc-updater` — 文档与 codemap
- `docs-lookup` — 当前库文档查询
- `harness-optimizer` — OpenCode 配置调优
- `loop-operator` — 自主循环监控

Rust 专属：

- `rust-reviewer` — Rust 惯用法与所有权评审
- `rust-build-resolver` — 编译/clippy 错误修复
- `rust-tdd-guide` — Rust TDD、cargo test/nextest、LSP JSON-RPC 测试
- `rust-security-reviewer` — unsafe/FFI、不可信 LSP 输入、路径、依赖审计

只读 agent（`permissions` 拒绝 edit）：`planner`、`architect`、`code-reviewer`、`docs-lookup`、`rust-reviewer`、`rust-security-reviewer`。

### Commands（17）

```
/build-fix          /checkpoint        /code-review       /eval
/learn              /orchestrate       /plan              /quality-gate
/rust-build         /rust-review       /rust-test         /security
/tdd                /test-coverage     /update-codemaps   /update-docs
/verify
```

- `/build-fix`、`/rust-build` → `rust-build-resolver`
- `/tdd`、`/rust-test`、`/test-coverage` → `rust-tdd-guide`
- `/security` → `rust-security-reviewer`
- `/verify` 使用 `cargo fmt`、`cargo check`、`cargo clippy -D warnings`、`cargo test`、可选 `cargo llvm-cov`/`cargo audit`

### Skills（26）

Rust：`rust-patterns`、`rust-testing`

语言无关工作流：`agent-introspection-debugging`、`agent-self-evaluation`、`agent-sort`、`architecture-decision-records`、`brownfield-retrospective`、`code-tour`、`codebase-onboarding`、`conventional-commit-review`、`council`、`dev-team`、`ecc-guide`、`eval-harness`、`git-workflow`、`growth-log`、`inherit-legacy-style`、`intent-driven-development`、`iterative-retrieval`、`living-docs-governance`、`loop-design-check`、`production-audit`、`product-lens`、`rules-distill`、`santa-method`、`skill-scout`

### Instructions（6）

`instructions/INSTRUCTIONS.md` 汇总 ECC 核心规则，并改写为 Rust 示例（`anyhow`/`Result`、所有权、serde 校验、cargo 质量门禁、LSP 测试）。`instructions/rust/*.md` 来自上游 `rules/rust/`，用于 `.rs` 文件时优先级更高。

## Rust-only 裁剪说明

未安装的 ECC 语言包：`arkts`、`c`、`cpp`、`csharp`、`fsharp`、`go`/`golang`、`harmonyos`、`java`、`javascript`、`kotlin`、`perl`、`php`、`python`、`rails`、`ruby`、`swift`、`typescript`，以及 `angular`/`react`/`vue`/`nuxt`/`react-native`/`web` 等规则包。

为避免在 Rust 项目中留下无法使用的 TypeScript/Playwright/PostgreSQL 工具链资产，另外移除了：

- Agents：`build-error-resolver`、`e2e-runner`、`database-reviewer`、`refactor-cleaner`、上游 `tdd-guide`、上游 `security-reviewer`（已由 Rust 版本替代）
- Commands：`e2e`、`setup-pm`、`refactor-clean`、`security-scan`、`harness-audit`，以及 Claude Code 专用的 `evolve`、`instinct-*`、`projects`、`promote`、`loop-start`、`loop-status`、`model-route`、`skill-create`
- Skills：`error-handling`、`e2e-testing`、`browser-qa`、`windows-desktop-e2e`、`verification-loop`、`tdd-workflow`、`click-path-audit`、`security-scan`、`ck`、`config-gc`、`delivery-gate`、`hookify-rules`、`plankton-code-quality`、`continuous-learning`、`continuous-learning-v2`、`deepseek-harness-setup`、`skill-stocktake`、`skill-host-compat`、`codehealth-mcp`、`configure-ecc`、`repo-scan`、`plan-canvas`、`ecc-recipes`、`skill-comply`、`strategic-compact`、`context-budget`、`coding-standards`（TS/JS 规范）、`ai-regression-testing`（TS/Vitest）、`unified-memory`（依赖 npm 运行时），以及依赖 Node 运行时的 `story-lifecycle`、`council-multi-model`

## 与上游安装器的差异

上游 `install.sh --target opencode` 只支持安装到 OpenCode 配置根目录（`OPENCODE_CONFIG_DIR`、`XDG_CONFIG_HOME/opencode` 或 `~/.config/opencode`），没有项目级 target；其 `opencode` profile 也会把 100+ 个命令、53 个通用技能和 hooks/plugin 运行时一起铺开。因此这里采用**项目级手工裁剪安装**：

1. 在上游仓库构建 OpenCode payload；
2. 安装到临时 staging 目录；
3. 只复制 Rust 与语言无关资产到项目的 `.opencode/`；
4. 统一转换为 OpenCode V2 格式并做 Rust 适配。

未安装 ECC hooks/plugin 运行时（与上游 `opencode` profile 默认排除 `hooks-runtime` 一致）：本项目没有 `plugins/`、`dist/`，也没有 OpenCode plugin 配置。Rust 质量检查通过手动命令执行，见 `instructions/rust/hooks.md`。

### 手工适配

- Rust 规则文件去掉上游 YAML frontmatter（`paths: **/*.rs`），改为由 `opencode.json` 的 `instructions` 显式加载。
- `instructions/rust/hooks.md` 重写为 “Rust Quality Checks (Manual)”，只包含 `cargo fmt`、`cargo clippy`、`cargo check`、`cargo test`。
- `INSTRUCTIONS.md` 的示例从 TypeScript/React/npm 改写为 Rust/axum 风格的 `Result`、所有权、serde、LSP JSON-RPC。
- agent 使用 OpenCode V2 markdown：frontmatter 为 `description`/`mode`/`permissions`，正文为系统提示。
- command 使用 V2 `subagent: true`（上游的 `subtask` 已改写）。
- 新增 `rust-tdd-guide`、`rust-security-reviewer` 两个 agent，替代上游 TS/Playwright/npm 导向的 `tdd-guide`、`security-reviewer`。
- 通用技能中的 `.claude/`、`~/.claude/`、`CLAUDE.md` 路径已改写为 `.opencode/`、`~/.config/opencode/`、`AGENTS.md`。

## 使用

在项目根目录启动 OpenCode 即可自动加载：

```bash
cd <项目根目录>
opencode
```

常用命令：

```text
/plan <需求>          创建实现计划
/tdd <需求>           Rust TDD 流程
/rust-review          审查 Rust 改动
/rust-build           修复 cargo/clippy 错误
/rust-test            补 Rust 测试
/verify               完整 Rust 质量门禁
/security             Rust 安全评审
/test-coverage        cargo-llvm-cov 覆盖率分析
```

## 更新

上游更新后，不要直接把 `install.sh` 指向本项目（它会写配置根目录）。建议重新 staging 后按本文件的清单做 diff：

```bash
cd /path/to/ECC
npm install --ignore-scripts --no-audit --no-fund
npm run build:opencode

OPENCODE_CONFIG_DIR="${TMPDIR:-/tmp}/ecc-stage" \
  node scripts/install-apply.js --target opencode --profile opencode --no-hooks
```

然后对比 `${TMPDIR:-/tmp}/ecc-stage` 与本目录，只同步 Rust 和语言无关资产；同步后重新运行下方的验证命令。

## 验证

```bash
cd <项目根目录>
opencode debug config    # 应列出 .opencode/opencode.json
opencode debug agents    # 应包含 rust-reviewer、rust-build-resolver、rust-tdd-guide、rust-security-reviewer
```

当前验证结果：配置文件 JSON 合法；`opencode debug config` 与 `opencode debug agents` 均正常；11 个 agent、17 个 command、26 个 skill 可被发现。

## 卸载

删除本目录即可，不影响全局 OpenCode 配置：

```bash
rm -rf .opencode
```
