---
title: Development
description: How to contribute to Roughly
---

## VS Code Extension Setup

- Run `cd editors/code && bun install`. This installs all necessary npm modules
- Press Ctrl+Shift+B in VS Code to start compiling the client in [watch mode](https://code.visualstudio.com/docs/editor/tasks#:~:text=The%20first%20entry%20executes,the%20HelloWorld.js%20file.).
- Switch to the Run and Debug View in the Sidebar (Ctrl+Shift+D).
- Select `Launch Client` from the drop down (if it is not already).
- Press ▷ to run the launch config (F5).
- In the [Extension Development Host](https://code.visualstudio.com/api/get-started/your-first-extension#:~:text=Then%2C%20inside%20the%20editor%2C%20press%20F5.%20This%20will%20compile%20and%20run%20the%20extension%20in%20a%20new%20Extension%20Development%20Host%20window.) instance of VSCode, open a document with a `.R` extension.

### Words of warning

The `launch.json` contains a setting:

```json
"autoAttachChildProcesses": true,
```

For me this led to the issue that the language server wasn't spawned because I had `CodeLLDB` from `nixpkgs` installed.


## Roadmap

* static analysis
  * type checking
  * name binding
    * are all names defined
    * unused names (variables & parameters)
* variable renaming
* inlay hints

## Formatting

### Comments in expressions

The main challenge in formatting code is handling comments because they can appear at any location. This is particualar hard to handle these expressions:

* if expression
* for expression
* repeat statment
* while expression
* binary operator
* extract operator

#### Example of if expressions

For example, in an if expression comments can appear at any location (numerated):

```r
if
# 1
( # open
  # 2
  a && b # condition
  # 3
) # close
# 4
{
  y
}
# 5
else
# 6
{
  4
}
```

With a structured AST, we might typically could write our code in a concise functional style:

```rs
let condition = fmt(field("condition")?);
let consequence = fmt(field("consequence")?);
let alternative = fmt(field("alternative")?);
format!("if({condition}) {{ {consequence} }} else {{ {alternative} }}");
```

However, due to the arbitrary placement of comments, we must adopt a more imperative style. This approach preserves comments but somewhat harder to comprehend:

```rs
let mut out = String::new();
let mut cursor = node.walk();
if cursor.goto_first_child() {
  loop {
    match cursor.field_name() {
      None => match child.kind() {
        "if" => out.push_str("if"),
        "else" => ...,
        "comment" => ...,
        _ => unreachable!(),
      },
      Some(field_name) => match field_name {
        "open" => ...,
        "condition" => ...,
        "close" => ...,
        "consequence" | "alternative" => ...,
        _ => unreachable!(),
      },
    };

    if !cursor.goto_next_sibling() {
        cursor.goto_parent();
        break;
    }
  }
};
out
```

## Tree-sitter: How to handle required fields

* For formatting we do an initial check if there are no errors, so we can safely assume that all required fields are present
* For type-checking we should do the same
* For checks/diagnostics that run while typing (syntax & fast), we cannot make any assumption
* Same is true for index. It should still be possible to index a file, while there are parse errors

## tree-sitter-r vs R parser

### No default for parameter

Accept by `tree-sitter-r` but rejected by R.

```R
function(parameter =) {}
```

See https://github.com/r-lib/tree-sitter-r/issues/161

### line break after `else`

This is valid R but cannot be parsed by `tree-sitter-r`

```R
if (TRUE) {
  1
} else
{
  2
}
```

### Two line breaks after `extract_operator`


This is valid R but cannot be parsed by `tree-sitter-r`

```R
foo$

bar
```

See https://github.com/r-lib/tree-sitter-r/issues/166

## Linting ideas

* Empty loops: for, while, repeat
* unused variables

## Type checking

* https://github.com/salsa-rs/salsa/blob/master/examples/calc/type_check.rs


## References

* R
  * https://github.com/wch/r-source/blob/trunk/src/main/gram.y
  * https://cran.r-project.org/doc/manuals/r-release/R-lang.html
  * https://www.reddit.com/r/rust/comments/uu47mk/comment/i9dn0yg/
* vs code
  * https://github.com/microsoft/vscode-extension-samples/tree/main/lsp-sample
  * https://github.com/semanticart/lsp-from-scratch
  * https://github.com/nix-community/vscode-nix-ide
  * https://github.com/ziglang/vscode-zig/
* tower-lsp
  * https://github.com/Desdaemon/-lsp/
  * https://github.com/FuelLabs/sway
  * https://github.com/IWANABETHATGUY/tower-lsp-boilerplate
  * https://github.com/TenStrings/glicol-lsp/blob/77e97d9c687dc5d66871ad5ec91b6f049de2b8e8/src/main.rs#L16
  * https://github.com/jfecher/ante/blob/5f7446375bc1c6c94b44a44bfb89777c1437aaf5/ante-ls/src/main.rs#L163
  * https://github.com/supabase-community/postgres-language-serve
* lsp-server
  * https://github.com/gleam-lang/gleam/tree/main/compiler-core/src/language_server

### Formatting

* https://homepages.inf.ed.ac.uk/wadler/papers/prettier/prettier.pdf

### Type Checker

* https://github.com/Glyphack/enderpy

### Out of order issue

* https://github.com/ebkalderon/tower-lsp/issues/284
* https://github.com/ethereum/fe/pull/1022
* https://github.com/oxalica/async-lsp
* https://github.com/tower-lsp-community/tower-lsp-server/issues/36
