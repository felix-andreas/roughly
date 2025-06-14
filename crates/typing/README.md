# Static Typing for R

> [!WARNING]
> This project is currently in the conceptual and planning phase. There is no working implementation yet, and all details are subject to change. Use this document for discussion and feedback only.

## Motivation & Challenges

Adding static type checking to R is inherently difficult due to its dynamic nature and extensive metaprogramming features. Only a subset of R code can be statically checked. For code that cannot be checked, type assertions are required, and it is the programmer's responsibility to ensure correctness.

## Type Hints: Approaches

### Type Inference vs. Explicit Type Hints

- **Everything is inferred** (like Gleam, Elm, Roc): Type hints are optional, and the type checker attempts to infer all types automatically. While this can make code concise, it may cause type errors to appear far from their source. All these languages also support explicit type annotations, which help with prototyping, documentation, and code readability.

### Option A: Type hints in R syntax

Embedding type hints directly in R code would require an alternative shell or frontend for R, compiling down to base R (similar to how TypeScript compiles to JavaScript). This approach is not feasible for early experimentation.

### Option B: JSDoc-style comments
JSDoc is an alternative syntax for TypeScript that uses the same underlying type system and type checker. In this approach, types are provided in comments, making it practical for prototyping and not requiring changes to R itself.

```r
#: @param count numeric  # scalar numeric input (expected length)
#: @param items character[] # character vector input
#: @return logical       # returns a scalar logical
has_expected_length <- function(count, items) {
  length(items) == count
}
```

> [!INFO]
> To distinguish from roxygen, use `#:` as the comment prefix:

## R's Type System: Overview

R has a small set of core types:
- Vectors
- Lists
- Language objects
- Expression objects
- Function objects
- NULL
- Builtin objects and special forms
- Promise objects
- Dot-dot-dot
- Environments
- Pairlist objects

In practice, only Vectors, Lists, NULL, and Environments are commonly used. R objects can have attributes (key-value pairs), with the `class` attribute being especially important (e.g., data.frames are lists with special attributes). Reference-based classes use environments (e.g., data.table, R6).

Vectors and lists can have arbitrary length, but some constructs (like `if`) require scalars (length-one vectors). A sound type system for R must distinguish between scalars and general vectors.

## Type System Goals

- Add type features that exist only at type-checking time (not at runtime)
- Support for:
  - Scalar (length-one vector)
  - Arrays (vector + array attributes)
  - Records (lists of fixed size with names)
  - Tuples (lists of fixed size, no names)
- Future:
  - sum types (tagged unions)
  - Maybe/Result monads

## Type Notation

The type of an R object must capture:
- Base type (vector, list, etc.)
- Class (for S3/S4)
- Scalar-ness (length 1 or not)
- (Optionally) attributes such as names

Below are some example notations and their descriptions:

| Type Notation                         | Description         |
|---------------------------------------|---------------------|
| `numeric`                             | scalar numeric      |
| `character[]`                         | character vector    |
| `list{name: character, age: numeric}` | record (named list) |
| `list{character, numeric}`            | tuple (unnamed)     |

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

## Structural vs. Nominal Typing

Structural typing is a natural fit for R due to its dynamic and flexible type system. In structural typing, types are considered compatible if their structure matches, regardless of their explicit names—this aligns well with R's philosophy and common usage patterns. Most type checking in R should therefore be structural by default, allowing for flexible and expressive code.

However, in the long run, it may be desirable to introduce nominal types for stricter guarantees. Nominal typing means that a type is only compatible with itself if it is explicitly declared as such, regardless of structural similarity. This can help prevent accidental misuse of types that happen to have the same structure but are conceptually different.

### Example: Nominal Type Syntax

```r
Person := list{name: character, age: numeric[1]}

# Function that only accepts a value of nominal type 'Person'
my_func <- function(x: Person) {
  # ...
}
```

In this example, `Person` is a nominal type. Even if another list has the same structure, it would not be accepted by `my_func` unless it is explicitly of type `Person`. This approach would require additional syntax for type creation and assignment, which remains an open question for future design.

For now, the focus is on structural typing, but nominal types may be added as the system evolves.

## Open Questions
- How much type inference vs. explicit annotation?
- How to handle generic functions and method dispatch (S3/S4)?
- How to report errors (CLI, IDE, inline)?
- Gradual typing: allow mixing typed and untyped code?

---

This document is a living draft. Feedback and contributions are welcome.
