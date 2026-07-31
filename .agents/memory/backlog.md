# Backlog

**Standing goal (user mandate): empty this list and keep the project at rust-analyzer quality.** The beta program that once organized it is complete; shipped work lives as one-line ledger entries at the bottom (rationale in `decisions.md`, contracts in the docs). Every open item sits in one of the sections below.

**Quality bar (acceptance):**
- **Sound on idiomatic R:** no known accepts-then-crashes holes on supported constructs; unsupported constructs may be refused loudly (sound-by-refusal is acceptable) but never silently mistyped.
- **Zero false positives on the ~200 most-used base functions** with `[check] typing = true` on idiomatic call forms.
- **Performance:** keystroke-to-diagnostics p50 ≤ 30 ms / p95 ≤ 100 ms at 300k LoC (read against the raw-parse floor the instrument prints — latency numbers swing ~1.4x with machine load); budgets pinned by `stats_witness` (per-line wall/memory/resolve-step ceilings) with the measurement instruments in `legacy/differential/tests/test_stats.rs`.
- **No server-killing input** (no `unwrap` panics on protocol-legal messages).

## Open — test-user round 3: typing enthusiasts (the type system, not the libraries)

Three simulated users probing **the type checker itself** rather than package coverage, from the docs
and `--help` only: one on parametric polymorphism and higher-order code, one on domain modelling and
nullability, one adversarial about the `#:` surface and where carets land. Each judged **location and
message as separate verdicts**, which is what makes this round different — a diagnostic that reads
perfectly while blaming the wrong expression counts as a failure here.

The four most serious claims were **re-verified independently** before filing; every one held, and two
turned out worse than reported.

Only the **open** findings are below; the closed ones, with what each measurement actually showed, are
in `test-user-reports.md`.

### D. Placement: precise inside an expression, coarse at every compound boundary

Caret placement was found excellent for ordinary nesting (four-deep calls, multi-line arguments,
lambdas, pipes) and the renderer is display-width aware while JSON stays in codepoints — both correct,
which is rarer than it sounds. The failures were all "collapse to the outermost node", and **four of
the five are fixed** under one rule now in the reference (§Where a finding points): a finding
underlines the smallest expression its message is about.

- **FIXED** — comparison operators (`<`, `==`, `>=`, `!=`) underlined the whole binary expression, so
  the underlined text contained both operand types and the message could not be read. They now blame
  the right operand, which is the `found` half of `expected …, found …`, and is where arithmetic
  already pointed. Worth recording: the *behaviour* is deliberate and correct — R coerces across
  atomic families (`10L < "9"` is TRUE, comparing `"10" < "9"` as text) and the reference documents
  the same-family rule as sound-by-refusal with that exact rationale, so only the caret was wrong.
- **FIXED** — a return-type mismatch with an `if`/`else` tail blamed the whole construct, and the
  offending arm's line was never rendered (the range clamps to the construct's first line). The
  declared return is now checked against each expression that can produce the result — a block to
  its tail, an `if`/`else` into both arms — and each failing one reports at its own site, the same
  rule an explicit `return` follows. The whole body's type stays the verdict, so no finding is added
  or dropped; when no single arm is at fault (an `if` with no `else` contributes an implicit `NULL`
  belonging to no expression) the construct keeps the one finding.
- **FIXED** — `$` / `[[` underlined the entire access chain rather than the bad key. Both now point
  at the key, including a position (`x[[5L]]`). `ExpressionKind::Field` gained a `name_range`, the
  same shape `CallArgument::name_range` already used for the same reason; `[[`'s key range came free
  from the argument expression.
- **FIXED (the surplus half)** — surplus positional arguments blamed the callee; they now blame the
  first argument with no formal left to take it, which is the one the reader must remove. A
  *missing* argument still blames the callee: there is no argument to point at.
- **FIXED** — a record mismatch printed two long near-identical type dumps to diff by eye, and a
  nested one never named the path. It now names the one field that failed, the same treatment the
  function-mismatch fix gave a signature: *expected `logical` for field `active`, found `character`*,
  and `retry.count` for a nested one. A field the value lacks, and one it has that is not declared,
  each say so; a **renamed** field is the interesting case, because it goes missing and turns up
  misspelled at once, so the finding names both (*expected a field `identifier` here, and this list
  has `idenifier` instead*) rather than reporting the absence alone. Pairs fields by name, which is
  what `compatible` does, so the explanation cannot disagree with the verdict — and optionality is
  deliberately not compared, because `compatible` does not either. Whole types are still printed when
  the failure is not about one field (a record against a non-record, or against `list[T]`), which is
  the case they do explain. Every site that reports two types side by side goes through one
  `Checker::mismatch`, so the narrowing cannot be present at one and missing at another; that also
  picked up the `@new` nominal path for free.

  **The caret is FIXED too, so the documented exception is gone.** A type carries no source ranges, so
  the field path is walked back against the expression that *built* the record — a `list(...)` call,
  whose tagged arguments are its fields. The value's `ExprId` now rides on `CallArgument` (and reaches
  `@new`, the declared-value checks and a parameter default), and the single `Checker::type_mismatch`
  funnel returns the whole `TypeError` rather than just a kind, so the range narrows with the message
  and cannot narrow at one site but not another. Which part gets the caret follows the message: the
  offending **value** for a field whose type does not fit, the field's **name** for one the type does
  not declare (that message is about the name), and the innermost list for a **missing** field, since
  nothing at the path exists to point at. Where the walk finds nothing — a variable holding a record —
  the whole value stays the blame and the message still names the field; pinned by a fixture.
- **FIXED (the spill half)** — annotation ranges ending at end-of-line spilled onto the next line, so
  editors squiggled across the break. Measured on a file of six deliberately broken `#:` regions:
  **four of eight findings** ran from the end of one line to column 1 of the next. Cause: an error
  reported *at* the current token blames that token, and at the end of a region the current token is
  the **newline**, whose span is exactly the break. A blame range now never crosses a line break — it
  collapses onto the last character of code on its own line (`Parser::on_one_line`, applied in
  `push_error` so the semantic range is right for the JSON, LSP and CLI paths alike, not patched in a
  renderer). Trailing whitespace is skipped so the caret lands on code: "expected a type" for
  `#: integer |` now points at the `|`, and "expected a return type after `->`" at the `>`. Twenty-nine
  fixture expectations moved by one character, every one of them off the newline and onto code. The
  rule is in the reference under §Where a finding points.

  **Still open (the other half)** — a parse error reported past the end of the file. The originally
  filed shape (line 10 of a 9-line file) did **not** reproduce: an unterminated `f <- function(x,` in a
  one-line file now reports at `1:14-1:15`, in range. Either an earlier fix covered it or the note was
  imprecise; needs a fresh repro before it can be worked, and should be re-derived rather than trusted.

### F. Smaller, but cheap

- **Both of the entries that used to sit here were misdiagnosed, and the measurements are the
  keepable part.** They read: "`do.call` returns `Any`, which disables checking *and* blinds `strict`
  — corpus-authored `Any` where the docs say `Any` should appear only when a user writes it", and
  "`strict` only asks whether a binding *is* `Unknown`, not whether it *contains* one". Checked
  against the tool:

  - **Changing `do.call`'s return from `Any` to `Unknown` produces no strict finding at all.** Tried
    it: edited the stub, rebuilt, ran a strict project over `do.call(fun, args)`. Nothing. So `Any`
    is not what blinds strict, and swapping it would have been a change with a false rationale in its
    commit message.
  - **Strict has no binding-level `Unknown` test to widen.** It reports *origins recorded at
    construct sites* (`UnsupportedConstruct`, `UndeterminedReference`, `LoopWidened`,
    `RecursiveUnknown`) — there is nothing that inspects a finished type, so "only asks whether it
    *is* `Unknown`" describes a mechanism that does not exist.
  - The gap the second entry was reaching for **is real**: `g <- f` closes to `g: fn(p: Unknown) -> Unknown`
    (pinned by the `aliased_function_reference_exports_closed` fixture) and strict reports nothing,
    so calls through `g` are unchecked and it looks like a pass.
  - **`Any` is not corpus-authored by accident, and the docs were the wrong half.** The shipped stubs
    declare **176** entries whose return is `Any` — across nine files, not just `base` — and
    `-> Unknown` in **zero**, with the stub header naming each compromise. The reference bullet
    claiming `Any` "should appear only because the user explicitly wrote it" was simply false, and is
    fixed.
  - The one behavioural difference between them, verified: `@if-unknown` coerces an `Unknown` and is
    **refused** on an `Any` ("this is already `Any` — drop the annotation").

  **What is actually open**, then: should strict report a binding whose exported type *contains*
  `Unknown`? That is a design question, not a bug fix, and it needs volume measured before it is
  chosen — strict already emits **2879 findings on dplyr and 2458 on shiny** with typing and strict
  forced on, and `erase_residual_vars` gives every aliased function `Unknown` parameters, so a
  containment sweep would fire on an idiom (`g <- f`) that ordinary R uses constantly. Decide it as a
  design note with numbers, not as a one-line widening.
- **NOT cheap, and the current refusal is the sound choice — measured, so do not "just widen the
  constraint".** Filed as: `logical` is accepted at a declared `integer` parameter but rejected at an
  inferred `numeric` one, so the same function is accepted or rejected depending on whether the type
  was written down. Both halves reproduce (`bump <- function(x) x + 1L; bump(TRUE)` is refused; the
  same body under `#: fn(n: integer) -> integer` accepts it), and R does promote — `TRUE + 1L` is
  `2L`, and the checker's own arithmetic rules say so.

  The obvious repair is to bind the numeric variable to `integer` when a `logical` argument arrives,
  which is exactly R's promotion. It is wrong, and the counter-example is ordinary R:

  ```r
  bump    <- function(x) x + 1L          # R: bump(TRUE) is 2L, integer
  checked <- function(x) { stopifnot(x + 1L > 0L); x }   # R: checked(TRUE) is TRUE, logical
  ```

  Both infer `<T: numeric> fn(x: T) -> …`, and one binding of `T` cannot be `integer` for the first
  and `logical` for the second — so the promotion produces a *wrong return type*, which the project
  ranks below a refusal ("a gap means checks are skipped, not that wrong answers are produced").
  Verified against R 4.3.3.

  Closing it properly needs the promotion to live in the type, not the binding — a scheme like
  `<T: numeric> fn(x: T) -> promote(T)`, i.e. a type-level function the language does not have. Same
  family as the next item, and they should be designed together.

- **A numeric variable shared by two parameters refuses ordinary mixed arithmetic** (found while
  measuring the item above). `add <- function(a, b) a + b` infers `<T: numeric> fn(a: T, b: T) -> T`,
  tying both operands to one variable, so `add(1L, 1.5)` reports ``expected `integer`, found
  `double` `` — R gives `2.5`. This is a false positive on about as plain a piece of R as exists, and
  it is the same missing piece: the operand types need a numeric *join* (`integer` with `double` is
  `double`), not unification.
- `@new` is unrestricted project-wide, so `domain-modeling.md`'s *"the only door in"* and
  `concepts.md`'s *"provably came from there"* overstate it. Either add an encapsulation modifier or
  soften both sentences to "by convention".
- A narrowing failure on a field or behind `&&` produces a message **byte-identical** to having written
  no guard at all. The docs know the fix ("lift the value into a local first"); the diagnostic should
  say it.
- Nominal unions are unchecked through `$` while structural unions are exact; and there is no
  discriminator for nominal types (`is.list` is true of both arms), so tagged unions cannot be narrowed
  at all.
- An alias cycle is reported on the *use* with a whole-statement caret and never at the declaration; an
  unused cyclic alias is not reported at all, though the reference says definition cycles are errors.
- A second `#:` annotation on one line is silently swallowed (first wins), while harmless trailing prose
  errors — the ambiguous input is the quiet one.
- Missing-argument errors do not name the parameter, though the sibling wrong-name error lists all of
  them. Near-miss suggestions have a length floor that misses short field names (`person$nam`).

**What the round says overall.** Both testers who could reach a verdict said the core is real — genuine
HM with generalization, an occurs check, per-parameter variance, working generic nominals, airtight
nominal distinctness, and no cascades outside the `@param` case. The gap is not the engine; it is that
**diagnostics render the artifact unification left behind rather than the fact that failed**, and that
the nominal story protects construction but nothing after it.

## FIXED — a field write was lost across items in a package but not in a script

Identical code, two answers. A structural record written at top level and read in a later top-level
statement:

```r
#: list{name: character, age: integer}
record <- list(name = "Ada", age = 36L)
record$age <- "now a character"
reading <- want_chr(record$age)      # wants character
```

As a **script** this is clean — the write retypes the field and the read sees `character`. In a
**package** it reports ``expected `character`, found `integer` ``, so the write is lost at the item
boundary and the read answers from the pre-write type. A package's `R/` files are sourced top-down,
so the write really does happen first and the package answer is the false positive.

Found while fixturing the nominal field-write fix, and pre-existing — the structural path is
untouched by it. The write applies *within* one item either way (the same code inside a function body
is clean in both kinds), so this is the cross-item export.

**Diagnosed, not yet fixed.** The export is fine: naming does mint a `TopLevel` binding for a
replacement target's base, so the writing statement item exports `record` with the written type in
its `top_level_bindings`. The fault is precedence in `SalsaGlobals::scheme`, which resolves in this
order:

1. `script_definition` — position-aware, handles definitions *and* statement writers, and is the
   reason scripts get this right. It is unreachable for a package, because `script_items` is only
   populated for scripts.
2. `package_definitions` — definition items only, no statement writers, no position.
3. `conditional_slot_scheme` — where the statement writers actually live.

A name with both a definition and later statement writes stops at (2), so every later write is
invisible. `conditional_slot_items` already collects the writer; nothing ever asks it.

The obvious repair — extend the position-aware same-file lookup to package files — has a constraint
that must not be broken: `package_definitions` encodes "later files, and later assignments within one
file, override earlier ones", so a same-file lookup that short-circuits would lose a later *file's*
override. The two orderings have to compose rather than one replacing the other. The conditional case
(`if (flag) record$age <- ...`) must still join rather than replace, which is what
`conditional_slot_scheme` unions for.

**Fixed by giving package files the position-aware lookup scripts already had.** `SalsaGlobals` built
its ordered item list for scripts only; it is now built for both kinds, because a file is sourced
top-down whichever it is. An **immediate** read consults the nearest earlier writer in its own file
before the project-wide map, which is what makes the rewrite visible.

