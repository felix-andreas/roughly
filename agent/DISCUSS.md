# Discussion

This file is scratch space for active design discussion.

Use it for short summaries or temporary notes during a design conversation.

## Roughly diagnostics migration

## Fixture renderers

### Current duplication

- `tests/fixture_renderers.rs` still owns a test-only `SimpleTypeRenderer`.
- That duplicates crate rendering logic that already exists in:
  - `src/diagnostic.rs` for `CoreType`-style rendering
  - `src/type_syntax.rs` for `SurfaceType` rendering
- The fixture file also duplicates HIR traversal and formatting that is structurally very close to
  `Module::render` in `src/hir.rs`, then layers naming-specific labels on top.
- `render_interface_snapshot` also re-derives crate formatting choices for:
  - definition kind labels
  - type parameter formatting
  - type scheme rendering

### Likely target shape

- Move core type and type-scheme rendering into crate code and make fixtures call that directly.
- Move inference-error kind rendering into crate code if fixtures still need the reduced
  `error: ...` snapshots for `InferenceError`.
- Keep one crate-owned structural HIR renderer/traversal, and let fixture-specific naming renderers
  add binding labels on top of that shared traversal instead of reimplementing the whole walk.
- If the interface snapshot format is intended to stay durable, move that renderer into crate code
  as well.

### Recommended first cleanup pass

- First move the `SimpleTypeRenderer` responsibilities into crate code:
  - `render_core_type`
  - `render_type_scheme`
  - quantified variable naming (let's do that)
- Then rewrite `tests/fixture_renderers.rs` to use those crate functions.
- Leave the naming-specific `@binding` fixture output test-only for now unless we decide that exact
  debug rendering is a supported crate contract.
