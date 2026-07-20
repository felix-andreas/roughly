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
- `crates/syntax/tests/errors` — golden Elm-style error-message rendering, organized by
  area (lexer, delimiters, control flow, functions/calls, `#:` type and directive grammar,
  recovery locality). The coverage contract: every distinct error message the lexer or
  parser can emit has at least one case here, valid-but-tricky shapes are pinned as
  `no errors`, and recovery cases assert that one broken construct reports once and leaves
  the rest of the file clean
- `crates/semantics/tests/typing` — the typing suite: each case runs the full semantic
  pipeline on one package file (shipped stubs installed) and renders every named top-level
  definition's exported scheme (`name: <T: numeric> fn(x: T) -> T`) followed by the file's
  diagnostics (`start..end severity[code] message`, byte offsets)
- `crates/semantics/tests/typing-scripts` — the same pipeline over script documents (one
  sequential top-down scope)
- `crates/semantics/tests/typing-strict` — the strict stream: the per-file typing mode and
  the `strict`-code diagnostics appended after the ordinary rendering
- `crates/semantics/tests/typing-imports` — package-metadata cases: leading `#namespace `
  lines form the case's NAMESPACE source and `#description ` lines its DESCRIPTION source,
  installed as the `PackageMetadata` input before rendering. The directive lines stay in the
  analyzed text as ordinary comments, so expectation offsets are honest. Attach facts need no
  directive: a `library(data.table)` statement in the case source activates the conditional
  stub namespace through the same scan the hosts run. This suite is new-stack only — the
  oracle has no package-metadata concept — and is deliberately absent from the differential
  fixture arm
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

The comparison harness itself — finding extraction from each stack, the class +
range-containment matching policy, and the accepted oracle-deficit filters — lives in the
`differential` crate's library, shared by every arm.

### The differential fuzz arm

