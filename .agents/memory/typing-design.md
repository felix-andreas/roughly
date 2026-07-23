# Open type-system design questions

Forward-looking design space for Roughly's type system: questions that are **not yet decided** and the options on the table, with the current stopgap and why. This is distinct from `decisions.md` (the log of what has been *settled*) — entries here are live deliberations. When one is resolved, move the decision + rationale to `decisions.md` and delete it here. The settled contract is always `docs/src/content/docs/typing/reference.md`; nothing here is contract until it lands there.

*(Resolved and moved to `decisions.md` §Beta-semantics: generic vector element types → atomic-element constraint on the existing constraint mechanism; ad-hoc overloading → ordered overload sets with probe-then-rollback, traits deferred; multi-member unions → join/annotation-only, never bound into unification variables; R variable model → mutable slots with union-at-join reads.)*

## 1. Tags / discriminated unions via a Roughly stdlib

**Question.** How to provide Roc-style tags (OCaml polymorphic variants) for R: a compiler-known Roughly library exposing tag **constructors** and a **`match`** function with exhaustive-pattern checking.

**Direction (user):** provide them through a stdlib the checker knows specially, not through new R syntax — annotated R stays ordinary R. Post-beta.

**Design space to work out before building:**
- representation: a tagged value is presumably `list(tag = "Name", value = ...)` at runtime — does the type system model it as `union` of nominal-ish tag types, or as a new core form?
- exhaustiveness: `match(x, Some = fn, None = fn)` — checking that the named arguments cover the union members needs literal argument-name awareness at one blessed callee; how special is that call form allowed to be?
- do general unions (already decided) + literal discriminant fields suffice, or do tags need their own type former?
- interaction with strict mode and with narrowing (a `match` arm should see the narrowed member type).

## 2. S3 dispatch

**Question.** How to type S3 generics (`print`, `summary`, `plot`, `format`, `predict`, …) where the result depends on the class of the first argument.

**Options.** Per-class overload sets on the generic's stub (cheap once overloads land — `print : fn(x: data.frame) -> data.frame` etc.); a real class-hierarchy model with `UseMethod` awareness (heavier, needed for user-defined S3 classes); or leave generics `Any` until traits.

**Current stopgap:** S3 generics are `Any`/missing in the corpus. Revisit once overload sets are in use — overloads may cover the stdlib need without a dispatch model.

## 3. data.frame / matrix modeling

**Question.** Column-level typing for `data.frame` (`df$col`, `df[, "col"]`) and dimensionality for matrices.

**Notes.** Beta ships `data.frame` as an opaque nominal (via `@type` in `.Rtypes`) — honest but shallow. Column typing likely wants row-polymorphic records over an opaque carrier; matrix wants an element type without dimension tracking first. Both interact with `[`/`[[` semantics and with the `x[i, j]` lowering. Design after the beta semantics settle.

## 4. Traits / typeclasses

**Question.** A general capability mechanism (numeric, atomic-element, comparable, `+`-overloadable/S3) that could subsume the ad-hoc constraint kinds and overload sets.

**Notes.** Two base constraint kinds (numeric, atomic-element; plus their meet, scalar-numeric) and stub overload sets exist; if a third *independent* constraint kind or user-facing overloading pressure appears, that is the tripwire to design traits properly instead of accreting. Keep overload sets and constraints shaped so a trait system can absorb them (constraints already quantify in schemes; overloads are per-name lists). Two candidate pressures were examined and deliberately did NOT trip the wire (decisions.md): a "comparable" kind for two-flexible comparisons (nearly vacuous — R compares across atomic families), and intersection constraints for union-commitment conflicts (first-use commitment + annotation is the spec). The wire stays armed.

## 5. Variadic inference bridging

**Question.** `function(x, ...)` bodies: inference now needs to lower a trailing `...` as a rest parameter (decided direction in `backlog.md` Phase 1) — but what type does `...` have *inside* the body (`list(...)`, `..1`, forwarding to another variadic)?

**Current stopgap:** `...` uses inside a body stay `Unknown`; only the signature-level rest parameter is bridged. Full `...` semantics (forwarding compat, `..N` access) is open.

