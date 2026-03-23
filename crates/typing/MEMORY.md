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

- This session was a documentation-structure and architecture-planning session only.
  - No implementation code was changed.
  - The main work was reorganizing the crate steering documents and rewriting the architecture and testing documents around the agreed phase model.

- Current steering-document layout:
  - persistent authoritative: `README.md`, `SEMANTICS.md`, `ARCHITECTURE.md`, `STRUCTURE.md`, `TESTING.md`
  - working: `TECHNICAL_DEBT.md`, `DECISION_LOG.md`
  - ephemeral: `TODOS.md`, `OPEN_DECISIONS.md`, `DISCUSS.md`, `MEMORY.md`
  - `AGENTS.md` now explains the role of each document kind and the update rules for them

- Agreed architectural direction already recorded in `DECISION_LOG.md`:
  - `check` stays the top-level orchestration entry point
  - `parser` is not a real `typing` crate phase and should leave the public surface
  - phases should be `lower`, `naming`, `typecheck`, with diagnostics as output rather than a phase
  - annotation parsing should happen during lowering exactly once
  - naming stays distinct from lowering, even if they run back to back
  - `hir.rs` and `lower.rs` should be separate files
  - HIR should move to stable arena/id-based storage
  - keep one `typecheck.rs` for now
  - keep builtins, compatibility, and interface extraction inside `typecheck.rs` for now
  - successful-check fixtures should be split into multiple suites
  - use `expressions` as the suite name for the current smaller-expression fixture category

- `ARCHITECTURE.md` was rewritten this session.
  - It now records the new phase model, representation boundaries, and project-level direction.
  - It also records that `parse.rs` is not part of the long-term public phase structure.
  - It now explicitly requires fast single-file re-analysis and dependent rechecking through exported interfaces, even for closed files.

- `STRUCTURE.md` was added this session.
  - It now records the desired near-term file split and the role of each file.

- `TESTING.md` was rewritten this session.
  - It now records the intended suite split as `annotations`, `naming`, `expressions`, `bindings`, `interfaces`, and `diagnostics`.
  - The implementation and fixture directories still need to be migrated to match that contract.

- Still-open design questions in `OPEN_DECISIONS.md`:
  - whether naming resolves both value names and type names
  - whether naming produces a new resolved artifact or HIR plus side tables
  - the exact typecheck environment shape beyond “builtins and imported interfaces”

- Testing direction discussed this session:
  - replace the current `inference` fixture suite
  - intended suite split is now `annotations`, `naming`, `expressions`, `bindings`, `interfaces`, and `diagnostics`
  - this is recorded in `TESTING.md` and `TODOS.md`
  - `DISCUSS.md` contains a short note summarizing that direction

- Recommended next step for the next agent:
  - do not change implementation first
  - migrate the fixture harness and fixture directories to match `TESTING.md`
  - then start the code refactor for HIR/lowering/naming boundaries

- Global requirement added near the end of the session:
  - keep single-file rechecking fast while still reporting dependent type errors across the project when exported interfaces change
  - this is now reflected both in `AGENTS.md` goals and in `ARCHITECTURE.md` project-level direction

- Important caution:
  - persistent authoritative documents must only be changed after user request or discussion
  - in this case, the user has explicitly been discussing and directing the architecture and testing rewrite, so updating `ARCHITECTURE.md` and `TESTING.md` was intentional
