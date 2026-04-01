# Naming Suite

This README is the authoritative contract for the naming fixture matrix.

If the naming fixture layout, coverage split, or intended semantics coverage changes, update this
file and `TESTING.md` in the same session as the fixture changes.

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

Type-name coverage belongs in `global`, not a dedicated local type suite, because type names are
project-global by semantics.

## Matrix

The naming suite should cover:

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
  - `while` body
  - `repeat` body
  - other file
  - non-package consumer file
- competing bindings
  - none
  - outer lexical binding
  - package-global binding
  - earlier top-level binding
  - later top-level binding
  - global type name shadowed by type parameter

## Local files

- `top_level.R.test`
  - top-level binders and unresolved package-global uses in one file
- `parameters.R.test`
  - parameter binders, parameter uses, and parameter survival across later local statements
- `locals.R.test`
  - local assignments, rebinding, and assignment-RHS lookup before rebinding
- `closures.R.test`
  - nested functions and capture of enclosing locals and parameters
- `for_bindings.R.test`
  - `for`-variable binders and loop-body lookup
- `scope_reuse.R.test`
  - constructs that do not create a fresh value scope, especially braces, loop bodies, `while`,
    and `repeat`
- `shadowing.R.test`
  - nearest-wins behavior once several candidate binders exist

## Global files

- `top_level.R.test`
  - one-file package-global resolution of top-level names
- `parameters.R.test`
  - mirrored parameter cases with package globals resolved
- `locals.R.test`
  - mirrored local-binding cases with package globals resolved
- `closures.R.test`
  - mirrored closure cases with package globals resolved
- `for_bindings.R.test`
  - mirrored `for`-binder cases with package globals resolved
- `scope_reuse.R.test`
  - mirrored non-scope cases with package globals resolved
- `shadowing.R.test`
  - lexical shadowing plus cross-file rebinding and cross-file shadow interactions
- `cross_file_values.R.test`
  - package-global value lookup across files without heavy local-shadow complications, including
    rebinding chains
- `type_lookup.R.test`
  - successful project-global type-name resolution, including cross-file lookup and forward
    references
- `type_shadowing.R.test`
  - type parameters shadowing project-global type names
- `type_failures.R.test`
  - unknown-type and duplicate-name failures for the project-global type namespace
- `scripts.R.test`
  - non-package files do not contribute globals but can consume package globals
- `failures.R.test`
  - broad package-level failure interactions that do not belong naturally in one other file

## Naming-specific expectations

- mirrored local/global cases should preserve the same group/case names where possible
- mirrored global cases should use `MultiFile`, even when the package has one file
- `scope_reuse` proves whether a construct creates a new scope boundary at all
- `shadowing` proves which binding wins once several candidate binders exist
- `scripts` should verify both value-name and type-name non-contribution from non-package files
- type failure coverage is first-class, not optional overflow
- there is no `misc.R.test` today; if a new case does not fit an existing file, prefer renaming or
  refining the split before adding `misc`
