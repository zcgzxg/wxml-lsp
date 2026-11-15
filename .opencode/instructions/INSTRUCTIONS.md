# ECC - OpenCode Instructions

This document consolidates ECC's core rules for OpenCode, adapted for this Rust project. The Rust-specific rules in `instructions/rust/*.md` take precedence for `.rs` files.

## Security Guidelines (CRITICAL)

### Mandatory Security Checks

Before ANY commit:
- [ ] No hardcoded secrets (API keys, passwords, tokens)
- [ ] All untrusted input validated and bounded
- [ ] File paths confined to the workspace
- [ ] No shell interpolation in `Command`
- [ ] Every `unsafe` block justified with `// SAFETY:`
- [ ] Dependencies audited (`cargo audit`)
- [ ] Error messages don't leak sensitive data

### Secret Management

```rust
// NEVER: Hardcoded secrets
const API_KEY: &str = "sk-proj-xxxxx";

// ALWAYS: Environment variables, validated at startup
fn load_api_key() -> anyhow::Result<String> {
    std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY not configured")
}
```

### Security Response Protocol

If security issue found:
1. STOP immediately
2. Use **rust-security-reviewer** agent
3. Fix CRITICAL issues before continuing
4. Rotate any exposed secrets
5. Review entire codebase for similar issues

---

## Coding Style

### Ownership and Immutability (CRITICAL)

Rust values are immutable by default. Prefer borrowing over cloning, and return new values instead of mutating shared state:

```rust
// WRONG: unnecessary mutation and clone
fn rename(user: &mut User, name: String) {
    user.name = name;
}

// CORRECT: borrow by default, return a new value when ownership changes
#[derive(Clone)]
struct User { name: String, /* ... */ }

fn renamed(user: &User, name: impl Into<String>) -> User {
    User { name: name.into(), ..user.clone() }
}
```

Do not add `.clone()` just to silence the borrow checker — understand the lifetime first.

### File Organization

MANY SMALL FILES > FEW LARGE FILES:
- High cohesion, low coupling
- 200-400 lines typical, 800 max
- Extract utilities from large components
- Organize by feature/domain, not by type

### Error Handling

ALWAYS propagate errors with `?` and add context. Libraries use typed errors (`thiserror`); applications use `anyhow`:

```rust
use anyhow::{Context, Result};

fn load_config(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&text).context("invalid config")
}
```

Never use `unwrap()` or `expect()` in production paths.

### Input Validation

Validate at system boundaries and make invalid states unrepresentable. Prefer parsing into typed values over ad-hoc checks:

```rust
use serde::Deserialize;
use validator::Validate;

#[derive(Deserialize, Validate)]
pub struct CreateUser {
    #[validate(email)]
    pub email: String,
    #[validate(range(min = 0, max = 150))]
    pub age: u8,
}
```

### Code Quality Checklist

Before marking work complete:
- [ ] Code is readable and well-named
- [ ] Functions are small (<50 lines)
- [ ] Files are focused (<800 lines)
- [ ] No deep nesting (>4 levels)
- [ ] Errors propagated with context; no `unwrap()` in production
- [ ] No `println!` debug output
- [ ] No hardcoded secrets or values
- [ ] `cargo fmt` and `cargo clippy -- -D warnings` pass
- [ ] No unnecessary `.clone()`; `unsafe` blocks justified with `// SAFETY:`

---

## Testing Requirements

### Minimum Test Coverage: 80%

Test Types (ALL required where applicable):
1. **Unit Tests** - `#[cfg(test)]` modules next to the code under test
2. **Integration Tests** - `tests/` directory; for this project, end-to-end LSP flows over JSON-RPC
3. **Property Tests** - `proptest` for parsers, position math, and other input-heavy logic

Run `cargo test` for the full suite. Use `cargo nextest` or `cargo llvm-cov` when available for faster runs and coverage.

### Test-Driven Development

MANDATORY workflow:
1. Write test first (RED)
2. Run test - it should FAIL
3. Write minimal implementation (GREEN)
4. Run test - it should PASS
5. Refactor (IMPROVE)
6. Verify coverage (80%+)

### Troubleshooting Test Failures

1. Use **rust-tdd-guide** agent with the `rust-testing` skill
2. Check test isolation
3. Verify mocks are correct
4. Fix implementation, not tests (unless tests are wrong)

---

## Git Workflow

### Commit Message Format

```
<type>: <description>

<optional body>
```

