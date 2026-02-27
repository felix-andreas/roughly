<h1>
  <p align="center">
    <img src="docs/public/logo.svg" alt="Logo" height="128" >
    <br />Roughly
  </p>
</h1>

<div align="center">

An extremely fast R language server and code formatter, written in Rust.
<br />
[Docs](https://roughly.felixandreas.me) · [Releases](https://github.com/felix-andreas/roughly/releases) · [VS Code Extension](https://marketplace.visualstudio.com/items?itemName=felix-andreas.roughly) · [Zed Extension](https://zed.dev/extensions/roughly)

</div>

> [!NOTE]
> Roughly can be used either as a standalone command-line tool or as an extension in supported editors like VS Code.

## Why Roughly?

This project was created to address the slow performance of the existing R language server on large codebases. Originally called *"The R(oughly good enough) language server"*, it began as a minimal but fast language server that supported only go-to-definition using regex-based indexing. Since then, the project has evolved into a full-featured language server with proper parsing, formatting, and linting, and the "good enough" part was eventually dropped from the name.

## Features

Roughly aims to support the following language server features (some are experimental or in progress):

- **Formatting**
  - Format entire document
  - Format selected code range *(🧪 experimental)*

- **Navigation**
  - Index global variables, S4 and R6 classes/methods
  - Search current document - <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>O</kbd> *in VS Code*
  - Search global workspace - <kbd>Ctrl</kbd> + <kbd>T</kbd> *in VS Code*
  - Go to definition
  - Find all references *(🧪 experimental)*

- **Diagnostics**
  - Syntax errors - *including missing or trailing commas*
  - Basic linting rules - *[full list here](https://roughly.felixandreas.me/linter/#semantics-checks)*
  - Warning for unused variables *(🧪 experimental)*
  - Error for undefined variable *(⚠️ missing)*
  - Argument validation for function calls *(⚠️ missing)*
  - Type checking *([💡 early design phase](crates/typing/README.md))*

- **Editing**
  - Autocomplete local and global variables
  - Autocomplete variables from other packages *(⚠️ missing)*
  - Rename local variables *(🧪 experimental)*
  - Rename global variables *(⚠️ missing)*
  - Signature help *(🔨 work in progress)*

## Roughly CLI

### Usage

Run Roughly as a formatter:

```
roughly fmt           # Format all files in the current directory
roughly fmt <path>    # Format all files in `<path>`
roughly fmt --check   # Only check if files would be formatted
roughly fmt --diff    # Only show the diff if files would be formatted
```

To run Roughly as a linter:

```
roughly check         # Check all files in the current directory
roughly check <path>  # Check all files in `<path>`
```

Or, to run Roughly as a language server:

```
roughly server        # Usually started automatically by your editor
```

### Installation

#### Download Binary (Recommended)

Download the pre-built binary for your platform from the [releases page](https://github.com/felix-andreas/roughly/releases).

#### Install with Cargo

If you have [Cargo](https://www.rust-lang.org/tools/install) installed, install Roughly with:

```sh
cargo install --git https://github.com/felix-andreas/roughly roughly
```

#### Build from Source

Alternatively, build from source:

```sh
cargo build --release
```

## VS Code extension

### Download from Marketplace (Recommended)

[![](https://vsmarketplacebadges.dev/version-short/felix-andreas.roughly.svg)](https://marketplace.visualstudio.com/items?itemName=felix-andreas.roughly)

Install the extension from the [VS Code Marketplace](https://marketplace.visualstudio.com/items?itemName=felix-andreas.roughly).

> [!NOTE]
> The VS Code extension from the marketplace includes a bundled version of the Roughly CLI **only for Windows and Linux x64**. If you are using macOS or a different architecture, you will need to install the Roughly CLI manually.

### Manual Installation

Alternatively, build the extension from source (or [download from releases](https://github.com/felix-andreas/roughly/releases)):

```bash
bun run package
```

Install the generated VSIX file:

```bash
code --install-extension roughly.vsix
```

### Extension Settings

You can customize the Roughly extension in VS Code through the following settings:

```jsonc
{
  // Use a custom binary instead of the bundled one
  "roughly.path": "/path/to/roughly",
  // Pass custom arguments; defaults to ["server"]
  "roughly.args": ["server", "--verbose"],
  // Enable experimental features
  "roughly.experimentalFeatures": ["rename", "range_formatting"],
}
```

> [!NOTE]
> For a complete list of experimental features and their descriptions, [see below](#experimental-features).

### Commands

You can access Roughly-specific commands in VS Code via the Command Palette (<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd>):

- **Roughly: Open logs**
- **Roughly: Start/Stop/Restart Server**
- **Roughly: Format workspace** (⚠️ missing)
- **Roughly: Show syntax tree** (⚠️ missing)

## Zed Extension

Install the extension from the [Zed Extension Gallery](https://zed.dev/extensions/roughly).

## RStudio Integration

Roughly can be used as an external formatter in RStudio. See the [RStudio setup guide](https://roughly.felixandreas.me/getting-started/#rstudio-formatter-only) for detailed instructions.

## Configuration

You can configure Roughly via a project-specific `roughly.toml` file:

```toml
[format]
# Number of spaces per indentation level
indent-width = 4
# Automatically detect the appropriate line ending
line-ending = "auto" # "lf" or "cr-lf"

[lint]
# Control the naming convention for variables and parameters
naming-style = "snake_case" # or "camelCase", omit to disable this lint entirely
```

## Experimental Features

Roughly includes several experimental features that can be enabled in the VS Code extension settings or via the CLI:

| Name               | Description                      |
| ------------------ | -------------------------------- |
| `all`              | Enable all experimental features |
| `goto_references`  | Find all references to a symbol  |
| `range_formatting` | Format selected code ranges      |
| `rename`           | Rename symbols                   |
| `unused`           | Warn about unused variables      |

## Development

See our [development documentation](https://roughly.felixandreas.me/development).

## License

This repository is licensed under [The Universal Permissive License Version 1.0](LICENSE).
