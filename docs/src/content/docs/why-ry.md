---
title: Why ry
description: Why R needs one fast toolchain with a type checker at its core — and how far along this one is
---

R has good tools, but they are separate tools. Linting, formatting, style, analysis and running the
code are five programs that each parse your source and each build their own partial picture of it.
None of them shares what it learned with the others, so each one starts over.

## Static, not a live session

R's language servers do know what your values are — they ask a live R session. That is why completion
on a fitted model works so well in RStudio.

That is also its limit: a session knows only code that has already run, in the state it happens to be in.
It cannot tell you about the branch you have not taken, the function nobody called, or the file you
just opened. ry answers from the source alone, so the answers exist in a pull request, in CI, and
in a file you have never run:

- a typo in a variable name
- an argument in the wrong position, or a call missing a required one
- a value that is sometimes `NULL`, used as though it never is
- a name you deleted in another file

And everything is served from **one** understanding: formatting, analysis, editor features and type
checking are views onto the same knowledge.

## Speed on large codebases

Large R projects are where tooling latency stops being a detail: once a check takes long enough to
break your train of thought, you stop running it, and the tool may as well not exist.

ry is written in Rust, and analysis is incremental: an edit re-checks only what that edit could
have affected, not the project. It is tested against roughly 970,000 lines of real R — 69 CRAN
packages plus R's own base library — and the check that an edit does not trigger more work than it
should runs on every change.

It also **needs no R installation**. `check` and `fmt` never load R and never execute your code, which
is what makes them safe in CI and instant in an editor. The one exception is the
[R console](/guides/r-console), which by definition runs R.

## Types in dynamic languages

Python has type hints, JavaScript got TypeScript, Ruby has RBS, Elixir is adding set-theoretic types.
Each stayed dynamic, kept types optional, and adopted them because finding type errors by running the
program stops scaling long before the codebase does.

R code is full of implicit type expectations, and nothing checks them until the code runs.

ry's approach rests on **inference**: the checker works out types from how values are used,
instead of making you declare them.

```r
scale <- function(x, factor) x * factor
```

`*` is arithmetic, so both parameters are numbers. Nothing was declared, and `scale("a", 2)` is
already an error. That is why most R needs no annotations at all — and the ones you do write live in
`#:` comments, so the file stays ordinary R that every other tool reads. Type checking is opt-in, so
you can adopt it one file at a time.

## Project status

ry is version `0.3.0-alpha`. It is not on CRAN, and it has one maintainer. What that means in
practice:

**Stable enough to build on.** The diagnostics, the `ry.toml` keys, the diagnostic codes, and the
JSON output are covered by tests that fail when they change. CI built on them will not break silently.

**Still gaining capability.** The type system is where the movement is. A new release may report
findings an older one did not — which is the point, but it means pinning a version is sensible if you
gate a build on a clean run.

**Where it runs.** `ry check` reads `.R` files and the R chunks of `.Rmd`, `.qmd`, and `.Rnw`
documents. The editor integration does not cover literate documents yet — you get them in `check` and
in CI, but not as you type. The formatter deliberately leaves them alone, since most of an `.Rmd` is
prose the formatter should not rewrite.

**What it will not do yet.** The gaps that matter most are data frames, S4, and R6 — see
[limitations](/type-checking/limitations) for the full picture before you decide how far to trust a
clean run.

## Next

- [Features](/features) — what you get, before you turn anything on
- [Tutorial](/type-checking/tutorial) — the type checker on real code
