# Open type-system design questions

Forward-looking design space for Roughly's type system: questions that are **not yet decided** and the options on the table, with the current stopgap and why. This is distinct from `decisions.md` (the log of what has been *settled*) — entries here are live deliberations. When one is resolved, move the decision + rationale to `decisions.md` and delete it here. The settled contract is always `docs/src/content/docs/typing-reference.md`; nothing here is contract until it lands there.

## 1. Generic / array-like element types (`T[]`)

**Question.** Should the type system express a shape-preserving generic over vectors — `<T> fn(x: T[]) -> T[]` (for `rev`, `sort`, `head`, `tail`, `unique`, `abs`, `sqrt`, …) — and if so, how?

**Why it is not trivial.**
- The core type cannot represent it today: `CoreType::Vector(Atomic)` / `NamedVector(Atomic)` store a bare `Atomic` enum, not a type, so "vector of `T`" has nowhere to land after lowering. The surface parser deliberately rejects `T[]` (`type_syntax.rs`, "generic atomic vector suffix types are not supported yet").
- **`T[]` is not sound for an arbitrary `T`.** Not every R type is array-like: atomic vectors hold atomic scalars; lists, closures, environments, S4 objects are not vector elements in the same sense. A naive `T[]` over any `T` would admit ill-formed element types. Whatever we adopt must constrain `T` to the things that *can* be a vector element.

**Options.**
- **(a) Generalize the core vector + an atomic-element constraint.** `CoreType::Vector(Box<CoreType>)` (and `NamedVector`) so an element can be a variable, plus a new constraint kind (analogous to the existing `<T: numeric>`) that proves `T` is an admissible element. High blast radius: touches every construction/match on `Vector(Atomic)` in `typecheck.rs`, compatibility, unification, variance, rendering, and a large fixture sweep; reopens element-variance questions the reference only partly settled.
- **(b) A trait / typeclass mechanism.** A general "is a vector element" (and more broadly, capability) predicate. Heavier machinery, but see §2 — one trait mechanism could serve both this and ad-hoc overloading, which argues for designing them together rather than bolting on (a).
- **(c) Status quo — concrete element types only.** Keep the fixed atomic element vectors (`numeric[]`, `character[]`, …); shape-preserving stdlib functions stay `Any` (their calls are already safe — never a false error).

**Current stopgap:** (c). The affected functions are `Any` in the stdlib corpus. Chosen because (a) is a broad, regression-prone change to the core type relation for low corpus yield, and (b) deserves a deliberate design rather than being forced by a handful of functions.

## 2. Ad-hoc overloading vs traits

**Question.** How do we type functions whose result type depends on the argument type — `abs`, `rep`, `seq`, `range`, and similar — where a single scheme can't be precise?

**Options.**
- **Overload sets.** Allow multiple type schemes per name; resolve a call by the first argument-compatible scheme. The `.Rti` grammar already permits repeated declarations of one name (loader last-wins) so a corpus need not be rewritten when this lands.
- **Traits / typeclasses.** A class mechanism (e.g. a `Numeric`-like class carrying `abs`/arithmetic) with associated method types; a call resolves through the class. More expressive and composable; larger design.

**Interaction with §1.** The user's steer was explicitly open between "a generic vector type or a trait or whatever." A trait/typeclass system could subsume both the array-element constraint (§1) *and* ad-hoc overloading (§2) under one mechanism. Before committing to overload sets, evaluate whether a single trait design pays for both — that is the more likely world-class shape.

**Current stopgap:** ad-hoc-overloaded functions are `Any` for v1; genuinely parametric higher-order functions (`lapply`, `Map`, `Reduce`, `identity`) keep real `<T> fn(...)` generics (already supported).

## Landed (no longer open)

- **Variadic `...` in annotations/stubs** and **dotted parameter names** (`na.rm`) are implemented; their semantics are in `typing-reference.md`. Variadic is effectively stub/declaration-only for now — annotating a rest parameter over an R `function(...)` body reports a spurious mismatch because inference still lowers `...` as an ordinary named parameter; whether to bridge that (an inference change, or a compat special-case for a trailing `...` parameter) is a deferred decision noted in `backlog.md`, not yet a committed design.
