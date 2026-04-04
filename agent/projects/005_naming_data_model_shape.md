[in-progress] Naming Data Model Shape

Active implementation and validation in progress.

## Unresolved questions

- What is the smallest honest `LocalNames` shape that still supports:
  - local lexical resolution
  - duplicate top-level export diagnostics
  - package-global resolution
  - later hover / goto-definition work
- Should package-visible globals get stable identities before any attempt to stabilize `ExpressionId`?
- How should top-level exported declarations be represented so duplicate symbols remain diagnosable?
- How should annotation/type-name resolution be represented in naming:
  - as a side list like `annotated_expressions`
  - or by normal walked structures / later explicit ids?
- For incremental naming, should cross-file non-local references stay symbol-keyed until the defining module is queried, instead of being eagerly rewritten to package-global ids?

## Discussion pass (2026-04-04)

### 1) Do we need provisional ids? What is the point?

Current point of `ProvisionalBindingId` in `naming.rs`:

- local pass can allocate bindings before package-global ordering is known
- package pass can later allocate a separate final `BindingId`
- the current code uses this as a staging bridge (`provisional_to_final`)

Assessment:

- This is an implementation staging device, not a semantic requirement.
- If local naming and package naming are separated by stable contracts, provisional ids are optional.

Recommendation:

- remove `ProvisionalBindingId` from the model
- use one local binding identity in local naming
- let package-global naming reference locals via `(DocumentId, LocalBindingId)` or export-only global ids

This keeps local facts stable within the local result and removes the remap layer entirely.

### 2) Can we simplify `PackageNamingContext`?

Yes. The current context carries both phase data and temporary remap machinery:

- `provisional_bindings`
- `provisional_to_final`
- `next_provisional_binding_id`
- `next_binding_id`

These exist mostly because of the two-id staging model. If we drop that model, the package context can shrink to:

- immutable inputs (modules/local names/interner)
- package indexes (`global_exports`, `types`)
- outputs (`resolutions`, `diagnostics`)

Suggested direction:

```rust
pub struct LocalBindingId(pub u32);

pub struct LocalNames {
    pub bindings: BTreeMap<LocalBindingId, BindingInfo>,
    pub resolutions: BTreeMap<ExpressionId, LocalBindingId>,
    pub non_locals: BTreeMap<ExpressionId, Symbol>,
    pub global_exports: BTreeMap<Symbol, LocalBindingId>,
}

pub struct GlobalBindingRef {
    pub document_id: DocumentId,
}

pub struct GlobalNames {
    pub symbol_to_binding: BTreeMap<Symbol, GlobalBindingRef>,
    pub resolutions: BTreeMap<ExpressionKey, GlobalBindingRef>,
}
```

This removes id remapping and keeps package-global lookup symbol-keyed.
The local binding id is recovered from the defining module's `global_exports` map.

### 3) Can we make naming less OOP and reduce helper-function sprawl?

Yes. In this file, context objects currently own many tiny methods, including one-off wrappers. A flatter phase-first layout would align better with current coding rules:

- keep `resolve_document_locally` as a focused stateful walker
- make package pass explicit free functions in top-down order:
  - `build_type_index`
  - `build_global_exports`
  - `resolve_non_locals`
  - `resolve_annotations_and_definitions`
- keep only helpers reused in multiple places (for example shared diagnostic formatting)
- inline one-off wrappers (`binding`, `binding_info`, `module_expression_range`) where used

The result is fewer tiny methods, less mutable global context state, and clearer incremental invalidation boundaries.

## Proposed decision direction

- Decide that provisional ids are a temporary migration seam and should be removed in project 5.
- Make local naming own one local id space only.
- Keep package-global lookup symbol-keyed, with package-level references pointing to the defining module only and local binding ids read from that module's `global_exports`.
- Delay distinct stable global declaration ids until hover/goto-definition requirements force them.

## Settled in this session

- Implemented: removed `ProvisionalBindingId` from naming data and code paths.
- Implemented: local naming now uses one `BindingId` space directly.
- Implemented: package-global export table stores `Symbol -> DocumentId`; concrete binding ids are recovered from each module's `global_exports`.

## Current discussion summary

- The preferred working names in this project are `LocalNames` and `GlobalNames`.
- `expression_ranges` has been removed from the current local naming data because ranges already exist in lowered HIR.
- `annotated_expressions` may be the wrong storage shape even though resolving type names inside annotations still belongs to naming.
- `ProvisionalBindingId` reflects the current implementation shape more than the semantic model.
- Local lexical facts should not semantically change during package-global naming.
- A simpler local result is attractive:
  - local bindings by one real binding id
  - local resolutions as `ExpressionId -> BindingId`
  - unresolved non-locals as `ExpressionId -> Symbol`
- `global_exports` should represent the effective exported binding per symbol.
- Export identity still needs the symbol, because symbol lookup drives package-global resolution.
- If the same symbol is exported multiple times in one file, local naming should warn and keep only the last binding in `global_exports`.
- Package-visible globals are a good target for stable identities.
- `ExpressionId` is not currently durable across relowering, so globals should be stabilized first if we pursue stable ids incrementally.
- A symbol-keyed global export table is attractive because symbol lookup stays stable across relowering even when expression or local binding ids change.
- That means changing one file does not require patching other files' non-local references just because local lowering ids were rebuilt.
- In that model, cross-file global references can stay keyed by symbol during package-global resolution.

