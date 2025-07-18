# Roughly

This extension provides support for the [R programming language](https://www.r-project.org/), including workspace symbol search, code formatting, and syntax diagnostics.

> **Note**
> The VS Code extension from the marketplace includes a bundled version of the Roughly CLI **only for Windows and Linux x64**. If you are using macOS or a different architecture, you will need to install the Roughly CLI manually.

## Features

Roughly aims to support the following language server features (some are experimental or in progress):

- **Formatting**
  - Format entire document
  - Format selected code range *(🧪 experimental)*

- **Navigation**
  - Index global variables, S4 and R6 classes/methods
  - Search current document - <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>O</kbd> *in VS Code*
  - Search global workspace - <kbd>Ctrl</kbd> + <kbd>T</kbd> *in VS Code*
  - Go to definition *(🧪 experimental)*
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

## Usage

The extension will automatically start the Roughly language server for R files. You can also use the built-in commands to start, stop, or restart the server, or open logs.

## Configuration

You can customize the Roughly extension in VS Code through the following settings:

```jsonc
{
  // Use a custom binary instead of the bundled one
  "roughly.path": "/path/to/roughly",
  // Pass custom arguments; defaults to ["server"]
  "roughly.args": ["server", "--verbose"],
  // Enable experimental features
  "roughly.experimentalFeatures": ["goto_definition", "range_formatting"],
}
```

## Highlights

### Format Document

![Format Document](https://assets-felixandreas-me.pages.dev/roughly/format.gif)

### Workspace Symbol Search <kbd>Ctrl</kbd> + <kbd>T</kbd>

![Workspace Symbol Search](https://assets-felixandreas-me.pages.dev/roughly/workspace-symbols.gif)

### Document Symbol Search  <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>O</kbd>

![Document Symbol Search](https://assets-felixandreas-me.pages.dev/roughly/document-symbols.gif)

### Syntax Errors

![Syntax Errors](https://assets-felixandreas-me.pages.dev/roughly/syntax-errors.gif)

## Links

* [📦 Source Code](https://github.com/felix-andreas/roughly)
* [📚 Documentation](https://roughly.felixandreas.me/)
