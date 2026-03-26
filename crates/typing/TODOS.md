# `typing` TODOs

This document tracks actionable planned work for the `typing` crate.

`SEMANTICS.md` is the user-facing contract. `ARCHITECTURE.md` describes implementation constraints. Keep this file focused on unfinished work.

## Planning rules

- Discuss important semantic changes with the user before implementation.
- Keep `SEMANTICS.md`, `ARCHITECTURE.md`, and fixture expectations aligned when behavior changes.
- Prefer concrete unfinished tasks over phase narration.
- Delete or rewrite stale tasks instead of preserving historical plans.

## Current priorities

### Type Name Resolution In Naming

- Move type-name resolution out of `lower.rs` and into `naming.rs`.
  - Keep lowering limited to parsing `SurfaceType`, `NamedTypeRef`, and definition declarations into HIR.
  - Stop doing semantic type-name checks such as unknown type, generic arity, or `@new`-on-alias during lowering.
- Extend naming to resolve both value names and type names in one pass over HIR.
  - Keep the implementation side-table based rather than introducing another transformed AST.
  - Maintain separate value and type namespaces during naming.
- Resolve type references during naming.
  - Resolve `NamedTypeRef` in `@new`.
  - Resolve `SurfaceType::Named(...)` inside annotations, type definitions, aliases, and nested generic arguments.
  - Resolve against earlier local definitions first, then imported interfaces once they exist.
- Move type-level diagnostics into naming.
  - Unknown type names.
  - Use-before-definition for type names if source-order rules remain part of the contract.
  - Wrong generic arity.
  - `@new` on aliases instead of nominal `@type`s.
- Update downstream consumers after the move.
  - Make `typecheck` consume resolved type-name information from naming side tables.
  - Move fixtures so lowering covers only lowering-owned failures while naming or diagnostics covers type-name resolution errors.

### Typecheck Boundaries

- Reshape typechecking around the new boundaries.
  - Make `typecheck` consume the naming output (`BindingId`) instead of raw lowered `Symbol` names.
  - Map builtin and imported interfaces to stable `BindingId`s so `typecheck` can look them up consistently.
  - Keep builtin typing, compatibility logic, and interface extraction inside `typecheck.rs` for now.
  - Replace the current inference-centric API with checked-file results that fit the new architecture.

### Checked File And Interfaces

- Expose checked-file and interface boundaries.
  - Define the checked-file result owned by `check.rs`.
  - Retain diagnostics, typed results, and file-interface extraction at that boundary.
  - Use per-file interfaces as the dependency boundary for later project scheduling.

### Project Rechecking

- Add the project-level rechecking foundation without overcommitting.
  - Keep single-file rechecking fast.
  - Make exported interfaces the dependency boundary.
  - Ensure dependent files can be rechecked when an imported interface changes, even if they are not open.
  - Avoid committing yet to reuse of inference or unification state across edits.
