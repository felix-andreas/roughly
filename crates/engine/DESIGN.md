# Engine design

The `engine` crate is Roughly's incremental analysis substrate. It holds a **generic red-green
memoized-query core** — no R, no dependency on the `analysis` crate — and, layered on top, the R query
group (`queries.rs`) and the engine-backed IDE view (`ide_view.rs`). This document describes how the
engine works and why it is shaped this way. The module doc on `src/engine.rs` is the condensed algorithm;
this is the full picture.

The core property: an edit recomputes work proportional to its **blast radius**, not the size of the
project, and a structurally irrelevant edit (a comment, a same-length rename) stops propagating the moment
a value stops changing.

---

## 1. Substrate: an in-house red-green core

The substrate is a ~200-line in-house memoization core rather than an off-the-shelf framework such as
salsa, because R's analysis graph needs only the core, not the surrounding machinery:

- **R's graph is shallow and almost entirely static.** The chain is `source → parse → lower → naming →
  interface → check`, with one bounded fixed-point for re-exports. The part that is actually needed —
  *memoized queries with automatic dependency-tracked invalidation* — is exactly what `engine.rs`
  implements. A general framework's proc-macro query definitions, trait/coherence layer, durability tiers,
  interned-id world, and cross-crate dependency graph are for deep, dynamic, multi-crate graphs that R
  does not have.
- **Full control over the one hard query.** The package-interface fixed-point (cyclic, with
  `Unknown`-pinning on genuine re-export cycles — see §5) is the single non-trivial query. Owning the
  engine lets that cycle live as a first-class query-body shape rather than being bent to a framework's
  cycle-recovery API.
- **Legibility.** The whole substrate is one readable file with no external API surface to track.

What a general framework would provide out of the box, and how the engine handles each:

| Capability | Status | How |
| --- | --- | --- |
| Cancellation | present | §6 — single-engine cooperative cancellation via an `Arc<AtomicBool>` token, unwinding a `Cancelled` sentinel caught at `fetch_cancellable` |
| Input removal / deletion | present | `remove_input` leaves a tombstone slot so dependents revalidate without re-executing an absent input (§2, §3) |
| Parallelism | not implemented | single-engine, demand-driven; a `Shared<T>` / memo-map alias localizes a future retrofit (§6) |
| Memo eviction | not implemented | `slot_count()` exposes table size; a possible later addition |
| Durability tiers | not adopted | the stdlib-stub input is set once and never invalidates; a coarse "high-durability input" marker would suffice if measurement showed a need |

---

## 2. The core

`src/engine.rs`, generic over a host-supplied `QueryGroup`:

- `Revision` — a `u32` newtype logical clock; `START` is the floor.
- `Stored` — a type-erased value (`Rc<dyn Any>`) plus a captured same-type equality `fn`. This is how the
  engine holds every query's value in one table yet still hands typed `Rc<T>` back, and how value-equality
  cutoff works without the engine knowing any concrete type.
- `QueryGroup` — host trait: `type Key` enumerates every query; `execute(&self, engine, key)` is the
  derived-query body dispatcher. `&self` carries host instrumentation (for example exec counters).
- `Engine<G>` — the database: `revision`, a single `slots: HashMap<Key, Slot>` table (inputs and derived
  unified), and a `dependency_stack` for runtime dependency recording.
  - `set_input` (`&mut self`) — bump the revision, value-equality backdate `changed_at`.
  - `fetch::<T>` (`&self`) — record-as-dependency, validate, return `Rc<T>`.
  - `validate` — the red-green decision: green-by-revision / green-by-early-cutoff / red-recompute.
  - `recompute` — push a frame, run the body, collect recorded dependencies, apply cutoff propagation.

A `Slot` stores `value`, `verified_at`, `changed_at`, `dependencies`, `is_input`. The comparator is **not**
stored per slot — recompute always compares the new value against the old using the freshly produced
comparator (same query ⇒ same type), so there is one source of the equality function, not a mirrored copy.

### The red-green algorithm

A single global `Revision` counter is the logical clock, bumped on every `set_input`. Each memo records,
in revision units, when it was last *verified* still-valid (`verified_at`) and when its value last
*changed* (`changed_at`), plus the queries it read. On `fetch`:

1. **Green (trivial):** if `verified_at == current revision`, return the cached value.
2. **Green (early cutoff):** otherwise deep-validate the recorded dependencies. If none *changed* after
   this memo's `verified_at`, nothing it read is different — bump `verified_at` to the current revision and
   return the cached value **without re-running the body**.
3. **Red (recompute):** some dependency changed, so re-run the body. If the new value equals the old one,
   keep the old `changed_at` so the change does **not** propagate downstream (*cutoff propagation*);
   otherwise record `changed_at = current revision`.

Inputs get the same treatment at the source: `set_input` **backdates** `changed_at` when the new value
equals the old, so a no-op re-set leaves every dependent green without running a single body.

### Acyclicity and the accidental-cycle guard

The core assumes an **acyclic** recorded dependency graph; the one genuine cyclic query is contained in
its own body (§5). An **accidental** cycle — a derived body transitively fetching itself — is caught by a
"currently-computing" key set: re-entering a key already on the recompute stack panics with
`query cycle detected: <key> is already being computed` instead of overflowing the stack. Domain cycle
*recovery* (`Unknown`-pinning) stays inside the interface body (§5); this guard is only the loud failure
for graph mistakes.

### Fetch-spine depth

An *acyclic* chain still nests on the host stack: fetch → `validate` → `recompute` → body → fetch …, one
level per dependency link, and the link count tracks the user's program (a re-export chain is one query
level per link), which no fixed thread stack can bound. Every deepening passes through `validate`, so
`validate` is the single guard point: within a red zone of overflow it grows the stack
(`stacker::maybe_grow`) instead of aborting, sized so the frames a body can push between two `validate`
entries (bounded by the lowering recursion cap) always fit. A mechanically generated deep chain slows
down; it never kills the process.

### Memory bound

Derived keys are minted per file **and per symbol**, so a long editing session accretes memo slots
nothing will fetch again (a global typed character by character mints per-symbol keys per keystroke;
a deleted file leaves its whole derived chain). `Engine::evict_stale_memos(keep_revisions)` is the
bound: it drops every **derived** slot not read within the window — `verified_at` is a faithful
last-used signal because validating a key green refreshes it — and a dropped slot simply recomputes
on its next fetch (a missing slot is "never computed"), so eviction is a pure memory/latency trade.
Inputs and tombstones are never evicted: an input cannot be recomputed on demand, and a tombstone
must outlive every stale dependency edge pointing at it. The LSP server sweeps on a fixed cadence of
input writes with a window far above any feature's revalidation rhythm. The interner stays
append-only by design (symbol ids are shared across values); its growth is bytes per distinct name
ever seen, accepted as negligible against the memo table.

### Input removal

`Engine::remove_input(key)` bumps the revision and replaces the input's slot with a **tombstone**: a
valueless input slot marked `changed_at = removal revision`. This is the minimal correct support for file
deletion (§3): a dependent that recorded the input revalidates and sees it changed (the tombstone's
`changed_at`) instead of recursing into a recompute of a now-absent input — which would hit the
"input queries are never executed" panic. A body that *fetches* a removed input directly is a host bug and
panics in `fetch`; the intended consumer is a fold over a *set* of inputs that, on revalidation, recomputes
against the now-smaller set and never fetches the removed key. The tombstone occupies the same slot 1:1;
reclaiming it is an eviction concern.

---

## 3. The R query graph

The graph is **fine-grained at the package interface**. The naive shape — one `package_naming` node
producing the whole global table, with `typecheck(f) ← package_naming` — is wrong: any file's export
change advances `package_naming.changed_at`, so **every** `typecheck(f)` re-runs HM inference, O(package)
per edit. Value-equality cutoff does not save it, because the whole-table value changes whenever any one
global does. The fix is a **per-symbol interface layer** so a referrer depends only on the symbols it
actually reads.

```
                 +-- config --------------------------------------------+
                 |                                                       |
 source_text(f) -+-> parse(f) -> lower(f) -> local_naming(f) --+         |
                 |                                              |        |
 document_kind(f)+----------------------------------------------+        |
                                                                |        |
 project_files --+-------> package_symbol_index  <--------------+        |
                                |   (the only all-files fold; names only) |
                                v                                        |
                        defining_item(symbol)   <-- firewall, one symbol |
                                |                                        |
                                v                                        |
                        global_scheme(symbol)   (per-symbol type; SCC    |
                                |                 cycles via §5 body)     |
                                v                                        |
                          typecheck(f) <-- global_scheme(s) for each s f references
                                |                                        |
                                v                                        |
                          diagnostics(f) <----------------------------- config
