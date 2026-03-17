# `typing` TODOs

This document tracks the implementation plan for the `typing` crate.

While `SEMANTICS.md` is being refined into the authoritative user-facing contract, parts of this plan are somewhat stale and should be treated carefully until they are rewritten to match the current semantics.

Keep it aligned with `crates/typing/SEMANTICS.md` first and `crates/typing/ARCHITECTURE.md` second while that refinement is in progress.

## Document hygiene

Keep this document high signal.

- Prefer concise, actionable todos over status narration.
- Remove or rewrite stale items when implementation changes.
- Keep durable design rules in `ARCHITECTURE.md`, not here.
- Keep cross-session handoff details in `MEMORY.md`, not here.
- When a task is done, either check it off or delete it if it no longer helps future work.

## Planning rules

- Important design decisions must be discussed with the user before implementation.
- During semantics refinement, treat `SEMANTICS.md` as the authority when this file and `ARCHITECTURE.md` lag behind it.
- Todos may reference sections of `ARCHITECTURE.md`.
- Prefer hierarchical todos.
- If the exact implementation steps are unclear, mark the todo with `(needs refinement)`.
- When work reaches a todo marked `(needs refinement)`, discuss it with the user before proceeding.
- If implementation changes make this document or `ARCHITECTURE.md` inaccurate, update both in the same session.

## Phase 0 — Planning and alignment

- [ ] Review `README.md`, `SEMANTICS.md`, `ARCHITECTURE.md`, and this document together before each major implementation phase.
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
- [x] Prefer fixture-based tests of rendered diagnostics.
- [x] Use string interning for repeated identifiers and field names in lowering and inference.
- [x] Use an explicit inference-variable state with path compression during unification.

## Phase 1 — Crate scaffolding

References:
- `ARCHITECTURE.md` → Goals
- `ARCHITECTURE.md` → File and module direction
- `ARCHITECTURE.md` → Public API direction

- [x] Reshape the crate to be library-first.
- [x] Do not keep a tiny binary for local experiments.
- [x] Add a narrow crate entry point for checking source text.
- [x] Add core result types for diagnostics and inferred type information.
- [x] Keep the early file layout modest and avoid over-fragmentation.

### Initial crate structure

- [x] Add a library root.
- [x] Move placeholder executable code out of the way or reduce it to a thin wrapper.
- [ ] Add minimal internal modules only when they support a real abstraction boundary.

## Phase 2 — Test harness and diagnostic fixtures

References:
- `ARCHITECTURE.md` → Testing strategy
- `ARCHITECTURE.md` → Error handling and diagnostics

- [x] Set up end-to-end tests that operate on R snippets.
- [x] Set up fixture-based testing for rendered diagnostics.
- [x] Establish a stable diagnostic rendering format suitable for fixture expectations.
- [x] Add a small helper API for snippet-based tests.
- [x] Add a fixture-based test harness for grouped R snippet cases with stable `group__case` identities.
- [ ] Decide how much inferred type information should appear in rendered diagnostic fixtures `(needs refinement)`.
- [x] Document the preferred fixture update/review workflow for this crate.

### Initial test coverage

- [x] Empty input produces no diagnostics.
- [x] Simple scalar literals produce no diagnostics.
- [x] Undefined names produce diagnostics.
- [x] Rely on the existing syntax checker to reject invalid syntax before typing runs.
- [x] Builtin arithmetic diagnostics cover `+` rejecting non-numeric operands.
- [x] Builtin arithmetic fixture expectations cover scalar/vector combinations for `+`.
- [x] Diagnostic rendering is stable enough for fixture expectations.
- [x] Grouped fixture files can define multiple cases with stable `group__case` identities.
- [x] Duplicate `group__case` identities are rejected by the harness.
- [x] Arity mismatch fixture expectations cover the actual argument count after lowering.

## Phase 3 — Parsing and lowering

References:
- `SEMANTICS.md` → Types
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
- [x] Avoid double-counting call arguments during lowering.
- [x] Lower `+` into builtin call form.
- [ ] Lower list-like constructions in line with `SEMANTICS.md`.
- [ ] Decide whether to lower `if` in the first syntax slice or defer it `(needs refinement)`.
- [ ] Keep `SEMANTICS.md` in sync with list-related fixture tests; both are part of the contract.

### Parser and lowering tests

- [x] Parse and lower a literal assignment.
- [x] Parse and lower a function definition.
- [x] Parse and lower a function call.
- [x] Regress call lowering so multi-argument calls do not duplicate wrapper nodes as extra arguments.
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
- [ ] Add a short file-to-responsibility map so contributors know where to update types, lowering, inference, and diagnostics.
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
- [x] Produce diagnostics for arity mismatches.
- [x] Produce diagnostics for non-callable values used as functions.

