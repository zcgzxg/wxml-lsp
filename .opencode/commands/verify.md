---
description: Run the Rust verification loop to validate an implementation
agent: build
---

# Rust Verify Command

Run the full verification loop for this workspace: $ARGUMENTS

## Your Task

Execute, in order, and stop at the first failure that blocks later steps:

1. **Format**: `cargo fmt --all -- --check`
2. **Check**: `cargo check --workspace --all-targets`
3. **Lint**: `cargo clippy --workspace --all-targets -- -D warnings`
4. **Unit + integration tests**: `cargo test --workspace`
5. **Doc tests**: `cargo test --workspace --doc`
6. **Coverage** (when `cargo-llvm-cov` is installed):
   `cargo llvm-cov --workspace --all-features --summary-only`
7. **Dependency advisories** (when `cargo-audit` is installed): `cargo audit`

If a tool is not installed, report it as SKIPPED and do not invent a result.
Use `cargo nextest run --workspace` instead of step 4 when nextest is the
project's configured runner.

## Verification checklist

### Code quality
- [ ] `cargo fmt` clean
- [ ] `cargo clippy` clean with `-D warnings`
- [ ] No `unwrap()`/`expect()` in production paths
- [ ] No `println!`/`dbg!` left in library or server code (use `tracing`)
- [ ] Every `unsafe` block has a `// SAFETY:` comment
- [ ] Public items have rustdoc where behavior is non-obvious

### Tests
- [ ] All unit and integration tests pass
- [ ] LSP JSON-RPC flows covered for changed handlers
- [ ] Error and edge cases covered
- [ ] Coverage >= 80% for changed code

### Security
- [ ] No hardcoded secrets
- [ ] Untrusted JSON-RPC/document input validated and bounded
- [ ] Path handling rejects traversal outside the workspace
- [ ] No shell interpolation in `Command`
- [ ] `cargo audit` clean or findings triaged

## Verification report

### Summary
- Status: PASS / FAIL
- Score: X/Y checks passed

### Details
| Check | Status | Notes |
|-------|--------|-------|
| Format | PASS/FAIL/SKIPPED | |
| Check | PASS/FAIL | |
| Clippy | PASS/FAIL | |
| Tests | PASS/FAIL | |
| Doc tests | PASS/FAIL | |
| Coverage | PASS/FAIL/SKIPPED | XX% (target: 80%) |
| Audit | PASS/FAIL/SKIPPED | |

### Action items
[If FAIL, list exactly what must be fixed and where]

---

**NOTE**: Run this before every commit and PR. Do not mark a check PASS without
showing the command output.
