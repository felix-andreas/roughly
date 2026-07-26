# Inline type syntax for R (a compiled dialect)

Status: proposal, nothing implemented. This document covers one thing — putting
types where the code is instead of in comments — plus the file-extension decision
and a survey of what owning a syntax would make possible later.

## 1. What is proposed

Today a type is written in a `#:` comment above the thing it describes:

```r
#: fn(region: character, amount: double) -> double
fee_for <- function(region, amount) { ... }
```

The proposal is a source language where it is written at the position it
describes, compiled to plain R:

```
fee_for <- function(region: character, amount: double) -> double { ... }
```

The `#:` form is not deprecated by this and stays the only carrier for ordinary
`.R` files. It is also the compile target (§4).

## 2. Why the comment carrier costs something

Comments were chosen so that annotated code stays ordinary R, and that was the
right trade. But the choice has four consequences, and they are the motivation
for this proposal — not "types look nicer inline".

**Every parameter name is written twice.** `function(region, amount)` already
names the parameters; the annotation names them again. Rename one and both must
change. The expanded form doubles it again, one `@param` line per parameter.
Duplication that a tool could eliminate is the ordinary reason to move a
declaration next to what it declares.

**Attachment rules exist only because the type is somewhere else.** A `#:`
comment binds to the statement below it, so the binding can be broken: an
intervening plain `#` comment detaches it, and interleaving with roxygen's `#'`
lines — the first thing a package author tries — does too. That was a real
complaint from an adoption review. None of these rules would exist if the type
were in the parameter list, because there would be nothing to attach.

**Some positions cannot be annotated at all.** A `#:` comment attaches to
statements, and a lambda inside a call argument is not a statement:

```r
out <- lapply(xs, function(v) v + 1L)     # `v` has no annotatable position
```

Verified: writing `#: fn(character) -> character` beside that lambda is **silently
ignored** — a deliberately contradictory type produced no diagnostic. So this is
not only a gap but a quiet one. (Independently: an annotation in a position that
cannot attach should be reported rather than dropped. Worth fixing in the comment
carrier regardless of this proposal.)

**Annotating a local is noisy.** It works — a `#: integer` line above a local
assignment is checked, verified — but it costs a whole line for one word:

```r
count: integer <- 0L        # proposed
```

Note what is *not* on this list: capability. Everything expressible inline is
expressible in a comment, apart from the lambda case. This proposal is about the
notation being in the right place, and §7 is where new capability would come from.

## 3. The syntax

Three positions, all reusing the existing type notation verbatim — same type
names, same generics (`<T: numeric>`), same shapes. No second type language.

```
fee_for <- function(region: character, amount: double) -> double {
  if (region == "north") amount * 0.07 else amount * 0.05
}

count: integer <- 0L

lapply(xs, function(v: integer) v + 1L)
```

Parameter types after `:`, return type after `->`, binding types after `:`. The
optional-parameter and generic forms carry over unchanged from the comment
notation, because they are the same grammar.

This is not a large parser change. The lexer already tokenizes the full type
notation inside `#:` regions and the annotation parser already builds real syntax
nodes from it. The dialect enables those nodes in three expression positions; it
does not invent a notation.

## 4. Compiling to R

**The output is annotated R.** Each inline type is emitted as the `#:` comment
that means the same thing, so the generated file independently type-checks under
today's contract:

```r
#: fn(region: character, amount: double) -> double
fee_for <- function(region, amount) { ... }
```

Two reasons for that target rather than deleting the types. It gives a complete
correctness test almost for free — checking the source and checking its output
must produce the same findings, with positions mapped — and the generated file
stays self-describing for anyone reading it without the original. §2's complaints
about the comment form do not apply to generated output: duplication and
attachment are the compiler's problem there, not a human's.

**Positions are mapped, not preserved.** Emitting a `#:` line above a statement
shifts everything below it, so one-to-one line identity is impossible. The
compiler keeps a line map and every diagnostic reports a position in the file the
author wrote. Generated files never carry findings of their own.

**Generated files are committed.** R packaging needs real sources under `R/`, and
a reviewer needs to see what ships. Each generated file carries a header naming
its source and a hash of it, so a hand edit or a stale file is detected rather
than silently merged; `roughly build --check` fails on either, the same shape as
`roughly fmt --check`. Output is deterministic because it renders through the
existing formatter.

**The editor never writes.** The language server analyses typed sources directly
and publishes findings on them; generation happens only in `roughly build` and in
a single `roughly build --watch` process the user starts. Two editors open on one
project would otherwise be two processes writing one path. This costs nothing in
the editor, because a typed source shadows its generated twin — the twin is
excluded from analysis when the source exists, so each definition is seen once.

## 5. The file extension

**`.Rt`.**

