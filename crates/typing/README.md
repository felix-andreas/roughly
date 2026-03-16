# Static Typing for R

This crate explores a static type system for a subset of R.

The current user-facing semantics contract lives in `SEMANTICS.md`.

The historical, non-authoritative design draft lives in `DRAFT.md`. It can be useful as a starting point for future discussions, but it does not define current behavior.

## Status

This crate is still evolving.

For now, treat these as the authoritative contracts:

- `SEMANTICS.md`
- fixture expectations under `tests/fixtures/`

## Running tests

The default test command is:

```sh
cargo test -p typing
```

The fixture-based tests cover:

- rendered diagnostics
- normalized inference behavior

A future version of this README can grow into a more tutorial-style introduction once the semantics and implementation are further along.