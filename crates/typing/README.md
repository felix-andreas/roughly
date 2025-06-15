# Static Typing for R

This document aims to serve as a design specification for the semantics of a static type checker for R, outlining the intended type system, annotation syntax, and key design considerations.

> [!WARNING]
> This project is currently in the conceptual and planning phase. This is a design document only—there is no working implementation yet, and all details are subject to change.

## Motivation & Challenges

Adding static type checking to R is inherently difficult due to its dynamic nature and extensive metaprogramming features. Only a subset of R code can be statically checked. For code that cannot be checked, type assertions are required, and it is the programmer's responsibility to ensure correctness.

## Type Hints: Approaches

A key design decision is how to implement type hints in R code.

### Option A: No Type hints

One approach is to avoid requiring any type hints, as seen in languages like Gleam, Elm, or Roc. In this model, the type checker attempts to infer all types automatically. While this can make code more concise, it may cause type errors to surface far from their actual source, making debugging more difficult. These languages also support explicit type annotations. In summary, type inference reduces boilerplate, but type hints or annotations are still needed for clarity and better error localization.

### Option B: Type hints in R syntax

Embedding type hints directly in R code would require an alternative shell or frontend for R, compiling down to base R (similar to how TypeScript compiles to JavaScript).

```r
function has_expected_length(items: character[], count: numeric): logical {
  length(items) == count
}
```

Or for variable declarations:

```r
x: numeric <- 4
names: character[] <- c("Alice", "Bob")
```

However, this syntax would require major changes to R's parser and is not practical for early experimentation.

### Option C: JSDoc-style comments

JSDoc is an alternative syntax for TypeScript that uses the same underlying type system and type checker. In this approach, types are provided in comments, making it practical for prototyping and not requiring changes to R itself.

```r
#: @param items character[] # character vector input
#: @param count numeric  # scalar numeric input (expected length)
#: @return logical       # returns a scalar logical
has_expected_length <- function(items, count) {
  length(items) == count
}

x <- 4 #: numeric
names <- c("Alice", "Bob") #: character[]
```

> [!INFO]
> To distinguish from roxygen, use `#:` as the comment prefix:

This is the option we use for now, as it enables experimentation without modifying R itself.

## R's Type System: Overview

R has a small set of core types:
- Vectors
- Lists
- Language objects
- Expression objects
- Function objects
- NULL
- Promise objects
- Dot-dot-dot
- Environments
- S4 objects
- Pairlist objects

In practice, the most frequently used R types are Vectors, Lists, NULL, and Environments. All R objects (except NULL) can also carry arbitrary attributes—key-value pairs that provide metadata or modify behavior. The `class` attribute is particularly significant, as it enables R's object-oriented features (for example, a `data.frame` is essentially a list with specific attributes, including `class = "data.frame"`).

Reference-based object systems, such as those used by `data.table` and R6, leverage environments to implement mutable state and encapsulation.

Vectors and lists can have arbitrary length, but some constructs (like `if`) require scalars (length-one vectors). Therefore a sound type system for R must distinguish between scalars and general vectors.

## Type System Goals

- Support for:
  - **Scalar** (length-one vector)
  - **Array** (vector + array attributes)
  - **Record** (lists of fixed size with names)
  - **Tuple** (lists of fixed size, no names)
  - **Unknown** (type could not be inferred)
  - **Any** (explicit opt-out of type checking)
- Future:
  - **Sum types** (tagged unions)
  - **Maybe/Result monads**
  - **Nominal types**

## Any and Unknown Types

The types `Any` and `Unknown` are both special, permissive types in the type system:

- **Any**: Every type can be coerced to `Any`. It is used when the programmer wants to explicitly opt out of type checking for a value or expression. Assigning a value to type `Any` disables type checking for that value, similar to `any` in TypeScript.

- **Unknown**: Every type can also be coerced to `Unknown`. This type is used when the type checker is unable to infer the type of an expression. It acts as a placeholder for an unresolved type.

While both types are permissive, their intent is different: `Any` is an explicit escape hatch for the programmer, while `Unknown` signals a limitation of the type checker.

> [!NOTE]
> In a possible strict mode, it could be a type error to use `Unknown` in a context where a specific type is required. This would help catch places where type inference failed and encourage more precise typing.

## Type Notation

The type of an R object must capture:
- Base type (vector, list, etc.)
- Class (for S3/S4)
- Scalar-ness (length 1 or not)
- (Optionally) attributes such as names

Below are some example notations and their descriptions:

| Type Notation                         | Description                       |
| ------------------------------------- | --------------------------------- |
| `null`                                | null                              |
| `numeric`                             | scalar numeric                    |
| `character[]`                         | character vector                  |
| `list[numeric]`                       | list (homogenous)                 |
| `list{name: character, age: numeric}` | record (heterogeneous, named)     |
| `list(character, numeric)`            | tuple (heterogeneous, positional) |
| `fn(character, age: numeric)`         | function                          |

### Variable Type Hint

Add a type hint for a variable using the following syntax:

