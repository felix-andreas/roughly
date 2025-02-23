<div align="center">

# Roughly 🔥

### The R(oughly good enough) Language Server

</div>

Welcome to Roughly, the language server where we skip the fancy AST parsing and go straight to the syntax (just regex), because who has time for that when your R project is already making your computer cry?

## Installation

### Client

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

## Features

* Completion
  * Globals
  * (TODO) Locals
* Formatting
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

## Project layout

Currenlty this extension assume that your `R` code has the following folder structure:

| Path        | Type      |
|-------------|-----------|
| `R`         | directory |
| `R/*.R`     | file      |
| `NAMESPACE` | file      |


## Development

- Run `bun install` in this folder. This installs all necessary npm modules in the client
- Open VS Code on this folder.
- Press Ctrl+Shift+B to start compiling the client in [watch mode](https://code.visualstudio.com/docs/editor/tasks#:~:text=The%20first%20entry%20executes,the%20HelloWorld.js%20file.).
- Switch to the Run and Debug View in the Sidebar (Ctrl+Shift+D).
- Select `Launch Client` from the drop down (if it is not already).
- Press ▷ to run the launch config (F5).
- In the [Extension Development Host](https://code.visualstudio.com/api/get-started/your-first-extension#:~:text=Then%2C%20inside%20the%20editor%2C%20press%20F5.%20This%20will%20compile%20and%20run%20the%20extension%20in%20a%20new%20Extension%20Development%20Host%20window.) instance of VSCode, open a document with a `.R` extension.
  - Press <kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>O</kbd> to search document symbols
  - Press <kbd>Ctrl</kbd> <kbd>T</kbd> to search workspace symbols

### Words of warning

The `launch.json` contains a setting:

```json
"autoAttachChildProcesses": true,
```

For me this led to the issue that the language server wasn't spawned because I had `CodeLLDB` from `nixpkgs` installed.


## References

* https://github.com/microsoft/vscode-extension-samples/tree/main/lsp-sample
* https://github.com/semanticart/lsp-from-scratch
* https://github.com/IWANABETHATGUY/tower-lsp-boilerplate
* https://www.reddit.com/r/rust/comments/uu47mk/comment/i9dn0yg/
* https://github.com/FuelLabs/sway
* https://github.com/gleam-lang/gleam/tree/main/compiler-core/src/language_server
* https://github.com/jfecher/ante/blob/5f7446375bc1c6c94b44a44bfb89777c1437aaf5/ante-ls/src/main.rs#L163
* https://github.com/ziglang/vscode-zig/
* https://github.com/nix-community/vscode-nix-ide
* https://github.com/wch/r-source/blob/trunk/src/main/gram.y
* https://cran.r-project.org/doc/manuals/r-release/R-lang.html
* https://github.com/TenStrings/glicol-lsp/blob/77e97d9c687dc5d66871ad5ec91b6f049de2b8e8/src/main.rs#L16

## License

[GNU General Public License v3.0](LICENSE)
