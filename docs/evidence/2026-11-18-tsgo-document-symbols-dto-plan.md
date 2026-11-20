# Native document symbols → LSP: DTO, mapping and routing plan

- Worktree: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration`
- HEAD at audit/write time: `497380c` (`docs(evidence): plan adapter type asset wiring`)
- Native implementation: `9be74b3` (`feat(tsgo): decode native document symbols`)
- Date: 2026-11-19 (filename keeps the requested `2026-11-18` label)
- Scope: additive design for exposing `NativeSession::document_symbols` through `wxml_types::Backend` and the LSP outline.
- Boundary: read-only. No production, test, Cargo, contract, delivery or Node change. The only new file is this one. No commit. **F1 is not closed.**
- Worktree note: during the audit, unrelated uncommitted changes appeared in `crates/wxml-lsp/src/backend_tsgo/{mod,engine,sandbox}.rs`, `crates/wxml-lsp/src/{lib,main}.rs` and `crates/wxml-lsp/tests/cli.rs` (not authored here). All line citations are for HEAD `497380c`; the C0, native-decoder, `structure.rs`, `lifecycle.rs`, `server.rs` and `semantics/handlers.rs` files cited here are unchanged by those edits.

## 0. Verdict

1. **Recommend a separate `Backend::document_symbols` method with its own DTOs** (Option B). Do not add a tree to `SymbolResult`, and do not turn `SymbolQuery` into a union.
2. The native result is already presentation-ready but **not representable by C0**: `SymbolResult` has no kind, range, selection range or children, and its `complete` bit is a rename/reference contract, not an outline contract.
3. **WXML `documentSymbol` must stay static.** The existing outline is a per-document WXML outline (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/structure.rs:790`). Script symbols carry script-file ranges and cannot be merged into a WXML `DocumentSymbol[]` without fabricating ranges.
4. The honest LSP route is `textDocument/documentSymbol` for a **script document** (`DocumentRoute::Script`). It needs a host reverse lookup (script → owning view) and a script snapshot identity; both are phase-3 prerequisites, not part of the DTO change.
5. Node stays honest: the new trait method has a default `Unsupported`; Node's existing `symbols` path and private protocol are untouched. No default backend is selected or switched.

## 1. Current state (code facts)

### 1.1 C0 boundary

- `Backend::symbols` is the only symbol method today; its default is `Unsupported` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs:381`).
- `SymbolRequest` binds one snapshot plus a `subject` script and a `SymbolQuery` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/symbols.rs:63`).
- `SymbolQuery` is `#[non_exhaustive]` and covers only `DataRoots`, `DataPath`, `EventHandler` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/symbols.rs:33`).
- `SymbolResult` carries `status`, `complete`, `reason`, `declaration`, `references`, `occurrences`, `insertion`, `fields`, `reserved` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/symbols.rs:176`). It has **no** document-symbol field and is **not** `#[non_exhaustive]`.
- C0 return ranges are UTF-8 `SourceRange`; the LSP layer converts to UTF-16 at the protocol edge (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs:3`).
- `BackendCapabilities` has four fields and `none()` builds all four (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs:298`, `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs:314`).

### 1.2 Native implementation

- `NativeSession::document_symbols(path)` captures the VFS text + content hash, fetches `getSourceFile` AST, rejects a stale snapshot, then decodes (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/session.rs:1593`).
- Decoder limits: `MAX_DOCUMENT_SYMBOLS = 256`, `MAX_DOCUMENT_SYMBOL_DEPTH = 32`, `MAX_DOCUMENT_SYMBOL_GAPS = 16` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/document_symbol.rs:47`, `:50`, `:55`).
- Evidence carries UTF-16 and UTF-8 ranges plus a verified name range and children (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/native/document_symbol.rs:96`).
- The walker records gaps for import/export kinds (`.../native/document_symbol.rs:230`), unsupported statements (`:239`), depth overflow (`:285`), 256 cap (`:317`), and constructor/static-block/call/construct/index member kinds (`:374`).
- `query::document_symbol::map_document_symbols` maps frozen v9 kinds to LSP numbers, preserves both coordinate systems, and makes any gap/truncation `Partial` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-backend-tsgo/src/query/document_symbol.rs:128`). `TypeAliasDeclaration` maps to `SymbolKind.Interface` while `node_kind` preserves the distinction (`.../query/document_symbol.rs:61`, `:94`).
- Empty evidence is `Supported` (checked empty file); skipped candidates/truncation never become a supported empty answer (`.../query/document_symbol.rs:159`).

