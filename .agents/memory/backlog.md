# Backlog

**Standing goal (user mandate): empty this list and keep the project at rust-analyzer quality.** The beta program that once organized it is complete; shipped work lives as one-line ledger entries at the bottom (rationale in `decisions.md`, contracts in the docs). Every open item sits in one of the sections below.

**Quality bar (acceptance):**
- **Sound on idiomatic R:** no known accepts-then-crashes holes on supported constructs; unsupported constructs may be refused loudly (sound-by-refusal is acceptable) but never silently mistyped.
- **Zero false positives on the ~200 most-used base functions** with `[check] typing = true` on idiomatic call forms.
- **Performance:** keystroke-to-diagnostics p50 ≤ 30 ms / p95 ≤ 100 ms at 300k LoC (read against the raw-parse floor the instrument prints — latency numbers swing ~1.4x with machine load); budgets pinned by `stats_witness` (per-line wall/memory/resolve-step ceilings) with the measurement instruments in `legacy/differential/tests/test_stats.rs`.
- **No server-killing input** (no `unwrap` panics on protocol-legal messages).

## Open — documentation review findings

Four independent reviews read the docs cold (a first-hour newcomer, an information architect, an
accuracy auditor executing every claim against the binary, and a positioning analyst who also
measured the competition). The accuracy fixes landed; what remains is below.

**Bugs the reviews found, each with a repro:**

- **Numeric conditions FIXED** (`if (length(x))`, `while (n)`, `!length(x)` — R coerces them, zero
  false and anything else true; `character`/`complex`/`raw`/vector conditions stay errors). One
  residual: a condition whose type is still *undetermined* is bound to `logical`, so
  `function(n) while (n) ...` infers `n: logical` and rejects a numeric caller. Fixing that
  properly needs a "coercible to a condition" constraint — a fourth constraint kind, which is the
  documented tripwire for designing traits rather than accreting (see `typing-design.md`).
- **Named-before-positional matching FIXED.** `match_arguments` walked the argument list once, in
  source order, so a positional argument could take a formal that a *later* named argument was
  going to claim (`vapply(xs, character(1), FUN = f)` reported a bogus "FUN given twice").
  Matching is now two passes in `argument_targets` — names claim their formals, then positionals
  fill what is left — computed once and shared by the checking loop and the rest-parameter
  forwarding scan, which previously duplicated the accounting and so duplicated the bug.
- **The accumulator idiom errors where R returns `NULL`.** `args <- list(); if (x) args$escape <-
  TRUE; args$escape`. Worth a design decision rather than a patch: a *definitely* absent field
  should error (that is the flagship win), a *possibly* absent one should yield `T | NULL`.
- **Literate-document prose blanking FIXED, and it hid a worse bug.** A non-breaking space is
  whitespace to Rust's `char::is_whitespace` and an unexpected character to R's lexer, so prose
  containing one reported a syntax error against a blank line. The same code blanked per
  *character* rather than per *byte*, so any non-ASCII prose shifted every byte offset after it —
  every downstream range is a byte offset, so diagnostics in later chunks were silently
  misplaced. Both are fixed by blanking each character to its own `len_utf8()` in spaces; the unit
  test that was supposed to catch this asserted char count instead of byte length.
- **Self-checking ggplot2 reports 1132 findings and takes 2.4s** — the one package a stub ships
  for. The proposed cause (the shipped stub colliding with the package's own definitions) does
  **not** reproduce minimally: a package named `ggplot2` that defines `geom_point` and calls it
  checks clean, so project-wins-over-corpus works. The real cause is unidentified and needs the
  actual source (fetch the corpus); the likely candidates are ggplot2's own ggproto/R6 layer and
  its NSE, both known gaps, in which case the number is honest rather than a bug.
- **Duplicate type names go unreported in script files.** `reference.md` says the duplicate-`@type`
  error fires "regardless of file"; it fires in package files only. The value-name analogue is
  deliberately exempt for scripts, type names are not.

**Documentation that is still wrong (verified, not yet fixed):** `reference.md` documents a
rest-parameter spelling (`...items`) that never parses, and cites `.agents/memory/typing-design.md`
— a published contract pointing at an unpublished file; `stdlib-stubs.md` names six symbols
(`BuiltinKind`, `parse_surface_type`, …) that exist only in the frozen legacy tree, and puts `...`
last in `paste` when the real declaration has it first (the position is load-bearing);
`architecture.md` still uses internal gate vocabulary; `development.md`'s re-bless command omits
`ROUGHLY_BLESS=1`; the `stub` diagnostic code and the SCREAMING_SNAKE naming exemption are emitted
but documented nowhere; five smaller `language-server.mdx` items (a `bun run package` with no root
`package.json`, three wrong VS Code palette titles, 4-of-5 code actions, `PAREN_EXPR` folding
omitted, a `--verbose` example whose own help says it is ignored).

**The structural problem, which is bigger than any single fix.** The site is organised around
Roughly's subsystems rather than around anything a reader wants to *do*, so there is no tutorial
and no how-to layer at all: `typing/guide.md` restates `typing/reference.md` in the reference's own
order for eleven of its thirteen sections and never asks the reader to run anything, and
`stdlib-stubs.md` is an internal design RFC (`## Problem`, sections titled "not buildable here")
sitting in the user sidebar. Six pages are missing, in value order: **adopting Roughly on an
existing codebase** (the per-file `# typing: on` ladder exists and is described in seven scattered
places, never as a table — every peer type checker leads with this page), a **diagnostics
reference** (codes are a contract used by `allow(CODE)`, `[lint] CODE = "off"` and the JSON output,
and no list exists), **CI** (exit codes, `--min-severity`, `fmt --check` and JSON Lines all exist
*for* CI, and the site has zero YAML), **why a type checker for R**, **limitations** (data frames,
matrix shape, the three object systems, known false positives), and **comparison** (Air, Jarl,
lintr, styler, languageserver, checkmate — a page that concedes formatting and lint breadth openly,
because one that wins everything reads as marketing).

**Positioning.** The headline leads with the formatter and linter — the two things Roughly loses at
today (Posit's Air is bundled in Positron; Jarl ships 71 rules with `--fix` and an LSP, 140× faster
than lintr) — and buries the one thing nobody else has. Measured facts the docs never state: 854
files / 166k lines in 3.6s, and lintr 38.2s vs 0.47s on dplyr. Research confirms **no one has ever
shipped a static type checker for R**: Vitek's group proved it viable (~80% of CRAN functions
monomorphic or nearly, 1.98% contract-failure rate) then pivoted to a JIT IR, the one Damas-Milner
attempt (`RTypeInference`) went dormant in 2021, and Posit's `ark` README states it plans
"sophisticated static analysis of R code" citing rust-analyzer while Positron ships a Rust type
checker for *Python*. Full reports and their paste-ready rewrites are the four
`docs-review-*.md` files produced by that round.

