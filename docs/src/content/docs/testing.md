---
title: Testing
description: The fixture-testing contract and suite structure
---

This crate prefers fixture tests for source-driven behavior because they are:

- easy for a human to read in diffs
- easy to extend into many cases quickly
- a good fit for verifying AI-generated changes against an explicit text contract

Fixture suites should describe the desired semantics as exhaustively as practical. Do not shape the
matrix around what the current implementation already happens to pass. If the suite is still
migrating toward the desired contract, record the missing coverage explicitly in the relevant
README instead of treating current gaps as intentional.

Use ordinary Rust tests only when the behavior is awkward to express as a rendered fixture.

## The rewrite stack's suites

The greenfield stack (`crates/syntax`, `crates/semantics`) has its own harness,
`syntax::testing::run_fixture_suite`: a `.R.test` file holds `#==== group` / `#---- case`
sections whose source is followed by a `#++++` expectation block; `ROUGHLY_BLESS=1` rewrites
expectations and `FIXTURE_FILTER=group__case` runs one case. Suites:

- `crates/syntax/tests/syntax` — golden lossless trees plus syntax errors (`debug_dump`)
- `crates/syntax/tests/tsr` — tree-sitter-r's parser corpus converted to the same format
- `crates/syntax/tests/errors` — golden Elm-style error-message rendering
- `crates/semantics/tests/typing` — the typing suite: each case runs the full semantic
  pipeline on one package file (shipped stubs installed) and renders every named top-level
  definition's exported scheme (`name: <T: numeric> fn(x: T) -> T`) followed by the file's
  diagnostics (`start..end severity[code] message`, byte offsets)
- `crates/semantics/tests/typing-scripts` — the same pipeline over script documents (one
  sequential top-down scope)
- `crates/semantics/tests/typing-strict` — the strict stream: the per-file typing mode and
  the `strict`-code diagnostics appended after the ordinary rendering
- `crates/format/tests/format` — the formatter golden suite (ported from the legacy suite):
  each case's source formats to the expected block, and the runner re-formats the output to
  assert idempotence on every case; a case whose expectation is a refusal renders the
  structured `FormatError`
- `crates/ide/tests/ide` — IDE feature fixtures: the case source carries one `$0` cursor
  marker (stripped before analysis) and the expectation renders each feature's result at
  that position (hover line with its absolute range, definition target range, reference
  ranges)

### The cross-stack differential gate

`cargo test -p differential` runs every case of the typing, typing-scripts, and typing-strict
suites through **both** stacks — the frozen legacy pipeline as the oracle and the rewrite's
`file_diagnostics` / `strict_diagnostics` — and compares the semantic diagnostic classes
(`type`, `annotation`, `unresolved`, `unused`, `strict`; syntax is excluded because the new
parser's errors are required to be better, not identical). The harness mirrors the legacy
publication rules: scripts are classified by path, type and strict findings honor the per-file
typing directive over the configured default, and annotation and naming findings are always
published. Two findings match when their class agrees and the new range equals or lies **inside** the
legacy range — strictly tighter ranges are an intended improvement, not a divergence. Message
text is not compared: wording is free to improve on the oracle's (the fixture suites are the
wording contract), and pairs whose messages differ are listed in an informational "wording
differences" report section. Cases where the oracle itself is wrong are allowlisted in the test with the reason,
the harness flags stale allowlist entries, and each suite's test fails on any unexplained
divergence; the per-case details land in `target/differential-<suite>.txt`.

A fourth, ignored-by-default arm compares the stacks over the real-file corpus
(`cargo test -p differential -- --ignored differential_corpus`, after `scripts/fetch-corpus.sh`):
every corpus `.R` file both parsers accept runs through both pipelines with the same matching
policy — files with syntax errors on either side are counted and skipped, since parity is scoped
to inputs both stacks parse cleanly. Its report (`target/differential-corpus.txt`) leads with a
frequency rollup of divergent messages, so one gap repeated across hundreds of files reads as one
line. Per-file panic guards on both sides keep one crash from killing the sweep: a new-stack
panic is recorded and fails the test; a legacy (oracle) panic is tolerated per case only when
allowlisted.

### The per-position IDE differential

`cargo test -p differential --test test_ide_differential` runs hover, goto-definition,
references, rename, signature help, and completion at **every byte position** of every
typing-suite case through both stacks (plus inlay-hint anchors once per case) and compares
targets, ranges, and label sets —
never prose, per the wording-freedom doctrine: definitions must agree on presence with the
rewrite's target equal to or inside one of the oracle's; reference and rename sets must be
identical; hovers must agree on presence with the rewrite's range equal or inside the
oracle's; signature help must agree on presence, the signature-set size, the committed
overload index, and the active signature's active parameter; hint anchors must
match exactly; completion label sets must match with a completion DEFICIT always a divergence
(supersets are accepted — the rewrite's pools are deliberately richer: the type vocabulary
inside annotations, stub namespace exports after `pkg::`, an item-wide local pool). Divergence
classes accepted by policy are counted separately: the rewrite hovering or offering signatures
where the oracle does not (strictly more coverage), and the rewrite declining references/rename
on annotation type tokens with no project declaration (primitives, `fn`, binders — the oracle
offers spelled-name matches there). Cases where the
oracle's naming is wrong (forward capture, super-assignment, local mutual recursion) sit on a
committed allowlist that only accepts pure additions, and adjudicated design differences (the
rewrite hints only exported-scheme-consistent types) carry their reasons in a second list;
both fail when stale. The test is a hard gate: any unexplained divergence fails; details land
in `target/differential-ide.txt`.

