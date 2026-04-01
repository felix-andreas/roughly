# Human Notes (!AIs are not allowed to edit!)

- migrate roughly to AnalysisState

* rename
* goto defintion
* completion
* defintion
* hover
  * get closest
  * should show
  * lowered hir (also for typing comments)
  * naming
  * type checking
* references
* rename
* document symbol
  * seems like it is executed before didSave. so we must have a fast method here for global sections. (maybe full ast based as it is currently - not part of analysis)
* symbol

- what about pull diagnostics??

- integration into roughly is broken

- testing:
  - am i not a fan of attaching attaching path to diagnostic. we should rather have a tuple of (path/document_id, Vec<Diagnostics>)

- syntax:
  - copy syntax phase from roughly
- naming:
  - we should pass imported symbols, default namespaces, etc
  - should we have two phase: (currently we merge all modules?)
    - per document. resolves all locals (and remaining are assinged as globals)
    - per package. tries to resolve all globals
    - first phase says which globals are exposed by the module and which globals it expects
    - second phase tries to conslidate between multiple files. 
  - add support for <<- (what are the semantics??)
  - support for local(...) expression
  - better separation betwen phases:
    - resolve_document. resolves all locals and tells which symbols it exposes and which where unresolved (these are potential globals, imports from other packages or builtins)
    - resolve_package. takes all resolved document, parsed NAMESPACE information and builtins and consolidates it. creates diagnostics per document (warns if namespace or builtins are shadowed).
- state
  - document_store (after parsing)
  - hir (after lowering)
  - naming (after naming)
  - typecheck (after typecheck)



pub enum AttachedAnnotation {
    Expression {
        annotation: Annotation,
        range: Range,
    },
    BindingAndExpression {
        annotation: Annotation,
        range: Range,
    },
}

- bind_fixture_builtins <- we shouldn't do this in fixtures

- the test_fixtures is in a very bad shape. finish it
- bind_fixture_builtins. 

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


- reasonable fixture split. initially we had this
  - `type_syntax` - typing-comment syntax and normalized type rendering
  - `bindings` - top-level binding result types
  - `diagnostics` - final user-facing errors
  - `environment` - rebinding, shadowing, and scheme reuse across scopes
  - `expressions` - checked expression result types
  - `generalization` - quantified schemes produced at binding boundaries
  - `instantiation` - fresh reuse of generalized bindings at use sites
  - `interfaces` - exported per-file interface shapes
  - `lowering` - syntax-to-HIR lowering output
  - `naming` - binding introduction and use-site resolution
  - `substitution` - propagation of solved types through larger shapes
  - `unification` - solved monotypes during local inference
