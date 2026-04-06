# Naming Data Model Shape [done]

Reshape naming around document-local facts plus one compact package winner table.

## Outcome

Project 005 is implemented.

Naming now stores:

- document-local binding facts in `NamesLocal`
- one package-global winner table in `NamesGlobal`
- no duplicated package-global binding metadata
- no eagerly materialized package-global `ExpressionKey -> BindingId` resolutions

## Implemented shape

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

## Key decisions now reflected in code

- `BindingId` is document-local.
  - This is required for `run_naming` to rerun only changed documents without rebasing ids in unchanged documents.
- `run_naming` preserves unchanged local naming results and recomputes only changed or missing documents.
- Package naming rebuilds `global_bindings` package-wide from lowered modules plus local exports.
- Cross-file value lookup is derived from:
  - local `non_locals`
  - package `global_bindings`
  - winning document `global_exports`
- Binding metadata has one owner:
  - the defining document's `NamesLocal.bindings`
- Type-side naming work items are annotation-only:
  - `named_type_annotations`
  - top-level definitions are read directly from `Module.definitions`

## Why `global_bindings` is rebuilt for now

`Symbol -> DocumentId` is the right winner-table shape, but it is not enough to support fine-grained incremental maintenance by itself. If the current winner changes or disappears, that table alone does not tell us which earlier exporter should become the new winner.

For project 005 we therefore rebuild `global_bindings` package-wide whenever naming runs:

- simple
- robust
- keeps lookup constant-time after naming
- avoids repeated lazy scans across all document export tables

The future incremental upgrade path is to add a reverse index such as:

```rust
Symbol -> ordered exporters
```

That would let add/change/remove update only affected symbols without changing the higher-level lookup model.

## Tests

The implementation is covered by:

- local naming fixtures
- global naming fixtures
- IDE hover fixtures
- `cargo test -p analysis`
