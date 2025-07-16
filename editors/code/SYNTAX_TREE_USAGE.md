# VSCode Extension: Show Syntax Tree

This extension adds a command `Roughly: Show Syntax Tree` that displays the AST (Abstract Syntax Tree) of the current R file.

## Usage

1. Open an R file in VSCode
2. Use the command palette (Ctrl+Shift+P / Cmd+Shift+P)
3. Type "Roughly: Show Syntax Tree"
4. The AST will open in a new editor tab beside your current file

## Example

For an R file with content:
```r
# Test function
hello <- function(name) {
  paste("Hello", name)
}

result <- hello("World")
```

The command will show the AST structure with details about:
- Program structure
- Function definitions
- Variables assignments
- Function calls
- Comments
- And more...

## Requirements

- The `roughly` binary must be available (installed via the extension or in PATH)
- The current file must be an R file (`.R` or `.r` extension)
- The file must be saved to disk

## Error Handling

- Shows error if no active editor is found
- Shows error if current file is not an R file
- Shows error if file is not saved
- Shows error if roughly binary fails to execute