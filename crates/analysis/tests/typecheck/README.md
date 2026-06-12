# Typecheck Fixture Suite

This README is the authoritative contract for the typecheck fixture split.

If the typecheck fixture layout, coverage split, or intended semantics coverage changes, update
this file and `agent/TESTING.md` in the same session as the fixture changes.

Keep this document aligned with the actual suite. If the suite is still migrating toward the
contract below, record the remaining gaps here instead of implying that they are already covered.

The goal is an excellent semantics-first suite, not a layout that merely reflects the current
implementation or current passing cases.

## Suite split

The target typecheck split is:

- `tests/typecheck/bindings/`
- `tests/typecheck/expressions/`
- `tests/typecheck/interfaces/`
- later `tests/typecheck/project/`
- optional internal `tests/typecheck/unification/`

These suites are split by rendered output contract, not by internal inference substeps.

- `bindings` renders binding-boundary stored schemes
- `expressions` renders use-site expression result types
- `interfaces` renders final exported file surface
- `project` will later cover multi-file typed package behavior
- `unification` is optional internal engine coverage for raw metavariable-facing output

There is no intended long-term top-level split by `generalization`, `instantiation`,
`substitution`, or `environment`.

## Current status

All active suites exist:

- `bindings`
- `expressions`
- `interfaces`
- `project`
- `unification`

The old engine-centric `deprecated/` migration storage (`generalization`, `environment`,
`instantiation`, `substitution`) is deleted; its unique coverage was re-homed into the
target suites.

## Matrix

The typecheck fixture surface should explicitly cover:

- binding-boundary typing facts
  - stored monomorphic schemes
  - stored polymorphic schemes
  - rebinding history
  - binding annotations and coercions
- expression semantics
  - literals and names
  - assignments and blocks
  - calls and higher-order calls
  - arithmetic
  - vectors and lists
  - special types and unsupported constructs
  - control flow and loops
  - indexing
  - scoping and closure capture
  - polymorphic use-site behavior
- exported file surface
  - final visible bindings
  - exported aliases
  - exported nominal types
  - later generic exported definitions
- internal inference coverage if retained
  - monotypes before generalization
  - structural mismatch errors
  - occurs-check behavior
- later project semantics
  - cross-file value use
  - cross-file type use
  - package file winner behavior
  - non-package consumer behavior
  - later `Collate`-driven file order

There should be no catch-all overflow file. If a new case does not fit an existing file split,
refine the split instead of adding `misc`.

## Current gaps

The largest known gaps are:

- `project` currently renders per-file diagnostics only, because the check pipeline retains no
  typed artifact; typed cross-file snapshots need pipeline support first
- `Collate`-driven file order is not covered; the harness cannot model `DESCRIPTION` yet

## Naming guidance

- Prefer fixture groups named after semantic topic, for example `nullable_unions`,
  `higher_order_calls`, or `nominal_introduction`
- Prefer case names that state the rule being exercised, for example
  `if_without_else_returns_nullable_branch_type`
- Keep `group__case` names stable when moving a case between files or suites