```

Inputs (set from outside, never computed):

- `source_text(f)` — per-file source. The high-churn input; every keystroke is a `set_input`.
- `document_kind(f)` — package vs. script classification, a *separate fine-grained input* so a text-only
  edit does not invalidate via a kind read (input granularity must match reads).
- `project_files` — the workspace membership set: which `FileId`s exist. The engine does not enumerate its
  own inputs, so the file *set* is itself an input. Adding/removing a file is a `set_input(project_files,
  …)` (plus the file's own `source_text`/`document_kind` set or `remove_input`). This is the single source
  of truth for which files exist — there is no separate mirrored package/script set.
- `config` — project `roughly.toml` (`[check] typing/unused/strict`, …). Low churn.
- `stdlib_stubs` — the immutable stub library (base + project overrides). Not an engine input at all:
  it is set-once ambient state on the query group, outside the dependency graph, so it can never
  invalidate anything (routing stub edits through the graph is explicitly out of scope).

Queries (each edge is a recorded `fetch`, so the dependency is automatic):

| Query | Reads | Why / granularity |
| --- | --- | --- |
| `parse(f)` | `source_text(f)` | the tree is a pure function of the bytes |
| `lower(f)` | `parse(f)` | HIR is lowered from the tree |
| `local_naming(f)` | `lower(f)`, `document_kind(f)` | file-local resolution; also yields the file's **exported-name set** |
| `package_symbol_index` | `project_files`, each package file's `local_naming` export-name set | the def-map: `name → winning defining/re-exporting item`. **Names only, no schemes.** The one all-files fold; changes only on *structural* edits (add/remove/rename a top-level binding, add/remove/reclassify a file), **not** on body edits |
| `defining_item(symbol)` | `package_symbol_index` | **firewall**: projects one symbol's winner out of the index. Value-equality cutoff per symbol — when the index changes because symbol *x*'s winner changed, `defining_item(s)` for *s ≠ x* re-projects to the same value and cuts off |
| `global_scheme(symbol)` | `defining_item(symbol)`, then the winning file's `lower`/local inference for that item (or, for an acyclic re-export `a <- b`, `global_scheme(b)`; for a re-export **cycle**, the SCC interface body, §5) | the per-symbol exported **scheme**. Editing a function body recomputes only *its* `global_scheme`, not a global fold |
| `typecheck(f)` | `lower(f)`, `local_naming(f)`, `config`, and `global_scheme(s)` for **each symbol `s` that `f` references** | HM inference over the file. Records a dependency on exactly the interface symbols it reads — nothing more |
| `diagnostics(f)` | `typecheck(f)`, `local_naming(f)`, `config` | rendered output; `config` gates typing/unused/strict |

### Why the per-symbol layer bounds the blast radius

- `typecheck(f)` records `global_scheme(s)` for precisely the symbols `f` references. When global `g`'s
  scheme changes, only `global_scheme(g).changed_at` advances; only the `typecheck(f)` memos that recorded
  `global_scheme(g)` revalidate. That recorded set **is** a reverse-dependency index
  (`Symbol → {referrer}`), reconstructed automatically and exactly, with no mirror to patch.
- The expensive work (HM inference in `typecheck`) is therefore blast-radius-bounded for the high-churn
  case: editing a function body changes one symbol's `global_scheme` and re-typechecks only its referrers.
- The remaining all-files fold, `package_symbol_index`, is **names only** and recomputes only on
  *structural* export edits — rare relative to keystrokes. Even then the `defining_item` firewall confines
  the blast: only symbols whose winner actually changed propagate to `global_scheme` and onward. A body
  edit does not touch the index at all (the file's exported-name *set* is unchanged, so `local_naming`'s
  index contribution is value-equal and cuts off).
- For **package files**, `typecheck(f)` never reads `project_files` or `package_symbol_index` directly —
  it reaches the file set and the def-map only *behind* the per-symbol `global_scheme`/`defining_item`
  firewall. So no coarse all-files fold gates a package file's `typecheck`; adding an unrelated file
  cannot invalidate a package file that does not reference any symbol whose winner changed.
- **Scripts are the exception**: a script's inference fetches `project_files` and each file's
  `document_kind` directly (it must know the package universe to resolve globals), so any file
  add/remove/reclassification re-infers every open script in full. Acceptable while open-script counts
  are small; narrowing it behind a membership firewall is an open follow-up.

### File addition, deletion, and reclassification

All three are input edits over `project_files` plus per-file inputs; there is no separate mirrored file set
to keep in sync.

- **Add file `f`.** `set_input(project_files, …∪{f})`, `set_input(source_text(f), …)`,
  `set_input(document_kind(f), …)`. `package_symbol_index` recomputes (its `project_files` dependency
  changed); symbols `f` newly defines/wins flow through the firewall to `global_scheme`, and only referrers
  of those symbols revalidate. Files referencing nothing `f` changed are untouched.
- **Delete file `f`.** `set_input(project_files, …∖{f})` and `remove_input(source_text(f))` (and
  `document_kind(f)`). The `source_text(f)` tombstone makes `parse(f)`/`lower(f)`/`local_naming(f)` read as
  changed-and-absent for anything still holding them, but `package_symbol_index` — now folding a
  `project_files` without `f` — simply stops fetching `f`'s `local_naming`, so `f` drops out of winner
  selection. The per-symbol `global_scheme` queries for symbols `f` used to define recompute (their winner
  changed or vanished), and their referrers revalidate. `f`'s own `parse`/`lower`/… become dead memos. No
  body ever fetches the removed `source_text(f)` directly, so the tombstone panic-on-fetch never fires.
- **Reclassify `f` (package ↔ script).** `set_input(document_kind(f), …)`. `package_symbol_index` folds a
  file's exports only when its `document_kind` is `Package`, so flipping to `Script` drops `f`'s exports
  from the index exactly as a deletion would for naming purposes, while `f`'s own `parse`/`lower` stay live
  (it is still an open script). This is expressed purely as an input change — no reclassification
  bookkeeping.

---

## 4. Why recorded dependencies are the only read path

The correctness guarantee is that the tracked path (`fetch`) is the *only* path a body can read another
query, and reading **is** recording. There is no untracked default a body could bypass to read a stale
value without recording the dependency. A hand-maintained mirror of the dependency graph (a reverse-index
patched separately from the reads it mirrors) can drift out of sync with the reads; recorded dependencies
cannot, because they *are* the reads.

---

## 5. The cyclic query: re-export interface fixed-point

R allows mutual typed re-exports (`a <- b`; `b <- a` across files), which form a genuine dependency cycle
the acyclic core cannot express. This is the one non-trivial query. It does not reduce to per-symbol fetch
recursion; it lives as a single body that owns the whole strongly-connected component.

**Why not per-symbol fetch recursion.** `global_scheme(symbol)` resolves a *direct* definition or an
*acyclic* re-export `a <- b` by a plain `fetch(global_scheme(b))` — recorded, blast-radius-bounded,
value-equality cutoff, no global fold. But a genuine re-export *cycle* `a <- b`, `b <- a` would make
`global_scheme(a)` fetch `global_scheme(b)` fetch `global_scheme(a)` — re-entering a key already on the
recompute stack, which the accidental-cycle guard (§2) correctly panics on. The domain cycle must
therefore be resolved inside one body that never re-enters `fetch` on its own key.

**Shape.** A `reexport_interface(scc)` query resolves one SCC of mutually-re-exporting symbols and produces
its converged scheme sub-table; `global_scheme(symbol)` for a symbol in a cycle projects from its SCC's
result (members outside any cycle never reach this query). The body is a synchronous fixed-point:

1. Iterate to a fixed point (Jacobi-style: each round computed from the previous round's table).
2. **Convergence guard (round cap).** Acyclic re-export/forward-reference chains are monotone — each scheme
   transitions at most once (`Unknown` → concrete) — so they converge in ≤ `#globals + 1` rounds. Bound the
   loop by that (with slack). A chain near the bound must converge, not truncate to a stale `Unknown`, so
   the cap must not be smaller.
