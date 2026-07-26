---
title: Authoring stubs
description: The .Rtypes declaration format, overload sets, export manifests, and the scope rules for what belongs in the corpus
---

:::note[Status]
The standard-library stub format ships. The corpus is ~930 typed declarations across eleven
declaration-only `.Rtypes` stub files in the repository's top-level `types/` directory (`base`,
`stats`, `utils`, `methods`, `graphics`, `grDevices`, `datasets`, plus the conditional
`data.table`, `dplyr`, `ggplot2` and `testthat`), alongside generated `.exports` [manifests](#export-manifests) covering
every namespace R ships (the attached ones, the `::`-only ones such as `tools` and `parallel`, and
the conditional packages) plus fifteen manifest-only CRAN namespaces (the tidyverse and its
meta-package, `knitr`, `rlang`, `glue`, `jsonlite`, `R6` and friends) whose export sets keep
unresolved-name detection alive, all loaded and bound into the checker as a **set-once input** that
never invalidates a package edit (see [Incremental hygiene](#incremental-hygiene)). Project
[overrides](#override-precedence), [overload sets](#overloads-and-generics), and `pkg::name`
qualified access (with unknown-namespace warnings and not-exported errors) are supported. What is not
built is listed at the [end of this page](#what-is-not-built-yet). The authoritative typing
contract remains the [Typing Reference](/reference/type-system); this page describes how the standard
library feeds that contract.
:::

## Why stubs exist

Only a small kernel is built into the checker: the operators, plus `c` and `list`. Without stubs,
everything else in the standard library would resolve to `Unknown` — `T`/`F`/`pi` untyped, and
`length`, `nchar`, `seq_len`, `paste` and the rest with no signatures, so every call through them
would lose all type information.

The **stub format** closes that gap: declaration-only descriptions of base and other R packages, loaded
as immutable inputs, so the checker knows the standard library the same way rust-analyzer knows
`core`/`std`.

## Stub format

Stub files are **dedicated declaration-only files** with the extension `.Rtypes` ("R type information").
Each non-blank, non-comment line is a declaration:

```
name : <type-expr>
```

The type expression reuses the `#:` annotation type grammar verbatim — the same parser reads both —
so there is no second type notation to build or keep from drifting.
A stub file has no place to write a function body, so "declaration-only" is enforced *structurally* —
the way TypeScript `.d.ts` and Python `.pyi` are declaration-only by construction, not by convention.
Blank lines and `#` comments (whole-line or trailing) are ignored. The loader harvests each type
expression directly into a `TypeScheme`.

A declared `name` is an R identifier, an infix operator (`%in%`, a project's own `%||%`), or an S3
**operator method** — `+.Date`, `-.POSIXct`, `Arith.difftime`, `Compare.Date`, `Ops.gg`. The operator
spellings are how a stub gives a class arithmetic or comparison: see
[operator methods on a class](/reference/type-system#arithmetic-operators) for the dispatch order. The
suffix is the nominal's own name, so `+.ggplot : fn(e1: ggplot, e2: Any) -> ggplot` in a project stub
is all a `+`-based DSL needs.

```
# base.Rtypes — a fragment

# plain value bindings
T : logical
F : logical
pi : double

# a fixed-arity function signature
length : fn(x: Any) -> integer

# a parametric higher-order function — a real generic scheme, not Any
lapply : <T, U> fn(x: list[T], f: fn(T) -> U) -> list[U]
```

### Why a dedicated declaration format

The earlier approach shipped stubs as ordinary R files with placeholder bodies
(`length <- function(x) 0L`) whose `#:` annotation was harvested while the body was ignored. That let a
stub carry a meaningless, unreachable function body and required a full parse plus lowering just to
reach the annotation. A dedicated declaration file removes both problems: a body is unrepresentable, so
a stub cannot drift into carrying one, and the loader parses only declarations.

The extension `.Rtypes` evokes R's own `.Rd` documentation convention (R-something) without colliding with
`.Rd`, `.Rmd`, or `.Rda`/`.RData`. A JSON / TOML stub grammar was rejected because it would need a
second type notation; reusing the `#:` type grammar keeps one source of truth for type syntax across
inline annotations and stub files.

Besides value declarations, a line of the form `@type NAME` declares an **opaque nominal type** — a
named type with no inspectable representation, for standard-library values the type grammar cannot
describe structurally (`data.frame`, `factor`, `connection`, `Date`, ...). An opaque nominal is
compatible only with itself; values of it come only from functions declared to return it, so
`Sys.Date()` is a `Date` and passing it where `factor` is expected is a type error. Type names may
contain interior dots (`data.frame`), and a project's own `@type`/`@alias` of the same name shadows
the stub type. Consumers of these values keep `Any` parameters (R coerces liberally); the nominals
tighten *returns*. `$`, `[`, and `[[` on an opaque nominal yield `Unknown` instead of erroring —
`df$amount` and `df[rows, ]` stay usable, with each access surfaced under strict mode (see the
[Typing Reference](/reference/type-system#nominal-types)). Structural `@type NAME {REPRESENTATION}`
declarations are not expressible in `.Rtypes` — opaque nominals are the stub form.

### Cross-ecosystem note

Statically-typed hosts that publish types for foreign code use **separate** declaration files —
TypeScript `.d.ts`, Python `.pyi`, Sorbet `.rbi`. Dynamically-typed hosts retrofitting types onto their
*own* source put them **inline** — Elixir/Erlang `@spec`. R does both: inline `#:` is primary for a
project's own source, and separate `.Rtypes` files exist for foreign packages (base and CRAN) that cannot
be annotated at the source. `.Rtypes` reuses the same type grammar as the inline form, so the two are one
notation in two carriers.

### Masked (data-masking) functions

A declaration may carry the `@masked` attribute before its function type:

```
summarise : @masked fn(.data: Any, ...: Any) -> Any
```

It marks the function as evaluating its rest-absorbed arguments in a data mask (dplyr-style
non-standard evaluation): bare names in those argument positions are treated as column
references — no unresolved-name warnings — while arguments matching declared formals resolve
normally. `@masked` requires a variadic function type. See the typing reference's
"Data-masked evaluation (NSE)" section for the full semantics.

## Overloads and generics

**Repeating a name declares an ordered overload set.** Each further declaration of the same name
*within one source* appends a candidate:

```
sum : fn([na.rm]: logical, ...: integer[] | logical[]) -> integer
sum : fn([na.rm]: logical, ...: double[]) -> double
sum : fn([na.rm]: logical, ...: Any) -> Any
```

A call to the name commits the **first candidate that accepts the arguments** (so `sum(1L, 2L)` is
`integer` and `sum(1.5, 2.5)` is `double`); the call-site selection rules — probe isolation, the
fact-over-guess rule for arguments whose type is still undetermined, the two-round literal courtesy,
and the no-match error — are specified in the Typing Reference under
[Overload sets](/reference/type-system#overload-sets). **Order each set most-specific first and end it
with the most general candidate** — conventionally an `Any` fallback, which keeps mixtures the
candidates cannot express (`sum(TRUE, 1L)`) from erroring, and which a non-call use of the name
(passing it as a value) resolves to, so a value never carries a narrower contract than the calls the
same name accepts. A project override that redeclares a name **replaces its whole set**; an override
that wants overloads declares all of them itself.

The editor shows the whole set: signature help on a call lists every declared candidate — the
committed one active and rendered with that call site's types filled in, the rest with their declared
type parameters. An incomplete call matches no candidate yet and still lists the set, which is when it
helps most. Hover on the name shows the signature of the committed candidate, or of the last
declaration when the name is not being called, plus a `(+N overloads)` note; go-to-definition points
at the first declaration.

Three rules govern the current corpus:

- **Genuinely parametric functions get real generics.** Higher-order helpers whose result is a function
  of the argument *type* (`lapply`, `Reduce`, `print`, `invisible`, `setNames`, `suppressWarnings`, ...)
  are written with `<T> fn(...)` binders and keep precise polymorphic schemes. Element-preserving
  vector functions (`rev`, `sort`, `unique`, `head`, `tail`, `sample`, `rep`, set operations) use
  `<T> fn(x: T[]) -> T[]` — the element parameter is constrained to atomic types — usually as the
  first candidates of an overload set whose list form threads `list[T]` and whose `Any` fallback
  covers the rest.
  - **A function that hands back the shape it was given declares each shape, narrowest first.** R's
    selection and reordering operations go through `[`, which preserves both the atomic type and the
    names, so the declaration says so: `Filter` takes `T[]`, then `list[named: T]`, then `list[T]`,
    each returning its own shape — one `list[T]` return for all three would make `Filter(f, c(1, 2)) + 1`
    a type error on code R runs. `lapply`, `rev`, `unique`, `head` and `tail` carry the same
    `list[named: T]` candidate ahead of their plain list one, so a named list in is a named list out.
    A *fixed-shape* input still coerces to a name-keyed list on the way in, so a field read off the
    result is `T | NULL`: the operation may drop a name, and the exact field types are not carried
    through.
- **Type-preserving reductions get overload sets.** The reductions whose result's atomic type follows
  R's coercion order rather than one input's (`sum`/`min`/`max`/`range`/`pmin`/`pmax`,
  `cumsum`/`cummax`/`cummin`, `abs` with its integer/double split) declare one candidate per atomic
  family plus the `Any` fallback.
- **Value-dependent results fall back to `Any`.** A function whose return type varies by argument
  *value* or by arity (`seq`, `is`, `grep(value =)`) is given `Any` rather than a falsely-precise
  signature, so a call yields `Any` and never a spurious type or arity error. The name still
  resolves.

## Override precedence

A project can override or extend the shipped stubs by dropping `.Rtypes` files under `<project>/stubs/`.
The loader folds project sources over the shipped corpus in sorted path order, so a declaration a
project supplies **replaces** the shipped declaration of the same name — a project can correct a return
type or add a name the shipped corpus omits. A missing directory, an unreadable file, or a malformed
line is skipped: overrides are optional and one bad line must never block analysis.

**A project stub file's name declares its namespace**: `stubs/dplyr.Rtypes` declares the namespace
`dplyr`, so its declarations type bare reads *and* validate qualified reads — `dplyr::mutate` is a
known-namespace read with the stub's type, while `dplyr::filter` warns "not exported" until the file
declares `filter`. This is how a project types a third-party package today (hand-written; automatic
generating them from an installed library is not built). Exports are declaration-level, not winner-level: a project
file overriding a shipped name's type does not remove the name from its shipped namespace, so
`stats::sd` stays valid under an `sd` override. Hover shows a name's origin as the namespace of its
winning declaration.

Skipped never means silent. `ry check` reports every dropped override declaration as an error
on its stub line (a line that fails to parse, or a declaration naming an unresolvable type) and
treats an unreadable override file as an I/O failure, and the editor shows the same problems as
diagnostics while a `.Rtypes` file is open.

## Export manifests

Base alone exports ~1,400 names; the typed corpus deliberately curates a high-value subset. Without
more, every real export outside that subset would warn as unresolved — the checker could not tell
`recover` (a real `utils` export) from a typo. The **export manifests** close that gap: each shipped
namespace's `.Rtypes` file is paired with a `types/<namespace>.exports` file listing every name the
namespace really exports, one per line, generated from a live R session by
`scripts/export-manifests.R` (the header records the R version; rerun the script against a newer R to
refresh them — it refuses to overwrite a manifest recorded against a *newer* R than the session's,
because regenerating on an older one silently drops the names the newer version added and turns every
use of one into a false `unresolved`).

A manifest does not need a `.Rtypes` file beside it. **Manifest-only namespaces** — `tibble`,
`tidyr`, `readr`, `purrr`, `stringr`, `forcats`, `lubridate`, `magrittr`, `rlang`, `glue`, `scales`,
`knitr`, `jsonlite`, `R6`, `tidyverse` — carry no typed declarations at all, so every name from them
is `Unknown`. What they buy is a **knowable export set**, and that is worth more than it sounds:
attaching a package whose exports the checker cannot enumerate turns off unresolved-name detection for
the whole project (see [the tolerance](/reference/type-system#naming-and-scoping)), so before these manifests
existed a single `library(stringr)` meant a clean run said nothing about typos anywhere. Adding
declarations to any of them later is additive — the manifest stays the export set, the `.Rtypes` file
supplies types.

Manifest names are known globals with the weakest possible claim:

- a bare or `pkg::`-qualified read of a manifest name **always resolves** — no unresolved or
  not-exported warning — typing as the `.Rtypes` declaration when one exists and `Unknown`
  otherwise, so precision is the typed corpus's job and *silence about real names* is the
  manifest's
- completion offers manifest-only names (undecorated — there is no scheme to show; non-syntactic
  names are skipped bare and backtick-quoted after `pkg::`, since inserting them raw would change
  the syntax), and typo suggestions draw on them
- the unit suite pins the pairing both ways: every value declaration in a shipped `.Rtypes` file
  must appear in its namespace's manifest (a stubbed non-export is a hard failure — it catches
  declarations added to the wrong file, and names R moves between namespaces, e.g. `traceback`
  and `standardGeneric` live in `base`, not `utils`/`methods`), while a conditional namespace may
  additionally override a base name (data.table's class-preserving `merge`)

The manifests mirror how R exposes each namespace, in three tiers:

- **default-attached** (`base`, `stats`, `utils`, `graphics`, `grDevices`, `methods`,
  `datasets`): bare-visible everywhere, so their names resolve unconditionally (`datasets` is
  manifest-driven via the search-path listing — its objects are lazy data, not namespace
  exports — with the famous data frames typed in `datasets.Rtypes`, so `iris` is a
  `data.frame`, not `Unknown`)
- **R-shipped but unattached** (`tools`, `parallel`, `compiler`, `grid`, `splines`, `stats4`,
  `tcltk`): `pkg::` reads validate in every project — `tools::file_ext(path)` needs no
  `library(tools)`, exactly as in R — while bare reads resolve only once the project attaches
  or declares the package
- **conditional** (`data.table`, `dplyr`, `ggplot2`, `testthat`, plus every manifest-only CRAN
  namespace): manifest and stubs activate together
  ([Conditional namespaces](#conditional-namespaces)); inactive packages' names warn, bare and
  qualified alike. A **meta-package** activates the members it attaches: `library(tidyverse)`
  re-exports almost nothing itself and instead attaches `dplyr`, `ggplot2`, `tibble`, `tidyr`,
  `readr`, `purrr`, `stringr`, `forcats` and `lubridate`, so those nine activate with it — which is
  also how such a project gets dplyr's and ggplot2's *typed* declarations rather than a manifest's
  `Unknown`. The membership is checkable: the generator script prints what a live
  `library(tidyverse)` attaches

The manifests vendor R's export lists so analysis never needs an R installation; they are data, not
types — adding a manifest name costs nothing at check time until code actually reads it.

## The base environment

**One source of truth for everything scheme-shaped, plus a small justified hardcoded kernel.**

- **Value bindings.** `T`/`F` → monomorphic `Scalar(Logical)`; `pi` → `Scalar(Double)`. This closes
  the long-standing `T`/`F`/`pi` gap recorded in memory.
- **Base functions.** Each becomes a type scheme seeded into the template environment alongside the
  built-in kernel.

### What stays hardcoded, and why

The operators and `c`/`list` are an **algorithm, not a type**:

- numeric promotion lattice and comparison families for the operators;
- `c`'s variadic, `NULL`-dropping atomic promotion;
- `list`'s record/tuple synthesis.

The `#:` grammar cannot express these, so they stay built in. The "one source of truth" goal is met
for everything a *type* can describe; the kernel is an explicit, irreducible carve-out — documented
as such, not an oversight.

### Expressiveness gaps base functions expose

Two extensions that a faithful corpus needs have landed:

| Extension | Example | Form |
|-----------|---------|------|
| Variadics | `paste`, `sum`, `cat` | trailing `...: TYPE` rest parameter (`fn(...: Any) -> character`) |
| Dotted parameter names | `na.rm`, `length.out` | interior `.` allowed in parameter and field names |

The gaps below remain; each caps how precise the affected declarations can be:

| Gap | Example | Extension needed |
|-----|---------|------------------|
| Trailing-dot parameter names | `stop(call. =)`, `warning(immediate. =)` | parameter names currently allow interior dots only |
| Named-into-rest absorption | `data.frame(x = 1)`, `Sys.setenv(VAR = "v")`, `par(mfrow = ...)` | the checker never routes a named argument into `...`, so arbitrary-named-argument sinks must stay `Any` values |
| `Never` type | `stop`, `q` | without it, a `NULL` return claim would poison `x <- if (ok) v else stop(...)` joins, so these stay `Any` |
| Shape-mirroring returns | `rev(opts)$timeout` on a fixed-shape `opts` | a declaration cannot say "the same record back", so a selection or reordering returns a name-keyed `list[named: T]` and a field read off it is `T | NULL` rather than the field's own type |
| Nullable results under member-wise operators | `names`, `dim`, `nrow` | `T | NULL` returns false-positive on `1:nrow(df)` / `for (nm in names(x))` until flow narrowing or NULL-tolerant joins exist, so these return `Any` |

Three former rows of this table have closed. The type-preserving reductions declare
[overload sets](#overloads-and-generics), and the element-preserving functions (`rev`, `sort`,
`unique`, `head`, `tail`, `sample`, `rep`, the set operations) declare generic `T[]` signatures —
the `T[]` suffix carries the atomic-element bound specified in the Typing Reference under
[Type parameters](/reference/type-system#type-parameters-and-generic-application). Function compatibility
also stopped demanding a matching parameter count: a function serves a callback interface when it
accepts every call shape the interface promises, so spare *optional* formals are fine and a
callback-idiom stub can declare its real signature (`lapply(words, nchar)` works with `nchar`'s
display formals declared). Shape-mirroring functions whose *atomic* changes with the input (`nchar`,
`toupper` returning the input's shape) remain scalar-claim.

## Loading and namespacing

Stubs are **immutable, set-once inputs** — analogous to rust-analyzer's `Durability::HIGH`. They are
loaded, parsed, and interned once, and are never invalidated by ordinary user edits. The one
deliberate exception: the assembly also reads the package-metadata input, which activates the
[conditional namespaces](#conditional-namespaces) — a metadata flip (declaring or attaching such a
package) rebuilds the library, which is rare and worth the full refresh it causes.

The `StubLibrary` is a **flat set-once map**: `values: Symbol -> StubValue`, where each entry pairs
the name's ordered scheme list (one scheme per declaration — see
[overload sets](#overloads-and-generics)) with the declaration's source range and the namespace of
its winning declaration (`base`, `stats`, ..., or a project stub's file stem) for hover display.
Typing is not partitioned by namespace; alongside the flat map, a per-namespace export table
(namespace → declared names, built from every source) answers `pkg::name` validation, so an
override winning a name's type never un-exports it from its declaring namespace.

- Every shipped `.Rtypes` file is harvested into the one flat map, folded in file order — a later
  *source* redeclaring a name replaces its whole entry (the same rule that governs project
  overrides), while repeated declarations *within* one source build the name's overload set. The
  seven default-attached namespaces (`base`, `stats`, `utils`, `methods`, `graphics`, `grDevices`,
  `datasets`) are thus attached to the base scope together; conditional namespaces join the fold only when
  active.
- The flat map is seeded into the per-document inference template, so every stub name resolves as a
  bare global regardless of which shipped file declared it. A name's first scheme becomes its plain
  environment binding; a multi-scheme name additionally registers its overload set for call-site
  selection.
- `pkg::name` resolves against the same flat map: the qualified read has the stub's type exactly
  like the bare name, and the per-namespace export table powers the validation warnings (unknown
  namespace; name not exported by that namespace) specified in the Typing Reference under
  [Namespace access](/reference/type-system#namespace-access).

### Conditional namespaces

Some shipped namespaces describe packages R does **not** attach by default — `data.table`, `dplyr`,
`ggplot2` and `testthat` with types, and the [manifest-only](#export-manifests) CRAN set without.
Folding them in unconditionally would let `fread`, `mutate` or `str_to_upper` resolve (and steal a
typo warning) in projects that never use them, so a conditional namespace joins the assembly only
when the project uses the package:

- a `DESCRIPTION` dependency field (`Depends`/`Imports`/`Suggests`/`Enhances`) names it, or any
  `NAMESPACE` `import`/`importFrom` uses it as a source, or
- any project file attaches or loads it — a `library()` / `require()` / `requireNamespace()` /
  `loadNamespace()` call whose package argument is a literal name or string, anywhere in the file
  (the signal script workspaces have, since only packages carry metadata files), or
- the project ships its own `stubs/<pkg>.Rtypes` override for the namespace — writing one is
  itself the clearest declaration that the project uses the package, and the override then folds
  over the shipped declarations as usual, or
- a **meta-package** that attaches it is active: `library(tidyverse)` activates the nine packages it
  attaches, because attaching is how R makes their names reachable.

`testthat` earns its place for a different reason than the data packages: a package's `tests/`
directory is a third of its source and every line of it calls these names, so without the corpus the
only way to quiet them was the blanket tolerance an unknowable export set earns — which also silenced
typos in the package's *own* functions. With the stubs, `expect_eqaul` is reported with a suggestion.
Expectation `object` parameters are `Any` (an expectation compares whatever the code under test
produced), and each `expect_*` returns its `object`, which is the value testthat passes through
invisibly.

While inactive the namespace is simply absent: its names warn as unresolved, `pkg::` reads behave
like any stub-less package, and its `@type` nominals are unknown type names. The hosts keep the
attach facts current per synced file (the language server re-scans only the changed document and
fills the rest of the workspace in during idle priming), so activation never costs a whole-project
sweep on a keystroke.

### NAMESPACE import validation

Both `ry check` and the language server validate a package's `NAMESPACE` file against the
loaded stubs: every `importFrom(pkg, name)` whose namespace the stub corpus knows (shipped or
project) but does not export `name` is an **error** on the import site (`` `medain` is not exported
by `stats`, so this package will not load. ``) — R refuses to load a package with such an import, so
the code cannot run at all, exactly like the `export()` of a name the package never defines.
Namespaces without stubs are not checked — the stubs are the only export source the
checker has — and resolution semantics are unchanged: stubbed names resolve bare with or without an
import. The check is interner-free (a string-keyed export table built once from the loaded corpus),
so an open `NAMESPACE` editor buffer is validated live without touching the engine's shared
interner — the same isolation the `.Rtypes` stub-buffer path keeps.

**Not yet built (future namespacing):**

- A per-namespace map (`namespace -> {Symbol -> scheme}`) that keeps the shipped packages separate rather
  than folded flat (today two shipped packages cannot declare the same name with different types).
- `library(pkg)` attaching a namespace on demand.
- Gating bare third-party resolution on NAMESPACE imports (R-correct, but a deliberate strictness
  decision — today's flat fold is intentionally permissive).

### Incremental hygiene

In the [query engine](/contributing/architecture#the-semantics-database) the stub library is a
**set-once input**: it is established once and its revision never advances, so it can never invalidate
a query that reads it. That is the entire isolation property — a stub never triggers recomputation
because it never changes — and it is automatic, not something a separate check must enforce.

A package binding that *shadows* a stub name is an ordinary structural edit: it changes the package
symbol index's winner for that name, which flows through the per-symbol interface to exactly the files
that reference the name — the same path as any other definition appearing or disappearing. A stub
value and an unrelated user type that share a name (`#: @type T` versus the value `T`) live in the
type and value namespaces respectively and never interact.

## Scope discipline: static-reasonable only

**Modeled:**

- fixed-arity signatures;
- scalar constants;
- common vectorized atomic ops with a stable result shape;
- prenex rank-1 generics;
- nominal type/class declarations.

**Deliberately not modeled** (declare the static-stable signature and stop, or use `Any`):

- NSE / data-masking — `subset`, `with`, formulas;
- `...` forwarding tricks;
- S3/S4/R5/R6 dispatch dynamism — `UseMethod`, `NextMethod`;
- `do.call`, `match.arg`, `match.call`, `sys.call`, `Recall`;
- reflective env access — `environment`, `assign`, `get`, `eval`;
- partial argument matching;
- replacement functions — `` `names<-` ``.

**Rule:** if a function's return type is not a static function of its argument *types* (only of runtime
values or classes), omit it or give `Any`.

### Corpus compromise vocabulary

The corpus optimizes for (1) zero false errors on idiomatic calls, then (2) the most precise sound
return. The recurring compromises are named once in the `base.Rtypes` header and referenced per entry:

- **Any-param** — a parameter is `Any` although R documents a type: R itself coerces argument types
  (`nchar(42)` is legal), so a declared type would reject calls R accepts. The checker's coercions
  (`integer` widens to `double` at parameter positions, whole-number double literals count as
  `integer`, scalars coerce into vector positions) shrink the need for this, and the `T[]` generic
  design restores the rest of the parameter precision.
- **scalar-claim** — an elementwise (vectorized) function declares its scalar result form
  (`character`, not `character[]`): a scalar claim coerces into every vector position and can never
  false-positive downstream, while a vector claim would break `if (grepl(...))`.
- **type-preserving** — the result's type follows the input's (`sum`, `sort`, `rev`). Element-preserving
  vector functions declare `<T> fn(x: T[]) -> T[]`; reductions whose result follows R's coercion order
  declare one [overload candidate](#overloads-and-generics) per atomic family. Each set ends with an
  `Any` fallback, which is also what a call selects when an argument's type is still undetermined —
  a fallback taking `Any` binds nothing, so it never narrows the caller.
- **NULL-hybrid** — the result is `T`-or-`NULL` depending on the runtime value (`names`, `dim`,
  `nrow`); returns `Any` (see the gaps table).
- **named-formals** — declare the named formals whose *types* are worth checking (`paste`'s
  `sep`/`collapse`, `read.csv`'s `header`/`sep`). An undeclared named argument to a variadic
  function is not an error: it is absorbed by the rest parameter and checked against its element
  type (the checker's named-into-rest rule, matching R), so open named-argument spaces
  (`read.csv`'s pass-through to `read.table`, `lm`'s fitting controls) work with `...: Any` —
  declaring a formal buys a typed check, omitting it buys pass-through.

**Two-tier dynamic marker** (borrowed from typeshed): keep a real `Any` for genuinely untypeable
returns *distinct from* a greppable "incomplete / not-yet-typed" marker. The distinction makes partial
stubs first-class and improvable, and lets tooling find what still needs work — mirroring typeshed's
`Incomplete`-vs-`Any` split.

## Worked example

A base stub fragment:

```
T : logical
F : logical
pi : double
length : fn(x: Any) -> integer
nchar : fn(x: Any) -> integer
seq_len : fn(length.out: Any) -> integer[]
```

The schemes each produces:

| Binding | TypeScheme |
|---------|------------|
| `T`, `F` | `Scalar(Logical)` |
| `pi` | `Scalar(Double)` |
| `length` | `fn([], [x: Any], Integer)` |
| `nchar` | `fn([], [x: Any], Integer)` |
| `seq_len` | `fn([], [length.out: Any], Vector(Integer))` |

(`nchar`'s subject and `seq_len`'s count are Any-param — see the
[compromise vocabulary](#corpus-compromise-vocabulary) — so `nchar(42)` and `seq_len(10)` check clean
while the returns stay precise.)

R that type-checks against these stubs plus the hardcoded kernel:

```r
n    <- length(c(1L, 2L, 3L))  #: integer  (stub scheme)
half <- pi / 2                 #: double   (operator kernel on Scalar(Double))
flag <- T                      #: logical  (stub value binding)
```

This demonstrates stub schemes and the hardcoded kernel interoperating: `length`'s scheme types `n`,
the operator kernel promotes `pi / 2`, and the `T` value binding types `flag`.

`paste`'s variadics, its `sep`/`collapse` named formals, and `length.out`'s dotted parameter name are
expressible directly
(`paste : fn(...: Any, [sep]: character, [collapse]: character | NULL, [recycle0]: logical) -> character` — the rest parameter's position among the named ones is part of the signature, because it decides which parameters a positional argument can still fill).
One gap remains, degrading returns to the scalar-claim:

- `nchar`-style shape mirroring where the result *atomic* differs from the input's (the result
  shape should track the input vector shape); `rev`-style element preservation is covered by the
  generic `T[]` suffix.

## Per-edit cost

Because the stub library is a [set-once input](#incremental-hygiene), its mere presence adds no per-edit recheck cost: an
edit never invalidates it, and a body recheck still touches only the edited document and its referrers.
A document pays inference time only for the base names it actually references — that is the feature
working, not bookkeeping overhead — and a document that references no base names pays nothing for the
corpus being loaded.


## What is not built yet

Two extensions are designed but unbuilt, and their absence is visible:

- **Third-party packages beyond the shipped set.** The corpus covers the namespaces R itself
  ships, plus conditional stubs for `data.table`, `dplyr`, `ggplot2` and `testthat`. Any other
  CRAN package's names resolve to `Unknown` with an unknown-namespace warning. A project can
  close the gap for itself today by writing its own `stubs/<pkg>.Rtypes`; generating them by
  introspecting an installed library is the intended long-term answer, because which packages
  and versions are installed is a property of the project, not of ry.
- **R-version keying.** Base evolves across releases — functions appear, signatures change — and
  the shipped corpus describes one snapshot rather than the version a given project runs.
