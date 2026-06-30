---
title: Structure
description: The file structure of the analysis and engine crates
---

This document is the authoritative file structure for the analysis code. Two crates are involved: `analysis` holds the computational phases and the from-scratch checker, and `engine` holds the generic memoized-query core plus the R query bodies that drive incremental analysis (see [Architecture](/architecture)).

Keep it focused on the file split and the role of each file.

## `analysis` crate

- `analysis.rs`
  - the `Analysis` document store (parsed documents, edit/reparse)
  - `run_full`, the clean from-scratch checker retained as the differential oracle and the command-line path
  - package-level phase wiring (`resolve_package`, `typecheck`) as from-scratch passes
  - `check`, `lint`, `lower` entry points

- `document.rs`
  - parsed document type
  - document edit and reparse mechanics

- `hir.rs`
  - HIR data structures
  - stable ids
  - file-local semantic representation

- `ide.rs`
  - the IDE feature result types
  - the `IdeDatabase` fact-provider trait and its implementation for `Analysis`
  - the public IDE entry points

- `ide/generic.rs`
  - the interactive features (hover, completion, definition, references, rename, inlay hints, signature help) written once over `&dyn IdeDatabase`, so the identical orchestration runs on the from-scratch oracle and on the engine-backed view

- `type_syntax.rs`
  - typing-comment and type-declaration parsing
  - surface-type rendering

- `types.rs`
  - core type representation shared by inference and checking
  - inference variables, constraints, quantified variables, surface types, and type schemes

- `lower.rs`
  - syntax-to-HIR lowering

- `naming.rs`
  - scopes, bindings, use-site resolution
  - `resolve_document_locally` (file-local naming) and package-global resolution

- `typecheck.rs`
  - semantic checking
  - inference internals
  - compatibility logic
  - the irreducible builtin kernel (operators and core constructors)
  - interface extraction

- `stdlib.rs` + `stubs/*.R`
  - `stdlib.rs` loads the standard-library stubs
  - `stubs/base.R`, `stubs/stats.R`, `stubs/utils.R`, `stubs/methods.R` are declaration-only R carrying `#:` annotations, harvested into type schemes by the loader

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

- `tree.rs`
  - parser construction
  - rope-to-tree parsing
  - tree-sitter navigation
  - `kind` and `field` ids

## `engine` crate

- `engine.rs`
  - the generic red-green memoized-query core: revision clock, type-erased slots, runtime dependency recording, red-green validation with value-equality cutoff, input tombstones, and the accidental-cycle guard
  - no R knowledge and no dependency on `analysis`

- `queries.rs`
  - the R query bodies (`parse` → `lower` → `local_naming` → `package_symbol_index` → `defining_item` → `global_scheme` → `typecheck` → `diagnostics`, plus lint and the re-export interface fixed-point), each calling the corresponding `analysis` phase function

- `ide_view.rs`
  - `EngineIde`, the engine-backed implementation of `analysis`'s `IdeDatabase` trait, so the shared IDE features run over engine query results

## Deferred split

- the scheme-expressible standard library lives in `stdlib.rs` + `stubs/*.R`; only the irreducible builtin kernel (operators and core constructors) remains in `typecheck.rs`
- keep compatibility logic and interface extraction inside `typecheck.rs` for now; split those out only after the `typecheck` structure stabilizes

## Role of this document

Use this document for:

- the crate file split
- the intended responsibility of each file
- which splits are intentionally deferred

Do not use this document for:

- a changelog
- a task tracker
- detailed phase semantics
