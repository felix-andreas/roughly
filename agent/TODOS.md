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
- Retain typed expression results for tooling
  - per-document checking already computes expression types and exported schemes; storing them
    would let hover and inlay hints show checked types
- Lower and typecheck parameter default expressions
  - `has_default` exists on HIR parameters, but default expressions are dropped, so their types
    do not constrain parameter types
- Reclassify unknown-type-name diagnostics
  - the cascade is suppressed, but the remaining diagnostic is still labeled
    `Syntax Error: type syntax error: unknown type ...`; it should be a naming-owned diagnostic
    with friendlier wording
- Cheapen the typecheck no-op path
  - environment and type-definition fingerprints are rendered strings linear in package size per
    `typecheck` call; hash or version them
- `typecheck/project` follow-ups
  - package winner behavior with conflicting types
  - `Collate` coverage once the fixture harness models `DESCRIPTION`
- Decide function-type parameter variance and record it in `TYPING_SEMANTICS.md`

### Active Projects

- `projects/006_incremental_analysis_operation_model.md` — operation scheduling alignment
