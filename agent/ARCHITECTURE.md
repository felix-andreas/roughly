# Architecture

This document is the authoritative implementation architecture for the `analysis` crate.

`TYPING_SEMANTICS.md` is the authoritative user-facing typing contract. This file defines the implementation boundaries needed to realize that contract. Keep it focused on durable phase boundaries and representation boundaries.

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

## Pipeline

The file-local checking pipeline is:

parsed syntax -> `lower` -> `naming` -> `typecheck` -> checked-file results and diagnostics

`check` is the orchestration entry point around that pipeline. It wires phases together and returns file results, but it is not itself a semantic phase.

Syntax parsing is not a `analysis` crate phase. The checker may receive already-parsed syntax from `roughly` or from tests.

Diagnostics are not a separate phase. They are structured outputs produced by lowering, naming, and typechecking.

## Phase contracts

### `lower`

Input:

- one `workspace::Document`
- shared interner state when available

Output:

- `HirModule`
- lowering diagnostics

Responsibilities:

- lower supported R syntax into HIR
- preserve source order and source ranges
- intern spelled names
- parse annotation and type-declaration syntax exactly once
- attach parsed annotation payloads to the HIR nodes they govern
- collect explicit top-level HIR type declarations for `@type` and `@alias`
- collect executable top-level expressions separately from declarations
- reject declaration placements that cannot appear in HIR, such as non-top-level type definition blocks

Non-responsibilities:

- value-name resolution
- project-wide type-name resolution
- typechecking

Lowering is the front-end structural boundary. Later phases consume parsed HIR data, not raw `#:` text.

### `naming`

Input:

- lowered HIR for one package
- project file order
- project-global declaration tables derived from lowered files as needed

Output:

- `NamedModule`
- naming diagnostics

`NamedModule` is the named view consumed by typechecking. It may be represented as HIR plus side tables keyed by stable ids, but the phase contract is fixed even if the exact Rust types evolve.

Responsibilities:

- run file-local naming preparation for each lowered file
- introduce file-local value bindings
- construct lexical value scopes
- handle value shadowing
- resolve value use sites that can be decided from file-local lexical structure
- leave package-global value references unresolved during file-local naming, even when the declaration is in the same file
- collect top-level value declarations and top-level type declarations
- record unresolved value and type references that require package-global lookup
- run project-global resolution by updating those per-file naming results in place
- build one final package-global value table from top-level exports
- resolve every unresolved top-level value reference against that final package-global table
- build the project-global type namespace from top-level declarations
- resolve type references in annotations and declarations
- resolve type references against that project-global namespace
- assign package-visible project-level identities for top-level declarations
- diagnose unknown type names, duplicate type declarations, wrong type-argument arity, alias-versus-nominal misuse for `@new`, and cross-file top-level value collisions

Naming data is also a tooling boundary, not only a typechecking prerequisite.

The naming result should be rich enough to support:

- go-to-definition within a file
- local rename within a file
- project-level rename across files once cross-file naming data and project scheduling exist

Non-responsibilities:

- syntax parsing
- structural placement validation already enforced by lowering
- expression type inference and compatibility checking

Naming is the semantic name-resolution boundary. Lowering may still run document-by-document, but naming operates at package scope. Later phases must consume resolved binding identity and resolved type identity rather than re-resolving spelled names ad hoc.

Internally, naming should be split into:

1. file-local naming preparation
2. project-global resolution

The file-local preparation pass is authoritative for local lexical facts.
The project-global pass is authoritative for package-visible top-level value and type resolution.
This split does not require a separate durable intermediate artifact. The project-global pass may update the same naming result built by file-local resolution, leaving still-unresolved names in place when lookup fails.

Top-level declarations should not keep their preliminary file-local binding ids as their final package-visible identities.
The project-global pass should assign distinct project-level ids for package-visible declarations so cross-file naming facts are owned by the package-level result rather than by incidental file-local traversal order.

### `typecheck`

Input:

- `NamedModule`
- builtin typing information
- project summaries as needed for semantic checking and incremental invalidation

