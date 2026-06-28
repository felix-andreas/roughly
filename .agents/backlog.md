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

## Open questions (Director to resolve / ask)

- **Proper fuzz test?** We have a seeded deterministic soak (3000 fixed-seed ops + release-gated drift oracles). Do we also want a true random/continuous fuzzer (cargo-fuzz)? — asking CTO/Expert.
- **Strict-mode semantics** — confirm the interpretation above with the user (the phrasing was ambiguous).

## Notes

- **Pull diagnostics — yes, interesting.** The pull model (`textDocument/diagnostic` + workspace diagnostics, LSP 3.17) lets the client request diagnostics on demand rather than the server pushing on every change. It fits Roughly because Roughly *is* the only R checker (so it controls the full diagnostic lifecycle), and on-demand pull composes cleanly with the debounced/cancellable recheck model + the blast-radius incremental engine (compute dependents lazily when asked). Worth doing; sequence after strict mode + library typing since those define *what* diagnostics exist.

## After the backlog

- **Incremental-computation-engine migration** (salsa or in-house red-green). Direction + de-risking-spike plan recorded in `.agents/decisions/incremental-architecture-and-recheck.md`. Resume the shelved spike → go/no-go → phased migration (keeps tree-sitter + the M2 type core).
