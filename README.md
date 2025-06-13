<div align="center">

<img height="128px" src="docs/public/logo.svg" />

# Roughly

*The R(oughly good enough) Language Server*

[**📚 Docs**](https://roughly.felixandreas.me) | [**📦 Releases**](https://github.com/felix-andreas/roughly/releases) | [**🧩 VS Code Extension**](https://marketplace.visualstudio.com/items?itemName=felix-andreas.roughly)

</div>

Roughly is an R language server, linter, and code formatter, written in Rust.

> [!WARNING]  
> This project is a work in progress. Contributions and feedback are welcome!

## Installation

### Download Binary (Recommended)

Download the pre-built binary for your platform from the [releases page](https://github.com/felix-andreas/roughly/releases).

### Build from Source

Alternatively, build from source (requires the Rust nightly):

```sh
cargo build --release
```

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
roughly server          # Usually started automatically by your editor
```

## VS Code extension

### From Marketplace (Recommended)

Install directly from VS Code:
- Open VS Code
- Press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>X</kbd> to open Extensions
- Search for "roughly"
- Click "Install" on the extension by `felix-andreas`

Or, install using the [VS Code Marketplace website](https://marketplace.visualstudio.com/items?itemName=felix-andreas.roughly).

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

* Specify a custom binary path to use your own version of Roughly instead of the bundled one:

```json
  "roughly.path": "/path/to/roughly"
```

* Pass additional arguments to the language server, e.g to enable experimental features:

```json
  "roughly.args": ["server", "--experimental"]
```

## RStudio Integration

Roughly can be used as an external formatter in RStudio. See the [RStudio setup guide](https://roughly.felixandreas.me/getting-started/#rstudio-formatter-only) for detailed instructions.

You can configure Roughly as an [external formatter in RStudio](https://roughly.felixandreas.me/getting-started/#rstudio-formatter-only)


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

This repository is licensed under [The Universal Permissive License Version 1.0](LICENSE).
