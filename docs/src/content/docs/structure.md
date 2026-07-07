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
  - type-notation lexing (one pass shared by editor highlighting and by re-lexing a `#:` annotation's document text to map a cursor to the type token under it, for hover/goto/error-range narrowing)

- `types.rs`
  - core type representation shared by inference and checking
  - inference variables, constraints, quantified variables, surface types, and type schemes

- `lower.rs`
  - syntax-to-HIR lowering

- `naming.rs`
  - scopes, variable slots, use-site resolution, and the reaching-write flow analysis
    (definite-assignment warnings and the unused dead-store check)
  - `resolve_document_locally` (file-local naming) and package-global resolution

- `typecheck.rs` (+ `typecheck/`)
  - semantic checking
  - inference internals
  - compatibility logic
  - the irreducible builtin kernel (operators and core constructors)
  - interface extraction
  - `typecheck/environment.rs` — the variable-slot environment: bind/lookup, undo-logged entry
    writes, branch joins, captured-write notes, and the loop fixed point
  - `typecheck/unify.rs` — the unification core: variable allocation (fresh and rigid), the
    snapshot / rollback / commit machinery probes ride on, constraint raising, resolution,
    `unify` and its structural cases, directional `check_compatibility`, scheme instantiation and
    generalization, and function-type unification
  - `typecheck/operand.rs` — free-standing helpers behind the core: operand classification and
    numeric promotion for the builtin operators, comparison-family shapes, guard-refinement
    filtering, and the small pure `CoreType` transformations around unification and scheme import

- `stub.rs` + `stdlib.rs` + `stubs/*.Rtypes`
  - `stub.rs` parses the declaration-only stub format (`name : <type-expr>` lines, reusing the type-expression parser)
  - `stdlib.rs` loads the standard-library stubs and folds project overrides over them
  - `stubs/base.Rtypes`, `stubs/stats.Rtypes`, `stubs/utils.Rtypes`, `stubs/methods.Rtypes` are declaration-only stub files, harvested into type schemes by the loader

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

- the scheme-expressible standard library lives in `stub.rs` + `stdlib.rs` + `stubs/*.Rtypes`; only the irreducible builtin kernel (operators and core constructors) remains in `typecheck.rs`
- interface extraction stays inside `typecheck.rs` for now; compatibility logic now lives in `typecheck/unify.rs`

## Role of this document

Use this document for:

- the crate file split
- the intended responsibility of each file
- which splits are intentionally deferred

Do not use this document for:

- a changelog
- a task tracker
- detailed phase semantics
