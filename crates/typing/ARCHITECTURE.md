# Architecture

This document describes the implementation architecture for the `typing` crate.

`SEMANTICS.md` is the authoritative user-facing typing contract. This file should not restate that contract except where a semantic rule imposes an architectural constraint on parsing, lowering, type representation, inference, or diagnostics.

## Role of this document

Keep this file focused on durable implementation decisions:

- representation boundaries
- inference architecture
- lowering constraints
- diagnostic requirements
- testing strategy

Do not use this file as:

- a changelog
- a task tracker
- a restatement of user-facing typing rules

If `SEMANTICS.md` changes in a way that affects implementation structure, update this file in the same session.

## Core boundaries

Keep these layers conceptually separate:

- parsing and syntax access
- annotation parsing
- lowering
- type representations
- inference
- diagnostic rendering

Do not collapse these boundaries for short-term convenience. User-facing surface syntax, lowered semantic forms, and inference-time types must remain distinct.

## Scope of the implementation

The crate is a standalone Rust library for static type checking of a subset of R. It may later be integrated into `roughly`, but the standalone library remains the main implementation boundary while the checker is evolving.

In integrated use, syntax validity should still come from `roughly`'s syntax pipeline before type checking runs. This crate should focus on semantically checking syntactically valid input.

## Design constraints from the semantics

The semantics currently require a few implementation constraints that should be reflected directly in the design:

- compatibility and coercion are part of checking, so the implementation cannot model user-facing checking as pure type equality
- `Any` and `Unknown` are distinct concepts and must stay distinct in representation, inference, and diagnostics
- list shapes are not interchangeable; the implementation must preserve the difference between fixed-shape structural lists and homogeneous list shapes
- only function bodies introduce a new lexical scope; blocks and loops do not
- nullable unions are restricted to the `T | NULL` form, so the internal representation does not need a full open-ended union system

The semantics document defines what these rules mean for users. This document only records the implementation consequences.

## Type representation

The implementation should keep at least three layers of type representation.

### `SurfaceType`

This is the parsed annotation syntax from `#:` comments.

It should represent the full annotation surface described in `SEMANTICS.md`, but it should stay close to syntax rather than inference concerns.

### `CoreType`

This is the internal semantic type representation used by inference and compatibility checking.

It should represent:

- atomic types
- vector shapes
- list shapes
- function types
- `Any`
- `Unknown`
- `NULL`
- restricted nullable unions
- inference variables or references to them

`CoreType` should preserve the distinctions that matter for diagnostics and compatibility. In particular, it must not erase fixed-shape list information into homogeneous list forms too early.

### `TypeScheme`

This represents generalized bindings.

A type scheme contains:

- quantified type variables
- a `CoreType` body

This is the representation used for let-polymorphic bindings.

## Names, symbols, and bindings

Lowering and inference should use interned symbols for repeated textual names.

This should include at least:

- variable names
- function parameter names
- record fields

Interned symbols are session-scoped. A single checking run should reuse one interner across annotation parsing, lowering, builtin setup, inference, and diagnostic rendering. Fresh interners are still fine in isolated tests or other one-off parsing helpers, but the main pipeline should not create separate symbol universes for individual annotations.

Interned symbols are not the same as bindings. Distinct bindings in different function scopes may share the same interned symbol. If binding identity needs to be modeled separately, represent it separately rather than overloading the symbol itself.

Diagnostics must still render human-readable names from source text.

## Parsing and annotation extraction

The checker should not perform inference directly over raw parser nodes.

Parsing should be split into:

1. syntax parsing
2. annotation extraction and parsing
3. lowering into a representation suitable for inference

The annotation parser should remain separate from the inference engine. Annotation parsing failures should preserve enough source information for good diagnostics.

### Type-syntax parser strategy

Surface type syntax should use a handwritten recursive-descent parser over the original source slice rather than a general parser-combinator style library.

This is an architectural choice, not an incidental implementation detail:

- the surface grammar is small, nested, and delimiter-heavy
- parsing directly over the shared source text keeps allocations and reparsing pressure low
- explicit parser state makes it easier to stop at caller-owned delimiters such as `]`, `}`, `)`, and `,`
- that delimiter ownership is important for precise, local error messages in nested list and function types

