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

## Configuration

Linter behavior can be customized via the `roughly.toml` configuration file:

```toml
# Control the naming convention for variables and parameters
case = "snake_case"  # or "camelCase"
```

## Roadmap

Roughly's static analysis capabilities will expand in future versions to include:

- **Argument validation**: Checking that function calls provide the correct number of arguments
- **Type checking**: Basic type inference and validation for function arguments and returns
- **Unused variables**: Detection of unused variable declarations
- **Unreachable code**: Identification of code that will never be executed
- **Control flow analysis**: Detecting potential infinite loops or missing return statements
- **Package-specific rules**: Special rules for popular packages like dplyr, ggplot2, and data.table

These advanced static analysis features are currently in development and will be added in future releases.

## Integration

Roughly's linter integrates seamlessly with its language server, providing real-time feedback in compatible editors like VS Code. Diagnostics appear as you type, with appropriate highlighting and hover information.
