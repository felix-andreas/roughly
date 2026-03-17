# `typing` Architecture

This document describes the intended implementation architecture for the `typing` crate.

It is currently somewhat stale while the semantics are being refined. During this phase, `SEMANTICS.md` is the authoritative document for user-facing behavior, type forms, and compatibility rules. If this document and `SEMANTICS.md` disagree, follow `SEMANTICS.md`.

The crate is a standalone library for static type checking of a subset of R. Later it may be integrated into `roughly`, but the standalone library remains the primary implementation boundary while the type checker is still evolving. In integrated use, syntax validity should still be decided by `roughly`'s existing syntax-checking pipeline before type checking runs.

As semantics stabilize, this document should be brought back into tighter alignment with the implementation and the semantics contract.

Project planning is tracked in `crates/typing/TODOS.md`, though that plan is also somewhat stale while semantics are being refined.

User-facing typing semantics are tracked in `crates/typing/SEMANTICS.md`.

Cross-session continuity is tracked in `crates/typing/MEMORY.md`.

Important design decisions must be discussed with the user before implementation. If a planned step is ambiguous, under-specified, or introduces a meaningful semantic tradeoff, stop and discuss it first. Changes to `crates/typing/SEMANTICS.md` must always be discussed with the user first.

## Document hygiene

Keep this file high signal.

- Keep it focused on durable design decisions and architectural constraints.
- Do not use it as a changelog, status report, or session diary.
- Put task tracking in `TODOS.md`.
- Put user-facing typing semantics in `SEMANTICS.md`.
- Put cross-session handoff notes in `MEMORY.md`.
- Keep `SEMANTICS.md` and the fixture suites in sync; both are part of the contract.
- While semantics are still being refined, prefer making this document explicitly incomplete over silently restating stale assumptions.
- When implementation meaningfully changes the design contract, update this file in the same session.

## Goals

The initial goals are:

- Build a standalone Rust library for type checking R code.
- Use Hindley–Milner style inference as the foundation.
- Support internal polymorphism in v1.
- Do not require explicit generics syntax in v1.
- Develop the checker test-first.
- Use R snippets as the primary test input format.
- Prefer fixture-based tests of rendered diagnostics for end-to-end behavior.
- Treat fixture expectations as part of the testing contract, not as disposable snapshots.
- Keep `SEMANTICS.md` and the fixture suites aligned as the user-facing contract.
- Aim for Elm- and Rust-like diagnostic quality, with clear, precise, and actionable error messages.
- Support later integration into `roughly` without redesigning the core engine.

## Non-goals for v1

The first version should not attempt to model all of R.

Explicit non-goals for v1:

- Full coverage of base R syntax and semantics
- S3 dispatch modeling
- S4 dispatch modeling
- NSE and metaprogramming completeness
- Environment and reference semantics
- General union types beyond the nullable `T | NULL` form described in `SEMANTICS.md`
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
- lowered semantic syntax
- internal inference types
- generalized type schemes

User-facing types and internal inference types should not be conflated.

The checker should keep access to the original source text and syntax tree for diagnostics, while lowering supported syntax into a simpler semantic representation for typing. Lowered nodes should preserve source ranges so diagnostics can refer back to the original code precisely.

### Value-like model for the supported subset

R is not globally immutable, but the supported subset should be treated as value-like for the initial checker.

This means the initial design focuses on ordinary expressions, vectors, lists, records, tuples, and functions. Mutable or reference-oriented parts of R should be deferred and handled as unsupported syntax or `Unknown`.

### Equality-based typing in v1

The first implementation should use equality-based unification, not subtyping.

This keeps the inference engine simpler and preserves principal typing more naturally than mixing HM with structural subtyping from the start.

### Unsupported lowered constructs degrade to `Unknown`

When the checker encounters a construct from syntactically valid input that lowering or inference does not yet support, it should infer `Unknown` rather than failing catastrophically.

In standalone crate tests, syntax errors may still be reported locally. In integrated use, syntax diagnostics should come from `roughly`'s existing syntax checker, and `typing` should focus on type checking syntactically valid input.

