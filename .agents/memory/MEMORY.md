# Memory

Cross-session knowledge base for the agents building Roughly. Three horizons:

- **Short-term** — current focus and loose ends. Prune aggressively; delete each item once it is resolved or obvious from the tree.
- **Mid-term** — active priorities, open bugs, and technical debt. Lives across sessions until done.
- **Long-term** — durable, non-obvious design decisions and their rationale. Only things a future agent would otherwise rediscover. Keep terse and point at code or the docs.

Companion documents (kept separate only because they are larger in scope): `worklog.md` (one line per autonomous work cycle — the *only* deliberately chronological file here; an every-3-hours routine appends to it), `backlog.md` (the prioritized work punch-list), `decisions.md` (the settled architecture decision log), `typing-design.md` (open, not-yet-decided type-system design questions), `typedr-design.md` (inline type syntax as a compiled dialect — user-initiated, unscheduled; its own recommendation is NOT to build it for inline typing alone, because the ergonomic case does not survive checking, and to build it only if the later capabilities it enables — checked record/tuple constructors, tagged unions — are wanted), `repl-design.md` (the rofy-successor REPL: runtime-loaded R with no build-time linking — user-initiated design, unscheduled), and the repo-root `NSE.md` (working draft for checking data masking, data.table first — user-requested location; graduates into `typing-design.md` §7 / the typing reference as it settles). Authoritative user- and contributor-facing specs live in the docs site (`docs/src/content/docs/`): `reference/type-system.md` (the typing contract), `type-checking/tutorial.md`, `contributing/{architecture,structure,testing,authoring-stubs}.md` — point at them, don't duplicate. Do not spawn new knowledge files for small things; inline them here. Every entry must be **context-free and timeless** (clear to a reader with zero project history — no milestone/phase/gate names, commit hashes, or "this session") and **high-signal** (terse; point at code/docs).

## Short-term

- **Adoption reviews are the current source of truth for what to fix next.** Three independent black-box reviews (docs and `--help` only, no source access) simulated an analysis-script user, a CRAN package author, and a numerical-computing user on real multi-file projects. They converged on the same walls, which now head `backlog.md` with a minimal repro each. Two lessons worth keeping: the reviews found *quadratic* behaviour the whole in-house instrument suite had missed, because every witness measured many files rather than one file with many top-level items — a shape real repositories have (generated bindings, a 4,000-line `utils.R`) and the corpus did not; and the highest-value fixes were not new features but *reporting* honesty — a per-item cap on findings, per-call argument caps, and a verdict that changed with the command line all made a clean run mean nothing.
- **A per-item performance trap to watch for.** Any per-item query that walks or re-classifies the whole file is quadratic in top-level items, and the file-count-based witnesses will not see it. `item_annotation_syntax` did exactly that (13.9s on 2,000 statements). The pattern to use instead: compute once per file in a memoized query, probe per item — the per-item query still cuts off downstream work, because an untouched item's derived value compares equal. `item_spans` is the single source of top-level item identity and ranges; never re-derive the classification, and never scan it per item.

