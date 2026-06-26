# Typecheck Expressions Suite

This README is the authoritative contract for the `tests/typecheck/expressions/` suite.

If the coverage split or intended semantics coverage changes, update this file and
the testing docs page in the same session as the fixture changes.

## Purpose

`expressions` answers:

- what type an expression evaluates to at a use site
- what normalized checking error kind appears when a case intentionally targets failure shape

This suite is the primary home for ordinary language semantics.

## Current files

- `arithmetic.R.test`
- `basics.R.test`
- `comparison.R.test`
- `control_flow.R.test`
- `functions.R.test`
- `indexing.R.test`
- `lists.R.test`
- `nominals.R.test`
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
  - `%%`, `%/%`, `^`, and `**`
  - unsupported `%op%` specials staying `Unknown`
  - invalid operand diagnostics as normalized error kinds
- comparison, negation, and ranges
  - comparison families (numeric, character, logical) and cross-family rejection
  - shape lifting to `logical[]`
  - unary `!` on logical shapes and rejection elsewhere
  - `:` endpoint typing including whole-number double literals
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
  - inferred functions with parameter names and optional defaults
  - compact function annotations, including `[name]: TYPE` optional parameters
  - expanded function annotations
  - named and positional parameters, named arguments out of order
  - argument compatibility coercions and `Unknown`-argument acceptance
  - arity mismatch
  - calling non-functions
  - higher-order calls and function-typed arguments
- nominals and aliases
  - `@new` introduction success and failure
  - nominal projection through operators, indexing, and iteration
  - nominal function parameters and returns
  - alias and generic alias use
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

- backtick-quoted `$` name sugar is not covered (lowering does not model it yet)
- multi-file package-visible expression behavior lives in `project`, which currently renders
  diagnostics only

## What does not belong here

- stored binding schemes as the primary assertion
- final exported interface snapshots
- raw metavariable-facing inference output

## Naming guidance

- keep one semantic file per topic rather than adding overflow files
- prefer case names that describe the user-facing rule, for example
  `record_bracket_coerces_to_map_like_list`