### Monomorphic tests

- [x] Infer integer literal types.
- [x] Infer double literal types.
- [x] Infer character literal types.
- [x] Infer function parameter and return constraints from usage.
- [x] Report type mismatches in calls.
- [x] Report calling a non-function.
- [x] Report unknown names.
- [x] Report arity mismatches with the actual lowered argument count.
- [x] Fixture-based monomorphic diagnostics.

## Phase 6 — Let-polymorphism and internal generics

References:
- `ARCHITECTURE.md` → Hindley–Milner approach
- `ARCHITECTURE.md` → No explicit generics syntax in v1

- [x] Implement free type variable computation.
- [x] Implement generalization at bindings.
- [x] Implement instantiation at use sites.
- [x] Ensure generalized bindings get fresh variables per use.
- [x] Verify that polymorphic bindings work across repeated calls.
- [x] Add initial builtin support for `+` arithmetic and `c(...)` vector construction used by arithmetic tests.
- [ ] Keep explicit generic annotation syntax deferred.

### Polymorphism tests

- [x] `identity` works at `integer` and `character` in one snippet.
- [x] Higher-order apply-style examples infer correctly.
- [x] Repeated instantiations do not leak constraints across use sites.
- [x] Fixture-based polymorphism diagnostics for failure cases.

## Phase 7 — Lists, tuples, and records

References:
- `SEMANTICS.md` → Types
- `ARCHITECTURE.md` → Lists, tuples, and records

- [ ] Infer positional `list(...)` expressions as tuple-like values, including `list()` as the empty tuple-like case.
- [ ] Infer named `list(...)` expressions as map-like values.
- [ ] Emit a type error for mixed named and unnamed entries.
- [ ] Add coercion from tuple-like `list(...)` values into array-like `list[...]` targets.
- [ ] Add coercion from map-like `list(...)` values into homogeneous map-like `list[key: value]` targets.
- [ ] Reject coercion from array-like values into tuple-like targets.
- [ ] Reject coercion from map-like values into fixed-shape record-like targets.

### Container tests

- [ ] `list()` infers as the empty tuple-like case.
- [ ] Positional `list(1L, 2L, 3L)` infers as a tuple-like value.
- [ ] Named `list(foo = 1L, bar = "foo")` infers as a map-like value.
- [ ] Mixed `list(1L, bar = "foo")` reports a type error.
- [ ] `#: list[integer]` accepts tuple-like `list(1L, 2L, 3L)`.
- [ ] `#: list[character: integer]` accepts named `list(foo = 1L, bar = 2L)`.

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
- [ ] Fixture-based diagnostics for malformed annotations.

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

- [ ] Keep unsupported lowered forms from aborting inference when they appear inside otherwise valid syntax.
- [ ] Ensure `Unknown` reduces cascading diagnostics.
- [ ] Keep the behavior consistent and snapshot-tested where supported syntax can still lower to `Unsupported`.

### Unsupported syntax tests

- [ ] Downstream inference continues after lowered unsupported constructs.
- [ ] Fixture coverage pins the current `Unknown` behavior for lowered unsupported constructs that can still arise from syntactically valid input.

## Phase 11 — Builtin environment

References:
- `ARCHITECTURE.md` → Builtin environment

- [ ] Start with a minimal builtin environment.
- [ ] Key builtin and lexical environments by interned symbols.
- [ ] Add builtins only when required by tests.
- [ ] Discuss semantics with the user before adding nontrivial builtins.
- [ ] Decide the first builtin set `(needs refinement)`.

### Candidate early builtins

- [x] `+`
- [x] `c(...)` for numeric vector construction in arithmetic tests
- [ ] Comparison operators `(needs refinement)`
- [ ] `list(...)` `(needs refinement)`
- [ ] `length(...)` `(needs refinement)`

## Phase 12 — Public API shaping

- [ ] Unify `typing` diagnostics with the current syntax-checking pipeline in `roughly` so syntax errors still come from the existing checker and type checking runs only on syntactically valid input.

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

- [ ] Improve diagnostic precision so type errors point at the failing expression instead of a fallback range.
- [ ] Improve type rendering so diagnostics read like Elm/Rust style messages instead of debug output.
- [ ] Improve `Unknown` behavior coverage for lowered unsupported constructs that can still arise from syntactically valid input.
- [ ] Lower list-like constructions.