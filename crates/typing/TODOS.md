# `typing` TODOs

This document tracks actionable planned work for the `typing` crate.

`SEMANTICS.md` is the user-facing contract. `ARCHITECTURE.md` describes implementation constraints. Keep this file focused on unfinished work.

## Planning rules

- Discuss important semantic changes with the user before implementation.
- Keep `SEMANTICS.md`, `ARCHITECTURE.md`, and fixture expectations aligned when behavior changes.
- Prefer concrete unfinished tasks over phase narration.
- Delete or rewrite stale tasks instead of preserving historical plans.

## Current priorities

### Lowering and representations

- [ ] Lower `list(...)` in line with current semantics:
  - [ ] preserve named versus unnamed elements
  - [ ] preserve whether names are statically known
  - [ ] reject mixed named and unnamed elements
- [ ] Lower `if`, blocks, loops, and indexing forms with precise source ranges.
- [ ] Lower enough indexing structure to distinguish positional `[[...]]`, name-based `[[...]]`, `$name`, and list/vector `[...]`.
- [ ] Finish annotation attachment for bindings, expressions, and functions.
- [ ] Extend `SurfaceType` parsing for the full currently documented annotation surface:
  - [ ] vector shapes
  - [ ] list shapes
  - [ ] nullable unions
  - [ ] compact and expanded function annotations
  - [ ] `#:?` and `#:!`

### Compatibility and checking

- [ ] Implement a compatibility/coercion layer separate from unification for user-facing checking.
- [ ] Implement checked annotations, unknown-only assertions, and trusted assertions with the current semantics.
- [ ] Implement current `Any`, `Unknown`, and `NULL` behavior in a way that preserves useful diagnostics.
- [ ] Implement restricted nullable unions `T | NULL`.
- [ ] Implement list-shape inference and coercions in line with the current semantics:
  - [ ] tuple-like and record-like default inference
  - [ ] array-like and map-like introduction through annotations and coercion
  - [ ] reverse-coercion rejections
- [ ] Implement function call checking for required, optional, positional, and named arguments.
- [ ] Implement `if`, block, loop, and indexing typing rules from `SEMANTICS.md`.
- [ ] Keep builtin support intentionally small and add new builtins only when tests require them.

### Diagnostics and fixtures

- [ ] Improve source-range precision so type errors point at the failing expression instead of a fallback range.
- [ ] Improve type rendering so diagnostics consistently use the user-facing forms from `SEMANTICS.md`.
- [ ] Improve higher-order mismatch diagnostics so they point to the most useful site and avoid unresolved placeholders where possible.
- [ ] Add or update fixture coverage for:
  - [ ] list-shape inference and coercions
  - [ ] annotation assertions (`#:`, `#:?`, `#:!`)
  - [ ] nullable unions and `if`
  - [ ] indexing
  - [ ] `Any` and `Unknown`

### Integration and API

- [ ] Keep the public API small while the checker is evolving.
- [ ] Preserve the boundary where syntax errors come from `roughly`'s syntax pipeline and type checking runs on syntactically valid input.
- [ ] Revisit the integration surface with `roughly` only after the current semantics are implemented and tested more completely.
