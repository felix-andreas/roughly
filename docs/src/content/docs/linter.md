---
title: Linter
description: Documentation for Roughly's R code linter and static analyzer
---

Roughly includes a built-in linter to catch errors and enforce consistent coding practices in your R code.

## Usage

Lint your R files using the command line:

```sh
roughly check              # Check all files in the current directory
roughly check <path>       # Check all files in <path>
```

## Current Checks

The linter is divided into two main categories of checks:

### Syntax Checks

Syntax checks identify structural problems in your code that would cause R to raise errors:

- **Missing delimiters**: Detects unclosed parentheses, brackets, and braces
- **Missing operands**: Identifies binary operators missing their right-hand side operand
- **Missing function bodies**: Finds function definitions without an implementation
- **Unexpected delimiters**: Warns about unexpected closing delimiters like `}`, `)`, or `]`
- **General syntax errors**: Catches other syntax issues that would prevent code execution

Example:
```r
# Missing closing bracket
foo <- function(x {
  x + 1
}

# Missing right-hand side
y <- 
```

### Semantics Checks

Semantics checks enforce coding conventions and best practices that won't necessarily cause errors but may lead to bugs or reduce code quality:

- **Variable naming convention**: Enforces consistent naming style (snake_case or camelCase) according to your configuration
- **Parameter naming convention**: Ensures function parameters follow the same naming convention
- **Assignment operator**: Recommends using `<-` rather than `=` for variable assignment
- **Trailing commas**: Flags unnecessary trailing commas in function calls
- **Boolean values**: Use `TRUE` and `FALSE` over `T` and `F`

Example:
```r
# Using = instead of <- for assignment (warning)
x = 10

# Inconsistent naming convention (warning)
calculate_mean <- function(dataSet) {
  # ...
}

# Trailing comma (warning)
result <- sum(1, 2, 3,)
```

### Opt-in Checks

Three further checks are available but off by default; enable them in `roughly.toml`:

```toml
[check]
typing = true   # report type errors and function-call argument mismatches
unused = true   # report assignments whose value is never read
strict = true   # report expressions whose type the checker could not determine
```

- **Unused assignments** (diagnostic code `unused`): Flags assignments whose value is never read
  on any control-flow path — an unread variable, or a dead store overwritten before every read.
  Conditional updates and loop accumulators that a later read observes are *not* flagged. Function
  parameters, `for`-loop variables, top-level (package-visible) bindings, and names starting with
  `.` or `_` are never reported.
- **Type checking**: Reports type errors and argument mismatches from Roughly's static type
  checker. Type *inference* is always on (it powers editor features); this setting controls whether
  `roughly check` surfaces type-error diagnostics. See [Type Checker](/type-checker).
- **Strict mode**: Reports the places where the checker genuinely could not determine a type. See
  the [typing reference](/typing-reference#strict-mode) for exactly what strict mode flags.

## Configuration

For details on configuring the linter, see the [Configuration](/configuration) page.

## Roadmap

Roughly's static analysis will continue to expand in future versions to include:

- **Unreachable code**: Identification of code that will never be executed
- **Control flow analysis**: Detecting potential infinite loops or missing return statements
- **Package-specific rules**: Special rules for popular packages like dplyr, ggplot2, and data.table

These features are not yet implemented.

## Integration

Roughly's linter integrates seamlessly with its language server, providing real-time feedback in compatible editors like VS Code. Diagnostics appear as you type, with appropriate highlighting and hover information.