- **Fixtures directive DONE:** the legacy ide fixture port landed (81 cases; both real bugs it surfaced — the sibling-scope completion leak and the `@new` declaration hover — are fixed and fixture-pinned) and the identity-proving differential arms are retired (user decision — the new stack's fixtures are the contract; no oracle agreement needed). `legacy/differential` is benchmark-only (`test_stats.rs`). AGENTS.md and testing.md updated.
- **REPL Windows support is VERIFIED on real Windows + R 4.5.2** — the pty e2e suite (7 tests, now enabled over ConPTY) runs green, plus `roughly run` exit codes, `system()`, `~`, and `.Platform$GUI`. The original `STATUS_ACCESS_VIOLATION` root cause: `GA_initapp` lives in `Rgraphapp.dll`, not `R.dll`, and skipping it crashes `readconsolecfg`. Recipe details and the follow-on fixes (CRLF feed normalization, `home` from `getRUser()`, the UTF-8 code-page manifest, wrapper-script path vars on Unix) live in `repl-design.md`. The Unix side of these changes is compile/clippy-checked against x86_64-unknown-linux-gnu only — the next Unix session with R should re-run the pty suite.
- **Manifests are the cheap lever on the tolerance hole, and R is installable here to make them.** A
  namespace with an `.exports` manifest has a knowable export set, so attaching it never triggers the
  blanket unresolved tolerance — no typed declarations needed, every name just types `Unknown`. That
  is how the tidyverse and friends stopped switching the check off project-wide. Two traps: the
  generator must run on an R at least as new as the one each existing manifest records (it now refuses
  and says so — regenerating on an older R silently drops names like `%||%` and turns every use into a
  false `unresolved`), and a *meta*-package like `tidyverse` attaches its members rather than
  exporting them, so activation has to expand to them (`stubs::META_PACKAGE_MEMBERS`; the generator
  prints what a live session attaches, so the list is checkable). Installing the tidyverse from source
  needs system headers first: `libcurl4-openssl-dev libharfbuzz-dev libfribidi-dev libfreetype-dev
  libpng-dev libjpeg-dev` (curl, textshaping and ragg fail without them, taking the meta-package with
  them).
- **The Unix REPL is e2e-verified against real R** — the full pty suite (7 tests) runs green (see `repl-design.md` for the two harness requirements: answer the editor's cursor-position query; serialize sessions). **R installs in agent containers**: plain `apt-get install -y --no-install-recommends r-base-core` is enough for a distro R (Ubuntu ships 4.3.3), or add the CRAN apt repository (`cloud.r-project.org/bin/linux/ubuntu`, key `marutter_pubkey.asc`) for current R; `install.packages` compiles data.table/dplyr/testthat/ggplot2 from source in minutes. **Two build-dependency traps:** `fs` (a testthat dependency) needs `apt-get install libuv1-dev`, and every stub corpus addition must have its `.exports` manifest generated from a live session (`Rscript scripts/export-manifests.R`) — a manifest written from memory turns a real export into a false `unresolved`. Watch disk: the R toolchain + target/ can exhaust the session allowance — delete `target/debug/incremental` first.
- **Operating model (user-directed): full ownership, no check-in gates** — see the AGENTS.md ownership mandate. Pull the highest-value item from `backlog.md` §Open (semantics first), land it as a complete slice (contract-first docs, implementation, fixtures, full gates: workspace tests + clippy `-D warnings` + fmt), commit + push per green slice, keep memory/backlog current in the same slice. Only constraints: work directly on `main` (user directive), no new pull requests, and **keep the legacy crates in-tree (user directive)** — the final deletion sweep is deferred until the user asks for it, regardless of gate status.
- **The greenfield rewrite is COMPLETE and shipping.** `crates/{syntax,semantics,format,ide,roughly}` are the product; every gate of the target-architecture record holds (decisions.md); the architecture/structure/testing docs pages describe the current stack; shipped work is the backlog ledger. The frozen legacy stack (`*-legacy` + `fixtures`) remains only as the benchmark baseline. Current scale: the corpus instruments run ~965K lines / 81 packages — new 19.0s / 1.0 GiB vs legacy 30.3s / 2.0 GiB, parallel == sequential findings exactly. (All in-container timings are effectively ≤2-core numbers: the dev container's 4 vCPUs deliver ~1.8x of native compute at 4 threads — see the parallel-cold-pass entry in backlog.md before drawing any parallelism conclusion here.)
- The widened whole-workspace CI is staged in `.github/pending-ci.yml`; a human must `git mv` it into `.github/workflows/`. Until then the active CI gates only the product crate's suites — run the workspace battery locally per slice.
- Current state: the backlog §Open holds no open user-requested items, plus items blocked on external action (extension release for hover fences, CI hardware for deep-fuzz/llvm-cov, the human `git mv` of pending-ci.yml) and two standing habits — grow typing/ide fixture coverage, and the "when touched" notes on overload candidates. Coverage-guided fuzz targets cover parse/format/semantics with the lint layer folded in. The NSE ladder's verb level is SHIPPED: conditional data.table + dplyr stub namespaces, the bracket result-shape classifier, typed-subject masking, formal-aware `@masked`, and the native-pipe desugar compose end to end (see the decision records); `NSE.md` holds the remaining rung (column vocabulary + membership checks, gated on the data.frame row-type design). Details live in the backlog ledger.
- The landing-page hero animation is user-owned (do not touch). NOTES.md is human-maintained (never edit).
- **A stub-corpus addition no longer re-blesses the IDE fixtures.** They used to pin absolute byte
  offsets into `base.Rtypes`, so adding any declaration shifted them and forced a re-bless that
  proved nothing. The renderer now prints the token the range covers instead. If a corpus addition
  ever makes those cases fail again, the range is genuinely wrong — do not bless it away.
- **Landing-page code samples are captured from the running tools, not written by hand.** The
  language-server, check and formatter showcases in `docs/src/pages/index.astro` hold generated
  markup in the frontmatter (`ideCode`, `analysisCode`, `analysisOutput`, `formatterCode`): hovers,
  completions, references, rename edits and inlay hints come from driving `roughly server` over
  LSP, and the diagnostics and diffs from `roughly check` / `roughly fmt --diff`. Columns, caret
  widths and gutter widths are the ones the tools report, so **re-capture rather than hand-edit** —
  an edited sample silently becomes a fake screenshot of a real product. Astro strips
  whitespace-only element content, so that markup must be rendered through `set:html`.

## Mid-term

- **User direction (standing): drive autonomously toward best-possible shape; empty `backlog.md` §Open; work continuously.** `backlog.md` holds the open work (semantics opportunistic items, editor polish, structure/perf levers, website) and the one-line shipped ledger.
- **The type system admits only what is fast to check, which means Hindley-Milner (user directive).** Declaration files (`.Rtypes`) carry the one sanctioned exception, today ad-hoc overloading; a user's `#:` annotation is pure HM and declares exactly one signature. Traits/typeclasses are **declined, not deferred** — they are the textbook-correct way to get ad-hoc polymorphism into HM, which is precisely why they are not the bar here. General subtyping is out for the same reason; a declared coercion at a named boundary is the HM-compatible shape any variance work must take. Full record in `decisions.md`; evaluate every proposed type-system feature against it before designing.
- **Open type-system design questions** live in `typing-design.md`: tags/discriminated-union `match` (post-beta, user direction), S3 dispatch, data.frame/matrix modeling, variadic `...` body semantics, NAMESPACE/import model. (Traits are closed — see above.)
- Audit habit: treat "landed" claims in old notes with suspicion; the verified state is `backlog.md` + the fixture suites.

## Long-term

### Documentation

- **The docs site is organised by purpose, not by component**, in five sidebar groups: Introduction
  (getting-started, why-roughly, features), Type checking (tutorial, concepts, domain-modeling, stubs,
  limitations), Guides (adopting, continuous-integration, r-console), Reference (configuration, cli,
  diagnostic-codes, formatting-rules, type-system), Contributing. The installation page is deliberately
  absent from the sidebar — getting-started closes with the extension links and a one-line install, and
  the page itself only covers awkward cases. Introduction pages live at the site root; every other page
  nests under its group.
- **The rule that keeps pages from bleeding into each other**: explain a thing once, in prose, where it
  is introduced; tabulate it once, in Reference; never half-explain it in a third place. The previous
  layout documented suppression in three pages and configuration in three more. When a page needs a
  fact it does not own, it links.
- **Terminology is fixed, no synonyms**: finding (not issue/problem), diagnostic code (not rule/lint
  code), annotation (not type hint), *check* for the command and *code analysis* for the capability
  (never "linting" for either), the language server (not "the LSP"), nominal type, project (not
  workspace).
- **Breadcrumbs come from the sidebar, not the URL** — `docs/src/components/PageTitle.astro` walks
  `starlightRoute.sidebar` for the entry marked current. It exists because both Starlight and
  starlight-theme-black render a hardcoded two-level `Docs > title`. Declaring `components.PageTitle` in
  `astro.config.ts` makes the theme skip its own override and log a warning; that warning is expected.
- **`reference/formatting-rules.md` is generated**, not authored — `crates/format/tests/test_format_docs.rs`
  runs every example in `crates/format/tests/formatter.template.md` through the formatter and writes the
  page. Edit the template and rebless with `ROUGHLY_BLESS=1`; editing the page directly is always wrong,
  and a site-wide link rewrite must include the template or the generated page silently keeps stale links.
- **Docs claims are verified against the binary, never against other docs.** A prior pass shipped a
  fabricated JSON example and formatter behaviour the tool does not have; the rewrite caught a
  fabricated `--output json` sample still in the install page. Run examples before writing them down.

### Architecture

- **The product stack** is a hand-written lexer/parser emitting lossless rowan trees (`crates/syntax`; `#:` annotations are first-class grammar) under a salsa-based analysis core (`crates/semantics`: file input → parse → item tree with insertion-stable identity → per-item green subtrees → per-item HIR/naming → HM inference → interned types → diagnostics), with `crates/ide` as pure reads, `crates/format` on syntax only, and `crates/roughly` as the LSP server + CLI. The contract is `architecture.md` (read it before touching the engine or scheduling) and `structure.md` (file layout). Position independence: per-item derived values carry item-relative ranges; `semantics::item_node` is the ONLY re-anchoring edge.
- **Cyclic package interfaces resolve through one canonical fixpoint**, never through whichever query arrived first: `interface_sccs` (static item→winner reference graph, iterative Tarjan, canonical project order) feeds `scc_schemes` (Jacobi rounds from all-Unknown; convergence by table equality; the round cap pins ALL members), and `item_check` adopts the canonical scheme as the single exported truth. Member checks inside the fixpoint run `check_item_with_annotation` directly — never through `item_check` — so no salsa cycle forms; salsa cycle recovery stays only as a backstop for edges the static graph cannot see. Converging recursion types precisely; the remainder is attributed under strict mode (decisions.md).
- **The server** runs one async-lsp frontend thread + one worker owning the db. Latest-edit-wins rides salsa's cancellation token: edits flip the token; every subsequent job starts on a fresh storage-handle clone (`refresh_cancellation`). The two-wave publish gates on a real query split (`parse_stage_diagnostics` never computes naming/typing). Host assembly (config gating, typing modes, suppressions, lints) is shared between server and CLI in `crates/roughly/src/diagnostics.rs` so the surfaces cannot drift.
- **Correctness rests on the new stack's own fixture suites** — they are the semantics contract (the identity-parity program against the frozen legacy oracle is complete and retired by user decision; `legacy/differential` is benchmark-only). Fuzzing is pipeline-wide from each stage's first commit (decisions.md).

### Quality bar (the acceptance contract future work is held to)

- **Performance:** keystroke-to-diagnostics p50 ≤ 30 ms / p95 ≤ 100 ms at 300k LoC (always read latency numbers against the raw-parse floor the instrument prints; machine load swings them ~1.4×); budgets pinned by `stats_witness`; instruments in `legacy/differential/tests/test_stats.rs`.
- **Soundness:** the type relation is pure (probe-then-rollback, order-independent given program order); nominal variance is single and correct; resolve errors are never swallowed; recursion is guarded everywhere; UTF-16 position handling is correct including non-BMP; IDE features never panic on stale ranges.

### Invariants and traps (new stack — a well-meaning refactor silently breaks these)

- **Salsa:** hot tracked queries returning maps/vecs use `returns(ref)` — `returns(clone)` deep-clones per call (measured ~8s of corpus render). Input setters need the `salsa::Setter` trait in scope. Never run the stats instruments batched in one process — resident/peak RSS numbers pollute each other; the protocol is one process per instrument.
- **Formatter (rowan attachment differs from tree-sitter's):** trailing comments attach INSIDE expression nodes — every closer-placement / after-comment decision must be TOKEN-level (`last_token_before`), never element-level; line math uses `significant_range` (some nodes swallow trailing trivia); `]]` closes with two `]` tokens and the FIRST is the placement anchor; `# fmt: skip` recognizes three comment attachments. The formatter refuses only on R-grammar errors — annotation-grammar errors carry `SyntaxError.in_annotation` and the block renders verbatim; never re-derive that split from positions. Raw strings (`r"(...)"`) reproduce byte-for-byte.
- **Parser error doctrine:** a lexer `ERROR_TOKEN` is already precisely diagnosed — the parser consumes it silently as a placeholder atom (`primary()`), and a statement that contained one suppresses its own boundary report (`statement_had_lexer_error`); one mistake must never fan out into a report storm (the fuzz harness pins errors ≤ linear in tokens, plus non-empty messages and in-bounds ranges). The golden error suite (`crates/syntax/tests/errors`, by area) covers every distinct message template; when adding a parser error path, add its case in the same change.
- **Fixtures:** consecutive `#:` lines STITCH into one annotation region, and mixing `@type` with a compact form invalidates the whole block — separate regions need a blank line. The `group__case` id is the test identity; ids must be unique per suite.
- **IDE position model:** cursor → smallest end-INCLUSIVE non-empty HIR expression with NameRef tie-win; `position_in_item` tries strict containment then item-end-touch. Directive-name readers must join only TOUCHING tokens (`@if-unknown`), or every name gains a trailing space and matches nothing.
- **Corpus instruments:** the only cross-stack instrument left is the benchmark (`legacy/differential/tests/test_stats.rs`); the comparison arms went with the identity-parity retirement. The acceptance gate for the parser IS hard (zero unadjudicated divergences vs tree-sitter, R as referee).
- **Windows-gnu cross-link needs a synthesized `synchronization` import lib:** reedline → crossterm → winapi links `-lsynchronization` (the `WaitOnAddress` API set); cargo-zigbuild sets `WINAPI_NO_BUNDLED_LIBRARIES=1` (winapi's bundled import libs break zig's lld) and zig's bundled mingw-w64 ships no `synchronization.def`, so the flake's windows `preBuild` generates the import lib with `zig dlltool` and adds it via `RUSTFLAGS -L` (see `flake.nix`). A crane deps-only build won't catch a breakage here — the dummy sources never reference reedline, so the winapi `-l` flags reach only the real final link.

### Typing — durable implementation rationale (the spec is `reference/type-system.md`; this is the *why*)

- **HM with union-find variables and an undo-log environment:** all writes funnel through the undo-log so branches, loop passes, and function bodies roll back without cloning the environment; loops iterate to a fixed point (cap 3, widen-to-Unknown, diagnostics only on the stable pass). `<T>` annotation binders are rigid skolems: they refuse to bind while their scope checks, then generalize back out.
- **Unions normalize in exactly one place** (`types` union construction: flatten, dedupe keeping first, `NULL` last, `Any`/`Unknown` absorb, singleton unwraps). Never build a `Union` literally. Unification is the invariant floor (set equality + the single nullable member-wise case); all directional member-wise logic lives in compatibility. No union constraint is ever imposed on an inference variable (HM-speed guardrail).
- **Exported schemes close at the export edge** (`erase_residual_vars`): substitute bound vars, erase unbound to Unknown, follow var bindings only — a raw inference var escaping into a foreign table is a crash class the fuzzer caught.
- **The capture model mirrors R's environments:** naming pre-mints frame assignment targets so closures resolve later-written names (an unassignable slot does not shadow — lookup falls outward), and the checker re-checks a body ONCE when a captured-write join grows after being read.
- **One `TypeRenderer` instance must span everything that shares names** (a whole signature, both sides of an expected/found pair) — a fresh renderer restarts the `T`/`U`/`V` numbering and collapses distinct variables.
- **Every finding inside an item is reported; only the item's *export* is cut.** A failed item exports `Unknown` so downstream items do not check against an untrustworthy shape, but the findings inside it are all reported: a failing expression records `Unknown`, which is compatible with everything, so later checks read a poisoned value as an absent fact instead of cascading off it. The report dedups per site and kind, because speculative paths (overload probing, guard edges) can record the same failure twice. Truncating to the first finding — which this once did — makes a green run meaningless, since most code lives inside function bodies; three independent adoption reviews called it a blocker.

### Error handling

- **Coherence failures panic.** Document-sync/analysis-sync failures in the LSP path are unrecoverable — `panic!`, never best-effort logging over corrupted state. But IDE feature lookups must never panic: a not-yet-bound local falls back to `Unknown`.

### Conventions / traps

- **Verify with CI's toolchain, not the container default.** CI uses the *latest* stable; a stale container toolchain has an older clippy (misses newer lints) and a rustfmt that wraps differently — `rustup update stable` first. Check for a formatting *diff*, not just `cargo fmt`'s exit code (piping through `head` masks the exit via SIGPIPE). The `rustfmt.toml` `imports_granularity`/`group_imports = One` rules are nightly-only and skipped (with warnings) on stable.
- CI gates exclude `rofy` (needs local R) and `zed_roughly` (wasm toolchain) — verify with `--workspace --exclude rofy --exclude zed_roughly`.
- Standard-library stubs are **declaration-only `.Rtypes` files** in the top-level `types/` directory — each line `name : <type-expr>` reusing the `#:` annotation grammar (no second type parser; loader in `semantics/src/stubs.rs`). Project overrides in `<project>/stubs/*.Rtypes` fold on top (project wins). The assembled library is a **set-once singleton input** — never route stub files through document sync. Repeating a name in one source declares an ordered overload set (end every set with an `Any` fallback — the unresolved-argument path selects the last candidate); `@type NAME` declares an opaque stub nominal.
- `[check] typing` defaults **off** — CLI probes need `[check]\ntyping = true` in a `roughly.toml`, or probe through the fixture harness. `[check] unused` defaults ON (user directive).
- `docs/…/formatter.md` is **generated** from `crates/format/tests/formatter.template.md` via `cargo test -p format --test test_format_docs` (`ROUGHLY_BLESS=1` rewrites); never edit the output file.
- **Legacy-only traps live with the frozen stack** (consult only when touching it): tree-sitter node/field ids are grammar-version-pinned integer constants; tree-sitter parses `#:` annotations as opaque comments; its fixture runner resolves a relative stub `base_path` against the test CWD. Do not extend legacy; data files may be duplicated across stacks, code never.
- **UI design guideline:** no `border-l` left-accent rail on rounded elements (generic-AI look) — use a full ring, fill, underline, or weight change instead.
