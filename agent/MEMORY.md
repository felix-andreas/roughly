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
- The type checker now covers comparison operators, `!`, `%%`, `%/%`, `^`, `:`, `c()` emptiness,
  call-site compatibility (with `Unknown`-argument cascade suppression), inferred parameter names
  with optional defaults, `@new` validation, and nominal-to-representation projection at
  operators/indexing/iteration. `TYPING_SEMANTICS.md` and `DECISION_LOG.md` record the semantics.
- Biggest known checker gap: arithmetic/comparison on unannotated parameters errors because there
  is no numeric-class constraint on inference variables (`function(x) x + 1L` fails). Needs a
  design discussion before fixing; see TODOS.
- Project 007 (typecheck fixture rework) is done; the active fixture split is `bindings`,
  `expressions`, `interfaces`, `project` (per-file diagnostics), `unification`. The `deprecated/`
  fixture storage is deleted.
