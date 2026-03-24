# `typing` TODOs

This document tracks actionable planned work for the `typing` crate.

`SEMANTICS.md` is the user-facing contract. `ARCHITECTURE.md` describes implementation constraints. Keep this file focused on unfinished work.

## Planning rules

- Discuss important semantic changes with the user before implementation.
- Keep `SEMANTICS.md`, `ARCHITECTURE.md`, and fixture expectations aligned when behavior changes.
- Prefer concrete unfinished tasks over phase narration.
- Delete or rewrite stale tasks instead of preserving historical plans.

## Current priorities

- Apply the desired file split in code.
  - Add `hir.rs` as the explicit representation boundary.
  - Add `naming.rs` for scopes, bindings, and use-site resolution.
  - Add `diagnostic.rs` as the shared diagnostics module.
  - Rename or reshape the current checker files so they match `STRUCTURE.md`.

- Rebuild the front-end boundary around HIR.
  - Convert HIR to stable arena or id-based storage.
  - Keep source ranges and source-order information available on HIR items.
  - Represent annotation payloads and definition blocks directly in HIR.

- Introduce the naming phase.
  - Add a naming entry point after lowering.
  - Keep naming distinct even if lowering and naming run back to back.
  - Add binding identities and scope construction.
  - Add use-site resolution over HIR.
  - Resolve the open naming-output choice from `OPEN_DECISIONS.md` before locking in the representation.

- Reshape typechecking around the new boundaries.
  - Make `typecheck` consume the naming output instead of raw lowered names.
  - Keep builtin typing, compatibility logic, and interface extraction inside `typecheck.rs` for now.
  - Replace the current inference-centric API with checked-file results that fit the new architecture.

- Expose checked-file and interface boundaries.
  - Define the checked-file result owned by `check.rs`.
  - Retain diagnostics, typed results, and file-interface extraction at that boundary.
  - Use per-file interfaces as the dependency boundary for later project scheduling.

- Migrate the fixture harness and fixture directories to the new suite split.
  - Add `bindings` and `interfaces` suites.
  - Add `naming` fixture coverage.
  - Keep `annotations`, `expressions`, and `diagnostics` as first-class suites.

- Add the project-level rechecking foundation without overcommitting.
  - Keep single-file rechecking fast.
  - Make exported interfaces the dependency boundary.
  - Ensure dependent files can be rechecked when an imported interface changes, even if they are not open.
  - Avoid committing yet to reuse of inference or unification state across edits.
