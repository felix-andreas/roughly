# Engine — R0 design

The `engine` crate is the substrate for the analysis-engine rewrite onto a memoized-query model
(decision record `.agents/decisions/incremental-architecture-and-recheck.md`, "REWRITE EXECUTION"). It
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
| Cancellation | not yet | designed in (§6) — cooperative revision/flag check at query entry |
| Parallelism | not yet | designed in (§6) — concurrency-safe memo table, parallel independent queries |
| LRU / memo eviction | not yet | deliberate later slice; `slot_count()` already exposes table size |
| Durability tiers | not adopted | the stdlib-stub input is "set once"; a coarse "high-durability input" marker is cheaper than salsa's tier system if measurement shows a need |

These are designed in deliberately, not hand-waved. The bar for cutover (decision record) requires
cancellation + parallelism available and per-edit cost O(blast-radius).

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
(§5).

### Deliberately out of the core for R0

- **Input removal / file deletion.** Removing an input is a host-level re-keying concern (and interacts
  with eviction); the core stays minimal. R1 wiring will decide whether deletion is a host operation over
  the slot table or a core primitive with explicit dependent invalidation. Not needed by any smoke test.
- Cancellation, parallelism, eviction (§6).

---

## 3. The query graph for the rewrite

```
                 +-- config -----------------------+
                 |                                  |
 source_text(f) -+-> parse(f) -> lower(f) -> local_naming(f) --+
                 |                                              |
 stdlib_stubs ---+------------------------------> package_naming <--- (all files' local_naming)
                                                        |
                                                        v
                                          typecheck(f) -> diagnostics(f)
```

Inputs (set from outside, never computed):

- `source_text(file)` — per-file source. The high-churn input; every keystroke is a `set_input`.
- `config` — project `roughly.toml` (`[check] typing/unused/strict`, …). Low churn.
- `stdlib_stubs` — the immutable stub stub-library (base + CRAN). Set once; effectively a high-durability
  input. Kept out of any reverse-dep bookkeeping in the hand-rolled model — here it is simply an input
  whose `changed_at` never advances, so it never invalidates anything.

Edges (each a recorded `fetch`, so the dependency is automatic):

