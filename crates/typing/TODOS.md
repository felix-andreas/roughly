# `typing` TODOs

This document tracks actionable planned work for the `typing` crate.

`SEMANTICS.md` is the user-facing contract. `ARCHITECTURE.md` describes implementation constraints. Keep this file focused on unfinished work.

## Planning rules

- Discuss important semantic changes with the user before implementation.
- Keep `SEMANTICS.md`, `ARCHITECTURE.md`, and fixture expectations aligned when behavior changes.
- Prefer concrete unfinished tasks over phase narration.
- Delete or rewrite stale tasks instead of preserving historical plans.

## Current priorities

### Split `types.rs` By Phase

- Separate the current mixed type data model into phase-shaped representations.
  - Move HIR attachment metadata such as `AttachedAnnotation` out of `types.rs` and into `hir.rs`.
  - Keep syntax-layer type forms grouped as front-end data instead of mixing them with typechecker internals.
  - Keep `CoreType`, `TypeScheme`, and inference-only identifiers as the semantic internal layer used by `typecheck.rs`.
  - Decide whether resolved type references need their own representation immediately after naming or can stay as a follow-up project.
- Reasoning:
  - `types.rs` currently mixes surface syntax, HIR attachment concerns, and typechecker internals in one file, while `lower.rs` and `hir.rs` now have much cleaner phase boundaries.
  - The current `SurfaceType` shape still uses raw `Symbol`s with no stable ids for nested type references, which is awkward now that naming owns type-name resolution.
  - Cleaning this split should make later project-global type resolution and semantic named-type support easier to add without further phase leakage.

### Type Name Resolution In Naming

- Finish the naming-phase type-resolution project.
  - Add wrong-generic-arity checks in naming instead of parser-local or lowering-local validation.
  - Introduce an explicit resolved type-name result for downstream consumers instead of having naming only emit diagnostics.
  - Resolve type references against the project-global type namespace once multi-file naming exists.
  - Make `typecheck` consume resolved type-name information from naming rather than reinterpreting raw `SurfaceType` trees.

### Typecheck Boundaries

- Reshape typechecking around the new boundaries.
  - Make `typecheck` consume the naming output (`BindingId`) instead of raw lowered `Symbol` names.
  - Map builtin and project-global resolved names to stable identities so `typecheck` can look them up consistently.
  - Keep builtin typing, compatibility logic, and interface extraction inside `typecheck.rs` for now.
  - Replace the current inference-centric API with checked-file results that fit the new architecture.

### Checked File And Interfaces

- Expose checked-file and project-summary boundaries.
  - Define the checked-file result owned by `pipeline.rs`.
  - Retain diagnostics, typed results, and any project-summary extraction needed at that boundary.
  - Use those summaries as one possible dependency boundary for later project scheduling and incremental invalidation.

### Project Rechecking

- Add the project-level rechecking foundation without overcommitting.
  - Keep single-file rechecking fast.
  - Make project-visible names and any later checked-file summaries the dependency boundary.
  - Ensure dependent files can be rechecked when earlier project-visible names change, even if those dependent files are not open.
  - Avoid committing yet to reuse of inference or unification state across edits.

### Project-Global Type Namespace

- Add project-level type declaration collection and resolution.
  - Decide and document the exact semantics for same-file forward references and cross-file type references.
  - Collect top-level `@type` and `@alias` declarations across the project before resolving type references.
  - Make duplicate type-name diagnostics project-wide rather than file-local.
  - Feed that project-wide type namespace into naming without changing value-name semantics.
  - Keep room for incremental recomputation when one file changes.
- Expand the fixture harness for multi-file naming and diagnostics cases.
  - Define a fixture input format that can represent more than one file.
  - Add naming fixtures for cross-file type references and cross-file duplicate declarations once semantics are settled.
  - Keep single-file fixtures simple; multi-file syntax should be opt-in rather than forced on every suite.
- Reasoning:
  - If type names become project-global and order-independent, file-local naming is no longer enough for type resolution.
  - The current fixture harness only models one file at a time, so it cannot express the intended cross-file behavior.
  - This work is a prerequisite for discussing project-global type tooling behavior with confidence.

### Fixture Harness Multi-File Generations

- Detailed plan: `projects/000_fixture_harness_multi_file_generations.md`
- `typing::workspace` and the `fixtures` crate parser/runner milestones are implemented.
- Teach `typing` suite renderers to execute workspace-style generations instead of only `Simple`
  cases and current `naming` `MultiFile` cases.
- Then adopt those APIs in more `typing` fixture cases and in `roughly`.
- Reasoning:
  - Package-global naming semantics cannot be tested properly with the current single-file fixture shape.
  - Later incremental package-recheck behavior will also need generation-based fixture cases.
  - Reusing the existing incremental tree update path matters for correctness and for later benchmarking of incremental typing.
  - Once the harness gains its own language, it needs direct parser and harness tests so syntax or project-state changes do not silently break the suite.

### AnalysisState Simplification And Package Removal

- Detailed plan: `projects/003_analysis_state_simplification_and_package_removal.md`
- Make `AnalysisState` the sole owner of documents and durable phase state.
- Remove `Package` from the typing pipeline.
- Make fixture tests use `AnalysisState` directly instead of package-specific setup.

### Naming Phase Split And Global Resolution

- Detailed plan: `projects/002_naming_phase_split_and_global_resolution.md`
- Introduce stable module identity into naming storage.
- Keep local naming file-scoped and make package-global value resolution use one final symbol table.
- Store naming results in a tooling-friendly shape instead of relying on package-wide HIR remapping.
