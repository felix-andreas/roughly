# Typecheck Unification Suite

This README is the authoritative contract for the `tests/typecheck/unification/` suite if this
internal suite is retained.

If the suite is renamed or removed, update this file and `agent/TESTING.md` in the same session.

## Purpose

`unification` is internal-facing coverage for raw inference behavior before generalization.

It answers:

- what monotype shape inference produces while constraints are still expressed with metavariables
- which structural mismatch and occurs-check failures appear at that internal layer

This suite should stay deliberately small and focused.

## Current files

- `basics.R.test`
- `functions.R.test`

## Matrix

The unification suite should explicitly cover:

- basic monotypes
  - scalar literals
  - assignment followed by use
- function literals
  - identity
  - constant functions
  - several parameters
  - higher-order argument positions
  - returned functions
- structural failures
  - tuple length mismatch
  - record field mismatch
  - occurs-check failure

If this suite remains, it may also cover:

- function arity mismatch at raw engine level
- named parameter mismatch at raw engine level
- operator constraints that leave metavariable-bearing shapes before later resolution

## Current gaps

Known missing or thin areas:

- named parameter internal mismatch coverage
- more operator-driven raw constraint coverage
- more list-shape unification coverage beyond the current mismatch cases

## What does not belong here

- user-facing exported surfaces
- final stored binding schemes
- ordinary resolved expression result types when no raw metavariable behavior is being asserted

## Naming guidance

- keep case names focused on internal constraint fact, for example
  `self_application_occurs_check`
- avoid growing this suite into a second general-purpose semantic suite
