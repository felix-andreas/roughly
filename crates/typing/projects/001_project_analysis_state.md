# Project Analysis State And Phase APIs [planning]

## Goal

Introduce a better project-level analysis state for `typing` so that:

- multi-file analysis does not require fixture-harness-specific merge logic
- repeated analysis after fixture generations has one coherent state owner
- fixture tests can use normal typing-crate APIs with small explicit phase calls

The immediate target is lowering and naming.
The design must also leave room for later typechecking and project rechecking.

## Non-goals

- redesign the full typechecking architecture in this project
- settle every later incremental scheduling detail up front
- force fixture tests into one exact helper shape beyond using normal typing APIs

## Unresolved questions

- None currently recorded.

## Settled direction

- `Workspace` stays outside `AnalysisState`
- the likely long-lived owner is `ProjectSession { workspace, analysis }`
- `AnalysisState` is one top-level state object with explicit per-phase caches inside it
- walkers such as lowering and naming contexts stay transient
- per-document lowered results are durable cached analysis data
- project file order is defined by `Workspace`
- naming results must be reusable for tooling such as go-to-definition and rename
- naming should split into file-local preparation plus project-global resolution
- the file-local naming pass should resolve local lexical facts eagerly and leave only package-wide questions unresolved
- top-level declarations should receive distinct project-level identities during the project-global naming pass instead of reusing provisional file-local ids

## State model

### Top-level owner

The intended long-lived owner is:

```rust
struct ProjectSession {
    workspace: Workspace,
    analysis: AnalysisState,
}
```

This owner is suitable for:

- fixture tests
- generation-based fixture updates
- later editor-style incremental analysis

### Persistent state

Persistent state is long-lived derived analysis data that survives across repeated analyses of one evolving workspace.

The intended persistent state is:

```rust
struct AnalysisState {
    interner: Interner,
    lowering: LoweringState,
    naming: NamingState,
}
```

```rust
struct LoweringState {
    documents: BTreeMap<PathBuf, LoweredDocument>,
    project_index: LoweredProjectIndex,
    dirty_documents: BTreeSet<PathBuf>,
}
```

```rust
struct NamingState {
    project_result: Option<ProjectNamingResult>,
    file_results: BTreeMap<PathBuf, FileNamingResult>,
    dirty_documents: BTreeSet<PathBuf>,
}
```

Persistent state should include:

- shared interner
- per-document lowered results
- project-level lowered indexes
- per-document naming preparation results
- naming results
- invalidation metadata

Here, invalidation metadata means the bookkeeping that marks which cached results are stale after document edits and which later phases need recomputation.

### Transient state

Transient state is rebuilt during phase execution and is not kept as the durable representation.

Transient state should include:

- lowering walkers
- naming walkers
- temporary phase-local assembly data

## Phase contracts

The project should make phase inputs and outputs explicit.

### Workspace

Input owner:

- document text
- parsed trees
- package or standalone membership
- project file order

`Workspace` is the source of truth for parse state and document membership.
It is not semantic analysis state.

### Lowering

Inputs:

- `&Workspace`
- `&mut AnalysisState`

Persistent inputs read:

- shared interner
- existing lowering cache
- dirty-document metadata

Persistent outputs written:

- `LoweringState.documents`
- `LoweringState.project_index`
- updated dirty-document metadata for later phases
- any interned names needed by the lowered results

Transient outputs:

- lowering walkers and temporary assembly data used only while recomputing

Conceptual API:

```rust
fn run_lowering(
    workspace: &Workspace,
    analysis: &mut AnalysisState,
) -> Result<&LoweringState, AnalysisError>
```

The first durable project-level artifact after parsing is the lowering cache plus the project-level lowered index derived from it.
That index may include:

- ordered document list
- top-level declarations by document
- any other project-level lowered lookup needed before naming

### Naming

Inputs:

- `&Workspace`
- `&mut AnalysisState`

Persistent inputs read:

- shared interner
- current lowering cache
- project file order from `Workspace`
- existing naming cache

Persistent outputs written:

- `NamingState.project_result`
- `NamingState.file_results`
- updated naming invalidation metadata

Transient outputs:

- project-global lookup tables assembled from per-file naming artifacts
- naming walkers and temporary lookup structures

Conceptual API:

```rust
fn run_naming(
    workspace: &Workspace,
    analysis: &mut AnalysisState,
) -> Result<&ProjectNamingResult, AnalysisError>
```

The naming result should support:

- binding identities
- definition sites
- use sites
- cross-file naming facts needed for go-to-definition
- future rename support

The intended split inside naming is:

1. file-local preparation
   - resolve local lexical references inside one file
   - collect top-level declarations
   - record unresolved references that may require package-global lookup
2. project-global resolution
   - build package-global declaration tables
   - resolve cross-file top-level values and project-global type references
   - assign project-level identities to package-visible declarations

### Later phases

Later phases should follow the same pattern:

- input: `&Workspace`
- state: `&mut AnalysisState`
- output: references or handles to durable cached phase results

This project does not need to fully design typechecking, but it should establish the boundary that later phases consume earlier cached semantic results instead of rebuilding them ad hoc in tests or renderers.

## Fixture API target

Fixture helpers should only:

- create a workspace from fixture input
- apply grouped fixture edits to that workspace

Phase execution in tests should use normal typing APIs and stay explicit.

Conceptual test shape:

```rust
let mut session = fixture_session(fixture)?;
run_lowering(&session.workspace, &mut session.analysis)?;
let naming = run_naming(&session.workspace, &mut session.analysis)?;
```

That shape is not a hard API requirement, but it captures the intended boundary:

- setup helpers only manage the workspace
- analysis helpers own semantic caches
- tests call the phases they inspect

## Implementation plan

### 1. Introduce the project session and analysis caches [pending]

- add the first `ProjectSession` or equivalent top-level owner
- add `AnalysisState` with explicit `LoweringState` and `NamingState`
- keep the initial state model limited to the fields needed for lowering and naming

### 2. Move lowering results into durable per-document state [pending]

- stop treating `LoweringContext` as the long-lived owner of lowered state
- store lowered documents in `LoweringState`
- build a project-level lowered index from the per-document results

### 3. Move multi-file naming assembly into analysis code [pending]

- remove fixture-harness-specific merge or remap logic from `tests/test_fixtures.rs`
- build per-file naming preparation results inside analysis state
- build project-global declaration tables from those per-file artifacts
- make naming consume analysis-owned project inputs instead of fixture-owned ad hoc ones
- assign project-level identities during project-global resolution rather than reusing provisional file-local ids

### 4. Expose normal phase APIs for tests and later consumers [pending]

- add `run_lowering` and `run_naming` style entry points
- keep phase calls explicit in tests
- keep fixture helpers focused on workspace creation and updates

### 5. Migrate naming fixtures to the new APIs [pending]

- rewrite `run_naming_fixture` around workspace plus analysis state
- make single-file and multi-file cases share the same phase path
- preserve current naming behavior and snapshots unless behavior intentionally changes

### 6. Leave a clean extension point for later phases [pending]

- make the new state layout clearly ready for later typechecking caches
- do not over-design typechecking in this project
- record any follow-up work discovered during implementation in `TODOS.md` or a later project plan
