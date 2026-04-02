# Roughly Typing Save Diagnostics Integration [planning]

## Goal

Integrate the `analysis` crate into `roughly` for a first usable milestone that publishes typing
diagnostics on save while leaving the existing fast `roughly` diagnostics path in place during
editing.

The target result is:

- `roughly` maintains real document state in one long-lived `analysis::Analysis`
- `analysis` runs against real document paths instead of the temporary `current.R` adapter
- `did_save` publishes package-aware typing diagnostics
- `did_change` keeps using the existing fast `roughly` syntax and lint diagnostics
- outline, hover, goto-definition, references, and rename are not blockers for this milestone

## Non-goals

- replace `roughly` fast diagnostics on `did_change`
- add a fast typing diagnostics path during typing
- migrate document outline/symbol indexing into `analysis`
- ship typing-backed hover, goto-definition, references, or rename
- solve long-term incremental rechecking of dependent package files
- eliminate duplicated parse state between `roughly` and `analysis`

## Unresolved questions

- Should disabled typing-backed editor features be removed from advertised LSP capabilities
  immediately, or kept behind explicit runtime errors during the transition? (no we do explicit errors)
- Should `Analysis` track dirty-on-disk package files directly, or should `roughly` own the dirty
  path set and resync `Analysis` before each full typing run? (analysisstate owns it)

## Settled direction

- keep `roughly` fast syntax and lint diagnostics on `did_change`
- run typing diagnostics only on `did_save` for the first milestone
- keep `index::index` for document symbols and outline for now
- retain package context in `analysis::Analysis` rather than dropping closed documents eagerly
- preload package files from `R/` into `analysis::Analysis` during server initialization
- when watched non-open package files change, mark them dirty and refresh them before the next full
  typing run
- use real workspace-relative document paths in `analysis::Analysis`
- defer tree-sitter node ids in HIR until typing-backed hover or goto-definition work starts

## Target shape

### Server behavior

`roughly` should behave as follows:

- `initialize`
  - index package files as today
  - preload package files from `R/` into `analysis::Analysis`
- `did_open`
  - update both the existing open-document parse state and the corresponding `analysis::Analysis`
    document
  - keep publishing the current non-typing diagnostics path
- `did_change`
  - update only the existing fast `roughly` diagnostics path
  - update the open-document copy in `analysis::Analysis`
  - do not run typing diagnostics yet
- `did_save`
  - refresh any dirty non-open package files in `analysis::Analysis`
  - run full typing analysis for the package
  - publish typing diagnostics for the saved file
- `did_close`
  - remove the open-document parse state from `roughly`
  - retain the file in `analysis::Analysis` as package context
- watched file changes for non-open package files
  - update outline state as today
  - mark the corresponding typing document dirty for later refresh

### Typing-side support

For this milestone, `analysis` does not need a new fast frontend diagnostics API.

It does need:

- path-based document synchronization that is safe to call from `roughly`
- a way to refresh one document from disk by path before a full package run
- package document ordering that follows package collation rather than insertion order
- convenient retrieval of diagnostics for one document after a full package run

## Constraints and blockers

The main blockers for this milestone are:

1. `analysis` package ordering is still insertion-order rather than package collation order.
2. `roughly` does not yet preload package files into `analysis::Analysis`.
3. `roughly` does not yet keep non-open watched file changes synchronized with typing state.
4. the current `roughly::typing_diagnostics` adapter still uses the temporary `current.R` document
   shape instead of real package paths.

The following are explicitly not blockers for this milestone:

- lack of a fast typing path on `did_change`
- lack of typing-backed hover or goto-definition
- lack of typing-backed outline/document symbols
- duplicated parse state between `roughly` and `analysis`

## Implementation plan

### 1. Record the milestone and align working docs [done]

- capture the settled first-milestone behavior in `DISCUSS.md`
- add this project plan
- update `TODOS.md` so the integration work points here

### 2. Make package ordering explicit in `analysis` [pending]