```r
x = 4 #: numeric
```

### Function Parameter and Return Type Annotation

Annotate function parameters and return types using comment-based hints:

```r
#: @param a integer
#: @param b integer
#: @return integer
function(a, b) a + b
```

> [!NOTE]
> The notation for annotating the class of an object (e.g., S3/S4 or custom classes) remains an open question and is subject to future design.

## Nominal Typing (Future)

Structural typing is a natural fit for R due to its dynamic and flexible type system. In structural typing, types are considered compatible if their structure matches, regardless of their explicit names—this aligns well with R's philosophy and common usage patterns. Most type checking in R should therefore be structural by default, allowing for flexible and expressive code.

However, in the long run, it may be desirable to introduce nominal types for stricter guarantees. Nominal typing means that a type is only compatible with itself if it is explicitly declared as such, regardless of structural similarity. This can help prevent accidental misuse of types that happen to have the same structure but are conceptually different.

### Example: Nominal Type Syntax

To define a nominal type, instantiate and use them:

```r
#: Person := list{name: character, age: numeric}

# use the following syntax, to instantiate a nominal type
person <- list(name = "alice", age = 25) #:= Person

# function that only accepts a nominal 'Person' type
#: @param x Person
func <- function(x) {
  # ...
}

func(person) # ✅ works

func(list(name = "bob", age = 20)) # ❌ type error
```

In this example, `Person` is a nominal type. Even if another list has the same structure, it would not be accepted by `func` unless it is explicitly of type `Person`.

For now, the focus is on structural typing, but nominal types may be added as the system evolves.

## Union Types (Future)

Union types allow a value to be one of several possible types. This is particularly valuable in R, where functions often accept multiple input types.

### Approaches for Union Types

There are four main possibilities for implementing union types in R:

1. **Structural Untagged Unions**: Allowing a value to be any of several types, checked structurally (e.g., `numeric|character`).
2. **Structural Tagged Unions**: Using a tag attribute to distinguish between variants, but not requiring a nominal type declaration. This is likely the easiest and most idiomatic approach for R.
3. **Nominal Untagged Unions**: Defining a union type by name, but not using explicit tags at runtime.
4. **Nominal Tagged Unions (Sum Types)**: Defining a union type by name and using explicit tags at runtime, similar to algebraic data types in functional languages.

### Option: Structural Tagged Unions

For most use cases, structural tagged unions are the most practical. They use a tag attribute to distinguish between variants, and can be implemented with a lightweight runtime library. This approach integrates well with R's dynamic nature and does not require extensive boilerplate.

```r
#: @param input numeric
#: @return [Ok numeric, Err character]
safe_sqrt <- function(input) {
  if (input >= 0) {
    sqrt(input) |> with_tag("Ok")
  } else {
    "Cannot take square root of negative number" |> with_tag("Err")
  }
}

#: @param result [Ok numeric, Err character]
#: @return numeric
handle_result <- function(result) {
  switch(
    tag(result),
    Ok = result,
    Err = {
      warning(result)
      NA
    }
  )
}
```

This approach would benefit from a lightweight runtime library (e.g., `typing`) that provides constructor functions and utilities for working with tagged unions:

```r
with_tag <- function(tag, value) {
  attr(value, "tag") <- tag
  value
}

tag <- function(value) {
  tag <- attr(value, "tag")
  if (is.null(tag)) stop("Not a sum type")
  tag
}
```

Consider creating utility functions to simplify the creation and handling of frequently used tagged unions (such as `Result`, `Maybe`, etc.). These helpers can reduce boilerplate and promote consistent usage patterns throughout your codebase.

```r
#: @param value <T>
#: @return [Ok <T>]
Ok <- function(value) {
  value |> with_tag("Ok")
}

#: @param error <E>
#: @return [Err <E>]
Err <- function(error) {
  error |> with_tag("Err")
}

#: @param x [Ok<T>, Err <E>]
#: @return logical
is_ok <- \(x) tag(x) == "Ok"

#: @param x [Ok<T>, Err <E>]
#: @return logical
is_err <- \(x) tag(x) == "Err"
```

### Exhaustiveness Checking

A key benefit of union types is that the type checker can enforce exhaustiveness in pattern matching or switch statements. This means that all possible variants of a union type must be handled explicitly, preventing bugs from unhandled cases.

For example, consider a `Maybe` type with two variants: `Just` and `Nothing`:

```r
#: @param result [Just numeric, Nothing]
#: @return numeric
process_result <- function(result) {
  switch(
    tag(result),
    Just = get_value(result),
    Nothing = 0
    # If a new variant is added to the Maybe type, the type checker will report an error
    # if it is not handled here.
  )
}
```

If you later extend `Maybe` with a new variant (e.g., `Unknown`), the type checker will require you to update all switch statements that handle `Maybe` to cover the new case, ensuring your code remains correct and robust.

## Open Questions

- How much type inference vs. explicit annotation?
- How to handle generic functions and method dispatch (S3/S4)?
- Gradual typing: allow mixing typed and untyped code?
