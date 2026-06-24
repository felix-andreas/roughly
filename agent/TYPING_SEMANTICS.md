# TYPING_SEMANTICS

This document defines the user-facing typing semantics contract.

Over time, semantic content should move here from older design documents. Until that migration is complete, keep this document focused, high signal, and authoritative for the areas it covers.

Under the current lower-supervision workflow, the agent may decide and record new or changed
semantics here directly, but every such change must also be recorded in `DECISION_LOG.md` with its
rationale, backed by fixtures, and surfaced to the user for review. Prefer the simplest principled
semantics and flag genuinely contentious forks explicitly rather than silently locking them in.

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

Consecutive `#:` lines with no blank line between them are treated as a single annotation block.

Most annotation blocks are attached to the following binding or expression.

A block consisting only of `@type` and `@alias` lines is instead a definition block.

Definition blocks are not attached to the following binding or expression. They only provide a compact way to write several top-level `@type` or `@alias` declarations together.

There are four annotation forms:

- `#: TYPE`
  - checked annotation
- `#: @trust TYPE`
  - trusted coercion
- `#: @if-unknown TYPE`
  - unknown-only coercion
- `#: @new NOMINAL_TYPE`
  - nominal introduction

Additional block rules:

- a block may contain exactly one compact annotation line
- a block may contain an expanded function annotation made of multiple `@param` and `@return` / `@returns` lines
- a block may contain one or more `@type` and `@alias` lines
- compact, expanded, and definition forms cannot be mixed in the same block

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
#: @type Cat {list{ name: character }}
#: @type Dog {list{ name: character }}
```

## Naming and scoping

### Project file order

Project file order follows normal R package collation order.

- if `DESCRIPTION` provides `Collate`, that order is used
- otherwise files are ordered by the default `C`-locale collation of package source files

When this document refers to an earlier or later file, it means earlier or later in that project file order.

### Value names

Top-level value names are package-global across files.

- a top-level binding may be referenced from another file
- if several files define the same top-level value name, the later file wins
- if several package files define the same top-level value name, both the overwritten earlier
  definition and the overwriting later definition should warn

Cross-file references are scheme-based:

- a reference to another file's top-level binding sees that binding's generalized exported type
  scheme
- type information does not flow back into the exporting file through inference; a call in one
  file never changes the inferred type of a function defined in another file
- within one file, top-level names also resolve to the final exported scheme of that name, so a
  use placed before the definition still sees the definition's type

Inside executable code, value naming remains lexical.

- function parameters introduce local bindings
- local assignments introduce local bindings in the current lexical scope
- later assignments in the same scope rebind that name in that scope
- local bindings shadow outer and package-global bindings of the same name
- ordinary braced blocks do not introduce a new value scope by themselves
- `for` introduces a loop-local binding for the iteration variable

### Type names

Top-level `@type` and `@alias` declarations share one project-global namespace.

- type references may resolve to declarations in the same file or in another file
- forward references are allowed
- duplicate type names are errors regardless of file or declaration kind
- every declaration participating in a duplicate-name conflict is erroneous
- type parameters are local binders and shadow project-global type names

All current `@type` and `@alias` declarations are top-level and project-global.

### Non-package documents

Files that are not package source files, such as script-like documents under `scripts/`, do
not contribute to the package-global value or type namespaces.

A script executes top-down, so its top level is one sequential lexical scope, like a function
body:

- a top-level binding is visible only after its assignment
- rebinding a name changes later uses, exactly like local rebinding
- a use before any script-local or package-global definition is an unresolved name

Scripts are typechecked like package files: they check against package-global value schemes and
project-global types, plus their own script-local bindings and type declarations.

- a non-package document may still resolve package-global value names from package files
- a non-package document may still resolve project-global `@type` and `@alias` names from package
  files
- top-level value bindings in a non-package document are not visible to package files or to other
  non-package documents through package-global naming
- top-level `@type` and `@alias` declarations in a non-package document are not visible to package
  files or to other non-package documents through the project-global type namespace
- a package file and a non-package document may reuse the same top-level value or type name without
  a package-global name conflict
- duplicate top-level value names inside a non-package document do not produce the package-global
  duplicate-binding warning; they behave like ordinary script-local rebinding
  - Reasoning: R scripts commonly rely on the global namespace, so warning on top-level rebinding in
    non-package documents would add unnecessary noise outside package-visible naming

### Future direction

The current semantics use one project-global type namespace.

In the future, the language may add file-local opaque types.

A file-local opaque type would:

- be nameable only within its defining file
- be constructible and directly mutable only within its defining file
- remain opaque outside that file except through values and operations the file explicitly exposes

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

### Unknown-only coercions

`#: @if-unknown TYPE` is an unknown-only coercion.

