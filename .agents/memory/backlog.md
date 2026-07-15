# Backlog

**Standing goal (user mandate): empty this list and keep the project at rust-analyzer quality.** The beta program that once organized it is complete; shipped work lives as one-line ledger entries at the bottom (rationale in `decisions.md`, contracts in the docs). Every open item sits in one of the sections below.

**Quality bar (acceptance):**
- **Sound on idiomatic R:** no known accepts-then-crashes holes on supported constructs; unsupported constructs may be refused loudly (sound-by-refusal is acceptable) but never silently mistyped.
- **Zero false positives on the ~200 most-used base functions** with `[check] typing = true` on idiomatic call forms.
- **Performance:** keystroke-to-diagnostics p50 ≤ 30 ms / p95 ≤ 100 ms at 300k LoC; completion p95 ≤ 50 ms; budgets pinned by committed counter witnesses (`crates/engine/tests/test_benchmark.rs`), wall-clock tables on demand (`benchmark_ide_read_latency`, `roughly debug analysis-stats`).
- **No server-killing input** (no `unwrap` panics on protocol-legal messages).

## Open — semantics

- **Recursion strict attribution:** recursion is now typed everywhere it soundly can be (local letrec; top-level mutual groups generalize together — decisions.md), and top-level SELF-recursion deliberately keeps the tolerant `Unknown` (tree-fold shapes need recursive types). Open: strict mode records no origin on those deliberately-`Unknown` self-recursive schemes — attribution needs an origin on the binding, identically in both pipelines (differential must stay byte-exact).
- **Design forks, decide-and-implement (one `decisions.md` entry each):** third constraint kind (two-flexible-operand comparison — the recorded traits tripwire); order-dependent compatibility commits. (NAMESPACE bare-resolution gating: DECIDED — stays ungated; the shipped corpus is R's default-attached search path, gating falls out of the post-beta import model; decisions.md.)
- Overload candidates when touched: `is`, `extends`, `grep(value =)`, `cor` (vector vs matrix — needs matrix nominals). `Date`/`POSIXct` arithmetic refuses loudly today — revisit if real code makes it noisy.

## Open — editor & polish

- Hover type fences (user-confirmed: no highlighting in current editor builds): the server tags the fences `roughly-type` and the VS Code extension in-repo ships a grammar for that id — needs a released extension update to reach users. Zed renders the fence plain until its extension registers an equivalent fence language (tree-sitter grammar required); consider falling back to tagging fences `r` for Zed if that proves distant.
- Callback-idiom stub sweep: declare optional formals on single-parameter-capped stubs when touched (`nchar`-style).

## Open — structure & performance

- Engine/analysis dedupe, case-by-case: the surface-type resolvers ×3 and the SCC fixed point are duplicated by design (the engine variants read per-symbol queries to keep dependencies narrow) — dedupe only with an abstraction that preserves that. Shared rope/tree helpers between `analysis` and `roughly` are plain debt.
- A keystroke in a huge file still pays whole-file re-inference: one authoritative `Typecheck` (now shared with the exports — decisions.md "one inference per file per revision"), **plus 2+ more whole-file rounds per demanded interface SCC through that file** (`resolve_interface_scc` re-infers whole member files per fixed-point round; every top-level self-recursive function is a singleton self-edge SCC). Going below whole-file granularity needs partial/per-definition re-inference — a design, not a tweak; it would also make SCC rounds infer only member definitions. Key soundness constraints discovered: letrec-member and monotype (non-function) exports depend on the whole file; `top_level_capture_repass` forces two whole-file passes; recorded expression types resolve against the final substitution.
- Cold-pass CPU is now parse-bound: `lower (+parse)` is ~2.5s of a 3.7s cold pass at 302K LoC (tree-sitter parse dominates). Levers if it matters: parallel parse outside the single-threaded engine, or a faster parser (see the parser question, post-beta).
- First workspace-symbols query after a cold start re-parses candidate files on demand (trees are no longer resident; per-file symbol items cache across requests). If it's felt, derive the outline from the resident `Module` instead of the tree.
- Smaller memory levers if the resident set matters again: `Module.span_index` is derived state built eagerly per module (~30 MiB at 302K LoC; only open files need it — a lazy build would drop it for the corpus); `Range` in HIR nodes is 48 bytes (tree-sitter shape) and could pack to u32-based ~24.
- Smaller alloc-churn levers if churn matters again: `parameter_variances` recomputed per nominal compat check; per-node `String` in formatter output assembly; lowering allocates per-node text (`node_text` `String`s) — visible in cold-pass profiles now that inference is fast.
- CI: the widened whole-workspace workflow is staged in `.github/pending-ci.yml` — a human must `git mv` it (automated tokens lack workflow scope). Authoritative perf numbers need the CI runner.

## Open — website & docs

- Landing page (user-owned hero animation — do not touch): "IDE features in your editor" tabs look bad + layout shift on click (reserve dimensions); formatting-section examples not distinct enough.
- Full docs-site accuracy pass once the remaining semantics land.

## Post-beta (explicitly out of scope for now)

- Tags / discriminated unions via a compiler-known stdlib `match` (design in `typing-design.md` first).
- S3 dispatch modeling (`UseMethod`) — prerequisite for honest `print`/`summary`/`plot`.
- data.frame column-level typing; matrix dimensionality; real S4 typing.
- Traits/typeclasses (tripwire: the third constraint kind).
- NAMESPACE-aware imports (`import()`/`importFrom()` checking), CRAN stub auto-generation via R introspection, R-version-keyed corpora, stubtest validation (R-dependent).
- Parser question (user, `NOTES.md`): hand-rolled recursive descent vs tree-sitter — revisit post-beta; parsing is not a bottleneck (recommendation recorded).

## Shipped ledger (one line each; rationale in `decisions.md`, contracts in the docs site)

- **Checker constant factors (~4× whole-file inference):** allocation-free free-variable walker (no more resolve-clone per recursion level), dense `EntryTable` vector for the union-find, Tarjan letrec-membership (was per-candidate transitive walks), precomputed winner-test lookups + batch exported bindings, FxHash for the never-iterated state maps; per-file interface edges (`WalkShadowed`/`FileInterfaceEdges`) projected per symbol (hub sweep 763→33ms).
- **One inference per file per revision:** `Typecheck` owns `FileInference { check, exports }`; `ExportedSchemes` is a shared-pointer projection (still the value-eq firewall); the could-not-resolve typo hint is memoized per symbol with an allocation-free corpus scan; the letrec candidate-edge scan is one arena pass; `Diagnostics` fetches tree-readers adjacently (one parse per file on cold prime). Cold 10.1s → 3.7s at 302K LoC, keystroke 5.4 → 1.7ms; `analysis-stats` splits lint / package-naming / render stages, and a `profiling` cargo profile (release + symbols) supports sampling.
- **Interface routing:** same-file backward references are walk-resolved, never interface-imported (kills the fake same-file SCC/Tarjan blowup — 100× on chain files; counter witness); scripts overlay their declarations on the memoized package type environment (was O(scripts × package files)).
- **Semantics core:** multi-member unions; mutable-slot model with union joins; `<<-`/`->`/replacement forms; coercion policy; name-aware signature matching; `Unknown`/`Any` tolerance in `c`/`for`/`$`/`[[`/`[`; flow-sensitive guard narrowing (+ divergence-aware joins, `missing()` supplied-state); `is.null` shaping of unconstrained variables (unannotated coalesce); elided annotation returns; `...` as positioned rest parameter end-to-end; variadic bridging into callbacks; `switch`/`return` as checked control flow; dispatch-table `[[` unions; computed-key container refinement; positional `[[` record extraction; S4 `@` slot lowering.
- **Stub unlock:** `T[]` constrained generics; overload sets (probe-committed, two-round selection, signature-help/hover display); opaque `@type` nominals (data.frame/factor/matrix/…); named-into-rest absorption (typed `read.csv`, `lm`); ~530-declaration corpus across 6 namespaces; project-stub namespaces (`pkg::name`); NAMESPACE import validation + `unused-import` lint; `library()`/`require()` NSE quoting; stub-error surfacing on every surface.
- **Trust & UX:** config subsystem rebuild (nearest-ancestor discovery, per-lint severity, config-file diagnostics, reload refresh); per-file typing directives `# typing: on|off|strict` (tri-state `TypingMode`, one gate everywhere); data-masked NSE resolution (data.table brackets, with-family); strict-mode product story; unified lint framework; CLI rendering + exit-code contract; `roughly debug analysis-stats` workspace performance diagnosis.
- **Editor:** hover quality (`name : TYPE`, overload notes, constraint display); annotation cursor features via re-lexing (hover/goto/completion in `#:` comments); insert-annotation code action (round-trips); unused fade-outs; formatter rewrite with `#:` block awareness; letrec naming (local recursive closures resolve).
- **Engine & scheduling:** red-green core with per-symbol interface firewalls, SCC fixed point, tombstones, eviction, stacker-grown validation spine; durability tiers (open docs LOW, corpus HIGH; sound downgrade re-min through cutoff nodes); memoized completion index + `NamesGlobal`-valued symbol index (zero-copy reads); two-wave diagnostics publish + idle-time semantic wave + lossless preemption pairing + background prime; error-tolerant lowering ("a broken region reports its syntax error and nothing else"); differential correctness vs from-scratch oracle over adversarial edit streams, byte-exact, IDE features included; committed latency witnesses (at-rest reads ≤ 32 memos, size-independent post-keystroke walk, blast-radius exec counters); memory shape at scale (rope-only corpus inputs + on-demand trees, single-retained modules, boxed annotations: 1 GiB → ~300 MiB at 302K LoC) and O(open) keystroke validation (fold split over the `OpenFiles` seam, FxHash memo table: 11K → ~280 slots/keystroke); `analysis-stats` reports per-phase memory, typing-burst recompute counts, and walk attribution.
- **Docs:** getting-started leads with a real bug; installation split out; typing guide + reference as contracts; architecture/structure/testing contributor pages; linter/configuration/stdlib-stubs pages current.
