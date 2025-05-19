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

![Format Document](https://github.com/user-attachments/assets/44c426d7-ab87-4626-942a-0b9b87a32512)

### Workspace Symbol Search (Ctrl + T)

![Workspace Symbol Search](https://github.com/user-attachments/assets/f948a45f-0762-4c3a-b244-f405cbc7f0d9)

### Document Symbol Search (Ctrl + Shift + O)

![Document Symbol Search](https://github.com/user-attachments/assets/eed98b0d-1e1b-4f6c-83fb-5738e7aa631b)

### Syntax Errors

![Syntax Errors](https://github.com/user-attachments/assets/ffcad99d-941f-4cda-a746-575a28ef4607)

## Documentation

See [github.com/felix-andreas/roughly](https://github.com/felix-andreas/roughly) for more information.
