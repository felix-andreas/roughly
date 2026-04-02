# Naming Suite

This README is the authoritative contract for the naming fixture matrix.

If the naming fixture layout, coverage split, or intended semantics coverage changes, update this
file and `TESTING.md` in the same session as the fixture changes.

Keep this document aligned with the actual fixture suite. If the suite is still migrating toward
the contract below, record the remaining gaps here instead of implying that they are already
covered.

## Suite split

The naming fixtures live under:

- `tests/naming/local/`
- `tests/naming/global/`

`local` shows the file-local lexical view before package-global resolution.

`global` is the primary contract. It should mirror the core lexical cases from `local`, but run as
package naming, then add package-only behavior on top.

The expected difference in mirrored cases is:

- `local` leaves package-global references unresolved
- `global` resolves package-global references against the final package view

Mirrored local/global cases should preserve the same `group__case` names where possible.

Mirrored global cases should use `MultiFile`, even when the package has one file.

Type-name coverage belongs in `global`, not a dedicated local type suite, because type names are
project-global by semantics.

There is no `misc.R.test` today. If a new case does not fit an existing file, prefer refining the
split before adding a catch-all overflow file.

## Current status

The directory split and mirrored local/global lexical files are in place.

The suite is close to exhaustive against the contract below.

The only remaining gaps are:

- `Collate`-driven file-order fixtures once the harness can model `DESCRIPTION`
- nested inner type-parameter shadowing in `type_shadowing`, which is currently blocked because
  higher-rank type-parameter binders inside nested type syntax are not supported

