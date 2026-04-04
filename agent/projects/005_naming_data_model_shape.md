[planning] Naming Data Model Shape

Concept phase only. Not ready to be implemented.

## Unresolved questions

- Should local naming use one real `BindingId` space immediately, rather than `ProvisionalBindingId` plus later remapping?
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

- [pending] Decide whether to replace `ProvisionalBindingId` with one local binding id space in the data model.
- [pending] Decide the package-global naming shape and whether stable global ids are part of that contract immediately.
- [pending] Decide whether annotation resolution needs explicit stored ids or only a different traversal strategy.
