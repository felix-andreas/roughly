# Technical Debt

This document records current structural debt in the `analysis` crate.

It is not the architecture contract and not the action plan. `ARCHITECTURE.md` remains authoritative for intended design, and `TODOS.md` remains the actionable plan. This file exists to capture important implementation debt that is already present and should be paid down deliberately.

Keep this file focused on concrete, current debt that affects correctness, testability, performance, or the ability to evolve the checker.

## Current debt

### Typechecking and inference-engine concerns are mixed together

The current `typecheck.rs` file contains multiple responsibilities at once:

- HM-style inference state and unification
- lexical environment handling
- annotation application
- compatibility checking
- builtin typing rules
- expression-level typechecking

This makes the file hard to reason about, harder to test by phase, and harder to evolve toward the architecture described in `ARCHITECTURE.md`. The file has grown past 3,500 lines and should be split along those seams.

### `generalize` re-walks the whole environment per binding

`generalize` collects free type variables by cloning and walking every scheme in the environment on every binding boundary. That is quadratic in bindings per file and conflicts with the incremental-analysis performance goals. Level-based generalization (as in typical efficient HM implementations) would remove the environment walk.

### The package typecheck loop stops at the first error

`analysis::typecheck` `break`s out of the per-document loop on the first inference error and stores diagnostics-only output (`output: ()`). One run reports at most one type error per package, later documents silently get no checking, and no typed artifact survives for tooling, `typecheck/project` snapshots, or incremental analysis.

### Parameter default expressions are dropped during lowering

HIR `Parameter` records `has_default` only. The default expression itself is not lowered, named, or typechecked, so defaults neither constrain the parameter type nor get checked themselves.

### Unknown type names produce a syntax-error plus a cascade

An unresolved type name in an annotation renders as `Syntax Error: type syntax error: unknown type ...` and then the checked annotation fails again with `expected Unknown, found ...`. The first diagnostic should be naming-owned (per `ARCHITECTURE.md`) and the second suppressed.

### Function-type variance is unspecified and covariant in practice

`check_compatibility` checks function parameters covariantly (argument-to-parameter direction per position). Sound variance would be contravariant parameters. `TYPING_SEMANTICS.md` does not yet define variance; decide and align.

### Rope and tree-sitter helper logic overlaps with `roughly`

`roughly/src/index.rs` still walks the AST with its own symbol-indexing logic for the per-keystroke document-symbol path, and small rope/text helpers exist on both sides.

### Tree-sitter node matching is string-based in hot front-end code

The `analysis` front end currently matches tree-sitter node kinds and fields through string-based APIs such as `kind()` and `child_by_field_name()` in several places, while `roughly` uses `kind_id()`/`field_id()` style access. Consolidate on id-based matching for performance and consistency.
