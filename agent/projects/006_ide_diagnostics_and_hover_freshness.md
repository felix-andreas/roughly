[planning] Incremental Analysis Pipeline

## Goal

Make the analysis pipeline simpler and more incrementally correct for IDE use.

- `Analysis` should own freshness and rerun decisions internally.
- Callers should request the phase or IDE action they need without passing changed-document sets.
- Lowering should stay eager enough for fast syntax and structural diagnostics while typing.
- Naming and typecheck should run lazily, but they should still run on save and when required by IDE actions such as hover and later rename.
- The design should support retaining diagnostics by phase and version so save-time semantic diagnostics are not unnecessarily discarded while typing.

## Why this project exists

The current incremental story is split across multiple competing mechanisms:

- `Analysis` tracks pending package file changes by path.
- Phase freshness is partly inferred from cache presence or absence.
- Callers can explicitly pass `changed_documents` into `run_lowering` and `run_naming`.
- Package semantics are invalidated by eagerly clearing whole stores and diagnostics.

That makes freshness hard to reason about because the source of truth for "what is stale" is split between internal state, cache state, and public API.

This project should collapse that into one mechanism.

## LSP constraint

There is also a protocol constraint behind the diagnostics part of this project.

LSP diagnostics are published per file as one full list. The protocol does not let us incrementally update only the lowering diagnostics, only the naming diagnostics, or only the typecheck diagnostics for a file while leaving the rest untouched.

That matters for IDE behavior:

- lowering should refresh while the user is typing
- naming and typecheck should not necessarily rerun on every change
- but we may still want to keep showing the last semantic diagnostics until a later save or IDE action reruns those phases

So analysis needs to retain diagnostics per phase and per version internally, then merge the retained phase diagnostics into one LSP list when publishing diagnostics for a file.

## Chosen direction

Use versions as the only freshness mechanism.

Do not keep parallel dirty sets or rely on cache misses as freshness signals.

The intended direction is:

- `Analysis` owns freshness internally
- public phase entry points do not accept changed-document lists
- phase execution is driven by comparing current versions against last-built versions
- package file changes should be synchronized into analysis state immediately rather than queued for a later sync step

## Freshness model

### Document-scoped freshness

Each loaded document has a current document version.

That version should advance when the analysis-visible contents of the document change, including:

- add
- edit
- reload from disk

Lowering and file-local naming compare against that document version:

- lowered results record the document version they were built from
- local naming results record the document version they were built from
- rerun lowering when `lowered_version != document_version`
- rerun local naming when `local_naming_version != document_version`

### Package-scoped freshness

Package-global naming and typecheck are package-level work, so in the first slice they can use one coarse package version.

- package version advances when package-visible analysis inputs change
- first slice: increment it on any package document add, edit, reload, or delete
- package-global naming records the package version it was built from
- package typecheck records the package version it was built from
- rerun package-global naming when `global_naming_version != package_version`
- rerun typecheck when `typecheck_version != package_version`

This first slice is intentionally coarse. We can narrow package invalidation later if needed.

### Artifact freshness versus diagnostic freshness

Artifact freshness is primary.

Diagnostics alone are not enough to decide freshness. A clean phase still needs a recorded built-at version for its semantic artifact, otherwise a phase with no diagnostics would appear stale forever.

In practice, each retained phase result should store one produced-at version that applies to both:

- the semantic output artifact
- the diagnostics produced by that phase run

The important point is not "two separate counters per phase". The important point is that freshness is tracked on the retained phase result, not inferred from whether diagnostics happen to exist.

## Diagnostics model

Diagnostics should be retained by phase and version inside analysis.

For example:

- lint diagnostics at version `v`
- lowering diagnostics at version `v`
- naming diagnostics at version `v`
- typecheck diagnostics at version `v`

When a client asks for diagnostics for a file, analysis should:

1. ensure the required phases are fresh for the requested operation
2. gather the currently retained diagnostics for that file from each phase
3. publish one consolidated LSP diagnostics list for that file

This keeps phase execution and LSP publishing separate:

- phases own their own diagnostics
- LSP sees only the merged result

## Intended API direction

The public pipeline API should stop exposing freshness decisions.

Target shape:

- `analysis::lint(state)`
- `analysis::lower(state)`
- `analysis::resolve_document(state)`
- `analysis::resolve_package(state)`
- `analysis::typecheck(state)`

Each function should mean "ensure this phase is up to date for the currently loaded analysis state".

Callers should not tell analysis which documents are dirty. Analysis should already know that from its versions.

`analysis::check(state)` can remain as a convenience orchestration entry point built on top of those phase functions.

The public phase stores on `Analysis` should also be removed:

- `pub lowering: LoweringStore`
- `pub naming: NamingStore`
- `pub typecheck: TypecheckStore`

That state should remain internal to `Analysis`. The public API should expose queries and phase entry points, not mutable access to cached pipeline internals.

## Settled decisions

- freshness is version-based only
- analysis state stays current immediately as edits and watched file changes happen
- lowering remains the eager typing-time phase
- package resolution and typecheck run lazily, but they still run on save and when required by IDE actions
- keep the naming split public for now as `resolve_document` and `resolve_package`
- `resolve_package` still runs even if some files have lowering errors
- `typecheck` is skipped when there are blocking lower or naming errors in the first slice
- diagnostics are retained per phase and version, then merged into one LSP diagnostics list per file
- analysis owns freshness and rerun decisions internally
- callers do not pass changed-document sets
- cached phase internals are not public API
- the first replacement query surface lives directly on `Analysis`
- `close` must restore the on-disk version if the open document was not saved, so close handling changes analysis state immediately rather than queueing later sync work
- keep one package version in the first slice, incremented whenever any package document add, edit, reload, or delete changes package analysis inputs
- `lint` should use the same retained output shape as the other phases, with `output: ()`
- first-slice typecheck diagnostic ownership stays package-scoped as it is today; improving per-document ownership is deferred follow-up work
- version bump rules:
  - add a document: create the retained document state with a fresh document version
  - edit a loaded document: bump that document's version
  - reload from disk: bump that document's version if the loaded document state changes
  - delete a document: remove that document's retained document-scoped outputs
  - open by itself does not bump a version unless it changes the loaded document state
  - close by itself does not bump a version unless it restores different on-disk contents into analysis state
  - watched file changes for closed files are handled as add, reload, or delete events
