---
title: Structure
description: The intended file structure of the analysis crate
---

This document is the authoritative desired file structure for the `analysis` crate.

Keep it focused on the intended near-term file split and the role of each file.

## Desired file split

- `analysis.rs`
  - top-level analysis owner
  - document lifecycle and invalidation
  - versioned retained phase outputs
  - package-level phase wiring
  - `Analysis`
  - `check`
  - `lint`
  - `lower`
  - `resolve_document`
  - `resolve_package`
  - `typecheck`

- `document.rs`
  - parsed document type
  - document edit and reparse mechanics

- `hir.rs`
  - HIR data structures
  - stable ids
  - file-local semantic representation

- `ide.rs`
  - editor-facing semantic queries over analysis state
  - hover and later goto-definition / references-style lookups

- `type_syntax.rs`
  - typing-comment and type-declaration parsing
  - surface-type rendering

- `types.rs`
  - core type representation shared by inference and checking
  - inference variables, constraints, quantified variables, surface types, and type schemes

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
  - the irreducible builtin kernel (operators and core constructors)
  - interface extraction

- `stdlib.rs` / `stdlib_base.R`
  - embedded standard-library type stubs
  - declaration-only R carrying `#:` annotations, harvested into type schemes by the stub loader

- `diagnostic.rs`
  - structured diagnostics
  - diagnostic rendering

- `interner.rs`
  - interned symbol storage

- `lint.rs`
  - file-local non-semantic lint diagnostics
  - style and surface checks over parsed trees

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
  - possible later editor and LSP-facing abstraction
  - current implementation may be broader than the desired near-term boundary
  - keep only the minimum workspace-style mutation helpers needed while the analysis boundary is being clarified

## Deferred split

- the scheme-expressible standard library now lives in `stdlib.rs`; only the irreducible builtin
  kernel (operators and core constructors) remains in `typecheck.rs`
- keep compatibility logic and interface extraction inside `typecheck.rs` for now
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
