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
- Docs: user guide at `docs/src/content/docs/type-checking.md`; high-level `human-overview.html` at
  repo root; README features updated.
- Remaining big items: near-constant incremental recheck *after an edit* (needs the incremental
  package-naming + interface-version redesign per `AGENTS.md`, design with user first); precise
  type-syntax error ranges (`SurfaceType` carries no per-node ranges, so naming underlines the whole
  annotation — an invasive change threading ranges through type parsing).
