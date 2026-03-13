# `typing` TODOs

This document tracks the implementation plan for the `typing` crate.

It is a living planning document and should be kept in sync with `crates/typing/ARCHITECTURE.md`.

## Planning rules

- Important design decisions must be discussed with the user before implementation.
- Todos may reference sections of `ARCHITECTURE.md`.
- Hierarchical todos are preferred.
- If the exact implementation steps are unclear, mark the todo with `(needs refinement)`.
- When work reaches a todo marked `(needs refinement)`, discuss it with the user before proceeding.
- If implementation changes make this document or `ARCHITECTURE.md` inaccurate, update the documents in the same session.

## Phase 0 — Planning and alignment

- [ ] Review `README.md`, `ARCHITECTURE.md`, and this document together before each major implementation phase.
- [ ] Keep the scope aligned with the current v1 goals in `ARCHITECTURE.md`.
- [ ] Discuss any important semantic change with the user before implementation.
- [ ] Keep open questions visible until they are resolved or deliberately deferred.

### Decision log checkpoints

- [x] Distinguish `integer` and `double` from the beginning.
- [x] Use these list-shape rules:
  - homogeneous positional => `List`
  - heterogeneous positional => `Tuple`
  - named => `Record`
  - mixed named and unnamed => type error
- [x] Infer `Unknown` for unsupported syntax.
- [x] Follow the staged HM plan:
  - monomorphic inference first
  - let-polymorphism immediately after
- [x] Support internal generics in v1.
- [x] Do not add explicit generics syntax in v1.
- [x] Use R snippets as the primary test input format.
- [x] Prefer snapshot tests of rendered diagnostics.

## Phase 1 — Crate scaffolding

References:
- `ARCHITECTURE.md` → Goals
- `ARCHITECTURE.md` → File and module direction
- `ARCHITECTURE.md` → Public API direction

- [ ] Reshape the crate to be library-first.
- [ ] Decide whether to keep a tiny binary for local experiments `(needs refinement)`.
- [ ] Add a narrow crate entry point for checking source text.
- [ ] Add core result types for diagnostics and inferred type information.
- [ ] Keep the early file layout modest and avoid over-fragmentation.

### Initial crate structure

- [ ] Add a library root.
- [ ] Move placeholder executable code out of the way or reduce it to a thin wrapper.
- [ ] Add minimal internal modules only when they support a real abstraction boundary.

## Phase 2 — Test harness and snapshot workflow

References:
- `ARCHITECTURE.md` → Testing strategy
- `ARCHITECTURE.md` → Error handling and diagnostics

- [ ] Set up end-to-end tests that operate on R snippets.
- [ ] Set up snapshot testing for rendered diagnostics.
- [ ] Establish a stable diagnostic rendering format suitable for snapshots.
- [ ] Add a small helper API for snippet-based tests.
- [ ] Decide how much inferred type information should appear in snapshots `(needs refinement)`.

### Initial test coverage

- [ ] Empty input produces no diagnostics.
- [ ] Simple scalar literals produce no diagnostics.
- [ ] Undefined names produce diagnostics.
- [ ] Unsupported syntax produces `Unknown` behavior and any intended diagnostics.
- [ ] Diagnostic rendering is stable enough to snapshot.

## Phase 3 — Parsing and lowering

References:
- `ARCHITECTURE.md` → Parsing and lowering
- `ARCHITECTURE.md` → Scope of the supported language subset

- [ ] Keep tree-sitter parsing separate from semantic lowering.
- [ ] Define a lowered internal representation for the supported subset.
- [ ] Lower top-level sequences.
- [ ] Lower symbol references.
- [ ] Lower scalar literals.
- [ ] Lower assignments.
- [ ] Lower function definitions.
- [ ] Lower function calls.
- [ ] Lower list-like constructions.
- [ ] Decide whether to lower `if` in the first syntax slice or defer it `(needs refinement)`.

### Parser and lowering tests

- [ ] Parse and lower a literal assignment.
- [ ] Parse and lower a function definition.
- [ ] Parse and lower a function call.
- [ ] Parse and lower list-like constructions.
- [ ] Preserve source ranges needed for diagnostics.

## Phase 4 — Type representations