- it is allowed only when the inferred type is `Unknown`
- if the checker already knows the source type, using `#: @if-unknown` is an error, even if the requested type matches that known type
- if the coercion is allowed, the annotated binding or expression is then treated as having type `TYPE`

Examples:

```r
#: @if-unknown integer
value <- unsupported_value
```

This is valid only if `unsupported_value` has inferred type `Unknown`.

```r
#: @if-unknown integer
value <- 1L
```

This is an error because the checker already knows the type.

`#: @if-unknown TYPE` is intended for filling in inference gaps without overriding known information when the checker has no better type than `Unknown`.

### Trusted coercions

`#: @trust TYPE` is a trusted coercion.

- it tells the checker to treat the annotated value as `TYPE` without requiring ordinary compatibility at that annotation site
- this is the “trust me bro” escape hatch
- it is similar in spirit to TypeScript’s `as`
- conceptually, `#: @trust TYPE` is like coercing through `Any` and then to `TYPE`, but written directly because that is more ergonomic

Examples:

```r
#: @trust integer
value <- external_input
```

```r
#: @trust fn(count: integer) -> character
render_count <- callback
```

Trusted coercions can hide real mistakes and should be used only when the programmer knows more than the checker.

### Nominal introduction

`#: @new NOMINAL_TYPE` introduces a nominal value.

- `NOMINAL_TYPE` must be a nominal type reference declared with `@type`
- `NOMINAL_TYPE` may be either a bare nominal name such as `Person` or a generic nominal application such as `Person<integer>`
- aliases, structural types, unions, function types, and other non-nominal type forms are not allowed after `@new`
- generic nominal types must be fully applied, so if `Person<T>` is declared then `@new Person` is an error
- the annotated value must be compatible with that nominal type's underlying representation type
- if the annotation succeeds, the annotated binding or expression is then treated as having type `NOMINAL_TYPE`
- if the annotated value already has type `NOMINAL_TYPE`, the annotation is allowed and has no further effect
- `@new` is an annotation form, not a type expression, so it cannot appear inside compact type syntax or expanded function annotations

Examples:

```r
#: @type Person {list{ name: character, age: double }}

#: @new Person
value <- list(name = "bob", age = 20)
```

```r
#: @type Person<T> {list{ value: T }}

#: @new Person<integer>
value <- list(value = 1L)
```

```r
#: @type Person {list{ name: character, age: double }}

#: Person
value <- list(name = "bob", age = 20)
```

The second example is an error because an ordinary checked annotation for a nominal type requires the value to already be nominally typed as `Person`.

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

### Reserved constants

R's reserved constants infer their fixed scalar atomic type:

- `TRUE` and `FALSE` infer as `logical`
- `NA` infers as `logical`; `NA_integer_`, `NA_real_`, `NA_complex_`, and `NA_character_` infer as
  `integer`, `double`, `complex`, and `character`
- `Inf` and `NaN` infer as `double`
- an imaginary literal such as `1i` infers as `complex`
- `NULL` infers as `NULL`

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

### Type parameters, aliases, and nominal types

Type expressions may bind type parameters with a leading universal binder:

- `<T> TYPE`
- `<T, U, ...> TYPE`

Examples:

- `<T> list[T]`
- `<T> list{ value: T }`
- `<T, U> fn(T) -> U`
- `<T> fn(T) -> T | NULL`

For now, universal binders are rank-1 only.

- a `<...>` binder is allowed only at the outermost level of a user-facing type expression
- nested binders are not allowed inside other type expressions
- higher-rank polymorphism is not supported for now

Examples of not allowed forms:

- `fn(f: <T> fn(T) -> T) -> integer`
- `list{ value: <T> list[T] }`

Named type definitions use `#:` lines with directive syntax.

