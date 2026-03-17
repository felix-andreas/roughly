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

- `SEMANTICS.md` is now the single source of truth for user-facing typing semantics. Changes to it must be discussed with the user first. Fixture expectations and `SEMANTICS.md` are both contract documents and must stay aligned.
- `DRAFT.md` holds older, non-authoritative semantics ideas. Do not implement from it directly. Use it only as a discussion starting point.
- Fixture suites are split by feature under `tests/fixtures/diagnostics/` and `tests/fixtures/inference/`. Fixture identity still comes only from `group__case`, and duplicate identities are rejected across the whole suite.
- Agreed list semantics:
  - unnamed `list(...)` is tuple-like
  - `list()` is the empty tuple-like case
  - named `list(...)` is map-like
  - mixed named and unnamed entries are an error
  - tuple-like lists can coerce to `list[...]`
  - map-like lists can coerce to `list[key: value]`
  - reverse coercions are disallowed
  - tuple/map-like rendered forms use `{}` in user-facing semantics
- Agreed vector semantics:
  - scalar-like: `T`
  - array-like: `T[]`
  - map-like: `T[named]`
- Agreed special-type semantics:
  - `NULL` is the unit type and is incompatible with every other type
  - `Any` is compatible with everything
  - `Unknown` is only compatible with `Any`
- Agreed annotation forms:
  - `#: T` checked compatibility-based annotation
  - `#:? T` only allowed when inferred type is `Unknown`
  - `#:! T` trusted assertion / “trust me bro” cast
  - annotations are preceding `#:` comments attached to the following binding or expression
- Agreed function-type semantics:
  - only `#:` comments
  - either expanded `@param` / `@return(s)` form or compact `fn(...) -> ...` form, never both
  - optional parameters use `[...]`
  - unnamed function-type parameters are positional-only; named calls are an error in that case
  - omitted return annotations default to `NULL`
  - higher-order function types are allowed
- Agreed nullable-union semantics:
  - only `T | NULL` / `NULL | T` are allowed for now
  - nullable unions are allowed anywhere a type can appear
  - nested nullable unions collapse internally
  - main motivation is `if` without `else`
- Agreed `if` semantics:
  - condition must be scalar `logical`
  - `if` without `else` returns `T | NULL`
  - `if ... else` requires equal branch types unless one branch is `NULL`
  - `T` with `NULL` yields `T | NULL`
- Higher-order mismatch diagnostics still tend to report the constraint-introducing site and may render unresolved placeholders like `type1` instead of the eventual call-site type. Do not “fix” fixture expectations without deciding whether to improve diagnostic precision.
- Some constructs from otherwise valid input still lower to `Unsupported`, and nested names inside those lowered unsupported forms are not recursively lowered.
- Annotation implementation is still incomplete. The semantics contract is ahead of the code in several areas, including lists, unions, `if`, `Any` / `Unknown`, and assertion forms.
- In integrated use, syntax errors should come from `roughly`'s existing syntax checker before `typing` runs.
- Function parameters with defaults are still too minimal for end-to-end named-argument mismatch diagnostics. Do not reintroduce those fixtures yet.