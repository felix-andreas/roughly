---
title: Formatter
description: Documentation for Roughly's R code formatter.
---
<!-- R CODE IN THIS FILE IS FORMATTED AND SAVED TO docs/content/formatter.md -->

Roughly includes a non-invasive R code formatter that emphasizes readability while respecting the existing structure of your code.

## Usage

Format your R files using the command line:

```sh
roughly fmt                # Format all files in the current directory
roughly fmt <path>         # Format all files in <path>
roughly fmt --check        # Only check if files would be formatted
roughly fmt --diff         # Show diff of formatting changes without applying them
```

## Philosophy

The formatter follows these key principles:

* **Non-invasive formatting**: The formatter only adds line breaks if expressions are already multi-line, and won't break one-liners unnecessarily
* **Consistent style**: Standardizes spacing, indentation, and other aspects of R code style

## Formatting Rules

Roughly applies specific formatting rules to different R code constructs. Here are the detailed rules with before/after examples:


### Binary Operators

Spaces are added around binary operators, except for the range operator (`:`):

```r
# Before formatting
x<-1
1+2
1:10
```

Multi-line expressions with binary operators maintain indentation for readability:

```r
# Before formatting
foo() %>%
bar() %>%
baz()

# After formatting
foo() %>%
  bar() %>%
  baz()
```

### Blocks

Code blocks are consistently indented. Single-line blocks remain compact (even with semicolons), while multi-line blocks format each expression on its own line:

```r
# Before formatting
{foo;bar}
{
foo; bar
}

# After formatting
{ foo; bar }
{
  foo
  bar
}
```

### Comments

Comments are formatted by adding a space after the hash if necessary:

```r
# Before formatting
#foo
#'foo

# After formatting
# foo
#' foo
```

The formatter preserves:
- Roxygen comments (`#'`)
- Special comment types (e.g., `##`, `###`, `#!/usr/bin/env Rscript`)

### Empty Lines

Only one empty line is allowed between code blocks. Successive newlines are merged:

```r
# Before formatting
function() {
  foo()


  bar()
}

# After formatting
function() {
  foo()

  bar()
}
```

### Function Calls

Arguments are formatted with consistent spacing around equals signs. Single-line argument lists remain on one line, while multi-line arguments receive proper indentation:

```r
# Before formatting
foo(a=1,b=2)
bar(a=1,
    b=2)

# After formatting
foo(a = 1, b = 2)
bar(
  a = 1,
  b = 2
)
```

### Function Definitions

Function definitions follow consistent formatting rules for parameters and body:

```r
# Before formatting
foo <- function(a=1, b=2) {}
bar <- function(a=1,
                b=2) {}

# After formatting
foo <- function(a = 1, b = 2) {}
bar <- function(
  a = 1,
  b = 2
) {}
```

### If Statements

One-line conditional statements are preserved, but multi-line bodies trigger consistent formatting for all parts:

```r
# Before formatting
if (x) {y} else {z}
if (x) {
  y
} else {z}

# After formatting
if (x) { y } else { z }
if (x) {
  y
} else {
  z
}
```

For if-statements with multi-line conditions, the formatter enforces a multiline block structure for the body:

```r
# Before formatting
if (
  x && y
) z


# After formatting
if (
  x && y
) {
  z
}
```

### Loops

For `for`, `while`, and `repeat` loops, a block is always enforced for the body:

```r
# Before formatting
for (i in 1:3) foo()
while (TRUE) foo()
repeat foo()

# After formatting
for (i in 1:3) {
  foo()
}
while (TRUE) {
  foo()
}
repeat {
  foo()
}
```

### Parenthesized Expressions

Parenthesized expressions maintain their layout, with consistent formatting for multi-line expressions:

```r
# Before formatting
(a+b)
(a +
 b)

# After formatting
(a + b)
(
  a + b
)
```

### Strings

String literals are consistently formatted, preserving escape sequences and multi-line content:

```r
# Before formatting
x <- "hello"
y <- 'world'

# After formatting
x <- "hello"
y <- "world"
```

### Subset Operations

Subset operations (`[]` and `[[]]`) follow the same formatting rules as function calls:

```r
# Before formatting
x[i=1,j=2]
x[i=1,
  j=2]

# After formatting
x[i = 1, j = 2]
x[
  i = 1,
  j = 2
]
```

## Format Suppression

You can disable formatting for specific code sections using the `# fmt: skip` comment directive:

```r
# fmt: skip
matrix(
  c(
    1, 2,
    3, 4
  ),
  nrow=2
) # This code won't be reformatted

matrix(c(1, 2,
         3, 4), nrow = 2)  # This code will be formatted

matrix(c(1, 2,
         3, 4), nrow=2) # fmt: skip
# The line above won't be reformatted
```

The `fmt: skip` directive can be placed:
- Before a specific line to skip formatting that line
- At the end of a line to skip formatting that line

## Special Cases

The formatter intelligently handles various R idioms and special patterns:

- **Blocks in calls or parenthesized expressions**: When a single code block is inside parentheses, it doesn't receive an additional level of indentation (e.g. `foo({ expr })`)
- **Empty blocks**: Consistently formats empty blocks (`{}`)
- **Empty lines in R6 definitions**: Allows one empty line in R6 class definitions
- **Expression sequences**: Maintains readability in sequences with semicolons (e.g., `{ expr1; expr2 }`)
- **Matrix indexing**: Handles complex subsetting with empty dimensions (`[,,]`)
- **Multi-line strings**: Preserves structure in multi-line string literals
- **Special comments**: Respects shebangs, roxygen documentation, and other comment types
- **Switch statements**: Properly formats fallthrough cases (`case = ,`)

## Line Endings

The formatter automatically detects and preserves the line ending style (`LF` or `CRLF`) used in the original file.

## Configuration

For details on configuring the formatter, see the [Configuration](/configuration) page.
