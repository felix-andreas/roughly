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

- Should save republish diagnostics for every package file, or only for files whose retained
  diagnostics changed?
- Should hover remain naming-only for now, or start requesting `typecheck` once typed hover data is
  retained?
- What is the minimum package-scoped freshness rename should require for a future global rename?
- When package-scoped work reruns, what query should expose the set of affected documents whose LSP
  diagnostics must be republished?

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

### 4. Save only republishes diagnostics for saved file

Current code:

- `did_save` in `crates/roughly/src/server.rs` runs `analysis::run_full`
- it then publishes diagnostics only for the saved document

Why this mismatches the plan:

- save is supposed to surface full semantic diagnostics for the saved package snapshot
- package-global naming or typecheck can change diagnostics in dependent files
- those files currently keep stale diagnostics on the client

### 5. Rename is not integrated with analysis freshness model

Current code:

- LSP rename in `crates/roughly/src/server.rs` is still unsupported
- dormant rename logic in `crates/roughly/src/rename.rs` uses tree-walk scope heuristics instead of
  analysis naming data

Why this mismatches the plan:

- future rename should be an IDE action over the same retained semantic caches
- current rename path is either disabled or bypasses the analysis phase model entirely

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

### 3. Align save-time package diagnostics [pending]

- define affected-document diagnostics publication
- republish package-wide semantic diagnostics after save-time package refresh
- add LSP coverage for dependent-file diagnostic refresh

### 4. Rebuild rename on analysis state [blocked]

- depends on deciding the first supported rename scope
- local rename should use naming identities
- global rename should wait until package freshness requirements are explicit

## Success criteria

- edit-time diagnostics include current document naming diagnostics without forcing package
  semantics on every keystroke
- hover no longer relies on broader-than-necessary package orchestration
- save updates client diagnostics for every affected package file
- rename, when re-enabled, uses analysis naming data and the same freshness model as other IDE
  actions
