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

This is not a replacement for `AGENTS.md`, `ARCHITECTURE.md`, `SEMANTICS.md`, or `TODOS.md`.

- `AGENTS.md` contains crate-specific working rules and workflow expectations.
- `ARCHITECTURE.md` is the maintained design contract.
- `SEMANTICS.md` is the user-facing semantics contract and must stay in sync with fixture expectations.
- `TODOS.md` is the maintained execution plan.
- `MEMORY.md` is a compact continuity document for session-to-session handoff.

If code changes make this document inaccurate, update it in the same session.

## Active continuity

- `SEMANTICS.md` is now the single source of truth for user-facing typing semantics. Changes to it must be discussed with the user first.
- `README.md` now carries the broad crate goals and non-goals. `ARCHITECTURE.md` was rewritten to focus on implementation constraints rather than restating semantics.
- Recent list-semantics decisions:
  - user-facing rendered forms are `list{...}`, `list{name: ...}`, `list[T]`, and `list[named: T]`
  - `list(...)` currently defaults to tuple-like or record-like inference where possible
  - this default is intentionally not treated as final; distinct tuple/record constructors remain a possible later direction
- Recent annotation decision:
  - `#:? TYPE` is allowed only on `Unknown`, and if accepted the annotated expression is then treated as `TYPE`
- Recent implementation-facing design decisions from discussion:
  - only functions introduce lexical scope
  - indexing `Any` should yield `Any`
  - `if ... else` should allow widening rather than requiring exact branch equality only; this still needs to be reflected cleanly in the semantics contract
- Function parameters with defaults are still too minimal for end-to-end named-argument mismatch diagnostics. Do not reintroduce those fixtures yet.
- Higher-order mismatch diagnostics still tend to point at the constraint-introducing site and may render unresolved placeholders like `type1`.
- Some syntactically valid constructs still lower to `Unsupported`, and nested names inside those forms may escape more precise lowering and diagnostics.