## 6. NAMESPACE / import model

**Question.** How library scoping should work: `library(pkg)` attaches, `importFrom` in NAMESPACE, search-path order, masking warnings.

**Notes.** Beta ships `pkg::name` resolution against namespace-partitioned stubs. The attach/import model (what `library(dplyr)` makes visible, masking diagnostics) is undesigned; it determines when an unresolved name is *really* unresolved, so it directly feeds strict mode's usefulness on real projects.

## 7. Non-standard evaluation (data masking: dplyr, data.table, EDSLs like ompr)

**Question.** How far can masked expressions — names resolving against a *data* context the
callee constructs at run time (`mutate(df, y = x * 2)`, `DT[x > 3, .(m = mean(y)), by = z]`,
`add_variable(model, x[i], i = 1:10)`) — be statically checked, and what machinery does each
step need?

**Current state (implemented, sound-by-refusal).** Three mechanisms: `quiet_reads` in naming
(masked-context reads are never "unresolved" but still keep definers alive), `masked_subsets`
(brackets the naming walk recognizes as data.table syntax evaluate indexes in the data's frame;
the whole bracket types `Unknown`), and the stub corpus's `masked` set (variadic callees whose
`...` is data-masked, `with()`-family). Zero false positives; zero checking inside the mask.

**The design ladder (each step is independently shippable):**

1. **Masking contracts in the stub language.** Extend `.Rtypes` so a signature can declare
   which argument supplies the data context and which arguments are masked by it
   (`mutate : fn(.data: data.frame, ...: masked(.data)) -> data.frame`). Pure metadata — it
   moves the currently hardcoded masked-callee knowledge into the corpus, per-package and
   overridable, without new type theory.
2. **Column vocabulary, not types.** Once data.frame carries column-level structure (design
   question 3), a masked name can be checked for *membership* in the data argument's column
   set plus the lexical environment — catching column typos, the most common NSE bug — while
   masked expressions still type `Unknown`. `.data$x` / `.env$x` pronouns resolve exactly.
3. **Typed masked expressions.** Check the masked expression in an environment extended with
   the columns' types. The gate is the *result* type: `mutate` extends the row type, so the
   return needs record-extension (sequential, in argument order); grouped operations
   (`summarise`, data.table `by=`) change the frame's shape. Tidy-eval injection (`!!`,
   `{{ var }}`) needs a column-reference kind in the type system — the expensive tail, likely
   permanently behind explicit annotations.
4. **EDSLs (ompr).** No data context exists — names are declared by prior builder calls
   (`add_variable`) and live only in the model object. Generic checking is unrealistic;
   the honest options are quiet-read suppression (today) or package-specific extensions.

**Precedent.**

- **R itself:** `R CMD check`'s "no visible binding for global variable" NOTE is the oldest
  static-checker-vs-NSE collision; the ecosystem's answers — `utils::globalVariables()`
  suppression and the `.data` pronoun — validate both the suppression baseline (step 0) and
  explicit pronouns as the bridge to checkability.
- **TypeScript query builders** (Prisma, Kysely, Drizzle): column sets encoded as object
  types, masked names checked via `keyof`/mapped types — the direct analogue of step 2/3,
  and proof the "membership first, expression types second" ladder works at ecosystem scale.
- **F# type providers** (FSharp.Data): schemas imported at compile time generate typed
  accessors — the strongest form of step 2's "know the columns", at the cost of a
  compile-time data dependency.
- **Python:** mypy/pyright deliberately do NOT type pandas columns (pandas-stubs types
  operations, columns stay stringly); schema checking lives in runtime validators (pandera).
  The mainstream punt marks how far the cost curve bends — and where Roughly can
  differentiate, since steps 1-2 fit its architecture (stub contracts + structural records)
  without new inference machinery.

**Recommendation.** Steps 1-2 after data.frame columns land (question 3 gates this); step 3
only for the pronoun/explicit forms; step 4 stays suppressed. Never regress the zero-false-
positive property: every step must keep unknown masking constructs silent rather than guessed.
