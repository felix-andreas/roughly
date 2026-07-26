---
title: Features
description: What ry gives you before you configure anything — and the one flag that changes the rest
---

Everything below works with no configuration and no annotations. The last section needs one line of
configuration.

## Navigation and completion

R already has a language server, written in R. On a large project it becomes slow enough that people
turn it off. The alternative is to stay inside RStudio or Positron, where the analysis is
good but does not travel to any other editor.

ry is a standard language server, so go-to-definition, find-references, completion and hover work
in VS Code, Zed, Neovim and Helix alike. It reads your whole project, so a name defined in one file
completes in another:

```r
# R/utils.R
normalise_region <- function(x) tolower(trimws(x))
```

```r
# R/report.R
normalise_re      # completes to normalise_region
```

## Syntax errors

ry parses R with its own hand-written parser rather than calling R or reusing a grammar. That
is what makes these messages possible. Forget a comma in a list:

```r
config <- list(
  title = "Revenue"
  subtitle = "by quarter",
  width = 800
)
```

R reports the token it could not accept:

```text
Error: unexpected symbol in:
"  title = "Revenue"
  subtitle"
```

ry reports what is missing, and points at the place it should go:

```text
error[syntax-error]: missing `,` between these arguments
 --> a.R:2:20
2 |   title = "Revenue"
                       ^
```

The same comparison with a parenthesis left open:

```r
if (x > 1 {
  y
}
```

```text
Error: unexpected '{' in "if (x > 1 {"
```

```text
error[syntax-error]: unclosed `(` in the `if` condition; expected a matching `)`
 --> a.R:1:4
1 | if (x > 1 {
       ^
```

R's parser stops at the first error, because its job is to run your code. A parser written for tooling
carries on, keeps the rest of the file analyzable, and can say which construct was left open.

## Formatting

`ry fmt` fixes spacing, indentation and bracing:

```r
x<-c(1,2,3)
if(x>1){y<-2}
```

```r
x <- c(1, 2, 3)
if (x > 1) { y <- 2 }
```

What it does not do is decide where your line breaks go. Both of these are already formatted, and both
stay exactly as written:

```r
totals <- summarise(data, by = region)

breakdown <- summarise(
  data,
  by = region
)
```

You chose one call on a line and the other spread over four; the formatter keeps both. A formatter that
reflows to a column limit would rewrite one of them, and your next diff would show line breaks instead
of the change you made. There are no options to argue about — the reasoning is in
[formatting rules](/reference/formatting-rules#philosophy).

## Rename

Rename does not search and replace. It runs the same analysis that answers go-to-definition: every
occurrence is resolved to the binding it refers to, and only the occurrences bound to the one you
picked are edited.

That matters because the same word is not the same thing twice. A local `total` inside one function and
a global `total` used by another are different bindings. A parameter shadows the global of the same
name for the length of the function. The word appearing inside a string is not a binding at all.
Renaming one of them must leave the others alone, across every file in the project.

## One more thing

Everything above comes from one model of your project: which names exist, which binding each
occurrence refers to, and where each is visible. That model is what separates these features from text
search, and building it is most of the work.

The type checker goes one step further. It does not only know which binding a name refers to — it
knows what kind of value that binding holds. Completion does not only know which names exist, it knows
what is inside them:

```r
account <- list(holder = "ada", balance = 120.5)
account$        # balance, holder — with their types
```

Nothing declared those fields. And hovering a function you never annotated gives you its full type:

```r
make <- function(x) list(x, 1L)
```

```text
make: fn(x: T) -> list{T, integer}
```

It takes any value and returns a two-element list of that value and an integer. Inference produced
that, and it has been running the whole time.

One thing is still off by default: reporting the places where those types disagree.

```toml
# ry.toml
[check]
typing = true
```

```r
parse_count <- function(raw) {
  count <- 0L
  if (raw == "unknown") {
    count <- "?"
  }
  count + 1L
}

print(parse_count("12"))
```

```text
error[type-mismatch]: expected a numeric value (`integer` or `double`), found `integer | character`
 --> parse.R:6:3
6 |   count + 1L
      ^^^^^
```

One line of configuration, no annotations, no change to the code.

Type checking a whole project sounds expensive, and this is where it would show. It does not, because
analysis is incremental: an edit re-checks only what that edit could have affected, not the project.
ry is tested against ry 970,000 lines of real R — 69 CRAN packages plus R's base library.

- [Tutorial](/type-checking/tutorial) — put it on real code
- [Concepts](/type-checking/concepts) — how it works out what it knows
- [Adopting an existing codebase](/guides/adopting) — turning it on without drowning
