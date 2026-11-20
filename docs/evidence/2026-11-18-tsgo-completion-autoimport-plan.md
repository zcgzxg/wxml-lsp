# F1 completion auto-import / per-entry documentation audit and plan

- Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD at audit start: `a17794a4519ab5b83baf5d5b9d677ca5c129a704` (`fix(delivery): close native rollback recovery gaps`).
- HEAD observed at write time: `5541ee019d7d25e03b1e4a15d3f72d12470322c0` (`fix(delivery): harden native package build`). The only intervening commit touches `crates/wxml-delivery/**`; all completion/contract files cited here are unchanged between the two commits (`git diff --stat a17794a..HEAD -- crates/wxml-backend-tsgo/src/query/completion.rs crates/wxml-backend-tsgo/src/native/{wire,session,protocol}.rs crates/wxml-lsp/src/backend_tsgo crates/wxml-types/src/backend.rs docs/reference/tsgo-native-contract.md` is empty).
- Audit date: 2026-11-19 (filename keeps the requested 2026-11-18 label)
- Scope: pinned native completion path, native wire/session, LSP adapter, F1 contract, and the already-downloaded 7.1.0-beta artifact/source under `/tmp/opencode/tsgo-spike`.
- Boundary: read-only. No production, test, Cargo, contract, delivery, backend-default or Node change. The only new file is this one. No commit. **F3 is not closed or touched.**
- Worktree note: at audit time the worktree had unrelated uncommitted changes in `crates/wxml-backend-tsgo/src/{input.rs,native/session.rs,native/vfs.rs}` plus untracked delivery/type-asset tests. The completion-related `session.rs` lines cited below are outside those diffs; all other cited Rust files are clean at the audited commits.

## 0. Verdict

1. **F1 completion auto-import is NOT implemented.** The adapter creates snapshots without `prepareAutoImports`, sends `includeSymbol:false` unconditionally, does not deserialize the optional `symbol`, and has no per-entry auto-import marker.
2. **Per-entry documentation is BLOCKED by the pinned API.** `getCompletionEntryDetails` does not exist in the 7.1.0-beta generated API, and `CompletionEntryResponse` has no `documentation`, `displayParts`, `data`, `origin`, `source`, or `hasAction`. Available symbol documentation calls (`getDocumentationComment`/`getJsDocTags`) are not completion-entry details and must not be used to synthesize them.
3. **The existing session can be prepared without reopening the process, but not without a new snapshot.** `updateSnapshot.changes.prepareAutoImports` is valid and is wired by the engine source. The current Rust session has no such method: `update_project` only accepts an advancing `AnalysisInput` and a full VFS. A new prepare-only method would rotate the engine snapshot id and release the old snapshot.
4. **`includeSymbol:false` has a server-side lazy path.** When `getCompletionsAtPosition` reports `ErrNeedsAutoImports`, the engine clones a prepared snapshot for that request and retries. That path is symbol-less and not a stable session state; `includeSymbol:true` unprepared fails closed.
5. **Even if an auto-import entry is returned, it is not an actionable LSP auto-import today.** The C0 `CompletionItem` has no additional-text-edit/import-edit field, and the native response has no `data`/`textEdit`/import specifier. The current adapter also only accepts `CompletionKind::ExpressionMember`; auto-import completion is an identifier/global completion feature, not a `u.` member completion.
6. **No per-entry origin marker exists in the pinned DTO.** `name`, `sortText`, `insertText`, `filterText`, `detail`, `labelDetails`, and `symbol` are not an auto-import boolean. `symbol` is present for ordinary imported members too. Marking an entry as an auto-import without a new explicit API field would be a fabrication.
7. **Therefore F1-21 remains open/blocked, B-07 remains open, and non-empty completion remains `Partial` for documentation even if auto-import plumbing is added.** No F3 claim is made.

---

## 1. Pinned 7.1.0-beta API facts

Downloaded source/artifact used (no binary download performed):
- `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts`
- `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_async_types.ts`
- `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_async_api.ts`
- `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go`
- `/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc` (already downloaded ELF; not executed for this audit)

### 1.1 Snapshot creation flag exists

`createSnapshot` takes a top-level `prepareAutoImports?: DocumentIdentifier`:

- `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:300-307`
  - line 304-305: `PrepareAutoImports identifies the file whose auto-import indexes should be ready in the new snapshot.` / `prepareAutoImports?: DocumentIdentifier`.
