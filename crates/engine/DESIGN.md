# Engine — R0 design

The `engine` crate is the substrate for the analysis-engine rewrite onto a memoized-query model
(decision record `.agents/memory/decisions/incremental-architecture-and-recheck.md`, "REWRITE EXECUTION"). It
holds the **generic red-green memoization core only** — no R, no `analysis` dependency. The R queries are
layered on in R1+. This document is the crate's own design home (the module doc on `src/engine.rs` is the
condensed algorithm; this is the full plan).

Status: **R0 — substrate + design landed.** The core, its smoke tests, and this design exist; no real
query bodies yet.

---

## 1. Substrate: in-house red-green vs. salsa

**Decision (CTO/architect, not relitigated here): in-house red-green.** The spike (`643d85a` / promoted
here) proved a ~300-LoC core reproduces salsa's memoization behavior over a real R phase chain with a GO
verdict. Why in-house wins for Roughly:

- **R's static graph needs only the core, not the machinery.** salsa's bulk is for *deep, dynamic,
  multi-crate* graphs: proc-macro query definition, a trait/coherence layer, durability tiers, an
  interned-id world, a cross-crate dependency graph. R's analysis graph is shallow and almost entirely
  static (source → lower → naming → interface → check), one bounded fixed-point for re-exports. The part
  we actually need — *memoized queries with automatic dependency-tracked invalidation* — is exactly the
  ~200 lines in `engine.rs`. Importing the rest is machinery for its own sake.
- **Full control over the one hard query.** The package-interface fixed-point (cyclic, with
  `Unknown`-pinning on genuine re-export cycles, §5) is the single non-trivial query. Owning the engine
  lets the cycle interface live as a first-class query body shape we fully control, rather than bending it
  to salsa's cycle-recovery API.
- **Legibility and no external API churn.** The whole substrate is one readable file the team owns. salsa
  has a history of hard API breaks; the rewrite is already a large change without tracking an external
  framework's churn.

**Honest accounting — what salsa would have given for free, and our plan for each:**

| salsa feature | Status here | Plan |
| --- | --- | --- |
| Cancellation | **landed (R3)** | §6 — single-engine cooperative cancellation: an `Arc<AtomicBool>` token checked at `recompute` entry and the interface fixed-point's round boundaries; abandons by unwinding a `Cancelled` sentinel caught at `fetch_cancellable`, which resets the transient stack/computing state and commits no partial memo |
| Parallelism | **decided against for now** | §6 — single-engine, demand-driven; parallelism is a separable, evidence-driven later slice. A `Shared<T>` / memo-map alias is in place so a future retrofit is localized |
| Input removal / deletion | **in the core** | `remove_input` leaves a tombstone slot so dependents revalidate without re-executing an absent input (§2, §3) |
| LRU / memo eviction | not yet | deliberate later slice; `slot_count()` already exposes table size |
| Durability tiers | not adopted | the stdlib-stub input is "set once"; a coarse "high-durability input" marker is cheaper than salsa's tier system if measurement shows a need |

These are designed in deliberately, not hand-waved. The bar for cutover (decision record) requires
cancellation available and per-edit cost O(blast-radius); parallelism is reclassified from a cutover
prerequisite to an evidence-driven later optimization (§6), because the engine is demand-driven and has no
eager cold-check cost for it to parallelize.

---

## 2. The core (what exists now)

`src/engine.rs`, generic over a host-supplied `QueryGroup`:

- `Revision` — a `u32` newtype logical clock; `START` is the floor.
- `Stored` — a type-erased value (`Rc<dyn Any>`) plus a captured same-type equality `fn`. This is how the
  engine holds every query's value in one table yet still hands typed `Rc<T>` back, and how value-eq
  cutoff works without the engine knowing any concrete type.
- `QueryGroup` — host trait: `type Key` enumerates every query; `execute(&self, engine, key)` is the
  derived-query body dispatcher. `&self` carries host instrumentation (e.g. exec counters).
