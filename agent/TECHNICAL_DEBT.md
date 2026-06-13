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

### Interface fingerprints render strings linear in package size

The round-2 cache key renders every package-global scheme into one environment-fingerprint string on every `typecheck` call, and round-1 renders the full type-definition table. Both are linear in package size per call even when nothing changed. Hashing per-document interface versions (or comparing structured fingerprints) would make the no-op path closer to constant.

### Round-1 interfaces degrade cyclic and deeply chained top-level references

The document interface settles in two passes, so acyclic define-then-alias and forward references resolve, but reference chains needing more than two passes and genuinely cyclic top-level definitions export `Unknown`. A worklist to a fixed point (bounded) would remove the depth limit.

### Typed expression results still are not retained

Per-document checking computes `ModuleCheck.expression_types` and exported schemes, but only diagnostics and interfaces are stored. Hover and inlay hints cannot show checked expression types yet.

### Parameter default expressions are dropped during lowering

HIR `Parameter` records `has_default` only. The default expression itself is not lowered, named, or typechecked, so defaults neither constrain the parameter type nor get checked themselves.

### Unknown type names are classified as syntax errors

An unresolved type name in an annotation renders as `Syntax Error: type syntax error: unknown type ...` even though resolution is a naming fact, not a syntax fact. Typecheck now suppresses the follow-up cascade (`InferenceError::UnresolvedAnnotationType` is swallowed at annotation application), but the remaining diagnostic should move to naming-owned classification and wording.

### Function-type variance is unspecified and covariant in practice

`check_compatibility` checks function parameters covariantly (argument-to-parameter direction per position). Sound variance would be contravariant parameters. `TYPING_SEMANTICS.md` does not yet define variance; decide and align.

### Rope and tree-sitter helper logic overlaps with `roughly`

`roughly/src/index.rs` still walks the AST with its own symbol-indexing logic for the per-keystroke document-symbol path, and small rope/text helpers exist on both sides.

### Tree-sitter node matching is string-based in hot front-end code

The `analysis` front end currently matches tree-sitter node kinds and fields through string-based APIs such as `kind()` and `child_by_field_name()` in several places, while `roughly` uses `kind_id()`/`field_id()` style access. Consolidate on id-based matching for performance and consistency.
