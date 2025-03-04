---
title: Development
description: How to contribute to Roughly
---

## Client Setup

- Run `cd client && bun install`. This installs all necessary npm modules in the client
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

