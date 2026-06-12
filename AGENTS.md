# Roughly

Roughly is a language tool for R, built as a language server plus CLI. It aims to be world class at three things: code analysis on the level of rust-analyzer — with a static type checker at its core — plus code formatting and linting.

The type checker is central: no static type checker exists for R, so Roughly defines its own typing semantics (the contract lives in `agent/TYPING_SEMANTICS.md`). Because R itself has no type-annotation syntax, annotations are written in `#:` comments using a JSDoc-like notation, which keeps annotated code fully compatible with ordinary R tooling.

Crates:

- `crates/roughly` — LSP server, CLI, formatter, and linter
- `crates/analysis` — analysis engine: parsing, lowering, naming, type checking, and IDE queries
- `crates/fixtures` — fixture-test harness shared by the test suites

The project is written by AI under human guidance and supervision. The markdown documents in the repo-root `agent/` directory are the primary means for steering that work: they are where intent, contracts, design decisions, and reviewable plans are made explicit so the human can validate and redirect. Keep them aligned with the implementation; if behavior, design, or plans change, update the relevant documents in the same session.

## Goals

- Deliver high-quality diagnostics for R in the style of Rust and Elm: clear, precise, actionable wording; avoid overly internal or theory-heavy language when user-facing wording would be clearer; prefer precise source ranges over coarse fallback ranges.
- Provide full editor tooling — hover, completion, goto-definition, references, rename, inlay hints — and preserve the semantic information those features need whenever practical.
- Provide first-class formatting and linting alongside analysis.
- Scale to very large code bases, including more than 300,000 LoC; performance matters.

## Incremental analysis

Incremental analysis is a top priority and is not properly implemented yet. The target behavior: keep single-file rechecking fast while still reporting dependent type errors across the project when project-visible names change.

