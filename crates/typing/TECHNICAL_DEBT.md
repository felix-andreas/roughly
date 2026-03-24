# Technical Debt

This document records current structural debt in the `typing` crate.

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

The `typing` crate currently carries rope and tree-sitter helper code that overlaps with helper logic in the `roughly` crate.

This includes text extraction and source-position helper functionality that is conceptually shared infrastructure rather than checker-specific behavior.

That overlap increases maintenance cost and creates drift risk between the syntax pipeline in `roughly` and the checker front end.

### Tree-sitter node matching is string-based in hot front-end code

The `typing` front end currently matches tree-sitter node kinds and fields through string-based APIs such as `kind()` and `child_by_field_name()`.

`roughly` already uses `kind_id()` and `field_id()` style access in several places. That difference suggests an opportunity to consolidate syntax constants and traversal helpers into reusable shared infrastructure.

The current string-based matching is not only repetitive. It also leaves performance and consistency improvements on the table, especially in front-end code that will run frequently across large code bases.

### The main check pipeline does not retain successful semantic results

The current public checking result is diagnostics-only.

That is enough for some tests, but it leaves later tooling work without a stable checked artifact to consume and encourages recomputation in places that will eventually need typed results.

### There is duplicate `CheckResult` structure

`CheckResult` currently exists in more than one module.

That is a small debt item, but it is a sign that result ownership and rendering boundaries are still blurry.
