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

- Fresh-session starting point:
  - Read `AGENTS.md` first.
  - Then read `DECISION_LOG.md`, `OPEN_DECISIONS.md`, `TODOS.md`, `DISCUSS.md`, `ARCHITECTURE.md`, `STRUCTURE.md`, and `TESTING.md`.
  - Do not treat older architectural prose elsewhere in the crate as settled if it conflicts with those files.

- Recent implementation progress:
  - `hir.rs` now uses `HirArena` and stable `ExpressionId`s.
  - Lowering now processes `#:` annotations in a single pass directly into the `HirArena`.
  - `naming.rs` resolves scopes and value bindings into stable `BindingId`s.
  - Added a dedicated `lowering` fixture suite utilizing a new `Module.render()` method.

- Current steering-document layout:
  - persistent authoritative: `README.md`, `SEMANTICS.md`, `ARCHITECTURE.md`, `STRUCTURE.md`, `TESTING.md`
  - working: `TECHNICAL_DEBT.md`, `DECISION_LOG.md`
  - ephemeral: `TODOS.md`, `OPEN_DECISIONS.md`, `DISCUSS.md`, `MEMORY.md`
  - `AGENTS.md` now explains the role of each document kind and the update rules for them

- Agreed architectural direction already recorded in `DECISION_LOG.md`:
  - `check` stays the top-level orchestration entry point
  - phases should be `lower`, `naming`, `typecheck`, with diagnostics as output rather than a phase
  - keep one `typecheck.rs` for now
  - keep builtins, compatibility, and interface extraction inside `typecheck.rs` for now

- Still-open design questions in `OPEN_DECISIONS.md`:
  - the exact typecheck environment shape beyond “builtins and imported interfaces”

- Recommended next step for the next agent:
  - Reshape `typecheck.rs` to consume the `NamingResult` bindings instead of the raw `Symbol`s.
  - Handle mapping builtins and imported interfaces into stable `BindingId`s so typecheck can use them uniformly.
  - Fix the failing tests in `diagnostics` and `expressions` suites, which were failing before the arena/naming refactor began.

- Global requirement added near the end of the session:
  - keep single-file rechecking fast while still reporting dependent type errors across the project when exported interfaces change
  - this is now reflected both in `AGENTS.md` goals and in `ARCHITECTURE.md` project-level direction

- Important caution:
  - persistent authoritative documents must only be changed after user request or discussion
  - current authoritative-document drift to discuss before editing:
    - `README.md` points to `IMPLEMENTATION_GAPS.md`, but that file is absent
    - `SEMANTICS.md` still references the old `tests/fixtures/inference/` path
