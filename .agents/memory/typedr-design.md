# Kinds and exhaustive dispatch in R

Status: proposal. §4 is ready to build and valuable on its own. §5 needs the
questions in §6 answered first.

## 1. The gap

R's way of saying "this value is one of several kinds" is the `class` attribute,
and its way of handling one is a chain of `inherits` tests:

```r
area <- function(shape) {
  if (inherits(shape, "Circle")) pi * shape$r^2
  else if (inherits(shape, "Square")) shape$s^2
}
```

Add a third kind and `area` returns `NULL` for it. The `NULL` travels and fails
somewhere else — the failure this tool exists to prevent. Nothing in R reports it.

## 2. Values already get kinds

This much works today, unchanged:

```r
#: @type Circle {list{r: double}}
#: @type Square {list{s: double}}
#: @alias Shape {Circle | Square}

#: @new Circle
circle <- structure(list(r = 2), class = "Circle")

#: fn(shape: Shape) -> double
area <- function(shape) 1
area(circle)          # accepted
```

`@new` mints the nominal type; the `class` attribute makes the value a real S3
object at run time. So the missing pieces are narrower than they look: the checker
cannot **narrow** a kind inside a branch (§4), and cannot tell you a branch is
**missing** (§5). Minting is solved.

## 3. The type never comes from the class attribute

The tempting shortcut is to read `class = "Circle"` as a declaration of type.
Three reasons not to, and together they answer whether a separate attribute like
`tag` would give better control.

**It would reverse a settled contract.** `reference.md` states that "`@new` is the
ONLY nominal introduction: a checked annotation on a structural value is a type
error even when the value matches the representation". A second, weaker minting
path — matching by representation, silently producing a plain record when the
representation does not fit — is exactly the duplicated-source-of-truth case the
design bar says to stop for. `decisions.md` records this hole being found and
closed once already.

**It would reject correct code.** R class vectors are usually longer than one
entry, and taking the first is a guess rather than a gap:

```r
class(dplyr::group_by(df, a))   # "grouped_df" "tbl_df" "tbl" "data.frame"
```

Minting from the first entry types that value as `grouped_df`, which then fails at
any parameter annotated `data.frame` — code that is accepted today, and correct.
The project's rule for a construct it cannot model is refusal to `Unknown`, never
a guess; asserting an exact nominal identity for a value the checker knows is also
three other things is a guess.

**It is where the control comes from.** Because a kind is minted only where the
checker sees `@new`, a `class` attribute it did not mint confers nothing: a value
arriving from another package with `class = "Circle"` is not this project's
`Circle`. That is the property a private `tag` attribute was meant to buy, and it
is already available — so the extra attribute would cost the thing that makes
kinds worth having. A `tag`ged value is not an S3 object: `print.Circle` would not
dispatch, `inherits()` would not see it, and every package that branches on class
would see a bare list. Reusing `class` keeps the runtime carrier idiomatic while
the *type* stays closed.

So: **the class attribute is the runtime carrier, and the annotation is the
type.** They are set together at one site and never inferred from each other.

*(Rejected middle option, recorded so it is not re-proposed: an opt-in marker
inside the class vector, `class = c("Circle", "roughly_kind")`. It gives
closed-world detection while staying S3, but writes tool-specific strings into
user data and forfeits idiomatic output.)*

## 4. Step one: `inherits()` narrows

The narrowing machinery today covers `is.null` and the `is.*` family. Extending it
to `inherits` makes this check, with the else branch knowing `shape` is a `Square`:

```r
if (inherits(shape, "Circle")) pi * shape$r^2 else shape$s^2
```

**It narrows by name, not by reading attributes.** The tested string is matched
against the *member names* of the subject's union. Nothing inspects a runtime
class, so §3's argument is untouched.

**Two cases must be handled rather than fall through.** If the tested name is an
`@alias` for a union covering every member, the test is statically true and
narrows nothing. If it names something no member can be — a typo, or an unrelated
class — the kept set is empty; today the refinement is silently dropped and the
branch body is checked against the unnarrowed union, which is a wrong answer with
no diagnostic. It should report that no member of the union can have that class.

**Cost, stated accurately.** This is not a table row. Guard recognition currently
requires exactly one unnamed argument that is a bare name (`check.rs`, the
`[argument] = arguments.as_slice()` gate), and the discriminating payload here is
the *second* argument's literal, so both the recognition shape and the guard kind
change. Two `inherits` forms must be refused rather than misread:
`inherits(x, "S", which = TRUE)` returns an integer, and `inherits(x, c("A","B"))`
tests a vector.

**Why it is worth doing alone:** `inherits` guards are everywhere in R, written by
people who will never declare a kind.

## 5. Step two: exhaustive dispatch

The shipping form is a call, because R already has everything needed:

```r
switch_class(shape,
  Circle = pi * shape$r^2,
  Square = shape$s^2
)
```

**Implementation.** The body must forward `...` into `switch` rather than
materialise it — `list(...)` forces every branch, which would evaluate all of them
for their side effects:

```r
switch_class <- function(.value, ...) {
  switch(class(.value)[1], ..., stop("unhandled class: ", class(.value)[1]))
}
```

The `stop()` default is not decoration: R's `switch` returns `NULL` invisibly when
nothing matches, which is the failure mode §1 is about. The first formal must not
be named `x` (or anything a class might be called): R matches a *branch* named `x`
to the formal, so `switch_class(z, x = ...)` would silently pass the branch as the
subject. A dotted name is R's own convention for this.

