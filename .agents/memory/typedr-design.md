# TypedR — a typed dialect of R that compiles to R

Status: **proposal, and the recommendation is currently NOT to build the package
half of it** (§3). Nothing is implemented. This document exists so that whoever
picks it up starts from a reasoned design instead of re-deriving one, so the costs
are visible before the work starts rather than after, and so the existing attempt
at the same idea (§13) is read before the second one begins.

Read it in order. Each section earns the next one.

## 1. The problem this is meant to solve

R gives you one compound data structure that people use for everything: the
list. In practice a list plays five different roles.

```r
person  <- list(name = "Ada", age = 36)          # a record: fields by name
pair    <- list("celsius", 21.5)                 # a tuple: meaning by position
options <- list()                                # a map: keys known at run time
options[[key]] <- value
circle  <- structure(list(r = 2), class = "Circle")   # an object of some kind
shape   <- if (round) circle else rectangle           # one of several kinds
```

The checker handles the first three well: they are shapes, and shapes are what a
type system is for. The last two are where it stops, and the reason is
structural rather than a missing feature. **In R, "what kind of thing is this"
is a string in an attribute.** `structure(list(r = 2), class = "Circle")` says
"Circle" the same way a comment says it: to a reader, not to a tool. The
matching read is `inherits(x, "Circle")` — another string, unconnected to the
first by anything a checker can follow.

This is not a corner case. In an adoption review where a package author turned
on the strictest checking, **31 of their 34 findings traced to a single call
shape**: `structure(list(...), class = ...)`. That is the idiom real R packages
are built out of, and it is opaque by construction.

The consequence that matters is not the missing type. It is the missing
**exhaustiveness**. When a value is one of several kinds, R's idiom for handling
it is a chain:

```r
area <- function(shape) {
  if (inherits(shape, "Circle")) {
    pi * shape$r^2
  } else if (inherits(shape, "Rectangle")) {
    shape$w * shape$h
  }
}
```

Add a third kind later and this function silently returns `NULL` for it. `NULL`
then flows onward and fails somewhere else entirely — the exact failure mode the
whole tool exists to catch. Nothing in R, and nothing a comment can add, will
tell you that this function forgot a case.

So the gap is: **R has no way to say "this value is one of these N shapes" that
a reader or a checker can verify, and no way to be told when a use of it is
incomplete.**

## 2. Why comment annotations reach a ceiling here

The `#:` comment annotations are the right answer for types, and this proposal
does not deprecate or replace them. They cost nothing: every other R tool sees a
comment, the file stays ordinary R, and there is no build step. For declaring
what a function takes and returns, they are already the whole job.

They have also already been pushed at *this* problem. `@type` and `@new` let you
declare a named type and mint values of it, and it works — the same reviewer who
lost 31 findings to `structure()` got from 34 findings down to 9 with four lines
of annotation. But note what they had to do: hand-declare a type describing a
`list()` the checker was looking straight at. The annotation restates what the
code already said.

The harder half does not fit at all, and it is worth being precise about why.
Exhaustive case analysis needs three things: a place to *declare* the set of
kinds, a place to *construct* one and have the construction checked, and a
place to *use* one where the checker can see every case and complain about the
missing one. The first two can be expressed in comments. The third cannot,
because there is no R construct to attach it to. The closest thing R has is
`switch(class(x)[1], ...)`, and that is an ordinary function call. To make it
checkable you would have to teach the checker that *this particular call to this
particular function* is a case analysis — that is, to bless a magic call shape.

That is a real option, not a dead end. It is the fork in §3.

## 3. The fork to settle first: a blessed library, or a dialect

There are two honest routes to exhaustive case analysis in R, and they lead to
different products. **This has to be decided before any implementation starts,
because the answer decides whether the rest of this document gets built.**

**Route A — a library the checker knows specially.** Ship an R package with tag
constructors and a `match` function. Files stay ordinary `.R`. The checker
recognises calls to those specific functions and checks exhaustiveness on them.

- Keeps every advantage the tool currently sells: your code is R, every other
  tool works, no build step, nothing to install for a collaborator.
