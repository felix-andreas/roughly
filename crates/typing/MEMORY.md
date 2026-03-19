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

- `src/type_syntax.rs` is currently intentionally reduced to a compiling stub at the top-level annotation/type parser entrypoint.
- `parse_annotation_type()` currently returns:
  - `InvalidSyntax { message: "TODO: replace the current type-syntax stub with a recursive-descent parser." }`
- The old in-progress parser attempts were removed because repeated local fixes were not converging and were leaving the file in a misleading state.
- Keep the helper code that still supports expanded annotation parsing and rendering unless the rewrite makes replacement clearly better.

## Recursive-descent parser conception

The next implementation should be a clean recursive-descent parser, not another patch on the removed approach.

Recommended shape:

- One parser state struct holding:
  - source text
  - byte position
  - interner
- One primary entry:
  - `parse_type()`
- One contextual recursive entry:
  - `parse_type_until(stop_context)`

The key contract for `parse_type_until(stop_context)` should be:

- parse exactly one type expression at the current level
- after parsing a primary expression, inspect the next character
- if the next character belongs to the caller boundary for this level, return immediately
- only continue through `|` when the current context allows nullable-union continuation
- do not rely on substring reparsing for nested constructs as the main strategy

## Data-oriented performance guidance

The replacement parser should be performance-conscious and low-allocation from the start.

Preferred constraints:

- operate over the original source slice with byte offsets
- avoid building temporary substring `String`s during parsing
- avoid reparsing nested bodies by calling the top-level parser on extracted substrings
- prefer returning borrowed ranges / offsets during parsing and only constructing owned values at the semantic output boundary
- keep parser state in a compact struct with simple scalar fields
- prefer small plain-data context structs / enums over trait objects or deeply layered parser abstractions
- avoid recursive helper designs that allocate intermediate collections unless they are part of the final `SurfaceType`
- for identifier parsing, prefer scanning spans in-place and interning from slices rather than allocating temporary owned identifier text first
- for delimiter matching, prefer one-pass cursor movement instead of pre-scanning large regions into separate buffers

Practical implication:

- `list[...]`, `list{...}`, and `fn(...)` should be parsed directly from the shared cursor over the original input
- nested constructs should consume their own delimiters in-place and return with the cursor already positioned after the construct
- stop-context handling should be expressed as simple branch checks on the next byte / char, not by slicing and reparsing

## Delimiter ownership model

Each syntactic form should consume its own opening and closing delimiters internally:

- `list[...]`
  - consume `list[`
  - parse its body
  - consume the matching `]`
- `list{...}`
  - consume `list{`
  - parse tuple-like or record-like items
  - consume the matching `}`
- `fn(...)`
  - consume `fn(`
  - parse parameter list
  - consume the matching `)`
  - optionally parse `-> return_type`

Nested constructs must fully consume their own delimiters before returning.

## Stop-context conception

Use an explicit stop-context structure that describes which delimiters belong to the caller:

- stop on `,` for record items / tuple items / function parameters
- stop on `]` when parsing inside `list[...]`
- stop on `}` when parsing a record or tuple field/item value
- stop on `)` when parsing function parameters
- do not stop before `|` in contexts that allow nullable unions

Important rule:

- boundary-stop checks should happen after parsing the primary expression and before deciding whether to continue parsing more syntax at the same level

## Parsing strategy by construct

### Lists

For `list[...]`:

- first detect whether the body starts with `named:`
- if yes:
  - parse exactly one value type after `named:`
  - that value type may itself be nested `list[...]`, `list{...}`, or `fn(...)`
- if not:
  - parse exactly one element type

### Records / tuples

For `list{...}`:

- parse one item at a time until `}`
- detect `name:` to distinguish record fields from tuple items
- reject mixing named and unnamed items in the same `list{...}`

### Functions

For `fn(...)`:

- parse parameters one at a time until `)`
- detect `name:` to distinguish named parameters from positional parameters
- then optionally parse `-> return_type`

## Known failure shape that must be covered first

The critical regression case to solve first is:

- `list{meta:list{items:list[named:list{integer,character}}}}`

Related nearby cases that should also pass:

- `list[named:list{integer,character}]`
- `list{items:list[named:list{integer,character}]}`
- `list{meta:list{items:list[named:list{integer,character}],render:character}}`
- `list{meta:list{items:list[named:list{integer,character}],render:fn(integer)->list{label:character}}}`

The difficult edge case is when a nested named-list value is followed by stacked closing delimiters with no sibling field after it.

## Recommended implementation discipline

- Start from the minimal parser-only unit tests for the exact failing strings.
- Keep the focused `tests/test_lib.rs` parser regression tests for nested named-list shapes.
- Once the parser is stable, remove any temporary tracing and dead helper code.
- Only then re-evaluate fixture expectations and broader crate tests.