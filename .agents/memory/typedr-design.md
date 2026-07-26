# Kinds and exhaustive dispatch in R (and whether that needs a dialect)

Status: proposal, nothing implemented. The first two steps below need no new
syntax and are worth doing regardless of how the last one is carried.

## 1. The gap

R's way of saying "this value is one of several kinds" is the `class` attribute,
and its way of handling one is a chain of `inherits` tests:

```r
circle <- structure(list(r = 2), class = "Circle")

area <- function(shape) {
  if (inherits(shape, "Circle")) pi * shape$r^2
  else if (inherits(shape, "Square")) shape$s^2
}
```

Add a third kind and `area` returns `NULL` for it. The `NULL` travels and fails
somewhere else — the failure this whole tool exists to prevent. Nothing in R
reports it.

The checker cannot help yet, and it is worth being exact about why. Declare the
kinds and run it today:

```r
#: @type Circle {list{r: double}}
#: @type Square {list{s: double}}
#: @alias Shape {Circle | Square}

circle <- structure(list(r = 2), class = "Circle")
```

`circle` infers as `list{r: double}`, not as `Circle`. **The class attribute is
ignored**, so passing `circle` where `Shape` is expected is an error on correct
code. And `inherits(shape, "Circle")` narrows nothing, so even with the type
declared, the branch bodies go unchecked.

An adoption review made the cost concrete: 31 of one reviewer's 34 findings under
the strictest setting traced to `structure(list(...), class = ...)`, the idiom R
packages are built from.

## 2. Reuse R's own mechanism

There is no reason to invent a word like "tag". **A kind is an S3 class** — R
already stores it, already dispatches on it, and every R programmer already reads
it. Two consequences:

- A kind is a nominal type whose name is the class string. `@type Circle` and
  `class = "Circle"` are the same fact stated twice; the job is to connect them.
- A set of kinds is an ordinary union: `@alias Shape {Circle | Square}`. `Shape`
  exists only in annotations. **It is never written into a class attribute**,
  because it is not a class — it is a name for a choice between two.

That second point rules out a design worth naming so nobody proposes it later:
emitting `class = c("Circle", "Shape")`. R uses a class vector to express
inheritance, and this type system has no subtyping — decided deliberately
(`decisions.md`), because subtype inference is a different and slower algorithm
than unification. So only `class(x)[1]` is modelled: it is the dispatch key, and
it is the whole of what a kind means here. Further class entries are ignored, as
any other unmodelled construct is.

## 3. Three steps, in dependency order

**Step 1 — `class =` mints the nominal.** When `structure()`'s first argument
matches the representation a declared `@type` gives, and its `class` argument is a
string literal naming that type, the result has that nominal type. One rule, in
the `structure` handling that already exists. It converts the largest single source
of lost type information into the intended one, and needs no new notation.

**Step 2 — `inherits()` narrows.** The narrowing machinery already exists for
`is.null` and the `is.*` family; a class test is another entry in the same table,
keyed on the string literal. Then this checks, with the else branch knowing
`shape` is a `Square`:

```r
if (inherits(shape, "Circle")) pi * shape$r^2 else shape$s^2
```

Independently valuable: `inherits` guards are everywhere in R, written by people
who will never declare a kind.

**Step 3 — exhaustiveness.** Steps 1 and 2 make the branches correct but still
cannot say "you forgot `Triangle`". That needs a construct where the checker sees
every branch at once — the only part needing anything new, and the subject of §4.

The order is not arbitrary. Step 2 is useless without step 1, because nothing has
a kind to narrow. Step 3 is noise without step 2, because branch bodies would be
checked against the whole union rather than against the member.

## 4. The construct

```r
when (shape) {
  Circle -> pi * shape$r^2
  Square -> shape$s^2
}
```

The checker reports an error naming any member of `shape`'s union that no branch
covers. Inside a branch, `shape` has that member's type — step 2's narrowing,
applied per branch instead of per guard.

