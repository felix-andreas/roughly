# `typing` Architecture

This document describes the planned architecture for the `typing` crate.

The crate starts as a standalone library for static type checking of a subset of R. Later it should be integrated into `roughly`, but the standalone library remains the primary implementation boundary while the type checker is still evolving.

## Status

This document is a living design document.

If implementation changes make this document inaccurate, it should be updated in the same session as the code change. If the implementation and this document disagree, that mismatch should be treated as work to do, not as an acceptable state.

## Collaboration process

This crate is developed collaboratively with the user.

Important design decisions must be discussed with the user before implementation. If a planned step is ambiguous, under-specified, or introduces a meaningful semantic tradeoff, stop and discuss it first.

Project planning is tracked separately in `crates/typing/TODOS.md`.

Rules for planning:

- Hierarchical todos are preferred.
- Todos may reference sections of this document.
- If the exact implementation steps are not yet clear, mark the todo with `(needs refinement)`.
- When work reaches a todo marked `(needs refinement)`, discuss it with the user before proceeding.
- As implementation evolves, keep both this document and `TODOS.md` up to date.

## Goals

The initial goals are:

- Build a standalone Rust library for type checking R code.
- Use Hindley–Milner style inference as the foundation.
- Support internal polymorphism in v1.
- Do not require explicit generics syntax in v1.
- Develop the checker test-first.
- Use R snippets as the primary test input format.
- Prefer snapshot tests of rendered diagnostics for end-to-end behavior.
- Support later integration into `roughly` without redesigning the core engine.

## Non-goals for v1

The first version should not attempt to model all of R.

Explicit non-goals for v1:

- Full coverage of base R syntax and semantics
- S3 dispatch modeling
- S4 dispatch modeling
- NSE and metaprogramming completeness
- Environment and reference semantics
- Union types
- Subtyping
- Variance
- Nominal typing
- Exhaustiveness checking
- Attribute-sensitive typing beyond what is strictly required for supported constructs

These may be added later, but the initial checker should stay focused on a small, principled subset.

## Design principles

### Small HM core first

The type checker should be built around a small inference core with:

- type variables
- unification
- instantiation
- generalization
- type schemes

Even though explicit generics syntax is deferred, internal polymorphism is part of v1.

### Separate user syntax from inference internals

The architecture should distinguish between:

- parsed R syntax
- parsed type annotation syntax
- internal inference types
- generalized type schemes

User-facing types and internal inference types should not be conflated.

### Value-like model for the supported subset

R is not globally immutable, but the supported subset should be treated as value-like for the initial checker.

This means the initial design focuses on ordinary expressions, vectors, lists, records, tuples, and functions. Mutable or reference-oriented parts of R should be deferred and handled as unsupported syntax or `Unknown`.

### Equality-based typing in v1

The first implementation should use equality-based unification, not subtyping.

This keeps the inference engine simpler and preserves principal typing more naturally than mixing HM with structural subtyping from the start.

### Unsupported constructs degrade to `Unknown`

When the checker encounters unsupported syntax, it should infer `Unknown` rather than failing catastrophically.

Whether every unsupported construct also emits a diagnostic can be refined over time, but the default behavior is to preserve forward progress and avoid cascades.

## Scope of the supported language subset

The initial supported subset should be intentionally small.

Planned early support:

- scalar literals
- symbol references
- assignments
- function definitions
- function calls
- top-level sequences
- list-like constructions with the rules described below

Likely early follow-up support:

- `if` expressions or statements
- blocks
- simple builtins needed for tests

Deferred until later:

- indexing
- replacement functions
- formulas
- promises
- `...`
- environments
- object systems
- dispatch-heavy builtins
- advanced control flow

## Type model

The checker needs to model both R-shaped types and HM inference variables.

### Base categories

The initial type space should include:

- `Any`
- `Unknown`
- `Null`
- scalar atomics
- atomic vectors
- homogeneous lists
- records
- tuples
- function types
- inference variables

### Atomic types

The initial atomic categories should be kept distinct:

- `logical`
- `integer`
- `double`
- `complex`
- `character`
- `raw`

This crate should distinguish `integer` and `double` from the start.

### Scalars and vectors

The type system should distinguish between:

- scalar atomic values
- atomic vectors

This follows the design in `README.md` and matches an important practical distinction in R.

### Lists, tuples, and records

The current intended rules are:

- homogeneous positional list-like values infer as `List`
- heterogeneous positional list-like values infer as `Tuple`
- named entries infer as `Record`
- mixed named and unnamed entries are a type error

This rule should be documented in diagnostics and tested explicitly.

### `Any`

`Any` is an explicit opt-out from type checking.

It should behave as a permissive boundary in type checking and should exist because the programmer asked for it, not because inference failed.

### `Unknown`

`Unknown` represents a type that the checker could not infer or a construct the checker does not yet support.

It should allow inference to continue and reduce error cascades. It is not the same as `Any`, because it represents missing knowledge rather than an explicit escape hatch.

## Recommended internal representation split

The implementation should use at least three layers of type representation.

### `SurfaceType`

This is the type syntax parsed from comments and annotations.

Examples include:

- `integer`
- `double`
- `character[]`
- `list[integer]`
- `list{name: character, age: integer}`
- `list(character, integer)`
- `fn(character, age: integer)`

### `CoreType`

This is the internal inference representation.

It should include all of the semantic forms above, plus inference variables.

### `TypeScheme`

This represents generalized bindings.

A type scheme contains:

- quantified type variables
- a `CoreType` body

This is required for let-polymorphism.

## Hindley–Milner approach

