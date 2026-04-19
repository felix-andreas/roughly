# Typecheck Generalization Suite [temporary]

This directory is a temporary migration suite.

Its current fixture output contract duplicates `tests/typecheck/bindings/`.
Do not treat it as a permanent top-level taxonomy.

## Current role

Current files:

- `annotations.R.test`
- `basics.R.test`
- `functions.R.test`

These cases currently cover stored binding schemes after local constraints settle.

## Intended destination

Move or rewrite these cases into:

- `tests/typecheck/bindings/annotations.R.test`
- `tests/typecheck/bindings/basics.R.test`
- `tests/typecheck/bindings/functions.R.test`

## Guidance

- Prefer deduplicating against existing `bindings` cases rather than moving files mechanically.
- Do not add new long-term coverage here.
