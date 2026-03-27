# Human Notes (!AIs are not allowed to edit!)

- we should resolve types during lowering

- (update semantics) maybe later support local types. @new would work only in the same file. also can only be edited in local file (opaque like type)

- better document split: (https://chatgpt.com/c/69c5985f-f050-8332-bf70-4834bc44b3c4)
  - Overview / Goals
  - Requirements
    - Functional
    - Non-functional
  - Behavior
    - Use cases or flows
  - Architecture
  - Interfaces
  - Constraints
  - Testing / Acceptance

- errors of earlier phases should be shown first.
    atm we have this bug:
    
    A `#:` typing comment must be followed immediately by an expression. (typing syntax-error)
    
    for an expression like this:
    
    [@typing.R (18:19)](file:///home/felix/Projects/roughly/R/typing.R#L18:19) 
    
    we shouldn't errors of the previous phase first. i think the problem here is that we first


- naming:
  - add support for <<- (what are the semantics/
  - support for local

- hir only supports attaching annotations to assignment

- get rid of old test_infer.

- verify fixtures test suite. can we consoldate different fixture suites?

- new test suits:
  - project tests (multiple files)
  - incremtenal test (or maybe call it e2e??) <- change files
  - benchmark tests (static)
  - benchmark tests (incremental)

- separate module for parsing
  - contains rope
  - contains tree-siter
  - contains incremental update -> this can be used for benchmark tests

- separate tools module to generatively create large R code bases

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
