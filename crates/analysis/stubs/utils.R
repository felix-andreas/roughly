# Standard-library stubs: utils package.
#
# Declaration-only R; see base.R for the format. These helpers either take optional arguments or return
# a value whose shape mirrors their input, so most are widened to `Any`; stubbing them still makes the
# names resolve and keeps strict mode from flagging them.

# widened to Any: the result is a prefix/suffix of the argument, so its shape mirrors the input.

#: Any
head <- function(x, ...) x

#: Any
tail <- function(x, ...) x

# widened to Any: structure-dependent or side-effecting, with optional arguments.

#: Any
str <- function(object, ...) NULL

#: Any
read.csv <- function(file, ...) NULL

#: Any
write.csv <- function(x, ...) NULL

#: Any
install.packages <- function(pkgs, ...) NULL

#: Any
modifyList <- function(x, val, ...) x

#: fn(object: Any) -> NULL
help <- function(object) NULL
