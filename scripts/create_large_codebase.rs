use std::{fs, io::Write, path::Path};

const CONSTANT: &str = "<name> <- 12345";

const FUNCTION: &str = r#"
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
"#;

const S4_CLASS: &str = r#"
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
"#;

const S4_METHODS: &str = r#"
setGeneric("<name>", function(x) standardGeneric("<name>"))
setGeneric("<name><-", function(x, value) standardGeneric("<name><-"))

setMethod("<name>", "<class>", function(x) x@<name>)
setMethod("<name><-", "<class>", function(x, value) {
  x@name <- value
  x
})
"#;

const R6_CLASS: &str = r#"
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
"#;
fn main() {
    use std::{fs, path::Path};

    let base_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../R-large-codebase");
    let r_dir = base_dir.join("R");

    if r_dir.exists() {
        fs::remove_dir_all(&r_dir).unwrap();
    }

    fs::create_dir_all(&r_dir).unwrap();

    for file in (b'a'..=b'z').flat_map(|c| (0..4).map(move |n| format!("{}{}", c, n))) {
        let code = [
            // Constants
            ["ALPHA", "BETA", "GAMA"]
                .map(|name| CONSTANT.replace("<name>", &format!("{}.{}", file, name))),
            // Functions
            ["alpha", "beta", "gama"]
                .map(|name| FUNCTION.replace("<name>", &format!("{}.{}", file, name))),
            // S4 Classes and Methods
            ["Alpha", "Beta", "Gamma"].map(|name| {
                let class_code = S4_CLASS.replace("<name>", &format!("{}.{}", file, name));
                let methods_code = (0..3)
                    .map(|i| {
                        S4_METHODS
                            .replace("<name>", &format!("{}.{}.method{}", file, name, i))
                            .replace("<class>", &format!("{}.{}", file, name))
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("{}\n{}", class_code, methods_code)
            }),
            // R6 Classes
            ["AlphaR6", "BetaR6", "GammaR6"]
                .map(|name| R6_CLASS.replace("<name>", &format!("{}.{}", file, name))),
        ]
        .into_iter()
        .flat_map(|group| group)
        .collect::<Vec<_>>()
        .join("\n");

        let file_path = r_dir.join(format!("{}.R", file));
        fs::write(file_path, code).expect("Failed to write file");
    }
}
