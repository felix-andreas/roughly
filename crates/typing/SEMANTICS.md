# `typing` Semantics

This document is the user-facing semantics contract for the `typing` crate.

Over time, semantic content should move here from older design documents. Until that migration is complete, keep this document focused, high signal, and authoritative for the areas it covers.

All changes to this document must be discussed with the user first.

This document and the fixture suites are both part of the contract. Keep them in sync:

- `tests/fixtures/diagnostics/`
- `tests/fixtures/inference/`

If semantics change, update this document and the relevant fixture expectations in the same session.

## Scope and authority

This document is the single source of truth for user-facing typing semantics.

Use it for:

- type syntax
- inferred type shapes
- coercion rules
- user-facing rendered type forms that appear in fixtures

Other crate documents may summarize or reference these rules, but they should not redefine them.

When semantics are unclear or missing:

1. discuss them with the user first
2. record the agreed behavior here
3. keep the fixtures aligned with the agreed behavior

The main motivation for the current restricted union feature is to support `if` expressions without an `else` branch.

## Typing comment syntax

Typing annotations use preceding `#:` comments attached to the following binding or expression.

This applies to all typing annotations in this crate, not only function annotations.

There are three annotation forms:

- `#: TYPE`
  - checked annotation
- `#:? TYPE`
  - unknown-only assertion
- `#:! TYPE`
  - trusted assertion

Examples:

```r
#: integer
value <- 1L
```

```r
#: list[integer]
value <- list(1L, 2L, 3L)
```

```r
#: fn(count: integer) -> integer
double_count <- function(count) count + count
```

## Type annotations and assertions

### Checked annotations

`#: TYPE` is a checked annotation.

The annotated value must be compatible with `TYPE`.

This is compatibility-based, not exact-equality-based. Checked annotations may therefore allow widening where the semantics explicitly define it.

If the annotation succeeds, the value is accepted through coercion when needed, and the annotated binding or expression is then treated as having type `TYPE`.

Example:

```r
#: list[integer]
value <- list(1L, 2L, 3L)
```

This is valid because `{integer, integer, integer}` is compatible with `list[integer]`.

### Unknown-only assertions

`#:? TYPE` is an unknown-only assertion.

It is allowed only when the inferred type is `Unknown`.

If the checker already knows the source type, using `#:?` is an error, even if the asserted type matches that known type.

Examples:

```r
#:? integer
value <- unsupported_value
```

This is valid only if `unsupported_value` has inferred type `Unknown`.

```r
#:? integer
value <- 1L
```

This is an error because the checker already knows the type.

`#:? TYPE` is intended for filling in inference gaps without overriding known information.

### Trusted assertions

`#:! TYPE` is a trusted type assertion.

It tells the checker to treat the annotated value as `TYPE` without requiring ordinary compatibility at that annotation site.

This is the “trust me bro” escape hatch. It is similar in spirit to TypeScript’s `as`.

Conceptually, `#:! TYPE` is like asserting through `Any` and then to `TYPE`, but written directly because that is more ergonomic.

Examples:

```r
#:! integer
value <- external_input
```

```r
#:! fn(count: integer) -> character
render_count <- callback
```

Trusted assertions can hide real mistakes and should be used only when the programmer knows more than the checker.

## Types

### Atomic names

Use original R type names in semantics and fixtures:

- `logical`
- `integer`
- `double`
- `complex`
- `character`
- `raw`
- `NULL`

Do not rename them to aliases like `bool`, `int`, `float`, or `string`.

### Vector shapes

Atomic vector types have three user-facing shapes:

- scalar-like
- array-like
- map-like

#### Scalar-like vectors

A bare atomic type name means a scalar-like value.

Examples:

- `character`
- `integer`
- `double`

#### Array-like vectors

Appending `[]` means an array-like vector.

Examples:

- `character[]`
- `integer[]`
- `double[]`

#### Map-like vectors

Appending `[named]` means a map-like vector keyed by names.

Examples:

- `character[named]`
- `integer[named]`
- `double[named]`

Vector coercions:

- scalar-like vectors `T` can coerce to array-like vectors `T[]`
- map-like vectors `T[named]` can coerce to array-like vectors `T[]`
- reverse coercions are not allowed unless explicitly stated by another rule

Whether a coercion changes the resulting type depends on the construct using it.

## `NULL`

