---
title: Reference
description: The precise static-typing semantics contract for Roughly's R type checker
---

This is the authoritative specification of Roughly's typing semantics — the precise contract the type checker implements. For a gentler, example-driven introduction, start with the [Type Checker guide](/typing).

This page is the single source of truth for the user-facing typing semantics: the type syntax, the inferred type shapes, the coercion rules, and the rendered type forms that appear in errors and hovers.

## Typing comment syntax

Typing annotations use preceding `#:` comments attached to the following binding or expression.

This applies to all typing annotations in Roughly, not only function annotations.

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
#: @param render_count {fn(integer) -> character}
#: @param count {integer}
#: @param [label] {character}
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
- a bare top-level `{ }` block executes unconditionally, so its direct-child assignments are
  package globals too, exactly like a top-level `name <- value`; assignments inside `if`/`for`/`while`
  bodies are conditionally executed and are not yet package globals (a cross-file reference to such a
  name is unresolved), pending a future conditional-global tier

Cross-file references are scheme-based:

- a reference to another file's top-level binding sees that binding's generalized exported type
  scheme
- type information does not flow back into the exporting file through inference; a call in one
  file never changes the inferred type of a function defined in another file
- within one file, top-level names also resolve to the final exported scheme of that name, so a
  use placed before the definition still sees the definition's type

Inside executable code, value naming is lexical over **mutable variable slots**, matching R's
environment semantics: a scope holds one variable per name, and assignment mutates it.

- a function body, `local(expr)`, and a script's top level each form one variable scope (a frame)
- function parameters introduce a variable slot in the function's frame; assigning to a parameter
  name writes that same slot
- the first `<-`/`=` assignment to a name in a frame creates its variable slot; every later
  assignment to that name in the same frame **writes the same slot** — it does not create a new
  shadowing binding
- an assignment inside a conditional branch or loop body writes the enclosing frame's slot,
  exactly like an unconditional assignment (braces and control flow do not introduce scopes)
- variable slots shadow outer and package-global bindings of the same name; a slot that cannot
  have been assigned yet at a read (no write reaches it on any path) does not shadow — the read
  resolves outward, as R's runtime lookup would
- `for` introduces a loop-local slot for the iteration variable, re-initialized from the iterable
  on every iteration; assigning to the loop variable inside the body writes that slot
- `local(expr)` evaluates `expr` in a fresh child scope and takes its value as the whole expression's
  type (for the common `local({ ... })`, the block's last-expression type); assignments inside are
  local and do not leak to the enclosing scope, while references still see enclosing names. The
  syntactic single-argument `local(...)` call is treated as this construct; rebinding `local` to a
  user function does not change that (a current limitation)
- `library(pkg)`, `require(pkg)`, and `help(topic)` evaluate their first argument non-standardly:
  a bare name there is the package or topic **name** (`library(stats)` means `library("stats")`),
  not a value reference, so it is read as that character literal and never resolved (or warned
  about) as a variable. This applies to the syntactic call to the bare function name whose first
  argument is positional and a bare identifier; a string argument, a named first argument
  (`library(package = pkg)`), or a qualified callee is an ordinary call, and rebinding `library`
  to a user function does not change the quoting (the same limitation as `local`)

At a package document's top level, conditionally executed assignments (inside a top-level `if`,
`for`, `while`, or `repeat`) are not package-visible, but within the same document they behave
like a variable slot: a later top-level read resolves to it, with the maybe-undefined warning
below when an unassigned path also reaches. A conditional reassignment of a name that already has
an unconditional top-level definition keeps resolving to the package-global winner.

### Replacement-form assignment

A replacement-form assignment (`x$field <- v`, `x[["name"]] <- v`, `x[[key]] <- v`, `x@slot <- v`)
reads the base variable, applies the write, and writes the result back to the base's slot, so the
slot's type reflects the update:

- a **known-field** write — `x$field <- v` or `x[["literal"]] <- v` — on a record-like `x` sets
  that field's type to `v`'s type (adding the field if absent); a subsequent `x$field` reads the
  updated type. The same write on an **empty** `list()` starts a record-like `list{field: V}`.
- a **computed-key** write — `x[[key]] <- v` with a non-literal key — cannot name a field
  statically, so it refines the container's element type rather than a specific field:
  - an **empty** `list()` becomes a map-like `list[named: V]`
  - a map-like `list[named: T]` becomes `list[named: T | V]`
  - an array-like `list[T]` becomes `list[T | V]` (stays array-like — its reads are not nullable)
  - a record-like or fixed-shape (tuple-like) container is left unchanged: a dynamic write does
    not statically alter a shape whose fields are individually known, and widening it would lose
    precision the code has not given up
- the accessor spine and index/key expressions are ordinary reads (their own errors surface); a
  replacement whose accessor spine has no variable at its root (`f(x)$a <- v`) is refused as an
  unsupported construct (`Unknown`, a strict-mode origin)

Because a map-like name read is `T | NULL` (the key may be absent — see
[`[[` on lists](#-on-lists)), building a map with computed-key writes and then reading a key back
yields `V | NULL`; guard it (`is.null`) before a use that needs `V`.

### Control-flow joins

A read of a variable sees every write that can reach it, so control flow **joins** the states a
variable can be in:

- after `if` without `else`, a variable written in the branch has the join of its pre-`if` type
  and the branch's written type
- after `if ... else`, a variable has the join of the two branch outcomes (a branch that does not
  write contributes the pre-`if` state)
- a loop body may run zero or more times: reads inside the body and after the loop see the join of
  the pre-loop state and the state flowing around the loop's back edge (the body is re-checked
  until this stabilizes; a variable whose type keeps growing structurally is widened to `Unknown`)
- `repeat` runs at least once, so after the loop the variable has the body's resulting state (back
  edges still join inside the body)
- joining equal types keeps the type; genuinely different types join into their union, exactly as
  `if ... else` result values do; joining with `Unknown` is `Unknown`

Joins and generalization:

- a variable with exactly one reaching write keeps that write's generalized (possibly polymorphic)
  scheme, so `f <- function(x) x` inside a body stays `<T> fn(x: T) -> T`
- when writes merge at a join, the variable holds the join of the written types as a **monotype**
  (a scheme-producing write contributes its instantiated body); conditional reassignment therefore
  monomorphizes

Definite assignment:

- a read some path can reach with **no** prior write to the variable keeps resolving to the
  variable but warns that the name might be undefined (introduced only in conditionally executed
  code)
- a read no write can reach at all does not resolve to the variable (see the shadowing rule above)

Unused (dead-store) analysis follows from the same reaching sets when the `unused` check is
enabled: an assignment whose written value no read can observe on any path warns
``warning[unused] `x` is assigned but never used.`` at the assignment site. Package-visible
top-level assignments, parameters, `for` variables, and `.`/`_`-prefixed names are not reported.

Examples:

- `f <- function(flag) { x <- 1L; if (flag) { x <- 2L }; x }` is clean: both writes reach the
  read, and `x` reads as `integer`
- `f <- function() { total <- 0L; for (i in 1:3) { total <- total + i }; total }` is clean: the
  accumulator write is read on the next iteration and after the loop, and `total` stays `integer`
- `f <- function(flag) { x <- 1L; if (flag) x <- "two"; x + 1L }` is a type error: `x` reads as
  `integer | character`, and `+` rejects the `character` member
