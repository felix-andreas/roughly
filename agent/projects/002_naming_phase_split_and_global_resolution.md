# Naming Phase Split And Global Resolution [in-progress]

## Goal

Reshape naming so it has an explicit file-local preparation step and an explicit package-global resolution step, with package-global value lookup using one final symbol table.

The result should support:

- package-global value resolution without remembering earlier overwritten definitions at use sites
- duplicate-global diagnostics
- a standalone `run_naming` phase entry point
- stored naming state that tooling can query for go-to-definition and completion
- a path toward incremental recomputation where one changed file does not require recomputing file-local naming for every file

## Non-goals

- fully redesign typechecking in this project
- settle every later incremental dependency detail before landing the phase split
- require a separate tree-shaped `NamedModule` if side tables remain sufficient

## Unresolved questions

- Whether `run_typecheck` should switch immediately to consuming naming-owned resolved identities, or in a follow-up slice after `run_naming` is stable
- Whether resolved type-reference storage should land in the same project or a follow-up project after the value-side split is complete

## Settled direction

- `lower` and `naming` stay separate phases
- `analysis.rs` should expose a standalone `run_naming`
- `run_naming` must consume already-lowered package state rather than running lowering internally
- HIR `Module` stays the structural representation
- per-file semantic analysis should use stable module identity rather than path-keyed tables
- introduce a `ModuleId` or equivalent stable file-analysis identity
- workspace and analysis state may still map paths to modules, but naming storage should not use paths as the primary semantic key
- naming data may remain side tables rather than a second tree-shaped `NamedModule`
- the resolved naming state must be stored so tooling can query it
- file-local naming must not resolve package-global names, even within the same file
- file-local naming should record explicit top-level exports and unresolved references
- package-global value resolution should:
  - build one final symbol table for the package
  - resolve every unresolved global reference against that final table
  - use the latest declaration in package order as the winning definition
  - not preserve earlier overwritten bindings at use sites
- duplicate top-level value definitions should still produce diagnostics
- imports and builtins should be passed into package-global resolution through explicit lookup tables, even if the initial implementation uses dummy data

## Target shape

### Phase API

Near-term public orchestration should be:

- `run_lowering`
- `run_naming`
- `run_typecheck`
- `check`

### File-local naming result

Each file-local naming pass should produce explicit artifacts:

- stable file identity
- local expression resolutions
- introduced bindings
- top-level exported value bindings
- top-level exported type declarations
- unresolved value references
- annotations or type-reference work items that need package-global resolution

The package pass should consume those artifacts rather than rescanning the full HIR tree.

### Package-global naming result

The package-global pass should produce durable side tables for:

- final binding identities
- expression-to-binding resolutions
- package-global value index for tooling and completion
- resolved type-reference information
- naming diagnostics

Those tables should be keyed by stable semantic ids, not by filesystem paths. The intended shape is:

- `ModuleId`
- `(ModuleId, ExpressionId)` for expression-local facts
- `(ModuleId, DefinitionId)` for definition-local facts

Paths should remain an integration lookup owned by workspace or analysis state:

- `PathBuf -> ModuleId`
- `ModuleId -> current path` when needed for diagnostics or editor integration

That keeps semantic caches stable across rename operations and avoids threading paths through every naming table.

### Why this matters for incremental analysis

The current package remapping approach is package-wide work:

- clone expressions from every module
- rewrite embedded local ids
- rebuild package modules in one shared arena

That is the wrong cost model for incremental naming.

The target model for this project is:

- keep HIR ids local to one module
- keep naming results local to one module
- merge only compact package-level summaries such as exports and unresolved references

This avoids full-package HIR remapping as a prerequisite for naming and gives a much better base for:

- one-file edits
- file rename without semantic identity churn
- future dependency-driven invalidation

## Implementation plan

### 1. Add the explicit project plan and settle the package-global semantics [done]

- capture the two-phase naming direction
- capture the final-symbol-table semantics for package-global values
- capture the requirement that naming state remain queryable for tooling

### 2. Expose a standalone `run_naming` phase entry point [done]

- add `run_naming` in `analysis.rs`
- keep the public API explicit enough that tests and later tooling can call naming directly
- keep the public API explicit enough that tests and later tooling can call naming directly

### 3. Make the file-local pass produce explicit package-resolution artifacts [done]

- introduce `ModuleId` or equivalent stable file identity into the naming state shape
- stop using path as the primary key for file-local naming results
- extend the local naming result with:
  - top-level value exports
  - unresolved value references
  - annotation work items needed later
- keep local lexical resolution behavior unchanged
- do not resolve globals during the local pass

### 4. Rebuild package-global value resolution around one final symbol table [done]

- build the final package-global value table from file-local exports
- key package-global and file-local naming facts by `ModuleId`-based node identities
- use the final winning binding for every unresolved global reference
- emit duplicate-global diagnostics
- store the final package-global value index in the naming result

### 5. Stop using a second recursive expression walk for value resolution [done]

- resolve local and global value references from the local-pass side tables
- keep only the minimum package-global work needed for annotations and type declarations
- if needed, collect annotation work items during the local pass so the package pass can avoid a full expression-tree walk
- stop requiring package-wide HIR remapping for naming

### 6. Update fixtures and diagnostics coverage [in-progress]

- split naming fixtures into `naming_local` and `naming_global`
- update naming fixtures to match final-symbol-table semantics
- add explicit duplicate-global diagnostic coverage
- make naming snapshots non-blocking in the presence of warnings

### 7. Stage later consumers onto naming-owned resolved state [pending]

- make `typecheck` consume naming-owned binding identities
- add resolved type-reference storage for downstream consumers
- move toward durable project-level naming caches in `AnalysisState`

### Follow-up after this project [pending]

- decide whether `ModuleId` should be owned by lowering state, workspace, or a shared project-analysis identity allocator
