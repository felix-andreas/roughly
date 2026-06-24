---
title: Type Checking
description: Roughly's static type checker for R and its typing-comment syntax
---

Roughly includes a static type checker for R. R has no type system of its own, so
Roughly defines one: it infers types for ordinary R code and lets you add optional
annotations to tighten and document those types. Inference is Hindley-Milner style,
so most code is checked without any annotations at all.

Because R has no type-annotation syntax, annotations live in `#:` comments. They look
like comments to every other R tool, so annotated code stays fully compatible with
regular R.

## Status

Type checking is experimental and opt-in. It is off unless you turn it on.

Enable it in `roughly.toml`:

```toml
[check]
typing = true
```

Or pass it as an experimental feature on the command line:

```sh
roughly check --experimental-features typing
```

In the VS Code extension, add `"typing"` (or `"all"`) to `roughly.experimentalFeatures`.
Typed hover information additionally requires the `hovering` feature.

## What it does

When enabled, the checker infers a type for every expression and reports a
`type-error` diagnostic when the types do not line up. A few examples:

```r
#: integer
value <- "hello"
# error[type-error] expected `integer`, found `character`

1L + "a"
# error[type-error] expected a numeric value (`integer` or `double`), found `character`
```

Inferred types flow through bindings, function calls, operators, and indexing, so
mistakes are caught even without annotations:

```r
add <- function(a, b) a + b   # inferred: <T: numeric> fn(a: T, b: T) -> T
add(1L, "x")
# error[type-error] expected a numeric value (`integer` or `double`), found `character`
```

When the checker cannot model a construct it falls back to `Unknown` instead of
erroring, so one unsupported expression does not cascade into noise.

## Typing comments

A typing annotation is one or more `#:` lines attached to the binding or expression
that follows immediately. Consecutive `#:` lines with no blank line between them form
a single annotation block. A blank line between the comment and the expression is an
error.

```r
#: integer
value <- 1L

#: list[integer]
values <- list(1L, 2L, 3L)

#: fn(count: integer) -> integer
double_count <- function(count) count + count
```

There are four annotation forms:

| Form | Meaning |
| --- | --- |
| `#: TYPE` | checked annotation |
| `#: @trust TYPE` | trusted coercion (escape hatch) |
| `#: @if-unknown TYPE` | fill in a type only when inference gave up |
| `#: @new NOMINAL` | introduce a value of a nominal type |

A block may hold exactly one compact annotation, or an expanded function annotation
(`@param`/`@return` lines), or one or more `@type`/`@alias` definitions. These forms
cannot be mixed in the same block.

### Checked annotations

`#: TYPE` checks that the value is compatible with `TYPE`. Checking is
compatibility-based, not exact equality, so it allows the widening coercions defined
below. If the check passes, the binding is then treated as having `TYPE`.

```r
#: list[integer]
value <- list(1L, 2L, 3L)   # ok: list{integer, integer, integer} coerces to list[integer]
```

### Trusted coercions

`#: @trust TYPE` tells the checker to treat the value as `TYPE` without checking
compatibility at that site. It is the "trust me" escape hatch, similar to TypeScript's
`as`. Use it only when you know more than the checker, since it can hide real mistakes.

```r
#: @trust integer
value <- external_input
```

### Unknown-only coercions

`#: @if-unknown TYPE` is allowed only when the inferred type is `Unknown`. It fills in
an inference gap without overriding anything the checker already knows. Using it on a
value whose type is already known is an error.

```r
#: @if-unknown integer
value <- unsupported_value   # ok only if `unsupported_value` is Unknown
```

## Type definitions

`@type` and `@alias` lines define named types. They share one project-global
namespace, forward references are allowed across files, and duplicate names are errors.
A block of only `@type`/`@alias` lines is a definition block and is not attached to the
following expression.

### Aliases

`#: @alias NAME {TYPE}` defines a structural alias. Using the alias is exactly the same
as writing its underlying type; it creates no new type identity.

