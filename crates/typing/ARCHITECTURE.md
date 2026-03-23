# Architecture

This document is the authoritative implementation architecture for the `typing` crate.

`SEMANTICS.md` is the authoritative user-facing typing contract. This file describes the implementation boundaries required to realize that contract. It should stay focused on durable representation and pipeline decisions rather than status notes or historical plans.

## Role of this document

Keep this file focused on:

- phase boundaries
- representation boundaries
- binding and scope architecture
- typechecking architecture
- diagnostic constraints

Do not use this file as:

- a changelog
- a task tracker
- a restatement of user-facing typing rules

## Phase overview

The intended per-file pipeline is:

1. Parse R syntax with tree-sitter.
2. Lower to annotated HIR.
   - Parse type syntax from annotations and type declarations.
   - Attach parsed annotation payloads to the relevant HIR items.
   - Represent definition blocks as declaration items in source order.
3. Run naming over annotated HIR.
4. Typecheck named HIR.
5. Produce file results for diagnostics and tooling.

When the checker is embedded in `roughly`, it may enter this pipeline after syntax parsing. In that mode, the checker should be able to consume an already-parsed tree together with shared analysis state rather than reparsing source text internally.

## File-local pipeline

The checker should be organized around a small number of explicit per-file phases.

### 1. Parse R syntax

Parse R source into a syntax tree using tree-sitter.

This phase is responsible only for R syntactic structure. It should not perform lowering, name resolution, or typechecking.

### 2. Lower to annotated HIR

Lower supported R syntax into annotated HIR. This is the front-end artifact consumed by later phases.

It should contain:

- lowered expressions and other semantic items
- precise source ranges
- interned names where appropriate
- parsed annotations attached to the relevant HIR items
- type declarations as HIR items

Later phases should consume parsed annotation and declaration data, not raw `#:` text.

Definition blocks should be represented as declarations, not as annotations attached to following expressions.

Annotation parsing may happen during lowering or in an adjacent helper, but the output boundary is the same: annotated HIR.

The annotation parser should remain directly testable as a separate module.

### 3. Naming

Resolve names in annotated HIR to binding identities.

This is the canonicalization phase. It should stay separate from lowering even if the implementation runs it immediately afterward.

Naming is responsible for:

- binding introduction
- lexical scope handling
- shadowing
- use-site resolution
- cross-file name lookup

Naming should produce either resolved HIR or HIR plus side tables keyed by stable HIR identities. The important boundary is that later phases should no longer reason about raw textual names alone when binding identity matters.

### 4. Typecheck

Typecheck named HIR using the user-facing rules from `SEMANTICS.md`.

This is the top-level semantic checking phase. It is responsible for:

- expression checking
- annotation checking
- compatibility rules
- builtin typing rules
- producing typed results for tooling and diagnostics

Hindley-Milner-style inference is an internal mechanism of this phase, not the architectural name of the whole phase.

### 5. Produce file results

File results should retain at least:

- diagnostics
- typed expression results for the active file
- file interface extraction

## Project-level pipeline

Multi-file checking should build on the file-local pipeline rather than bypass it.

The checker should be designed to run with shared analysis state across files rather than as isolated single-file invocations. That shared state is the place for data that should survive across checks, such as interned names and project-level caches.

The exact incremental project design is still open. The architecture should leave room for at least these directions:

- dependency tracking with rechecking when a file's public interface changes
- caching intermediate semantic results and reusing unaffected work across checks
- future finer-grained invalidation below the whole-file level if the naming and typecheck representations make that practical

The intended later project-level stages are:

1. build or load imported file interfaces
2. run naming and typechecking with those interfaces in scope
3. extract the checked file interface
4. track dependencies and invalidation

Per-file interfaces are the boundary between file-local checking and project scheduling.

The architecture should not assume yet that only full-file rechecking is possible, but it also should not commit to reusing unification or inference state across edits until that design is worked out explicitly.

## Performance and reuse

The checker should be designed for repeated analysis, not only one-off single-file runs.