- Costs: the checker gains special knowledge of particular function names, which
  is a category of complexity the design bar is normally hostile to. Construction
  is a function call, so it can be checked but not *restricted* — nothing stops
  someone building the underlying list by hand and bypassing the contract. And
  the notation is whatever R's call syntax allows, which for pattern matching is
  clumsy: named arguments standing in for patterns, with no place to bind a
  variant's fields.

**Route B — a dialect with real syntax** (the rest of this document). A separate
source language, compiled to plain R.

- Gets real declaration, construction and `match` syntax, and can restrict
  construction because the constructor is syntax rather than a callable
  function.
- Costs a build step and an ecosystem split. §5 states this in full; it is the
  serious objection.

**A recorded direction points at Route A.** `typing-design.md` §1 carries an
earlier instruction: provide tags "through a stdlib the checker knows specially,
not through new R syntax — annotated R stays ordinary R". This document is Route
B and therefore contradicts it. That is not an oversight to route around: the
two routes want the same capability, and building both would be waste. Settle
the fork explicitly, record the answer in `decisions.md`, and delete the losing
half.

**Route A's stated cost is avoidable, which changes the balance.** The objection
above — that the checker would gain hardcoded knowledge of particular function
names — assumes the knowledge lives in the checker. It does not have to. The
corpus already carries exactly this kind of fact as a declaration attribute:
`@masked` marks a function whose rest arguments evaluate inside a data frame, and
the checker reads that from the stub rather than knowing dplyr's verb names. Case
analysis can ride the same mechanism:

```
match : @exhaustive fn(x: Any, ...: Any) -> Any
```

The attribute says "the named arguments of this call are cases over the first
argument's kinds; report any kind no argument covers". The checker gains one
general rule, the *names* stay data, a project can declare its own equivalent for
a hand-rolled dispatcher, and the whole thing is overridable like every other
stub. That is the same shape as every other special form already supported, and it
passes the design bar that the original objection said it would fail.

What Route A still cannot do is *restrict* construction — nothing stops someone
building the underlying list by hand and skipping the constructor. Weigh that
honestly: this is a gradual checker with `Any` as a sanctioned escape hatch and
`Unknown` wherever a construct is unmodelled. It tolerates far larger holes than
that one, deliberately. The unrestrictable constructor is a real cost and a small
one.

**Recommendation: take Route A for the capability, and do not build a package
dialect.** The reasoning is in §5 and in the prior art (§14): the tool's
distinguishing property is that it works on R nobody has modified, and a dialect
spends exactly that to buy notation. Route A keeps it and gets the part that
matters — being told when a case is missing.

That leaves one place where a dialect costs almost nothing, and it is worth
keeping alive: **standalone scripts run through the REPL** (§9), where there is no
generated file, no packaging, and no collaborator who needs a toolchain. If a
dialect ships at all, that is where it should ship first — and it is the corner
the existing prior art does not occupy.

The rest of this document specifies Route B in full anyway, for two reasons: the
fork should be decided between two designs rather than between a design and a
sketch, and the script-only dialect above is Route B minus its packaging half, so
the specification is what it would be built from.

## 4. What the dialect adds

Three layers. They are ordered so that each is useful alone and the later ones
need the earlier ones.

### Layer 1 — types where the code is

```
half <- function(x: integer, scale: double = 1) -> double {
  x / (2 * scale)
}
count: integer <- 0L
```

Same type notation as the comment form — same names, same generics
(`<T: numeric>`), same shapes. Nothing new is expressible; this is the comment
annotation moved into the code, which removes the attachment rules (which
comment belongs to which statement) and the duplication of parameter names.

Be honest about the value: **this layer is ergonomics, not capability.** On its
own it does not justify a dialect. It is worth building first because it is the
cheapest way to prove the machinery — the source folder, the compiler, the
editor integration, the position mapping — with almost no new meaning to get
wrong.

### Layer 2 — naming shapes

```
type Point = {x: double, y: double}

origin <- Point{x = 0, y = 0}
pair   <- #(1L, "one")
```

`type Name = {...}` names a record shape; `Point{...}` constructs one and the
construction is checked against the declaration, so a misspelled or missing
field is caught where the mistake is rather than at some later read. Tuples get
a constructor for the positional shape. (`#(...)` is a placeholder spelling —
see §13.)