Types: feat, fix, refactor, docs, test, chore, perf, ci

### Pull Request Workflow

When creating PRs:
1. Analyze full commit history (not just latest commit)
2. Use `git diff [base-branch]...HEAD` to see all changes
3. Draft comprehensive PR summary
4. Include test plan with TODOs
5. Push with `-u` flag if new branch

### Feature Implementation Workflow

1. **Plan First**
   - Use **planner** agent to create implementation plan
   - Identify dependencies and risks
   - Break down into phases

2. **TDD Approach**
   - Use **rust-tdd-guide** agent
   - Write tests first (RED)
   - Implement to pass tests (GREEN)
   - Refactor (IMPROVE)
   - Verify 80%+ coverage

3. **Code Review**
   - Use **code-reviewer** agent immediately after writing code
   - Address CRITICAL and HIGH issues
   - Fix MEDIUM issues when possible

4. **Commit & Push**
   - Detailed commit messages
   - Follow conventional commits format

---

## Agent Orchestration

### Available Agents

| Agent | Purpose | When to Use |
|-------|---------|-------------|
| planner | Implementation planning | Complex features, refactoring |
| architect | System design | Architectural decisions |
| rust-tdd-guide | Rust test-driven development | New features, bug fixes |
| code-reviewer | Code review | After writing code |
| rust-security-reviewer | Rust security analysis | Before commits |
| rust-build-resolver | Fix Rust build errors | When the build fails |
| rust-reviewer | Idiomatic Rust review | Rust code changes |
| doc-updater | Documentation | Updating docs |
| docs-lookup | Library documentation | Current API references |
| harness-optimizer | Agent harness tuning | Config and throughput |
| loop-operator | Autonomous loop control | Monitoring and intervention |

### Immediate Agent Usage

No user prompt needed:
1. Complex feature requests - Use **planner** agent
2. Code just written/modified - Use **code-reviewer** agent
3. Bug fix or new feature - Use **rust-tdd-guide** agent
4. Architectural decision - Use **architect** agent

---

## Performance Optimization

### Model and Subagent Strategy

- Use the model configured in OpenCode; do not hardcode model names in project files.
- Delegate broad exploration to the **explore** subagent and focused reviews to the specialist agents below.
- Keep the main thread for decisions and edits, not for long search loops.

### Context Window Management

Avoid last 20% of context window for:
- Large-scale refactoring
- Feature implementation spanning multiple files
- Debugging complex interactions

### Build Troubleshooting

If build fails:
1. Use **rust-build-resolver** agent
2. Analyze error messages
3. Fix incrementally
4. Verify after each fix

---

## Common Patterns

### Fallible API Boundaries

Return `Result` with a typed error and let callers decide how to present failures:

```rust
pub fn parse_document(text: &str) -> Result<Document, ParseError> {
    // ...
}
```

### Repository Pattern with Traits

Encapsulate I/O behind a trait so tests can substitute an in-memory implementation:

```rust
pub trait DocumentStore: Send + Sync {
    fn load(&self, uri: &Url) -> Result<Document, StoreError>;
    fn save(&self, doc: &Document) -> Result<(), StoreError>;
}
```

### Newtype Pattern

Use newtypes to prevent argument mix-ups and make invalid states unrepresentable:

```rust
struct DocumentUri(String);
struct ByteOffset(u32);
```

---

## OpenCode-Specific Notes

This project-local install does **not** include ECC's hook runtime or plugin. Run the checks manually.

### After Writing/Editing Code
- `cargo fmt`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`

### Before Committing
- Run security checks manually
- Verify no secrets in code
- Run full test suite

### Commands Available

Use these commands in OpenCode:
- `/plan` - Create implementation plan
- `/tdd` - Enforce TDD workflow
- `/code-review` - Review code changes
- `/security` - Run security review
- `/rust-review` - Rust-specific review
- `/rust-build` - Fix Rust build errors
- `/rust-test` - Rust TDD workflow
- `/build-fix` - Fix Rust build errors
- `/verify` - Run the Rust verification loop
- `/test-coverage` - Analyze coverage with cargo-llvm-cov
- `/checkpoint` - Save progress state
- `/update-docs` - Update documentation
- `/orchestrate` - Multi-agent workflow

---

## Success Metrics

You are successful when:
- All tests pass (80%+ coverage)
- No security vulnerabilities
- Code is readable and maintainable
- Performance is acceptable
- User requirements are met