References:
- `ARCHITECTURE.md` → Type model
- `ARCHITECTURE.md` → Recommended internal representation split

- [ ] Define `Atomic` categories:
  - [ ] `logical`
  - [ ] `integer`
  - [ ] `double`
  - [ ] `complex`
  - [ ] `character`
  - [ ] `raw`
- [ ] Define `SurfaceType`.
- [ ] Define `CoreType`.
- [ ] Define `TypeScheme`.
- [ ] Define inference variable identities.
- [ ] Define type environments.
- [ ] Define internal type pretty-printing for diagnostics and debugging.
- [ ] Decide how `Any` and `Unknown` participate in unification in detail `(needs refinement)`.

### Representation invariants

- [ ] Distinguish surface annotation syntax from inference internals.
- [ ] Support scalar atomics and atomic vectors distinctly.
- [ ] Support `List`, `Tuple`, and `Record`.
- [ ] Support function types.
- [ ] Support inference variables in `CoreType`.
- [ ] Support quantified variables in `TypeScheme`.

## Phase 5 — Monomorphic inference core

References:
- `ARCHITECTURE.md` → Hindley–Milner approach
- `ARCHITECTURE.md` → Inference pipeline

- [ ] Implement fresh inference variable creation.
- [ ] Implement substitutions or an equivalent unification state.
- [ ] Implement occurs checks.
- [ ] Implement unification for atomic types.
- [ ] Implement unification for function types.
- [ ] Implement unification for `List`.
- [ ] Implement unification for `Tuple`.
- [ ] Implement unification for `Record`.
- [ ] Implement inference for literals.
- [ ] Implement inference for symbol references.
- [ ] Implement inference for assignments.
- [ ] Implement inference for function definitions.
- [ ] Implement inference for function calls.
- [ ] Produce diagnostics for type mismatches.
- [ ] Produce diagnostics for arity mismatches.
- [ ] Produce diagnostics for non-callable values used as functions.

### Monomorphic tests

- [ ] Infer integer literal types.
- [ ] Infer double literal types.
- [ ] Infer character literal types.
- [ ] Infer function parameter and return constraints from usage.
- [ ] Report type mismatches in calls.
- [ ] Report calling a non-function.
- [ ] Report unknown names.
- [ ] Snapshot monomorphic diagnostics.

## Phase 6 — Let-polymorphism and internal generics

References:
- `ARCHITECTURE.md` → Hindley–Milner approach
- `ARCHITECTURE.md` → No explicit generics syntax in v1

- [ ] Implement free type variable computation.
- [ ] Implement generalization at bindings.
- [ ] Implement instantiation at use sites.
- [ ] Ensure generalized bindings get fresh variables per use.
- [ ] Verify that polymorphic bindings work across repeated calls.
- [ ] Keep explicit generic annotation syntax deferred.

### Polymorphism tests

- [ ] `identity` works at `integer` and `character` in one snippet.
- [ ] Higher-order apply-style examples infer correctly.
- [ ] Repeated instantiations do not leak constraints across use sites.
- [ ] Snapshot polymorphism diagnostics for failure cases.

## Phase 7 — Lists, tuples, and records

References:
- `ARCHITECTURE.md` → Lists, tuples, and records

- [ ] Infer homogeneous positional list-like expressions as `List`.
- [ ] Infer heterogeneous positional list-like expressions as `Tuple`.
- [ ] Infer named entries as `Record`.
- [ ] Emit a type error for mixed named and unnamed entries.
- [ ] Decide how empty list-like constructs should be typed `(needs refinement)`.
- [ ] Decide which R syntax maps to these constructions in the first implementation slice `(needs refinement)`.

### Container tests

- [ ] Homogeneous positional example infers as `List`.
- [ ] Heterogeneous positional example infers as `Tuple`.
- [ ] Named example infers as `Record`.
- [ ] Mixed named and unnamed example reports a type error.
- [ ] Snapshot diagnostics for malformed container expressions.

## Phase 8 — Annotation parsing

References:
- `ARCHITECTURE.md` → Annotation model
- `ARCHITECTURE.md` → Recommended internal representation split

