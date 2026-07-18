---
title: Structure
description: The file structure of the syntax, semantics, ide, format, and roughly crates
---

This document is the authoritative file structure for Roughly's code. The
crate graph and phase boundaries live in [Architecture](/architecture); this
page records the file split and the role of each file. The `*-legacy` crates
keep their own (frozen) layout and are not documented here.

## `syntax` crate (`src/syntax.rs` is the root)

- `lexer.rs` — the hand lexer, including `#:` annotation regions as
  structured trivia
- `parser.rs` — the recursive-descent/Pratt parser onto rowan green trees,
  including the annotation type grammar and statement-anchored recovery
- `kind.rs` — `SyntaxKind`: every token and node kind, with display names
- `ast.rs` — typed AST views (`Option`-returning accessors over the raw tree)
- `reparse.rs` — statement-splice incremental reparse (an optimization; parse
  correctness never depends on it)
- `testing.rs` — the fixture harness (`.test` format, `ROUGHLY_BLESS`,
  `FIXTURE_FILTER`, duplicate-id rejection) shared by every fixture suite

## `semantics` crate (`src/semantics.rs` is the root)

- `semantics.rs` — the salsa database, inputs (`SourceFile`, `ProjectFiles`,
  typing-mode directives), the item tree with insertion-stable identities,
  per-item syntax anchoring, the package interface (`global_scheme` fixpoint),
  and item-span queries
- `hir.rs` — per-item HIR: expressions with item-relative ranges, lowering
  from the syntax tree
- `naming.rs` — the mutable-slot variable model: scopes, reaching-write flow,
  captures, data-masked evaluation recognition, unused outputs
- `types.rs` — interned types (`Ty`/`TyKind`), schemes, constraints, union
  normalization
- `infer.rs` — the inference table: union-find entries, unification, the
  directional compatibility relation, memoized deep resolution
- `check.rs` — the inference walk per item: environment undo-log, calls and
  overload probing, control flow, annotations enforcement, strict origins
- `annotations.rs` — lowering `#:` annotation nodes onto interned types;
  block-form rules
- `stubs.rs` — the `.Rtypes` corpus: parsing, the assembled library
  (schemes, nominals, masked verbs, namespace exports), and loader-problem
  reporting
- `lints.rs` — the style lints and their configuration types
- `diagnostics.rs` — the diagnostics edge (parse-stage and full per-file
  sets, strict rendering) and the one user-facing `TypeRenderer`

## `ide` crate (`src/ide.rs`, single file)

All eight feature families in one module over `semantics` queries: hover, the
shared occurrence engine behind definition/references/rename, inlay hints,
signature help, completion (with the shared smart-case matcher), code
actions, document/workspace symbols, annotation-type and S4 navigation.

## `format` crate (`src/format.rs`, single file)

The preserving formatter over the syntax tree, plus its configuration types.

## `roughly` crate (`src/roughly.rs` is the root)

- `main.rs` — the CLI surface and exit-code contract (0 clean, 1 findings,
  2 usage/configuration/IO errors)
- `cli.rs` — `check` (project assembly, rendering, NAMESPACE and stub-
  override reports) and `fmt` implementations
- `server.rs` — the LSP server: frontend/worker threading, document sync,
  push/pull diagnostics, all feature endpoints, semantic tokens, stub and
  NAMESPACE buffers
- `config.rs` — `roughly.toml` discovery and parsing
- `diagnostics.rs` — the shared diagnostics assembly (config gating, strict
  escalation, suppression comments) used by both the server and the CLI
- `namespace.rs` — NAMESPACE import parsing and validation
- `position.rs` — the line index: byte offsets ↔ line/column in bytes or
  UTF-16 code units
