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

### A. The checker gives a WRONG answer, not a skipped one — two confirmed cases (both fixed)

`limitations.md` sells the whole trust story on one sentence: *"a gap means checks are **skipped**, not
that wrong answers are produced."* Both of these broke it, and both were silent under `strict = true`.
Both are now fixed. Shared lesson worth keeping: each one produced a type belonging to *neither* of the
program's possible states — a stale field type, and a merged signature — and in both cases that single
wrong type generated a false positive and a false negative simultaneously, which is why either could be
mistaken for a mere precision gap when read from one direction only.

1. **FIXED — a field write through a nominal value was discarded, and the stale belief was used both
   ways.** `replacement_written_type` handled `Record` and empty `Tuple` and let everything else fall
   through to `_ => prior`, so a nominal kept its declared field types after a contradicting write. The
   write is now **checked against the representation rather than applied to it**: a nominal's
   representation is fixed, so a value the declared field type refuses is reported at the value, a
   field the representation does not carry is reported by name, and the binding keeps its nominal type
   either way — which is what makes the `@type` invariant hold past construction. Both spellings (`$`
   and `[["literal"]]`) go through the one path; an opaque or non-record representation stays quiet
   (R only warns and coerces for `x$f <- v` on an atomic, so there is nothing to refuse). Structural
   records still retype, which is the contrast the report drew. Reference updated, seven fixtures.

2. **FIXED — a branch join involving a function slot invented a type belonging to neither path.**
   `join_writes_reporting` let a branch's scheme replace whatever preceded it, and a monotype replace a
   scheme, so a conditionally reassigned function kept **one** arm. `pick` above exported
   `fn(shout: logical) -> fn(x: T) -> character` — the parameter from the identity branch, the return
   from the `paste0` branch, a signature neither function has. Both directions followed: a
   character-expecting call on `pick(FALSE)(1L)` was **accepted** though R gives `integer`, and numeric
   use was rejected with ``found `character` ``, a claim about the value that was simply untrue.

   The join now unions the two entries' monotypes, and unions rather than unifying: `join_types` tries
   unification first, and two *instantiated* schemes always unify through their fresh variables
   (`fn(x: T) -> T` with `fn(x: U) -> character` by binding `T := character`), which is exactly how the
   fabricated signature arose. `pick` is now
   `<T, U> fn(shout: logical) -> fn(x: T) -> T | fn(x: U) -> character`, calling it returns
   `integer | character`, and a single reaching write still keeps let-polymorphism. The reference
   already specified this join correctly — the implementation had simply never matched it.

   **The original report's framing was wrong and is corrected here:** this was not "correct code
   rejected". R fails on the other path (`pick(TRUE)(1L) + 1L` is `non-numeric argument to binary
   operator`), so refusing the repro is right; what was broken was the fabricated type and the
   inaccurate message. The repro still reports, now as ``found `integer | character` ``.

### B. False positives on the first thing a newcomer writes

3. **FIXED — guard narrowing did not fire at file top level.** `recognize_guard` required the tested
   read to be in `naming.resolutions`, and a top-level variable read by a *later* statement is not:
   scopes are per-item, so it is a non-local. The guard bailed at the slot lookup, which is why the
   byte-identical guard inside a function body was clean — there, everything is one item.

   A non-local read now gets a checker-minted slot (ids allocated above every id naming issued, so
   they cannot collide), seeded with the type the read observed, and the ordinary environment carries
   the refinement from there — no second place to keep flow state. Null, negated and family guards all
   narrow at top level now.

   **One limit is inherent and is now documented rather than hidden:** a refinement does not outlive
   the statement it was made in, because checking is per top-level statement. So
   `if (is.null(x)) stop(...)` followed by a *separate* top-level statement does not narrow, while the
   same idiom inside a function body or one braced block does. Fixtured both ways. Making it cross
   statements would mean threading flow state between items, which `item_check`'s per-item memoization
   is built to avoid — not worth it for this, but that is the reason, not an oversight.

4. **A duplicate type name is unchecked in script projects.** Verified: two `@alias Thing` declarations
   in a `ry.toml` project produce no duplicate error; the second silently wins, so a `Thing` declared
   `double` on line 1 yields ``expected `character`, found `double` `` — unfalsifiable from the visible
   source. The same input **in a package reports it properly**, so the check exists and does not run for
   the other project kind, contradicting `diagnostic-codes.md` ("anywhere in the project") and
   `ry check --help` ("the directory holding `ry.toml` or `DESCRIPTION`").

5. **`= NULL` is exempt from the default-value check.** `#: fn([title]: character)` with
   `title = NULL` passes, while `title = 42L` is caught — and `NULL` then provably reaches a
   `character` parameter. This is the most common way R spells an optional argument. Related and worth
   stating in the docs: `[title]` relaxes only the *call*; inside the body `title` is an unqualified
   `character`, so the bracket looks like optionality and enforces none. `character | NULL` is the
   spelling that works.