- `f <- function() { x <- 1L; x <- 2L; y <- x; y }` warns that the first write to `x` is unused
  (a dead store)

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
- `integer` shapes coerce to the corresponding `double` shapes (`integer` to `double`, `integer[]` to `double[]`, `integer[named]` to `double[named]`, and compositions such as scalar `integer` to `double[]`); the reverse never holds
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

A binder name may carry a constraint, written `NAME: CONSTRAINT`:

- `<T: numeric> fn(values: T) -> T`
- `<T: numeric, U> fn(x: T, y: U) -> T`

Two constraint names are writable:

- `numeric` — the parameter instantiates only to a numeric scalar (`integer`, `double`) or a
  numeric vector (`integer[]`, `double[]`, and their `[named]` forms)
- `atomic` — the parameter instantiates only to one of the six atomic scalar types; the same bound
  that using a parameter as a vector element (`T[]`) imposes

Any other constraint name is an annotation error that names the available constraints. An argument
whose type violates a constraint is a type error at the call that imposed it. A written constraint
composes with positional bounds exactly like an inferred one: `T: numeric` used as a `T[]` element
holds both bounds and instantiates only to a scalar `integer` or `double` (see
[Numeric inference variables](#numeric-inference-variables)).

The constraint works in both directions. It restricts what callers may instantiate `T` to, and it
is a promise the annotated function's own **body may rely on**: with `<T: numeric> fn(x: T) -> T`,
the body may use `x` numerically (`x + 1L`, `x > 0L`) because every admissible instantiation is
numeric. A bound the binder does not declare stays refused — a plain `<T>` body doing arithmetic is
a type error, since the annotation admits non-numeric arguments — and `atomic` does not imply
`numeric`.

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
#: @param value {Person}
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

Type parameters are also allowed in the atomic vector suffix forms:

- `T[]`
- `T[named]`

Using a type parameter as a vector element restricts it: `T` in `T[]` carries the **atomic-element
bound** and can only instantiate to one of the six atomic types (`logical`, `integer`, `double`,
`complex`, `character`, `raw`). This is what makes element-preserving signatures expressible —
`sort : <T> fn(x: T[]) -> T[]` types `sort(c("b", "a"))` as `character[]` and `sort(c(1L))` as
`integer[]`, while `sort(list(1))` is a type error because a list is not an atomic element type.

- a scalar argument coerces into a generic vector parameter and binds the element (`<T> fn(x: T[])`
  called with `2.5` binds `T := double`)
- `[[` on a generic vector `T[]` extracts `T`
- an arithmetic operator over a `T[]` operand additionally requires the element to be numeric (the
  variable then holds both bounds: a scalar `integer` or `double`); the result keeps the element —
  `sort(x) + 1L` is still `T[]` — unless a `double` operand promotes the result to `double[]`
- a comparison over a `T[]` operand yields `logical[]`; a numeric partner constrains the element
  numeric

A bound that can no longer be satisfied — binding an element variable to a non-atomic type, or
requiring a `character` element to be numeric — is a type error at the expression that imposed it.

Writing `X[]` where `X` is neither an atomic type nor a type parameter (a record, a function, a
nominal type) is an annotation error: vectors hold atomic elements only, and the diagnostic points
at the `list[X]` spelling for a list of such values.

Named generic aliases and nominal types are applied with angle brackets.

Examples:

- `Box<integer>`
- `Pair<integer, character>`
- `Person<integer>`

The same generic application syntax is used by `@new` when introducing a value of a generic nominal type.

- `#: @new Person<integer>` is valid when `Person<T>` is declared with `@type`
- `#: @new Person` is an error when `Person<T>` is generic and therefore requires type arguments

Type argument counts must match the declared parameter count exactly.

In `@type NAME<T, U, ...> {TYPE}` and `@alias NAME<T, U, ...> {TYPE}`, the declared type parameters are in scope only within `TYPE`. A type parameter shadows a project-global type of the same name, and it is not a generic: applying type arguments to it (`Wrap<integer>` where `Wrap` is a parameter) is an annotation error rather than a reference to the shadowed global.

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

**Opaque nominal types** have no representation to project. Standard-library stubs declare types
the type grammar cannot describe structurally (`data.frame`, `factor`, `connection`, `Date`, ...)
as bare `@type NAME` — see [Standard library stubs](/stdlib-stubs). For these:

- `$`, `[`, and `[[` are accepted and the result is `Unknown` rather than an error: the R object
  behind such a class commonly supports value-dependent access (`df$amount`, `df[rows, ]`), and
  refusing would reject the most idiomatic R there is
- the access is not checked further — no field-existence, index-count, or index-type checking —
  so `df[i, j]` and `df[rows, ]` both pass
- every such access is an unsupported construct under [strict mode](#strict-mode): the untyped
  result is deliberate and visible, not silent
- all other structural requirements on an opaque nominal (arithmetic, loop iteration, ...) remain
  type errors, and the nominal identity itself still checks exactly like any other nominal type

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

#### Type-argument variance

When two applications of the same generic nominal type are checked for compatibility — for example `Box<integer>` against `Box<integer | NULL>` — each type argument is checked in the direction determined by **where its type parameter occurs** in the representation type. The variance of each parameter is computed from its occurrences:

- a **function return** position, a **container or structural element** position (`list` item, `list{...}` field, tuple item, vector element, and union member), and a **direct** occurrence are *covariant*: they preserve the checking direction, so `Box<integer>` is compatible where `Box<integer | NULL>` is expected (a narrower argument satisfies a wider one);
- a **function parameter** position is *contravariant*: it flips the checking direction, so for `@type Handler<T> {fn(value: T) -> NULL}`, `Handler<integer | NULL>` is compatible where `Handler<integer>` is expected, but `Handler<integer>` is **not** compatible where `Handler<integer | NULL>` is expected (otherwise a `NULL` could reach a function that only accepts `integer`);
- a parameter that occurs in **both** a covariant and a contravariant position is *invariant*: its argument must match exactly in both directions, so `Cell<integer>` and `Cell<integer | NULL>` are mutually incompatible for `@type Cell<T> {list{ get: T, set: fn(value: T) -> NULL }}`;
- a parameter that does not occur constrains nothing and accepts any argument.

When a type parameter occurs inside a **nested generic application** — for example `T` inside `Sink<T>` within `@type Outer<T> {Sink<T>}` — it is treated conservatively as *invariant*, because the inner type's own per-parameter variance is not yet composed with the outer direction. This is sound (it never admits an unsound widening or narrowing); the deferred refinement is to compose the outer polarity with the inner nominal's variance so that sound nested covariant cases are re-admitted.

If a generic nominal has no visible definition, every argument is checked invariantly. This is deliberately conservative: a missing definition over-rejects (requires an exact argument match) rather than over-accepting an unsound widening.

Covariance of container and structural element positions is an explicit assumption: although R lists and vectors are mutable, compatibility treats their element positions covariantly so that `@new`/checked inference and the structural coercions (such as scalar-to-vector and `T` into `T | NULL`) work. This trades the soundness a mutable invariant container would require for the inference ergonomics those coercions depend on.

Unification is the **invariant floor**: when it must produce a single representative type (for example, inferring a type argument shared by two occurrences), it unifies every nominal argument by equality regardless of the parameter's compatibility variance. This is consistent with compatibility, because a unified pair is compatible in both directions — unification is strictly stronger than compatibility.

### Union types

A union type `A | B | ...` describes a value that has one of the member types. Any number of members is allowed, and any type may be a member; `T | NULL` — the nullable form of `T` — is simply the two-member special case.

- union syntax is allowed anywhere a type can appear, including:
  - variable annotations
  - function parameters
  - function returns
  - compact function type annotations
  - nested function types
  - list and map-like list annotations
- a union describes which shapes a value can take; it does not merge or coerce its members
- a type may be **parenthesized for grouping**: `(TYPE)` means exactly `TYPE` and adds no structure
  of its own. Grouping is what makes a union with a *function-type member* writable — in
  `fn() -> integer | NULL` the `->` extends over the whole union (a function returning
  `integer | NULL`), so the optional callback is written `(fn() -> integer) | NULL`, which is also
  the form such a union renders as. A `<T>` binder may not appear inside parentheses; binders stay
  at the outermost level of an annotation

Examples:

- `integer | character`
- `integer | character | NULL`
- `character[] | NULL`
- `integer[] | character[]`
- `fn(count: integer | NULL) -> character | logical | NULL`
- `(fn() -> integer) | NULL` — an optional callback: a function returning `integer`, or `NULL`

Not allowed:

- `NULL | NULL` — a union of only `NULL` members is rejected as invalid type syntax (write `NULL`)

#### Union normalization

Unions are kept in one normal form, so equivalent spellings mean — and render as — the same type:

- **flat**: a union member that is itself a union flattens into the enclosing union; for example an alias expanding to `(A | B) | C` normalizes to `A | B | C`
- **deduplicated**: repeated members collapse, keeping the first occurrence; `integer | character | integer` normalizes to `integer | character`
- **order-insensitive**: member order does not affect meaning; `integer | NULL` and `NULL | integer` are the same type. Rendering preserves first-occurrence order, except that `NULL` always renders last
- **singleton collapse**: a union whose members collapse to a single type is that type; `integer | integer` is `integer`, and a nullable of `NULL` itself normalizes to `NULL`
- **`Any` absorbs**: a union with an `Any` member is `Any` (every value already satisfies `Any`)
- **`Unknown` absorbs**: otherwise, a union with an `Unknown` member is `Unknown` (the union claims no more than "not statically known")

Normalization also applies to unions the checker builds itself (branch joins, alias expansion, `NULL`-producing lookups), so a rendered union is always flat, deduplicated, and at least two members.

### Union compatibility

Compatibility treats a union on the two sides differently:

- **into a union (expected side)**: a value fits an expected union when it fits *any* member
  - `T` is compatible with any union containing `T`; `integer` is compatible with `integer | character | NULL`
  - `NULL` is compatible with any union containing `NULL`
  - the usual coercions apply per member; a value coercible to some member fits the union
- **out of a union (actual side)**: a union value must be accepted in *every* shape it can take, so a union is compatible with an expected type only when each of its members is
  - a union is compatible with any wider union: `integer | NULL` is compatible with `integer | character | NULL`
  - a union is **not** compatible with a plain member type: `integer | character` is not compatible with `integer`, and `T | NULL` is not compatible with plain `T`
- member checks are attempted in member order, and a failed member attempt leaks no inference bindings into the next attempt

### Union unification

Unification (used where two types must become one representative type, such as inferring a shared type argument) is stricter than compatibility — it is the invariant floor:

- an inference variable may be bound *to* a union, exactly like any other type
- two unions unify only when their member sets are equal (member order is presentation, not identity)
- the single member-wise case is the nullable shape: `T | NULL` unifies with `U | NULL` by unifying `T` with `U` when each side has exactly one non-`NULL` member, which is what lets a `<T> ... T | NULL` scheme instantiate against a concrete nullable
- there is no member-matching search inside unification; directional member-wise reasoning lives entirely in compatibility

## Operators

### Operators over union operands

Control-flow joins and heterogeneous containers produce union-typed operands, so every operator
below accepts unions **member-wise**:

- a union operand is accepted where **every** member is accepted; one unacceptable member rejects
  the whole operand (the diagnostic shows the full union type)
- the result is the **join of the per-member results** (for a binary operator, over every pair of
  left and right members)

Examples:

- `(integer | double) + integer` is `integer | double` (each member is numeric; `integer + integer`
  is `integer`, `double + integer` is `double`)
- `(integer | double) > 0L` is `logical`
- `(integer | character) + 1L` is a type error: the `character` member is not numeric
- `(integer | NULL) + 1L` is a type error: the `NULL` member is not numeric
- `rec$a` on `list{a: integer} | list{a: character}` is `integer | character`; the access is an
  error if any member lacks the field
- `for` over `integer[] | character[]` binds the loop variable as `integer | character`

### `if` expressions

#### `if` without `else`

- requires a scalar `logical` condition
- infers the branch body as type `T`
- produces the result type `T | NULL` (the missing branch contributes `NULL` to the join)
- union normalization applies: a `NULL` body stays `NULL`, an already-nullable body stays a single `T | NULL`, and an `Unknown` body stays `Unknown`

Examples:

- `if (flag) 1L` infers as `integer | NULL`
- `if (flag) { }` infers as `NULL`

#### `if ... else`

- requires a scalar `logical` condition
- **joins** the two branch types into the result type:
  - branches that unify share that type: `if (flag) 1L else 2L` is `integer`, and `if (cond) a else b` over two unconstrained values keeps them unified as one polymorphic type
  - a `NULL` branch joins by union without constraining the other branch: one branch `T` and one branch `NULL` produce `T | NULL`
  - branches with genuinely different types produce their union: `if (flag) 1L else "foo"` is `integer | character` — different branch types are **not** a type error
  - an `Unknown` branch makes the whole conditional `Unknown` rather than claiming the other branch's type
- the join does not merge or coerce branch types beyond unification; it only records the alternatives

Examples:

- `if (flag) 1L else 2L` infers as `integer`
- `if (flag) 1L else NULL` infers as `integer | NULL`
- `if (flag) NULL else 2L` infers as `integer | NULL`
- `if (flag) 1L else "foo"` infers as `integer | character`
- `if (flag) { } else { }` infers as `NULL`
- `if (c(TRUE, FALSE)) 1L else 2L` is invalid because the condition is not scalar `logical`

#### Diverging branches

A branch **diverges** when it never falls through to the code after the `if`: it is (or is a block
ending in) `return(...)`, `stop(...)`, `break`, or `next`, or an `if ... else` both of whose
branches diverge. A diverging branch contributes neither its value nor its variable-slot state:

- `x <- if (c) return(NULL) else 5` gives `x` type `double`, not `NULL | double`
- variable writes inside a diverging branch do not join into the state after the `if` — only the
  surviving branch's state flows on

`stop(...)` is recognized by its bare name, like `local` and `return`; rebinding `stop` is not
modeled.

### Guard narrowing

A condition that is a **type-guard predicate applied to a plain local variable** refines that
variable's type along the `if` edges. The variable keeps the refined type inside each branch until
a branch write replaces it, and the refinements merge back at the join exactly like branch writes.

The recognized guards, with `x` a local variable (including parameters):

| condition | true edge | false edge |
|---|---|---|
| `is.null(x)` | `x : NULL` | the `NULL` member is removed from `x`'s union |
| `is.character(x)` | union members that are not `character`-family are removed | `character`-family members are removed |
| `is.logical(x)`, `is.integer(x)`, `is.double(x)`, `is.function(x)`, `is.list(x)` | as above, for that family | as above |
| `is.numeric(x)` | as above, where the family is `integer` **or** `double` | as above |
| `!cond` | the two edges swap | |

Rules and limits:

- a *family* (`is.character`, …) membership test covers the scalar and the vector of the atomic
  (`is.character` is true for `character` and `character[]`); `is.list` covers every list shape
  (`list[T]`, `list[named: T]`, and fixed-shape lists); `is.function` covers function types
- **narrowing filters union members**; a member whose family cannot be decided statically (an
  inference variable, a flexible-element vector, an opaque nominal) is conservatively kept on
  *both* edges
- `is.null(x)` on an `Any` or `Unknown` variable refines the **true** edge to `NULL` (the runtime
  guarantees it); family guards do **not** refine `Any`/`Unknown` — inventing a concrete shape
  there would false-positive against scalar-claim standard-library signatures
- `is.null(x)` on a **completely unconstrained inference variable** (an unannotated, so-far-unused
  parameter) **shapes** it: the test asserts `NULL` is a possible inhabitant, so the variable
  becomes `T | NULL` for a fresh `T`, and the edges then narrow as an ordinary union (true edge
  keeps `NULL` and the undecidable `T`; false edge is `T`). This is what types the unannotated
  coalesce idiom — `function(value, fallback) if (is.null(value)) fallback else value` generalizes
  to `<T> fn(value: T | NULL, fallback: T) -> T`, the same scheme its annotated form declares.
  Two consequences: testing a parameter for `NULL` and then using it *unguarded* is a genuine
  finding (the test itself declared `NULL` possible), and the shaping never fires on a variable
  that already carries a constraint (a numeric-constrained variable cannot hold `NULL`) or on a
  declared (rigid) type parameter — an annotation's contract is not reshaped
- when a guard cannot fire (`is.null(x)` on a union with no `NULL` member), no refinement happens —
  the checker does not type dead branches specially
- combined with a [diverging branch](#diverging-branches), the surviving edge's refinement
  **persists after the `if`** — the idiomatic early-exit guard:

  ```r
  #: fn(x: integer | NULL) -> integer
  f <- function(x) {
    if (is.null(x)) {
      return(0L)
    }
    x + 1L   # x : integer here
  }
  ```

- only reads of **local variable slots** narrow (parameters, function locals, script locals);
  package globals and arbitrary expressions (`is.null(f(x))`, `is.null(x$field)`) do not
- conditions combined with `&&` / `||` are not decomposed yet
- `is.na(x)` is not a type guard: `NA`-ness is a value property, not a type property, in this
  system
- narrowing never touches an unresolved inference variable; an unannotated parameter is not pinned
  by a guard

### Blocks

- a block evaluates to the type of its last expression
- if a block has no contents, it evaluates to `NULL`
- if the last expression is terminated with `;`, the block evaluates to `NULL`
- if the last expression has type `Unknown`, the block evaluates to `Unknown`

### `return`

`return(x)` exits the enclosing function with `x` (`return()` exits with `NULL`). It is a
control-flow construct, not a call: the syntactic call to the bare name `return` is recognized
during lowering, like `local`.

- a function's return type is the **union** of every `return` value's type in its body with the
  body's trailing value: `function() { if (c) return("foo"); 5 }` is `fn() -> character | double`
- the `return` expression itself yields no observable value where it stands, so — like `break` and
  `next` — it types as `NULL` locally and is not a strict origin
- the returned value expression is checked like any other; its errors surface normally
- a `return` inside a loop exits the whole function, so it abandons the loop iteration like `break`
  for control-flow purposes
- a top-level `return` (an R runtime error) still checks its value; it joins no function's return
  type

### `switch`

`switch(subject, a = ..., b = ..., default)` selects one branch by the subject's runtime value.
Selection cannot be modeled statically, but the call is fully checked:

- the subject and **every branch** are type checked; an error inside any branch surfaces like
  anywhere else
- the call's type is the **union of the branch value types**; `NULL` joins the union unless a
  default (unnamed, non-first) branch exists, because an unmatched `switch` returns invisible
  `NULL`
- a named branch with no value falls through to the next branch in R; it contributes no type of
  its own

### Name references

- a name reference evaluates to the type currently bound to that name
- if the name is not bound, the checker reports an unknown-name diagnostic
- after an unknown-name diagnostic, the reference expression is treated as `Unknown` so checking can continue without cascading secondary type errors

### Namespace access

`pkg::name` (and `pkg:::name`) reads one name directly from a package namespace, bypassing
lexical scoping.

- a namespace is **known** when stubs declare it: the shipped standard-library packages, plus any
  project stub file — `stubs/dplyr.Rtypes` declares the namespace `dplyr`
  (see [Standard library stubs](/stdlib-stubs))
- when the stubs declare `name` in `pkg`, the qualified read has the stub's type, exactly like the
  bare name
- an unknown namespace warns (`unknown package namespace `foobar``); a known namespace that does
  not declare the name warns (``bazqux` is not exported by `stats``)
- exports are declaration-level: a project stub overriding a shipped name's *type* does not remove
  the name from its shipped namespace, so `stats::sd` stays valid under an `sd` override
- an unvalidated qualified read types as `Unknown`, and that reference is a strict origin
- `::` and `:::` are not distinguished: the checker does not model the exported/internal split

### Function calls

- a function call evaluates to the callee's return type
- if the callee expression is `Unknown`, the call evaluates to `Unknown`
- if the callee's return type is `Unknown`, the call evaluates to `Unknown`
- if the callee is a **union whose members are all function types** (the dispatch-table idiom:
  `handlers[[name]](...)`), the call must be valid against **every** member — the value could be
  any of them — and evaluates to the union of the member return types; each member is checked in
  an isolated probe, so no member's argument bindings leak into another's
- function calls also follow the named, positional, and optional parameter rules defined under `Function types`

A function call is a type error when:

- required arguments are missing
- too many arguments are provided **and the callee has no rest parameter**
- an argument value is incompatible with the corresponding parameter type

Argument checking is compatibility-based, not exact-equality-based:

- the ordinary coercions defined in this document apply at parameter positions, for example scalar-like `T` into array-like `T[]` and `T` or `NULL` into `T | NULL`
- `integer` is compatible where `double` is expected (scalar-like, array-like, and map-like alike): R freely promotes integers in numeric contexts, so `mean(1L)` and `sd(c(1L, 2L))` are not errors. The widening is directional — `double` is never accepted where `integer` is expected, and unification does not widen
- a **whole-number `double` literal** such as `10` or `3` counts as `integer` at a parameter position — `seq_len(10)` and `substr(x, 1, 3)` are as valid as their `10L`/`1L`/`3L` spellings, generalizing the rule the `:` operator already applies to its endpoints. A fractional literal (`2.5`) and a `double`-typed *variable* holding a whole number are still rejected at an `integer` parameter
- an argument whose type is `Unknown` is accepted at any parameter; the reason the value became `Unknown` was already diagnosed where it happened, and repeating it at every later use would only cascade noise

A **rest parameter** (`...: TYPE`) changes how surplus arguments are handled. Its position in the
signature mirrors the position of `...` in the R formal list, and argument matching follows R's
rule for formals around the dots:

- a rest parameter adds no required arguments, so a variadic function may be called with none (`paste()` is legal)
- a positional argument first fills the unfilled parameters declared **before** the rest parameter,
  in order — exactly as R fills formals before `...` positionally (`wrap("a", "b")` on
  `fn(x: character, ...: character)` gives `x = "a"` and sends `"b"` to the rest)
- once the pre-rest parameters are filled, any number of remaining positional arguments are
  absorbed by the rest parameter, each checked against its element type
- a positional argument never fills a parameter declared **after** the rest parameter; those are
  matched by name only (as in R), so `sum(1, 2, na.rm = TRUE)` with
  `fn(...: integer[] | logical[], [na.rm]: logical)` sends `1` and `2` to the rest and `na.rm` by
  name
- a named argument matching **no declared parameter** is also absorbed by the rest parameter and checked against its element type — R collects unmatched keywords into `...`, the pass-through idiom variadic wrappers rely on (`read.csv(file, colClasses = "character")`)
- a named argument that **duplicates a declared parameter already given** stays a named-parameter error even with a rest parameter (R rejects a formal matched by multiple actual arguments); without a rest parameter, any unmatched named argument is an error as before

### Overload sets

A standard-library stub name may declare **several signatures** (an ordered overload set — see the
[stdlib stubs page](/stdlib-stubs) for the declaration surface). Calls to such a name resolve per
call site:

- candidates are tried **in declaration order**, and the call commits the **first** candidate whose
  parameters accept the arguments; that candidate's return type is the call's type
  (`sum(1L, 2L)` is `integer`, `sum(1.5, 2.5)` is `double`)
- each failed candidate is probed in isolation: nothing it bound leaks into the next candidate or
  into the committed result
- selection needs concrete argument types. When any argument's type is still an undetermined
  inference variable (an unannotated parameter of an enclosing function, for example), selection is
  skipped and the **last** declaration — by corpus convention the most general — is used, so a
  wrapper like `function(x) sum(x)` keeps its parameter unconstrained
- the [whole-number literal rule](#function-calls) does not steer selection: candidates are first
  tried against the arguments' true types (`sum(1, 2)` selects the `double` candidate, matching what
  R computes), and only if no candidate accepts them is the set retried with the literal-as-integer
  courtesy — so a name whose only fitting candidate wants `integer` still accepts `foo(1)`
- when no candidate accepts the arguments, the call is a type error naming the overloaded callee and
  how many signatures were tried, with the first candidate's failure as the concrete hint
- every non-call use of an overloaded name (passing it as a value, hover) sees the **first**
  declaration

Only a plain or namespace-qualified stub name can be overloaded. A local or package binding that
shadows the name disables its overload set — the binding wins everywhere, calls included.

### Indexing

`[[` is single-element extraction.

`[` is the general subsetting operator in R. In the current supported semantics, it is defined only for certain list forms.

`$name` behaves as `[["name"]]` on lists, records, and the tolerated opaque nominals — but **not**
on atomic vectors: R rejects `$` on every atomic vector (`$ operator is invalid for atomic
vectors`), named ones included, so `c(foo = 1L)$foo` is a type error that points at `[[` while
`c(foo = 1L)[["foo"]]` extracts `integer | NULL`.

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
- for map-like `list[named: T]`, name-based `[[` returns `T | NULL`; positional and computed `[[`
  return `T` (runtime indexing failures are not modeled, as for array-like lists)

For tuple-like lists, `[[` with a literal position is precise; a computed position is the union
of the item types.

- if the literal position exists, the result is that element's type
- if a literal position does not exist, the access is a type error
- if the position is not known statically as a literal, the result is the **union of the item
  types** (a computed position could reach any item — the same rule `for` iteration over a
  fixed-shape list uses)

For fixed-shape record-like lists, `[[` with a literal field name or literal position is precise;
a computed index is the union of the field types (record fields are declaration-ordered, so
`x[[1L]]` extracts the first field exactly as R does).

- if the literal field exists, the result is that field's type
- if the literal position exists, the result is that position's field type
- if the index is neither a literal name nor a literal position, the result is the **union of
  the field types** — this is what types the dispatch-table idiom, `handlers[[name]](...)`
- if a literal field name or position does not exist, the access is a type error

Runtime indexing failures are not modeled by the type system.

#### `[` on vectors

`[` on vectors is not currently part of the supported operator semantics.

In particular, this document does not currently define `[` for scalar-like, array-like, or map-like vectors.

Use `[[` for supported vector indexing instead.

#### `[` on lists

`[` slices a list: the result is a sub-list, so the subject's fixed shape does not survive into the result type.

- for array-like `list[T]`, `[` returns `list[T]`
- for map-like `list[named: T]`, `[` returns `list[named: T]`
- for a tuple-like list, `[` returns `list[T]` where `T` is the **union of the item types**; `list(1L, "foo")[1L]` is `list[integer | character]`
- for a record-like list, `[` returns `list[named: T]` where `T` is the union of the field value types
- slicing the empty list yields `list[NULL]` (`T` is the union of zero item types, `NULL`)

For a homogeneous fixed-shape list the union collapses, so the result matches the plain coercion to the array-like or map-like shape.

#### Indexing opaque nominal types

`$`, `[`, and `[[` on an opaque nominal type (`data.frame`, `factor`, ...) yield `Unknown` without
further checking; see [Nominal types](#nominal-types) for the rule and its rationale.

#### Indexing an unresolved inference variable

`$`, `[[`, and `[` on an unresolved inference variable — an unannotated parameter whose shape is
never pinned down, as in `function(node) node$value`, `function(x) x[[1L]]`, or `function(x) x[1L]`
— yield `Unknown` rather than a "not a list" / "unsupported `[`" error, and leave the variable
unconstrained. Reading fields, elements, and slices off a value whose shape the author never wrote
down is how idiomatic R walks recursive and generic data (a tree fold, a generic accessor), so
refusing here would flag ordinary code. The access is instead sound-by-refusal and surfaced as an
unsupported construct under [strict mode](#strict-mode), exactly as for an opaque nominal.
Recovering the field or element type by constraining the variable to a record-with-field or
indexable shape is future work.

Some indexing forms remain unsupported for now. In particular, this document does not currently define `[` on vectors.

### Numeric inference variables

An unannotated value used as a numeric operand is constrained to be numeric rather than rejected.
A numeric constraint restricts an inference variable to `integer` or `double` (any vector shape).

Two other bounds exist alongside it. The **atomic-element** bound restricts a variable to a scalar
atomic type; it is introduced by using a type parameter as a vector element (`T[]` — see
[Type parameters and generic application](#type-parameters-and-generic-application)) and renders
`<T: atomic>`. A variable that acquires both bounds — a generic vector element used arithmetically —
holds their meet, rendered `<T: scalar numeric>`: a scalar `integer` or `double`. It defaults to
`double` at a binding boundary exactly like a plain numeric variable.

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

**Shape of a flexible operand.** An operand whose shape is still an inference variable (an
unannotated parameter) is treated as **scalar-like** in the shape rules below and in the comparison
rules — a deliberate scalar claim, the same compromise the standard-library corpus applies to
elementwise functions: a scalar result coerces into every vector position, so the claim can never
produce a false error downstream, at the cost of not tracking vector-in/vector-out shape through
such a function. The exception is an operand carrying the atomic-element bound (`T[]` — a generic
vector), whose operator results are genuinely vector-shaped (see
[Type parameters and generic application](#type-parameters-and-generic-application)).

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
- an inference-variable operand (an unannotated parameter, `1:n`) acquires the **scalar numeric**
  bound — a scalar `integer` or `double` — so passing a numeric vector through the enclosing
  function is a type error at the call, matching R's endpoint truncation warning being a bug; the
  result is `double[]` since the endpoint may instantiate at `double`

Examples:

- `1L:10L` returns `integer[]`
- `1:10` returns `integer[]` even though the literals are `double`, because both are whole-number literals
- `1.5:3L` returns `double[]`
- `x:10L` returns `double[]` when `x` has type `double`

### Combine `c(...)`

`c(...)` builds an atomic vector from scalar-like, array-like, and map-like atomic arguments:

- with no arguments, `c()` returns `NULL`, matching R
- `NULL` arguments are dropped, matching R; `c(x, NULL)` is `c(x)` and `c(NULL)` is `NULL`
- a union-typed argument participates member-wise: `NULL` members are dropped first (at runtime the
  value is either `NULL` — dropped by `c` — or one of the other members), and every remaining
  member must be an atomic vector type and joins the coercion like a separate argument; an
  accumulator seeded with `NULL` therefore combines cleanly — with `acc` of type
  `double[] | NULL`, `c(acc, 1.0)` is `double[]`
- every non-`NULL` argument must be an atomic vector type; lists are not supported
- a non-concrete argument whose element type is not statically known — `Any`, `Unknown`, or an
  unresolved inference variable (an unannotated parameter, `function(x) c(x, 1L)`) — is tolerated
  rather than rejected: the combined element atomic is indeterminate, so the whole result is
  `Unknown` (a strict-mode origin when the argument is an unresolved variable). This keeps `c` from
  a false "expected `integer`, found `T`" on generic wrappers and from cascading on an
  already-`Unknown` value; claiming a concrete element type would be unsound, since a later argument
  could widen the atomic
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
- `function(x) c(x, 1L)` infers as `fn(x: T) -> Unknown` (the unannotated `x` leaves the element
  atomic indeterminate)

### Assignment operator `<-`

- `name <- expr` writes the type of `expr` into `name`'s variable slot in the current scope,
  creating the slot on the first write (see `Value names` for the slot model)
- if the assignment has an attached typing annotation, the assigned expression is checked using the annotation rules from this document
- the assignment expression itself has the type of the assigned expression
- a later assignment in the same scope writes the same variable: on a straight-line path the new
  write replaces the old type, and writes merging from different control-flow paths join (see
  `Control-flow joins`)
- **recursion (letrec for closures):** a function-valued assignment's own name is visible inside
  its body — the target is pre-bound to a fresh type variable before the body is inferred, and
  the variable unifies with the inferred function type (monomorphic recursion, the classic
  `let rec` rule). `fact <- function(k) if (k <= 1L) 1L else k * fact(k - 1L)` therefore types as
  `fn(integer) -> integer`, and a call violating the recursively-inferred signature is an error.
  Recursion is monomorphic: the recursive uses share one instantiation (no polymorphic
  recursion). Mutual recursion between two *local* closures is out of letrec's per-binding reach —
  the earlier closure's reference to the later name stays a loud unresolved reference — while
  top-level mutual recursion resolves through the package interface fixed point

Examples:

- after `x <- 1L`, `x` has type `integer`
- after `x <- 1L; x <- "foo"`, later uses of `x` have type `character`
- after `x <- 1L; if (flag) x <- "foo"`, later uses of `x` have type `integer | character`
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

Loop bodies are checked to a control-flow fixed point: variables written in the body join across
iterations and with the pre-loop state (see `Control-flow joins`).

### `for`

- has the form `for (name in value) body`
- requires an iterable iteration source:
  - scalar-like, array-like, and map-like vectors iterate with the scalar element type
  - array-like `list[T]` and map-like `list[named: T]` iterate with element type `T`
  - tuple-like and record-like lists iterate with the **union of their item types** (which collapses to the single item type for a homogeneous list), so heterogeneous fixed-shape lists are iterable: `for (item in list(a = 1L, b = "two")) ...` binds `item` as `integer | character`
  - the empty list `list()` is iterable with element type `NULL` (the union of zero item types)
  - a union of iterables iterates member-wise: `integer[] | character[]` binds the loop variable
    as `integer | character`
  - `NULL` is iterable and runs zero iterations (legal R), binding the loop variable as `NULL`
  - `Any` iterates with `Any` items; `Unknown` iterates with `Unknown` items (an already-failed
    source does not produce a second error on the loop)
  - an opaque nominal value iterates with `Any` items (its element shape is not visible to the
    checker)
  - a still-unresolved inference variable (an unannotated parameter, say) is **not constrained**
    by iteration — R iterates vectors and lists, and neither shape may be committed for the
    caller — so the loop variable degrades to `Unknown`
  - any other source (a function, say) is an error reported on the source expression: ``this
    `for` sequence is `fn() -> integer`, which cannot be iterated — expected a vector or list.``
- the iteration source is evaluated once, before any iteration
- does not itself change the type of the iterated value outside the loop
- inside the loop body, the bound name has the iterated element type; it is re-initialized from
  the iterable on every iteration, so an assignment to it inside the body does not survive into
  the next iteration's start
- the loop variable is not visible after the loop

### `while`

- requires a scalar `logical` condition
- the condition is re-evaluated before every iteration, so reads in it also see the loop's joined
  state
- the whole `while` expression evaluates to `NULL`

### `repeat`

- has no condition
- the body runs at least once, so variables written in it are definitely assigned after the loop
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
- `@forall T: numeric` — binder constraints use the same names and semantics as the compact
  `<T: numeric>` form (see [Type parameters, aliases, and nominal types](#type-parameters-aliases-and-nominal-types))
- `@param name {TYPE}`
- `@param [name] {TYPE}` for optional parameters
- `@return {TYPE}`
- `@returns {TYPE}`

Additional rules:

- repeated `@forall` lines are allowed and accumulate in source order
- duplicate type parameter names in the same annotation block are errors
- `@forall` directives must appear before any `@param`, `@return`, or `@returns` directive
- the bracket syntax for optional parameters follows JSDoc-style notation
- if no `@return` or `@returns` annotation is provided, the return type is **elided**: on a checked
  annotation of a function definition it is inferred from the function's body (see
  [Elided return types](#elided-return-types)); in every position with no body to infer from it
  means `NULL`
- at most one `@return` or `@returns` directive may appear in the block
- `@param` directives must appear before `@return` or `@returns`

Examples:

```r
#: @param count {integer}
#: @param [label] {character}
#: @return {integer}
double_count <- function(count, label = NULL) { count + count }
```

```r
#: @param count {integer}
log_count <- function(count) { }
```

```r
#: @forall T
#: @param value {T}
#: @return {T}
identity <- function(value) value
```

```r
#: @forall T
#: @param condition {logical}
#: @param value {T}
#: @return {T | NULL}
then_some <- function(condition, value) {
  if (condition) value
}
```

```r
#: @forall T
#: @forall U
#: @param left {T}
#: @param right {U}
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

A function may declare a **rest parameter** to accept a variable number of arguments:

- `fn(...) -> RETURN_TYPE` — accepts any number of arguments of any type (`...` is shorthand for `...: Any`)
- `fn(...name: TYPE) -> RETURN_TYPE` — each additional argument must have type `TYPE`; the name is optional and, if given, is discarded (rest arguments are matched by position, never by name)
- `fn(prefix: TYPE, ...: TYPE) -> RETURN_TYPE` — a rest parameter may follow fixed parameters
- `fn(...: TYPE, [option]: TYPE) -> RETURN_TYPE` — named parameters may also follow the rest
  parameter; they are matched **by name only**, exactly like R formals declared after `...`

There may be at most one rest parameter. Its position is part of the signature and mirrors the
position of `...` in the R formal list: parameters written before it fill positionally, parameters
written after it fill by name only (see [Function calls](#function-calls)).

Additional rules:

- if the return type is omitted, it is **elided** — inferred from the body on a checked definition
  annotation, `NULL` everywhere else (see [Elided return types](#elided-return-types))
- when a compact function annotation starts with `<...>`, the binder introduces rank-1 type parameters for the whole function type
- compact function annotations do not use `fn<T>(...)`; the supported binder form is `<T> fn(...) -> ...`

Examples:

```r
#: fn(...: character) -> character
join <- function(...) paste0(...)

#: fn(x: character, ...: character) -> character
wrap <- function(x, ...) paste0(x, ": ", paste(...))
```

The annotation's `...` must appear in the same position as the function's `...` formal — both
count the parameters declared before it (see
[Function type compatibility](#function-type-compatibility)).

### Elided return types

Both annotation styles allow the return type to be left unwritten: an expanded block with no
`@return`/`@returns` line, or a compact `fn(...)` with no `-> RETURN_TYPE`. An elided return is not
the same as a written `NULL`; what it means depends on whether there is a function body to infer
from:

- on a **checked annotation of a function definition** — the annotation sits on a `function(...)`
  literal whose body is checked against it — the return type is **inferred from the body**, exactly
  as it would be with no annotation at all. Annotating only the parameters is the common partial
  form (`@param u {integer}` on `add_one <- function(u) u + 1L` infers `fn(u: integer) -> integer`),
  and it must not silently pin the return
- in every position with **no body to infer from**, an elided return means `NULL`, matching R
  functions that are called for their side effects. This covers a nested function *type* (a callback
  parameter such as `@param cb {fn(integer)}`), a [trusted coercion](#trusted-coercions) or
  [`@if-unknown` coercion](#unknown-only-coercions) (which adopt exactly the written type without
  consulting the body), and an annotation on a value that is not a function literal (`g <- f` with a
  `#: fn(integer)` annotation)

A function that genuinely returns `NULL` can always say so explicitly with `@returns {NULL}` or
`-> NULL`, and that explicit form is enforced: a body returning anything non-`NULL` against it is a
type error.

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
- a formal the body tests with `missing(name)` is also optional at call sites — R's
  optional-without-default idiom (`function(name, punct) if (missing(punct)) … else …punct…` may
  be called without `punct`)
- `missing(name)` on a **defaultless** formal of the current function also narrows the formal's
  supplied state along the branch edges, exactly like a type guard:
  - on the edge where `missing(name)` is true, reading `name` is an error (R would fail the read
    at run time: "argument is missing, with no default"); writing it is legal and supplies it
    (`if (missing(punct)) punct <- "!"`)
  - on the edge where `missing(name)` is false, the formal is supplied and reads are ordinary;
    a diverging true edge (`if (missing(x)) stop(...)`) leaves the rest of the body on the
    supplied edge, and `!missing(name)` swaps the edges
  - after the branches rejoin, the formal counts as unsupplied only if it is unsupplied on **both**
    edges, so only definite runtime failures are reported
  - a formal with a default is never narrowed: reading it while unsupplied evaluates the default,
    which is legal
  - `missing()` applies only to the immediate function's own formals, matching R; an enclosing
    function's formal is not narrowed inside a nested function
- a `...` formal becomes a **rest parameter** with element type `Any`, at the position it holds in
  the formal list — `function(x, ...) …` infers as `fn(x: T, ...: Any) -> …`, and calls check
  against it by the [rest-parameter rules](#function-calls) (surplus positionals and unmatched
  keywords are absorbed; formals after the `...` are matched by name only)
- the values reaching `...` are not tracked into the body: a body use of `...` (forwarding it to
  another call) types as `Unknown`
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

Parameter names — and record field names — may contain interior `.`, matching R's identifier convention for arguments like `na.rm` and `length.out`:

- `fn(x: double, na.rm: logical) -> double`
- `list{na.rm: logical}`

The leading character must still be a letter or `_`, and the dot is interior only. Type names and type parameter names are unaffected: a type reference or a `<...>` binder name may not contain `.`.

### Function type compatibility

Parameter names are part of the call interface, and R matches call arguments against the
definition's formal names — so names participate in compatibility:

- named parameters pair **by name**: `fn(a: integer, b: character)` accepts a function defined
  `function(b, a)`, and each annotation type binds to the same-named formal regardless of order
- unnamed (positional) parameter types pair with the remaining parameters left to right, so
  `fn(count: integer) -> NULL` and `fn(integer) -> NULL` are mutually compatible
- an annotation may not *rename* a parameter: `fn(count: integer) -> integer` over
  `function(n) n` is an error, because it would promise callers a name the runtime rejects
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

#### Callback forwarding at variadic call sites

R's apply family invokes its callback as `FUN(element, ...)`, forwarding the caller's surplus
arguments — so a callback with more formals than the declared interface is still correct when the
call forwards the difference. At a call to a **variadic** function, a function-typed argument that
fails the plain interface check is re-checked as that forwarded invocation:

- forwarded **named** arguments (the ones the rest parameter would absorb) consume the callback's
  same-named formals first, each checked against its formal's type
- the interface's parameter types — the elements the callee will pass — then fill the callback's
  remaining formals in order, followed by forwarded **positional** arguments
- formals the invocation leaves unfilled must have defaults
- the callback's return type must satisfy the interface's return type (covariant)
- the re-check is a probe: on failure nothing it bound survives, and the reported error is the
  plain interface mismatch

Consequences: `lapply(words, gsub, pattern = "a", replacement = "o")` checks `gsub(word,
pattern = "a", replacement = "o")` and types as `list[character]`; `lapply(words, nchar)` accepts
`nchar`'s optional display formals; a forwarded argument of the wrong type fails the probe and the
call errors.

Variadic compatibility is conservative:

- a variadic function type is compatible only with another variadic function type; their rest element types are contravariant, like ordinary parameters, and the fixed prefixes must match by the rules above
- the rest parameters must sit at the **same position**: the number of parameters declared before
  `...` must agree on both sides, because the position decides which parameters callers may fill
  positionally
- a variadic function type and a fixed-arity function type are never compatible, in either direction

This over-rejects some safe pairings (for example a fixed function that happens to accept the same arguments), but it never admits an unsound one.

Because inference gives a `...` formal a rest parameter at its formal position (see
[Inferred function types](#inferred-function-types)), an annotation with a rest parameter checks
against a `function(…, ..., …)` definition like any other function annotation.

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
#: @param render_count {fn(integer) -> character}
#: @param count {integer}
#: @return {character}
apply_renderer <- function(render_count, count) { render_count(count) }
```

## Unsupported constructs

- when the checker encounters a syntactically valid construct that is not yet supported, the construct may infer as `Unknown`
- this allows checking to continue even when the checker cannot model the construct precisely
- whether an unsupported construct also produces a diagnostic is a construct-specific decision

## Syntax errors

A file with syntax errors is still analyzed. Analysis is error-tolerant, with one governing rule:
**a broken region reports its syntax error and nothing else** — the checker draws no semantic
conclusions from source that failed to parse.

- every well-formed statement in the file is analyzed normally: definitions keep their exports,
  references resolve, and genuine type errors outside the broken region still surface
- a broken statement contributes nothing — no names, no reads, no diagnostics beyond the syntax
  error covering it
- a broken **assignment** whose name side is intact keeps its definition: the value degrades to a
  hole that types as `Unknown`, so dependents neither lose resolution nor see a wrong type while
  the value is mid-edit; the hole is not a strict-mode origin (the syntax error already marks it)
- a **checked annotation** on such a broken definition binds its declared type unchecked — the
  definition keeps its contract for callers until the value parses again, at which point the value
  is checked against the annotation as usual

The practical consequence in an editor: while one construct is half-typed, the rest of the file —
and every other file in the package — keeps its diagnostics, hovers, and completions stable; the
only new squiggle is the syntax error itself.

### S4 slot access

`x@slot` reads (and `x@slot <- v` writes) an S4 object slot. S4 objects are not modeled, but the
construct is fully lowered:

- a slot read types as `Unknown` and is a strict-mode origin
- the subject expression is inferred; its own type errors surface
- the subject's variable read counts for naming, unused analysis, references, and rename
- a slot write is an ordinary replacement-form assignment of its base variable

## Strict mode

Strict mode is an opt-in check controlled by the `[check] strict` switch (default off).

- it does not change inference and introduces no new typing rules
- it adds diagnostics at `Unknown` origins and escalates unresolved references
- the typecheck phase already runs to produce the inferred types; strict mode reads those types
  and reports the places where the checker genuinely could not determine one

### Unresolved references escalate to errors

Unresolved references carry the `unresolved` diagnostic code:

- a bare name the resolver cannot find in the package, its imports, or builtins
- an unknown package namespace in `pkg::name`
- a name a known namespace does not export

Outside strict mode these are warnings. Under strict (configured, or via the per-file directive)
they are **errors**: a name the checker cannot see is a hole in the checked surface, not a hint.

### Per-file directive

A plain top-level comment sets one file's typing mode, overriding the configured `[check]`
switches in both directions:

```r
# typing: off      # no type or strict diagnostics for this file
# typing: on       # type checking on for this file, strict off
# typing: strict   # type checking and strict mode on for this file
```

- `off` silences the file's type errors and strict diagnostics even when the configuration checks
  types; `on` opts a single file into type checking in an otherwise unchecked workspace; `strict`
  additionally enables [strict mode](#strict-mode) for the file
- the `#: @strict` form remains supported: `#: @strict` is `# typing: strict`, and `#: @strict off`
  is `# typing: on` (type-checked, but not strictly)
- the last directive in the file wins; a `typing:`-prefixed comment with any other value is
  reported as an error rather than silently ignored
- the directive changes only which diagnostics are published for that file — inference and every
  other check are untouched, so hover and the other IDE features keep working under `off`

### Data-masked evaluation (NSE)

R evaluates some argument positions inside a data frame's own environment, where bare names are
column references no lexical scope can see. Roughly recognizes these positions structurally and
treats reads there that resolve to no binding as **column references**: silent `Unknown`, no
could-not-resolve warning, no strict origin.

Recognized masks:

- a `[` call carrying an unambiguous **data.table signature** — a `by =` / `keyby =` argument, a
  `:=` column assignment, a `.()` list call, or a `.SD` / `.N` / `.I` / `.BY` / `.GRP` / `.EACHI`
  special — masks all of that bracket's index arguments, and the whole bracket expression types as
  `Unknown` (base indexing rules do not judge `[.data.table`)
- the base masking family `with()`, `within()`, `subset()`, `transform()` masks every argument
  after the data (a locally defined function of the same name masks nothing)

Names inside a mask that *do* resolve — a local variable used in `j`, a standard-library function
like `sum` — keep their ordinary resolution and typing. Base-R indexing (`m[i, j]`) carries no
data.table marker and keeps full lexical checking.

A project `.Rtypes` stub can declare its own masking function with the `@masked` attribute —
the way to teach Roughly a dplyr-style verb:

```
filter : @masked fn(.data: Any, ...: Any) -> Any
mutate : @masked fn(.data: Any, ...: Any) -> Any
```

Calls to a `@masked` name (bare or `pkg::name`) evaluate the arguments the `...` rest parameter
absorbs inside the data's frame: bare names there are column references. Arguments matching the
declared formals (`.data` above) resolve normally, a locally defined function of the same name
masks nothing, and `@masked` on a non-variadic declaration is a stub error.

For dynamic bindings outside any recognized mask, the ecosystem-standard escape hatch works: a
top-level `globalVariables(c("a", "b"))` / `utils::globalVariables(...)` call (literal string
arguments) declares those names as dynamically bound for the whole package, and could-not-resolve
is suppressed for them everywhere. Undeclared names keep warning.

### What strict mode flags

In strict mode, an expression or binding whose inferred type is `Unknown` at the point it is
*introduced* is a diagnostic. Strict mode targets `Unknown` only:

- `Unknown` is the "could-not-determine" type and is what strict mode reports.
- `Any` is the explicit, intentional escape hatch and is always tolerated — a value typed `Any`
  never produces a strict diagnostic, even in strict mode.

### Origins, not propagation

`Unknown` is also used internally as an error-recovery value and as a propagation value: a binary
operator with an `Unknown` operand yields `Unknown`, a call whose callee or return is `Unknown`
yields `Unknown`, a block whose last expression is `Unknown` yields `Unknown`, and unifying with
`Unknown` yields the other type. If strict mode flagged every expression that *resolves* to
`Unknown`, a single root cause would spray a duplicate diagnostic across every downstream use.

Strict mode therefore flags `Unknown` only at its **origin** — the site that first introduces a
non-error `Unknown` into the type lattice — and never at a site that merely propagated `Unknown`
from a child, operand, callee, or referenced binding that is already (or will itself be) flagged at
its own origin.

The origin sites are:

- **an unsupported construct** — a syntactically valid construct the checker does not yet model
  (`Unknown` enters the lattice here);
- **a name reference whose resolved type is `Unknown` because the referenced binding has no known
  type** — for example a base-environment or library binding that has not been given a type yet.
  This composes with library typing (see below).

The following are explicitly **not** strict origins:

- an `Unknown` that arose from **error recovery**: when an expression fails to type-check, the
  underlying type error is already reported, and the recovered `Unknown` is not flagged again (no
  double-report);
- an `Unknown` that was merely **propagated** into a parent expression (binary operators, calls,
  blocks, indexing, `if`/`else`, assignments) from a child that is itself an origin or a
  propagation of one;
- a reference to a **local binding** or a **package-global binding** whose type is `Unknown`: the
  origin is the *defining* site of that binding (its own file), so the reference propagates rather
  than re-originates. This is what keeps a single root `Unknown` from producing a diagnostic in
  every file that references it;
- an **unresolved name** reference: naming already reports "could not resolve", so strict mode does
  not double-report it (an unresolved name is a naming diagnostic, not an `Unknown` origin).

Because every downstream use of a flagged `Unknown` is a propagation site rather than an origin, a
single origin used in many later expressions produces exactly one strict diagnostic.

### Composition with library typing

Strict mode is defined as a property of the inferred type at origin sites — "a genuine `Unknown`
origin is an error" — not as an enumerated denylist of today's unsupported constructs. As inference
and library/stdlib stubs improve, fewer origins exist (an unstubbed library function that today has
no known type will, once stubbed, resolve to a real type), so strict mode's diagnostics shrink
automatically without any change to the strict-mode rule itself.

### Diagnostics

Strict diagnostics use a distinct diagnostic category (code `strict`) so they can be filtered
independently of type errors. Each origin is reported once, at the precise range of the origin
expression:

- a binding whose value originates an `Unknown` reads
  `strict mode: could not determine the type of \`x\`; add a type annotation`;
- a bare expression that originates an `Unknown` reads
  `strict mode: this expression has an undetermined type (\`Unknown\`)`.
