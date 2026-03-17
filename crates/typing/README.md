# Static Typing for R

This crate explores a static type system for a subset of R.

The authoritative user-facing typing contract lives in `SEMANTICS.md`.

Fixture expectations under `tests/fixtures/` are also part of the current contract for user-visible behavior.

## What this crate is for

The goal is to make a useful subset of R statically checkable while keeping the resulting types and diagnostics readable to R programmers.

The current semantics focus on a small set of ideas:

- atomic R types such as `logical`, `integer`, `double`, and `character`
- scalar-like, array-like, and map-like vector shapes
- tuple-like and map-like `list(...)` values
- function types written in `#:` comments
- explicit `Any`, `Unknown`, and `NULL`
- nullable unions written as `T | NULL`
- `if` expressions with explicit nullability behavior

For the precise rules, examples, and user-facing rendered type forms, read `SEMANTICS.md`.

## Status

This crate is still evolving, but `SEMANTICS.md` is authoritative for the semantics it covers.

## Running tests

The default crate test command is:

```sh
cargo test -p typing
```

The fixture-based tests cover:

- rendered diagnostics
- normalized inference behavior