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

This makes the file hard to reason about. The file is now past 3,900 lines. `STRUCTURE.md` and
`DECISION_LOG.md` deliberately defer this split ("keep builtin typing, compatibility logic, and
interface extraction inside `typecheck.rs` for now") until the internal structure stabilizes. The
inference engine is now stable and well-organized top-down, so the deferral is ready to revisit:
pulling the inference state / unification engine, the builtin typing rules, and interface extraction
into sibling modules would each remove a clear seam. This is a deliberate structural decision to take
with the user before the move, since it contradicts the current `STRUCTURE.md` deferral.

### Interface fingerprints render strings linear in package size

The round-2 cache key renders every package-global scheme into one environment-fingerprint string on every `typecheck` call, and round-1 renders the full type-definition table. Both are linear in package size per call even when nothing changed. Hashing per-document interface versions (or comparing structured fingerprints) would make the no-op path closer to constant. The `just bench` suite measures this: single-file recheck currently scales roughly linearly with package size (~13ms at 10k LoC, ~1.8s at 200k LoC) instead of staying near-constant.

### Round-1 interfaces degrade cyclic and deeply chained top-level references

The document interface settles in two passes, so acyclic define-then-alias and forward references resolve, but reference chains needing more than two passes and genuinely cyclic top-level definitions export `Unknown`. A worklist to a fixed point (bounded) would remove the depth limit.

### Type-syntax error ranges are coarse

Naming reports an unresolved type name (`I could not resolve type ...`, naming-owned) over the whole annotation range rather than the offending token, because `SurfaceType` carries no per-node source ranges. Threading ranges through type-syntax parsing would let `list{age: intgr}` underline only `intgr`.

### Rope and tree-sitter helper logic overlaps with `roughly`

`roughly/src/index.rs` still walks the AST with its own symbol-indexing logic for the per-keystroke document-symbol path, and small rope/text helpers exist on both sides.

### Tree-sitter node matching is string-based in hot front-end code

The `analysis` front end currently matches tree-sitter node kinds and fields through string-based APIs such as `kind()` and `child_by_field_name()` in several places, while `roughly` uses `kind_id()`/`field_id()` style access. Consolidate on id-based matching for performance and consistency.
