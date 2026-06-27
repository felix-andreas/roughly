---
title: Architecture
description: Implementation architecture and durable design constraints of the analysis crate
---

This document is the authoritative implementation architecture for the `analysis` crate.

The [Typing Reference](/typing-reference) is the authoritative user-facing typing contract. This page defines the implementation boundaries needed to realize that contract. Keep it focused on durable phase boundaries and representation boundaries.

## Role of this document

Use this document for:

- phase boundaries
- representation boundaries
- lint architecture
- naming and scope architecture
- typechecking architecture

Do not use this document for:

- a changelog
- a task tracker
- a restatement of user-facing typing rules

## Pipeline

The analysis phase surface is:

parsed syntax -> `lint` -> `lower` -> `naming` -> `typecheck` -> checked-file results and diagnostics

`check` is the orchestration entry point around that pipeline. It wires phases together and returns file results, but it is not itself a semantic phase.

Syntax parsing is not a `analysis` crate phase. The checker may receive already-parsed syntax from `roughly` or from tests.

Diagnostics are not a separate phase. They are structured outputs produced by lint, lowering, naming, and typechecking.

## Phase contracts

### `lint`

Input:

- one `workspace::Document`
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

#### Incremental model

Typechecking is incremental at document grain through a two-phase interface model:

- The interface phase computes each package document's exported value schemes by a package-level
  fixed-point. Each round builds the package-global table (the winning document's exported scheme
  per name, `Unknown` where not yet computed), then recomputes every document whose version, the
  type-definition fingerprint, or a referenced scheme changed, binding the table's schemes for the
  names it references and re-extracting its exports. Iterating to a fixed point lets re-exports and
  forward references resolve both within and across files (`second <- first` exports `first`'s
  scheme even when `first` lives in another file). Each document's interface is cached by document
  version, the type-definition fingerprint, and a dependency fingerprint of the referenced schemes,
  so an edit only re-derives the changed document and its dependents; the round cap stops genuine
  cycles (which keep `Unknown`).
- The package interface table maps each package-global name to the winning document's exported
  scheme. Its rendered form, together with the type-definition fingerprint, is the environment
  fingerprint.
