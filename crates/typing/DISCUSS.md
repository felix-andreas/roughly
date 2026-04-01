# Typing Crate Discussion

This file is scratch space for active design discussion.

Use it for short summaries or temporary notes during a design conversation.

## Naming suite restructure

### Agreed

- restructure under:
  - `tests/naming/README.md`
  - `tests/naming/local/`
  - `tests/naming/global/`
- `README.md` should contain the testing matrix and explain local vs global resolution
- `global` is the primary contract and should replicate `local` plus package-only cases
- `local` should stay focused on lexical value naming; project-global type resolution belongs in
  `global`
- mirrored `global` cases should preserve the same group/case names as `local`
- mirrored `global` cases should use `MultiFile`, even for a single package file

### Proposed layout

Local:

- `top_level.R.test`
  - package-global names viewed before package-global resolution
- `parameters.R.test`
  - function-parameter binders and uses
- `locals.R.test`
  - local assignment binders and same-scope rebinding behavior
- `closures.R.test`
  - nested functions and closure lookup across lexical scopes
- `for_bindings.R.test`
  - `for`-variable binder semantics and loop-body lookup
- `scope_reuse.R.test`
  - constructs that do not create a fresh value scope, especially braces and loop bodies
- `shadowing.R.test`
  - nearest-wins behavior once several candidate binders exist
- `misc.R.test`
  - leftover local naming cases that do not fit the intended split cleanly
  - includes any rare pre-global type-name cases that genuinely belong to local naming

Global:

- `top_level.R.test`
  - single-file package-global resolution of top-level names
- `parameters.R.test`
  - same parameter cases as local, but with globals resolved
- `locals.R.test`
  - same local-binding cases as local, but with globals resolved
- `closures.R.test`
  - same closure cases as local, but with globals resolved
- `for_bindings.R.test`
  - same `for`-binder cases as local, but with globals resolved
- `scope_reuse.R.test`
  - same non-scope cases as local, but with globals resolved
- `shadowing.R.test`
  - lexical shadowing plus cross-file rebinding and cross-file shadow interactions
- `cross_file_values.R.test`
  - package-global value lookup across files without local shadow complications
- `type_lookup.R.test`
  - successful project-global type-name resolution, including cross-file lookup and forward references
- `type_shadowing.R.test`
  - type parameters shadowing project-global type names
- `type_failures.R.test`
  - unknown-type and duplicate-name failure coverage for the project-global type namespace
- `scripts.R.test`
  - script files do not contribute global names but can consume package-global names
- `failures.R.test`
  - broad package-level failure interactions that do not belong naturally in one other file
- `misc.R.test`
  - leftover global naming cases that do not fit the intended split cleanly

### Comprehensive suite plan

The intent is that `global` mirrors the core lexical sections from `local`, with the same
group/case names where possible, then adds package-only sections.

There should be no dedicated local `types.R.test`. Type names are project-global by semantics, so
the naming contract for types belongs in `global`. Local should only carry a type-related case when
it proves a genuinely pre-global failure shape and does not depend on package-wide type lookup.

#### `top_level.R.test`

Purpose:

- top-level binder introduction
- top-level uses before and after rebinding
- simple package-global lookup shape

Core cases:

- one top-level binding followed by one use
- top-level use before the first binding in the same file
- two distinct top-level bindings followed by separate uses
- top-level call resolves top-level callee and top-level argument
- top-level rebinding creates a fresh binding
- earlier use and later use both resolve to the final package-global winner in global mode
- assignment RHS at top level sees the final package-global winner in global mode
- top-level use in local mode stays unresolved as package-global

Global-only extras:

- same-file top-level rebinding warning pair
- one-file package-global behavior under `MultiFile`

#### `parameters.R.test`

Purpose:

- function-parameter binders
- parameter uses
- parameter interactions with package globals

Core cases:

- parameter referenced directly in function body
- multiple parameters referenced in order
- later parameter use after unrelated local statements
- parameter shadows top-level binding of the same name
- unused parameter still introduces a binder in rendered output
- nested function closes over enclosing parameter
- local assignment later shadows parameter

