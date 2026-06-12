# Testing

This crate prefers fixture tests for source-driven behavior because they are:

- easy for a human to read in diffs
- easy to extend into many cases quickly
- a good fit for verifying AI-generated changes against an explicit text contract

Fixture suites should describe the desired semantics as exhaustively as practical. Do not shape the
matrix around what the current implementation already happens to pass. If the suite is still
migrating toward the desired contract, record the missing coverage explicitly in the relevant
README instead of treating current gaps as intentional.

Use ordinary Rust tests only when the behavior is awkward to express as a rendered fixture.

## Fixture format

The current analysis fixture runner lives in `tests/test_fixtures.rs`.
Fixture parsing itself lives in the separate `fixtures` crate.

Some suites may include a local `README.md` with more detailed strategy, coverage expectations, or renderer-specific guidance. Use `TESTING.md` for crate-level test contracts and suite-local README files for suite-specific concepts that would otherwise make this document too long.

The shared naming suite README at `tests/naming/README.md` is authoritative for the naming fixture
matrix and the local-versus-global suite split. Keep it in sync with naming fixture changes in the
same session.

Top-level fixture suites should generally also include a local `README.md` with a coverage matrix.
Use those README files to describe the suite's rendered output contract, semantic matrix, what does
not belong in the suite, and naming guidance for fixture groups and cases.

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

IDE-style suites may also use action operations:

```text
#!!!! hover lookup.hover
R/main.R:1:30
#++++
<expected hover output>
```

Rules:

- `#!!!!` introduces an IDE action, not a workspace document
- the action body is the runner-defined action input
- the following `#++++` block is the rendered output for that action path
- actions are snapshot-local and do not carry forward into later generations
- the shared fixture parser only records the action name and path; the suite runner defines what
  each action means
- use this shape for focused IDE suites such as `hover`
- the shared `ide` runner currently implements `hover`, `completion`, `rename`,
  `goto_definition`, and `references`; keep new IDE actions in that one runner rather than
  per-action suite-specific conventions
- the per-action request and output formats are documented in `tests/ide/README.md`

### Current migration status

The parser now understands both fixture shapes.

`naming/local` uses `Simple` cases and runs only the file-local naming pass.
`naming/global` uses `MultiFile` cases and runs package-global naming on the initial generation.
`typecheck/project` uses `MultiFile` cases and runs the full pipeline with typing enabled.
Older engine-centric typecheck fixture files still live under `tests/typecheck/deprecated/`
(`generalization`, `instantiation`, `substitution`, `environment`) as migration storage; they are
not wired as active suites. The active split is `bindings`, `expressions`, `interfaces`,
`project`, and the internal `unification` suite.
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
- `bindings` - binding-boundary stored type schemes
- `diagnostics` - final user-facing errors
- `expressions` - checked expression result types
- `interfaces` - exported per-file interface shapes
- `ide` - editor-facing queries (hover, completion, rename, goto_definition, references) over
  multi-file workspace state, split into per-feature subdirectories under `tests/ide/`
- `lint` - file-local lint diagnostics
- `lowering` - syntax-to-HIR lowering output
- `naming/local` - file-local binding introduction and lexical use-site resolution
- `naming/global` - package-global resolution across multiple files
- `project` - multi-file typed package behavior, currently rendered as per-file diagnostics
- `unification` - optional internal raw-inference coverage for metavariable-facing engine behavior

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

### `lint`

Purpose:

- file-local lint diagnostics
- style checks that depend only on parsed tree structure and source text

Expected output should show:

- rendered lint diagnostics directly
- no lowering, naming, or typechecking output mixed into the suite

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
- control-flow-sensitive local availability belongs in `maybe_undefined.R.test`, where names remain
  locally resolved but carry a naming warning when introduction is conditional
- mirrored global cases should preserve the same group/case names as their local counterparts
- mirrored global cases should use `MultiFile`, even when the package has one file
- project-global type-name cases belong in `tests/naming/global/`, not a dedicated local type suite
- package-only naming coverage currently lives in `cross_file_values`, `type_lookup`,
  `type_shadowing`, `type_failures`, `scripts`, and `failures`
