# Backlog — Production-readiness punch-list

**Status (2026-06-29):** the engine cutover is technically complete (one memoized query engine is the sole analysis backend; hand-rolled incremental machinery + drift oracles deleted; all 6 done-bar gates met, Expert-accepted). **But the PROJECT is NOT production-ready.** A component passing its tests ≠ releasable.

**Goal:** bring the ENTIRE repository to **impeccable, rust-analyzer-level production quality**. Repository hygiene is expected, not optional. The Director (CEO) owns finding and fixing everything short of that bar proactively — the user must not have to point things out. Do **not** declare ready until it genuinely is; the **user merges**, not us — we only report when it is in impeccable production-ready state.

**Standing rules (user):** anything testable uses fixtures (not ad-hoc unit tests). Comments must be **context-free** — no internal milestone/process references (R0–R3, M1–M4, "Phase 4", "gate (c)", "3f", commit hashes); a fresh reader with zero project history must understand them (now in AGENTS.md).

---

## P0 — Production-ready gate (must all be DONE before "releasable")

### 1. Stubs — proper format + standard-library coverage  *(owner: CTO; hard requirement)*
- Replace the `crates/analysis/src/stdlib_base.R` Rust-embedded `include_str!` blob with **proper stub files**, ideally written in the same `#:` typing syntax.
- CTO designs a **reasonable, documented stub format** (a real spec, not a blob).
- Stub files must be **overridable**: a project supplies its own → takes **precedence** over the shipped stubs.
- **Real stub files for at least the standard libraries (base, stats, utils, methods, …) MUST exist.** Without standard-library stubs it is NOT production-ready. (Coverage: comprehensive via real-R introspection — `getNamespaceExports()` + `formals()` — per the decided approach below.)
- Design decisions preserved below ("Stub design — decided").

### 2. Docs + website — perfect shape  *(owner: DX; subjective → show the user before calling done)*
- **Landing page:** particles are all on the RIGHT + the logo is built from SQUARE shapes → make particles **organic** (not squares), distributed properly.
- **Scroll behaviour:** the particles should **morph into the heading** "Modern developer tooling for R." — instead of the heading appearing on top of the particles.
- **"IDE features in your editor." section:** most tabs look bad + there is **layout shift** when a tab is clicked → fix both.
- **Formatting-section examples:** not distinct — they only add spacing around operators. Replace with **inconsistently-formatted code that morphs into a consistent shape** (more organic, and genuinely distinct examples — auto-bracing, alignment, etc., read the formatter to pick meaningful ones).
- Full **docs-site accuracy + clarity pass** (the docs are contracts).
- (Prior DX website attempt was disliked — raise the design bar; show the user the result for a visual gut-check before declaring done.)

### 3. Code hygiene + coding-guidelines compliance — whole repo  *(owner: CTO)*
- **Compliance audit against the project's Rust coding guidelines** (AGENTS.md): **top-down ordering** (e.g. `crates/roughly/src/server.rs` flagged as violating), `use`-qualification style, no organizational/summary comments, full-word names, no needless helper indirection, make-illegal-states-unrepresentable, etc. Fix every violation.
- **Context-free comments:** sweep ALL committed code for internal-milestone/process references (R0/R1/R2/R3, M1–M4, "Phase N", "gate (a–f)", "3f", "the audit", commit hashes, "the spike") and rewrite them to be context-free — explain the "why" in domain terms a fresh reader understands. This is pervasive after the cutover.
- General polish to rust-analyzer level (dead code, stray TODOs, error-handling, naming).

### 4. Remove `insta`  *(owner: CTO)*
- Overkill for ~4 snapshot use-cases (`test_tree` node-kinds/field-names in analysis + roughly). Replace with plain assertions, drop the `insta` dep + the `snapshot`/`snapshot-delete-unreferenced` justfile recipes + the `.snap` files.

### 5. Release-readiness audit — find what we're missing  *(owner: Expert + reviewers)*
- A brutal whole-repo pass: is this in a releasable, rust-analyzer-quality state? Cover code hygiene, docs, website, tests, packaging/release, error handling, naming, dead code, panics, public API surface, README/CONTRIBUTING, CI. **Every finding is added to this punch-list.** The user should not be the one finding these.

---

## P1 — Post-release (features; NOT required for production-ready)

*User: "you don't need to add more features, but standard library stubs must exist." So these wait.*

- **Stub system, fuller** (see `stub-system-requirements` memory + "Stub design" below): NAMESPACE-aware import-checking (strict-mode warn on missing stub — `import(pkg)` needs a whole-package stub; `importFrom(pkg, item)` needs the item defined); **configurable severity** (missing stub = error vs inferred-`Unknown`); CRAN auto-generation by introspecting installed packages.
- **`#:` semantic-token highlighting** — colour the typing syntax inside `#:` comments (LSP semantic tokens; optionally a tree-sitter/TextMate injection in the VS Code + Zed extensions for instant offline colouring).
- **Sub-linear validation walk** — the residual O(N) per-edit red-green validation (correct, not flat in N); a durability/changed-input-tracking slice.
- **Cutover responsiveness follow-ups (Expert):** debounce + cancel the edit-path (push) diagnostics; an end-to-end server latest-edit-wins test (frontend→worker); pull-diagnostics flicker (`DiagnosticServerCancellationData` instead of empty-full); record the 281k memory number + an explicit acceptance verdict in the decision log.

---

## Stub design — decided (Expert, 2026-06-28) — keep

Tiered, both as `#:` decl-only stubs (no bespoke binary format):
- **stdlib (base/stats/utils/methods):** curated in-repo, compiled into the binary, selected by detected **R version**; loaded once; the "known" universe for strict mode.
- **CRAN (third-party, post-release):** discover installed packages (`.libPaths()`/renv/DESCRIPTION), auto-generate shallow stubs by introspecting real R (`getNamespaceExports()` + `formals()` → arity + arg names, `Any`/`Incomplete` returns), cached per version; optional curated overrides; unstubbed → `Any`, never a hard error. `pkg::name` needs a `NamespaceGet` HIR node (today `Unsupported`).
- **Isolation (hard gate):** stubs are immutable high-durability inputs — kept out of the engine's incremental dependency graph (set-once input).
- Validate stubs against real R (a stubtest-style CI check diffing curated stubs vs `formals()`/`getNamespaceExports()`).
- Update `docs/.../stdlib-stubs.md` to match the chosen format + the override mechanism.
