# Human Notes (!AIs are not allowed to edit!)

- get rid of old test_infer.

- verify fixtures test suite. can we consoldate different fixture suites?

- scoping rules? (canonicalization in genreal)
- lsp
  - inlay hints
  - hovering
- ideas
  - open records?

- combine check and typing (e.g. syntax error checking)

- total phases
  - why is collecting type annotations a separate haset

- write down exact phases (e.g.)
  - synatx checking
  - lowering (does it include canonilization)
  - inference
  - unifications

- lowering
  - cache for expressions
    - keep lowering ids (based on tree-sitter id). only re-lower if tree-sitter changed
    - maybe expression id = file id + tree-sitter id
  - proper error handling instead of 
  - use kind_id & field_id instead of kind and name
  - don't use node_text for has_trailing_semicolon.
  - must lowering support all syntax??
  - do we need lowering?
  
- unify usage of rope helpers in roughly and typing crate
  - shared tree helpers
  - shared rope helpers (maybe hide behind opaque struct)
  
- do we need lowering?
  - can't we just have a table for annotations (key is tree-sitter id)
  - can't even typing check not use tree-sitter directly ??
  
  
- free variables in fixtures:

I have a question in the expression/functions fixture test suite we have:

#---- identity_function
identity <- function(x) x
#++++
fn(type1) -> type1


but this should probably rather be <T> fn(T) -> T. shouldn't it?

also if not. shouldn't we render free variables differently in the fixutre ? what syntax whould you recommedn?

- questions?
  - do we need canonicalization? (is it part of lowering)
  - when hovering: how do we go from tree-sitter id to type info?
    - probably use our own ranges for now. tree-sitter id does not map 1:1 to nodes.
  - do we need separate lowering tests
  - do we need scoping tests?
  - is infer the correct name (wound't typecheck by more appropriate?)
  - how to make type checking incremental: how to handle case if non-opend file depends on type in another file. no only other file changes. how does the type in the other file gets updated?
