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

### VS Code extension

Bundle the client (or [download from here](https://github.com/felix-andreas/roughly/releases)):

```
bun run package
```

Install the VS code extension:

```
code --install-extension roughly.vsix
```

### Server

Build the server (or [download from here](https://github.com/felix-andreas/roughly/releases)):

```
cargo build --release
```

Configure the client via the `settings.json` to use the server binary:

```json
{
  "roughly.path": "<path>"
}
```

## Usage

Start the language server:

```
roughly lsp
```

To run roughly as a formatter:

```
roughly fmt                # Format all files in the current directory
roughly fmt <path>         # Format all files in `<path>`
roughly fmt --check        # Only check if files would be formatted
roughly fmt --diff         # Only show diff if files would be formatted
```

Or, to run Roughly as a linter:

```
roughly check               # Check all files in the current directory
roughly check <path>        # Check all files in `<path>`
```

## Configuration

You can configure roughly via a project-specific `roughly.toml` file:

```toml
case = "snake_case" # or camelCase
spaces = 2
```

## Documentation

For comprehensive documentation, visit [roughly.felixandreas.me](https://roughly.felixandreas.me).

## Features

* Completion
  * Globals
  * (WIP) Locals
* Formatting
* Diagnostics
  * Syntax
  * Missing commans, Trailing commas, 
  * Assignments, casing
* Indexing
  * Globals
  * S4
    * Classes
    * Generics
    * Methods
  * (TODO) R6
* Goto Document Symbol <kbd>Ctrl</kbd> <kbd>Shift</kbd> + <kbd>O</kbd>
* Goto Workspace Symbol <kbd>Ctrl</kbd> + <kbd>T</kbd>
* VS Code Extension
  * Commands
    * Start/Stop/Restart the Language Server
    * Open logs

## Project layout

Currently this extension assumes that your `R` code has the following folder structure:

| Path        | Type      |
|-------------|-----------|
| `R`         | directory |
| `R/*.R`     | file      |
| `NAMESPACE` | file      |


## Development

See our [development documentation](https://roughly.felixandreas.me/development).

## License

This repository is licensed under the [GNU General Public License v3.0](LICENSE).
