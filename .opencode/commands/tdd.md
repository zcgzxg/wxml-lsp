---
description: Enforce test-driven development in Rust with 80%+ coverage
agent: rust-tdd-guide
subagent: true
---

# Rust TDD Command

Implement the following with strict test-driven development: $ARGUMENTS

## TDD cycle (mandatory)

```
RED → GREEN → REFACTOR → REPEAT
```

1. **RED** — write one failing test and run it. Confirm it fails for the
   expected reason, not because the crate does not compile.
2. **GREEN** — write the smallest implementation that passes.
3. **REFACTOR** — improve names and structure while the suite stays green.
4. **REPEAT** — one behavior per cycle.

## Your task

### Step 1: Define the shape
- Define the Rust types: structs for data, enums for variants, traits for
  boundaries.
- Add function signatures with `todo!()` so the test can compile.
- Keep the public surface as small as the behavior requires.

### Step 2: Write the failing test (RED)
- Unit test in `#[cfg(test)] mod tests`; integration test in `tests/` when
  exercising the public API.
- Cover the happy path, edge cases (empty, non-ASCII WXML text, max offsets),
  and error variants.
- Run `cargo test <name_fragment>` and show the failure.

### Step 3: Implement the minimum (GREEN)
- No premature abstraction, no extra features.
- Return `Result` for fallible operations; no `unwrap()` in production code.
- Run the same test and show it passing.

### Step 4: Refactor (IMPROVE)
- Remove duplication, tighten names, and keep behavior fixed.
- Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings`.

### Step 5: Check coverage
```bash
cargo llvm-cov --workspace --all-features --summary-only
```
Target 80% for changed code; 100% for path/URI validation and other
security-sensitive branches.

## Test types to include

- **Unit tests** — parsing, range math, validation, error mapping.
- **Integration tests** — public crate API and LSP JSON-RPC flows
  (`initialize` → `didOpen` → `completion`/`hover` → assert response).
- **Property tests** — round-trip and offset invariants with `proptest`.
- **Async tests** — `#[tokio::test]` for handlers; use timeouts, never sleeps.

---

**MANDATORY**: The test must exist and fail before the implementation. Never
skip the RED phase. Do not weaken a test to make it pass.
