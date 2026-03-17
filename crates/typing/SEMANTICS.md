# Semantics

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

- the annotated value must be compatible with `TYPE`
- checking is compatibility-based, not exact-equality-based
- checked annotations may therefore allow widening where the semantics explicitly define it
- if the annotation succeeds, the value is accepted through coercion when needed, and the annotated binding or expression is then treated as having type `TYPE`

Example:

```r
#: list[integer]
value <- list(1L, 2L, 3L)
```

This is valid because `list{integer, integer, integer}` is compatible with `list[integer]`.

### Unknown-only assertions

`#:? TYPE` is an unknown-only assertion.

- it is allowed only when the inferred type is `Unknown`
- if the checker already knows the source type, using `#:?` is an error, even if the asserted type matches that known type
- if the assertion is allowed, the annotated binding or expression is then treated as having type `TYPE`

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

`#:? TYPE` is intended for filling in inference gaps without overriding known information when the checker has no better type than `Unknown`.

### Trusted assertions

`#:! TYPE` is a trusted type assertion.

- it tells the checker to treat the annotated value as `TYPE` without requiring ordinary compatibility at that annotation site
- this is the “trust me bro” escape hatch
- it is similar in spirit to TypeScript’s `as`
- conceptually, `#:! TYPE` is like asserting through `Any` and then to `TYPE`, but written directly because that is more ergonomic

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

#### Vector coercions

- scalar-like vectors `T` can coerce to array-like vectors `T[]`
- map-like vectors `T[named]` can coerce to array-like vectors `T[]`
- reverse coercions are not allowed unless explicitly stated by another rule

Whether a coercion changes the resulting type depends on the construct using it.

### List shapes

List types currently appear in four user-facing forms:

- tuple-like, rendered as `list{T1, T2, ...}`
- record-like, rendered as `list{name: T, ...}`
- array-like, rendered as `list[T]`
- map-like, rendered as `list[named: T]`

R uses `list(...)` for several different collection meanings, and the type system needs to distinguish them.

Tuple-like and record-like lists are fixed-shape collections where positions or field names are part of the type. Array-like and map-like lists are homogeneous collections where every element has the same type, and the specific position or name is not part of the type.

| Shape | Fixed size | Homogeneous | Names or positions meaningful in the type |
| --- | --- | --- | --- |
| `list{T1, T2, ...}` | yes | no | positions |
| `list{name: T, ...}` | yes | no | names |
| `list[T]` | no | yes | no |
| `list[named: T]` | no | yes | no |

`list(...)` expressions may correspond to any of these meanings. For now, the checker defaults to the fixed-shape forms when it has enough information:

- tuple-like: when all elements are unnamed
- record-like: when all elements are named
- Mixing named and unnamed elements is a type error.

Array-like and map-like list types are primarily produced by annotations or by coercing structural list shapes.


#### Current default and open design question

For now, `list(...)` defaults to tuple-like or record-like inference where possible, even when a homogeneous array-like or map-like interpretation might also make sense.

Examples:

- `list(1L, 2L, 3L)` currently infers as `list{integer, integer, integer}`, not `list[integer]`
- `list(foo = 1L, bar = 2L)` currently infers as `list{foo: integer, bar: integer}`, not `list[named: integer]`

This is not set in stone. If this default turns out to be awkward in practice, it may be reasonable to introduce distinct tuple and record constructors later, even if they remain runtime aliases of R lists.

#### List coercions

- tuple-like lists can coerce to array-like `list[T]` when each tuple element is compatible with `T`
- record-like lists can coerce to array-like `list[T]` when each field value is compatible with `T`
- map-like lists can coerce to array-like `list[T]` when each field value is compatible with `T`
- record-like lists can coerce to map-like `list[named: T]` when each field value is compatible with `T`
- map-like lists can coerce to map-like `list[named: T]` when each field value is compatible with `T`
- reverse coercions are not allowed:
  - array-like `list[T]` values do not coerce back into tuple-like, record-like, or map-like values
  - map-like `list[named: T]` values do not coerce back into fixed-shape record-like values

#### Tuple-like lists

A `list(...)` expression with only unnamed elements infers as tuple-like, even when all element types are the same.

Examples:

- `list()` infers as `list{}`
- `list(1L, 2L, 3L)` infers as `list{integer, integer, integer}`
- `list(1L, "foo")` infers as `list{integer, character}`

#### Record-like lists

A `list(...)` expression with only named elements infers as record-like when the element names are known statically.

Examples:

- `list(foo = 1L, bar = "foo")` infers as `list{foo: integer, bar: character}`

#### Array-like lists

