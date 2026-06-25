# Memory

This document stores cross-session context.

Keep this file compact and aggressively pruned. It should preserve only high-value continuity that is likely to matter in a later session.

<!-- Do not remove this purpose section unless the user explicitly asks for it. New items added here should also be preserved unless the user explicitly asks to remove or rewrite them. -->
Its purpose is to preserve important implementation state, open questions, and loose ends between agentic sessions, especially when the active context window is too small to carry the full design and implementation history forward.

Use this document to record:

- current implementation status
- unresolved design questions
- diagnostic quality goals
- known technical debt
- next recommended steps
- any subtle decisions that should not be rediscovered from scratch

If code changes make this document inaccurate, update it in the same session.

## Active continuity

- Type-checker soundness audit (adversarial workflow) ran; 15 confirmed findings, 7 clusters fixed
  this session (return-position compatibility, if/else branch unification + Unknown propagation,
  `x[[2]]` double-literal position, `&&`/`||` nominal projection, range `:` → `double[]` for numeric
  vars, script-local `@type` resolution in naming). Two remain, documented in `projects/008` as
  needing type-system work: polymorphic function annotations not enforced against the body (needs
  skolemization / rigid type variables) and generic nominal type arguments checked covariantly
  regardless of variance (needs per-parameter variance computation). Both unsound; do not rush.

- Unused-local-variable detection landed and graduated from experimental: naming tags bindings with
  `BindingKind` and computes `unused_bindings` (local assignments no symbol use resolves to); gated
  warnings in `document_diagnostics` when `check.unused`. Enable via `[check] unused = true`. Fixtures
  in `tests/unused/`. Excludes params, for-vars, top-level/exported, `.`/`_` names.
- S4 navigation is unified into `ide::symbol_occurrences_at`: when the cursor is not on an identifier,
  a structural S4 scan (`s4_symbol_at`/`collect_s4_occurrences` in `analysis::ide`) resolves
  class/generic names in `setClass`/`setGeneric`/`setMethod`/`new`. goto-def, references, and rename
  all work cross-file through one path. `@` slot access still `Unsupported` (see `projects/008`).
- Goto-definition is now O(1) file: `symbol_occurrences_at` takes an `OccurrenceScope`; definition
  scans only the declaring document. `just bench` includes `benchmark_ide_100k/200k`
  (goto-def ~2ms, find-references ~240ms, completion 97ms/20k items — last two are documented debt).
- Workspace symbols + completion now share `ide::search_match` (subsequence matching, smart-case,
  <3-char queries are prefix-only, ranked by `MatchScore`). Workspace search collects-then-ranks-then
  -truncates(128). Keywords stay prefix-only. Spec in DECISION_LOG. Unit tests in ide.rs
  (`search_match_tests`) and symbols.rs (`workspace_tests`).
- Index/field-access diagnostics are now dedicated (`field does not exist`, `position N does not
  exist`, `expected a list`, `cannot index … without a statically known position/field name`, `[ is
  not supported on …`) with whole-access ranges.

- Fixed a production LSP panic: `local binding BindingId(N) should be prebound for typecheck`
  (typecheck.rs Symbol arm). Root cause: naming creates a local binding for an assignment that
  typecheck never visits (e.g. an assignment inside a call argument typecheck doesn't infer, like
  `switch(x, a = (k <- 1L))`, or any Unsupported subtree), so `lookup_local_name` returned `None`
  and panicked. Fix: the Symbol arm now falls back to `CoreType::Unknown` for any not-yet-bound local
  (forward/recursive/conditionally-defined/uninferred) instead of panicking — IDE requests must never
  crash. Regression: `audit_errors_probe__unbound_local_in_uninferred_call_arg_no_panic`. The two
  former local branches (definite vs maybe-undefined) were merged; `is_maybe_undefined_expression` is
  no longer used in typecheck.

- Type checker capability landed (all in `crates/analysis/src/`, fixture-backed):
  - Numeric constraint on inference variables (`types::Constraint`, carried on `Unbound` entries and
    quantified in `TypeScheme`). `function(x) x + 1L` → `<T: numeric> fn(x: T) -> T`; calling with a
    non-numeric arg errors at the call site; a numeric var that escapes a binding without being
    abstracted by a function parameter defaults to `double`. Defaulting is level-gated in
    `default_free_numeric` so a local binding inside a polymorphic function does not monomorphize the
    enclosing param.
  - Typed expression results are retained: round-2 `InferenceState` records each expression's type;
    `Analysis::checked_expression_type` exposes it. Hover is human-readable by default (type +
    variable definition/scope as unnamed blocks; phase dumps only under a `### Debug` heading when
    debug is on; `HoverInfo` = `contents` + `debug`). `ide::inlay_hints` emits inferred-type hints for
    unannotated bindings (concrete types only), and `ide::signature_help` shows the called function's
    signature with the active parameter. All wired into the LSP server, capabilities gated on typing.
  - Expression-level annotations (e.g. `#: @new User` on a block's final expression) are applied in
    the inference wrapper, not only on assignments.
  - The package interface settles by a dependency-cached package-level fixed-point, so re-exports and
    forward references resolve within AND across files (`second <- first`, `get_base <- function()
    base`, deep cross-file chains). `typecheck` short-circuits when the package version is unchanged
    (repeated IDE calls are O(1)). Recheck-after-edit cost rose modestly; a reverse-dependency index
    is the next bounded perf step (see TECHNICAL_DEBT).
  - Record types reject duplicate field names and allow a trailing comma.
  - Function compatibility is contravariant in parameters, covariant in returns.
  - Unresolved type names are naming-owned diagnostics (`Diagnostic::naming_error`, "I could not
    resolve type ...").
  - Parameter defaults are lowered/named/typechecked (`hir::Parameter.default: Option<ExpressionId>`);
    `NULL` default is the optional sentinel (always allowed); annotated non-`NULL` defaults must be
    compatible; unannotated defaults do not pin the param type.
- Incremental at document grain (two-round interface model; `ARCHITECTURE.md` "Incremental model").
  Generalization is level-based. `just bench` generates synthetic 10k/100k/200k packages
  (`tests/common/mod.rs`, `tests/test_benchmark.rs`); cold full check ~157ms / 2.9s / 10.7s and
  single-file recheck ~13ms / 291ms / 1.8s — recheck still scales ~linearly (fingerprint rendering +
  package naming are O(package); see TECHNICAL_DEBT).
- Cross-file references are scheme-based; no inference flow across files. Scripts have a sequential
  top level and are typechecked. `analysis::typecheck` returns recomputed document ids.
- Editor features (hover w/ typed section, completion, rename, goto-definition, references, inlay
  hints) live in `analysis::ide`; document symbols intentionally stay AST-based in `roughly`.
- Docs: user guide at `docs/src/content/docs/type-checker.md`; high-level `human-overview.html` at
  repo root; README features updated.
- Remaining big items: near-constant incremental recheck *after an edit* (needs the incremental
  package-naming + interface-version redesign per `AGENTS.md`, design with user first); precise
  type-syntax error ranges (`SurfaceType` carries no per-node ranges, so naming underlines the whole
  annotation — an invasive change threading ranges through type parsing).