Both compile to `list(...)`. They are names for shapes the type system already
has, and the typing reference already anticipated exactly this: it notes that
distinct record and tuple constructors may arrive later "even if they remain
runtime aliases of R lists".

### Layer 3 — kinds, and being told when you miss one

This is the reason to do any of it.

```
type Shape =
  | Circle(radius: double)
  | Rectangle(width: double, height: double)

area <- function(shape: Shape) -> double {
  match (shape) {
    Circle(radius)          -> pi * radius^2
    Rectangle(width, height) -> width * height
  }
}
```

Add a `Triangle` variant and `area` becomes an error that names the case it does
not handle. A `_ ->` arm opts out deliberately. Each arm sees the variant's
fields as ordinary bound names, so there is no `shape$radius` on a value that
might not have one.

**Kinds are nominal, not structural.** A variant belongs to the union it was
declared in, rather than any record with a `radius` field being a `Circle`. Two
reasons. It maps onto machinery that exists — a variant is a named type, the sum
is a union of those names, per-arm narrowing is the flow narrowing already
implemented, exhaustiveness is union coverage. And the structural alternative
needs types that talk about "any record with at least these fields", which is a
harder inference problem with a well-earned reputation for unreadable error
messages. Diagnostics quality is a stated goal; this is where that goal cashes
out as a design constraint. Nothing prevents revisiting if nominal kinds prove
too rigid in practice.

**The compiled form is ordinary S3**, and this is load-bearing rather than
incidental:

```r
Circle <- function(radius) structure(list(radius = radius), class = c("Circle", "Shape"))
```

A plain-R consumer can `inherits(x, "Shape")`, write S3 methods on variants, and
print them — so a typed data structure is still a first-class citizen of the
host ecosystem. That property is what makes it possible to adopt the dialect in
one file of an existing package instead of all of it.

## 5. What it costs

The current pitch is: *your code stays ordinary R, and every other R tool keeps
working*. A dialect inverts that, and pretending otherwise would make this
document useless.

A `.Rt` file is not R. Until it is compiled, `roxygen2` does not document it,
`devtools::load_all()` does not load it, RStudio does not highlight it, and CRAN
will not take it. Every contributor who touches the typed source needs Roughly
installed. Code review sees two files where it saw one. A runtime error's
traceback names the generated file, not the file the author wrote.

The mitigations are real but partial. What ships *is* R, so consumers of the
package are unaffected. The generated output is committed and formatted, so a
reviewer can read it and a debugger can point at it. Adoption is per-file, so a
package can hold one typed file and thirty plain ones.

The comparison worth making is TypeScript, which won this trade decisively — but
its users already ran a build step for every project, and R package authors
mostly do not. That difference is the whole risk, and it is a judgement about R
programmers rather than about compilers. It should be made deliberately, ideally
against something real: build Layer 1, put it in front of R package authors, and
find out whether a build step is a shrug or a wall.

## 6. File layout and naming

A package gains a `TypedR/` folder beside `R/`. Each file in it compiles to a
file of the same stem under `R/`, which is what R runs and what ships.

```
TypedR/shapes.Rt     →  R/shapes.R
```

**The extension is `.Rt`.** Derivation, since this is the kind of choice that
otherwise gets re-argued: every extension in R's family puts the `R` first —
`.Rmd`, `.Rnw`, `.Rd`, `.Rout`, `.Rproj`, `.Rprofile`, `.Rbuildignore`. Nothing
in the ecosystem front-loads a qualifier. `.Rt` follows that convention, sorts
beside `.R` in a listing, and is unclaimed by R's own tooling. Two rejected
alternatives: `.tR` reverses R's convention and reads as a typo of `.R` (and on
a case-insensitive filesystem collapses to `.tr`, which signals nothing);
`.TypedR` is self-documenting but no language ships its full name as an
extension — TypeScript is `.ts`, PureScript is `.purs` — because the cost is
paid every time somebody types it.

**Stub files stay `.Rtypes`.** The convention elsewhere is host extension plus a
marker — `.d.ts`, `.pyi`, `.rbi` — which here would suggest `.Ri`, and as pure
family design (`.R` source, `.Rt` typed, `.Ri` declarations) it is tidier. It is
not worth a user-visible break: `.Rtypes` is shipped, documented, referenced by
the project-override convention, and says what it is. `.Rt` and `.Rtypes`
already read as siblings.

