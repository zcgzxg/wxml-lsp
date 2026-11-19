# P6-2 third native gate wave: completion routing, Behavior and guard ranges (2026-11-20)

## 0. Identity and boundary

- Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD when the evidence was recorded: `814ca5e8de1a3845b5890235a6dd51b1709ae6de`
  (`feat(tsgo): route script diagnostics and expression completion`)
- Date: 2026-11-20
- Scope: `crates/wxml-lsp/tests/native_parity/{native_engine_real,native_gates}.rs`,
  `docs/plans/tsgo-test-ledger.json` (gate status and gate evidence only), this
  evidence document and the plan status. No production code, Cargo file, Node
  path, default backend or `#[ignore]` attribute is changed. Nothing is
  committed.
- Case ledger is unchanged: **36 `green` / 267 `planned` / 36
  `retired-with-decision`**. This record only moves gate status.

**Result: 16 `gate_green` / 7 `required_not_migrated` / 23 total.** Three gates
were promoted with real native stdio tests: `G-P5-E2E-03/11/16`. This is an
opt-in required-native-job result (`--ignored`), not a default-CI pass, not
`mandatory_green`, and not a P6/G5/G6/F3 closure.

---

## 1. Why these three gates became writable

Three committed slices landed after the 2026-11-19 typed-asset promotion:

| Commit | Behavior |
| --- | --- |
| `393ff0d` | `optionalTypes` compose into union hover facts; static Behavior properties join the shadow; guard wrappers map back to exact original expression mounts. |
| `0ca4375` | Local missing event-handler diagnostics from Rust script facts. |
| `647dd8d` | Local event-handler definition from Rust script facts. |
| `814ca5e` | Root (`{{  }}`) and trailing-dot (`{{ user. }}`) expression completion; script diagnostics routing. |

That is enough for these three gates without a production edit:

| Gate | Native replacement | What the native test proves |
| --- | --- | --- |
| `G-P5-E2E-03` | `native_expression_and_event_completion_smoke` | Member completion (`user.na` → `name`), event completion (`onT` → `onTap`) with a `textEdit` that starts on the whole handler token, root completion (`{{  }}` → `user`) and trailing-dot completion (`{{ user. }}` → `name`). |
| `G-P5-E2E-11` | `native_component_behavior_data_and_properties` | An inline `Behavior` contributes `sharedCount: number` with its `共享计数` JSDoc and a definition landing exactly on `sharedCount` in `card.ts`; `optionalTypes: [Number]` renders `string \| number`; the clean component publishes `[]`. |
| `G-P5-E2E-16` | `native_ac006_conditional_union_narrowing` | The `conditional-union` fixture keeps three exact hovers (`state.user.name` string + doc, `state.code` number + doc, guarded `maybeUser.name` string + doc), exactly two errors at the exact `age` and `maybeUser` tokens, and no degradation warning (`Supported`). |

The `G-P5-E2E-16` assertion is the one the 2026-11-19 record listed as blocked:
the `maybeUser` error is now mapped to the original token instead of being
cropped at generated shadow text, so the status stays `Supported`.

---

## 2. Required command and result

Environment:

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE \
  WXML_TEST_NATIVE=1 \
  WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
  TSGO_TYPINGS_ROOT=/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/miniprogram-api-typings \
  cargo test -p wxml-lsp --test native_parity native_engine_real \
    -- --ignored --test-threads=1
```

Result: **16 passed / 0 failed / 36 filtered**, exit 0, 46.78 s. The sixteen
tests are the three new gates plus the thirteen 2026-11-19 gates.

Pinned identity:

| Field | Value |
| --- | --- |
| Engine | TypeScript `7.1.0-beta`, `@typescript/typescript-linux-x64` |
| Source commit | `3e9236361fa97a9e6c685f4833ac457ae2d8c50a` |
| AST protocol | `9` |
| `lib/tsc` SHA-256 | `a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70` |
| `lib/tsc` size | `24,522,912` bytes |
| Standard library | `engine/lib` (`116` `lib*.d.ts` copied into the disposable package) |
| Typings payload | `miniprogram-api-typings` `5.3.1` at `/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/miniprogram-api-typings` (26 files, 20 `.d.ts` mounted) |

Fail-closed RED run (same command without `WXML_TEST_NATIVE=1`):

```bash
env -u WXML_TYPE_BACKEND -u WXML_BACKEND_DIR -u WXML_NODE -u WXML_TEST_NATIVE \
  WXML_REQUIRE_REAL_ENGINE=1 \
  TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
  TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
  TSGO_TYPINGS_ROOT=/home/zxg/code/rust/wxml-lsp/backends/typescript/node_modules/miniprogram-api-typings \
  cargo test -p wxml-lsp --test native_parity native_engine_real \
    -- --ignored --test-threads=1
