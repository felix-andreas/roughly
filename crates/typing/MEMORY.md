# `typing` Memory

This document stores cross-session context for the `typing` crate.

Its purpose is to preserve important implementation state, open questions, and loose ends between agentic sessions, especially when the active context window is too small to carry the full design and implementation history forward.

Use this document to record:

- current implementation status
- unresolved design questions
- diagnostic quality goals
- known technical debt
- next recommended steps
- any subtle decisions that should not be rediscovered from scratch

This is not a replacement for `ARCHITECTURE.md` or `TODOS.md`.

- `ARCHITECTURE.md` is the maintained design contract.
- `TODOS.md` is the maintained execution plan.
- `MEMORY.md` is a compact continuity document for session-to-session handoff.

If code changes make this document inaccurate, it should be updated in the same session.

## Collaboration rules to remember

- Important design decisions must be discussed with the user before implementation.
- If a todo is marked `(needs refinement)`, stop and discuss it before proceeding.
- Keep `ARCHITECTURE.md`, `TODOS.md`, and this file aligned when implementation meaningfully changes.
- Diagnostic quality is a core product goal. We want Elm- and Rust-like error messages: clear, precise, actionable, and user-centered.

## Current implementation status

The crate already has the following pieces in place:

### Crate structure

Current modules:

- `check`
- `diagnostics`
- `infer`
- `interner`
- `lower`
- `parse`
- `types`

The crate is library-first, with a minimal binary wrapper still present.

### Parser and syntax diagnostics

Implemented:

- tree-sitter R parser setup
- syntax parsing entry points
- syntax diagnostics rendering
- snapshot tests for syntax errors

### Lowering

Implemented lowering support for:

- top-level sequences
- identifiers
- `NULL`
- `TRUE` / `FALSE`
- integer literals
- float literals
- string literals
- assignments via `<-` and `=`
- function definitions
- calls
- simple wrapped expressions:
  - parenthesized expressions
  - braced expressions
- unsupported fallback nodes

Important current limitation:

- unsupported constructs currently lower to `Unsupported`
- nested names inside unsupported constructs are not yet recursively interned or lowered

### Interning

Implemented:

- `Symbol`
- `Interner`
- string interning for repeated names
- resolving symbols back to source text

Current intended uses:

- variable names
- function parameter names
- record field names
- later builtin and lexical environments

### Type representations

Implemented:

- `Atomic`
- `SurfaceType`
- `CoreType`
- `TypeScheme`
- `InferenceVariableId`
- `RecordField<T>`
- `FunctionType<T>`

Important type decisions already made:

- distinguish `integer` and `double`
- no subtyping in v1
- no variance in v1
- no union types in v1
- internal generics/polymorphism are part of v1
- no explicit generic syntax in v1

### Inference engine foundation

Implemented:

- inference-variable state
- fresh variable creation
- representative lookup
- path compression
- occurs check
- unification for:
  - atomic scalar/vector equality
  - `List`
  - `Tuple`
  - `Record`
  - `Function`
- simple lexical environment keyed by `Symbol`

### Monomorphic expression inference

Implemented expression inference for:

- literals
- symbol lookup
- assignment
- function expressions
- calls
- unsupported expressions -> `Unknown`

Important limitation:

- this is currently monomorphic usage inference only
- no let-generalization / instantiation yet

### End-to-end type diagnostics

Implemented:

- lowering + inference are now run from `check()`
- inference failures are converted into user-facing diagnostics
- snapshot tests exist for:
  - unknown name
  - call argument type mismatch
  - calling a non-function

Important limitation:

- diagnostic ranges for type errors are still coarse
- current type diagnostics may still expose implementation-oriented type rendering in some cases
- the present messages are a baseline, not the final Elm/Rust-quality target

## Known loose ends

### 1. Type error ranges are not yet precise

Current behavior:

- type diagnostics use a fallback range instead of the most relevant expression range

Desired direction:

- carry precise failure ranges through inference errors
- report the exact call expression / symbol / argument / body region that caused the problem

This is likely the highest-value next improvement for diagnostic quality.

### 2. Diagnostic wording still needs refinement

Current diagnostics are usable but not yet at the target quality bar.

Needs work:

- reduce raw debug-style type output
- make mismatch messages more explanatory
- refer to source constructs more directly
- prefer user-facing language over internal terminology

### 3. Let-polymorphism is not implemented yet

Architecture says:

- monomorphic inference first
- then let-polymorphism

Current state:

- monomorphic inference exists
- generalization and instantiation do not yet exist

Important consequence:

- identity-style reuse across unrelated call sites is not yet truly HM-polymorphic

### 4. Unsupported syntax behavior is still minimal

Current behavior:

- unsupported expressions become `Unknown`
- nested names inside unsupported syntax are not processed

This may affect:
- diagnostics
- future type flow
- user expectations

Needs discussion before broadening behavior.

### 5. `if` is still unresolved

Open design question:

- should `if` be in the first serious semantic slice after the current foundation,
  or should it wait until after better diagnostics / polymorphism / annotations?

### 6. Lists / tuples / records are in the type model but not yet fully lowered/inferred from R syntax

The semantic rules are already decided:

- homogeneous positional => `List`
- heterogeneous positional => `Tuple`
- named => `Record`
- mixed named + unnamed => type error

But the full syntax-to-lowered support for these forms is not yet finished.

### 7. Annotation parsing is not started

Still pending:

- parsing `#:` comment annotations
- attaching them to assignments/functions
- converting `SurfaceType` annotations into `CoreType` constraints

## Important design decisions already settled

These should not be reopened casually without discussing with the user:

- separate `integer` and `double`
- unsupported syntax infers `Unknown`
- internal generics are part of v1
- no explicit generic syntax in v1
- string interning should be used in lowering/inference
- inference-variable state should use path compression
- diagnostics should aim for Elm/Rust quality
- development should be test-driven
- end-to-end tests should use R snippets
- rendered diagnostics should be snapshot-tested
- important design decisions must be discussed with the user first

## Recommended next step

The best next step is:

1. improve type diagnostic precision and quality

Concretely:

- attach precise source ranges to inference failures
- stop using the fallback-first-line range for type errors
- improve error wording for:
  - unknown names
  - type mismatches
  - calling non-functions
  - arity mismatches

Why this is the best next step:

- the type-checking pipeline now exists end to end
- the user explicitly cares about high-quality error messages
- improving range fidelity and message quality will pay off before expanding semantics further

## Recommended step after that

After better diagnostics:

2. implement let-polymorphism

Concretely:

- free type variable computation
- generalization at bindings
- instantiation at use sites
- polymorphism tests on R snippets

That will move the checker from “monomorphic typed subset” toward actual HM behavior.

## Things to be careful about in the next session

- Do not silently change semantic behavior without updating `ARCHITECTURE.md` and `TODOS.md`.
- Do not add broad new syntax support without checking whether it is a meaningful design decision.
- Do not accept rough diagnostics as “good enough”; the explicit quality goal is high.
- Keep using snapshot tests when diagnostic wording or ranges change.
- Preserve the distinction between:
  - syntax layer
  - lowering layer
  - inference layer
  - diagnostic rendering layer

## Session handoff summary

At the end of this session, the `typing` crate has:

- parsing
- syntax diagnostics
- lowering
- interning
- type representations
- inference state with path compression
- monomorphic expression inference
- first end-to-end type diagnostics

The most important unfinished work is now:

- better type diagnostic ranges and wording
- then let-polymorphism
- then annotations and richer R data forms