- `Engine<G>` — the database: `revision`, a single `slots: HashMap<Key, Slot>` table (inputs and derived
  unified), and a `dependency_stack` for runtime dependency recording.
  - `set_input` (`&mut self`) — bump revision, value-eq backdate `changed_at`.
  - `fetch::<T>` (`&self`) — record-as-dependency, validate, return `Rc<T>`.
  - `validate` — the red-green decision: green-by-revision / green-by-early-cutoff / red-recompute.
  - `recompute` — push a frame, run the body, collect recorded deps, apply cutoff propagation.

A `Slot` stores `value`, `verified_at`, `changed_at`, `dependencies`, `is_input`. The comparator is **not**
stored per slot — recompute always compares the new value against the old using the freshly produced
comparator (same query ⇒ same type), so there is one source of the equality function, not a mirrored copy.

The core assumes an **acyclic** recorded dependency graph; the one cyclic query is contained in its body
(§5). An **accidental** cycle — a derived body transitively fetching itself, the mistake R1 is most likely
to introduce — is caught by a "currently-computing" key set: re-entering a key already on the recompute
stack panics with `query cycle detected: <key> is already being computed` instead of overflowing the
stack. Domain cycle *recovery* (`Unknown`-pinning) stays inside the interface body (§5); this guard is only
the loud failure for graph mistakes.

### Input removal (now a core primitive)

`Engine::remove_input(key)` bumps the revision and replaces the input's slot with a **tombstone**: a
valueless input slot marked `changed_at = removal revision`. This is the minimal correct support for file
deletion (§3): a dependent that recorded the input revalidates and sees it changed (the tombstone's
`changed_at`) instead of recursing into a recompute of a now-absent input — which would hit the
"input queries are never executed" panic. A body that *fetches* a removed input directly is a host bug and
panics in `fetch`; the intended consumer is a fold over a *set* of inputs that, on revalidation, recomputes
against the now-smaller set and never fetches the removed key. The tombstone occupies the same slot 1:1
(no second mirrored entry); reclaiming it is an eviction concern, deferred. Smoke-tested by the `removal`
cases in `tests/test_engine.rs` (drop-from-fold and add→delete→re-add).

### Deliberately out of the core for R0

- Cancellation, parallelism, eviction (§6).

---

## 3. The query graph for the rewrite

The graph is **fine-grained at the package interface**. The naive shape — one `package_naming` node
producing the whole global table, with `typecheck(f) ← package_naming` — is wrong: any file's export change
advances `package_naming.changed_at`, so **every** `typecheck(f)` re-runs HM inference, O(package) per
edit, regressing M3's per-referrer precision. Value-eq cutoff does **not** save it, because the whole-table
value changes whenever any one global does. The fix is a **per-symbol interface layer** so a referrer
depends only on the symbols it actually reads.

