# Fixture Harness Multi-File Generations [planning]

## Goal

Extend the fixture harness so one test case can describe a `project`, not only a single file, and so grouped generations can model incremental project edits.

This work should reuse the same incremental tree update path already used by `roughly` instead of reimplementing rope and tree-sitter edit logic inside the typing fixture harness.

The intended direction is to introduce a new crate for the reusable project/edit engine rather than burying that logic inside the typing fixture harness.

## Unresolved questions

- How should expectations attach across generations:
  - one expectation per generation
  - final generation only
  - or either, depending on explicit markers?
- What should the new crate be called, and what should its exact API boundary be?
- Where should parser tests and end-to-end harness tests live if the project/edit engine is split out?

## Settled direction

- Use `project` as the main abstraction for multiple documents.
- Parse each fixture case into an initial project snapshot plus later generations.
- Treat each generation as one grouped project edit step.
- Run analysis after each generation.
- Keep single-file cases as the default when no generation block is present.
- Use explicit generation blocks with `#.... vN`.
- Use bare filenames such as `#---- a.R` for whole-file content in a generation.
- Support first-class operations for:
  - whole-file replacement
  - range edits
  - delete
  - move / rename
- Prefer full-file restatement in small tests.
- Allow range edits for large files or edit-heavy tests.
- Start with an R-specific reusable project/edit layer.

## Planned work

### 1. Finalize fixture syntax [planning]

- Define the exact grammar for:
  - backward-compatible single-file cases
  - `#.... vN` generation blocks
  - whole-file entries using bare filenames
  - `delete`, `move`, and `edit` operations
  - expected-output attachment across generations
- Record the syntax and authoring guidance in `TESTING.md` once settled.

### 2. Add parser tests for the fixture language [planning]

- Cover:
  - valid single-file cases
  - valid multi-file initial generations
  - valid multi-generation cases
  - grouped edits in the same generation
  - whole-file replacement
  - range edits
  - delete
  - move
- Reject clearly:
  - malformed nesting
  - duplicate generation labels in one case
  - ambiguous file or operation entries
  - invalid operation syntax

### 3. Build the reusable incremental `Project` engine [planning]

- Introduce a new crate for the reusable project/edit engine.
- Reuse the same `Rope`, `Tree::edit`, and reparsing behavior currently used by `roughly`.
- Support:
  - add file
  - replace file
  - edit file range
  - delete file
  - move file
- Preserve tree reuse across generations.
- Keep the API suitable for later reuse by `ServerState`.

### 4. Wire the fixture harness to the `Project` engine [planning]

- Parse a fixture case into:
  - initial project state
  - later grouped generations
- Apply one generation at a time.
- Expose the resulting project state to each test suite.
- Let each consumer decide whether it inspects every generation or only selected ones.

### 5. Add direct harness tests [planning]

- Add parser-focused tests for the new fixture mini-language.
- Add project-evolution tests that exercise grouped generations and incremental edits.
- Add backward-compatibility tests so existing single-file fixtures remain supported.

### 6. Add real typing fixtures that use the new format [planning]

- Start with multi-file naming fixtures.
- Then add multi-file diagnostics fixtures.
- Later add project-recheck and incremental-typing fixtures when the semantics and APIs are ready.

## Why this project exists

- Project-global naming semantics cannot be tested properly with the current single-file fixture shape.
- Later incremental and project-recheck behavior also need generation-based fixture cases.
- Reusing the existing incremental tree update path matters for correctness and for later benchmarking of incremental typing.
- Once the harness gains its own language, it needs direct parser and harness tests so syntax or project-state changes do not silently break the suite.