The explicit gap list is in [Current gaps](#current-gaps).

## Matrix

The naming suite should explicitly cover:

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
  - non-package consumer file
- competing bindings
  - none
  - outer lexical binding
  - package-global binding
  - earlier top-level binding
  - later top-level binding
  - global type name shadowed by type parameter

`scope_reuse.R.test` is the explicit home for proving that blocks and loop bodies do not create a
fresh value scope. `shadowing.R.test` is for cases where several binders exist and we need to prove
which one wins.

`while` and `repeat` scope-reuse fixtures already exist today and belong under `scope_reuse`.

## Local files

### `top_level.R.test`

Purpose:

- top-level binder introduction
- top-level uses before and after rebinding
- simple file-local unresolved package-global lookup shape

Required coverage:

- one top-level binding followed by one use
- top-level use before the first binding in the same file
- two distinct top-level bindings followed by separate uses
- top-level call resolves top-level callee and top-level argument
- top-level rebinding creates a fresh binding

### `parameters.R.test`

Purpose:

- function-parameter binders
- parameter uses
- parameter interactions with package globals

Required coverage:

- parameter referenced directly in function body
- multiple parameters referenced in order
- later parameter use after unrelated local statements
- parameter shadows top-level binding of the same name
- unused parameter still introduces a binder in rendered output
- nested function closes over enclosing parameter
- local assignment later shadows parameter

### `locals.R.test`

Purpose:

- local assignment binders
- same-scope rebinding
- RHS lookup before the new binding is introduced

Required coverage:

- one local assignment followed by one use
- local assignment shadows top-level binding
- local assignment shadows parameter
- two local assignments with the same name create two bindings
- later local use resolves to the latest local binding
- assignment RHS sees the pre-existing binding, not the new one
- assignment RHS can refer to another outer binding while rebinding a different name

### `closures.R.test`

Purpose:

- nested functions
- capture of enclosing locals and parameters
- lookup across several lexical layers

Required coverage:

- nested function closes over outer local
- nested function closes over outer parameter
- nested function closes over top-level global
- inner function parameter shadows enclosing binding
- inner function local shadows enclosing binding
- multi-hop nested function closes over an outer local through an intermediate function
- multi-hop nested function uses the nearest shadowing binding rather than an earlier outer one

### `for_bindings.R.test`

Purpose:

- `for`-variable binder introduction
- loop-body lookup involving the loop variable

Required coverage:

- `for` variable introduces a fresh binding
- `for` variable shadows outer top-level binding
- loop body resolves another outer binding while the loop variable is in scope
- assignment inside loop body uses the loop binding on the RHS
- assignment inside loop body can use another outer binding on the RHS
- nested closure inside loop body captures the loop binding

### `scope_reuse.R.test`

Purpose:

- prove which constructs do not introduce a fresh value scope

Required coverage:

- braces do not create a fresh scope
- binding introduced inside braces remains visible after the braces
- nested braces still reuse the same scope chain
- braces inside a function do not restore an outer binding
- loop body does not introduce a fresh scope beyond the loop-variable binder
- braces inside a loop body do not restore the pre-loop binding
- assignment inside a loop body remains visible later in that same body
- `while` body reuses the enclosing scope
- `repeat` body reuses the enclosing scope

### `shadowing.R.test`

Purpose:

- nearest-wins resolution once several candidate binders with the same name exist

Required coverage:

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

## Global files

### `top_level.R.test`

Purpose:

- single-file package-global resolution of top-level names

Required coverage:

- the mirrored local `top_level` cases
- same-file top-level rebinding warning pair
- one-file package-global behavior under `MultiFile`
- earlier use and later use both resolve to the final package-global winner
- assignment RHS at top level sees the final package-global winner

### `parameters.R.test`

Purpose:

- mirrored parameter cases with package globals resolved

Required coverage:

- the mirrored local `parameters` cases
- non-shadowed globals resolve when parameter lookup does not win
- function parameter still beats the final package-global winner

### `locals.R.test`

Purpose:

- mirrored local-binding cases with package globals resolved

Required coverage:

- the mirrored local `locals` cases
- outer package globals resolve when not shadowed
- local assignment still beats the final package-global winner

### `closures.R.test`

Purpose:

- mirrored closure cases with package globals resolved

Required coverage:

- the mirrored local `closures` cases
- nested function closes over a global from another file
- nested function with local shadow still ignores the cross-file final winner

### `for_bindings.R.test`

Purpose:

- mirrored `for`-binder cases with package globals resolved

Required coverage:

- the mirrored local `for_bindings` cases
- outer globals resolve when not shadowed
- loop variable still beats the final package-global winner

### `scope_reuse.R.test`

Purpose:

- mirrored non-scope cases with package globals resolved

Required coverage:

- the mirrored local `scope_reuse` cases
- unresolved globals in local mode become resolved globals here when not shadowed

### `shadowing.R.test`

Purpose:

- lexical shadowing plus cross-file rebinding and cross-file shadow interactions

Required coverage:

- the mirrored local `shadowing` cases
- earlier-file global is shadowed by later-file global
- later-file global is still shadowed by a local binding in executable code
- later-file global is still shadowed by a function parameter
- cross-file consumer with no local shadow resolves to the final package-global winner
- cross-file consumer with local shadow ignores the final package-global winner

### `cross_file_values.R.test`

Purpose:

- package-global value lookup across files without heavy local-shadow interactions

Required coverage:

- later file uses earlier global
- earlier file uses later global
- earlier file can be consumed by several later files
- later file call resolves cross-file callee and argument
- later file rebinding wins for later consumers
- three-file chain where the last file sees the final winner
- three-file rebinding chain where first, middle, and last files all participate
- package file order matters for winner selection under default file ordering
- package file order matters for winner selection under `Collate` once fixture support exists

### `type_lookup.R.test`

Purpose:

- successful project-global type-name resolution

Required coverage:

- annotation uses nominal type from another file
- annotation uses alias from another file
- nominal introduction uses nominal type from another file
- definition references a previous definition in the same file
- definition forward reference resolves across files
- annotation forward reference resolves across files
- generic type parameter is in scope inside its definition
- annotation uses a same-file declared nominal
- annotation uses a same-file alias
- generic nominal introduction with fully applied type arguments succeeds

### `type_shadowing.R.test`

Purpose:

- type parameter shadowing of project-global type names

Required coverage:

- generic type parameter shadows a global nominal type from the same file
- generic type parameter shadows a global nominal type from another file
- generic type parameter shadows a global alias from the same file
- generic type parameter shadows a global alias from another file
- inner type parameter wins over an outer/global type name in nested type syntax
- duplicate type-parameter names in the same annotation block are rejected in the phase that owns
  that check

### `type_failures.R.test`

Purpose:

- project-global type-name failure coverage

Required coverage:

- unknown type in annotation
- unknown type nested in a generic argument
- unknown type in a definition
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

### `scripts.R.test`

Purpose:

- non-package documents should not contribute package-global names
- non-package documents can still consume package globals

Required coverage:

- script file consumes a package-global value
- script file consumes a package-global type name
- script file top-level binding does not become visible to package files
- script file top-level binding does not become visible to other script files through package
  naming
- script file type declaration does not become visible to package files
- script file type declaration does not become visible to other script files through package naming
- package file and script file with the same top-level name do not conflict in the package-global
  table
- package file and script file with the same type name do not conflict in the package-global type
  table

### `failures.R.test`

Purpose:

- broad package-level failure interactions that span several semantic areas

Required coverage:

- duplicate top-level value names across two files warn at both sites
- duplicate top-level value names across three files produce the full warning chain
- duplicate type names across several files mark every conflicting declaration
- one file introduces a later winner while another file still has a local shadowing case
- cross-file value lookup plus cross-file type failure in the same package
- package contains both package files and script files with diagnostics on both sides

## Current gaps

The current fixture suite is still missing or misplacing the following contract cases:

- `global/cross_file_values.R.test` still lacks `Collate`-driven file-order coverage because the
  fixture harness does not yet model `DESCRIPTION`.
- `global/type_shadowing.R.test` still lacks the nested inner-type-parameter shadowing case because
  higher-rank type-parameter binders inside nested type syntax are currently unsupported.
