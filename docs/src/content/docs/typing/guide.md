---
title: Guide
description: A guided tour of Roughly's static type checker for R — from inference-only checking to annotations, generics, and strict mode
---

Roughly includes a static type checker for R. R has no type system of its own, so
Roughly defines one: it infers types for ordinary R code and lets you add optional
annotations to tighten and document those types. Inference is Hindley-Milner style,
so most code is checked without any annotations at all.

Because R has no type-annotation syntax, annotations live in `#:` comments. They look
like comments to every other R tool, so annotated code stays fully compatible with
regular R.

This page is a guide: it introduces the type system one concept at a time, in the order
you will meet them in real code. For the full, precise semantics — every coercion,
operator, and annotation rule — see the [Typing Reference](/typing/reference).

## Start without annotations

You do not need to write a single annotation to benefit from the checker. It always
runs to power editor features: hover shows the inferred type of any expression, inlay
hints show inferred types of bindings, and signature help shows function signatures as
you type. That works out of the box.

```r
count <- 3L          # hover: count : integer
ratio <- count / 2   # hover: ratio : double
greet <- function(name) paste("hi", name)
                     # hover: greet : <T> fn(name: T) -> character
```

Type-*error* diagnostics are opt-in, so a project that has not adopted typing is not
flooded with messages. Turn them on in `roughly.toml`:

```toml
[check]
typing = true
```

With that set, `roughly check` and your editor report a `type-mismatch` diagnostic
wherever types do not line up — even in completely unannotated code, because inferred
types flow through bindings, calls, operators, and indexing:

```r
add <- function(a, b) a + b   # inferred: <T: numeric> fn(a: T, b: T) -> T
add(1L, "x")
# error[type-mismatch] expected a numeric value (`integer` or `double`), found `character`
```

Two things are worth noticing in that example, because they are the heart of the
system:

- The checker worked out on its own that `a` and `b` must be numeric — using `+` is
  what constrained them.
- The result is a *generic* function: `add(1L, 2L)` is `integer`, `add(0.5, 1)` is
  `double`. Inference does not force one answer where R allows several.

