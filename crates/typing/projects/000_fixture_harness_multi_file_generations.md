# Fixture Harness Multi-File Generations [in-progress]

## Goal

Extend the fixture harness so one test case can describe a `workspace`, not only a single file, and so grouped generations can model incremental document edits across package-attached and standalone files.

This work should reuse the same incremental tree update path already used by `roughly` instead of reimplementing rope and tree-sitter edit logic inside the typing fixture harness.

The intended direction is to introduce:

- a reusable document-management crate for workspace/package/document state plus incremental parsing
- a separate fixture crate that can use that document-management crate later
- the document-management crate should model a workspace that can contain multiple packages plus standalone documents

The first implementation step is only the document-management crate.

## Current status

- [done] Implemented the first milestone as a new `workspace` crate at `crates/workspace`.
- [done] The crate owns parser state, package roots, package and standalone document buckets, and incremental text/tree updates.
- [done] Direct crate tests cover package registration, bucket invariants, range edits, move/delete, and tree reuse across incremental edits.
- [done] Implemented a new `fixtures` crate at `crates/fixtures` that parses single-file, multi-file, and generation-based workspace cases.
- [in-progress] `typing` now reads fixtures through the `fixtures` crate, but its suite renderers still only execute `Simple` cases.
- [planning] Adoption in `roughly` is still pending.

## First milestone

The first milestone is only the `workspace` crate.

That crate is responsible for:

- storing document text as ropes
- storing parsed trees
- applying incremental edits
- routing documents into the right semantic bucket inside one workspace

That crate is not responsible for:

- fixture syntax parsing
- typing or naming analysis
- LSP request or notification types
- `roughly` diagnostics or indexing

## Unresolved questions

- None currently recorded for the first `workspace` crate milestone.

## Settled direction

- Call the new crate `workspace`.
- Use `workspace` as the top-level abstraction.
- Model analysis around `Package` rather than `Project`.
- Keep `Package` as a semantic unit of analysis rather than a filesystem-root owner.
- Store package roots in `Workspace`, not in `Package`.
- Let one `Workspace` contain:
  - multiple packages
  - standalone documents not attached to any package
- Let one `Package` contain:
  - package documents that contribute to the package-level namespace
  - auxiliary documents such as scripts or tests that can see package symbols but do not contribute to the package-level namespace
- Start with an R-specific reusable workspace/document layer.
- Let the `workspace` crate own its parser state.
- The first crate only handles workspace/package/document state and incremental parse updates.
- Keep direct tests for the `workspace` crate in the `workspace` crate itself.
- A separate fixture crate will parse the fixture mini-language.
- In the later testing framework built on top of that parsing:
  - expectations attach per document per generation
  - a generation may explicitly declare no expectation
  - an earlier expectation carries forward until replaced or explicitly cleared
- The testing framework may combine parsed fixture data with `workspace` document state.
- The document-management crate must cover the current document use cases in `roughly/src/server.rs`.
- Support first-class operations for:
  - whole-file replacement
  - range edits
  - delete
  - move / rename

## Core model

The `workspace` crate should model three layers:

- `Workspace`
  - owns parser state
  - owns package roots
  - owns the set of packages
  - owns standalone documents that are attached to no package
- `Package`
  - is the semantic unit of analysis
  - does not own filesystem roots
  - groups the documents that belong to one package
- `Document`
  - stores the current rope and parsed tree for one file
  - is addressed by path within the workspace

Within one package, documents fall into two buckets:

- package documents
  - contribute to the package-level namespace
- auxiliary documents
  - can resolve against package-visible symbols
  - do not contribute back to the package-level namespace

Outside packages, a workspace may also contain:

- standalone documents
  - are attached to no package
  - are still parsed and managed by the workspace

## Behavioral invariants

The implementation should preserve these invariants:

- parser state is owned by `Workspace`
- package roots are owned by `Workspace`
- `Package` remains path-free
- every managed document is in exactly one bucket:
  - package document
  - auxiliary document
  - standalone document
- package membership is explicit in the API rather than inferred ad hoc during later analysis
- package documents contribute to package-global names
- auxiliary documents do not contribute to package-global names
- standalone documents do not belong to any package
- all document mutations update both rope state and parse tree state together
- incremental reparsing should reuse the previous tree whenever possible

## API boundary

The first crate should provide explicit operations around the settled model:

- workspace creation
- package registration
- package-root registration and lookup
- document add or replace with explicit package, auxiliary, or standalone kind
- document lookup by path
- range edit for an existing document
- document delete
- document move or rename