The R literal `NULL` has type `NULL`.

`NULL` is the default unit type in this type system.

Examples:

- `NULL` infers as `NULL`
- empty blocks infer as `NULL`

`NULL` is incompatible with every other type.

## `Any` and `Unknown`

### `Any`

`Any` is the explicit escape hatch from static type checking.

Every type is compatible with `Any`, and `Any` is compatible with every type.

`Any` should appear only because the user explicitly wrote it.

### `Unknown`

`Unknown` means the checker could not infer a more specific type.

`Unknown` may arise from unsupported constructs, partially supported constructs, or insufficient type information.

`Unknown` is only compatible with `Any`.

`Unknown` is not compatible with ordinary concrete types, and it is not an explicit escape hatch.

`Unknown` should remain visible in user-facing output and fixture expectations.

## List shapes

List types currently appear in four user-facing forms:

- tuple-like, rendered as `{T1, T2, ...}`
- named, rendered as `{name: T, ...}`
- homogeneous, rendered as `list[T]`
- homogeneous named, rendered as `list[key: value]`

`list(...)` expressions infer only the first two structural shapes:

- tuple-like, when all elements are unnamed
- named, when all elements are named

Homogeneous and homogeneous named list types are mainly reached through annotations and coercions rather than direct inference from `list(...)`.

Mixed named and unnamed elements are a type error.

The default inferred type for homogeneous unnamed `list(...)` still needs a deliberate decision.

For now, unnamed `list(...)` continues to infer as tuple-like, even when all element types are the same. This may be awkward for expressions such as `list(1L, 2L, 3L)[1 + 1]`.

List coercions:

- tuple-like lists can coerce to homogeneous `list[T]` when each tuple element is compatible with `T`
- named lists can coerce to homogeneous `list[T]` when each field value is compatible with `T`
- named lists can coerce to homogeneous named `list[key: value]` when each field value is compatible with `value`
- reverse coercions are not allowed:
  - homogeneous `list[T]` values do not coerce back into tuple-like or named values
  - homogeneous named `list[key: value]` values do not coerce back into fixed-shape named values

### Tuple-like lists

A `list(...)` expression with only unnamed elements always infers as tuple-like, even when all element types are the same.

Examples:

- `list()` infers as `{}`
- `list(1L, 2L, 3L)` infers as `{integer, integer, integer}`
- `list(1L, "foo")` infers as `{integer, character}`

This caveat does not change the current semantics. It marks an area that still needs refinement.

### Named lists

A `list(...)` expression with only named elements infers as named.

Examples:

- `list(foo = 1L, bar = "foo")` infers as `{foo: integer, bar: character}`

### Mixed named and unnamed lists

All elements in `list(...)` must be either all named or all unnamed.

Example:

- `list(1L, bar = "foo")` is a type error

## Union types

For now, the only supported union form is a nullable union with `NULL`.

Examples:

- `integer | NULL`
- `NULL | integer`
- `character[] | NULL`
- `fn(count: integer | NULL) -> character | NULL`

`T | NULL` and `NULL | T` mean the same thing.

This is the nullable form of `T`, but for now it remains explicit in the surface syntax rather than being treated as implicit nullability.

Nullable union syntax is allowed anywhere a type can appear, including:

- variable annotations
- function parameters
- function returns
- compact function type annotations
- nested function types
- list and keyed-list annotations

Only nullable unions are allowed for now.

Not allowed:

- `integer | character`
- `integer | character | NULL`
- any union with more than one non-`NULL` member
- writing `NULL | NULL` in user-facing type syntax

`NULL | NULL` is not valid user-facing syntax, even though it remains a relevant internal edge case for implementation.

### Nullable union compatibility

The compatibility rules are:

- `T` is compatible with `T | NULL`
- `NULL` is compatible with `T | NULL`
- `T | NULL` is not compatible with plain `T`

Nested nullable unions collapse internally. For example, `(T | NULL) | NULL` normalizes to `T | NULL`, and `NULL | NULL` normalizes internally to `NULL`.

## Operators

### `if` expressions

#### `if` without `else`

An `if` expression without an `else` branch:

- requires a scalar `logical` condition
- infers the branch body as type `T`
- produces the result type `T | NULL`

Examples:

- `if (flag) 1L` infers as `integer | NULL`
- `if (flag) { }` infers as `NULL`