## 7. Compiling to R

**The target is annotated plain R.** Everything expressible as a `#:` comment is
emitted as one, so the generated file independently type-checks under the
existing contract. This is worth more than tidiness: it gives a cheap and
complete correctness test — checking the source and checking its output must
produce the same findings (§11).

**Generated files are committed.** R packaging needs real files under `R/`, and a
reviewer needs to see what actually ships. Each carries a header naming its
source and a hash of it:

```r
# Generated by roughly from TypedR/shapes.Rt — do not edit.
```

**Editing a generated file is caught, not merged.** The hash in the header
detects both a hand edit and a stale file, and `roughly build --check` fails on
either — the same shape as `roughly fmt --check`, which projects already run in
CI. Output is deterministic because it is rendered through the existing
formatter, so the check has no false alarms.

**Positions are mapped, not preserved.** A `#:` line emitted above a statement
shifts the lines below it, so one-to-one line identity is not achievable. The
compiler keeps a per-file line map, and every diagnostic reports the position in
the file the author wrote. Generated files never carry findings of their own.

## 8. Who writes files: the editor never does

The obvious design is "the language server compiles on save". It should not,
and the reason generalises.

If the editor writes files, then two editors open on one project — a second
window, or a terminal session alongside an IDE — are two processes writing the
same path. That is a torn-file risk, a redundant-work problem, and a potential
loop where one server's write is the other's file-change event. Solving it means
electing a leader between processes that cannot see each other.

The way out is to notice that **no established typed dialect generates output
from its language server.** TypeScript's server does not emit JavaScript;
`tsc --watch` or the bundler does. The compile step belongs to a build tool.

So: the language server is **read-only with respect to `R/`**. It analyses the
`TypedR/` sources directly, publishes findings on them, and never writes
anything. Two servers can both do that safely, because analysis is pure. This
costs nothing in editor experience, because the editor never needed the
generated file: a typed source **shadows** its generated twin (the twin is
excluded from analysis when the source exists, so each definition is seen once).
Generated R is needed only to run the code, to ship it, and for plain-R
consumers — all build-time or run-time concerns.

Generation therefore happens in exactly two places: `roughly build` (explicit),
and `roughly build --watch` as a single process the user starts when they want
the twins kept fresh while iterating.

If two builds ever do overlap, three properties make it safe without any
coordination protocol, and they are all properties the design wants anyway:
output is deterministic, so two compilers of the same source produce the same
bytes; the header hash means a compile whose output already matches **writes
nothing at all**; and writes are atomic (write a temporary file, rename it), so
no reader ever sees a half-written file. An advisory lock under `.roughly/` is
available as a backstop but should not be load-bearing. (The REPL already has
this shape internally: concurrent R sessions in one process are serialised by a
session lock rather than by hoping.)

**On sharing analysis between editors:** worth naming as considered and
declined. No mainstream language server does it, because the protocol is a
stateful one-to-one conversation — unsaved buffer contents, per-client document
versions — so a shared process needs a session-multiplexing layer in exchange
for a modest saving. The tractable version of the same wish is a **persistent
on-disk cache**, which gives cold-start speed to every process with no
coordination at all. That is the durability-tier lever already noted in
`backlog.md`, and it is the thing to build if start-up cost is the actual
complaint.

## 9. Running typed code

Two paths, and they want different things.

For a package, the answer is the committed twin: R runs `R/shapes.R` like any
other file, so `load_all`, `R CMD check` and CRAN all work unchanged.

For a standalone script there is no reason to leave a file on disk.
`roughly run script.Rt` should type-check, compile in memory, and execute —
which is possible because the REPL already embeds a real R runtime and has a
headless runner for exactly this shape of job (`repl-design.md`). The same
applies to the interactive console: a typed prompt is the REPL feeding compiled
lines to the same runtime. This is the most attractive small piece of the whole
proposal — a typed scripting language with no build artifacts — and it is worth
weighing whether it lands before the package story rather than after, since it
avoids the entire generated-file question.

