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

`list(...)` has two supported structural shapes in the current design:

- tuple-like, when all elements are unnamed
- map-like, when all elements are named

Mixed named and unnamed elements are a type error.

### Tuple-like lists

A `list(...)` expression with only unnamed elements always infers as tuple-like, even when all element types are the same.

Examples:

- `list()` infers as `{}`
- `list(1L, 2L, 3L)` infers as `{integer, integer, integer}`
- `list(1L, "foo")` infers as `{integer, character}`

The empty list is tuple-like:

- `list()` infers as `{}`

### Map-like lists

A `list(...)` expression with only named elements infers as map-like.

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

## `if` expressions

### `if` without `else`

An `if` expression without an `else` branch:

- requires a scalar `logical` condition
- infers the branch body as type `T`
- produces the result type `T | NULL`

Examples:

- `if (flag) 1L` infers as `integer | NULL`
- `if (flag) { }` infers as `NULL`

If the branch body already has type `NULL`, the result normalizes to `NULL`.

### `if ... else`

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

It is a type error when the branches do not match and neither branch is `NULL`.

Examples:

- `if (flag) 1L else "foo"` is a type error
- `if (flag) c(TRUE, FALSE) else 1L` is invalid because the condition is not scalar `logical`

## Coercions

### Tuple-like to homogeneous list

Tuple-like lists can be coerced into homogeneous `list[...]` types when each tuple element is compatible with the target item type.

Example:

```r
#: list[integer]
list(1L, 2L, 3L)
```

This is valid because `{integer, integer, integer}` can be coerced to `list[integer]`.

Homogeneous list values do not coerce back into tuple-like values.

### Map-like list to homogeneous keyed list

Map-like lists can be coerced into homogeneous keyed `list[key: value]` types when each field value is compatible with the target value type.

Example:

```r
#: list[character: integer]
list(foo = 1L, bar = 2L)
```

This is valid because `{foo: integer, bar: integer}` can be coerced to `list[character: integer]`.

Homogeneous keyed list values do not coerce back into fixed-shape map-like values.

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