## Working direction

- Reshape the current local naming result toward a `LocalNames` data model based on semantic facts rather than implementation staging details.
- Keep duplicate top-level exports representable until package-global diagnostics have run.
- Keep a real package-global naming result rather than collapsing everything to ad hoc symbol lookup.
- Treat stable package-visible global identity and local lexical identity as separate concerns.

## Candidate shape

```rust
pub struct LocalNames {
    pub bindings: BTreeMap<BindingId, BindingInfo>,
    pub resolutions: BTreeMap<ExpressionId, BindingId>,
    pub global_exports: BTreeMap<Symbol, BindingId>,
    pub non_locals: BTreeMap<ExpressionId, Symbol>,
}
```

Why this is attractive:

- one local binding identity model
- export symbol access is explicit
- package-global resolution can consume the effective export table directly
- duplicate same-symbol exports can still be diagnosed while constructing the map
- symbol-keyed exports avoid coupling cross-file lookup to unstable expression ids

Rule:

- `global_exports` means effective exported binding per symbol
- if a later top-level binding exports the same symbol, local naming emits the warning and overwrites the earlier entry

### Possible package result

```rust
pub struct NamingResult {
    pub local: HashMap<DocumentId, LocalNames>,
    pub globals: GlobalNames,
}

pub struct GlobalNames {
    pub bindings: BTreeMap<GlobalBindingId, BindingInfo>,
    pub symbol_to_binding: BTreeMap<Symbol, GlobalBindingId>,
    pub resolutions: BTreeMap<ExpressionKey, GlobalBindingId>,
}
```

Why this is attractive:

- separates local lexical facts from package-visible identity explicitly
- leaves room for stable global ids without forcing local ids to become durable

## Stable globals

- Stable package-visible globals are worth pursuing.
- First target:
  - stable top-level exported bindings only
- Not first target:
  - local bindings
  - `ExpressionId`

Possible first key:

```rust
pub struct GlobalBindingKey {
    pub document_id: DocumentId,
    pub symbol: Symbol,
    pub top_level_index: u32,
}
```

Known limitation:

- `top_level_index` shifts when earlier exports are inserted

Longer-term direction:

- replace `top_level_index` with crate-owned top-level syntax provenance

## Important distinction

- A symbol-keyed global table is a good stable lookup mechanism.
- It is not automatically a full stable declaration identity model.
- Those are different jobs:
  - symbol-keyed lookup helps cross-file name resolution survive relowering
  - stable declaration identity helps tooling and invalidation distinguish one declaration site from another

## Symbol-keyed globals and incremental analysis

### Proposed split

- Package-global lookup:
  - `Symbol -> ModuleId`
- Per-module export lookup:
  - `ModuleId -> Symbol -> BindingId`
  - or `ModuleId -> Symbol -> ExpressionId`

Then resolving a non-local symbol works in two steps:

1. use the package-global table to find which module currently exports the symbol
2. use that module's local export table to find the actual exported binding/expression inside the module

### Why this helps incrementality

- Cross-file references stay keyed by symbol.
- Symbols are stable across relowering in a way `ExpressionId` is not.
- If file `A` changes and gets relowered, its local export ids may change.
- But files `B`, `C`, and `D` that refer to global symbol `foo` do not need to be patched just because `A` rebuilt local ids.
- Only file `A`'s local export table and the package-global symbol table need recomputation.

### Difference from the current approach

Current naming shape:

- package-global resolutions end up as `ExpressionKey -> BindingId`
- final `BindingId` values are allocated fresh during each naming run
- `ExpressionId` values are also snapshot-local

Practical consequence today:

- a full rerun can change ids even when the user-visible global symbol relationships did not change
- cross-file naming facts are therefore tied to one naming snapshot
- this makes incremental reuse harder because there is no naturally stable package-global lookup key

With the proposed split:

- package-global lookup is stable at the symbol layer
- local exported identities can be rebuilt per changed module
- dependent files can continue to say "I reference symbol `foo`" without being rewritten just because `foo`'s defining file got new local ids

### Important limitation

- This helps incremental name resolution.
- It does not by itself solve all tooling identity problems.
- If we need durable "go to this exact declaration site" identity across edits, we still want a stable declaration identity model on top of symbol lookup.

### Recommendation

- Use symbol-keyed tables as the package-global indirection layer.
- Keep id-based detail inside the defining module.
- Treat that split as an incremental-analysis optimization and a cleaner semantic boundary, even if we later add stable declaration ids on top.

### Hint for definitions

- Apply the same split to top-level definitions.
- Package-global definition lookup should stay symbol-keyed.
- The defining module should then map that symbol to the concrete local definition/binding id.
- This keeps cross-file references to definitions stable across relowering for the same reason as global value lookup:
  - other files keep referring to the symbol
  - only the defining module has to rebuild its local ids

## Tasks

- [done] Decide whether to replace `ProvisionalBindingId` with one local binding id space in the data model.
- [done] Decide package-global naming should be symbol-keyed at package level and module-local-id keyed inside each module.
- [pending] Decide whether annotation resolution needs explicit stored ids or only a different traversal strategy.