If the branch body already has type `NULL`, the result normalizes to `NULL`.

#### `if ... else`

An `if ... else` expression:

- requires a scalar `logical` condition
- requires both branches to have the same type, unless one branch is `NULL`
- returns the shared branch type when both branches match exactly
- returns `T | NULL` when one branch has type `T` and the other has type `NULL`

Examples:

- `if (flag) 1L else 2L` infers as `integer`
- `if (flag) 1L else NULL` infers as `integer | NULL`
- `if (flag) NULL else 2L` infers as `integer | NULL`
- `if (flag) { } else { }` infers as `NULL`

This construct does not use any additional coercion beyond the nullable-union rule above.

It is a type error when the branches do not match and neither branch is `NULL`.

Examples:

- `if (flag) 1L else "foo"` is a type error
- `if (flag) c(TRUE, FALSE) else 1L` is invalid because the condition is not scalar `logical`

### Indexing

`[[` is single-element extraction.

`[` is the general subsetting operator in R. In the current supported semantics, it is defined only for certain list forms.

`$name` is syntactic sugar for `[[\"name\"]]`.

Backtick-quoted names follow the same rule.

#### `[[` on vectors

`[[` is allowed on scalar-like, array-like, and map-like vectors and extracts a single element.

- for a scalar-like vector `T`, `[[` returns `T`
- for an array-like vector `T[]`, `[[` returns `T`
- for a map-like vector `T[named]`, name-based `[[` returns `T | NULL`

Runtime indexing failures are not modeled by the type system.

#### `[[` on lists

`[[` is allowed on lists.

- for homogeneous `list[T]`, `[[` returns `T`
- for homogeneous keyed `list[key: value]`, name-based `[[` returns `value | NULL`

For tuple-like lists, positional `[[` is allowed only when the index is known statically as a literal position.

- if the literal position exists, the result is that element's type
- if the position is not known statically as a literal, the access is a type error

For map-like fixed-shape lists, name-based `[[` is allowed only when the field name is known statically as a literal name.

- if the literal field exists, the result is that field's type
- if the field name is not known statically as a literal, the access is a type error
- if a literal field name is known statically and does not exist, the access is a type error

Runtime indexing failures are not modeled by the type system.

#### `[` on vectors

`[` on vectors is not currently part of the supported operator semantics.

In particular, this document does not currently define `[` for scalar-like, array-like, or map-like vectors.

Use `[[` for supported vector indexing instead.

#### `[` on lists

`[` is currently defined only for homogeneous list shapes.

Tuple-like or map-like fixed-shape list values may be used with `[` only when they can coerce to a homogeneous list shape.

- for homogeneous `list[T]`, `[` returns `list[T]`
- for homogeneous keyed `list[key: value]`, `[` returns `list[key: value]`

When `[` accepts a tuple-like or fixed map-like list through coercion, the resulting type is the homogeneous list type produced by that coercion.

Some indexing forms remain unsupported for now. In particular, this document does not currently define `[` on vectors, and tuple-like or fixed map-like `[[` access requires statically known literal indices or names.

### Arithmetic operators

For now, arithmetic operators are defined only for numeric operands:

- `integer`
- `double`

Map-like vectors may participate via compatibility with array-like vectors.

Arithmetic does not preserve map-likeness.

#### Binary `+`, `-`, and `*`

Binary `+`, `-`, and `*` use these rules:

- atomic result:
  - `integer op integer` returns `integer`
  - if either operand is `double`, the result is `double`
- shape result:
  - if both operands are scalar-like, the result is scalar-like
  - otherwise, the result is array-like

Examples:

- `integer + integer` returns `integer`
- `integer - double` returns `double`
- `double * integer[]` returns `double[]`
- `integer[named] + integer` returns `integer[]`

#### Binary `/` and `**`

Binary `/` and `**` use these rules:

- atomic result:
  - always `double`
- shape result:
  - if both operands are scalar-like, the result is scalar-like
  - otherwise, the result is array-like

Examples:

- `integer / integer` returns `double`
- `double ** integer` returns `double`
- `integer[] / integer` returns `double[]`

#### Unary `-`

Unary `-` accepts `integer` and `double`.

Its result rules are:

- atomic result:
  - `-integer` returns `integer`
  - `-double` returns `double`