### 1.3 LSP outline today

- `documentSymbolProvider: true` is advertised unconditionally (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/lifecycle.rs:98`); the request is dispatched at `:284`.
- `Server::document_symbol` rejects non-template documents with `[]` before parsing (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/structure.rs:805`).
- The WXML outline uses `StructureFacts` (`.../structure.rs:171`), caps at 2,000 nodes / 32 levels (`.../structure.rs:64`, `:67`), and serializes hierarchically or flat (`.../structure.rs:574`, `:605`).
- Hierarchical support is a client flag (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/server.rs:123`, `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/lifecycle.rs:386`).
- Script overlays are tracked by the host, not by `server.workspace` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/diagnostics.rs:174`); effective script text is available via `ProjectHost::file_text` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:456`).
- `view_for`/`view_for_file` resolve a **WXML** path only (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/project_host.rs:421`, `:1220`); there is no script→view reverse lookup.

## 2. DTO options

### Option A — extend `SymbolResult` with a tree

Shape: add `SymbolQuery::DocumentSymbols` and `SymbolResult.document: Option<DocumentSymbolTree>`.

| Area | Impact |
| --- | --- |
| `wxml-types` | `SymbolResult` is not `#[non_exhaustive]`; every struct literal breaks. `unsupported()` must initialize a new field (`.../wxml-types/src/symbols.rs:203`). |
| Node backend | Struct literal at `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/response.rs:1635` and the degraded literal at `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/mod.rs:665` must add the field; Node must set `None` or grow its private protocol. |
| Static semantics | Reads only known fields, so it compiles; but `complete` (rename gate at `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/semantics/handlers.rs:1437`, `:1774`) now sits next to a payload that uses `truncated`/`skipped`, inviting misuse. |
| Adapter | `symbols()` becomes a dispatcher; one method has two unrelated completeness contracts. |
| Scheduler | `JobKind::Symbols` already carries a query (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/scheduler.rs:74`); `symbols_done` still expects `SymbolResult`. |

Verdict: **reject**. It is a compile-time break for a semantically different feature and pollutes the rename/reference DTO.

### Option B — separate `Backend::document_symbols` (recommended)

Shape: new C0 module plus a defaulted trait method.

```rust
// crates/wxml-types/src/document_symbols.rs (new)
pub const MAX_DOCUMENT_SYMBOLS: usize = 256;
pub const MAX_DOCUMENT_SYMBOL_DEPTH: usize = 32;
pub const MAX_DOCUMENT_SYMBOL_GAPS: usize = 16;

#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DocumentSymbolKind {
    Module, Class, Method, Property, Enum, Interface,
    TypeAlias, Function, Variable, EnumMember,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSymbolsRequest {
    pub context: RequestContext, // owning WXML view snapshot
    pub subject: FileId,         // script whose outline is requested
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: DocumentSymbolKind,
    pub range: SourceRange,           // UTF-8, original subject
    pub selection_range: SourceRange, // UTF-8 name token, inside range
    pub children: Vec<DocumentSymbol>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSymbolGap {
    pub start: Option<u32>, // UTF-8 offset in subject; None if unmappable
    pub reason: String,     // bounded, display-safe
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSymbolsResult {
    pub symbols: Vec<DocumentSymbol>,
    pub status: CapabilityStatus,
    pub truncated: bool,
    pub skipped: usize,
    pub gaps: Vec<DocumentSymbolGap>,
}
```

Trait addition, with the same fail-closed pattern as `definition`/`completion`:

```rust
fn document_symbols(
    &self,
    _request: DocumentSymbolsRequest,
) -> Result<DocumentSymbolsResult, BackendError> {
    Ok(DocumentSymbolsResult::unsupported(
        "document symbols are not implemented",
    ))
}
```

| Area | Impact |
| --- | --- |
| `wxml-types` | Additive module + re-export in `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/lib.rs:45`. `SymbolResult` and `SymbolQuery` untouched. |
| Node backend | Compiles unchanged; inherits the default `Unsupported`. No private protocol change (`.../backend_node/request.rs:677`, `:722`). |
| Static semantics | Untouched. `node_query` (`.../semantics/handlers.rs:783`) and all `SymbolResult` consumers keep their current types. |
| Adapter | Overrides the method; existing `symbols()` keeps returning `Unsupported` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:414`). |
| Capabilities | No new field needed in phase 1. Per-request `status` is authoritative. If a flag is later added, update `none()` (`.../wxml-types/src/backend.rs:314`), Node (`.../backend_node/mod.rs:777`), mock (`.../wxml-types/src/mock.rs:218`), adapter (`.../backend_tsgo/mod.rs:441`). |
| Scheduler | Needs a new job/outcome only in phase 3; no change to `JobKind::Symbols`. |

Verdict: **recommend**. Lowest coupling, no Node/static-semantics churn, and the completeness contract is independent.

### Option C — request-kind discriminated union

Shape: `SymbolQuery::DocumentSymbols` plus a result payload enum (`Facts(SymbolFacts)` / `Outline(DocumentSymbolsResult)`), or a `SymbolResult` enum.

| Area | Impact |
| --- | --- |
| All consumers | Field access (`result.declaration`, `result.complete`, ...) must become a match; rename/code-action code at `.../semantics/handlers.rs:1437`, `:1774`, `:1832` changes. |
| Node | Must construct a new payload shape in `.../backend_node/response.rs:1635`; a protocol response can now be two shapes. |
| Adapter | Same dispatcher problem as A, plus a breaking type change. |

Verdict: **reject**. Highest churn, no capability gain over B.

### Recommendation

**Option B.** Add `DocumentSymbolsRequest`/`DocumentSymbolsResult`/`DocumentSymbol`/`DocumentSymbolKind`/`DocumentSymbolGap` with a defaulted trait method. Keep `SymbolResult` and `SymbolQuery` exactly as they are.

## 3. LSP mapping

### 3.1 Hierarchical vs flat

- If `client.hierarchical_symbols` is true (`.../server.rs:123`), emit `DocumentSymbol[]` with `children`, `range` and `selectionRange`.
- Otherwise emit flat `SymbolInformation[]` with `location.uri` = requested script URI and `containerName` = enclosing symbol name. `SymbolInformation` has no `selectionRange`; use the declaration `range`.
- Do not mix shapes. Do not emit `SymbolInformation` with a WXML URI for script symbols.
- Reuse the existing all-or-nothing serialization discipline (`.../structure.rs:815`): if a range cannot be converted, log and return `[]` rather than a half-serialized outline. (Alternative: drop only the bad node; see §8.)

### 3.2 Kind mapping

| Native declaration | C0 `DocumentSymbolKind` | LSP `SymbolKind` |
| --- | --- | --- |
| `FunctionDeclaration` | Function | 12 |
| `ClassDeclaration` | Class | 5 |
| `InterfaceDeclaration` | Interface | 11 |
| `TypeAliasDeclaration` | **TypeAlias** | **11 (Interface)** |
| `EnumDeclaration` | Enum | 10 |
| `ModuleDeclaration` (`namespace`/`module`) | Module | 2 |
| `VariableDeclaration` | Variable | 13 |
| `PropertyDeclaration` / `PropertySignature` / get/set accessor | Property | 7 |
| `MethodDeclaration` / `MethodSignature` | Method | 6 |
| `EnumMember` | EnumMember | 22 |

The native mapper already produces these LSP numbers (`.../query/document_symbol.rs:61`). The C0 DTO keeps `TypeAlias` distinct so the LSP layer can prove the mapping; the adapter derives it from `DocumentSymbol.node_kind` (`.../query/document_symbol.rs:94`) because the existing enum collapses it to `Interface`. Do not add `Constructor`, `CallSignature`, etc. as kinds: they are gaps.

### 3.3 Ranges and selection ranges

- `range` = native declaration range including leading trivia; `selection_range` = verified name token. Native already checks `selection.start < selection.end`, `selection.end <= node.end`, and declaration containment (`.../native/document_symbol.rs:461`).
- After shadow→original mapping, re-check the same invariants against the original script bytes. If either range fails, drop the symbol and mark the result `Partial`; **never clamp** (the definition mapper uses the same no-clamp rule at `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/definition.rs:137`).
- LSP requires `selectionRange` to be contained by `range`; the serializer must assert this even after the adapter validates.
- Children must be contained by their parent’s mapped range; drop a child that is not, and mark `Partial`.

### 3.4 Caps, depth and gaps

- 256 nodes max per file, applied in source order by the native walker (`.../native/document_symbol.rs:317`). `truncated = true` when the cap is hit; the 257th node is not a gap.
- 32 levels max (`.../native/document_symbol.rs:285`); a deeper node is a gap.
- 16 stored gaps max; `skipped` still counts every gap (`.../native/document_symbol.rs:491`).
- The LSP response has no completeness field. The serializer returns the decoded subset and logs a bounded reason when `status != Supported`; it must not append a placeholder symbol. The 256 cap is intentionally lower than the WXML outline’s 2,000; document the asymmetry rather than raising it in the LSP layer.

### 3.5 UTF-16

- C0 carries UTF-8 `SourceRange` only; the LSP edge converts with `LineIndex`.
- For a script document, build the line index from the **effective script text** (overlay first, then disk): `ProjectHost::file_text` (`.../project_host.rs:456`), not the WXML document.
- Reuse `line_index_range_value` (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/handlers/mod.rs:102`).
- Emoji (2 UTF-16 units, 4 UTF-8 bytes), CJK (1 unit, 3 bytes), and names with astral characters must be tested in both `range` and `selectionRange`.

## 4. Preserving existing facts and Partial semantics

### 4.1 Existing facts stay where they are

- `DataRoots`/`DataPath`/`EventHandler` facts, `references`, `occurrences`, `fields`, `reserved`, `insertion`, `declaration` remain exclusively in `SymbolResult` and `Backend::symbols`.
- No `DocumentSymbols` variant is added to `SymbolQuery`; the Node validator (`.../backend_node/request.rs:677`) and the private protocol JSON (`.../backend_node/request.rs:722`) do not change.
- Rename/code-action gates keep using `SymbolResult.complete` (`.../semantics/handlers.rs:1437`, `:1774`, `:1832`). Document symbols have no reference tokens, `expected_text`, insertion anchor, or reserved names, so they **must not** be used to build edits.

### 4.2 Partial / gaps

- `Supported` requires all of: native status `Supported`, `truncated == false`, `skipped == 0`, `gaps.is_empty()`, no adapter mapping drops, no host degradation.
- Any dropped symbol/range/gap/truncation/host degradation → `Partial` with a bounded reason. The decoded subset is still returned; an outline is advisory, unlike rename/reference completeness.
- Empty + `Supported` means “checked, this script declares nothing”. Empty + `Partial`/`Unsupported` must not be presented as “no symbols”; the LSP returns `[]` (no standard partial-outline channel) and logs the status.
- `DocumentSymbolGap.start` is optional because a generated/shadow-only position may not map to original script bytes; do not invent an offset.
- Do not upgrade a status because mapping “mostly worked”; follow `mark_partial` (`.../backend_tsgo/definition.rs:191`).

## 5. Request routing, Node compatibility and the default backend

### 5.1 Which handler sends the request

1. Client sends `textDocument/documentSymbol` for a script URI.
2. `lifecycle.rs:284` dispatches to `structure::document_symbol` (`.../structure.rs:790`).
3. Before the `template_document` short-circuit (`.../structure.rs:805`), branch on `DocumentRoute::Script` (`.../project_host.rs:179`).
4. Resolve the owning view with a new defaulted host method (recommended):
   ```rust
   fn view_for_script(&self, _file: &FileId) -> Result<ViewTarget, HostError> {
       Err(HostError::NoProject)
   }
   ```
   Implement it by scanning the project’s views for `script_file == file`; return the unique owner or `HostError::Unsupported` when the script is shared/not an owner. This avoids choosing an arbitrary WXML context.
5. Build the `RequestContext` from the owning view’s WXML snapshot. Preferred: a host method returning a stable `Arc<SourceSnapshot>` for the script keyed by `(file, revision)`. MVP fallback: require the owning WXML document to be open (`host.source(&wxml_file)`); otherwise return `[]` and log. Do not synthesize a `SnapshotId` per request (it would defeat `RequestIdentity` session reuse at `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/input.rs:74`).
6. Submit a scheduler job (`JobKind::DocumentSymbols { subject }`, `Outcome::DocumentSymbols`) with a script stamp (project revision + script revision + view revision + backend epoch). The backend call is blocking and must not run on the protocol loop (`.../wxml-types/src/backend.rs:336`).
7. On completion, re-check the stamp/epoch; stale → `[]`/`ContentModified`. Serialize `DocumentSymbolsResult` as §3.
8. WXML requests keep the static path and never start the backend.

**Rejected route:** appending script symbols to the WXML `documentSymbol` response. `DocumentSymbol.range`/`selectionRange` are relative to the requested document; script ranges would break outline navigation. Flat `SymbolInformation` could carry a script URI, but mixing URIs into a WXML outline is still a different feature and is not attempted.

### 5.2 Node compatibility

- Node does not override `document_symbols`; the default returns `Unsupported` with no process start.
- Existing `symbols()` (`.../backend_node/mod.rs:649`) and its response mapper (`.../backend_node/response.rs:1586`) are untouched.
- Do not derive document symbols from Node `symbols` facts: they lack kind, range, selection range and children.
- A script `documentSymbol` under Node returns `[]`, exactly as today (`.../structure.rs:805`).

### 5.3 No default backend switch

- `TsgoBackendAdapter` remains unconstructed by `server.rs`/config (`/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/mod.rs:5`).
- The new method is reachable only after P6 selects the adapter. This design does not change the default backend, capabilities advertisement, or delivery.
- No new `documentSymbolProvider` registration is needed: it is already advertised at `lifecycle.rs:98`. Do not claim script-outline support in `initialize` until a backend actually implements it.

## 6. Tests

| Target | Test | Purpose | Phase |
| --- | --- | --- | --- |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/document_symbols.rs` | `document_symbols_result_keeps_tree_ranges_and_children` | DTO shape, UTF-8 ranges, nested children, clone/eq | 1 |
| same | `document_symbols_caps_are_256_32_16` | Constants match native limits | 1 |
| same | `unsupported_answer_has_no_fabricated_symbols` | `Unsupported` ⇒ empty tree, no gaps | 1 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-types/src/backend.rs` | `document_symbols_defaults_to_unsupported_rather_than_empty_success` | Mirrors the definition default test at `:639` | 1 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/tests.rs` | `document_symbols_map_shadow_ranges_back_to_original_script` | Shadow→original mapping, selection range containment | 2 |
| same | `document_symbols_mapping_gap_is_partial_and_keeps_the_subset` | No clamp, no fabricated symbol | 2 |
| same | `document_symbols_keep_truncated_skipped_and_gaps` | 256/depth/gap pass-through stays `Partial` | 2 |
| same | `document_symbols_reject_a_subject_outside_the_prepared_view` | No path resolution, no Node fallback | 2 |
| same | `document_symbols_native_unsupported_is_not_an_empty_success` | Mirrors `symbols_are_explicitly_unsupported` at `:1385` | 2 |
| same (gated by `TSGO_BIN`, `WXML_REQUIRE_REAL_ENGINE=1`) | `real_engine_document_symbols_roundtrip` | Real pinned engine through the adapter; CJK/emoji | 2 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/structure.rs` tests | `script_document_symbols_are_hierarchical_with_selection_ranges` | Hierarchical serialization, UTF-16, CJK/emoji | 3 |
| same | `script_document_symbols_flatten_without_hierarchical_support` | `SymbolInformation` + `containerName` | 3 |
| same | `script_document_symbols_do_not_pad_gaps_or_truncation` | 256 cap/gaps never become placeholders | 3 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/server.rs` tests | `script_document_symbol_request_routes_to_the_backend` | `setup_with_backend` (`:741`) + mock result; WXML path untouched | 3 |
| same | `script_document_symbol_stale_stamp_returns_empty` | Stamp/epoch drop | 3 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_node/tests.rs` | `node_document_symbols_are_unsupported_without_starting_node` | Default `Unsupported`; no transport line | 2 |
| `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/structural_editing.rs:744`, `:775` | existing hierarchy/flat tests | Regression: WXML outline unchanged | 3 |

Do **not** add a 17th `native_parity` partition: `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/tests/native_parity.rs:192` asserts exactly 16 partitions and empty skeletons (`:165`). If the ledger later gets a document-symbol case, route it into an existing partition or extend the ledger in a separate change.

Real-engine decode coverage already exists in `NativeSession` tests (`.../native/session.rs:2598`, `:2710`, `:2752`, `:2796`); the new real-engine test above is the adapter-level composition, not a replacement.

## 7. Phases and what remains BLOCKED

### Phase order

1. **DTO + trait default** (no behavior change): new `document_symbols` module, exports, `Backend::document_symbols` default, unit tests. Node/static semantics untouched.
2. **Adapter mapping**: `QuerySession::document_symbols` (new seam next to `/home/zxg/.local/share/opencode/worktree/e15985/tsgo-migration/crates/wxml-lsp/src/backend_tsgo/session.rs:58`), `backend_tsgo/document_symbol.rs` mapping (shadow/verbatim), fake-session tests, opt-in real-engine test. Still no server route.
3. **LSP route**: host `view_for_script` + script snapshot, `JobKind::DocumentSymbols`/`Outcome::DocumentSymbols`, serializer + handler branch, server unit tests. Behavior activates only for a backend that implements the method; Node returns `[]`.
4. **P6 selection + editor acceptance** (out of scope): select the adapter, advertise the capability, validate Zed outline. Not approved by this document.

### BLOCKED / not claimed

- **External/generated symbols:** import/export/export-assignment/namespace-export declarations are explicit gaps (`.../native/document_symbol.rs:230`). No import/export outline entries.
- **Constructors, class static blocks, call/construct/index signatures, computed names, string/numeric property names:** no Identifier name slot; gaps (`.../native/document_symbol.rs:374`, module docs `:16`).
- **Rename:** still no native API; document symbols cannot supply reference tokens or `expected_text` (`.../backend_tsgo/mod.rs:26`). Rename stays blocked/unchanged.
- **DataPath/EventHandler script facts:** the native `symbols` method remains `Unsupported` (`.../backend_tsgo/mod.rs:414`); this design does not wire them.
- **WXML outline merge:** rejected; per-document range contract.
- **Workspace/cross-file symbol index:** out of scope; native queries one file at a time.
- **Detail/signature, tags/deprecated, type annotations:** native evidence has no such fields; do not fabricate.
- **JS parity:** unproven; real-engine tests are TS-only.
- **Shared scripts / multiple owning views:** MVP rejects (`view_for_script` must return a unique owner).
- **Script outline when the owning WXML is not open:** MVP returns `[]` unless the phase-3 script snapshot plumbing lands.
- **Non-Unix roots / Windows drive/UNC:** unchanged platform limit (`.../backend_tsgo/mod.rs:44`).
- **F1:** not closed; no capability claim, no default switch, no contract change.

## 8. Open decisions for review

1. `BackendCapabilities.document_symbols`: omit in phase 1 (recommended, avoids four literal updates) vs add with `Unsupported` defaults.
2. Host reverse lookup: `view_for_script` (recommended) vs requiring the owning WXML to be open and scanning `server.workspace`.
3. Serializer failure policy: all-or-nothing (`[]` + log, consistent with `structure.rs:838`) vs drop only the invalid node (better UX, but needs a status channel).
4. Whether `TypeAlias` belongs in C0 or only in the LSP mapping. Recommended: C0 keeps it; LSP maps it to 11.
5. `DocumentSymbolGap` provenance: keep only a bounded `reason` (recommended) vs carry opaque native `node_kind`/`name_kind`.
