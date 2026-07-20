# NSE — working draft

Ideas-in-progress for checking non-standard evaluation (data masking). The settled
framing — the four-step design ladder and the precedent survey — lives in
`.agents/memory/typing-design.md` §7; this file is the sketchpad where concrete
designs get drafted before they graduate into that record or the typing reference.
**data.table is the pressing target.**

## Where we stand (implemented, sound-by-refusal)

- `quiet_reads` (naming): reads inside a masked context are never reported
  "unresolved", but still keep their lexical definers alive.
- `masked_subsets` (naming): a `[` bracket carrying an unambiguous data.table
  signature — `by =`/`keyby =`, a `:=` call, `.()`, or a `.SD`-family special
  (`.SD`, `.N`, `.I`, `.BY`, `.GRP`, `.EACHI`) — masks its index arguments;
  with an unknown subject the whole bracket types `Unknown`.
- The stub corpus's `@masked` verb set: variadic callees whose `...` is
  data-masked (dplyr verbs, the `with()` family).
- **Idea 1 below is SHIPPED** (typing-reference "Data-masked evaluation" is the
  contract; the decision record has the shape): a conditional `data.table`
  stub namespace (active only when the project declares or attaches the
  package) gives values the `data.table` class; a bracket whose subject IS
  that nominal masks all its index reads (typed-subject masking — no
  syntactic marker needed) and classifies its result by `j`'s syntax
  (filters/joins/`:=`/`.()`/`list()`/grouped `j` keep the class; the rest
  refuse as `Unknown`).

Zero false positives, zero checking inside the mask. Every idea below must keep
that property: unknown masking constructs stay silent, never guessed.

## data.table

### The surface we must model

`DT[i, j, by]` is a query language in one bracket:

| Piece | Meaning | Static handle |
| --- | --- | --- |
| `i` | row filter / join (`x > 3`, another DT, `on =`) | masked expression over columns |
| `j` | select / compute / assign (`x`, `.(m = mean(y))`, `x := …`) | decides the **result shape** |
| `by` / `keyby` | grouping | changes what `.N`/`.SD` mean, not the result class |
| specials | `.SD`, `.N`, `.I`, `.BY`, `.GRP`, `.EACHI`, `.SDcols` | vocabulary injected by data.table itself |
| `:=` | **assignment by reference** — mutates `DT` in place | the subject's type *evolves* across statements |
| `set*()` | `setnames`, `setkey`, `setDT`, `set()` | same in-place evolution, ordinary call syntax |
| chaining | `DT[…][…]` | result shape of one bracket feeds the next |

### Idea 1 — result-shape classifier for `j` (no column knowledge needed)

Today the whole bracket is `Unknown`. But the *class* of the result is largely
decided by the syntax of `j`, before we know any column:

- `j` absent (`DT[i]`) → same class as the subject (`data.table`).
- `j` is `.(…)` or `list(…)` → `data.table`.
- `j` is a `:=` call → the subject's class (returned invisibly), plus the
  in-place evolution noted below.
- `j` is a bare column name (`DT[, x]`) → a vector — but *which* vector needs
  columns, so: `Unknown` (still better: we know it is *not* a data.table, which
  a `nominal-not` refinement could carry once unions support it — probably not
  worth machinery yet).
- anything else (calls, `with = FALSE`, character `j`, `..var`) → `Unknown`.

This is shippable now as a small, sound upgrade: type `DT[i]`,
`DT[, .(…)]`, and `DT[, x := …]` as `data.table` (nominal) instead of
`Unknown`, refuse the rest exactly as today. It makes chains
(`DT[a > 1][, .(m = mean(b)), by = g]`) keep their class end-to-end, which is
what downstream code branches on.

### Idea 2 — `:=` and the evolution problem

`DT[, y := x * 2]` adds a column *in place*. A column-aware checker must treat
this like our loop-carried/top-level rebinding logic: the binding's type after
the statement is the old row type extended with `y`. Consequences to draft:

- Aliasing: `DT2 <- DT; DT2[, y := 1]` also changes `DT`. Honest options:
  (a) refuse column-level claims after an alias escapes, (b) treat
  `data.table` column sets as lower bounds only ("has at least these columns").
  Lower-bounds is the pandas-stubs-shaped compromise and probably right:
  membership checks stay useful, exactness is never claimed.
- Deletion (`DT[, y := NULL]`) breaks lower-bound monotonicity — under (b) a
  delete must widen the whole binding back to column-unknown, or track exact
  sets only in linear (no-alias-escape) regions.
- `set*()` functions are the same problem in ordinary call syntax; their stubs
  can carry the same contract when the contract language (ladder step 1) lands.

### Idea 3 — column vocabulary sources (ladder step 2 applied to data.table)

Where column knowledge can come from, cheapest first:

1. Literal constructors: `data.table(x = 1:3, y = "a")`,
   `as.data.table(list(...))` — exact names, exact element types.
2. `:=` with a literal LHS (name, `c("a","b")` character vector) — extends the
   set; `` `:=`(a = …, b = …) `` functional form too.
3. Annotations: `#: DT: data.table` today; once question 3 (data.frame
   modeling) settles a row-type syntax, `#: DT: data.table<x: integer[], …>` —
   the escape hatch for `fread()` and friends, which are opaque statically
   (F#-style compile-time schema import is out of scope).
4. Joins/`melt`/`dcast` — derived shapes; late, hard, low priority.

With a vocabulary, the first checkable facts inside the mask are *membership*
(column typos — the dominant real-world NSE bug) for bare names in `i`/`j`/`by`,
with the lexical environment as fallback (data.table looks up unmatched names
lexically), so a name is flagged only when it is in *neither* scope — preserving
zero false positives.

### Suggested sequencing for data.table

1. ~~Result-shape classifier (idea 1)~~ — **DONE**, including the conditional
   stub namespace and typed-subject masking it needed to be usable (fixture
   group `datatable` in the typing-imports suite pins the behavior).
2. Masking contracts in the stub language (ladder step 1) so `@masked` and the
   bracket heuristic move into per-package stub metadata (`data.table`'s stub
   declares the bracket semantics and the `set*()` contracts).
3. Column vocabulary + membership checks (idea 3, gated on question 3's
   row-type design), with `:=` evolution under the lower-bound model (idea 2).

## dplyr (second)

- The verb set is already suppressed via `@masked`. Contracts (step 1) give the
  data-argument/masked-argument pairing; membership checks then reuse the same
  column-vocabulary machinery, with `.data$x` / `.env$x` pronouns resolving
  exactly.
- Result shapes are *harder* than data.table's: `mutate` extends, `summarise`
  collapses groups, `select` projects — each needs record extension/projection
  on the row type. Same machinery as `:=` evolution, but per-verb in stubs, not
  in bracket syntax.
- Tidy-eval injection (`!!`, `{{ }}`, `across()`) stays refused indefinitely.

## ompr / builder EDSLs (suppress only)

No data context exists at the call site — `add_variable(model, x[i], i = 1:10)`
declares `x` into the model object. Names live in a value, not a frame. Keep
quiet-read suppression; a package-specific extension could thread declared names
through the builder chain's return type, but that is bespoke per package and not
on any near-term path.

## Open questions

- ~~Where does the result-shape classifier live?~~ Settled: the checker
  (`infer_index`, next to the vector-`[` rules), keyed on the `data.table`
  nominal; migrate into stub contracts when the contract language (step 2)
  exists.
- Lower-bound column sets vs. exact sets in linear regions — pick one before
  idea 3; lower-bound is the current lean.
- Annotation syntax for row types is owned by design question 3 — NSE should
  consume whatever it settles, not invent a second syntax.
