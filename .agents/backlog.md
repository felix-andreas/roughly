# Backlog — Production-readiness punch-list

**Status (2026-06-29):** the engine cutover is technically complete (one memoized query engine is the sole analysis backend; hand-rolled incremental machinery + drift oracles deleted; all 6 done-bar gates met, Expert-accepted). **But the PROJECT is NOT production-ready** — a component passing its tests ≠ releasable.

**Goal:** bring the ENTIRE repository to **impeccable, rust-analyzer-level production quality**. Repo hygiene is expected, not optional. The Director (CEO) finds + fixes everything short of that bar proactively — the user must not have to point things out. Do NOT declare ready until it genuinely is; the **user merges**, not us — report only when impeccable.

**Standing rules (user):** testable things use fixtures, not ad-hoc unit tests. Comments must be **context-free** — no internal milestone/process references (R0–R3, M1–M4, "Phase 4", "gate (c)", "3f", "the spike", commit hashes); a fresh reader with zero project history must understand them (AGENTS.md).

**Sources:** user review + the Expert release-readiness audit (2026-06-29). Anchors are `file:line` at audit time (may drift).

---

## P0 — blocks release

- **CI — there is none** *(CTO)*. `.github/workflows/` is empty; every "green" has been manual. Add a workflow: `cargo build/test --all-targets --all-features`, `clippy -D warnings`, `fmt --check`, and the `#[ignore]` perf/memory/fuzz benches with thresholds. This gate would have caught most P1s below.
- **`architecture.md` is a half-stale CONTRACT** *(DX, with CTO for engine facts)*. Describes deleted machinery as live (≈190–630: reverse-dep index, dirty-set, `maintain_package_naming`, drift oracles — all gone; every named symbol greps to 0 files) and the new `crates/engine` is documented nowhere. Rewrite to describe the actual engine (red-green substrate, revision clock, worker + cancellation; `analysis` = from-scratch oracle + CLI). Remove the M3/M4 headings + the "Slice plan" task-tracker (≈613–630). Stale code-comment mirror: `analysis.rs:651–658`. Source the engine architecture from `.agents/decisions/incremental-architecture-and-recheck.md` + `crates/engine/DESIGN.md`.
- **Packaging unsafe to publish** *(CTO)*. Internal crates `{analysis,engine,fixtures,rofy}` lack `publish = false` + have squatter-bait names; `roughly` lacks `license`/`description`/`repository` despite shipping a `LICENSE`. Add them.
- **Server-crash read-handlers** *(CTO)*. `uri.to_file_path().unwrap()` in 11 read handlers (`server.rs:892,945,1008,1048,1090,1130,1182,1227,1282,1323,1377`) → a non-`file:` URI (`untitled:`/`vscode-vfs:`) panics the worker → `process::exit(1)`. They return `Result` and have `path_not_found_error` available. Also `server.rs:866` `unreachable!()` on a future `FileChangeType`, and 2 client-send `.unwrap()` (`server.rs:439,847`). Real user-facing crash — fix.
- **Stubs — proper format + standard-library coverage** *(CTO; user hard requirement)*. Replace the `crates/analysis/src/stdlib_base.R` blob (≈12 base fns) with a proper, documented stub format (ideally the `#:` typing syntax); real stubs for the standard libraries (base, stats, utils, methods, …); files **overridable** (project supplies its own → precedence). "No new features, but stdlib stubs must exist." (Stub design preserved below.)
- **Docs + website — perfect shape** *(DX; subjective → screenshot to the user before declaring done)*:
  - Landing page: particles all on the RIGHT + logo built from SQUARE shapes → **organic** particles, distributed.
  - On scroll, particles should **morph into the heading** "Modern developer tooling for R." — not the heading appearing on top.
  - "IDE features in your editor." tabs look bad + **layout shift** on click → fix both (reserve dimensions).
  - Formatting-section examples not distinct (just operator spacing) → inconsistently-formatted code **morphing into consistent shape**; pick genuinely distinct examples (auto-bracing, alignment).
  - Full docs-site accuracy + clarity pass (contracts). Prior attempt disliked — raise the design bar.

## P1 — should-fix before release

