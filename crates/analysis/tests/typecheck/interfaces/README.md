# Typecheck Interfaces Suite

This README is the authoritative contract for the `tests/typecheck/interfaces/` suite.

If the coverage split or intended semantics coverage changes, update this file and
`agent/TESTING.md` in the same session as the fixture changes.

## Purpose

`interfaces` answers:

- what final surface a file exports
- which top-level binding shape remains visible after rebinding settles
- which `@type` and `@alias` definitions appear in the exported surface

This suite renders final exported interface snapshots, not binding history.

## Current files

- `functions.R.test`
- `types.R.test`

## Matrix

The interfaces suite should explicitly cover:

- exported value bindings
  - one simple binding
  - several bindings in source order
  - latest binding wins after top-level rebinding
- exported function shapes
  - monomorphic annotated functions
  - polymorphic inferred functions
  - higher-order functions
- exported type definitions
  - aliases
  - nominal types
  - later generic aliases
  - later generic nominal types
- mixed exported surface
  - values plus aliases
  - values plus nominals
  - several type definitions plus values

## Current gaps

Known missing or thin areas:

- exported nominal type coverage is thin
- generic export coverage is thin
- more mixed value/type export snapshots are needed
- multi-file package interface behavior belongs in later `project`

## What does not belong here

- per-assignment rebinding history
- use-site expression results
- raw inference metavariables

## Naming guidance

- prefer groups such as `exports`, `types`, `nominals`
- prefer case names that state exported-surface rule, for example
  `latest_binding_wins_in_interface`
