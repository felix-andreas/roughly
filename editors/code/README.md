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

* Specify a custom binary path to use your own version of Roughly instead of the bundled one:

```json
  "roughly.path": "/path/to/roughly"
```

* Pass additional arguments to the language server, e.g., to enable experimental features:

```json
  "roughly.args": ["server", "--experimental"]
```

## Highlights

### Format Document

![Format Document](https://assets-felixandreas-me.pages.dev/roughly/format.gif)

### Workspace Symbol Search (Ctrl + T)

![Workspace Symbol Search](https://assets-felixandreas-me.pages.dev/roughly/workspace-symbols.gif)

### Document Symbol Search (Ctrl + Shift + O)

![Document Symbol Search](https://assets-felixandreas-me.pages.dev/roughly/document-symbols.gif)

### Syntax Errors

![Syntax Errors](https://assets-felixandreas-me.pages.dev/roughly/syntax-errors.gif)

## Links

* [📦 Source Code](https://github.com/felix-andreas/roughly)
* [📚 Documentation](https://roughly.felixandreas.me/)