## Open — adoption review findings (unfixed items, each with a minimal repro)

Three independent black-box adoption reviews (an analysis-script user, a CRAN package author, a
numerical-computing user; docs + `--help` only, no source access) simulated real projects and
converged on the same walls. What they found and is now fixed is in the ledger; what remains is
below, ranked by how often a real user hits it.

- **Overload selection with a flexible argument FIXED** (fact-versus-guess probing — see
  `decisions.md`): a candidate that fits without narrowing the caller's open types beats one that
  does not, and a single fitting candidate is selected outright. The apply family is now unblocked
  but still untyped where its result shape is *value*-dependent: `sapply`/`mapply`/`Map`/`tapply`
  stay `Any` because `simplify = FALSE` and a vector-returning callback change the result shape
  without changing any argument type, so no overload set can discriminate them — typing only their
  *parameters* (leaving the return `Any`) is the reachable win, at the cost of rejecting R's
  function-name-as-string form (`sapply(x, "length")`, already rejected for `lapply`).
- **The three object systems, measured (S3 partial, S4 and R6 recognition-only).** S3 *operator*
  dispatch is real (`+.Date`, `Arith.X`, `Ops.X` method names are built and dispatched, and the
  linter knows `generic.class` names), but `UseMethod` is not modelled — a generic call is `Unknown`
  — and `structure(list(...), class = "dog")` produces a plain record, so the class attribute is
  data. S4 is untyped end to end: `setClass`/`setGeneric`/`setMethod`/`new` are `Any` stubs, `x@slot`
  has no type, a slot typo against a class declared two lines above is silent
  (`setClass("A", representation(x = "numeric")); new("A", y = 1)`), and — the one *active* false
  positive of the three — `setGeneric("f", ...)` does not define `f`, so every call to a project's
  own S4 generic reports `unresolved`. R6 has no stub at all (`R6::R6Class` reports `unknown package
  namespace R6`); method bodies resolve `self`/`private`/`super` as a special case, but the class,
  its fields and its methods are `Unknown`, so `obj$typo()` is silent and completion after `self$`
  offers every record field in the workspace. All three are recognized structurally by the IDE
  outline (`classify_symbol_call`), which is where the type-side work can start. Fix order by pain:
  the `setGeneric` false positive, then an R6 stub, then S4 slot types, then `UseMethod`.
- **Matrix SHAPE is still untracked.** `%*%`/`%o%`/`%x%` now return the `matrix` nominal and the class
  has its arithmetic and comparison methods, so matrix expressions type and compose — but
  `matrix`/`t`/`solve`/`dim`/`crossprod`/`diag`/`apply` still return `Any`, so a transposed dimension
  or a non-conformable product is invisible. Declaring them `-> matrix` is easy; the value is in
  *dimensions*, which needs a shape-carrying matrix type (see the data.frame row-type design). Note
  the trap this session hit: making a constructor return a real nominal without also declaring the
  class's operator methods turns every `m + 1` into a false error.
- **A project's own `%op%` stays untyped by design** (the result is `Unknown`, a strict-mode origin):
  it may be an NSE wrapper whose right operand is quoted, like magrittr's `%>%`, and checking that as
  an ordinary call would reject correct code. Lowering `%op%` to the call it is (the documented `|>`
  precedent) would type it and give goto/references on the operator — weigh that against the NSE risk
  and the `unresolved` a bare-script `%>%` would gain.
- **`lapply` drops names**, so "named list in, named list out" is inexpressible. A list of
  *functions* is also still rejected — `lapply(list(mean = mean, sd = sd), function(f) f(1:3))` now
  joins the element to a union of two function types, and the callback check cannot yet satisfy a
  union in a contravariant parameter position. (The plain heterogeneous case is fixed: an open
  `list[T]` element takes the join.)
- **Everything from a `data.frame` is `Unknown`, and `Unknown` satisfies every annotation** — so on
  data-frame-heavy code annotations look protective and are not. This is the design consequence that
  decides the tool's value for analysis users; it needs at minimum a way to *see* that a check was
  skipped (strict mode, once it reports origins).
- **An S3 method declared in R is reported `unused`.** `Arith.Point <- function(e1, e2) ...` with a
  `#:` annotation *works* — `p + q` dispatches through it and `p + 1L` is correctly refused — but the
  binding warns `assigned but never used`, because dispatch is not a read. The lints already know
  `generic.class` names (`is_s3_method_name`, used for the unused-formal and naming-style
  exemptions); the unused analysis needs the same knowledge, at least for a method whose generic or
  operator group the checker dispatches through.
- **Only the STUB route to an operator method on a project nominal is blocked.** Declaring the method
  as an annotated R function works (verified above), which is what an author writes anyway for a real
  S3 class; the gap is narrower than "operator methods need ergonomics" — a `.Rtypes` stub cannot see
  a `@type` the R source declares (`this declaration does not load: I do not know the type Meters`),
  so only the stub spelling is unreachable. Decide whether stub sources should see project `@type`
  declarations at all, or whether the R-side declaration is simply the answer and the docs should say
  so.
