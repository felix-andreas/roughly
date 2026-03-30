# Structure

This document is the authoritative desired file structure for the `typing` crate.

Keep it focused on the intended near-term file split and the role of each file.

## Desired file split

- `pipeline.rs`
  - top-level orchestration
  - package-level phase wiring
  - `check`
  - `run_lowering_and_naming`
  - `run_typecheck`

- `document.rs`
  - parsed document type
  - document edit and reparse mechanics

- `hir.rs`
  - HIR data structures
  - stable ids
  - file-local semantic representation

- `type_syntax.rs`
  - typing-comment and type-declaration parsing
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

- `package.rs`
  - analysis unit
  - package document and script buckets
  - package traversal and fallback policy

- `text.rs`
  - source-text position and range types
  - rope-based text helpers
  - keep only text helpers that are genuinely reused by `typing`

- `tree.rs`
  - parser construction
  - rope-to-tree parsing
  - tree-sitter navigation
  - `kind` and `field` ids
  - current implementation may temporarily be narrower than this target shape while the shared tree utility surface settles

- `workspace.rs`
  - editor and LSP-facing mutation orchestration
  - workspace package buckets
  - detached-script insertion plus document rename, delete, and edit workflows
  - do not mirror the `Package` mutation API

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