**Name.** Both short candidates are taken in the audience's own libraries:
`purrr::when` exists (a predicate-based functional `if` — different construct,
same name) and `dplyr` occupies `case_when`/`case_match`. `match` and `switch` are
base R. A collision is not merely confusing: whichever package attaches second
masks the other, and this project's own `shadows-namespace` lint reports it.
`switch_class` collides with nothing, says what it dispatches on, and evokes the
`switch` it compiles to. If a dialect ever adds a keyword, `when` can be the sugar
then — a keyword cannot be masked.

**The call's type is the union of the branch types**, matching the rule already
documented for `switch`. Not `Any`: a construct whose result is compatible with
everything and invisible to strict mode would be less typed than the `switch` it
replaces, which would defeat the purpose.

**Four diagnostics**, and the second matters more day to day than the first:

- a member of the subject's union that no branch covers;
- a branch naming something that is not a member — the misspelled-branch case,
  which otherwise reaches `stop()` at run time;
- two branches naming the same member;
- a subject with no union to check — an unannotated parameter, `Any`, or
  `Unknown`. Exhaustiveness cannot be decided there, so it is a strict-mode
  origin rather than silence. This is the limitation that made generic dispatch
  underdeliver (`decisions.md`), and it applies here for the same reason: R code
  is most dynamic exactly where dispatch matters most.

**Recognition is declarative, not a hardcoded name.** The corpus already carries
this class of fact as a declaration attribute — `@masked` marks a data-masking
function, and the checker reads it from the stub rather than knowing dplyr's verb
names:

```
switch_class : @exhaustive fn(.value: Any, ...: Any) -> Any
```

`@exhaustive` means "the named arguments are branches over the first argument's
union". It also determines the call's type per the rule above, so the declared
return position is unused and must be written `Any`. One general rule, names as
data, and a project can declare its own dispatcher the same way.

**What this cannot do** is restrict construction: nothing stops a caller building
the list by hand and skipping `@new`. That is not a small hole — it is what makes
exhaustiveness advisory rather than guaranteed. It is the same class of hole as
`Any` and as every unmodelled construct that becomes `Unknown`, and it is the
price of staying inside R.

## 6. Open questions

- Whether `inherits` narrowing should also handle the `all.equal`-style negative
  (`!inherits(...)`) on the false edge — the existing guard machinery has both
  edges, so this is probably free, but it is untested.
- What a class vector longer than one entry should do at an `@new` site: accept
  silently, or report under strict mode that the extra entries mean something the
  checker does not model. Silence is what manufactures false confidence.
- Whether a branch that opts out of exhaustiveness is worth having, or whether
  opting out means writing `switch` directly.
- Whether the compiled or hand-written `stop()` default should type as `Never`
  rather than `Any`. The reference notes `Never` is unimplemented and `stop`
  returns `Any` as a stand-in; that makes a hand-written `switch` with a `stop`
  default type as `Any` today, which is worth fixing independently.
- How editing the union alias interacts with the interface firewall: an alias is a
  project-wide declaration, so widening it re-checks every dispatch in the
  workspace. Acceptable, but it should be measured rather than assumed.
- If the union comes from a stub, a package upgrade can change exhaustiveness
  verdicts. Every language with exhaustive matching across a library boundary has
  this; the usual answer is a default branch.

## 7. Whether this needs a dialect

It does not. §4 and §5 are comment annotations plus one library function, so files
stay ordinary `.R`: no build step, no rewrite, every other R tool unaffected.

A separate language would add types in the code rather than in comments
(ergonomics, not capability), checked record and tuple construction, and
construction that cannot be bypassed. Against that, a typed file is not R until it
is compiled, so roxygen2, `devtools::load_all()`, RStudio and CRAN all see nothing
until then, and every contributor who touches the typed source needs this tool
installed. The risk is not that a build step is unfamiliar — `R CMD build` is one,
and roxygen2 is code generation that most modern packages already run. It is that
the *source of truth* stops being R, for collaborators and for CRAN.

One corner escapes most of that: **standalone scripts**. `roughly run script.Rt`
can type-check, compile in memory and execute through the R runtime the REPL
already embeds — no generated file, no packaging, no collaborator toolchain. If a
dialect ships at all, that is where it is cheapest.

Two notes for whoever revisits it. The extension should be `.Rt`: every extension
in R's family puts the `R` first (`.Rmd`, `.Rnw`, `.Rd`, `.Rout`), and `.Rt` is
unclaimed by R's own tooling, while `.tR` reverses the convention and reads as a
typo. And branch syntax cannot use `->`: in R that is right-assignment, so
`Circle -> pi * r^2` parses as `pi * r^2 <- Circle`.

## 8. Prior art

**`we-data-ch/typr`** is a typed language for R, written in Rust, transpiling to
R, extension `.ty`. It is a sibling language rather than a superset — `fn`
replaces `function`, statements end in semicolons, booleans are lowercase — so an
existing R file cannot adopt it by adding anything; it must be rewritten. That
spends the property this project treats as its core asset: useful answers about R
nobody has modified.

Its type reasoning runs on Prolog, which is a required install, and it emits an
`adt.pl` carrying the reasoning. That is expressive, and bought with unbounded
search plus an external build dependency. Whether it could be made to answer at
editor latency is not something this survey can judge — its documentation site
could not be read, and its own README calls the project an early prototype — but
the shape of the bet is the opposite of the fast-to-check rule in `decisions.md`,
and it is useful to have a public example of that road.

Worth copying: it leads with package-authoring experience rather than with types.

**Other work.** R packages that check types at run time (`typed`, `checkmate`,
`dgkf/typewriter`) show the notation is welcome and confirm the gap — none can say
anything before the line executes. The paper *Towards a Type System for R*
(ICOOOLPS 2019) surveys which R idioms resist static description; read it before
§5.