3. **`Unknown`-pinning for genuine cycles.** A pure re-export cycle is *non-monotone* — members oscillate
   (period-2 swap) and never settle. A symbol whose rendering returns to an earlier value while differing
   from the previous round is on a cycle → pin it to `Unknown`, collapsing the cycle and restoring
   monotonicity so the loop converges.

Downstream `global_scheme`/`typecheck` depend on the SCC result normally; value-equality cutoff means a
converged result equal to the previous one stops propagation.

Test coverage (`tests/test_reexport.rs`):

- a **monotone re-export chain** (`a <- b <- c …`) converges to the concrete schemes;
- a **genuine period-2 cycle** (`a <- b`, `b <- a`) pins its members to `Unknown` and converges (does not
  spin to the round cap);
- a **chain near the round bound** converges fully and is **not** truncated to a stale `Unknown`.

---

## 6. Concurrency: single-engine, off-thread, cooperative cancellation, demand-driven

The engine is a **single engine, run off the main thread, with cooperative cancellation and demand-driven
(lazy) evaluation** — not a parallel-from-the-start engine. Three reasons:

1. **A correct parallel red-green engine is research-grade.** Concurrent revalidation, cross-thread
   in-progress/cycle detection, and cancellation interleaving are where memoized-query engines get subtle.
   Shared-mutable concurrency would trade the correctness-by-construction guarantee for exactly the class
   of bug the design removes, and fight the legibility goal.
