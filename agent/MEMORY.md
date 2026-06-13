# Memory

This document stores cross-session context.

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

- Typecheck is incremental at document grain (two-round interface model; see `ARCHITECTURE.md`
  "Incremental model"). Generalization is level-based. Measured: 300k-line synthetic package cold
  check ~8.6s via CLI; 500-file in-process package: full ~0.7s, single-file recheck ~56ms
  (ignored benchmark test in `tests/test_incremental.rs`).
- Cross-file references are scheme-based; no inference flow across files. Scripts have a
  sequential top level and are typechecked. `analysis::typecheck` returns recomputed document
  ids; `did_save` republishes those diagnostics.
- All editor features (hover, completion, rename, goto-definition, references) live in
  `analysis::ide`; document symbols intentionally stay AST-based in `roughly/src/server.rs`.
- Biggest known checker gap: arithmetic/comparison on unannotated parameters errors (no numeric
  constraint kind). Needs a design discussion before fixing; see TODOS.
- Typed expression results are computed but not retained; hover cannot show checked types yet.
