# Testing

This crate prefers fixture tests for source-driven behavior because they are:

- easy for a human to read in diffs
- easy to extend into many cases quickly
- a good fit for verifying AI-generated changes against an explicit text contract

Use ordinary Rust tests only when the behavior is awkward to express as a rendered fixture.

## Fixture format

The fixture harness lives in `tests/test_fixtures.rs`.

Each `.test` file uses:

```text
#==== group_name
#---- case_name
<input source>
#++++
<expected rendered output>
```

Notes:

- `group__case` is the stable test identity
- `group__case` names must be unique across the suite
- expected output should render semantic facts, not incidental debug structure

## Focused runs

Run one focused fixture case with:

```sh
TYPING_FILTER=group__case cargo test -p typing --test test_fixtures <suite> -- --nocapture
```

Current `<suite>` names:

- `annotations`
- `inference`
- `diagnostics`

## Current suites

### `annotations`

Purpose:

- annotation syntax parsing
- normalization
- rendered surface-type contract

Expected output should show:

- normalized type syntax
- precise parse-error kind for invalid cases

Example:

```text
#==== functions
#---- expanded_function_annotation
@param {integer} count
@param {character} [label]
@returns {character}
#++++
fn(integer, label: character | NULL) -> character
```

### `inference`

Purpose:

- normalized inferred types for expressions and small snippets
- focused inference behavior such as scoping, control flow, and polymorphism

Expected output should show:

- rendered inferred types
- or a normalized inference error kind if the suite intentionally targets failure shape

Example:

```text
#==== polymorphism
#---- let_polymorphism_across_uses
id <- function(x) x
list(id(1L), id("a"))
#++++
list{integer, character}
```

### `diagnostics`

Purpose:

- user-facing checking behavior
- rendered diagnostics, wording, and ranges

Expected output should show:

- final rendered diagnostic text as the user should see it

Example:

```text
#==== arithmetic
#---- non_numeric_plus_operand
1L + "a"
#++++
error[type-error] expected `numeric`, found `character`
--> 1:6-1:9
| 1L + "a"
```

## Planned suites

When adding a new phase, prefer adding a fixture suite for it. Each suite should render stable semantic facts rather than raw debug dumps.

### `lowering`

Purpose:

- syntax-to-IR lowering

Expected output should show:

- expression kind tree
- attached annotation presence
- normalized child structure

Example shape:

```text
#==== control_flow
#---- annotated_if_expression
#: integer | NULL
if (flag) 1L
#++++
if
  condition: symbol(flag)
  consequence: integer(1L)
  alternative: <none>
  annotation: integer | NULL
```

### `naming`

Purpose:

- binding introduction
- shadowing
- use-site resolution

Expected output should show:

- `bind name -> bN`
- `use name -> bN`

Binding ids should be normalized per fixture in source order.

Example shape:

```text
#==== closures
#---- shadow_and_capture
x <- 1L
f <- function(x) {
  g <- function() x
  g()
}
f(2L)
x
#++++
bind x -> b1
bind f -> b2
bind x -> b3
bind g -> b4
use x -> b3
use g -> b4
use f -> b2
use x -> b1
```

### `interfaces`

Purpose:

- per-file exported interface rendering
- generalized exported types
- exported type definitions

Expected output should show:

- exported bindings and normalized rendered type schemes
- exported aliases or nominal declarations

Prefer normalized interface contents over opaque hashes.

Example shape:

```text
#==== polymorphic_exports
#---- identity_and_constant
id <- function(x) x
const <- function(x, y) x
#++++
export id: fn(type1) -> type1
export const: fn(type1, type2) -> type1
```

### `project`

Purpose:

- multi-file dependency tracking
- invalidation behavior
- cross-file diagnostics

Expected output should show:

- normalized dependency facts
- invalidated files or definitions
- final diagnostics

Example shape:

```text
#==== invalidation
#---- interface_change_propagates
# file: A.R
render_count <- function(x) paste0(x)
# file: B.R
render_count(1L) + 1L
#++++
invalidate A, B
error[type-error] expected `numeric`, found `character`
```

### `hover`

Purpose:

- hover target selection
- rendered hover types

Expected output should show:

- hovered source fragment or marker
- rendered type

Example shape:

```text
#==== nested_expression
#---- call_result_and_subexpression
id <- function(x) x
id(1L + 2L)
#++++
hover 1L + 2L @ 2:4-2:11 -> integer
hover id(1L + 2L) @ 2:1-2:12 -> integer
```

Keep this document aligned with `tests/test_fixtures.rs` and the actual fixture directories.