R's family puts the `R` first and follows it with a short lowercase mnemonic:
`.Rmd`, `.Rnw`, `.Rd`, `.Rout`, `.Rproj`, `.Rprofile`, `.Rbuildignore`. `.Rt`
follows that pattern, sorts beside `.R` in a listing, and is unclaimed by R's own
tooling (checked against the `tools` package and R's `share` tree). The prefix
also already means "R-family, not R itself" — `.Rmd` is markdown — so it carries
no false promise that R can execute the file.

Rejected:

- **`.tR`** reverses the convention; nothing in R's ecosystem front-loads a
  qualifier. It reads as a typo of `.R`, and on a case-insensitive filesystem it
  is the same file as `.tr`, which signals nothing.
- **`.TypedR`** is self-documenting but no language ships its full name as an
  extension — TypeScript is `.ts`, PureScript is `.purs` — because the cost is
  paid every time it is typed, and mixed case mid-extension invites mistakes.
- **`.ty`** is taken by an existing typed-R project (§8) and is not R-family.
- **`.Rty`** also follows the convention and is more explicit; it is the runner-up
  if `.Rt` proves too terse in practice.

**Stub files stay `.Rtypes`.** The convention elsewhere is host extension plus a
marker (`.d.ts`, `.pyi`, `.rbi`), which would suggest `.Ri` and would make a tidier
family, but renaming a shipped, documented surface for symmetry is not worth it.
`.Rt` and `.Rtypes` already read as siblings.

**The source directory needs one packaging detail.** Typed sources cannot live in
`R/` — that is where their output goes — so they need a sibling directory, and R
package layout already claims `src/` for compiled code. Whatever it is called, it
must be listed in `.Rbuildignore`: `R CMD check` emits a NOTE for non-standard
top-level directories, and the typed sources should not ship in the tarball
anyway, since the generated R is what R runs.

## 6. What it costs, and where to start

A `.Rt` file is not R. Until it is compiled, roxygen2 will not document it,
`devtools::load_all()` will not load it, RStudio will not highlight it, and CRAN
will not accept it. Every contributor who touches a typed source needs this tool
installed.

The risk is not that a build step is unfamiliar — `R CMD build` is one, and
roxygen2 is code generation that most modern packages already run. It is that the
*source of truth* stops being R, for collaborators and for CRAN.

**One corner escapes almost all of it: standalone scripts.** `roughly run
script.Rt` can type-check, compile in memory and execute through the R runtime the
REPL already embeds — no generated file, no packaging, no collaborator toolchain,
nothing committed. That is where a dialect is cheapest to try and cheapest to
abandon, and it answers the adoption question with evidence instead of prediction:
if a typed script is pleasant enough that people want it for packages too, the
packaging half has earned its cost.

## 7. What owning a syntax would make possible later

This section is a survey, not a plan. The useful way to rank these is by **what
they need at run time**, because that is what determines the cost.

**Nothing — pure erasure.** Inline types (§3) are the whole of this category. The
compiled R is what the author would have written by hand, plus comments. This is
why inline typing is the right first step: it has no semantic surface to get
wrong.

**A list — a construction the compiler writes.** Record and tuple constructors
with checked construction sites; the type system already has both shapes
structurally, and the reference already anticipates named constructors for them.
Compiles to `list(...)`. A constructor that is syntax rather than a callable
function is also the only way to make construction non-bypassable, which no
comment-carrier design can offer.

**A representation decision — tagged unions.** Declaring that a value is one of
several kinds, with dispatch that reports the case you forgot. This is the feature
with the most obvious value and the one that most needs a deliberate design,
because the runtime representation is a real choice with no default: a dedicated
field the compiler owns (`list(.tag = "...", ...)`) is controllable and closed,
whereas leaning on R's legacy class attribute inherits an open, mutable,
inheritance-shaped mechanism this type system does not model. Do not treat the
legacy option as the obvious one. Nothing here should be designed until inline
typing has shipped and the dialect has users.

**A code transformation — anything that changes evaluation.** Block scope is the
example worth knowing: R's braces are not a scope, so a dialect could give them
one, but the compiled output would no longer resemble the input and every debugging
story gets harder. This category is where a dialect stops being a frontend and
becomes a language. Approach with suspicion.

## 8. Prior art

**`we-data-ch/typr`** is a typed language for R, written in Rust, transpiling to
R, extension `.ty`. It is a sibling language rather than a superset: `fn` replaces
`function`, statements end in semicolons, booleans are lowercase. An existing R
file therefore cannot adopt it by adding anything — it must be rewritten. That is
the opposite of the position taken here, where every valid R file is intended to
be a valid typed file with the types being the only additions.

Its type reasoning runs on Prolog, a required install alongside R, with the
reasoning emitted to an `adt.pl`. Whether that could answer at editor latency is
not something this survey can judge — its documentation site could not be read and
its own README calls the project an early prototype — but the shape of the bet is
the opposite of the fast-to-check rule in `decisions.md`, and it is useful to have
a public example of that road.

Worth copying: it leads with package-authoring experience rather than with types.

**The compiles-to-host lineage** — TypeScript to JavaScript, Sorbet's `.rbi` for
Ruby, Python's stub files — converged on the same split this project already has:
declarations in a separate file for foreign code, annotations inline for your own.
Inline typing is the second half of that pattern, which `.Rtypes` already
implements for the first.

## 9. Open questions

- Whether the return-type marker should be `->`. It is unambiguous in a function
  header, but `->` is R's right-assignment operator, so a shared lexer must not
  treat it as one in that position. If that proves awkward, `:` before the body is
  the alternative.
- Whether a typed file may also contain `#:` comments. Leaning no — one carrier
  per file means the attachment rules never apply.
- What the source directory is called, and whether the choice should be
  configurable.
- Whether the script path (§6) ships before any packaging support, which would
  mean no `roughly build` in the first version at all.
- Whether inline types should be permitted on a function's `...`, which the
  comment notation types as a rest parameter today.
- Whether the formatter treats a typed file as a first-class input from day one,
  or whether formatting is only defined on generated output at first.
