# Backlog — Production-readiness punch-list

**Status:** the engine cutover is complete (one memoized query engine is the sole analysis backend; the hand-rolled incremental machinery + drift oracles are deleted). **But the PROJECT is NOT production-ready** — a component passing its tests ≠ releasable.

**Goal:** bring the ENTIRE repository to **impeccable, rust-analyzer-level production quality**. Repo hygiene is expected, not optional. The Director (CEO) finds + fixes everything short of that bar proactively — the user must not have to point things out. Do NOT declare ready until it genuinely is; the **user merges**, not us — report only when impeccable.

**Standing rules (user):** testable things use fixtures, not ad-hoc unit tests. Comments must be **context-free** — no internal milestone/process references (R0–R3, M1–M4, "Phase 4", "gate (c)", "3f", "the spike", commit hashes); a fresh reader with zero project history must understand them (AGENTS.md).

**Sources:** user review + an Expert release-readiness audit. Anchors are `file:line` and may drift.

---

## P0 — blocks release

- **Activate the widened CI pipeline** *(human — token-gated)*. The core-gating CI (whole workspace minus `rofy`/`zed_roughly`) is written and locally verified (clippy 0 warnings + full test suite green) but staged in `.github/pending-ci.yml`, NOT active: an automated session's token lacks GitHub `workflow` scope so it cannot write under `.github/workflows/`. A human must `git mv .github/pending-ci.yml .github/workflows/ci.yml`, push, and confirm the run is green. The clippy fixes it depends on are already merged on this branch.
- **`architecture.md` is a half-stale CONTRACT** *(DX, with CTO for engine facts)*. Describes deleted machinery as live (≈190–630: reverse-dep index, dirty-set, `maintain_package_naming`, drift oracles — all gone; every named symbol greps to 0 files) and the new `crates/engine` is documented nowhere. Rewrite to describe the actual engine (red-green substrate, revision clock, worker + cancellation; `analysis` = from-scratch oracle + CLI). Remove the M3/M4 headings + the "Slice plan" task-tracker (≈613–630). Stale code-comment mirror: `analysis.rs:651–658`. Source the engine architecture from `.agents/memory/decisions.md` + `crates/engine/DESIGN.md`.
- **Stubs — standard-library coverage + highlighting** *(CTO + Expert; user hard requirement)*. The **format is done**: dedicated declaration-only `.Rti` files (`name : <type-expr>`, reusing `type_syntax::parse_surface_type`), parsed by `stub.rs`, loaded + override-folded by `stdlib.rs`; the ~12-fn base/stats/utils/methods corpus is ported and there is a `tests/stub` fixture suite; `stdlib-stubs.md` + `structure.md` are updated. Parametric HOFs get real `<T> fn(...)` generics, ad-hoc overloads get `Any`, and the grammar permits repeated declarations (loader last-wins) so overload sets can arrive later without a corpus rewrite. Two type-syntax extensions have landed: **dotted parameter names** (`na.rm`, `length.out`; interior `.` in parameter/field names only, via `member_name_span_at`) and **variadic `...`** (`FunctionType.variadic: Option<Box<Type>>` shared by Surface+Core; arity absorbs surplus positionals via the existing `check_argument`; conservative compatibility — variadic-only-with-variadic; no inference change). A representative variadic corpus (`paste`/`paste0`/`cat`/`sum`/`prod`/`min`/`max`/`range`/`mean`, `na.rm` optionals) is in `base.Rti`. **Known limitation:** annotating a variadic over a hand-written `function(...)` body reports a spurious type mismatch, because inference lowers R's `...` as an ordinary named parameter (unchanged, per the no-inference-change scope) so the annotation-vs-body compat rejects it; variadic is therefore effectively stub/declaration-only. Bridging it needs either an inference change (make `function(...)` infer a variadic) or a compat special-case (treat a trailing `...`-named param as satisfying a variadic expectation) — a deliberate follow-up decision, not done. **Remaining:** (1) **real stdlib corpus** — expand base/stats/utils/methods well beyond the seed set. Numeric variadic elements use `Any` (not `double`) because the checker does not widen `integer`→`double` at a parameter position (would falsely reject `sum(1L, 2L)`); precise numeric elements need that widening or the generic-vector work. The **generic `T[]` suffix is deferred** (`rev`/`sort`/`abs`/… stay `Any`): the core vector can't hold a type variable and `T[]` isn't sound for arbitrary `T` (not all types are array-like) — the generic-vector-vs-trait design is an open question in `typing-design.md`; (2) **syntax highlighting** — LSP semantic tokens first (one server impl covers inline `#:` + `.Rti` files in both editors), tree-sitter/TextMate second; (3) **`pkg::name`** — needs a real `NamespaceGet` HIR node (today `Unsupported`) + per-namespace `StubLibrary` structure (currently flat, all folded into base); (4) type/class (`@type`) declarations in `.Rti` (not yet expressible); (5) **wire project-override discovery** — `stdlib.rs::discover_project_stub_sources` exists and is `.Rti`-correct but is called by nothing, so the loader-level override (`load_with_overrides`) is never fed project stubs: the mandated "project supplies its own → precedence" is not functional end-to-end until analysis/server setup invokes discovery. CRAN-tier introspection + R-version keying + stubtest validator are R-dependent future slices (see `decisions.md` / `stdlib-stubs.md` §7-9).
- **Docs + website — perfect shape** *(DX; subjective → screenshot to the user before declaring done)*:
  - Landing page: particles all on the RIGHT + logo built from SQUARE shapes → **organic** particles, distributed.
  - On scroll, particles should **morph into the heading** "Modern developer tooling for R." — not the heading appearing on top.
  - "IDE features in your editor." tabs look bad + **layout shift** on click → fix both (reserve dimensions).
  - Formatting-section examples not distinct (just operator spacing) → inconsistently-formatted code **morphing into consistent shape**; pick genuinely distinct examples (auto-bracing, alignment).
  - Full docs-site accuracy + clarity pass (contracts). Prior attempt disliked — raise the design bar.

