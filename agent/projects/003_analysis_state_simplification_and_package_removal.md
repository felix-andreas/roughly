# Analysis Simplification And Package Removal [done]

## Goal

Make `Analysis` the sole durable owner of typing inputs and derived phase state.

The target result is:

- no `Package` owner in the typing pipeline
- one stable internal `DocumentId`
- path-based document mutation APIs on `Analysis`
- separate lowering, naming, and typecheck stores keyed by `DocumentId`
- per-document diagnostics stored in `Analysis`

This should remove the current path-keyed duplication and give later phases one coherent state owner.

## Non-goals

- fully redesign typechecking semantics in this project
- settle long-term package ordering semantics
- reintroduce a broader `Workspace` owner as the semantic center
- optimize for the smallest migration if a cleaner phase boundary requires wider edits

## Unresolved questions

- None currently recorded.

## Settled direction

- `Analysis` owns `Document`
- `Analysis` owns shared parser and interner state
- `DocumentId` is the stable internal identity
- public mutation stays path-based:
  - `add_document`
  - `edit_document`
  - `delete_document`
- `Package` should be removed from the typing pipeline
- one file-local HIR module uses `DocumentId` as its stable identity
- HIR `Module` remains the structural representation
- naming and typecheck remain side tables keyed by stable ids
- diagnostics are stored per document inside `Analysis`
- phase execution is manual; `Analysis` does not run phases automatically on edit
- sorting is not part of the new design for now
- `non_package_documents: Set<DocumentId>`
  - every document inside this set does not contribute to the package namespace
  - every document not in this set does contribute to the package namespace

## Target shape

The intended top-level shape is:

```rust
struct Analysis {
    documents: DocumentTable<DocumentId, Document>,
    document_ids_by_path: HashMap<PathBuf, DocumentId>,
    non_package_documents: Set<DocumentId>,
    interner: Interner,
    parser: Parser,
    lowering: LoweringStore,
    naming: NamingStore,
    typecheck: TypecheckStore,
    diagnostics: HashMap<DocumentId, Vec<PhaseDiagnostic>>,
}
```

```rust
struct LoweringStore {
    modules: HashMap<DocumentId, Module>,
}
```

```rust
struct NamingStore {
    locals: HashMap<DocumentId, LocalNamingResult>,
    package: PackageNamingResult,
}
```

`DocumentTable` is only a placeholder for “stable-key storage”.
The important requirement is stable `DocumentId`, not a specific container crate.

## Phase boundary consequences

After this redesign:

- lowering operates on documents owned by `Analysis`
- naming operates on lowered modules owned by `Analysis`
- typecheck operates on naming output owned by `Analysis`
- fixtures build and mutate `Analysis` directly

This means the current `Package`-shaped APIs in `analysis.rs` should disappear.

## Migration notes

The main architectural migration is:

- from path-keyed caches plus `Package`
- to `DocumentId`-keyed caches plus one `Analysis`

That implies:

- replace path-keyed phase tables with `DocumentId`-keyed tables
- remove `ModuleId` where it is only mirroring file identity
- stop reconstructing package order by sorting paths
- remove path-sorted package assembly as a concept

If a temporary adapter is needed during the migration, keep it narrow and delete it once all callers use `Analysis` directly.

## Implementation plan

### 1. Record the redesign and align the working docs [done]

- capture the settled `Analysis` direction in `DISCUSS.md`
- add this project plan
- update `TODOS.md` so it points at this project instead of the abandoned earlier analysis-state plan

### 2. Introduce the new `Analysis` core model [done]

- add stable `DocumentId`
- replace the current path-keyed lowered-document cache shape
- add document storage owned by `Analysis`
- add `document_ids_by_path`
- add `non_package_documents`
- move parser ownership onto `Analysis`
- expose shared interner access without keeping the current `LoweringContext` as the top-level state owner

### 3. Move document lifecycle APIs onto `Analysis` [done]

- implement `add_document(path, document) -> DocumentId`
- implement path-based `edit_document`
- implement path-based `delete_document`
- ensure edits and deletes invalidate:
  - lowering cache
  - naming cache
  - typecheck cache
  - diagnostics for the affected document
- dependent-document invalidation for changed exported globals remains a later follow-up
- keep mutation ownership inside `Analysis` so invalidation cannot be bypassed accidentally

### 4. Remove `Package` from `analysis.rs` [done]

- make `check` operate on `Analysis`
- make `run_lowering` operate on selected `DocumentId`s or all package documents
- make `run_naming` consume `Analysis` lowering results directly
- make `run_typecheck` consume `Analysis` naming results directly
- remove temporary package-assembly work that only exists because `Package` currently owns documents

### 5. Rewrite lowering storage around `DocumentId` [done]

- store lowered `Module`s directly in `LoweringStore.modules`
- remove path-keyed `LoweredDocument`
- remove `module_ids_by_path`
- remove `next_module_id`
- replace file identity uses with `DocumentId`

### 6. Rewrite naming storage around `DocumentId` [done]

- replace `ModuleId` in naming storage with `DocumentId`
- keep `LocalNamingResult` per document
- keep package-level naming tables in `NamingStore.package`
- ensure naming diagnostics attach to `DocumentId`
- keep the file-local and package-global naming split intact during the migration

### 7. Reshape typecheck inputs and temporary package remapping [in-progress]

- update typecheck entry points to consume `Analysis` state instead of `Package`
- remove path-sorted package assembly
- keep the temporary package remapping logic local to `analysis.rs`
- do not preserve sorting helpers in the new design

### 8. Update fixtures and tests to use `Analysis` directly [done]

- construct `Analysis` directly in fixture tests
- stop using package-specific setup for multi-file typing tests
- add direct coverage for:
  - package-contributing documents
  - non-package documents
  - path-based edit and delete invalidation
- keep phase execution explicit in tests

### 9. Cleanup and remove stale adapters [in-progress]

- delete code paths that still assume `Package` owns documents
- delete remaining path-keyed semantic caches
- remove obsolete wrappers introduced only for the migration
- keep `DISCUSS.md`, `TODOS.md`, and implementation aligned at the end of the project
