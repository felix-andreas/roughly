# Inline type syntax for R (a compiled dialect)

Status: proposal, nothing implemented.

**Recommendation up front: do not build this for inline typing alone.** The
ergonomic case is weaker than it looks (§3), and the cost is close to total — a
typed file is not R until compiled, so every other R tool is blind to it and every
contributor needs this tool installed. Build it if and only if the capabilities in
§7 are wanted, because those need a syntax and inline typing is the cheapest way to
prove the machinery they would sit on. §3 and §7 are the two sections that decide
it; the rest is design detail for whoever says yes.

## 1. What it is

A source language where a type is written at the position it describes, compiled
to plain R. Today:

```r
#: <T: numeric> fn(values: T, scale: T) -> T
scale_by <- function(values, scale) values * scale
```

Proposed:

```
scale_by <- function<T: numeric>(values: T, scale: T) -> T {
  values * scale
}
```

## 2. Four inline positions, and `#:` stays

Types go in four places, all reusing the existing notation verbatim:

```
scale_by <- function<T: numeric>(values: T, scale: T) -> T { ... }
count: integer <- 0L
lapply(xs, function(v: integer) v + 1L)
```

Generic binders, parameter types, return type, binding type. The binder needs its
own position because the existing notation puts it as a prefix on the *whole* type
expression (`<T> TYPE`), and inline there is no whole type expression to prefix —
every language with generics solves it the same way.

**`#:` comments remain available in a typed file, and that is deliberate.** Several
annotation forms are not type expressions at all and have no inline position:
`@type` and `@alias` declare types, `@new` mints a nominal value, and `@trust`,
`@if-unknown` and `@strict` are directives. The reference is explicit that `@new`
"is an annotation form, not a type expression". Banning `#:` in typed files — which
an earlier draft leaned toward — would make the dialect **strictly less expressive
than plain R**: no record types, no nominals, no generic aliases.

Keeping both carriers also makes conversion additive. A `.R` file becomes a `.Rt`
file by renaming it; nothing must be rewritten, and annotations move inline one at
a time or never. The division is clean enough to state in one line: **inline syntax
carries types on code; `#:` carries declarations and directives.**

## 3. The ergonomic case, corrected

An earlier draft of this document listed four costs of the comment carrier. Three
do not survive checking, and the correction matters more than the original claim
because it is what moves the decision to §7.

**Parameter names are not duplicated — that was wrong.** Names in the compact form
are optional. This checks, with no name written twice:

```r
#: fn(character, double) -> double
fee_for <- function(region, amount) { ... }
```

Writing the names is a style choice for readability, not a tax the carrier imposes.

**Broken attachment is loud, not silent.** An intervening plain `#` comment does
detach a `#:` block, but the result is an error whose message contains the fix:
"A `#:` typing comment must be followed immediately by an expression — put it
directly above the definition, below any roxygen2 block." The natural roxygen
order works with no diagnostic. This is one self-explaining error, not a trap.

**Annotating a local works.** A `#: integer` line above a local assignment is
checked. `count: integer <- 0L` is shorter; that is a preference.

**One real gap remains.** A `#:` comment attaches to statements, and a lambda
inside a call argument is not a statement, so its parameter cannot be annotated:

```r
out <- lapply(xs, function(v) v + 1L)     # `v` has no annotatable position
```

Worse, an annotation written there is *silently ignored* — a deliberately
contradictory type produces no diagnostic. Two separate defects hide in that: the
silence, and the gap. **The silence should be fixed in the comment carrier
regardless of this proposal** — an annotation in a position that cannot attach
should be reported. That is a small, independently valuable change and it belongs
in the backlog either way. But a diagnostic only makes the failure visible; it does
not give you a way to type the lambda. Only inline syntax does.

So the honest ergonomic case is: one position that comments cannot reach, plus
shorter locals. That does not justify a source language.

## 4. Compiling to R

**The output is annotated R.** Each inline type is emitted as the `#:` comment
meaning the same thing, so the generated file independently type-checks under
today's contract. That gives a complete correctness test nearly for free — checking
the source and checking its output must produce the same findings — and keeps the
generated file self-describing. §3's complaints do not apply to generated output:
duplication and attachment are the compiler's problem there, not a human's.