- `#: @type NAME {TYPE}`
  - defines a nominal type named `NAME` with underlying representation type `TYPE`
- `#: @type NAME<T, U, ...> {TYPE}`
  - defines a generic nominal type named `NAME` with type parameters `T, U, ...`
- `#: @alias NAME {TYPE}`
  - defines a structural alias named `NAME` for `TYPE`
- `#: @alias NAME<T, U, ...> {TYPE}`
  - defines a generic structural alias named `NAME` with type parameters `T, U, ...`

Type and alias definitions share the same namespace.

It is an error if a `@type` or `@alias` definition reuses a name that is already defined by either form.

Consecutive `@type` and `@alias` lines in the same block are allowed and are equivalent to writing them as separate blocks.

Examples:

```r
#: @type Cat {list{ name: character }}
#: @type Dog {list{ name: character }}
```

This is equivalent to:

```r
#: @type Cat {list{ name: character }}

#: @type Dog {list{ name: character }}
```

A definition block cannot mix `@type` or `@alias` lines with ordinary checked annotations, assertions, nominal introduction, or expanded function annotation lines.

Examples of invalid mixed blocks:

```r
#: @type Person {list{ name: character, age: double }}
#: list{ name: character, age: double }
value <- list(name = "bob", age = 20)
```

```r
#: @type Person {list{ name: character, age: double }}
#: @param {Person} value
identity_person <- function(value) value
```

Definitions are project-global rather than block-local.

That means:

- consecutive `@type` and `@alias` lines in one block are still equivalent to separate blocks
- named type references are not limited to earlier lines in the same block
- forward references are allowed across both block and file boundaries

#### Type parameters and generic application

Type parameters may appear inside structural type expressions and function types.

Examples:

- `list[T]`
- `list{ value: T }`
- `fn(T) -> T`
- `T | NULL`

For now, type parameters are not allowed in atomic vector suffix forms.

This is deferred because forms like `T[]` and `T[named]` would imply that `T` is restricted to atomic vector element types. The generic system does not yet model that kind of constraint, so these forms remain disallowed for now instead of implicitly introducing one.

Not allowed:

- `T[]`
- `T[named]`

Named generic aliases and nominal types are applied with angle brackets.

Examples:

- `Box<integer>`
- `Pair<integer, character>`
- `Person<integer>`

The same generic application syntax is used by `@new` when introducing a value of a generic nominal type.

- `#: @new Person<integer>` is valid when `Person<T>` is declared with `@type`
- `#: @new Person` is an error when `Person<T>` is generic and therefore requires type arguments

Type argument counts must match the declared parameter count exactly.

In `@type NAME<T, U, ...> {TYPE}` and `@alias NAME<T, U, ...> {TYPE}`, the declared type parameters are in scope only within `TYPE`.

- `Pair<integer, character>` is valid for `Pair<T, U>`
- `Pair<integer>` is an error for `Pair<T, U>`
- `Pair<integer, character, double>` is an error for `Pair<T, U>`

#### Type aliases

A type alias is purely structural.

- using an alias name in a type annotation is equivalent to writing its underlying type directly
- aliases may appear anywhere an ordinary type expression may appear
- generic aliases may use their type parameters anywhere inside their underlying type expression
- aliases do not create fresh type identity
- aliases are compatible with other types exactly as their underlying type is
- alias definition cycles are errors

Example:

```r
#: @alias PersonShape {list{ name: character, age: double }}

#: PersonShape
value <- list(name = "bob", age = 20)
```

Aliases may also appear inside larger type expressions.

```r
#: @alias Person {list{ name: character, age: double }}

#: list{ owner: Person }
value <- list(owner = list(name = "bob", age = 20))
```

This behaves exactly as if `Person` were replaced with `list{ name: character, age: double }`.

Generic aliases may abstract over structural types.

```r
#: @alias Box<T> {list{ value: T }}

#: Box<integer>
value <- list(value = 1L)
```

#### Nominal types

A nominal type creates a fresh type identity, even when another nominal type has the same underlying representation type.

