---
title: Limitations
description: What ry cannot check yet — the gaps that matter before you adopt it
---

ry is a type checker for a language that was not designed to have one. Some of R resists static
analysis, some of it is simply not built yet, and the difference matters when you decide how much to
trust a clean run.

One rule makes the rest of this page readable: **when the checker cannot determine a type, the value
becomes `Unknown`, and `Unknown` is compatible with everything.** That is what stops one unmodelled
construct from cascading into a screen of errors. It is also the shape of every gap below: a gap
means checks are *skipped*, not that wrong answers are produced.

Strict mode makes most of those skips visible:

```toml
# ry.toml
[check]
typing = true
strict = true
```

It reports every place a value became `Unknown`, and every call whose result the shipped
declarations could not describe — `min()` on a classed value, say, where the corpus ends the
overload set with a permissive fallback. It is how you find out how much of a file is actually being
checked, and it keeps a gap from looking like a pass — including the attached-package tolerance
described [below](#non-standard-evaluation), which it reports as an undetermined read rather than
leaving silent.

## Data frames

**Everything you take out of a `data.frame` is `Unknown`.** Columns have no types, so this is
accepted:

```r
#: fn(df: data.frame) -> integer
count_rows <- function(df) df$whatever
```

`df$whatever` is `Unknown`, `Unknown` satisfies `integer`, and the annotation reports nothing — even
though the column may not exist and would not be an integer if it did.

This is the gap that most affects analysis code, because the data frame is where analysis code
lives. Typed column vocabularies need a row-type design that does not exist yet. Until it does,
annotations on data-frame-heavy code look protective and are not, and `strict = true` is the only
way to see it.

Matrices are the same story one level down: `%*%`, `%o%` and `%x%` produce a `matrix`, and matrix
arithmetic and comparison are checked, but **dimensions are not tracked**. A non-conformable product
or a transposed dimension is invisible.

## R's object systems

**S3 is largely covered; S4 and R6 are opaque.** Operator dispatch (`+.Date`, `Ops.Class`) resolves
statically, a directly called method is just a function, and `structure(x, class = "dog")` keeps
`x`'s type because a class attribute is data rather than a type. `UseMethod` dispatch, and
everything in S4 and R6, is `Unknown`.

The full table is in the reference under
[object systems](/reference/type-system#object-systems-s3-s4-r6), which also explains why the line
falls there. What does work today is declaring the class yourself — see
[domain modeling](/type-checking/domain-modeling).

## Non-standard evaluation

`dplyr`, `data.table`, `ggplot2` and `testthat` have shipped stubs, and their data-masked verbs are
understood: bare column names inside `mutate()` or a `data.table` bracket are column references, not
unresolved variables, and classes flow through a pipeline. A full
`read.csv |> mutate |> filter` chain checks clean.

Outside those, a data-masking function ry does not know about reports its column names as
unresolved. The ecosystem's standard escape hatch works — a top-level
`utils::globalVariables(c("a", "b"))` silences them for the whole package.

**Attaching a package ry does not know weakens the `unresolved` check.** A `library(pkg)` whose
exports ry cannot enumerate means any bare name *could* be one of them, so otherwise-unresolved bare
names are tolerated rather than reported — project-wide, not just in that file. This is what keeps
the tool usable on real code, but where it applies a clean run says less than it appears to.

Three things narrow the hole:

1. **Most packages are already known.** ry ships the export lists of the packages R code attaches
   most — see [what ships](/type-checking/stubs#what-ships). Attaching any of them keeps the check
   fully on: a real export resolves, and a typo beside it is still reported with a suggestion.
2. **A near miss of a name in scope is still reported.** `library(shiny)` cannot explain `repositry`
   sitting next to a `repository` parameter, so that stays a finding. In a package this covers your
   top-level definitions too; in a loose script it covers locals and parameters.
3. **`library(yourpkg)` costs nothing.** A `library()` naming your own package weakens nothing, so
   the one in `tests/testthat.R` is harmless.

The way to close it is a two-line [`stubs/<pkg>.Rtypes`](/type-checking/stubs), which also silences
the unknown-namespace warning. **Strict mode surfaces it**: a tolerated read is genuinely
undetermined, so under `[check] strict` each one is reported where it is read, naming the package
declaration as the fix. It stays silent in ordinary runs — that silence is the point of the
tolerance, since without it every export of an unstubbed `library()` would be a false `unresolved`.

A project's own `%op%` is deliberately left opaque: it may be an NSE wrapper whose right operand is
quoted rather than evaluated (magrittr's `%>%` is the canonical case), and checking that as an
ordinary call would reject correct code.

## One signature per function

A `#:` annotation declares exactly one signature, and there is no way to give your own function
several. The standard-library declarations do have them — `sum` is `integer` in and `integer` out
but `double` in and `double` out — and that asymmetry is deliberate rather than pending.

A name with several signatures has no single most general type, so a call to it has to be resolved
by trying candidates instead of inferred outright. That costs the principal-type guarantee the
checker's speed and stability rest on. It is an acceptable cost for a small curated corpus that has
to describe a standard library nobody designed with types in mind, and an unacceptable one across a
whole codebase. Where you would reach for an overload, a
[union](/reference/type-system#union-types) parameter usually says the same thing —
`fn(x: integer | character) -> character` — and two functions with distinct names always do.

## Scope of analysis

`source()` calls are not followed. Package files under `R/` share one namespace; every other file is
analyzed on its own. See [project discovery](/reference/configuration#project-discovery) for the
rules.

Files directly under `tests/testthat/` are an **approximation**: ry analyzes them as one shared
environment, helpers first. Real testthat only shares `helper*.R` and `setup*.R` that way — each
`test-*.R` runs in its own child environment. So a name one test file defines and another uses will
resolve here and fail when you actually run the tests. The approximation is deliberate, because it
keeps helper-defined names resolving, but it is more permissive than testthat is.

---

None of the above is a reason not to adopt it — see
[adopting an existing codebase](/guides/adopting) for how to turn it on a piece at a time, and
[project status](/why-ry#project-status) for how far along the whole thing is.
