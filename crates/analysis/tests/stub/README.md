# Stub declaration-parser suite

Exercises `analysis::stub::parse_stub_declarations`, the declaration-line layer for `.Rtypes` stub files.
The parser splits each non-blank, non-comment line into a name and a type expression and reuses the `#:`
annotation type parser (`parse_surface_type`) for the type half, so this suite covers the declaration
layer, not the type grammar itself (that is `tests/type_syntax`).

## Rendered output

Each `Simple` case is one stub source. The runner renders, in source-line order:

- a parsed declaration as `name : <type>`, where the type is re-rendered from the parsed
  `SurfaceType` (so a round-trip mismatch shows up);
- a malformed line as `error[line N]: message` (zero-based line).

## Coverage matrix

| Group                | What it pins |
|----------------------|--------------|
| `plain`              | function declarations, multi-parameter signatures, dotted names (`is.null`) |
| `values`             | atomic value/constant declarations and `Any` values |
| `generics`           | `<T> fn(...)` binders survive the reuse of the type parser (real generic schemes, not `Any`) |
| `comments_and_blanks`| trailing `#` comments and blank/comment lines are ignored |
| `repeated`           | repeating a name parses both declarations (grammar admits overload sets) |
| `malformed`          | missing `:`, empty name, missing type, an invalid type expression, and a valid/malformed mix — each malformed line is reported without dropping the valid ones |

## Not in scope

The type-expression grammar (parsed by `parse_surface_type`) is covered by `tests/type_syntax`; do not
duplicate type-shape cases here. How harvested declarations become schemes and resolve as base globals is
covered by `tests/typecheck/stdlib` and the engine differential suite.
