# Fixtures

This crate parses the `.test` fixture language used by the `analysis` crate.

The crate currently parses two author-facing fixture shapes:

- `Simple`
- `MultiFile`

`MultiFile` also covers later grouped generations of edits, moves, and deletes.

This crate is not the authoritative place to document fixture authoring rules. Use [`agent/TESTING.md`](../../agent/TESTING.md) for the fixture syntax contract and authoring guidance.