- [ ] Parse `#:` comment lines.
- [ ] Attach variable annotations to assignments.
- [ ] Attach parameter annotations to functions.
- [ ] Attach return annotations to functions.
- [ ] Parse scalar atomic annotation syntax.
- [ ] Parse vector annotation syntax.
- [ ] Parse `List` annotation syntax.
- [ ] Parse `Tuple` annotation syntax.
- [ ] Parse `Record` annotation syntax.
- [ ] Parse function annotation syntax.
- [ ] Decide how strict annotation attachment rules should be `(needs refinement)`.

### Annotation tests

- [ ] Variable annotation matches assigned expression.
- [ ] Variable annotation mismatch reports a diagnostic.
- [ ] Parameter annotation constrains function inputs.
- [ ] Return annotation constrains function outputs.
- [ ] Snapshot diagnostics for malformed annotations.

## Phase 9 — Applying annotations during inference

References:
- `ARCHITECTURE.md` → Inference pipeline

- [ ] Convert `SurfaceType` into `CoreType` constraints.
- [ ] Apply variable annotations during assignment checking.
- [ ] Apply parameter annotations during function checking.
- [ ] Apply return annotations during function checking.
- [ ] Decide whether annotations behave as exact constraints or partially trusted assertions `(needs refinement)`.

### Annotation enforcement tests

- [ ] Good annotations produce no diagnostics.
- [ ] Bad annotations produce readable mismatch diagnostics.
- [ ] `Any` works as an explicit escape hatch.
- [ ] `Unknown` remains distinct from `Any` in diagnostics and behavior.

## Phase 10 — Unsupported syntax and `Unknown`

References:
- `ARCHITECTURE.md` → Unsupported constructs degrade to `Unknown`

- [ ] Define which unsupported constructs only infer `Unknown`.
- [ ] Define which unsupported constructs also emit diagnostics `(needs refinement)`.
- [ ] Ensure unsupported constructs do not cause inference to abort.
- [ ] Ensure `Unknown` reduces cascading diagnostics.
- [ ] Keep the behavior consistent and snapshot-tested.

### Unsupported syntax tests

- [ ] Unsupported expression yields stable behavior.
- [ ] Downstream inference continues after unsupported syntax.
- [ ] Snapshot diagnostics for unsupported syntax cases.

## Phase 11 — Builtin environment

References:
- `ARCHITECTURE.md` → Builtin environment

- [ ] Start with a minimal builtin environment.
- [ ] Add builtins only when required by tests.
- [ ] Discuss semantics with the user before adding nontrivial builtins.
- [ ] Decide the first builtin set `(needs refinement)`.

### Candidate early builtins

- [ ] Arithmetic operators `(needs refinement)`
- [ ] Comparison operators `(needs refinement)`
- [ ] `c(...)` `(needs refinement)`
- [ ] `list(...)` `(needs refinement)`
- [ ] `length(...)` `(needs refinement)`

## Phase 12 — Public API shaping

References:
- `ARCHITECTURE.md` → Public API direction
- `ARCHITECTURE.md` → Integration plan for `roughly`

- [ ] Decide the first stable library entry point `(needs refinement)`.
- [ ] Decide what inferred information is returned in addition to diagnostics `(needs refinement)`.
- [ ] Keep the public API small while the checker is still evolving.
- [ ] Revisit the API before integration into `roughly`.

## Phase 13 — Integration preparation

References:
- `ARCHITECTURE.md` → Integration plan for `roughly`

- [ ] Identify the minimum interface needed by `roughly` `(needs refinement)`.
- [ ] Keep standalone tests strong before integrating.
- [ ] Add integration-oriented checks only after the standalone core is trustworthy.
- [ ] Update both planning documents when integration requirements become clearer.

## Ongoing maintenance

- [ ] Keep `ARCHITECTURE.md` current with implementation.
- [ ] Keep this document current with implementation progress.
- [ ] Convert resolved `(needs refinement)` items into concrete tasks after discussion.
- [ ] Remove stale todos that no longer reflect the plan.
- [ ] Add new tasks only when they reflect real planned work.

## Current next steps

- [ ] Finalize this planning document.
- [ ] Reshape the crate toward a library-first structure.
- [ ] Add the initial snapshot-based test harness.
- [ ] Discuss the first executable syntax slice if needed before implementation starts.