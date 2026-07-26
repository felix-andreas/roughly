---
title: Why Roughly
description: Why R needs a unified, fast toolchain with a type checker at its core — and how far along this one is
---

R has good tools. What it does not have is tooling that understands your code.

## Knowing what a value is, without running the code

R's existing language servers do know what your values are — they ask a live R session. That is a real
advantage, and it is why completion on a fitted model or a loaded data frame works so well in RStudio.

It is also the limitation. A live session knows about code that has *already run*, in the state that
session happens to be in. It cannot tell you about the branch you have not taken, the function nobody
called yet, or the file you just opened in a fresh session. And it can only answer if the packages are
installed and the objects exist.

Roughly answers the same questions from the source alone, which makes the answers available everywhere
the code is — in a pull request, in CI, in a file you have never run:

- a typo in a variable name
- an argument in the wrong position, or a call missing a required one
- a value that is sometimes `NULL`, used as though it never is
- a name that no longer exists because you deleted it in another file

None of these are style problems, so a tool that only sees syntax cannot find them. R finds them for
you at runtime, in the middle of the job that mattered.

The other half is that everything is served from **one** understanding. Formatting, code analysis,
editor features, and type checking are views onto the same knowledge rather than separate programs each
parsing your source and building a partial picture of it.

## Tooling written in R is too slow where it matters most

R is a fine language to analyse code *in*, until the codebase gets big. The projects that most need
help — the package with 40,000 lines, the pipeline that grew for six years — are exactly the ones
where an R-implemented tool becomes slow enough that you stop running it. Tooling you switch off is
tooling you do not have.

Roughly is written in Rust, and analysis is incremental: an edit re-checks what the edit could have
affected, not the project. It is built for codebases in the hundreds of thousands of lines, because
that is where the value is.

It also **needs no R installation**. `check` and `fmt` never load R, never execute your code, and never
depend on which packages happen to be installed. That is what makes them safe in CI and instant in an
editor. (The one exception is the [R console](/guides/r-console), which by definition runs R.)

## Every mature dynamic language grew types

Python has type hints and mypy. JavaScript got TypeScript. Elixir is adding set-theoretic types. Ruby
has RBS. In each case the language stayed dynamic, the types were optional and gradual, and the
ecosystem adopted them because the alternative — finding type errors by running the program — stops
scaling long before the codebase does.

R has not had this. It is not because R is unsuited to it: R code is full of implicit type
expectations, and they are exactly the expectations that break in production.

Roughly's answer is deliberately R-shaped. There is no new syntax and no new file format — annotations
live in `#:` comments, so annotated code is still ordinary R that every other tool reads. Most code
needs no annotations at all, because [use decides type](/type-checking/concepts): `a + b` makes `a` and
`b` numbers, `paste0` makes its arguments strings. And type checking is opt-in, so you can adopt it one
file at a time.

## Project status

Roughly is version `0.3.0-alpha`. It is not on CRAN, and it has one maintainer. Being honest about
what that means:

**Stable enough to build on.** The diagnostics, the `roughly.toml` keys, the diagnostic codes, and the
JSON output are covered by tests that fail when they change. CI built on them will not break silently.

**Still gaining capability.** The type system is where the movement is. A new release may report
findings an older one did not — which is the point, but it means pinning a version is sensible if you
gate a build on a clean run.

**Where it runs.** `roughly check` reads `.R` files and the R chunks of `.Rmd`, `.qmd`, and `.Rnw`
documents. The editor integration does not cover literate documents yet — you get them in `check` and
in CI, but not as you type. The formatter deliberately leaves them alone, since most of an `.Rmd` is
prose it has no business rewriting.

**What it will not do yet.** The gaps that matter most are data frames, S4, and R6 — see
[limitations](/type-checking/limitations) for the full picture before you decide how far to trust a
clean run.

## Next

- [Features](/features) — what you get, before you turn anything on
- [Tutorial](/type-checking/tutorial) — the type checker on real code
