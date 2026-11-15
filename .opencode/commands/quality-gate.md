# Quality Gate Command

Run the Rust quality gate on demand for a file, crate, or the whole workspace.

## Usage

`/quality-gate [path|.] [--fix] [--strict]`

- default target: the whole workspace
- `--fix`: run `cargo fmt --all` before checking
- `--strict`: treat clippy warnings as errors (default in this project)

## Pipeline

1. **Format** — `cargo fmt --all -- --check` (`--fix` runs `cargo fmt --all`)
2. **Check** — `cargo check --workspace --all-targets`
3. **Lint** — `cargo clippy --workspace --all-targets -- -D warnings`
4. **Tests** — `cargo test --workspace` (skip only when the target is a single
   non-code file and say so)
5. **Report** — list every failure with `path:line`, the command that found it,
   and the smallest fix. Do not claim a check passed without running it.

## Notes

This command mirrors the manual checks in `instructions/rust/hooks.md`; there is
no hook runtime in this installation.

## Arguments

$ARGUMENTS:
- `[path|.]` optional target path
- `--fix` optional
- `--strict` optional