**Why `when`.** `match` is `base::match` and `switch` is `base::switch`; taking
either would shadow a function every R programmer uses. `when` and `case` are both
free in base R. `when` is the closest well-known name for this construct
(Kotlin's), and `case` reads like one branch rather than the whole analysis.
Either is defensible; `when` is the recommendation.

**Branches bind nothing.** An earlier sketch had `Circle(radius) -> ...`
destructuring the variant. Dropping that keeps `shape$r` as the way to read a
field — already checked, because `shape` is narrowed — and it has a consequence
out of proportion to the simplification, in §5.

### What it compiles to

```r
switch(class(shape)[1],
  Circle = pi * shape$r^2,
  Square = shape$s^2,
  stop("unhandled class: ", class(shape)[1])
)
```

`switch` on a string with named branches is R's own dispatch idiom, so the output
is ordinary R that an R programmer can read and step through in a debugger.

The `stop()` default is not decoration. R's `switch` returns `NULL` invisibly when
nothing matches, which is precisely the failure mode §1 is about. A value reaching
this dispatch with a class the checker never saw must fail loudly here rather than
return `NULL` to the caller.

## 5. Carrier: a function now, syntax later

Because branches bind nothing, `when` is **a call with named arguments**:

```r
when(shape, Circle = pi * shape$r^2, Square = shape$s^2)
```

R evaluates arguments lazily, so only the selected branch runs — the semantics of
the block form, with no new syntax. The capability therefore does not depend on a
dialect:

- **As a library function** in a small R package, files stay ordinary `.R`: no
  build step, no rewrite, every other R tool unaffected.
- **As dialect syntax**, the block form compiles to the same `switch`.

The checker rule is the same either way, and it should not be hardcoded to a name.
The corpus already carries this class of fact as a declaration attribute —
`@masked` marks a data-masking function, and the checker reads that from the stub
rather than knowing dplyr's verb names. The same mechanism serves here:

```
when : @exhaustive fn(x: Any, ...: Any) -> Any
```

meaning "the named arguments are branches over the first argument's union; report
any member none of them covers". One general rule, names as data, and a project
can declare its own dispatcher the same way.

**What the library form cannot do** is restrict construction: nothing stops
someone building the list by hand and skipping `structure()`. In a gradual checker
with `Any` as a sanctioned escape hatch and `Unknown` wherever a construct is
unmodelled, that is a real hole and a small one.

## 6. What a dialect would add beyond this

If steps 1–3 land, the remaining case for a separate language is narrower than it
first looks:

- **Types in the code** rather than in `#:` comments — ergonomics, not capability.
- **Record and tuple constructors** whose construction sites are checked.
- **Construction that cannot be bypassed**, a constructor being syntax rather than
  a callable function.

Against that: a `.Rt` file is not R. Until it is compiled, roxygen2 does not
document it, `devtools::load_all()` does not load it, RStudio does not highlight it
and CRAN will not take it; every contributor who touches the typed source needs
this tool installed. TypeScript won this trade, but its users already ran a build
step for every project and R package authors mostly do not. That difference is the
whole risk, and it is a judgement about R programmers rather than about compilers.

One corner escapes almost all of it: **standalone scripts**. `roughly run
script.Rt` can type-check, compile in memory and execute through the R runtime the
REPL already embeds — no generated file, no packaging, no collaborator toolchain.
If a dialect ships at all, that is where it is cheapest, and it is the corner the
existing prior art does not occupy.

**Extension, if it happens: `.Rt`.** Every extension in R's family puts the `R`
first — `.Rmd`, `.Rnw`, `.Rd`, `.Rout`, `.Rproj` — and `.Rt` follows that while
being unclaimed by R's own tooling. `.tR` reverses the convention and reads as a
typo of `.R`; `.TypedR` is self-documenting, but no language ships its full name
as an extension. Stub files stay `.Rtypes`.

## 7. Recommendation

Do steps 1 and 2 now. They fix wrong answers on unmodified code, need no new
notation, and are prerequisites for everything after. Then ship `when` as a
library function carrying the `@exhaustive` attribute. Decide on a dialect
afterwards, with the capability already delivered and no longer arguing for it —
and if one is built, start with scripts.

## 8. Prior art

**`we-data-ch/typr`** is a typed language for R, written in Rust, transpiling to
R, extension `.ty`. Two of its choices are instructive.

It is a sibling language rather than a superset: `fn` replaces `function`,
statements end in semicolons, booleans are lowercase. An existing R file cannot
adopt it by adding anything — it must be rewritten. That spends the property this
project treats as its core asset: useful answers about R nobody has modified.
Being fair about what the break buys, method-style calls (`alice.is_minor()`) are
the best discovery mechanism a typed language has, and **we cannot have them,**
because `.` is an ordinary character in R identifiers — `alice.is_minor` is
already a legal variable name.

Its type reasoning runs on Prolog: SWI-Prolog is a required install, and it emits
an `adt.pl` carrying the reasoning. That is expressive, bought with unbounded
search, an external build dependency and no incremental path; it cannot become an
editor-latency language server without being replaced. That is the road the
fast-to-check rule in `decisions.md` declined on principle, now observable in
practice. Its own README calls the project a buggy prototype, and the
architectural point stands separately from its maturity.

Worth copying: it leads with package-authoring experience rather than with types.

**Other work.** R packages that check types at run time (`typed`, `checkmate`,
`dgkf/typewriter`) show the notation is welcome and confirm the gap — none can say
anything before the line executes. The paper *Towards a Type System for R*
(ICOOOLPS 2019) is the closest thing to a survey of which R idioms resist static
description; read it before step 3.

*Limit of this survey: typr's documentation site could not be read, so its
treatment of exhaustiveness is unconfirmed.*

## 9. Open questions

- `when` versus `case`.
- Whether `structure()` mints a nominal only for a literal class string, or also
  when the class comes from a variable the checker can trace.
- Whether a branch that opts out of exhaustiveness (`_ ->`, or `.default =` in the
  call form) is worth having, or whether opting out should mean writing `switch`
  directly.
- Whether a class vector longer than one entry should be reported under strict
  mode rather than silently ignored, since it means something this type system
  does not model.
- Whether the script dialect (§6) is worth scheduling at all, and if so whether it
  precedes or follows a package story.