```
                 +-- config --------------------------------------------+
                 |                                                       |
 source_text(f) -+-> parse(f) -> lower(f) -> local_naming(f) --+         |
                 |                                              |        |
 document_kind(f)+----------------------------------------------+        |
                                                                |        |
 project_files --+-------> package_symbol_index  <--------------+        |
 stdlib_stubs ---+              |   (the only all-files fold; names only) |
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
  edit does not invalidate via a kind read (the spike's lesson — match input granularity to reads).
- `project_files` — the workspace membership set: which `FileId`s exist. The engine does not enumerate its
  own inputs, so the file *set* is itself an input. Adding/removing a file is a `set_input(project_files, …)`
  (plus the file's own `source_text`/`document_kind` set or `remove_input`). This is the single source of
  truth for "which files exist" — there is no separate mirrored package/script set.
- `config` — project `roughly.toml` (`[check] typing/unused/strict`, …). Low churn.
- `stdlib_stubs` — the immutable stub library (base + CRAN). Set once; its `changed_at` never advances, so
  it never invalidates anything.

Queries (each edge is a recorded `fetch`, so the dependency is automatic):

| Query | Reads | Why / granularity |
| --- | --- | --- |
| `parse(f)` | `source_text(f)` | the tree is a pure function of the bytes |
| `lower(f)` | `parse(f)` | HIR is lowered from the tree |
| `local_naming(f)` | `lower(f)`, `document_kind(f)` | file-local resolution; also yields the file's **exported-name set** |
| `package_symbol_index` | `project_files`, each package file's `local_naming` export-name set, `stdlib_stubs` | the def-map: `name → winning defining/re-exporting item`. **Names only, no schemes.** The one all-files fold; changes only on *structural* edits (add/remove/rename a top-level binding, add/remove/reclassify a file), **not** on body edits |
| `defining_item(symbol)` | `package_symbol_index` | **firewall**: projects one symbol's winner out of the index. Value-eq cutoff per symbol — when the index changes because symbol *x*'s winner changed, `defining_item(s)` for *s ≠ x* re-projects to the same value and cuts off |
| `global_scheme(symbol)` | `defining_item(symbol)`, then the winning file's `lower`/local inference for that item (or, for an acyclic re-export `a <- b`, `global_scheme(b)`; for a re-export **cycle**, the SCC interface body, §5) | the per-symbol exported **scheme**. Editing a function body recomputes only *its* `global_scheme`, not a global fold |
| `typecheck(f)` | `lower(f)`, `local_naming(f)`, `config`, and `global_scheme(s)` for **each symbol `s` that `f` references** | HM inference over the file. Records a dependency on exactly the interface symbols it reads — nothing more |
| `diagnostics(f)` | `typecheck(f)`, `local_naming(f)`, `config` | rendered output; `config` gates typing/unused/strict |

### Why the per-symbol layer makes the "dissolve" claim true

The dissolve claim (below) holds **only at this granularity**:

- `typecheck(f)` records `global_scheme(s)` for precisely the symbols `f` references. When global `g`'s
  scheme changes, only `global_scheme(g).changed_at` advances; only the `typecheck(f)` memos that recorded
  `global_scheme(g)` revalidate. That recorded set **is** the M3 reverse-dependency index
  (`Symbol → {referrer}`), reconstructed automatically and exactly, with no mirror to patch and no drift
  oracle.
- The expensive work (HM inference in `typecheck`) is therefore blast-radius-bounded for the high-churn
  case: editing a function body changes one symbol's `global_scheme`, re-typechecks only its referrers.
- The remaining all-files fold, `package_symbol_index`, is **names only** and recomputes only on
  *structural* export edits — rare relative to keystrokes. Even then the `defining_item` firewall confines
  the blast: only symbols whose winner actually changed propagate to `global_scheme` and onward. A body
  edit does not touch the index at all (the file's exported-name *set* is unchanged, so `local_naming`'s
  index contribution is value-eq and cuts off).
- Crucially, `typecheck(f)` **never** reads `project_files` or `package_symbol_index` directly — it reaches
  the file set and the def-map only *behind* the per-symbol `global_scheme`/`defining_item` firewall. So no
  coarse all-files fold gates `typecheck`; adding an unrelated file cannot invalidate a file that does not
  reference any symbol whose winner changed.

### File addition, deletion, and reclassification (A2)

All three are input edits over `project_files` + per-file inputs; there is no separate mirrored file set to
keep in sync (the source of the hand-rolled script↔package drift bug).

- **Add file `f`.** `set_input(project_files, …∪{f})`, `set_input(source_text(f), …)`,
  `set_input(document_kind(f), …)`. `package_symbol_index` recomputes (its `project_files` dep changed);
  symbols `f` newly defines/wins flow through the firewall to `global_scheme`, and only referrers of those
  symbols revalidate. Files referencing nothing `f` changed are untouched.
- **Delete file `f`.** `set_input(project_files, …∖{f})` and `remove_input(source_text(f))` (and
  `document_kind(f)`). The `source_text(f)` tombstone makes `parse(f)`/`lower(f)`/`local_naming(f)` read as
  changed-and-absent for anything still holding them, but `package_symbol_index` — now folding a
  `project_files` without `f` — simply stops fetching `f`'s `local_naming`, so `f` drops out of winner
  selection. The per-symbol `global_scheme` queries for symbols `f` used to define recompute (their winner
  changed or vanished), and their referrers revalidate. `f`'s own `parse`/`lower`/… become dead memos
  (eviction is a later slice). No body ever fetches the removed `source_text(f)` directly, so the tombstone
  panic-on-fetch never fires.
- **Reclassify `f` (package ↔ script).** `set_input(document_kind(f), …)`. `package_symbol_index` folds a
  file's exports only when its `document_kind` is `Package`, so flipping to `Script` drops `f`'s exports
  from the index exactly as a deletion would for naming purposes, while `f`'s own `parse`/`lower` stay live
  (it is still an open script). This is expressed purely as an input change — no reclassification bookkeeping.

### How the hand-rolled M3/M4 structures dissolve

The whole point of the rewrite — every bespoke incremental structure becomes a consequence of recorded
deps + revisions, with no mirror to drift:

- **M3 reverse-dependency index** (`Symbol → {DocumentId}`, hand-patched, guarded by a debug drift
  oracle) → **recorded `global_scheme(symbol)` dependencies.** A referrer that read a global recorded a
  `fetch(global_scheme(s))`; when `s`'s scheme changes, `global_scheme(s).changed_at` advances and only the
  `typecheck` memos that recorded it re-run. The recorded per-symbol dependency set *is* the reverse index,
  maintained automatically. No reverse index to patch, no oracle to run.
- **Dirty-set + candidate selection** (`dirty ∪ documents_referencing(changed_globals)`) → **revision
  bumps + validation.** "Dirty" is just "input `changed_at` == current revision"; candidate selection is
  the validation walk discovering which memos a changed dependency reaches.
- **String dependency fingerprints + type fingerprints** (re-keying round-2 typecheck on a rendered hash
  of referenced schemes) → **value-eq early cutoff at `global_scheme`.** Instead of hashing the referenced
  schemes into a fingerprint string and comparing, each `global_scheme(s)` value either changed or did not;
  a referrer whose referenced symbols' schemes are unchanged cuts off before `typecheck` re-runs. The
  fingerprint was a hand-rolled stand-in for per-symbol value equality.
- **M4 incremental package-naming + incremental type index** (candidate indexes, materialized tables,
  winner/duplicate patching, five drift assertions) → **derived queries with cutoff.** The def-map
  (`package_symbol_index`) and the per-symbol `global_scheme` are derived queries; an edit that does not
  change a file's exported-name set produces an equal `local_naming` contribution, which cuts off before
  the index re-folds anything observable, and an edit that does not change a symbol's scheme cuts off at
  `global_scheme`. The per-name winner/duplicate logic stays as the *body* of `package_symbol_index`; what
  disappears is the mirrored incremental machinery and its oracles — the silent-stale bug class becomes
  structurally impossible (no mirror ⇒ nothing to drift).

---

## 4. Why this kills the silent-stale class

The 4+ silent-staleness bugs (decision record) all had the same shape: a hand-maintained mirror of the
true dependency graph that the *untracked* read path could bypass. In the query model the tracked path
(`fetch`) is the *only* path a body can read another query, and reading **is** recording. There is no
untracked default to forget. That is the structural guarantee the rewrite is buying.

---

## 5. The cyclic query: re-export interface fixed-point

R allows mutual typed re-exports (`a <- b`; `b <- a` across files), which form a genuine dependency cycle
the acyclic core cannot express. This is the one non-trivial query, and it is honest to say it **does not
dissolve** under the rewrite: it **relocates, verbatim**, from `analysis.rs` into a query body, carrying its
full correctness burden — the `#globals + slack` round cap and the period-2 oscillation guard — with it.