```r
#: @alias PersonShape {list{ name: character, age: double }}

#: PersonShape
value <- list(name = "bob", age = 20)
```

Aliases may be generic:

```r
#: @alias Box<T> {list{ value: T }}

#: Box<integer>
value <- list(value = 1L)
```

### Nominal types

`#: @type NAME {TYPE}` defines a nominal type with a fresh identity. Two nominal types
are incompatible even if their underlying representation is identical, and an ordinary
structural value is not compatible with a nominal type unless you introduce it with
`@new`.

```r
#: @type Person {list{ name: character, age: double }}

#: @new Person
person <- list(name = "bob", age = 20)
```

`@new` accepts a bare nominal name or a fully-applied generic nominal (`Person<integer>`).
Aliases, unions, function types, and other non-nominal forms are not allowed after `@new`.

A nominal value is compatible with its underlying representation, and operators or
indexing project it down to that representation:

```r
#: @type Person {list{ name: character }}

#: @new Person
person <- list(name = "bob")

person$name   # character: `$` sees the representation type
```

## Function annotations

A function can be annotated in one of two styles. They cannot be mixed for the same
function.

### Compact style

A single `fn(...)` type, with an optional `-> RETURN_TYPE` (omitted means `NULL`):

```r
#: fn(count: integer) -> integer
double_count <- function(count) count + count

#: fn(count: integer, [label]: character) -> integer
double_count <- function(count, label = NULL) count + count

#: <T> fn(value: T) -> T
identity <- function(value) value
```

Named parameters (`name: TYPE`) may be called by name; bare positional parameters
(`fn(integer)`) may not. Optional parameters use bracket syntax and must be named:
`[label]: character`. A leading `<T>` binder introduces type parameters for the whole
function type.

### Expanded style

`@param` and `@return`/`@returns` lines, with an optional leading `@forall`:

```r
#: @param {fn(integer) -> character} render_count
#: @param {integer} count
#: @param {character} [label]
#: @returns {character}
apply_renderer <- function(render_count, count, label = NULL) {
  if (!is.null(label)) paste0(label, ": ", render_count(count)) else render_count(count)
}
```

```r
#: @forall T
#: @param {T} value
#: @return {T}
identity <- function(value) value
```

`@forall` lines must come before `@param`, and `@param` before `@return`. Optional
parameters use the JSDoc-style `[name]` bracket. If no return is given, it defaults to
`NULL`.

### Inferred function types

An unannotated function gets its type from its definition and uses. Every parameter is
named (R parameters always match by name or position), a parameter with a default is
optional, and unconstrained parameters generalize at the binding:

```r
identity <- function(x) x          # <T> fn(x: T) -> T
double_count <- function(x) x + x  # <T: numeric> fn(x: T) -> T
```

## Type vocabulary

### Atomic scalars

Roughly uses R's own type names: `logical`, `integer`, `double`, `complex`,
`character`, `raw`, and `NULL`. A bare name (`integer`) is a scalar-like value.

### Vector shapes

Atomic types have three shapes:

- `T` — scalar-like (e.g. `integer`)
- `T[]` — array-like vector (e.g. `integer[]`)
- `T[named]` — map-like vector keyed by names (e.g. `integer[named]`)

A scalar-like `T` coerces to `T[]`, and a map-like `T[named]` coerces to `T[]`. The
reverse coercions are not allowed.

### List shapes

R uses `list(...)` for several different collection meanings, so the checker
distinguishes four list forms:

| Form | Description |
| --- | --- |
| `list{T1, T2, ...}` | tuple-like: fixed size, positions matter |
| `list{name: T, ...}` | record-like: fixed size, field names matter |
| `list[T]` | array-like: homogeneous, positions not in the type |
| `list[named: T]` | map-like: homogeneous, name-keyed |

`list(...)` infers to a fixed-shape form by default: all-unnamed elements become
tuple-like, all-named become record-like, and mixing named and unnamed elements is an
error.