- The engine maps it to `apiRequest.PrepareAutoImports` and validates it:
  - `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:1423-1425`
  - validation at `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:1659-1672` (`IsPreparedForImportingFile`; failure is a client error, not a silent no-op).
- `updateSnapshot` accepts the same create params under `changes`:
  - `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:331-334`
  - `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:1476-1478`
  - validation at `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:1502-1505`.

### 1.2 Completion request flag exists

`getCompletionsAtPosition` takes `includeSymbol?: boolean`:

- `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:985-993` (line 992).
- `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_async_types.ts:387-392` (`CompletionOptions`; only `triggerCharacter` and `includeSymbol`).
- `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_async_api.ts:1654-1662` forwards `includeSymbol`.

### 1.3 Completion entry shape has no origin/docs/data

`CompletionEntryResponse` is exactly:

- `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:1677-1687`
  - `name`, `kind?`, `sortText?`, `insertText?`, `filterText?`, `detail?`, `labelDetails?`, `symbol?`.
- The async `CompletionEntry` has the same fields; `symbol` is only populated when `includeSymbol:true`:
  - `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_async_types.ts:394-405` (line 403-404).
- `CompletionInfoResponse` has only `isIncomplete` and `entries`:
  - `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:995-999`.
- `SymbolResponse` contains symbol identity/declarations, not completion docs or auto-import origin:
  - `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:270-279`.
- The session copies only `Label`, `SortText`, `InsertText`, `FilterText`, `Detail`, `Kind`, `LabelDetails`, `Symbol`:
  - `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:5537-5563`.

### 1.4 `getCompletionEntryDetails` does not exist

- The generated API method list contains only `getCompletionsAtPosition`:
  - `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:186`.
- A literal search in the three downloaded generated API files found **zero** occurrences of `getCompletionEntryDetails`.
- The repository already treats it as a legacy/unreachable method:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/protocol.rs:489-506` (line 498).
- There are no `documentation` or `displayParts` fields in the downloaded completion/entry interfaces:
  - `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_async_types.ts:394-405`
  - `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:1677-1687`

### 1.5 Auto-import engine behavior that matters

- `includeSymbol:true` + `ErrNeedsAutoImports` fails instead of lazily preparing:
  - `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:5507-5510` (`snapshot is not prepared for auto-imports`).
- `includeSymbol:false` + `ErrNeedsAutoImports` clones a prepared snapshot and retries:
  - `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:5511-5532`
  - `CloneSnapshotWithAutoImports` at line 5515; optional background adoption at line 5517.
- `includeSymbol:true` also opts into checker lifetime:
  - `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:5482-5484`.
- Existing member probe (`u.`) returned only `name`/`age` and never demonstrated an auto-import entry:
  - `/tmp/opencode/tsgo-spike/logs/exp06.stdout:31-32`
  - `/tmp/opencode/tsgo-spike/logs/exp06_openfiles_prep_inclTrue.json` (only `name`, `age` with symbols).
  This is consistent with auto-imports being an identifier/global completion feature, not a member completion. The new test must prove this rather than assume it.

---

## 2. Current implementation and exact drop points

### 2.1 Snapshot creation drops `prepareAutoImports`

- `SnapshotChanges` has no prepare field:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:78-99`
- `SnapshotChanges::to_value` emits only `openFiles`, `closeFiles`, `openProjects`, `closeProjects`, optional `fileSystem`:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:148-161`
- `create_snapshot_params` / `update_snapshot_params` only wrap that value:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:164-177`
- `open_project` builds changes from `create_changes` and calls `CreateSnapshot`:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:592-611`
- `create_changes` opens the project and every source, with no prepare path:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1672-1680`
- `update_project` builds a full VFS update and never sets a prepare path:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:694-709`
  - `UpdateSnapshot` call at lines 715-718.

Consequence: the Rust session cannot send `prepareAutoImports` at all, at create or update time.

### 2.2 Completion request drops `includeSymbol`

- `completion_params` hardcodes `includeSymbol: false`:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:231-250` (line 245).
- `NativeSession::completions` documents `includeSymbol:false` and calls that helper:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1553-1579`
- The LSP `QuerySession` seam has no options parameter:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:82-87`
  - production implementation at lines 174-180.