When the checker cannot model a construct, it does not guess: the expression becomes
`Unknown` and checking continues without cascading errors. More on that
[below](#the-gradual-boundary-any-and-unknown).

## The shape of R values

To read inferred types — and later to write annotations — you need the type
vocabulary. It mirrors how R values actually behave.

### Atomic values and the three vector shapes

R's atomic types are `logical`, `integer`, `double`, `complex`, `character`, and
`raw`. Every atomic value in R is a vector, but code treats them in three
distinguishable ways, and the type system keeps them apart:

| You write | Meaning | Example value |
|-----------|---------|---------------|
| `integer` | scalar-like: a single value | `1L` |
| `integer[]` | array-like: many values, positional | `c(1L, 2L)` |
| `integer[named]` | map-like: values keyed by names | `c(a = 1L)` |

Literals are scalar-like. `c(...)` builds array-like vectors (or map-like ones when
every element is named) and applies R's promotion rules, so `c(1L, 2.5)` is
`double[]` and `c(1L, "a")` is `character[]`.

The distinction matters because extraction and iteration behave differently per
shape: `x[[1L]]` on an `integer[]` gives `integer`; name-based `[[` on a map-like
vector gives `integer | NULL`, because the name may be absent at runtime.

### Lists

R lists carry more structure, and the type system offers four forms, from loosest to
most precise:

- `list[integer]` — array-like: any number of elements, all the same type.
- `list[named: integer]` — map-like: keyed by names, all values the same type.
- `list{integer, character}` — tuple-like: exactly these elements, in this order.
- `list{name: character, age: double}` — record-like: exactly these named fields.

`list(...)` infers the precise form: `list(1L, "a")` is the tuple
`list{integer, character}`, and `list(name = "Ada", age = 36)` is the record
`list{name: character, age: double}`. Field access is checked — `person$name` on the
record above is `character`, and `person$nmae` is a type error.

### `NULL`

`NULL` has its own type, and "a value or `NULL`" is written as a union: `integer |
NULL`. This is how R idioms like optional list fields and absent names are modeled —
name-based access returns `T | NULL` rather than pretending the value is always
there.

## When types meet: coercions

R freely promotes values in well-understood ways, and the checker mirrors the safe
ones instead of demanding exact matches at every boundary:

- **Scalars coerce into vector positions.** Where `integer[]` is expected, `1L` is
  accepted — a scalar is a length-one vector.
- **`integer` widens to `double`** (in any shape): `mean(1L)` and `sd(c(1L, 2L))`
  are fine. The reverse never holds — a `double` is not accepted where `integer` is
  required.
- **Whole-number literals count as `integer` at parameter positions.** R programmers
  write `seq_len(10)`, not `seq_len(10L)`; the literal `10` passes where `integer`
  is expected. A fractional literal (`2.5`) or a `double` *variable* does not.
- **`T` coerces into `T | NULL`** — a plain value fits wherever an optional one is
  expected.

These are directional conveniences applied when checking a value against an
expectation. Inference itself never widens: a binding inferred `integer` stays
`integer`.

## More than one possibility: unions

Control flow can leave a variable with more than one possible type, and R code does
this on purpose. The checker models it with unions instead of rejecting it:

```r
value <- if (condition) 1L else "one"
# value : integer | character
```

Reassignment across branches works the same way — the checker tracks variables as
mutable slots the way R actually treats them:

```r
x <- 1L
if (flag) x <- "two"
x + 1L
# error[type-mismatch]: `x` is `integer | character` here, and `+` rejects the
# `character` member
```

A union value must be acceptable in *every* shape it can take. Operators and field
access apply member-wise: comparing `integer | double` against a number is fine
(both members are numeric); adding `integer | character` is an error (one member is
not).

When a member is impossible, guard it the way you already do in R — the checker
**narrows** the variable along the branches:

```r
#: fn(x: integer | NULL) -> integer
f <- function(x) {
  if (is.null(x)) {
    return(0L)
  }
  x + 1L   # x : integer here — the NULL member is gone
}
```

`is.null`, the `is.character`/`is.numeric`/`is.integer`/`is.double`/`is.logical`
family, `is.function`, and `is.list` all narrow, negation (`!is.null(x)`) swaps the
branches, and an early exit (`return`, `stop`) carries the surviving type past the
`if`, as above. The [typing reference](/typing/reference#guard-narrowing) lists the
exact rules and limits.

## The gradual boundary: `Any` and `Unknown`

Two special types make the system gradual rather than all-or-nothing:

- **`Any` is a deliberate opt-out.** It is compatible with everything in both
  directions and produces no diagnostics. Standard-library functions whose result
  cannot be described statically return `Any`; you can annotate with it too.
- **`Unknown` is an honest "could not determine".** An unsupported construct or an
  unresolved name yields `Unknown`. It also flows without erroring — the checker
  diagnoses the *cause* once, where the type became `Unknown`, instead of cascading
  a second error at every later use. [Strict mode](#strict-mode) turns those origins
  into visible diagnostics.

The difference matters: `Any` says "I chose not to check this"; `Unknown` says "the
checker could not check this". Strict mode reports the latter, never the former.

## Your first annotation

Inference carries you far, but annotations let you state intent, tighten interfaces,
and document code. An annotation is one or more `#:` lines attached to the binding or
expression that follows immediately (no blank line in between; consecutive `#:` lines
form one block):

```r
#: integer
port <- 8080L
```

A plain type annotation is **checked**: the inferred type of the value must be
compatible with the declared type, and the declaration becomes the binding's type.

```r
#: list[integer]
sizes <- list(1L, 2L)     # ok: the tuple fits the array-like claim

#: integer
label <- "west"
# error[type-mismatch] expected `integer`, found `character`
```

Two escape hatches cover the boundary with untypeable code:

- `#: @trust TYPE` asserts a type **without checking** — for values the checker
  cannot see through (foreign calls, reflection). Use sparingly; a wrong `@trust` is
  a wrong type from then on.
- `#: @if-unknown TYPE` applies only when the inferred type is `Unknown` — it refines
  what the checker could not determine but never overrides what it could. This is
  the safe way to annotate around unsupported constructs.

## Annotating functions

Functions are where annotations pay off most: the annotation is checked against the
body *and* becomes the signature every caller is checked against.

### Compact style

```r
#: fn(count: integer) -> integer
double_it <- function(count) count * 2L
```

- Optional parameters wrap their name in brackets: `fn(count: integer, [label]:
  character) -> integer`.
- A variadic function declares its rest parameter as `...: TYPE`; every surplus
  positional argument is checked against that element type.
- Parameter names must match the function's formals — the checker matches call
  arguments by name exactly like R does, and it will tell you when an annotation
  names a parameter the function does not define.

### Expanded style

For longer signatures, `@param` and `@returns` document one parameter per line.
The parameter name comes first, its type in braces:

```r
#: @param render_count {fn(integer) -> character}
#: @param count {integer}
#: @param [label] {character}
#: @returns {character}
render <- function(render_count, count, label = "n") {
  paste(label, render_count(count))
}
```

Both styles produce the same signature; pick per function. The formatter keeps both
styles tidy.

### Alongside roxygen2

A `#:` block attaches to the statement that follows it *immediately*, so in a documented
package the annotation goes **last** — directly above the definition, below the roxygen2
block:

```r
#' Add one to a count.
#'
#' @param count An integer.
#' @return The incremented count.
#' @export
#: fn(count: integer) -> integer
add_one <- function(count) count + 1L
```

The other order does not work, and says so: a roxygen line between the annotation and the
definition breaks the attachment, and the `#:` block reports that it is dangling. The two
notations are complementary rather than redundant — roxygen documents for humans and
generates `.Rd`, `#:` states the types — so their `@param` entries are written separately
and neither validates the other.

## Naming shapes: aliases and nominal types

Structural types describe shape, but repeating `list{name: character, age: double}`
everywhere is noise, and sometimes two identically-shaped values should *not* be
interchangeable. Two declaration forms cover this:

### `@alias` — a name for a shape

```r
#: @alias PersonShape {list{ name: character, age: double }}

#: PersonShape
ada <- list(name = "Ada", age = 36)
```

An alias is purely structural: `PersonShape` and the written-out list type are the
same type. Aliases can be generic: `#: @alias Box<T> {list{ value: T }}`, used as
`Box<integer>`.

### `@type` — a distinct (nominal) type

```r
#: @type Person {list{ name: character, age: double }}

#: @new Person
ada <- list(name = "Ada", age = 36)
```

`@type` declares a **nominal** type: values only have it where you introduce it with
`@new`, and another `@type` with the same shape is a *different* type. Use it for
domain concepts — `Meters` and `Seconds` can both wrap `double`, and passing one where
the other is declared is an error:

```r
#: fn(m: Meters) -> Meters
scale_up <- function(m) m

scale_up(elapsed)   # error: expected `Meters`, found `Seconds`
```

The `@new` introduction checks the value against the declared representation, so a
nominal type is a checked claim, not a cast.

**Where the separation holds.** It holds wherever a type is written down: annotations,
parameters, returns, record fields. It does *not* hold under arithmetic and comparison —
a nominal with a representation projects to that representation for operators, so
`distance + elapsed` is accepted and yields `double`. If you want operator-level
separation too, declare the class's
[operator methods](/typing/reference#arithmetic-operators): once `Arith.Meters` exists,
only the pairings it names are allowed and `Meters + Seconds` is an error.

Nominal types can be generic too (`@type Pair<T, U> {...}`, introduced with
`@new Pair<integer, character>`), and type arguments are checked with the variance
the representation implies.

## Generic functions

A `<T>` binder makes a signature polymorphic:

```r
#: <T> fn(value: T) -> T
identity <- function(value) value
```

Inside the body, `T` is opaque — the body must work *for every* `T`, so it cannot
add requirements the signature does not state (using `value + 1L` above would be an
error: the signature never promised `T` is numeric). At call sites `T` is filled in
per call: `identity(1L)` is `integer`, `identity("a")` is `character`.

Two bounds refine what a type parameter accepts. They arise on their own from *how*
the parameter is used, and you can also state them directly in the binder
(`<T: numeric>`, or `@forall T: numeric` in the expanded style):

- **numeric** — inferred for unannotated parameters used arithmetically
  (`<T: numeric> fn(x: T) -> T` for `function(x) x + 1L`). Only `integer` and
  `double` (scalar or vector) satisfy it. Write it yourself when the constraint is
  part of the interface rather than an accident of the body.
- **atomic** — using a parameter as a vector *element* type, `T[]`, restricts `T`
  to the six atomic types (writable as `<T: atomic>`). This is how
  element-preserving signatures are written:

```r
#: <T> fn(x: T[]) -> T[]
shuffle <- function(x) sample(x)

shuffle(c(1L, 2L))    # integer[]
shuffle(c("a", "b"))  # character[]
shuffle(list(1L))     # error: a list is not an atomic vector
```

A parameter that picks up both bounds — a generic vector element used
arithmetically — is a scalar `integer`-or-`double` and renders as
`<T: scalar numeric>`.

## The standard library

Base R functions have no annotations to read, so Roughly ships **stub
declarations** for `base`, `stats`, `utils`, `methods`, `graphics`, and
`grDevices` — declaration-only `.Rtypes` files using the same type notation as `#:`
comments. That is why `length(x)` is `integer` and `paste(...)` is `character` out
of the box, and why hovering a base name shows a real signature plus its origin
package.

Three things are useful to know:

- **Type-preserving functions are typed faithfully.** `sum(1L, 2L)` is `integer`
  while `sum(0.5, 1)` is `double` (overloaded per atomic family), and
  `sort(c("b", "a"))` is `character[]` (generic `T[]` signatures). You do not pay a
  precision penalty for using base R.
- **`pkg::name` works.** A qualified read has the same type as the bare name; an
  unknown package or a name the package does not export gets a warning.
- **Projects can override, extend — or type whole packages.** Drop `.Rtypes` files
  under `<project>/stubs/` — a declaration there replaces the shipped one of the
  same name, and the file's name declares a namespace: `stubs/dplyr.Rtypes` makes
  `dplyr::mutate` a known, typed read. Repeating a name declares an ordered
  overload set. See [Stdlib Stubs](/stdlib-stubs) for the format and rules.

## Strict mode

Everything so far reports what the checker *knows* is wrong. Strict mode surfaces
what it *could not check*: each place a type genuinely became `Unknown` — an
unsupported construct, an unresolved reference — gets a `strict` diagnostic at its
origin (one per cause, not one per use).

```toml
[check]
strict = true
```

Strict also escalates **unresolved references** — a name the resolver cannot find, an unknown
`pkg::` namespace, a name a package does not export — from warnings to errors: under strict, a name
the checker cannot see is a hole in the checked surface.

Strict is a per-file conversation, too: a `#: @strict` comment at the top of a file
opts that file in regardless of the config, and `#: @strict off` opts it out. That
makes it practical to hold your typed core to the strict bar while legacy files
migrate gradually.

Strict never flags `Any` — opting out is a choice, and the checker respects it.

## How the checker thinks

A short mental model that explains most behavior you will see:

- **Inference is Hindley-Milner.** Unannotated code gets principal types via
  unification; functions generalize at bindings, so helpers stay polymorphic.
- **Checking against expectations is compatibility, not equality.** The safe
  coercions (scalar→vector, `integer`→`double`, `T`→`T | NULL`) apply where a value
  meets a declared expectation. Compatibility checks are probes: a failed check
  leaves no trace behind.
- **Sound over complete.** The checker prefers refusing a construct loudly (and
  saying so under strict) to silently mistyping it. R constructs that resist static
  typing — non-standard evaluation, reflective environment tricks — degrade to
  `Unknown` rather than to wrong answers.

## Where to go next

- [Typing Reference](/typing/reference) — the authoritative semantics: every type
  form, operator rule, coercion, and annotation, precisely specified.
- [Stdlib Stubs](/stdlib-stubs) — how the standard library is typed and how to
  override it per project.
- [Configuration](/configuration) — the `[check]` keys (`typing`, `strict`,
  `unused`) and everything else in `roughly.toml`.
