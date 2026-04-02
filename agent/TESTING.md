# Testing

This crate prefers fixture tests for source-driven behavior because they are:

- easy for a human to read in diffs
- easy to extend into many cases quickly
- a good fit for verifying AI-generated changes against an explicit text contract

Use ordinary Rust tests only when the behavior is awkward to express as a rendered fixture.

## Fixture format

The current analysis fixture runner lives in `tests/test_fixtures.rs`.
Fixture parsing itself lives in the separate `fixtures` crate.

Some suites may include a local `README.md` with more detailed strategy, coverage expectations, or renderer-specific guidance. Use `TESTING.md` for crate-level test contracts and suite-local README files for suite-specific concepts that would otherwise make this document too long.

The shared naming suite README at `tests/naming/README.md` is authoritative for the naming fixture
matrix and the local-versus-global suite split. Keep it in sync with naming fixture changes in the
same session.

The `fixtures` crate currently parses two fixture shapes:

- `Simple`
- `MultiFile`

All analysis fixture runners should normalize their results to one shared output shape:

```rust
Result<Vec<Vec<FixtureRunFile>>, String>
```

Where each inner `Vec<FixtureRunFile>` is one snapshot and each snapshot contains per-file rendered
outputs by path. Snapshot outputs are matched to expected generations by position, not by a
runner-supplied name. `Err(...)` is for runner failure only. Phase failures should still be
rendered into normal fixture output.

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
- the runner still returns the shared structured output shape, even for `Simple`

For `type_syntax`, invalid cases use the same `Simple` shape as valid cases. Put the actual type-syntax source in the input block and snapshot the rendered parse error in `#++++`. Do not prefix invalid inputs with `error:`.

### `MultiFile`

Use `MultiFile` when a case needs explicit file paths. It may have only an initial set of whole-file
inputs, or it may also include later grouped workspace edits.

```text
#==== group_name
#---- case_name
#---- a.R
<file contents>
#++++
<expected rendered output for a.R>
#---- b.R
<file contents>
#++++
<expected rendered output for b.R>
#.... v1
#---- a.R
<file contents>
#++++
<expected rendered output for a.R>
#---- b.R
<file contents>
#++++
<expected rendered output for b.R>
#.... v2
#---- edit a.R 3:1-3:4 -> "foo"
#++++
<expected rendered output for a.R>
#---- move b.R -> c.R
#++++ any
#---- delete d.R
```

Rules:

- the initial `MultiFile` inputs are the first snapshot
- each `#.... vN` block describes one grouped workspace edit step
- each input or operation block is followed immediately by any expectation update for that path
- bare filenames such as `#---- a.R` mean whole-file contents for that snapshot
- `edit`, `move`, and `delete` are first-class workspace document operations
- expectations carry forward by path from one generation to the next unless replaced
- if a checked path is missing an expectation in the first generation, that is an error
- `#++++ any` means the immediately preceding file or operation is expected, but its contents are
  not asserted
- deleting a document means later generations should not produce output for that path unless the
  file is reintroduced
- moving a document carries the expectation from the source path to the destination path
- extra actual outputs beyond the expected paths are an error
- in analysis suites, use explicit paths such as `R/a.R` when package behavior matters; the runner
  does not add package prefixes for you
- generation entries may also retarget the asserted output with `#++++ path`

### Current migration status

The parser now understands both fixture shapes.

`naming/local` uses `Simple` cases and runs only the file-local naming pass.
`naming/global` uses `MultiFile` cases and runs package-global naming on the initial generation.
The other analysis fixture runners still execute only `Simple` cases.
Later-generation `MultiFile` support is still waiting on broader renderer-side adoption in
`analysis`.

### File loading

The fixture runner only loads files with the `.test` extension, so suite-local `README.md` files are ignored by the runner.

## Focused runs

Run one focused fixture case with:

```sh
FIXTURE_FILTER=group__case cargo test -p analysis --test test_fixtures <suite> -- --nocapture
```

Default crate test command:

```sh
cargo test -p analysis
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
- `ide/hover` - hover rendering over multi-file workspace state
- `lowering` - syntax-to-HIR lowering output
- `naming/local` - file-local binding introduction and lexical use-site resolution
- `naming/global` - package-global resolution across multiple files
- `substitution` - propagation of solved types through larger shapes
- `unification` - solved monotypes during local inference

Keep focused test-running guidance minimal in this document. Exact suite adoption may lag behind the intended split while implementation and fixture migration are still in progress.

## IDE fixture direction

The current hover suite lives under `tests/ide/hover` and uses ordinary `MultiFile` fixtures plus
`.hover` request files. The shared fixture grammar now supports `#++++ path` on generation entries,
which lets one workspace mutation update the expectation for a different request file.

Use that low-level shape when a suite only needs one request kind and one assertion after each
workspace step.

Reasoning:

- IDE requests such as hover are assertions over the current workspace state, not always over the
  mutated source file itself.
- The lower-level `fixtures` crate should support this pattern generally because it is also useful
  outside hover.

Longer-term IDE direction:

- If IDE coverage expands to `rename`, `goto_definition`, and `assert_content`, prefer one combined
  IDE fixture runner rather than forcing every request through path-targeted `#++++`.
- The likely split is:
  - `#----` for workspace mutations
  - `#!!!!` for IDE requests such as `hover`, `rename`, `goto_definition`, and `assert_content`
- Keep `#++++ path` as the shared low-level fixture capability even if IDE suites later standardize
  on `#!!!!`.

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

### Naming

Purpose:

- binding introduction within one document
- lexical shadowing
- use-site resolution within one document
- package-global resolution after the local pass
- project-global type-name resolution

The detailed naming matrix lives in `tests/naming/README.md`.

Expected output should show:

- normalized binding identities
- definition-site and use-site relationships
- and diagnostics when the case intentionally targets naming failure behavior

Rules:

- `tests/naming/local/` covers the file-local lexical view before package-global resolution
- `tests/naming/global/` is the primary contract and should mirror the local lexical cases while
  also covering package-only behavior
- mirrored global cases should preserve the same group/case names as their local counterparts
- mirrored global cases should use `MultiFile`, even when the package has one file
- project-global type-name cases belong in `tests/naming/global/`, not a dedicated local type suite
- package-only naming coverage currently lives in `cross_file_values`, `type_lookup`,
  `type_shadowing`, `type_failures`, `scripts`, and `failures`
- keep the shared naming README updated whenever the naming matrix changes

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
