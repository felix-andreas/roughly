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

- Typecheck fixture migration active. Deprecated suites remain under `tests/typecheck/deprecated/` as migration storage only.
- `typecheck_interfaces` nominal export case now passes after preserving nominal identity for `@new` values in stored schemes.
- Expanded `@forall` binding fixture now passes after preserving annotation-local binders through typecheck lowering.
- Alias annotation bindings and nominal-to-structural compatibility at binding boundaries now pass after teaching typecheck about temporary type-definition summaries and registering those definitions in the bindings fixture runner.
