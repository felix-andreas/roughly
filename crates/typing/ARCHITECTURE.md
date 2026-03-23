# Architecture

This document is the authoritative implementation architecture for the `typing` crate.

`SEMANTICS.md` is the authoritative user-facing typing contract. This file defines the implementation boundaries needed to realize that contract. Keep it focused on durable phase boundaries and representation boundaries.

## Role of this document

Use this document for:

- phase boundaries
- representation boundaries
- naming and scope architecture
- typechecking architecture

Do not use this document for:

- a changelog
- a task tracker
- a restatement of user-facing typing rules

## File-local pipeline

The intended file-local pipeline is:

1. `check`
2. `lower`
3. `naming`
4. `typecheck`
5. diagnostics output and checked-file results

`check` is the top-level orchestration entry point. It is responsible for wiring the phases together and returning file results.

Syntax parsing is not a real `typing` crate phase. The checker may receive already-parsed syntax from `roughly` or from tests. Parser setup is integration glue, not a core architectural boundary.

Diagnostics are not a pipeline phase. Diagnostics are structured output produced by lowering, naming, and typechecking.

## Phase boundaries

### `lower`

Input:

- parsed syntax
- source access
- shared interner state when available

Output:

- `HirFile`

Responsibilities:

- lower supported R syntax into HIR
- preserve source order and source ranges
- intern spelled names
- parse annotation and type-declaration syntax exactly once
- attach parsed annotation payloads to the HIR items they govern
- represent definition blocks as explicit HIR declarations

Lowering is the front-end boundary. Later phases should consume parsed annotation and declaration data, not raw `#:` text.

Lowering stays distinct from naming even if the implementation runs both phases back to back.

### `naming`

Input:

- `HirFile`
- builtins and imported interfaces as needed for name lookup

Output:

- a named or resolved view of the file

Responsibilities:

- binding introduction
- lexical scope construction
- shadowing
- use-site resolution
- any additional name resolution required before typechecking

The exact output shape is still open:

- a new resolved artifact
- or HIR plus side tables keyed by stable ids

What is fixed is the phase boundary: later phases must be able to distinguish binding identity from spelled names.

### `typecheck`

Input:

- named program representation
- builtin and imported environment information

Output:

- `CheckedFile`

Responsibilities:

- expression checking
- annotation checking
- compatibility and coercion rules
- builtin typing rules
- typed results for tooling
- file interface extraction

Inference is an internal mechanism of `typecheck`, not the architectural name of the phase.

## Representation boundaries

### Syntax tree

The syntax tree preserves surface structure and syntax ranges, but it is not the long-term semantic representation used by later phases.

### Surface type syntax

Annotations and type declarations should first be parsed into a syntax-oriented type representation.

Do not collapse user-written type syntax directly into inference-oriented semantic types.

The annotation and declaration parser should remain directly testable as its own module.

### HIR

HIR is the front-end representation produced by lowering.

HIR should:

- remove parser-tree quirks
- represent expressions, annotations, and declarations explicitly
- preserve source ranges and source order
- use stable arena or id-based storage

Stable ids are required so later phases can use side tables for naming, typechecking, hover, and incremental analysis.

### Naming data

After naming, the checker needs binding identity in addition to spelled names.

Whether naming produces a new artifact or side tables is still open, but later phases must be able to distinguish:

- two bindings with the same spelled name
- a definition site from a use site
- which binding a particular use refers to

### Internal semantic types

Typechecking needs an internal semantic type representation that preserves the distinctions required by semantics, diagnostics, and inference.

It must represent:

- ordinary semantic types
- temporary unknowns
- inference variables
- generalized binding types
- exported interface types

## Project-level direction

Multi-file checking should build on the file-local pipeline rather than bypass it.

The checker should support shared analysis state across files for:

- interned names
- imported interfaces
- later project-level caches

The exact incremental project design is still open, but the architecture should leave room for:

- dependency tracking by interface changes
- reusing unaffected work across checks
- finer-grained invalidation later if the chosen naming and checked representations make that practical

The architecture should optimize for fast re-analysis of a single changed file.

File-local phases and artifacts should remain explicit so one file can be reparsed, relowered, renamed, and rechecked without unnecessary project-wide recomputation.

Project-level analysis should track dependencies through checked file interfaces.

If file `A` changes, dependent files such as `B` must be rechecked when `A`'s exported interface changes, even if those dependent files are not open.

Per-file interfaces are the boundary between file-local checking and later project scheduling.

The intended later project-level stages are:

1. build or load imported file interfaces
2. run naming and typechecking with those interfaces in scope
3. extract the checked file interface
4. track dependency invalidation and dependent diagnostics

The architecture should not assume that only full-file rechecking is possible, but it should also not commit yet to reusing unification or inference state across edits.

The desired near-term file split is recorded in `STRUCTURE.md`.

## Diagnostics

Diagnostics are part of the product surface, not a side effect.

Diagnostic data should stay structured until rendering so wording, ranges, and phase-local errors can be managed consistently.

## Testing seams

The architecture should expose stable phase boundaries for fixture testing.

At the architectural level, the important requirement is that the implementation support direct testing of:

- annotation and type syntax parsing
- naming results
- successful checked output
- rendered diagnostics