2. **Demand-driven evaluation shrinks parallelism's payoff.** The engine computes only what is *queried* —
   open files and their dependents. There is no eager cold pass to fan out across cores; the cold cost is
   paid lazily, per query, as the editor asks. Parallelism would help only rare workspace-wide batch
   operations (format-all, a project-wide rename, an initial index build), a separable optimization to add
   later on measured evidence.
3. **Cancellation, not parallelism, delivers live responsiveness.** Latest-edit-wins needs an in-flight
   cross-file pass on revision *N* to abandon cheaply when *N+1* arrives — a single-threaded property. With
   cancellation, a large workspace stays responsive single-threaded because each keystroke abandons the
   stale pass and the next pass recomputes only its blast radius.

**Cancellation (cooperative).** `Engine::fetch_cancellable(key, token)` installs an `Arc<AtomicBool>` token
for the duration of the fetch; `Engine::check_cancelled()` observes it at every `recompute` entry and at
the §5 fixed-point's round boundaries (the one loop the per-`recompute` check cannot reach, since it owns
its whole component in a single body). An explicit flag is used rather than "launch revision ≠ current
revision" because the off-thread driver flips the flag the moment a newer edit arrives, independent of when
the next `set_input` bumps the clock.

On cancellation the check **unwinds a `Cancelled` sentinel** (a typed panic) rather than threading a
`Result` through every query body: a body reads through the infallible `fetch` and uses the value directly,
so a `Result` would force every body and `fetch`'s signature to change. Because **nothing is committed
until a body returns** (the slot is written only at the end of `recompute`), an abandoned pass leaves *no*
partial memo; the single `fetch_cancellable` catch point clears the only transient state the unwound
ancestors left — the dependency stack and the `computing` set — so the engine is consistent for the next
fetch. A non-`Cancelled` panic (the accidental-cycle guard) is re-raised unchanged, so cancellation is
strictly additive: with no token installed `check_cancelled` is a no-op and the plain `fetch` path is
unchanged. (The raised `Cancelled` runs the process panic hook; a host that cancels per keystroke installs
a hook that ignores the `Cancelled` payload once at startup.) The `&mut self`/`&self` split remains the
precondition that no `fetch` overlaps a `set_input`. Covered by `tests/test_cancellation.rs`.