Shared analysis state should be reusable across many checks and should usually live at least as long as the editor or server session that owns it.

Interned symbols should be reused across checks that share analysis state. The main pipeline should not create separate symbol universes for individual annotations or individual file checks when shared state is available.

## Representation boundaries

Keep these conceptual representations distinct.

### Syntax tree

The syntax tree is the tree-sitter output for R source. It preserves surface structure and syntax ranges, but it is not the long-term semantic representation used by later phases.

### Parsed annotations and type declarations

Parse annotations and type declarations into a syntax-oriented representation before lowering them into internal checking types.

Do not collapse user-written type syntax directly into inference-oriented representations.

Annotation and declaration type syntax should use a handwritten recursive-descent parser over the original source slice rather than tree-sitter or a general parser-combinator layer.

This is a deliberate architectural choice:

- the type grammar is small, nested, and delimiter-heavy
- parsing directly over shared source text keeps allocations and reparsing pressure low
- explicit parser state makes it easier to stop at caller-owned delimiters such as `]`, `}`, `)`, and `,`
- delimiter ownership matters for precise, local error messages in nested list and function types

If the type parser grows, preserve those properties.

### Annotated HIR

Annotated HIR is the file-local semantic representation produced by the front end.

It should:

- remove parser-tree quirks
- represent expressions, annotations, and declarations explicitly
- attach parsed annotations to the items they govern
- include type declarations as first-class HIR items

Annotated HIR should be usable as a stable fixture target.

### Named program representation

After naming, the checker needs a representation in which binding identity is available in addition to spelled names.

Whether this is represented as a new tree or as side tables is an implementation choice. The architectural requirement is that later phases can distinguish:

- two bindings with the same spelled name
- a definition site from a use site
- which binding a particular use refers to

### Internal type representation

Typechecking needs an internal semantic type representation that preserves the distinctions required by the semantics, compatibility rules, and diagnostics.

In particular, it should not erase structural information too early, and it must be able to represent both ordinary semantic types and temporary unknowns introduced during inference.

### Generalized binding and interface representation

The checker also needs a representation for generalized binding types and exported file interfaces.

This is the boundary used for let-polymorphic bindings inside a file and for sharing checked information across files.

## Names, symbols, and bindings

Interned symbols and binding identities are different concepts and must stay separate.

Binding identities are needed for:

- shadowing
- precise hover and go-to-definition
- dependency tracking
- future fine-grained invalidation

Diagnostics must still be able to render human-readable names.

Naming owns scope construction and binding identity. The exact language rules for scope belong in `SEMANTICS.md`.

## Typechecking architecture

Typechecking should keep three concerns distinct even when they share helpers:

- HM-style inference machinery
- compatibility and coercion rules
- language-specific typing rules for builtins and operators

The implementation may split these into separate modules over time, but the conceptual separation should remain.

### HM engine

The HM core should provide:

- fresh inference variables
- unification
- instantiation
- generalization
- occurs checks
- representative lookup with path compression

This layer should operate over the internal type representation and type environments. It is a reusable engine, not the entire checker.

Representative lookup should use path compression.

### Compatibility

User-facing checking is not equality-only. Compatibility must stay separate from structural unification, with the exact compatibility rules defined by `SEMANTICS.md`.

### Builtins and special typing rules

Some builtins and operators require dedicated typing rules.

Those rules belong to typechecking, not lowering. Builtins may still be registered through ordinary name machinery so diagnostics and later tooling remain coherent.

## Diagnostics

Diagnostics are part of the product, not a side effect.

`SEMANTICS.md` and the fixture suite together define the user-facing contract. If implementation changes alter fixture-visible behavior, update the relevant documents in the same session.

## Testing seams

The architecture should support stable fixture tests at phase boundaries.

`TESTING.md` is authoritative for the fixture suites and their exact contracts.

At the architectural level, the important requirement is that the implementation expose clear, testable boundaries for the major phases rather than forcing all behavior through end-to-end checks only.