```

Result: **0 passed / 16 failed / 36 filtered**, exit 101. Every gate panics with
`WXML_TEST_NATIVE=1 is not set; the native gate job must request it explicitly`.
A missing opt-in is a failure, never a skip.

Discovery run before the promotion (18 ignored candidates: the 13 committed
gates plus five candidate replacements for `E2E-03/11/16/20/21`): **16 passed /
2 failed / 36 filtered**. `E2E-03/11/16` passed on the first run; the two
failures are recorded in §4, and their candidate tests were removed rather than
weakened.

---

## 3. Ledger and validator changes

- Three `ignored_rust_tests[]` rows move from `required_not_migrated` to
  `gate_green` and gain `replacement_test`, `replacement_kind`,
  `native_prerequisite`, `removal_condition` and `gate_evidence`.
- `native_gates.rs` freezes the new sets and counts (**16 `gate_green` / 7
  `required_not_migrated` / 23**), links every green gate to exactly one
  `native_engine_real` replacement, and scans the new `#[ignore]` tests.
- Validator result: `cargo test -p wxml-lsp --test native_parity` is
  **36 passed / 0 failed / 16 ignored** in the default job.
- `MIGRATED_CASES`, partition counts and all `cases[]` statuses are untouched.
  The case ledger stays **36 / 267 / 36**.

## 4. Remaining seven gates and exact blockers

| Gate | Exact blocker |
| --- | --- |
| `G-P5-PERF-01` | No native release baseline exists. The old test constructs `NodeBackend`/`NodeTransport`; there is no reproducible native p50/p95/p99, CPU/RSS or growth dataset to freeze. |
| `G-P5-STABILITY-01` | No native restart/epoch accounting or RSS baseline; the old test uses the Node backend and `/proc`. |
| `G-P5-E2E-01` | Non-owner script overlay propagation needs native restart/epoch replay; the adapter has one lazy session and drops it on invalidation but does not replay a new epoch. |
| `G-P5-E2E-02` | Event diagnostics, definitions and completion are native (re-probed here), but **event-handler hover is still absent**: hovering a static `bindtap="onShorthandTap"` value returns LSP `result: null`, and `metadata::token_in_tag` deliberately leaves event values on the (empty) backend path. The old gate requires hover contents plus an exact `onTap` range. |
| `G-P5-E2E-05` | `Backend::symbols()` still returns `SymbolResult::unsupported`; `DataPath`/`EventHandler` symbol facts are not wired. |
| `G-P5-E2E-20` | Component property binding diagnostics are not covered. Opening the `component-bindings` fixture publishes only the local `Event handler "missingHandler" ...` error; `title="{{ total }}"` produces **no** type diagnostic, although both hover queries (`pageTitle` = `string` + `页面标题`, `total` = `number` + `卡片总数`) and the `onCardTap` definition pass. The adapter capability still documents component targets as uncovered. |
| `G-P5-E2E-21` | All diagnostic ranges, definitions and completion assertions pass, but the non-literal page status class differs: the native payload for `pages/nonliteral/nonliteral.wxml` is `type checking is unavailable: type backend unavailable` (`prepare()` fails, merged status `Unavailable`), while the frozen gate requires `CapabilityStatus::Unsupported`. `pages/multi/multi.wxml` matches (no fabricated error, `Partial`). The widget page adds its pre-existing `registration has no data member` `Partial` warning, which the old gate does not assert. |

These rows carry no `replacement_test`, no `replacement_kind` and no
`gate_evidence`; the validator rejects a half-written replacement. The
`E2E-20`/`E2E-21` candidate tests were removed after the discovery run so the
frozen ignored-gate inventory only contains green-linked replacements.

## 5. Verification

| Command | Result |
| --- | --- |
| `cargo test -p wxml-lsp --test native_parity` | **36 passed / 0 failed / 16 ignored** |
| 16-gate required native run (§2) | **16 passed / 0 failed / 36 filtered** |
| Fail-closed RED run (§2) | **0 passed / 16 failed / 36 filtered** |
| `rustfmt --check` on the two changed test files | exit 0 |
| `cargo clippy -p wxml-lsp --test native_parity -- -D warnings` | exit 0 |
| `git diff --check` | exit 0 |

## 6. Limitations and non-claims

1. **Not `mandatory_green`.** All sixteen tests stay `#[ignore]`; the default
   job reports them ignored. Only the opt-in `--ignored` job runs them.
2. **Not default CI, not the default backend.** `--stdio` without
   `--backend native` still uses Node. Node is not deleted.
3. **No case promotion.** The case ledger stays 36 / 267 / 36. A green gate does
   not make its related `TS-*` cases green.
4. **P6/G5/G6/F3 remain open.** No perf, stability, restart/epoch, native symbol,
   event-hover or component-binding-diagnostic claim is made.
5. **Linux x86-64 only.** The native path rejects Windows drive/UNC and macOS is
   untested.
6. **Uncommitted slice.** The run happened at `814ca5e` with this test/ledger
   slice uncommitted; the required job copies `CARGO_BIN_EXE_wxml-lsp` into a
   disposable package with the pinned engine and assets. Raw stdout is not
   committed (evidence level E0).