The checker should use a standard HM-style inference pipeline.

### Monomorphic inference first, then generalization

The implementation plan should first establish:

- fresh type variables
- constraint generation
- unification
- occurs checks
- type environments

Once that is stable, the next step is let-polymorphism via:

- generalization at bindings
- instantiation at use sites

This recommendation should be followed in the implementation plan.

### Why let-polymorphism matters

Without let-polymorphism, a binding such as an identity function can only be used at one inferred type.

With let-polymorphism, the checker can infer a generalized binding and instantiate it independently at each use site.

That behavior is central to the planned HM design and is part of v1.

### No explicit generics syntax in v1

Even though v1 supports internal polymorphism, explicit generic syntax in annotations is deferred.

That means generic behavior exists in inference and schemes, but users do not yet write parameterized type syntax directly.

## Annotation model

The current plan is to use comment-based annotations rather than extending R syntax.

The annotation mechanism should remain based on `#:` comments.

Planned annotation support:

- variable type hints
- function parameter annotations
- function return annotations

Deferred annotation support:

- explicit generics syntax
- nominal type declarations
- advanced type aliases
- union syntax

The annotation parser should remain separate from the HM inference engine.

## Parsing and lowering

The checker should not perform inference directly over raw parser nodes.

Instead, parsing should be split into:

- syntax parsing
- annotation extraction
- lowering into an internal representation suitable for inference

Reasons for this separation:

- parser node APIs are awkward for repeated semantic traversals
- tests become easier if the lowered representation is stable
- the inference engine should not depend on tree traversal details

The internal lowered representation does not need to mirror all of R. It only needs to encode the supported subset cleanly.

## Inference pipeline

The planned pipeline is:

1. Parse R source.
2. Extract and parse adjacent `#:` annotations.
3. Lower supported syntax into an internal expression/module representation.
4. Infer types over the lowered representation.
5. Apply annotation constraints where present.
6. Render diagnostics and inferred results.

Each stage should have its own tests where practical.

## Builtin environment

The checker will need an initial environment for builtins and operators.

This should start small and grow only as required by tests.

The initial builtin environment should contain only the symbols needed for the supported language subset and current test cases. It should not attempt to model all of base R up front.

If a builtin has semantics that are unclear or have important design consequences, discuss that with the user before implementing it.

## Error handling and diagnostics

Diagnostics are part of the product, not an afterthought.

Each diagnostic should aim to include:

- a source range
- a short summary
- enough type detail to explain the mismatch
- stable rendering suitable for snapshot tests

The rendered form of diagnostics should be treated as an interface used by tests.

As a result:

- changes to wording should be intentional
- snapshot tests should be updated only when the new wording is better or semantics changed
- diagnostics should prefer clarity over theory-heavy terminology

## Testing strategy

This crate should be developed test-first.

### Primary test style

The primary end-to-end tests should operate on R snippets.

Rendered diagnostics should be snapshot-tested.

This keeps tests close to the user-facing behavior and makes the system easier to evolve collaboratively.

### Secondary test style

Focused unit tests are still useful for internal pieces such as:

- unification
- occurs checks
- instantiation
- generalization
- annotation parsing

These tests are secondary to the snippet-based end-to-end tests, not a replacement for them.

### Why snapshots

Snapshot tests are a good fit because they capture:

- the presence or absence of errors
- source ranges
- diagnostic wording
- behavior shifts across refactors

This is especially valuable while the language subset and diagnostics are still being designed.

## File and module direction

The implementation should remain modest in structure early on.

Do not over-fragment the crate before the abstractions stabilize. Prefer a small number of files with clear responsibilities, and split further only when needed.

The likely conceptual areas are:

- syntax and lowering
- annotation parsing
- type representations
- inference
- diagnostics
- tests

These are architectural areas, not a required file layout.

## Public API direction

The standalone library API should stay narrow.

The eventual public API should center around checking source text and returning a structured result with diagnostics and inferred type information.

The exact API surface can be refined during implementation, but it should be shaped for both:

- standalone library use
- integration into `roughly`

If the public API design becomes a major decision point, discuss it with the user before committing to it.

## Integration plan for `roughly`

The checker should first mature as an independent crate.

Once the inference core, diagnostics, and tests are stable enough, integration into `roughly` can happen through a library boundary rather than by copying implementation details into another crate.

That should make integration safer and reduce churn while the checker is still evolving.

## Open questions

The following areas are intentionally left open and should be revisited with the user when relevant:

- Exact supported syntax in the first executable milestone
- Whether unsupported constructs always emit diagnostics or sometimes only infer `Unknown`
- The first builtin environment contents
- The exact lowering representation
- Whether `if` belongs in the first inference milestone or the second
- How much annotation syntax should be supported in the first implementation slice
- When to introduce vector-specific constructors and coercion-sensitive functions

## Current implementation recommendation

The current recommendation for the first implementation sequence is:

1. Write this architecture document.
2. Write `crates/typing/TODOS.md`.
3. Reshape the crate toward a library-first structure.
4. Add a test harness centered on R snippets and snapshot diagnostics.
5. Implement the minimum parsing and lowering needed for literal values, names, assignments, functions, and calls.
6. Implement monomorphic inference and unification.
7. Add let-polymorphism through generalization and instantiation.
8. Add list, tuple, and record inference.
9. Add annotation parsing and enforcement.
10. Expand supported syntax carefully from there.

This ordering is intentional:

- it gives early testable progress
- it keeps the HM core small
- it introduces polymorphism in v1 without requiring explicit generic syntax
- it leaves room to discuss important semantic choices before they harden into implementation