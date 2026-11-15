---
description: Fix Rust build, type, and clippy errors with minimal changes
agent: rust-build-resolver
subagent: true
---

# Rust Build Fix Command

Fix Rust compiler, type, borrow-checker, and clippy errors with minimal changes: $ARGUMENTS

## Your Task

1. **Run the checks**
   ```bash
   cargo check --workspace --all-targets
   cargo clippy --workspace --all-targets -- -D warnings
   ```
2. **Collect all errors** — group them by root cause, not by file order.
3. **Fix one root cause at a time** with the smallest diff.
4. **Re-run the same command** after each fix.
5. **Finish with** `cargo fmt --check`, clippy, and `cargo test --workspace`.

## Approach

### DO
- Fix ownership/borrowing with a borrow, a clone with a documented reason, or a
  restructure — in that order of preference.
- Add missing imports (`use`), feature flags, or workspace dependencies.
- Add explicit types where inference genuinely needs help.
- Fix `?` conversions by implementing `From` or mapping the error.
- Keep public APIs stable unless the compile error proves the signature wrong.

### DON'T
- Refactor unrelated code, add features, or change architecture.
- Add `#[allow(...)]` without a comment explaining why the lint is wrong.
- Add `unwrap()`/`expect()` to silence a `Result` error.
- Change business logic to make the compiler happy.
- Delete tests or weaken assertions.

## Common Rust errors

| Error | Typical fix |
|-------|-------------|
| E0382 use of moved value | Borrow instead of move, or clone deliberately |
| E0502 cannot borrow as mutable | Narrow the borrow scope; split the struct or use a method |
| E0597 value does not live long enough | Move ownership up; return an owned value |
| E0499 more than one mutable borrow | Restructure the call to avoid overlapping borrows |
| E0277 trait bound not satisfied | Add the bound, derive the trait, or fix the type |
| E0308 mismatched types | Convert explicitly with `From`/`TryFrom` |
| E0433 failed to resolve | Add the crate to `Cargo.toml` and `use` it |
| E0716 temporary dropped while borrowed | Bind the temporary to a `let` |
| `future is not Send` | Remove the non-`Send` value across `.await`, or use a scoped task |
| clippy `needless_borrow` / `redundant_clone` | Apply the lint's suggestion and re-test |

## Verification

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must pass. Report the commands run, the errors fixed, and any lint
that was intentionally allowed with its justification.

---

**IMPORTANT**: Get the build green with the smallest correct diff. No refactoring,
no improvements, no architectural changes.