- The adapter calls `session.completions(script_path, position)`:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/completion.rs:63`.
- The startup probe also calls `completion_params` with the hardcoded false:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:571-582`.

Consequence: no `symbol` is requested; the engine cannot attach symbol evidence to entries.

### 2.3 Response `symbol` is dropped by the wire DTO

`CompletionEntryResponse` includes `symbol`, but `RawCompletionEntry` has only `name`, `kind`, `sortText`, optional `detail`, optional `labelDetails`:

- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:446-469`
- The pinned response shape is `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:1677-1687`.

Serde's default ignores unknown JSON fields, so an emitted `symbol` is silently discarded. This is a second, independent drop even if `includeSymbol:true` were sent.

### 2.4 Mapping drops origin and marks auto-imports unavailable globally

- `CompletionEntry` has no origin/auto-import field:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/completion.rs:88-102`
- `map_completions` maps only label/detail/kind/sortText:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/completion.rs:134-160`
- Every non-empty list gets both `DocumentationUnavailable` and `AutoImportsUnavailable`:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/completion.rs:175-178`
- The gaps/reasons are defined at:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/completion.rs:43-86`.
- The LSP adapter always sets `documentation: None` and uses `label` for insertion:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/completion.rs:104-126`.
- Capability text repeats the same limitation:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:458-462`.
- The LSP adapter only accepts `ExpressionMember`:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/completion.rs:36-40`.

### 2.5 C0 DTO cannot carry an import edit

`wxml_types::CompletionItem` has label/kind/detail/documentation/insert_text/insert_text_format/replace/sort_text/filter_text only:

- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs:260-281`

There is no `additional_text_edits`, `text_edit`, import specifier, or command. The native API also exposes no `textEdit`/`data` for completion. Therefore even a correctly detected auto-import label cannot add the required `import { X } from "./mod"` edit through the current C0 boundary. Any public DTO change requires integration-owner review:

- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:529`.

This is the main reason auto-import must remain BLOCKED for user-visible completion, not merely "flag not sent".

---

## 3. Can an existing session enable auto-import without reopening?

### 3.1 Engine level: yes, with `updateSnapshot`

`updateSnapshot` accepts `changes.prepareAutoImports` because `changes` is a `CreateSnapshotParams`:

- `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:331-334`
- mapping at `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:1476-1478`
- validation at `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:1502-1505`.

This keeps the same process and session; it creates a new engine snapshot id, and the old snapshot must be released. It is not a process reopen.

### 3.2 Current Rust session: no

- There is no prepare-only method. `update_project` requires an `AnalysisInput`, a matching project/root, and a strictly advancing revision:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:671-709`
  - revision guard at lines 688-691.
- It always rebuilds and sends a full VFS (`fileSystem.kind="full"`), not a prepare-only change:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:694-703`.
- `open_project` can only set changes once, and currently never sets prepare:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:592-611`.

So as-is, enabling auto-import requires **either** a new `NativeSession` (reopen) **or** a code change that issues a prepare-only `updateSnapshot`. The API does not require reopening; the current adapter does.

### 3.3 Lazy fallback is not a supported enablement path

With `includeSymbol:false`, the engine can automatically clone a prepared snapshot for a single request:

- `/tmp/opencode/tsgo-spike/upstream/tsc_internal_api_session.go:5507-5532`

This means the current adapter might already receive auto-import entries on some global/identifier completion, despite not setting `prepareAutoImports`. But:
- the clone is internal to the request and symbol-less;
- the session's own snapshot remains unprepared;
- `includeSymbol:true` still fails closed before the clone (`5508-5510`);
- the Rust DTO cannot mark which entries were auto-imports.

Do not flip the capability to `Supported` based on this path. A real-engine test must establish what the engine actually returns for the exact requested context.

### 3.4 Recommended prepare mechanics (if approved)

Add both an initial and an on-demand path, but only after contract freeze:

1. `SnapshotChanges::prepare_auto_imports(path)` writes `{"prepareAutoImports":{"uri":...}}`.
2. `NativeSession::open_project_with_auto_imports(input, prepare_path)` uses it in `create_changes`.
3. `NativeSession::prepare_auto_imports(path)` resolves the path through the VFS, sends `updateSnapshot` with only `prepareAutoImports`, commits the new epoch, releases the old snapshot, and enforces the live-snapshot cap.
4. Cache `(epoch.snapshot, prepare_path)` so a keystroke does not trigger a snapshot per completion.
5. If a prepare fails, return a typed error/`Partial`; never silently continue and claim auto-imports are available.

---

## 4. DTO/status impact: which entries are auto-imports?

### 4.1 There is no reliable per-entry signal

| Candidate signal | Present in pinned response? | Reliable auto-import origin? | Allowed use |
| --- | --- | --- | --- |
| `name` | Yes | No | label only |
| `sortText` | Yes | No; native ordering, not origin | preserve order/sort key only |
| `insertText` / `filterText` | Optional | No | insertion/filter text only |
| `detail` | Optional | No; signature/type text | display detail only |
| `labelDetails.description` | Optional | Not defined as origin; may be module text | display only; never documentation |
| `symbol` | Only with `includeSymbol:true` | No; ordinary imported members also have symbols | future declaration evidence only |
| `data` / `origin` / `source` / `hasAction` | **No** | Not available | BLOCKED |
| list-level `isIncomplete` | Yes | N/A | status only |

The response has no `data` field at all:
- `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:1677-1687`.

`symbol` can show a declaration file, but it cannot prove "not imported" without reading the import graph. That would violate the contract's no-self-parsing rule:
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:597` ("不得自行执行代码/解析项目文件补全").