| Edge | Why |
| --- | --- |
| `parse(f)` ← `source_text(f)` | the tree is a pure function of the bytes |
| `lower(f)` ← `parse(f)` | HIR is lowered from the tree |
| `local_naming(f)` ← `lower(f)`, `document_kind(f)` | file-local resolution over the HIR; kind (package vs script) is a separate fine-grained input so a text-only edit does not invalidate naming via a kind read (the spike's lesson — keep input granularity matched to reads) |
| `package_naming` ← `local_naming(f)` for all `f`, `stdlib_stubs` | the package's global binding table + winner selection is a fold over every file's exported names plus the stubs |
| `typecheck(f)` ← `lower(f)`, `local_naming(f)`, `package_naming`, `config` | HM inference over the file, resolving cross-file names against the package interface |
| `diagnostics(f)` ← `typecheck(f)`, `local_naming(f)`, `config` | diagnostics are the rendered output; `config` gates typing/unused/strict |

### How the hand-rolled M3/M4 structures dissolve

The whole point of the rewrite — every bespoke incremental structure becomes a consequence of recorded
deps + revisions, with no mirror to drift:

- **M3 reverse-dependency index** (`Symbol → {DocumentId}`, hand-patched, guarded by a debug drift
  oracle) → **recorded query dependencies.** A referrer that read a global through `package_naming`
  recorded that read; when the global's winner changes, `package_naming.changed_at` advances and only the
  files whose `typecheck` recorded it re-run. No reverse index to maintain, no oracle to run.
- **Dirty-set + candidate selection** (`dirty ∪ documents_referencing(changed_globals)`) → **revision
  bumps + validation.** "Dirty" is just "input `changed_at` == current revision"; candidate selection is
  the validation walk discovering which memos a changed dependency reaches.
- **String dependency fingerprints + type fingerprints** (re-keying round-2 typecheck on a rendered hash
  of referenced schemes) → **value-eq early cutoff.** Instead of hashing the referenced schemes into a
  fingerprint string and comparing, `package_naming`'s value either changed or did not; if a file's
  referenced slice is unchanged, the equal value cuts off before `typecheck` re-runs. The fingerprint was
  a hand-rolled stand-in for value equality.
- **M4 incremental package-naming + incremental type index** (candidate indexes, materialized tables,
  winner/duplicate patching, five drift assertions) → **derived queries with cutoff.** `package_naming`
  is a derived query; an edit that does not change a file's exported-name set produces an equal
  `local_naming` contribution, which cuts off before `package_naming` re-folds anything observable. The
  per-name winner/duplicate logic stays as the *body* of that query; what disappears is the mirrored
  incremental machinery and its oracles — the silent-stale bug class becomes structurally impossible
  (no mirror ⇒ nothing to drift).

---

## 4. Why this kills the silent-stale class

The 4+ silent-staleness bugs (decision record) all had the same shape: a hand-maintained mirror of the
true dependency graph that the *untracked* read path could bypass. In the query model the tracked path
(`fetch`) is the *only* path a body can read another query, and reading **is** recording. There is no
untracked default to forget. That is the structural guarantee the rewrite is buying.

---

## 5. The cyclic query: package interface fixed-point

R allows mutual typed re-exports (`a <- b`; `b <- a` across files), which form a genuine dependency
cycle the acyclic core cannot express directly. This is the one non-trivial query.

**Shape:** `package_interface` is a derived query that computes each package global's exported scheme.
Re-exports make it self-referential: resolving `a`'s scheme requires `b`'s, which requires `a`'s. It maps
to a query that **re-enters itself**, resolved by a bounded fixed-point *inside the body* rather than by
the generic validation recursion:

1. Iterate to a fixed point (Jacobi-style: each round's table is computed from the previous round's), the
   same synchronous iteration the current `build_package_interface_table` uses.
2. **Convergence guard.** Acyclic re-export/forward-ref chains are monotone — each global's scheme
   transitions at most once (`Unknown` → concrete), so they converge in ≤ `#globals + 1` rounds. Bound the
   loop by that (with slack), exactly as the current code does after the round-cap bug fix.
3. **`Unknown`-pinning for genuine cycles.** A pure re-export cycle is *non-monotone* — members oscillate
   (period-2 swap) and never settle. Port the existing oscillation guard from `analysis.rs` verbatim as
   the cycle-recovery value: a symbol whose rendering returns to an earlier value while differing from the
   previous round is on a cycle → pin it to `Unknown`, which collapses the cycle and restores monotonicity
   so the loop converges.

In query terms the body is a self-contained fixed-point producing one `Stored` value (the converged
interface table). Downstream `typecheck(f)` queries depend on it normally; value-eq cutoff means a converged
table equal to the previous one stops propagation. A future refinement is to push cycle *detection* into
the core (an "in-progress" marker on the validation stack) and expose a recovery hook, but pinning inside
the body is the minimal correct mapping and reuses proven logic.

---

## 6. Cancellation & parallelism (designed, not yet built)

**Cancellation (cooperative).** Part A's live-as-you-type model needs the latest edit to win: a cross-file
pass started on revision *N* must abandon cheaply when revision *N+1* arrives. Plan: a query checks a
cancellation signal (a flag, or "current revision != the revision this pass was launched for") at body
entry / fixed-point round boundaries and bails with a sentinel that unwinds the fetch stack. Because
nothing is committed until a body returns, an abandoned pass leaves no partial memo — the next pass
recomputes cleanly. The `&mut self` / `&self` split already enforces "no fetch overlaps a `set_input`",
which is the precondition for revision-based cancellation.

**Parallelism.** Independent queries (different files' `parse`/`lower`/`local_naming`) have disjoint
dependency subtrees and can run concurrently. Plan: move the memo table behind a concurrency-safe
structure (sharded/`RwLock` map or a lock-free map) and make the dependency stack per-worker (thread-local
or passed explicitly), so recording stays correct under parallel bodies. The current `Cell`/`RefCell`
single-threaded model is the R0 substrate; the migration is a contained change to the storage and stack
types, not to the algorithm. salsa gets this from its runtime; we take it as a deliberate, measured slice.

---

## 7. Differential validation (R2 — the correctness proof)

The rewrite's correctness is proven **differentially against the production `analysis` crate**: for a
ported fixture subset, each new-engine query output must equal `analysis`'s output for the same phase
(parse tree, HIR, naming result, type errors, diagnostics). This is the cross-check the decision record
mandates and it **subsumes the F1 differential fuzzer** — F1's "incremental == full rebuild" invariant
becomes "new engine == old engine". Because the new engine produces the *same* results with
correct-by-construction invalidation, the old drift oracles are retired only once this cross-check is the
source of truth. Concretely R2 adds a harness that, over the soak/fixture corpus, drives both engines
through the same edit stream and asserts per-query equality after every edit.

---

## 8. Phase plan (R1 → R3)

- **R0 — substrate + design (this).** Done means: `engine` crate builds and tests standalone; the generic
  red-green core (revision, input backdating, derived memoization, runtime dep recording, early cutoff,
  cutoff propagation) is product-quality; smoke tests prove memoization / invalidation / early cutoff /
  dependency recording; this design is recorded. `analysis` untouched and green.
- **R1 — wire the real query bodies.** Define the R `QueryGroup` (`Key` over the §3 graph) with bodies
  calling the kept tree-sitter parse + M2 HM core + naming + typecheck (query bodies, not rewrites).
  Done means: the full chain `parse → … → diagnostics` runs through the engine on real R input, including
  the cyclic interface query (§5). Duplication from `analysis` is allowed; no code sharing yet.
- **R2 — differential validation (§7).** Done means: new-engine output == `analysis` output over the
  ported fixture subset under an edit stream; the cross-check is green and is the new correctness gate.
- **R3 — cancellation + parallelism (§6) and eviction.** Done means: cancellation and parallel
  independent queries available; per-edit cost measured O(blast-radius) and competitive with (better than)
  the hand-rolled path; the quality bar in the decision record is met. Production cutover is a separate,
  later decision made on this evidence.
