# Typecheck Bindings Suite

This README is the authoritative contract for the `tests/typecheck/bindings/` suite.

If the coverage split or intended semantics coverage changes, update this file and
`agent/TESTING.md` in the same session as the fixture changes.

## Purpose

`bindings` answers:

- what type scheme gets stored at a binding boundary
- how rebinding changes the stored scheme over time
- how binding-level annotations and coercions affect the stored scheme

This suite renders `name: TYPE_SCHEME` lines.

It does not render later use-site expression results as its primary contract.

## Current files

- `annotations.R.test`
- `basics.R.test`
- `functions.R.test`
- `nominals.R.test`

## Matrix

The bindings suite should explicitly cover:

- scalar and simple value bindings
  - scalar literal binding
  - repeated top-level rebinding
- inferred function bindings
  - identity
  - first / keep-left style functions
  - constant functions
  - higher-order functions
- generalized schemes
  - one type parameter
  - several type parameters
  - alias of a polymorphic binding
  - annotations that prevent generalization
- binding annotations
  - compact function annotations
  - expanded function annotations
  - optional parameters
  - checked annotations for non-function bindings
  - `@trust`
  - `@if-unknown`
  - `@new`
- named type usage at binding boundaries
  - aliases
  - nominal types
  - generic aliases
  - generic nominal types
  - `@forall` annotations on bindings

## Current gaps

Known missing or thin areas:

- rebinding-history coverage is thin; most cases assert one final scheme per name

## What does not belong here

- later call or use-site result types
- final exported file surface after collapsing rebindings
- cross-file package behavior
- raw metavariable-facing inference output

## Naming guidance

- prefer groups such as `basics`, `functions`, `annotations`, `nominals`
- prefer case names that say what scheme fact is being checked, for example
  `identity_binding_generalizes_to_single_type_parameter`
