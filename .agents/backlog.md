# Backlog

Director-owned feature backlog. **Sequencing directive (user):** clear this backlog **first**; the incremental-computation-engine migration (salsa / in-house memoized-query — see `.agents/decisions/incremental-architecture-and-recheck.md`) comes **after** the backlog is empty. A shadow spike of the query engine may be shelved (behind a feature flag) to resume then.

Testing rule (user, standing): **anything that can be tested should use fixtures** — they generate in bulk and read well in diffs. Formatter tests already migrated (`2a82133`).

## Active

| # | Item | Owner | Notes |
|---|------|-------|-------|
| 1 | **Strict mode** — a `[check] strict` switch. Interpretation (confirm): in **strict**, an expression/binding that types to `Unknown` (un-annotatable / un-inferrable / missing library type) is a **diagnostic**; default (non-strict) tolerates `Unknown`. Design-first + bulk fixtures. | CTO | Plays with library typing (#2 reduces false positives). |
| 2 | **Library typing** — represent types for R's stdlib (base/stats/utils) + external CRAN packages. Approach open: typeshed-like external `#:` decl-only `.R` stubs (design note `docs/.../stdlib-stubs.md`) vs. compiled-into-the-binary vs. other — **per the Expert's recommendation** (being asked). Subsumes the `T`/`F`/`pi` base-binding gap. | CTO + Expert | Stubs are immutable high-durability inputs (kept out of the incremental dep graph). Validate against real R (`formals()`/`getNamespaceExports()`). |
| 3 | **Auto-format `#:` type-hint comments** — consistent layout of annotation comments. Needs a strong test suite (now in place via the formatter fixture migration). Bulk fixtures. | DX | Uses analysis `type_syntax` read-only; flag if it needs an analysis-crate API. |
| 4 | **Pull diagnostics** — LSP `textDocument/diagnostic` (client pulls on demand) instead of/alongside server push. Pairs well with the decided debounced/on-demand recheck model + cancellation; lets the client control when (and which) diagnostics compute. Evaluate + implement. | CTO/DX | Good fit; see assessment below. |
| 5 | **Formatter adversarial edge-case review** — an adversarial agent hunts formatter edge cases; failures become fixtures. | DX | |
| 6 | **Docs to world-class state** — accuracy + clarity pass across the docs site (contracts). | DX | |
| 7 | **Website improvement** — adversarial marketing-expert review + improvements. | DX | |

## Resolved (Expert recommendations, 2026-06-28)

**#2 Library typing — APPROACH DECIDED: tiered stdlib-vs-CRAN, both as `#:` decl-only stubs (no bespoke format).**
- **stdlib (base/stats/utils/methods):** curated in-repo, **compiled into the binary**, selected by detected **R version**; loaded once at `Analysis::new`; the "known" universe for strict mode. First increment: `T`/`F`/`pi` + ~12–50 high-frequency base fns.
- **CRAN (third-party):** not shipped; discover the project's installed packages (`.libPaths()`/renv/DESCRIPTION), **auto-generate shallow stubs by introspecting real R** (`getNamespaceExports()` + `formals()` → arity + arg names, `Any`/`Incomplete` returns), cached per package version; optional curated overrides (typeshed-third-party model); unstubbed → `Any`, never a hard error. `pkg::name` needs a `NamespaceGet` HIR node (today `Unsupported`).
- **Isolation (hard gate):** stubs are immutable high-durability inputs — never in `global_bindings`/interface table/fingerprints/reverse-deps/dirty-set; verified by an isolation assertion + a zero-per-edit-cost benchmark.
- DoD = LT1–LT7 (format+SSOT; incremental isolation; stdlib embedded + R-version-keyed; CRAN per-project introspection; stubtest CI validator diffing curated stubs vs real R; scope discipline incl. optional/default params + the named-arg-lowering gap; type-syntax extensions gated). **Action:** revise `docs/.../stdlib-stubs.md` to add the CRAN tier + introspection-generation + R-version keying (currently stdlib-only).

**Fuzzing — DECIDED (Expert): YES, two `cargo-fuzz` targets (soak alone is insufficient).**
- **(F1) Differential incremental oracle** — random `add/edit/delete/rename` op-sequences via `arbitrary`; invariant: incremental `Analysis` == full rebuild across all five drift oracles **plus** diagnostics + `global_bindings` + type index. The soak generalized to coverage-guided/auto-minimizing; hunts the silent-stale class and is the cross-check that de-risks the eventual salsa migration. Build now or with the migration.
- **(F2) Parser + type-syntax + lowering robustness** — raw R + `#:` input; invariants: no panic / termination (S4 guards) / tree-sitter always-a-tree / no OOB on stale ranges (S6). Valuable **regardless** of architecture — build now (independent).
- Run nightly + time-boxed in CI; commit the corpus and every crash repro as a deterministic regression seed/fixture.

## Open questions (Director to resolve / ask)

- **Strict-mode semantics** — confirm the interpretation above with the user (the phrasing was ambiguous; Director's read: strict ⇒ `Unknown` is an error, default tolerates).

## Notes

- **Pull diagnostics — yes, interesting.** The pull model (`textDocument/diagnostic` + workspace diagnostics, LSP 3.17) lets the client request diagnostics on demand rather than the server pushing on every change. It fits Roughly because Roughly *is* the only R checker (so it controls the full diagnostic lifecycle), and on-demand pull composes cleanly with the debounced/cancellable recheck model + the blast-radius incremental engine (compute dependents lazily when asked). Worth doing; sequence after strict mode + library typing since those define *what* diagnostics exist.

## After the backlog

- **Incremental-computation-engine migration** (salsa or in-house red-green). Direction + de-risking-spike plan recorded in `.agents/decisions/incremental-architecture-and-recheck.md`. Resume the shelved spike → go/no-go → phased migration (keeps tree-sitter + the M2 type core).