Output:

- `CheckedFile`
- typechecking diagnostics

Responsibilities:

- expression checking
- annotation checking after type references are already resolved
- compatibility and coercion rules
- builtin typing rules
- typed results for tooling
- checked-file interface extraction

Non-responsibilities:

- parsing type syntax
- name resolution
- declaration placement validation

Inference is an internal mechanism of `typecheck`, not the architectural name of the phase.

## Representation boundaries

### Syntax tree

The syntax tree preserves surface structure and syntax ranges, but it is not the long-term semantic representation used by later phases.

### Surface type syntax

Annotations and type declarations are first parsed into a syntax-oriented type representation.

Do not collapse user-written type syntax directly into inference-oriented semantic types.

The annotation and declaration parser should remain directly testable as its own module.

### HIR

HIR is the structural front-end representation produced by lowering.

HIR should:

- remove parser-tree quirks
- represent top-level type declarations explicitly
- represent executable top-level expressions separately from type declarations
- preserve source ranges and source order
- use stable ids for nodes that later phases need to reference from side tables

The HIR module should model a file as:

- a top-level declaration collection
- a top-level executable expression list

Type declarations are not expression nodes, and their interleaving with top-level expressions is not semantically significant.

Expression HIR remains separate from type-syntax parsing concerns. A block expression contains executable child expressions only; it does not contain nested type declarations.

### Naming data

After naming, the checker needs semantic identity in addition to spelled names.

The named representation must distinguish:

- two value bindings with the same spelled name
- a value definition site from a value use site
- which value binding a use refers to
- a type declaration from a type reference
- which type declaration a type reference resolves to
- whether a resolved type name is nominal or an alias

The naming representation should preserve both:

- file-local naming facts needed to explain lexical resolution within one file
- project-level naming identities for package-visible declarations and cross-file references

For top-level value declarations, the final package-visible identity should come from the project-global naming pass rather than directly reusing a file-local provisional id.

The named representation should also preserve the information needed for editor tooling built on name resolution, especially:

- jumping from a use site to its definition site
- enumerating all use sites for local rename
- extending that same identity model to project-level rename when cross-file naming data is available

### Internal semantic types

Typechecking needs an internal semantic type representation that preserves the distinctions required by semantics, diagnostics, and inference.

It must represent:

- ordinary semantic types
- temporary unknowns
- inference variables
- generalized binding types
- exported interface types

## Project-level direction

Multi-file checking should build on the file-local lowering pipeline rather than bypass it.

The checker should support shared analysis state across files for:

- interned names
- project file order
- project-global declaration tables
- later project-level caches

That project-level direction should leave room for tooling operations built on naming identity, including cross-file go-to-definition and rename.

The architecture should optimize for fast re-analysis of a single changed file.

File-local phases and artifacts should remain explicit so one file can be reparsed and relowered without unnecessary project-wide recomputation, while naming and later semantic phases still operate on the package.

Project-level analysis should track dependencies through project-global names and any later checked-file summaries used for incremental invalidation.

If file `A` changes, later files that depend on `A`'s project-visible names must be rechecked when those visible names change, even if those dependent files are not open.

The intended later project-level stages are:

1. build or load project file order and project-global declaration tables
2. run `lower`
3. run file-local naming preparation
4. run project-global naming resolution and assign project-level declaration identities
5. run `typecheck`
6. extract any checked-file summaries needed for incremental invalidation
7. track dependency invalidation and dependent diagnostics

The architecture should not assume that only full-file rechecking is possible, but it should also not commit yet to reusing unification or inference state across edits.

The desired near-term file split is recorded in `STRUCTURE.md`.

## Diagnostics

Diagnostics are part of the product surface, not a side effect.

Diagnostic data should stay structured until rendering so wording, ranges, and phase-local errors can be managed consistently.

## Testing seams

The architecture should expose stable phase boundaries for fixture testing.

At the architectural level, the important requirement is that the implementation support direct testing of:

- annotation and type syntax parsing
- lowering results
- naming results
- successful checked output
- rendered diagnostics
