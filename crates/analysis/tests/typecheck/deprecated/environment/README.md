# Typecheck Environment Suite [temporary]

This directory is a temporary migration suite.

Its current fixture output contract duplicates `tests/typecheck/expressions/`.
Do not treat it as a permanent top-level taxonomy.

## Current role

Current files:

- `functions.R.test`
- `scoping.R.test`

These cases currently cover:

- rebinding visibility
- generalized scheme reuse
- parameter shadowing
- closure capture
- local shadowing

## Intended destination

Move or rewrite these cases into:

- `tests/typecheck/expressions/scoping.R.test`
- `tests/typecheck/expressions/polymorphism.R.test`

## Guidance

- Prefer moving existing cases into `expressions` over adding new cases here.
- If a new semantic topic is discovered, add it to the `expressions` suite rather than extending
  this temporary directory.