- first-slice stored phase results use separate document-scoped and package-scoped wrappers rather than one universal diagnostics payload

## Integration consequence

If versions are the only freshness mechanism, then analysis state needs to stay current as edits and file-system changes happen.

That implies:

- `did_change` edits the open document in analysis and bumps its document version
- `did_change_watched_files` immediately reloads or deletes closed package files in analysis
- `did_close` immediately reloads from disk or deletes from analysis instead of queuing a later sync

With that model, later phase requests only need version checks. They do not need a pending package-document sync queue.

## Open decisions

There are no remaining design blockers for the first implementation slice.

If implementation exposes new structural issues, record them here before widening the design.

## Stored phase result shape

The current preferred direction is to give each phase one retained result object that owns:

- the built-at version
- the semantic output artifact
- the diagnostics produced at that version

Conceptually:

```rust
struct Output<T> {
    version: Version,
    output: T,
    diagnostics: Vec<Diagnostic>,
}
```

The cleanest first-slice proposal is to use separate stored shapes for document-scoped and package-scoped phases instead of forcing one universal diagnostics field:

```rust
struct DocumentOutput<T> {
    version: Version,
    output: T,
    diagnostics: Vec<Diagnostic>,
}

struct PackageOutput<T> {
    version: Version,
    output: T,
    diagnostics: HashMap<DocumentId, Vec<Diagnostic>>,
}
```

This keeps the `Output<T>` idea for document-scoped phases and introduces one package-scoped equivalent where package work naturally produces per-document diagnostic buckets.

### Document-scoped phases

For document-scoped phases such as lowering and `resolve_document`, this shape fits naturally:

- lowering already produces `LoweringResult { module, diagnostics }`
- local naming can be wrapped the same way with `NamesLocal` plus diagnostics

So `DocumentOutput<T>` is a good fit there.

### Package-scoped phases

For package-scoped phases such as `resolve_package` and typecheck, the diagnostics payload is not naturally just `Vec<Diagnostic>`.

Current code evidence:

- `rebuild_package_naming` already returns package output plus `HashMap<DocumentId, Vec<Diagnostic>>`
- `Diagnostic` itself has no `DocumentId`

So for package phases, the natural stored shape is `PackageOutput<T>`.

### Typecheck caveat

There is one real design constraint in the current code.

Current typecheck merges all package modules into one synthetic module before inference, and the retained `Diagnostic` shape does not carry a document id. That means precise per-document typecheck diagnostic retention is not cleanly represented today.

This is not a blocker for the first slice. The first slice keeps package-scoped typecheck diagnostics and records the need for later source/document mapping work.

## First implementation slice

- [pending] Remove `changed_documents` from the public phase API.
- [pending] Add version tracking for document-scoped and package-scoped analysis artifacts.
- [pending] Make lowering and local naming rerun from document-version comparisons.
- [pending] Make global naming and typecheck rerun from one coarse package-version comparison.
- [pending] Replace pending package-document sync with immediate analysis-state updates on close and watched-file changes.
- [pending] Retain diagnostics per phase and version inside analysis.
- [pending] Update `roughly` diagnostics and hover call sites to use the simpler API.

## Next steps

- [pending] Refine the version model into concrete fields on analysis state and phase artifacts.
- [pending] Implement the first slice in `analysis.rs`:
  - internal versions
  - internal phase stores
  - new phase entry points
  - retained phase diagnostics
  - `DocumentOutput<T>` and `PackageOutput<T>` storage
- [pending] Update `roughly` to use the new phase API and immediate sync behavior.
- [pending] Add focused tests for:
  - lowering reruns after edit
  - package resolution reruns after package file changes
  - typecheck stays cached when versions have not changed
  - semantic diagnostics remain retained while typing until rerun

## Code-backed constraints

The current code shows one important constraint for the implementation plan:

- versions cannot live only on diagnostics

That is because semantic artifacts are already consumed directly by IDE and test code:

- hover reads lowered modules through `Analysis::module` and naming data through `analysis.naming.locals` and `analysis.naming.package`
- fixtures render lowered and named artifacts directly rather than only reading diagnostics

So artifact freshness must be tracked independently of diagnostic freshness. Diagnostics can carry their own produced-at version, but they cannot be the only freshness record.

The current code also shows the main migration dependencies:

- removing the public phase stores is not blocked, but hover and fixture code will need `Analysis` query methods to replace direct field access
- removing pending dirty-document sync is not blocked, because `did_change_watched_files` already updates closed files immediately; the remaining special case is `did_close`
- retained per-phase diagnostics will need a richer internal shape than the current `Vec<PhaseDiagnostic>` because the current representation stores phase tags but not produced-at versions
- typecheck currently has no retained semantic result, only diagnostics, so the first slice can version its execution cheaply, but later typing-powered IDE features will need a real retained typed artifact

## Later work

- narrow package-global invalidation beyond one coarse package version
- decide whether the public naming API should stay split or collapse into one entry point
- make hover and later IDE queries pull only the minimum required phases
- extend the same versioned phase-retention model to rename and other IDE actions
