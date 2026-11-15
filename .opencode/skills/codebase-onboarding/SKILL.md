---
name: codebase-onboarding
description: Analyze an unfamiliar codebase and generate a structured onboarding guide with architecture map, key entry points, conventions, and a starter AGENTS.md. Use when joining a new project or setting up OpenCode for the first time in a repo.
metadata:
  origin: ECC
---

# Codebase Onboarding

Systematically analyze an unfamiliar codebase and produce a structured onboarding guide. Designed for developers joining a new project or setting up OpenCode in an existing repo for the first time.

## When to Use

- First time opening a project with OpenCode
- Joining a new team or repository
- User asks "help me understand this codebase"
- User asks to generate a AGENTS.md for a project
- User says "onboard me" or "walk me through this repo"

## How It Works

### Phase 1: Reconnaissance

Gather raw signals about the project without reading every file. Run these checks in parallel:

```
1. Manifest detection
   → Cargo.toml, Cargo.lock, rust-toolchain.toml, .cargo/config.toml,
     workspace `members`

2. Crate and framework fingerprinting
   → tower-lsp / lsp-server / async-lsp, tokio, serde, serde_json,
     parser or formatter crates, proc-macro crates

3. Entry point identification
   → src/main.rs, src/lib.rs, src/bin/*, src/lsp/, src/parser/

4. Directory structure snapshot
   → Top 2 levels of the directory tree, ignoring target/, .git, vendor

5. Config and tooling detection
   → rustfmt.toml, clippy.toml, deny.toml, Makefile, Dockerfile,
     docker-compose*, .github/workflows/, .env.example, CI configs

6. Test structure detection
   → tests/, benches/, #[cfg(test)] modules, nextest.toml,
     proptest / rstest / mockall usage
```

### Phase 2: Architecture Mapping

From the reconnaissance data, identify:

**Tech Stack**
- Language(s) and version constraints
- Framework(s) and major libraries
- Database(s) and ORMs
- Build tools and bundlers
- CI/CD platform

**Architecture Pattern**
- Monolith, monorepo, microservices, or serverless
- Frontend/backend split or full-stack
- Transport: stdio JSON-RPC, TCP, or in-process

**Key Directories**
Map the top-level directories to their purpose:

<!-- Example for a Rust LSP project — replace with detected directories -->
```
src/lsp/         → LSP handlers and capabilities
src/parser/      → WXML parser and position mapping
src/workspace/   → Document and workspace state
src/lib.rs       → Public crate API
tests/           → Integration tests (JSON-RPC flows)
benches/         → Criterion benchmarks
```

**Data Flow**
Trace one request from entry to response:
- Where does a request enter? (router, handler, controller)
- How is it validated? (middleware, schemas, guards)
- Where is business logic? (services, models, use cases)
- How does it reach the database? (ORM, raw queries, repositories)

### Phase 3: Convention Detection

Identify patterns the codebase already follows:

**Naming Conventions**
- File naming: kebab-case, camelCase, PascalCase, snake_case
- Component/class naming patterns
- Test file naming: `*.test.ts`, `*.spec.ts`, `*_test.go`

**Code Patterns**
- Error handling style: try/catch, Result types, error codes
- Dependency injection or direct imports
- State management approach
- Async patterns: callbacks, promises, async/await, channels

**Git Conventions**
- Branch naming from recent branches
- Commit message style from recent commits
- PR workflow (squash, merge, rebase)
- If the repo has no commits yet or only a shallow history (e.g. `git clone --depth 1`), skip this section and note "Git history unavailable or too shallow to detect conventions"

### Phase 4: Generate Onboarding Artifacts

Produce two outputs:

#### Output 1: Onboarding Guide

