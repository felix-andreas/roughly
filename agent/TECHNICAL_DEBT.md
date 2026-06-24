# Technical Debt

This document records current structural debt in the `analysis` crate.

It is not the architecture contract and not the action plan. `ARCHITECTURE.md` remains authoritative for intended design, and `TODOS.md` remains the actionable plan. This file exists to capture important implementation debt that is already present and should be paid down deliberately.

Keep this file focused on concrete, current debt that affects correctness, testability, performance, or the ability to evolve the checker.

## Current debt

### Single-file recheck still scales with package size

`typecheck` short-circuits when the package version is unchanged (so repeated IDE requests on an
unchanged package are O(1)), but a single-file *edit* still pays package-scoped work: `resolve_package`
rebuilds package naming, the interface fixed-point scans every document each round to compare its
dependency fingerprint (only changed documents are re-derived, but the scan itself is O(documents)),
and the environment fingerprint is rendered over all globals. The `just bench` suite measures this:
single-file recheck after an edit is ~13ms at 10k LoC and ~0.4s at 100k LoC. Two bounded steps would
help before the larger redesign: a reverse-dependency index (name -> referencing documents) so the
interface fixed-point only revisits affected documents instead of scanning all, and incremental
package naming. The full near-constant model is the incremental-analysis redesign `AGENTS.md` says to
design with the user first.

### Type-syntax error ranges are coarse

Naming reports an unresolved type name (`I could not resolve type ...`, naming-owned) over the whole annotation range rather than the offending token, because `SurfaceType` carries no per-node source ranges. Threading ranges through type-syntax parsing would let `list{age: intgr}` underline only `intgr`.

### Rope and tree-sitter helper logic overlaps with `roughly`

`roughly/src/index.rs` still walks the AST with its own symbol-indexing logic for the per-keystroke document-symbol path, and small rope/text helpers exist on both sides.

### Tree-sitter node matching is string-based in hot front-end code

The `analysis` front end currently matches tree-sitter node kinds and fields through string-based APIs such as `kind()` and `child_by_field_name()` in several places, while `roughly` uses `kind_id()`/`field_id()` style access. Consolidate on id-based matching for performance and consistency.