```r
list(1L, 2L, 3L)           # list{integer, integer, integer}
list(foo = 1L, bar = "x")  # list{foo: integer, bar: character}
```

Fixed-shape lists coerce to the homogeneous forms when every element is compatible with
the element type: tuple-like and record-like lists coerce to `list[T]`, and record-like
lists also coerce to `list[named: T]`. The reverse coercions do not hold.

### `T | NULL`, `Any`, `Unknown`

The only supported union is the nullable union `T | NULL` (equivalently `NULL | T`). It
is the nullable form of `T`: a plain `T` or `NULL` is compatible with `T | NULL`, but a
`T | NULL` is not compatible with plain `T`. Unions of two non-`NULL` members are not
supported.

`Any` is the explicit escape hatch: it is compatible with every type in both
directions, and should appear only when you write it. `Unknown` means the checker could
not infer something more specific; it is compatible only with `Any`, so it does not
silently satisfy concrete annotations.

### Generics

A type can bind type parameters with a leading binder, which is rank-1 (allowed only at
the outermost level):

```
<T> list[T]
<T, U> fn(T) -> U
<T> fn(T) -> T | NULL
```

Generic aliases and nominal types are applied with angle brackets (`Box<integer>`,
`Pair<integer, character>`), and the argument count must match the declaration exactly.

## Operators and indexing

Arithmetic (`+`, `-`, `*`, `/`, `^`, `%%`, `%/%`) is defined for numeric operands
(`integer`, `double`, and numeric-constrained type variables). `+`, `-`, `*`, `%%`, and
`%/%` return `integer` when both operands are `integer` and `double` otherwise; `/` and
`^` always return `double`. The result is scalar-like only when both operands are
scalar-like, and array-like otherwise.

```r
1L + 1L          # integer
1L - 1.5         # double
1L / 2L          # double
c(1L, 2L) * 2L   # integer[]
```

Comparisons (`<`, `<=`, `>`, `>=`, `==`, `!=`) require both operands in the same family
(numeric, `character`, or `logical`) and return `logical`. Boolean `&&` / `||` require
scalar `logical` operands. The range operator `from:to` builds an `integer[]` or
`double[]` sequence from scalar numeric operands, and `c(...)` builds an atomic vector
from atomic arguments.

For indexing, `[[` extracts a single element and `$name` is sugar for `[["name"]]`:

- `[[` on a vector `T`, `T[]` returns `T`; on `T[named]` by name it returns `T | NULL`
- `[[` on `list[T]` returns `T`; on `list[named: T]` by name returns `T | NULL`
- on tuple-like / record-like lists, `[[` needs a statically known literal index or
  field name

`[` is currently defined only for array-like and map-like lists (returning the same
list shape); other `[` forms are not yet modeled.

## Numeric inference variables

An unannotated value used as a numeric operand is constrained to be numeric (`integer`
or `double`) rather than rejected. When such a constraint survives to a function
boundary, it generalizes into a numeric-constrained type parameter, rendered
`<T: numeric>`:

```r
function(x) x + 1L   # <T: numeric> fn(x: T) -> T
function(x) -x       # <T: numeric> fn(x: T) -> T
function(x) x > 0L   # <T: numeric> fn(x: T) -> logical
function(x) x / 2    # <T: numeric> fn(x: T) -> double
```

A numeric-constrained variable that is not abstracted by a parameter defaults to
`double`, matching R's treatment of bare numbers. Calling such a function with a
non-numeric argument is an error at the call site.

## Control flow

- `if` without `else` produces `T | NULL` (or `NULL` if the branch is `NULL`); the
  condition must be scalar `logical`.
- `if ... else` requires both branches to share a type, except that one branch may be
  `NULL`, giving `T | NULL`.
- A block evaluates to the type of its last expression, or `NULL` if empty or
  terminated with `;`.
- `for`, `while`, and `repeat` all evaluate to `NULL`. A `for` loop iterates over any
  value coercible to an array-like vector or `list[T]`, binding the element type inside
  the body.
