# Structure

This document is the authoritative desired file structure for the `typing` crate.

Keep it focused on the intended near-term file split and the role of each file.

## Desired file split

- `check.rs`
  - top-level orchestration
  - phase wiring
  - checked-file result assembly

- `hir.rs`
  - HIR data structures
  - stable ids
  - file-local semantic representation

- `annotations.rs`
  - annotation and type-declaration parsing
  - surface-type rendering

- `lower.rs`
  - syntax-to-HIR lowering

- `naming.rs`
  - scopes
  - bindings
  - use-site resolution

- `typecheck.rs`
  - semantic checking
  - inference internals
  - compatibility logic
  - builtin typing
  - interface extraction

- `diagnostic.rs`
  - structured diagnostics
  - diagnostic rendering

- `interner.rs`
  - interned symbol storage

## Not part of the long-term public phase structure

- `parse.rs`
  - parser setup for tests or external integration only

## Deferred split

- keep builtin typing, compatibility logic, and interface extraction inside `typecheck.rs` for now
- split those out only after the `typecheck` structure stabilizes

## Role of this document

Use this document for:

- the desired crate file split
- the intended responsibility of each file
- which splits are intentionally deferred

Do not use this document for:

- a changelog
- a task tracker
- detailed phase semantics
