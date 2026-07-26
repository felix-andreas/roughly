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

Covered in full by the typing reference under [object systems](/typing/reference#object-systems-s3-s4-r6).
In short:

| | Checked |
| --- | --- |
| S3 operator dispatch (`+.Date`, `Arith.Class`, `Ops.Class`) | **Yes** — statically resolved |
| A directly called S3 method (`print.myclass(x)`) | Yes, as an ordinary function |
| `UseMethod` dispatch | No — the call is `Unknown` |
| `structure(x, class = "dog")` | The value keeps `x`'s type — the class attribute is data, not a type — so its fields stay checkable |
| S4 (`setClass`, `new`, `x@slot`, `setMethod`) | No — `Unknown` throughout |
| R6 (`R6Class`, `$new`, fields, methods) | No — `Unknown` throughout |

The reference explains why the line falls there, and shows the one thing that *does* work today:
declaring the class yourself with `#: @type` and `#: @new` gives it a fully checked nominal,
constructor arity and all.

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

Four things narrow the hole, and the first is the big one. **Roughly ships the export lists of the
packages R code attaches most**: the standard library, the tidyverse (including `library(tidyverse)`
itself, which activates the nine packages it attaches), `data.table`, `testthat`, `knitr`, `rlang`,
`glue`, `magrittr`, `scales`, `jsonlite` and `R6`. Attaching any of those keeps the check fully on —
a real export resolves, a typo beside it is still reported with a suggestion. Beyond that list: a
near miss of a name your own project binds — a top-level definition, or a local or parameter in scope
— is reported anyway, because `library(shiny)` cannot explain `repositry` next to a `repository`
parameter; a `library()` naming **your own package** buys nothing at all, so the `library(yourpkg)`
in `tests/testthat.R` does not weaken anything; and writing a two-line `stubs/<pkg>.Rtypes` for a
package Roughly does not ship restores full checking there too, as well as silencing the
unknown-namespace warning. `strict = true`
also makes the tolerated reads visible.

A project's own `%op%` is deliberately left opaque: it may be an NSE wrapper whose right operand is
quoted rather than evaluated (magrittr's `%>%` is the canonical case), and checking that as an
ordinary call would reject correct code.

## Where Roughly runs

`roughly check` reads `.R` files and the R chunks of `.Rmd`, `.qmd` and `.Rnw` documents. The
**editor integration does not cover literate documents yet** — you get them in `check` and in CI,
not as you type. The formatter deliberately leaves them alone; most of an `.Rmd` is prose it has no
business rewriting.

`source()` calls are not followed. Package files under `R/` share one namespace, and so do the files
directly under `tests/testthat/` (testthat sources them into one environment, helpers first); every
other file is analysed on its own. See [where Roughly looks](/getting-started#where-roughly-looks).

## Turning it on for an existing project

Enabling `typing = true` on a large codebase that has never been type-checked will report findings
— possibly many. That is not a reason to avoid it, but it is a reason not to do it in one step:

1. Start with **no configuration at all**. Lints and unresolved names need no opt-in and find real
   mistakes on day one.
2. Add `[check] typing = true` and read what comes back before changing anything. Most of it will
   fall into a few shapes.
3. Turn the checker on **per file** while you work through them — a `# typing: on` comment at the
   top of a file enables it there regardless of the project setting, and `# typing: off` exempts a
   file you are not ready for.
4. Add `strict = true` last, on the files you care most about. It is a much stronger claim: it
   reports everything the checker could not determine, which is exactly what you want on a module
   you intend to rely on.

Known shapes you are likely to hit on real code, none of which mean your code is wrong: a value
that may be `NULL` used without a guard, an empty-list accumulator that later gains fields, and a
value the checker inferred as something other than a function being called.

## Maturity

Roughly is version `0.3.0-alpha`, is not on CRAN, and has one maintainer. The diagnostics, the
`roughly.toml` keys, the diagnostic codes and the JSON output are stable enough to build CI on —
they are covered by tests that fail when they change — but the type system is still gaining
capability, and a release may report findings an earlier one did not.
