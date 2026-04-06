[in-progress] Naming Data Model Shape

Reshape naming data around the current incremental direction and remove leftover snapshot-shaped state.

## Goal

Make naming store semantic facts in the shape we actually want:

- one local binding id space
- symbol-keyed package-global lookup
- no persisted duplicate-detection-only data
- no eagerly materialized package-global value resolutions
- explicit naming work items for type-name lookup instead of coarse re-walk lists

## Settled direction

- Local naming uses one `BindingId` space directly.
- Package-global value lookup is symbol-keyed at package level:
  - `Symbol -> DocumentId`
- The defining module's local export table remains the source of the concrete exported `BindingId`.
- Persist only effective exported bindings per symbol in local naming:
  - `global_exports`
- Duplicate top-level bindings in package files should warn, but naming does not need to remember overwritten exports after diagnostics are produced.
- Duplicate top-level bindings in non-package documents do not produce the package-global duplicate-binding warning.
- Package-global non-local lookup should not be eagerly materialized as `ExpressionKey -> BindingId`.
- Stable exported declaration identity is out of scope for project 005.
- Prefer `non_locals` over `unresolved_values` for the value-side local naming table.
- Rebuild `global_bindings` from all local export tables whenever naming runs in project 005.

## Discussion pass (2026-04-06)

- Type-side explicit work items should cover annotations, not top-level definitions.
  - Lowering already collects top-level definitions in `Module.definitions`.
  - The package pass can read definitions directly from lowered HIR without storing duplicate definition work items in naming.
  - The real gap is annotation-owned type references, because they currently require the coarse `annotated_expressions` re-walk.
- `NamesGlobal` should be slimmer than the current implementation.
  - `NamesGlobal.bindings` is duplicated binding metadata.
  - `NamesGlobal.resolutions` is duplicated derived state.
  - Under the current design bar, both are design failures because they create multiple sources of truth.
  - The more robust shape is:
    - local binding facts live in `NamesLocal`
    - package-global symbol indirection lives in `NamesGlobal`
    - package-global resolutions are derived on demand from local `non_locals` plus package `global_bindings` plus the winning document's `global_exports`

## Target shape

```rust
pub struct NamesLocal {
    pub bindings: BTreeMap<BindingId, BindingInfo>,
    pub expression_resolutions: BTreeMap<ExpressionId, BindingId>,
    pub global_exports: BTreeMap<Symbol, BindingId>,
    pub non_locals: BTreeMap<ExpressionId, Symbol>,
    pub named_type_annotations: Vec<ExpressionId>,
}

pub struct NamesGlobal {
    pub global_bindings: BTreeMap<Symbol, DocumentId>,
}
```

Constraints:

- `global_exports` stores only the effective exported binding per symbol for one document.
- Duplicate-export diagnostics are emitted during naming, not preserved as exported-history data.
- Cross-file value lookup should go through:
  - local `non_locals`
  - package-global `global_bindings`
  - defining module `global_exports`
- Type-name lookup should use explicit stored annotation ids with named type references, not "revisit every annotated expression".

Why `NamesGlobal` has this shape:

- `global_bindings` is the package-global indirection layer:
  - it tells us which document currently wins for a symbol
- concrete `BindingInfo` remains owned by the defining document's `NamesLocal.bindings`
- this avoids duplicated binding metadata between local and global naming results
- `global_bindings` is rebuilt from local exports whenever naming runs
- keeping this compact winner table is justified because otherwise every cross-file global lookup would need to rescan all local export maps
- `NamesGlobal` should not store package-wide `ExpressionKey -> BindingId` resolutions because those are snapshot-local derived data

Incremental-update tradeoff:

- `Symbol -> DocumentId` is a good package winner table.
- It is not, by itself, a complete incremental-maintenance structure.
- If one document changes or is removed, this table alone does not tell us which earlier document should become the new winner for affected symbols.

That leaves three options:

1. Rebuild `global_bindings` from all local `global_exports` whenever package naming runs.
   - simplest
   - robust
   - the right choice while naming is still effectively package-wide

2. Keep `global_bindings` and later add a reverse incremental index.
   - example:
     - `Symbol -> ordered exporters`
   - then add/change/remove only updates affected symbols
   - this is the right next step once package naming itself becomes incremental