- **Fails `cargo fmt --check`** *(CTO)* — 255-line diff vs the committed `rustfmt.toml`.
- **45 clippy warnings** *(CTO)* — unused imports (`BTreeMap,BTreeSet,Diagnostic,DocumentId,Symbol`), too-many-args fns (8–11/7), `clone` on `Copy`, manual `is_multiple_of`, missing `Default`s, collapsible `if`, `contains_key`+`insert`, `expect_fun_call`.
- **Top-down ordering — MAJOR violators** *(CTO)*: `typecheck.rs` (`InferenceError` def 4695 used 110; `Binding` def 4805 used 41; `BuiltinKind`/`SubscriptKind` stranded), `naming.rs` (`TypeInfo` def 1197 used 143; `TypeResolver`/`DocumentNamingContext` after their callers). (`server.rs` is only MINOR — P2.)
- **Context-free / process-leak comments** *(CTO)* — ~21 `.rs` hits incl. **public module doc-comments** (`engine/src/queries.rs:5,10,60,120,134,740,784`; `engine/src/engine.rs:6,43,53,78`; `server.rs:153,161`; `analysis.rs:384`; `typecheck.rs:482`; `ide.rs:111`). Plus **`crates/engine/DESIGN.md`** (28 R0/R1/spike/gate refs — a milestone narrative that ships in the crate): rewrite as a clean architecture doc.
- **`structure.md` wrong** *(DX)* — cites non-existent `workspace.rs` + `resolve_document` (it's `resolve_document_locally`); omits `ide/generic.rs` + the whole `engine` crate.
- **Perf/memory/fuzz tests `#[ignore]`'d + ungated** *(CTO)* — 12 ignored tests; wire into CI with thresholds; **record the 281k memory number + acceptance verdict** in the decision log.
- **Sloppy TODO/HACK** *(CTO)* — `server.rs:72` tokio-main "???", `format.rs:1163` "HACK … ths" typo, `index.rs:207` incomplete TODO.
- **Secondary doc staleness** *(DX)* — `development.md:11` "three crates" omits `engine`; `testing.md` omits the engine differential harnesses; `stdlib-stubs.md` (≈123–139,233–235) couples to the deleted substrate + leaks M3/M4; it's still titled "(Proposal)".
- **Remove `insta`** *(CTO; in progress)* — overkill for ~4 `test_tree` snapshots; plain assertions, drop the dep + `snapshot*` justfile recipes + `.snap` files.

## P2 — polish

- Public API too broad *(CTO)* — `roughly`/`analysis` `lib.rs` expose every module `pub mod`; tighten `pub(crate)` (compounds packaging).
- Shell-side symbol pipeline = second source of truth *(CTO)* — `roughly/src/{index,symbols,tree}.rs` re-walk the tree for symbols instead of deriving from engine HIR/naming.
- Editor version drift + no CHANGELOG *(DX)* — workspace `0.2.4-alpha`, package.json `0.2.4`, zed `extension.toml` stale `0.2.0-alpha.3`; add a CHANGELOG.
- `crates/rofy` *(CTO)* — experimental extendr REPL with debug `println!` (`lib.rs:48,58`); mark experimental / exclude from lint gates.
- Minor ordering *(CTO)* — `server.rs` (`Job`/`EngineWorker`/`LanguageServer` interleave), `analysis.rs`, `format.rs`, `cli.rs`, `engine.rs`.
- Borderline panics (deliberate — review) *(CTO)* — `server.rs:858` watched-file panic, `:483-490` startup aborts, `:684` per-keystroke `expect(format!())` alloc; `cli.rs:177-179` redundant per-loop re-fetch.

*(Error-handling audit: of 119 `unwrap`/`expect`, only the 11 read-handler URIs + 2 client-sends + 1 `unreachable!` are real defects; the rest are justified coherence-path/local-invariant/infallible. No `Result` silently swallowed.)*

---

## Post-release — features (NOT required for production-ready)

*User: "you don't need to add more features, but standard library stubs must exist." These wait.*
- **Stub system, fuller:** NAMESPACE-aware import-checking (strict-mode warn on missing stub — `import(pkg)` needs a whole-package stub, `importFrom(pkg,item)` needs the item defined); **configurable severity** (missing stub = error vs inferred-`Unknown`); CRAN auto-generation via introspection.
- **`#:` semantic-token highlighting** — colour the typing syntax in `#:` comments (LSP semantic tokens; optional tree-sitter/TextMate injection in the editors).
- **Sub-linear validation walk** — the residual O(N) per-edit red-green validation (correct, not flat in N).
- **Cutover responsiveness follow-ups (Expert):** debounce + cancel the edit-path (push) diagnostics; an end-to-end server latest-edit-wins test; pull-diagnostics flicker (`DiagnosticServerCancellationData`).

---

## Stub design — decided (Expert, 2026-06-28) — keep

Tiered, both as `#:` decl-only stubs (no bespoke binary format):
- **stdlib (base/stats/utils/methods):** curated in-repo, compiled into the binary, selected by detected R version; loaded once; the "known" universe for strict mode.
- **CRAN (post-release):** discover installed packages (`.libPaths()`/renv/DESCRIPTION), auto-generate shallow stubs via real-R introspection (`getNamespaceExports()`+`formals()` → arity + arg names, `Any`/`Incomplete` returns), cached per version; curated overrides; unstubbed → `Any`. `pkg::name` needs a `NamespaceGet` HIR node.
- **Isolation (hard gate):** stubs are immutable high-durability inputs — kept out of the engine's incremental dependency graph (set-once input).
- Validate stubs against real R (a stubtest-style check). Update `docs/.../stdlib-stubs.md` to the chosen format + override mechanism (and drop "(Proposal)").
