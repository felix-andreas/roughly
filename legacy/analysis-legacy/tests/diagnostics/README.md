# Diagnostics Suite

This README is the authoritative contract for the `tests/diagnostics/` fixture suite.

If the diagnostics coverage split or intended contract changes, update this file and
the testing docs page in the same session as the fixture changes.

Keep this document aligned with the actual suite. If a future incremental-analysis suite later
absorbs this contract, record that transition explicitly here rather than silently letting the
coverage drift.

## Purpose

The diagnostics suite answers:

- what final rendered diagnostics the user sees
- which diagnostic code, wording, range, and ordering are part of the current contract
- how diagnostics from several phases compose in one rendered output

This suite is for rendered diagnostics, not for general semantic success coverage.

## Current files

- `arithmetic.R.test`
- `basics.R.test`
- `calls.R.test`
- `lists.R.test`
- `polymorphism.R.test`
- `special_types.R.test`
- `types.R.test`

## Matrix

The diagnostics suite should explicitly cover:

- syntax and typing-comment diagnostics
  - missing type expression
  - malformed type syntax
  - invalid directive ordering
  - invalid mixed typing-comment forms
  - top-level-only definition placement
- naming diagnostics
  - unknown names
  - duplicate top-level bindings
  - maybe-undefined warnings
- type errors
  - arithmetic operand mismatch
  - calling non-functions
  - arity mismatch
  - list-shape mismatch
  - nullable-union mismatch
  - `NULL` mismatch
  - `Unknown` mismatch where required
- polymorphism diagnostics
  - later invalid use after earlier valid use
  - higher-order polymorphic conflict
- cross-phase rendering behavior
  - several diagnostics in one output
  - ordering
  - stable ranges
  - stable diagnostic codes

## Current gaps

Known missing or thin areas:

- multi-file diagnostics coverage
- exported-surface diagnostics coverage
- incremental-analysis retention and invalidation behavior
- broader nominal success/failure diagnostics matrix once typed nominal coverage grows

## What does not belong here

- semantic success cases whose only assertion is a type result
- raw inference-metavariable output
- binding-scheme snapshots

## Naming guidance

- prefer files named after the user-facing area whose diagnostics they cover
- prefer case names that state the rendered rule, for example
  `nullable_union_not_assignable_to_plain_type`
