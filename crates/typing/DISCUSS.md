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