**Why it cannot just be per-symbol fetch recursion.** §3's `global_scheme(symbol)` resolves a *direct*
definition or an *acyclic* re-export `a <- b` by a plain `fetch(global_scheme(b))` — recorded, blast-radius,
value-eq cutoff, no global fold. But a genuine re-export *cycle* `a <- b`, `b <- a` would make
`global_scheme(a)` fetch `global_scheme(b)` fetch `global_scheme(a)` — re-entering a key already on the
recompute stack, which now (correctly) **panics via the accidental-cycle guard** (§2). The domain cycle must
therefore be resolved inside a single body that owns the whole strongly-connected component, never
re-entering `fetch` on its own key.

**Shape.** A `reexport_interface(scc)` query resolves one SCC of mutually-re-exporting symbols and produces
its converged scheme sub-table; `global_scheme(symbol)` for a symbol in a cycle projects from its SCC's
result (members outside any cycle never reach this query at all). The body is the synchronous fixed-point
the current `build_package_interface_table` already runs:

1. Iterate to a fixed point (Jacobi-style: each round computed from the previous round's table).
2. **Convergence guard (round cap).** Acyclic re-export/forward-ref chains are monotone — each scheme
   transitions at most once (`Unknown` → concrete) — so they converge in ≤ `#globals + 1` rounds. Bound the
   loop by that (with slack), exactly as the current code does after the round-cap bug fix. **Not** a
   smaller cap: a chain near the bound must converge, not truncate to a stale `Unknown`.
3. **`Unknown`-pinning for genuine cycles.** A pure re-export cycle is *non-monotone* — members oscillate
   (period-2 swap) and never settle. Port the oscillation guard from `analysis.rs` verbatim: a symbol whose
   rendering returns to an earlier value while differing from the previous round is on a cycle → pin it to
   `Unknown`, collapsing the cycle and restoring monotonicity so the loop converges.

In query terms the body is a self-contained fixed-point producing one `Stored` value (the SCC's converged
sub-table). Downstream `global_scheme`/`typecheck` depend on it normally; value-eq cutoff means a converged
result equal to the previous one stops propagation. A future refinement is to push cycle *detection* into
the core (an "in-progress" marker the validation stack already has, §2) and expose a recovery hook, but
pinning inside the body is the minimal correct mapping and reuses proven logic.

**Required R1 tests (focused, ship with the body):**

- a **monotone re-export chain** (`a <- b <- c …`) converges to the concrete schemes;
- a **genuine period-2 cycle** (`a <- b`, `b <- a`) pins its members to `Unknown` and converges (does not
  spin to the round cap);
- a **chain near the round bound** converges fully and is **not** truncated to a stale `Unknown`
  (the regression the round-cap bug fix closed).

---

## 6. Concurrency: single-engine, off-thread, cooperative cancellation, demand-driven

**Decision (CTO, recorded here with justification): a single engine, run off the main thread, with
cooperative cancellation and demand-driven (lazy) evaluation — NOT an `Arc`/parallel-from-the-start
engine.** The three reasons:

1. **A correct parallel red-green engine is research-grade.** Concurrent revalidation, cross-thread
   in-progress/cycle detection, and cancellation interleaving are exactly where memoized-query engines get
   subtle. The whole reason this rewrite exists is correctness-by-construction — eliminating the silent-stale
   class — and bolting on shared-mutable concurrency from day one trades that guarantee for the very class
   of bug we are removing, while fighting the legibility goal (the substrate is one readable file the team
   owns).
2. **Demand-driven evaluation shrinks parallelism's payoff.** The engine computes only what is *queried* —
   open files and their dependents. There is no eager O(300k) cold pass to fan out across cores; the cold
   cost is paid lazily, per query, as the editor asks. Parallelism would help only rare workspace-wide batch
   operations (format-all, a project-wide rename, an initial index build), which is a separable optimization
   to add later **on measured evidence**, not a precondition.
3. **Cancellation, not parallelism, delivers live responsiveness.** Part A's "latest edit wins" needs an
   in-flight cross-file pass on revision *N* to abandon cheaply when *N+1* arrives — a single-threaded
   property. With cancellation, a 300k-LoC workspace stays responsive single-threaded because each keystroke
   abandons the stale pass and the next pass recomputes only its blast radius.

**Cancellation (cooperative) — landed in R3.** `Engine::fetch_cancellable(key, token)` installs an
`Arc<AtomicBool>` token for the duration of the fetch; `Engine::check_cancelled()` observes it at every
`recompute` entry and at the §5 fixed-point's round boundaries (the one loop the per-`recompute` check
cannot reach, since it owns its whole component in a single body). An explicit flag was chosen over
"launch-revision `!=` current revision" because the off-thread driver flips the flag the moment a newer
edit arrives, independent of when the next `set_input` bumps the clock.

