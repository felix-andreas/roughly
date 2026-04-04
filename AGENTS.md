# ONGOING MIGRATION

We are currenlty in an ongoing migration. The entire anlysis from roughly was re-written in the analysis crate (including a type checker). we are now moving roughly step by step towards this new type checker. (REMOVE THIS NOTE WHEN MIGRATION IS FINISHED)

# Rust coding guidelines

* Do not write organizational or comments that summarize the code. Comments should only be written in order to explain "why" the code is written in some way in the case there is a reason that is tricky / non-obvious.
* Prefer implementing functionality in existing files unless it is a new logical component. Avoid creating many small files.
* Avoid using functions that panic like `unwrap()`, instead use mechanisms like `?` to propagate errors.
* Be careful with operations like indexing which may panic if the indexes are out of bounds.
* Never silently discard errors with `let _ =` on fallible operations.
* Never create files with `mod.rs` paths - prefer `src/some_module.rs` instead of `src/some_module/mod.rs`.
* When creating new crates, prefer specifying the library root path in `Cargo.toml` using `[lib] path = "...rs"` instead of the default `lib.rs`, to maintain consistent and descriptive naming (e.g., `gpui.rs` or `main.rs`).
* Avoid creative additions unless explicitly requested
* Use full words for variable names (no abbreviations like "q" for "queue")
* Prefer importing types directly. For functions, prefer at least one module-level import instead of fully qualifying every call; fully qualified paths are still fine when needed to avoid ambiguity.
* Organize modules top-down. Put core types and public functions first, order container types before the types they contain, and keep private types and helper functions after public items in the same caller-before-callee order.
* Do not optimize for the smallest safe fix. When you touch an area, bring it to the intended shape for that change, remove dead paths or temporary seams, and pay down nearby technical debt needed to keep the code coherent. You are responsible for code quality, not just feature delivery.
* Avoid helper-function indirection when logic is only used once and does not materially improve testability or readability. Prefer inlining small one-off solutions unless doing so would create large duplication.

## Error handling

- Do not swallow analysis, synchronization, or document-loading errors in this crate or its integrations.
- If an operation is required to keep analysis state coherent, surface the failure immediately with `panic!` rather than logging and continuing with corrupted or stale state.
- In particular, document-sync or analysis-sync failures in the LSP path are unrecoverable and should panic immediately rather than trying to keep the server alive in a bad state.
- Example: if syncing an open document into analysis state fails during `did_open`, `did_change`, or `did_save`, do not fall back to stale state or best-effort logging; `panic!`.

# Rules Hygiene

The `AGENTS.md` file is read by every agent session. Keep it extremely high-signal.

## High bar for new rules

Editing or clarifying existing rules is always welcome. New rules must meet **all three** criteria:
1. **Non-obvious** — someone familiar with the codebase would still get it wrong without the rule.
2. **Repeatedly encountered** — it came up more than once (multiple hits in one session counts).
3. **Specific enough to act on** — a concrete instruction, not a vague principle.

Rules that apply to a single crate belong in that crate's own `AGENTS.md` file, not the repo root.

## What NOT to put in `AGENTS.md`

Avoid architectural descriptions of a crate (module layout, data flow, key types). These go stale fast and the agent can gather them by reading the code. Rules should be **traps to avoid**, not **maps to follow**.

# Analysis Crate

This crate is written by AI under human guidance and supervision. The markdown documents in `agent/` are the primary means for steering the AI: they are where intent, contracts, design decisions, and reviewable plans must be made explicit so the human can validate and redirect the work. Keep these documents aligned with the implementation. If behavior, design, or plans change, update the relevant documents in the same session.

## Goals

- Deliver high-quality diagnostics for R in the style of Rust and Elm.
- Support language-tooling features such as hover and inlay hints, so preserve the semantic information needed for them whenever practical.
- Scale to very large code bases, including code bases larger than 300,000 LoC; performance matters.
- Keep single-file rechecking fast while still reporting dependent type errors across the project when project-visible names change.
- Prefer clear, precise, actionable diagnostic wording.
- Avoid overly internal or theory-heavy diagnostic language when user-facing wording would be clearer.
- Keep fixture expectations updated only when wording or behavior intentionally improves.
- Prefer precise source ranges over coarse fallback ranges whenever possible.

## Skills

If the user says:

- `get started`: read the relevant steering documents and `MEMORY.md`, then continue with the next actionable item in `TODOS.md` (assume fresh context unless the documents indicate otherwise).
- `cleanup memory`: aggressively remove resolved, stale, or low-value session-specific details, while preserving this purpose section and any continuity that will still matter next session.
- `code check`: review the relevant code for compliance with local coding guidelines. Report findings first and explicitly verify top-down module ordering plus the preferred `use` qualification style; in Rust, types should usually be imported directly, and functions should usually have at least one module-level import instead of repeated fully qualified calls unless ambiguity requires qualification.
- `discuss`: move the active design discussion into `DISCUSS.md`, or into the relevant `projects/` file if the topic already has a project; continue the discussion there in later turns, not only in chat, and remove resolved points as they are settled.
- `authorative check`: compare the authoritative documents against the fixture suites and report contradictions, stale wording, or missing documented coverage.
- `implementation check`: compare the implementation against the authoritative documents and report contract or architecture mismatches.
- `session check`: do an end-of-session closure pass. Verify that decisions, open questions, and new work discovered during the session are either resolved or captured in the right documents; look especially for thread sprawl where side investigations created uncaptured follow-up work. Check that `TODOS.md`, `projects/`, `DISCUSS.md`, and the authoritative documents are consistent, then report anything still hanging.

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
- `TYPING_SEMANTICS.md`
  - Authoritative desired typing semantics contract.
  - Keep it concise, explicit, and in sync with the agreed semantics fixtures.
  - Do not rewrite it to match temporary implementation gaps
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
  - Keep only the minimal focused test-running information: suite names and the `FIXTURE_FILTER` workflow.

### Working documents

Update working documents proactively during the session (located in `agent/`):

- `TECHNICAL_DEBT.md`
  - Current structural debt and implementation seams that should be paid down deliberately.
  - Keep it focused on present debt, not speculative future work or session history.
- `DECISION_LOG.md`
  - Durable record of settled decisions discussed with the user.
  - Keep each entry in `decision` then `reason` form.
  - Reflect durable decisions into the authoritative documents when the wording there is ready.
- `projects/`
  - Detailed project plans for larger multi-step efforts that would make `TODOS.md` too crowded.
  - Name project files with a three-digit numeric prefix followed by a short snake_case title, for example `000_fixture_harness_multi_file_generations.md`.
  - Each project file should declare one top-level project state: `[planning]`, `[in-progress]`, `[done]`, or `[discarded]`.
  - Put unresolved questions near the top of each project file, before the implementation plan, so they are easy to notice.
  - Discuss and settle those unresolved questions with the user before starting implementation work on that project.
  - Individual tasks inside a project should also carry explicit state markers so progress is visible within the file.
  - Task-level states may also use `[blocked]` when progress is waiting on a decision or prerequisite.
  - Keep `TODOS.md` as the index of active work and reference the relevant project file there instead of duplicating the full plan.

### Ephemeral documents

Update ephemeral documents proactively during the session.
Delete or trim ephemeral documents once their content is resolved or no longer useful.

- `TODOS.md`
  - Actionable plan.
  - Keep only concrete unfinished work.
  - Use concise bullets and short nested bullets when they clarify sequencing.
  - If a task is marked `(needs refinement)` or appears stale, discuss it with the user before acting on it.
- `DISCUSS.md`
  - Scratch space for active design discussion.
  - When a turn is an active design discussion, put the substantive answer in `DISCUSS.md`, not only in chat.
  - Keep it short and move anything durable into `DECISION_LOG.md`.
  - Keep a dedicated `Open decisions` section inside this file and update it after each focused design discussion pass.
- `MEMORY.md`
  - Inter-session memory only: use it for continuity that is easy to lose between sessions, not as a general session dump.
  - Keep it clean by default; remove resolved points, stale notes, and low-value session chatter automatically.
  - Only preserve a broader dump of session details when the user explicitly asks for that.
  - Do not repeat stable design, finished work, or obvious current code state.
  - If the remaining context is unclear or disputed, discuss it with the user instead of preserving ambiguous notes.

### Hygiene

Keep steering documents high-signal:

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
- Run focused fixture cases with `FIXTURE_FILTER=group__case cargo test -p analysis --test test_fixtures <suite> -- --nocapture`.
- Prefer running focused crate tests while iterating.
- `cargo test -p analysis` is the default crate test command.
- Keep fixture `group__case` names stable as the test identity.
- Reject duplicate fixture `group__case` names across the suite instead of silently shadowing one case with another.
- Review fixture expectation changes deliberately when output changes.
- Do not reintroduce end-to-end named-argument mismatch fixtures until function-parameter lowering can represent the needed semantics.
