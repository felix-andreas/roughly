# `typing` TODOs

This document tracks actionable planned work for the `typing` crate.

`SEMANTICS.md` is the user-facing contract. `ARCHITECTURE.md` describes implementation constraints. Keep this file focused on unfinished work.

## Planning rules

- Discuss important semantic changes with the user before implementation.
- Keep `SEMANTICS.md`, `ARCHITECTURE.md`, and fixture expectations aligned when behavior changes.
- Prefer concrete unfinished tasks over phase narration.
- Delete or rewrite stale tasks instead of preserving historical plans.

## Current priorities

- Reshape typechecking around the new boundaries.
  - Make `typecheck` consume the naming output (`BindingId`) instead of raw lowered `Symbol` names.
  - Map builtin and imported interfaces to stable `BindingId`s so `typecheck` can look them up consistently.
  - Keep builtin typing, compatibility logic, and interface extraction inside `typecheck.rs` for now.
  - Replace the current inference-centric API with checked-file results that fit the new architecture.

- Expose checked-file and interface boundaries.
  - Define the checked-file result owned by `check.rs`.
  - Retain diagnostics, typed results, and file-interface extraction at that boundary.
  - Use per-file interfaces as the dependency boundary for later project scheduling.

- Migrate the fixture harness and fixture directories to the new suite split.
  - Add `bindings` and `interfaces` suites.
  - Add `naming` fixture coverage.
  - Fix the failing tests in `diagnostics` and `expressions` that are failing due to known implementation gaps.

- Add the project-level rechecking foundation without overcommitting.
  - Keep single-file rechecking fast.
  - Make exported interfaces the dependency boundary.
  - Ensure dependent files can be rechecked when an imported interface changes, even if they are not open.
  - Avoid committing yet to reuse of inference or unification state across edits.