On cancellation the check **unwinds a `Cancelled` sentinel** (a typed panic) rather than threading a
`Result` through every query body: a body reads through the infallible `fetch` and uses the value directly,
so a `Result` would force every body and `fetch`'s signature to change — the opposite of additive. The
unwind is exactly the "sentinel that unwinds the `fetch` stack" this section always called for. Because
**nothing is committed until a body returns** (the slot is written only at the end of `recompute`), an
abandoned pass leaves *no* partial memo; the single `fetch_cancellable` catch point clears the only
transient state the unwound ancestors left — the dependency stack and the `computing` set — so the engine
is consistent for the next fetch. A non-`Cancelled` panic (the accidental-cycle guard) is re-raised
unchanged, so cancellation is strictly additive: with no token installed `check_cancelled` is a no-op and
the plain `fetch` path is byte-for-byte unchanged. (The raised `Cancelled` runs the process panic hook; a
host that cancels per keystroke installs a hook that ignores the `Cancelled` payload once at startup.) The
`&mut self`/`&self` split is still the precondition that no `fetch` overlaps a `set_input`. Covered by
`tests/test_cancellation.rs` (latest-edit-wins from another thread, consistent state, correct post-edit
result, cycle-guard-still-fires, no-token-unchanged).

**The `Shared<T>` hedge (implemented now).** So that a future parallel retrofit is a localized change rather
than a pervasive rewrite, the shared-pointer type and the memo table are kept behind thin aliases — today
`type Shared<T> = Rc<T>` and a single `RefCell<HashMap<…>>` memo map reached through one accessor seam.
Swapping to `Arc<T>` plus a concurrency-safe map (sharded/`RwLock` or lock-free) and a per-worker dependency
stack then touches only the storage and stack types, not the red-green algorithm. **Honest responsiveness
ceiling:** until that retrofit, workspace-wide batch operations run on one core; interactive edits do not
need it, because they are blast-radius-bounded and cancellable. We state this rather than implying free
parallelism.

