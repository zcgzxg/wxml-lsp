# P7 native gate runner (2026-11-20)

## 0. Identity and boundary

- Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- Requested base HEAD: `7ca92a3` (`docs(delivery): adjudicate crash roll-forward semantics`)
- Verification HEAD: `94c8a66` (`test(delivery): verify default native zed path`).
  The worktree is shared; `e4fed4a`, `64df115` and `94c8a66` landed, and
  unrelated `crates/**` files were modified, while this runner was being
  validated. Those commits and files are not authored here. The local PASS is a
  point-in-time result on that moving tree; the runner itself is deterministic
  for the pinned engine and typings.
- Files authored here:
  - `scripts/native-gates.sh` (new)
  - `.github/workflows/ci.yml` (`workflow_dispatch` input and an independent
    `native-gates` job; the existing `check` and `native-package` jobs are
    unchanged)
  - this evidence file
- No production `crates/**` source, Cargo file, default backend, Node path,
  `#[ignore]` attribute or delivery source is changed. Nothing is committed.

**Result: local `NATIVE_GATES: PASS 26/26`** (23 `native_engine_real` + 3
`symbols_edits`). This is an opt-in required-native-job result. It is not a
default-CI pass, not `mandatory_green`, and not a P7/G7 closure.

---

## 1. Runner contract

`scripts/native-gates.sh` fails closed with exit **2** when a required input is
missing, relative, unpinned or unsafe. It never turns a missing package into a
skip.

| Input | Meaning |
| --- | --- |
| `--package DIR` (required) | Installed v2 native package containing `engine/lib/tsc` and `typings/`; must be absolute and exist. |
| `--engine FILE` | Override for `<package>/engine/lib/tsc`. |
| `--typings DIR` | Override for `<package>/typings`; accepts the typings package itself or its parent directory. |
| `--output DIR` | Log directory; default `/tmp/opencode/native-gates/<UTC timestamp>`; inside-repository paths are rejected. |
| `--skip-build` | Reuse an existing `cargo build --release -p wxml-lsp`. |
| `--verify-no-node` | Optional installer-scope `scripts/verify-no-node.sh` run after the 26 gates; requires `--installer` and `--install-root`. |

The runner exports these variables to every gate child:

```text
WXML_TEST_NATIVE=1
WXML_REQUIRE_REAL_ENGINE=1
TSGO_BIN=<pinned engine>
TSGO_ASSET_ROOT=<package>/engine
TSGO_TYPINGS_ROOT=<typings package directory>
```

It also removes `WXML_TEST_NATIVE_PACKAGE`, `WXML_TYPE_BACKEND`,
`WXML_BACKEND_DIR` and `WXML_NODE` from the child environment.

### 1.1 Pinned identity

Checked before the build and before any gate:

| Field | Expected | Observed |
| --- | --- | --- |
| `lib/tsc --version` | `Version 7.1.0-beta` | `Version 7.1.0-beta` |
| SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` | match |
| Size | `24,522,912` bytes | match |
| Typings | `index.d.ts` and `types/index.d.ts` | present |

### 1.2 Why `WXML_TEST_NATIVE_PACKAGE` is deliberately not exported

The gate with the diagnostic id `G-P5-E2E-13`
`native_static_wxs_answers_survive_an_unavailable_type_backend` deletes its
fixture engine after server startup to prove the unavailable-backend path; see
 `crates/wxml-lsp/tests/native_parity/native_engine_real.rs:1405`.

A first ad hoc attempt exported an explicit installed package through
`WXML_TEST_NATIVE_PACKAGE`. It passed 17 gates, then that gate removed
`engine/lib/tsc` from the shared package and the remaining 6 gates failed
(`explicit native package ... does not contain engine/lib/tsc`). The package was
restored from an identical pinned copy
(`/tmp/opencode/f3-14-default/install-root/versions/0.1.0/engine/lib/tsc`, same
`sha256`) for the runs below.

The runner therefore uses `--package` only as the source of the pinned engine
and typings, and passes `TSGO_*` so every gate builds a disposable package in
`/tmp/opencode`. This is required for a repeatable batch and is the main reason
the runner does not blindly forward an installed package.

---

## 2. Local verification

### 2.1 Positive runs

```bash
bash -n scripts/native-gates.sh
./scripts/native-gates.sh --package /tmp/opencode/tsgo-native-pkg
```

Result at `e4fed4a`: `NATIVE_GATES: PASS 26/26`, build PASS, logs in
`/tmp/opencode/native-gates/20261119T193125Z/`.

After another worker advanced the worktree to `64df115`, the same runner was
rerun at the new HEAD:

```bash
./scripts/native-gates.sh --package /tmp/opencode/tsgo-native-pkg \
  --output /tmp/opencode/native-gates/final-verify
