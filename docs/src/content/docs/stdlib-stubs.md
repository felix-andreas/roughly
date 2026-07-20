---
title: Stdlib Stubs
description: The standard-library stub format (.Rtypes declaration files) that teaches the type checker base/stats/utils instead of resolving them to Unknown
---

:::note[Status]
The standard-library stub format ships. The corpus is ~530 declarations across six declaration-only
`.Rtypes` stub files under `crates/semantics/stubs/` (`base.Rtypes`, `stats.Rtypes`, `utils.Rtypes`,
`methods.Rtypes`, `graphics.Rtypes`, `grDevices.Rtypes`), all loaded and bound into the checker as a
**set-once input** that never invalidates a package edit (see
[Incremental hygiene](#incremental-hygiene)). Project [overrides](#override-precedence),
[overload sets](#overloads-and-generics), and `pkg::name` qualified access (with unknown-namespace
and not-exported warnings) are supported. Still **proposed / not yet built**: the CRAN tier
(per-project introspection, §7), R-version keying of the embedded corpus (§8), the stubtest CI
validator, and structural (non-opaque) `@type` declarations in `.Rtypes`. Sections below mark which
is which. The authoritative typing contract remains the
[Typing Reference](/typing-reference); this note describes how the standard library feeds that
contract.
:::

## Problem

The checker's only hardcoded "library" is roughly 19 `BuiltinKind` entries: the operators plus `c` and
`list`. Without stubs, everything else in the standard library resolves to `Unknown`: `T`/`F`/`pi` are
untyped, and base functions (`length`, `nchar`, `seq_len`, `paste`, ...) have no signatures, so calls
through them lose all type information.

The **stub format** closes that gap: declaration-only descriptions of base and other R packages, loaded
as immutable inputs, so the checker knows the standard library the same way rust-analyzer knows
`core`/`std`.

## 1. Stub format

Stub files are **dedicated declaration-only files** with the extension `.Rtypes` ("R type information").
Each non-blank, non-comment line is a declaration:

```
name : <type-expr>
```

The type expression reuses the `#:` annotation type grammar verbatim (parsed by the same
`parse_surface_type` entry point), so there is no second type notation to build or keep from drifting.
A stub file has no place to write a function body, so "declaration-only" is enforced *structurally* —
the way TypeScript `.d.ts` and Python `.pyi` are declaration-only by construction, not by convention.
Blank lines and `#` comments (whole-line or trailing) are ignored. The loader harvests each type
expression directly into a `TypeScheme`.

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
[Typing Reference](/typing-reference#nominal-types)). Structural `@type NAME {REPRESENTATION}`
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
unresolved-argument fallback to the last candidate, the two-round literal courtesy, and the no-match
error — are specified in the Typing Reference under
[Overload sets](/typing-reference#overload-sets). Non-call uses of the name (passing it as a
value) see the first candidate, so the corpus orders each set most-specific first and ends it with
the most general candidate — conventionally an `Any` fallback, which also keeps mixtures the
candidates cannot express (`sum(TRUE, 1L)`) from erroring. A project override that redeclares a name
**replaces its whole set**; an override that wants overloads declares all of them itself.

The editor shows the whole set: signature help on a call lists every declared candidate with the
committed one active, and hover on the name shows the committed candidate's signature (the primary
declaration when the name is not being called) plus a `(+N overloads)` note.

Three rules govern the current corpus:

- **Genuinely parametric functions get real generics.** Higher-order helpers whose result is a function
  of the argument *type* (`lapply`, `Reduce`, `print`, `invisible`, `setNames`, `suppressWarnings`, ...)
  are written with `<T> fn(...)` binders and keep precise polymorphic schemes. Element-preserving
  vector functions (`rev`, `sort`, `unique`, `head`, `tail`, `sample`, `rep`, set operations) use
  `<T> fn(x: T[]) -> T[]` — the element parameter is constrained to atomic types — usually as the
  first candidates of an overload set whose list form threads `list[T]` and whose `Any` fallback
  covers the rest.
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
CRAN generation is the proposed §7 tier). Exports are declaration-level, not winner-level: a project
file overriding a shipped name's type does not remove the name from its shipped namespace, so
`stats::sd` stays valid under an `sd` override. Hover shows a name's origin as the namespace of its
winning declaration.

Skipped never means silent. `roughly check` reports every dropped override declaration as an error
on its stub line (a line that fails to parse, or a declaration naming an unresolvable type) and
treats an unreadable override file as an I/O failure, and the editor shows the same problems as
diagnostics while a `.Rtypes` file is open.

## 2. Base-environment model

**One source of truth for everything scheme-shaped, plus a small justified hardcoded kernel.**

- **Value bindings.** `T`/`F` → monomorphic `Scalar(Logical)`; `pi` → `Scalar(Double)`. This closes
  the long-standing `T`/`F`/`pi` gap recorded in memory.
- **Base functions.** Each becomes a `TypeScheme` seeded into the template environment alongside the
  builtins (wired through `inference_state_with_builtins`).

### What stays hardcoded, and why

The operators and `c`/`list` are an **algorithm, not a type**:

- numeric promotion lattice and comparison families for the operators;
- `c`'s variadic, `NULL`-dropping atomic promotion;
- `list`'s record/tuple synthesis.

The `#:` grammar cannot express these, so the 19 `BuiltinKind` entries remain. The "one source of
truth" goal is met for everything a *type* can describe; the kernel is an explicit, irreducible
carve-out — documented as such, not an oversight.

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
| Extra-optional-tolerant function compatibility | `lapply(words, nchar)` breaks if `nchar` declares its optional formals | function compatibility requires matching parameter counts, so callback-idiom stubs must stay single-parameter |
| `Never` type | `stop`, `q` | without it, a `NULL` return claim would poison `x <- if (ok) v else stop(...)` joins, so these stay `Any` |
| Nullable results under member-wise operators | `names`, `dim`, `nrow` | `T | NULL` returns false-positive on `1:nrow(df)` / `for (nm in names(x))` until flow narrowing or NULL-tolerant joins exist, so these return `Any` |

Two former rows of this table have closed: the type-preserving reductions declare
[overload sets](#overloads-and-generics), and the element-preserving functions (`rev`, `sort`,
`unique`, `head`, `tail`, `sample`, `rep`, the set operations) declare generic `T[]` signatures —
the `T[]` suffix carries the atomic-element bound specified in the Typing Reference under
[Type parameters](/typing-reference#type-parameters-and-generic-application). Shape-mirroring
functions whose *atomic changes* with the input (`nchar`, `toupper` returning the input's shape)
remain scalar-claim.

## 3. Loading and namespacing

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
  six default-attached namespaces (`base`, `stats`, `utils`, `methods`, `graphics`, `grDevices`)
  are thus attached to the base scope together; conditional namespaces join the fold only when
  active.
- The flat map is seeded into the per-document inference template, so every stub name resolves as a
  bare global regardless of which shipped file declared it. A name's first scheme becomes its plain
  environment binding; a multi-scheme name additionally registers its overload set for call-site
  selection.
- `pkg::name` resolves against the same flat map: the qualified read has the stub's type exactly
  like the bare name, and the per-namespace export table powers the validation warnings (unknown
  namespace; name not exported by that namespace) specified in the Typing Reference under
  [Namespace access](/typing-reference#namespace-access).

### Conditional namespaces

Some shipped stubs describe packages R does **not** attach by default — today `data.table` and
`dplyr`. Folding them in unconditionally would let `fread` or `mutate` resolve (and steal a typo
warning) in projects that never use them, so a conditional namespace joins the assembly only when
the project uses the package:

- a `DESCRIPTION` dependency field (`Depends`/`Imports`/`Suggests`/`Enhances`) names it, or any
  `NAMESPACE` `import`/`importFrom` uses it as a source, or
- any project file attaches or loads it — a `library()` / `require()` / `requireNamespace()` /
  `loadNamespace()` call whose package argument is a literal name or string, anywhere in the file
  (the signal script workspaces have, since only packages carry metadata files), or
- the project ships its own `stubs/<pkg>.Rtypes` override for the namespace — writing one is
  itself the clearest declaration that the project uses the package, and the override then folds
  over the shipped declarations as usual.

While inactive the namespace is simply absent: its names warn as unresolved, `pkg::` reads behave
like any stub-less package, and its `@type` nominals are unknown type names. The hosts keep the
attach facts current per synced file (the language server re-scans only the changed document and
fills the rest of the workspace in during idle priming), so activation never costs a whole-project
sweep on a keystroke.

### NAMESPACE import validation

Both `roughly check` and the language server validate a package's `NAMESPACE` file against the
loaded stubs: every `importFrom(pkg, name)` whose namespace the stub corpus knows (shipped or
project) but does not export `name` warns on the import site (`` `medain` is not exported by
`stats`. ``). Namespaces without stubs are not checked — the stubs are the only export source the
checker has — and resolution semantics are unchanged: stubbed names resolve bare with or without an
import. The check is interner-free (a string-keyed export table built once from the loaded corpus),
so an open `NAMESPACE` editor buffer is validated live without touching the engine's shared
interner — the same isolation the `.Rtypes` stub-buffer path keeps.

**Not yet built (future namespacing):**

- A per-namespace map (`namespace -> {Symbol -> scheme}`) that keeps the shipped packages separate rather
  than folded flat (today two shipped packages cannot declare the same name with different types).
- `library(pkg)` attaching a namespace on demand (see §7).
- Gating bare third-party resolution on NAMESPACE imports (R-correct, but a deliberate strictness
  decision — today's flat fold is intentionally permissive).

### Incremental hygiene

In the [query engine](/architecture#incremental-analysis-the-query-engine) the stub library is a
**set-once input**: it is established once and its revision never advances, so it can never invalidate
a query that reads it. That is the entire isolation property — a stub never triggers recomputation
because it never changes — and it is automatic, not something a separate check must enforce.

A package binding that *shadows* a stub name is an ordinary structural edit: it changes the package
symbol index's winner for that name, which flows through the per-symbol interface to exactly the files
that reference the name — the same path as any other definition appearing or disappearing. A stub
value and an unrelated user type that share a name (`#: @type T` versus the value `T`) live in the
type and value namespaces respectively and never interact.

### Two required integration edits

1. Naming's `is_builtin_symbol` / resolution guard must accept stub names — otherwise resolution emits
   `could not resolve length`.
2. The typecheck non-local `Symbol` arm must fall back to the seeded base scheme before defaulting to
   `Unknown`.

## 4. Scope discipline — "static-reasonable only"

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
- **type-preserving** — the result's atomic type follows the input's (`sum`, `sort`, `rev`); the
  reductions declare [overload sets](#overloads-and-generics), and shape-mirroring names still
  return `Any` (never a falsely-precise `double`) until the `T[]` generic design lands.
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

## 5. Worked example

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
(`paste : fn([sep]: character, [collapse]: character | NULL, [recycle0]: logical, ...: Any) -> character`).
One gap remains, degrading returns to the scalar-claim:

- `nchar`-style shape mirroring where the result *atomic* differs from the input's (the result
  shape should track the input vector shape); `rev`-style element preservation is covered by the
  generic `T[]` suffix.

## 6. Sequencing and risks

### Sequencing

The stub library plugs into the query engine as a set-once input, so it adds no per-edit recheck
cost: its revision never advances, and a file that references a stub records that dependency like any
other input it reads.

**Smallest first increment:** seed `T`/`F`/`pi` plus ~12 high-frequency base functions, parsed once
from an embedded `base.Rtypes`, behind the template-seeding path — before any namespace / `::` / `library()`
machinery. This closes the `T`/`F`/`pi` gap with only the two integration edits from §3.

### Risks

- **Type-syntax expressiveness.** Variadics, dotted parameter names, overload sets, and the generic
  `T[]` suffix have landed; functions whose result *atomic* changes with the input's shape
  (`nchar`) remain scalar-claim.
- **The operator/`c` kernel cannot migrate.** One-source-of-truth is necessarily partial.
- **Corpus size.** Base alone is ~1400 exports. Curate a high-value subset; treat stubs as living
  docs with real drift risk. The full validation tool is a **stubtest-equivalent** that introspects real
  R signatures via `formals()` / `getNamespaceExports()` and diffs them against the `#:` annotations
  (R-dependent, future — §7-9). A **name-level** slice of it already ships and runs in the ordinary unit
  suite (no R): `tests/test_stdlib_exports.rs` diffs the names each corpus file declares against a
  checked-in snapshot of that namespace's real exports (`tests/stdlib_exports/<namespace>.txt`). The
  snapshots are best-effort full export lists written from R knowledge (hundreds of names per package,
  including operators and names no stub will cover) — a static stand-in until real-R generation — so
  the printed per-package coverage percentage is an honest gauge, not a self-referential one. Policy —
  the corpus must be a **subset** of the snapshot (every stubbed name must be a real export; a stubbed
  non-export is a hard failure); unstubbed real exports are allowed and only gauge-counted, and no
  percentage is ever asserted. The suite additionally asserts that every shipped stub source parses
  and harvests **cleanly** (the loader drops malformed lines rather than aborting, and dropped
  *override* declarations are reported by `roughly check` and the editor) and that the loader-visible
  names match the corpus files exactly.
- **`Any` over-permissiveness** silences real errors — hence the two-tier marker in §4.
- **Incremental isolation** is automatic — a set-once input; see §3.

### Per-edit cost

Because the stub library is a set-once input (§3), its mere presence adds no per-edit recheck cost: an
edit never invalidates it, and a body recheck still touches only the edited document and its referrers.
A document pays inference time only for the base names it actually references — that is the feature
working, not bookkeeping overhead — and a document that references no base names pays nothing for the
corpus being loaded.

## 7. CRAN tier — per-project introspection (proposed, not built)

The embedded corpus covers base/stats/utils/methods — the packages every R session attaches. Third-
party CRAN packages are **not shipped**; they are discovered and stubbed **per project**, because the
installed set and versions are a property of the project's library, not of Roughly.

The CRAN/introspected stubs are the **same kind of input** as the embedded corpus — an immutable,
set-once input that never invalidates a package edit (the §3 hygiene contract). The only difference is
provenance: they are **generated by introspecting real R** rather than curated by hand. Everything in
§3 (incremental hygiene) applies to them unchanged.

### Discovery

Resolve the project's installed package set the way R itself would, in priority order:

- **renv / project library** — if an `renv.lock` (or active `renv`) is present, it pins exact package
  versions; use it as the source of truth.
- **`DESCRIPTION`** — `Imports` / `Depends` / `Suggests` enumerate the packages the project may load.
- **`.libPaths()`** — the fallback: the actual library directories the R installation would search.

The discovered set, plus each package's installed version, keys the stub cache.

### Introspection-generated shallow stubs

For each discovered package, generate a **shallow** stub by running real R against the installed
package:

- `getNamespaceExports(pkg)` enumerates the exported names.
- `formals(fn)` gives each function's **arity and argument names** (including `...` and defaults).

This yields the *call surface* — enough to check arity and argument names — but **not** return types:
real return types are not introspectable from `formals()`, so every generated return is `Any` (or the
greppable `Incomplete` marker from the §4 two-tier scheme, so tooling can find what still needs a
precise type). A shallow stub therefore catches "no such export" and gross arity/argument-name
mistakes while staying sound (never claiming a false-precise return).

- **Cached per package version.** A generated stub is a pure function of `(package, version)`, so it is
  cached on that key and regenerated only when the installed version changes — the same immutability
  the embedded corpus enjoys, just keyed per project.
- **Optional curated overrides** (the typeshed third-party model): a hand-written `.Rtypes` stub for a
  high-value package (or specific functions) layers over the generated shallow stub, supplying precise
  returns the introspection cannot. Curated overrides win where present; generation fills the rest.
- **Unstubbed → `Any`, never a hard error.** A package with no generated and no curated stub (R not
  available, package not installed, generation failed) resolves its names to `Any`. A missing stub
  must never be a diagnostic — it degrades precision, not correctness.

## 8. R-version keying of the embedded corpus (proposed, not built)

Base evolves across R releases — functions are added, signatures change — so the embedded corpus is
not version-agnostic in general. The shipped corpus is **keyed by detected R version**: Roughly
embeds one curated corpus per supported major.minor line (or a base corpus plus per-version deltas),
and selects the one matching the project's R.

### Version detection and configuration

- **Detected** — query the R installation on `PATH` (`R --version`, or `R.version` fields), the same
  installation discovery the CRAN tier uses.
- **Configured** — an explicit project setting (e.g. in Roughly's project config, or read from
  `DESCRIPTION`'s `Depends: R (>= x.y)`) overrides detection, so a project pins the R semantics it
  targets regardless of the machine's installed R.
- **Default** — when neither is available, fall back to a pinned baseline version, documented as the
  default, so analysis is deterministic without an R installation present.

The selected version keys which embedded corpus folds into the template environment at
`Analysis::new`. Because the corpus is still an immutable base-environment input, version selection
happens once at construction; it never participates in the incremental graph.

## 9. Future implementation slice — sequencing (not buildable here)

The CRAN tier and R-version keying are a **design discussion + tooling item**, not buildable in the
current environment, because of a hard **environment dependency**: introspection-generated stubs and
the stubtest validator both need a **real R installation available to run** (`getNamespaceExports()`,
`formals()`, `R --version`). None of that can run where Roughly's tests run today, so this is sequenced
as a future slice, in dependency order:

1. **Introspection generator** — the tool that runs R, harvests `getNamespaceExports()` + `formals()`
   into shallow `.Rtypes` stubs (returns `Any`/`Incomplete`), and caches them per package version. Gated on
   an available R; ships as offline tooling that writes a cache the checker consumes.
2. **stubtest validator (LT5)** — a stubtest-equivalent CI check that introspects real R and **diffs**
   the curated stubs (embedded corpus *and* curated CRAN overrides) against the live signatures,
   flagging drift. Also R-dependent; runs in CI where R is installed, not in the unit suite.

The `NamespaceGet` HIR node for `pkg::name` has landed: a qualified read resolves against the flat
corpus with namespace-tag validation (§3). What this slice would add on top is a *per-namespace*
stub map for third-party packages, so a CRAN package's names resolve only through its own namespace.
Until it lands, third-party names resolve to `Unknown` with the unknown-namespace warning — the
non-erroring degrade from §7.
