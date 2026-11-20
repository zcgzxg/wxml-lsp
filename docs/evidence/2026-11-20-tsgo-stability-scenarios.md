# G-P5-STABILITY-01 scenario closure (2026-11-20)

## 0. Identity and scope

- Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD: `b6c1ced` (`docs(p7): record clean native gate rerun`)
- Date: 2026-11-20
- Scope: `crates/wxml-lsp/tests/q2_stability.rs`, the new local support module
  `crates/wxml-lsp/tests/q2_stability_support/mod.rs`, and this evidence file.
  No production code, default backend, Node path, ledger, README, release
  script, `performance_bench.rs` or `type_assets.rs` is changed by this slice.
  Nothing is committed.
- The worktree carried unrelated edits during the evidence runs
  (`type_assets.rs`, `docs/evidence/README.md`,
  `docs/reference/tsgo-delivery-contract.md`) plus this slice. The evidence
  runs therefore used
  `WXML_STABILITY_ALLOW_DIRTY=1`; the strict clean-worktree default is
  unchanged and still fails without that local escape hatch.

**Result: 4 scenarios IMPLEMENTED / 1 BLOCKED in the canonical run.**
The blocked scenario is the external RSS baseline ratio; its comparison seam
is implemented, but no independent native server-tree baseline artifact is
committed, and the retired Node/Rust baseline is not comparable. A separate
seam probe (a previous native report used as an external baseline) reached
5/0 with ratio `1.006`, but that report is a run-local artifact, not a
committed baseline; the canonical gate result stays 4/1.

## 1. Why all assertions stay in one ignored test

`native_parity/native_gates.rs` freezes the whole workspace ignored-gate count
at `23` (`IGNORED_GATE_COUNT`) and maps each ignored function to the ledger.
Adding a new `#[ignore]` test for each scenario would be counted as gate drift,
not coverage. The five scenarios are therefore independent helper functions
called from the existing frozen test:

```text
ten_round_lifecycle_stability
├── run_typing_burst_and_cancel
├── run_resource_watched_round_trip
├── run_app_overlay_round_trip
├── run_bounded_queue_burst
└── external_rss_baseline (seam; BLOCKED without a committed artifact)
```

Each helper starts and stops its own real `wxml-lsp --stdio --backend native`
server and engine against the existing disposable pinned package. The helper
module only adds the raw protocol shapes the shared synchronous Session cannot
express (burst notifications, cancel by id, watched-file diagnostics without a
version bump, 40 in-flight requests). Its child environment, PATH tripwire and
private `TMPDIR` mirror `native_lsp::Session`.

## 2. Canonical result

Command (release, real package):

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
    -u WXML_TEST_NATIVE_PACKAGE \
  WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
  TSGO_TYPINGS_ROOT=/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/miniprogram-api-typings \
  WXML_STABILITY_ALLOW_DIRTY=1 WXML_STABILITY_LABEL=scenario-release-final \
  WXML_STABILITY_OUT=/tmp/opencode/q2-bench/stability-scenario-release-final.json \
  cargo test -p wxml-lsp --release --test q2_stability -- \
    --ignored --test-threads=1