- **Smaller, each with a one-line repro in the reports:** messages leak unbound type variables (`list[T] | T[]`) and expand an alias on
  only one side of an expected/found pair; `unused` false-positives on a write followed by `break`;
  closure re-entry is unmodelled, so the `if (!is.null(cache)) return(cache); cache <<- v` memo idiom
  yields `T | NULL`; a generic parameter cannot have a non-`NULL` default; the `unused` write-then-`break`
  false positive is NOT reproducible (verified across `for`/`while`/`repeat` and both used and genuinely
  dead writes) — drop it unless a concrete shape resurfaces; no `--fix`, no stdin, and no CLI way to ask "what type is this?" (which makes debugging an inference surprise guesswork for a
  CLI-only user). `sum(1, 2, 3,)` formatting to `sum(1, 2, 3, )` is NOT a defect and stays: the
  trailing comma introduces a missing argument, so it parses identically to the `alist(, )` idiom the
  space serves, and the `trailing-comma` lint reports the mistake.
- **Literate documents are analysed by `check` but not by the editor.** `.Rmd` / `.qmd` / `.Rnw`
  chunks are converted to an R program by blanking every non-R character (`syntax::literate`), so
  ranges need no translation and `check` reports at the original line and column. The LSP path still
  ignores them: `did_change` hands the engine incremental edits against the document the editor
  holds, and the converted text is a different buffer, so wiring it needs the original text kept
  alongside the analysed one (or the conversion applied per-edit). The formatter deliberately stays
  out — most of an `.Rmd` is prose.
- **An unannotated helper that wraps an operator over a class fails, and the tie is why.**
  `add_layer <- function(plot, layer) plot + layer` infers `<T: numeric> fn(plot: T, layer: T)` — the
  two flexible operands are tied to ONE variable — so `add_layer(base, geom_point())` reports
  `expected ggplot, found gg`. Same shape for dates: `add_days <- function(d, n) d + n`. Annotating
  the helper fixes it and the message is clear, but the tie is an over-commitment: R's `+` never
  required its operands to share a type, and a class that declares `+.Class` accepts pairings the tie
  forbids. **This is the "traits" / third-constraint-kind question in `typing-design.md`, now tripped a
  third time by shipped features** — the right fix is a "supports this operator" constraint instead of
  `Numeric`, replacing both the tie and the `declares_arithmetic` relaxation that lets an
  arithmetic-declaring class satisfy `Numeric` today. Next stub corpus addition that returns a real
  nominal will trip it again.
- **A type error inside `expect_error(...)` is still reported.** `expect_error(f("bad type"))` is how
  you test a type-related failure, and the call really is type-incorrect, so the finding is defensible
  — but it needs a suppression to write that test. Decide whether an expectation that asserts a
  condition should suppress type findings in its payload, or whether `# roughly: allow(...)` is the
  answer and the guide should say so.

## Open

 — semantics

- (Stub completeness audit CLOSED by the export-manifest layer — see the decision record and `stdlib-stubs.md` §Export manifests. `uname`-style reports remain user-project names: the fix stays a project stub or the DESCRIPTION-import tolerance.)

- **Legacy ide fixture port DONE** (fixtures directive, first half): 81 cases ported into `crates/ide/tests/ide/*_ported.R.test` (real legacy corpus: 134 cases / 206 operation sites; ~36 already covered; 15 skipped as genuinely multi-file — the harness is one `SourceFile` per case; deliberate improvements blessed). Cross-file navigation coverage now rests on the LSP tests — consider a multi-file fixture harness extension if that surface grows.

- (Design forks all DECIDED — two-flexible comparison stays unconstrained without a third constraint kind, union compatibility commits flexibles at first use in program order, NAMESPACE bare-resolution stays ungated; decisions.md has the three records.)
- Overload candidates when touched: `is`, `extends`, `grep(value =)`, `cor` (vector vs matrix — needs matrix nominals). `Date`/`POSIXct` arithmetic refuses loudly today — revisit if real code makes it noisy.

## Open — editor & polish

- Hover type fences (user-confirmed: no highlighting in current editor builds): the server tags the fences `roughly-type` and the VS Code extension in-repo ships a grammar for that id — needs a released extension update to reach users. Zed renders the fence plain until its extension registers an equivalent fence language (tree-sitter grammar required); consider falling back to tagging fences `r` for Zed if that proves distant.

## Open — structure & performance