The default behavior for lowered unsupported constructs is to preserve forward progress and avoid cascades.

## Scope of the supported language subset

The initial supported subset should be intentionally small.

Planned early support:

- scalar literals
- symbol references
- assignments
- function definitions
- function calls
- top-level sequences
- simple builtins needed for tests
- list-like constructions with the rules described below

Likely early follow-up support:

- `if` expressions or statements
- blocks
- broader builtin coverage beyond the small tested subset

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

These deferred constructs may still lower to `Unknown` when they appear inside otherwise valid syntax before dedicated support exists.

## Type model

The checker needs to model both user-facing semantic shapes and HM inference variables.

User-facing type semantics are defined in `SEMANTICS.md` and are currently more up to date than this document. This file should only keep the implementation-facing constraints that follow from those semantics.

### Base categories

The initial type space should include:

- `Any`
- `Unknown`
- `Null`
- scalar atomics
- atomic vector shapes
- list-like structural shapes
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

### User-facing shape semantics

Scalar-like, array-like, map-like, tuple-like, and other user-facing shape rules belong in `SEMANTICS.md`.

Architecture work should preserve those semantics, not redefine them here.

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

The user-facing notation and examples are defined in `SEMANTICS.md`. `SurfaceType` should be able to represent the semantic forms described there.

### `CoreType`

This is the internal inference representation.

It should include all of the semantic forms above, plus inference variables.

### `TypeScheme`

This represents generalized bindings.

A type scheme contains:

- quantified type variables
- a `CoreType` body

This is required for let-polymorphism.

### Interned symbols

The lowering and inference layers should use string interning for repeated identifiers and field names.

This should include at least:

- variable names
- function parameter names
- record field names

The current implementation uses interned symbols already. Any future representation changes should preserve human-readable diagnostic rendering and should not blur the distinction between symbol identity and binding identity.

Interning gives a stable canonical symbol identity for repeated text, which keeps nested environments and name lookup simpler and cheaper than repeatedly storing and comparing owned strings.

Interned symbols are not the same as bindings. Two different bindings may share the same interned symbol if they use the same textual name in different lexical scopes. If binding identity becomes important later, it should be represented separately from the interned symbol.

Diagnostics must still render human-readable names. Interning should therefore remain an internal representation detail, with diagnostics resolving symbols back to their source text.

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

### Unification state and path compression

The inference engine should use an explicit inference-variable state rather than relying only on repeatedly applied substitution maps.

That state should support:

- unbound inference variables
- variable-to-variable links
- bindings from inference variables to concrete or structured types

The current implementation already follows this shape. Alternative storage choices may be explored later, but representation changes should be justified by measured simplicity or performance improvements rather than adopted casually during feature work.

Representative lookup should use path compression so chains of linked type variables collapse toward their final representative over time. This keeps repeated lookups efficient during inference.

Path compression does not replace the occurs check. The occurs check is still required when binding a variable to a structured type in order to reject infinite types.

### Why let-polymorphism matters

Without let-polymorphism, a binding such as an identity function can only be used at one inferred type.

With let-polymorphism, the checker can infer a generalized binding and instantiate it independently at each use site.

That behavior is central to the planned HM design and is part of v1.

### No explicit generics syntax in v1

Even though v1 supports internal polymorphism, explicit generic syntax in annotations is deferred.

That means generic behavior exists in inference and schemes, but users do not yet write parameterized type syntax directly.

## Builtin functions and operators

The checker may model a small set of builtins before broader R builtin coverage is attempted.

Builtins should still enter the system through ordinary lowering as symbol references and calls where practical, so they remain visible in diagnostics and fit the lowering/inference boundaries.

Some builtins have typing behavior that is awkward to express as an ordinary equality-based HM function type. In those cases, it is acceptable for inference to recognize a builtin binding and apply a dedicated rule instead of relying only on generic function unification.

The current builtin slice is intentionally small:

- `+`
- `c(...)`

`+` is lowered as a call to the builtin symbol `+` and typed with a dedicated inference rule.