`cargo test -p differential --test test_fuzz_differential` runs seeded random programs through
both stacks with the same matching policy (a package arm and a script arm, deterministic per
seed, `FUZZ_ITERS` scales the budget). The fixture arm only compares sources someone thought to
write down; this arm sweeps the space between, where a diagnostic class one stack silently lacks
shows up as a divergence instead of going unnoticed — it is what surfaced the missing script
unresolved checks, duplicate type-name errors, and conditional-slot semantics. Its generator is
differential-specific: the template pool is biased toward programs that PRODUCE findings both
stacks model, and strict mode stays off (strict attribution on recursive shapes diverges by
design and only the fixture arm's case allowlist can express that). Divergences accepted by the
shared filters are counted, never failed, and each filter is deliberately narrow with the
accepted class named in a rollup:

- **oracle deficits** — forward-capture unresolved/unused warnings the rewrite correctly
  resolves (site-scoped), bindings kept alive by pipeline reads the oracle does not model,
  Unknown-tolerant type checks the oracle over-rejects, and vector `[` refusals (the oracle
  never defined vector subsetting; the rewrite types it);
- **legacy slot tolerance** — use-before-write reads inside the defining statement the oracle
  misses because it mints the slot first;
- **model divergences** — unpaired type findings over *unstable names* (multiply-bound,
  self-referential, or forward-captured names, closed transitively), where the oracle's
  settled-slot frame model and the rewrite's sequential/last-wins model legitimately differ;
  an unpaired legacy/new pair with an identical message across a def-use edge (the oracle
  blames the defining item where the rewrite blames the reading site — one finding, two blame
  conventions); plus the rewrite's NULL-union strictness the oracle collapses to `Unknown`
  before reaching.

A default-suite arm sweeps the FROZEN STACK'S OWN fixture corpus
(`legacy_corpus_differential`): every single-file case input from the legacy
`typecheck`/`naming`/`type_syntax`/`diagnostics`/`unused`/`realworld` suites runs through both
stacks with the shared policy — years of accumulated shapes the rewrite's suites do not spell
out, compared without porting a single legacy expectation (the oracle recomputes its findings,
so legacy renderings never constrain the rewrite). Every case must match or carry an
adjudicated allowlist entry with its reason; stale entries fail.

A further ignored-by-default arm compares the stacks over the real-file corpus
(`cargo test -p differential -- --ignored differential_corpus`, after `scripts/fetch-corpus.sh`):
every corpus `.R` file both parsers accept runs through both pipelines with the same matching
policy — files with syntax errors on either side are counted and skipped, since parity is scoped
to inputs both stacks parse cleanly. Its report (`target/differential-corpus.txt`) leads with a
frequency rollup of divergent messages, so one gap repeated across hundreds of files reads as one
line. Per-file panic guards on both sides keep one crash from killing the sweep: a new-stack
panic is recorded and fails the test; a legacy (oracle) panic means the oracle cannot be
consulted for that file, so it is counted in the rollup and the file skipped.

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
the verbatim path. The invariant battery itself is exported as
`format::check_format_invariants` (with `syntax::testing::check_parse_invariants` for the
parser and `semantics::testing::check_semantics_input` for the semantic pipeline — the
latter folds every lint under an everything-on configuration into the rendering, so the
lint layer inherits the never-panic, determinism, geometry, and incremental invariants) so
the coverage-guided targets below share the exact same contracts, and each harness's
`fuzz_regressions_hold_invariants` pins every input those targets have ever broken.

### Coverage-guided fuzzing

`fuzz/` is a cargo-fuzz crate (its own workspace, excluded from the main one) with libFuzzer
targets `parse`, `format`, and `semantics`, each a thin wrapper over the exported
invariant batteries (the `semantics` target derives its document kind and incremental edit
from the input bytes, so one byte stream drives the full battery including the splice
cache). It
needs nightly: `cargo install cargo-fuzz`, then from `fuzz/`:

```sh
./../scripts/seed-fuzz-corpus.sh          # seed corpus from all fixture case sources
cargo +nightly fuzz run format -- -max_total_time=600 -print_final_stats=1
cargo +nightly fuzz run parse  -- -max_total_time=600
```

The seeder extracts every fixture case's source across the repository into
`fuzz/corpus/{parse,format}/` (content-addressed, so re-running only adds). Corpus and
artifacts are gitignored — durable regressions belong in the `REGRESSIONS` battery or a
fixture case, not in the corpus. On a find: `cargo +nightly fuzz tmin format <artifact>`
minimizes it; fix the bug, replay the artifact, add the minimized input to `REGRESSIONS`
(plus a golden fixture case when the shape deserves a pinned rendering). A coverage report
for judging corpus quality needs the llvm-tools component:
`cargo +nightly fuzz coverage format && cargo cov -- show` per the
[cargo-fuzz coverage guide](https://rust-fuzz.github.io/book/cargo-fuzz/coverage.html).
Deep runs are manual/scheduled work; the bounded in-tree batteries remain the default-suite
gate.

### The lint fixture suites

`crates/semantics/tests/lints/` runs `lints::lint_file` under the default configuration and
`tests/lints-style/` under `naming-style = "snake_case"` plus `unused-parameter = "warn"`
(both off by default); each case renders every finding as `start..end severity[code] message`
(or `clean`). Run with `cargo test -p semantics --test test_lint_fixtures`.

### The CLI contract suite

`crates/roughly/tests/test_cli.rs` drives the real `roughly` binary end to end: diagnostic
rendering (1-based positions, gutters, carets), JSON Lines output, the documented exit-code
contract (0 clean, 1 findings, 2 usage/configuration/IO errors), configuration discovery and
failure, per-file `# typing:` directives, suppression comments, NAMESPACE validation, project
stub overrides (including loader-problem reporting), data-masked evaluation, and the
formatter's `--check`/`--diff`/in-place modes.

### The LSP behavioral suite

`crates/roughly/tests/test_lsp.rs` spawns the real `roughly server` process and drives it over
LSP stdio with an async-lsp test client. Coverage: capability negotiation (position encodings,
pull diagnostics, refresh, label offsets, snippets), document sync (incremental and full-text,
untitled/non-`file:` buffers, close-time disk rereads), the two-wave push contract and its
settled-superset invariant, burst settling under latest-edit-wins cancellation, pull
diagnostics (result ids, unchanged reports, retryable cancellation, push suppression),
diagnostic refresh on save and config change, the config matrix (live reload, ancestor
configs, failure keeps the previous config, the config-file diagnostic), every feature
endpoint including UTF-16/UTF-8 range correctness with BMP and non-BMP content and
out-of-bounds safety, semantic tokens for `#:` bodies, and `.Rtypes`/NAMESPACE buffer serving.

The cancelled-pull test is deterministic through a fault-injection seam: with the
`ROUGHLY_TEST_DELAY_PULL_MS` environment variable set, the server announces each diagnostics pull
by creating the `ROUGHLY_TEST_PULL_MARKER` file and then holds the pull (until the cancellation
token flips, bounded by the delay) before computing. The test sends its `didChange` only after the
marker appears, so the edit's flip provably lands while the pull is in flight — the retryable
`SERVER_CANCELLED` response, and the successful retry after it, can be asserted without
sleeping-and-hoping. The variables exist only for this test; production runs never set them.

### The perf and memory witnesses

`crates/differential/tests/test_stats.rs` (ignored; needs the fetched corpus and a release
build) carries the measurement instruments — one process per stack reporting wall, phase
splits, and resident/peak memory into `target/stats-{new,legacy}.txt` — and `stats_witness`,
the CI-checkable assertion form: cold-pass wall per line, resident bytes per line, and
resolve steps per line (the resolve-memoization regression tripwire) against budgets set from
the measured gate numbers with headroom.

## Fixture format

Every suite on the shipping stack runs through one harness, `syntax::testing::run_fixture_suite`
(shared by the `syntax`, `semantics`, `format`, and `ide` crates and the differential). A `.test`
file holds groups of cases:

```text
#==== group_name
#---- case_name
<input source>
#++++
<expected rendered output>
```

Rules:

- `group__case` is the stable test identity; names must be unique across the suite — duplicates are
  rejected rather than silently shadowing one another
- what the expectation *shows* is the suite runner's contract (schemes, diagnostics, tree dumps,
  hover output, …); suite-specific behavior belongs in the runner, not in fixture syntax
- the runner only loads files with the `.test` extension, so suite-local `README.md` files are
  ignored by the runner
- some suites keep a local `README.md` with the suite's rendered-output contract and coverage
  matrix; keep those in sync with fixture changes in the same session

## Focused runs

Run one focused fixture case with `FIXTURE_FILTER` and the suite's test target, for example:

```sh
FIXTURE_FILTER=group__case cargo test -p semantics --test test_typing_fixtures -- --nocapture
FIXTURE_FILTER=group__case cargo test -p format --test test_format_fixtures -- --nocapture
FIXTURE_FILTER=group__case cargo test -p ide --test test_ide_fixtures -- --nocapture
```

The default crate test commands while iterating are `cargo test -p semantics` (analysis behavior)
and `cargo test -p differential` (cross-stack agreement).

## Blessing expectations

Set `ROUGHLY_BLESS=1` to rewrite the `#++++` expectation blocks in the source `.test` files in
place from the actual runner output instead of failing on a mismatch:

```sh
ROUGHLY_BLESS=1 cargo test -p semantics --test test_typing_fixtures
```

Behavior:

- only the content body of each `#++++` block is rewritten; directive lines and surrounding spacing
  are preserved, so a blessed file is byte-identical to what a human would have written and
  re-running without bless passes
- `FIXTURE_FILTER` still applies, so you can bless a single case
- blessing an already-correct suite changes no bytes

Review every blessed change before committing: bless captures whatever the runner currently
produces, so it will happily record an intentionally wrong outcome if the implementation is wrong.
Fixtures are the desired-semantics contract, not a regression archive — accept a changed
expectation only when the behavior or wording intentionally improved.

## The frozen legacy stack's harnesses

The previous implementation stays in-tree (`crates/analysis-legacy`, `crates/engine-legacy`,
`crates/roughly-legacy`, with its own `crates/fixtures` harness) as the cross-implementation oracle
the differential compares against, and as the benchmark baseline. Its suites still run —
`cargo test -p analysis-legacy` / `-p engine-legacy` / `-p roughly-legacy`, and the workspace-wide
battery covers them — but the stack is frozen: do not extend its fixtures or harnesses, and never
share code between the two stacks. Its `fixtures` crate parses the same `Simple` shape plus a
`MultiFile` shape (explicit file paths and grouped workspace edits) that its engine-era suites use.
Beyond fixtures, the legacy engine carries its own differential regression net (engine output
asserted equal to a from-scratch rebuild over adversarial edit streams, per-position IDE parity
against a fresh-analysis oracle, exec-counter and memory witnesses); it documents the bar the
rewrite's own harnesses were built to meet.

## Testing guidance

- Prefer adding or tightening fixtures before writing parser-local or crate-local unit tests unless the behavior is genuinely awkward to express as a fixture.
- When adding a new phase or module, add or extend a fixture suite for that phase before relying on ad hoc unit tests.
- Use the lightest fixture change that captures the failing shape.
- Keep fixture expectation changes deliberate when output changes.
- Prefer keeping typecheck coverage in fixtures unless the behavior is too low-level or too
  mechanical to express clearly in rendered fixture output.
