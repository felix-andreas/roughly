---
title: Stdlib Stubs (Proposal)
description: Design note proposing a standard-library stub framework so the type checker knows base/stats/utils instead of resolving them to Unknown
---

:::caution[Status: partially implemented]
A first increment of the standard library ships: `T`/`F`/`pi` plus a curated set of base functions
are declaration-only `#:` stubs under `crates/analysis/stubs/` (`base.R`, `stats.R`, `utils.R`,
`methods.R`), loaded by the stub loader and bound into the checker as a **set-once input** that never
invalidates a package edit (see [Incremental hygiene](#incremental-hygiene)). Still **proposed / not
yet built**: the CRAN tier (per-project introspection, §7), R-version keying of the embedded corpus
(§8), the stubtest CI validator, and `pkg::name` (`NamespaceGet`). Sections below mark which is which.
The authoritative typing contract remains the [Typing Reference](/typing-reference); this note
describes how the standard library feeds that contract.
:::

## Problem

The checker's only "library" today is roughly 19 hardcoded `BuiltinKind` entries: the operators plus
`c` and `list`. Everything else in the standard library resolves to `Unknown`. `T`/`F`/`pi` are
untyped, and base functions (`length`, `nchar`, `seq_len`, `paste`, ...) have no signatures, so calls
through them lose all type information.

This note proposes a **stub framework**: declaration-only descriptions of base and other R packages,
loaded as immutable inputs, so the checker knows the standard library the same way rust-analyzer knows
`core`/`std`.

## 1. Stub format

**Decision: reuse the existing `#:` annotation syntax in declaration-only `.R` stub files.**

A stub file is ordinary R source whose bindings carry `#:` annotations. A loader harvests each
annotation directly into a `TypeScheme` and **does not run body inference** — the placeholder body is
ignored, so it can be `0L`, `NULL`, or anything parseable.

Three declaration shapes cover the standard library:

```r
# function signature — annotation becomes the scheme; body `0L` is never inferred
#: fn(x: Any) -> integer
length <- function(x) 0L

# plain value binding
#: logical
T <- TRUE

#: double
pi <- 0

# type / class declaration
#: @type factor {integer[]}
```

### Why reuse `#:` instead of a bespoke format

A separate `.d.R` / JSON / TOML stub grammar was considered and rejected:

- reuses the existing `type_syntax` parser and the `Definition` pipeline — no second type grammar to
  build and keep from drifting;
- human-readable, and loadable / lintable / formattable by Roughly *and* by ordinary R tooling;
- one source of truth for type notation across user code and stubs.

### Cross-ecosystem note

Statically-typed hosts that publish types for foreign code use **separate** stub files — TypeScript
`.d.ts`, Python `.pyi`, Sorbet `.rbi`. Dynamically-typed hosts retrofitting types onto their *own*
source put them **inline** — Elixir/Erlang `@spec`. R parallels the latter for its own source, so
inline `#:` is primary; separate stub files exist only for foreign packages that cannot be annotated
at the source (base and CRAN).

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

Faithfully stubbing base requires type-syntax extensions:

| Gap | Example that fails today | Extension needed |
|-----|--------------------------|------------------|
| Variadics (highest value) | `paste`, `sum`, `c` | `...args: T` parameter form |
| Dotted parameter names | `na.rm`, `length.out` | allow `.` in the identifier grammar |
| Generic atomic suffix | `rev`, `head`, shape-threading | accept `T[]` (type variable + suffix) |
| Overloading | one name today = one scheme | multiple schemes per name |

Until these land, affected functions degrade to `Any` / `@trust`.

## 3. Loading and namespacing

Stubs are **immutable, set-once inputs** — analogous to rust-analyzer's `Durability::HIGH`. They are
loaded, parsed, and interned once, and are never invalidated by user edits.

- Store as an immutable `StubLibrary`: `namespaces: namespace -> {Symbol -> scheme}`, plus a base
  `TypeDefinitionEnvironment`.
- Default-attached namespaces (`base` + R defaults) fold into the template environment, so their names
  resolve as bare globals.
- `library(pkg)` attaches more namespaces (follow-up work).
- `pkg::name` resolves against `StubLibrary.namespaces`. Today `NAMESPACE_OPERATOR` lowers to
  `Unsupported`; this requires adding a `NamespaceGet` node.

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

**Two-tier dynamic marker** (borrowed from typeshed): keep a real `Any` for genuinely untypeable
returns *distinct from* a greppable "incomplete / not-yet-typed" marker. The distinction makes partial
stubs first-class and improvable, and lets tooling find what still needs work — mirroring typeshed's
`Incomplete`-vs-`Any` split.

## 5. Worked example

A base stub fragment in the proposed format:

```r
#: logical
T <- TRUE

#: logical
F <- FALSE

#: double
pi <- 0

#: fn(x: Any) -> integer
length <- function(x) 0L

#: fn(x: character) -> integer
nchar <- function(x) 0L

#: fn(length_out: integer) -> integer[]
seq_len <- function(length_out) 0L
```

The schemes each produces:

| Binding | TypeScheme |
|---------|------------|
| `T`, `F` | `Scalar(Logical)` |
| `pi` | `Scalar(Double)` |
| `length` | `fn([], [x: Any], Integer)` |
| `nchar` | `fn([], [x: Character], Integer)` |
| `seq_len` | `fn([], [length_out: Integer], Vector(Integer))` |

R that type-checks against these stubs plus the hardcoded kernel:

```r
n    <- length(c(1L, 2L, 3L))  #: integer  (stub scheme)
half <- pi / 2                 #: double   (operator kernel on Scalar(Double))
flag <- T                      #: logical  (stub value binding)
```

This demonstrates stub schemes and the hardcoded kernel interoperating: `length`'s scheme types `n`,
the operator kernel promotes `pi / 2`, and the `T` value binding types `flag`.

**Signatures the current grammar cannot yet express** (flagged, degrade to `Any`/`@trust` until the
extensions land):

- `nchar`'s shape polymorphism (result shape should track the input vector shape);
- `paste`'s variadics (`...`);
- `length.out`'s dotted parameter name.

## 6. Sequencing and risks

### Sequencing

The stub library plugs into the query engine as a set-once input, so it adds no per-edit recheck
cost: its revision never advances, and a file that references a stub records that dependency like any
other input it reads.

**Smallest first increment:** seed `T`/`F`/`pi` plus ~12 high-frequency base functions, parsed once
from an embedded `base.R`, behind the template-seeding path — before any namespace / `::` / `library()`
machinery. This closes the `T`/`F`/`pi` gap with only the two integration edits from §3.

### Risks

- **Type-syntax expressiveness.** Variadics, dotted names, and the generic `T[]` suffix are needed for
  a faithful corpus; until they land, degrade to `Any` / `@trust`.
- **The operator/`c` kernel cannot migrate.** One-source-of-truth is necessarily partial.
- **Corpus size.** Base alone is ~1200 functions. Curate a high-value subset; treat stubs as living
  docs with real drift risk. The validation tool is a **stubtest-equivalent** that introspects real R
  signatures via `formals()` / `getNamespaceExports()` and diffs them against the `#:` annotations.
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
- **Optional curated overrides** (the typeshed third-party model): a hand-written `#:` stub for a
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
   into shallow `#:` stubs (returns `Any`/`Incomplete`), and caches them per package version. Gated on
   an available R; ships as offline tooling that writes a cache the checker consumes.
2. **stubtest validator (LT5)** — a stubtest-equivalent CI check that introspects real R and **diffs**
   the curated stubs (embedded corpus *and* curated CRAN overrides) against the live signatures,
   flagging drift. Also R-dependent; runs in CI where R is installed, not in the unit suite.
3. **`NamespaceGet` HIR node for `pkg::name`** — today `NAMESPACE_OPERATOR` lowers to `Unsupported`;
   resolving `pkg::name` against `StubLibrary.namespaces` needs a real `NamespaceGet` node carrying the
   namespace + name. This is the one piece buildable without R (it is pure lowering/resolution), but it
   is only *useful* once a namespace has stubs to resolve against, so it is sequenced with the tier it
   serves.

Until this slice lands, `pkg::name` stays `Unsupported` and third-party names resolve to `Any` — the
safe, non-erroring degrade from §7.