- shape result:
  - scalar-like and array-like operands keep their shape
  - map-like vectors may participate via compatibility with array-like vectors, and the result is array-like

Examples:

- `-1L` returns `integer`
- `-c(1L, 2L)` returns `integer[]`
- `-c(foo = 1L, bar = 2L)` returns `integer[]`

### Assignment operator `<-`

`name <- expr` binds `name` to the type of `expr` in the current scope.

If the assignment has an attached typing annotation, the assigned expression is checked using the annotation rules from this document.

The assignment expression itself has the type of the assigned expression.

Later assignments in the same scope rebind the name. The new binding uses the new assigned type.

Examples:

- after `x <- 1L`, `x` has type `integer`
- after `x <- 1L; x <- "foo"`, later uses of `x` have type `character`
- `y <- (x <- 1L)` gives both `x` and `y` type `integer`

### Boolean operators `&&` and `||`

`&&` and `||` are defined only for scalar `logical` operands.

Both operands must have type `logical`.

The result type is scalar `logical`.

Array-like and map-like logical vectors are not accepted.

Examples:

- `TRUE && FALSE` returns `logical`
- `flag || other_flag` returns `logical`
- `c(TRUE, FALSE) && TRUE` is a type error
- `TRUE || c(FALSE, TRUE)` is a type error



## Loops

`for`, `while`, and `repeat` all evaluate to `NULL`.

### `for`

A `for` loop has the form `for (name in value) body`.

The iteration source must be coercible to array-like iteration. This includes:

- values that can coerce to array-like vectors `T[]`
- tuple-like or map-like lists that can coerce to homogeneous `list[T]`

`for` only checks whether the iteration source can be coerced to the required shape. It does not itself change the type of the iterated value outside the loop.

Inside the loop body, the bound name has the iterated element type `T`.

### `while`

A `while` loop requires a scalar `logical` condition.

The whole `while` expression evaluates to `NULL`.

### `repeat`

A `repeat` loop has no condition.

The whole `repeat` expression evaluates to `NULL`.

## Function types

Function annotations use only `#:` comments.

A function may be annotated in exactly one of these two styles:

- expanded style with `@param` and `@return` or `@returns`
- compact style with a single `fn(...) -> ...` annotation

It is not allowed to mix these two styles for the same function.

### Expanded function annotations

Expanded function annotations use these forms:

- `@param {TYPE} name`
- `@param {TYPE} [name]` for optional parameters
- `@return {TYPE}`
- `@returns {TYPE}`

The bracket syntax for optional parameters follows JSDoc-style notation.

If no `@return` or `@returns` annotation is provided, the function type defaults to returning `NULL`.

Examples:

```r
#: @param {integer} count
#: @param {character} [label]
#: @return {integer}
double_count <- function(count, label = NULL) { count + count }
```

```r
#: @param {integer} count
log_count <- function(count) { }
```

### Compact function annotations

Compact function annotations use a single function type:

- `fn(name: TYPE) -> RETURN_TYPE`
- `fn(TYPE) -> RETURN_TYPE`
- `fn(name: TYPE, [optional_name]: TYPE) -> RETURN_TYPE`
- `fn(TYPE, [TYPE]) -> RETURN_TYPE`

If no return type is specified, the function type defaults to returning `NULL`.

Examples:

```r
#: fn(count: integer) -> integer
double_count <- function(count) count + count
```

```r
#: fn(count: integer, [label]: character) -> integer
double_count <- function(count, label = NULL) count + count
```

```r
#: fn(count: integer)
log_count <- function(count) { }
```

### Named and positional parameters

Parameter names in function types are part of the call interface.

- named parameters may be called with named arguments
- unnamed parameters are positional only

Example:

- `fn(count: integer) -> integer` allows calling with `count = 1L`
- `fn(integer) -> integer` makes it a type error to call with named arguments

Optional parameters follow the same rule:

- `fn(count: integer, [label]: character) -> integer`
- `fn(integer, [character]) -> integer`

### Higher-order function types

Function types may appear inside other function types.

Examples:

- `fn(transform: fn(integer) -> character) -> character`
- `fn(fn(integer) -> character, integer) -> character`

Expanded annotations may also use function types directly.

Example:

```r
#: @param {fn(integer) -> character} render_count
#: @param {integer} count
#: @return {character}
apply_renderer <- function(render_count, count) { render_count(count) }
```

