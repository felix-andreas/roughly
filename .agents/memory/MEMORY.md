# Memory

Cross-session knowledge base for the agents building Roughly. Three horizons:

- **Short-term** — current focus and loose ends. Prune aggressively; delete each item once it is resolved or obvious from the tree.
- **Mid-term** — active priorities, open bugs, and technical debt. Lives across sessions until done.
- **Long-term** — durable, non-obvious design decisions and their rationale. Only things a future agent would otherwise rediscover. Keep terse and point at code or the docs.

Authoritative specs live in the docs site (`docs/src/content/docs/`), not here: `typing-reference.md` (the typing contract) + `type-checker.md` (the guide), `architecture.md`, `structure.md`, `testing.md`. Keep those current; keep this file for state and rationale.

## Short-term

- **M1 (audit P0/quick-wins) landed** on `feat/hm-type-checker` as 8 green commits (`abf6f36`..`60b4096`). Each: own commit, `cargo test -p analysis` + `-p roughly` green, `cargo check --all-targets` clean. Items:
  - span→ExpressionId index (`Module.span_index`, O(arena)→O(log n)) + `HirArena::try_get` non-panicking IDE access.
  - UTF-16 positionEncoding negotiation (prefer utf-8, else utf-16); internal column = UTF-8 byte offset (tree-sitter Point); single conversion seam in `roughly/src/position.rs`; symbol/diagnostic paths routed through it (Item now carries byte-column `TextRange`).
  - resolve-error propagation in `check_compatibility` (no more `unwrap_or(Unknown)`); recursion-depth guards (inference `RECURSION_LIMIT=128`; type-syntax parser `TYPE_SYNTAX_RECURSION_LIMIT=160`; separate lowering guard for alias-body expansion). Native annotation overflow ≈200; ordering 128<141<160<200.
  - references/rename text prefilter (no persistent index) — find-refs@100k 218ms→67ms; provably equivalent (interner bijection over the same byte range).
  - completion cap 128 + `isIncomplete` (`CompletionResult`); inlay-hint viewport filter.
  - **`ROUGHLY_BLESS=1` auto-bless** for the fixture harness (rewrites `#++++` block bodies in place; idempotent on the real suite; leaves `#++++ any` alone). Use it for intentional expectation changes. Documented in `testing.md`.
- Known wording quirk (pre-existing): the parser recursion diagnostic renders as `error[syntax-error] Syntax Error: ...` (doubled prefix) — same as other annotation-syntax errors; tracked under the diagnostic-wording mid-term item.
- Steering docs are agents-first: single `.agents/memory/MEMORY.md`; authoritative specs in the docs site; human notes in `.local/` (untracked).

## Mid-term

Active priorities and debt:

- **Incremental recheck after an edit still scales with package size.** `typecheck` short-circuits on an unchanged package version (repeated IDE calls are O(1)), but an edit still pays package-scoped work: `resolve_package` rebuilds package naming, the interface fixed-point scans every document to compare its dependency fingerprint (O(documents)), and the global env fingerprint is rendered over all globals. ~13ms@10k, ~0.4s@100k (`just bench`). Bounded next steps: a reverse-dependency index (name → referencing documents) and incremental package naming. The full near-constant model is the incremental redesign — design with the user first and record it on the docs architecture page.
- **Find-references / rename / completion: M1 cheap wins landed; structural reuse still pending.** A text prefilter cut find-refs@100k 218ms→67ms (commit `31be96d`) and completion is now capped at 128 + `isIncomplete` (`b93ba77`). Remaining cost is the per-document tree walk across all docs — the deeper fix is the **reverse-dependency index** (package-global Symbol → referring DocumentIds) feeding both refs/rename and the interface fixed-point. That index is the M-series keystone (also unblocks per-doc round-2 typecheck keying). goto-definition is already O(1)-file.
- **Type-syntax error ranges are coarse.** `SurfaceType` carries no per-node ranges, so unresolved-type / annotation errors underline the whole annotation. Thread ranges through type-syntax parsing so `list{age: intgr}` underlines only `intgr`.
- **Tree-sitter access is string-based in the `analysis` front end** (`kind()`/`child_by_field_name()`), while `roughly` uses `kind_id()`/`field_id()`. Consolidate on id-based matching; dedupe the rope/tree helpers and the AST-walking symbol indexer shared with `roughly` (`roughly/src/index.rs`).
- `resolve_document` public phase entry + edit-time orchestration.
- `typecheck/project` follow-ups: package-winner behavior with conflicting types; `Collate` coverage once the fixture harness models `DESCRIPTION`.

Open typing soundness / quality gaps:

