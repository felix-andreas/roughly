# Testing

This crate prefers fixture tests for source-driven behavior because they are:

- easy for a human to read in diffs
- easy to extend into many cases quickly
- a good fit for verifying AI-generated changes against an explicit text contract

Use ordinary Rust tests only when the behavior is awkward to express as a rendered fixture.

## Fixture format

The current typing fixture runner lives in `tests/test_fixtures.rs`.
Fixture parsing itself lives in the separate `fixtures` crate.

Some suites may include a local `README.md` with more detailed strategy, coverage expectations, or renderer-specific guidance. Use `TESTING.md` for crate-level test contracts and suite-local README files for suite-specific concepts that would otherwise make this document too long.

The `fixtures` crate currently parses three fixture shapes:

- `Simple`
- `MultiFile`
- `Generational`

### `Simple`

Use `Simple` when one case has one implicit main input document and one expected output.

```text
#==== group_name
#---- case_name
<input source>
#++++
<expected rendered output>
```

Rules:

- `Simple` uses a bare `#++++` separator with no path
- `group__case` is the stable test identity
- `group__case` names must be unique across the suite
- suite-specific behavior belongs in the suite runner, not in fixture syntax

For `type_syntax`, invalid cases use the same `Simple` shape as valid cases. Put the actual type-syntax source in the input block and snapshot the rendered parse error in `#++++`. Do not prefix invalid inputs with `error:`.

### `MultiFile`

Use `MultiFile` when one case has multiple whole-file inputs and per-file expectations, but no later generations.

```text
#==== group_name
#---- case_name
#---- a.R
<file contents>
#---- b.R
<file contents>
#++++ a.R
<expected rendered output for a.R>
#++++ b.R
<expected rendered output for b.R>
```

Rules:

- every `#---- path` entry before the first `#++++` is a whole-file input
- every `#++++ path` entry is a per-file expectation
- `MultiFile` does not use `edit`, `move`, `delete`, or explicit expectation clearing

### `Generational`

Use `Generational` when a case needs grouped workspace edits across later generations.

```text
#==== group_name
#---- case_name
#.... v1
#---- a.R
<file contents>
#---- b.R
<file contents>
#++++ a.R
<expected rendered output for a.R>
#++++ b.R
<expected rendered output for b.R>
#.... v2
#---- edit a.R 3:1-3:4 -> "foo"
#---- move b.R -> c.R
#---- delete d.R
#++++ a.R
<expected rendered output for a.R>
#++++ none c.R
```

Rules:

- each `#.... vN` block describes one grouped workspace edit step
- all `#----` entries in a generation come before any `#++++` expectations for that generation
- bare filenames such as `#---- a.R` mean whole-file contents for that generation
- `edit`, `move`, and `delete` are first-class workspace document operations
- `#++++ none path` explicitly clears a carried expectation for that document
- one fixture case may describe package documents, auxiliary documents, and standalone documents
- if a document already has an expectation from an earlier generation, that expectation carries forward until replaced or explicitly cleared

### Current migration status

The parser now understands all three fixture shapes.

The `naming` fixture runner executes `Simple` and `MultiFile` cases.
The other typing fixture runners still execute only `Simple` cases.
Treat `Generational` and the remaining multi-file suite support as parser-level functionality that is still waiting on renderer-side adoption in `typing`.

### File loading

The fixture runner only loads files with the `.test` extension, so suite-local `README.md` files are ignored by the runner.

### Historical simple example

Every `.test` file may still use the original simple shape:

```text
#==== group_name
#---- case_name
<input source>
#++++
<expected rendered output>
```

Planned architecture:

- the reusable `workspace` crate owns workspace/package/document state and incremental parsing
- the separate `fixtures` crate parses this fixture language
- the testing framework may combine parsed fixture data with `workspace` document state
- `typing` should not own a second workspace/document engine inside the fixture harness

## Focused runs

Run one focused fixture case with:

```sh
FIXTURE_FILTER=group__case cargo test -p typing --test test_fixtures <suite> -- --nocapture
```

Default crate test command:

```sh
cargo test -p typing
```

## Suite direction

The intended fixture suites are:

- `type_syntax` - typing-comment syntax and normalized type rendering
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

### `type_syntax`

Purpose:

- typing-comment syntax parsing
- normalization
- rendered surface-type contract

Expected output should show:

- normalized type syntax
- precise parse-error kind for invalid cases

Invalid `type_syntax` fixtures use the same `Simple` shape as successful ones. The runner parses the input and compares either the rendered type or the rendered parse error directly.

### `lowering`

Purpose:

- syntax-to-HIR lowering
- annotation attachment
- stable expression representation
- diagnostics that arise during lowering itself, for example annotation attachment failures

Expected output should show:

- rendered AST-like dump of the HIR module
- attached types on expressions
- or rendered diagnostics when the suite intentionally targets failures produced by lowering

### `naming`

Purpose:

- binding introduction
- shadowing
- use-site resolution

Expected output should show:

- normalized binding identities
- definition-site and use-site relationships

The naming suite should be organized as a small coverage matrix rather than a grab bag of examples.

At minimum, that matrix should cover:

- top-level bindings
- function parameters
- local rebinding inside blocks
- nested functions closing over outer bindings
- inner bindings shadowing outer bindings
- loop bindings such as `for` variables
- assignment RHS resolution before the new binding is introduced
- naming diagnostics for type references that fail during naming

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