An array-like list `list[T]` represents a list whose elements all share a common element type `T`. Array-like lists do not have fixed positional semantics and do not require element names to be statically known. They are normally introduced via annotations or by coercion from tuple-like, record-like, or map-like shapes when all values are compatible with `T`.

#### Map-like lists

A map-like list `list[named: T]` represents a name-keyed collection whose values all share a common value type `T`. Map-like lists do not require the set of names to be statically known and are typically produced by annotations or by coercion from structural list shapes whose element names are not statically available.

#### Mixed named and unnamed lists

All elements in `list(...)` must be either all named or all unnamed.

Example:

- `list(1L, bar = "foo")` is a type error

### `NULL`

- the R literal `NULL` has type `NULL`
- `NULL` is the default unit type in this type system
- `NULL` is incompatible with every other type

Examples:

- `NULL` infers as `NULL`
- empty blocks infer as `NULL`

### `Any` and `Unknown`

#### `Any`

- `Any` is the explicit escape hatch from static type checking
- every type is compatible with `Any`
- `Any` is compatible with every type
- `Any` should appear only because the user explicitly wrote it

#### `Unknown`

- `Unknown` means the checker could not infer a more specific type
- `Unknown` may arise from unsupported constructs, unresolved names, partially supported constructs, or insufficient type information
- `Unknown` is only compatible with `Any`
- `Unknown` is not compatible with ordinary concrete types
- `Unknown` is not an explicit escape hatch
- `Unknown` should remain visible in user-facing output and fixture expectations
- `Unknown` is used to preserve progress and reduce cascading secondary diagnostics

### `Never`

- `Never` has no values
- it represents expressions that do not return normally
- `Never` is compatible with every type
- it is useful for non-returning constructs and calls
- it is not important to implement `Never` in v1

### Union types

For now, the only supported union form is a nullable union with `NULL`.

- `T | NULL` and `NULL | T` mean the same thing
- this is the nullable form of `T`, but for now it remains explicit in the surface syntax rather than being treated as implicit nullability
- nullable union syntax is allowed anywhere a type can appear, including:
  - variable annotations
  - function parameters
  - function returns
  - compact function type annotations
  - nested function types
  - list and map-like list annotations
- only nullable unions are allowed for now

Examples:

- `integer | NULL`
- `NULL | integer`
- `character[] | NULL`
- `fn(count: integer | NULL) -> character | NULL`

Not allowed:

- `integer | character`
- `integer | character | NULL`
- any union with more than one non-`NULL` member
- writing `NULL | NULL` in user-facing type syntax

`NULL | NULL` is not valid user-facing syntax, even though it remains a relevant internal edge case for implementation.

### Nullable union compatibility

- `T` is compatible with `T | NULL`
- `NULL` is compatible with `T | NULL`
- `T | NULL` is not compatible with plain `T`
- nested nullable unions collapse internally
- for example, `(T | NULL) | NULL` normalizes to `T | NULL`, and `NULL | NULL` normalizes internally to `NULL`

## Operators

### `if` expressions

#### `if` without `else`

- requires a scalar `logical` condition
- infers the branch body as type `T`
- produces the result type `T | NULL`
- if the branch body already has type `NULL`, the result normalizes to `NULL`

Examples:

- `if (flag) 1L` infers as `integer | NULL`
- `if (flag) { }` infers as `NULL`

#### `if ... else`

- requires a scalar `logical` condition
- requires both branches to have the same type, unless one branch is `NULL`
- returns the shared branch type when both branches match exactly
- returns `T | NULL` when one branch has type `T` and the other has type `NULL`
- does not use any additional coercion beyond the nullable-union rule above

Examples:

- `if (flag) 1L else 2L` infers as `integer`
- `if (flag) 1L else NULL` infers as `integer | NULL`
- `if (flag) NULL else 2L` infers as `integer | NULL`
- `if (flag) { } else { }` infers as `NULL`

It is a type error when the branches do not match and neither branch is `NULL`.

Examples:

- `if (flag) 1L else \"foo\"` is a type error
- `if (c(TRUE, FALSE)) 1L else 2L` is invalid because the condition is not scalar `logical`

### Blocks

- a block evaluates to the type of its last expression
- if a block has no contents, it evaluates to `NULL`
- if the last expression is terminated with `;`, the block evaluates to `NULL`
- if the last expression has type `Unknown`, the block evaluates to `Unknown`

### Name references

- a name reference evaluates to the type currently bound to that name
- if the name is not bound, the checker reports an unknown-name diagnostic
- after an unknown-name diagnostic, the reference expression is treated as `Unknown` so checking can continue without cascading secondary type errors

### Function calls

