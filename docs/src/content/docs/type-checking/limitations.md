---
title: Limitations
description: What Roughly cannot check yet, stated plainly — the gaps that matter before you adopt it
---

Roughly is a type checker for a language that was not designed to have one. Some of R resists
static analysis, some of it is simply not built yet, and you should know which is which before
you decide how much to trust a clean run.

One rule makes the rest of this page readable: **when the checker cannot determine a type, the
value becomes `Unknown`, and `Unknown` is compatible with everything.** That is what stops one
unmodelled construct from cascading into a screen of errors. It is also the shape of every gap
below — a gap means checks are *skipped*, not that wrong answers are produced.

Turning on strict mode makes those skips visible:

```toml
# roughly.toml
[check]
typing = true
strict = true
```

Strict mode reports every place a value became `Unknown`, and every call whose result the shipped
declarations could not describe — `min()` on a classed value, say, where the corpus ends the overload
set with a permissive fallback. It is the honest way to find out how much of a file is actually being
checked, and the only way to keep a gap from looking like a pass.

## Data frames

**Everything you take out of a `data.frame` is `Unknown`.** Columns have no types, so this is
accepted:

```r
#: fn(df: data.frame) -> integer
count_rows <- function(df) df$whatever
```

`df$whatever` is `Unknown`, `Unknown` satisfies `integer`, and the annotation reports nothing —
even though the column may not exist and would not be an integer if it did.

This is the gap that most affects analysis code, because the data frame is where analysis code
lives. Typed column vocabularies need a row-type design that does not exist yet. Until it does,
annotations on data-frame-heavy code look protective and are not, and `strict = true` is the only
way to see it.

Matrices are the same story one level down: `%*%`, `%o%` and `%x%` produce a `matrix`, and matrix
arithmetic and comparison are checked, but **dimensions are not tracked**. A non-conformable
product or a transposed dimension is invisible.

## R's object systems

**S3 is largely fine; S4 and R6 are opaque.** Operator dispatch (`+.Date`, `Ops.Class`) resolves
statically, a directly called method is just a function, and `structure(x, class = "dog")` keeps `x`'s
type because a class attribute is data rather than a type. `UseMethod` dispatch, and everything in S4
and R6, is `Unknown`.

The full table is in the reference under
[object systems](/reference/type-system#object-systems-s3-s4-r6), which also explains why the line
falls there. The thing that *does* work today is declaring the class yourself — see
[domain modeling](/type-checking/domain-modeling).

## Non-standard evaluation

`dplyr`, `data.table`, `ggplot2` and `testthat` have shipped stubs, and their data-masked verbs are
understood: bare column names inside `mutate()` or a `data.table` bracket are column references,
not unresolved variables, and classes flow through a pipeline. A full `read.csv |> mutate |> filter`
chain checks clean.

Outside those, a data-masking function Roughly does not know about will report its column names as
unresolved. The ecosystem's standard escape hatch works — a top-level
`utils::globalVariables(c("a", "b"))` silences them for the whole package.

**Attaching a package Roughly does not know weakens the `unresolved` check.** A `library(pkg)` whose
exports Roughly cannot enumerate means any bare name *could* be one of them, so otherwise-unresolved
bare names are tolerated rather than reported — project-wide, not just in that file. This is what
keeps the tool usable on real code, but where it applies a clean run says less than it looks like it
does.

Four things narrow the hole:

1. **Most packages are already known.** Roughly ships the export lists of the packages R code attaches
   most — see [what ships](/type-checking/stubs#what-ships). Attaching any of them keeps the check
   fully on: a real export resolves, and a typo beside it is still reported with a suggestion.
2. **A near miss of a name in scope is still reported.** `library(shiny)` cannot explain `repositry`
   sitting next to a `repository` parameter, so that stays a finding. In a package this covers your
   top-level definitions too; in a loose script it covers locals and parameters.
3. **`library(yourpkg)` costs nothing.** A `library()` naming your own package weakens nothing, so the
   one in `tests/testthat.R` is harmless.
4. **You can close it yourself** with a two-line [`stubs/<pkg>.Rtypes`](/type-checking/stubs), which
   also silences the unknown-namespace warning.

And `strict = true` makes every tolerated read visible either way.

A project's own `%op%` is deliberately left opaque: it may be an NSE wrapper whose right operand is
quoted rather than evaluated (magrittr's `%>%` is the canonical case), and checking that as an
ordinary call would reject correct code.

## One signature per function

A `#:` annotation declares exactly one signature, and there is no way to give your own function
several. The standard-library declarations do have them — `sum` is `integer` in and `integer` out but
`double` in and `double` out — and that is a deliberate asymmetry rather than something to be
extended later.

A name with several signatures has no single most general type, so a call to it has to be resolved by
trying candidates instead of inferred outright. That costs the guarantee that makes this checker fast
and its answers stable. It is worth paying for a small curated corpus that has to describe a standard
library nobody designed with types in mind, and it is not worth paying across your codebase. Where
you would reach for an overload, a [union](/reference/type-system#union-types) parameter usually says the
same thing — `fn(x: integer | character) -> character` — and two functions with distinct names always
do.

## Scope of analysis

`source()` calls are not followed. Package files under `R/` share one namespace; every other file is
analysed on its own. See [project discovery](/reference/configuration#project-discovery) for the rules.

Files directly under `tests/testthat/` are an **approximation**: Roughly analyses them as one shared
environment, helpers first. Real testthat only shares `helper*.R` and `setup*.R` that way — each
`test-*.R` runs in its own child environment. So a name one test file defines and another uses will
resolve here and fail when you actually run the tests. The approximation is deliberate (it keeps
helper-defined names resolving), but it is more permissive than testthat is.

---

None of the above is a reason not to adopt it — see
[adopting an existing codebase](/guides/adopting) for how to turn it on a piece at a time, and
[project status](/why-roughly#project-status) for how far along the whole thing is.