**The generated file under `R/` is the runtime truth for everyone else.** R runs
it, `source()` reads it, other packages see it, CRAN ships it. Nothing outside this
tool ever reads a `.Rt` file. That is the whole of the interop story and it should
be stated rather than implied.

**Analysis reads the source; a stale twin is reported, not ignored.** A typed
source shadows its generated twin, so each definition is analysed once. But
shadowing alone would make a stale committed twin invisible — the configuration
where a reviewer reads one program and the author reads another. So the twin's
header hash is checked during analysis too, and a mismatch is a project-level
diagnostic in the editor, not only a `roughly build --check` failure.

**Mixed packages need no special rule.** A typed source participates in the package
namespace exactly as its twin would, and collation order is defined over the
logical file set — source where one exists, twin otherwise — which is total, so
"later file wins" keeps its meaning.

**No position map has to be persisted.** Diagnostics come from analysing the
source, so positions are already the author's. A map is needed only to trace a
*runtime* error in generated code back to its origin, which is a debugging
convenience; the committed, formatter-normalised output plus a header pointer is
the fallback every compiled-to-host language relies on.

**The editor never writes.** Generation happens in `roughly build` and in a single
`roughly build --watch` the user starts. Two editors open on one project would
otherwise be two processes writing one path, and determinism plus the header hash
means a redundant build writes nothing at all.

## 5. The file extension: `.Rt`

R's family puts the `R` first, then a short lowercase mnemonic: `.Rmd`, `.Rnw`,
`.Rd`, `.Rout`, `.Rproj`, `.Rbuildignore`. `.Rt` follows that, sorts beside `.R`,
and is unclaimed in R's own tooling (checked against the `tools` package and R's
`share` tree). The prefix already means "R-family, not R itself" — `.Rmd` is
markdown — so it promises nothing about being executable.

Rejected: **`.tR`** reverses the convention, which nothing in R's ecosystem does,
and reads as a typo of `.R`. **`.TypedR`** is a full language name, which no
language ships as an extension, and mixed case mid-extension invites mistakes.
**`.ty`** is taken by an existing typed-R project (§8) and is not R-family.
**`.Rty`** is the runner-up if `.Rt` proves too terse.

Two corrections to arguments an earlier draft made here. Case-insensitive
filesystems are not a reason to prefer `.Rt` over `.tR` — `.Rt` collapses to `.rt`
just as `.tR` collapses to `.tr`, and R's own file collector already accepts both
`.R` and `.r`. And typed sources *can* physically live in `R/`: R's collector
ignores unknown extensions there. A sibling directory is still right — for tarball
hygiene, for roxygen, and so a reader is never unsure which file ships — and
whatever it is called it belongs in `.Rbuildignore`, since `R CMD check` notes
non-standard top-level directories.

**Stub files stay `.Rtypes`.** `.Ri` would make a tidier family alongside `.d.ts`,
`.pyi` and `.rbi`, but renaming a shipped, documented surface for symmetry is not
worth it.

## 6. The parser work, honestly

An earlier draft claimed "this is not a large parser change". That was wrong, and
the reason is specific: **the type grammar is not self-delimiting.** It currently
lives in a region the R grammar skips as trivia, and every type-parsing function
takes a precomputed region end derived from newlines. Inline there is no region
end, so termination becomes a grammar decision at two genuinely ambiguous points:

- **where a return type ends and the body begins.** `-> double { ... }` is easy;
  `-> fn(integer) -> double { ... }` is a higher-order return type containing its
  own arrow, and the `{` could belong to either.
- **`<` and `>` as binder brackets versus comparison operators.** R's own grammar
  cannot disambiguate, since `a < b > c` is a parse error in R.

Two things that are *not* problems, verified against R's parser: parameter types
and `->` in the return position are pure extensions — `function(region: character)`
and `function(a, b) -> double {` are both hard parse errors in R today, so nothing
is being reinterpreted.

