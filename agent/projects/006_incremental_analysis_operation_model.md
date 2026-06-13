[planning] Incremental Analysis Operation Model

## Goal

Capture the intended operation-triggered incremental analysis model now that the authoritative
documents have been updated, then use that plan to drive conformance work in `analysis` and
`roughly`.

This project is about scheduling and freshness behavior, not about changing the core semantic phase
boundaries.

## Non-goals

- redesign the semantic responsibilities of `lint`, `lower`, `naming`, or `typecheck`
- settle fine-grained declaration-level invalidation in this project
- redesign typecheck storage beyond what is needed to make current operation triggers coherent
- commit yet to running full project semantics on every edit

## Unresolved questions

- Should hover remain naming-only for now, or start requesting `typecheck` once typed hover data is
  retained?
- Should `did_save` in non-typing mode also republish naming diagnostics for dependent files?

## Resolved

- Save republishes diagnostics for every document whose typecheck output changed:
  `analysis::typecheck` (and `run_full` in typing mode) returns the recomputed document ids, and
  `did_save` republishes exactly those plus the saved file.
- Typecheck itself is now incremental at document grain (two-round interface model recorded in
  `ARCHITECTURE.md`), so save-time package refresh only pays for stale documents.
- Rename is implemented in `analysis::ide` on naming data, sharing resolution with
  goto-definition and references.

## Current idea

The intended steady-state model is:

- edit / keystroke:
  - refresh `lint`
  - refresh `lower`
  - refresh `resolve_document`
  - publish diagnostics for current document from retained phase outputs
- hover / rename / similar IDE actions:
  - first ensure current document-scoped phases for the unsaved buffer
  - then request only the minimum package-scoped naming or typecheck work that operation needs
- save:
  - request full semantic diagnostics for the saved package snapshot
  - still reuse versioned caches so only stale package work reruns
  - republish diagnostics for every affected file, not only the saved file

## Core mechanism

- `Analysis` owns one versioned retained cache for phase outputs
- document-scoped phases compare against document version
- package-scoped phases compare against package version
- operations do not own separate semantic stores
- operations differ only by which freshness floor they request over shared retained outputs

## Why this model

- keeps keystroke latency bounded by document-local work
- keeps local diagnostics and local tooling current in unsaved buffers
- allows hover and later rename to ask for broader semantics only when necessary
- keeps save as the point where the user sees package-wide semantic diagnostics
- preserves a path toward finer-grained invalidation later without rewriting phase boundaries

## Current implementation mismatches

### 1. No explicit `resolve_document` phase entry point

Current code:

- `crates/analysis/src/analysis.rs` computes local naming only inside `resolve_package`
- `crates/analysis/src/lib.rs` exports `run_fast`, `run_full`, `lint`, `lower`,
  `resolve_package`, and `typecheck`, but not `resolve_document`

Why this mismatches the plan:

- the design now treats document-scoped naming as a real operation boundary
- callers cannot request it directly today

### 2. Typing-time path still runs only `lint` + `lower`

Current code:

- `run_fast` in `crates/analysis/src/analysis.rs` only runs `lint` and `lower`
- `did_open` and `did_change` in `crates/roughly/src/server.rs` still call `run_fast`

Why this mismatches the plan:

- current edit path does not refresh document-scoped naming
- local naming diagnostics and local tooling facts can therefore stay stale while typing

### 3. Hover requests broader package work than intended

Current code:

- `crates/analysis/src/ide.rs` calls `lower` and then `resolve_package`

Why this mismatches the plan:

- hover should ensure current document-scoped phases first, then request minimum package-scoped
  work
- current implementation jumps straight to package resolution entrypoint
- because `resolve_package` also backfills local naming for every stale document, hover currently
  depends on a broader orchestration path than the new design intends

### 4. (resolved) Save republishes all affected typecheck diagnostics

`did_save` republishes diagnostics for every document `run_full` reports as recomputed. The
non-typing path still republishes only the saved file; naming-only dependent refresh remains open.

### 5. (resolved) Rename runs on analysis naming data in `analysis::ide`

## Desired target shape

### Phase API

- [pending] expose `resolve_document` as a real public phase entry point
- [pending] keep `resolve_package` responsible only for package-scoped naming refresh once
  document-scoped naming is already an explicit boundary
- [pending] de-emphasize `run_fast` / `run_full` in favor of operation-driven phase requests, or
  redefine them in terms of the new trigger model

### LSP edit and save behavior

- [pending] `did_open` and `did_change` should refresh `lint`, `lower`, and `resolve_document`
- [pending] `did_save` should refresh package-scoped semantics and republish diagnostics for all
  affected documents
- [pending] watched-file changes and close handling should continue updating analysis state
  immediately, then rely on normal freshness checks later

### IDE actions

- [pending] hover should use the narrowest available semantic path consistent with current results
- [pending] rename should be rebuilt on analysis naming data rather than tree-only heuristics
- [pending] global rename should define how many package documents must be made fresh before edits
  are computed

## Implementation plan

### 1. Expose the document-scoped naming boundary [pending]

- add `resolve_document`
- make local naming freshness explicit and directly callable
- update `ide` callers to use it instead of relying on `resolve_package` side effects

### 2. Align edit-time orchestration [pending]

- replace `run_fast` call sites or redefine `run_fast` to include `resolve_document`
- verify diagnostics publication still merges retained phase outputs correctly

### 3. Align save-time package diagnostics [done]

- `typecheck` returns recomputed document ids; `did_save` republishes them all

### 4. Rebuild rename on analysis state [done]

- rename lives in `analysis::ide` on naming identities

## Success criteria

- edit-time diagnostics include current document naming diagnostics without forcing package
  semantics on every keystroke
- hover no longer relies on broader-than-necessary package orchestration
- save updates client diagnostics for every affected package file
- rename, when re-enabled, uses analysis naming data and the same freshness model as other IDE
  actions
