---
title: Configuration
description: Configuration options for Roughly
---

Roughly is deliberately opinionated and minimal in its configuration options. It prefers sensible defaults over excessive customization.

## Configuration File

Configure Roughly with a `roughly.toml` file in your project. Roughly uses the nearest one: it searches a starting directory and then each of its ancestors, and the first `roughly.toml` found wins. If none is found, the defaults below apply.

The starting directory depends on how Roughly runs:

- **Editor (language server):** the search starts at the workspace root your editor announces when it starts the server; the server process's working directory is used only when the editor announces no root (for example, a single file opened without a folder). Edits to the workspace's `roughly.toml` are picked up live, and diagnostics refresh immediately so toggles like `[check] strict` apply without further edits.
- **CLI:** the search starts at each file or directory argument (a file's own directory), so `roughly check R/utils.R` and `roughly check .` inside the same project resolve the same configuration.

```toml
[format]
# number of spaces per indentation level (default: 2)
indent-width = 2
# line ending: "auto" (detect), "lf", or "cr-lf" (default: "auto")
line-ending = "auto"

[lint]
# naming convention for variables and parameters:
# "snake_case" or "camelCase"; omit to disable this check
naming-style = "snake_case"

[check]
# report type errors and function-call argument mismatches (default: false)
typing = true
# report assignments whose value is never read (default: false)
unused = true
# report sites with a genuinely undetermined (`Unknown`) type (default: false)
strict = false
```

## Formatting — `[format]`

- `indent-width` — spaces per indentation level. Default `2`.
- `line-ending` — `"auto"` detects the appropriate ending, or force `"lf"` / `"cr-lf"`. Default `"auto"`.

## Linting — `[lint]`

- `naming-style` — enforce `"snake_case"` or `"camelCase"` for variable and parameter names. Omit the key to disable the naming check entirely.
- Per-lint levels — every other lint is keyed by its stable code and takes `"off"`, `"warn"`, or `"error"` (omit the key to keep the lint's built-in severity):

```toml
[lint]
assignment-operator = "off"
boolean-shorthand = "error"
missing-comma = "warn"
trailing-comma = "warn"
unused-parameter = "warn"  # default off: opt in to flag never-used function parameters
```

`unused-parameter` is **off by default** — R signatures legitimately carry ignored formals (an S3
method must match its generic's signature; callbacks receive arguments they ignore) — so it only
speaks when a project opts in. Parameters named with a leading `.` or `_` are never reported.

For one-off exceptions, prefer a [suppression comment](/linter#suppressing-diagnostics) over turning a lint off project-wide.

## Checking — `[check]`

Type *inference* always runs — it powers editor features such as hover types, inlay hints, and signature help regardless of these settings. The `[check]` flags only control which diagnostics `roughly check` and your editor surface, so a project that has not adopted annotations is not flooded with messages.

- `typing` — report `type-error` diagnostics, including function-call argument mismatches. See the [Typing guide](/typing). Default `false`.
- `unused` — report assignments whose value is never read (`unused` diagnostics). Default `false`.
- `strict` — report each site with a genuinely undetermined (`Unknown`) type — an unsupported construct or a reference to a binding with no known type. See [strict mode](/typing-reference#strict-mode). Default `false`. A single file can override this switch with a top-level `#: @strict` or `#: @strict off` comment.

## Debugging — top-level `debug`

- `debug` — surface internal analysis facts, such as a "Debug" section in editor hovers showing the lowered representation. A developer aid for working on Roughly itself. Default `false`.

## Invalid Configuration

`roughly.toml` is validated strictly: an unknown key, a misspelled key (for example `naming_style` instead of `naming-style`), or a value of the wrong type is an error. The message names the file and points at the offending line and column, for example:

```
invalid config in /path/to/roughly.toml at line 2, column 1: unknown field `naming_style`, expected `naming-style`
```

Where the error surfaces:

- **Editor:** the language server never crashes on a malformed config. At startup it falls back to the defaults; on a live edit to `roughly.toml` it keeps the previous configuration. Either way it reports the error as an editor error message **and** publishes a diagnostic on `roughly.toml` itself, pointing at the offending line — it stays in the problems panel until the config loads again.
- **CLI:** `roughly check` and `roughly fmt` print the error to stderr and exit 2 (see the
  [exit-code table](/installation#exit-codes)).

## Legacy Keys

The top-level `case` and `spaces` keys are deprecated spellings of `lint.naming-style` and `format.indent-width`. They still parse, and take precedence over the section keys when both are present.

## Default Behavior

If no `roughly.toml` is found, Roughly uses 2-space indentation, automatic line endings, no naming check, and all `[check]` diagnostics off.
