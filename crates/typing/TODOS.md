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
- During the scaffolding phase, it is fine to split functionality into different files when that establishes cleaner boundaries.

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
- [x] Use string interning for repeated identifiers and field names in lowering and inference.
- [x] Use an explicit inference-variable state with path compression during unification.

## Phase 1 — Crate scaffolding

References:
- `ARCHITECTURE.md` → Goals
- `ARCHITECTURE.md` → File and module direction
- `ARCHITECTURE.md` → Public API direction

- [x] Reshape the crate to be library-first.
- [ ] Decide whether to keep a tiny binary for local experiments `(needs refinement)`.
- [x] Add a narrow crate entry point for checking source text.
- [x] Add core result types for diagnostics and inferred type information.
- [x] Keep the early file layout modest and avoid over-fragmentation.

### Initial crate structure

- [x] Add a library root.
- [x] Move placeholder executable code out of the way or reduce it to a thin wrapper.
- [ ] Add minimal internal modules only when they support a real abstraction boundary.

## Phase 2 — Test harness and snapshot workflow

References:
- `ARCHITECTURE.md` → Testing strategy
- `ARCHITECTURE.md` → Error handling and diagnostics

- [x] Set up end-to-end tests that operate on R snippets.
- [x] Set up snapshot testing for rendered diagnostics.
- [x] Establish a stable diagnostic rendering format suitable for snapshots.
- [x] Add a small helper API for snippet-based tests.
- [ ] Decide how much inferred type information should appear in snapshots `(needs refinement)`.

### Initial test coverage

- [x] Empty input produces no diagnostics.
- [x] Simple scalar literals produce no diagnostics.
- [x] Undefined names produce diagnostics.
- [ ] Unsupported syntax produces `Unknown` behavior and any intended diagnostics.
- [x] Diagnostic rendering is stable enough to snapshot.

## Phase 3 — Parsing and lowering

References:
- `ARCHITECTURE.md` → Parsing and lowering
- `ARCHITECTURE.md` → Scope of the supported language subset

- [x] Keep tree-sitter parsing separate from semantic lowering.
- [x] Define a lowered internal representation for the supported subset.
- [x] Define source-carrying lowered nodes so diagnostics can point back to original code.
- [x] Introduce interned symbols for names used in lowered syntax.
- [x] Lower top-level sequences.
- [x] Lower symbol references.
- [x] Lower scalar literals.
- [x] Lower assignments.
- [x] Lower function definitions.
- [x] Lower function calls.
- [ ] Lower list-like constructions.
- [ ] Decide whether to lower `if` in the first syntax slice or defer it `(needs refinement)`.

### Parser and lowering tests

- [x] Parse and lower a literal assignment.
- [x] Parse and lower a function definition.
- [x] Parse and lower a function call.
- [ ] Parse and lower list-like constructions.
- [x] Preserve source ranges needed for diagnostics.

## Phase 4 — Type representations

References:
- `ARCHITECTURE.md` → Type model
- `ARCHITECTURE.md` → Recommended internal representation split

- [x] Define `Atomic` categories:
  - [x] `logical`
  - [x] `integer`
  - [x] `double`
  - [x] `complex`
  - [x] `character`
  - [x] `raw`
- [x] Define `SurfaceType`.
- [x] Define `CoreType`.
- [x] Define `TypeScheme`.
- [x] Define inference variable identities.
- [x] Define interned symbol identities.
- [x] Define the interner API and ownership model.
- [ ] Define type environments.
- [ ] Define internal type pretty-printing for diagnostics and debugging.
- [ ] Decide how `Any` and `Unknown` participate in unification in detail `(needs refinement)`.

### Representation invariants

- [x] Distinguish surface annotation syntax from inference internals.
- [x] Support scalar atomics and atomic vectors distinctly.
- [x] Support `List`, `Tuple`, and `Record`.
- [x] Support function types.
- [x] Support inference variables in `CoreType`.
- [x] Support quantified variables in `TypeScheme`.

## Phase 5 — Monomorphic inference core

References:
- `ARCHITECTURE.md` → Hindley–Milner approach
- `ARCHITECTURE.md` → Inference pipeline

- [x] Implement fresh inference variable creation.
- [x] Implement an explicit inference-variable state.
- [x] Implement representative lookup for inference variables.
- [x] Implement path compression during representative lookup.
- [x] Implement occurs checks.
- [x] Implement unification for atomic types.
- [x] Implement unification for function types.
- [x] Implement unification for `List`.
- [x] Implement unification for `Tuple`.
- [x] Implement unification for `Record`.
- [x] Implement inference for literals.
- [x] Implement inference for symbol references.
- [x] Implement inference for assignments.
- [x] Implement inference for function definitions.
- [x] Implement inference for function calls.
- [x] Produce diagnostics for type mismatches.
- [ ] Produce diagnostics for arity mismatches.
- [x] Produce diagnostics for non-callable values used as functions.

### Monomorphic tests

- [x] Infer integer literal types.
- [ ] Infer double literal types.
- [ ] Infer character literal types.
- [x] Infer function parameter and return constraints from usage.
- [x] Report type mismatches in calls.
- [x] Report calling a non-function.
- [x] Report unknown names.
- [x] Snapshot monomorphic diagnostics.

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
- [ ] Key builtin and lexical environments by interned symbols.
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

- [x] Finalize this planning document.
- [x] Reshape the crate toward a library-first structure.
- [x] Add the initial snapshot-based test harness.
- [x] Define the lowered AST shape, including source ranges and interned symbols.
- [x] Define the interner boundary and how diagnostics resolve interned names back to text.
- [x] Implement expression inference over the lowered AST for literals, names, assignments, functions, and calls.
- [x] Connect inference errors to rendered diagnostics.
- [ ] Improve diagnostic precision so type errors point at the failing expression instead of a fallback range.
- [ ] Improve type rendering so diagnostics read like Elm/Rust style messages instead of debug output.
- [ ] Add a persistent `MEMORY.md` document for cross-session context and open design loose ends.