# Human Notes (!AIs are not allowed to edit!)

* can we get rid of tree.rs in roughly?
* source is still (typing(naming) should be something else)
* saved_document_diagnostics is called twice (not needed)
* current_document_diagnostics and saved_document_diagnostics and document_diagnostics is way to complidated. we always want to show all diags (and merge them) we should leave commetn why this the case (see LSP Contraints in 006)

* server.rs did_change doucment sync is too complicated
* currently doesn't support lowering operators like >= (we need to test custom operators)
* invalidate removes outputs (do we want this?)
* we must have a format for builtins (like a .rtypes file or something)
* simplify check function (is it used by server.rs? shouldn't return diags)

* we should somehow pass config into analysis (e.g. if typecheck or not and if strict or not or if debug (show lowering and parsing on hover))

* question: how do we know if we need to re-run package phases based of document ids?? (if the idea is to do this incrementallys)

* we should keep track of what needs to be run
* should global naming test render global ids or tuple (document_id, binding_id)
* add wording inherent complexity and irreducible form to agents.md design bar

* resolve_package is to complicated
  * move format.rs tests to custom fixtures

* should we store a globals table (by interened symbol id)? 
  * we distinghisuh between local binding and global binding?
  * when a file is changed we now all it dependents because can just check which file consumes it, or if a file get's delete we just need to diff globals

I would like to introduce hovering test fixture suite. it should look similar to normal multi-file fixture but has special files: test-case-name.hover another-case.hover

this contain hover position and expecation is hover output. is it easy to implemtn this? let's get the basic test-uite in order (and then we will discuss * should we store a globals table (by interened symbol id)? 
testing matrix)

test suite should be ide/hover &  and test_fixtures ide_hover

* duplicate symbols should use DiagnosticRelatedInformation

* lsp features
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
* migrate roughly to AnalysisState
* scripts behaviour
  * what about type defintions and globals in this file (do they leak) 
  * we cannot just use a fresh analysis state per script as it needs access to analysis state of package.
  
* fixutres:
  * add possiblity to simulate (did_change, did_save, did_close and did_watched_files_change)

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
  
- unify usage of rope helpers in roughly and analysis crate
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


- reasonable fixture split. initially we had this:
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
