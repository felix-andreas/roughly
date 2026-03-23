# Hover Types and Incremental Analysis

## Current crate state

- Lowering already assigns every lowered expression a stable `ExpressionId` and a source `Range`.
- Inference already maintains type state for inference variables and lexical bindings.
- The checker does not currently retain successful inferred types per expression after checking finishes.
- The crate is currently file-local. It does not yet have a project-level import or dependency model.

## Hover type data

### Recommendation

Use lowered expression identity as the primary semantic key:

- `ExpressionId` for semantic tables
- `Range` for position lookup
- optional parser provenance such as a tree-sitter node id only as auxiliary metadata

### Why

- `ExpressionId` matches the lowered IR and inference pipeline.
- A tree-sitter node id is useful for syntax provenance, but lowering is not a strict 1:1 mapping from parser nodes to semantic expressions.
- Some lowered nodes are synthetic.
- Some parser wrappers are normalized away during lowering.

### Recommended storage strategy

Use a hybrid model.

Keep globally:

- exported interfaces for all analyzed files
- dependency and invalidation data
- source snapshot identity such as a version or content hash

Keep locally only for open files, hot files, or active hover requests:

- lowered `Module`
- `expression_types`, indexed by `ExpressionId`
- diagnostics

This avoids retaining expression-level type data for the entire workspace while still supporting precise hover for the files the user is actively editing.

### Hover computation strategy

Do not try to infer only the hovered expression in isolation.

Instead:

- use cached expression-level analysis for open or recently analyzed files
- otherwise re-run local analysis for the current file, or at least the enclosing top-level definition
- typecheck that local unit against cached project-level exported interfaces

Reasons:

- expression types often depend on surrounding bindings and prior unification
- the global environment alone is not enough for arbitrary nested expressions
- reanalyzing one file is usually simpler than reconstructing a precise local environment for one arbitrary node

### Representation

For files that do retain expression-level results, the initial representation can be simple:

- `Vec<CoreType>` or `Vec<Option<CoreType>>`, indexed by `ExpressionId.0`

If memory later matters, move to:

- `Vec<TypeId>`
- a compact interned type arena behind `TypeId`

### Resolution timing

Do not assume the first type produced for an expression is final.

Recommended approach:

- record the raw `CoreType` result for each expression during inference
- resolve all recorded types after inference completes

This preserves later refinements introduced by unification.

### Coverage caveats

Expression hover and definition hover are different problems.

Current lowering does not make every hover target a lowered expression:

- function parameters have ranges, but not `ExpressionId`
- assignment targets are stored as symbols, not as separate lowered expressions

If hover should cover definitions as well as expressions, add explicit definition-level tables instead of overloading expression tables.

## Tree-sitter ids

### Recommendation

If tree-sitter ids are stored, use them only as auxiliary source provenance.

Good uses:

- mapping current syntax nodes back to lowered expressions
- parser-facing caches
- incremental-lowering hints

Do not use them as the primary key for:

- inference state
- expression type tables
- project-level semantic dependency tracking

### Why not rely on them for hover

Hover results belong to a specific analyzed source snapshot.

If type checking runs only on save, then after unsaved edits:

- current editor positions may not match saved ranges
- syntax identity alone does not make the semantic analysis current

So hover should be tied to a document version or content hash, not only to node ids.

An on-demand local reanalysis for the current file also helps here, because hover can be answered against the current unsaved buffer instead of only against the last saved typecheck snapshot.

## Incremental checking across files

### Baseline model

Use dependency tracking plus interface invalidation.

Per file:

- parse and lower
- check and infer
- compute an exported interface
- hash that exported interface

When file `A` changes:

1. reanalyze `A`
2. recompute `A`'s exported interface hash
3. if the interface hash changed, invalidate dependents of `A`
4. reanalyze invalidated dependents transitively

This handles cases where a function in `A` changes type and a use site in `B` must now fail.

### Interface vs implementation

Track these separately:

- file content hash
- exported interface hash

Not every edit in `A` should force rechecking `B`.

Examples:

- whitespace-only change: content changes, interface does not
- body change with unchanged exported type: implementation changes, interface may not
- return type change: interface changes, dependents must be rechecked

## Granularity

### File-level invalidation

Pros:

- simple
- good first implementation

Cons:

- rechecks too much

### Definition-level invalidation

Recommended longer-term direction.

Track dependencies between definitions such as:

- functions
- exported bindings
- type aliases
- nominal types

Then invalidate only dependents of the changed definition or exported symbol.

This is the likely sweet spot for the crate once project-level analysis exists.

### Expression-level invalidation

Possible, but likely too much machinery for an early implementation.

## SCCs

`SCC` means strongly connected component.

In a dependency graph, an SCC is a maximal set of nodes where each node is reachable from every other node.

Example:

- `A::foo` depends on `B::bar`
- `B::bar` depends on `A::foo`

Then `foo` and `bar` are in the same SCC.

Why this matters:

- acyclic dependencies can be scheduled in topological order
- cyclic dependencies cannot
- recursive groups should be analyzed as one unit

For incremental checking, use SCCs as the scheduling unit once definition-level dependencies exist.

## Recommended implementation order

1. Keep `ExpressionId` as the primary semantic identity.
2. Introduce a project-level analysis layer above the current file-local checker.
3. Store exported interfaces and dependency data for all files.
4. Tag analysis results with a source version or content hash.
5. Compute and retain expression-level hover data only for open files, hot files, or active hover requests.
6. For non-cached files, answer hover by reanalyzing the current file or enclosing top-level definition against cached project interfaces.
7. Start with file-level dependency invalidation based on exported interface hashes.
8. Move to definition-level dependency tracking if file-level invalidation becomes too coarse.
9. Use SCCs for recursive dependency groups.