`c(...)` is currently supported only as a minimal builtin needed to express vector-producing test cases for arithmetic. It should not be treated as a commitment to full R `c(...)` semantics.

For `+`, the current semantics are:

- operands must be numeric
- numeric currently means `integer` or `double`
- scalar + scalar is allowed
- vector + scalar is allowed
- scalar + vector is allowed
- vector + vector is allowed
- if either operand is `double`, the result is `double`
- otherwise the result is `integer`
- if either operand is a vector, the result is a vector
- otherwise the result is a scalar

## Annotation model

The current plan is to use comment-based annotations rather than extending R syntax.

The annotation mechanism should remain based on `#:` comments.

The exact supported annotation forms and their meaning are defined in `SEMANTICS.md`. In particular, user-facing behavior should follow `SEMANTICS.md` for checked annotations, unknown-only assertions, trusted assertions, and function annotation forms.

Planned annotation support:

- variable annotations
- function parameter annotations
- function return annotations

Deferred annotation support:

- explicit generics syntax
- nominal type declarations
- advanced type aliases

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

Lowering should not discard the syntax information needed for high-quality diagnostics. The checker should retain access to source text and the syntax tree, while lowered nodes keep precise source ranges and interned symbols where appropriate.

## Inference pipeline

The checker pipeline is:

1. Parse R source.
2. Extract and parse adjacent `#:` annotations.
3. Lower supported syntax into an internal expression or module representation.
4. Infer types over the lowered representation.
5. Apply annotation constraints where present.
6. Render diagnostics and inferred results.

The checker should not perform inference directly over raw parser nodes.

Builtin and lexical environments should be keyed by interned symbols rather than raw strings. The builtin environment should start small and grow only as required by tests.

## Error handling and diagnostics

Diagnostics are part of the product.

A core goal is Elm- and Rust-like diagnostic quality. Error messages should be clear, precise, and actionable, and should help the user understand what went wrong and where.

Each diagnostic should aim to include:

- a source range
- a short summary
- enough type detail to explain the mismatch
- stable rendering suitable for fixture-based end-to-end tests

The rendered form of diagnostics is part of the test interface.

Diagnostic wording changes should be intentional. Fixture expectations should be updated only when wording improves or semantics change. Diagnostics should prefer clarity over theory-heavy terminology.

## Testing strategy

This crate should be developed test-first.

The primary end-to-end tests should operate on R snippets and assert rendered diagnostics through fixture-based expectations.

The testing contract should follow these rules:

- `SEMANTICS.md` and the fixture suites together define the user-facing contract
- diagnostics fixtures are the primary user-facing contract for rendered errors
- diagnostics fixture assertions should stay strict
- inference fixtures should use normalized rendered types and should also be treated as contractual
- fixture files may be split by feature for readability
- fixture identity should come only from `group__case`, not from the filename
- duplicate `group__case` names across the whole fixture suite should be rejected
- fixture expectations should be updated only when behavior, wording, source ranges, or normalized rendering intentionally change
- when fixture-visible semantics change, update `SEMANTICS.md` in the same session
- if user-facing semantics are still unclear, discuss them with the user first and then record the resolved rule in `SEMANTICS.md`

Prefer fixture-authored tests over Rust-authored tests when the behavior is naturally expressed as an R snippet.

Focused unit tests are still useful for internal pieces such as:

- unification
- occurs checks
- instantiation
- generalization
- annotation parsing
- lowering details that would be awkward to express in fixture form

These tests are secondary to the snippet-based end-to-end tests, not a replacement for them.

## File and module direction

The implementation should remain modest in structure early on.

During the current scaffolding phase, splitting functionality into different files is encouraged when it helps establish clear boundaries and avoids forcing unrelated concerns into one file too early.

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

- Whether unsupported constructs always emit diagnostics or sometimes only infer `Unknown`
- Whether `if` belongs in the next supported slice
- How much annotation syntax should be supported in the first implementation slice
- When to introduce vector-specific constructors and coercion-sensitive functions