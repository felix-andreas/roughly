# Standard-library stubs: methods package (S4).
#
# Declaration-only R; see base.R for the format. The S4 machinery is dynamic — class membership and
# coercion results depend on runtime class definitions and dispatch — so almost everything is widened to
# `Any`. Stubbing the names still resolves them and keeps strict mode from flagging them. (S4 class,
# generic, and method *names* are tracked separately as string literals, not through these bindings.)

# widened to Any: `is(object)` returns the class vector while `is(object, class2)` returns a logical, an
# arity-dependent result the grammar cannot express.

#: Any
is <- function(object, class2) FALSE

#: Any
as <- function(object, Class) NULL

#: Any
new <- function(Class, ...) NULL

#: Any
validObject <- function(object, ...) object

#: Any
setClass <- function(Class, ...) NULL

#: Any
setGeneric <- function(name, ...) NULL

#: Any
setMethod <- function(f, signature, definition, ...) NULL

#: Any
setValidity <- function(Class, method) NULL

#: Any
setRefClass <- function(Class, ...) NULL

# Single-logical class predicates with a fixed signature.

#: fn(Class: character) -> logical
isVirtualClass <- function(Class) FALSE

#: fn(class1: character, class2: character) -> logical
extends <- function(class1, class2) FALSE
