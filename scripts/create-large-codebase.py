import string
import shutil
import sys
from pathlib import Path
from itertools import product

constant = r"""<name> <- 12345"""

function = r"""
<name> <- function(n) {
  if (n <= 0) {
    return(NULL)
  } else if (n == 1) {
    return(0)
  } else if (n == 2) {
    return(1)
  } else {
    return(<name>(n - 1) + <name>(n - 2))
  }
}
"""

s4_class = r"""
setClass("<name>",
  slots = c(
    name = "character",
    age = "numeric"
  ),
  prototype = list(
    name = NA_character_,
    age = NA_real_
  )
)
"""

s4_methods = r"""
setGeneric("<name>", function(x) standardGeneric("<name>"))
setGeneric("<name><-", function(x, value) standardGeneric("<name><-"))

setMethod("<name>", "<class>", function(x) x@<name>)
setMethod("<name><-", "<class>", function(x, value) {
  x@name <- value
  x
})
"""

r6_class = r"""
<name> <- R6Class("<name>",
  public = list(
    initialize = function(...) {
      for (item in list(...)) {
        self$add(item)
      }
    },
    add = function(x) {
      private$queue <- c(private$queue, list(x))
      invisible(self)
    },
    remove = function() {
      if (private$length() == 0) return(NULL)
      # Can use private$queue for explicit access
      head <- private$queue[[1]]
      private$queue <- private$queue[-1]
      head
    }
  ),
  private = list(
    queue = list(),
    length = function() base::length(private$queue)
  )
)
"""


base_dir = Path(__file__).parent.parent / "R-large-codebase"
r_dir = base_dir / "R"

if r_dir.exists():
    shutil.rmtree(r_dir)

r_dir.mkdir(exist_ok=True, parents=True)

# Files per letter prefix; the default 4 yields ~25K LoC, pass a larger count to scale up
# (48 yields ~300K LoC, the performance target scale).
files_per_prefix = int(sys.argv[1]) if len(sys.argv) > 1 else 4

for file in map("".join, product(string.ascii_lowercase, map(str, range(files_per_prefix)))):
    code = "\n".join(
        [
            *(
                constant.replace("<name>", f"{file}.{name}")
                for name in ["ALPHA", "BETA", "GAMA"]
            ),
            *(
                function.replace("<name>", f"{file}.{name}")
                for name in ["alpha", "beta", "gama"]
            ),
            *(
                s4_class.replace("<name>", f"{file}.{name}")
                + "\n".join(
                    s4_methods.replace("<name>", f"{file}.{name}.method{i}").replace(
                        "<class>", f"{file}.{name}"
                    )
                    for i in range(3)
                )
                for name in ["Alpha", "Beta", "Gamma"]
            ),
            *(
                r6_class.replace("<name>", f"{file}.{name}")
                for name in ["AlphaR6", "BetaR6", "GammaR6"]
            ),
        ]
    )
    (r_dir / f"{file}.R").write_text(code)
