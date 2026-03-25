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

- `typing_syntax` - typing-comment syntax and normalized type rendering
- `bindings` - top-level binding result types
- `diagnostics` - final user-facing errors
- `environment` - rebinding, shadowing, and scheme reuse across scopes
- `expressions` - checked expression result types
- `generalization` - quantified schemes produced at binding boundaries
- `instantiation` - fresh reuse of generalized bindings at use sites
- `interfaces` - exported per-file interface shapes
- `lowering` - syntax-to-HIR lowering output
- `naming` - binding introduction and use-site resolution
- `substitution` - propagation of solved types through larger shapes
- `unification` - solved monotypes during local inference

Keep focused test-running guidance minimal in this document. Exact suite adoption may lag behind the intended split while implementation and fixture migration are still in progress.

## Suite contracts

### `typing_syntax`

Purpose:

- typing-comment syntax parsing
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

### `unification`

Purpose:

- monotypes produced while inference solves local constraints
- how expression shapes and operators constrain inference variables
- higher-order function shapes before binding-level generalization

Expected output should show:

- rendered monotypes using inference metavariables such as `?1`

### `generalization`

Purpose:

- generalized schemes assigned at top-level binding boundaries
- which variables remain polymorphic after local constraints are solved
- how annotations or concrete operations prevent quantification

Expected output should show:

- `name: TYPE_SCHEME` lines
- explicit quantifiers such as `<T>` when a binding is polymorphic

### `instantiation`

Purpose:

- how generalized bindings are reused at later use sites
- whether each use site receives a fresh instantiation
- interaction between polymorphic bindings and higher-order calls

Expected output should show:

- generalized binding schemes for top-level assignments
- instantiated expression result types for later expressions

### `bindings`

Purpose:

- top-level binding results
- generalized binding types
- checked binding-level behavior

Expected output should show:

- normalized rendered binding types
- binding-level checked facts rather than raw engine state

### `substitution`

Purpose:

- propagation of solved types through nested type structure
- how local constraints update higher-order and returned-function shapes
- solved binding and call results after inference variables are replaced consistently

Expected output should show:

- generalized binding schemes for top-level assignments when applicable
- rendered expression result types after solved substitutions have propagated

### `environment`

Purpose:

- binding storage and rebinding across the top-level environment
- aliasing and reuse of generalized schemes
- shadowing across function parameters and nested scopes

Expected output should show:

- generalized binding schemes for top-level assignments when applicable
- rendered expression result types for later uses under the resulting environment

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
- When adding a new phase or module, add or extend a fixture suite for that phase before relying on ad hoc unit tests.
- Use the lightest fixture change that captures the failing shape.
- Keep fixture expectation changes deliberate when output changes.
