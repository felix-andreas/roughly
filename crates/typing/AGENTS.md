# Roughly Type Checker

This crate is written by AI under human guidance and supervision. The markdown documents in this crate are the primary means for steering the AI: they are where intent, contracts, design decisions, and reviewable plans must be made explicit so the human can validate and redirect the work. Keep these documents aligned with the implementation. If behavior, design, or plans change, update the relevant documents in the same session.

## Goals

- Deliver high-quality diagnostics for R in the style of Rust and Elm.
- Support language-tooling features such as hover and inlay hints, so preserve the semantic information needed for them whenever practical.
- Scale to very large code bases, including code bases larger than 300,000 LoC; performance matters.
- Prefer clear, precise, actionable diagnostic wording.
- Avoid overly internal or theory-heavy diagnostic language when user-facing wording would be clearer.
- Keep fixture expectations updated only when wording or behavior intentionally improves.
- Prefer precise source ranges over coarse fallback ranges whenever possible.

## Skills

If the user says:

- `get-started`: read the relevant crate documents, then continue with the next actionable item in `TODOS.md` (assume fresh context unless the documents indicate otherwise).
- `cleanup memory`: aggressively remove resolved, stale, or low-value session-specific details, while preserving this purpose section and any continuity that will still matter next session.

## Documents

Before making significant changes in this crate, review these files and keep them aligned. These documents are the primary way to steer AI work in this crate, so keeping them current is part of the implementation work.

- `README.md`
  - Human-facing crate overview and pointers to the authoritative documents.
  - Avoid agent workflow details and implementation-status churn.
- `SEMANTICS.md`
  - Authoritative meaning of the type checker and authoritative user-facing typing semantics contract.
  - Keep it concise, explicit, and in sync with the fixture tests.
  - All semantic changes must be discussed with the user first.
- `ARCHITECTURE.md`
  - Authoritative design decisions and architectural constraints.
  - Keep only durable design decisions and constraints, not a changelog or session diary.
  - If implementation changes the design, or if the intended direction conflicts with this document, discuss it with the user before proceeding and update the document in the same session if the change is accepted.
- `TESTING.md`
  - Authoritative fixture-testing contract and suite structure.
  - This is the primary contract for how fixture-based tests validate the low-level implementation details of the type checker.
  - Keep it aligned with `tests/test_fixtures.rs` and the actual fixture directories.
  - Keep only the minimal focused test-running information: suite names and the `TYPING_FILTER` workflow.
- `TODOS.md`
  - Actionable plan.
  - Keep only concrete unfinished work.
  - Remove completed "next steps" once they stop helping planning.
- `MEMORY.md`
  - Handoff-only continuity that is easy to lose between sessions.
  - Keep it clean; remove resolved points, stale notes, and low-value session chatter.
  - Do not repeat stable design, finished work, or obvious current code state.
  - If the remaining context is unclear or disputed, discuss it with the user instead of preserving ambiguous notes.

Keep all crate documents extremely high signal:

- remove stale status notes
- remove vague process language
- remove low-value session chatter
- prefer concise, actionable updates
- do not duplicate the same information across documents

When in doubt, delete weak context instead of preserving it.

## Collaboration with the user

This crate is developed collaboratively with the user, with the documents above used to steer the AI.

- Important design decisions must be discussed with the user before implementation.
- If a task in `TODOS.md` is marked `(needs refinement)`, or if a task appears stale because semantics have moved on, stop and discuss it before proceeding.
- Do not silently lock in semantics for difficult language-design questions.
- If user-facing semantics are unclear, discuss them with the user first and then write the resolved semantics down in `SEMANTICS.md`.
- Keep interning, lowering, and inference decisions consistent with `ARCHITECTURE.md`.
- Prefer making uncertainty explicit over guessing.

## Testing strategy

- `TESTING.md` is authoritative for the fixture setup and how fixtures are used to validate the low-level implementation details of the type checker.
- Prefer fixtures because they are the primary way to validate the type checker, they are easy for humans to read in diffs, and they make it easy to create many tests quickly.
- Prefer fixture-based tests for rendered diagnostics, normalized inference behavior, naming output, interface rendering, and other source-driven behavior whenever possible.
- Prefer adding or tightening fixtures before writing parser-local or engine-local unit tests unless the behavior is genuinely awkward to express as a fixture.
- Favor fixture renderers that expose semantic facts rather than implementation detail.
- When adding a new phase or module, add or extend a fixture suite for that phase before relying on ad hoc unit tests.
- Use the lightest fixture change that captures the failing shape.
- Read `TESTING.md` before changing the fixture harness or adding a new fixture suite.
- Run focused fixture cases with `TYPING_FILTER=group__case cargo test -p typing --test test_fixtures <suite> -- --nocapture`.
- Prefer running focused crate tests while iterating.
- `cargo test -p typing` is the default crate test command.
- Keep fixture `group__case` names stable as the test identity.
- Reject duplicate fixture `group__case` names across the suite instead of silently shadowing one case with another.
- Review fixture expectation changes deliberately when output changes.
- Treat `SEMANTICS.md` and the fixture suite as contract documents; keep them in sync.
- Do not change `SEMANTICS.md` without discussing it with the user first.
- Do not reintroduce end-to-end named-argument mismatch fixtures until function-parameter lowering can represent the needed semantics.