- **Diagnostics-phase remainder:** the duplicate-binding/duplicate-type O(files²) walk is killed (see the ledger); the post-burst workspace revalidate (~1.1s at 713K LoC, user measurement) should shrink too — every file's diagnostics used to depend on every file's ranges through those walks, so any edit re-executed all of them — but re-measure on the real workspace to confirm before closing.
- **The rewrite is complete and shipping** (decisions.md "target architecture" record): every phase gate holds — corpus/round-trip/acceptance/fuzzing, semantic parity via the differentials, cutover suites, perf + memory + keystroke budgets, order-independent fixpoint, multi-core stress. The legacy crates stay in-tree **by user directive** until the user asks for the final deletion sweep; when that comes, migrate the remaining fixture data out of the legacy trees and archive a final corpus parity report first.
- **Parallel cold pass: measure on real hardware before optimizing further.** The investigation (`crates/roughly/examples/parallel_probe.rs` is the reproduction tool) found: (a) the long-recorded "4 workers buy only 1.2x" was mostly a measurement artifact — this container's 4 vCPUs deliver only ~1.8x of lock-free native compute at 4 threads (~1.2x at 2), so no in-container parallel number is meaningful; (b) the one real structural serializer was `interface_sccs` demanding naming for every item inside one salsa query (25-51% of cold wall depending on package shape) — fixed by the CLI's parallel per-item naming warm before the fan-out (mgcv cold pass 0.99s → 0.80s even in the throttled container); (c) salsa's same-query blocking is negligible (53 blocks per 5,657 executions) and the interface DAG is wide (ggplot2: 1,198 items, depth 22), so no fixpoint-scheduling work is warranted until a real-hardware measurement says otherwise.
- **Coverage-guided fuzzing landed** (`fuzz/` crate, testing.md documents the workflow): libFuzzer targets `parse`, `format`, and `semantics` over the exported invariant batteries plus `scripts/seed-fuzz-corpus.rs` (a `cargo +nightly -Zscript` single-file script, like all of `scripts/`); the lint layer is folded into the semantics battery (everything-on config), closing the last unfuzzed stage. First sessions found and fixed nine formatter bugs and two splice-equivalence bugs (a middle ending mid-construct must refuse suffix reuse; an empty-suffix splice must not rebase the old end-of-file error) — all pinned in per-harness `REGRESSIONS` batteries. Remaining: a scheduled deep-fuzz run on real CI hardware, and an `llvm-cov` coverage report (recipe in testing.md; skipped in-container for disk).
- CI: the widened whole-workspace workflow is staged in `.github/pending-ci.yml` — a human must `git mv` it into `.github/workflows/` (automated tokens lack workflow scope). Until then CI gates only the product crate's own suites; the workspace battery runs locally per slice. Authoritative perf numbers need the CI runner.

## Open — website & docs

- (The landing-page hero animation is user-owned — do not touch.)

## Open — REPL (v1 shipped; the analysis wiring is the open rung)