If the parser grows, preserve those properties. Convenience abstractions are acceptable only if they do not give up low-allocation parsing or degrade error locality.

## Lowering

Lowering should translate supported R syntax into a smaller semantic IR designed for typing.

The lowered IR should:

- preserve precise source ranges
- use interned symbols where appropriate
- encode the supported expression forms directly
- avoid baking parser-tree quirks into inference

Lowering should preserve the distinctions needed by the semantics instead of reconstructing them later in inference. In particular:

- function boundaries must be explicit because they introduce scope
- assignment and name-reference forms should lower distinctly
- list construction should preserve named versus unnamed elements and whether names are statically known
- indexing forms should preserve whether access is positional, name-based, or `$` sugar

Unsupported constructs may still lower into explicit unsupported forms that infer as `Unknown`. That is preferable to collapsing them into ad hoc partial representations throughout inference.

## Inference model

The inference engine should use a Hindley-Milner-style core with:

- fresh type variables
- unification
- instantiation
- generalization
- type environments
- occurs checks

Inference should not be modeled as equality-only over user-facing types. The implementation needs two related mechanisms:

- unification for inference variables and structurally comparable types
- compatibility checking for user-facing coercions and annotation checking

Those mechanisms should stay conceptually distinct even if they share helper code.

### Inference-variable state

The inference engine should use explicit mutable state for inference variables rather than only repeated substitution maps.

That state should support:

- unbound inference variables
- variable-to-variable links
- bindings from variables to structured types

Representative lookup should use path compression so repeated lookups collapse variable-link chains over time.

Path compression does not replace the occurs check. The occurs check is still required when binding a variable to a structured type.

### Generalization

Let-polymorphism should be implemented through:

- generalization at bindings
- instantiation at use sites

Even without explicit generics syntax, generalized bindings are part of the intended implementation model.

## Environments and scope

The checker needs at least these environments:

- builtin bindings
- lexical value bindings
- annotation context as needed during checking

Builtin and lexical environments should be keyed by interned symbols rather than raw strings.

Scope handling should follow the current semantic rule that only functions introduce new lexical scope. Blocks and loops should reuse the surrounding scope rather than introducing a fresh one.

## Builtins and special typing rules

Some builtins and operators will not fit cleanly as ordinary HM function values with only structural unification.

When that happens, it is acceptable for inference to recognize a builtin binding or lowered operator form and apply a dedicated typing rule.

That does not justify bypassing the ordinary pipeline entirely. Builtins should still enter through ordinary lowering and name resolution where practical so diagnostics remain coherent.

The builtin surface should stay intentionally small and grow only as tests require it.

## Checker pipeline

The end-to-end pipeline should be:

1. parse R source
2. extract and parse adjacent `#:` annotations
3. lower supported syntax into the semantic IR
4. infer and check types over the lowered IR
5. apply annotation rules and compatibility checks
6. render diagnostics and inferred results

The checker should retain access to source text and syntax trees for diagnostics, but inference should operate on lowered forms.

## Diagnostics

Diagnostics are part of the product, not a side effect.

A core goal is Elm- and Rust-like diagnostic quality: clear, precise, actionable messages with accurate source ranges.

To support that, the implementation should preserve:

- source ranges on lowered nodes
- enough original spelling information to render names and types clearly
- enough structural information to explain mismatches in user-facing terms

Rendered diagnostics are part of the fixture contract and should remain stable unless wording or semantics intentionally change.

## Testing strategy

The primary contract tests should remain fixture-based end-to-end tests over R snippets.

Use fixture tests for:

- rendered diagnostics
- normalized inferred types
- user-visible compatibility behavior

Use focused Rust tests for internal mechanisms when fixture tests would be awkward, especially:

- unification
- occurs checks
- generalization
- instantiation
- annotation parsing
- lowering details with no direct user-facing rendering

`SEMANTICS.md` and the fixture suite together define the user-facing contract. If implementation changes alter fixture-visible behavior, update the relevant documents in the same session.

## Public API direction

The public API should stay narrow.

It should center on checking source text and returning structured diagnostics plus inferred type information suitable for both:

- standalone library use
- integration into `roughly`

If public API shape becomes a meaningful design decision, discuss it with the user before committing to it.
