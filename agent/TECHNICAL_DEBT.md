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

This makes the file hard to reason about, harder to test by phase, and harder to evolve toward the architecture described in `ARCHITECTURE.md`.

### Builtin registration is duplicated

Builtin registration logic is duplicated between the main checker flow and the fixture helpers.

This creates drift risk and makes tests depend on setup that is not owned by a shared boundary.

### Rope and tree-sitter helper logic overlaps with `roughly`

`roughly/src/tree.rs` now re-exports the parser constructors and the `kind`/`field` id tables from `analysis` and keeps only CLI- and formatter-specific helpers, so the former wholesale duplication is gone.

Remaining overlap: `roughly/src/index.rs` still walks the AST with its own symbol-indexing logic for the per-keystroke document-symbol path, and small rope/text helpers exist on both sides.

### Tree-sitter node matching is string-based in hot front-end code

The `analysis` front end currently matches tree-sitter node kinds and fields through string-based APIs such as `kind()` and `child_by_field_name()`.

`roughly` already uses `kind_id()` and `field_id()` style access in several places. That difference suggests an opportunity to consolidate syntax constants and traversal helpers into reusable shared infrastructure.

The current string-based matching is not only repetitive. It also leaves performance and consistency improvements on the table, especially in front-end code that will run frequently across large code bases.

### The main check pipeline does not retain successful semantic results

The current public checking result is diagnostics-only.

That is enough for some tests, but it leaves later tooling work without a stable checked artifact to consume and encourages recomputation in places that will eventually need typed results.

### `workspace` still overlaps too much with `package`

The current `workspace` API still exposes package-shaped mutation helpers.

That overlap makes the intended boundary harder to read: `Package` is the analysis unit, while `Workspace` should stay the editor-facing registry and mutation helper around packages and detached scripts.
