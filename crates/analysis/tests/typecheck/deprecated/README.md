# Typecheck Deprecated Suites

This directory is temporary migration storage for older engine-named fixture suites.

These fixture directories remain during migration, but they are not wired as active suites and they
are not part of the target long-term taxonomy. The target split lives in sibling directories:

- `tests/typecheck/bindings/`
- `tests/typecheck/expressions/`
- `tests/typecheck/interfaces/`
- later `tests/typecheck/project/`
- optional internal `tests/typecheck/unification/`

## Current contents

- `environment/`
- `generalization/`
- `instantiation/`
- `substitution/`

## Contract

- keep these suites only while cases are being moved or deduplicated
- do not add new long-term coverage here
- prefer rewriting or relocating cases into the target suites rather than extending deprecated
  directories
- keep each deprecated child README in sync with the intended destination during migration