- keep the shared naming README updated whenever the naming matrix changes

### `expressions`

Purpose:

- use-site and evaluation-site typing behavior
- resolved expression result types
- focused checking behavior for small snippets

Expected output should show:

- rendered checked expression types
- or a normalized checking error kind when the suite intentionally targets failure shape

Rules:

- `expressions` is the home for ordinary language semantics such as calls, control flow, indexing,
  arithmetic, special types, polymorphic use, and scoping effects
- cases from older `tests/typecheck/deprecated/environment/`,
  `tests/typecheck/deprecated/instantiation/`, and
  `tests/typecheck/deprecated/substitution/` directories should migrate here when their rendered
  output is ordinary expression results rather than binding schemes

### `bindings`

Purpose:

- binding-boundary typing facts
- generalized type schemes stored for names
- rebinding history at the assignment boundary
- binding-level annotation behavior

Expected output should show:

- `name: TYPE_SCHEME` lines
- explicit quantifiers such as `<T>` when a binding is polymorphic

Rules:

- `bindings` asks what type scheme gets stored at a binding boundary, not what later use sites
  evaluate to
- `bindings` may show repeated top-level rebinding history instead of collapsing to the final
  export only
- cases from older `tests/typecheck/deprecated/generalization/` fixtures belong here

### `interfaces`

Purpose:

- per-file exported interface rendering
- final exported bindings after top-level rebinding settles
- exported type definitions
- mixed exported value-plus-type surface

Expected output should show:

- normalized exported bindings
- exported aliases or nominal declarations
- final surviving export order
- function interface shapes, including named or optional parameters when present

Rules:

- `interfaces` answers what a file exposes, not what happened at each binding boundary
- `interfaces` may collapse repeated top-level rebindings to the final visible export
- `interfaces` is separate from `bindings` because its rendered output contract is different
- default to `Simple` fixtures here; most interface rules are file-local exported-surface checks
- if multi-file support lands, use it here only for several independent file-local interface
  snapshots in one workspace state
- cross-file value/type visibility, package winner behavior, and later-generation workspace edits
  belong in `project`, not `interfaces`

### `unification`

Purpose:

- raw internal inference behavior
- monotypes produced while inference solves local constraints
- how expression shapes constrain inference variables before generalization

Expected output should show:

- rendered monotypes using inference metavariables such as `?1`

Rules:

- this suite is internal-facing and optional
- keep it only when raw metavariable snapshots remain useful as fixtures
- direct Rust tests are still preferred for tiny `InferenceState` invariants

### `project`

Purpose:

- cross-file value and type use through package-global naming
- later-file winner behavior
- script versus package document behavior

Expected output should show:

- per-file rendered diagnostics, with `No diagnostics.` for clean files

Rules:

- the suite runs the full pipeline on `MultiFile` fixtures with typing enabled
- typed cross-file snapshots wait on the pipeline retaining checked-file results; the detailed
  matrix lives in `tests/typecheck/project/README.md`

### `diagnostics`

Purpose:

- user-facing checking behavior
- rendered diagnostics, wording, and ranges

Expected output should show:

- final rendered diagnostic text as the user should see it

Rules:

- keep the dedicated diagnostics suite for now
- do not use diagnostics fixtures as a substitute for missing semantic type-output coverage in
  `bindings`, `expressions`, or `interfaces`
- if a future incremental-analysis suite later becomes the owner of full rendered diagnostics,
  revisit this split then rather than preemptively collapsing it now

## Testing guidance

- Prefer adding or tightening fixtures before writing parser-local or engine-local unit tests unless the behavior is genuinely awkward to express as a fixture.
- When adding a new phase or module, add or extend a fixture suite for that phase before relying on ad hoc unit tests.
- Use the lightest fixture change that captures the failing shape.
- Keep fixture expectation changes deliberate when output changes.
- Prefer keeping typecheck coverage in fixtures unless the behavior is too low-level or too
  mechanical to express clearly in rendered fixture output.