- **UNSOUND (remaining):** generic nominal type arguments are checked covariantly regardless of variance. `Handler<integer>` is accepted where `Handler<integer | NULL>` is expected even when `T` occurs only in a contravariant (function-parameter) position, so a `NULL` can flow into a `fn(integer)`. Fix: compute each type parameter's variance from where it occurs in the representation and check each argument in its variance (`check_compatibility` Nominal-vs-Nominal arm). Rare pattern; do not rush.
- **S4:** find-references / rename for S4 names need a use-site index; `@` slot access still lowers to `Unsupported` (needs a `Slot` HIR node + lowering/typing); slots-as-class-children in the outline.
- **Structural constraints on inferred params:** `function(x) x$name` / `x[[1L]]` / `c(x, 1L)` leak `type1` or error instead of constraining `x` to a record / indexable / atomic. Needs row/shape constraints on inference variables, analogous to the numeric constraint.
- `T` / `F` base bindings (need a base-environment model); vectorized `&` / `|` (need a semantics decision).
- Diagnostic wording: alias-cycle not reported on unused declarations; `@if-unknown`-on-known wording; annotation-semantic errors render under `syntax-error` with a doubled `Syntax Error:` prefix.

## Long-term

Durable design decisions and non-obvious facts (point to code/docs; do not re-derive):

- **Incremental at document grain (two-round interface model).** Per-document local naming, then a package-level dependency-cached interface fixed-point. Generalization is level-based. Cross-file references are scheme-based — no inference flow across files. Each document's interface is cached on a dependency fingerprint of the schemes it references; the fixed-point only re-derives changed documents (but still scans all to compare — see mid-term). Authoritative detail on the docs architecture page.
- **Typing core.** HM inference with union-find inference variables. Numeric constraint (`types::Constraint`) carried on `Unbound` entries and quantified in `TypeScheme`; `function(x) x + 1L` → `<T: numeric> fn(x: T) -> T`. A numeric var that escapes a binding without being abstracted by a function parameter defaults to `double`; defaulting is level-gated (`default_free_numeric`) so a local inside a polymorphic function does not monomorphize the enclosing param.
- **Polymorphic annotations are enforced via rigid (skolem) variables.** A `<T>` binder lowers to a rigid var (`rigid_variables` map in `InferenceState`, `fresh_rigid_variable`) that refuses to be bound or constrained while the body is checked, then generalizes back to `<T>`; instantiation at call sites uses ordinary fresh vars. Checks in `bind_variable` / `unify_variables` / `constrain_type`; rendered by declared name via `display_with_rigid_names`. Function-annotation checking is unified in `infer_function_expression` (return focused-checked for a clean message, then whole-signature for parameter shape).
- **Compatibility rules.** Structural (record/tuple) compatibility is checked covariantly per element/field and unifies variables (lets `@new` / checked annotations infer through inference-variable fields). Function types are contravariant in parameters, covariant in returns. Return position is checked with compatibility, not equality. if/else unifies its branches (`NULL` → nullable, `Unknown` → `Unknown`, otherwise unify).
- **Type-error surfacing is decoupled from the phase.** The typecheck phase runs lazily for the typing IDE features (hover types, inlay hints, signature help) — all on by default. Type-error *diagnostics* are gated behind `[check] typing = true` (default off): `document_diagnostics` and proactive `run_full` only include/run them when set. `Analysis::type_errors_enabled()`. `[check] unused = true` gates unused-local warnings the same way.
- **Search-like IDE features share `ide::search_match`:** subsequence matching, smart-case (any uppercase in the query ⇒ case-sensitive), queries shorter than 3 chars are prefix-only, ranked by `MatchScore` (exact/prefix/substring/subsequence tier, then first-match position). Workspace symbols collect → rank → truncate(128); completion reuses it; reserved keywords stay prefix-only.
- **S4 navigation is unified into `ide::symbol_occurrences_at`.** Identifier resolution first; when the cursor is on a string literal, a structural S4 scan (`s4_symbol_at` / `collect_s4_occurrences`) resolves class/generic names in `setClass`/`setGeneric`/`setMethod`/`new`. goto-def, find-references, and rename all work cross-file through this one path.
- **Unused-local detection.** Naming tags each binding with a `BindingKind`; `unused_bindings` are local assignments no symbol use resolves to. Excludes parameters, for-loop variables, top-level/exported bindings, and `.`/`_`-prefixed names.
- **Index/field-access diagnostics are dedicated** (`field b does not exist`, `position N does not exist`, `expected a list`, `cannot index … without a statically known position/field name`, `[ is not supported on …`) with whole-access ranges.
- **Error handling: coherence failures panic.** LSP document-sync / analysis-sync failures are unrecoverable and panic immediately. But IDE feature lookups must never panic — the typecheck Symbol arm falls back to `Unknown` for a not-yet-bound local (forward / recursive / conditionally-defined / inside an uninferred subtree).
- **Graduated, always-on:** hover, find-references, rename, plus the typing IDE features above. Document symbols intentionally stay AST-based in `roughly` for the per-keystroke path.
- **Benchmarks.** `just bench` builds synthetic 10k/100k/200k packages (`crates/analysis/tests/common/mod.rs`, `test_benchmark.rs`): cold full check, single-file recheck, and IDE-feature latency (`benchmark_ide_100k/200k`).
