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

- **`naming-style`**: Enforces consistent naming style (snake_case or camelCase) for variables and function parameters, according to your configuration. `SCREAMING_SNAKE_CASE` conforms under either style at any scope — it is R's universal spelling for a constant, so the check never rewrites it
- **`assignment-operator`**: Recommends using `<-` rather than `=` for variable assignment
- **`trailing-comma`**: Flags an unnecessary trailing comma after a call's last argument. (An earlier `missing-comma` lint is retired — the parser now rejects `f(1 2)` with a syntax error, exactly as R does; the config key still parses and is ignored)
- **`boolean-shorthand`**: Use `TRUE` and `FALSE` over `T` and `F`
- **`unused-parameter`** *(off by default)*: Flags function parameters no read ever uses. Off
  unless enabled in `roughly.toml` (`[lint] unused-parameter = "warn"`), because R signatures
  legitimately carry ignored formals; `.`/`_`-prefixed names are never reported, and an S3
  method's formals are never reported at all — `format.myclass(x, ...)` that ignores `x` is
  matching its generic, which R requires. A name counts as an S3 method when the part before its
  last dot is a name the standard-library corpus declares, so `my.helper` is still checked
- **`stub`**: a problem in a `.Rtypes` [stub file](/stdlib-stubs) the project ships — a declaration
  that does not parse, or one naming a type no vocabulary declares. Reported against the stub file
  itself, since a stub that does not load silently withdraws the types it was meant to provide
- **undefined exports**: an `export(name)` in the `NAMESPACE` naming something the package defines
  nowhere at top level is an error — `R CMD check`'s "undefined exports", reported before install
  rather than at it. Only explicit `export()` names are checked: `exportPattern` is a regex R
  resolves at load time, and `exportClasses`/`exportMethods`/`S3method` name S4 and S3 entities
  rather than bindings
- **`unused-import`** *(off by default)*: Flags an `importFrom(pkg, name)` in the `NAMESPACE` whose
  `name` appears in no checked source. Off unless enabled (`[lint] unused-import = "warn"`); usage is
  a conservative token scan (a name used via `pkg::name` or an operator import like `%>%` counts), so
  it never false-positives on a real use, and whole-namespace `import(pkg)` is not checked
- **`shadows-builtin`** *(off by default)*: Flags a top-level binding whose name `base` exports
  (`mean <- function(x) ...`) — every bare read in the project now resolves to the binding instead
  of the builtin. Off unless enabled (`[lint] shadows-builtin = "warn"`) because rebinding a base
  name is often deliberate: an S3 method definition, or intentional masking in a script. Dotted S3
  method names (`print.myclass`) are not base exports and are never flagged
- **`shadows-namespace`** *(off by default)*: The same check for names declared by the non-`base`
  stub namespaces, which resolve bare exactly like builtins; the message names the shadowed symbol
  (``Top-level binding `sd` shadows `stats::sd`.``). Enable with
  `[lint] shadows-namespace = "warn"`

Example:
```r
# Using = instead of <- for assignment (warning)
x = 10

# Inconsistent naming convention (warning)
calculate_mean <- function(dataSet) {
  # ...
}

# Trailing comma (error)
result <- sum(1, 2, 3,)
```

### Suppressing diagnostics

A `# roughly: allow(code, ...)` comment suppresses matching diagnostics on its own line (as a
trailing comment) or on the line directly below it. The codes are the bracketed names diagnostics
render with — see the [diagnostic codes reference](/diagnostics) for every one — and
`allow(all)` suppresses everything for that line:

```r
flag <- T  # roughly: allow(boolean-shorthand)

# roughly: allow(unused)
scratch <- compute_debug_info()
```

Use suppressions for genuine exceptions; a file that needs many of them is usually asking for a
configuration change instead — every lint's level can be set project-wide in
[`roughly.toml`](/configuration#linting--lint) (`assignment-operator = "off"`).

### Opt-in Checks

Three further checks are controlled by the `[check]` section of `roughly.toml`. The unused check
is on by default; the other two are opt-in:

```toml
[check]
typing = true    # report type errors and function-call argument mismatches (default: false)
unused = false   # opt out of unused-assignment warnings (default: true)
strict = true    # report expressions whose type the checker could not determine (default: false)
```

- **Unused assignments** (diagnostic code `unused`, on by default): Flags assignments whose value
  is never read on any control-flow path — an unread variable, or a dead store overwritten before
  every read. In standalone scripts this includes top-level bindings no later statement or nested
  function reads. Conditional updates and loop accumulators that a later read observes are *not*
  flagged. Function parameters, `for`-loop variables, package top-level (package-visible)
  bindings, names starting with `.` or `_`, and bindings inside a syntax-error region are never
  reported.
- **Type checking**: Reports type errors and argument mismatches from Roughly's static type
  checker. Type *inference* is always on (it powers editor features); this setting controls whether
  `roughly check` surfaces `type-mismatch` diagnostics. See the [Typing guide](/typing/guide).
- **Strict mode**: Reports the places where the checker genuinely could not determine a type. See
  the [typing reference](/typing/reference#strict-mode) for exactly what strict mode flags.

## Configuration

For details on configuring the linter, see the [Configuration](/configuration) page.

## Not implemented yet

- **Unreachable code** — statements that can never be executed
- **Control-flow analysis** — potential infinite loops, missing returns
- **Package-specific lint rules** — checks that encode a particular package's conventions. Note
  this is about *lints*: `dplyr`, `data.table`, `ggplot2` and `testthat` are already understood by
  the type checker, including their data-masked verbs, through
  [conditional stub namespaces](/stdlib-stubs#conditional-namespaces)
- **Automatic fixes** — every finding is reported, none are rewritten; there is no `--fix`

## Integration

The linter runs inside the [language server](/language-server), so the same diagnostics appear
live in your editor as you type — no separate lint step needed.
