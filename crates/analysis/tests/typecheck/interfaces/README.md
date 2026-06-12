# Typecheck Interfaces Suite

This README is the authoritative contract for the `tests/typecheck/interfaces/` suite.

If the coverage split or intended semantics coverage changes, update this file and
`agent/TESTING.md` in the same session as the fixture changes.

## Purpose

`interfaces` answers:

- what final surface a file exports after top-level declarations settle
- which surviving binding shape remains visible for each exported value name
- which `@type` and `@alias` definitions appear alongside exported values
- what final source order the surviving exported entries render in

This suite renders final exported interface snapshots, not binding history.

## Fixture shape

The current `interfaces` runner accepts `Simple` fixtures only.

That default is intentional: most interface rules are file-local exported-surface rules, so one
input file and one final snapshot is usually clearest.

If `MultiFile` support is later added here, use it only when one case genuinely needs to snapshot
several file-local interfaces in one workspace state without asserting package-visible interaction.

Do not use `interfaces` as catch-all home for multi-file typed behavior. Cases about:

- cross-file value resolution
- cross-file type resolution
- package file winner behavior
- script versus package consumer behavior
- workspace edits across generations

belong in later `tests/typecheck/project/`.

## Current files

- `functions.R.test`
- `nominals.R.test`
- `types.R.test`

## Matrix

The interfaces suite should explicitly cover:

- exported value bindings
  - one simple scalar binding
  - several surviving value exports in final source order
  - mixed scalar and non-scalar value exports
- top-level rebinding collapse
  - latest binding wins after top-level rebinding
  - latest binding wins when shape changes, for example value -> function or function -> value
  - rebinding one exported name does not hide unrelated exported names
  - surviving export order follows final surviving declaration order
- exported function shapes
  - monomorphic annotated functions
  - compact annotated functions
  - expanded annotated functions
  - named-parameter functions
  - optional-parameter functions
  - polymorphic inferred functions
  - explicit `@forall` annotated functions
  - higher-order functions
- exported type definitions
  - aliases
  - nominal types
  - aliases referenced by later exported values
  - nominals referenced by later exported values
  - generic aliases
  - generic nominal types
- mixed exported surface
  - several type definitions plus several values
  - values plus aliases
  - values plus nominals
  - values whose final exported type uses alias names
  - values introduced with `@new` that preserve nominal identity in exported surface
  - function exports plus type definitions in same file

## Current gaps

Known missing or thin areas:

- expanded annotation export coverage is thin
- explicit `@forall` export coverage is thin
- rebinding coverage mostly covers one-name collapse, not richer mixed surfaces
- more mixed value/type export snapshots are needed
- multi-file package interface behavior belongs in later `project`

## What does not belong here

- per-assignment rebinding history
- use-site expression results
- raw inference metavariables
- package-visible cross-file typing behavior
- diagnostics wording or source ranges

## Naming guidance

- prefer groups such as `exports`, `functions`, `types`, `mixed`
- prefer case names that state exported-surface rule, for example
  `latest_binding_wins_in_interface`