- a nominal type name may appear anywhere an ordinary type expression may appear
- a generic nominal type may use its type parameters anywhere inside its underlying representation type
- a nominal type is compatible with itself
- two different nominal types are not compatible with each other, even if their representation types are identical
- an ordinary structural value is not compatible with a nominal type unless it is introduced with `@new`
- a value of a nominal type is compatible with its underlying representation type
- when an operator, indexing form, or loop iteration requires a structural shape, a nominal value is projected to its underlying representation type; the projected result is structural, not nominal

Projection examples:

```r
#: @type Person {list{name: character}}

#: @new Person
person <- list(name = "bob")

person$name
```

`person$name` has type `character` because `$` sees the representation type of `Person`.

```r
#: @type Meters {double}

#: @new Meters
height <- 1.8

height + height
```

`height + height` has type `double`; arithmetic projects `Meters` to `double` and the result does not keep the nominal identity.

Examples:

```r
#: @type Person {list{ name: character, age: double }}
#: @type Pet {list{ name: character, age: double }}
```

`Person` and `Pet` are distinct and incompatible nominal types.

```r
#: @type Person {list{ name: character, age: double }}

#: @new Person
person <- list(name = "bob", age = 20)

#: list{ name: character, age: double }
shape <- person
```

This is valid because nominal values are compatible with their underlying representation type.

```r
#: @type Person {list{ name: character, age: double }}

#: fn(value: Person) -> character
get_name <- function(value) value$name
```

Nominal type names may be used in function annotations and nested type expressions.

```r
#: @type Person {list{ name: character, age: double }}

#: fn(value: Person) -> character
get_name <- function(value) value$name

get_name(list(name = "bob", age = 20))
```

This is an error because an ordinary structural value is not compatible with `Person` without `@new`.

Generic nominal types are parameterized on the declared name.

```r
#: @type Person<T> {list{ value: T }}

#: @new Person<integer>
person <- list(value = 1L)

#: list{ value: integer }
shape <- person
```

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

Argument checking is compatibility-based, not exact-equality-based:

- the ordinary coercions defined in this document apply at parameter positions, for example scalar-like `T` into array-like `T[]` and `T` or `NULL` into `T | NULL`
- an argument whose type is `Unknown` is accepted at any parameter; the reason the value became `Unknown` was already diagnosed where it happened, and repeating it at every later use would only cascade noise

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

### Numeric inference variables

An unannotated value used as a numeric operand is constrained to be numeric rather than rejected.
A numeric constraint restricts an inference variable to `integer` or `double` (any vector shape).

- when the constraint reaches a binding boundary still unresolved and abstracted by a function
  parameter, it generalizes into a numeric-constrained type parameter, rendered `<T: numeric>`
- a numeric-constrained variable that escapes a binding without being abstracted by a function
  parameter defaults to `double`, matching R's treatment of bare numbers as doubles
- instantiating a `<T: numeric>` scheme yields a fresh numeric-constrained variable, so calling
  such a function with a non-numeric argument is a type error at the call site
- comparison against a concrete numeric operand also constrains a flexible operand to numeric;
  comparison against a non-numeric family leaves it unconstrained, because the system has no
  character-or-logical constraint

Examples:

- `function(x) x + 1L` infers as `<T: numeric> fn(x: T) -> T`
- `function(x) -x` infers as `<T: numeric> fn(x: T) -> T`
- `function(x) x > 0L` infers as `<T: numeric> fn(x: T) -> logical`
- `function(a, b) a + b` infers as `<T: numeric> fn(a: T, b: T) -> T`
- `function(x) x / 2` infers as `<T: numeric> fn(x: T) -> double`
- calling `(function(x) x + 1L)` with `"oops"` is a type error: `expected a numeric value
  (`integer` or `double`), found `character``

### Arithmetic operators

For now, arithmetic operators are defined only for numeric operands:

- `integer`
- `double`
- inference variables constrained to be numeric (see `Numeric inference variables`)

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

#### Binary `/`, `**`, and `^`

Binary `/`, `**`, and `^` use these rules:

- `^` and `**` are the same operator; `**` is R's parser alias for `^`
- atomic result:
  - always `double`
- shape result:
  - if both operands are scalar-like, the result is scalar-like
  - otherwise, the result is array-like

Examples:

- `integer / integer` returns `double`
- `double ** integer` returns `double`
- `2L ^ 3L` returns `double`
- `integer[] / integer` returns `double[]`

#### Binary `%%` and `%/%`

