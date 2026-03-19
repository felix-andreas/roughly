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

This is not a replacement for `AGENTS.md`, `ARCHITECTURE.md`, `SEMANTICS.md`, or `TODOS.md`.

- `AGENTS.md` contains crate-specific working rules and workflow expectations.
- `ARCHITECTURE.md` is the maintained design contract.
- `SEMANTICS.md` is the user-facing semantics contract and must stay in sync with fixture expectations.
- `TODOS.md` is the maintained execution plan.
- `MEMORY.md` is a compact continuity document for session-to-session handoff.

If code changes make this document inaccurate, update it in the same session.

## Active continuity

- `src/type_syntax.rs` is in the middle of a parser rewrite from delimiter-splitting helpers toward a cursor-based `TypeParser`.
- The old splitter-based root cause is no longer the main blocker; the remaining failure is still the `tests/types` fixture `record_like_lists__deeply_nested_record_like_list`.
- The exact failing source remains:
  - `list{meta:list{items:list[named:list{integer,character}},render:fn(integer)->list{label:character}}}`
- The parser currently succeeds on several nearby shapes, including:
  - `list{items:list[named:list{integer,character}]}`
  - `list{items:list[named:list{integer,character}],render:fn(integer)->list{label:character}}`
  - `list{meta:list{render:fn(integer)->list{label:character}}}`
  - `list{meta:list{items:list[named:list{integer,character}],render:character}}`
  - `list{meta:list{items:list[named:list{integer,character}],render:fn(integer)->character}}`
  - `list{meta:list{items:list[named:character|NULL],render:character}}`
- The current failing error moved during the rewrite:
  - earlier: `invalid syntax in type expression (while parsing field 'items') (while parsing field 'meta')`
  - then: `missing closing delimiter ] (while parsing field 'items') (while parsing field 'meta')`
  - latest: `unexpected closing delimiter } (while parsing field 'meta')`
- This strongly suggests the remaining bug is in record-field boundary detection for nested values, not in the basic parsing of `list[...]`, `list{...}`, or `fn(...)` individually.
- `parse_record_field_value(...)` was introduced to slice one record field value before reparsing it, but it still needs correct stopping behavior at the containing record boundary.
- Best next steps:
  - debug `parse_record_field_value(...)` in `src/type_syntax.rs`
  - make it stop at the sibling comma for the current record depth without consuming the containing `}` into the field slice
  - rerun the focused failing fixture:
    - `TYPING_FILTER=deeply_nested_record_like_list cargo test -p typing --test test_fixtures types -- --nocapture`
  - then rerun:
    - `cargo test -p typing`
- Fixture workflow clarification from this session:
  - prefer expressing parser debugging through `tests/types/*.R.test` first
  - use focused fixture execution as the default loop for type-syntax work
  - keep parser-local unit tests only for parser machinery that is awkward to express through fixtures