### 4.2 Safe marking rule

- **Do not add `auto_import: bool` per entry in this slice.** There is no field to populate it from.
- Mark at the *result* level only:
  - `AutoImportsUnavailable`: auto-imports were not requested/prepared.
  - New internal `AutoImportOriginUnavailable`: prepared, but the pinned DTO has no per-entry origin marker.
- Keep `DocumentationUnavailable` for every non-empty list. Even with auto-imports prepared, non-empty results remain `Partial`.
- Preserve native order and non-empty `sortText`; do not reorder auto-import candidates by guessed origin.
- If a future pinned artifact adds `data`/origin/`isImportStatementCompletion`, then add a typed per-entry origin and only then publish it. Do not backfill from `sortText`, `symbol`, module path, or imports.

### 4.3 Status table

| Situation | Required status | Gaps |
| --- | --- | --- |
| Auto-import not prepared / request flag off; non-empty list | `Partial` | `DocumentationUnavailable`, `AutoImportsUnavailable` |
| Auto-import prepared; non-empty list | `Partial` | `DocumentationUnavailable`, `AutoImportOriginUnavailable` (new) |
| Auto-import prepared; explicit empty list, no truncation | `Supported` | none |
| Engine returns `null` | `Partial` | `NoResponse` |
| Engine marks list incomplete | `Partial` | `IncompleteList` (+ others as applicable) |
| 256-item cap | `Partial` | `Truncated` (+ others) |
| `includeSymbol:true` unprepared and auto-imports needed | Error, or `Unsupported`/`Partial` after deliberate mapping | **never** empty `Supported` |
| Entry returned but no import edit is representable | `Partial` (label may be shown only if clearly non-actionable) or `Unsupported` | do not pretend the import was inserted |

### 4.4 Current tests that freeze the old behavior

These must be updated deliberately when status semantics change, not silently:
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/completion.rs:313-327` expects both old gaps.
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/tests.rs:1008-1016` expects the old exact `Partial` reason.
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/tests.rs:1303-1313` expects the old truncated reason.
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/tests.rs:2757-2759` asserts the real-engine reason contains both `documentation` and `auto-import`.

---

## 5. Per-entry documentation: BLOCKED, do not synthesize

### 5.1 No completion-details API

- `getCompletionEntryDetails` is absent: `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:186` is the only completion method.
- `CompletionEntryResponse` has no docs/display parts: `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_proto.generated.ts:1677-1687`.
- `CompletionEntry` has no docs/display parts: `/tmp/opencode/tsgo-spike/upstream/packages_typescript_src_api_async_types.ts:394-405`.

### 5.2 Symbol docs are not completion-entry docs

The engine does expose:
- `getDocumentationComment` and `getJsDocTags` in the allowlist:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/protocol.rs:67-68`
- They are used by hover through a symbol reference:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:156-170`

