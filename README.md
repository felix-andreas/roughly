<div align="center">

<img height="128px" src="docs/public/logo.svg" />

# Roughly

*The R(oughly good enough) Language Server*

[**📚 Docs**](https://roughly.felixandreas.me) | [**📦 Releases**](https://github.com/felix-andreas/roughly/releases)

</div>

Roughly is an R language server, linter, and code formatter, written in Rust.

> [!WARNING]  
> This project is a work in progress. Contributions and feedback are welcome!

## Installation

### Roughly CLI

Build the CLI (or [download from here](https://github.com/felix-andreas/roughly/releases)):

```
cargo build --release
```

### VS Code extension

Bundle the client (or [download from here](https://github.com/felix-andreas/roughly/releases)):

```
bun run package
```

Install the VS code extension:

```
code --install-extension roughly.vsix
```

Configure the VS Code extension via the `settings.json` to use the roughly binary:

```json
{
  "roughly.path": "<path>"
}
```

### RStudio (formatter only)

You can configure Roughly as an [external formatter in RStudio](https://roughly.felixandreas.me/getting-started/#rstudio-formatter-only)

## Usage

Run roughly as a formatter:

```
roughly fmt             # Format all files in the current directory
roughly fmt <path>      # Format all files in `<path>`
roughly fmt --check     # Only check if files would be formatted
roughly fmt --diff      # Only show diff if files would be formatted
```

To run Roughly as a linter:

```
roughly check           # Check all files in the current directory
roughly check <path>    # Check all files in `<path>`
```

Or, to run Roughly as a language server:

```
roughly lsp             # Usually started automatically by your editor
```

## Configuration

You can configure roughly via a project-specific `roughly.toml` file:

```toml
case = "snake_case" # or camelCase
spaces = 2
```

## Features

* Completion
  * Globals
  * (WIP) Locals
* Formatting
* Diagnostics
  * Syntax
  * Missing and trailing commas
  * Assignments, casing
* Indexing
  * Globals
  * S4
    * Classes
    * Generics
    * Methods
  * (TODO) R6
* Goto Document Symbol (VS Code shortcut <kbd>Ctrl</kbd> <kbd>Shift</kbd> + <kbd>O</kbd>)
* Goto Workspace Symbol (VS Code shortcut  <kbd>Ctrl</kbd> + <kbd>T</kbd>)
* VS Code Extension
  * Commands (<kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>P</kbd>)
    * Start/Stop/Restart the Language Server
    * Open logs

## Development

See our [development documentation](https://roughly.felixandreas.me/development).

## License

This repository is licensed under the [GNU General Public License v3.0](LICENSE).
