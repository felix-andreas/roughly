# Typing Crate Discussion

This file is scratch space for active design discussion.

Use it for short summaries or temporary notes during a design conversation.

## Current topic

Naming architecture for multi-file packages.

Question:

- Should package naming keep using one temporary merged module, or should it split into a file-local naming pass plus a project-global resolution pass?

Current assessment:

- The current merged-module approach is acceptable as a short-term implementation device inside analysis.
- It is probably not the right long-term shape if we want clean incremental analysis, per-file diagnostics, and tooling-friendly naming data.
- The file-local phase should fully resolve non-global lexical uses before the project-global pass runs.

Why the merged-module approach is attractive:

- it is simple to get working
- it reuses the existing lexical walker with minimal new abstractions
- it naturally models package-global value ordering when files are visited in project order
- it keeps fixture migration moving while the project-level APIs are still forming

Why the merged-module approach is awkward:

- it erases file boundaries and then has to reconstruct them with side tables
- per-file diagnostics and per-file rendered outputs become extra bookkeeping instead of a natural consequence of the data model
- incremental invalidation becomes less explicit because the naming walk is phrased as one package-sized traversal
- it couples project-global resolution and file-local lexical resolution into one pass, which makes later tooling queries harder to explain and cache
- it encourages “project naming by synthetic concatenation” instead of “project naming over explicit per-file artifacts”

Leaning:

- A better long-term design is a two-stage naming pipeline:

1. File-local naming preparation
   - walk each lowered file independently
   - build local lexical scopes and local binding identities
   - collect top-level value declarations
   - collect top-level type declarations
   - resolve all references that can be decided by file-local lexical scope
   - record only the unresolved references that may need project-global lookup
   - keep all results keyed by real file paths and file-local ids

2. Project-global resolution
   - build package-global declaration tables from the collected top-level data
   - resolve unresolved top-level value references against package-global declarations
   - resolve type references against the package-global type namespace
   - diagnose duplicate top-level declarations and cross-file collisions
   - produce one project naming result that still points back to per-file data

Reasoning about the split:

- Value naming really has two different rules:
  - lexical lookup inside executable code
  - package-global lookup for top-level names across files
- Those rules are easier to maintain and cache if they are separate phases.

What the file-local pass should do:

- assign binding identities for top-level assignments in the file
- assign binding identities for parameters, local assignments, and loop variables
- resolve any use that can be decided from lexical structure already present in the file
- when lookup falls out of local scopes, record an unresolved candidate instead of deciding too early that it is missing

What should remain unresolved after the file-local pass:

- value references whose answer may come from the package-global top-level environment
- type references that depend on the package-global type namespace
- cross-file duplicate and shadowing diagnostics that require the package-wide view

Why I prefer resolving locals eagerly in the file-local pass:

- local lexical resolution is intrinsic to one file and does not need project assembly
- this gives cleaner incremental caches because local facts do not depend on package merging
- the project-global pass can then focus only on genuinely package-wide questions
- it avoids a second pass that still has to understand ordinary local scope mechanics

What I would avoid:

- a first pass that only collects declarations but leaves all expression-name resolution for the global pass

That would keep too much responsibility in the global phase and would lose most of the architectural value of splitting naming at all.

Why this split seems better:

- local lexical reasoning stays local
- project-global semantics become explicit instead of implicit in one merged traversal
- cross-file diagnostics naturally retain file ownership
- later incremental work can invalidate one file’s local naming facts separately from the global tables
- rename and go-to-definition become easier to phrase over stable per-file artifacts plus project-global indexes

Main cost of the split:

- it needs a more explicit intermediate representation for unresolved references and top-level declarations
- some logic that is currently one traversal becomes two coordinated passes
- binding identity design must stay stable across those passes

Recommendation for near-term work:

- Treat the current merged-module naming as a temporary internal analysis implementation, not the desired architecture.
- When we next reshape naming or analysis state, move toward explicit per-file naming artifacts plus a project-global resolution pass.
- Make the file-local pass authoritative for locals, and the project-global pass authoritative for cross-file top-level value and type resolution.
- Use distinct project-level ids for top-level declarations instead of reusing provisional file-local ids.

## Settled points

- Naming should split into file-local preparation and project-global resolution.
- The file-local naming pass should resolve local lexical facts eagerly.
- The project-global naming pass should assign distinct project-level ids for top-level declarations.
- Fixture runners now return `Result<Vec<FixtureOutput>, String>`.
- Each `FixtureOutput` is one snapshot with per-file outputs keyed by path.
- `Err(...)` is runner failure, not a rendered phase result.
- Expectations carry forward across generations by path.
- `#++++ any` means the immediately preceding file or operation is expected, but its contents are
  not asserted.
- `delete` removes the carried expectation for that path.
- `move` carries the expectation from the source path to the destination path.
- Extra actual outputs beyond expected paths are fixture failures.

## Open decisions

- None currently recorded.
