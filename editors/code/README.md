# Roughly

This extension provides support for the [R programming language](https://www.r-project.org/), including workspace symbol search, code formatting, and syntax diagnostics.

> **Note**
> The VS Code extension from the marketplace includes a bundled version of the Roughly CLI **only for Windows and Linux x64**. If you are using macOS or a different architecture, you will need to install the Roughly CLI manually.

## Features

* Autocomplete
* Code Formatting
* Syntax Diagnostics
* Workspace Symbol Search

## Usage

The extension will automatically start the Roughly language server for R files. You can also use the built-in commands to start, stop, or restart the server, or open logs.

## Configuration

You can customize the Roughly extension in VS Code through the following settings:

```jsonc
{
  // Use a custom binary instead of the bundled one
  "roughly.path": "/path/to/roughly",
  // Pass extra arguments to the language server
  "roughly.args": ["server", "--extra", "arg"],
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
