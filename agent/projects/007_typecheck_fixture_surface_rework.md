# Typecheck Fixture Surface Rework [done]

## Goal

Realign the typecheck fixture surface with `agent/TYPING_SEMANTICS.md` so user-facing semantics
are the primary contract and inference-mechanism details are secondary helper coverage.

## Outcome

The semantics-first split is in place and the engine-centric taxonomy is gone:

- active suites: `bindings`, `expressions`, `interfaces`, `project`, internal `unification`
- `tests/typecheck/deprecated/` migration storage is deleted; unique cases were re-homed first
- suite-local `README.md` files carry the per-suite contracts and coverage matrices
- `agent/TESTING.md` describes the split and the `project` suite contract

The type-model blocker identified during this project (named types, binders, and `@new` being
erased in core typing) was resolved in the `feat/hm-type-checker` implementation work:
`CoreType::Nominal` preserves identity, aliases expand structurally with cycle detection,
binders lower to fresh variables and re-generalize at binding boundaries, and `@new` validates
the value against the nominal representation type.

Backlog coverage landed across:

- `bindings`: annotations (checked/`@trust`/`@if-unknown`), `@forall`, nominals, generics,
  optional/default parameters
- `expressions`: comparison, ranges, modulo/power, nominal introduction and projection,
  argument coercions, named-argument calls, `@forall` use sites, `Unknown` propagation
- `interfaces`: nominal, generic, higher-order, optional-parameter, and mixed exports
- `project`: cross-file values, cross-file types, script isolation (diagnostics-rendered)
- `diagnostics`: operator, nominal, and call wording

## Follow-ups (tracked in TODOS.md)

- typed cross-file snapshots for `project` wait on the pipeline retaining checked-file results
- `Collate`-driven file order waits on fixture-harness `DESCRIPTION` support
- `unification` stays as a small internal suite until raw metavariable snapshots stop being useful
