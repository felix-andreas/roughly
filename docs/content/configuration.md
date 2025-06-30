---
title: Configuration
description: Configuration options for Roughly
---

Roughly is deliberately opinionated and minimal in its configuration options. It prefers sensible defaults over excessive customization.

## Configuration File

You can configure Roughly via a project-specific `roughly.toml` file placed at the root of your project:

```toml
[format]
# number of spaces per indentation level
indent-width = 4
# automatically detect the appropriate line ending
line-ending = "auto" # "lf" or "cr-lf"

[lint]
# control the naming convention for variables and parameters
naming-style = "snake_case" # or "camelCase", omit to disable this lint entirely
```

## Default Behavior

If no configuration file is found, Roughly will:

1. Look for a `roughly.toml` file in the project root directory
2. Fall back to the default values if no file is found
3. Use the defaults of no case linting and `2` spaces for indentation

Roughly's formatter and linter are opinionated tools that don't aim to support every possible coding style. Instead, they enforce a consistent, readable style based on R community practices.
