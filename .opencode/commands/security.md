---
description: Run a Rust security review focused on unsafe, untrusted input, paths, secrets, and dependencies
agent: rust-security-reviewer
subagent: true
---

# Rust Security Review Command

Conduct a security review of the specified Rust code: $ARGUMENTS

## Your Task

Review the scope against the project's threat model: an LSP server receiving
untrusted JSON-RPC, document text, file URIs, and workspace paths from an editor.

## Checklist

### Memory safety
- [ ] Every `unsafe` block has a `// SAFETY:` comment and a sound invariant
- [ ] No unnecessary `transmute`, raw pointer arithmetic, or `from_raw_parts`
- [ ] FFI validates pointers, lengths, ownership, and panics at the boundary
- [ ] `Send`/`Sync` impls are justified

### Untrusted input
- [ ] serde bounds payload size and recursion; closed schemas reject unknown fields
- [ ] Byte-offset slicing handles multi-byte UTF-8 without panicking
- [ ] Offset/length arithmetic is checked or saturating; no lossy `as` casts
- [ ] Regex/parser loops are bounded (no catastrophic backtracking)
- [ ] JSON-RPC errors are returned instead of panicking

### Paths and commands
- [ ] `..` and absolute paths are rejected or confined to the workspace
- [ ] Canonicalized paths are checked against the workspace prefix
- [ ] `Command` uses argument arrays, never a shell string
- [ ] Symlinks cannot escape the workspace without an explicit policy

### Secrets and logging
- [ ] No API keys, tokens, or credentials in source or fixtures
- [ ] `tracing`/`Debug` output does not leak secrets or full document text
- [ ] Error messages returned to the client are redacted

### Dependencies
- [ ] `cargo audit` is clean or findings are triaged
- [ ] `cargo deny check advisories bans licenses sources` passes when configured
- [ ] `build.rs` has no unexpected network or code execution
- [ ] Risky features (TLS, HTTP, filesystem) are not enabled unnecessarily

### Availability
- [ ] Queues, channels, and caches are bounded
- [ ] Oversized documents are rejected
- [ ] No `unwrap()`/`expect()` on attacker-controlled data
- [ ] A single request cannot block the server indefinitely

## Commands to run

```bash
cargo audit
cargo deny check advisories bans licenses sources   # if configured
cargo tree -d
rg -n 'unsafe|transmute|from_raw_parts|Command::new|unwrap\(\)|expect\(' src
```

## Report format

Group findings as **Critical**, **High**, **Medium**, **Low**. For each finding:
- `path/to/file.rs:line`
- What the attacker controls
- Impact and a reproduction sketch
- Concrete fix

End with **BLOCK** if any Critical/High finding exists, otherwise **PASS WITH
NOTES**. List any check that could not be run as SKIPPED.

---

**IMPORTANT**: Security issues are blockers. Do not proceed until critical
issues are resolved.
