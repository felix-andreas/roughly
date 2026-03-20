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

- `src/type_syntax.rs` now has a working recursive-descent parser for compact surface types plus the existing expanded-block annotation parser.
- The old nested named-list parser failure is resolved for well-formed inputs. The recent remaining failures were stale tests and fixtures that omitted the closing `]` in `list[named: ...]`.
- Record-field parse errors now carry field-name context, for example `... (while parsing field \`items\`)`.
- Temporary fixture debug logging that printed normalized named-list cases was removed.
- The type parser still treats identifiers and record-field names as ASCII-only. `list{naïve: integer}` currently fails as an unknown type starting at `na`; that is current behavior, not a newly introduced regression.
- One parser audit note remains: vector suffix parsing is still permissive for non-atomic keywords such as `Any`, `Unknown`, and `NULL`. Tightening that would be a user-facing syntax decision and should be reviewed deliberately rather than folded into unrelated parser work.
