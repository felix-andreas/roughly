---
title: Formatter
description: Documentation for Roughly's R code formatter.
---

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

Roughly's formatter applies specific rules to each type of R code construct. Here are the detailed rules with examples:

### Arguments

Arguments are formatted with spaces around equals signs in assignments. Single-line argument lists remain on one line, but when any argument spans multiple lines, all arguments are placed on their own lines with proper indentation:

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

### Binary Operators

Spaces are added around binary operators, except for `:` which creates a range:

```r
# Before formatting
x<-1
y=2
1:10

# After formatting
x <- 1
y = 2
1:10
```

Multiline expressions with binary operators are indented to maintain readability:

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

Contents of a block are always indented. Single line blocks remain on one line (even with semicolons), but multiline blocks have each expression on its own line:

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

### Calls

Function calls follow similar formatting rules as arguments:

```r
# Before formatting
foo(a=1, b=2)
bar(a=1,
  b=2)
baz({
  a
})

# After formatting
foo(a = 1, b = 2)
bar(
  a = 1,
  b = 2
)
baz({
  a
})
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

### Function Definitions

Function definitions follow similar rules as calls:

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

### Parenthesized Expressions

Parenthesized expressions follow similar rules to calls and blocks:

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

### If Statements

One-line if statements are preserved, but if the condition or any body is multiline, all bodies become multiline:

```r
# Before formatting
if (x) {y} else {z}
if (x) {
  y
} else {z}
if (
  x
) { y }

# After formatting
if (x) { y } else { z }
if (x) {
  y
} else {
  z
}
if (
  x
) {
  y
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

### Strings

String literals are consistently formatted using double quotes (`"`) instead of single quotes (`'`):

```r
# Before formatting
x <- 'hello'

# After formatting
x <- "hello"
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

matrix(c(1,2,
3, 4), nrow=2) # fmt: skip
# The line above won't be reformatted
```

The `fmt: skip` directive can be placed:

- Before a specific line to skip formatting that line
- At the end of a line to skip formatting that line

## Handling Special Cases

The formatter intelligently handles various R code idioms and special patterns:

- **Switch statements**: Properly formats switch statements with fallthrough cases (`case = ,`)
- **Multi-line strings**: Preserves indentation and structure in multi-line string literals
- **Special comments**: Respects shebangs, roxygen documentation, and other special comment types
- **Emtpy lines in R6 ddefintions**: One empty line is allowed in R6 class definitions for better readability.
- **Empty blocks**: Formats empty blocks (`{}`) consistently
- **Matrix indexing**: Properly handles complex subsetting operations with multiple empty dimensions (`[,,]`)
- **Expression sequences**: Maintains readability in expression sequences (e.g., `{ expr1; expr2 }`)

## Line Endings

The formatter automatically detects and preserves the line ending style (`LF` or `CRLF`) used in the original file.
