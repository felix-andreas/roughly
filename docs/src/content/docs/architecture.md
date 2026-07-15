---
title: Architecture
description: Implementation architecture of Roughly's analysis engine and its durable phase and representation boundaries
---

This document is the authoritative implementation architecture for Roughly's analysis. Two crates cooperate:

- **`engine`** — a generic red-green *memoized query* core. It holds no R knowledge and does not depend on `analysis`; it is the substrate that turns the analysis phases into incrementally-recomputed queries with automatic, dependency-tracked invalidation.
- **`analysis`** — the computational phases: parsing (over tree-sitter), lowering, naming, type inference, lint, and the IDE logic. The engine drives incremental analysis by running these phases as query bodies. `analysis` also exposes a clean *from-scratch* checker, `run_full`, retained as the correctness oracle (see [Differential correctness](#differential-correctness)).

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
- **Validation (red-green)** — a memo is *green by revision* (already verified this revision), *green by durability* (see below), *green by early cutoff* (all of its dependencies revalidated to equal values, so it need not re-run), or *red* (a dependency changed; recompute).
- **Durability** — every input declares how often it changes (`LOW`: the open documents an editor mutates per keystroke; `HIGH`: the unopened on-disk corpus, project membership, config), and every memo records the minimum durability it transitively read. A memo whose level saw no input change since its last verification is green in O(1), so a keystroke's validation never walks the unopened files' chains — and because each all-files fold is split into a durable (non-open) sub-fold plus an open-file overlay, the folds green in O(open files) too, leaving the whole per-keystroke walk bounded by the open set plus the edited file's chain (size-independent; pinned by a committed witness). Opening a file downgrades its text input; the downgrade counts as a change at the old level and the validation walk re-records durability minimums as it goes, so even dependents whose values never change pick up the weaker level (otherwise the next keystroke would be invisible to them — a stale read).
- **Value-equality early cutoff** — when a recomputed query produces a value equal to its previous value, its dependents do not recompute. This is what lets an edit that does not change an exported scheme stop at that boundary instead of propagating onward.
- **Input removal** — deleting an input leaves a tombstone, so a dependent revalidates against the now-smaller input set instead of re-executing an absent input.
- **Accidental-cycle guard** — a derived body that transitively fetches itself fails loudly rather than overflowing the stack; this is treated as a programming error. (The one *intended* cycle is contained in a single body — see [the re-export cycle](#the-re-export-interface-cycle).)

### The query graph

Inputs (set from outside, never computed):

- **`source_text(file)`** — per-file source. The high-churn input: every keystroke sets it. The value is the document's text (a rope) plus, for open documents only, the editor's incrementally-maintained parse tree; the on-disk corpus is fed text-only, and a tree is derived on demand (into a small bounded cache) when one of the file's tree-reading queries actually runs — retaining a tree per file would dominate the resident set at large scale (~60× the source bytes), while the tree is a pure function of the text.
- **`open_files`** — the open-document set, changing only on open/close. A pure performance seam (fold values are identical for any contents): it splits each all-files fold into a durable sub-fold over non-open files plus an open-file overlay, so a keystroke's validation walk is O(open files), not O(package). Hosts that never set it get the empty default.
- **`document_kind(file)`** — package source vs. script, kept as a *separate* input so a text-only edit does not invalidate through a kind read.
- **`project_files`** — the set of files that exist. This is the single source of truth for membership; adding or removing a file is an edit to this input plus the file's own `source_text` / `document_kind`.
- **`config`** — the project `roughly.toml` (`[format]`, `[lint]`, `[check] typing/unused/strict`).
- **`stdlib_stubs`** — the immutable standard-library stubs. Set once; it never invalidates anything.

Derived queries (every edge is a recorded `fetch`, so the dependency is automatic):

| Query | Reads | Role |
| --- | --- | --- |
| `lower(file)` | `source_text(file)` | HIR (the tree is a pure function of the text, materialized on demand) |
| `local_naming(file)` | `lower(file)`, `document_kind(file)` | file-local resolution; also yields the file's exported-name set |
| `package_symbol_index` | `project_files`, `open_files`, its durable sub-fold, the open files' exported-name sets | the def-map: name → winning defining/re-exporting item. **Names only, no types.** Changes on *structural* edits (add/remove/rename a top-level binding, add/remove/reclassify a file), not on body edits. Like every all-files fold, it is split into a durable (non-open) sub-fold — green in O(1) per keystroke via durability — plus an open-file overlay merged by `project_files` position, value-identical to folding every file directly |
| `defining_item(symbol)` | `package_symbol_index` | a firewall projecting one symbol's winner out of the index, so a change to one symbol's winner cuts off for the others |
| `completion_exports(file)` | `lower(file)`, `local_naming(file)`, the file's exported-name set | one file's exported globals as ready-made completion entries (label, kind, callability); value-equal across body edits |
| `package_completion_index` | `package_symbol_index`, each winner's `completion_exports` | the package-wide completion source; a completion request fetches this one memo instead of touching every file's module and naming |
| `global_scheme(symbol)` | `defining_item(symbol)`, then the winning file's inference for that item (or the re-export cycle body) | the per-symbol exported type scheme |
| `typecheck(file)` | `lower(file)`, `local_naming(file)`, `config`, and `global_scheme(s)` for **each cross-file or forward same-file symbol `s` the file references** (a same-file reference after its definition is resolved by the inference walk itself and needs no interface edge) | the file's **one** whole-file HM inference: it yields both the authoritative check (diagnostics, recorded expression types) and the file's exported value schemes. A per-file `exported_schemes` projection shares the exports allocation and is the value-eq seam referrers cut off on, so a body edit costs a single inference and never re-runs `global_scheme` unless a scheme actually changed |
| `diagnostics(file)` | `typecheck(file)`, `lint(file)`, lowering diagnostics, `config` | the rendered diagnostics; `config` gates typing/unused/strict |

### Automatic dependency-tracked invalidation

The fine-grained, per-symbol interface layer is what makes invalidation precise. `typecheck(file)` records `global_scheme(s)` for exactly the symbols the file references. When a global's scheme changes, only that symbol's `changed_at` advances, and only the `typecheck` memos that recorded it revalidate. That recorded per-symbol dependency set *is* the reverse-dependency map — reconstructed automatically and exactly, with nothing to patch and nothing to drift.

The consequences:

- Editing a function body changes one symbol's scheme and re-typechecks only its referrers. The names-only `package_symbol_index` does not re-fold for a body edit, because the file's exported-name set is unchanged and cuts off.
- `typecheck(file)` never reads `project_files` or `package_symbol_index` directly — it reaches the file set and the def-map only *behind* the per-symbol `global_scheme` / `defining_item` firewall — so adding an unrelated file cannot invalidate a file that does not reference a symbol whose winner changed.
- There is no hand-maintained reverse-dependency index, dirty-set, or dependency fingerprint. Each of those was a stand-in for what the recorded `fetch` graph plus value-equality cutoff now provide directly, and each carried a class of silent-staleness bug (a mirror the untracked read path could bypass). With `fetch` as the only read path, that bug class is structurally impossible.

### The re-export interface cycle

R allows mutual typed re-exports (`a <- b` in one file, `b <- a` in another), a genuine dependency cycle the acyclic core cannot express through plain `fetch` recursion (it would re-enter a key already being computed and trip the accidental-cycle guard). It is resolved inside a single query body that owns the whole strongly-connected component: a bounded fixed-point that iterates to convergence — acyclic re-export and forward-reference chains are monotone (each scheme transitions at most once, `Unknown` to concrete) and converge within a bound proportional to the number of globals — with an oscillation guard that pins a genuinely cyclic symbol to `Unknown`, collapsing the cycle so the loop converges. Downstream queries depend on its converged result normally, and value-equality cutoff stops propagation when that result is unchanged. This is the one non-trivial query body; it carries its full correctness burden (the convergence bound and the oscillation guard) rather than dissolving.

Interface edges are file-granular (they mirror exactly the schemes a whole-file inference imports), and real packages routinely reference each other's files both ways — so these components are routinely whole file *clusters*, not just re-export pairs. The fixed point therefore does not re-infer member files wholesale per round. A member file that provably decomposes — every top-level binding a single-assignment function or scalar literal, no letrec group, no captured-write re-pass, and no other statement writing the top-level frame — is re-inferred per member *definition*: each definition is checked against the current round's table (plus the memoized schemes of everything outside the component, and locally-resolved same-file helpers), which is provably the same environment the whole-file walk would give it. Files that do not decompose keep the whole-file round. Both granularities skip any unit none of whose in-component reads changed in the previous round — the round function is pure in those reads, so the previous output is reused and a converging chain costs its frontier per round, not the whole cluster. Per-file contribution maps merged in file order preserve the exact last-writer-wins semantics for a symbol exported by several member files.

### Concurrency and diagnostics scheduling

The engine uses non-thread-safe interior pointers, so it is not shared across threads. The shipped language server runs it on one dedicated worker thread, off the main thread, and is **demand-driven**: editor queries compute only what they ask for — open files and their dependents. Live responsiveness comes from **cooperative cancellation and scheduling**, not parallelism (the model rust-analyzer uses, adapted to a single-owner worker):

- **Document sync always runs to completion.** Applying an edit to the engine's inputs is cheap and uncancellable; a sync failure is a coherence failure and ends the process rather than continuing on corrupt state.
- **Diagnostics publish in two waves (push clients).** An edit immediately publishes the classes that are pure per-file functions of the parse — syntax, lint, local naming — so a syntax squiggle never waits on type checking. The full semantic set (type errors, package naming, strict origins, unused) follows as a superseding version-less publish computed at **idle time**.
- **Idle work yields to everything.** Deferred semantic publishes run only when the job queue is empty, under an idle-interrupt token: the worker resets the token before each empty poll, and the frontend flips it after enqueuing *any* job — an edit, a read, or a notification. That pairing makes preemption lossless (a job enqueued before the poll is received; one enqueued after it flips the token after the reset, so the idle unit observes it at its next cancellation check), and in-flight idle work abandons at the next recompute boundary, unwinds without committing a partial memo, and is requeued. A typing burst therefore costs one settled semantic computation, not one per keystroke — latest edit wins.
- **Interactive reads are cancellable by edits.** Each edit flips the read-cancellation token; an in-flight cross-file read abandons and answers empty (best-effort lookups) or a retryable protocol error (pull diagnostics, rename). A mere read never cancels another read.
- **A background prime warms the cold start.** After `initialized`, package files are fetched one per idle slot (preempted like any idle work), so the first interactive request finds the interface graph computed instead of paying whole-reachable-project inference. The prime is a cache-warmer only: it publishes nothing and changes no semantics.
- **No document is left stale.** An owed semantic publish survives cancellation by requeuing; a save (or config reload) marks every open document owed, and pull clients are asked to re-pull via `workspace/diagnostic/refresh` instead.

Parallel evaluation is deliberately not adopted: a correct parallel red-green engine is research-grade, and because evaluation is demand-driven there is no eager cold pass for it to fan out across cores. The shared-pointer and memo-table types are kept behind thin aliases so a future parallel retrofit, if ever justified by measurement, stays localized.

Lowering itself is **error-tolerant**, which is what makes the first wave useful mid-edit: a tree with syntax errors keeps every well-formed statement (a broken assignment even keeps its definition, with the value degraded to a typed hole — an annotated definition keeps its *declared* scheme), so a file's exports do not flap while one construct is half-typed. The committed engine witness pins the consequence: a malformed round-trip re-folds the package symbol index **zero** times and re-typechecks only the edited file. Broken regions emit their syntax error and nothing else — holes resolve no names, prove no types, and record no strict origins (see the typing reference on syntax errors).

### Differential correctness

The engine's correctness is held to a differential check against `analysis::run_full` — a clean from-scratch checker built fresh for the current file set, never an incremental path. (Comparing against an incremental path could ratify a stale result on both sides; the from-scratch rebuild cannot.) Over randomized and adversarial edit streams — interleaved edits and queries, add/delete/re-add, package↔script reclassification, renames, re-export and value-reference cycles, malformed input — after every edit the engine's output must equal a fresh full rebuild of the then-current state, byte-exact on rendered diagnostics and per-cursor-position for every IDE feature. This from-scratch checker is retained permanently as the regression net — and only as that: `roughly check` runs on the engine (the same query graph the server uses, through a shared diagnostics assembly), so the CLI, the editor, and the differential all exercise one implementation, and the CLI inherits every engine performance property (per-symbol interface firewalls, per-definition SCC rounds, one parse per file).

### IDE queries

The interactive features — hover, completion, go-to-definition, find references, rename, inlay hints, signature help — are written once, generic over an `IdeDatabase` fact-provider trait. Both the from-scratch oracle and the engine-backed view implement that trait, so the identical orchestration runs on both and the engine-backed output is differential-checked per cursor position (cross-file included) against the oracle. Per-keystroke features are O(1) over the cached typecheck of the queried file plus a sub-linear span lookup — a point query on an unchanged file triggers no re-inference; cross-file features (find references, rename, workspace symbols) may scan the project behind a cheap text prefilter but never resurrect a persistent occurrence index.

### Performance characteristics

A body edit's recompute is bounded by its blast radius — the edited file plus its referrers — with no whole-package fold (the names-only def-map and the per-symbol scheme both cut off). Memory is linear in workspace size: full memoization trades roughly a constant factor of space for incrementality, and demand-driven evaluation pays the cold cost lazily per query. The red-green validation walk is bounded too: unopened files' chains validate in O(1) via durability, and each all-files fold is split into a durable (non-open) sub-fold plus an open-file overlay, so the per-keystroke walk is O(open files + the edited file's own references) — size-independent, not O(package). At rest, every point query touches a constant number of memos — completion included, since the global source is one memoized fold. Both properties are pinned by committed counter witnesses in the engine's benchmark suite (constant at-rest prime scope; the same post-keystroke walk at 100 and 300 files). Memory at rest holds no parse trees beyond the open documents and a small on-demand cache.

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
