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

- Typing integration now uses a rope-backed primary API:
  - `typing::check(node, rope, analysis_state)`
  - `typing::check_source(source, parser, analysis_state)` is the source-based helper, mainly for tests and standalone use
- `roughly` typing diagnostics no longer convert the rope to a `String` or reparse just to run typing diagnostics
- `roughly` diagnostics integration now matches the host crate style better:
  - `typing_diagnostics::analyze(node, rope, analysis_state)`
- `roughly::diagnostics::analyze_fast` no longer carries unused typing state
- `typing` now depends on `ropey` and has rope-backed text helpers in `src/text.rs`
- `typing::lower` now supports rope-backed lowering through:
  - `lower_root(node, rope, lowering_context)`
  - `lower_node_with_rope(node, rope, lowering_context)`

## Next recommended steps

- Run focused `typing` crate tests and then broader `roughly` tests to validate the rope-backed migration end to end
- Do a naming cleanup pass in `typing` so the new API split is explicit and consistent:
  - keep `check` as the primary rope-backed entrypoint
  - keep `check_source` clearly marked as the helper path
  - review any remaining source-oriented helper names that now wrap rope-backed behavior
- Review `typing/src/text.rs` for whether the helper surface is the right long-term shape or whether some functions should stay private to reduce API noise
- Consider whether `typing::diagnostics` should also gain rope-backed rendering helpers for consistency, or whether keeping rendering source-based is sufficient
- Evaluate whether `LoweringContext` should eventually separate long-lived interner state from per-check expression-id state in LSP usage
- If performance matters further, the next likely payoff is incremental typing over existing trees/modules rather than parser construction reuse
