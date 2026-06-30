# Notes

> [!IMPORTANT]
> Maintained by humans. AI Agents are not allowed to edit this file.

general:
- rename the project. first decide for a name. canidates: ry, r5y
- make it easier to install
- verify if crate/folder structure is reasonable

developer tooling:
- restructure justfile to infer kind from alpha or beta-postfix

research:
- how does our approach compare to salsa?
- bring docs and readmes update-to-date

stubs:
- is analysis/stubs the right place?
- do we have a script to verify the stubs are exhaustive?
- we should different format (maybe .rtypes)
- not every stub should be Any

typing:
- verify how good NAMESPACE and imports work
- verify/add support for <<- (what are the semantics??)
- support for local(...) expression
- `identity <- function(x) x`. should it be infered as "fn(a: ?1) -> ?1". is there a reason to not show generic here?
- `add <- function(a, b) a + b`. should we have a constraint here `fn(a: T, b: T) -> T where T : <constraint>`. this would require some sort of trait system. (look at roc how they solve it)
- we need some kind of soundness audit and document unsound behaviour (goal should be to strive for soudness)
- we need some kind of trait/type classes system (at least internally to support + overloading (s3)), but later we can also expose it
- when unable to infer type we should make this visible on hover instead of showing nothing
- verify how incremental analysis works and if we have tests to prove it
- zed extension version shouldn't be coupled to roughly version because it doesn't bundle the language server. instead it should have an independet version number

naming:
- in conditionals (should the binding still be resolved correctly?). in current fixture tests they are kept unresolved
- make conditional test fixture exhaustive
- (update semantics) maybe later support local types. @new would work only in the same file. also can only be edited in local file (opaque like type)

website:
- better slogan (rust-analyzer: Bringing a great IDE experience to the Rust programming language.)
- make really good landing page
- write user guide


type syntax errors
- should be more local: list {age: intgr} <- this should only underline intgr

config:
- debug shouldn't be an experimentall feature but normal config

agents md
- add wording inherent complexity and irreducible form to agents.md design bar
- general cleanup
  
- fixtures:
- add possiblity to simulate (did_change, did_save, did_close and did_watched_files_change)

- what about pull diagnostics??

testing:
- verify how good e2e test is
- verify benchmark
  - cold
  - hot (incremental)
