# Decision record: incremental architecture & recheck-trigger model

**Status:** Part A (recheck trigger) — decided. Part B (hand-roll vs. memoized-query framework) — **OPEN, deferred to the user.** Part C (value-interface-table slice) — **paused pending Part B.**

**Recorded:** 2026-06-28, after two independent expert reviews commissioned with *effort explicitly excluded and engineering quality as the sole criterion* — the persistent RA/LSP expert (full M1–M4 context) and a fresh outside reviewer (no stake in prior work). They agreed on Parts A and C and **split on Part B**; that split is itself the key signal.

## Why this exists

M1–M4 built hand-rolled incremental analysis — reverse-dependency index, incremental package naming, incremental type index — each guarded by debug "drift assertions" (incremental result == full rebuild) plus a seeded soak. Along the way we caught **4+ silent-staleness bugs** (winner-diff baseline staleness from intervening IDE queries; script-vs-package drift; the fixed-point round-cap leaving deep chains stale; the bare-block predicate gap). Before building the next slice (incremental value-interface-table), we stopped to decide the foundation.

---

## Part A — Recheck trigger model (DECIDED)

Both experts corrected the common "rust-analyzer checks on save" belief:
- rust-analyzer runs **two** diagnostic streams: (1) its own native type/name diagnostics **live as you type** (debounced, cancellable on the salsa snapshot), and (2) `cargo check` **flycheck on save** for authoritative full errors. Save-only applies to stream (2) *because there is a slower authoritative backstop to defer to.*
- **Roughly has no stream (2). It *is* the only R type checker** (no prior static checker for R exists). So save-only would give the user **zero type feedback while typing** — strictly worse than rust-analyzer.

**Decision — tiered, debounced, live-as-you-type, with save as a force-full pass:**
- **Per-keystroke, debounced ~150–250 ms:** syntax + lint + lower + file-local naming + the **edited file's own** type errors. Budget: p95 ≤ 100 ms (feels live).
- **Cross-file / package diagnostics:** debounced ~300–500 ms after typing settles, **bounded to blast radius**, cancellable (latest edit wins).
- **On save:** force a full authoritative pass (natural commit point + safety net).
- **Interactive queries** (hover / completion / goto / signature help): on demand, < 50–100 ms.

This is a forward design target; the current server does not yet implement the debounce/cancel tiers (that is part of the deferred M5 "off-thread responsiveness" work).

---

## Part B — Hand-roll vs. memoized-query framework (OPEN — user's call)

The fork: continue hand-rolling per-structure incrementality (guarded by drift assertions), **or** adopt a memoized-query / automatic-invalidation substrate (salsa, or a minimal in-house red-green engine) so invalidation is correct-by-construction.

**Both experts agree on two things:** salsa's *heavy* machinery (macro expansion, trait/coherence solver, multi-crate graph, durability tiers) is genuinely **unneeded** for R — the real question is only its **core**: memoized queries with automatic dependency-tracked invalidation. And: the drift-assertion safety net has a real hole (below) that should be closed regardless.

**Fresh reviewer → (c) keep hand-rolling, within strict limits + a salsa "tripwire".**
- R's dependency graph is shallow and static (source → lower → naming → interface → check; one-hop reverse-dep + one bounded fixed-point for re-exports). salsa's machinery tames *deep, dynamic* graphs — importing it for a shallow static one is machinery for its own sake.
- salsa relocates rather than eliminates the discipline (you must route every read through a tracked query; an off-query read reintroduces the same staleness).
- Migrating already-correct, drift-asserted, soak-covered code is a fresh net-negative bug surface; salsa's own API has churned hard.
- Adopt a framework **only when a tripwire fires** (see below).

**RA/LSP expert → (b) adopt a memoized-query framework** (and it *changed its recommendation toward this* once told to weigh quality over effort).
- The 4+ silent-stale bugs are empirical evidence that hand-rolling the invalidation **core** is error-prone *independent of R's simplicity*.
- The safety net is **structurally incomplete**: the drift assertions are **debug-only**, and they are **blind to shared-rule bugs** (incremental and oracle share the membership predicate, so a wrong *rule* is invisible — mitigated only by fixtures). It cannot catch the full bug class even in principle.
- "R is simpler" justifies not needing salsa's macro/trait machinery, but **not** its core — which is exactly what keeps getting hand-rolled wrong. R's regularity makes the **migration easy**, not the framework unnecessary.
- The real dividing line is **automatic dependency-tracked invalidation vs. hand-maintained mirrored invalidation.** In a framework the *tracked* path is the natural default; hand-rolled makes the *untracked* path the default (read `global_bindings` directly) and tracking the bespoke addition you can forget — which is how every one of these bugs happened.
- A framework also delivers **cancellation, parallelism, and LRU eviction** structurally — all of which the Part A live-debounced model and the 300k responsiveness goal need, and none of which hand-rolling provides without large bespoke effort that reproduces the same bug class.