The composition constraint is handled by splitting on the read kind rather than the document kind: a
**deferred** read stands aside for a package and falls through to the project-wide winner, because a
function body runs after the whole package is sourced, so a later file's override must win. In a
script the closure runs once that file's frame has settled, so it still scans the file. Verified:
forward references from a function body, mutual recursion, self-recursion, and a later file
overriding an earlier one all still resolve correctly.

## Open — user reports (from the maintainer, not a simulated round)

### REPL resize is not tracked

The width fix itself is closed and archived in `test-user-reports.md`. What stays open: a stock
terminal session handles `SIGWINCH` by calling `R_SetOptionWidth`, which R does not export, and the
console feed is the only other way in — the mechanism already shown to desynchronise the editor when
used between prompts. Worth revisiting if someone finds a third route.

### A variable is reported as unused — NEEDS A REPRO

Filed from a maintainer note that arrived truncated ("…variable is reported as unused"), so the
triggering shape is unknown. Do not guess at it: the `unused` warning has several paths (script
liveness, package top level, parameters, `shadows-namespace` interaction) and a fix aimed at the
wrong one is worse than none. Ask for the snippet before working it.

## Open — diagnostic wording is not styled consistently

Six findings shown side by side in the README read as though five people wrote them: three
`type-mismatch` messages are lowercase sentence fragments with em dashes, `Unexpected comma after last
argument` and `Use TRUE, not T, for Boolean values` are capitalised (the second imperative), and
``` `tmp` is assigned but never used. ``` is the only one carrying a full stop. Caught by a README
reviewer who noticed the page claims a finding "says the same thing" everywhere while the sample
visibly disagrees with itself.

**Measured before acting, and the proposed rule does not survive it.** Across 62 message literals in
`diagnostics.rs`: 36 lowercase with no period, 14 lowercase with a period, 9 capitalised. So
"lowercase fragment, no period" is the plurality but nothing like a convention — and it cannot be
applied blanket, because the capitalised ones are capitalised for reasons: `R's $ operator` is a
proper noun, and `I could not resolve …` / `I do not know the type …` / `I cannot construct an
infinite type` are first-person sentences. Lowercasing those yields `r's` and `i could not`.

The rule that actually fits the corpus is two-shape: a message that is a **complete sentence** is
sentence-cased and ends with a period; a message that is a **fragment** (the expected/found family)
is lowercase with none. Under that rule the real violations are far fewer than "five people wrote
them" suggests — mostly sentences missing their period.

**Unblocked** — the reporter change that re-captured every rendered sample has landed, so a message
reworded here only invalidates the samples that quote it. Whichever samples they are must be
re-captured in the same slice: there is still no harness for that (see the next item), so it is a
manual pass over the pages named in the docs-sample item below.

## Open — nothing verifies the rendered samples in the docs

Roughly two dozen blocks across `features`, `getting-started`, `reference/{cli,configuration,diagnostic-codes}`,
`type-checking/{tutorial,concepts,domain-modeling}`, `index.astro` and the README's generated
`.github/diagnostic.svg` show real tool output. Nothing checks them, so they drift silently and the
drift is only ever found by someone re-capturing by hand. Two things this proves rather than predicts:

- A reporter change left every one of them stale for days, and the cost of re-capturing them by hand
  is most of what made that change expensive to land.
- A **parser** change — narrowing an unterminated argument list's boundary — silently falsified the
  headline missing-comma sample in `features.md`, the very example chosen to show off the parser. It
  was not noticed until the sample was re-run for an unrelated reason, and `main` had shipped it
  wrong. This is the failure the "run the tool before claiming what it does" rule exists to prevent,
  and it does not scale to prose that was true when written.

**Shape of the fix.** Each sample needs its project (a `ry.toml`, one or more source files) and its
command recorded next to it, so a harness can rebuild the project, run the command, and diff the
captured block. Two candidate homes: a fixture-suite-style directory whose renderer output *is* the
docs block, or front-matter/HTML comments in the pages naming a fixture directory. Prefer whichever
lets the docs stay readable as prose — the samples are teaching material, not test data. The blocks
that excerpt one finding out of several need a way to say so (a first-N-findings or a filter-by-code
selector), because several pages legitimately do that.

Until it exists, treat "changed a message or a blame range" as implying a manual sweep of the pages
above, and say in the commit which ones were re-run.

## Open — release-artifact versions have drifted apart

`Cargo.toml` is `0.3.0-alpha`, `editors/code/package.json` is `0.3.0`, `editors/zed/extension.toml` is
`0.2.4-alpha`. The VS Code number may be deliberate — the Marketplace rejects a prerelease suffix — but
Zed's is simply stale, and nothing keeps the three in step. Decide whether one source of truth is
possible (a release script that stamps all three) or record why not.

Related and user-owned: the extension is published as `felix-andreas.roughly`, and
`felix-andreas.ry` is a 404, so every README and docs link that the rename swept to the new identifier
points at nothing until it is republished. The README now links the working identifier and says why.
Verified: `felix-andreas.roughly` returns 200, `felix-andreas.ry` returns 404.

## Open — test-user round 2 findings

Five simulated users, each on a distinct project of their own writing, learning the tool from the
docs alone (no source access) and trying to reach a clean run. Reports are the
`users/feedback-*.md` files from that round. **Recorded here before any of it is fixed**, per user
directive.

### From the Shiny dashboard user (571-line app, 15 planted mistakes, reached a clean run in ~40 min)

Their verdict: three real bugs found and welcomed, but "a clean run on a Shiny project means much
less than the docs imply" — they finished with exit code 0 and five real bugs still in the code.
Their own top three, in their order:

1. **One `library(pkg)` anywhere in the project disabled nearly all bare-name resolution, including
   typos of in-scope locals and parameters. FIXED.** Their repro: a file containing only
   `library(shiny)` made `repositry` — a one-letter typo of a **parameter in lexical scope on the same
   line** — stop being reported, along with every other bare unresolved name; three planted typos
   survived a `no problems` run on the real app. The blanket tolerance itself is right (an unstubbed
   package's export set is unknowable), so the fix narrowed it in two ways: the near-miss carve-out
   now covers locals and parameters of the enclosing item as well as top-level project symbols (a
   name one edit away from a binding of your own is a typo, not somebody else's export), and a
   `library()` naming the project itself buys no tolerance at all (the package author's #2). Both are
   now documented on the `unresolved` row of `diagnostics.md`, in the reference's resolution rules,
   and on the limitations page.
2. **The required/optional annotation mismatch FIXED.** Optionality now comes from the formals — a
   formal with a default is optional in R and no annotation can change that — so the exported
   signature takes it from the code and the annotation's disagreement is reported once at the
   definition, naming the fix (`write [currency]`). Callers of correct R are clean. The `[name]`
   bracket form is now in the guide as well as the reference.
3. **A parameter's type is not seeded from its default value** — `f <- function(s = settings) s$hsot`
   is missed while `settings$hsot` and a local alias are both caught. **Analysed, and the obvious fix
   is a false-positive generator, so it needs the design rather than a patch.** Binding the parameter
   to its default's type makes every caller passing anything else wrong, and reporting the field
   against the default's shape breaks the commonest idiom of all: `function(x = list()) x$name`, where
   the default is an empty accumulator and callers supply the real record — R answers `NULL` there, so
   an error would be plain wrong. The honest model is that such a parameter is
   `default's type | whatever callers pass`, which is still open, so a field access on it should be at
   most `T | NULL` (the same rule the accumulator fix established for unions) rather than an error.
   Recovering the user's case needs to know the argument is never supplied anywhere — whole-program
   information the checker does not have. Leave it missed rather than trade it for the class of false
   positive this round spent its time removing.

Also from them: `$` on an unannotated parameter constrains nothing, so the parameter accepts
anything; `unresolved` changes severity when `strict` is on (documented now, in the diagnostics table
and the reference); R6 is invisible (correctly
documented, cost recorded); and they asked for a page saying what does and does not work for Shiny.

### From the TDD package author (`tallyr`, 500 lines, green in real R 4.3.3)

Their package **passes in real R** (`pkgload::load_all` + `test_dir`), so every warning below is
provably a false positive. Clean run cost 5 edits on 500 lines; they would enable `typing` in CI
tomorrow and would **not** enable `strict`, `shadows-namespace` or `unused-parameter` — "three of
five opt-in lints are unusable on a package with an S3 class and a testthat suite".

1. **`structure(list(...), class = "x")` erasing the record type FIXED.** It yielded `Unknown`,
   against a documented promise of "a plain record; the class attribute is data", so the field typo
   the guide headlines was caught on a bare `list()` and **silently dropped** once `class =` was
   added — the objects real packages are built from. **31 of their 34 strict findings traced to this
   one call.** `structure()` now returns its first argument's type, so the record survives.
   Deliberately NOT changed: the class attribute still does not mint a nominal type — `@new` remains
   the only nominal introduction, and reading `class[1]` instead would type a
   `c("grouped_df", "tbl_df", "tbl", "data.frame")` value as `grouped_df` and then reject it at a
   `data.frame` parameter. See `contributing/design/inline-type-syntax.md` §3.
2. **`library(yourpkg)` in `tests/testthat.R` FIXED.** The project's own name — `DESCRIPTION`'s
   `Package` field, now carried on the metadata input — earns no tolerance: its export set is not
   unknowable, those exports being the project's own definitions. `usethis` generates that file, so
   this was switching off unresolved detection in every testthat package. The Shiny user's #1 (the
   near-miss carve-out reaching locals and parameters) is the other half and is also fixed.
3. **`tests/testthat/helper-*.R` FIXED.** Files directly under `tests/testthat/` now share one
   namespace, as testthat's own loading does (helpers first, then tests) — so a shared fixture is
   neither `unused` at its definition nor `unresolved` at its uses, while a real typo in a test still
   reports with the helper suggested. Documented in "where Roughly looks" and on the limitations
   page.
4. **`shadows-namespace` fires on locals inside `test_that()` and calls them "Top-level binding".**
   Diagnosed but deliberately not patched yet: naming is *right* that the binding lives in the file's
   frame, because R braces are not a scope and the checker cannot know `test_that` makes an
   environment. What the lint needs is **syntactic** nesting — "written as a direct statement of the
   file" — which the binding model does not currently distinguish from "belongs to the file's frame".
   Adding that must not disturb the unused rules, where `TopLevel` means package-visible; both shadow
   lints are default-off, so this waits for the model change rather than a special case. The wording
   is part of the bug: a local two levels deep should never be called a top-level binding.
5. **`unused-parameter` flagging user-defined S3 generics and their methods FIXED**, along with the
   default-on `unused` reporting an S3 method as dead (a separate finding from the first round, same
   missing knowledge). A project's own generics are now discovered by their bodies — a top-level
   definition whose read set contains `UseMethod` — across the whole package namespace, so a generic
   in `R/speak.R` covers `speak.dog` in `R/dog.R`; the generic itself is exempt too, since it declares
   the dispatch argument and never touches it. `is_s3_method_name`/`s3_generics` live in
   `semantics.rs` beside the other project-level projections, shared by the lint and the unused walk.
6. **A bad `importFrom` FIXED — it is an error now**, matching its `export()` sibling: R refuses to
   *load* such a package, so it is not survivable advice and must not pass a `--min-severity error`
   gate. A bad `pkg::name` *read* stays a warning, and the reference says why: a bad import stops
   loading outright, a bad qualified read fails only if that line runs.
7. **`#: @new` does not stop `structure()` being a strict origin** — the documented S3 remedy failing
   at exactly the site it exists for. Related to (1).
8. **S3 generic/method signature consistency is unchecked** — both planted violations missed, and
   `R CMD check` catches both. Roughly already does the rarer undefined-export check.

Docs gaps they hit: the adjacency rules say a plain `#` comment breaks `#:` attachment and never
mention `#'`, so interleaving `#: @param` with roxygen's `#' @param` — the first thing a roxygen
user tries — fails; the guide never shows `...` or `[optional]` in an annotation, and **every** S3
method needs both; `configuration.md` omits `DESCRIPTION` as a project root marker.

What they praised: the partial-match catch (`sourc =` → `source`, which R silently accepts and
nothing else in their toolchain catches), `NAMESPACE` being genuinely load-bearing with import-typo
and undefined-export validation at the right line, **`#:` verified invisible to `R CMD check`**, the
formatter and JSON output as production-ready, and the annotation-adjacency message for naming
roxygen2 and giving the fix. They also confirmed the `expect_error` decision was right and that the
"testing that something is rejected" subsection worked first try.

### From the Quarto/Rmd analyst (656 lines across two literate documents, 10 planted mistakes)

Their verdict: **"today, no"** — they would hit the ggplot2 errors within an hour and be fully off by
end of day, *never knowing the default checks were not running*. **"With #1, #2, #3 fixed,
permanently yes, including CI. That gap is three bugs, not a redesign."**

1. **`library()` of a common package killing unresolved-name checking project-wide FIXED** — the
   third and loudest report of the same hole. The fix is that **a manifest is enough**: a namespace
   with an `.exports` list has a knowable export set, so the blanket tolerance never applies to it,
   and no types are needed. Fifteen manifest-only CRAN namespaces now ship (`tibble`, `tidyr`,
   `readr`, `purrr`, `stringr`, `forcats`, `lubridate`, `magrittr`, `rlang`, `glue`, `scales`,
   `knitr`, `jsonlite`, `R6`, `tidyverse`), every name from them typing `Unknown` while typos beside
   them report with suggestions. `library(tidyverse)` additionally activates the nine packages it
   *attaches* (`stubs::META_PACKAGE_MEMBERS`) — a meta-package re-exports almost nothing, and
   activating members is also what hands such a project dplyr's and ggplot2's typed declarations
   instead of a manifest's `Unknown`. A package with no manifest (`janitor`) still earns the
   tolerance, correctly. Remaining: `janitor` and any other package a user names — the remedy is a
   two-line project stub, and the limitations page now says so on the user-facing side.
2. **ggplot2 `+` chains FIXED.** The corpus declared `+.ggplot` but nothing for a component pair,
   so `theme_minimal() + theme(...)` and a chain whose left operand was lost through `%>%` fell
   through to the numeric rules and reported arithmetic on a `gg`. R routes all of it through the
   single `+.gg` method, which the corpus now declares; a genuine mistake (`plot + 1L`) is still
   caught, naming both classes.
3. **Inline `` `r expr` `` FIXED.** The conversion recognizes inline spans as code: the delimiter and
   language tag blank to spaces, the expression keeps its bytes and its offset, and the closing
   backtick becomes the `;` that separates two inline expressions on one prose line. So a value a
   report only displays is used rather than unused, and a typo inside an inline expression reports at
   the right line and column. A plain Markdown code span (`` `total` ``), a span naming another
   language, `` `rate` `` (not an `r` tag), and an unclosed span all stay prose.
4. **`source()` is not followed, so a helpers file is pure noise in both directions** — all ten
   helper functions reported `unused`, and every *correct* call to them reported `unresolved`,
   indistinguishable from their planted typo. It also killed the wrong-arity catch, which works well
   within a file.
5. **Byte-offset columns and the misaligned caret FIXED.** A reported column now counts characters —
   in the rendered header, in `--output json`, and in the server's `file:line:column` hover strings —
   and the caret is padded by terminal cells, so it lands under the glyph even for double-width text.
   See the decision record; the JSON field documentation moved with it.
6. **5 of 10 planted bugs caught, and all 5 were cosmetic** (`=`, `T`/`F`, trailing comma). Missed:
   two column typos, two function-name typos, one wrong arity. Unknown *functions* inside `mutate`
   and `filter` are swallowed along with the column names.
7. **`fmt` on a target with no R in it FIXED** — it reports `0 files formatted` and exits 0, as
   `check` already did. A stage with nothing to do must not fail a pipeline, and a pre-commit hook
   handing the formatter a literate document it deliberately skips must not fail the commit.
8. An unclosed chunk reports their English prose as an unresolved variable rather than the missing
   fence. **The `strict` severity escalation is no longer silent** — the `unresolved` row of
   `diagnostics.md` and the strict-mode section of the reference both say that turning strict on
   raises every `unresolved` finding to an error without changing the count, and why that matters to a
   `--min-severity error` gate. The behaviour itself is right (a name the checker cannot see is a hole
   in the checked surface); being undocumented where a user looks was the bug.

What they praised, and it is worth recording because it was the part they expected to be broken:
**the literate handling itself is the strongest part of the tool.** Line numbers exact across all 545
lines of `.qmd`/`.Rmd`, ASCII columns exact, prose ignored, every chunk-header form parsed (including
`#| fig-cap: "A caption with = signs"` not fooling the `=` lint), `{python}`/`{sql}`/`{bash}`/`{ojs}`
and bare fences all correctly skipped, CRLF and Sweave and no-YAML all mapped correctly, suppression
comments working inside chunks. Also **zero false positives on 50 lines of idiomatic tidyverse** —
bare column names through the whole verb set, `case_when`, `across(where(is.numeric), ~ .x * 2)`,
tidyselect helpers, joins and `aes()` — which was their biggest fear going in.
`getting-started.mdx`'s "where Roughly looks" section is "the best thing in the docs". The guide
loses them where its showcase uses `list(...)`: swapping it for `read.csv()` makes the identical
mistake report nothing, and the page calls that "the whole pitch" with no caveat at the point of
contact.

### From the ETL engineer (919-line data.table/DBI pipeline, 12 planted mistakes, 8 caught)

Their verdict: `fmt --check` in CI today; `check --min-severity error` after two fixes; `typing =
true` as a merge gate **not yet** — "not because it's noisy, but because after #1, #2 and #10 a green
run doesn't yet mean what it needs to mean at 3am". Adoption cost: 34 warnings on first run, 30 of
them `unknown package namespace 'DBI'`, fixed by a **3-line `DESCRIPTION` with `Imports:`** that no
doc page mentions for this purpose — while the path the docs *do* recommend (hand-writing
`stubs/*.Rtypes`) cost five files, fixed less, and was actively harmful (see 3).

1. **Column-name typos inside `DT[...]` are invisible** — `orders[, net := gross_ammount * 1.19]` and
   `by = currrency` both silently accepted. Meanwhile the only name it *did* report was a correct one
   (see 5). Exactly inverted: silent where the typo is real, noisy where the name is right. A known
   gap, but `limitations.md` should say plainly that column names are never validated.
2. **`library(pkg)` for an unstubbed package disables `unresolved` project-wide** — the **fourth
   independent report**, and their version is the most damning: `library(totallyMadeUpPackage)`, a
   package that *does not exist*, buys the same blanket tolerance. Their "clean run was a mirage".
3. **CI BREAKER: setting `[check] exclude` disables the built-in `renv/`/`packrat/` skip.** No key →
   1 file; `exclude = []` → 1 file; `exclude = ["nothing-here/"]`, matching nothing → renv and
   packrat both walked. The docs' own example `exclude = ["scripts/"]` would drag an entire renv
   library into CI. The user-supplied list must *extend* the vendored defaults, not replace them.
4. **Any `stubs/` file deactivates the shipped conditional namespaces.** One stub for an unrelated
   package made 19 `data.table::` calls unresolved while bare `fread` still resolved, and made
   `data.table` unusable as an annotation type name — which silently suppressed a real type error.
5. **`setkey`/`setorder`/`unique(DT, by=)` FIXED.** `setkey` and `setorder` name columns rather than
   values, so they are `@masked` like the other NSE verbs (the `v`-suffixed forms take a character
   vector and never needed it). `unique`'s fallback candidate is variadic now, because `unique` is a
   generic whose methods take arguments base's signature does not name — the typed candidates stay
   exact, so `unique(c(1L, 2L))` is still `integer[]`. Verifying that turned up one more: every
   column-name parameter in the data.table stub was declared scalar `character`, so
   `setkeyv(DT, c("id", "date"))` — the whole point of the `v` forms — was an error. They are
   `character[]` now, which accepts one name or several.
6. **`data.table` is rejected where `data.frame` is expected, and the wrong direction is accepted.**
   `needs_df(data.table(a = 1L))` is legal R and reports `expected data.frame, found data.table`;
   `needs_dt(data.frame(a = 1L))`, the direction that really is wrong, reports nothing. This is
   **nominal subtyping** — R's class vector for a data.table is `c("data.table", "data.frame")` — and
   it is the same deferred design as S4's `contains=` and R6's `inherit=`: `TyKind::Named` matches by
   exact name and nothing in the compatibility relation carries a hierarchy. The tractable corner is a
   *declared*, acyclic nominal-extends-nominal relation in the stub vocabulary; do it as that design,
   not as a data.table special case.
7. **`on.exit()` reads FIXED.** R stores the expression and runs it at return, so it observes the
   *last* value of what it reads; a read inside it now keeps every write of that name in the frame
   alive, exactly as a closure capture does. A genuine dead store beside a guard still reports.
8. **Recursion defeats `is.logical()` narrowing** — the identical non-recursive function is clean.
9. **A maybe-`NULL` value is only caught by arithmetic.** `toupper`, `nchar` and `substr` on a
   `character | NULL` all pass, despite the guide promising this class of check.

What they praised: record-field inference from a plain `list()` with `Did you mean batch_size?`
("excellent and worth adopting for alone"); named-argument typo suggestions, arity checks,
argument-order errors ranged on the argument, branch-union arithmetic and non-function calls all
correct with "the best error wording I've seen in an R tool"; and **the data.table bracket really
works** — a 162-line transform module using `:=`, `.SD`, `.SDcols`, `.N`, `dcast` and a non-equi join
with `by = .EACHI` produced zero NSE complaints. Exit codes, JSON Lines, `--min-severity`, config
discovery, unknown-key warnings and suppression comments all matched the docs. 0.15s on 919 lines in
a debug build.

### From the time-series quant (620 lines, 10 planted mistakes, 3 caught)

Their headline: "the date/time operator modelling is the best static checking of R dates I have
seen — and it is surrounded by false positives that fire on ordinary code, plus a `Date` type that
evaporates the moment a date touches `c()`, `[`, `min()`, or a `for` loop." They would adopt lints
and `unresolved` today, and `typing` on one module "because the date operator work is worth real
money" — but not tell anyone with matrix-heavy code to turn typing on until (1) and (3) are fixed.

1. **Their soundness diagnosis was WRONG, and the real problem behind it is FIXED.** Verified:
   `min(Date)` is not typed `integer` — it selects the corpus's trailing `Any` candidate, so the check
   is *skipped*, not wrong, and the quality bar holds. What the finding exposed is that `Any` is
   exempt from strict mode, so the feature whose whole purpose is finding gaps missed the commonest
   one. An overload selection that commits an `Any` return now records a strict origin, so
   `min(d)` on a `Date` is reported under `strict = true`.
2. **CONFIRMED and genuinely unsound-adjacent: a `<T: numeric>` bound admits a `Date`, and the body's
   arithmetic then fails in R.** `#: <T: numeric> fn(a: T, b: T) -> T` called as `add_them(d1, d2)`
   is accepted, while the direct `d1 + d2` is correctly rejected with `` `+` is not defined between
   `Date` and `Date` `` — so a one-line helper launders a real error. This is the
   `declares_arithmetic` relaxation: a class that declares *any* arithmetic method satisfies
   `Numeric` wholesale, which does not follow — `Date` declares `+.Date` for `Date + integer` and has
   no `Date + Date`. **This is the traits / third-constraint-kind tripwire, now tripped a fifth
   time**, and it is the mechanism behind their missed "adding two dates via a helper" bug. A
   constraint meaning "supports this operator with these operands" replaces both this relaxation and
   the operand tie. Do not patch it again; design it.
   Also: `sort`/`head`/`rev` on dates report ``found `T[]` ``, leaking an unbound type variable.
3. **FP — `x[i, j]` on an unannotated parameter FIXED.** A subject whose shape the author never
   wrote down is now sound-by-refusal for *any* index shape, which is what the reference already
   promised; a subject whose shape *was* written down still refuses a shape no rule covers
   (`c(1L, 2L)[1L, 2L]` is an error).
4. **FP — `c()` on a classed value FIXED, and it does better than not erroring:** a class declaring
   a `c.Class` method keeps its class through concatenation (`c(d1, d2)` is a `Date`, and
   `dates + d2` is still refused as `Date + Date`), which is R's own dispatch rule. A nominal with no
   such method is indeterminate rather than an error.
5. **FP — `vapply(x, f, numeric(1))` errors whenever the callback is not statically `double`.** R
   accepts a wider template; they confirmed with `roughly run` that
   `vapply(1:3, function(i) i, numeric(1))` works.
6. **`Date` does not survive real code.** Survives `d + 1L`, `d2 - d1`, `d1 < d2`, `format`,
   `seq.Date` and `while` cursors; **lost to `Unknown`** through `dates[i]`, `dates[mask]`,
   `for (d in dates)` and `Reduce`; **wrong** through `min`/`max`; **an error** through `c()`. The
   same bug is caught in one place and missed in another — `cursor + cursor` after a `while` loop is
   flagged, the identical `d + d` inside `for (d in dates)` is not. No doc describes this boundary.
7. **`#: @type TradeDate {Date}` disables all Date arithmetic** — `settle - trade` is rejected — which
   kills the guide's own recipe for domain types over dates. The working route is a project
   `.Rtypes` declaring `-.SettleDate`, which the guide never mentions (and a stub cannot see an
   inline `@type`, the gap already recorded below). Also **`Date[]` is inexpressible**, and
   `Days + Days` yields `double`, so a units tag dies on arithmetic.
8. **Strict mode's own advice does not work.** The message says "add a type annotation" and **no
   annotation form silences it** — `#:`, `@if-unknown`, `@trust` and `#: Any` all still report, the
   last contradicting `reference.md` directly. Only `# roughly: allow(strict)` works.
9. **`&` and `|` are unmodelled**, so `TRUE & FALSE` is `Unknown` and every logical mask kills
   downstream checking. The reference documents `&&`/`||` with no counterpart, and the only mention
   of `&` is a parenthetical inside the NSE discussion.
10. **Formatter — `m[i, , drop = FALSE]` becomes `m[i,, drop = FALSE]`**, removing the space that makes
   the empty dimension slot visible, in exactly the code where miscounting slots is the bug. It is
   idempotent, so deliberate, and undocumented.
11. **Misleading message — any `X[Y]` annotation reports "only one compact annotation fits in a `#:`
    block — separate the annotations with a blank line" when there is only one annotation.**

What they praised: **every date expression R rejects or warns on was caught across 21 probes, with
zero misses and zero false alarms**, including `Sys.time() - Sys.Date()` — which R only *warns*
about and which they have shipped to production. `` `-` is not defined between `POSIXct` and
`Date` `` is "the best message in the tool". Also: field-typo detection with the full inferred record
shape, cross-file resolution with typo suggestions at zero config, arity messages in plain language,
syntax-error recovery, clean CI-ready JSON, 31.5k lines across 120 files in 2.2s on a debug build,
and `roughly run` with an embedded R letting them verify every claim ("underadvertised"). Units
nominals and domain matrix types both work and produce excellent messages.

## Open — documentation review findings

Four independent reviews read the docs cold (a first-hour newcomer, an information architect, an
accuracy auditor executing every claim against the binary, and a positioning analyst who also
measured the competition). The accuracy fixes landed; what remains is below.

**Bugs the reviews found, each with a repro:**

- **Numeric conditions FIXED** (`if (length(x))`, `while (n)`, `!length(x)` — R coerces them, zero
  false and anything else true; `character`/`complex`/`raw`/vector conditions stay errors). One
  residual: a condition whose type is still *undetermined* is bound to `logical`, so
  `function(n) while (n) ...` infers `n: logical` and rejects a numeric caller. Fixing that
  properly needs a "coercible to a condition" constraint — a fourth constraint kind, which is the
  documented tripwire for designing traits rather than accreting (see `contributing/design/open-questions.md`).
- **Named-before-positional matching FIXED.** `match_arguments` walked the argument list once, in
  source order, so a positional argument could take a formal that a *later* named argument was
  going to claim (`vapply(xs, character(1), FUN = f)` reported a bogus "FUN given twice").
  Matching is now two passes in `argument_targets` — names claim their formals, then positionals
  fill what is left — computed once and shared by the checking loop and the rest-parameter
  forwarding scan, which previously duplicated the accounting and so duplicated the bug.
- **The accumulator idiom FIXED, on the design the report proposed.** `$` on a union subject no
  longer demands the field on every member: a field some shapes carry and others do not reads as
  `T | NULL`, because that is what R answers for a name a list lacks. A field **no** shape carries
  stays an error — the typo check is the whole value — and a structural refusal (`$` on an atomic
  vector) is still hard from any member. The "did you mean" is drawn from every field the union can
  carry, so the suggestion survives a member with no fields at all.
- **Literate-document prose blanking FIXED, and it hid a worse bug.** A non-breaking space is
  whitespace to Rust's `char::is_whitespace` and an unexpected character to R's lexer, so prose
  containing one reported a syntax error against a blank line. The same code blanked per
  *character* rather than per *byte*, so any non-ASCII prose shifted every byte offset after it —
  every downstream range is a byte offset, so diagnostics in later chunks were silently
  misplaced. Both are fixed by blanking each character to its own `len_utf8()` in spaces; the unit
  test that was supposed to catch this asserted char count instead of byte length.
- **Self-checking ggplot2 reports 1132 findings and takes 2.4s** — the one package a stub ships
  for. The proposed cause (the shipped stub colliding with the package's own definitions) does
  **not** reproduce minimally: a package named `ggplot2` that defines `geom_point` and calls it
  checks clean, so project-wins-over-corpus works. The real cause is unidentified and needs the
  actual source (fetch the corpus); the likely candidates are ggplot2's own ggproto/R6 layer and
  its NSE, both known gaps, in which case the number is honest rather than a bug.
- **Duplicate type names go unreported in script files.** `reference.md` says the duplicate-`@type`
  error fires "regardless of file"; it fires in package files only. The value-name analogue is
  deliberately exempt for scripts, type names are not.

**Documentation that is still wrong (verified, not yet fixed):** `reference.md` documents a
rest-parameter spelling (`...items`) that never parses, and cites `contributing/design/open-questions.md`
— a published contract pointing at an unpublished file; `stdlib-stubs.md` names six symbols
(`BuiltinKind`, `parse_surface_type`, …) that exist only in the frozen legacy tree, and puts `...`
last in `paste` when the real declaration has it first (the position is load-bearing);
`architecture.md` still uses internal gate vocabulary; `development.md`'s re-bless command omits
`ROUGHLY_BLESS=1`; the `stub` diagnostic code and the SCREAMING_SNAKE naming exemption are emitted
but documented nowhere; five smaller `language-server.mdx` items (a `bun run package` with no root
`package.json`, three wrong VS Code palette titles, 4-of-5 code actions, `PAREN_EXPR` folding
omitted, a `--verbose` example whose own help says it is ignored).

**The structural problem.** The site was organised around Roughly's subsystems rather than around
anything a reader wants to *do*. Largely addressed: `typing/guide.md` is now a **tutorial** — eight
numbered steps from a zero-config first run to strict mode, each with a runnable example and output
captured from the binary, ending where the reference begins — and `stdlib-stubs.md` is no longer an
internal RFC. Also landed: the diagnostics reference (all fourteen codes, tables built by running
the tool), a CI workflow, and `limitations.md`.

**Comparison page DONE** (`comparison.md`): a capability table plus a "where you should use
something else" section that hands formatting to Air and rule breadth to Jarl and lintr outright,
and states the alpha/one-maintainer position. Every claim about another project was verified from
that project's own documentation, and no benchmark number appears that was not measured here —
second-hand numbers about competitors are the fastest way to lose the argument.

**Still missing:** a **"why a type checker for R"** explanation page (where the intrigue lives) and
a dedicated **adoption how-to** (the ladder is on the limitations page but deserves its own).

**Positioning (partly addressed — the comparison page landed and the primacy claim is now
defensible: "the only one that infers types" rather than "the first one", since two unmaintained
annotation-only attempts exist and a hostile reader finds them in one search).** The headline leads
with the formatter and linter — the two things Roughly loses at
today (Posit's Air is bundled in Positron; Jarl ships 71 rules with `--fix` and an LSP, 140× faster
than lintr) — and buries the one thing nobody else has. Measured facts the docs never state: 854
files / 166k lines in 3.6s, and lintr 38.2s vs 0.47s on dplyr. Research confirms **no one has ever
shipped a static type checker for R**: Vitek's group proved it viable (~80% of CRAN functions
monomorphic or nearly, 1.98% contract-failure rate) then pivoted to a JIT IR, the one Damas-Milner
attempt (`RTypeInference`) went dormant in 2021, and Posit's `ark` README states it plans
"sophisticated static analysis of R code" citing rust-analyzer while Positron ships a Rust type
checker for *Python*. Full reports and their paste-ready rewrites are the four
`docs-review-*.md` files produced by that round.

## Open — adoption review findings (unfixed items, each with a minimal repro)

Three independent black-box adoption reviews (an analysis-script user, a CRAN package author, a
numerical-computing user; docs + `--help` only, no source access) simulated real projects and
converged on the same walls. What they found and is now fixed is in the ledger; what remains is
below, ranked by how often a real user hits it.

- **Overload selection with a flexible argument FIXED** (fact-versus-guess probing — see
  `decisions.md`): a candidate that fits without narrowing the caller's open types beats one that
  does not, and a single fitting candidate is selected outright. The apply family is now unblocked
  but still untyped where its result shape is *value*-dependent: `sapply`/`mapply`/`Map`/`tapply`
  stay `Any` because `simplify = FALSE` and a vector-returning callback change the result shape
  without changing any argument type, so no overload set can discriminate them — typing only their
  *parameters* (leaving the return `Any`) is the reachable win, at the cost of rejecting R's
  function-name-as-string form (`sapply(x, "length")`, already rejected for `lapply`).
- **The three object systems, measured (S3 partial, S4 and R6 recognition-only).** S3 *operator*
  dispatch is real (`+.Date`, `Arith.X`, `Ops.X` method names are built and dispatched, and the
  linter knows `generic.class` names), but `UseMethod` is not modelled — a generic call is `Unknown`
  — and `structure(list(...), class = "dog")` produces a plain record, so the class attribute is
  data. S4 is untyped end to end: `setClass`/`setGeneric`/`setMethod`/`new` are `Any` stubs, `x@slot`
  has no type, a slot typo against a class declared two lines above is silent
  (`setClass("A", representation(x = "numeric")); new("A", y = 1)`), and — the one *active* false
  positive of the three — `setGeneric("f", ...)` does not define `f`, so every call to a project's
  own S4 generic reports `unresolved`. R6 has no stub at all (`R6::R6Class` reports `unknown package
  namespace R6`); method bodies resolve `self`/`private`/`super` as a special case, but the class,
  its fields and its methods are `Unknown`, so `obj$typo()` is silent and completion after `self$`
  offers every record field in the workspace. All three are recognized structurally by the IDE
  outline (`classify_symbol_call`), which is where the type-side work can start. Fix order by pain:
  the `setGeneric` false positive, then an R6 stub, then S4 slot types, then `UseMethod`.
- **Matrix SHAPE is still untracked.** `%*%`/`%o%`/`%x%` now return the `matrix` nominal and the class
  has its arithmetic and comparison methods, so matrix expressions type and compose — but
  `matrix`/`t`/`solve`/`dim`/`crossprod`/`diag`/`apply` still return `Any`, so a transposed dimension
  or a non-conformable product is invisible. Declaring them `-> matrix` is easy; the value is in
  *dimensions*, which needs a shape-carrying matrix type (see the data.frame row-type design). Note
  the trap this session hit: making a constructor return a real nominal without also declaring the
  class's operator methods turns every `m + 1` into a false error.
- **A project's own `%op%` stays untyped by design** (the result is `Unknown`, a strict-mode origin):
  it may be an NSE wrapper whose right operand is quoted, like magrittr's `%>%`, and checking that as
  an ordinary call would reject correct code. Lowering `%op%` to the call it is (the documented `|>`
  precedent) would type it and give goto/references on the operator — weigh that against the NSE risk
  and the `unresolved` a bare-script `%>%` would gain.
- **A list of functions FIXED**, and the cause was not the union: `function_compatible` demanded an
  *exact* parameter count, so `mean` (one required plus two optional) could not serve a
  one-argument callback interface, and the union of two such functions failed member-wise. Arity is
  a range now — a function serves an interface when it accepts every call shape the interface
  promises — so extra optional parameters are fine while requiring too many, or refusing an
  argument the interface sends, still fail. `lapply(list(mean, sd), function(g) g(1:3))` is
  `list[double]`. `lapply` keeps its input's names too (`list[named: T]` declared as its narrower
  first candidate), which is what forced the overload tiebreak to become plain first-match — see
  `decisions.md`.
- **Everything from a `data.frame` is `Unknown`, and `Unknown` satisfies every annotation** — so on
  data-frame-heavy code annotations look protective and are not. This is the design consequence that
  decides the tool's value for analysis users; it needs at minimum a way to *see* that a check was
  skipped (strict mode, once it reports origins).
- **An S3 method declared in R being reported `unused` FIXED** — the unused walk shares the
  method-name knowledge with the lints, and a project's own generics count, not just the corpus's.
  Dispatch is still not a read, so the exemption is by name shape (`generic.class` for a generic that
  exists), which is the same signal R itself uses to find the method.
- **Only the STUB route to an operator method on a project nominal is blocked.** Declaring the method
  as an annotated R function works (verified above), which is what an author writes anyway for a real
  S3 class; the gap is narrower than "operator methods need ergonomics" — a `.Rtypes` stub cannot see
  a `@type` the R source declares (`this declaration does not load: I do not know the type Meters`),
  so only the stub spelling is unreachable. Decide whether stub sources should see project `@type`
  declarations at all, or whether the R-side declaration is simply the answer and the docs should say
  so.
- **Smaller, each with a one-line repro in the reports:** messages leak unbound type variables (`list[T] | T[]`) and expand an alias on
  only one side of an expected/found pair; `unused` false-positives on a write followed by `break`;
  closure re-entry is unmodelled, so the `if (!is.null(cache)) return(cache); cache <<- v` memo idiom
  yields `T | NULL`; (a generic parameter rejecting a non-`NULL` default is CORRECT, not a gap — `<T> fn(x: T, [fallback]: T)` defaulting to `0L` would return `0L` from a call the signature promises returns `character`; `T | NULL` with a `NULL` default works because `NULL` is a declared member); the `unused` write-then-`break`
  false positive is NOT reproducible (verified across `for`/`while`/`repeat` and both used and genuinely
  dead writes) — drop it unless a concrete shape resurfaces; no `--fix`, no stdin, and no CLI way to ask "what type is this?" (which makes debugging an inference surprise guesswork for a
  CLI-only user). `sum(1, 2, 3,)` formatting to `sum(1, 2, 3, )` is NOT a defect and stays: the
  trailing comma introduces a missing argument, so it parses identically to the `alist(, )` idiom the
  space serves, and the `trailing-comma` lint reports the mistake.
- **Literate documents are analysed by `check` but not by the editor.** `.Rmd` / `.qmd` / `.Rnw`
  chunks are converted to an R program by blanking every non-R character (`syntax::literate`), so
  ranges need no translation and `check` reports at the original line and column. The LSP path still
  ignores them: `did_change` hands the engine incremental edits against the document the editor
  holds, and the converted text is a different buffer, so wiring it needs the original text kept
  alongside the analysed one (or the conversion applied per-edit). The formatter deliberately stays
  out — most of an `.Rmd` is prose.
- **An unannotated helper that wraps an operator over a class fails, and the tie is why.**
  `add_layer <- function(plot, layer) plot + layer` infers `<T: numeric> fn(plot: T, layer: T)` — the
  two flexible operands are tied to ONE variable — so `add_layer(base, geom_point())` reports
  `expected ggplot, found gg`. Same shape for dates: `add_days <- function(d, n) d + n`. Annotating
  the helper fixes it and the message is clear, but the tie is an over-commitment: R's `+` never
  required its operands to share a type, and a class that declares `+.Class` accepts pairings the tie
  forbids. **This is the "traits" / third-constraint-kind question in `contributing/design/open-questions.md`, now tripped a
  third time by shipped features** — the right fix is a "supports this operator" constraint instead of
  `Numeric`, replacing both the tie and the `declares_arithmetic` relaxation that lets an
  arithmetic-declaring class satisfy `Numeric` today. Next stub corpus addition that returns a real
  nominal will trip it again.
- **A type error inside `expect_error(...)` — DECIDED: it stays reported**, and
  `# roughly: allow(type-mismatch)` is the answer (decision record in `decisions.md`; documented on
  the diagnostics page). Suppressing inside expectation payloads would blind genuine mistakes in
  tests and needs an open-ended list of function families. The open follow-up is the stronger form:
  a suppression that reports when the expected finding does *not* appear, like
  `@ts-expect-error` — a feature for every code, not a special case.

## Open

 — semantics

- (Stub completeness audit CLOSED by the export-manifest layer — see the decision record and `stdlib-stubs.md` §Export manifests. `uname`-style reports remain user-project names: the fix stays a project stub or the DESCRIPTION-import tolerance.)

- **Legacy ide fixture port DONE** (fixtures directive, first half): 81 cases ported into `crates/ide/tests/ide/*_ported.R.test` (real legacy corpus: 134 cases / 206 operation sites; ~36 already covered; 15 skipped as genuinely multi-file — the harness is one `SourceFile` per case; deliberate improvements blessed). Cross-file navigation coverage now rests on the LSP tests — consider a multi-file fixture harness extension if that surface grows.

- (Design forks all DECIDED — two-flexible comparison stays unconstrained without a third constraint kind, union compatibility commits flexibles at first use in program order, NAMESPACE bare-resolution stays ungated; decisions.md has the three records.)
- **FIXED — an annotation in a call's argument list is now reported.** It attached to nothing and
  said nothing, so a deliberately wrong type beside a lambda argument was invisible. The cause:
  `statement_annotations` sees an `ARGUMENT` node next, which is not an expression kind, so the block
  classifies as dangling and never attaches — and the placement walk visits only statement sequences,
  never an argument list, so nothing reported it either.

  **The scope is narrower than this item claimed, and getting that wrong twice is the lesson.** The
  first implementation reported every annotation the placement walk did not reach, which is a false
  positive on four positions that do attach; it was built, measured, and reverted. Verified against a
  no-annotation control, because an uncontrolled probe read `lapply`'s own "not a function" error as
  evidence the annotation had applied:

  | position | parent node | attaches? |
  | --- | --- | --- |
  | braceless function body | `FUNCTION_DEF` | yes |
  | braceless `if` branch | `IF_EXPR` | yes |
  | parenthesised expression | `PAREN_EXPR` | yes |
  | call argument, any target | `ARGUMENT_LIST` | **no** |

  So the check keys on `ARGUMENT_LIST` alone. Two fixtures pin the reports and two pin the
  attaching positions, so a future widening has to break them first.

  Residual gap, unchanged: reporting the silence does not give the lambda parameter an annotatable
  position, which stays the one genuine expressiveness argument for inline type syntax
  (`contributing/design/inline-type-syntax.md` §3). The message says to lift the function to its own
  binding rather than "move it up a line", which would annotate the wrong thing.
- Overload candidates when touched: `is`, `extends`, `grep(value =)`, `cor` (vector vs matrix — needs matrix nominals). `Date`/`POSIXct` arithmetic refuses loudly today — revisit if real code makes it noisy.
- **A list operation over a RECORD still loses the field types.** `rev`/`unique`/`head`/`tail`/`Filter` now declare a `list[named: T]` candidate ahead of the plain list one, so a name survives and a field read is `T | NULL` instead of a missing-field error — but a fixed-shape input coerces to a name-keyed list on the way in, so the exact field types are gone and the read stays nullable. Only a shape-mirroring return ("the same record") fixes it, and the type language has no way for a stub to say that; `rev` is the case where the claim would be exactly right (it reorders and drops nothing), while `head`/`tail`/`Filter` genuinely may drop a name and are correctly nullable. Same family as the data.frame row-type and matrix-shape designs.

## Open — a package's own `pkg::name` reads are resolved but not validated

FIXED: the project's own package (from `DESCRIPTION`'s `Package` field) is now a known namespace
whatever the stubs say, so `withr::defer()` inside `withr` reads the project's own definition and has
its type instead of `Unknown`, and the `unknown package namespace` false positive is gone. The own
package also wins over a stub namespace of the same name, matching the rule that a package binding
shadows a stub name.

