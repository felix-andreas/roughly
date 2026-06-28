---
title: Configuration
description: Configuration options for Roughly
---

Roughly is deliberately opinionated and minimal in its configuration options. It prefers sensible defaults over excessive customization.

## Configuration File

Configure Roughly with a `roughly.toml` file in your project. Roughly uses the nearest one, searching the target file's directory and then its ancestors; if none is found, the defaults below apply.

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
# report unused local variables and parameters (default: false)
unused = true
# report sites with a genuinely undetermined (`Unknown`) type (default: false)
strict = false
```

## Formatting — `[format]`

- `indent-width` — spaces per indentation level. Default `2`.
- `line-ending` — `"auto"` detects the appropriate ending, or force `"lf"` / `"cr-lf"`. Default `"auto"`.

## Linting — `[lint]`

- `naming-style` — enforce `"snake_case"` or `"camelCase"` for variable and parameter names. Omit the key to disable the naming check entirely.

## Checking — `[check]`

Type *inference* always runs — it powers editor features such as hover types, inlay hints, and signature help regardless of these settings. The `[check]` flags only control which diagnostics `roughly check` and your editor surface, so a project that has not adopted annotations is not flooded with messages.

- `typing` — report `type-error` diagnostics, including function-call argument mismatches. See the [Type Checker](/type-checker). Default `false`.
- `unused` — report unused local variables and parameters. Default `false`.
- `strict` — report each site with a genuinely undetermined (`Unknown`) type — an unsupported construct or a reference to a binding with no known type. See [strict mode](/typing-reference#strict-mode). Default `false`.

## Default Behavior

If no `roughly.toml` is found, Roughly uses 2-space indentation, automatic line endings, no naming check, and all `[check]` diagnostics off.

Roughly's formatter and linter are opinionated tools that don't aim to support every possible coding style. Instead, they enforce a consistent, readable style based on R community practices.