But the contract explicitly forbids using them to fabricate completion details:
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:600` ("`getCompletionEntryDetails` 不存在 ... 不得用 `getTypeOfSymbol`/docs 拼出“详情”冒充原生详情").
- The completion matrix repeats the same rule: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:545`.
- `labelDetails.description` is explicitly not documentation:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs:429-444` (line 433-434).

### 5.3 Required outcome

- Leave `CompletionItem.documentation = None`.
- Leave `detail` as native signature/detail only; never turn it into docs.
- Keep `DocumentationUnavailable` and `Partial` for non-empty lists.
- Mark a future completion-details capability `BLOCKED` until the pinned API adds a real per-entry details method/field. Do not add a hover-to-completion bridge as a workaround.

---

## 6. Test plan (exact file targets)

### 6.1 Fake/wire tests for flags (no engine)

`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/wire.rs`:
- Extend `query_params_use_integers_and_document_identifiers` (`:510-572`) to assert:
  - `completion_params(..., include_symbol=true)` emits `"includeSymbol": true`; false remains false.
  - `SnapshotChanges::new().prepare_auto_imports("/work/main.ts").to_value()` emits `"prepareAutoImports":{"uri":"file:///work/main.ts"}`.
  - `update_snapshot_params(7, &changes)["changes"]["prepareAutoImports"]` is present.
  - An absent prepare path emits no key (do not send `null`).
- Extend the response parser tests (`:618-665`) to deserialize an entry with `"symbol": {...}` and prove `RawCompletionEntry` captures it if the DTO is extended; if not extended, assert it is intentionally ignored and no status depends on it.

`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/completion.rs`:
- Keep/extend `a_non_empty_list_reports_documentation_and_auto_import_gaps` (`:313-327`).
- Add a context-aware mapping test:
  - disabled -> `AutoImportsUnavailable`;
  - prepared -> no `AutoImportsUnavailable`, but `AutoImportOriginUnavailable`;
  - both -> `DocumentationUnavailable`;
  - empty prepared list -> `Supported` with no gaps.
- Add a test that no per-entry origin is invented from `name`/`sortText`/`symbol`.

### 6.2 Fake adapter tests for flags/status

`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/tests.rs`:
- Extend `FakeLog` (`:340-358`) and `FakeCompletionOutcome` (`:295-318`) to record/return `include_symbol`.
- Add `completion_requests_symbol_evidence_when_auto_imports_are_enabled`:
  - assert the adapter calls `completions(..., include_symbol=true)` only for the enabled path;
  - assert `documentation` is still `None`;
  - assert status contains the docs gap and not a false `Supported`.
- Add `completion_prepare_failure_never_returns_supported_empty`:
  - fake prepare failure -> `Partial`/error, no empty `Supported`.
- Add `completion_auto_import_origin_is_not_fabricated`:
  - a fake entry with a symbol and a module-like `labelDetails.description` still has no `auto_import` marker and no docs.
- Update the exact reason assertions at `:1008-1016` and `:1303-1313` only after the status contract is frozen.

### 6.3 Real-engine auto-import test

Preferred contract target (currently absent; F1-21's planned file):
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/f1_query.rs`
  - `completion_without_details_is_visibly_degraded` (F1-21).
  - `completion_auto_import_entry_requires_prepared_snapshot`.
  - `completion_auto_import_entry_reports_engine_origin_if_available` (only if a future API field exists; otherwise assert BLOCKED/no marker).

If the file is not created in this slice, extend:
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/real_engine.rs:112-126` pattern.

Fixture:
- `main.ts`: `const value: Us` (cursor after `Us`, no import).
- `model.ts`: `export interface User { name: string }`.
- Snapshot: `fileSystem.kind="full"` with both files, `openFiles` both, `prepareAutoImports` = `main.ts`.
- Request: `getCompletionsAtPosition` at the `Us` cursor, `includeSymbol:true`.
- Expected: an entry `User` from `model.ts`; capture `sortText`, `detail`, `labelDetails`, `symbol`; assert no `documentation`/`displayParts`/`data`.
- Negative control: same query on an unprepared snapshot with `includeSymbol:true` must return the engine "not prepared" error; the adapter must not catch it and silently retry without symbols or return empty `Supported`.
- Observation control: same query with `includeSymbol:false` on an unprepared snapshot; record whether the lazy clone returns `User`. Do not make the capability depend on this until the result is stable and tested.

### 6.4 Required-mode fail-closed

`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/real_engine.rs:112-126` is the reference pattern:
- `WXML_REQUIRE_REAL_ENGINE=1` + missing `TSGO_BIN` must panic.
- New auto-import test must use the same guard; no `return`/skip in required mode.
- `prepareAutoImports` rejection (file outside VFS, missing file, wrong project) must fail the request, not silently omit the field.
- `includeSymbol:true` unprepared must fail closed; map deliberately to `Unsupported`/`Partial` rather than the current generic `BackendError::Internal` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:552-581`).
- No Node fallback and no empty `Supported` on any failure.

