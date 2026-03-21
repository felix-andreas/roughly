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

- Generic type syntax and semantics were discussed and written into `SEMANTICS.md`.
- Fixture suites were reorganized into three intended categories:
  - `tests/annotations/invalid_syntax.R.test`
  - `tests/annotations/invalid_semantics.R.test`
  - `tests/annotations/unsupported.R.test`
- A new `tests/annotations/generics.R.test` suite was added as the contract target for generics syntax and annotation behavior.
- Current implementation does not yet support the generics syntax covered by `tests/annotations/generics.R.test`.
- The recommended implementation order is:
  1. make `tests/annotations/generics.R.test` pass for the intended supported generic syntax
  2. then implement the distinction between invalid syntax, invalid semantics, and unsupported constructs
  3. only after the supported generics path works, reconcile diagnostics and failure behavior with the three failure-oriented fixture suites
- Do not start by “fixing” failure fixtures to current parser behavior. Treat the generics fixture suite as the primary target contract first, then handle failure cases deliberately afterward.
