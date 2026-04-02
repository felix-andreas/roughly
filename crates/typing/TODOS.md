# `typing` TODOs

This document tracks actionable planned work for the `typing` crate.

`SEMANTICS.md` is the user-facing contract. `ARCHITECTURE.md` describes implementation constraints. Keep this file focused on unfinished work.

## Planning rules

- Discuss important semantic changes with the user before implementation.
- Keep `SEMANTICS.md`, `ARCHITECTURE.md`, and fixture expectations aligned when behavior changes.
- Prefer concrete unfinished tasks over phase narration.
- Delete or rewrite stale tasks instead of preserving historical plans.

## Current priorities

### Split `types.rs` By Phase

- Separate the current mixed type data model into phase-shaped representations.
  - Move HIR attachment metadata such as `AttachedAnnotation` out of `types.rs` and into `hir.rs`.
  - Keep syntax-layer type forms grouped as front-end data instead of mixing them with typechecker internals.
  - Keep `CoreType`, `TypeScheme`, and inference-only identifiers as the semantic internal layer used by `typecheck.rs`.
  - Decide whether resolved type references need their own representation immediately after naming or can stay as a follow-up project.
- Reasoning:
  - `types.rs` currently mixes surface syntax, HIR attachment concerns, and typechecker internals in one file, while `lower.rs` and `hir.rs` now have much cleaner phase boundaries.
  - The current `SurfaceType` shape still uses raw `Symbol`s with no stable ids for nested type references, which is awkward now that naming owns type-name resolution.
  - Cleaning this split should make later project-global type resolution and semantic named-type support easier to add without further phase leakage.

### Type Name Resolution In Naming

- Finish the naming-phase type-resolution project.
  - Add wrong-generic-arity checks in naming instead of parser-local or lowering-local validation.
  - Introduce an explicit resolved type-name result for downstream consumers instead of having naming only emit diagnostics.
  - Resolve type references against the project-global type namespace once multi-file naming exists.
  - Make `typecheck` consume resolved type-name information from naming rather than reinterpreting raw `SurfaceType` trees.

### Typecheck Boundaries

- Reshape typechecking around the new boundaries.
  - Make `typecheck` consume the naming output (`BindingId`) instead of raw lowered `Symbol` names.
  - Map builtin and project-global resolved names to stable identities so `typecheck` can look them up consistently.
  - Keep builtin typing, compatibility logic, and interface extraction inside `typecheck.rs` for now.
  - Replace the current inference-centric API with checked-file results that fit the new architecture.

### Checked File And Interfaces

- Expose checked-file and project-summary boundaries.
  - Define the checked-file result owned by `analysis.rs`.
  - Retain diagnostics, typed results, and any project-summary extraction needed at that boundary.
  - Use those summaries as one possible dependency boundary for later project scheduling and incremental invalidation.

### Project Rechecking

- Add the project-level rechecking foundation without overcommitting.
  - Keep single-file rechecking fast.
  - Make project-visible names and any later checked-file summaries the dependency boundary.
  - Ensure dependent files can be rechecked when earlier project-visible names change, even if those dependent files are not open.
  - Avoid committing yet to reuse of inference or unification state across edits.

### Roughly Typing Save Diagnostics Integration

- Detailed plan: `projects/004_roughly_typing_save_diagnostics_integration.md`
- Integrate `typing::Analysis` into `roughly` using real document paths and retained package
  context.
- Keep current fast `roughly` diagnostics on `did_change`.
- Publish full package-aware typing diagnostics on `did_save`.
- Synchronize non-open watched package file changes into typing state before full typing runs.
