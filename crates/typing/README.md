# Static Typing for R

This crate explores a static type system for a subset of R.

The authoritative user-facing typing contract lives in `SEMANTICS.md`.

Fixture expectations under `tests/annotations/`, `tests/inference/`, and `tests/diagnostics/` are also part of the current contract for user-visible behavior.

## What this crate is for

The goal is to make a useful subset of R statically checkable while keeping the resulting types and diagnostics readable to R programmers.

The project is guided by a few broad design goals:

- use Hindley-Milner style inference as the foundation
- aim for a sound type checker within the supported subset
- treat the supported subset as value-like to avoid early variance problems that arise with mutable state
- aim for Rust- and Elm-like diagnostic quality: clear, precise, and actionable errors

It also has clear non-goals for v1:

- full coverage of base R syntax and semantics
- S3 dispatch modeling
- S4 dispatch modeling
- NSE and metaprogramming completeness
- environment and reference semantics

The current semantics focus on a small set of ideas:

- atomic R types such as `logical`, `integer`, `double`, and `character`
- scalar-like, array-like, and map-like vector shapes
- tuple-like, record-like, array-like, and map-like `list(...)` values
- function types written in `#:` comments
- explicit `Any`, `Unknown`, and `NULL`
- nullable unions written as `T | NULL`
- `if` expressions with explicit nullability behavior

For the precise rules, examples, and user-facing rendered type forms, read `SEMANTICS.md`. For implementation structure, read `ARCHITECTURE.md`.

## Status

This crate is still evolving, but `SEMANTICS.md` is authoritative for the semantics it covers.

## Running tests

The default crate test command is:

```sh
cargo test -p typing
```

The fixture-based tests cover three different layers of behavior:

- `tests/annotations/`
  - tests the annotation parser and renderer
  - this suite is about annotation syntax, normalization, and rendering, not typing semantics
- `tests/inference/`
  - tests inferred results for individual expressions and small snippets
  - this is the focused suite for inference behavior
- `tests/diagnostics/`
  - tests higher-level checking behavior and failure cases
  - this structure is not fixed; some failure cases may later move into `inference`, or we may add dedicated suites such as unification fixtures

The default crate test command is:

```sh
cargo test -p typing
```
