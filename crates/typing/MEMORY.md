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

- `src/type_syntax.rs` now has a working recursive-descent parser for compact surface types plus the existing expanded-block annotation parser.
- Consecutive `#:` lines are now parsed as a single annotation block in both diagnostics and lowering.
- Invalid annotation blocks now report explicit block-level errors for:
  - mixed compact and expanded forms
  - multiple compact annotations
  - duplicate `@return` / `@returns`
  - `@param` after `@return`
- This fixes the old false `type syntax error: expected a type` on valid expanded multi-line annotation blocks in `roughly`.
- Record-field parse errors carry field-name context, for example `... (while parsing field \`items\`)`.
- The type parser still treats identifiers and record-field names as ASCII-only. `list{naïve: integer}` currently fails as an unknown type starting at `na`; that is current behavior, not a newly introduced regression.
- One parser audit note remains: vector suffix parsing is still permissive for non-atomic keywords such as `Any`, `Unknown`, and `NULL`. Tightening that would be a user-facing syntax decision and should be reviewed deliberately rather than folded into unrelated parser work.
- A separate annotated-function inference limitation remains: compact and expanded function annotations still do not feed parameter types into the function body during inference, so examples like `#: fn(count: integer) -> integer` on `function(count) count + count` still fail with `invalid plus operand`.
