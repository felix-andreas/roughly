# Open type-system design questions

Forward-looking design space for Roughly's type system: questions that are **not yet decided** and the options on the table, with the current stopgap and why. This is distinct from `decisions.md` (the log of what has been *settled*) — entries here are live deliberations. When one is resolved, move the decision + rationale to `decisions.md` and delete it here. The settled contract is always `docs/src/content/docs/typing-reference.md`; nothing here is contract until it lands there.

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

**Notes.** Two base constraint kinds (numeric, atomic-element; plus their meet, scalar-numeric) and stub overload sets exist; if a third *independent* constraint kind or user-facing overloading pressure appears, that is the tripwire to design traits properly instead of accreting. Keep overload sets and constraints shaped so a trait system can absorb them (constraints already quantify in schemes; overloads are per-name lists).

## 5. Variadic inference bridging

**Question.** `function(x, ...)` bodies: inference now needs to lower a trailing `...` as a rest parameter (decided direction in `backlog.md` Phase 1) — but what type does `...` have *inside* the body (`list(...)`, `..1`, forwarding to another variadic)?

**Current stopgap:** `...` uses inside a body stay `Unknown`; only the signature-level rest parameter is bridged. Full `...` semantics (forwarding compat, `..N` access) is open.

## 6. NAMESPACE / import model

**Question.** How library scoping should work: `library(pkg)` attaches, `importFrom` in NAMESPACE, search-path order, masking warnings.

**Notes.** Beta ships `pkg::name` resolution against namespace-partitioned stubs. The attach/import model (what `library(dplyr)` makes visible, masking diagnostics) is undesigned; it determines when an unresolved name is *really* unresolved, so it directly feeds strict mode's usefulness on real projects.