Still open: **the name itself is not checked**, so `withr::typoed_name()` reports nothing. Validating
it against the project's definitions was implemented, measured, and removed — across the CRAN corpus
every candidate report was a false positive, because a package's export set is not the set of names
its sources bind:

- a **re-export** — `shiny` has `importFrom(htmltools, validateCssUnit)` beside
  `export(validateCssUnit)`, so the name is exported with no definition in the package;
- an **S4 generic** from `setGeneric("raster", ...)` (`raster`), which S4 opacity already covers;
- a **lazy-loaded dataset** under `data/` — `survival::survexp.us` lives in `data/survexp.rda`;
- a binding installed by **`.onLoad`** (`cli`'s `symbol`);
- an **S3 generic re-exported from another package** (`broom`'s `glance`, from `generics`).

Closing this needs the package's real export set, which means reading `NAMESPACE` `export()` *and*
resolving re-exports, plus a decision about `exportPattern` (a regex over names, so it makes the set
unknowable and must fall back to silence). Worth doing — a typo in a self-qualified call is
otherwise invisible — but it is a namespace-model slice, not a one-line check.

## Closed — the overload corpus is NOT inflated by missing grammar (investigated; premise was false)

This was filed as "two absent stub-grammar features"; both features exist, and collapsing the corpus
buys one line. Recorded so it is not re-derived.

**The constrained binder already works**, in `#:` annotations and `.Rtypes` files alike. Measured:
`zzabs : <T: numeric> fn(x: T) -> T` in a project stub types `zzabs(1L)` as `integer`, `zzabs(2.5)`
as `double`, `zzabs(c(1L, 2L))` as `integer[]`, `zzabs(c(1.5, 2.5))` as `double[]`, and rejects
`zzabs("no")`. Vectors included — so the "shape-mirroring return" is not a separate missing feature
either; it falls out of the binder.

**The extra candidates are not redundancy, they carry facts a binder cannot state.** Three of them:

- **`logical` promotes to `integer`, it does not preserve.** R gives `abs(TRUE)` → `integer`, so a
  type-preserving binder would be wrong; the concrete `fn(x: integer) -> integer` candidate is what
  catches logical, because a concrete `integer` parameter accepts `logical` by coercion while a
  `numeric`-constrained *variable* refuses it. That asymmetry is deliberate and correct here.
- Sets whose int and logical arms are already unioned into one line (`cumsum`, `cummin`, `cummax`)
  gain nothing: the binder replaces a line that already covers two cases.
- `min`/`max`/`range`/`sort` carry a `character` candidate, which no numeric binder subsumes.

A trial collapse of `abs` from five candidates to four was behaviour-identical across seven probes
(scalar/vector × integer/double, logical, logical vector, and the wrapper) — and `abs` is the only
set with that scalar-and-vector-times-int-and-double shape. One line, in one function, is not worth a
corpus-wide edit, so it was reverted.

Wording trap, still worth keeping: never say these functions "have no principal scheme" — they do,
and now the declaration language *can* spell it. The reason for the sets is R's coercion table, not
the grammar.

## Open — a stdlib wrapper loses all type information (found while investigating the above)

`function(x) abs(x)` infers `fn(x: T) -> Any`, and the same holds for `sum`, `cumsum` and every other
set: wrapping a standard-library numeric function in one of your own throws the types away. The cause
is the fact-beats-guess rule — the `Any` fallback fits while binding nothing, so it beats every
candidate that would narrow `x`.

The rule is right in general and the fallback is load-bearing: a nominal with an `Arith.`/`+.` method
satisfies the numeric constraint, so forcing `x` numeric would reject a user's S3 class that
legitimately defines `abs.myclass`. Any fix has to keep that working, which is why this is a design
slice and not a tweak — the candidate shape is "if every non-fallback candidate imposes the same
constraint, imposing it is a fact rather than a guess", which needs a decision record and adversarial
review before it is written.

**Prerequisite FIXED, and it was a live false positive on its own.** The escape hatch above only half
worked: `declares_arithmetic` consulted the **stub library alone**, while operator dispatch resolves a
method through the global scope, which includes the project's own sources. So a package defining
`+.Money` had its `+` dispatched correctly and its class *refused* by the numeric constraint —
`bump <- function(x) x + 1L; bump(price)` reported ``expected a numeric value (`integer` or
`double`), found `Money` `` on code R runs fine (checked: R prints 6). The set is now computed from
stubs **and** the project (`GlobalEnv::arithmetic_classes`, memoized per corpus and per project;
a script's own top level counts too, the way its `@type` declarations already do) and carried on the
inference table beside `definitions`. A class that declares no arithmetic method is still refused —
R halts on that one too, checked both ways. Measured on the corpus: no finding changes across
data.table, dplyr, ggplot2 and shiny, so this loosening removed nothing that was load-bearing there;
the fixtures are the coverage.

Worth knowing for the design slice above: the escape hatch it depends on is only now actually
general. Any measurement of "how often would constraining `x` reject real code" taken before this
would have overstated the cost.

## Open — a misplaced config key was a silent no-op, and the class of bug is not closed

FIXED for the specific case: a key written at the top level that belongs under a table now names the
table (`ignoring config key `typing` — it belongs under `[check]``) instead of saying only that it is
unknown. Writing `typing = true` outside `[check]` had loaded clean, checked nothing, and reported
"no problems" — the same failure mode as an unresolvable namespace disabling unresolved-name
checking, and the one this project treats as the worst kind: a clean run indistinguishable from a run
that never happened.

Still open, and the reason this stays filed: **an ignored key is a warning, and a warning can be
missed.** A config file is small, hand-written and rarely revisited, so the cost of refusing outright
is low and the cost of proceeding is a project that silently is not checked. Consider making an
unknown key a hard error when the file is a *local* `ry.toml` while keeping the warning for forward
compatibility only where it is actually needed. Decide it deliberately; the current forward-compat
rationale (`config.rs`, `Config::unknown_keys`) is written down and is not obviously wrong.

## FIXED — strict mode now surfaces reads tolerated by an unknown attached package

The blanket tolerance was the one hole strict mode left open, and three docs pages had claimed the
opposite before saying plainly that it did not close it. With a `library(<package with no
manifest>)` in the project, a read of a name nothing defines produced **no finding at all** under
`[check] typing = true, strict = true`. That is the failure mode this project treats as worst: a
clean run that reads as "I understood everything" while an unknown `library()` silently switched a
whole class of checking off project-wide.

Closed by making the two streams share one decision instead of one of them guessing. The tolerance
was a `continue` buried in a 65-line loop in `unresolved_diagnostics`; that loop is now
`classify_non_local_read`, returning `Resolvable` / `Tolerated` / `Unresolved`. The ordinary check
reports the last, strict reports the middle — so strict reports **exactly** the reads the ordinary
check let through, and the two cannot drift. (Duplicating the tolerance rule into `strict_diagnostics`
was the obvious alternative and is the shape that caused the arithmetic-constraint bug: two places
deciding the same thing from different sources.)

Verified end to end rather than reasoned: strict on reports each tolerated read; strict **off** is
still silent, which is the whole point of the tolerance; a near miss of a name the project itself
binds was never tolerated and stays an `unresolved` finding; and the remedy the message names — a
`stubs/<pkg>.Rtypes` — actually closes it.

**This cannot be a fixture, and finding that out cost a blessed pair of them that tested nothing.**
The tolerance keys on `PackageMetadata`, a salsa input only a real project sets, so a single-file
fixture never triggers it — the two fixtures written first blessed as ordinary `unresolved` warnings
while their comments claimed to be testing strict. They were deleted; the test lives in the CLI
suite (`strict_reports_a_read_the_attached_package_tolerance_silenced`), which builds real projects,
and asserts all three behaviours above.

## FIXED (htmltools) / Open (mgcv, and it is a DIFFERENT cause) — spinning on CPU at flat memory

Checking `htmltools`'s package directory (7,669 lines) runs past **five minutes** at 100% CPU and
**42 MB RSS**, measured at 182 seconds. Constant memory is the distinguishing fact: it rules out the
non-converging-fixpoint shape that made `rlang` fail, and that diagnosis held — fixing the fixpoint
took `rlang` from a 213-second death to a 9-second clean run and left `htmltools` timing out
unchanged, with no cycle panic in its output. So this is an algorithm that is superlinear or
non-terminating within a bounded working set. `mgcv` (37,253 lines) behaves the same way.

### Localised and profiled; the fix is NOT where the time is spent

**One file does it.** Timing each `htmltools/R/*.R` alone: every file completes under 900 ms except
`tag_query.R` (1,563 lines), which alone exceeds 25 s. Start there, not with the package.

**The shape.** `tagQuery_` defines a local closure `newTagQuery(selected)` that returns
`structure(list(...))` whose ~40 fields are closures each calling `newTagQuery` again. So the record
type is *self-referential through its own fields*, and the type expands per level rather than being
folded. Prefix-bisection points at the line completing the first such method — but note prefix
bisection is confounded here, because a truncated prefix has a syntax error and syntax errors suppress
checking; the cliff is partly that the code became parseable.

**Where the time goes**, from a symbolised sample of the running process:
`semantics::types::substitute_rigid` recursing into `salsa::interned::…::intern`. Every instantiation
of a scheme mentioning that type walks and re-interns the whole thing.

**The time is NOT wasted work, which is the finding that matters.** Two candidate fixes in
`substitute_rigid` were implemented and measured, and both were reverted:

- returning `ty` unchanged when the substitution is empty (monomorphic instantiation);
- a memoised per-interned-type `rigid_names` set, returning any subtree whose rigids are disjoint from
  the substitution unchanged.

Neither moved `tag_query.R` at all, and an interleaved A/B on 323K lines showed **no** difference
(baseline 4.69–4.93 s, patched 4.69–4.75 s). So the substituted rigids genuinely pervade a genuinely
enormous type: the walk is doing real work on a type that should never have grown that large.

**Corrected by measurement: `substitute_rigid` WAS the hot path, and the earlier "no output" readings
were an instrumentation artifact.** A counter printing to stderr through a pipe loses its output when
`timeout` kills the process, so three separate probes read as silence and were taken as evidence the
function was cold. Redirecting stderr to a file instead showed **278 million calls in 40 seconds**.
Always redirect a probe to a file when the process under test will be killed, and always make the
probe fire once on entry so its silence can be distinguished from its absence.

**Fixed, and it is the DAG-as-tree bug.** Interning makes a type a DAG — one subtree is reached by
every path mentioning it — and a record whose ~40 fields all return that record is reached once per
field per level, so an unmemoised walk is exponential in depth. `substitute_rigid` now memoises per
node for the duration of one top-level call, which is sound because the substitution is fixed for that
call and a node's answer cannot depend on how it was reached. On the pathological file the walk drops
from 278M calls to under 2M; interleaved on both corpora it wins every round by ~2%, with identical
findings.

**The hang is still open, and the memo did not fix it** — a second bottleneck now dominates
`tag_query.R`. Re-sample the profile to find it; the sampler and the corrected probe method are the
tools to use.

### The type's growth is now measured, and it is combinatorial, not iterative

Instrumenting the captured-write join to report the **tree** size (paths, not distinct nodes) of each
type it erases, largest-so-far only:

```
877 -> 8823 -> 104655 -> 104657 -> 1046623 -> 5000000 (probe ceiling)
```

Roughly ten times per step. So the type genuinely explodes, every walk over it is a symptom, and
making individual walks cheaper cannot fix it — which matches the evidence, because each memoised
walk simply hands the hot spot to the next one.

**Six fixes have been implemented, measured, and reverted.** Recorded so none is tried a seventh time:

1. `substitute_rigid` early-out on an empty substitution — no effect.
2. `substitute_rigid` memoised disjoint-rigid skip — no effect.
3. `substitute_rigid` matching by reference instead of cloning `TyKind` — no effect.
4. `substitute_rigid` per-node memo — **kept** (278M calls to under 2M, ~2% on both corpora,
   identical findings) but does **not** fix the hang.
5. `erase_vars` as a tracked query — no effect on the hang, unmeasured on normal input, reverted.
6. Capping how many times one captured slot's join may grow — no effect, and the *reason* is the
   useful part: the cap is per slot, and each of the ~40 method slots grows only once or twice. The
   explosion is the **product across slots**, not iteration within one, so no per-slot counter can
   see it.

**FIXED, exactly there.** `type_size` is a tracked query counting a type **as a tree** — paths, not
distinct nodes — saturating at a ceiling of 100,000, and `Checker::record` gives any composite past
that ceiling `Unknown` instead. Tree size is the number that matters because a consumer walking a type
pays the tree it denotes, while sharing keeps the stored graph small; a distinct-node count is blind to
exactly the case this exists for. Only composites are measured, so scalars never pay for the ask.

Results: `tag_query.R` **>200 s → 56 ms**, the whole `htmltools` package **>5 min → 129 ms**. Findings
are byte-identical across 1,951 files of real CRAN sources (p18, MASS, ggplot2), and interleaved it is
~7% *faster* on 323k lines, winning every round — the ceiling cuts off moderately oversized types too.
`record` is the right site because every expression's inferred type passes through it; the capture-join
site, tried first, fires only nine times on that file and bounding it changed nothing.

**`mgcv` is NOT the same bug.** It still exceeds 200 s with this fix in place, so the shared-cause
assumption in this item's title was wrong. It needs its own investigation, starting from the
per-file timing sweep that localised `htmltools` to one file.

**Remaining from the original item.** The bound has to be on the size of a constructed type itself — Widening past the bound to `Unknown` is the sound-by-refusal move the loop join already
makes for a variable whose type keeps growing structurally. Computing the size cheaply needs care: the
DAG is small while the tree is enormous, so a distinct-node count will not see the problem and a naive
tree count is itself exponential — it wants a memoised size where `size(node) = 1 + sum(size(child))`,
which is O(DAG) and yields the true tree magnitude.
That is the recursion-widening question (fold to a recursive nominal, or widen to `Unknown` past a size
bound), which is a semantics design decision rather than an optimisation, and wants a decision record.
A size bound on constructed types would also be a general safety net: nothing currently caps how large
one type may get.

**Measurement trap, learned here the hard way.** Do not A/B two binaries in separate blocks on this
machine. A non-interleaved comparison showed baseline 9.6 s against patched 4.8 s — an apparent 2×
win that was entirely load drift from a build still finishing during the baseline block. Interleaving
the two binaries run-by-run showed the true difference: zero. Any perf claim here needs interleaved
runs.

Also still open from the same investigation: **`targets` 1.12.0 (63,979 lines) takes 43.7 s** where a
similarly sized ggplot2 takes 2.0 s — a ~20x outlier, R6-heavy code the obvious suspect, unprofiled.
A smaller sibling worth a look because it is cheap to profile: **`MASS` (5,951 lines) takes 6.5 s**,
roughly 900 lines/second where the corpus average is ~50,000.

**Missing end-to-end coverage for both cycle fixes.** Neither has a fixture. The failing inputs are
whole CRAN packages, and synthetic cases built from the suspected mechanisms did not reproduce
either one — for the non-convergence, a self-growing definition, three mutual-recursion shapes and an
overloaded-call cycle all converge fine, because a single item pins and settles and the bug needs
several members' pins to interact. What is pinned instead is the structural property the fix rests on
(`refusal_is_idempotent` in `semantics.rs`), which is the part that can be tested without
reproducing the cycle. The end-to-end guard rests on the corpus suites.

## Open — the formatter is slower than the type checker

Measured on a release build over 3,835 files / 703,289 lines / 30 MiB of real CRAN sources, in a
throttled container (so treat the absolute numbers as an upper bound, and see the parallel-cold-pass
note before drawing conclusions about scaling):

- `ry check .` with `typing = true` — 5.6 s, 5.8 s, 6.0 s
- `ry fmt --check .` — 10.1 s, 9.9 s

**Formatting costs roughly twice what parsing, naming, inference, type checking and lint assembly cost
together, which is backwards.** `fmt --check` should be the cheaper command: it parses and renders, but
it never builds an item tree, never resolves a name, and never runs inference. Worth an
`analysis-stats`-style phase breakdown before assuming a cause — plausible candidates are that
`--check` renders every file to a string and compares whole buffers rather than short-circuiting on the
first difference, that it is single-threaded where `check` fans out, or that the render path allocates
per node. Whichever it is, it is the kind of thing a user notices, because `fmt` is the command they
run most often.

## Open — editor & polish

- Hover type fences (user-confirmed: no highlighting in current editor builds): the server tags the fences `roughly-type` and the VS Code extension in-repo ships a grammar for that id — needs a released extension update to reach users. Zed renders the fence plain until its extension registers an equivalent fence language (tree-sitter grammar required); consider falling back to tagging fences `r` for Zed if that proves distant.

## Open — structure & performance

- **Diagnostics-phase remainder:** the duplicate-binding/duplicate-type O(files²) walk is killed (see the ledger); the post-burst workspace revalidate (~1.1s at 713K LoC, user measurement) should shrink too — every file's diagnostics used to depend on every file's ranges through those walks, so any edit re-executed all of them — but re-measure on the real workspace to confirm before closing.
- **The rewrite is complete and shipping** (decisions.md "target architecture" record): every phase gate holds — corpus/round-trip/acceptance/fuzzing, semantic parity via the differentials, cutover suites, perf + memory + keystroke budgets, order-independent fixpoint, multi-core stress. The legacy crates stay in-tree **by user directive** until the user asks for the final deletion sweep; when that comes, migrate the remaining fixture data out of the legacy trees and archive a final corpus parity report first.
- **Parallel cold pass: measure on real hardware before optimizing further.** The investigation (`crates/roughly/examples/parallel_probe.rs` is the reproduction tool) found: (a) the long-recorded "4 workers buy only 1.2x" was mostly a measurement artifact — this container's 4 vCPUs deliver only ~1.8x of lock-free native compute at 4 threads (~1.2x at 2), so no in-container parallel number is meaningful; (b) the one real structural serializer was `interface_sccs` demanding naming for every item inside one salsa query (25-51% of cold wall depending on package shape) — fixed by the CLI's parallel per-item naming warm before the fan-out (mgcv cold pass 0.99s → 0.80s even in the throttled container); (c) salsa's same-query blocking is negligible (53 blocks per 5,657 executions) and the interface DAG is wide (ggplot2: 1,198 items, depth 22), so no fixpoint-scheduling work is warranted until a real-hardware measurement says otherwise.
- **Coverage-guided fuzzing landed** (`fuzz/` crate, testing.md documents the workflow): libFuzzer targets `parse`, `format`, and `semantics` over the exported invariant batteries plus `scripts/seed-fuzz-corpus.rs` (a `cargo +nightly -Zscript` single-file script, like all of `scripts/`); the lint layer is folded into the semantics battery (everything-on config), closing the last unfuzzed stage. First sessions found and fixed nine formatter bugs and two splice-equivalence bugs (a middle ending mid-construct must refuse suffix reuse; an empty-suffix splice must not rebase the old end-of-file error) — all pinned in per-harness `REGRESSIONS` batteries. Remaining: a scheduled deep-fuzz run on real CI hardware, and an `llvm-cov` coverage report (recipe in testing.md; skipped in-container for disk).
- CI: the widened whole-workspace workflow is staged in `.github/pending-ci.yml` — a human must `git mv` it into `.github/workflows/` (automated tokens lack workflow scope). Until then CI gates only the product crate's own suites; the workspace battery runs locally per slice. Authoritative perf numbers need the CI runner.

## Open — website & docs

- (The landing-page hero animation is user-owned — do not touch.)
- **One-line installer.** Today the non-Rust route is download-a-tarball-from-Releases; there is no
  `curl … | sh`, Homebrew, or winget path (uv and Ruff both ship one). The installation page states
  this is planned with no date — if the plan changes, that claim has to change with it.
- **Every release is marked a pre-release**, so `releases/latest/` resolves to the old `0.1.1` tag
  rather than the newest build. The CI guide works around it by pinning an explicit tag; promoting a
  release would let the docs recommend `latest` instead.
- **Rework the typing reference's presentation** (`reference/type-system.md`, ~2700 lines): tables and
  short bullets instead of prose subsections, preserving every normative claim. Deliberately deferred
  out of the docs restructure so the contract got a dedicated pass.

## Open — REPL (v1 shipped; the analysis wiring is the open rung)

- **v1 SHIPPED and e2e-VERIFIED against real R** (`crates/repl` behind `roughly repl`; `contributing/design/repl.md` has the architecture, status, and the two pty-harness requirements): runtime-loaded R (no build-time link — the workspace builds R-less everywhere), reedline console inside the ReadConsole hook, lexer highlighting, conservative completeness with R's continuation as the safety net, SIGINT interrupt routing. The pty e2e suite (skip-if-no-R) runs green against real R — agent containers CAN install R (recipe in MEMORY.md short-term), so run `cargo test -p roughly --test test_repl_e2e` before REPL-touching changes, anywhere.
- **Analysis-backed Tab completion SHIPPED** (first analysis rung; `contributing/design/repl.md` has the seam design): typed signatures for stdlib names, session bindings, `pkg::` exports, manifest names — `SessionCompleter` seam keeps the repl crate syntax-only, `AnalysisCompleter` in roughly runs `ide::completion` over the session-as-script. **Open — remaining rungs:** live-session facts (the R environment listing unioned into completions), pre-evaluation diagnostics on pending input, hover on the input line, graphics-device story (versioned mirror structs, see the design record). The headless runner is shipped.
- **REPL Windows: real-machine smoke test pending.** The embedding is implemented (`contributing/design/repl.md` has the recipe: Rstart callbacks via R_DefParamsEx's version handshake, sibling-DLL preloading, RGui→LinkDLL switch, UserBreak+deferred interrupt pair) and compile/clippy-verified against x86_64-pc-windows-gnu — but no Windows machine with R has ever executed it. Smoke: `roughly repl` (prompt, evaluate, Ctrl-C, vi mode) and `roughly run` (output, exit 0/1). Known caveat to watch: terminal VT input handling in the editor layer.

## Open — delete the `rofy` crate (user-approved, unlike the other legacy crates)

**The user has given an explicit go for `rofy` alone, conditional on parity in every sense.** Every
other crate under `legacy/` still needs its own explicit go and must stay in-tree until then.

Why this one is different: `rofy` is the *predecessor* of a shipped component, not a frozen oracle.
It embeds R through `extendr`/libR-sys, so bindgen runs at build time against a local R and the
binary carries a load-time dependency on libR — which is exactly why it is excluded from CI and from
every gate. `crates/repl` replaced that approach with runtime symbol binding (`contributing/design/repl.md`), is
e2e-verified against real R on both Unix ptys and Windows ConPTY, and already exceeds it: headless
runner, vi keybindings, SIGINT routing, analysis-backed Tab completion. `rofy` is 266 lines across
`lib.rs` and `main.rs`, with no graphics-device code, so the parity surface is small.

**The payoff is not the deleted lines, it is the gate.** `--exclude rofy` currently rides along in
every command in CI (`.github/pending-ci.yml`), the docs, `MEMORY.md`, and every agent's muscle
memory. Deleting the crate reduces the canonical invocation to
`cargo test --workspace --exclude zed_roughly`, and one fewer exclusion is one fewer thing a future
session gets wrong.

Do this before deleting:

- **Establish parity explicitly rather than by assumption.** Enumerate what `rofy` does, confirm
  `crates/repl` does each, and record any deliberate non-goal. Two files is a short read; do the read
  instead of trusting this note.
- **Confirm nothing depends on it** — workspace members, `Cargo.toml` feature wiring, scripts, and
  the R-`parse()` acceptance cross-check, which `decisions.md` describes as "run locally like `rofy`"
  and which must keep working after the crate is gone.
- **Then drop `--exclude rofy` everywhere it appears** in the same change, and say so in
  `MEMORY.md` — the exclusion outliving the crate would be worse than either.

## Open — rename to `ry`: what is left

**Done.** The language is `ry`. The crate is `crates/ry`, published as `ry-lang` (plain `ry` is taken
on crates.io) with both the library and binary named `ry`. Docs, editors, scripts, CI and the README
carry the new name; documentation is pointed at ry-lang.org.

**Three surfaces keep their former spelling permanently**, because each lives where a rename breaks
silently: `roughly.toml` is still read (both names are checked in one directory before walking up, so
the new name wins a tie); `# roughly: allow(...)` still suppresses, which matters most because that
one lives inside users' source files; and every `RY_*` variable falls back to its `ROUGHLY_*` name.
The VS Code extension reads `ry.*` settings and falls back to `roughly.*`. The REPL history directory
is moved once rather than renamed, so nobody loses their history.

**Left for the user, because an agent should not decide them:**

- **The GitHub repository name.** Docs, badges and install commands now say `felix-andreas/ry`, so
  they are wrong until the repository is renamed. GitHub redirects the old URLs, so this is safe to do
  whenever — but it is currently the one inconsistency in the tree.
- **The VS Code Marketplace identifier.** `package.json` now says `felix-andreas.ry`; the Marketplace
  does **not** redirect an identifier, so publishing under it creates a new listing and starts
  installs and ratings from zero. Revert that one field if keeping the listing matters more.
- **Registering `ry-lang` on crates.io**, and `ry-lang.org`.
- **The landing-page hero animation** (`docs/src/pages/index.astro`) still spells out "Roughly" in
  particles — `ROUGHLY_LINES` is the ASCII art it draws. It is user-owned by standing instruction, so
  it was left untouched deliberately; it needs the new name from whoever owns it.

## Post-beta (explicitly out of scope for now)

- Tags / discriminated unions via a compiler-known stdlib `match` (design in `contributing/design/open-questions.md` first).
- S3 dispatch modeling (`UseMethod`) — prerequisite for honest `print`/`summary`/`plot`.
- data.frame column-level typing; matrix dimensionality; real S4 typing.
- Traits/typeclasses (tripwire: the third constraint kind).
- CRAN stub auto-generation via R introspection, R-version-keyed corpora, stubtest validation (R-dependent). (NAMESPACE/DESCRIPTION awareness moved to Open — semantics by user ask.)

## Shipped ledger (one line each; rationale in `decisions.md`, contracts in the docs site)

- **A non-converging cycle now terminates, because the refusal no longer depends on the round that produced it:** `item_check_recover` pinned only `scheme` at the round cap and took `ItemCheck`'s six other fields from the freshly recomputed value, so every round returned a different check, the recovery's own equality test could never succeed, and salsa iterated to its `MAX_ITERATIONS` of 200 — 184 rounds past the cap — then panicked with `too many cycle iterations`, or exhausted memory first, whichever the machine reached (the two symptoms were always one bug). Its sibling recoveries were safe only incidentally: `global_scheme_recover` and `statement_binding_recover` return a bare `TypeScheme`, so their pin is already a constant, and `item_check` is the only one of the three with a composite return. The pin now re-pins what was already returned, a fixed point by construction, and cuts both export surfaces rather than just one — `top_level_bindings` was leaking moving schemes out of an item that had already been declared non-converging, which its own doc comment said it did not. Verified on the real reproduction: rlang's whole package directory went from a 213-second death to a clean 9-second run reporting 806 findings, and the flattened 163-file variant that had been OOM-killed now finishes in 14 seconds. `refusal_is_idempotent` pins the property; `htmltools` still stalls with no cycle panic, confirming it is a genuinely separate pathology.

- **`ry check` no longer aborts on cyclic package interfaces:** `scc_schemes` was the one query in the interface-fixpoint chain with no salsa cycle recovery, while `item_check` and `global_scheme` — the queries either side of it — both had one. Its doc comment asserted that member checks run directly so "no salsa cycle forms", and the decision record reserved recovery "as a backstop for edges the static graph cannot see"; the backstop was never installed, so such an edge aborted the process. `interface_sccs` builds edges only from names appearing in an item's source, so a name the checker *constructs* is invisible to it and the group is not as maximal as the fixpoint assumes. Recovery pins the group to `Unknown` — the answer the round cap already gives — and refuses on first disagreement rather than iterating, because the group runs its own bounded fixpoint internally and letting salsa iterate it too multiplies those rounds into an out-of-memory kill (measured: the iterating version turned the panic into exit 137, which is worse than the bug). Verified on the real reproduction: rlang's `R/` went from exit 101 to a clean run with 838 findings.

- **A manifest is enough to keep unresolved detection alive, and fifteen CRAN namespaces now ship
  one:** attaching a package whose exports the checker cannot enumerate disables the check
  project-wide, which three independent adoption reviews reported as the single worst hole — a clean
  run was indistinguishable from "not checked". Manifest-only namespaces (no `.Rtypes`, every name
  `Unknown`) close it for the tidyverse, `knitr`, `rlang`, `glue`, `magrittr`, `scales`, `jsonlite`
  and `R6`; `library(tidyverse)` activates the nine members it attaches, so dplyr's and ggplot2's
  *typed* declarations come with it. The generator script now refuses to overwrite a manifest recorded
  against a newer R than the running session, because doing so drops the names that version added and
  turns each use into a false `unresolved`.

- **A bad `NAMESPACE` `importFrom` is an error, and the strict severity jump is documented:** R
  refuses to load a package whose import names a non-export, so a warning let it pass a
  `--min-severity error` gate — it now matches its `export()` sibling. A bad `pkg::name` read stays a
  warning (it fails only if the line runs), and the reference states the asymmetry. Separately, two
  reviews were surprised that `strict = true` raises every `unresolved` finding to an error; the
  behaviour is right, so the diagnostics table and the strict-mode section now say so.

- **Reported columns count characters, and the caret lands under the glyph:** byte columns disagreed
  with every editor on any line carrying non-ASCII text and pushed the caret right of the code it
  accused — sometimes past the end of the line. Columns are characters now (header, JSON, and the
  server's human-readable locations); caret padding is terminal cells, so double-width text aligns
  too. The `--output json` field documentation changed with it (see `decisions.md`).

- **S3 dispatch counts as a use, and a project's own generics are real generics:** the default-on
  `unused` lint called `speak.dog` dead and the opt-in `unused-parameter` called a generic's dispatch
  argument ignored — both false positives on working code, since dispatch is neither a read nor a call
  the checker sees. A generic is now any top-level definition whose read set contains `UseMethod`,
  unioned across the package namespace (a generic in one file covers a method in another), and both
  the generic and its `generic.class` methods are exempt. `is_s3_method_name`/`s3_generics` sit in
  `semantics.rs` with the other project-level projections rather than in the lint module, because two
  diagnostic layers share them.

- **A package's own `library()` call no longer silences unresolved names:** the unknowable-export-set
  tolerance skips the project's own name (`DESCRIPTION`'s `Package`, now on the metadata input), so the
  `library(yourpkg)` `usethis` writes into `tests/testthat.R` stops switching off unresolved detection
  package-wide. Two independent adoption reviews hit this.

- **Shape-preserving stubs actually preserve the shape:** `Filter` returned `list[T]` for every input, so `Filter(f, c(1, 2, 3)) + 1` — R selects with `[`, which keeps the atomic type — was a hard type error; it now declares the atomic, named-list and plain-list forms in that order. `rev`/`unique`/`head`/`tail` gained the named-list candidate too, so a name read off a reordered or sliced list is no longer a missing-field error (see the residual record in §Open).

- **Overload sets now read like the corpus is written, and `lapply` keeps its input's names:** the "all fits are guesses → last candidate wins" tiebreak is gone (a general `Any` fallback is already a *fact*, which was the only case it protected), so declaration order means first-match everywhere and a narrower candidate can be declared first — which is what let `lapply` gain `fn(x: list[named: T], ...) -> list[named: U]` without handing a value use of the name the narrow contract. The all-fail diagnostic no longer buries the answer behind "I tried all N signatures": when every candidate fails identically, or one candidate got strictly further into the argument list than the rest, that candidate's own finding is reported at its own argument's range. Signature help stopped requiring a commitment, so an incomplete call (`lapply(|)` — no candidate matches yet) lists the set instead of showing nothing, with the committed candidate rendered instantiated for the call site.

- **`roughly check` stack overflow fixed:** the check fan-out's scoped worker threads ran on default 2 MiB stacks while cross-file interface resolution recurses per dependency edge — a ~500-file definition chain aborted the whole command (`analysis-stats` survived only because the command thread carries `ANALYSIS_STACK_SIZE`). All three scoped spawns in `cli.rs` now use `ANALYSIS_STACK_SIZE`; verified on the 5,000-file synthetic tree (SIGABRT → clean exit-1 with findings).

- **Duplicate-diagnostics O(files²) walk killed:** `duplicate_binding_diagnostics`/`duplicate_type_diagnostics` rebuilt a project-wide occurrence map per file AND made every file's diagnostics depend on every file's ranges (any edit re-executed them all). Now: range-free per-file name projections (`top_level_binding_names`/`type_declaration_names`, value-equal under body edits) feed memoized project duplicate maps filtered to real duplicates (near-empty in healthy projects — the value-equality firewall), and per-file diagnostics fetch ranges only for files actually involved in a duplication. Same-tree A/B at ~530K LoC / 5,000 files: semantic render 12.1s → 3.7s, cold 22.8s → 14.4s, diagnostics byte-count identical.

- **Export manifests — every real R export resolves:** `types/<ns>.exports` (generated from live R by `scripts/export-manifests.R`) covers every namespace R ships in three tiers — default-attached (now incl. `datasets`, famous frames typed `data.frame`) bare-visible unconditionally; `QUALIFIED_ONLY_NAMESPACES` (tools/parallel/compiler/grid/splines/stats4/tcltk) always valid after `::` but bare only when attached/declared; conditional CRAN namespaces gated with their stubs. Manifest names resolve (typing `Unknown` without a typed declaration), feed completion and typo suggestions, and a unit test pins every `.Rtypes` value declaration as a real export of its namespace (caught `traceback`/`standardGeneric` misfiled — both are base exports). Kills the could-not-resolve false-positive class for the whole shipped standard library.

- **Diagnostics-phase whale killed (was 69-80% of the cold pass at scale):** `declared_global_variable` scanned every project file per non-local read — O(reads × files), ~375M memo lookups on a script-heavy workspace — and script-local cross-statement reads paid the project-wide guard chain before their own frame-slot resolution. Now one memoized project-level `globalVariables` union set (`project_global_variable_declarations`) plus cheapest-first guard order (masked → frame slots → package/stub → super-globals → declared globals → imports). Reproduced at 305K LoC / 2,500 files: diagnostics 22.7s → 2.9s, cold total 28.0s → 8.9s, diagnostic output byte-identical; `analysis-stats` permanently splits the phase into semantic render / lints / assembly — the instrument that found it.

- **Formal-aware `@masked` + the conditional dplyr namespace:** the stub loader records each masked verb's pre-`...` formal names (`StubLibrary::masked` map) and naming resolves arguments matching them (position or name) while masking what `...` absorbs — zero-formal masks (`join_by`) mask everything, restoring the reference's documented contract; `dplyr.Rtypes` ships as the second conditional namespace with class-preserving `@masked` verbs (`<T> fn(.data: T, ...) -> T`), class-preserving joins, and the tidy-select/verb vocabulary, so piped verb chains type end to end with masked column reads (`dplyr` fixture group in typing-imports; decision record has the collision/shadowing note).
- **The native pipe types as the call it is:** `hir::lower_pipe` desugars `x |> f(y)` to `f(x, y)` (R's own parse-time rewriting) with the `_` placeholder substituted as the one named argument R allows it to be (`pipe_shape`, syntax-level; the `_` token never lowers, nothing dangles); everything R rejects keeps the opaque-operator lowering. Naming/typing/overloads/arity/strict/IDE inherit the real call; piped-value errors blame the left-hand range; pipelines type end to end. Gate terms renegotiated via two `ACCEPTED_DIVERGENCES` entries (oracle never modeled pipes); the two strict cases that pinned pipe-as-origin repurposed to `%in%`; `%>%` deliberately stays opaque (a real function, not sugar). Reference documents the rule under Function calls → The native pipe.
- **data.table awareness (NSE ladder rung 1):** a conditional stub namespace (`crates/semantics/stubs/data.table.Rtypes` — the `@type data.table` nominal + ~45 declarations) joins stub assembly only when `metadata::namespace_active` (DESCRIPTION/NAMESPACE declaration, or a `library()`-family call found by the per-file `file_attached_namespaces` scan, unioned into `PackageMetadata.attached` by the hosts — server incrementally per synced file + idle prime); a bracket whose subject is the `data.table` nominal masks all index reads (`ItemCheck::masked_reads` skips the unresolved warning — kills the `DT[speed > 20]` false-positive class) and classifies its result by `j`'s syntax (no/empty `j`, `:=`, `.()`/`list()`, grouped `j` keep the subject's class; the rest refuse as Unknown with a strict origin). Typing-reference "Data-masked evaluation" + stdlib-stubs "Conditional namespaces" are the contracts; `datatable` fixture group in the differential-excluded typing-imports suite; decision record has the cycle/keystroke-cost analysis.
- **`analysis-stats` ported to the new stack (user ask):** `roughly debug analysis-stats [path]` (`crates/roughly/src/stats.rs`) assembles the workspace exactly as `check` does (stubs, metadata, Collate order) and reports staged cold-pass phase timings (load / parse / lower+naming / typecheck / render) with per-phase resident-set growth and peak, slowest-files-by-typecheck, and a typing-burst probe on the slowest + median + small files with `CHECK_EXECUTIONS`/`RESOLVE_CALLS` attribution and the raw re-parse latency floor. Forces typing on with a note; documented on the development docs page; CLI contract test pins the report sections.
- **Missing-comma parse recovery (user ask, rust-analyzer style):** when a token that can start a new element follows a complete argument or parameter, the parser reports `` missing `,` between these arguments``/``…parameters`` anchored at the empty range right after the previous element — on that element's line, not wherever the next element starts — and parses the next element normally (a proper `ARGUMENT`/`PARAMETER` node, no `ERROR` wrapping, so downstream analysis sees the intended list). Junk that cannot start an element keeps the old `expected `,` or …, found …` recovery. `starts_expression` mirrors `primary`'s entry set (kept in lockstep); golden error cases pin single-line, cross-line, and junk-recovery shapes.
- **`DESCRIPTION` `Collate` file order implemented:** `parse_description_collate` (`Collate`, falling back to `Collate.unix`); both hosts rank package files by Collate index before the path tiebreak (unlisted files order after the listed ones), the server reorders the project when a DESCRIPTION change moves the collation, and a CLI contract test pins the winner flip. Closes the reference's "Project file order" promise, which had no implementation.
- **NAMESPACE/DESCRIPTION metadata feeds resolution (user ask):** `semantics::metadata` owns the NAMESPACE parser (moved from the host crate) plus a DCF DESCRIPTION dependency parser and the singleton `PackageMetadata` input; `importFrom(pkg, name)` names and stub-described `import(pkg)` exports are known bare reads, a stub-less `import(pkg)` tolerates all otherwise-unresolved bare reads (export set unknowable — the zero-false-positive rule), and `pkg::` reads of declared-but-undescribed namespaces stay quiet. Hosts install the input next to the stubs (server refreshes on NAMESPACE buffer sync + NAMESPACE/DESCRIPTION watcher events, diffing parsed facts). New `typing-imports` fixture suite (metadata directives documented in testing.md; excluded from the differential arm — the oracle has no metadata concept); typing-reference "Package imports" section is the contract; decision record in decisions.md.
- **Blame-range precision (trailing trivia + parens):** the real-file corpus scan found two systematic same-finding range near-misses vs the oracle — expression ranges swallowing trailing whitespace/comments the Pratt loop consumed while peeking for the next operator (fixed: HIR lowering stores the trivia-trimmed significant range), and blame sites reporting a parenthesized wrapper instead of the expression inside (fixed: type-error blame drills through `Paren`; strict origins and missing-formal reads deliberately keep binding-site ranges). Both pinned by fixture cases verified to fail without the fixes; the scan's 39 paren + 8 trivia near-misses went to zero and file-level corpus matching rose 3,293 → 3,303/4,638.
- **`[` on vectors defined (was the largest real-code gap):** the typing reference specifies the full index-shape × subject-shape matrix (scalar-like numeric/character index → the scalar-claim element, with the negative-index caveat documented under the flexible-operand compromise; vector-like and logical-mask indexes → the subject's vector shape, names surviving; character indexes legal on any vector; undetermined indexes claim scalar and stay unconstrained; list/function/complex/raw indexes error) and `subset_result` implements it through a `vector_index_shape` classifier. Seven fixture cases pin every row; the oracle never defined vector `[`, so its refusals are an oracle-deficit class in the shared filter (1,415 accepted on the real-file corpus — the rewrite stopped reporting them on real code) plus per-case adjudications in the fixture and IDE arms. The NSE/data-masking design ladder is recorded as typing-design question 7.
- **Corpus-scale verification + dots-forwarding arity fix:** the real-file corpus arm reran after the cross-item-read and typed-NA changes — 3,322/4,638 files match (up 54 from the prior sweep; oracle-deficit acceptances rose ~975 because the rewrite now resolves reads the oracle cannot, e.g. R6 `self`/`private`). The sweep's one genuine false-positive class is fixed: a call argument that is the enclosing function's bare `...` forwards an unknown number of arguments, so such calls now skip both arity checks (`CallArgument::forwards_dots`; typing-reference Function calls documents the rule). The sweep harness also gained the documented-but-missing oracle-side panic guard (an oracle panic is counted and its file skipped instead of killing the run).
- **Callback-idiom stub sweep closed by audit:** the capped-stub premise no longer holds — every high-use base/stats/utils stub already declares its real optional formals (`nchar`'s type/allowNA/keepNA included), and the remaining single-parameter stubs are genuinely unary in R; a fixture pins both directions (`nchar(x, type = "bytes")` directly and `lapply(list(...), nchar)` through callback forwarding).
- **Overload/compatibility fixture sweep + reference fix:** ten typing cases pin overload-set rules (undetermined arguments use the general candidate, the catch-all corpus convention, value-use resolution, local shadowing disabling the set), numeric-variable generalization (`-x`, `x / 2`), and function-type compatibility (by-name parameter pairing, the rename refusal, parameter contravariance both directions, variadic-never-pairs-with-fixed); a CLI contract test reaches the no-matching-overload error through a fully-constrained project stub override (unreachable via shipped stubs — every set ends in an `Any` catch-all). Writing them exposed a reference/implementation contradiction on non-call uses of overloaded names: the implementation deliberately resolves to the last (most general) candidate with recorded rationale; the reference wrongly said first — the reference is fixed.
- **Indexing/guard fixture sweep:** thirteen typing cases pin the documented `[[`/`[`/`$` rules (vector element extraction, the map-like positional/name-based asymmetry, declaration-ordered record positions, backtick fields, record slices, unpinned-parameter tolerance) and the guard-narrowing rules (negated guards, `is.numeric`/`is.function` families, scalar+vector family membership, guards that cannot fire, expression guards never narrowing, the tested-then-unguarded finding) — all matched the contract on first bless. The per-position IDE differential over the new cases caught one real defect, fixed: `$` field completion now offers non-syntactic names in their backtick-quoted (insertable) spelling via the new `syntax::is_syntactic_name`; plus one adjudication (the rewrite hints a scheme for an unpinned-field reader the oracle leaves untyped).
- **Operator/constant fixture sweep + typed-NA fix:** eight typing cases pin the documented operator rules that had no fixture (`%%`/`%/%`, `^`/`**`, unary `-`/`!`, comparison families, `:` shapes incl. the scalar-numeric bound, `&&`/`||` scalar-only) and the reserved constants; writing them caught and fixed a real bug — the typed `NA_*` constants all lowered through the bare-`NA` catch-all and inferred `logical` (now `LiteralKind::Na(NaAtom)` carries the atom at lowering).
- **Per-item interface projections:** whole-project walks (`interface_sccs`, `conditional_slot_items`, `script_definition`'s statement branch) read `item_interface_reads` / `item_top_level_names` — small tracked projections of naming — instead of full `ItemNaming`, so a body edit that shifts ranges without changing a name backdates and no project-wide walk re-executes per keystroke (event-counted by `examples/keystroke_probe.rs`: walks went 8/8 edits → 0/8; misc.r 8.2→4.9ms, zxx.R 5.9→3.3ms medians in-container). New-item edits still re-run the walks, as they must. Architecture.md documents the projection firewall.
- **Shadow lints landed (default-off):** `shadows-builtin` (a top-level binding over a `base` export) and `shadows-namespace` (over another stub namespace's name, message names the shadowed symbol) in `lints::shadow_lints` — driven purely by the stub corpus's `exports_by_namespace`/`declaring_namespace` because bare resolution is ungated, so no NAMESPACE-import plumbing and no CLI/LSP drift; dotted S3 names are naturally exempt (not exports). Fixtures in the lints-style suite, CLI contract test, linter + configuration docs updated.
- **Top-level unwritten-path reads observe the cross-item binding:** a top-level slot's read-before-write (a loop's first iteration, a rebinding statement's right-hand side) resolves through `GlobalEnv::scheme(name, deferred=false)` — nearest earlier item in scripts, definition winner/conditional slot in packages — mirroring the unused check's cross-item-read rule; the observed type is materialized as the slot's pre-state (`pre_materialized`, re-established after each loop-pass rollback) so the loop join keeps it and first-iteration type errors survive to the reported stable pass. Self-referential-only names keep the tolerant `Unknown` (cycle recovery's initial), and sequential script rebindings now chain types (`n <- n + 0.5` after `n <- 1L` is `double`). Typing-reference "Definite assignment" documents the rule.
- **Missing-diagnostic probes closed:** generic-application arity errors at the applied name (`generic type `Box` expects 1 type argument, but found 2.`), non-generic-with-arguments, and bare-generic-must-be-applied (except under `@new`, whose representation check infers arguments) — vocabulary-side checks in the annotation-rule family over lowering-recorded `applied_references`, with mis-applied names flooring to `Unknown` in the relations so the one arity error never cascades. Probes confirmed missing-required-argument and duplicate-formal detection already existed; their wording now says what happened (`a required argument is missing`, `names the argument `x` more than once`).
- **Legacy-corpus parity closed; the corpus arm is a default gate:** all 1,523 comparable single-file case inputs from the frozen stack's own suites match through the shared differential policy (1 adjudicated acceptance: the oracle blames a vector-element violation at the alias declaration where the rewrite blames the use site). The sweep drove, slice by slice: the annotation block-form validation package (attachment/dangling rules incl. the blank-line association fix, directive ordering, duplicate/unknown type parameters, applied binders, `@new` shape + on-alias, vector-element atomicity, nesting caps, definitions top-level-only; refused blocks drop their payload), declared-function shape checks (optional-needs-default, rest-position, both variadic directions; renderer places `...` at its boundary), expression-level annotations (the constructor idiom — statement-level attachment at any depth through one application seam), alias-typed callees calling through their expansion, elided nested returns meaning `NULL`, frame-scoped capture liveness (`Scope::id`), conditional top-level slots exporting per-binding schemes (`ItemCheck::top_level_bindings`, `statement_binding_scheme` with cycle recovery, joined across writers), export-edge generalization of constrained residual variables (`close_scheme`: `<T: numeric>` survives, unconstrained erases), and `missing()` supplied-state flow (`EnvEntry::MissingFormal`: reading a no-default formal on the missing branch errors; writes supply it; the marker is branch-local). Plus the differential fuzz arm (six gaps) and the unknown-type-name class (the reported `Instument` bug) from the same assessment.

- **Error-message release pass:** the golden error suite grew 14 -> 78 cases organized by area, now covering every distinct lexer/parser message template plus recovery-locality and valid-stays-clean pinning (testing.md documents the coverage contract); writing it exposed and fixed a real cascade class — lexer `ERROR_TOKEN`s re-diagnosed by the parser (up to 4 reports for one mistake, now 1) via silent placeholder-atom consumption + statement-level suppression; the fuzz harness gained error-quality invariants (non-empty messages, in-bounds ranges, cascade bound linear in tokens). CI was red on the recorded newer-clippy trap (collapsible_match on 1.97) — fixed against CI's actual toolchain.
- **Docs accuracy pass + truthful landing examples:** development.md rewritten for the shipping crate layout, testing.md's legacy-era half compressed into a scoped frozen-stack section, linter/configuration/stdlib-stubs stale claims fixed (missing-comma retirement, stub path), shipped stubs migrated into `crates/semantics/stubs/` (the product no longer include_str!s from the legacy tree), the staged CI's rationale refreshed — and the landing page's formatter examples replaced after verifying each against the real binary: the old panels showed behavior the formatter doesn't have (bracing `if` one-liners, splitting one-line pipelines, aligning `=` columns); the new panels show verified auto-bracing of loops, operator/comma spacing, and multi-line indent normalization, with the tabs' reserved-dimensions fix confirmed already in place.
- **Formatter docs generation ported to the product:** `crates/format/tests/test_format_docs.rs` regenerates `docs/formatter.md` from `crates/format/tests/formatter.template.md` through the shipping formatter (every template example formats byte-identically to the legacy output); the legacy generator and its template copy are removed.
- **Recursion precision + strict attribution:** the canonical interface fixpoint types converging recursion precisely (top-level `fact`: `fn(n: integer) -> integer`; mutual groups generalize — the old tolerant-`Unknown` self-recursion contract is superseded in the typing reference), and strict mode now attributes the remainder: a cycle member with a clean body whose exported scheme still carries `Unknown` gets a binding-level `RecursiveUnknown` origin; pinned-at-cap cycles keep their read-site origins (decisions.md).
- **Corpus growth + wording polish:** the fetch manifest gained 28 large CRAN packages (Matrix/MASS/mgcv/survival/Hmisc/caret/sf/...) and the source-extension pattern widened to R's full set (`.R/.r/.S/.s/.q` — mgcv ships `.r`, Hmisc `.s`; the four corpus loaders match): corpus 507K -> 965K lines (81 packages, 30.5 MiB, 4640 files), all parsing with zero acceptance/round-trip divergences. At the new scale (per-process instrument protocol — batching the stats tests in one process pollutes the RSS numbers): new stack 19.0s (19.7µs/line) / 1.0 GiB resident vs legacy 30.3s / 2.0 GiB — 0.63x wall, 0.50x memory; parallel == sequential findings exactly (the canonical-fixpoint invariant holds at scale). The one new acceptance divergence was a real lexer bug (`..2dge` mislexed as `..2` + `dge` — R has no dot-dot token at lex time, so a longer name wins; fixed with a golden case). Diagnostic wording pass per the Rust/Elm bar: real pluralization for call-arity/named-argument errors, the `invalid semantics:` prefix dropped and the three `#:` block-form refusals rewritten to name the fix (separate blocks with a blank line), `NotAFunction`/`MixedListElements`/index-shape phrasing tightened.
- **Product-surface polish batch (new stack):** hover definition summaries ("Local variable/Package global, defined at `path:line:col`", stub origin namespace + overload count, maybe-undefined note), `debug = true` hover debug sections (Lowering/Naming/Parsing), S4/R6 document-symbol hierarchy (kinds + R6 member children, workspace symbols include members with real kinds, `fn(params)` details), a deterministic cancelled-pull LSP test (`ROUGHLY_TEST_DELAY_PULL_MS` fault-injection seam holds the pull until the edit's flip lands), **unused warnings on by default** (user directive; `[check] unused = false` opts out) with two script-unused fixes it forced: bare-statement reads (`print(x)`) keep bindings alive, and a definer inside an R-grammar error region never warns.

- **`roughly check` runs on the engine:** the CLI builds the same query graph the server uses (server `ProjectFiles` ordering, shared `assemble_engine_file_diagnostics` in `crates/roughly/src/diagnostics.rs`), so it inherits every engine performance property; `run_full` remains purely the differential oracle. Cluster repro check: 1.34s → 0.28s.
- **Per-definition interface-SCC rounds:** mutually-referencing file clusters (one giant SCC under file-granular edges — THE real-workspace whale, 95% of a 700K-LoC user cold pass) now re-infer per member definition for provably-decomposable files (`scc_definition_plan`), with change-driven skips at both granularities, per-file contribution merges (exact last-writer-wins), one snapshot/rollback inference state per fixed point, change-event oscillation history, and a dense borrow-only `SymbolScc` Tarjan. Cluster repro: cold typecheck 3.6s → 0.2s, member-file keystroke 359 → 34ms. `analysis-stats` now bursts a median and a small file besides the slowest.
- **Checker constant factors (~4× whole-file inference):** allocation-free free-variable walker (no more resolve-clone per recursion level), dense `EntryTable` vector for the union-find, Tarjan letrec-membership (was per-candidate transitive walks), precomputed winner-test lookups + batch exported bindings, FxHash for the never-iterated state maps; per-file interface edges (`WalkShadowed`/`FileInterfaceEdges`) projected per symbol (hub sweep 763→33ms).
- **One inference per file per revision:** `Typecheck` owns `FileInference { check, exports }`; `ExportedSchemes` is a shared-pointer projection (still the value-eq firewall); the could-not-resolve typo hint is memoized per symbol with an allocation-free corpus scan; the letrec candidate-edge scan is one arena pass; `Diagnostics` fetches tree-readers adjacently (one parse per file on cold prime). Cold 10.1s → 3.7s at 302K LoC, keystroke 5.4 → 1.7ms; `analysis-stats` splits lint / package-naming / render stages, and a `profiling` cargo profile (release + symbols) supports sampling.
- **Interface routing:** same-file backward references are walk-resolved, never interface-imported (kills the fake same-file SCC/Tarjan blowup — 100× on chain files; counter witness); scripts overlay their declarations on the memoized package type environment (was O(scripts × package files)).
- **Semantics core:** multi-member unions; mutable-slot model with union joins; `<<-`/`->`/replacement forms; coercion policy; name-aware signature matching; `Unknown`/`Any` tolerance in `c`/`for`/`$`/`[[`/`[`; flow-sensitive guard narrowing (+ divergence-aware joins, `missing()` supplied-state); `is.null` shaping of unconstrained variables (unannotated coalesce); elided annotation returns; `...` as positioned rest parameter end-to-end; variadic bridging into callbacks; `switch`/`return` as checked control flow; dispatch-table `[[` unions; computed-key container refinement; positional `[[` record extraction; S4 `@` slot lowering.
- **Stub unlock:** `T[]` constrained generics; overload sets (probe-committed, two-round selection, signature-help/hover display); opaque `@type` nominals (data.frame/factor/matrix/…); named-into-rest absorption (typed `read.csv`, `lm`); ~530-declaration corpus across 6 namespaces; project-stub namespaces (`pkg::name`); NAMESPACE import validation + `unused-import` lint; `library()`/`require()` NSE quoting; stub-error surfacing on every surface.
- **Trust & UX:** config subsystem rebuild (nearest-ancestor discovery, per-lint severity, config-file diagnostics, reload refresh); per-file typing directives `# typing: on|off|strict` (tri-state `TypingMode`, one gate everywhere); data-masked NSE resolution (data.table brackets, with-family); strict-mode product story; unified lint framework; CLI rendering + exit-code contract; `roughly debug analysis-stats` workspace performance diagnosis.
- **Editor:** hover quality (`name : TYPE`, overload notes, constraint display); annotation cursor features via re-lexing (hover/goto/completion in `#:` comments); insert-annotation code action (round-trips); unused fade-outs; formatter rewrite with `#:` block awareness; letrec naming (local recursive closures resolve).
- **Engine & scheduling:** red-green core with per-symbol interface firewalls, SCC fixed point, tombstones, eviction, stacker-grown validation spine; durability tiers (open docs LOW, corpus HIGH; sound downgrade re-min through cutoff nodes); memoized completion index + `NamesGlobal`-valued symbol index (zero-copy reads); two-wave diagnostics publish + idle-time semantic wave + lossless preemption pairing + background prime; error-tolerant lowering ("a broken region reports its syntax error and nothing else"); differential correctness vs from-scratch oracle over adversarial edit streams, byte-exact, IDE features included; committed latency witnesses (at-rest reads ≤ 32 memos, size-independent post-keystroke walk, blast-radius exec counters); memory shape at scale (rope-only corpus inputs + on-demand trees, single-retained modules, boxed annotations: 1 GiB → ~300 MiB at 302K LoC) and O(open) keystroke validation (fold split over the `OpenFiles` seam, FxHash memo table: 11K → ~280 slots/keystroke); `analysis-stats` reports per-phase memory, typing-burst recompute counts, and walk attribution.
- **Docs:** getting-started leads with a real bug; installation split out; typing guide + reference as contracts; architecture/structure/testing contributor pages; linter/configuration/stdlib-stubs pages current.
