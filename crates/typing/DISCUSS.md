# Typing Crate Discussion

This file is scratch space for active design discussion.

Use it for short summaries or temporary notes during a design conversation.

## Naming suite restructure

Resolved:

- the naming fixtures are split under:
  - `tests/naming/README.md`
  - `tests/naming/local/`
  - `tests/naming/global/`
- `README.md` is now the authoritative naming-suite contract
- `global` is the primary contract and mirrors the core lexical sections from `local`
- type-name coverage lives in `global`, not in a separate local type suite
- script-file naming behavior is now recorded in `SEMANTICS.md`

## Roughly integration

Resolved for the first milestone:

- `roughly` keeps its current fast syntax and lint diagnostics on `did_change`
- typing diagnostics run only on `did_save` for now
- `roughly::index::index` stays in place for outline/document symbols for now
- hover, goto-definition, references, and rename should not block this integration
- `typing::Analysis` should retain package context instead of dropping closed documents eagerly
- duplicated parse state between `roughly` and `typing` is acceptable for the first milestone

Implication:

- a fast typing diagnostics API is no longer a blocker for the first usable integration
- the first usable rollout can be:
  - keep `roughly` diagnostics on `did_open` / `did_change`
  - maintain `typing::Analysis` document state by real path
  - run full typing diagnostics on `did_save`

Main work still needed:

- stop using the temporary `current.R` typing adapter and use real document paths in
  `typing::Analysis`
- make `roughly` maintain package documents in `typing::Analysis`, not only the currently open file
- ensure non-open file changes from file watching keep typing state in sync
- fix package document ordering in `typing` so package-wide checks follow package collation rather
  than insertion order

Deferred until after the first milestone:

- a fast typing path for `did_change`
- typing-backed hover and goto-definition
- storing tree-sitter node ids on lowered HIR for typing-backed editor lookups
- moving outline/indexing from `roughly` into `typing`

## Hover by phase

Target:

- hover should be able to show layered information from:
  - lowering
  - naming
  - typing
- the natural output shape is a stacked markdown view with one section per phase that has useful
  information for the hovered syntax

What already exists:

- lowering already stores HIR per document in `Analysis.lowering.modules`
- HIR expressions and definitions already carry source ranges
- naming already stores:
  - local expression-to-binding resolutions
  - final package resolutions
  - binding metadata including module and source range
- `roughly` hover already knows how to find the tree-sitter node at a document position and render
  markdown

Main gap:

- typecheck results are thrown away today
- `TypecheckStore` is empty, so after `typing::check` finishes there is no persistent
  expression-to-type map for hover to read

Current state:

- `typing::Analysis::hover(path, position)` now exists
- hover lookup uses the smallest HIR expression or definition whose range contains the position
- hover already exposes:
  - lowering information
  - naming information for expressions
- `roughly` hover now renders that layered analysis output
- typing hover is still blocked on persisted typecheck results

Needed for a useful first hover:

1. A stable way to map a hover position to the relevant lowered item
   - initial version: use the smallest HIR expression or definition whose range contains the point
   - later version: store tree-sitter node ids on HIR once typing-backed editor lookups need the
     extra stability

2. A persistent typecheck result store
   - store inferred type per expression id
   - likely also store binding/type scheme information for bound names
   - this should be written during `run_typecheck`, not reconstructed ad hoc by hover

3. A hover-facing analysis API
   - something like `analysis.hover(path, point)` returning a structured `HoverInfo`
   - `roughly` should render that, rather than poking through several internal maps directly

Recommended section contents:

- lowering
  - HIR kind for the hovered expression or definition
  - for symbols: the lowered symbol name
- naming
  - resolved binding id
  - binder kind and binder source range
  - whether the resolution is local or package-global
  - unresolved name if naming failed
- typing
  - inferred type for the hovered expression
  - type scheme for bindings when relevant
  - clear “not available because typing failed” state when typecheck did not complete

Recommended implementation order:

1. Make `TypecheckStore` real and persist inferred types
2. Add typing hover section
3. Extend hover target coverage if we want richer type-definition or annotation hover

Likely blockers:

- range-only lookup can be ambiguous for nested expressions, so the selection rule must be explicit
  and stable
- type inference currently reports diagnostics but does not preserve intermediate results, so some
  refactoring in `typecheck.rs` is required before hover can show types
- if we want hover on annotations or type definitions, we may also want definition-oriented hover
  data, not only expression-oriented hover

## Open decisions

1. How should `roughly` populate package files into `typing::Analysis` for the first milestone?
   Current options:
   - preload package files from `R/` on initialize (yes re-use existing codee)
   - lazily load package files on first save / first package-typing request

2. How should `roughly` handle watched non-open file changes once typing state exists?
   Current options:
   - update `typing::Analysis` from disk on watched file create/change/delete
   - invalidate package typing state and lazily reload later (we need a way to mark files as dirty in AnalysisState, and then on next run update dirty files)

3. How should fixture coverage model real package collation once the harness can read
   `DESCRIPTION`/`Collate`? (skip for now)

4. Do we want nested inner type-parameter shadowing coverage to wait on future higher-rank syntax
   support, or do we want a different surface form to express that case earlier? (skip for now)
