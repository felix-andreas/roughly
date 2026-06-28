# Embedded base-package stub corpus (stdlib, R-version-agnostic subset).
#
# Declaration-only R: each binding carries a `#:` annotation that the loader harvests into a
# TypeScheme. The placeholder bodies are NEVER inferred — they exist only so the file is ordinary,
# parseable R that Roughly and plain R tooling can both read. See docs/.../stdlib-stubs.md.
#
# Scope discipline (§4): only signatures whose result is a static function of the argument *types*
# are modeled precisely. Shape-polymorphic and variadic functions, which the current type syntax
# cannot express, deliberately DEGRADE to `Any` rather than claim a false-precise shape. Each such
# degrade is marked `# degrade:` with the reason.

#: logical
T <- TRUE

#: logical
F <- FALSE

#: double
pi <- 0

# --- precise: result shape is independent of the argument shape ---

#: fn(x: Any) -> integer
length <- function(x) 0L

#: fn(x: Any) -> logical
is.null <- function(x) FALSE

#: fn(length_out: integer) -> integer[]
seq_len <- function(length_out) 0L

#: fn(along_with: Any) -> integer[]
seq_along <- function(along_with) 0L

# --- doc-sanctioned scalar approximation (§5 worked example): vectorized in real R, the result
#     atomic type is still static even though the result *shape* is not ---

#: fn(x: character) -> integer
nchar <- function(x) 0L

#: fn(x: character) -> character
toupper <- function(x) ""

#: fn(x: character) -> character
tolower <- function(x) ""

#: fn(x: Any) -> logical
is.na <- function(x) FALSE

#: fn(x: Any) -> integer
as.integer <- function(x) 0L

#: fn(x: Any) -> character
as.character <- function(x) ""

#: fn(x: Any) -> double
as.numeric <- function(x) 0

# --- precise: fixed arity, result atomic type static (substr requires all three positions) ---

#: fn(x: character, start: integer, stop: integer) -> character
substr <- function(x, start, stop) ""

# --- degrade: shape polymorphism (result shape == argument shape) is not expressible as `T -> T`
#     yet; declaring a concrete shape would be falsely precise, so the return degrades to `Any` ---

#: fn(x: Any) -> Any
rev <- function(x) x

#: fn(x: Any) -> Any
abs <- function(x) x

#: fn(x: Any) -> Any
sqrt <- function(x) x

#: fn(x: Any) -> Any
sort <- function(x) x

# --- degrade: variadics (`...`) are not expressible yet; the whole binding is `Any`, so a call
#     yields `Any` (never a spurious arity/type error and never a false-precise result) ---

#: Any
paste <- function(...) ""

#: Any
sum <- function(...) 0L

#: Any
min <- function(...) 0L

#: Any
max <- function(...) 0L

#: Any
mean <- function(x, ...) 0

# --- degrade: optional/extra arguments (e.g. `ignore.case`, `fixed`, `last`) are not yet expressible,
#     so a fixed-arity signature would spuriously arity-error real calls; the whole binding is `Any` ---

#: Any
substring <- function(text, first, last) ""

#: Any
gsub <- function(pattern, replacement, x) ""

#: Any
sub <- function(pattern, replacement, x) ""
