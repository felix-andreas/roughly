# Decision Log

Keep newest decisions at the top.

- search-like IDE features (workspace symbols, completion) share one subsequence matcher,
  `ide::search_match`, mirroring rust-analyzer.
  - The rule is subsequence matching: every query character must appear in the candidate in order,
    not necessarily contiguously, so `Istrumnt` matches `instrument`. Matching is case-insensitive
    unless the query contains an uppercase character, in which case it becomes case-sensitive (smart
    case). Queries shorter than 3 characters fall back to prefix matching (rust-analyzer downgrades
    very short fuzzy inputs likewise) so a one- or two-character completion query does not surface
    scattered noise; an empty query matches everything. Results are ranked by a `MatchScore`: tier
    (exact, prefix, contiguous substring, scattered subsequence) then first-match position, with the
    caller's alphabetical order as the final tiebreak. Workspace-symbol search collects all matches
    then sorts and truncates to 128 (previously it prefix-filtered and truncated in arbitrary map
    order). Completion reuses the same matcher and ranking for locals/globals/fields; the fixed
    reserved-keyword set stays prefix-only because no one searches `function` by typing `con`.
    Reference: rust-analyzer `SearchMode`/`Query` in `crates/hir-def/src/import_map.rs` and
    `crates/ide-db/src/symbol_index.rs`.

- hover, find-references, and rename graduated from experimental flags to always-on LSP capabilities.
  - All three are fixture- and LSP-integration-tested, so they no longer hide behind
    `--experimental-features`. The `goto_references`/`hovering`/`rename` flags now emit the
    "stabilized, you can remove it" warning (like `goto_definition`); `ExperimentalFeatures` keeps only
    `debug`, `range_formatting`, `unused`, `typing`. The docs page is named "Type Checker" (consistent
    with Formatter/Linter), slugged `/type-checker`, ordered before Linter, and linked from the docs
    header nav and landing page.

- the package interface is computed by a dependency-cached package-level fixed-point, so cross-file
  re-exports and forward references resolve.
  - The old round-1 computed each document's interface in isolation (other files' names checked as
    `Unknown`), so `second <- first` or `get_base <- function() base` exported `Unknown` when the
    referenced binding lived in another file. The interface phase now iterates: build the package
    table, recompute documents whose version / type-definitions / referenced schemes changed (binding
    the table for referenced names), rebuild the table, until stable. Each document's interface is
    cached on a dependency fingerprint of the schemes it references, so an edit only re-derives the
    changed document and its dependents. Recheck cost rose modestly (the per-round dependency-
    fingerprint scan is O(documents) until a reverse-dependency index is added) but cross-file types
    are now correct. Recorded in `ARCHITECTURE.md`.

- structural compatibility (record-vs-record, tuple-vs-tuple) is covariant per element and unifies
  variables, so `@new`/checked annotations infer through inference-variable fields.
  - `check_compatibility` had no record/tuple arms, so `@new Instrument` on `list(id = id, name =
    name)` inside an unannotated function could not unify the parameter variables against the nominal
    representation and errored. With covariant field/element checking, `Instrument <- function(id,
    name) { #: @new Instrument; list(id = id, name = name) }` now infers `fn(id: integer, name:
    character) -> Instrument` (and the generic `Box<T>` case likewise). It also makes nested list
    coercions work at field positions.

- reserved R constants lower to typed atomic literals.
  - `NA` is `logical`; `NA_integer_`/`NA_real_`/`NA_complex_`/`NA_character_` carry their atomic type;
    `Inf`/`NaN` are `double`; an imaginary literal like `1i` is `complex`. Added one HIR variant
    `ExpressionKind::AtomicConstant(Atomic)`. (`T`/`F` remain unhandled — they are rebindable base
    bindings needing a base-environment model.) Recorded in `TYPING_SEMANTICS.md`.

- `c(...)` follows R's atomic coercion hierarchy and drops `NULL`.
  - Was: only `integer`/`double` could mix; everything else errored. Now mixed atomics coerce to the
    widest along `logical < integer < double < complex < character` (so `c(1L, NA)` is `integer[]`,
    `c(1L, "a")` is `character[]`), `raw` only combines with `raw`, and `NULL` arguments are dropped
    (`c(x, NULL)` is `c(x)`, `c(NULL)` is `NULL`). This matches R and is needed for pervasive NA use.
    Recorded in `TYPING_SEMANTICS.md`.

