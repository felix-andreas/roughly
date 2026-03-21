- scoping rules?
- annotations
  - generics
- lsp
  - inlay hints
  - hovering
- ideas
  - open records?

- keep lowering ids (based on tree-sitter id). only re-lower if tree-sitter changed
  - maybe expression id = file id + tree-sitter id
  
- unify usage of rope helpers in roughly and typing crate
  - shared tree helpers
  - shared rope helpers (maybe hide behind opaque struct)
