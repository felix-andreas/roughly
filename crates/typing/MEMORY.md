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

## Document hygiene

Keep this file handoff-oriented and aggressively pruned.

- Keep durable design rules in `ARCHITECTURE.md`.
- Keep user-facing semantics in `SEMANTICS.md`.
- Keep planned work and completion state in `TODOS.md`.
- Keep crate workflow guidance in `AGENTS.md`.
- Keep `MEMORY.md` only for continuity that is easy to lose between sessions.
- Remove resolved items once they stop being useful for resuming work.
- Avoid repeating broad implementation summaries that can be recovered from the code or other crate documents.

## Active continuity

- Fixture suites are now split by feature under `tests/fixtures/diagnostics/` and `tests/fixtures/inference/`. Fixture identity still comes only from `group__case`, and duplicate identities are rejected across the whole suite.
- `SEMANTICS.md` is now intended to become the user-facing semantics contract over time. Changes to it must be discussed with the user first, and it must be kept in sync with fixture expectations. Both are part of the contract.
- Agreed list semantics to preserve:
  - `list(...)` with only unnamed entries is tuple-like
  - `list()` is the empty tuple-like case
  - `list(...)` with only named entries is map-like
  - mixed named and unnamed entries are an error
  - tuple-like values can be coerced to homogeneous `list[...]`
  - map-like values can be coerced to homogeneous `list[character: T]`
  - the reverse coercions should remain disallowed
  - use original R type names such as `integer`, `double`, and `character`
  - use `{}` rendering for tuples and records in user-facing semantics
- Higher-order mismatch diagnostics still tend to report the constraint-introducing site and may render unresolved placeholders like `type1` instead of the eventual call-site type. Do not “fix” fixture expectations without deciding whether to improve diagnostic precision.
- Some constructs from otherwise valid input still lower to `Unsupported`, and nested names inside those lowered unsupported forms are not recursively lowered.
- Function parameters with defaults are still too minimal for end-to-end named-argument mismatch diagnostics. Do not reintroduce those fixtures yet.
- Annotation work is not finished. The lowered representation now has annotation storage, but attachment semantics are not settled. For now, treat trailing assignment annotations as the intended near-term scope and do not assume preceding `#:` attachment works.
- In integrated use, syntax errors should come from `roughly`'s existing syntax checker before `typing` runs. Keep `typing` focused on syntactically valid input and on lowered unsupported constructs that can still arise within that input.
- `if` remains an open sequencing question. If work reaches it, discuss whether it belongs before or after further diagnostics / annotations work.
