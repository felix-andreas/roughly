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
* **Readability first**: Makes formatting choices that enhance code readability

## Formatting Rules

Based on the implementation, Roughly's formatter follows these rules:

- **Indentation**: Uses spaces for indentation (configurable via `spaces` in `roughly.toml`)
- **Comments**:
  - Reformats single-line comments like `#foo` to `# foo` for better readability
  - Preserves roxygen comments (`#'`) and only adds a space if missing (`#'foo` → `#' foo`)
  - Preserves other special comment types (e.g., `##`, `###`, `#!/usr/bin/env Rscript`)
  
- **Expressions**:
  - Adds appropriate spacing around operators (`x<-1` → `x <- 1`)
  - Formats binary operators with consistent spacing
  - Preserves pipeline operators (`|>`, `%>%`) style with appropriate line breaks
  
- **Function calls**:
  - Standardizes argument spacing (`foo(a=1,b=2)` → `foo(a = 1, b = 2)`)
  - For multi-line function calls, properly indents arguments
  - Preserves special cases like braced expressions in arguments

- **Blocks**:
  - Properly formats braced expressions with consistent indentation
  - Preserves single-line blocks (`{ foo; bar }`) when they're already single-line
  - Adds proper indentation for multi-line blocks

- **Control flow**:
  - Formats `if`, `for`, `while`, and `repeat` statements consistently
  - Ensures proper indentation of conditional bodies
  - Formats `if-else-if` chains with consistent style

## Format Suppression

You can disable formatting for specific code sections using the `# fmt: skip` comment directive:

```r
# fmt: skip
foo <- c(1,2,
3)  # This code won't be reformatted

bar <- c(1, 2,
         3)  # This code will be formatted

foo <- c(1,2,
3) # fmt: skip
# The line above won't be reformatted
```

The `fmt: skip` directive can be placed:

- Before a specific line to skip formatting that line
- At the end of a line to skip formatting that line

## Handling Special Cases

The formatter intelligently handles various R code idioms and special patterns:

### Package-Specific Formatting

- **R6 Classes**: Preserves proper spacing between methods and fields in R6 class definitions
- **Data.table**: Special handling for data.table syntax like `DT[, .(column)]` and `:=` assignment chains
- **dplyr/Tidyverse**: Maintains readability of pipe chains with operators like `%>%` and `|>`

### Structural Elements

- **Switch statements**: Properly formats switch statements with fallthrough cases (`case = ,`)
- **Multi-line strings**: Preserves indentation and structure in multi-line string literals
- **Special comments**: Respects shebangs, roxygen documentation, and other special comment types

### Edge Cases

- **Empty blocks**: Formats empty blocks (`{}`) consistently
- **Matrix indexing**: Properly handles complex subsetting operations with multiple empty dimensions (`[,,]`)
- **Expression sequences**: Maintains readability in expression sequences (e.g., `{ expr1; expr2 }`)

## Line Endings

The formatter automatically detects and preserves the line ending style (LF or CRLF) used in the original file.
