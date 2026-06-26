# Human Notes

(!AIs are not allowed to edit!)

simplify/restrucutre steering documents:
* the workflow has changed. the project was written by ai under human supervision. the human supervision is much lower now. more of the code is written agentic. instead now a ai drives the development
* this must be reflected in the steering documents
* the restructuring of the working documents should make it easier for agents to work
* we should get rid of some of them, because it is micro-managind
* i need you input on how trewrite and restructure them as you see fit.
* their primary gaol is to accumulate a knowledge base. so certain things to have to be rediscovered all the time
* goal is also to spent less time on steering documents and more on implemnetation

my suggestions:
* get rid of discuss.md (if the human wants to give detailed feedback in an document it can be done in .local folder with is not in tree)
* i would like to conslidate DECISION_LOG, TODOS, TECHINCAL_DEBT.md and MEMORY and only keep MEMORY.md. i would probpose to have three sections short, mid and long-term memory. the short-term memory should be cleaned-up very aggressively. mid and long-term memory should be filled with info that is relevant over multiple sessions.
* furthermore i propose the steering doucments to organized by bullets points 
* you don't need to mention 
* currently we have:
  - persistent authoritative documents: durable contract documents
  - working documents: durable engineering-state documents
  - ephemeral documents: short-lived session documents
* basically i would combine working and ephemeral documents into a single MEMORY.md located at .agents/memory/MEMORY.md. you are free to also move the projects to .agents/memory/projects but you can delete them if you think they are useless
* persistent authoritative documents should be moved to official documentation (note that it is still mandadtory to keep them up to date)
  * typing should be its own section in docs
  - `TYPING_SEMANTICS.md` <- this should be hard-core technical reference/specification. furthermore we should have a tutorial (it is partly there)
  these should got into contributing section (this probably more a human overview. contributing is a euphemism here)
  - `ARCHITECTURE.md`
    - `STRUCTURE.md`
    - `TESTING.md`
* goal is also here to make sure docs are in really good shape (we should make this clear in agents.md as well)
* generally speaking i would also recommend to keep steering documents bullet points





## backlog


website:
* better slogan (rust-analyzer: Bringing a great IDE experience to the Rust programming language.)
* make really good landing page
* write user guide

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
* bring docs and readmes update-to-date
* rename. canidates ry, r5y

* naming:
  * in conditionals (should the binding still be resolved correctly?). in current fixture tests they are kept unresolved
  * make conditional test fixture exhaustive
  - (update semantics) maybe later support local types. @new would work only in the same file. also can only be edited in local file (opaque like type)
* type syntax errors
  * should be more local: list {age: intgr} <- this should only underline intgr


* simplify check function (is it used by server.rs? shouldn't return diags)
* debug shouldn't be an experimentall feature but normal config

* incremental analysis
  * we need a test-suite (we can use multi-file and a dedicated save action)

* add wording inherent complexity and irreducible form to agents.md design bar
  
* fixtures:
  * add possiblity to simulate (did_change, did_save, did_close and did_watched_files_change)

- what about pull diagnostics??

- naming:
  - we should pass imported symbols, default namespaces, etc
  - add support for <<- (what are the semantics??)
  - support for local(...) expression
  - better separation betwen phases:
    - resolve_document. resolves all locals and tells which symbols it exposes and which where unresolved (these are potential globals, imports from other packages or builtins)
    - resolve_package. takes all resolved document, parsed NAMESPACE information and builtins and consolidates it. creates diagnostics per document (warns if namespace or builtins are shadowed).

- new test suits:
  - project tests (multiple files)
  - incremtenal test (or maybe call it e2e??) <- change files
  - benchmark tests (static)
  - benchmark tests (incremental)

- combine check and typing (e.g. syntax error checking)