Global-only extras:

- same cases as local, but with non-shadowed globals resolved
- function parameter still beats final package-global winner

#### `locals.R.test`

Purpose:

- local assignment binders
- same-scope rebinding
- RHS lookup before the new binding is introduced

Core cases:

- one local assignment followed by one use
- local assignment shadows top-level binding
- local assignment shadows parameter
- two local assignments with the same name create two bindings
- later local use resolves to the latest local binding
- assignment RHS sees the pre-existing binding, not the new one
- assignment RHS can refer to another outer binding while rebinding a different name

Global-only extras:

- same cases as local, but outer package globals resolve when not shadowed
- local assignment still beats final package-global winner

#### `closures.R.test`

Purpose:

- nested functions
- capture of enclosing locals and parameters
- lookup across several lexical layers

Core cases:

- nested function closes over outer local
- nested function closes over outer parameter
- nested function closes over top-level global
- inner function parameter shadows enclosing binding
- inner function local shadows enclosing binding
- multi-hop nested function closes over an outer local through an intermediate function
- multi-hop nested function uses nearest shadowing binding rather than an earlier outer one

Global-only extras:

- nested function closes over global from another file
- nested function with local shadow still ignores cross-file final winner

#### `for_bindings.R.test`

Purpose:

- `for`-variable binder introduction
- loop-body lookup involving the loop variable

Core cases:

- `for` variable introduces a fresh binding
- `for` variable shadows outer top-level binding
- loop body resolves another outer binding while the loop variable is in scope
- assignment inside loop body uses loop binding on the RHS
- assignment inside loop body can use another outer binding on the RHS
- nested closure inside loop body captures loop binding

Global-only extras:

- same cases as local, but outer globals resolve when not shadowed
- loop variable still beats final package-global winner

#### `scope_reuse.R.test`

Purpose:

- prove which constructs do not introduce a fresh value scope

Core cases:

- braces do not create a fresh scope
- binding introduced inside braces remains visible after the braces
- nested braces still reuse the same scope chain
- braces inside a function do not restore an outer binding
- loop body does not introduce a fresh scope beyond the loop-variable binder
- braces inside a loop body do not restore the pre-loop binding
- assignment inside a loop body remains visible later in that same body

Global-only extras:

- same cases as local, but unresolved globals become resolved when not shadowed

#### `shadowing.R.test`

Purpose:

- nearest-wins resolution once several candidate binders with the same name exist

Core cases:

- parameter shadows global
- local shadows parameter
- local shadows global
- inner nested-function parameter shadows enclosing local
- inner nested-function local shadows enclosing parameter
- inner nested-function local shadows enclosing local
- `for` variable shadows global
- repeated shadow chain across global -> parameter -> local
- repeated shadow chain across parameter -> local -> nested local
- use after inner rebinding picks the nearest visible binding

Global-only extras:

- earlier-file global is shadowed by later-file global
- later-file global is still shadowed by a local binding in executable code
- later-file global is still shadowed by a function parameter
- cross-file consumer with no local shadow resolves to the final package-global winner
- cross-file consumer with local shadow ignores the final package-global winner

#### `cross_file_values.R.test`

Purpose:

- package-global value lookup across files without heavy local-shadow interactions

Cases:

- later file uses earlier global
- earlier file uses later global
- earlier file can be consumed by several later files
- later file call resolves cross-file callee and argument
- later file rebinding wins for later consumers
- three-file chain where the last file sees the final winner
- three-file rebinding chain where first, middle, and last files all participate
- package file order matters for winner selection under default file ordering
- package file order matters for winner selection under `Collate` once fixture support exists

#### `type_lookup.R.test`

Purpose:

- successful project-global type-name resolution

Cases:

- annotation uses nominal type from another file
- annotation uses alias from another file
- nominal introduction uses nominal type from another file
- definition references previous definition in same file
- definition forward reference resolves across files
- annotation forward reference resolves across files
- generic type parameter is in scope inside its definition
- annotation uses same-file declared nominal
- annotation uses same-file alias
- generic nominal introduction with fully applied type arguments succeeds

