---
title: Architecture
description: Implementation architecture of Roughly's analysis engine and its durable phase and representation boundaries
---

This document is the authoritative implementation architecture for Roughly's analysis. Two crates cooperate:

- **`engine`** — a generic red-green *memoized query* core. It holds no R knowledge and does not depend on `analysis`; it is the substrate that turns the analysis phases into incrementally-recomputed queries with automatic, dependency-tracked invalidation.
- **`analysis`** — the computational phases: parsing (over tree-sitter), lowering, naming, type inference, lint, and the IDE logic. The engine drives incremental analysis by running these phases as query bodies. `analysis` also exposes a clean *from-scratch* checker, `run_full`, retained as the correctness oracle (see [Differential correctness](#differential-correctness)) and used directly by the command-line path.

The [Typing Reference](/typing-reference) is the authoritative user-facing typing contract. This page defines the implementation boundaries needed to realize that contract.

## Role of this document

Use this document for:

- phase boundaries
- representation boundaries
- the query-engine / incremental-analysis model
- naming and scope architecture
- typechecking architecture

Do not use this document for:

- a changelog
- a task tracker
- a restatement of user-facing typing rules

## Pipeline

The analysis phase surface is:

parsed syntax -> `lint` -> `lower` -> `naming` -> `typecheck` -> checked-file results and diagnostics

These phases are pure functions of their inputs. The engine wires them together as memoized queries (see [Incremental analysis](#incremental-analysis-the-query-engine)); `run_full` wires them together as one from-scratch pass. Neither wiring is itself a semantic phase.

Syntax parsing produces the tree the phases consume; the phases never re-parse spelled `#:` text after lowering.

Diagnostics are not a separate phase. They are structured outputs produced by lint, lowering, naming, and typechecking.

## Phase contracts

### `lint`

Input:

- one document's parsed syntax and source text
- lint configuration

Output:

- lint diagnostics

Responsibilities:

- run file-local style and surface checks over parsed syntax and source text
- produce diagnostics that do not require HIR, naming, or type information

Non-responsibilities:

- HIR construction
- value-name resolution
- type-name resolution
- typechecking

Lint is a separate file-local phase.

### `lower`

Input:

- one document's parsed syntax
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

- lowered HIR for one file
- the package's set of files and the exported names each contributes

Output:

- a named view of the file (HIR plus side tables keyed by stable ids)
- naming diagnostics

The named view is what typechecking consumes. The phase contract is fixed even if the exact Rust types evolve.

Responsibilities:

- run file-local naming preparation for each lowered file
- introduce file-local value bindings
- construct lexical value scopes
- handle value shadowing
- resolve value use sites that can be decided from file-local lexical structure
- leave package-global value references unresolved during file-local naming, even when the declaration is in the same file
- collect top-level value declarations and top-level type declarations
- record unresolved value and type references that require package-global lookup
- resolve package-global value references against the package's table of exported top-level names
- build the project-global type namespace from top-level declarations and resolve type references in annotations and declarations against it
- assign package-visible project-level identities for top-level declarations
- diagnose unknown type names, duplicate type declarations, wrong type-argument arity, alias-versus-nominal misuse for `@new`, and cross-file top-level value collisions

Naming data is also a tooling boundary, not only a typechecking prerequisite.

The naming result is rich enough to support:

- go-to-definition within a file
- local rename within a file
- project-level rename across files

Non-responsibilities:

- syntax parsing
- structural placement validation already enforced by lowering
- expression type inference and compatibility checking

Naming is the semantic name-resolution boundary. Lowering runs per file; package-global resolution operates over the package. Later phases consume resolved binding identity and resolved type identity rather than re-resolving spelled names ad hoc.

Naming is split into:

1. file-local naming preparation — authoritative for local lexical facts; also yields the file's exported-name set
2. package-global resolution — authoritative for package-visible top-level value and type resolution

Known limitation — which top-level assignments are package globals: a bare top-level `{ }` block executes unconditionally, so its direct-child assignments (including nested bare blocks) are package globals, exactly like a top-level `name <- value`. Assignments inside `if`/`for`/`while` bodies are conditionally executed and are not yet package globals — a cross-file reference to such a name reports "could not resolve" — pending a future conditional-global (weak-global) tier. This is the single membership rule used everywhere a binding's package-global status is decided, so the answer cannot disagree between sites.

Top-level declarations do not keep their preliminary file-local binding ids as their final package-visible identities. Package-global resolution assigns distinct project-level ids for package-visible declarations, so cross-file naming facts are owned by the package-level result rather than by incidental file-local traversal order.

### `typecheck`

Input:

- the named view of one file
- builtin typing information
- the exported type schemes of the package-global symbols the file references

Output:

- a checked file (typed results and the file's exported interface)
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

#### Cross-file interface

`typecheck` infers each file against the exported type schemes of the package-global symbols it references. A symbol's exported scheme is computed once, per symbol, and shared with every file that references it (the per-symbol interface layer in [Incremental analysis](#incremental-analysis-the-query-engine)). Re-exports and forward references resolve through that shared interface, so `second <- first` exports `first`'s scheme even when `first` lives in another file.

Consequences that are part of the contract:

- cross-file references see the exporting file's generalized scheme; type information does not flow back across file boundaries through inference (a call in one file never changes the inferred type of a function defined in another file)
- interface schemes move between per-file inference states by importing: quantified variables are re-bound to fresh local ids, and stray free variables erase to `Unknown`
- checking recovers per top-level expression, so every error in every file is reported

Generalization is level-based: variables created while inferring a binding's value live one level deeper than the binding boundary, unification propagates the lower level outward, and generalization quantifies exactly the variables deeper than the current level without walking the environment.

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

For top-level value declarations, the final package-visible identity should come from the package-global naming pass rather than directly reusing a file-local provisional id.

The named representation should also preserve the information needed for editor tooling built on name resolution, especially:

- jumping from a use site to its definition site
- enumerating all use sites for local rename
- extending that same identity model to project-level rename across files

### Internal semantic types

Typechecking needs an internal semantic type representation that preserves the distinctions required by semantics, diagnostics, and inference.

It must represent:

- ordinary semantic types
- temporary unknowns
- inference variables
- generalized binding types
- exported interface types

## Incremental analysis: the query engine

Incremental analysis is built on the `engine` crate, a generic red-green memoized query core. The phases above are written once, as ordinary functions, and the engine runs them as *queries* whose results are cached and recomputed only when something they actually read has changed. Invalidation is therefore a consequence of recorded dependencies, not a hand-maintained mirror of the dependency graph — which is the structural property the whole design is for.

### The red-green core

- **Revision** — a logical clock bumped on every input change.
- **Slots** — one table holds every query's memoized value (type-erased) together with the revision it was last verified at, the revision it last changed at, and the dependencies it recorded. Inputs and derived queries live in the same table.
- **Inputs vs. derived queries** — inputs are set from outside and never computed; derived queries are produced by a body that reads other queries.
- **`fetch`** — the only way a body reads another query. It records that query as a dependency of the currently-computing query, validates it, and returns the value. Reading *is* recording; there is no untracked read path to forget.
- **Validation (red-green)** — a memo is *green by revision* (already verified this revision), *green by early cutoff* (all of its dependencies revalidated to equal values, so it need not re-run), or *red* (a dependency changed; recompute).
- **Value-equality early cutoff** — when a recomputed query produces a value equal to its previous value, its dependents do not recompute. This is what lets an edit that does not change an exported scheme stop at that boundary instead of propagating onward.
- **Input removal** — deleting an input leaves a tombstone, so a dependent revalidates against the now-smaller input set instead of re-executing an absent input.
- **Accidental-cycle guard** — a derived body that transitively fetches itself fails loudly rather than overflowing the stack; this is treated as a programming error. (The one *intended* cycle is contained in a single body — see [the re-export cycle](#the-re-export-interface-cycle).)

### The query graph

Inputs (set from outside, never computed):

- **`source_text(file)`** — per-file source. The high-churn input: every keystroke sets it.
- **`document_kind(file)`** — package source vs. script, kept as a *separate* input so a text-only edit does not invalidate through a kind read.
- **`project_files`** — the set of files that exist. This is the single source of truth for membership; adding or removing a file is an edit to this input plus the file's own `source_text` / `document_kind`.
- **`config`** — the project `roughly.toml` (`[format]`, `[lint]`, `[check] typing/unused/strict`).
- **`stdlib_stubs`** — the immutable standard-library stubs. Set once; it never invalidates anything.

Derived queries (every edge is a recorded `fetch`, so the dependency is automatic):

| Query | Reads | Role |
| --- | --- | --- |
| `parse(file)` | `source_text(file)` | the tree is a pure function of the bytes |
| `lower(file)` | `parse(file)` | HIR |
| `local_naming(file)` | `lower(file)`, `document_kind(file)` | file-local resolution; also yields the file's exported-name set |
| `package_symbol_index` | `project_files`, each package file's exported-name set, `stdlib_stubs` | the def-map: name → winning defining/re-exporting item. **Names only, no types.** The single all-files fold; it changes on *structural* edits (add/remove/rename a top-level binding, add/remove/reclassify a file), not on body edits |
| `defining_item(symbol)` | `package_symbol_index` | a firewall projecting one symbol's winner out of the index, so a change to one symbol's winner cuts off for the others |
| `global_scheme(symbol)` | `defining_item(symbol)`, then the winning file's inference for that item (or the re-export cycle body) | the per-symbol exported type scheme |
| `typecheck(file)` | `lower(file)`, `local_naming(file)`, `config`, and `global_scheme(s)` for **each symbol `s` the file references** | HM inference over the file |
| `diagnostics(file)` | `typecheck(file)`, `lint(file)`, lowering diagnostics, `config` | the rendered diagnostics; `config` gates typing/unused/strict |

### Automatic dependency-tracked invalidation

The fine-grained, per-symbol interface layer is what makes invalidation precise. `typecheck(file)` records `global_scheme(s)` for exactly the symbols the file references. When a global's scheme changes, only that symbol's `changed_at` advances, and only the `typecheck` memos that recorded it revalidate. That recorded per-symbol dependency set *is* the reverse-dependency map — reconstructed automatically and exactly, with nothing to patch and nothing to drift.

The consequences:

- Editing a function body changes one symbol's scheme and re-typechecks only its referrers. The names-only `package_symbol_index` does not re-fold for a body edit, because the file's exported-name set is unchanged and cuts off.
- `typecheck(file)` never reads `project_files` or `package_symbol_index` directly — it reaches the file set and the def-map only *behind* the per-symbol `global_scheme` / `defining_item` firewall — so adding an unrelated file cannot invalidate a file that does not reference a symbol whose winner changed.
- There is no hand-maintained reverse-dependency index, dirty-set, or dependency fingerprint. Each of those was a stand-in for what the recorded `fetch` graph plus value-equality cutoff now provide directly, and each carried a class of silent-staleness bug (a mirror the untracked read path could bypass). With `fetch` as the only read path, that bug class is structurally impossible.

### The re-export interface cycle

R allows mutual typed re-exports (`a <- b` in one file, `b <- a` in another), a genuine dependency cycle the acyclic core cannot express through plain `fetch` recursion (it would re-enter a key already being computed and trip the accidental-cycle guard). It is resolved inside a single query body that owns the whole strongly-connected component: a bounded fixed-point that iterates to convergence — acyclic re-export and forward-reference chains are monotone (each scheme transitions at most once, `Unknown` to concrete) and converge within a bound proportional to the number of globals — with an oscillation guard that pins a genuinely cyclic symbol to `Unknown`, collapsing the cycle so the loop converges. Downstream queries depend on its converged result normally, and value-equality cutoff stops propagation when that result is unchanged. This is the one non-trivial query body; it carries its full correctness burden (the convergence bound and the oscillation guard) rather than dissolving.

### Concurrency

The engine uses non-thread-safe interior pointers, so it is not shared across threads. The shipped language server runs it on one dedicated worker thread, off the main thread, and is **demand-driven**: it computes only what an editor query asks for — open files and their dependents — with no eager whole-workspace pass.

Live responsiveness comes from **cooperative cancellation**, not parallelism. Each edit flips a cancellation token; an in-flight cross-file pass observes it at recompute boundaries and abandons by unwinding to the `fetch` entry point, committing no partial memo, so the next pass recomputes only its blast radius — latest edit wins. Edit notifications run to completion (uncancellable), so a file's published diagnostics are never left stale; interactive read requests are cancellable. A coherence failure on the worker (a sync that cannot keep state consistent) is unrecoverable and ends the process rather than continuing on corrupt state.

Parallel evaluation is deliberately not adopted: a correct parallel red-green engine is research-grade, and because evaluation is demand-driven there is no eager cold pass for it to fan out across cores. The shared-pointer and memo-table types are kept behind thin aliases so a future parallel retrofit, if ever justified by measurement, stays localized.

### Differential correctness

The engine's correctness is held to a differential check against `analysis::run_full` — a clean from-scratch checker built fresh for the current file set, never an incremental path. (Comparing against an incremental path could ratify a stale result on both sides; the from-scratch rebuild cannot.) Over randomized and adversarial edit streams — interleaved edits and queries, add/delete/re-add, package↔script reclassification, renames, re-export and value-reference cycles, malformed input — after every edit the engine's output must equal a fresh full rebuild of the then-current state, byte-exact on rendered diagnostics and per-cursor-position for every IDE feature. This from-scratch checker is retained permanently as the regression net; it is also the command-line path, since a one-shot batch check needs no incrementality.

### IDE queries

The interactive features — hover, completion, go-to-definition, find references, rename, inlay hints, signature help — are written once, generic over an `IdeDatabase` fact-provider trait. Both the from-scratch oracle and the engine-backed view implement that trait, so the identical orchestration runs on both and the engine-backed output is differential-checked per cursor position (cross-file included) against the oracle. Per-keystroke features are O(1) over the cached typecheck of the queried file plus a sub-linear span lookup — a point query on an unchanged file triggers no re-inference; cross-file features (find references, rename, workspace symbols) may scan the project behind a cheap text prefilter but never resurrect a persistent occurrence index.

### Performance characteristics

A body edit's recompute is bounded by its blast radius — the edited file plus its referrers — with no whole-package fold (the names-only def-map and the per-symbol scheme both cut off). Memory is linear in workspace size: full memoization trades roughly a constant factor of space for incrementality, and demand-driven evaluation pays the cold cost lazily per query. The residual per-edit cost that is *not* flat in workspace size is the red-green validation walk — a cheap revalidation per file with no inference; driving that sub-linear (durability tiers or sharded def-maps) is a known, deferred optimization.

## Diagnostics

Diagnostics are part of the product surface, not a side effect.

Diagnostic data should stay structured until rendering so wording, ranges, and phase-local errors can be managed consistently.

## Testing seams

The architecture exposes stable phase boundaries for fixture testing, plus the differential cross-check above.

At the architectural level, the important requirement is that the implementation support direct testing of:

- annotation and type syntax parsing
- lowering results
- naming results
- successful checked output
- rendered diagnostics
- engine output against a from-scratch rebuild over edit streams