```

Result: `NATIVE_GATES: PASS 26/26`, exit 0, `head_before=head_after=64df115`.
Summary: `/tmp/opencode/native-gates/final-verify/summary.txt`. The worktree
then advanced to `94c8a66`; the final runner was rerun there and recorded
`NATIVE_GATES: PASS 26/26` in `/tmp/opencode/native-gates/final-final/`
(summary `head: 94c8a66`).

```text
build: PASS
gate: PASS native_parity::native_engine_real 23/23
gate: PASS native_parity::symbols_edits 3/3
NATIVE_GATES: PASS 26/26
```

`--skip-build` was also verified at the same package: `NATIVE_GATES: PASS
26/26`. A CI-like `CARGO_TERM_COLOR=always` environment was also run with
`--skip-build`; the runner forces `CARGO_TERM_COLOR=never` for the build and
gates, so the `test result:` parser still sees clean text and the run passed
26/26 with no ANSI escapes in the gate log.

### 2.2 Optional installer scope

```bash
./scripts/native-gates.sh --package /tmp/opencode/tsgo-native-pkg --skip-build \
  --output /tmp/opencode/native-gates/no-node-verify \
  --verify-no-node \
  --installer /tmp/opencode/f3-14-default/release/wxml-lsp-0.1.0-linux-x86_64/bin/wxml-lsp-install \
  --install-root /tmp/opencode/f3-14-default/install-root
```

Result: `NO_NODE_SCOPE: PASS (installer scope)` and `NATIVE_GATES: PASS 26/26`.
A forced failure (`--installer /bin/false` with the same valid root) reported
`NATIVE_GATES: PASS 26/26` plus `FAILED_SCOPE: no-node-installer (rc=1)` and
exited 1, so the optional scope cannot hide a failure behind the gate pass.
This is auxiliary installer evidence only; it does not close F3-02 or the
default-backend work. The existing `native-package` CI job already runs
`scripts/verify-no-node.sh` against its freshly installed package.

### 2.3 Negative runs (exit 2, never skip)

| Case | Command shape | Exit |
| --- | --- | --- |
| No arguments | `scripts/native-gates.sh` | 2 |
| Wrong engine version | `--package <pkg> --engine /tmp/opencode/native-gates-negative/lib/tsc` (a `true` copy) | 2 |
| Wrong engine digest | same path replaced by a script that prints `Version 7.1.0-beta`; SHA-256 differs | 2 |
| Missing typings | `--package <pkg> --typings /tmp/opencode/native-gates-empty-typings` | 2 |
| Output inside repo | `--package <pkg> --output <worktree>/native-gates-logs` | 2 |
| Relative package | `--package tmp/opencode/tsgo-native-pkg` | 2 |

The inside-repository output directory was not created. `bash -n
scripts/native-gates.sh` passed and the workflow file parsed as valid YAML with
jobs `check`, `native-package`, `native-gates`.

---

## 3. CI job

`.github/workflows/ci.yml` now has a `workflow_dispatch` boolean input
`native_gates` (default `false`) and an independent `native-gates` job:

- It runs only when dispatched with `native_gates=true`.
- It requires a pre-provisioned package through repository variable
  `TSGO_NATIVE_PACKAGE` or, as a fallback, the `TSGO_NATIVE_PACKAGE` secret
  (the variable takes precedence). `TSGO_NATIVE_TYPINGS` is an optional
  override and `TSGO_NATIVE_RUNNER` optionally selects a self-hosted runner
  label (default `ubuntu-latest`).
- If neither package source is set, or the resolved path does not exist, a
  dedicated preflight step writes a GitHub `::error::` annotation and exits 1.
  It does **not** skip.
- The 26 gates run through `scripts/native-gates.sh`; logs are uploaded as the
  `native-gates` artifact.
- The existing `check` job and `native-package` job are not modified. The
  `native-package` job continues to run its own `verify-no-node.sh` installer
  check.

The pinned 7.1.0-beta engine is a 24.5 MiB Linux x86_64 artifact that is not
vendored in this repository. A GitHub-hosted `ubuntu-latest` runner will fail
the preflight unless the package is mounted there; a self-hosted Linux x86_64
runner with the package is the intended configuration.

---

## 4. Remaining BLOCKED

- **CI job not executed on GitHub.** No runner with the package was available,
  so the job definition, preflight and artifact upload are locally reviewed but
  unexecuted.
- **Cross-platform not validated.** The runner and the pinned engine are Linux
  x86_64 only; macOS/Windows engine identity and path behavior are untested.
- **The old 23 `#[ignore]` tests remain opt-in.** This runner does not remove an
  `#[ignore]` attribute or change the default workspace test result, so the
  ordinary CI job still reports those tests as ignored. P7/G7 are not closed.
- **Shared-package hazard remains in the test sources.** Any batch that exports
  `WXML_TEST_NATIVE_PACKAGE` to an installed package can have its engine deleted
  by `G-P5-E2E-13`. The runner avoids this with per-gate fixtures; the tests
  themselves are unchanged by this task.
- **Not claimed:** default native backend, Node removal, delivery/source
  changes, or `mandatory_green`.
