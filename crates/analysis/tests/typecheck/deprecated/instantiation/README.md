# Typecheck Instantiation Suite [temporary]

This directory is a temporary migration suite.

Its current fixture output contract duplicates `tests/typecheck/expressions/`.
Do not treat it as a permanent top-level taxonomy.

## Current role

Current files:

- `basics.R.test`
- `functions.R.test`

These cases currently cover polymorphic use-site behavior:

- repeated fresh instantiation
- aliased polymorphic use
- higher-order polymorphic calls

## Intended destination

Move or rewrite these cases into:

- `tests/typecheck/expressions/functions.R.test`
- `tests/typecheck/expressions/polymorphism.R.test`

## Guidance

- Prefer asserting user-facing use-site semantics in `expressions` rather than preserving
  `instantiation` as a separate suite name.
- Do not add new long-term coverage here.