#### `type_shadowing.R.test`

Purpose:

- type parameter shadowing of project-global type names

Cases:

- generic type parameter shadows global nominal type from same file
- generic type parameter shadows global nominal type from another file
- generic type parameter shadows global alias from same file
- generic type parameter shadows global alias from another file
- inner type parameter wins over outer/global type name in nested type syntax
- duplicate type parameter names in the same annotation block are rejected in the phase that owns that check

#### `type_failures.R.test`

Purpose:

- project-global type-name failure coverage

Cases:

- unknown type in annotation
- unknown type nested in generic argument
- unknown type in definition
- duplicate nominal name in one file
- duplicate nominal name across files
- duplicate alias name in one file
- duplicate alias name across files
- nominal/alias collision in one file
- nominal/alias collision across files
- every conflicting declaration gets diagnosed, not only the later one
- wrong type-argument arity on a global nominal or alias reference
- `@new` rejects aliases
- `@new` rejects unknown names
- `@new` rejects non-nominal type forms
- `@new` rejects under-applied generic nominals
- `@new` rejects over-applied generic nominals if that is diagnosed in naming/type resolution

#### `scripts.R.test`

Purpose:

- non-package documents should not contribute package-global names
- non-package documents can still consume package globals

Cases:

- script file consumes a package-global value
- script file consumes a package-global type name
- script file top-level binding does not become visible to package files
- script file top-level binding does not become visible to other script files through package naming
- script file type declaration does not become visible to package files
- script file type declaration does not become visible to other script files through package naming
- package file and script file with same top-level name do not conflict in the package-global table
- package file and script file with same type name do not conflict in the package-global type table

#### `failures.R.test`

Purpose:

- package-level failure interactions that span several semantic areas

Cases:

- duplicate top-level value names across two files warn at both sites
- duplicate top-level value names across three files produce the full warning chain
- duplicate type names across several files mark every conflicting declaration
- one file introduces a later winner while another file still has a local shadowing case
- cross-file value lookup plus cross-file type failure in the same package
- package contains both package files and script files with diagnostics on both sides

#### `misc.R.test`

Purpose:

- temporary overflow bucket for cases that do not fit the intended split yet

Rules:

- prefer moving cases out of `misc` into a real section once a pattern appears
- do not let `misc` become the default dumping ground

Possible cases:

- awkward mixed value/type cases that are not yet numerous enough for their own section
- temporary regression cases discovered during implementation work

### Coverage notes

The matrix should explicitly cover:

- binder kinds
  - top-level binding
  - function parameter
  - local assignment
  - `for` variable
  - type declaration
  - type parameter
- lookup sites
  - same scope
  - inner scope
  - sibling statement after rebinding
  - nested function
  - other file
- competing bindings
  - none
  - outer lexical binding
  - package-global binding
  - earlier top-level binding
  - later top-level binding
  - global type name shadowed by type parameter

Global needs extra emphasis on:

- much stronger failure coverage
- cross-file rebinding
- cross-file shadowing
- type-name failures
- script-file behavior

Still-missing-if-we-want-to-call-this exhaustive:

- explicit `while` and `repeat` scope-reuse cases if those constructs are lowered in naming-relevant form
- explicit coverage for whatever phase owns duplicate type-parameter-name diagnostics
- explicit `Collate`-driven file-order fixtures once the fixture harness can model `DESCRIPTION`

`scope_reuse.R.test` is the explicit home for proving that blocks and loop bodies do not create a
fresh value scope. `shadowing.R.test` is for cases where several binders exist and we need to prove
which one wins.

### Open decisions

1. Should global failure cases live in one `failures.R.test`, or stay next to the behavior they
   fail?
   My current lean: keep behavior-specific failures next to the behavior, and reserve
   `failures.R.test` for broad package-diagnostic interactions that cut across several behaviors. (yes)

2. Do you want script-file cases isolated in `scripts.R.test`, or folded into
   `cross_file_values.R.test`? (i don't mnd)