- a function call evaluates to the callee's return type
- if the callee expression is `Unknown`, the call evaluates to `Unknown`
- if the callee's return type is `Unknown`, the call evaluates to `Unknown`
- function calls also follow the named, positional, and optional parameter rules defined under `Function types`

A function call is a type error when:

- required arguments are missing
- too many arguments are provided
- an argument value is incompatible with the corresponding parameter type

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

- for array-like `list[T]`, `[[` returns `T`
- for map-like `list[named: T]`, name-based `[[` returns `T | NULL`

For tuple-like lists, positional `[[` is allowed only when the index is known statically as a literal position.

- if the literal position exists, the result is that element's type
- if the position is not known statically as a literal, the access is a type error

For fixed-shape record-like lists, name-based `[[` is allowed only when the field name is known statically as a literal name.

- if the literal field exists, the result is that field's type
- if the field name is not known statically as a literal, the access is a type error
- if a literal field name is known statically and does not exist, the access is a type error

Runtime indexing failures are not modeled by the type system.

#### `[` on vectors

`[` on vectors is not currently part of the supported operator semantics.

In particular, this document does not currently define `[` for scalar-like, array-like, or map-like vectors.

Use `[[` for supported vector indexing instead.

#### `[` on lists

`[` is currently defined only for array-like and map-like list shapes.

Tuple-like, record-like, or map-like list values may be used with `[` only when they can coerce to an array-like or map-like list shape.

- for array-like `list[T]`, `[` returns `list[T]`
- for map-like `list[named: T]`, `[` returns `list[named: T]`

When `[` accepts a tuple-like, record-like, or map-like list through coercion, the resulting type is the array-like or map-like list type produced by that coercion.

Some indexing forms remain unsupported for now. In particular, this document does not currently define `[` on vectors, and tuple-like or fixed-shape record-like `[[` access requires statically known literal indices or names.

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

- `name <- expr` binds `name` to the type of `expr` in the current scope
- if the assignment has an attached typing annotation, the assigned expression is checked using the annotation rules from this document
- the assignment expression itself has the type of the assigned expression
- later assignments in the same scope rebind the name
- the new binding uses the new assigned type

Examples:

- after `x <- 1L`, `x` has type `integer`
- after `x <- 1L; x <- "foo"`, later uses of `x` have type `character`
- `y <- (x <- 1L)` gives both `x` and `y` type `integer`

### Boolean operators `&&` and `||`

- `&&` and `||` are defined only for scalar `logical` operands
- both operands must have type `logical`
- the result type is scalar `logical`
- array-like and map-like logical vectors are not accepted

Examples:

- `TRUE && FALSE` returns `logical`
- `flag || other_flag` returns `logical`
- `c(TRUE, FALSE) && TRUE` is a type error
- `TRUE || c(FALSE, TRUE)` is a type error

## Loops

`for`, `while`, and `repeat` all evaluate to `NULL`.

### `for`

- has the form `for (name in value) body`
- requires an iteration source coercible to array-like iteration
- accepted iteration sources include:
  - values that can coerce to array-like vectors `T[]`
  - tuple-like, record-like, or map-like lists that can coerce to array-like `list[T]`
- only checks whether the iteration source can be coerced to the required shape
- does not itself change the type of the iterated value outside the loop
- inside the loop body, the bound name has the iterated element type `T`

### `while`

- requires a scalar `logical` condition
- the whole `while` expression evaluates to `NULL`

### `repeat`

- has no condition
- currently evaluates to `NULL`
- in the future, it may infer as `Never` when the checker can infer that the loop body does not contain a `break`

## Function types

Function annotations use only `#:` comments.

A function may be annotated in exactly one of these two styles:

- expanded style with `@param` and `@return` or `@returns`
- compact style with a single `fn(...)` annotation, with an optional `-> RETURN_TYPE`

It is not allowed to mix these two styles for the same function.

### Expanded function annotations

Expanded function annotations use these forms:

- `@param {TYPE} name`
- `@param {TYPE} [name]` for optional parameters
- `@return {TYPE}`
- `@returns {TYPE}`

Additional rules:

- the bracket syntax for optional parameters follows JSDoc-style notation
- if no `@return` or `@returns` annotation is provided, the function type defaults to returning `NULL`

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

Additional rule:

- if the return type is omitted, it is implicitly `NULL`

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

- function types may appear inside other function types

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

## Unsupported constructs

- when the checker encounters a syntactically valid construct that is not yet supported, the construct may infer as `Unknown`
- this allows checking to continue even when the checker cannot model the construct precisely
- whether an unsupported construct also produces a diagnostic is a construct-specific decision
