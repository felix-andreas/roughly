---
title: Linter
description: Documentation for Roughly's R code linter and static analyzer
---

Roughly includes a built-in linter to catch errors and enforce consistent coding practices in your R code.

## Usage

Lint your R files using the command line:

```sh
roughly check                # Check all files in the current directory
roughly check <path>         # Check all files in <path>
roughly check --output json  # One JSON object per diagnostic, on stdout (for CI)
```

The exit code is 0 when there are no diagnostics, 1 when any diagnostic is reported (warnings
included), and 2 on a usage, configuration, or I/O error. The JSON output contract and the full
exit-code table live on the [installation page](/installation#exit-codes).

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

Semantics checks enforce coding conventions and best practices that won't necessarily cause errors but may lead to bugs or reduce code quality. Each check has a stable code, shown in brackets in diagnostics (`warning[naming-style]`) and usable in [suppression comments](#suppressing-diagnostics):

- **`naming-style`**: Enforces consistent naming style (snake_case or camelCase) for variables and function parameters, according to your configuration
- **`assignment-operator`**: Recommends using `<-` rather than `=` for variable assignment
- **`missing-comma`** / **`trailing-comma`**: Flags a missing comma between call arguments and an unnecessary trailing comma after the last one
- **`boolean-shorthand`**: Use `TRUE` and `FALSE` over `T` and `F`
- **`unused-parameter`** *(off by default)*: Flags function parameters no read ever uses. Off
  unless enabled in `roughly.toml` (`[lint] unused-parameter = "warn"`), because R signatures
  legitimately carry ignored formals; `.`/`_`-prefixed names are never reported

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

### Suppressing diagnostics

A `# roughly: allow(code, ...)` comment suppresses matching diagnostics on its own line (as a
trailing comment) or on the line directly below it. The codes are the bracketed names diagnostics
render with — the lint codes above plus `unused`, `unresolved`, and the other check codes —
and `allow(all)` suppresses everything for that line:

```r
flag <- T  # roughly: allow(boolean-shorthand)

# roughly: allow(unused)
scratch <- compute_debug_info()
```

Use suppressions for genuine exceptions; a file that needs many of them is usually asking for a
configuration change instead — every lint's level can be set project-wide in
[`roughly.toml`](/configuration#linting--lint) (`assignment-operator = "off"`).

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
  `roughly check` surfaces type-error diagnostics. See the [Typing guide](/typing).
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

The linter runs inside the [language server](/language-server), so the same diagnostics appear
live in your editor as you type — no separate lint step needed.