### 6.5 Regression for existing `u.` completion

- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/tests.rs:2652-2731` (`optional_real_engine_completion_smoke`) must still return exactly `name`, `age` in native order at both `u.` and `u.na`; `documentation` remains `None`.
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/real_engine.rs:369-387` must still contain `name`/`age`.
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/type_assets.rs:587-601` (`wx.`) must remain non-empty and contain `getSystemInfoSync`.
- `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/tests/perf_budget.rs:467-503` completion workload must keep its budget; a prepare-per-keystroke design would fail this gate.
- Fake tests `completion_does_not_publish_an_empty_sort_text` (`:1036-1069`) and `completion_truncation_is_reported` (`:1262-1314`) must keep native order/`None` sort text and the 256 cap.

---

## 7. Security and cost

### 7.1 Request/snapshot size

- Adding `prepareAutoImports` adds one document identifier to a create/update request; negligible relative to the full VFS body.
- The full VFS remains bounded by:
  - `MAX_INPUT_FILES = 20_000` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/input.rs:31-32`);
  - `MAX_INPUT_BYTES = 48 MiB` (`:34-41`);
  - `MAX_TYPE_ASSET_FILES = 256` (`:92`);
  - `MAX_TYPE_ASSET_BYTES = 8 MiB` (`:97`).
- The snapshot request is preflighted against a budget before allocation:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1898-1913`.
- `prepareAutoImports` itself can make the engine build a project-wide export index; CPU/memory/latency are not bounded by the one path field. Bound them by the existing input/asset caps, `Deadline::snapshot` (`:608-612`, `:715-719`), and the live-snapshot cap. Do not add retries.
- `includeSymbol:true` increases the response: one symbol object per symbolic entry, up to `MAX_COMPLETION_ITEMS = 256` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs:184`). Symbols contain declaration handles and a file descriptor. Parse only fields needed; do not retain/publish full symbols. The current `RawCompletionEntry` ignores `symbol`, so simply flipping the flag adds wire bytes without benefit.

### 7.2 No host filesystem

- The contract requires `fileSystem.kind="full"` and forbids layering the host FS:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/docs/reference/tsgo-native-contract.md:301`.
- The VFS answers only from its virtual tables; missing paths get `{"kind":"missing"}` / `false` / empty entries, never a host read:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:336-389`.
- Before any snapshot the dispatcher uses typed fail-closed answers, never JSON-RPC errors as deny:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/vfs.rs:392-410`.
- The prepare path must be resolved through the VFS and rejected if outside the project root, like every other path (`NativeSession::file_context` at `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1682-1698`). Do not pass an unchecked host path.

### 7.3 No Node, no external code

- The supervisor builds the frozen argv and spawns the pinned ELF directly, with no shell, no Node, and no `--runExternalCode`:
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/supervisor.rs:1-8`
  - `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/config.rs:361-374`
  - `Command::new` with cleared env and piped stdio at `supervisor.rs:506-519`.
- Auto-import must not introduce a JS/Node helper, a module resolver process, or a second engine. Everything must go through the existing JSON-RPC session and VFS.

### 7.4 Data exposure

- `symbol.reference.file.fileName` contains an absolute engine path. Do not publish it in LSP items, statuses, or logs.
- If a future UI shows a module source, convert to a project-relative display string; never expose the host root.
- Do not put module text into `documentation`; that would both fabricate docs and leak paths.

---

## 8. Ordered slice plan

### Slice 0 - product/contract decision (BLOCKS the rest)

- Decide whether F1 completion will add a non-member/identifier completion context that can actually yield auto-imports. The current C0 adapter only accepts `ExpressionMember` (`backend_tsgo/completion.rs:36-40`), and the `u.` probe returns only members. Without this decision, prepare/flag plumbing produces no user-visible auto-import.
- Freeze B-07 in `docs/reference/tsgo-native-contract.md:793`: exact params, visible degradation, and whether auto-import is in scope for F1.
- Do not change C0 `CompletionItem` without integration-owner review (`tsgo-native-contract.md:529`).

