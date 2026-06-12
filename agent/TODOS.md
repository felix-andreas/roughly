# TODOs

## Planning rules

- Discuss important semantic changes with the user before implementation.
- Keep `TYPING_SEMANTICS.md`, `ARCHITECTURE.md`, and fixture expectations aligned when behavior changes.
- Prefer concrete unfinished tasks over phase narration.
- Delete or rewrite stale tasks instead of preserving historical plans.

## Current priorities

- Discuss arithmetic/comparison on unannotated parameters (needs refinement)
  - `function(x) x + 1L` currently errors with invalid operand because HM has no numeric-class
    constraint and the engine refuses to guess between `integer` and `double`
  - this is the biggest practical usability gap for real R code; options include a numeric
    constraint kind on inference variables, defaulting, or bidirectional expectations
- Retain checked-file semantic results in the pipeline
  - package typecheck currently stores diagnostics only and `break`s on the first error, so one
    run reports at most one type error per package and `typecheck/project` cannot render typed
    snapshots
  - prerequisite for incremental analysis (project 006) and richer hover/inlay output
- Lower and typecheck parameter default expressions
  - `has_default` exists on HIR parameters, but default expressions are dropped, so their types
    do not constrain parameter types
- Improve unknown-type-name diagnostics
  - an unresolved type name renders as `Syntax Error: type syntax error: unknown type ...` plus a
    cascading `expected Unknown, found ...` type error; should be one naming-owned diagnostic with
    the cascade suppressed
- `typecheck/project` follow-ups
  - package winner behavior with conflicting types
  - `Collate` coverage once the fixture harness models `DESCRIPTION`
  - workspace-edit generations

### Active Projects

- `projects/006_incremental_analysis_operation_model.md` — incremental analysis design
