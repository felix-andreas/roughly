# Testing

This crate prefers fixture tests for source-driven behavior because they are:

- easy for a human to read in diffs
- easy to extend into many cases quickly
- a good fit for verifying AI-generated changes against an explicit text contract

Use ordinary Rust tests only when the behavior is awkward to express as a rendered fixture.

## Fixture format

The fixture harness lives in `tests/test_fixtures.rs`.

Each `.test` file uses:

```text
#==== group_name
#---- case_name
<input source>
#++++
<expected rendered output>
```

Notes:

- `group__case` is the stable test identity
- `group__case` names must be unique across the suite
- expected output should render semantic facts, not incidental debug structure

## Focused runs

Run one focused fixture case with:

```sh
TYPING_FILTER=group__case cargo test -p typing --test test_fixtures <suite> -- --nocapture
```

Default crate test command:

```sh
cargo test -p typing
```

## Suite direction

The intended fixture suites are:

- `annotations`
- `lowering`
- `naming`
- `expressions`
- `bindings`
- `interfaces`
- `diagnostics`

Keep focused test-running guidance minimal in this document. Exact suite adoption may lag behind the intended split while implementation and fixture migration are still in progress.

## Suite contracts

### `annotations`

Purpose:

- annotation syntax parsing
- normalization
- rendered surface-type contract

Expected output should show:

- normalized type syntax
- precise parse-error kind for invalid cases

### `lowering`

Purpose:

- syntax-to-HIR lowering
- annotation attachment
- stable expression representation

Expected output should show:

- rendered AST-like dump of the HIR module
- attached types on expressions

### `naming`

Purpose:

- binding introduction
- shadowing
- use-site resolution

Expected output should show:

- normalized binding identities
- definition-site and use-site relationships

### `expressions`

Purpose:

- smaller checked-expression cases
- normalized expression result types
- focused checking behavior for small snippets

Expected output should show:

- rendered checked expression types
- or a normalized checking error kind when the suite intentionally targets failure shape

### `bindings`

Purpose:

- top-level binding results
- generalized binding types
- checked binding-level behavior

Expected output should show:

- normalized rendered binding types
- binding-level checked facts rather than raw engine state

### `interfaces`

Purpose:

- per-file exported interface rendering
- generalized exported types
- exported type definitions

Expected output should show:

- normalized exported bindings
- exported aliases or nominal declarations

### `diagnostics`

Purpose:

- user-facing checking behavior
- rendered diagnostics, wording, and ranges

Expected output should show:

- final rendered diagnostic text as the user should see it

## Testing guidance

- Prefer adding or tightening fixtures before writing parser-local or engine-local unit tests unless the behavior is genuinely awkward to express as a fixture.
- Favor fixture renderers that expose semantic facts rather than implementation detail.
- When adding a new phase or module, add or extend a fixture suite for that phase before relying on ad hoc unit tests.
- Use the lightest fixture change that captures the failing shape.
- Keep fixture expectation changes deliberate when output changes.
