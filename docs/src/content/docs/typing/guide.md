---
title: Guide
description: Learn Roughly's type checker for R by using it — from your first run to annotations, domain types, and strict mode
---

This is a walkthrough, not a specification. You will turn the checker on, run it on real code,
read what it says, and add annotations only where they earn their place. Every output on this page
is what the tool actually prints.

For the exact rules behind any of it, the [typing reference](/typing/reference) is the contract.
For what the checker cannot do, [limitations](/limitations) is honest about it.

## 1. Run it before you configure anything

Make a file. This one has three ordinary mistakes in it:

```r
# clean.R
summarise_orders <- function(orders) {
  totals = sapply(orders, function(o) o$amount)
  verbose <- T
  mean(totals)
}
```

```bash
roughly check
```

```text
warning[unused]: `summarise_orders` is assigned but never used.
 --> clean.R:1:1
1 | summarise_orders <- function(orders) {
    ^^^^^^^^^^^^^^^^

warning[assignment-operator]: Use <-, not =, for assignment
 --> clean.R:2:10
2 |   totals = sapply(orders, function(o) o$amount)
             ^

warning[unused]: `verbose` is assigned but never used.
 --> clean.R:3:3
3 |   verbose <- T
      ^^^^^^^

warning[boolean-shorthand]: Use TRUE, not T, for Boolean values
 --> clean.R:3:14
3 |   verbose <- T
                 ^

4 problems in 1 file
```

No configuration, no annotations, no R installation. Every code in brackets is a
[stable name](/diagnostics) you can suppress individually.

Type errors are the one thing you opt into, because they are the part that can be noisy on code
that has never been checked:

```toml
# roughly.toml
[check]
typing = true
```

## 2. What the checker already knows

With that one line, Roughly reads your code the way a compiler would — no annotations required. It
knows `config` is a list with three named fields, and that one of them is misspelled:

```r
config <- list(input_path = "orders.csv", vat_rate = 0.07, currency = "EUR")

total_with_tax <- function(subtotal) {
  subtotal + subtotal * config$tax_rate
}

subtotals <- c(240.00, 99.50, 1250.00)
invoices <- sapply(subtotals, total_with_tax)
cat("grand total:", sum(invoices), "\n")
```

```text
error[type-mismatch]: field `tax_rate` does not exist in `list{input_path: character, vat_rate: double, currency: character}`. Did you mean `vat_rate`?
 --> billing.R:4:25
4 |   subtotal + subtotal * config$tax_rate
                            ^^^^^^^^^^^^^^^
```

Someone renamed the field and missed a use. R would not have told you here — it would have told you
on line 9, inside `sum`, with `invalid 'type' (list) of argument`. Remove the `sapply` and R does
not complain at all: `$` on a missing name is `NULL`, and the report goes out with a number missing.

This is the whole pitch, and it cost one line of configuration.

## 3. How it knows: use decides type

Nothing above was annotated. Roughly works out what a value is from what you do with it:

```r
scale_by <- function(value, factor) value * factor
```

`*` is arithmetic, so both parameters must be numeric — and since `*` requires them to agree, they
are the *same* numeric type. The inferred signature is:

```text
<T: numeric> fn(value: T, factor: T) -> T
```

Read that as: for any numeric type `T`, this takes two `T`s and returns a `T`. So
`scale_by(2L, 3L)` is `integer`, `scale_by(0.5, 2.0)` is `double`, and inference did not force one
answer where R allows several. Hover the name in your editor to see this without running anything.

The same reasoning tracks values through control flow, so guards work:

```r
#: fn(n: integer | NULL) -> integer
add_one <- function(n) n + 1L
```

```text
error[type-mismatch]: expected a numeric value (`integer` or `double`), found `integer | NULL`
 --> guard.R:2:24
2 | add_one <- function(n) n + 1L
                           ^
```

`n` might be `NULL`, and `NULL + 1L` is an error in R. Guard it and the finding goes away, because
after the `return` the checker knows `n` cannot be `NULL` any more:

```r
#: fn(n: integer | NULL) -> integer
add_one_safely <- function(n) {
  if (is.null(n)) return(0L)
  n + 1L
}
```

## 4. Your first annotation

Inference describes what your code *does*. An annotation states what it is *for* — and that is a
different, stronger claim. Use one when you want the contract checked at every call site rather
than inferred from the body.

Annotations live in `#:` comments, so every other R tool sees a comment and your code stays
ordinary R:

```r
#: fn(region: character, amount: double) -> double
fee_for <- function(region, amount) {
  if (region == "north") amount * 0.07 else amount * 0.05
}

order <- list(region = "north", amount = 1240.5)
fee_for(order$amount, order$region)
```

```text
error[type-mismatch]: expected `character`, found `double`
 --> ann.R:7:9
7 | fee_for(order$amount, order$region)
            ^^^^^^^^^^^^

error[type-mismatch]: expected `double`, found `character`
 --> ann.R:7:23
7 | fee_for(order$amount, order$region)
                          ^^^^^^^^^^^^
```

Two arguments in the wrong order, both reported, each pointing at the argument rather than the
call. Without the annotation this particular mistake is still caught — `region == "north"` makes
`region` a `character` on its own — but the annotation is what makes the *contract* explicit and
the error land at the caller.

There is an expanded style too, which suits functions with many parameters and sits naturally
beside roxygen2:

```r
#' Fee for an order.
#: @param region {character}
#: @param amount {double}
#: @returns {double}
fee_for <- function(region, amount) { ... }
```

## 5. Give your domain its own types

This is where a type checker starts paying for itself. `@type` declares a **nominal** type — one
that is distinct from its representation, so two things that are both `double` underneath stop
being interchangeable:

```r
#: @type Meters {double}
#: @type Seconds {double}

#: fn(distance: Meters, time: Seconds) -> double
speed <- function(distance, time) distance / time

#: @new Meters
run_length <- 400

#: @new Seconds
run_time <- 52.5

speed(run_length, run_time)
speed(run_time, run_length)
```

```text
error[type-mismatch]: expected `Meters`, found `Seconds`
  --> nom.R:14:7
14 | speed(run_time, run_length)
           ^^^^^^^^

error[type-mismatch]: expected `Seconds`, found `Meters`
  --> nom.R:14:17
14 | speed(run_time, run_length)
                     ^^^^^^^^^^
```

The first call is accepted; only the swapped one is reported. Both are numbers, both are `double`
at run time, and R will never distinguish them — nor will any test that happens to pass. `@new` is what mints a nominal value — a plain `400` stays a plain `double`,
which is what keeps the distinction meaningful.

When you want a name for a shape *without* a new identity — shorthand rather than a distinction —
use `@alias`:

```r
#: @alias Row {list{name: character, n: integer}}
```

An alias is transparent: `Row` and its expansion are the same type everywhere.

## 6. Generics, when you actually have them

You rarely need to write one — inference produces them on its own, as `scale_by` did above. Write
one when you want to *promise* that a function preserves its argument's type:

```r
#: <T> fn(x: T[]) -> T[]
shuffle <- function(x) sample(x)

shuffle(c(1L, 2L))    # integer[]
shuffle(c("a", "b"))  # character[]
shuffle(list(1L))     # error[type-mismatch]: expected `T[]`, found `list{integer}`
```

`T` is bound by the caller, once per call. The third line fails because `T[]` is an *atomic* vector
and a list is not one.

## 7. Ask what the checker could not see

A clean run means "no errors found". It does not mean "everything was checked" — an unmodelled
construct becomes `Unknown`, and `Unknown` is compatible with everything so that one gap does not
cascade into a screen of noise.

Strict mode reports those gaps:

```toml
[check]
typing = true
strict = true
```

```text
error[strict]: strict mode: this expression has an undetermined type (`Unknown`)
 --> R/a.R:2:28
2 | count_rows <- function(df) df$whatever
                               ^^^^^^^^^^^
```

That is the honest way to find out how much of a module is really being checked. It is a much
stronger claim than ordinary checking, so turn it on where you rely on the answer, not everywhere
at once.

## 8. Turning it on for real code

Enabling `typing = true` across a codebase that has never been checked will find things. Work
through them in stages rather than in one sitting — the ladder, and the shapes you are most likely
to hit, are on the [limitations page](/limitations#turning-it-on-for-an-existing-project). The key
tool is the per-file directive, which overrides the project setting either way:

```r
# typing: on       # check this file even if the project has typing off
# typing: off      # skip this one while you work through the rest
# typing: strict   # hold this module to the stronger standard
```

## Where to go next

- [Typing reference](/typing/reference) — the precise rules for everything above
- [Limitations](/limitations) — data frames, object systems, and the adoption ladder
- [Diagnostic codes](/diagnostics) — every code, and how to suppress one
- [Standard library types](/stdlib-stubs) — what ships typed, and how to add your own