### The semantics fuzz harness

Fuzzing is pipeline-wide and from each stage's first commit, not a parser-only concern.
`crates/semantics/tests/test_fuzz.rs` runs generated programs (biased toward reference cycles,
closures, annotations, and typing-mode directives), token soup, and two-file projects through
the full semantic pipeline, checking on every input: never-panic (salsa fixpoints must
converge), determinism across fresh databases, diagnostic-range geometry, and **incremental
equivalence** — output after editing a file through the salsa setter equals a fresh database on
the edited text, and editing back restores the original. `FUZZ_ITERS` scales the budgets; the
bounded default runs in `cargo test -p semantics`, and `fuzz_deep` (ignored) carries long runs.
On a panic the harness prints the generating inputs.

### The formatter fuzz harness

`crates/format/tests/test_fuzz.rs` holds the formatter's arm of the same doctrine. On every
input — valid-program seeds, byte-level seed mutations, token soup, random bytes, and real
corpus files when fetched — it checks: never-panic (`format` either succeeds or refuses with a
structured error), determinism, and **idempotence**: whenever formatting succeeds, formatting
the output again must succeed and reproduce it byte-for-byte. The refusal path is part of the
property: a file with an R-grammar syntax error is refused, while errors raised by the `#:`
annotation grammar (marked `in_annotation` by the parser) only send the affected block down
the verbatim path.

## Fixture format

The current analysis fixture runner lives in `tests/test_fixtures.rs`.
Fixture parsing itself lives in the separate `fixtures` crate.

The `roughly` crate's formatter tests reuse the same harness: `crates/roughly/tests/test_format.rs`
runs a `tests/format` suite through the `fixtures` crate, so the `Simple` `#++++` shape,
`ROUGHLY_BLESS=1`, and `FIXTURE_FILTER` all apply there too. Run it with
`cargo test -p roughly --test test_format`.

Some suites may include a local `README.md` with more detailed strategy, coverage expectations, or renderer-specific guidance. Use this page for crate-level test contracts and suite-local README files for suite-specific concepts that would otherwise make this page too long.

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
- the shared `ide` runner currently implements `hover` (and `hover_debug`), `completion`,
  `rename`, `goto_definition`, `references`, `signature_help`, and `inlay_hints`; keep new IDE
  actions in that one runner rather than per-action suite-specific conventions
- the per-action request and output formats are documented in `tests/ide/README.md`

### Current migration status

The parser now understands both fixture shapes.

`naming/local` uses `Simple` cases and runs only the file-local naming pass.
`naming/global` uses `MultiFile` cases and runs package-global naming on the initial generation.
`typecheck/project` uses `MultiFile` cases and runs the full pipeline with typing enabled.
The old engine-centric `tests/typecheck/deprecated/` migration storage is deleted. The active
split is `bindings`, `expressions`, `interfaces`, `project`, and the internal `unification`
suite.
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

## Blessing expectations

Set `ROUGHLY_BLESS=1` to rewrite the `#++++` expectation blocks in the source `.test` files in
place from the actual runner output instead of failing on a mismatch:

```sh
ROUGHLY_BLESS=1 cargo test -p analysis --test test_fixtures
```

Behavior:

- only the content body of each `#++++` (or `#++++ path`) block is rewritten; directive lines, file
  order, and surrounding blank-line spacing are preserved, so a blessed file is byte-identical to
  what a human would have written and re-running without bless passes
- `#++++ any` blocks are left untouched
- carried-forward expectations are blessed at the generation where their block is written, not where
  they are reasserted by carry-forward
- `FIXTURE_FILTER` still applies, so you can bless a single case
- blessing an already-correct suite changes no bytes
- bless only repairs expectation drift; a fixture with a structural mismatch (wrong snapshot count,
  a missing or extra output path, or duplicate outputs) is still reported as a failure with the
  normal diff rather than rewritten

Review every blessed change before committing: bless captures whatever the runner currently
produces, so it will happily record an intentionally wrong outcome if the implementation is wrong.

## Suite direction

The intended fixture suites are:

- `type_syntax` - typing-comment syntax and normalized type rendering
- `bindings` - binding-boundary stored type schemes
- `diagnostics` - final user-facing errors
- `unused` - unused-assignment (dead-store) warnings from the reaching-write analysis, run with the `unused` check enabled
- `expressions` - checked expression result types
- `interfaces` - exported per-file interface shapes
- `ide` - editor-facing queries (hover, completion, rename, goto_definition, references) over
  multi-file workspace state, split into per-feature subdirectories under `tests/ide/`
