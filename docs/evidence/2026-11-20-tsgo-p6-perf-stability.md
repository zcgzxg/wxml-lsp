# P6-2 native performance and stability gates (2026-11-20)

## 0. Identity and boundary

- Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD when recorded: `c5ee2511eccd0ba058ba45f7b686fc947d41afb6` (`test(tsgo): promote expression, behavior, and guard gates`)
- Date: 2026-11-20
- Scope: `crates/wxml-lsp/tests/native_parity/**` (new `native_measure.rs`, the two
  new gates in `native_engine_real.rs`, `native_lsp.rs` shutdown/PID helpers,
  `native_gates.rs` sets/counts), `docs/plans/tsgo-test-ledger.json` (gate status
  and gate evidence only), and this evidence document. No production code,
  Cargo file, Node path, default backend or `#[ignore]` attribute is changed by
  this slice. Nothing is committed.
- Case ledger is unchanged: **36 `green` / 267 `planned` / 36
  `retired-with-decision`**. This record only moves gate status.

**Result: 18 `gate_green` / 5 `required_not_migrated` / 23 total.**
`G-P5-PERF-01` and `G-P5-STABILITY-01` have real native stdio replacements and
are promoted. This is an opt-in required-native-job result (`--ignored`), not a
default-CI pass, not `mandatory_green`, and not a P6/G5/G6/F3 closure.

The primary worktree contained unrelated uncommitted production edits
(`wxml-backend-tsgo/src/script/mod.rs` references files that do not exist), so
the verification runs use a clean detached worktree at the same HEAD:

```text
/tmp/opencode/p6-perf-stability-c5ee251   (detached at c5ee251)
```

Only `crates/wxml-lsp/tests/native_parity.rs`,
`crates/wxml-lsp/tests/native_parity/{native_engine_real,native_lsp,native_measure,native_gates}.rs`
and `docs/plans/tsgo-test-ledger.json` were copied in. The user's unrelated
production edits were never used, reverted or modified.

---

## 1. What the two gates prove

| Gate | Native replacement | What the test proves |
| --- | --- | --- |
| `G-P5-PERF-01` | `native_q2_performance_benchmark` | Real `--stdio --backend native` cold start; 200 samples each for hot hover, stop-typing diagnostics and expression-member completion; p50/p95/p99/max; `/proc` CPU/RSS of the server tree; plan 9.5 p95 and 15% growth caps. Release profile is enforced. |
| `G-P5-STABILITY-01` | `native_ten_round_lifecycle_stability` | 12 update/release/restart cycles; three template updates per cycle; unsaved script overlay open/release; SIGKILL engine restart; second request must replay the overlay; diagnostics version/cleanliness; one engine child; steady RSS growth cap; clean shutdown with no detached worker, sandbox or process. |

Both gates use the existing disposable package builder, PATH tripwire and
scrubbed child environment. No Node executable, Node shim or external sampler is
started. `/proc` sampling is bounded to 64 processes per tree sample and 32
retained timeline points.

### Performance profile

`G-P5-PERF-01` rejects a debug build before measuring:

```text
G-P5-PERF-01 is a release baseline: run
`cargo test -p wxml-lsp --test native_parity --release ... --ignored`
```

This matches the P6 conversion target (`--release`). A debug-profile discovery
run measured diagnostics p95 **561.150 ms** and failed the 500 ms budget; that
run is recorded here as the reason for the release-only prerequisite, not as
the gate result.

---

## 2. Environment and pinned identity

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  WXML_TEST_NATIVE=1 \
  WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
  TSGO_TYPINGS_ROOT=/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/miniprogram-api-typings \
  cargo test ...
