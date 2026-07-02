---
title: Formatter
description: Documentation for Roughly's R code formatter.
---
<!-- THIS FILE IS GENERATED AUTOMATICALLY.MAKE CHANGES TO tests/format/formatter.template.md INSTEAD -->

Roughly includes a non-invasive R code formatter that emphasizes readability while respecting the existing structure of your code.

## Usage

Format your R files using the command line:

```sh
roughly fmt           # Format all files in the current directory
roughly fmt <path>    # Format all files in <path>
roughly fmt --check   # List files that would be reformatted, without writing
roughly fmt --diff    # Show a diff of formatting changes without applying them
```

`--check` and `--diff` exit 1 when any file would change, so they slot straight into CI. Errors
(for example a file that cannot be parsed) exit 2 — see the full
[exit-code table](/getting-started#exit-codes).

## Philosophy

Roughly's non-invasive approach means it respects your existing line breaks and won’t arbitrarily split expressions you’ve chosen to keep on one line. The formatter is guided by these principles:

* **Single-line expressions remain single-line:** The formatter only adds line breaks if the expression is already multi-line, and will never break single-line expressions into multiple lines (with [one exception](#loops)).
* **Flexible style preservation:** Both compact ("hugged") and expanded forms for nested expressions are supported, so your preferred style is respected (see [hugging behavior](#hugging-behavior)).
* **Automatic braces only when needed:** Braces are added automatically only where they prevent subtle bugs (see [auto-bracing](#auto-bracing)).
* **Minimal configuration:** The formatter works out of the box with sensible defaults, so you can use it immediately without extra setup (see [configuration](/configuration)).

## Formatting Rules

Below is a comprehensive list of rules describing the behavior of the formatter for different kinds of expressions, including edge cases where special handling is applied.

### Binary Operators

**Assignment operators** always have spaces around them:

```r
# Before formatting
x<-1
data<<-compute()

# After formatting
x <- 1
data <<- compute()
```

**Binary operators** have spaces around them, except for the range (`:`) and power (`^`) operators:

```r
# Before formatting
result=x+y*z
power=base^exponent
sequence=1:10

# After formatting
result = x + y * z
power = base^exponent
sequence = 1:10
```

**Pipeline operators** maintain proper indentation when expressions span multiple lines:

```r
# Before formatting
data %>%
filter(condition) %>%
select(value)

# After formatting
data %>%
  filter(condition) %>%
  select(value)
```

### Unary Operators

Unary operators receive appropriate spacing based on their type and context:

```r
# Before formatting
result = ! condition
value = - 42
formula = ~x + y

# After formatting
result = !condition
value = -42
formula = ~ x + y
```

**Special spacing rule:** The `~` (formula) operator gets a space when followed by complex expressions, but not when followed by simple identifiers.

### Blocks

**Multiline blocks** always have a newline after the opening brace and before the closing brace:

```r
# Before formatting
{ x <- 1
  print(x)
}

# After formatting
{
  x <- 1
  print(x)
}
```

**Single-line blocks** are allowed, including those with semicolons. The formatter adds a space after `{` and before `}` for readability:

```r
# Before formatting
{x <- 1; print(x)}

# After formatting
{ x <- 1; print(x) }
```

**Semicolons in multiline blocks** are split into separate lines for clarity:

```r
# Before formatting
{
  x <- 1; print(x)
}

# After formatting
{
  x <- 1
  print(x)
}
```

**Empty blocks** have no space between the braces:

```r
# Before formatting
{  }

# After formatting
{}
```

### Parenthesized Expressions

Single-line parenthesized expressions are always formatted in a "hugging" style—there is no extra space between the opening parenthesis and the enclosed expression:

```r
# Before formatting
( x + y )

# After formatting
(x + y)
```

Multiline parenthesized expressions can be formatted in either hugged or expanded style; the formatter preserves both.

```r
# expanded
(
  expression +
    other_part
)

# hugged
(expression +
  other_part)
```

### If Expressions

**Single-line if-else:** Single-line `if-else` expressions are allowed and preserved, since `if` is an expression in R and can be used as a ternary operator:

```r
x <- if (condition) consequence else alternative
```

**Nested if-else:** Nested `if-else` chains are formatted so each `else if` and `else` starts on its own line, with all branches aligned at the same indentation level—no extra indentation for nested cases.

```r
if (a) {
  x
} else if (b) {
  y
} else {
  z
}
```

**Auto-bracing for multiline if-else:** Whenever an `if-else` spans multiple lines, all branches are always wrapped in braces for clarity and consistency:

```r
# Before formatting
if (condition) {
  consequence
} else alternative

# After formatting
if (condition) {
  consequence
} else {
  alternative
}
```

**Auto-bracing for multiline conditions:** If an `if` expression has a multiline condition, the formatter ensures the body is wrapped in braces even if it's a single expression:

```r
# Before formatting
if (
  a && b
) body

# After formatting
if (
  a && b
) {
  body
}
```

### Loops

Loops are the only expressions that are **not allowed** on a single line.

Because `for`, `while`, and `repeat` loops are used solely for their side effects and do not return meaningful values, the formatter always requires these loops to be written across multiple lines with explicit braces (see [auto-bracing](#auto-bracing)). This ensures that side-effecting code is visually distinct from pure expressions.

**For loops** always enforce braced blocks for the body, ensuring consistency:

```r
# Before formatting
for(item in sequence) run_effect(item)

# After formatting
for (item in sequence) {
  run_effect(item)
}
```

**While loops** follow similar block enforcement rules:

```r
# Before formatting
while(condition) action()

# After formatting
while (condition) {
  action()
}
```

**Repeat loops** also enforce braced blocks:

```r
# Before formatting
repeat action()

# After formatting
repeat {
  action()
}
```

**Multiline for loop headers:** The formatter allows both of the following styles for multiline `for` loop headers, preserving your preferred structure:

```r
for (
  item in sequence
) {}

for (
  item
  in sequence
) {}
```

### Function Calls

Function calls receive consistent formatting with proper spacing around argument separators and assignment operators.

```r
# Before formatting
call(a,b=1,...)

# After formatting
call(a, b = 1, ...)
```
**Multiline function calls:** Once two arguments appear on different lines, the call is treated as multiline, and each argument is formatted on its own line for clarity.

```r
# Before formatting
call(
  a = x,
  b = y, c = z
)

# After formatting
call(
  a = x,
  b = y,
  c = z
)
```

**Nested function calls** can use either a hugged style—where the inner call starts right after the outer call's parenthesis—or an expanded style. Both are preserved by the formatter, letting you choose the most readable form for your code.

```r
# Hugged format - both functions start on the same line
result <- outer(inner(
  arg
))

# Expanded format - also valid
result <- outer(
  inner(
    arg
  )
)
```

**Compact multiline calls:** When a call spans multiple lines solely because a single argument (often the final one) is itself multiline, the formatter keeps the other arguments on the original line rather than placing every argument on its own line, preserving a concise, compact layout:

```r
# This format is preserved - only last argument spans multiple lines
call(a = x, b = y, c = inner(
  expr
))
```

This behavior is particularly useful for testing frameworks and S4 method definitions:

```r
test_that("description", {
  expect_equal(result, expected)
})

setMethod("method", "Class", function(x) {
  # ... implementation
}, sealed = TRUE)
```

In the `setMethod` example: Even though `sealed = TRUE` is on a different line than the other arguments, only the function body is multiline, so the formatter preserves this layout.

**Note:** You can always opt in to the fully expanded multiline style: if you add a newline so that at least two arguments of a call are on different lines, the formatter treats it as multiline and will place every argument on its own line.

### Function Definitions

**Single-line functions:** Functions with a simple, single-expression body can be written on one line, with or without braces.

```r
add <- function(x, y) x + y
double <- function(x) { x * 2 }
```

**Multiline functions:** If the function body spans multiple lines, braces are always added—even if the body starts on the same line as the function declaration.

```r
# Before formatting
function()
    call(a = x, b = y)

# After formatting
function() {
  call(a = x, b = y)
}
```

**Exception – multiline call on same line:** If the function body is a function call that starts on the same line and is itself multiline, braces are not required.

```r
fn <- function() call(
  a = x,
  b = y
)
```

**Anonymous functions (lambda expressions):** Anonymous functions using `\` are formatted the same way as named functions, supporting both single-line and multiline bodies.

```r
lapply(data, \(x) x + 1)

lapply(data, \(x) {
  y <- x * 2
  y + 1
})
```

### Switch Statements

Switch statements are formatted like normal function calls. For fallthrough cases (e.g., `case = ,`), an extra space is added after the `=` to clearly indicate the fallthrough.

```r
result <- switch(
  type,
  "a" = handle_a(),
  "b" = ,
  "c" = handle_bc(),
  "default" = handle_default()
)
```

### Subsetting

**Bracket subsetting** follows the same formatting rules as function calls:

```r
# Before formatting
data[ row,col ]
data[[ "name" ]]

# After formatting
data[row, col]
data[["name"]]
```

### Extract & Namespace Operators

**Extract and namespace operators** (`$`, `@`, `::`, `:::`) are formatted without spaces around them:

```r
# Before formatting
collection $ item
collection @ item
pkg :: process
pkg ::: filter

# After formatting
collection$item
collection@item
pkg::process
pkg:::filter
```

When chaining extract or namespace operators across multiple lines, the formatter indents each subsequent line to make the chain visually distinct and easy to follow:

```r
# Before formatting
object$
call(x)$
call(x, y)

# After formatting
object$
  call(x)$
  call(x, y)
```

### String Literals

String literals receive intelligent quote normalization. The formatter prefers double quotes (`"`) unless the string contains unescaped double quotes:

```r
# Before formatting
message <- 'Hello world'
quoted_content <- 'Say "hello"'

# After formatting
message <- "Hello world"
quoted_content <- 'Say "hello"'
```

Multi-line string literals always keep their original indentation and line breaks, no matter where they appear. Even if surrounding code is refactored or deleted, the formatter never changes the internal content of multi-line strings.

```r
# Before formatting
# { <- parent block gets deleted
    x <- "This is a multi-line string.
          It preserves
          indentation and line breaks."
# }

# After formatting
# { <- parent block gets deleted
x <- "This is a multi-line string.
          It preserves
          indentation and line breaks."
# }
```

[Raw string literals](https://search.r-project.org/R/refmans/base/html/Quotes.html) (`r"(...)"`, `R"[...]"`, and their custom-dash variants) are preserved byte-for-byte. Quote normalization is never applied to them, since their whole purpose is to hold characters — including quotes and backslashes — that would otherwise need escaping.

```r
# Before formatting
path <- r"(C:\Users\me)"
quoted <- r"(He said "hi")"

# After formatting
path <- r"(C:\Users\me)"
quoted <- r"(He said "hi")"
```

### R6 Class Definitions

Class definitions with empty lines between methods are preserved:

```r
PersonClass <- R6Class(
  "Person",
  public = list(
    initialize = function(name) {
      private$name <- name
    },

    get_name = function() {
      return(private$name)
    }
  )
)
```

### Comments

The formatter ensures that a space is inserted after the `#` for standard comments. For special comment types like Roxygen (`#'`) and plumber (`#*`) comments, the space is placed after the initial two characters:

```r
# Before formatting
# comment with space
#comment without space
#'roxygen comment
#*plumber comment
#'string' <- commented out string
#!/usr/bin/env Rscript

# After formatting
# comment with space
# comment without space
#' roxygen comment
#* plumber comment
#'string' <- commented out string
#!/usr/bin/env Rscript
```

Additional exceptions to this rule are:

- Commented-out strings such as `#'string'` are left unchanged, since inserting a space (e.g., `#' string'`) would alter the content of the string.
- [Shebangs](https://en.wikipedia.org/wiki/Shebang_(Unix)), for example `#!/usr/bin/env Rscript`, remain unchanged.
- [Quarto](https://quarto.org/docs/computations/execution-options.html) and knitr cell options, for example `#| echo: false`, are kept verbatim, since their `key: value` payload is read by machines.

### Type Annotations

Roughly's type annotations are written in `#:` comments. The formatter treats each block of consecutive `#:` lines as one unit: the block is parsed with the same annotation grammar the type checker uses, and — only when it parses — re-rendered with the canonical spacing used throughout the [typing reference](/typing-reference): one space after `#:`, no space before `,` or `:` and one space after, spaces around `|` and `->`, and no padding inside `(`, `[`, `{`, or generic `<...>`. A leading type-parameter binder such as `<T>` is followed by a space.

```r
# Before formatting
#:fn( x:integer,y:double )->character
render <- function(x, y) as.character(x + y)
#: list[ named : double ]
weights <- list(a = 1.0)
#: Either< E , A >|NULL
outcome <- NULL

# After formatting
#: fn(x: integer, y: double) -> character
render <- function(x, y) as.character(x + y)
#: list[named: double]
weights <- list(a = 1.0)
#: Either<E, A> | NULL
outcome <- NULL
```

Anything that does not parse as an annotation — prose, a dotted type name, a `pkg::fun` reference, a malformed type — is left verbatim beyond ensuring the single space after `#:`, so the formatter never corrupts a comment it does not understand. Consecutive `#:` lines form one annotation block (exactly as the type checker groups them), so a block that is not a single valid annotation — for example two compact annotations with no blank line between them — is also left as written.

The reformat is deliberately non-invasive: token order, identifier casing, and your line breaks are preserved. A single-line annotation stays on one line, an expanded annotation (one written with `@param` / `@return` lines) keeps one directive per line rather than being collapsed into a compact `fn(...)`, and content lines are never rejoined.

```r
# Before formatting
#: @param { integer } count
#: @param { fn(integer)->character } render
#: @returns { character }

# After formatting
#: @param {integer} count
#: @param {fn(integer) -> character} render
#: @returns {character}
```

When an annotation wraps across several `#:` lines, indentation and closing brackets are normalized from the shape of the opening brackets. An opening bracket followed by more content on its own line is *hugged*: its closer stays glued to the token before it. An opening bracket that ends its line is *expanded*: its closer gets a line of its own, aligned with the line that opened it. Each line is indented one step (`indent-width` spaces) per enclosing expanded bracket. Both canonical styles are stable:

```r
# expanded
#: @type Future {
#:   list{
#:     instrument: Instrument,
#:     maturity: integer
#:   }
#: }

# hugged
#: @type Future {list{
#:   instrument: Instrument,
#:   maturity: integer
#: }}
```

and mixed closer shapes normalize to the nearest consistent style:

```r
# Before formatting
#: @type Future {
#:   list{
#:     instrument: Instrument,
#:     maturity: integer
#: }}

# After formatting
#: @type Future {
#:   list{
#:     instrument: Instrument,
#:     maturity: integer
#:   }
#: }
```

A blank line, a non-`#:` comment, or ordinary code ends an annotation block, so unrelated comments are never pulled into one. Trailing empty `#:` lines at the end of a block are dropped.

### Line Spacing

The formatter normalizes line spacing between expressions, allowing at most one empty line:

```r
# Before formatting
x <- 1
y <- 2


z <- 3

# After formatting
x <- 1
y <- 2

z <- 3
```

### Line Endings

The formatter automatically detects and preserves the line ending style (`LF` or `CRLF`) used in the original file.

## Format Suppression

You can disable formatting for specific code sections using the `# fmt: skip` comment directive. This is useful when you want to preserve specific formatting for readability, such as aligned data structures. The skipped expression is preserved byte-exactly, including the column of its first line, so hand-aligned constructs keep their alignment.

The `fmt: skip` directive can be placed before any expression to skip formatting for it:

```r
matrix(
  # fmt: skip
  c(
    1, 2,
    3, 4
  ), # only the c(..) call won't be reformatted
  nrow = 2
)
```

Or, at the end of a line to skip the previous expression:

```R
# the entire matrix(..) call won't be reformatted
matrix(c(1, 2,
         3, 4), nrow=2) # fmt: skip
```

For a whole region, use `# fmt: off` and re-enable formatting with `# fmt: on`. The region between the directives is preserved byte-exactly — original indentation, columns, and blank lines. The directives work at the top level as well as inside functions and `{ ... }` blocks; a region left open runs to the end of its enclosing block.

```r
f <- function() {
  # fmt: off
    x  <-  c(1,   2,
             30,  4)
  # fmt: on
  x
}
```

You can also skip formatting for an entire file by placing `# fmt: skip-file` at the top of the file. This directive must be placed at the very beginning of the file to take effect.

## Rationale

This section explains the key design decisions that guided the formatter's implementation.

### Auto-Bracing

**Accidental bugs:** It's easy to accidentally introduce subtle bugs when omitting braces in loops, function definitions, or `if` expressions. For example, if you later add a line after an unbraced `if`, only the first line is controlled by the condition:

```r
# unbraced condition
if (condition)
  line1
  line2 # <- is meant to be in body

# how it is interpreted:
if (condition)
  line1
line2 # <- gets executed unconditionally
```

Therefore, the formatter always adds braces to **`if` expressions** and **function definitions** whenever the body spans multiple lines.

```r
# Before formatting
if (condition)
  action()

# After formatting
if (condition) {
  action()
}
```

For control flow structures such as `for`, `while`, and `repeat` loops, the formatter always adds braces around the body—regardless of its length—since single-line loops are not allowed (see [Loops](#loops)).

```r
# Before formatting
for (item in sequence)
  action()

# After formatting
for (item in sequence) {
  action()
}
```

### Hugging Behavior

"Hugging" refers to how nested expressions are formatted in multiline contexts—keeping them compact by allowing inner expressions to start on the same line as the outer expression's opening delimiter. This is part of Roughly's non-invasive approach: both hugged and expanded formats are allowed.

**Nested function calls** can be formatted in a hugged style:

```r
# Hugged format - both functions start on the same line
result <- outer(inner(
  arg
))

# Expanded format - also valid
result <- outer(
  inner(
    arg
  )
)
```

**Parenthesized expressions** can also use hugging:

```r
(expression +
  other_part)

# Also allowed
(
  expression +
    other_part
)
```

See the [compact multiline calls](#function-calls) section under Function Calls for more details. The formatter preserves this style, which is especially useful for S4 methods and testing frameworks.