## 10. Why this is a frontend, not a second product

The reason the estimate is not enormous is that the existing pipeline is
already separated along the right seam.

- **The type system does not know where types are written.** The typing
  reference defines meaning over types, not over comments. Records and tuples
  already exist as list shapes. Kinds decompose into named types, unions, and
  flow narrowing — all implemented.
- **The type notation is already a real grammar**, tokenized and parsed into
  proper syntax nodes inside `#:` regions. The dialect promotes that grammar
  from comments into code positions; it does not invent a second notation.
- **Everything after parsing consumes the internal representation, not the
  surface text.** Naming, inference, the package interface, the editor features
  and the incremental machinery see lowered items and types. A dialect construct
  that lowers to the same shapes flows through all of it unchanged.

Concretely: `syntax` gains a dialect switch on the parser and a handful of node
kinds, with the R dialect byte-for-byte unaffected (the existing corpus and
round-trip gates pin that); `semantics` lowers the new constructs onto shapes it
already has, adding no new kind of type for Layers 1–2 and only "a sum is a
union of its variants" for Layer 3; `format` learns the new nodes and doubles as
the compiler's emitter; `ide` gets the new files for free; `roughly` grows the
source folder, the twin shadowing, and `build`.

## 11. How we would know it works

Testing follows the standing doctrine — every stage gets coverage as it is
built, not afterwards — and one gate does most of the work.

- **Source and output must agree.** Checking `TypedR/shapes.Rt` and checking the
  generated `R/shapes.R` must produce the same findings, with positions mapped.
  This is the whole correctness argument for the compiler in one test.
- **Compilation fixtures**: typed input, expected R output, as readable diffs.
- **Type-checking fixtures** over typed sources, with the existing renderers.
- **Generated output always parses as R**, with zero errors — a property to
  fuzz, not just to sample.
- **Recompiling committed output is a no-op**, which is the determinism the
  drift check depends on.
- The fuzzing battery gains the dialect: never panic, keep the source
  recoverable from the tree, stay deterministic; plus compile-then-parse.
- **The R dialect's existing gates stay green throughout.** The claim that
  plain-R users pay nothing has to be evidence, not intent.

## 12. Delivery, with a decision gate

Each layer lands whole — grammar, checking, compiler, formatter, fixtures,
fuzzing — because a half-landed dialect has no users to learn from.

1. **Layer 1, types where the code is.** Pure translation onto comment
   annotations: no new meaning, no new runtime shapes. Proves the folder, the
   twin shadowing, `build`, the drift check and the position mapping.
2. **Decision gate.** Put Layer 1 in front of R package authors and answer the
   §5 question: is a build step a shrug or a wall? A dialect that only sugars
   annotations is not worth shipping if the answer is "wall" — the honest
   outcome then is to stop, keep comments as the only carrier, and take Route A
   in §3 for case analysis.
3. **Layer 2, records and tuples.** Constructor syntax over existing shapes;
   settles the constructor question the typing reference left open.
4. **Layer 3, kinds and `match`.** Nominal variants, S3 output, exhaustiveness.
   The actual prize.

## 13. Prior art, and what it settles

A typed R that compiles to R is not a new idea, and the existing attempt is
informative enough to change this document's recommendation rather than merely
decorate it.

### TypR (`we-data-ch/typr`)

A typed language for R, implemented in Rust, transpiling to R. Extension `.ty`.
Its own example:

```
type Person = list {
	name: char,
	age: int
};

new_person <- fn(name: char, age: int): Person {
	list(name = name, age = age)
};

is_minor <- fn(p: Person): bool {
	p$age < 18
};

alice <- new_person("Alice", 35);
alice.is_minor()
```

Three of its choices are worth studying, and only the third is one to copy.

**It is a sibling language, not a superset.** `fn(...)` replaces `function(...)`,
the return type follows a colon, statements end in semicolons, booleans are
`true`/`false` rather than `TRUE`/`FALSE`. An existing R file cannot become a
TypR file by adding anything; it has to be rewritten. That forfeits the property
this project treats as its core asset — useful answers about R that nobody has
modified — and it forfeits it for every user, not just the ones who opt in,
because there is nothing to opt into short of a rewrite.

