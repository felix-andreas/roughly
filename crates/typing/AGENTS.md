# Roughly Type Checker

This crate is written by AI under human guidance and supervision. The markdown documents in this crate are the primary means for steering the AI: they are where intent, contracts, design decisions, and reviewable plans must be made explicit so the human can validate and redirect the work. Keep these documents aligned with the implementation. If behavior, design, or plans change, update the relevant documents in the same session.

## Goals

- Deliver high-quality diagnostics for R in the style of Rust and Elm.
- Support language-tooling features such as hover and inlay hints, so preserve the semantic information needed for them whenever practical.
- Scale to very large code bases, including code bases larger than 300,000 LoC; performance matters.
- Keep single-file rechecking fast while still reporting dependent type errors across the project when exported interfaces change.
- Prefer clear, precise, actionable diagnostic wording.
- Avoid overly internal or theory-heavy diagnostic language when user-facing wording would be clearer.
- Keep fixture expectations updated only when wording or behavior intentionally improves.
- Prefer precise source ranges over coarse fallback ranges whenever possible.

## Skills

If the user says:

- `get-started`: read the relevant steering documents and `MEMORY.md`, then continue with the next actionable item in `TODOS.md` (assume fresh context unless the documents indicate otherwise).
- `cleanup memory`: aggressively remove resolved, stale, or low-value session-specific details, while preserving this purpose section and any continuity that will still matter next session.

## Steering Documents

This crate is developed under user guidance and steering.

The documents below are the primary means for steering work in this crate.

Document kinds:

- persistent authoritative documents: durable contract documents
- working documents: durable engineering-state documents
- ephemeral documents: short-lived session documents

Before making significant changes in this crate, review these files and keep them aligned.

### Persistent authoritative documents

Request or discuss changes to persistent authoritative documents before editing them.

If a persistent authoritative document is outdated, contradictory, or clearly no longer matches the implementation or agreed direction, inform the user even if they did not explicitly ask about that document.

Use these documents to surface uncertainty and record agreed decisions, not to silently lock in unresolved design choices.

- `README.md`
  - Human-facing crate overview and pointers to the other persistent documents.
  - Avoid agent workflow details and implementation-status churn.
- `SEMANTICS.md`
  - Authoritative typing semantics contract.
  - Keep it concise, explicit, and in sync with the fixture tests.
  - Discuss unclear or changed user-facing semantics with the user first, then record the resolved behavior here.
- `ARCHITECTURE.md`
  - Authoritative architectural constraints and durable phase or representation boundaries.
  - Keep only durable design decisions and constraints, not a changelog or session diary.
  - If implementation changes the design, or if the intended direction conflicts with this document, discuss it with the user before proceeding and update the document in the same session if the change is accepted.
- `STRUCTURE.md`
  - Authoritative desired file structure for the crate.
  - Keep it focused on the intended file split and the role of each file.
- `TESTING.md`
  - Authoritative fixture-testing contract and suite structure.
  - Keep it aligned with `tests/test_fixtures.rs` and the intended fixture structure; note temporary migration gaps explicitly.
  - Keep only the minimal focused test-running information: suite names and the `TYPING_FILTER` workflow.

### Working documents

Update working documents proactively during the session.

- `TECHNICAL_DEBT.md`
  - Current structural debt and implementation seams that should be paid down deliberately.
  - Keep it focused on present debt, not speculative future work or session history.
- `DECISION_LOG.md`
  - Durable record of settled decisions discussed with the user.
  - Keep each entry in `decision` then `reason` form.
  - Reflect durable decisions into the authoritative documents when the wording there is ready.

### Ephemeral documents

Update ephemeral documents proactively during the session.
Delete or trim ephemeral documents once their content is resolved or no longer useful.

- `TODOS.md`
  - Actionable plan.
  - Keep only concrete unfinished work.
  - Use concise bullets and short nested bullets when they clarify sequencing.
  - If a task is marked `(needs refinement)` or appears stale, discuss it with the user before acting on it.
- `OPEN_DECISIONS.md`
  - Active unresolved design questions and their current options.
  - Keep it focused on decisions that still need user input.
- `DISCUSS.md`
  - Scratch space for active design discussion.
  - Keep it short and move anything durable into `DECISION_LOG.md` or `OPEN_DECISIONS.md`.
- `MEMORY.md`
  - Handoff-only continuity that is easy to lose between sessions.
  - Keep it clean; remove resolved points, stale notes, and low-value session chatter.
  - Do not repeat stable design, finished work, or obvious current code state.
  - If the remaining context is unclear or disputed, discuss it with the user instead of preserving ambiguous notes.

### Hygiene

Keep all steering documents extremely high signal:

- remove stale status notes
- remove vague process language
- remove low-value session chatter
- prefer concise, actionable updates
- do not duplicate the same information across documents

When in doubt, delete weak context instead of preserving it.

## Testing strategy

- Prefer fixtures because they are the primary way to validate the type checker, they are easy for humans to read in diffs, and they make it easy to create many tests quickly.
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
- Do not reintroduce end-to-end named-argument mismatch fixtures until function-parameter lowering can represent the needed semantics.
