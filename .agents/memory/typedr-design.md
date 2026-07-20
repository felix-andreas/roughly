# TypedR — a compiled dialect with inline types (design proposal)

Status: **proposal, not scheduled**. Nothing here is implemented; no contract
page exists yet. This document records the intended shape so a future slice
can start from a settled design instead of re-deriving it. Promote the
decided parts to a docs-site page when implementation begins.

## The idea

A package gains a `TypedR/` source folder next to `R/`. Files in it are
written in **TypedR**: R with inline type annotations plus a small set of
constructs R lacks — first-class records and tuples, and tagged unions with
`match`. Every TypedR file compiles automatically to a plain R file in `R/`,
which is what R itself runs and what CRAN ships. Plain-R consumers never see
TypedR; the compiled output is idiomatic, readable R.

The existing `#:` comment annotations stay fully supported and are not
deprecated: they are the zero-cost path for annotating plain R. TypedR is
the opt-in path for code that wants types (and richer data modeling) as
first-class syntax.

## Why this fits Roughly specifically

Three assets make this a frontend, not a second product:

1. **The type system is already carrier-agnostic.** The typing reference
   defines semantics over types, not over `#:` comments. Records and tuples
   already exist as list shapes (`list{name: T}`, `list{T1, T2}`) — the
   reference explicitly leaves room for "distinct tuple and record
   constructors later, even if they remain runtime aliases of R lists".
   Tagged unions decompose into machinery that exists: nominal types
   (`@type` / `@new`), unions, and guard narrowing.
2. **The type grammar is already written.** The lexer tokenizes full type
   syntax inside `#:` regions (`in_annotation` mode) and the annotation
   parser builds real syntax nodes for it. TypedR promotes that grammar
   from comment trivia into expression positions; it does not invent a
   second type syntax.
3. **The pipeline is item-granular and dialect-tolerant.** Items lower to
   HIR independently; naming, inference, the package interface, and the IDE
   layer consume HIR and types, not surface syntax. A TypedR item that
   lowers to the same HIR shapes flows through every downstream stage —
   including incremental analysis — unchanged.

## Surface design

File extension: `.tR` (R-family at a glance). Windows case-insensitivity
makes `.tr` equivalent; the tools treat them identically.

### Inline annotations (phase 1)

```
half <- function(x: integer, scale: double = 1) -> double {
  x / (2 * scale)
}
count: integer <- 0L
```

- Parameter types, return types, and binding ascriptions use the existing
  type grammar verbatim (same names, same generics `<T: numeric>`, same
  shapes).
- Compilation is **pure erasure onto `#:` comments**: the generated R keeps
  the declaration as the equivalent `#:` annotation, so the output of the
  compiler is exactly the input language the checker already speaks.

### Records and tuples (phase 2)

```
type Point = {x: double, y: double}

origin <- Point{x = 0, y = 0}      # record construction, checked fields
pair <- #(1L, "one")               # tuple construction, fixed positions
```

