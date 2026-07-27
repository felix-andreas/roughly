---
title: Tutorial
description: Learn ry's type checker by putting it on real R, one step at a time
---

This tutorial covers the type checker: writing an annotation, what inference gives you without one,
unions and `NULL`, declaring your own domain types, generics, and strict mode. It assumes you can
already run `ry check` on a project.

:::note
Type errors are opt-in. Add this to `ry.toml`, or put `# typing: on` at the top of a file:

```toml
[check]
typing = true
```
:::

## 1. Your first annotation

Here is a function with a type written above it:

```r
#: fn(price: double, rate: double) -> double
apply_discount <- function(price, rate) {
  price * rate
}
```

That first line is the annotation. It reads left to right: `fn(` the parameters and their types `)`,
then `->` and the return type. It lives in a `#:` comment, so R and every other R tool see a comment
and carry on — annotated code is still ordinary R.

Now call it wrongly:

```r
apply_discount(100, "0.2")
```

```text
error[type-mismatch]: expected `double`, found `character`
 --> t.R:6:21
6 | apply_discount(100, "0.2")
                        ^^^^^
```

Found without running anything, and pointing at the argument rather than at the line that would have
failed.

The annotation is a promise the checker holds you to in both directions. Claim the wrong return type
and it reports that too, pointing at the body:

```r
#: fn(price: double, rate: double) -> character
apply_discount <- function(price, rate) {
  price * rate
}
```

```text
error[type-mismatch]: expected `character`, found `double`
 --> a.R:3:3
3 |   price * rate
      ^^^^^^^^^^^^
```

## 2. Inference

Delete the annotation entirely and run it again:

```r
apply_discount <- function(price, rate) {
  price * rate
}

apply_discount(100, "0.2")
```

```text
error[type-mismatch]: expected `double`, found `character`
 --> t.R:5:21
5 | apply_discount(100, "0.2")
                        ^^^^^
```

The same error. `*` is arithmetic, so `price` and `rate` are numbers — the checker worked that out
from the body. This is **inference**, and it is why most R needs no annotations at all.

Which kind of inference decides how far you can trust it. ry uses **Hindley–Milner** inference, the
algorithm behind ML, Haskell and Elm. Two properties matter here:

- It computes a **principal type** — the single most general type consistent with how a value is
  used. Not a guess, not a heuristic. Run it twice and you get the same answer; run it on a
  colleague's machine and they get yours. (Calls into the standard library are the one place a
  *choice* is made, because a handful of R's builtins are declared with several signatures; the
  [rules for that](/reference/type-system#overload-sets) are fixed and deterministic too.)
- It does not silently accept a contradiction. Within the part of your program it can model, if the
  types cannot line up, it reports that.

The second clause carries the limit. R has constructs no type system can follow — `UseMethod`
dispatch, data-frame columns, S4. There the checker yields `Unknown` rather than guessing, and
`Unknown` is compatible with everything, so one gap produces no consequential errors.
[Strict mode](#7-strict-mode) is how you find those gaps.

## 3. When to annotate

Inference handles the interior. Annotate at the edges:

- **Exported functions**, and anything else another file or another person calls. The annotation is
  documentation the checker enforces, and it stops a change to the body quietly changing the
  contract.
- **Where you want a promise held.** If a function must return a `double`, say so, and the day
  someone makes it return a list you find out immediately.
- **Where inference cannot see.** A value that arrives from a data frame, a database, or `readRDS()`
  is `Unknown`. If you know what it is, say so.

Not every local variable. Annotating `n <- 1L` adds nothing the checker did not already know.

## 4. `NULL` and narrowing

This is the error most likely to be a real bug in code you have already shipped:

```r
#: fn(config: list{retries: integer} | NULL) -> integer
retry_count <- function(config) {
  config$retries
}
```

```text
error[type-mismatch]: expected a list, found `list{retries: integer} | NULL`
 --> a.R:3:3
3 |   config$retries
      ^^^^^^^^^^^^^^
```

The `|` makes a union: `config` is *either* that list *or* `NULL`. You cannot reach into it until
you have ruled out `NULL`. Add the guard and the error goes away:

```r
#: fn(config: list{retries: integer} | NULL) -> integer
retry_count <- function(config) {
  if (is.null(config)) return(3L)
  config$retries
}
```

```text
1 file checked, no problems
```

The checker follows the `if`: after the early return, `config` cannot be `NULL` any more, so the
field access is fine. That is called narrowing.

## 5. Domain types

Two `double`s that must never be mixed up:

```r
#: @type Celsius {double}

#: @type Fahrenheit {double}

#: fn(t: Celsius) -> Fahrenheit
to_fahrenheit <- function(t) t
```

```text
error[type-mismatch]: expected `Fahrenheit`, found `Celsius`
```

`@type` makes a name that is its own type, distinct from everything else even when the
representation is identical. This is the part inference cannot do for you: only you know that these
two numbers mean different things.

See [domain modeling](/type-checking/domain-modeling) for constructors and validation.

## 6. Generics

An unannotated function that constrains nothing is already generic:

```r
identity2 <- function(x) x
```

Hover it and you get `<T> fn(x: T) -> T` — for any type `T`, takes one and returns the same one. Add
arithmetic and the type narrows on its own to `<T: numeric> fn(x: T) -> T`. Neither has to be asked
for, and neither has to be maintained.

One caveat: a single `T` used twice means *the same* type twice. Against
`<T: numeric> fn(value: T, factor: T) -> T`, the call `scale_by(2L, 0.5)` is reported, because
`integer` and `double` are different `T`s. Widen one side yourself when you want to mix.

## 7. Strict mode

A clean run means the checker found no contradictions. It does not mean it checked everything.
Strict mode reports every place a value became `Unknown`:

```toml
# ry.toml
[check]
typing = true
strict = true
```

```r
summarise <- function(frame) {
  frame$amount
}
```

```text
error[strict]: strict mode: this expression has an undetermined type (`Unknown`)
 --> a.R:2:3
2 |   frame$amount
      ^^^^^^^^^^^^
```

Data-frame columns have no types yet, so that expression was never really checked, and without
strict mode it looked like a pass. Turn this on for the modules you rely on, not across a legacy
codebase on day one.

## 8. Adopting it a file at a time

You do not have to convert a project at once. A directive at the top of a file beats the project
setting, in either direction:

```r
# typing: on       # check this file even if the project has typing off
# typing: off      # skip this one while you work through the rest
# typing: strict   # hold this module to the stronger standard
```

[Adopting an existing codebase](/guides/adopting) has the order that works on a project with
findings already in it.

## Where to go next

- [Concepts](/type-checking/concepts) — the full vocabulary: vectors, records, `Any` vs `Unknown`
- [Domain modeling](/type-checking/domain-modeling) — nominal types instead of S4, R6, or S7
- [Limitations](/type-checking/limitations) — where the checker cannot help yet
- [Type system reference](/reference/type-system) — the precise rules