- hover output is human-readable by default; phase dumps moved under a debug-only section.
  - Default hover shows unnamed primary blocks — the inferred type and, for a variable use, where it
    is defined and whether it is local or package-global. The old `Lowering`/`Naming`/`Typing`/`Parsing`
    phase sections only render under a named `### Debug` heading when debug mode is on. The phase
    sections were originally a debugging aid; the new default is for humans. `HoverInfo` now carries
    `contents: Vec<String>` plus `debug: Vec<DebugSection>`.

- an expression-level annotation (for example `#: @new User` on a block's final expression) is
  applied wherever the expression is inferred, not only on assignments.
  - `apply_annotation` previously ran only for assignment bindings, so `@new`/checked annotations on a
    bare expression (such as a function's returned `list(...)`) were silently ignored and the value
    kept its structural type. The inference wrapper now applies any non-binding attached annotation.

- record types reject duplicate field names and allow a trailing comma.
  - `list{ id: integer, id: integer }` is now an `InvalidSemantics` error (matching duplicate type
    parameters); `list{ id: integer, name: character, }` parses, so multi-line record annotations can
    use a trailing comma.

- the document interface settles by a bounded fixed-point instead of two fixed passes.
  - Deeply chained top-level references (`a <- b <- c <- 1L`) and forward references now resolve; the
    loop stops as soon as no export is `Unknown` or the exports stop changing, so genuine cycles still
    settle (keeping `Unknown`) and shallow cases stay one or two rounds.

- `typecheck` returns immediately when the package version is unchanged since the last completed run.
  - Repeated IDE requests (successive hover / inlay-hint / signature-help calls without an edit) no
    longer re-run package-scoped work. The package version bumps on every document or check-config
    change, so the guard is invalidated exactly when something relevant changed.

- parameter default expressions are lowered, named, and typechecked, but do not pin an unannotated
  parameter's type.
  - Defaults were previously dropped entirely (`Parameter` only stored `has_default`). The default
    expression is now lowered to an `ExpressionId`, resolved in the function scope (all parameters in
    scope, matching R's lazy default evaluation), and typechecked. An error inside a default is
    reported and a non-`NULL` default for an annotated parameter must be compatible with the declared
    type. A `NULL` default is always allowed because it is R's optional-parameter sentinel (e.g.
    `function(count, label = NULL)` with `[label]: character`). An unannotated parameter's type comes
    from its uses, not its default, so `function(value, width = NULL)` keeps `width` polymorphic.

- inlay hints and signature help are built on retained checked types, in `analysis::ide` and wired
  into the LSP server.
  - `ide::inlay_hints` shows inferred types on unannotated bindings (concrete types only, so
    polymorphic/`Unknown` bindings stay unannotated rather than showing internal variables).
  - `ide::signature_help` shows the called function's inferred signature with the active parameter
    derived from how many arguments precede the cursor. Both LSP capabilities are gated on typing.

- typed expression results are retained per document so hover, inlay hints, and signature help can
  show checked types.
  - Round-2 typecheck records each expression's resolved type by id (`InferenceState` recording,
    gated to round 2 to avoid interface-round cost). `Analysis::checked_expression_type` exposes it
    and hover renders a `Typing` section. The IDE fixture runner now enables typing.

- function-type compatibility is contravariant in parameters and covariant in returns.
  - `check_compatibility` previously checked parameters covariantly, which was unsound: a function
    accepting only `integer` was wrongly accepted where a function accepting `integer | NULL` was
    required. Parameters are now checked contravariantly (expected parameter compatible with actual
    parameter) and the return covariantly. Recorded in `TYPING_SEMANTICS.md`.

- unannotated values used arithmetically carry a `numeric` constraint instead of erroring, and
  numeric-constrained inference variables generalize as `<T: numeric>` or default to `double`.
  - `function(x) x + 1L` previously failed with `expected a numeric value, found type1`, which was
    the single biggest usability gap for real R code. Inference variables now carry a constraint
    (`Constraint::Numeric`); arithmetic, unary `-`, `:`, and numeric comparison constrain a flexible
    operand instead of rejecting it. The constraint generalizes into a rank-1 numeric type parameter
    (so `f(1L)` and `f(2.5)` both type-check while `f("x")` errors at the call site), and a numeric
    variable that escapes a binding without being abstracted by a function parameter defaults to
    `double`. This is a lightweight qualified-type / numeric-type-class model; only the `numeric`
    constraint exists for now. Recorded in `TYPING_SEMANTICS.md`.

- typecheck is incremental at document grain via a two-round interface model; cross-file references are scheme-based with no inference flow across files.
  - Round 1 computes each package file's exported schemes in isolation (run twice so define-then-alias settles), round 2 checks every document against the package interface table. Caches key on document version plus rendered interface fingerprints, so body edits recheck one file and interface changes recheck dependents. The old whole-package single-inference-state model let a call in one file silently solve a function's parameter types in another file, which was order-dependent and impossible to invalidate at document grain.

- generalization is level-based instead of walking the environment.
  - The environment walk was quadratic and made a 300k-line package take ~29s; levels plus binding only referenced interface schemes brought a cold full check to ~8.6s and a single-file recheck in a 500-file package to ~56ms.

- scripts resolve their top level sequentially like a function body and are typechecked.
  - Script self-references previously warned `could not resolve` because the local pass deferred all top-level names to package-global resolution, which scripts never join. Sequential scoping matches script execution order and makes script-local rebinding behave as documented.

- a failed top-level binding recovers as `Unknown` on both its local and package-global lookup paths, and checking continues with the next top-level expression.
  - One error no longer hides every later error in the package; this also keeps per-document checking coherent when a winner binding fails.

- inferred function types carry every parameter as a named, position-matchable parameter; defaults make parameters optional; parameter names are call interface, not type identity.
  - R parameters are always matchable by name and position, so dropping names made named-argument calls on unannotated functions impossible and hover output worse. Function types unify and check compatibility positionally across the flattened parameter list, so `fn(integer)` and `fn(count: integer)` stay interchangeable. An expected-optional parameter requires an actual default.

- call arguments are checked with compatibility instead of unification, and `Unknown` arguments are accepted at any parameter.
  - Unification rejected the documented coercions (`T` into `T[]`, `T` into `T | NULL`) at parameter positions. Accepting `Unknown` arguments suppresses cascade errors after the original cause was already diagnosed.

- comparison operators, unary `!`, `%%`, `%/%`, `^`, `:`, and `c()` emptiness are now defined semantics, recorded in `TYPING_SEMANTICS.md`.
  - These are everyday R constructs; leaving them `Unknown`/unsupported made the checker useless on real code. `:` counts whole-number double literals as integer endpoints to match R's runtime behavior for `1:10`. Comparisons require one comparison family (numeric, character, logical) and produce `logical` with the arithmetic shape rule. `c()` with no arguments is `NULL`, matching R.

- `@new` typechecks the annotated value against the nominal representation type, and `@new` type arguments lower through the ordinary annotation-lowering path.
  - The previous implementation trusted `@new` unconditionally and erased named type arguments to `Unknown`, so nominal introduction silently accepted wrong values.

- typing-time analysis should eagerly refresh `lint`, `lower`, and `resolve_document`, while package resolution and typecheck stay lazy until save or an IDE action needs them.
  - This keeps local diagnostics and local tooling current in the unsaved buffer without paying package-scoped semantic cost on every keystroke. One versioned phase cache remains the only source of truth, so save and IDE actions request broader freshness over the same retained artifacts instead of building separate caches.

- maybe-undefined value diagnostics should be emitted by local naming while preserving local binding resolution.
  - The semantic fact is file-local control-flow availability, not package-global lookup failure. Local naming preserves the local `BindingId` for tooling and typecheck and emits the warning at the local use site.

- typecheck should use naming-owned `BindingId` for local value lookups instead of rebuilding local lexical resolution by raw symbol.
  - Naming is already the source of truth for file-local binding identity, so typecheck now binds local assignments, parameters, and `for` variables under those ids and resolves local symbol uses through `expression_resolutions`. Package/global lookup remains a separate boundary for now.

- typecheck should consume naming outputs instead of performing a second symbol-based name-resolution pass for package/global references.
  - Naming is the source of truth for unresolved/global value resolution, and typecheck now treats naming-missing references as `Unknown` instead of emitting duplicate unknown-name diagnostics. Package/global lookup and top-level winner binding behavior come from naming.

- `lint` should be a separate file-local phase and test suite, rather than being folded into `lower`.
  - Lint rules depend only on parsed tree structure and source text, but they are not part of HIR construction. Keeping `lint` separate preserves a cleaner structural boundary while still letting `analysis` own all diagnostic production.

- binding ids should stay document-local, and `run_naming` should preserve unchanged local naming results while rerunning only changed or missing documents.
  - Package-global binding ids drift under incremental recomputation and create the wrong dependency shape. Document-local ids keep binding ownership local, make the incremental boundary real, and let package lookup resolve through `global_bindings` plus the winning document's `global_exports`.

- project 005 should rebuild `global_bindings` package-wide from local export tables on each naming run, rather than trying to maintain that winner table incrementally now.
  - This keeps the implementation simple while naming is still package-wide, avoids repeated lazy package scans for cross-file lookups, and leaves a clean future upgrade path to a reverse `Symbol -> ordered exporters` index once package naming itself becomes incremental.

- naming should keep local use sites, maybe-undefined use sites, and non-local use sites distinct, and binder-side local ids should live in one shared binder table instead of one table per binder kind.
  - `unresolved_values` is misleading because many of those names are only unresolved in the file-local pass, not after package-global lookup. `annotated_expressions` is too coarse because it records that an annotation exists, not the actual type-name references naming still needs to resolve. The old split between `function_parameter_bindings` and `for_bindings` also duplicated one binder-ownership fact.

- stable exported declaration identity is out of scope for project 005.
  - Project 005 only needs a cleaner naming data model and better incremental boundaries. Durable declaration identity matters later for tooling features across edits, not for this naming cleanup.

- package-global non-local lookup should not be eagerly materialized as `ExpressionKey -> BindingId`; it should stay symbol-keyed until a later consumer resolves through package exports.
  - Eager materialization ties naming results to snapshot-local binding ids and makes incremental reuse worse. The symbol-keyed package export table is the more stable boundary for project 005.

- duplicate top-level value names should warn only when they conflict in package-visible naming; non-package documents may reuse those names without the package-global duplicate-binding warning.
  - Package files contribute to one package-global value namespace, so conflicts there are meaningful and should warn. Non-package documents do not contribute to that namespace, so their top-level rebinding should stay script-local.

- naming should use one local `BindingId` space directly and drop `ProvisionalBindingId`, and package-global symbol tables should point to defining modules without redundantly storing module-local binding ids.
  - Provisional ids were a migration seam, not a semantic requirement. Keeping package tables symbol-keyed plus module-keyed improves incremental recomputation boundaries while module-local `global_exports` remains the source of concrete local binding ids.

- non-package documents can resolve package-global value and type names, but they do not contribute
  back to those namespaces or conflict with package files on same-name declarations.
  - This matches the script-file naming contract now captured in `TYPING_SEMANTICS.md` and the naming
    fixtures: package-attached scripts are consumers of package-global namespaces, not producers.

- `run_naming` should consume lowered package state rather than triggering lowering itself.
  - The phase boundary should be real so tests, tooling, and incremental scheduling can call lowering and naming separately. `run_naming` itself should not hide lowering work behind a combined wrapper.

- package-global value resolution uses one final symbol table built from top-level exports, and the file-local naming pass does not resolve globals even within the same file.
  - This matches the intended runtime-like package semantics better than preserving earlier top-level bindings at individual use sites, and it gives naming a cleaner split between file-local lexical facts and package-global consolidation. Duplicate top-level value definitions still warn on both the overwritten and overwriting declarations.

- `workspace` should stay thinner than `package` and should not mirror package mutation APIs.
  - `Package` is the analysis unit and should own package contents directly. `Workspace` is the editor-facing registry and mutation helper around packages plus detached scripts, not a second package API surface.

- fixture runners return structured snapshots instead of pre-rendered joined strings.
  - The fixture suite now compares per-snapshot per-file outputs directly, carries expectations
    forward across generations, uses `#++++ any` for unchecked contents, and treats extra actual
    outputs as failures.

- parsed-document storage lives in `analysis::workspace`, not in a separate `workspace` crate.
  - The package and script bucket model is now part of the analysis phase boundary, and keeping it inside `analysis` simplifies the API and removes stale cross-crate surface area.

- package-attached non-contributing files are `scripts`.
  - They are attached to a package, can resolve against the package namespace, and do not contribute back to that namespace.

- `lower` stays file-local, while `analysis.rs` owns package-scoped phase orchestration and full `check`.
  - The package is the unit of naming and later semantic phases, but lowering still needs to run directly on individual parsed documents.

- `type_syntax` simple fixtures compare the parser's rendered success or rendered failure directly, with no `error:` sentinel in fixture input.
  - The fixture body should contain only the actual source under test. The runner always calls `parse_type_syntax` on that source and compares either the rendered type or the rendered parse error.

- the fixture crate parses the fixture language, and the testing framework combines that parsed data with `workspace` state.
  - Expectations attach to the immediately preceding file or operation. If a document already has an expectation from an earlier generation, that expectation carries forward until replaced, deleted, or moved.

- type names are project-global, and top-level value names are package-global across files.
  - `@type` and `@alias` declarations share one project-global namespace, forward references are allowed across files, and duplicate type names are errors at every conflicting declaration. Top-level value names are visible across package files, later files in package collation order win on conflicts, and both overwritten and overwriting value definitions should warn. We also want to leave room for a future file-local opaque-type feature.

- naming data is a tooling boundary as well as a typechecking boundary.
  - The naming result should support go-to-definition and local rename within a file now, and it should scale to project-level rename once cross-file naming data exists.

- HIR stores top-level type declarations separately from top-level executable expressions.
  - Type declarations are not expression nodes, and their interleaving with executable top-level expressions is not semantically significant for later phases.

- non-top-level `@type` and `@alias` definition blocks are lowering errors.
  - Declaration placement is a structural front-end concern, so the checker should reject it before naming or typechecking.

- `@new` accepts nominal type references, including generic nominal applications.
  - This keeps nominal introduction aligned with ordinary nominal type syntax while still rejecting aliases and structural type forms; generic nominal types therefore require full type arguments such as `@new Person<integer>`.

- assignment annotations live only on the assignment expression, not in a second `Assign.annotation` slot.
  - `AttachedAnnotation` now uses explicit variants for expression-only versus binding-and-expression attachment, so duplicating assignment annotations in HIR was redundant.

- The parser module for `#:` typing blocks is `type_syntax`.
  - It handles both attachable annotations and standalone `@type` / `@alias` definition blocks, so `annotations` was too narrow.

- The public parser entrypoint is `parse_type_syntax`, and its result type is `TypeSyntax`.
  - This removes the old naming overlap where the public parser used `annotation` for both the whole parsed block and one semantic variant.

- `check` remains the top-level orchestration entry point.
  - This keeps the public entry point aligned with the full checking pipeline instead of exposing internal phases.

- `parser` is not a real `analysis` crate phase and should not remain on the public crate surface.
  - Syntax parsing is external integration or test support, while the checker should be able to consume already-parsed syntax.

- `naming` is the agreed phase name.
  - The phase resolves names to stable identities, but `naming` is the agreed architectural term.

- `typecheck` is the semantic checking phase, and inference is an internal mechanism inside it.
  - This matches the intended phase boundary better than treating HM inference as the phase itself.

- diagnostics are not a top-level pipeline phase.
  - Diagnostics are output produced by lowering, naming, and typechecking rather than a separate execution stage.

- annotation parsing should happen during lowering exactly once.
  - This avoids duplicate work and gives lowering a clean annotated front-end boundary.

- `hir.rs` and `lower.rs` should be separate files.
  - Keeping the representation separate from the lowering logic makes the phase boundary clearer.

- naming should stay distinct from lowering.
  - This preserves a clean front-end boundary even if the implementation runs both phases back to back.

- `diagnostic.rs` should exist as a shared module for structured diagnostics and rendering.
  - Keeping diagnostic data, codes, severities, ranges, and rendering together makes wording changes and multi-phase reporting easier than rendering directly at error creation sites.

- HIR should use arena or id-based storage.
  - Stable ids make naming tables, type tables, hover lookups, and later incremental work fit the architecture better than nested tree-only storage.

- keep one `typecheck.rs` for now.
  - Splitting out a separate engine file can wait until the internal structure is clearer.

- keep builtin typing, compatibility logic, and interface extraction inside `typecheck.rs` for now.
  - Those splits should wait until the typechecking structure stabilizes.

- successful-check fixtures should be split into separate suites rather than one overloaded suite.
  - Different checked outputs have different contracts and should not be forced into a single renderer.

- use `expressions` as the suite name for smaller checked-expression cases.
  - This matches the current `inference` suite purpose more closely than `typecheck`.

- Naming produces side tables keyed by stable ids.
  - Now that HIR uses an arena and `ExpressionId`, side tables prevent allocating an entire new `NamedFile` tree. It also simplifies mapping AST locations for hover tools.

- Naming resolves only value names for now.
  - Since types are currently represented by `SurfaceType` from annotations, type-level scoping isn't necessary yet.
