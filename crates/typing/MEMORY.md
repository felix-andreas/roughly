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

- `typing::check(node, rope, analysis_state)` is the primary rope-backed API.
- `typing::check_source(source, parser, analysis_state)` remains the source-based helper path for tests and standalone use.
- `roughly` typing diagnostics now analyze against the existing rope and tree instead of reparsing source text.
- `typing::lower` and `typing::text` now have rope-backed helpers that support this path.

## Open follow-up

- Validate the rope-backed migration with focused `typing` tests and then broader `roughly` tests.
- Keep the API split explicit: rope-backed entrypoints should read as the primary path, while source-based helpers should stay clearly secondary.
- Revisit whether `src/text.rs` should expose its current helper surface publicly or keep more of it internal.
- Longer-term LSP work may want `LoweringContext` to separate long-lived interner state from per-check expression-id state.