---

## 7. Differential validation (R2 — the correctness proof)

The rewrite's correctness is proven **differentially against the production `analysis` crate**, with two
deliberate sharpenings over a naive "compare the two engines" check:

1. **Ground truth is `analysis`'s FULL from-scratch rebuild, not its incremental path.** The oracle is
   `run_full` on a *freshly built* `Analysis` for the current file set — never `analysis`'s own incremental
   recheck. This matters: the incremental path is exactly what carried the silent-stale bug class the
   rewrite exists to eliminate, so comparing against it could ratify a stale result on both sides. The
   invariant is `new_engine_output(after an edit stream) == analysis_full_rebuild_output(of the final
   state)`, per query phase (parse tree, HIR, naming, type errors, diagnostics).
2. **Randomized / coverage-guided over the whole corpus and adversarial edit streams.** This is not a finite
   fixture subset replayed once. A harness drives the new engine through *randomized* edit streams over the
   full soak/fixture corpus and the adversarial interleavings the decision record calls out —
   edit→query→edit, add→delete→re-add, package↔script flips — and after every edit asserts equality against a
   fresh full rebuild of the then-current state.

This is what "**subsumes the F1 differential fuzzer**" means precisely: F1's invariant
("incremental == full rebuild") survives as `new_engine_output == analysis_full_rebuild_output`, run as a
randomized edit stream — **not** collapsed into a one-shot finite fixture comparison. Because the new engine
produces the same results with correct-by-construction invalidation, the old drift oracles are retired only
once this randomized full-rebuild cross-check is the source of truth.

