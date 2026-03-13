# `typing` crate agent guidance

This file contains crate-local rules and working conventions for `crates/typing`.

## Document map

Before making significant changes in this crate, review these files and keep them aligned:

- `README.md`
  - high-level semantics and intended type-system behavior
- `ARCHITECTURE.md`
  - the maintained design contract and implementation direction
- `TODOS.md`
  - the maintained work breakdown and progress tracker
- `MEMORY.md`
  - cross-session continuity, loose ends, and current implementation caveats

When implementation meaningfully changes, update the relevant documents in the same session.

It is important to keep all of these documents high signal and up to date:
- avoid stale status notes
- avoid vague process language
- avoid dumping low-value session chatter into them
- prefer concise, actionable updates that help future sessions resume work quickly
- whenever implementation meaningfully changes, update the affected documents in the same session

For quick crate navigation, use this rough file-to-responsibility map:

- `src/parse.rs`
  - parser setup and syntax-tree creation
- `src/lower.rs`
  - lowering from Tree-sitter syntax into the semantic IR
- `src/types.rs`
  - type representations such as `SurfaceType`, `CoreType`, and `TypeScheme`
- `src/infer.rs`
  - inference state, unification, occurs checks, and inference errors
- `src/diagnostics.rs`
  - user-facing diagnostic rendering and type pretty-printing
- `src/check.rs`
  - end-to-end checking pipeline from source text to diagnostics
- `tests/`
  - end-to-end fixture and snapshot coverage for user-visible behavior

## Collaboration with the user

This crate is developed collaboratively with the user.

- Important design decisions must be discussed with the user before implementation.
- If a task in `TODOS.md` is marked `(needs refinement)`, stop and discuss it before proceeding.
- Do not silently lock in semantics for difficult language-design questions.
- Prefer making uncertainty explicit over guessing.

## Diagnostic quality

A core goal of this crate is high-quality diagnostics in the style of Elm and Rust.

When changing diagnostics:

- prefer clear, precise, actionable wording
- avoid overly internal or theory-heavy language when user-facing wording would be clearer
- keep snapshot tests updated only when wording or behavior intentionally improves
- prefer precise source ranges over coarse fallback ranges whenever possible

## Architecture expectations

Keep the layers conceptually separate:

- parsing and syntax access
- lowering
- type representations
- inference
- diagnostic rendering

Do not collapse these boundaries casually just for short-term convenience.

## Cross-session memory

Use `MEMORY.md` to preserve important continuity between sessions, especially when:

- a lot has changed
- the context window is getting tight
- work stops in the middle of a design or implementation thread

`MEMORY.md` is for handoff memory, not for replacing the design contract or task tracker.

## External sparring partner

If there is a difficult design decision to make, or after a large amount of implementation churn, it can be useful to use Gemini CLI as a sparring partner for reflection or comparison.

Rules for this:

- ask the user for permission first
- do not use Gemini CLI without explicit user approval
- use it as a discussion/sanity-check aid, not as a replacement for the crate's documented design process
- after using it, fold any relevant conclusions back into `ARCHITECTURE.md`, `TODOS.md`, or `MEMORY.md` as appropriate

## Practical workflow reminders

- Keep changes test-driven.
- Prefer end-to-end tests on R snippets for user-visible behavior.
- Prefer snapshot tests for rendered diagnostics.
- Prefer running focused crate tests while iterating.
- `cargo test -p typing` is the default crate test command.
- `cargo nextest run -p typing` is available, but use whichever Rust test runner is most appropriate for the task.
- Use the repository `justfile` as the first place to look for shared development and snapshot commands.
- Keep fixture-based snapshot names stable by treating `group__case` as the snapshot identity.
- Reject duplicate fixture `group__case` names across the suite instead of silently shadowing one case with another.
- When intentionally changing diagnostic output, use the repository snapshot review workflow rather than updating snapshots blindly.
- Preserve or improve source-range fidelity when making inference or lowering changes.
- Keep interning, lowering, and inference decisions consistent with `ARCHITECTURE.md`.
- Do not reintroduce end-to-end named-argument mismatch fixtures until function-parameter lowering can represent the needed semantics.