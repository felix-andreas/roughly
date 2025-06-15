<div align="center">

<img height="128px" src="docs/public/logo.svg" />

# Roughly

[**📚 Docs**](https://roughly.felixandreas.me) | [**📦 Releases**](https://github.com/felix-andreas/roughly/releases) | [**🧩 VS Code Extension**](https://marketplace.visualstudio.com/items?itemName=felix-andreas.roughly)

*An R language server, linter, and code formatter, written in Rust.*

</div>

Roughly can be used either as a standalone command-line tool or as an extension in supported editors like VS Code.

## Features

Roughly aims to support the following language server features (some are experimental or in progress):

- **Symbol Search**
  - Indexing of global symbols, S4 classes/generics/methods and R6 classes/methods
  - Search current document (*VS Code:* <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>O</kbd>)
  - Search global workspace (*VS Code:* <kbd>Ctrl</kbd> + <kbd>T</kbd>)

- **Diagnostics**
  - Syntax errors (including missing or trailing commas)
  - Basic linting rules (e.g. `<-` assignment and variable naming)
  - Unused variables (🧪 experimental)

- **Formatting**
  - Entire documents
  - Selected ranges (🧪 experimental)

- **Code Completion**
  - Local symbols
  - Global symbols
  - Package symbols (⚠️ missing)
  - Signature help (⚠️ missing)

## Roughly CLI

You can install the Roughly CLI by downloading a pre-built binary or by building from source.

### Download Binary (Recommended)

Download the pre-built binary for your platform from the [releases page](https://github.com/felix-andreas/roughly/releases).

### Build from Source

Alternatively, build from source (requires the Rust nightly):

```sh
cargo build --release
```

### Usage

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

Roughly can also be used as a VS Code extension.

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


```jsonc
{
  // Use a custom binary instead of the bundled one
  "roughly.path": "/path/to/roughly",
  // Pass extra arguments to the language server
  "roughly.args": ["server", "--experimental"],
}
```

### Commands

You can access Roughly-specific commands in VS Code via the Command Palette (<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd>):

- **Roughly: Start/Stop/Restart Server**
- **Roughly: Open logs**

## RStudio Integration

Roughly can be used as an external formatter in RStudio. See the [RStudio setup guide](https://roughly.felixandreas.me/getting-started/#rstudio-formatter-only) for detailed instructions.

## Configuration

You can configure roughly via a project-specific `roughly.toml` file:

```toml
case = "snake_case" # or camelCase
spaces = 2
```

## Development

See our [development documentation](https://roughly.felixandreas.me/development).

## License

This repository is licensed under [The Universal Permissive License Version 1.0](LICENSE).
