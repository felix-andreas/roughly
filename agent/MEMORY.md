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

- All editor features (hover, completion, rename, goto-definition, references) now live in
  `analysis::ide`. Definition, references, and rename share one `SymbolTarget` resolution plus a
  common occurrence scan; the `ide` fixture suite covers each feature per-directory under
  `tests/ide/`.
- The old `roughly` modules `completion.rs`, `rename.rs`, `definition.rs`, and `references.rs`
  are deleted; `roughly/src/tree.rs` re-exports `analysis` parsing helpers and `kind`/`field`
  tables and keeps only CLI/formatter helpers.
- Document symbols intentionally stay AST-based (per-keystroke path, top-level symbols only);
  see the comment in `roughly/src/server.rs::document_symbol`.
