---
description: Analyze Rust test coverage with cargo-llvm-cov and close meaningful gaps
agent: rust-tdd-guide
subagent: true
---

# Rust Test Coverage Command

Analyze coverage and add tests for the gaps that matter: $ARGUMENTS

## Your Task

1. **Run coverage**:
   ```bash
   cargo llvm-cov --workspace --all-features --summary-only
   ```
   For a detailed per-line report:
   ```bash
   cargo llvm-cov --workspace --all-features --html
   ```
   The report is written under `target/llvm-cov/html/index.html`.
   If `cargo-llvm-cov` is unavailable, fall back to
   `cargo nextest run --workspace` plus a manual review, and say so.

2. **Identify low-coverage modules**, prioritizing changed code and
   security-sensitive paths over raw percentage.
3. **Write tests** following the `rust-testing` skill: unit tests in
   `#[cfg(test)]`, integration tests in `tests/`, `proptest` for parser and
   range math, `#[tokio::test]` for async handlers.
4. **Re-run coverage** and report the delta.

## Coverage targets

| Code type | Target |
|-----------|--------|
| Changed code | 80% |
| Parser / range and position math | 90% |
| Path, URI, and input validation | 100% |
| LSP request handlers | 80% |
| Utilities | 90% |

## Report format

### Summary
```
cargo llvm-cov --workspace --all-features
TOTAL   XX.XX% lines   XX.XX% functions   XX.XX% regions
```

### Low-coverage modules
| Module | Lines | Missing |
|--------|-------|---------|
| `src/...` | XX% | [specific functions/branches] |

### Tests added
- `path/to/file.rs` — [behavior covered]
- `tests/...` — [flow covered]

### Remaining gaps
[Why each gap is hard or low-value to cover; do not add assertion-free tests]

---

**IMPORTANT**: Coverage is a metric, not a goal. Prefer one meaningful assertion
over ten executed lines. Do not add tests that only call a function and assert
nothing.