- **v1 SHIPPED and e2e-VERIFIED against real R** (`crates/repl` behind `roughly repl`; `repl-design.md` has the architecture, status, and the two pty-harness requirements): runtime-loaded R (no build-time link — the workspace builds R-less everywhere), reedline console inside the ReadConsole hook, lexer highlighting, conservative completeness with R's continuation as the safety net, SIGINT interrupt routing. The pty e2e suite (skip-if-no-R) runs green against real R — agent containers CAN install R (recipe in MEMORY.md short-term), so run `cargo test -p roughly --test test_repl_e2e` before REPL-touching changes, anywhere.
- **Analysis-backed Tab completion SHIPPED** (first analysis rung; `repl-design.md` has the seam design): typed signatures for stdlib names, session bindings, `pkg::` exports, manifest names — `SessionCompleter` seam keeps the repl crate syntax-only, `AnalysisCompleter` in roughly runs `ide::completion` over the session-as-script. **Open — remaining rungs:** live-session facts (the R environment listing unioned into completions), pre-evaluation diagnostics on pending input, hover on the input line, graphics-device story (versioned mirror structs, see the design record). The headless runner is shipped.
- **REPL Windows: real-machine smoke test pending.** The embedding is implemented (`repl-design.md` has the recipe: Rstart callbacks via R_DefParamsEx's version handshake, sibling-DLL preloading, RGui→LinkDLL switch, UserBreak+deferred interrupt pair) and compile/clippy-verified against x86_64-pc-windows-gnu — but no Windows machine with R has ever executed it. Smoke: `roughly repl` (prompt, evaluate, Ctrl-C, vi mode) and `roughly run` (output, exit 0/1). Known caveat to watch: terminal VT input handling in the editor layer.

## Post-beta (explicitly out of scope for now)

- Tags / discriminated unions via a compiler-known stdlib `match` (design in `typing-design.md` first).
- S3 dispatch modeling (`UseMethod`) — prerequisite for honest `print`/`summary`/`plot`.
- data.frame column-level typing; matrix dimensionality; real S4 typing.
- Traits/typeclasses (tripwire: the third constraint kind).
- CRAN stub auto-generation via R introspection, R-version-keyed corpora, stubtest validation (R-dependent). (NAMESPACE/DESCRIPTION awareness moved to Open — semantics by user ask.)

## Shipped ledger (one line each; rationale in `decisions.md`, contracts in the docs site)

- **`roughly check` stack overflow fixed:** the check fan-out's scoped worker threads ran on default 2 MiB stacks while cross-file interface resolution recurses per dependency edge — a ~500-file definition chain aborted the whole command (`analysis-stats` survived only because the command thread carries `ANALYSIS_STACK_SIZE`). All three scoped spawns in `cli.rs` now use `ANALYSIS_STACK_SIZE`; verified on the 5,000-file synthetic tree (SIGABRT → clean exit-1 with findings).

- **Duplicate-diagnostics O(files²) walk killed:** `duplicate_binding_diagnostics`/`duplicate_type_diagnostics` rebuilt a project-wide occurrence map per file AND made every file's diagnostics depend on every file's ranges (any edit re-executed them all). Now: range-free per-file name projections (`top_level_binding_names`/`type_declaration_names`, value-equal under body edits) feed memoized project duplicate maps filtered to real duplicates (near-empty in healthy projects — the value-equality firewall), and per-file diagnostics fetch ranges only for files actually involved in a duplication. Same-tree A/B at ~530K LoC / 5,000 files: semantic render 12.1s → 3.7s, cold 22.8s → 14.4s, diagnostics byte-count identical.

- **Export manifests — every real R export resolves:** `types/<ns>.exports` (generated from live R by `scripts/export-manifests.R`) covers every namespace R ships in three tiers — default-attached (now incl. `datasets`, famous frames typed `data.frame`) bare-visible unconditionally; `QUALIFIED_ONLY_NAMESPACES` (tools/parallel/compiler/grid/splines/stats4/tcltk) always valid after `::` but bare only when attached/declared; conditional CRAN namespaces gated with their stubs. Manifest names resolve (typing `Unknown` without a typed declaration), feed completion and typo suggestions, and a unit test pins every `.Rtypes` value declaration as a real export of its namespace (caught `traceback`/`standardGeneric` misfiled — both are base exports). Kills the could-not-resolve false-positive class for the whole shipped standard library.

- **Diagnostics-phase whale killed (was 69-80% of the cold pass at scale):** `declared_global_variable` scanned every project file per non-local read — O(reads × files), ~375M memo lookups on a script-heavy workspace — and script-local cross-statement reads paid the project-wide guard chain before their own frame-slot resolution. Now one memoized project-level `globalVariables` union set (`project_global_variable_declarations`) plus cheapest-first guard order (masked → frame slots → package/stub → super-globals → declared globals → imports). Reproduced at 305K LoC / 2,500 files: diagnostics 22.7s → 2.9s, cold total 28.0s → 8.9s, diagnostic output byte-identical; `analysis-stats` permanently splits the phase into semantic render / lints / assembly — the instrument that found it.

- **Formal-aware `@masked` + the conditional dplyr namespace:** the stub loader records each masked verb's pre-`...` formal names (`StubLibrary::masked` map) and naming resolves arguments matching them (position or name) while masking what `...` absorbs — zero-formal masks (`join_by`) mask everything, restoring the reference's documented contract; `dplyr.Rtypes` ships as the second conditional namespace with class-preserving `@masked` verbs (`<T> fn(.data: T, ...) -> T`), class-preserving joins, and the tidy-select/verb vocabulary, so piped verb chains type end to end with masked column reads (`dplyr` fixture group in typing-imports; decision record has the collision/shadowing note).
- **The native pipe types as the call it is:** `hir::lower_pipe` desugars `x |> f(y)` to `f(x, y)` (R's own parse-time rewriting) with the `_` placeholder substituted as the one named argument R allows it to be (`pipe_shape`, syntax-level; the `_` token never lowers, nothing dangles); everything R rejects keeps the opaque-operator lowering. Naming/typing/overloads/arity/strict/IDE inherit the real call; piped-value errors blame the left-hand range; pipelines type end to end. Gate terms renegotiated via two `ACCEPTED_DIVERGENCES` entries (oracle never modeled pipes); the two strict cases that pinned pipe-as-origin repurposed to `%in%`; `%>%` deliberately stays opaque (a real function, not sugar). Reference documents the rule under Function calls → The native pipe.
- **data.table awareness (NSE ladder rung 1):** a conditional stub namespace (`crates/semantics/stubs/data.table.Rtypes` — the `@type data.table` nominal + ~45 declarations) joins stub assembly only when `metadata::namespace_active` (DESCRIPTION/NAMESPACE declaration, or a `library()`-family call found by the per-file `file_attached_namespaces` scan, unioned into `PackageMetadata.attached` by the hosts — server incrementally per synced file + idle prime); a bracket whose subject is the `data.table` nominal masks all index reads (`ItemCheck::masked_reads` skips the unresolved warning — kills the `DT[speed > 20]` false-positive class) and classifies its result by `j`'s syntax (no/empty `j`, `:=`, `.()`/`list()`, grouped `j` keep the subject's class; the rest refuse as Unknown with a strict origin). Typing-reference "Data-masked evaluation" + stdlib-stubs "Conditional namespaces" are the contracts; `datatable` fixture group in the differential-excluded typing-imports suite; decision record has the cycle/keystroke-cost analysis.
- **`analysis-stats` ported to the new stack (user ask):** `roughly debug analysis-stats [path]` (`crates/roughly/src/stats.rs`) assembles the workspace exactly as `check` does (stubs, metadata, Collate order) and reports staged cold-pass phase timings (load / parse / lower+naming / typecheck / render) with per-phase resident-set growth and peak, slowest-files-by-typecheck, and a typing-burst probe on the slowest + median + small files with `CHECK_EXECUTIONS`/`RESOLVE_CALLS` attribution and the raw re-parse latency floor. Forces typing on with a note; documented on the development docs page; CLI contract test pins the report sections.
- **Missing-comma parse recovery (user ask, rust-analyzer style):** when a token that can start a new element follows a complete argument or parameter, the parser reports `` missing `,` between these arguments``/``…parameters`` anchored at the empty range right after the previous element — on that element's line, not wherever the next element starts — and parses the next element normally (a proper `ARGUMENT`/`PARAMETER` node, no `ERROR` wrapping, so downstream analysis sees the intended list). Junk that cannot start an element keeps the old `expected `,` or …, found …` recovery. `starts_expression` mirrors `primary`'s entry set (kept in lockstep); golden error cases pin single-line, cross-line, and junk-recovery shapes.
- **`DESCRIPTION` `Collate` file order implemented:** `parse_description_collate` (`Collate`, falling back to `Collate.unix`); both hosts rank package files by Collate index before the path tiebreak (unlisted files order after the listed ones), the server reorders the project when a DESCRIPTION change moves the collation, and a CLI contract test pins the winner flip. Closes the reference's "Project file order" promise, which had no implementation.
- **NAMESPACE/DESCRIPTION metadata feeds resolution (user ask):** `semantics::metadata` owns the NAMESPACE parser (moved from the host crate) plus a DCF DESCRIPTION dependency parser and the singleton `PackageMetadata` input; `importFrom(pkg, name)` names and stub-described `import(pkg)` exports are known bare reads, a stub-less `import(pkg)` tolerates all otherwise-unresolved bare reads (export set unknowable — the zero-false-positive rule), and `pkg::` reads of declared-but-undescribed namespaces stay quiet. Hosts install the input next to the stubs (server refreshes on NAMESPACE buffer sync + NAMESPACE/DESCRIPTION watcher events, diffing parsed facts). New `typing-imports` fixture suite (metadata directives documented in testing.md; excluded from the differential arm — the oracle has no metadata concept); typing-reference "Package imports" section is the contract; decision record in decisions.md.
- **Blame-range precision (trailing trivia + parens):** the real-file corpus scan found two systematic same-finding range near-misses vs the oracle — expression ranges swallowing trailing whitespace/comments the Pratt loop consumed while peeking for the next operator (fixed: HIR lowering stores the trivia-trimmed significant range), and blame sites reporting a parenthesized wrapper instead of the expression inside (fixed: type-error blame drills through `Paren`; strict origins and missing-formal reads deliberately keep binding-site ranges). Both pinned by fixture cases verified to fail without the fixes; the scan's 39 paren + 8 trivia near-misses went to zero and file-level corpus matching rose 3,293 → 3,303/4,638.
- **`[` on vectors defined (was the largest real-code gap):** the typing reference specifies the full index-shape × subject-shape matrix (scalar-like numeric/character index → the scalar-claim element, with the negative-index caveat documented under the flexible-operand compromise; vector-like and logical-mask indexes → the subject's vector shape, names surviving; character indexes legal on any vector; undetermined indexes claim scalar and stay unconstrained; list/function/complex/raw indexes error) and `subset_result` implements it through a `vector_index_shape` classifier. Seven fixture cases pin every row; the oracle never defined vector `[`, so its refusals are an oracle-deficit class in the shared filter (1,415 accepted on the real-file corpus — the rewrite stopped reporting them on real code) plus per-case adjudications in the fixture and IDE arms. The NSE/data-masking design ladder is recorded as typing-design question 7.
- **Corpus-scale verification + dots-forwarding arity fix:** the real-file corpus arm reran after the cross-item-read and typed-NA changes — 3,322/4,638 files match (up 54 from the prior sweep; oracle-deficit acceptances rose ~975 because the rewrite now resolves reads the oracle cannot, e.g. R6 `self`/`private`). The sweep's one genuine false-positive class is fixed: a call argument that is the enclosing function's bare `...` forwards an unknown number of arguments, so such calls now skip both arity checks (`CallArgument::forwards_dots`; typing-reference Function calls documents the rule). The sweep harness also gained the documented-but-missing oracle-side panic guard (an oracle panic is counted and its file skipped instead of killing the run).
- **Callback-idiom stub sweep closed by audit:** the capped-stub premise no longer holds — every high-use base/stats/utils stub already declares its real optional formals (`nchar`'s type/allowNA/keepNA included), and the remaining single-parameter stubs are genuinely unary in R; a fixture pins both directions (`nchar(x, type = "bytes")` directly and `lapply(list(...), nchar)` through callback forwarding).
- **Overload/compatibility fixture sweep + reference fix:** ten typing cases pin overload-set rules (undetermined arguments use the general candidate, the catch-all corpus convention, value-use resolution, local shadowing disabling the set), numeric-variable generalization (`-x`, `x / 2`), and function-type compatibility (by-name parameter pairing, the rename refusal, parameter contravariance both directions, variadic-never-pairs-with-fixed); a CLI contract test reaches the no-matching-overload error through a fully-constrained project stub override (unreachable via shipped stubs — every set ends in an `Any` catch-all). Writing them exposed a reference/implementation contradiction on non-call uses of overloaded names: the implementation deliberately resolves to the last (most general) candidate with recorded rationale; the reference wrongly said first — the reference is fixed.
- **Indexing/guard fixture sweep:** thirteen typing cases pin the documented `[[`/`[`/`$` rules (vector element extraction, the map-like positional/name-based asymmetry, declaration-ordered record positions, backtick fields, record slices, unpinned-parameter tolerance) and the guard-narrowing rules (negated guards, `is.numeric`/`is.function` families, scalar+vector family membership, guards that cannot fire, expression guards never narrowing, the tested-then-unguarded finding) — all matched the contract on first bless. The per-position IDE differential over the new cases caught one real defect, fixed: `$` field completion now offers non-syntactic names in their backtick-quoted (insertable) spelling via the new `syntax::is_syntactic_name`; plus one adjudication (the rewrite hints a scheme for an unpinned-field reader the oracle leaves untyped).
- **Operator/constant fixture sweep + typed-NA fix:** eight typing cases pin the documented operator rules that had no fixture (`%%`/`%/%`, `^`/`**`, unary `-`/`!`, comparison families, `:` shapes incl. the scalar-numeric bound, `&&`/`||` scalar-only) and the reserved constants; writing them caught and fixed a real bug — the typed `NA_*` constants all lowered through the bare-`NA` catch-all and inferred `logical` (now `LiteralKind::Na(NaAtom)` carries the atom at lowering).
- **Per-item interface projections:** whole-project walks (`interface_sccs`, `conditional_slot_items`, `script_definition`'s statement branch) read `item_interface_reads` / `item_top_level_names` — small tracked projections of naming — instead of full `ItemNaming`, so a body edit that shifts ranges without changing a name backdates and no project-wide walk re-executes per keystroke (event-counted by `examples/keystroke_probe.rs`: walks went 8/8 edits → 0/8; misc.r 8.2→4.9ms, zxx.R 5.9→3.3ms medians in-container). New-item edits still re-run the walks, as they must. Architecture.md documents the projection firewall.
- **Shadow lints landed (default-off):** `shadows-builtin` (a top-level binding over a `base` export) and `shadows-namespace` (over another stub namespace's name, message names the shadowed symbol) in `lints::shadow_lints` — driven purely by the stub corpus's `exports_by_namespace`/`declaring_namespace` because bare resolution is ungated, so no NAMESPACE-import plumbing and no CLI/LSP drift; dotted S3 names are naturally exempt (not exports). Fixtures in the lints-style suite, CLI contract test, linter + configuration docs updated.
- **Top-level unwritten-path reads observe the cross-item binding:** a top-level slot's read-before-write (a loop's first iteration, a rebinding statement's right-hand side) resolves through `GlobalEnv::scheme(name, deferred=false)` — nearest earlier item in scripts, definition winner/conditional slot in packages — mirroring the unused check's cross-item-read rule; the observed type is materialized as the slot's pre-state (`pre_materialized`, re-established after each loop-pass rollback) so the loop join keeps it and first-iteration type errors survive to the reported stable pass. Self-referential-only names keep the tolerant `Unknown` (cycle recovery's initial), and sequential script rebindings now chain types (`n <- n + 0.5` after `n <- 1L` is `double`). Typing-reference "Definite assignment" documents the rule.
- **Missing-diagnostic probes closed:** generic-application arity errors at the applied name (`generic type `Box` expects 1 type argument, but found 2.`), non-generic-with-arguments, and bare-generic-must-be-applied (except under `@new`, whose representation check infers arguments) — vocabulary-side checks in the annotation-rule family over lowering-recorded `applied_references`, with mis-applied names flooring to `Unknown` in the relations so the one arity error never cascades. Probes confirmed missing-required-argument and duplicate-formal detection already existed; their wording now says what happened (`a required argument is missing`, `names the argument `x` more than once`).
- **Legacy-corpus parity closed; the corpus arm is a default gate:** all 1,523 comparable single-file case inputs from the frozen stack's own suites match through the shared differential policy (1 adjudicated acceptance: the oracle blames a vector-element violation at the alias declaration where the rewrite blames the use site). The sweep drove, slice by slice: the annotation block-form validation package (attachment/dangling rules incl. the blank-line association fix, directive ordering, duplicate/unknown type parameters, applied binders, `@new` shape + on-alias, vector-element atomicity, nesting caps, definitions top-level-only; refused blocks drop their payload), declared-function shape checks (optional-needs-default, rest-position, both variadic directions; renderer places `...` at its boundary), expression-level annotations (the constructor idiom — statement-level attachment at any depth through one application seam), alias-typed callees calling through their expansion, elided nested returns meaning `NULL`, frame-scoped capture liveness (`Scope::id`), conditional top-level slots exporting per-binding schemes (`ItemCheck::top_level_bindings`, `statement_binding_scheme` with cycle recovery, joined across writers), export-edge generalization of constrained residual variables (`close_scheme`: `<T: numeric>` survives, unconstrained erases), and `missing()` supplied-state flow (`EnvEntry::MissingFormal`: reading a no-default formal on the missing branch errors; writes supply it; the marker is branch-local). Plus the differential fuzz arm (six gaps) and the unknown-type-name class (the reported `Instument` bug) from the same assessment.

- **Error-message release pass:** the golden error suite grew 14 -> 78 cases organized by area, now covering every distinct lexer/parser message template plus recovery-locality and valid-stays-clean pinning (testing.md documents the coverage contract); writing it exposed and fixed a real cascade class — lexer `ERROR_TOKEN`s re-diagnosed by the parser (up to 4 reports for one mistake, now 1) via silent placeholder-atom consumption + statement-level suppression; the fuzz harness gained error-quality invariants (non-empty messages, in-bounds ranges, cascade bound linear in tokens). CI was red on the recorded newer-clippy trap (collapsible_match on 1.97) — fixed against CI's actual toolchain.
- **Docs accuracy pass + truthful landing examples:** development.md rewritten for the shipping crate layout, testing.md's legacy-era half compressed into a scoped frozen-stack section, linter/configuration/stdlib-stubs stale claims fixed (missing-comma retirement, stub path), shipped stubs migrated into `crates/semantics/stubs/` (the product no longer include_str!s from the legacy tree), the staged CI's rationale refreshed — and the landing page's formatter examples replaced after verifying each against the real binary: the old panels showed behavior the formatter doesn't have (bracing `if` one-liners, splitting one-line pipelines, aligning `=` columns); the new panels show verified auto-bracing of loops, operator/comma spacing, and multi-line indent normalization, with the tabs' reserved-dimensions fix confirmed already in place.
- **Formatter docs generation ported to the product:** `crates/format/tests/test_format_docs.rs` regenerates `docs/formatter.md` from `crates/format/tests/formatter.template.md` through the shipping formatter (every template example formats byte-identically to the legacy output); the legacy generator and its template copy are removed.
- **Recursion precision + strict attribution:** the canonical interface fixpoint types converging recursion precisely (top-level `fact`: `fn(n: integer) -> integer`; mutual groups generalize — the old tolerant-`Unknown` self-recursion contract is superseded in the typing reference), and strict mode now attributes the remainder: a cycle member with a clean body whose exported scheme still carries `Unknown` gets a binding-level `RecursiveUnknown` origin; pinned-at-cap cycles keep their read-site origins (decisions.md).
- **Corpus growth + wording polish:** the fetch manifest gained 28 large CRAN packages (Matrix/MASS/mgcv/survival/Hmisc/caret/sf/...) and the source-extension pattern widened to R's full set (`.R/.r/.S/.s/.q` — mgcv ships `.r`, Hmisc `.s`; the four corpus loaders match): corpus 507K -> 965K lines (81 packages, 30.5 MiB, 4640 files), all parsing with zero acceptance/round-trip divergences. At the new scale (per-process instrument protocol — batching the stats tests in one process pollutes the RSS numbers): new stack 19.0s (19.7µs/line) / 1.0 GiB resident vs legacy 30.3s / 2.0 GiB — 0.63x wall, 0.50x memory; parallel == sequential findings exactly (the canonical-fixpoint invariant holds at scale). The one new acceptance divergence was a real lexer bug (`..2dge` mislexed as `..2` + `dge` — R has no dot-dot token at lex time, so a longer name wins; fixed with a golden case). Diagnostic wording pass per the Rust/Elm bar: real pluralization for call-arity/named-argument errors, the `invalid semantics:` prefix dropped and the three `#:` block-form refusals rewritten to name the fix (separate blocks with a blank line), `NotAFunction`/`MixedListElements`/index-shape phrasing tightened.
- **Product-surface polish batch (new stack):** hover definition summaries ("Local variable/Package global, defined at `path:line:col`", stub origin namespace + overload count, maybe-undefined note), `debug = true` hover debug sections (Lowering/Naming/Parsing), S4/R6 document-symbol hierarchy (kinds + R6 member children, workspace symbols include members with real kinds, `fn(params)` details), a deterministic cancelled-pull LSP test (`ROUGHLY_TEST_DELAY_PULL_MS` fault-injection seam holds the pull until the edit's flip lands), **unused warnings on by default** (user directive; `[check] unused = false` opts out) with two script-unused fixes it forced: bare-statement reads (`print(x)`) keep bindings alive, and a definer inside an R-grammar error region never warns.

- **`roughly check` runs on the engine:** the CLI builds the same query graph the server uses (server `ProjectFiles` ordering, shared `assemble_engine_file_diagnostics` in `crates/roughly/src/diagnostics.rs`), so it inherits every engine performance property; `run_full` remains purely the differential oracle. Cluster repro check: 1.34s → 0.28s.
- **Per-definition interface-SCC rounds:** mutually-referencing file clusters (one giant SCC under file-granular edges — THE real-workspace whale, 95% of a 700K-LoC user cold pass) now re-infer per member definition for provably-decomposable files (`scc_definition_plan`), with change-driven skips at both granularities, per-file contribution merges (exact last-writer-wins), one snapshot/rollback inference state per fixed point, change-event oscillation history, and a dense borrow-only `SymbolScc` Tarjan. Cluster repro: cold typecheck 3.6s → 0.2s, member-file keystroke 359 → 34ms. `analysis-stats` now bursts a median and a small file besides the slowest.
- **Checker constant factors (~4× whole-file inference):** allocation-free free-variable walker (no more resolve-clone per recursion level), dense `EntryTable` vector for the union-find, Tarjan letrec-membership (was per-candidate transitive walks), precomputed winner-test lookups + batch exported bindings, FxHash for the never-iterated state maps; per-file interface edges (`WalkShadowed`/`FileInterfaceEdges`) projected per symbol (hub sweep 763→33ms).
- **One inference per file per revision:** `Typecheck` owns `FileInference { check, exports }`; `ExportedSchemes` is a shared-pointer projection (still the value-eq firewall); the could-not-resolve typo hint is memoized per symbol with an allocation-free corpus scan; the letrec candidate-edge scan is one arena pass; `Diagnostics` fetches tree-readers adjacently (one parse per file on cold prime). Cold 10.1s → 3.7s at 302K LoC, keystroke 5.4 → 1.7ms; `analysis-stats` splits lint / package-naming / render stages, and a `profiling` cargo profile (release + symbols) supports sampling.
- **Interface routing:** same-file backward references are walk-resolved, never interface-imported (kills the fake same-file SCC/Tarjan blowup — 100× on chain files; counter witness); scripts overlay their declarations on the memoized package type environment (was O(scripts × package files)).
- **Semantics core:** multi-member unions; mutable-slot model with union joins; `<<-`/`->`/replacement forms; coercion policy; name-aware signature matching; `Unknown`/`Any` tolerance in `c`/`for`/`$`/`[[`/`[`; flow-sensitive guard narrowing (+ divergence-aware joins, `missing()` supplied-state); `is.null` shaping of unconstrained variables (unannotated coalesce); elided annotation returns; `...` as positioned rest parameter end-to-end; variadic bridging into callbacks; `switch`/`return` as checked control flow; dispatch-table `[[` unions; computed-key container refinement; positional `[[` record extraction; S4 `@` slot lowering.
- **Stub unlock:** `T[]` constrained generics; overload sets (probe-committed, two-round selection, signature-help/hover display); opaque `@type` nominals (data.frame/factor/matrix/…); named-into-rest absorption (typed `read.csv`, `lm`); ~530-declaration corpus across 6 namespaces; project-stub namespaces (`pkg::name`); NAMESPACE import validation + `unused-import` lint; `library()`/`require()` NSE quoting; stub-error surfacing on every surface.
- **Trust & UX:** config subsystem rebuild (nearest-ancestor discovery, per-lint severity, config-file diagnostics, reload refresh); per-file typing directives `# typing: on|off|strict` (tri-state `TypingMode`, one gate everywhere); data-masked NSE resolution (data.table brackets, with-family); strict-mode product story; unified lint framework; CLI rendering + exit-code contract; `roughly debug analysis-stats` workspace performance diagnosis.
- **Editor:** hover quality (`name : TYPE`, overload notes, constraint display); annotation cursor features via re-lexing (hover/goto/completion in `#:` comments); insert-annotation code action (round-trips); unused fade-outs; formatter rewrite with `#:` block awareness; letrec naming (local recursive closures resolve).
- **Engine & scheduling:** red-green core with per-symbol interface firewalls, SCC fixed point, tombstones, eviction, stacker-grown validation spine; durability tiers (open docs LOW, corpus HIGH; sound downgrade re-min through cutoff nodes); memoized completion index + `NamesGlobal`-valued symbol index (zero-copy reads); two-wave diagnostics publish + idle-time semantic wave + lossless preemption pairing + background prime; error-tolerant lowering ("a broken region reports its syntax error and nothing else"); differential correctness vs from-scratch oracle over adversarial edit streams, byte-exact, IDE features included; committed latency witnesses (at-rest reads ≤ 32 memos, size-independent post-keystroke walk, blast-radius exec counters); memory shape at scale (rope-only corpus inputs + on-demand trees, single-retained modules, boxed annotations: 1 GiB → ~300 MiB at 302K LoC) and O(open) keystroke validation (fold split over the `OpenFiles` seam, FxHash memo table: 11K → ~280 slots/keystroke); `analysis-stats` reports per-phase memory, typing-burst recompute counts, and walk attribution.
- **Docs:** getting-started leads with a real bug; installation split out; typing guide + reference as contracts; architecture/structure/testing contributor pages; linter/configuration/stdlib-stubs pages current.