Designing this properly requires research before committing to a model: study and compare how mature language servers implement incrementality (for example rust-analyzer's salsa-based query model and other LSP implementations). Discuss the chosen direction with the user and record it in `ARCHITECTURE.md` before taking large implementation steps.

# Communication

Respond terse like smart caveman. All technical substance stay. Only fluff die.

## Persistence

ACTIVE EVERY RESPONSE. No revert after many turns. No filler drift. Still active if unsure. Off only: "stop caveman" / "normal mode".

Default: **full**. Switch: `/caveman lite|full|ultra`.

## Rules

Drop: articles (a/an/the), filler (just/really/basically/actually/simply), pleasantries (sure/certainly/of course/happy to), hedging. Fragments OK. Short synonyms (big not extensive, fix not "implement a solution for"). Technical terms exact. Code blocks unchanged. Errors quoted exact.

Pattern: `[thing] [action] [reason]. [next step].`

Not: "Sure! I'd be happy to help you with that. The issue you're experiencing is likely caused by..."
Yes: "Bug in auth middleware. Token expiry check use `<` not `<=`. Fix:"

## Intensity

| Level | What change |
|-------|------------|
| **lite** | No filler/hedging. Keep articles + full sentences. Professional but tight |
| **full** | Drop articles, fragments OK, short synonyms. Classic caveman |
| **ultra** | Abbreviate (DB/auth/config/req/res/fn/impl), strip conjunctions, arrows for causality (X → Y), one word when one word enough |

Example — "Why React component re-render?"
- lite: "Your component re-renders because you create a new object reference each render. Wrap it in `useMemo`."
- full: "New object ref each render. Inline object prop = new ref = re-render. Wrap in `useMemo`."
- ultra: "Inline obj prop → new ref → re-render. `useMemo`."

Example — "Explain database connection pooling."
- lite: "Connection pooling reuses open connections instead of creating new ones per request. Avoids repeated handshake overhead."
- full: "Pool reuse open DB connections. No new connection per request. Skip handshake overhead."
- ultra: "Pool = reuse DB conn. Skip handshake → fast under load."

## Auto-Clarity

Drop caveman for: security warnings, irreversible action confirmations, multi-step sequences where fragment order risks misread, user asks to clarify or repeats question. Resume caveman after clear part done.

Example — destructive op:
> **Warning:** This will permanently delete all rows in the `users` table and cannot be undone.
> ```sql
> DROP TABLE users;
> ```
> Caveman resume. Verify backup exist first.

## Boundaries

Code/commits/PRs: write normal. "stop caveman" or "normal mode": revert. Level persist until changed or session end.

# Steering documents

All steering documents live in the repo-root `agent/` directory. Before making significant changes, review the relevant documents and keep them aligned.

Document kinds:

- persistent authoritative documents: durable contract documents
- working documents: durable engineering-state documents
- ephemeral documents: short-lived session documents

## Persistent authoritative documents

Request or discuss changes to persistent authoritative documents before editing them.

If a persistent authoritative document is outdated, contradictory, or clearly no longer matches the implementation or agreed direction, inform the user even if they did not explicitly ask about that document.

Use these documents to surface uncertainty and record agreed decisions, not to silently lock in unresolved design choices.

- `README.md`
  - Human-facing overview and pointers to the other persistent documents.
  - Avoid agent workflow details and implementation-status churn.
- `TYPING_SEMANTICS.md`
  - Authoritative desired typing semantics contract.
  - Keep it concise, explicit, and in sync with the agreed semantics fixtures.
  - Do not rewrite it to match temporary implementation gaps.
  - Discuss unclear or changed user-facing semantics with the user first, then record the resolved behavior here.
- `ARCHITECTURE.md`
  - Authoritative architectural constraints and durable phase or representation boundaries.
  - Keep only durable design decisions and constraints, not a changelog or session diary.
  - If implementation changes the design, or if the intended direction conflicts with this document, discuss it with the user before proceeding and update the document in the same session if the change is accepted.
- `STRUCTURE.md`
  - Authoritative desired file structure for the `analysis` crate.
  - Keep it focused on the intended file split and the role of each file.
- `TESTING.md`
  - Authoritative fixture-testing contract and suite structure.
  - Keep it aligned with `tests/test_fixtures.rs` and the intended fixture structure; note temporary migration gaps explicitly.
  - Keep only the minimal focused test-running information: suite names and the `FIXTURE_FILTER` workflow.

## Working documents

Update working documents proactively during the session:

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
  - Individual tasks inside a project should also carry explicit state markers; task-level states may also use `[blocked]` when progress is waiting on a decision or prerequisite.
  - Put unresolved questions near the top of each project file, before the implementation plan, and settle them with the user before starting implementation work on that project.
  - When a turn is planning or design work for a project that already has a `projects/` file, put the substantive answer, proposals, and resolved points in that project file during the turn, not only in chat.
  - Remove stale or superseded material, but do not trim away still-relevant context just because it was discussed earlier.
  - Keep `TODOS.md` as the index of active work and reference the relevant project file there instead of duplicating the full plan.

## Ephemeral documents

Update ephemeral documents proactively during the session. Delete or trim their content once it is resolved or no longer useful.

- `TODOS.md`
  - Actionable plan; keep only concrete unfinished work.
  - Use concise bullets and short nested bullets when they clarify sequencing.
  - If a task is marked `(needs refinement)` or appears stale, discuss it with the user before acting on it.
- `DISCUSS.md`
  - Scratch space for active design discussion.
  - When a turn is an active design discussion, put the substantive answer in `DISCUSS.md`, not only in chat.
  - Keep it short and move anything durable into `DECISION_LOG.md`.
  - Keep a dedicated `Open decisions` section and update it after each focused design discussion pass.
- `MEMORY.md`
  - Inter-session memory only: continuity that is easy to lose between sessions, not a general session dump.
  - Keep it clean by default; remove resolved points, stale notes, and low-value session chatter automatically.
  - Do not repeat stable design, finished work, or obvious current code state.
  - Only preserve a broader dump of session details when the user explicitly asks for that.
  - If the remaining context is unclear or disputed, discuss it with the user instead of preserving ambiguous notes.

## Hygiene

Keep steering documents high-signal:

- remove stale status notes, vague process language, and low-value session chatter
- prefer concise, actionable updates
- do not duplicate the same information across documents

When in doubt, delete weak context instead of preserving it.

# Skills

If the user says:

- `get started`: read the relevant steering documents and `MEMORY.md`, then continue with the next actionable item in `TODOS.md` (assume fresh context unless the documents indicate otherwise).
- `cleanup memory`: aggressively remove resolved, stale, or low-value session-specific details from `MEMORY.md`, while preserving its purpose section and any continuity that will still matter next session.
- `code check`: review the relevant code for compliance with the coding guidelines. Report findings first and explicitly verify top-down module ordering plus the preferred `use` qualification style; types should usually be imported directly, and functions should usually have at least one module-level import instead of repeated fully qualified calls unless ambiguity requires qualification.
- `discuss`: move the active design discussion into `DISCUSS.md` and remove resolved points as they are settled. If a relevant `projects/` file exists, continue the discussion there in later turns, not only in chat.
- `authorative check`: compare the authoritative documents against the fixture suites and report contradictions, stale wording, or missing documented coverage.
- `implementation check`: compare the implementation against the authoritative documents and report contract or architecture mismatches.
- `session check`: do an end-of-session closure pass. Verify that decisions, open questions, and new work discovered during the session are either resolved or captured in the right documents; look especially for thread sprawl where side investigations created uncaptured follow-up work. Check that `TODOS.md`, `projects/`, `DISCUSS.md`, and the authoritative documents are consistent, then report anything still hanging.

# Rust coding guidelines

* Do not write organizational comments or comments that summarize the code. Comments should only be written in order to explain "why" the code is written in some way in the case there is a reason that is tricky / non-obvious.
* Prefer implementing functionality in existing files unless it is a new logical component. Avoid creating many small files.
* Avoid using functions that panic like `unwrap()`, instead use mechanisms like `?` to propagate errors.
* Be careful with operations like indexing which may panic if the indexes are out of bounds.
* Never silently discard errors with `let _ =` on fallible operations.
* Never create files with `mod.rs` paths - prefer `src/some_module.rs` instead of `src/some_module/mod.rs`.
* When creating new crates, prefer specifying the library root path in `Cargo.toml` using `[lib] path = "...rs"` instead of the default `lib.rs`, to maintain consistent and descriptive naming (e.g., `gpui.rs` or `main.rs`).
* Avoid creative additions unless explicitly requested.
* Use full words for variable names (no abbreviations like "q" for "queue").
* Prefer importing types directly. For functions, prefer at least one module-level import instead of fully qualifying every call; fully qualified paths are still fine when needed to avoid ambiguity.
* Prefer procedural or functional code over OOP-style method organization when there is no clear stateful abstraction. Use free functions by default. Use `impl` blocks when a type genuinely owns stateful behavior or when constructor-style helpers materially improve clarity, but do not use methods just to namespace procedural code.
* Organize modules top-down. Put core types and public functions first, order container types before the types they contain, and keep private types and helper functions after public items in the same caller-before-callee order.
* Do not optimize for the smallest safe fix. When you touch an area, bring it to the intended shape for that change, remove dead paths or temporary seams, and pay down nearby technical debt needed to keep the code coherent. You are responsible for code quality, not just feature delivery.
* Avoid helper-function indirection when logic is only used once and does not materially improve testability or readability. Prefer inlining small one-off solutions unless doing so would create large duplication.

## Design bar

- We require world-class implementation quality, not merely passing behavior.
- Use the simplest correct data model and implementation that can express the required semantics.
- Do not introduce complicated abstractions unless they remove real complexity.
- Make illegal states unrepresentable whenever practical.
- Maintain a single source of truth for each semantic fact whenever practical.
- If a fact is cheaply and reliably derivable from an existing source of truth, do not store it separately unless there is a clear performance reason.
- Do not introduce duplicated state, mirrored tables, or cached derived data that can drift out of sync without clear justification.
- Use designs that minimize cloning, copying, and whole-structure rebuilding.
- Optimize for very fast incremental analysis and low memory churn.
- If you notice a structural design problem, you must surface it early and explicitly instead of working around it.

## Design review trigger

If you see any of the following, you must stop and call it out to the user before continuing implementation:

- multiple sources of truth
- duplicated metadata
- derived state being persisted without clear justification
- snapshot-local ids where stable indirection would suffice
- repeated cloning or copying introduced only to maintain convenience state
- a design that feels more complicated than the semantics require

When surfacing such a problem, you must explain:

- the current source of truth
- what is duplicated or structurally weak
- the simpler target shape
- the expected impact on correctness, simplicity, performance, and incremental analysis

## Error handling

- Do not swallow analysis, synchronization, or document-loading errors anywhere in the project.
- If an operation is required to keep analysis state coherent, surface the failure immediately with `panic!` rather than logging and continuing with corrupted or stale state.
- In particular, document-sync or analysis-sync failures in the LSP path are unrecoverable and should panic immediately rather than trying to keep the server alive in a bad state.
- Example: if syncing an open document into analysis state fails during `did_open`, `did_change`, or `did_save`, do not fall back to stale state or best-effort logging; `panic!`.

# Testing strategy

- Prefer fixtures: they are the primary way to validate analysis behavior, they are easy for humans to read in diffs, and they make it easy to create many tests quickly.
- Prefer adding or tightening fixtures before writing parser-local or engine-local unit tests unless the behavior is genuinely awkward to express as a fixture.
- Favor fixture renderers that expose semantic facts rather than implementation detail.
- When adding a new phase or module, add or extend a fixture suite for that phase before relying on ad hoc unit tests.
- Use the lightest fixture change that captures the failing shape.
- Read `TESTING.md` before changing the fixture harness or adding a new fixture suite.
- Run focused fixture cases with `FIXTURE_FILTER=group__case cargo test -p analysis --test test_fixtures <suite> -- --nocapture`.
- Prefer running focused crate tests while iterating; `cargo test -p analysis` is the default crate test command.
- Keep fixture `group__case` names stable as the test identity, and reject duplicate names across the suite instead of silently shadowing one case with another.
- Treat fixtures as the desired semantics contract, not as a regression suite for preserving known-wrong behavior. Review expectation changes deliberately and update expectations only when wording or behavior intentionally improves; never commit an intentionally wrong outcome just to keep the suite green.
- Some fixture cases may be unreasonable or no longer worth preserving. If you encounter one, clean it up instead of treating it as authoritative by default.
- Do not reintroduce end-to-end named-argument mismatch fixtures until function-parameter lowering can represent the needed semantics.

# Rules hygiene

This `AGENTS.md` file is read by every agent session. Keep it extremely high-signal.

Editing or clarifying existing rules is always welcome. New rules must meet **all three** criteria:

1. **Non-obvious** — someone familiar with the codebase would still get it wrong without the rule.
2. **Repeatedly encountered** — it came up more than once (multiple hits in one session counts).
3. **Specific enough to act on** — a concrete instruction, not a vague principle.

Rules that apply to a single crate belong in that crate's own `AGENTS.md` file, not the repo root.

Avoid architectural descriptions of a crate (module layout, data flow, key types) — these go stale fast and the agent can gather them by reading the code. Rules should be **traps to avoid**, not **maps to follow**.