Modulo `%%` and integer division `%/%` follow the same rules as binary `+`, `-`, and `*`:

- atomic result:
  - `integer op integer` returns `integer`
  - if either operand is `double`, the result is `double`
- shape result:
  - if both operands are scalar-like, the result is scalar-like
  - otherwise, the result is array-like

Other `%op%` special operators are unsupported constructs.

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

### Comparison operators

`<`, `<=`, `>`, `>=`, `==`, and `!=` compare two operands of the same comparison family:

- the comparison families are:
  - numeric: `integer` and `double`, freely mixed
  - `character`
  - `logical`
- both operands must belong to the same family; comparing across families is a type error
- `complex` and `raw` operands are not supported
- map-like vectors participate via compatibility with array-like vectors
- result:
  - atomic result is always `logical`
  - if both operands are scalar-like, the result is scalar-like
  - otherwise, the result is array-like

Examples:

- `1L < 2L` returns `logical`
- `1L == 1.5` returns `logical`
- `"a" < "b"` returns `logical`
- `c(1L, 2L) > 1L` returns `logical[]`
- `1L < "a"` is a type error

### Unary `!`

Logical negation `!` accepts only `logical` operands:

- `!logical` returns `logical`
- `!logical[]` returns `logical[]`
- `!logical[named]` returns `logical[]`; negation does not preserve map-likeness
- any other operand is a type error

### Range operator `:`

`from:to` builds a numeric sequence:

- both operands must be scalar-like `integer` or `double`
- if both operands are `integer`, the result is `integer[]`
- a whole-number `double` literal operand such as `1` or `10` counts as `integer` here, matching R's runtime behavior for `:`
- otherwise, if either operand is `double`, the result is `double[]`
- array-like or non-numeric operands are type errors

Examples:

- `1L:10L` returns `integer[]`
- `1:10` returns `integer[]` even though the literals are `double`, because both are whole-number literals
- `1.5:3L` returns `double[]`
- `x:10L` returns `double[]` when `x` has type `double`

### Combine `c(...)`

`c(...)` builds an atomic vector from scalar-like, array-like, and map-like atomic arguments:

- with no arguments, `c()` returns `NULL`, matching R
- `NULL` arguments are dropped, matching R; `c(x, NULL)` is `c(x)` and `c(NULL)` is `NULL`
- every non-`NULL` argument must be an atomic vector type; lists are not supported
- mixed atomic arguments coerce to the widest type along R's coercion hierarchy
  `logical < integer < double < complex < character`; `raw` does not participate and only combines
  with `raw`
- if every argument is named, the result is map-like `T[named]`
- otherwise the result is array-like `T[]`

Examples:

- `c(1L, 2L)` returns `integer[]`
- `c(1L, 2.5)` returns `double[]`
- `c(TRUE, 1L)` returns `integer[]`
- `c(1L, NA)` returns `integer[]`
- `c(1L, "a")` returns `character[]`
- `c(foo = 1L, bar = 2L)` returns `integer[named]`

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

- expanded style with optional `@forall`, then `@param`, and `@return` or `@returns`
- compact style with a single `fn(...)` annotation, with an optional `-> RETURN_TYPE`

It is not allowed to mix these two styles for the same function.

When function annotations use consecutive `#:` lines, those lines are one annotation block for that function, not separate independent annotations.

### Expanded function annotations

Expanded function annotations use these forms:

- `@forall T,U,...`
- `@forall T`
- `@param {TYPE} name`
- `@param {TYPE} [name]` for optional parameters
- `@return {TYPE}`
- `@returns {TYPE}`

Additional rules:

- repeated `@forall` lines are allowed and accumulate in source order
- duplicate type parameter names in the same annotation block are errors
- `@forall` directives must appear before any `@param`, `@return`, or `@returns` directive
- the bracket syntax for optional parameters follows JSDoc-style notation
- if no `@return` or `@returns` annotation is provided, the function type defaults to returning `NULL`
- at most one `@return` or `@returns` directive may appear in the block
- `@param` directives must appear before `@return` or `@returns`

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

```r
#: @forall T
#: @param {T} value
#: @return {T}
identity <- function(value) value
```

```r
#: @forall T
#: @param {logical} condition
#: @param {T} value
#: @return {T | NULL}
then_some <- function(condition, value) {
  if (condition) value
}
```

