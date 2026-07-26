# Open type-system design questions

Forward-looking design space for Roughly's type system: questions that are **not yet decided** and the options on the table, with the current stopgap and why. This is distinct from `decisions.md` (the log of what has been *settled*) — entries here are live deliberations. When one is resolved, move the decision + rationale to `decisions.md` and delete it here. The settled contract is always `docs/src/content/docs/reference/type-system.md`; nothing here is contract until it lands there.

*(Resolved and moved to `decisions.md` §Beta-semantics: generic vector element types → atomic-element constraint on the existing constraint mechanism; ad-hoc overloading → ordered overload sets with probe-then-rollback, confined to declaration files, traits declined; multi-member unions → join/annotation-only, never bound into unification variables; R variable model → mutable slots with union-at-join reads.)*

## 1. Tags / discriminated unions via a Roughly stdlib

**Question.** How to provide Roc-style tags (OCaml polymorphic variants) for R: a compiler-known Roughly library exposing tag **constructors** and a **`match`** function with exhaustive-pattern checking.

**Direction (user):** provide them through a stdlib the checker knows specially, not through new R syntax — annotated R stays ordinary R. Post-beta.

**Unresolved tension:** `typedr-design.md` proposes the opposite route — real syntax in a compiled dialect — for the same capability. Both reach exhaustive case analysis; they differ in whether the checker blesses particular call shapes or owns a grammar, and in whether a build step is acceptable. Building both is waste, so settle the fork (it is §3 of that document) before either is implemented, and delete the losing half.

**Design space to work out before building:**
- representation: a tagged value is presumably `list(tag = "Name", value = ...)` at runtime — does the type system model it as `union` of nominal-ish tag types, or as a new core form?
- exhaustiveness: `match(x, Some = fn, None = fn)` — checking that the named arguments cover the union members needs literal argument-name awareness at one blessed callee; how special is that call form allowed to be?
- do general unions (already decided) + literal discriminant fields suffice, or do tags need their own type former?
- interaction with strict mode and with narrowing (a `match` arm should see the narrowed member type).

## 2. S3 dispatch

**Question.** How to type S3 generics (`print`, `summary`, `plot`, `format`, `predict`, …) where the result depends on the class of the first argument.

**Options.** Per-class overload sets on the generic's stub (`print : fn(x: data.frame) -> data.frame`
etc.) — available now, and the sanctioned form since it lives in a declaration file; or a class-
hierarchy model with `UseMethod` awareness (heavier, and the only thing that reaches user-defined S3
classes). Traits are no longer an option (§4).

**Current stopgap:** S3 generics are `Any`/missing in the corpus. The operator method tables that DO
ship (`+.Date`, `Arith.difftime`) show the shape a per-class answer takes.

## 3. data.frame / matrix modeling

**Question.** Column-level typing for `data.frame` (`df$col`, `df[, "col"]`) and dimensionality for matrices.

**Notes.** Beta ships `data.frame` as an opaque nominal (via `@type` in `.Rtypes`) — honest but shallow. Column typing likely wants row-polymorphic records over an opaque carrier; matrix wants an element type without dimension tracking first. Both interact with `[`/`[[` semantics and with the `x[i, j]` lowering. Design after the beta semantics settle.

## 4. Traits / typeclasses — CLOSED, declined

**Question (settled).** A general capability mechanism (numeric, atomic-element, comparable,
`+`-overloadable/S3) that could subsume the ad-hoc constraint kinds and overload sets.

**Answer: no, and the tripwire is disarmed** — the type system admits only what is fast to check,
which means Hindley-Milner, and declaration files carry the one sanctioned exception (the record is
in `decisions.md`, §"The type system is Hindley-Milner, and stays fast to check"). Traits are the
textbook *correct* way to put ad-hoc polymorphism into HM — that is exactly what type classes were
invented for, and it is why overloading is not HM — but correct is not the bar here; checkable at
editor speed, with no vocabulary for a user to learn, is. Do not reopen this because a third
constraint kind appears: add the constraint, or accept the imprecision.

Historical note, so the reasoning is not lost: three pressures were examined against the old
tripwire and none of them tripped it — a "comparable" kind for two-flexible comparisons (nearly
vacuous, since R compares across atomic families), intersection constraints for union-commitment
conflicts (first-use commitment plus an annotation is the spec), and the `T[]` element type (which
became the atomic-element constraint on the existing mechanism). The pattern across all three is the
useful one: what looked like it needed traits was better served by one more constraint on machinery
that already existed.

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