```

Result: **1 passed / 0 failed / 9 filtered out**, `6.16 s`.

| # | Scenario | Status | Exact observed evidence |
| ---: | --- | --- | --- |
| 1 | Typing burst plus in-flight cancel | **IMPLEMENTED** | 10 `didChange` notifications (versions 2-11) sent without waiting; completion request `id=2` cancelled by `$/cancelRequest`; answer was `REQUEST_CANCELLED` (`-32800`, `request cancelled`); final version 11 diagnostics `[]`; hover `string`; 1 engine; clean shutdown `20.428 ms` |
| 2 | Resource delete/restore and watched files | **IMPLEMENTED** | `images/logo.png` deleted; `didChangeWatchedFiles` type `3`; diagnostic version `1` (no text change), code `style.missing-resource`, severity `2`, range `0:18-0:33`, exact message: local Image target `images/logo.png` does not exist; restored file + type `1`; diagnostics version `1` became `[]`; hover `string`; 1 engine |
| 3 | `app.json` overlay removal/restore | **IMPLEMENTED** | Unsaved `app.json` overlay `{"pages":[]}`; first hover after the ordered `didOpen` was empty (1 attempt); `didClose`; first hover was `string` (1 attempt); 1 engine |
| 4 | 40-request bounded queue burst | **IMPLEMENTED** | 40 hover requests sent before reading any response; 40/40 answered; 0 errors; 0 `server is busy`; 0 null results; post-burst hover `string`; 1 engine |
| 5 | External RSS baseline ratio | **BLOCKED** | No committed independent native server-tree baseline. Seam exists; canonical report records `summary.rss_metric=server_tree_rss_kib`, `summary.server_tree_rss_kib=158864`, ratio `null`, `ratio_max=1.15` |

The lifecycle part is unchanged: 10 rounds, engine `SIGKILL` every round, a
new engine PID, close/reopen every round, one engine at the end, and the
frozen 15% round-1-to-final server-tree RSS cap.

| Lifecycle metric | Release canonical |
| --- | ---: |
| Rounds | 10 |
| Baseline (round 1) RSS | `158,660 KiB` |
| Final RSS | `158,864 KiB` |
| Growth | `+0.129%` (cap `15%`) |
| Threads / processes at final round | `35 / 2` |
| Shutdown | `20.385 ms`, no detached worker, no sandbox, 0 processes left |
| Engine processes at end | 1 |

Raw report: `/tmp/opencode/q2-bench/stability-scenario-release-final.json`
(not committed).

The exact default-profile command also passed:

```bash
cargo test -p wxml-lsp --test q2_stability -- --ignored --test-threads=1
```

Result: **1 passed / 0 failed / 9 filtered out**, `38.82 s`; baseline
`164,832 KiB`, final `164,896 KiB`, growth `+0.039%`, 4 implemented / 1
blocked. Raw report:
`/tmp/opencode/q2-bench/stability-scenario-debug-verify.json`.

## 3. Scenario 5: seam and what is still missing

The seam is fail-closed:

- `WXML_NATIVE_STABILITY_BASELINE=<json>` points at an independent native
  stability report.
- The report must carry `summary.rss_metric = "server_tree_rss_kib"` and
  `summary.server_tree_rss_kib`; a report without that marker is rejected
  rather than compared with an incomparable metric.
- `WXML_NATIVE_STABILITY_REQUIRE_BASELINE=1` (or the legacy
  `WXML_STABILITY_REQUIRE_BASELINE=1`) makes a missing baseline a threshold
  failure.
- When supplied, the final server-tree RSS ratio must be `<= 1.15` — the same
  ratio cap as the retired gate, not a relaxed one.

What is missing: a committed native server-tree baseline artifact from a
dedicated independent run. The retired `WXML_STABILITY_RSS_BASELINE` artifact
is deliberately not reused: it mixed the Rust parent process with a Node
child, and its `summary.final_rss_kib` is not server-tree RSS. The ledger still
records the four now-covered scenarios (plus the external baseline) as
`BLOCKED`; updating the ledger is out of scope for this test/evidence slice.

Seam probe (not the canonical result): a previous native report
(`stability-scenario-final.json`, final `152,956 KiB`) was passed as
`WXML_NATIVE_STABILITY_BASELINE`; the run reached 5 implemented / 0 blocked
with `153,848 / 152,956 = 1.006x` (cap `1.15x`). This proves the comparison
path and the fail-closed metric guard; it does not create a committed
baseline and is not claimed as an unblocked gate result.

## 4. Fail-closed evidence

| Invocation | Result | Exit |
| --- | --- | ---: |
| ignored gate without `WXML_TEST_NATIVE=1` | panics at `NativeGate::require`: `WXML_TEST_NATIVE=1 is not set; the native gate job must request it explicitly` | 101 |
| required mode with nonexistent `TSGO_BIN` | panics at `NativeGate::require`: `native package for G-P5-STABILITY-01 is missing: TSGO_BIN points at ... which does not exist ...` | 101 |
| `WXML_NATIVE_STABILITY_REQUIRE_BASELINE=1` with no baseline | runs the lifecycle and four scenarios, then fails thresholds: `WXML_NATIVE_STABILITY_REQUIRE_BASELINE=1 but no native server-tree RSS baseline is available ...` | 101 |
| retired Node-style baseline without `summary.rss_metric` | panics after the lifecycle: `RSS baseline ... is not a native server-tree report; refusing to compare incomparable metrics` | 101 |

A missing engine or package never reaches the scenario code and never becomes
a skip in required mode. The two baseline probes show that the fifth seam is
also fail-closed rather than a quiet skip.

## 5. Verification summary

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo check -p wxml-lsp --all-targets` | pass |
| `cargo clippy -p wxml-lsp --all-targets -- -D warnings` | pass, no issues |
| `cargo test -p wxml-lsp --test q2_stability -- --ignored --test-threads=1` | 1 passed, 0 failed, 38.82 s |
| `cargo test -p wxml-lsp --release --test q2_stability -- --ignored --test-threads=1` | 1 passed, 0 failed, 6.16 s |
| `cargo test -p wxml-lsp --test native_parity native_gates::` | 12 passed, 0 failed (ignored-gate count and retarget checks unchanged) |

## 6. Explicit non-claims

- Scenario 1 proves the server cancels a queued/in-flight *request* and the
  final document state is clean. It does **not** prove that a running native
  TypeScript engine call is interrupted; the scheduler still cannot interrupt
  a synchronous backend call. No engine cancellation latency is claimed.
- Scenario 4 proves 40 concurrent hovers are admitted and answered. It does
  not probe the 128-slot admission boundary or claim a general queue-capacity
  proof.
- Scenario 3 uses the ordered JSON-RPC stream as the removal/restore barrier:
  the hover request is processed after `didOpen`/`didClose` on the same stdio
  stream. It does not additionally assert a particular diagnostics publish
  for the overlay.
- Scenario 5 remains BLOCKED in the canonical run. The seam probe is not a
  committed baseline and does not claim a Node-to-native RSS regression result.
- The 10-round lifecycle assertions and the 15% RSS threshold are unchanged.
  Scenario sessions run after the lifecycle measurement, each with a fresh
  server and engine, and cannot change the measured baseline.
- The lifecycle session keeps the existing `0 ms` debounce; the scenario
  sessions pin the retired gate's `120 ms` debounce through initialization
  options. Both values are recorded in the report.
- No Node process, Node shim, external sampler, production default change or
  ledger update is part of this slice.

## 7. Files

- `crates/wxml-lsp/tests/q2_stability.rs`
- `crates/wxml-lsp/tests/q2_stability_support/mod.rs`
- `docs/evidence/2026-11-20-tsgo-stability-scenarios.md` (this file)