3. Drop `global_bindings` and resolve lazily by scanning all local export tables.
   - simpler persisted shape
   - but every cross-file lookup becomes a package scan unless another cache is introduced
   - that is likely worse once there are many non-local references

Current decision:

- Keep `global_bindings`.
- Rebuild it from all local exports during package naming for project 005.
- Do not try to maintain it incrementally yet.
- When we later build true incremental package naming, add a reverse symbol-to-exporters index instead of replacing `global_bindings` with repeated lazy scans.

Why this is the right choice for now:

- it keeps the current implementation simple
- it avoids introducing a second incremental-maintenance structure before naming itself is incremental
- it gives constant-time winner lookup after naming has run
- it avoids repeated lazy scans across all documents for every cross-file reference
- it keeps the future upgrade path clear:
  - later add `Symbol -> ordered exporters`
  - then update only affected symbols on document add/change/remove
  - without changing the higher-level lookup model

Comparison with other high-quality language tools:

- rust-analyzer keeps compact derived summaries and global scope structures rather than lazily rescanning source on each query.
  - `ItemTree` is a per-file summary that acts as an invalidation barrier.
  - `DefMap` stores module scopes.
  - The system is explicitly designed so body edits do not invalidate global derived data.
- clangd keeps explicit indexes rather than repeated lazy scans.
  - `FileIndex` stores symbols from files separately.
  - `MergedIndex` layers dynamic and background indexes.
  - Queries use those indexes instead of rescanning all files.
- TypeScript keeps symbol tables plus incremental builder programs.
  - the binder populates local/export/member symbol tables
  - builder programs cache and update affected results incrementally

Takeaway:

- the common pattern is:
  - per-file summaries
  - a compact global index derived from those summaries
  - incremental rebuild or merge of the index
- the common pattern is not:
  - repeated lazy package-wide scans for each non-local lookup

Implication for this project:

- `global_bindings` is the right kind of data structure.
- The real design question is not whether to keep a compact winner table.
- The real design question is when to rebuild it package-wide versus when to add the reverse index needed for fine-grained incremental maintenance.

## Current mismatches

Current implementation still carries state we want to remove or rename:

- `NamesLocal.top_level_exports`
  - should become package-pass-only temporary state or disappear entirely
- `NamesLocal.unresolved_values`
  - should be renamed to `non_locals`
- `NamesLocal.annotated_expressions`
  - should be replaced by `named_type_annotations`
- `NamesGlobal.resolutions`
  - should be removed so package-global lookup stays symbol-keyed instead of snapshot-local
- `NamesGlobal.bindings`
  - should be removed so binding metadata has one owner: the local naming result of the defining document

## Type-side work items

`annotated_expressions` is too weak because it only records that an expression had an annotation.
It does not record the semantic naming work that remains.

The replacement should record type-name references that need naming-owned lookup.

That table should be analogous to value-side `non_locals`:

- value side:
  - "this expression refers to symbol `x` outside file-local lexical scope"
- type side:
  - "this annotation expression still needs project-global type-name lookup"

Why the replacement should be `named_type_annotations: Vec<ExpressionId>`:

- top-level definitions are already directly available in lowered HIR
- nested type syntax does not currently have stable inner ids
- a one-field wrapper type adds abstraction without adding information
- storing per-type-name items would invent a second representation of annotation type syntax before we have a clear downstream need
- `named_type_annotations` is explicit about why these expression ids are stored, unlike `annotated_expressions`

## Remaining tasks

- [pending] Remove stored `top_level_exports` from persisted naming state and keep any duplicate-detection-only data temporary to the package pass.
- [pending] Rename `unresolved_values` to `non_locals`.
- [pending] Replace `annotated_expressions` with `named_type_annotations`.
- [pending] Remove duplicated package-global binding metadata and keep binding ownership local to each document's `NamesLocal`.
- [pending] Remove eagerly materialized package-global `ExpressionKey -> BindingId` resolutions.
- [done] Decide how to maintain `global_bindings` for project 005.
- [pending] Keep or tighten fixture coverage around same-file and cross-file package duplicate bindings as the data model changes.
- [pending] Add or tighten fixtures around the final non-package duplicate-binding behavior if the naming shape changes there.

## Out of scope

- durable exported declaration identity across edits
- solving later tooling identity problems beyond what symbol-keyed lookup already needs
