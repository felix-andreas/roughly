<h1>
  <p align="center">
    <img src="docs/public/logo.svg" alt="Logo" height="128" >
    <br />ry
  </p>
</h1>

<div align="center">

An extremely fast R language server and code formatter, written in Rust.
<br />
[Docs](https://ry-lang.org) · [Releases](https://github.com/felix-andreas/ry/releases) · [VS Code Extension](https://marketplace.visualstudio.com/items?itemName=felix-andreas.ry) · [Zed Extension](https://github.com/felix-andreas/ry/tree/main/editors/zed)

</div>

> [!NOTE]
> ry can be used either as a standalone command-line tool or as an extension in supported editors like VS Code.

## Why ry?

This project was created to address the slow performance of the existing R language server on large codebases. Originally called *"The R(oughly good enough) language server"*, it began as a minimal but fast language server that supported only go-to-definition using regex-based indexing.

It has been rewritten twice since. The current stack dropped tree-sitter for a hand-written R parser, which is what makes diagnostics like these possible — each pointing at the character that is wrong, where a general-purpose grammar can only report that the file stopped making sense somewhere:

```
error[trailing-comma]: Unexpected comma after last argument
 --> a.R:1:12
1 | x <- c(1, 2,)
               ^

error[syntax-error]: missing `,` between these arguments
 --> a.R:2:16
2 | y <- list(a = 1 b = 2)
                   ^
```

On top of that parser sit a static type checker, a formatter, and a linter.

Each rewrite has also made the name shorter. *"Good enough"* went first, leaving **ry**; this one dropped everything except the first and last letter. The pattern is load-bearing — the better it gets, the less of it there is to type — and at this rate the next version is a single keystroke.

## Features

ry aims to support the following language server features (some are experimental or in progress):

- **Formatting**
  - Format entire document
  - Format selected code range *(🧪 experimental)*

- **Navigation**
  - Index global variables, S4 and R6 classes/methods
  - Search current document - <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>O</kbd> *in VS Code*
  - Search global workspace - <kbd>Ctrl</kbd> + <kbd>T</kbd> *in VS Code*
  - Go to definition
  - Find all references

- **Diagnostics**
  - Syntax errors - *including missing or trailing commas*
  - Basic linting rules - *[full list here](https://ry-lang.org/reference/diagnostic-codes)*
  - Warning for unused variables *(opt-in: `[check] unused`)*
  - Error for undefined variable *(⚠️ missing)*
  - Argument validation for function calls *(part of type checking)*
  - Static type checking — HM-style inference with nominal/structural types, function types, nullable unions, and numeric constraints, driven by [`#:` typing comments](https://ry-lang.org/type-checking/concepts). Inferred types power editor features by default; `type-mismatch` diagnostics are opt-in via `[check] typing`.

- **Editing**
  - Autocomplete local and global variables
  - Autocomplete variables from other packages *(⚠️ missing)*
  - Rename local variables
  - Rename global variables *(⚠️ missing)*
  - Signature help — inferred call signature with active parameter
  - Inlay hints — inferred types on unannotated bindings

## ry CLI

### Usage

Run ry as a formatter:

```
ry fmt           # Format all files in the current directory
ry fmt <path>    # Format all files in `<path>`
ry fmt --check   # Only check if files would be formatted
ry fmt --diff    # Only show the diff if files would be formatted
```

To run ry as a linter:

```
ry check         # Check all files in the current directory
ry check <path>  # Check all files in `<path>`
```

Or, to run ry as a language server:

```
ry server        # Usually started automatically by your editor
```

### Installation

For most users the [VS Code extension](#vs-code-extension) is the recommended way in — it bundles the
CLI, so there is nothing else to set up. Install the standalone CLI below when you want it for CI, for
RStudio, or on a platform without a bundled binary.

#### Download Binary

Download the pre-built binary for your platform from the [releases page](https://github.com/felix-andreas/ry/releases).

#### Install with Cargo

If you have [Cargo](https://www.rust-lang.org/tools/install) installed, install ry with:

```sh
cargo install --git https://github.com/felix-andreas/ry ry-lang
```

#### Build from Source

Alternatively, build from source:

```sh
cargo build --release
```

## VS Code extension

### Download from Marketplace (Recommended)

[![](https://vsmarketplacebadges.dev/version-short/felix-andreas.ry.svg)](https://marketplace.visualstudio.com/items?itemName=felix-andreas.ry)

Install the extension from the [VS Code Marketplace](https://marketplace.visualstudio.com/items?itemName=felix-andreas.ry).

> [!NOTE]
> The VS Code extension from the marketplace includes a bundled version of the ry CLI for **Linux x86_64, macOS aarch64 and Windows x86_64**. If you are using a different architecture, you will need to install the ry CLI manually.

### Manual Installation

Alternatively, build the extension from source (or [download from releases](https://github.com/felix-andreas/ry/releases)):

```bash
bun run package
```

Install the generated VSIX file:

```bash
code --install-extension ry.vsix
```

### Extension Settings

You can customize the ry extension in VS Code through the following settings:

```jsonc
{
  // Use a custom binary instead of the bundled one
  "ry.path": "/path/to/ry",
  // Pass custom arguments; defaults to ["server"]
  "ry.args": ["server", "--verbose"],
  // Enable experimental features
  "ry.experimentalFeatures": ["range_formatting"],
}
```

> [!NOTE]
> For a complete list of experimental features and their descriptions, [see below](#experimental-features).

### Commands

You can access ry-specific commands in VS Code via the Command Palette (<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd>):

- **ry: Open logs**
- **ry: Start/Stop/Restart Server**
- **ry: Format workspace** (⚠️ missing)
- **ry: Show syntax tree** (⚠️ missing)

## Zed Extension

Not in Zed's extension registry yet. Install it from
[`editors/zed`](https://github.com/felix-andreas/ry/tree/main/editors/zed) as a dev extension —
its README has the three steps.

## RStudio Integration

ry can be used as an external formatter in RStudio. See the [RStudio setup guide](https://ry-lang.org/installation#rstudio) for detailed instructions.

## Configuration

You can configure ry via a project-specific `ry.toml` file:

```toml
[format]
# Number of spaces per indentation level
indent-width = 4
# Automatically detect the appropriate line ending
line-ending = "auto" # "lf" or "cr-lf"

[lint]
# Control the naming convention for variables and parameters
naming-style = "snake_case" # or "camelCase", omit to disable this lint entirely

[check]
# Surface static type-checking errors (inferred types power editor features regardless)
typing = true
# Warn about unused local variables
unused = true
```

## Experimental Features

ry includes a few experimental features that can be enabled in the VS Code extension settings or via the CLI:

| Name               | Description                      |
| ------------------ | -------------------------------- |
| `all`              | Enable all experimental features |
| `range_formatting` | Format selected code ranges      |

Type checking and unused-variable warnings are no longer experimental — configure them under `[check]` in `ry.toml` (see above).

## Development

See our [development documentation](https://ry-lang.org/contributing/development).

## License

This repository is licensed under [The Universal Permissive License Version 1.0](LICENSE).
