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

If the user says "cleanup memory", prune resolved or low-value session-specific details aggressively, but keep this purpose section and preserve any still-useful continuity.

This is not a replacement for `AGENTS.md`, `ARCHITECTURE.md`, or `TODOS.md`.

- `AGENTS.md` contains crate-specific working rules and workflow expectations.
- `ARCHITECTURE.md` is the maintained design contract.
- `TODOS.md` is the maintained execution plan.
- `MEMORY.md` is a compact continuity document for session-to-session handoff.

If code changes make this document inaccurate, update it in the same session.

## Collaboration reminders

- Review `AGENTS.md` before making significant changes in this crate.
- Important design decisions must be discussed with the user before implementation.
- If a todo is marked `(needs refinement)`, stop and discuss it before proceeding.
- Keep `AGENTS.md`, `ARCHITECTURE.md`, `TODOS.md`, and this file aligned when implementation meaningfully changes.
- Diagnostic quality is a core goal. We want Elm- and Rust-like error messages: clear, precise, actionable, and user-centered.

## Current status

The crate currently has:

- parsing and syntax diagnostics
- lowering for:
  - top-level sequences
  - identifiers
  - `NULL`
  - `TRUE` / `FALSE`
  - integer, float, and string literals
  - assignments
  - function definitions
  - calls
  - simple wrapped expressions
- string interning
- core type representations
- inference state with path compression
- monomorphic expression inference
- end-to-end type diagnostics
- grouped fixture-based end-to-end tests under `tests/`

Important current limitations:

- unsupported syntax still lowers to `Unsupported`
- nested names inside unsupported syntax are not recursively lowered yet
- inference is still monomorphic
- some type diagnostics still fall back to coarse ranges
- some rendered types still expose internal inference variable names such as `t0`

## Highest-value unfinished work

### 1. Finish the current diagnostics pass

Recent work improved ranges and wording, but diagnostics are still not at the target quality bar.

Remaining focus:

- extend precise source ranges to remaining inference failures
- improve type rendering so diagnostics expose less internal machinery
- refine wording for higher-order and arity-related failures

### 2. Implement let-polymorphism

This is still the next major semantic step after the current monomorphic foundation.

Needed work:

- free type variable computation
- generalization at bindings
- instantiation at use sites
- polymorphism tests on R snippets

### 3. Unsupported syntax behavior is still minimal

Current behavior:

- unsupported expressions become `Unknown`
- nested names inside unsupported syntax are not processed

This may affect diagnostics and future type flow. Broader behavior here should be discussed before implementation.

### 4. `if` is still unresolved

Open design question:

- should `if` be part of the next semantic slice,
  or should it wait until after diagnostics / polymorphism / annotations?

### 5. Lists / tuples / records are not fully connected to R syntax yet

The type model already includes them, but lowering and inference for the intended R syntax are still incomplete.

### 6. Annotation parsing is not started

Still pending:

- parsing `#:` comment annotations
- attaching them to assignments/functions
- converting `SurfaceType` annotations into `CoreType` constraints

## Settled decisions worth preserving

These should not be reopened casually without discussion with the user:

- distinguish `integer` and `double`
- unsupported syntax infers `Unknown`
- internal generics are part of v1
- no explicit generic syntax in v1
- string interning should be used in lowering/inference
- inference-variable state should use path compression
- diagnostics should aim for Elm/Rust quality
- development should be test-driven
- end-to-end tests should use R snippets
- rendered diagnostics should be snapshot-tested

## Recommended next step

Continue improving diagnostic precision and rendering quality, then move to let-polymorphism.

## Things to watch in the next session

- Do not silently change semantics without updating the docs.
- Do not treat current diagnostics as “done”.
- Do not add broad new syntax support without checking whether it changes an important design decision.
- Keep snapshot-based tests in sync with intentional diagnostic changes.