### C. Annotations that validate themselves and then enforce nothing

6. **An annotation in call-argument position is parsed, name-resolved, and dropped.** Already filed as
   "silently dropped in a non-attachable position"; this round adds the part that makes it dangerous —
   **a typo inside such an annotation IS reported**, so the user gets positive feedback that it is live.
   With `@new` the drop resurfaces as a wrong error elsewhere in the file. Attachment was confirmed
   working at every other depth tried (block tail, `if` arm, `for` body, bare parentheses, binary
   operand), so argument position is the single hole.

7. **A `@param` naming a non-existent parameter cascades onto correct call sites.** One bad
   `@param missing_one` produced 5 errors: the primary says "**this** annotation names a parameter…"
   while underlining the *function definition*, and the invalid annotation's arity is then adopted, so
   four correct `f(1L)` calls are told they are missing an argument. Every caret in that output is on
   code the user must not change. `type-system.md` states the governing principle — *"a broken
   annotation never produces follow-on findings"* — for a list of shape violations that does not include
   this one; the rule should cover it.

### D. Placement: precise inside an expression, coarse at every compound boundary

Caret placement was found excellent for ordinary nesting (four-deep calls, multi-line arguments,
lambdas, pipes) and the renderer is display-width aware while JSON stays in codepoints — both correct,
which is rarer than it sounds. The failures are all "collapse to the outermost node":

- comparison operators (`<`, `==`, `>=`, `!=`) underline the whole binary expression, so the underlined
  text contains both operand types and the message cannot be read; arithmetic and unary `-` get it right
- a return-type mismatch with an `if`/`else` tail blames the whole construct, and the offending arm's
  line is never rendered
- `$` / `[[` underline the entire access chain rather than the bad key, even while the message names the
  key and suggests a correction
- surplus positional arguments blame the callee; a record mismatch is never attributed to the offending
  field, printing two ~340-character near-identical type dumps to diff by eye (nested is worse — the
  path is never named)
- out-of-range ranges: a parse error reported on line 10 of a 9-line file; annotation ranges ending at
  end-of-line spill onto the next line, so editors squiggle across the break

### E. Rendering drops information the message depends on

8. **Binder constraints are dropped from every rendered type**, so `lapply(words, function(s) s + 1L)`
   over a character list reports ``expected `fn(character) -> T`, found `fn(s: U) -> U` `` — which
   describes a call that *should* fit, and never mentions `character`, `+`, or numeric. The reference
   states the contract (`function(x) x + 1L` renders as `<T: numeric> fn(x: T) -> T`) and it is not met;
   the constraint is enforced but invisible, so an acceptable and an unacceptable function print
   identically. The good message already exists — annotating the lambda parameter produces a precise
   ``expected a numeric value…, found `character` `` — so the fix is to check the callback body against
   the instantiated parameter type rather than unify whole function types and print the residue. Same
   root cause behind the `Reduce` and `Filter` reports and the "expected `list[T] | T[]`, found
   `character[]`" message, which is false as printed.
9. **`(fn(A) -> B) | NULL` renders without its parentheses**, so it prints identically to
   `fn(A) -> (B | NULL)` — two different types. The docs name this exact form as the one that round-trips.
   Copying a type out of an error and back into an annotation silently changes it.
