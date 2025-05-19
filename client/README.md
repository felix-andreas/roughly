# Roughly

This extension provides support for the [R programming language](https://www.r-project.org/), including workspace symbol search, code formatting, and syntax diagnostics.

> **Note**
> The VS Code extension from the marketplace already includes a bundled version of the Roughly CLI, so you don't need to install the CLI separately.

## Usage

The extension will automatically start the Roughly language server for R files. You can also use the built-in commands to start, stop, or restart the server, or open logs.

## Configuration

You can customize the Roughly extension in VS Code through the following settings:

* Specify a custom binary path to use your own version of Roughly instead of the bundled one:

```json
  "roughly.path": "/path/to/roughly"
```

* Pass additional arguments to the language server, e.g to enable experimental features:

```json
  "roughly.args": ["lsp", "--experimental"]
```

## Highlights

### Format Document

![Format Document](https://github.com/user-attachments/assets/a03334a5-ed83-4f30-a4ea-7cbd615e4fdd)

### Workspace Symbol Search (Ctrl + T)

![Workspace Symbol Search](https://github.com/user-attachments/assets/e4c7cf42-d5fa-44b9-900b-5c7758f5f7e3)

### Document Symbol Search (Ctrl + Shift + O)

![Document Symbol Search](https://github.com/user-attachments/assets/0c608b3d-2eed-4372-b2d4-783ff67c6c0d)

### Syntax Errors

![Syntax Errors](https://github.com/user-attachments/assets/ef93a688-fc33-46dd-8cbb-7f3bab6948a7)

## Documentation

See [github.com/felix-andreas/roughly](https://github.com/felix-andreas/roughly) for more information.
