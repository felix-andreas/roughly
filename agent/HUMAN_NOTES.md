next goal

make sure that the project is in a respectable state. fyi the workflow has changed. the human supervision is much lower now. this should be refelected in the steering documents and we should get rid of some of them. rewrite and restructure them as you see fit. they are primarly for you know to accumulate a knowledge base. if it isn't working for you remove them. instead new document .local/human-overview/<name>.html which is a document that gives a human an overview about the current state, total architecture, features, non-obvious designs. like a high-level overview of important things in such a way that it helps to get an overvew. finish off the project. make sure the test coverage is perfect and code is in good shape (so that an programmer with OCD is satifisfied). update the project readme to current state and reference typing semantics. (move them to docs so they are visible to users and also writte a guide). if not allready existing write a test that benchmarks a syntetic 10k, 100k and 200k codebase with number (register it as just command). on the way if you discover something you think needs cleanup write it into some list and work until resolved. also resolve things technical debt document or remove things if they are out-dated

# Human Notes (!AIs are not allowed to edit!)

website:
* better slogan (rust-analyzer: Bringing a great IDE experience to the Rust programming language.)
* make really good landing page

* reduce number of steering documents
* ideally typing is not an experimental feature. it should be enabled via config toml 
* come up with strict mode
* we need some kind of soundness audit and document unsound behaviour (goal should be to strive for soudness)
* we need some kind of trait/type classes system (at least internally to support + overloading (s3)), but later we can also expose it
* when unable to infer type we should make this visible on hover instead of showing nothing
* typing errors should only be surfaced if typing is enabled in config
* migrate formatting use custom snapshots framework
* support builtins
  * we must have a format for builtins (like a .rtypes file or something)
* zed extension version shouldn't be coupled to roughly version because it doesn't bundle the language server. instead it should have an independet version number
* research if we need salsa?

* naming:
  * in conditionals (should the binding still be resolved correctly?). in current fixture tests they are kept unresolved
  * make conditional test fixture exhaustive
  - (update semantics) maybe later support local types. @new would work only in the same file. also can only be edited in local file (opaque like type)
* type syntax errors
  * should be more local: list {age: intgr} <- this should only underline intgr

## backlog

* simplify check function (is it used by server.rs? shouldn't return diags)
* debug shouldn't be an experimentall feature but normal config

* incremental analysis
  * we need a test-suite (we can use multi-file and a dedicated save action)

* add wording inherent complexity and irreducible form to agents.md design bar

* resolve_package is to complicated
  * move format.rs tests to custom fixtures

* should we store a globals table (by interened symbol id)? 
  * we distinghisuh between local binding and global binding?
  * when a file is changed we now all it dependents because can just check which file consumes it, or if a file get's delete we just need to diff globals

  
* fixtures:
  * add possiblity to simulate (did_change, did_save, did_close and did_watched_files_change)

- what about pull diagnostics??

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

- combine check and typing (e.g. syntax error checking)

- total phases
  - why is collecting type annotations a separate haset
