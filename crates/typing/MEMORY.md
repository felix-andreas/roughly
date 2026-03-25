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

- Fresh-session starting point:
  - Read `AGENTS.md` first.
  - Then read `DECISION_LOG.md`, `OPEN_DECISIONS.md`, `TODOS.md`, `ARCHITECTURE.md`, and `TESTING.md`.
  - Do not treat older architectural prose elsewhere in the crate as settled if it conflicts with those files.

- Still-open design questions in `OPEN_DECISIONS.md`:
  - the exact typecheck environment shape beyond “builtins and imported interfaces”

- Recommended next step for the next agent:
  - Reshape `typecheck.rs` to consume the `NamingResult` bindings instead of the raw `Symbol`s.
  - Handle mapping builtins and imported interfaces into stable `BindingId`s so typecheck can use them uniformly.

- Important caution:
  - persistent authoritative documents must only be changed after user request or discussion
  - current authoritative-document drift to discuss before editing:
    - `README.md` points to `IMPLEMENTATION_GAPS.md`, but that file is absent
    - `SEMANTICS.md` still references the old `tests/fixtures/inference/` path
