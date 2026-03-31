# Typing Crate Discussion

This file is scratch space for active design discussion.

Use it for short summaries or temporary notes during a design conversation.

## Current topic

### AnalysisState simplification

Settled:

- `AnalysisState` should use one stable `DocumentId` as the primary identity
- `AnalysisState` should own durable analysis state, not just transient lowering caches
- `AnalysisState` should own `Document` so edits, deletes, and diagnostic cleanup happen in one place
- HIR `Module` stays the structural representation
- naming and typecheck should stay as side tables keyed by stable ids, not duplicated trees
- `run_naming` stays the public phase boundary; file-local and package-global naming remain internal steps
- one file-local HIR module can use the same stable identity as its `DocumentId`; a separate `ModuleId` is not currently justified
- diagnostics should stay per document inside analysis state rather than becoming a separate merged-results subsystem
- public editing should stay path-based; `DocumentId` is an internal stable key rather than the main external API

Current tension:

- one large `DocumentState` record reduces identity duplication, but it mixes phase-owned data and tends to push fields toward `Option<_>`
- fully parallel maps preserve phase boundaries, but they recreate consistency work if every phase also owns its own path and document identity tables

Why the duplication appears with fully parallel maps:

- create, delete, or path-move operations would need to update every phase-local path index
- each phase would need to answer the same bookkeeping questions again:
  - does this document still exist
  - what is its current path
  - is this cached result stale because the document changed
- centralizing source ownership and path lookup once avoids repeating that logic in lowering, naming, and typecheck

Recommended compromise:

- keep one canonical document registry for source ownership and identity
- keep separate phase stores keyed by `DocumentId`

Suggested shape:

- `documents: DocumentTable<DocumentId, DocumentEntry>`
- `document_ids_by_path: HashMap<PathBuf, DocumentId>`
- `interner: Interner`
- parser state owned directly by `AnalysisState` once parsing moves into `typing`
- `lowering: LoweringStore`
- `naming: NamingStore`
- `typecheck: TypecheckStore`

`SlotMap` is not the point.
The only real requirement is a stable `DocumentId` that does not shift when another document is deleted.
That can be implemented with `slotmap`, `slab`, a custom arena, or another stable-key table.

Suggested `DocumentEntry`: (unnecesary wrapper)

- `document: Document`

Suggested `LoweringStore`:

- `modules: HashMap<DocumentId, Module>`

Suggested `NamingStore`:

- `locals: HashMap<DocumentId, LocalNamingResult>`
- `package: PackageNamingResult`

Suggested `TypecheckStore`:

- package-scoped inferred tables

Diagnostics direction:

- keep one consolidated diagnostics table in `AnalysisState`
- keep the entries phase-tagged so ordering and suppression rules stay explicit (yes)

Suggested diagnostics shape:

- `diagnostics: HashMap<DocumentId, Vec<PhaseDiagnostic>>`

Suggested `PhaseDiagnostic`:

- `phase: AnalysisPhase`
- `diagnostic: Diagnostic`

Why this compromise is better:

- it preserves phase boundaries, which matters for incremental scheduling and API clarity
- it avoids a giant optional `DocumentState`
- it still removes the worst duplication by centralizing source ownership, path lookup, and document lifecycle around `DocumentId`

Local versus global naming:

- store `LocalNamingResult` per document as the reusable incremental artifact
- build `PackageNamingResult` from all local naming results
- the package step should consume explicit per-document exports and unresolved references, not rescan the whole HIR

Recommended API direction:

- `add_document(path, document) -> DocumentId` (i renamed to add document)
- `edit_document(path, edit)` where `edit` mutates the stored `Document` and `AnalysisState` invalidates dependent phase caches afterward
- `delete_document(path)`
- `run_lowering(changed_documents: Option<&[DocumentId]>)`
- `run_naming(changed_documents: Option<&[DocumentId]>)`
- `run_typecheck(changed_documents: Option<&[DocumentId]>)`

Open decisions:

- should we remove `Package` entirely from the typing pipeline? (yes)
  - today `Package` is a second owner of `PathBuf -> Document` (yes)
  - that conflicts with the decision that `AnalysisState` owns documents
  - the likely replacement is package membership stored inside `AnalysisState`
- non-package documents should be represented by a `Set<DocumentId>`
  - every document inside this set is not part of the package
- should diagnostics be rebuilt eagerly after each phase run, or assembled lazily when read? (no, you must trigger everything in analysis state manually for now)

### Package boundary

The current `Package` shape does not fit the target architecture.

If `AnalysisState` owns `Document`, then `Package` becomes redundant as a second document store.
The cleaner direction is to remove `Package` from the typing pipeline and keep package membership inside `AnalysisState`.

That would make the state responsible for:

- document storage
- path lookup
- package membership
- script membership
- diagnostics
- phase caches

This also simplifies fixtures:

- fixtures can build an `AnalysisState` directly
- tests do not need a separate package-building layer just to run naming or typecheck
- lowering can still operate on one `Document` directly when a package context is unnecessary

Suggested package-membership shape inside `AnalysisState`:

- `non_package_documents: Set<DocumentId>`
- every document not in this set contributes to the package

The important part is that package membership uses `DocumentId`, not `PathBuf -> Document`.

### Order

Sorting should not be part of the new design.

The current path-sorting in `Package::ordered_documents_and_scripts` and `package_hir::sorted_modules` is a temporary artifact of storing documents in maps.
For now, the redesign should remove explicit sorting and defer canonical package order until there is a real reason to specify it.

That implies:

- do not design new APIs around sorted module lists (yes)
- do not preserve `sorted_modules` as a concept (yes)
- let later package-order semantics be added explicitly once needed (yes)
