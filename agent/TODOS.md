# TODOs

## Planning rules

- Discuss important semantic changes with the user before implementation.
- Keep `TYPING_SEMANTICS.md`, `ARCHITECTURE.md`, and fixture expectations aligned when behavior changes.
- Prefer concrete unfinished tasks over phase narration.
- Delete or rewrite stale tasks instead of preserving historical plans.

## Current priorities

- Split `typecheck.rs` along the `STRUCTURE.md` seams (inference engine, environment, builtins,
  compatibility, interface extraction). It is past 3,800 lines and is the top structural debt.
- Cheapen the typecheck no-op path: environment and type-definition fingerprints are rendered
  strings linear in package size per `typecheck` call (`just bench` shows linear single-file
  recheck). Hash or version them. (Larger incremental-naming redesign needs user discussion per
  `AGENTS.md`.)
- Round-1 interface worklist to a fixed point (removes the two-pass depth limit / cyclic `Unknown`).
- Consolidate tree-sitter access on id-based matching and dedupe rope/tree helpers with `roughly`.
- `resolve_document` public phase entry + edit-time orchestration (`projects/006`).
- More precise type-syntax error ranges (underline the offending token, not the whole annotation).
- `typecheck/project` follow-ups: package winner behavior with conflicting types; `Collate`
  coverage once the fixture harness models `DESCRIPTION`.

### Recently completed

- Numeric constraint on inference variables (`function(x) x + 1L` → `<T: numeric> fn(x: T) -> T`).
- Typed expression retention + typed hover (`### Typing` section), inlay hints, and signature help.
- Function-type contravariant parameter variance.
- Unknown type name reclassified as a naming-owned diagnostic.
- Parameter default expressions are lowered, named, and typechecked.
- Synthetic-package generator + `just bench` (10k/100k/200k) and shared generator for incremental
  benchmark.

### Active Projects

- `projects/006_incremental_analysis_operation_model.md` — operation scheduling alignment