**One position does collide.** `count: integer <- 0L` already parses in R, as the
replacement form `` `:<-`(count, integer, 0L) ``. No working program contains it —
`` `:<-` `` is not defined in base R, so the form errors at run time unless someone
defines that function deliberately — but the dialect must state that it claims the
form, and it must use the presence of `<-` to tell a binding declaration from a
sequence expression, because statement-level `a: b` is valid R.

That last point bounds the superset claim precisely: **every runnable R program is
a valid typed program**, which is weaker than "every parseable R file" and is the
honest version.

## 7. What a syntax would make possible

This is where the value is, if there is any. Two categories, separated by whether
the compiler has to invent a runtime representation.

**Representation already decided.** Inline types (§2) compile to comments over
unchanged code. Record and tuple constructors compile to `list(...)` — the type
system already has both shapes structurally, and a checked construction site is
what a constructor buys: a misspelled or missing field caught where the mistake is
rather than at some later read.

**Representation still open — tagged unions.** Declaring that a value is one of
several kinds, with dispatch that reports the case you forgot. This is the feature
with real pull and the one that most needs deliberate design, because the runtime
representation is an open choice: a field the compiler owns (`list(.tag = ...)`) is
closed and controllable, while R's class attribute is open, mutable, and carries
inheritance this type system does not model. Neither is obviously right and neither
should be assumed. Nothing here should be designed until inline typing has shipped
and the dialect has users.

Two claims an earlier draft made here were false and are withdrawn.
Non-bypassable construction is *not* something only syntax can offer — `@new` is
already the only nominal introduction and its construction is already checked
against the representation; and since a compiler-written constructor still emits
`list(...)`, any R code that fabricates the list bypasses it equally.

Anything that changes *evaluation* — block scope, for instance, since R's braces
are not a scope — is a different proposition: the output stops resembling the
input and every debugging story gets harder. That is where a dialect stops being a
frontend and becomes a language.

## 8. The cost, and what would actually test it

A `.Rt` file is not R. Until compiled, roxygen2 will not document it,
`devtools::load_all()` will not load it, RStudio will not highlight it, CRAN will
not accept it, and every contributor who touches it needs this tool. The risk is
not that a build step is unfamiliar — `R CMD build` is one and roxygen2 is code
generation most packages already run. It is that the source of truth stops being R,
for collaborators and for CRAN.

**Standalone scripts are the cheap place to start and they do not test that risk.**
`roughly run script.Rt` — type-check, compile in memory, execute through the R
runtime the REPL already embeds — needs no generated file, no packaging and no
collaborator toolchain. That makes it a good way to find out whether inline typing
is *pleasant*, and no evidence at all about whether package authors will accept a
generated `R/` tree, because scripts remove every variable that constitutes the
objection. Testing the packaging question requires putting a generated tree in
front of package authors.

## 9. Open questions

- Where a generic binder goes if not `function<T>(...)`, given that R's `<` is a
  comparison operator (§6).
- What the source directory is called.
- Whether inline types may annotate `...`, which the comment notation types as a
  rest parameter.
- Whether the script path (§8) ships with no `roughly build` at all in v1.
- Hover, go-to-definition and rename on an inline type position — expected to fall
  out of the existing type-position support, but unverified.
- Editor behaviour on a half-written inline type, where error recovery matters more
  than in a comment.

## 10. Prior art

**`we-data-ch/typr`** is a typed language for R, written in Rust, transpiling to R,
extension `.ty`. It is a sibling language rather than a superset — `fn` replaces
`function`, statements end in semicolons, booleans are lowercase — so an existing R
file cannot adopt it by adding anything. That is the opposite of the position here,
where renaming a file is the whole of adoption.

Its type reasoning runs on Prolog, a required install alongside R. Whether that
could answer at editor latency is not something this survey can judge — its
documentation site could not be read, and its own README calls the project an early
prototype — but the shape of the bet is the opposite of the fast-to-check rule in
`decisions.md`.

**The compiles-to-host lineage** — TypeScript, Sorbet's `.rbi`, Python's stub files
— converged on the same split this project already has: declarations in a separate
file for foreign code, annotations inline for your own. `.Rtypes` implements the
first half; this proposal is the second.