```r
#: @forall T
#: @forall U
#: @param {T} left
#: @param {U} right
#: @return {T}
keep_left <- function(left, right) left
```

### Compact function annotations

Compact function annotations use a single function type:

- `fn(name: TYPE) -> RETURN_TYPE`
- `fn(TYPE) -> RETURN_TYPE`
- `fn(name: TYPE, [optional_name]: TYPE) -> RETURN_TYPE`
- `<T> fn(name: TYPE) -> RETURN_TYPE`
- `<T, U, ...> fn(TYPE) -> RETURN_TYPE`

Optional parameters must be named: `[name]: TYPE`. A bare optional positional form like `fn(integer, [character])` is not supported.

Additional rules:

- if the return type is omitted, it is implicitly `NULL`
- when a compact function annotation starts with `<...>`, the binder introduces rank-1 type parameters for the whole function type
- compact function annotations do not use `fn<T>(...)`; the supported binder form is `<T> fn(...) -> ...`

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

```r
#: <T> fn(value: T) -> T
identity <- function(value) value
```

```r
#: <T> fn(condition: logical, value: T) -> T | NULL
then_some <- function(condition, value) {
  if (condition) value
}
```

### Inferred function types

An unannotated `function(...)` expression infers a function type directly from its definition:

- every parameter appears as a named parameter using its definition name, because R parameters are always matchable both by name and by position
- a parameter with a default value is optional at call sites
- parameter and return types are inferred; unconstrained parameters generalize at binding boundaries like any other inferred type
- default value expressions are typechecked: an error inside a default is reported, and a non-`NULL`
  default for an annotated parameter must be compatible with the declared type
- a `NULL` default is R's "no value" sentinel for an optional parameter, so it is always allowed
  regardless of the declared parameter type
- an unannotated parameter's type still comes from its uses, not from its default, so a non-`NULL`
  default does not pin the inferred parameter type

Examples:

- `function(x) x` infers as `<T> fn(x: T) -> T` at a binding boundary
- `function(count, label = NULL) count` may be called as `f(1L)`, `f(count = 1L)`, or `f(1L, "x")`

### Named and positional parameters

Parameter names in function types are part of the call interface.

- named parameters may be called with named arguments
- unnamed parameters are positional only

Example:

- `fn(count: integer) -> integer` allows calling with `count = 1L`
- `fn(integer) -> integer` makes it a type error to call with named arguments

Optional parameters follow the same rule and must be named:

- `fn(count: integer, [label]: character) -> integer`

### Function type compatibility

Parameter names describe the call interface, not the identity of a function type. Two function types match by position across the flattened parameter list:

- `fn(count: integer) -> NULL` and `fn(integer) -> NULL` are mutually compatible
- an annotation `fn(count: integer) -> integer` accepts a function defined as `function(n) n`, and calls through the annotated binding use the annotation's interface
- parameter counts must match
- an expected-optional parameter promises callers they may omit it, so the actual function must have a default for that parameter:
  - `fn(count: integer, [label]: character) -> integer` does not accept `function(count, label) count`
  - `fn(count: integer, label: character) -> integer` accepts `function(count, label = NULL) count`

Function compatibility is contravariant in parameters and covariant in the return type. A function
value is compatible with an expected function type when:

- each expected parameter type is compatible with the corresponding actual parameter type
  (contravariant: the actual function must accept every argument the expected interface may pass)
- the actual return type is compatible with the expected return type (covariant)

Examples:

- a function of type `fn(integer | NULL) -> integer` is accepted where `fn(integer) -> integer` is
  expected, because `integer` is compatible with `integer | NULL`
- a function of type `fn(integer) -> integer` is rejected where `fn(integer | NULL) -> integer` is
  expected, because the expected interface may pass `NULL`, which the actual function does not accept

### Higher-order function types

- function types may appear inside other function types
- rank-1 polymorphism is supported, but higher-rank polymorphism is not

Examples:

- `fn(transform: fn(integer) -> character) -> character`
- `fn(fn(integer) -> character, integer) -> character`

Not allowed:

- `fn(transform: <T> fn(T) -> T, integer) -> integer`
- `fn(fn(value: <T> list[T]) -> integer) -> integer`

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
