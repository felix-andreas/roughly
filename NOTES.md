# Notes

> [!IMPORTANT]
> Maintained by humans. AI Agents are not allowed to edit this file.

bugs:
* when saving roughly.toml: "received did_save for non-open document /home/felix/Projects/typing-demo/roughly.toml"

code quality:
- it still uses child_by_field_name instead by id
- there are some modules with a single file

user experience:
- hovering e.g. lapply should make the package clear
- currenlty hovering lapply shows: fn(x: list[?1], f: fn(?1) -> ?2) -> list[?2]. this isn't very user friendly 
- goto defintion on types in comments should work
- hovering on typing comments should work
- formatting multline type comments looks awkward:
  ```
  #: @type Instrument {list{
  #:         id: integer,
  #:         name: character
  #:     }}
  ```

  Should allow:
  ```
  #: @type Instrument {list{
  #:   id: integer,
  #:   name: character
  #: }}
  ```

  Or
  ```
  #: @type Instrument {
  #:   list{
  #:     id: integer,
  #:     name: character
  #:   }
  #:}
  ```
  And it should use indent size of formatter. the typing formatter needs much better coverage

stubs:
- is analysis/stubs the right place or should we save it top level?
- do we have a script to verify the stubs are exhaustive?
- not every stub should be Any

typing:
- support for local(...) expression
- `identity <- function(x) x`. should it be infered as "fn(a: ?1) -> ?1". is there a reason to not show generic here?
- when unable to infer type we should make this visible on hover instead of showing nothing

naming:
- in conditionals (should the binding still be resolved correctly?). in current fixture tests they are kept unresolved
- make conditional test fixture exhaustive
- (update semantics) maybe later support local types. @new would work only in the same file. also can only be edited in local file (opaque like type)

type syntax errors
- should be more local: list {age: intgr} <- this should only underline integer

- debug shouldn't be an experimental feature but normal config (check how is configured)
  
fixtures:
- fixtures: add possiblity to simulate (did_change, did_save, did_close and did_watched_files_change)

## Needs refinement

- is it worth to drop tree-sitter and write hand-rolled recursice decent parser?

- website: better slogan (rust-analyzer: Bringing a great IDE experience to the Rust programming language.)
- make really good landing page
  - find old commit where animation looked good
- website: improve user guide
- website: animation looked good on 673b0c44bf101a6f74b4e0feef73a1c3a8111246. it looked nice how the roughly logo was formed on inital load. also looked cool how it morphed into types. only difference should be that once you scroll down. the particles morph into the heading (there shouldn't be actual text on top only particles that form the heading)

general:
- verify if crate/folder structure is reasonable

- rename the project. first decide for a name. canidates: ry, r5y
- make it easier to install

- we need some kind of trait/type classes system (at least internally to support +-operator for multiple types), but later we can also expose it
- verify how incremental analysis works and if we have tests to prove it

- `add <- function(a, b) a + b`. should we have a constraint here `fn(a: T, b: T) -> T where T : <constraint>`. this would require some sort of trait system. (look at roc how they solve it)
- we need some kind of soundness audit and document unsound behaviour (goal should be to strive for soudness)

- how does our approach compare to salsa?
- bring docs and readmes update-to-date
- what about pull diagnostics??

- verify how good NAMESPACE and imports work
- verify/add support for <<- (what are the semantics??)

testing:
- verify how good e2e test is
- verify benchmark
  - cold
  - hot (incremental)