### Slice 1 - wire/API flags only

- Add `SnapshotChanges::prepare_auto_imports`, `completion_params(..., include_symbol)`, and optional `RawCompletionEntry.symbol`.
- Add exact JSON/deserialization tests in `crates/wxml-backend-tsgo/src/native/wire.rs`.
- No behavior change; no status change; no capability claim.

### Slice 2 - result status/context

- Add a completion context to `map_completions` so it knows whether auto-imports were requested/prepared.
- Add `AutoImportOriginUnavailable`; keep `DocumentationUnavailable`.
- Update fake tests in `crates/wxml-backend-tsgo/src/query/completion.rs` and `crates/wxml-lsp/src/backend_tsgo/tests.rs`.
- Still no auto-import entry classification.

### Slice 3 - prepare on the existing session

- Add `NativeSession::open_project_with_auto_imports` and/or `prepare_auto_imports(path)` using `updateSnapshot`.
- Rotate the engine snapshot, release the old one, enforce the live cap, preserve the session generation semantics, and cache `(snapshot, path)`.
- Add real-engine test proving no process restart (`starts` stays 1) and a new snapshot id.
- Test failure paths: outside-root path, missing file, prepare rejection. Fail closed.

### Slice 4 - request symbols and adapter plumbing

- Thread `include_symbol` through `NativeSession`, `QuerySession`, `LiveQuerySession`, and `backend_tsgo::completion`.
- Request symbols only when auto-import marking is desired; do not flip `includeSymbol` globally without prepare.
- Preserve `documentation: None`; preserve `sortText`, order, and 256 cap.
- Add adapter fake tests for flags/status and the `u.` regression.

### Slice 5 - real-engine auto-import gate

- Add the `Us`/`User` fixture test in `crates/wxml-backend-tsgo/tests/f1_query.rs` (or `real_engine.rs`).
- Assert prepared + `includeSymbol:true` returns the candidate; unprepared + `includeSymbol:true` fails closed; `includeSymbol:false` lazy path is recorded but not treated as a supported marker.
- Assert no docs/displayParts/origin are invented.
- Required-mode (`WXML_REQUIRE_REAL_ENGINE=1`) has no skip.

### Slice 6 - F1-21/capability/docs

- Only after slices 1-5 pass: update F1-21 status from `planned`; update capability text and contract only with precise wording.
- If per-entry docs/import edit remain unavailable, keep `Partial`/`Unsupported` and do not mark F1-21 resolved.
- Do not touch F3, delivery, backend default, or Node.

---

## 9. What must remain BLOCKED (non-claims)

1. **Per-entry completion documentation**: BLOCKED. No `getCompletionEntryDetails`, no docs/displayParts field. Do not synthesize from symbol docs.
2. **Per-entry auto-import origin**: BLOCKED. No `data`/origin/source field; `sortText`/`symbol` are not enough.
3. **Actionable auto-import insertion**: BLOCKED. C0 `CompletionItem` has no additional text edit/import edit, and the native response has no `data`/`textEdit`.
4. **LSP-visible auto-import while only `ExpressionMember` is implemented**: BLOCKED. Auto-import is an identifier/global completion feature; the current path only handles member completion.
5. **F1-21 / B-07 closure**: open. The contract still lists completion details/auto-import params as unfrozen (`docs/reference/tsgo-native-contract.md:769`, `:793`).
6. **F3 closure**: not claimed, not touched. No delivery, packaging, backend default, or Node change was made.

## 10. Minimal acceptance evidence for a future change

A future implementation may only claim "auto-import plumbing works" when all of the following exist:

- Exact JSON assertions for `prepareAutoImports` on create and update.
- Exact JSON assertion for `includeSymbol:true` on the request.
- A real-engine test showing an auto-import candidate from a second module with a prepared snapshot.
- A real-engine fail-closed test for unprepared + `includeSymbol:true`.
- A real-engine regression that `u.` still returns `name`/`age` in order with `documentation: None`.
- A fake test proving no per-entry auto-import marker is fabricated from `name`/`sortText`/`symbol`.
- A perf measurement showing prepare is cached and not per keystroke.
- Explicit statement that per-entry docs and import-edit insertion remain BLOCKED.

Without all of these, the honest status remains: **F1 completion auto-import not implemented; per-entry documentation BLOCKED; F1/F3 not closed.**