**The `Shared<T>` hedge.** So a future parallel retrofit is a localized change rather than a pervasive
rewrite, the shared-pointer type and the memo table are kept behind thin aliases — `type Shared<T> =
Rc<T>` and a single `RefCell<HashMap<…>>` memo map reached through one accessor seam. Swapping to `Arc<T>`
plus a concurrency-safe map and a per-worker dependency stack then touches only the storage and stack
types, not the red-green algorithm. Until such a retrofit, workspace-wide batch operations run on one core;
interactive edits do not need it, because they are blast-radius-bounded and cancellable.

---

## 7. Differential validation

Correctness is validated **differentially against the `analysis` crate's full from-scratch rebuild**, with
two deliberate sharpenings over a naive "compare two engines" check:

1. **Ground truth is `analysis`'s FULL from-scratch rebuild, not its incremental path.** The oracle is
   `run_full` on a *freshly built* `Analysis` for the current file set. Comparing against an incremental
   path could ratify a stale result on both sides; a full rebuild cannot. The invariant is
   `engine_output(after an edit stream) == analysis_full_rebuild_output(of the final state)`, per query
   phase (parse tree, HIR, naming, type errors, diagnostics).
2. **Randomized over the whole corpus and adversarial edit streams.** A harness drives the engine through
   *randomized* edit streams over the full fixture corpus and adversarial interleavings — edit→query→edit,
   add→delete→re-add, package↔script flips — and after every edit asserts equality against a fresh full
   rebuild of the then-current state.

