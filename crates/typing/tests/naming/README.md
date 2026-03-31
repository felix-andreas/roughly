# Naming Suite

This suite tests the `naming` phase.

The naming phase is responsible for:

- value binding introduction
- lexical scope construction
- shadowing
- use-site resolution
- naming diagnostics for type references that fail during naming

This suite should stay focused on naming facts. It should not be used to test lowering-only structure or final typechecking behavior unless the naming output is the thing under inspection.

## Output contract

Naming fixtures render binding identities directly in the snapshot, for example `x@b0` and `x@b1`.

That output is the main assertion mechanism for value naming:

- definition sites should show the binding id introduced there
- use sites should show the binding id they resolve to
- shadowing is visible when the same spelled name resolves to different ids in different scopes

When a case intentionally targets a naming failure, the fixture should instead render the naming diagnostic.

## Strategy

Each fixture case should ideally demonstrate one resolution rule.

Prefer many tiny cases over a few large cases that mix several scope transitions.

## File Split

### `scoping.R.test`

Use this file for top-level and block-local naming behavior:

- top-level binding referenced later at top level
- top-level rebinding introducing a fresh binding identity
- top-level unresolved uses resolving against the final winning global binding
- top-level assignment RHS resolving against the final winning global binding
- nested block using the nearest binding
- nested block shadowing an outer binding
- behavior after leaving an inner block

### `functions.R.test`

Use this file for function-local naming behavior:

- top-level binding referenced inside a function
- function parameter referenced in the function body
- function parameter shadowing an outer binding
- local assignment shadowing an outer binding inside a function body
- local assignment shadowing a parameter
- nested function closing over an outer local binding
- nested function closing over an outer parameter binding
- nested function parameter shadowing an enclosing binding
- nested function local binding shadowing an enclosing binding
- multi-hop nested function resolution through more than one enclosing scope
- multi-hop nested function resolution picking the nearest shadowing binding

### `globals.R.test`

Use this file for multi-file package-global naming behavior:

- a later file referencing a top-level value from an earlier file
- a later file resolving both a cross-file callee and a cross-file argument
- a later file rebinding a top-level value and all global uses observing the final winning binding
- a function body closing over a package-global value defined in another file
- an annotation using a nominal type declared in another file
- an annotation using an alias declared in another file
- a type definition using a forward reference to a type declared in a later file
- a generic type parameter shadowing a global type declared in another file
- `@new` using a nominal type declared in another file

### `loops.R.test`

Use this file for loop-specific naming behavior:

- `for` loop variable introducing a fresh binding
- `for` loop variable shadowing an outer binding only inside the loop body
- loop body references to other visible outer bindings
- assignment inside a `for` loop body where the RHS resolves against the loop binding
- assignment inside a `for` loop body where the RHS resolves against another visible outer binding

### `types.R.test`

Use this file for naming-owned type reference behavior:

- a definition referencing an earlier definition
- duplicate nominal type names
- duplicate alias names
- alias and nominal declarations colliding in the shared global type namespace
- an annotation referencing a declared nominal type
- an annotation referencing a declared alias
- generic type parameter scope inside a definition
- generic type parameter shadowing a global type name
- unknown type names in definitions
- unknown type names in annotations
- unknown type names nested inside generic arguments
- `@new` accepting a nominal type
- `@new` rejecting an alias

Positive type-name cases are currently less observable than value-name cases because the renderer does not yet print resolved type identities. Until that changes, error cases still belong here and positive cases can still serve as regression coverage when paired with surrounding naming output.

## Boundaries

Keep these distinctions clear:

- lowering suite:
  - parsing and attachment
  - HIR shape
  - lowering-owned diagnostics such as misplaced top-level type definitions

- naming suite:
  - scoping
  - shadowing
  - value use-site resolution
  - type-reference diagnostics that arise during naming

- diagnostics suite:
  - final rendered user-facing diagnostics across the whole pipeline

## Case naming

Use case names that describe the rule being exercised, for example:

- `parameter_shadows_outer`
- `local_shadows_parameter`
- `rhs_sees_outer_binding`
- `inner_function_closes_over_outer_local`
- `annotation_unknown_type`

That naming style makes gaps in the matrix easier to spot.
