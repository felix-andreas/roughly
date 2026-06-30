# Standard-library stubs: base package.
#
# Declaration-only R. Each top-level binding carries a `#:` type annotation that the analyzer harvests
# into the binding's exported type; the placeholder body (`0L`, `""`, `NULL`, ...) is never type-checked
# and exists only so the file is ordinary, parseable R that both this tool and plain R tooling can read.
#
# A `# widened to Any:` note marks a binding whose precise type the annotation grammar cannot yet
# express (variadic `...`, optional arguments, or a result shape that mirrors the argument). Such a
# binding is deliberately given `Any` rather than a falsely-precise signature: a call then yields `Any`,
# which never produces a spurious type or arity error. The name still resolves, so it is never reported
# as undefined and never flagged by strict mode.
#
# The arithmetic and comparison operators and the `c` / `list` constructors are NOT stubbed here: their
# result type is an algorithm over the argument types (numeric promotion, `NULL`-dropping combination,
# record synthesis) that a single annotation cannot describe, so they are modeled directly in the
# checker instead.

# Reserved value constants.

#: logical
T <- TRUE

#: logical
F <- FALSE

#: double
pi <- 0

# Length and sequence helpers: the result type is fixed by the function, independent of the argument's
# element type or shape.

#: fn(x: Any) -> integer
length <- function(x) 0L

#: fn(length_out: integer) -> integer[]
seq_len <- function(length_out) 0L

#: fn(along_with: Any) -> integer[]
seq_along <- function(along_with) 0L

# Type predicates: always return a single logical.

#: fn(x: Any) -> logical
is.null <- function(x) FALSE

#: fn(x: Any) -> logical
is.na <- function(x) FALSE

#: fn(x: Any) -> logical
is.numeric <- function(x) FALSE

#: fn(x: Any) -> logical
is.character <- function(x) FALSE

#: fn(x: Any) -> logical
is.logical <- function(x) FALSE

#: fn(x: Any) -> logical
is.function <- function(x) FALSE

#: fn(x: Any) -> logical
is.list <- function(x) FALSE

# Coercions: the target atomic type is fixed by the function, not by the argument.

#: fn(x: Any) -> integer
as.integer <- function(x) 0L

#: fn(x: Any) -> double
as.numeric <- function(x) 0

#: fn(x: Any) -> character
as.character <- function(x) ""

#: fn(x: Any) -> logical
as.logical <- function(x) FALSE

# Character scalars: the result atomic type is character (real R vectorizes over the input, but the
# element type is still fixed).

#: fn(x: character) -> integer
nchar <- function(x) 0L

#: fn(x: character) -> character
toupper <- function(x) ""

#: fn(x: character) -> character
tolower <- function(x) ""

#: fn(x: character, start: integer, stop: integer) -> character
substr <- function(x, start, stop) ""

#: fn(x: character, prefix: character) -> logical
startsWith <- function(x, prefix) FALSE

#: fn(x: character, suffix: character) -> logical
endsWith <- function(x, suffix) FALSE

# widened to Any: the result shape mirrors the argument shape, which the grammar cannot express as a
# type variable, so any concrete return type would be falsely precise.

#: fn(x: Any) -> Any
rev <- function(x) x

#: fn(x: Any) -> Any
abs <- function(x) x

#: fn(x: Any) -> Any
sqrt <- function(x) x

#: fn(x: Any) -> Any
sort <- function(x) x

#: fn(x: Any) -> Any
unique <- function(x) x

# widened to Any: variadic (`...`); the argument list cannot be expressed, so the whole binding is
# `Any` and a call yields `Any`.

#: Any
paste <- function(...) ""

#: Any
paste0 <- function(...) ""

#: Any
sum <- function(...) 0L

#: Any
prod <- function(...) 0L

#: Any
min <- function(...) 0L

#: Any
max <- function(...) 0L

#: Any
range <- function(...) 0L

# widened to Any: optional arguments (`na.rm`, `fixed`, `ignore.case`, `which`, ...) cannot yet be
# expressed, so a fixed-arity signature would wrongly reject valid calls.

#: Any
mean <- function(x, ...) 0

#: Any
trimws <- function(x, ...) ""

#: Any
substring <- function(text, first, last) ""

#: Any
gsub <- function(pattern, replacement, x, ...) ""

#: Any
sub <- function(pattern, replacement, x, ...) ""

#: Any
grepl <- function(pattern, x, ...) FALSE

#: Any
grep <- function(pattern, x, ...) 0L

#: Any
strsplit <- function(x, split, ...) NULL

#: Any
round <- function(x, ...) 0

#: Any
seq <- function(...) 0L

#: Any
rep <- function(x, ...) x

#: Any
vapply <- function(X, FUN, FUN.VALUE, ...) NULL

#: Any
lapply <- function(X, FUN, ...) NULL

#: Any
sapply <- function(X, FUN, ...) NULL