The API should make it possible for consumers to:

- inspect a document's rope
- inspect a document's tree
- distinguish package, auxiliary, and standalone documents
- find the package association for package-attached documents

The API should not require consumers such as `roughly::ServerState` to manage a separate parser.

## Planned work

### 1. Finalize the document-management crate boundary [done]

- Define the public API around:
  - workspace creation
  - package registration and package-root storage
  - document lookup
  - add or replace with explicit kind
  - range edit
  - delete
  - move / rename
- Define how paths map to document buckets in the public API:
  - explicit package document insertion
  - explicit auxiliary document insertion
  - explicit standalone document insertion
- Define the read API needed by `roughly`:
  - document lookup by path
  - access to rope and tree
  - package-root lookup
  - package association lookup where applicable

### 2. Build the reusable incremental document-management crate [done]

- Introduce the new crate.
- Reuse the same `Rope`, `Tree::edit`, and reparsing behavior currently used by `roughly`.
- Support:
  - add package
  - add or replace a document with explicit package, auxiliary, or standalone kind
  - edit document range
  - delete document
  - move document
- Preserve tree reuse across generations.
- Keep the API suitable for later reuse by `ServerState`.
- Keep the implementation focused on one coherent state owner rather than splitting parser ownership or edit bookkeeping across callers.

### 3. Add direct tests for the document-management crate [done]

- Place these tests in the `workspace` crate itself.
- Cover:
  - package registration
  - package document add or replace
  - auxiliary document add or replace
  - standalone document add or replace
  - range edit
  - delete
  - move
  - tree reuse across incremental edits
- Cover the invariants explicitly:
  - documents stay in exactly one bucket
  - auxiliary documents do not become package documents by accident
  - standalone documents stay detached from packages
  - package-root storage does not leak into `Package`
- Cover the document access patterns that `roughly/src/server.rs` depends on.

### 4. Build the separate fixture crate [in-progress]

- Define the exact grammar for:
  - backward-compatible single-file cases
  - `#.... vN` generation blocks
  - whole-document entries using bare filenames
  - `delete`, `move`, and `edit` operations
  - expected-output attachment per document per generation
  - explicit no-expectation markers
- Parse a fixture case into an initial workspace snapshot plus later grouped generations.
- Treat each generation as one grouped workspace edit step.
- Keep single-file cases as the default when no generation block is present.
- `fixtures` now parses the legacy and generation-based formats and has direct parser tests plus workspace-evolution tests.
- `typing/tests/test_fixtures.rs` now uses the `fixtures` crate for parsing and still executes legacy cases unchanged.
- `typing/tests/test_fixtures.rs` now uses the `fixtures` crate for parsing and still executes `Simple` cases unchanged.
- `type_syntax` simple fixtures now compare rendered parser success or failure directly, with no `error:` sentinel embedded in fixture input.
- Run analysis after each generation.
- Prefer full-file restatement in small tests.
- Allow range edits for large files or edit-heavy tests.
- Add parser-focused tests for the fixture mini-language.
- Add workspace-evolution tests that exercise grouped generations and incremental edits.
- Add backward-compatibility tests so existing single-file fixtures remain supported.
- Record the syntax and authoring guidance in `TESTING.md` once settled.
- Keep fixture parser tests with the fixture parser.
- Keep harness end-to-end tests with the testing framework that combines parsed fixtures with `workspace` state.

### 5. Use the new crates for typing fixture cases [planning]

- Start with multi-file naming fixtures.
- Then add multi-file diagnostics fixtures.
- Later add package-recheck and incremental-typing fixtures when the semantics and APIs are ready.

### 6. Adopt the document-management crate in `roughly` [planning]

- Replace the current ad hoc document state and incremental parse path in `roughly/src/server.rs`.
- Keep `ServerState` as the owner of workspace orchestration, indexing state, diagnostics publication, and typing analysis state.
- Make the `roughly` server use the shared document-management crate for file state and parse updates.

## Why this project exists

- Package-global naming semantics cannot be tested properly with the current single-file fixture shape.
- Later incremental package rechecking behavior also needs generation-based fixture cases.
- Reusing the existing incremental tree update path matters for correctness and for later benchmarking of incremental typing.
- The workspace/package/document split must be precise before implementation so `typing`, the future fixture crate, and `roughly` can all reuse the same model instead of growing slightly different state layers.
- Once the harness gains its own language, it needs direct parser and harness tests so syntax or project-state changes do not silently break the suite.
