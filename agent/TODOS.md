# TODOs

## Planning rules

- Discuss important semantic changes with the user before implementation.
- Keep `TYPING_SEMANTICS.md`, `ARCHITECTURE.md`, and fixture expectations aligned when behavior changes.
- Prefer concrete unfinished tasks over phase narration.
- Delete or rewrite stale tasks instead of preserving historical plans.

## Current priorities

### Active Projects

- `agent/projects/006_incremental_analysis_operation_model.md`
  - bring implementation into conformance with operation-triggered incremental analysis design
- `agent/projects/007_typecheck_fixture_surface_rework.md`
  - realign typecheck fixture taxonomy with `TYPING_SEMANTICS.md`
  - add missing multi-file and nominal/generic happy-path coverage
  - unblock next slice by preserving named types and explicit binders in typecheck
  - then finish current slice: expand `bindings/annotations`, add `bindings/nominals`, expand `interfaces/types`
