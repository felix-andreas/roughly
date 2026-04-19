# Typecheck Substitution Suite [temporary]

This directory is a temporary migration suite.

Its current fixture output contract duplicates `tests/typecheck/expressions/`.
Do not treat it as a permanent top-level taxonomy.

## Current role

Current files:

- `basics.R.test`
- `functions.R.test`

These cases currently cover resolved user-facing results after local solving has propagated through
calls and returned function shapes.

## Intended destination

Move or rewrite these cases into:

- `tests/typecheck/expressions/functions.R.test`
- `tests/typecheck/expressions/polymorphism.R.test`

## Guidance

- Prefer asserting the final user-facing result in `expressions`.
- Keep low-level solving details either in `unification` or direct Rust tests.
- Do not add new long-term coverage here.