---

## 8. Phase plan (R1 → R3)

- **R0 — substrate + design (this).** Done means: `engine` crate builds and tests standalone; the generic
  red-green core (revision, input backdating, derived memoization, runtime dep recording, early cutoff,
  cutoff propagation) is product-quality; smoke tests prove memoization / invalidation / early cutoff /
  dependency recording; this design is recorded. `analysis` untouched and green.
- **R1 — wire the real query bodies.** Define the R `QueryGroup` (`Key` over the §3 graph) with bodies
  calling the kept tree-sitter parse + M2 HM core + naming + typecheck (query bodies, not rewrites),
  including the **per-symbol interface layer** (`package_symbol_index` → `defining_item` → `global_scheme`,
  §3) and the **re-export fixed-point body** with its three focused tests (§5: monotone chain converges;
  period-2 cycle pins to `Unknown` and converges; chain near the round bound is not truncated). Done means:
  the full chain `parse → … → diagnostics` runs through the engine on real R input. Duplication from
  `analysis` is allowed; no code sharing yet.
- **R2 — differential validation (§7).** Done means: new-engine output == `analysis` *full from-scratch
  rebuild* over randomized edit streams across the whole corpus (incl. the adversarial interleavings); the
  cross-check is green and is the new correctness gate.
- **R3 — cancellation (§6) and the per-edit cost measurement.** **Done:** cooperative cancellation is
  available and latest-edit-wins (§6, `tests/test_cancellation.rs`). Per-edit cost is measured by
  `tests/test_benchmark.rs` (committed, `#[ignore]` for the heavy sizes) over a synthetic cross-file package
  at 10k / 100k / 300k LoC, against `analysis`'s incremental path. **Findings, recorded honestly:**
  - *Recompute is O(blast-radius), flat in N* (the headline, proven by exec counters): a body edit re-runs
    exactly the edited file + its referrer's HM inference and triggers **zero** O(package) recomputation —
    `PackageSymbolIndex` does not re-fold (names-only cutoff) and `PackageTypeDefinitions` does not re-fold
    (a new declarations-only `TypeDefinitionsModule` view cutoff, the type-side analog of `ExportedNames`;
    it previously folded `Lower` directly and re-ran on every keystroke).
  - *Wall time is ~10–13× lower than the hand-rolled path at every size, but both scale ~linearly in N.*
    The engine is not flat in wall time: confirming the all-files folds are unchanged is an O(package)
    **validation walk** (one cheap hash-lookup + early-cutoff bump per file, no inference). A core fix
    landed here — `validate` now clones a memo's dependency list only when it will actually walk it, not on
    the green-by-revision/​input fast path, removing an O(fan-in × N) quadratic when a high-fan-in fold is
    revalidated from many callers. Driving the residual O(N) validation **sub-linear** is the remaining
    work: the durability / changed-input-tracking slice (§1) or sharded per-module def-maps. Production's
    O(package) term is HM re-inference + a per-round interface-table rebuild, far costlier per unit N, which
    is why the engine wins by an order of magnitude despite both growing.
  - **Eviction** remains a later slice (`slot_count()` exposes table size).

  **Parallelism is reclassified** out of the cutover bar to an evidence-driven later optimization for
  workspace-wide batch ops (§6), localized behind the `Shared<T>` / memo-table aliases. Production cutover
  is a separate, later decision made on this evidence.