Covered by `tests/test_differential.rs` (and `tests/test_ide_differential.rs` for the IDE view).

---

## 8. Performance characteristics

Measured by `tests/test_benchmark.rs` (`#[ignore]` for the heavy sizes) over a synthetic cross-file package
at 10k / 100k / 300k LoC:

- **Recompute is O(blast-radius), flat in N** (proven by exec counters): a body edit re-runs exactly the
  edited file plus its referrers' HM inference and triggers **zero** O(package) recomputation.
  `package_symbol_index` does not re-fold (names-only cutoff) and the package type-definitions view does
  not re-fold (a declarations-only view cutoff, the type-side analog of the exported-name set).
- **Wall time scales roughly linearly in N**, because confirming the all-files folds are unchanged is an
  O(package) **validation walk** — one cheap hash-lookup plus early-cutoff bump per file, no inference.
  `validate` clones a memo's dependency list only when it will actually walk it, not on the
  green-by-revision/input fast path, avoiding an O(fan-in × N) blow-up when a high-fan-in fold is
  revalidated from many callers. Driving the residual O(N) validation sub-linear (durability /
  changed-input tracking, or sharded per-module def-maps) is possible future work.
- **Eviction** is not implemented (`slot_count()` exposes table size).

Parallelism (§6) is a possible later optimization for workspace-wide batch operations, localized behind the
`Shared<T>` / memo-table aliases.