## P1 — should-fix before release

- **Top-down ordering — MAJOR violators** *(CTO)*: `typecheck.rs` (`InferenceError` def 4695 used 110; `Binding` def 4805 used 41; `BuiltinKind`/`SubscriptKind` stranded), `naming.rs` (`TypeInfo` def 1197 used 143; `TypeResolver`/`DocumentNamingContext` after their callers). (`server.rs` is only MINOR — P2.)
- **Context-free / process-leak comments** *(CTO)* — ~21 `.rs` hits incl. **public module doc-comments** (`engine/src/queries.rs:5,10,60,120,134,740,784`; `engine/src/engine.rs:6,43,53,78`; `server.rs:153,161`; `analysis.rs:384`; `typecheck.rs:482`; `ide.rs:111`). Plus **`crates/engine/DESIGN.md`** (28 R0/R1/spike/gate refs — a milestone narrative that ships in the crate): rewrite as a clean architecture doc.
- **`structure.md` wrong** *(DX)* — cites non-existent `workspace.rs` + `resolve_document` (it's `resolve_document_locally`); omits `ide/generic.rs` + the whole `engine` crate.
- **Perf/memory/fuzz tests `#[ignore]`'d + ungated** *(CTO)* — 12 ignored tests; wire into CI with thresholds; **record the 281k memory number + acceptance verdict** in the decision log.
- **Sloppy TODO/HACK** *(CTO)* — `server.rs:72` tokio-main "???", `format.rs:1163` "HACK … ths" typo, `index.rs:207` incomplete TODO.
- **Secondary doc staleness** *(DX)* — `development.md:11` "three crates" omits `engine`; `testing.md` omits the engine differential harnesses; `stdlib-stubs.md` (≈123–139,233–235) couples to the deleted incremental substrate + leaks internal milestone names; it is still titled "(Proposal)". **Stale Roadmaps that list already-shipped features as future:** `language-server.mdx` (goto-def / refs / rename / type-info — contradicts its own Features section just above), `development.md` (type checking, rename, inlay, unused), `linter.md` (the `unused` check; plus typo "Booleans values" → "Boolean values"). **Install story inconsistent** across surfaces (getting-started says binary, website says `cargo install`, VS Code is one-click) — reconcile, ordered VS Code → binary → cargo.
- **Malformed-input regression test (engine `Lower`)** *(CTO)* — add a test asserting the engine emits no naming/type diagnostics on a transiently-malformed file (the `!root_node().has_error()` → empty `Module` short-circuit). The differential generator is well-formed, so it cannot catch a regression of this load-bearing invariant (see `MEMORY.md` engine invariants).
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

## Stub design — decided, keep

**Format mandate (user):** stubs are NOT ordinary R files. They are declaration-only files written in Roughly's own typing syntax — analogous to Python `.pyi` and TypeScript `.d.ts`. The exact surface form (file extension, whether it reuses `#:` annotations on empty signatures or a cleaner dedicated grammar, how overrides are expressed) is a CTO + Expert design pass — look at the `.pyi`/`.d.ts` ecosystems for precedent. Stubs must get **syntax highlighting** (LSP semantic tokens and/or a tree-sitter/TextMate injection grammar in the editors).

Tiered, decl-only stubs (no bespoke binary format):
- **stdlib (base/stats/utils/methods):** curated in-repo, compiled into the binary, selected by detected R version; loaded once; the "known" universe for strict mode.
- **CRAN (post-release):** discover installed packages (`.libPaths()`/renv/DESCRIPTION), auto-generate shallow stubs via real-R introspection (`getNamespaceExports()`+`formals()` → arity + arg names, `Any`/`Incomplete` returns), cached per version; curated overrides; unstubbed → `Any`. `pkg::name` needs a `NamespaceGet` HIR node.
- **Isolation (hard gate):** stubs are immutable high-durability inputs — kept out of the engine's incremental dependency graph (set-once input).
- Validate stubs against real R (a stubtest-style check). Update `docs/.../stdlib-stubs.md` to the chosen format + override mechanism (and drop "(Proposal)").