- `type Name = {field: T, ...}` is surface syntax for the existing
  record-like list shape (an `@alias` in today's terms); `Point{...}`
  construction checks field names and types at the construction site.
- Tuples get a constructor form (`#(a, b)` is a placeholder spelling — the
  final token is an open question) for the existing tuple-like shape.
- Both **compile to `list(...)`** — they are runtime aliases of R lists, as
  the typing reference anticipated. A nominal record (opaque outside its
  constructor) is the existing `@type` + `@new` semantics with `new Point{...}`
  surface syntax.

### Tagged unions and `match` (phase 3)

```
type Shape =
  | Circle(radius: double)
  | Rectangle(width: double, height: double)

area <- function(shape: Shape) -> double {
  match (shape) {
    Circle(radius) -> pi * radius^2
    Rectangle(width, height) -> width * height
  }
}
```

**Decision: classic nominal sum types, not polymorphic variants.**
Rationale:

- Each variant maps onto machinery that exists: a variant is a nominal
  record type; the sum name is an alias for the union of its variants;
  `match` narrowing is the guard-narrowing machinery applied per arm;
  exhaustiveness is union coverage — all four are implemented concepts.
- The compiled form is **idiomatic S3**: constructors emit
  `structure(list(radius = radius), class = c("Circle", "Shape"))`, so
  plain-R consumers can `inherits(x, "Shape")` and even write S3 methods on
  variants. The dialect's data types remain first-class citizens of the
  host ecosystem — this is the property that makes TypedR adoptable
  mid-package.
- Polymorphic variants require row polymorphism: a new constraint kind, a
  harder inference problem, and notoriously worse error messages. The
  design bar (simplest correct model; diagnostics quality) rules them out;
  nothing prevents revisiting once nominal sums prove insufficient in
  practice.

`match` compiles to a `switch` on the class tag with field bindings in each
arm. The checker enforces exhaustiveness (a missing variant is an error
naming it; a `_ ->` default arm opts out) and per-arm narrowing.

## Compilation model

- **Target = annotated plain R.** The compiler emits `#:` annotations for
  everything expressible as one, so the generated file independently
  type-checks under today's contract. This closes the loop cheaply: the
  semantic-preservation gate is "checking `TypedR/foo.tR` and checking the
  generated `R/foo.R` produce identical findings" (ranges mapped), which is
  a differential in the house style.
- **Line-preserving erasure wherever possible.** Inline annotations erase
  in place (the `#:` line is emitted above the statement, which does shift
  lines — so full 1:1 line identity is not achievable; instead the compiler
  maintains a per-file line map, and every diagnostic surface reports
  TypedR positions). Sugar constructs expand on their own lines with the
  formatter normalizing output.
- **Generated files are committed.** R packaging and CRAN need real files
  under `R/`; reviewers need readable diffs of what actually ships. Each
  generated file carries a header (`# Generated by roughly from
  TypedR/foo.tR — do not edit`), and `roughly build --check` fails CI on
  drift (same shape as `roughly format --check`). Determinism comes free:
  the emitter renders through the existing formatter.
- **Editing the generated file is an error the tools catch**, not a merge
  problem: the drift check compares a content hash recorded in the header.

## Fit with the existing crates

- `syntax` — one grammar, a `Dialect` parameter on `parse` (`R` |
  `TypedR`). The TypedR dialect enables: type positions after `:` in
  parameters and bindings, `-> T` in function heads, `type` declarations,
  record/tuple constructors, `match`. New node kinds (TYPE_ASCRIPTION,
  TYPE_DECL, RECORD_EXPR, TUPLE_EXPR, MATCH_EXPR, MATCH_ARM, VARIANT) join
  the existing tree; the type-syntax nodes are shared with the annotation
  grammar. The R dialect is byte-for-byte unaffected — the corpus,
  round-trip, and acceptance gates pin that.
- `semantics` — lowering maps TypedR constructs to HIR: ascriptions become
  the same annotation seams `#:` attachment uses today; `type` declarations
  become alias/nominal definitions; record/tuple constructors lower to the
  existing list-shape expressions (checked against the declared shape at
  the construction site); `match` gets a native HIR node (exhaustiveness
  and arm narrowing need variant knowledge that desugaring would erase).
  The type system gains **no new type formers** for phases 1–2; phase 3
  adds only "sum alias = union of variant nominals + variant field
  records", expressible in the current `TyKind` vocabulary.
- `format` — the formatter learns the new nodes (the annotation renderer
  already formats type syntax); TypedR files format like R files
  otherwise. The emitter reuses this for deterministic compiled output.
- `ide` — hover/completion/goto over TypedR files come from the same
  HIR/type queries; the type-vocabulary completion pool already exists for
  annotation positions and applies to inline type positions directly.
- `roughly` (CLI/LSP) — `ProjectFiles` gains TypedR documents; a TypedR
  file **shadows its generated twin** (the generated `R/foo.R` is excluded
  from analysis when `TypedR/foo.tR` exists, so items are defined once).
  The LSP watches `TypedR/`, republishes diagnostics on the source, and
  compiles on save; `roughly build` compiles the folder; `roughly build
  --check` gates CI. Generated files never carry diagnostics — findings
  map back through the line map.

## Diagnostics

TypedR diagnostics point at TypedR source, always. Two new classes:

- compile-blocking dialect errors (a `match` on a non-sum type, a record
  construction with a missing field) — same wording bar as everything else
  (Rust/Elm style, precise ranges);
- drift errors (`R/foo.R` edited by hand or stale) at the project level.

Runtime R errors reference generated lines; the committed, readable,
formatter-normalized output plus the header pointer is the debugging story
(same trade every compiled-to-JS language makes).

## Testing (pipeline-wide from day one, per the standing doctrine)

- **Transpile fixtures**: `.tR` input → golden generated R, in the fixture
  harness (readable diffs; the emitter is deterministic).
- **Typing fixtures** over TypedR sources, same renderer as today's suites.
- **Equivalence differential**: check(source) == check(compiled) findings,
  range-mapped — the semantic-preservation gate.
- **Round-trip**: generated output must parse under the R dialect with zero
  errors, always (a fuzz invariant, not just a fixture).
- **Fuzzing**: the TypedR dialect joins the parser invariant battery
  (never-panic, lossless, geometry, determinism) and gets a
  coverage-guided target; the compiler gets compile-then-parse and
  compile-idempotence (recompiling the committed output is a no-op)
  invariants; `match`/record/tuple generators join the semantics fuzzer.
- The R dialect's existing gates (corpus, differential vs legacy oracle)
  are untouched and must stay green throughout — TypedR must be provably
  zero-cost for plain-R users.

## Delivery phases (each lands whole: grammar + checker + emitter +
formatter + fixtures + fuzz)

1. **Inline annotations** — pure erasure; no new types, no new runtime
   shapes. Proves the dialect plumbing (folder, shadowing, build, drift
   check, line maps) with minimal semantic surface.
2. **Records and tuples** — constructor syntax over existing list shapes;
   settles the typing reference's open constructor question.
3. **Sum types and `match`** — nominal variants, S3-class compilation,
   exhaustiveness.

## Open questions

- Final spellings: the tuple constructor token; `new Point{...}` vs
  `Point{...}` for nominal vs structural records; `match (x)` vs `match x`.
- Whether a TypedR file may also contain `#:` annotations (leaning no —
  one carrier per dialect keeps attachment rules simple).
- Whether `roughly build` also formats sibling plain-R files (leaning no —
  build stays orthogonal to format).
- srcref fidelity: whether to emit `#line`-style markers for R debuggers
  or rely on the line map + readable output alone.
- Package metadata: does `TypedR/` need a DESCRIPTION field (a
  `Config/roughly/...` entry) so R tooling ignores it cleanly everywhere?
- Standalone scripts: the compilation model above is package-centric
  (committed twins under `R/`), but `roughly run foo.tR` — typecheck,
  compile in memory, execute through the embedded runtime — would give
  TypedR a script story with no generated file on disk. The execution
  backend is the REPL's planned headless runner (`repl-design.md`); decide
  whether standalone `.tR` scripts land with it or with a dialect phase.
