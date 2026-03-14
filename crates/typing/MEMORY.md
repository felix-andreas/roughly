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

This is not a replacement for `AGENTS.md`, `ARCHITECTURE.md`, or `TODOS.md`.

- `AGENTS.md` contains crate-specific working rules and workflow expectations.
- `ARCHITECTURE.md` is the maintained design contract.
- `TODOS.md` is the maintained execution plan.
- `MEMORY.md` is a compact continuity document for session-to-session handoff.

If code changes make this document inaccurate, update it in the same session.

## Document hygiene

Keep this file handoff-oriented and aggressively pruned.

- Keep durable design rules in `ARCHITECTURE.md`.
- Keep planned work and completion state in `TODOS.md`.
- Keep crate workflow guidance in `AGENTS.md`.
- Keep `MEMORY.md` only for continuity that is easy to lose between sessions.
- Remove resolved items once they stop being useful for resuming work.
- Avoid repeating broad implementation summaries that can be recovered from the code or other crate documents.

## Active continuity

- Higher-order mismatch diagnostics still tend to report the constraint-introducing site and may render unresolved placeholders like `type1` instead of the eventual call-site type. Do not “fix” fixture expectations without deciding whether to improve diagnostic precision.
- Unsupported syntax still lowers to `Unsupported`, and nested names inside unsupported syntax are not recursively lowered.
- Function parameters with defaults are still too minimal for end-to-end named-argument mismatch diagnostics. Do not reintroduce those fixtures yet.
- The current `c(...)` support is only a narrow arithmetic helper, not the settled list/tuple/record story.
- `if` remains an open sequencing question. If work reaches it, discuss whether it belongs before or after further diagnostics / annotations work.
- Recommended next step: improve diagnostic precision and wording, especially for higher-order failures, while preserving current polymorphism behavior.
