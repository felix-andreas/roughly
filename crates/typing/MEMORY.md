# `typing` Memory

This document stores cross-session context for the `typing` crate.

Keep this file compact and aggressively pruned. It should preserve only high-value continuity that is likely to matter in a later session.

<!-- Do not remove this purpose section unless the user explicitly asks for it. New items added here should also be preserved unless the user explicitly asks to remove or rewrite them. -->
Its purpose is to preserve important implementation state, open questions, and loose ends between agentic sessions, especially when the active context window is too small to carry the full design and implementation history forward.

Use this document to record:

- current implementation status
- unresolved design questions
- diagnostic quality goals
- known technical debt
- next recommended steps
- any subtle decisions that should not be rediscovered from scratch

If code changes make this document inaccurate, update it in the same session.

## Active continuity

- Discussed and documented nominal typing syntax in `SEMANTICS.md`:
  - `#: @type Name {TYPE}` defines a nominal type
  - `#: @alias Name {TYPE}` defines a structural alias
  - `#: @new Name` is nominal introduction
  - plain `#: Name` checks an already-nominal value
  - nominal values are compatible with their underlying structural representation
  - aliases and nominal types share one namespace
  - duplicate definitions are errors
  - definition blocks are source-ordered and cannot mix with ordinary annotation forms
- Renamed annotation keywords in `SEMANTICS.md`:
  - `#:? TYPE` → `#: @if-unknown TYPE`
  - `#:! TYPE` → `#: @trust TYPE`
  - terminology now uses “unknown-only coercion” and “trusted coercion”
- Updated inference fixtures to use `@if-unknown` and `@trust`:
  - `tests/inference/special_types.R.test`
  - `tests/inference/lists.R.test`
- Added `tests/types/named_types.R.test` for named-type / annotation-syntax coverage.
- Important parser/design conclusion from this session:
  - `CoreType` is the inference/unification model.
  - `SurfaceType` is the surface type-expression model, not the unification model.
  - The crate currently lacks a proper top-level annotation-syntax model for the full `#:` DSL.
  - We discussed a cleaner future direction: a single top-level syntax-item enum for annotation/comment syntax, rather than overloading `SurfaceType` for everything.
- Important code-state note:
  - `src/type_syntax.rs` was intentionally left in a conservative cleaned-up state after backing out a more invasive fixture-only parser experiment.
  - The oversized fixture-only named-reference parser was removed because it was getting too complex and hacky.
  - Current `type_syntax.rs` cleanly supports:
    - `SurfaceType`
    - `@if-unknown TYPE`
    - `@trust TYPE`
    - `@type Name {TYPE}`
    - `@alias Name {TYPE}`
  - It does **not** cleanly support bare named references like `Person` as type syntax items yet.
- Expect broken tests after this cleanup:
  - `tests/types/named_types.R.test` likely still expects named-reference behavior that is not implemented in the cleaned-up parser state.
  - This is intentional: user asked to prefer clean code over forcing a brittle implementation through.
- Open design question for next session:
  - Decide whether named references like `Person` should become part of `SurfaceType`.
  - If yes, parser design becomes simpler, but lowering/inference must then decide how to handle unresolved / nominal / alias names.
  - If no, narrow the fixture scope so annotation-syntax tests avoid nested named references until a fuller syntax model is introduced.
- Another open parser issue:
  - non-ASCII field names like `list{naïve: integer}` should eventually render correctly; current legacy parser behavior still rejects them.