Be fair about what breaking R's syntax *buys*, though, because it is real:
`alice.is_minor()` — a method-style call meaning `is_minor(alice)` — is genuinely
nice, since a completion list after `.` is the best discovery mechanism a typed
language has. **We cannot have it**, and the reason is worth recording so nobody
proposes it later: `.` is an ordinary character in R identifiers (`is.na`,
`as.character`, `my.var`), so `alice.is_minor` is already a legal variable name.
A dialect that stays R-compatible cannot reclaim the dot. Their willingness to
break compatibility is what makes their syntax pleasanter, and it is the same
decision that costs them incremental adoption. That trade is the fork in §3 in
concrete form.

**Its type reasoning runs on Prolog.** The tool requires SWI-Prolog installed
alongside R and Rust, and emits a Prolog file (`adt.pl`) that carries the type
reasoning. A logic engine is a powerful choice — subtyping, class hierarchies and
algebraic-datatype relations are all natural to express declaratively — but it
buys that with unbounded search, an external system dependency at build time, and
no path to incremental re-checking. It cannot become an editor experience that
answers in tens of milliseconds on a large package without being replaced.

This is the road not taken, running in public. The standing rule — admit only
what is fast to check, which means unification rather than search
(`decisions.md`) — was decided on principle; TypR is what the other principle
looks like when built. Their own README calls the project a prototype that is
"really buggy", and it would be unfair to pin that on the engine choice, but the
architectural consequence stands regardless of maturity: a Prolog-backed checker
and a keystroke-latency language server are different programs.

**It leads with package authoring, not with types.** Its stated aim is a better
experience building R packages and an easier path from a research paper to code.
That is the choice to copy. It correctly identifies that the felt pain is the
whole authoring experience, not the absence of a type annotation — and it is a
useful corrective to a proposal that could easily be written as "types, but as
syntax".

**What it validates:** that there is real appetite (a self-described buggy
prototype has attracted a few hundred stars), and that the destination — typed
source compiling to ordinary R — is one people find plausible. **What it leaves
open:** the whole editing experience, incremental checking, and working on
unmodified code. That is the space this project already occupies.

### Other attempts worth knowing

- **Runtime checking in R itself.** Packages exist that add type annotations
  enforced when the code runs (the `typed` package's `?` operator; `checkmate`
  and `assertthat` for argument assertions; `dgkf/typewriter` for annotations
  plus runtime checks). They prove the notation is welcome and confirm the gap:
  none of them can tell you anything before the line executes.
- **Academic work on typing R** exists, notably a paper titled "Towards a Type
  System for R" (ICOOOLPS 2019). Read it before designing the sum-type layer; it
  is the closest thing to a survey of which R idioms resist static description.
- **The compiles-to-host lineage**, which is where the packaging model here comes
  from: TypeScript to JavaScript, Sorbet's `.rbi` for Ruby, stub files for Python.
  The pattern every one of them converged on — declarations in a separate file
  for foreign code, annotations inline for your own — is the pattern already in
  place here (`.Rtypes` and `#:`).

### The honest limit of this survey

TypR's documentation site could not be read while writing this; the account above
comes from its README, its published crate metadata, its release history and
secondary sources. Its treatment of pattern matching and exhaustiveness in
particular is unconfirmed — if it has them, the "what it leaves open" claim above
needs revisiting, though not the conclusions about the engine or the syntax
break.

## 14. Open questions

- The fork in §3, before anything else.
- Whether the standalone-script path (§9) should lead instead of follow, since it
  sidesteps generated files entirely.
- Spellings: the tuple constructor; whether nominal and structural records are
  distinguished at the construction site (`new Point{...}` versus `Point{...}`);
  `match (x)` versus `match x`.
- Whether a typed file may also contain `#:` comments. Leaning no: one carrier
  per file keeps the attachment rules from mattering at all.
- Whether `roughly build` also formats sibling plain-R files. Leaning no: build
  and format stay separate commands.
- Whether to emit line-directive markers for R's debugger, or rely on the line
  map plus readable committed output.
- Whether `TypedR/` needs a `DESCRIPTION` entry so R's own tooling ignores it
  cleanly everywhere it walks a package.