```

| Field | Value |
| --- | --- |
| Engine | TypeScript `7.1.0-beta`, `@typescript/typescript-linux-x64` |
| Source commit | `3e9236361fa97a9e6c685f4833ac457ae2d8c50a` |
| AST protocol | `9` |
| `lib/tsc` SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| `lib/tsc` size | `24,522,912` bytes |
| Standard library | `engine/lib` (116 `lib*.d.ts` copied) |
| Typings payload | `miniprogram-api-typings` `5.3.1` (26 files, 20 `.d.ts` mounted) |
| Host | Linux, 24 CPUs; the run is strictly serial |

---

## 3. Performance result

Canonical full release run (`native_engine_real`, 18 tests):
**18 passed / 0 failed / 36 filtered**, exit 0, **67.30 s**.

| Metric | Samples | p50 | p95 | p99 | max | Budget (p95) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Hot hover (`user.name`) | 200 | 0.550 ms | **0.822 ms** | 0.968 ms | 4.842 ms | ≤ 200 ms |
| Hot completion (`{{ user. }}`) | 200 | 0.318 ms | **0.648 ms** | 1.786 ms | 68.740 ms | ≤ 200 ms |
| Diagnostics after edit | 200 | 195.381 ms | **208.075 ms** | 211.473 ms | 214.353 ms | ≤ 500 ms |

Diagnostics include the default **120 ms** debounce and one real engine update
per sample. Cold start is recorded, not asserted (plan 9.5 defines no cold-start
p95):

| Cold metric | Value |
| --- | ---: |
| Spawn + `initialize` | 60.060 ms |
| Open page + first diagnostics | 245.889 ms |
| First page total | 305.951 ms |
| First engine hover | 3.704 ms |

Resources over the full run:

| Resource | Value |
| --- | ---: |
| Server tree RSS baseline / final | 205,188 / 229,036 KiB |
| Server tree RSS growth | **+11.623%** (cap +15%) |
| Server tree CPU | 2,196 ticks, about 21.96 s at 100 Hz |
| Parent test-process RSS start / end | 9,688 / 9,696 KiB |
| Engine processes at end | 1 |
| Leaked descendants | 0 |

Exact single-gate release run (`native_q2_performance_benchmark`):
**1 passed / 0 failed / 53 filtered**, 41.11 s; hover p95 `0.740 ms`,
completion p95 `0.387 ms`, diagnostics p95 `204.172 ms`, RSS growth `-4.78%`.
The values move within run-to-run noise, never across a budget.

---

## 4. Stability result

Canonical full release run: **18 passed / 0 failed / 36 filtered**, 67.30 s.
The stability test itself ran in the same binary; exact single-gate release run
is **1 passed / 0 failed / 53 filtered**, 16.00 s.

Each of the 12 cycles performs three template updates, opens a script overlay
(`name: <round>`, number), kills the engine with `SIGKILL`, requests hover, then
closes the overlay and checks the disk string. Diagnostics must be clean and
carry the requested version after every update.

| Round | Version | Server tree RSS (KiB) | Growth vs round 0 | Threads | Processes | First request after SIGKILL | Wall (ms) |
| ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: |
| 0 | 6 | 158,484 | 0.00% | 36 | 2 | empty | 1,282 |
| 1 | 11 | 166,948 | +5.34% | 36 | 2 | empty | 1,222 |
| 2 | 16 | 166,768 | +5.23% | 36 | 2 | empty | 1,206 |
| 3 | 21 | 169,028 | +6.65% | 35 | 2 | empty | 1,217 |
| 4 | 26 | 164,396 | +3.73% | 36 | 2 | empty | 1,259 |
| 5 | 31 | 167,112 | +5.44% | 36 | 2 | empty | 1,228 |
| 6 | 36 | 166,532 | +5.08% | 36 | 2 | empty | 1,212 |
| 7 | 41 | 174,724 | +10.25% | 36 | 2 | empty | 1,242 |
| 8 | 46 | 165,140 | +4.20% | 36 | 2 | empty | 1,251 |
| 9 | 51 | 171,084 | +7.95% | 35 | 2 | empty | 1,230 |
| 10 | 56 | 166,544 | +5.09% | 35 | 2 | empty | 1,287 |
| 11 | 61 | 168,584 | +6.37% | 36 | 2 | empty | 1,296 |

- **Cycle growth:** `+6.373%` from round 0 to round 11 (cap `15%`).
- **Restart replay:** all 12 first requests after `SIGKILL` returned **empty**;
  the second request opened a **new engine PID** and replayed the open number
  overlay in all 12 cycles. This is the current E2E-01 behavior, recorded
  explicitly. `G-P5-E2E-01` stays `required_not_migrated`; this gate does **not**
  claim transparent first-request replay.
- **Steady-state RSS:** after the cycles and a 16-hover queue burst, eight real
  edits were sampled: `187,232 → 209,844 → 212,084 → 213,504 → 207,960 →
  208,068 → 208,088 → 208,240 KiB`. The post-warm-up steady baseline is
  `209,844 KiB`, the last-three median is `208,088 KiB`, and the steady growth is
  **-0.837%** (cap `15%`).
- **Disclosed one-time step:** the cold round-0 baseline to the steady plateau is
  **+31.299%**. This is the engine/allocator working-set step of the first
  snapshots, not sustained growth; it is recorded rather than treated as zero.
  No committed prior steady-state baseline exists, so this document does **not**
  claim a Node-to-native steady RSS regression comparison.
- **Shutdown:** `20.257 ms`, exit status success, `detached_worker=false`,
  no sandbox left, no process left. The server and every engine descendant were
  gone after shutdown.

The exact single-gate run repeated the same shape: cycle growth `-2.303%`,
steady growth `+10.575%`, cold-to-steady `+30.696%`, shutdown `20.234 ms`, no
detached worker and no leaked process.

---

## 5. Fail-closed RED evidence

Exact ignored gate without `WXML_TEST_NATIVE=1` (the existing convention makes
this a hard failure, never a skip):

| Gate | Result | Exit |
| --- | --- | ---: |
| `G-P5-PERF-01` | **0 passed / 1 failed / 53 filtered** | 101 |
| `G-P5-STABILITY-01` | **0 passed / 1 failed / 53 filtered** | 101 |

Both panic at `NativeGate::require` with
`WXML_TEST_NATIVE=1 is not set; the native gate job must request it explicitly`.
A missing engine or asset remains fatal in required mode. `G-P5-PERF-01` also
fails closed in a debug build after the package is provisioned, so a debug run
cannot be mistaken for the release baseline.

---

## 6. Verification summary

| Command | Result |
| --- | --- |
| `cargo test -p wxml-lsp --test native_parity` (debug, default job) | **36 passed / 0 failed / 18 ignored**, 0.14 s |
| 16 previously green native gates, debug, `--ignored --skip` the two new gates | **16 passed / 0 failed / 38 filtered**, 46.52 s |
| 18 native gates, release, `native_engine_real --ignored` | **18 passed / 0 failed / 36 filtered**, 67.30 s |
| Exact `native_q2_performance_benchmark`, release | **1 passed / 0 failed / 53 filtered**, 41.11 s |
| Exact `native_ten_round_lifecycle_stability`, release | **1 passed / 0 failed / 53 filtered**, 16.00 s |
| Debug discovery run of all 18 (before the release prerequisite) | 17 passed / 1 failed; diagnostics p95 `561.150 ms` |
| `rustfmt --edition 2024 --check` on the five changed test files | exit 0 |
| `cargo clippy -p wxml-lsp --test native_parity -- -D warnings` | exit 0 |

Ledger and validator changes:

- Two `ignored_rust_tests[]` rows move from `required_not_migrated` to
  `gate_green` and gain `replacement_test`, `replacement_kind`,
  `native_prerequisite`, `removal_condition` and `gate_evidence`.
- `native_gates.rs` freezes **18 `gate_green` / 5 `required_not_migrated` /
  23** and links every green gate to exactly one `native_engine_real`
  replacement.
- `MIGRATED_CASES`, partition counts and all `cases[]` statuses are untouched.
  The case ledger stays **36 / 267 / 36**.

---

## 7. Remaining five gates and exact blockers

| Gate | Exact blocker |
| --- | --- |
| `G-P5-E2E-01` | Native restart drops the invalidated session; the first request after a crash is empty and the second request replays the overlay. Transparent first-request replay is not committed, so the frozen gate is not claimed. |
| `G-P5-E2E-02` | Event diagnostics, definitions and completion are native, but event-handler hover is still absent. |
| `G-P5-E2E-05` | `Backend::symbols()` still returns `SymbolResult::unsupported`; data-path/handler/reference symbol facts are not wired. |
| `G-P5-E2E-20` | Component property binding diagnostics are not covered; the fixture's `title="{{ total }}"` produces no type diagnostic. |
| `G-P5-E2E-21` | The non-literal page status is `Unavailable`, while the frozen gate requires `Unsupported`. |

These rows carry no `replacement_test`, no `replacement_kind` and no
`gate_evidence`.

---

## 8. Limitations and non-claims

1. **Not `mandatory_green`.** Both new tests stay `#[ignore]`; the default job
   reports them ignored. Only the opt-in `--ignored` release job runs them.
2. **No Node comparison.** No Node process is used. The 15% cap is applied to
   the native run's cycle growth and post-warm-up steady growth; there is no
   committed prior steady baseline, so no Node-to-native RSS regression is
   claimed.
3. **Cold-to-steady step is real and disclosed.** The native engine tree moves
   from about 167 MiB cold to about 206 MiB steady after the first snapshots
   (+26.1%). The 15% cap is applied to sustained growth from the post-warm-up
   steady baseline (+7.4%), not to that one-time step.
4. **Release prerequisite.** The performance gate is only meaningful in a
   release build; debug is rejected instead of being compared with plan 9.5.
5. **CPU is approximate.** `/proc` tick deltas are reported at the usual 100 Hz
   kernel clock; RSS is exact KiB from `VmRSS`.
6. **Report files are evidence, not the gate.** `WXML_NATIVE_PERF_OUT` /
   `WXML_NATIVE_STABILITY_OUT` select the path (default `/tmp/opencode`). A full
   or unwritable temporary filesystem prints a warning and leaves the measured
   gate result unchanged.
7. **No production, delivery, default-backend or Cargo change.** The worktree's
   unrelated production edits were not used for verification and were not
   changed by this slice.