- `lint` - file-local lint diagnostics
- `lowering` - syntax-to-HIR lowering output
- `stub` - the declaration-line parser for `.Rtypes` stub files (`name : <type-expr>`), reusing the
  type-expression parser; the type grammar itself stays in `type_syntax`
- `naming/local` - file-local binding introduction and lexical use-site resolution
- `naming/global` - package-global resolution across multiple files
- `project` - multi-file typed package behavior, currently rendered as per-file diagnostics
- `realworld` - complete idiomatic R programs through the full production analysis, pinning the
  false-positive rate on clean code and true positives on buggy code
- `unification` - optional internal raw-inference coverage for metavariable-facing engine behavior

Keep focused test-running guidance minimal in this document. Exact suite adoption may lag behind the intended split while implementation and fixture migration are still in progress.

## Engine differential harnesses

Incremental analysis (the `engine` crate, see [Architecture](/architecture)) is not fixture-tested; it is held to a **differential regression net** that asserts the engine's output equals a from-scratch rebuild. These live in `crates/engine/tests/` (plus one in `crates/roughly/tests/`), and run with `cargo test -p engine`:

- `test_differential` — engine diagnostics == `analysis::run_full` (a fresh from-scratch build), byte-exact on rendered diagnostics, asserted after every edit over curated and randomized adversarial edit streams (interleaved edits and queries, add/delete/re-add, package↔script reclassification, renames, re-export and value cycles, malformed input).
- `test_ide_differential` — every IDE feature compared per cursor position against a fresh-`Analysis` oracle, cold and over incremental edit streams, single-file and cross-file.
- `test_ide_granularity` — exec-counter proofs that a per-keystroke point query on an unchanged file triggers zero re-inference.
- `test_symbols_differential` (in `crates/roughly`) — engine-served document and workspace symbols == the oracle.
- `test_engine`, `test_queries`, `test_reexport`, `test_cancellation`, `test_read_cancellation`, `test_memory`, `test_benchmark` — the memoized core, the query bodies, the re-export fixed-point, cooperative cancellation, and the memory / per-edit-cost measurements.
- `test_document_lifecycle` — scripts the LSP document events (`did_open`, `did_change`, `did_save`, `did_close`, `did_change_watched_files`) against the engine through the *same* event → engine-input mapping the server uses (`crates/roughly/src/server.rs`), so the incremental paths are driven the way an editor drives them. It models the open-buffer-vs-disk duality the events depend on (`did_change` is an incremental range edit against a live buffer; `did_close` of an on-disk package file reverts to disk; a watched change to an *open* file is ignored). It pins the invariants that had no other test in this form: a transiently-malformed edit emits no spurious semantic diagnostics and a following well-formed edit restores them (the `Lower` empty-module short-circuit, also pinned directly by `test_malformed_lower`), and a watched-file add/delete changes cross-file resolution.

Because the from-scratch oracle (`analysis::run_full`) is the ground truth, keep it correct and well-fixtured: the differential net proves the *engine* matches the oracle, while the fixtures above pin the oracle to the language.

This is a **Rust harness, not a fixture DSL**. The `MultiFile` fixture shape (whole-file create / range edit / delete / move with per-generation diagnostics) models document *state* transitions but has no open-set or on-disk-vs-buffer model and does not distinguish which LSP event drove a change, so an event sequence that turns on those distinctions (close-reverts-to-disk, ignore-watched-while-open, open→edit→save) is expressed as a scripted driver alongside the other engine incremental drivers rather than by extending the fixture grammar.

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

### `realworld`

Purpose:

- evaluate the type system on complete, idiomatic R programs rather than isolated constructs
- pin the end-to-end false-positive rate: realistic clean programs must produce `No diagnostics.`
- pin true positives: programs with planted bugs must report exactly the planted diagnostics

Expected output should show:

- final rendered diagnostic text through the full production analysis (typing and unused checks
  enabled, standard-library stub corpus loaded)

Rules:

- write cases as real programs (CSV pipelines, config handling, text processing), not minimal
  construct demos — construct-level coverage belongs in `expressions`, `bindings`, or `diagnostics`
- an unexpected diagnostic on a clean program is a product regression to fix, never an expectation
  to re-bless
- when the checker legitimately catches a program error in a supposedly-clean case, fix the R code
  (it was a true positive) or move it to the buggy catalog — do not bless the diagnostic away

## Testing guidance

- Prefer adding or tightening fixtures before writing parser-local or engine-local unit tests unless the behavior is genuinely awkward to express as a fixture.
- When adding a new phase or module, add or extend a fixture suite for that phase before relying on ad hoc unit tests.
- Use the lightest fixture change that captures the failing shape.
- Keep fixture expectation changes deliberate when output changes.
- Prefer keeping typecheck coverage in fixtures unless the behavior is too low-level or too
  mechanical to express clearly in rendered fixture output.