- The check phase checks every candidate document (package files and scripts) against the interface
  table, binding only the schemes the document references. The result is cached by document version,
  the type-definition fingerprint, and that document's own dependency fingerprint — the rendered
  schemes of exactly the package-global names it references — rather than a package-global key. The
  candidate set is chosen by reverse-dependency routing (`dirty docs ∪ reverse-deps of changed
  exports`; see [Reverse-dependency invalidation](#reverse-dependency-invalidation-m3)), so a
  body-only edit rechecks just the edited document, and an interface change rechecks exactly the
  changed document plus the documents that reference the changed name (`k + 1`), leaving independent
  documents untouched. A type-definition change is still package-global today and falls back to the
  full document set.

Consequences that are part of the contract:

- cross-file references see the exporting document's generalized scheme, and a re-export's own
  exported scheme is derived from what it re-exports; type information still does not flow back
  across file boundaries through inference (a call in one file never changes the inferred type of a
  function defined in another file)
- interface schemes move between per-document inference states by importing: quantified
  variables are re-bound to fresh local ids, and stray free variables erase to `Unknown`
- `typecheck` returns the set of documents whose output was recomputed so callers can republish
  exactly those diagnostics
- checking recovers per top-level expression, so every error in every document is reported

Generalization is level-based: variables created while inferring a binding's value live one
level deeper than the binding boundary, unification propagates the lower level outward, and
generalization quantifies exactly the variables deeper than the current level without walking
the environment.

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

The desired near-term file split is recorded on the [Structure](/structure) page.

## Incremental analysis

Analysis should be operation-driven rather than running one fixed whole-package pipeline on every
edit.

The single source of truth is one set of versioned retained phase outputs owned by `Analysis`.
Different operations request different freshness floors over those retained artifacts. They must not
create separate semantic caches per IDE action.

Freshness is split by scope:

- document-scoped phases compare their retained outputs against a document version
- package-scoped phases compare their retained outputs against a package version

The intended trigger policy is:

- on edit or keystroke, refresh document-scoped phases for the current document: `lint`, `lower`,
  and file-local naming preparation
- on hover, rename, and similar IDE actions, first ensure current document-scoped phases for the
  unsaved buffer, then request only the minimum package-scoped naming or typecheck work that action
  requires
- on save, request full semantic diagnostics for the saved package snapshot, while still rerunning
  only the package-scoped work whose retained outputs are stale

This model should preserve two boundaries:

- phase boundaries remain stable even if scheduling changes later
- future finer-grained invalidation should refine package-scoped work, not replace the retained
  artifact model

### Reverse-dependency invalidation (M3)

This is the design M3 implements to make package-scoped invalidation precise instead of
package-wide. It refines the package-scoped phases above; it does not replace the retained-artifact
model. Per-document dependency re-keying, the reverse-dependency index, the per-edit dirty set, and
the fixture-visible recompute scope are implemented; the check-phase prose in
[the typecheck incremental model](#incremental-model) describes the per-document dependency
fingerprints and reverse-dependency routing that are now in effect.

Four pieces:

1. **Per-document dependency re-keying.** The round-2 typecheck (the check phase) is re-keyed on
   each document's *own* dependency fingerprint — the per-document set of referenced schemes already
   computed in round-1 — instead of one global environment fingerprint. A document's check cache is
   then invalidated only when a scheme it actually references changes.
2. **Reverse-dependency index.** A package-global map from `Symbol` (a package-visible name) to the
   set of `DocumentId`s that refer to it.
3. **Per-edit dirty set.** The set of changed `DocumentId`s is carried into the package-scoped
   phases. The recompute candidate set is `dirty docs ∪ reverse-deps of changed exports`.
4. **Fixture-visible recompute scope.** The recomputed `DocumentId` set and the reason per document
   (body-edit vs interface-change) are exposed so the routing is testable, not just an internal
   optimization.

#### Maintenance invariant (single source of truth)

The reverse-dependency index is a pure function of the per-document naming outputs:

```
index == ⋃ over docs D of { (s, D) : s ∈ non_locals(D) }
```

It is *derived and patched per document* — to update a document, remove its own entries (the edges
keyed by that contributing `DocumentId`) then insert its new ones — never a hand-synced mirror.
Edges are keyed by the **name a reference attempts to resolve**, not by the resolved target. Keying
on the attempted name (not the resolved binding) is what keeps the following correct without special
cases:

- forward references to a not-yet-defined global,
- shadowing / last-writer-wins winner flips,
- deletion of a definition,
- local↔global transitions.

A debug-only assertion compares the patched index against a full rebuild and guards against drift.

#### One-hop correctness premise (contract precondition)

A single reverse-dependency hop suffices to pick recompute candidates **only because inference never
flows across files**: a document's exported scheme is a pure function of its own source, not of its
dependencies' bodies or inferred types. So changing document A's exports can only affect the
documents that directly reference A's exported names — one hop.

Re-exports (a top-level binding that aliases another global) are the genuinely transitive case. They
are not handled by extra reverse-dependency hops; they are handled by the existing two-round
interface fixed-point, which remains the convergence safety net.

If cross-file inference is ever added, the one-hop guarantee is void and this routing must be
replaced by a worklist iterated to a fixed point.

#### Find-references stays on the text prefilter

Find-references intentionally does *not* gain a persistent reverse-reference index. A full
reverse-*reference* index is exactly the fragile mirrored state rust-analyzer deliberately avoids;
find-references stays on the text-prefilter path. The reverse-*dependency* index here is the narrow,
cheaply derivable structure used only for invalidation routing — not a general reference store.

#### Exit proof (targeted, made durable as a fixture)

The design targets, and a fixture makes durable, the following:

- an interface change to a global `G` referenced by `k` documents recomputes exactly `k + 1`
  documents (`G`'s own document plus its `k` referrers);
- a body-only edit recomputes exactly `1` document and renders no `O(package)` fingerprint.

### Incremental package naming (M4 — target)

This is the design implemented next; it is **not yet in effect**. M3 made the interface fixed-point
and round-2 typecheck `O(blast-radius)`, but `resolve_package` still calls
`rebuild_package_naming`, which rebuilds the entire global binding table by scanning every package
document on each `package_version` bump (~111ms single-file recheck at 500 files). That is the last
`O(package)` cost on a body-only edit. M4 makes package naming incremental: when a document's
exported top-level names change, patch the affected names instead of rebuilding.

#### Source of truth and derived structures

Same discipline as the M3 reverse-dependency index — a pure fold of per-document naming outputs,
never a hand-synced mirror:

- **Per-document exported names.** Each document `D` contributes the set of `(Symbol, BindingInfo)`
  for its top-level assignments — exactly `NamesLocal.bindings` filtered to
  `kind == BindingKind::TopLevelAssignment`. A pure function of `D`'s local naming.
- **Per-name candidate index.** For each package-global `Symbol` `N`, the set of documents defining
  `N`, ordered by the **same** package document order winner selection uses — package
  path-lexicographic order (`package_document_ids`), **not** `DocumentId` numeric order.
  `Winner(N)` = the last candidate in path order (last-writer-wins).
  `global_bindings[N] == Winner(N)`, materialized for `O(1)` lookup.

#### Maintenance invariant (single source of truth)

```
candidate_index == ⋃ over docs D of { (N, D, binding) : N is a top-level binding of D }
global_bindings[N] == the path-last candidate of N
```

The index is patched per document at the local-naming recompute site and on delete (remove `D`'s own
entries by contributing `DocumentId`, then insert its new exported set), driven by the M3 dirty set.
A debug-only full-rebuild drift assertion (as in M3) guards it.

#### Incremental update on a document change

- Compute `D`'s new exported set; diff against `D`'s prior contribution (read from the candidate
  index's current membership of `D`). Affected names = (names `D` drops) ∪ (names `D` adds). A pure
  body edit that changes no top-level name leaves the exported set identical → zero affected names →
  no `global_bindings` change, which is the flat-recheck win.
- For each dropped name, remove `D` from `candidate_index[N]`; for each added name, insert `D` in
  path order. Recompute `Winner(N)` = path-last candidate for every affected `N` and update
  `global_bindings` (or remove `N` if it has no candidates left).

#### Diagnostics (correctness-critical)

Keep overwrite-warning pairs exact.

- **Overwrite warnings** are a pure function of a name's *ordered* candidate list: every candidate
  except the last gets "overwritten by a later top-level binding"; every candidate except the first
  gets "overwrites an earlier top-level binding". So they are name-keyed: when `candidate_index[N]`
  changes, regenerate `N`'s overwrite diagnostics for exactly the documents in `candidate_index[N]`,
  replacing the prior set for `N`. Store naming diagnostics so a name's overwrite contributions can
  be replaced without dropping unrelated diagnostics.
- **Builtin/namespace-shadow warnings** are per-`(document, binding)` and independent of winner
  selection; they are regenerated from `D`'s own bindings when `D` changes.

#### Correctness premise (contract precondition)

Winner selection is last-writer-wins in package path order, so a name's winner depends **only** on
the set of documents defining it and their path order — never on document bodies — so patching per
affected name is complete. The debug full-rebuild assertion enforces equality with a from-scratch
rebuild.

#### Scope and a deferred piece

M4 targets the value `global_bindings` table plus its naming diagnostics. The type-definition index
(`build_type_index` over `@type` / `@alias`) is also rebuilt in `rebuild_package_naming` today;
making it incremental pairs naturally with tightening the still-package-global
`type_definitions_fingerprint` from M3. This is **deferred**, not done.

#### Trigger change

Replace the monolithic `package_naming_output.version == package_version` full-rebuild guard with
incremental maintenance driven by the dirty set: `resolve_package` patches the dirty documents'
contributions and recomputes only affected names, leaving `package_naming_output` as the
materialized, versioned result.

#### Test plan (focused shadowing fixture suite)

Each case asserts both the resolved winner (via name resolution / a global-naming fixture) **and**
the exact overwrite-warning set, and that an incremental update matches a full rebuild:

- a document newly defining a name another document also defines (winner flips if the new one is
  path-later; both gain/lose the paired overwrite warnings);
- a document dropping a name (winner falls back to the next path-latest; the dropped document's and
  old winner's overwrite warnings update);
- the sole definer of a name being deleted (the name leaves `global_bindings`);
- adding an earlier-path document (it becomes a loser to later definers);
- rename and re-add.

#### Slice plan

- **M4.1** — add the per-document exported-names + per-name candidate index, maintained incrementally
  with the drift assertion; `global_bindings` still derived by full rebuild (pure addition, no
  behavior change).
- **M4.2** — make `resolve_package` patch `global_bindings` + overwrite diagnostics from the
  candidate index incrementally (the behavior-changing slice; guarded by the shadowing fixture suite
  + drift assertion + existing `naming_global` fixtures).
- **M4.3** (deferred/optional) — incremental type index, paired with per-doc type-def fingerprint.

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
