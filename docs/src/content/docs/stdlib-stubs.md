---
title: Stdlib Stubs (Proposal)
description: Design note proposing a standard-library stub framework so the type checker knows base/stats/utils instead of resolving them to Unknown
---

:::caution[Status: proposal]
This page is a **design note for review**, not a contract. Nothing here is implemented yet,
and the documented behavior is not binding until it is. Implementation is sequenced **after**
the M3/M4 incremental-analysis work (see [Sequencing](#sequencing)). The authoritative typing
contract remains the [Typing Reference](/typing-reference); this note only proposes how to feed
the standard library into that contract.
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

Stubs are **immutable, high-durability inputs** — analogous to rust-analyzer's `Durability::HIGH` and
the M3 reverse-dependency model. They are loaded, parsed, and interned **once at `Analysis::new`** and
are never invalidated by user edits.

- Store as an immutable `StubLibrary`: `namespaces: namespace -> {Symbol -> scheme}`, plus a base
  `TypeDefinitionEnvironment`.
- Default-attached namespaces (`base` + R defaults) fold into the template environment, so their names
  resolve as bare globals.
- `library(pkg)` attaches more namespaces (follow-up work).
- `pkg::name` resolves against `StubLibrary.namespaces`. Today `NAMESPACE_OPERATOR` lowers to
  `Unsupported`; this requires adding a `NamespaceGet` node.

### Incremental hygiene (the one subtle correctness risk)

Stub schemes are bound into the **template environment**, **not** into `global_bindings` and **not**
into the package interface table. Therefore they:

- never enter `render_dependency_fingerprint`;
- never create reverse-dependency edges.

Extend the M3 debug drift assertion to assert that base names never appear as dependency edges. If
stubs leaked into the package-global fingerprint, every base name would trigger spurious package-wide
invalidation.

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

Implement **after** the M3/M4 incremental keystone. M3 is done; this plugs in as a high-durability
input to the now-precise dependency model. Building it before M3 would have leaked base names into the
package-global fingerprint and caused spurious package-wide invalidation.

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
- **Index / fingerprint hygiene** is the one subtle correctness risk; see §3.