```markdown
# Onboarding Guide: [Project Name]

## Overview
[2-3 sentences: what this project does and who it serves]

## Tech Stack
<!-- Example for a Rust LSP project — replace with detected stack -->
| Layer | Technology | Version |
|-------|-----------|---------|
| Language | Rust | edition 2021/2024 |
| LSP runtime | tower-lsp / lsp-server | current |
| Async runtime | tokio | 1.x |
| Serialization | serde + serde_json | 1.x |
| Testing | cargo test + nextest + cargo-llvm-cov | - |

## Architecture
[Diagram or description of how components connect]

## Key Entry Points
<!-- Example for a Rust LSP project — replace with detected paths -->
- **Binary**: `src/main.rs` — process startup and stdio transport
- **Library**: `src/lib.rs` — public crate API
- **LSP handlers**: `src/lsp/` — initialize, diagnostics, completion, hover
- **Parser**: `src/parser/` — WXML tokenizer/parser and position mapping
- **Manifest**: `Cargo.toml` — workspace members, features, dependencies

## Directory Map
[Top-level directory → purpose mapping]

## Request Lifecycle
[Trace one LSP request from JSON-RPC input to response]

## Conventions
- [File naming pattern]
- [Error handling approach]
- [Testing patterns]
- [Git workflow]

## Common Tasks
<!-- Example for a Rust project — replace with detected commands -->
- **Run the server**: `cargo run`
- **Run tests**: `cargo test`
- **Run linter**: `cargo clippy --all-targets -- -D warnings`
- **Format**: `cargo fmt --check`
- **Build for production**: `cargo build --release`

## Where to Look
<!-- Example for a Rust LSP project — replace with detected paths -->
| I want to... | Look at... |
|--------------|-----------|
| Add an LSP method | `src/lsp/` |
| Change parsing | `src/parser/` |
| Add a test | `tests/` or the nearest `#[cfg(test)]` module |
| Change build config | `Cargo.toml` / `.cargo/config.toml` |
```

#### Output 2: Starter AGENTS.md

Generate or update a project-specific AGENTS.md based on detected conventions. If `AGENTS.md` already exists, read it first and enhance it — preserve existing project-specific instructions and clearly call out what was added or changed.

```markdown
# Project Instructions

## Tech Stack
[Detected stack summary]

## Code Style
- [Detected naming conventions]
- [Detected patterns to follow]

## Testing
- Run tests: `[detected test command]`
- Test pattern: [detected test file convention]
- Coverage: [if configured, the coverage command]

## Build & Run
- Dev: `[detected dev command]`
- Build: `[detected build command]`
- Lint: `[detected lint command]`

## Project Structure
[Key directory → purpose map]

## Conventions
- [Commit style if detectable]
- [PR workflow if detectable]
- [Error handling patterns]
```

## Best Practices

1. **Don't read everything** — reconnaissance should use Glob and Grep, not Read on every file. Read selectively only for ambiguous signals.
2. **Verify, don't guess** — if a framework is detected from config but the actual code uses something different, trust the code.
3. **Respect existing AGENTS.md** — if one already exists, enhance it rather than replacing it. Call out what's new vs existing.
4. **Stay concise** — the onboarding guide should be scannable in 2 minutes. Details belong in the code, not the guide.
5. **Flag unknowns** — if a convention can't be confidently detected, say so rather than guessing. "Could not determine test runner" is better than a wrong answer.

## Anti-Patterns to Avoid

- Generating a AGENTS.md that's longer than 100 lines — keep it focused
- Listing every dependency — highlight only the ones that shape how you write code
- Describing obvious directory names — `src/` doesn't need an explanation
- Copying the README — the onboarding guide adds structural insight the README lacks

## Examples

### Example 1: First time in a new repo
**User**: "Onboard me to this codebase"
**Action**: Run full 4-phase workflow → produce Onboarding Guide + Starter AGENTS.md
**Output**: Onboarding Guide printed directly to the conversation, plus a `AGENTS.md` written to the project root

### Example 2: Generate AGENTS.md for existing project
**User**: "Generate a AGENTS.md for this project"
**Action**: Run Phases 1-3, skip Onboarding Guide, produce only AGENTS.md
**Output**: Project-specific `AGENTS.md` with detected conventions

### Example 3: Enhance existing AGENTS.md
**User**: "Update the AGENTS.md with current project conventions"
**Action**: Read existing AGENTS.md, run Phases 1-3, merge new findings
**Output**: Updated `AGENTS.md` with additions clearly marked
