# Typecheck Expressions Suite

This README is the authoritative contract for the `tests/typecheck/expressions/` suite.

If the coverage split or intended semantics coverage changes, update this file and
`agent/TESTING.md` in the same session as the fixture changes.

## Purpose

`expressions` answers:

- what type an expression evaluates to at a use site
- what normalized checking error kind appears when a case intentionally targets failure shape

This suite is the primary home for ordinary language semantics.

It is also the intended home for most cases currently living under:

- `tests/typecheck/deprecated/environment/`
- `tests/typecheck/deprecated/instantiation/`
- `tests/typecheck/deprecated/substitution/`

## Current files

- `arithmetic.R.test`
- `basics.R.test`
- `control_flow.R.test`
- `functions.R.test`
- `indexing.R.test`
- `lists.R.test`
- `polymorphism.R.test`
- `scoping.R.test`
- `special_types.R.test`
- `vectors.R.test`

## Matrix

The expressions suite should explicitly cover:

- basics
  - literals
  - names after assignment
  - assignment expressions
  - blocks and trailing semicolons
  - unsupported constructs that remain `Unknown`
- arithmetic
  - scalar/scalar numeric cases
  - mixed integer/double cases
  - scalar/vector shape lifting
  - named-vector arithmetic losing map-likeness
  - invalid operand diagnostics as normalized error kinds
- vectors
  - scalar-like, array-like, and map-like vector shapes
  - coercions allowed by checked annotations
  - forbidden reverse coercions
- lists
  - tuple-like and record-like inference
  - homogeneous coercions to array-like and map-like list forms
  - forbidden reverse coercions
  - mixed named and unnamed rejection
- special types
  - `NULL`
  - `Any`
  - `Unknown`
  - nullable unions
  - `@if-unknown`
  - `@trust`
- control flow
  - `if` without `else`
  - `if ... else`
  - boolean operators
  - `for`, `while`, and `repeat`
- functions and calls
  - inferred functions
  - compact function annotations
  - expanded function annotations
  - named and positional parameters
  - arity mismatch
  - calling non-functions
  - higher-order calls
- indexing
  - vector `[[`
  - list `[[`
  - list `[` coercion cases
  - `$` sugar
- polymorphism
  - repeated fresh instantiation
  - aliased polymorphic bindings
  - higher-order polymorphic use
  - solved results after substitutions propagate
- scoping
  - lexical shadowing
  - closure capture
  - local rebinding visibility
  - parameter shadowing

## Current gaps

Known missing or thin areas:

- `@new` use-site success cases
- generic named type use-site success cases
- explicit `@forall` function annotation success cases
- more alias and nominal use-site success cases
- multi-file package-visible expression behavior belongs in later `project`

## What does not belong here

- stored binding schemes as the primary assertion
- final exported interface snapshots
- raw metavariable-facing inference output

## Naming guidance

- keep one semantic file per topic rather than adding overflow files
- prefer case names that describe the user-facing rule, for example
  `record_bracket_coerces_to_map_like_list`