10. **A rank-2 annotation produces 7 diagnostics and the first one is false** ("only one compact
    annotation fits in a `#:` block" — there is one). The correct message, *"higher-rank polymorphism is
    not supported"*, is buried second and coded `syntax-error` though it is a deliberate expressiveness
    limit. Same misdiagnosis class: a braceless `@param count integer` reports a form clash that does not
    exist, and prescribes a fix that would add a fourth error.

### F. Smaller, but cheap

- `do.call` returns `Any`, which disables checking *and* blinds `strict` — a hole in exactly the
  higher-order code this checker is for. Corpus-authored `Any` where the docs say `Any` should appear
  only when a user writes it.
- `strict` only asks whether a binding *is* `Unknown`, not whether it *contains* one, so
  `fn(Unknown) -> integer` passes silently. Undercuts the "only way to keep a gap from looking like a
  pass" claim.
- `logical` is accepted at a declared `integer` parameter but rejected at an inferred `numeric` one, so
  the same function is accepted or rejected depending on whether the type was written down. R promotes
  logical in arithmetic and the docs say so twice.
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

## Open — a field write is lost across items in a package but not in a script

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
is clean in both kinds), so this is specifically the cross-item export: a statement item's
`top_level_bindings` carries the binding's pre-write type rather than the written one. Check that
against the conditional-slot model in the reference before changing it, since a *conditional* write
(`if (flag) record$age <- ...`) genuinely must join rather than replace — the unconditional case is
the one that should not.

## Open — diagnostic wording is not styled consistently

Six findings shown side by side in the README read as though five people wrote them: three
`type-mismatch` messages are lowercase sentence fragments with em dashes, `Unexpected comma after last
argument` and `Use TRUE, not T, for Boolean values` are capitalised (the second imperative), and
``` `tmp` is assigned but never used. ``` is the only one carrying a full stop. Caught by a README
reviewer who noticed the page claims a finding "says the same thing" everywhere while the sample
visibly disagrees with itself.

Pick one style, write it into the diagnostics reference as a rule, and sweep. Lowercase fragment with
no trailing period is the most common shape already and matches the surrounding tools' conventions;
the lint messages are the outliers.

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
- **An annotation in a position it cannot attach to is silently dropped.** A `#:` block attaches to
  the following *statement*, so one written beside a lambda inside a call argument
  (`lapply(xs, #: fn(character) -> character` / `function(v) v + 1L)`) binds to nothing — and a
  deliberately contradictory type there produces no diagnostic at all. The detached-block case one
  line above a statement is already a loud `annotation` error naming the fix, so the machinery and
  the wording both exist; this is the same error in a position the check does not reach. Note the
  residual gap after fixing it: reporting the silence does not give the lambda parameter an
  annotatable position, which is the one genuine expressiveness argument for inline type syntax
  (`contributing/design/inline-type-syntax.md` §3).
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

The rule is right in general and the fallback is load-bearing: `declares_arithmetic` lets a nominal
with an `Arith.`/`+.` method satisfy the numeric constraint, so forcing `x` numeric would reject a
user's S3 class that legitimately defines `abs.myclass`. Any fix has to keep that working, which is
why this is a design slice and not a tweak — the candidate shape is "if every non-fallback candidate
imposes the same constraint, imposing it is a fact rather than a guess", which needs a decision
record and adversarial review before it is written.

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

## Open — `htmltools` and `mgcv` spin on CPU at flat memory

Checking `htmltools`'s package directory (7,669 lines) runs past **five minutes** at 100% CPU and
**42 MB RSS**, measured at 182 seconds. Constant memory is the distinguishing fact: it rules out the
non-converging-fixpoint shape that made `rlang` fail, and that diagnosis held — fixing the fixpoint
took `rlang` from a 213-second death to a 9-second clean run and left `htmltools` timing out
unchanged, with no cycle panic in its output. So this is an algorithm that is superlinear or
non-terminating within a bounded working set. `mgcv` (37,253 lines) behaves the same way.

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