**Tripwires that flip the decision to a framework** (per the fresh reviewer; the expert argues these are imminent enough to adopt now):
1. **Cross-file / whole-program type inference** (the architecture page's one-hop premise explicitly says this voids one-hop and forces a worklist).
2. **R namespace dependency edges** — `::`/`:::`, `library()`/`requireNamespace()` load-order, imported-package symbols. **Note: the planned stub framework introduces exactly this**, so it likely crosses this tripwire on its own.
3. **Dynamic dispatch resolution across files** (S4/R6 method dispatch creating data-dependent edges).

**Tie-breaker under the stated criteria (quality-first, effort irrelevant):** tilts toward the framework — "migration is risky/work" is the conservative view's main weight, and effort is excluded; and the project's own roadmap (stub framework + namespace resolution) likely trips the conservative reviewer's *own* tripwire. The recommended shape is a **de-risked incremental migration**: keep tree-sitter parsing (superior — lossless, free incremental reparse) and the now-sound M2 HM type core (they become the bodies of `parse`/`infer` queries, not rewrites); migrate phase-by-phase; **run the existing drift oracles as a live cross-check *during* migration**, retiring them only once the query path is proven; do a query-grain spike (per-file vs per-binding) first.

**Deferred migration query graph (for when chosen):** `parse(file)` → `lower(file)` → `local_naming(file)` → `package_naming` → `interface(file)` → `check(file)` → `diagnostics(file)`; inputs = per-file text + config. The reverse-dep index dissolves into salsa's recorded dependencies; the interface fixed-point maps to a cyclic query with `Unknown` recovery (the existing oscillation-pinning maps directly); string fingerprints are replaced by salsa early-cutoff.

**This decision is the user's** (AGENTS.md gates large incremental-analysis direction changes on user sign-off). Recorded here so the decision is made on the full analysis, not rediscovered.

---

## Part C — The value-interface-table slice (PAUSED)

`build_package_interface_table` is O(all package globals) with deep scheme clones, **rebuilt 2–3× per recheck regardless of edit blast radius** (~62 ms of the ~77 ms single-file recheck at ~60k synthetic globals; the winner-diff is a same-class ~8 ms cost). It is the **last O(package) flat-recheck cost** — recheck still scales with package size, not edit size.

Both experts: **not urgent** at real R package sizes (hundreds–low-thousands of exports, not 60k) under the Part A debounced model — the cost hides inside the debounce window. **Necessary at the 300k target**, where O(globals) dominates flat recheck. **But it should not be built as the next hand-rolled slice:** if Part B chooses a framework, the table becomes a memoized query and is *subsumed* — hand-rolling it first would be wasted and add a sixth bespoke mirror+oracle. If Part B stays hand-rolled, do it as an **in-place patch of the existing table** (patch only `changed_globals`/winner-flips, which M3/M4 already compute) — **no new mirrored index, no sixth oracle**. If it can't be done without a new mirror, that is the signal it isn't worth doing yet.

---

## No-regret action (correct regardless of Part B) — DO NOW

The drift assertions are `#[cfg(debug_assertions)]` and the soak runs in debug, so **release-path incremental correctness is unverified except indirectly.** Both experts call this the cheapest, highest-value hardening available. Action: make the drift-oracle + seeded-soak a hard CI gate that also exercises the **release** routing, and broaden the soak's adversarial interleavings (edit→IDE-query→edit, add→delete→re-add, package↔script transitions). This holds whether we keep hand-rolling or migrate (during a migration it becomes the cross-check).

## Open risk to carry

The **one-hop premise** ("a single reverse-dep hop suffices only because inference never flows across files") is the load-bearing assumption of the entire hand-rolled model. It is stated in prose and enforced by nothing at compile time — a future contributor adding a small cross-file inference shortcut silently voids it. Either encode it as a loud invariant at the inference boundary, or treat its violation as an automatic Part-B = framework trigger.
