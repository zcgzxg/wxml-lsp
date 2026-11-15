# Rust Quality Checks (Manual)

> Rust-specific rules. General ECC rules are consolidated in [INSTRUCTIONS.md](../INSTRUCTIONS.md).

This project-local install does not include ECC's hook runtime. Run these checks after editing `.rs` files:

- **cargo fmt** — format Rust sources
- **cargo clippy --all-targets -- -D warnings** — lint, treating warnings as errors
- **cargo check** — fast compilation check after changes
- **cargo test** — run the full test suite before committing
