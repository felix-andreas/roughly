# Typing Crate Discussion

Move settled points into `DECISION_LOG.md`.

## Current topic

State management for multi-file and incremental analysis, with the explicit goal of making fixture tests small and phase-shaped.

## Goal

The fixture suite should use normal typing-crate APIs.
Conceptually, the target shape is still:

```rust
let mut session = fixture_session(fixture)?;
run_lowering(&session.workspace, &mut session.analysis)?;
let naming = run_naming(&session.workspace, &mut session.analysis)?;
```

That means:

- fixture helpers only create or update a `Workspace`
- analysis owns cached semantic state
- tests call phases explicitly

## Current direction

- `Workspace` stays outside `AnalysisState`
- `AnalysisState` stores derived semantic state, not ropes or trees
- the likely top-level owner is `ProjectSession { workspace, analysis }`
- walkers such as lowering and naming contexts stay transient
- project file order is defined by `Workspace`
- naming results should be reusable for tooling such as go-to-definition and rename

## Working answers

### `AnalysisState`

Use one top-level `AnalysisState` with per-phase caches inside it rather than several unrelated top-level state objects.

Pseudocode:

```rust
struct AnalysisState {
    interner: Interner,
    lowering: LoweringState,
    naming: NamingState,
}
```

This keeps invalidation and cross-phase reuse in one place while preserving phase boundaries.

### Persistent data

Long-lived data should include:

- shared interner
- per-document lowered results
- project-global declaration tables
- naming results
- invalidation metadata

Here, invalidation metadata means the bookkeeping that says which cached phase results are stale after a document update and which later phases need recomputation.

### First durable project-level artifact

The first durable project-level artifact after parsing should be:

- per-document lowered results
- plus project-level indexes derived from them

Those indexes do not start only after naming.
Lowering already gives enough information to build project-level lowered indexes such as document order and top-level declarations.

### Merged project view

For now, a merged project-level lowered view should be derived temporarily rather than stored as the primary durable representation.

That suggests:

- store per-document lowered results durably
- derive any merged or ordered project view inside analysis when needed

### Phase calls in tests

Tests should call prerequisite phases explicitly.
That keeps the fixture shape honest about the phase pipeline while still keeping tests small.

### `LoweringContext`

Beyond shared interning, `LoweringContext` should mostly remain transient.
Durable lowered results should move into explicit cached analysis structures instead of staying owned implicitly by `LoweringContext`.

## Open decisions

- For project-global naming, should naming grow a true multi-document API now, or should analysis build a temporary merged project view for naming as the first migration step?
  - current recommendation: use a temporary merged project view first, as long as the merge logic moves into analysis code and out of the fixture harness

## Constraints

- the current merge or remap logic must move out of the fixture harness
- the design must support repeated analysis after fixture generations
- the design should leave room for later editor-style incremental updates
