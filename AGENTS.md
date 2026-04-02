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

# Additional Guidlines

When working on the `typing` crate also read the `crates/typing/AGENTS.md` file.

# Rules Hygiene

The `AGENTS.md` file is read by every agent session. Keep them high-signal.

## After any agentic session

## High bar for new rules

Editing or clarifying existing rules is always welcome. New rules must meet **all three** criteria:
1. **Non-obvious** — someone familiar with the codebase would still get it wrong without the rule.
2. **Repeatedly encountered** — it came up more than once (multiple hits in one session counts).
3. **Specific enough to act on** — a concrete instruction, not a vague principle.

Rules that apply to a single crate belong in that crate's own `AGENTS.md` file, not the repo root.

## What NOT to put in `AGENTS.md`

Avoid architectural descriptions of a crate (module layout, data flow, key types). These go stale fast and the agent can gather them by reading the code. Rules should be **traps to avoid**, not **maps to follow**.
