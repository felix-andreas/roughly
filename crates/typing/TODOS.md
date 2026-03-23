# `typing` TODOs

This document tracks actionable planned work for the `typing` crate.

`SEMANTICS.md` is the user-facing contract. `ARCHITECTURE.md` describes implementation constraints. Keep this file focused on unfinished work.

## Planning rules

- Discuss important semantic changes with the user before implementation.
- Keep `SEMANTICS.md`, `ARCHITECTURE.md`, and fixture expectations aligned when behavior changes.
- Prefer concrete unfinished tasks over phase narration.
- Delete or rewrite stale tasks instead of preserving historical plans.

## Current priorities

- Migrate the fixture harness and fixture directories to the new suite split.
  - Replace the current `inference` suite with separate successful-check suites.
  - Use `expressions` for smaller checked-expression cases.
  - Add `bindings` and `interfaces` suites.
  - Add `naming` fixture coverage.

- Clean up the front-end boundary.
  - Parse annotations during lowering exactly once.
  - Remove duplicate annotation parsing from the main checking flow.
  - Remove `parse.rs` from the public crate surface.
  - Keep parser setup only as test support or external integration glue.

- Introduce explicit HIR ownership.
  - Create `hir.rs` as the representation boundary.
  - Move lowered data structures out of `lower.rs`.
  - Convert the lowered representation to stable arena or id-based storage.
  - Keep source ranges and source-order information available on HIR items.

- Introduce a separate naming phase.
  - Add a naming entry point after lowering.
  - Keep naming distinct even if lowering and naming run back to back.
  - Delay the unresolved naming output choice until `OPEN_DECISIONS.md` is settled.

- Keep typechecking structurally simple at first.
  - Keep one `typecheck.rs` file initially.
  - Defer splitting builtins, compatibility, and interface extraction into separate modules.
  - Reshape the current inference code around the `typecheck` phase boundary.
