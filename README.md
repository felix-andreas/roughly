<div align="center">

<img src="docs/public/logo.svg" alt="" height="96">

# ry

**One toolchain for R.** A language server, a static type checker, a formatter and a linter
in a single binary — analysis never runs your code, and needs no R installation.

[**Docs**](https://ry-lang.org) ·
[**Releases**](https://github.com/felix-andreas/ry/releases) ·
[**VS Code**](https://marketplace.visualstudio.com/items?itemName=felix-andreas.ry) ·
[**Zed**](https://github.com/felix-andreas/ry/tree/main/editors/zed)

</div>

---

## Why ry

R projects have got big — packages with hundreds of files, analysis codebases nobody holds in their
head. At that size the thing you want from your tools is an understanding of the *whole* project that
is cheap enough to recompute constantly. That is the bet this project makes: build one analysis of
your source, keep it fast, and serve formatting, linting, editor features and type checking from it.

Here is `ry check` on real CRAN sources — package `R/` directories concatenated into single flat
projects, with type checking switched on. There is no persistent analysis cache, so each run is a
fresh process analysing the whole project from scratch:

| Project | Files | Lines | `ry check .` |
| --- | ---: | ---: | ---: |
| 3 packages (ggplot2, dplyr, shiny) | 414 | 101,120 | **1.96 s** |
| 6 packages (+ Matrix, survival, caret) | 711 | 174,964 | **3.29 s** |
| 18 packages (+ sf, raster, forecast, plotly, data.table, …) | 1,531 | 323,471 | **6.47 s** |

Median of five runs, single process, release build, on a 4-core Xeon @ 2.80 GHz — a modest container,
not a benchmark rig. Those three points sit at a near-constant ~50,000 lines per second, so nothing
is blowing up as the project grows, though three points over a 3.2× range is evidence of that and not
a proof of asymptotics.

Whole-project runs are the pessimistic number. In an editor the analysis is incremental. Opening a
file in that same 1,531-file project and then typing into it, measured from the `didChange`
notification to the diagnostics that reflect it:

| | 323,471 lines, type checking on |
| --- | ---: |
| Open a file → first full diagnostics | 2.03 s |
| Each edit after that → updated diagnostics | **~63 ms** |

(That is a leaf file. Editing something the whole project imports will cost more; the incremental
engine re-checks what your edit could have reached, so the honest summary is that the cost scales
with your change's blast radius rather than with the project.)

## What one shared analysis buys you

Because everything reads the same picture, the checker knows what your values *are* — not just what
they are named. Two lines of `ry.toml`, in your project root:

```toml
[check]
typing = true
```

and no annotations at all:

```r
total_with_tax <- function(items, tax_rate) {
  subtotal <- 0
  for (item in items) {
    subtotal <- subtotal + item
  }
  subtotal * (1 + tax_rate)
}

total_with_tax(list(19.99, 5.00), "0.07")
```

<img src="docs/public/diagnostic.svg" alt='error[type-mismatch]: expected a numeric value (`integer` or `double`), found `character`, at billing.R line 9 column 35, pointing at the string "0.07"' width="780">

Nothing declared `tax_rate` a number. It was inferred from the arithmetic it takes part in, and the
error lands on the argument that is wrong rather than on the call.

Resolution is project-wide rather than per-file, which is what makes rename and go-to-definition safe
rather than a search-and-replace — and what lets a typo be reported as a typo:

```r
# R/utils.R
normalise_region <- function(x) tolower(trimws(x))

# R/report.R
build_report <- function(regions) {
  normalise_regoin(regions)
}
```

```text
warning[unresolved]: I could not resolve `normalise_regoin` in this package, its imports, or builtins. Did you mean `normalise_region`?
 --> R/report.R:2:3
2 |   normalise_regoin(regions)
      ^^^^^^^^^^^^^^^^

1 problem in 2 files
```

That check survives `library()`. `ry` ships an export manifest for the common CRAN packages, so
`library(dplyr)` brings `filter` into scope as a known name while a typo beside it is still reported.

A sample of what else falls out of the same pass — type errors, lints and syntax errors together:

```text
error[type-mismatch]: this call supplies 1 argument, but the function requires 2 — a required argument is missing
error[type-mismatch]: this function has no parameter `zzz` — its named parameters are `a`, `b`
error[type-mismatch]: field `gamma` does not exist in `list{alpha: double, beta: double}`
error[trailing-comma]: Unexpected comma after last argument
warning[boolean-shorthand]: Use TRUE, not T, for Boolean values
warning[unused]: `tmp` is assigned but never used.
```

Any of them can be silenced for the next line with a comment — `# ry: allow(boolean-shorthand)` — and
the [full rule list](https://ry-lang.org/reference/diagnostic-codes) is in the docs.

## Errors that point at the character

`ry` parses R with a hand-written recursive-descent parser rather than by calling R or reusing a
grammar. A parser whose job is to *run* your code has no reason to continue past the first error; one
written for tooling has to, because everything after the mistake still needs analysing — and having
carried on, it can usually name what was missing:

```text
error[syntax-error]: missing `,` between these arguments
 --> config.R:2:20
2 |   title = "Revenue"
                       ^
```

## The rest of the toolchain

`ry fmt` fixes spacing, indentation and bracing, and deliberately does **not** reflow your line
breaks — so a formatting run never buries a real change under rewrapped arguments:

```text
Diff in ./messy.R:
1        |-x<-c(1,2,3)
2        |-total<-sum( x )
    1    |+x <- c(1, 2, 3)
    2    |+total <- sum(x)
3   3    | breakdown <- summarise(
4   4    |   data,
5   5    |   by = region,
1 file would be reformatted, 0 files already formatted
```

(The gutter is old line number, new line number, then the change.)

For CI, `--output json` emits JSON Lines, and the exit status is `0` clean / `1` findings / `2` usage
error:

```json
{"code":"type-mismatch","column":19,"endColumn":22,"endLine":4,"line":4,"message":"this call supplies 1 argument, but the function requires 2 — a required argument is missing","path":"/home/you/project/R/summarise.R","related":[],"severity":"error"}
```

<sub>(`path` is absolute in real output; shortened here to fit.)</sub>

In an editor it is an ordinary LSP server, so navigation, completion, hover, rename, signature help
and inlay hints work the same in VS Code, Zed, Neovim and Helix.

## An optional type system for R

This is the most ambitious part of the project, and it is **entirely optional**: type errors stay off
until `ry.toml` turns them on, and formatting, linting, navigation and unresolved-name checking never
need them.

R is dynamic by design. Non-standard evaluation, `substitute`, building calls at runtime and
assigning classes on the fly are idiomatic R, not corner cases, and no static checker is going to
type all of that. `ry` does not try to. It describes the part of R that has a stable shape — the
arithmetic, the lists and records, the functions you wrote — and stays quiet about the rest.

Most of the time you write nothing at all. When you do want a contract, types go in `#:` comments —
chosen over roxygen's `#'` so the two never collide, and so the file stays ordinary R that R runs
unchanged and every other R tool still reads:

```r
#: fn(items: double[], rate: double) -> double
total_with_tax <- function(items, rate) sum(items) * (1 + rate)
```

`double[]` is a vector of doubles; the [concepts page](https://ry-lang.org/type-checking/concepts/)
introduces the rest of the notation.

**The inference runs whether or not you switch type errors on**, which is the part that is easy to
miss: hover, completion, signature help and inlay hints are all reading inferred types, and `typing`
only decides whether mismatches get *reported*. With type checking fully off, the unannotated
function above still hovers as this — `<T, U: numeric>` meaning it works for any two types as long as
the second is a number:

```
total_with_tax: <T, U: numeric> fn(items: T, tax_rate: U) -> double
```

### Why Hindley–Milner

The core is Hindley–Milner unification — the foundation under ML, OCaml and Elm — extended with
unions, R's coercion rules and constrained type variables. It was chosen for how it computes: types
are *solved* by unification rather than *searched* for, which keeps inference close to linear on real
code and, just as importantly, predictable. Type systems that resolve by search can become
unpredictably slow on ordinary-looking expressions; Swift is the best-documented case, where
overloaded operators combined with literal-defaulting rules can make one line exceed the compiler's
complexity budget and produce
[a famous error message](https://danielchasehooper.com/posts/why-swift-is-slow/) asking you to break
the expression up. A tool meant to stay responsive on a 300,000-line project cannot afford a checker
that may give up on a single line.

So two things are deliberately absent from the language you write, and are meant to stay absent:

- **No declared subtyping.** There is no `extends` and no way to say one of your types is a subtype
  of another. Compatibility widens only in the directions R itself coerces — up R's numeric ladder
  (`logical` → `integer` → `double` → `complex`), a scalar into a vector, a member into a union, a
  fixed-shape list into an element-typed one — and never the reverse. A user-defined hierarchy turns
  inference into constraint solving over inequalities, which is where the decidability problems
  start.
- **No overload sets in ordinary R code.** A `#:` annotation declares exactly one signature. If a
  function should accept several shapes, give the parameter a union type or split it in two.

The exception is *declaration files*: the `.Rtypes` files that ship with `ry` describing R's standard
library, plus any override you drop in your project's `stubs/`. Nobody designed base R with types in
mind, and describing `min` or `abs` takes several signatures today because the declaration language
cannot yet write the constrained, shape-preserving scheme that would cover each in one. So a declared
name may carry several signatures, resolved per call site — a bounded search over a fixed,
hand-checked set, which is a very different cost from opening it up across a whole codebase.

**Full R compatibility is not the goal.** A checker that accepted every legal R program would have to
accept programs whose types only exist at run time, and it would have nothing left to say about any
of them. The bar here is narrower: code with a describable shape gets checked, and everything else
becomes `Unknown`, which is compatible with everything and quietly permits it.

The full semantics are in the [type system reference](https://ry-lang.org/reference/type-system/).

## What it does not do yet

`ry` is `0.3.0-alpha`. Because unmodelled values become `Unknown` and `Unknown` is compatible with
everything, every gap below means checks are *skipped* — not that wrong answers are produced. Adding
`strict = true` under `[check]` reports every place that happened, which is the way to see how much
of your code is actually being checked.

- **Data frames are untyped.** Everything pulled out of a `data.frame` is `Unknown`, so
  `df$column_that_does_not_exist` checks clean. Since data frames are where most R analysis code
  lives, this is the gap that matters most, and it is why the type checker is currently worth more on
  package code than on analysis scripts.
- **Base R is described, not modelled exhaustively.** Your own functions are inferred precisely —
  `k <- function(n) n + 1; k("hello")` is an error — but the shipped corpus covers 355 `base`
  functions rather than all of them, and 280 of those take `Any` somewhere on purpose, because a
  stricter declaration would reject calls R itself accepts. So `sqrt("hello")` reports nothing.
- **S4 and R6 are opaque**, as is `UseMethod` dispatch. S3 operator dispatch and directly-called
  methods (`print.myclass(x)`) do resolve.
- **It can still fall over.** Checked against 69 CRAN packages chosen for popularity and size —
  758,584 lines — 66 finish. `rlang` hits an internal analysis cycle it cannot settle and exits with
  an error rather than results; `htmltools` and `mgcv` were still working when they were cut off at
  four minutes. Ordinary projects are fine, but this is alpha software with sharp edges in the tail.

The full accounting is in [limitations](https://ry-lang.org/type-checking/limitations/).

## Install

Grab a binary from [Releases](https://github.com/felix-andreas/ry/releases), install the
[VS Code extension](https://marketplace.visualstudio.com/items?itemName=felix-andreas.ry) (it bundles
one), or build from source with `cargo install --git https://github.com/felix-andreas/ry ry-lang`.
Then:

```
ry check        # lint and type-check the project
ry fmt          # format it (--check and --diff for CI)
ry server       # the language server; your editor starts this for you
```

Zed is not in the extension registry yet — install it from
[`editors/zed`](https://github.com/felix-andreas/ry/tree/main/editors/zed) as a dev extension. For
RStudio, `ry` works as an external formatter; the
[setup guide](https://ry-lang.org/installation/#rstudio) has the steps.

## Development

Layout, test suites and the non-obvious corners are documented in
[Development](https://ry-lang.org/contributing/development/) and
[Architecture](https://ry-lang.org/contributing/architecture/).

<sub>This is the third implementation. The first was a regex index called *"The R(oughly good enough)
language server"*; the second, *Roughly*, added real analysis; this one added the type checker and a
hand-written parser. The name has got shorter each time, which at the current rate makes the next
version a single keystroke.</sub>

## License

[UPL-1.0](LICENSE) © Felix Andreas
