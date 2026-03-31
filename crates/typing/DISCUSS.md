# Typing Crate Discussion

This file is scratch space for active design discussion.

Use it for short summaries or temporary notes during a design conversation.

## Current topic

### Naming pipeline validation against expected shape

Compared against the expected design:

- There is no standalone pipeline phase named `run_naming`.
  - The current public pipeline entry point is `run_lowering_and_naming` in `src/pipeline.rs`. (we should have this)

- The pipeline does not return resolved modules.
  - `src/naming.rs` returns a `NamingResult` side table.
  - `src/pipeline.rs` returns `PackageLoweringAndNamingResult { modules, naming, diagnostics }`, where `modules` are still raw lowered `Module`s. (we must store this analysis state so things like goto defintion can use them)
  - There is no `ResolvedModule` or `NamedModule` type in the implementation. (this is not that important. as long as we have the side tables somewhere)

- `typecheck` does not consume resolved naming output.
  - `run_typecheck` in `src/pipeline.rs` remaps and merges raw `Module`s again. (should use it as input what we can fix this later)
  - It only uses naming for diagnostics, not for resolved value or type identities.

- The first naming step does resolve locals, but it does not produce the explicit per-module artifacts described in the expected shape.
  - `DocumentNamingContext::resolve_module` records local use-site resolutions plus parameter and loop bindings.
  - It does not return an explicit list of globals exposed by the module. (but don't we need this?)
  - It does not return an explicit list of unknown bindings; unresolved names are stored only as per-expression `UnresolvedValue` entries. (don't we need this?)

- Top-level value exports are not made explicit during the local pass.
  - The implementation creates provisional bindings for top-level assignments during local resolution.
  - The package-global export table is only built later while the second pass walks expressions and inserts seen top-level bindings into `top_level_bindings`. (what is important. the second pass should not walk the entire tree again)

(i think we should be able to cache local naming result after incremantal updates? is this possible? is there a better approach?)

- Global resolution in the second step is not driven by a first-pass "expected globals" list.
  - It revisits every expression and checks unresolved symbols against the evolving `top_level_bindings` map.
  - This means the package-global environment is constructed incrementally during the second walk rather than being an explicit artifact from the first walk. (why is it done this way? i want incremantal updates e.g. if you only change 1 out of 400 files, also in best case only resolve globals again for relevant files)

- Imports are not actually resolved yet. (this is okay we can pass an empty dummy list for low)
  - Unresolved values are described as possibly coming from imports or builtins, but `is_namespace_symbol` is still a stub that always returns `false`.
  - Builtins are currently recognized by a small string-matching fallback, not a real builtin table. (we should pass them similar to imports even if dummy table)

- Type definitions are checked during the second step, but resolved type identities are not carried forward as a resolved module representation. (we should store the resovle info somewhere)
  - The second pass diagnoses duplicate type names, unknown type names, and alias misuse for `@new`.
  - It does not produce a downstream named/resolved type-reference structure that later phases consume. (we must have this)

## Open decisions

- Should `pipeline.rs` grow a standalone `run_naming` entry point, with `run_lowering_and_naming` becoming a convenience wrapper or disappearing? (yes)
- Should naming produce a real `NamedModule` / resolved-module representation instead of raw `Module` plus side tables? (i don't mind as long as we have the info available. we need it for things liek goto defintion or auto-complete. basically an index of all globals)
- Should the file-local pass expose explicit per-file `exports` and `unresolved` sets, or is the current implicit representation acceptable if the package pass keeps reconstructing them? (i think we need this but okay if not)


requirement is that we store the resolved state so it can be consumed by goto defintion or autocompelte

### Proposed architecture

I think the proper architecture is:

- keep `lower` and `naming` as separate phases
- add a standalone `run_naming`
- keep HIR `Module` as the structural tree
- store naming as package-level side tables plus per-file local naming caches
- make later phases consume naming output, even if `typecheck` temporarily keeps reading raw HIR too

This gives us the important property:

- HIR stays the stable structural representation
- naming becomes the semantic index for tooling
- incremental updates can reuse file-local naming and rerun only the package-global consolidation

I do not think we need a separate tree-shaped `NamedModule` if the side tables are complete and stored in analysis state. A separate `NamedModule` would mostly duplicate HIR and make incremental invalidation harder.

### Recommended split

#### 1. file-local naming preparation

For each lowered file, produce a `LocalNamingResult` with explicit artifacts:

- local resolutions
  - expression use site -> local binding or unresolved value reference
- local bindings
  - all binding definitions introduced in the file
- exported top-level values
  - ordered list of top-level value declarations introduced by the file
- exported top-level types
  - ordered list of top-level `@type` / `@alias` declarations introduced by the file
- unresolved value references
  - only the use sites that escaped lexical resolution and need package/import/builtin lookup
- unresolved type references
  - type references from annotations and declarations that need package-global lookup

This should be enough to avoid walking the full HIR again in the package pass.

#### 2. package-global naming resolution

Build a `PackageNamingResult` from the set of `LocalNamingResult`s:

- global value index
  - symbol -> ordered package-visible declarations
- global type index
  - symbol -> declared type identity and kind
- final value resolutions
  - use site -> final binding identity
- final type resolutions
  - type reference site -> resolved type identity / alias-vs-nominal classification
- diagnostics
  - duplicate declarations, unknown globals, unknown types, alias misuse, shadowing builtins/imports

The package pass should operate over:

- file exports
- unresolved reference lists
- declaration metadata

It should not rescan every expression in every file.

### Incremental direction

For incremental updates, analysis state should store:

- lowered documents
- local naming results per file
- package naming result

On one-file change:

- re-lower that file
- recompute only that file's `LocalNamingResult`
- rebuild package-global indexes from all file export tables
- rerun global resolution only for affected unresolved references

At first, it is acceptable to rerun global resolution for all files after rebuilding the package indexes. That is still much better than recomputing local naming for all files. Later we can add dependency tracking from unresolved references to symbols.

### Pipeline shape

Recommended public orchestration:

- `run_lowering`
- `run_naming`
- `run_typecheck`
- `check`

`run_lowering_and_naming` can exist as a temporary convenience wrapper, but it should not be the architectural phase boundary.

### What naming should return

I think `run_naming` should return a package-scoped result, something like:

- remapped package modules or references to lowered modules
- local naming results by file
- final package naming tables
- diagnostics

The key point is not the exact Rust type name. The key point is that the resolved state is durable and queryable for:

- goto definition
- completion
- hover
- rename
- later typechecking

### Important unresolved point

The main thing still worth deciding is value-name order semantics.

If package-global value resolution is:

- order-insensitive
  - global resolution can just use the final package export table
- order-sensitive
  - the package pass should replay only top-level declaration/use events in order, not rescan full HIR

I think the architecture above supports either choice, but we should decide this explicitly because it changes the exact shape of `exported top-level values` and `unresolved value references`.

## Open decisions

- Should package-global value resolution be order-sensitive or order-insensitive? (use the last definition, but we want errors/warnings as described)
- Should `run_typecheck` immediately switch to consuming naming-owned resolved identities, or do we stage that after `run_naming` exists? (later)
