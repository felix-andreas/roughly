# TODOs

## Planning rules

- Discuss important semantic changes with the user before implementation.
- Keep `TYPING_SEMANTICS.md`, `ARCHITECTURE.md`, and fixture expectations aligned when behavior changes.
- Prefer concrete unfinished tasks over phase narration.
- Delete or rewrite stale tasks instead of preserving historical plans.

## Current priorities

- Near-constant incremental recheck *after an edit* (incremental package naming + interface-version
  tracking). The repeated-call no-op path is already O(1), but an edit still pays package-scoped
  work; this is the incremental-analysis redesign `AGENTS.md` says to design with the user first.
- More precise type-syntax error ranges (underline the offending token, not the whole annotation);
  needs per-node ranges on `SurfaceType`.
- Consolidate tree-sitter access on id-based matching and dedupe rope/tree helpers with `roughly`.
- `resolve_document` public phase entry + edit-time orchestration (`projects/006`).
- `typecheck/project` follow-ups: package winner behavior with conflicting types; `Collate`
  coverage once the fixture harness models `DESCRIPTION`.

### Recently completed

- Numeric constraint on inference variables (`function(x) x + 1L` → `<T: numeric> fn(x: T) -> T`).
- Typed expression retention; human-readable hover (type + variable definition/scope, debug-gated
  phase dumps); inlay hints; signature help.
- Function-type contravariant parameter variance.
- Unknown type name reclassified as a naming-owned diagnostic.
- Parameter default expressions are lowered, named, and typechecked.
- Expression-level annotations (e.g. `#: @new` on a bare/returned expression) are applied.
- Record types reject duplicate field names and allow a trailing comma.
- Bounded fixed-point document interface (deep forward/alias chains resolve).
- `typecheck` short-circuits on an unchanged package version (repeated IDE calls are O(1)).
- Synthetic-package generator + `just bench` (10k/100k/200k) and shared generator for incremental
  benchmark.

### Active Projects

- `projects/006_incremental_analysis_operation_model.md` — operation scheduling alignment