- stop using `DocumentId` insertion order as package file order
- introduce package-path ordering based on current semantics:
  - default `C`-locale collation of package source files
  - leave `DESCRIPTION` / `Collate` support as a later follow-up if needed
- make `Analysis::package_document_ids()` return documents in that package order
- add focused tests for package ordering independent of editor open order

Reasoning:

- package-global naming and duplicate-resolution diagnostics are wrong if file order follows editor
  insertion order
- this must be correct before `roughly` relies on package-wide typing on save

### 3. Add package preload support in `roughly` initialization [pending]

- reuse the existing workspace `R/` scan during server startup
- parse package files from disk into `analysis::Analysis` using their real paths
- ensure package files that are later opened reuse the same path entry instead of creating a second
  typing document
- keep non-package files out of the package-contributing set

Verification:

- saving an open file can resolve globals and type declarations from unopened package files
- duplicate top-level naming warnings still reflect package-wide state

### 4. Replace the temporary typing adapter with real-path analysis updates [pending]

- stop using `roughly::typing_diagnostics` as a `current.R` wrapper
- add server-side helpers that:
  - insert or replace one `analysis::Analysis` document from open-buffer contents
  - apply incremental edits to the corresponding typing document on `did_change`
  - run full typing analysis on demand
- keep the LSP diagnostic conversion logic, but source diagnostics from real document ids and paths

Cleanup:

- remove the temporary `current.R` document convention from the integration path
- keep the old module only if a small conversion helper is still useful

### 5. Track dirty non-open package files [pending]

- add server-side state for package files that changed on disk while not open
- on watched file create/change/delete:
  - update `workspace_items` as today
  - mark the typing path dirty instead of eagerly reparsing everything
- before the next full typing run:
  - reload dirty files from disk into `analysis::Analysis`
  - delete removed files from `analysis::Analysis`
  - clear the dirty set after successful synchronization

Reasoning:

- this preserves package context without forcing every watched file event to trigger full typing
- it keeps `analysis::Analysis` coherent enough for package-wide save diagnostics

### 6. Wire milestone behavior into `did_open` / `did_change` / `did_save` / `did_close` [pending]

- `did_open`
  - update `analysis::Analysis` from the opened buffer contents
  - continue publishing the existing non-typing diagnostics path
- `did_change`
  - continue publishing the existing fast `roughly` diagnostics path
  - mirror the text edits into `analysis::Analysis`
- `did_save`
  - synchronize dirty non-open files into `analysis::Analysis`
  - run full typing analysis
  - publish typing diagnostics for the saved file
- `did_close`
  - remove only the open-buffer parse state from `roughly`
  - keep package context warm in `analysis::Analysis`

### 7. Gate or disable not-yet-integrated typing-backed features [pending]

- decide whether unsupported typing-backed features should:
  - stop being advertised in LSP capabilities
  - or remain behind explicit runtime errors (yes. uncomment so code for future reference)
- ensure the chosen behavior does not block compilation or server startup

The preference for this milestone is to avoid advertising capabilities that are intentionally not
implemented yet.

### 8. Add integration coverage [pending]

- add focused tests or manual verification coverage for:
  - save-time typing diagnostics with unopened package dependencies
  - save-time typing diagnostics after an unopened package file changes on disk
  - package ordering independent of file open order
  - retaining package context across `did_close`

If direct `roughly` integration tests are too expensive initially, add the narrowest tests that
still pin the state-synchronization behavior.

## Acceptance criteria

This project is complete when:

- `roughly` no longer uses the temporary `current.R` typing adapter
- package files from `R/` are present in `analysis::Analysis` even when not open
- watched non-open package file changes are reflected on the next full typing run
- `did_save` publishes package-aware typing diagnostics for the saved file
- `did_change` still uses the existing fast `roughly` diagnostics path
- package-wide typing results no longer depend on editor open order

## Follow-up after this milestone

- add a true fast typing frontend path for `did_change`
- decide whether `roughly` or `analysis` should own the shared parse state long-term
- add tree-sitter node ids to HIR when typing-backed hover/goto-definition starts
- migrate outline/document symbols into `analysis` if that still looks worthwhile
