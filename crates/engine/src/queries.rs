//! The R query group layered on the generic red-green core in `engine.rs`.
//!
//! This is the only part of the crate that depends on `analysis`. The query *bodies* do not reimplement
//! parse / lower / naming / inference — they call `analysis`'s public phase functions verbatim (the
//! rewrite keeps tree-sitter + the M2 HM type core as bodies; see `DESIGN.md` §8 R1). Each body reads its
//! dependencies through [`Engine::fetch`], so the engine records the exact dependency edges automatically.
//!
//! # The per-symbol interface layer (the headline)
//!
//! The graph is fine-grained at the package interface (`DESIGN.md` §3): `ExportedNames` →
//! `PackageSymbolIndex` → `DefiningItem` → `GlobalScheme` → `Typecheck`. The win is that
//! [`Key::ExportedNames`] is a **names-only** cutoff query: editing a function *body* changes
//! `LocalNaming` but not the exported-name *set*, so `ExportedNames` recomputes to an equal value and cuts
//! off before [`Key::PackageSymbolIndex`] — the lone all-files fold — re-runs at all. A referrer records a
//! dependency on `GlobalScheme(s)` for precisely the symbols it references, so when one global's scheme
//! changes only its actual referrers re-typecheck (the M3 reverse index, reconstructed for free).
//!
//! # Ambient host state vs. inputs
//!
//! The shared [`Interner`] (held inside a reused [`LoweringContext`]), the tree-sitter [`Parser`], and the
//! stub library are *ambient* host state on the group struct behind `RefCell` — not query inputs — exactly
//! like salsa's interned/host state. The shared interner is what makes [`Symbol`] ids consistent across
//! files: every body interns through it, so a memoized value compares equal to a fresh recompute.

use {
    crate::{Engine, QueryGroup, Stored},
    analysis::{
        document::{Document, DocumentId},
        hir::{ExpressionId, ExpressionKind, Module},
        interner::{Interner, Symbol},
        lower::{LoweringContext, lower},
        naming::{
            DocumentKind, DocumentNamingComputation, NamesGlobal, NamesLocal,
            resolve_document_locally,
        },
        stdlib::StubLibrary,
        tree::new_parser,
        typecheck::{
            ExportedValue, InferenceError, ModuleCheck, StrictOriginKind, StrictUnknownOrigin,
            TypeDefinitionEnvironment, inference_state_with_builtins_in_interner,
        },
        types::{CoreType, TypeScheme},
    },
    analysis::diagnostic::{Diagnostic, render_type_scheme},
    std::{
        cell::{Cell, RefCell},
        collections::{BTreeMap, BTreeSet, HashMap},
    },
    tree_sitter::{Parser, Point, Range},
};

/// A workspace file. The `analysis` crate keys documents by [`DocumentId`]; the engine keys queries by
/// this raw id and wraps it in `DocumentId` only when calling an `analysis` function.
pub type FileId = u32;

/// Every query in the R graph (inputs and derived alike), per `DESIGN.md` §3.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    // --- Inputs (set from outside, never executed) -------------------------------------------------
    /// Per-file source text — the high-churn input; every keystroke is a `set_input`.
    SourceText(FileId),
    /// Package-vs-script classification, a *separate* fine-grained input so a text-only edit does not
    /// invalidate naming through a kind read.
    DocumentKind(FileId),
    /// The workspace membership set (which files exist), in path order. The single source of truth for
    /// "which files exist"; the `PackageSymbolIndex` fold reads it.
    ProjectFiles,
    /// Project check configuration (`[check]` flags). Low churn; `Typecheck` records it.
    Config,

    // --- Derived ----------------------------------------------------------------------------------
    Parse(FileId),
    Lower(FileId),
    LocalNaming(FileId),
    /// **Names-only** export set of a file (sorted [`Symbol`]s). The explicit value-eq cutoff that keeps a
    /// body edit from re-folding `PackageSymbolIndex`.
    ExportedNames(FileId),
    /// The lone all-files fold: `name → winning file`, names only. Recomputes only on *structural* export
    /// edits (add/remove/rename a top-level binding, add/remove/reclassify a file), not on body edits.
    PackageSymbolIndex,
    /// The package-global type-definition environment: a fold of every package module's top-level
    /// `@type`/`@alias` declarations, the analog of production's `TypeDefinitionEnvironment::from_modules`
    /// over all package modules. The value is `TypeDefinitionEnvironment` (value-eq), so a non-type edit
    /// (a body change that leaves every package module's type declarations identical) recomputes to an
    /// equal value and cuts off before `Typecheck`/`GlobalScheme` re-run. Scripts fold this plus their own
    /// module (script-local declarations shadow package ones) directly in `infer_file`.
    PackageTypeDefinitions,
    /// The package-wide fallback diagnostic range: byte `0..len` of the first package file's first line,
    /// matching `Analysis::fallback_range`. `Diagnostic::from_inference_error` uses it for the handful of
    /// inference errors that carry no specific source range, so the engine reproduces production's range
    /// for those exactly. Depends only on `ProjectFiles` and the first package file's `SourceText`; the
    /// range is value-eq, so an edit that does not change that first line cuts off.
    FallbackRange,
    /// The firewall: project one symbol's winning file out of the index. Value-eq per symbol.
    DefiningItem(Symbol),
    /// The per-symbol exported scheme. Editing a function body recomputes only *its* `GlobalScheme`.
    GlobalScheme(Symbol),
    /// The single out-edge of the re-export graph: `Some(b)` iff `symbol`'s winning definition is a pure
    /// re-export `a <- b` of another package global `b`, else `None`. A global re-exports at most one
    /// other, so the graph is *functional* (out-degree ≤ 1) — which is what makes SCC detection a chain
    /// walk (see [`Key::ReexportScc`]).
    ReexportTarget(Symbol),
    /// The strongly-connected component of the re-export graph containing `symbol`, as the **canonical
    /// (sorted)** member list. Empty means a trivial, non-self-looping SCC (acyclic — keep the R1a
    /// `GlobalScheme` fetch-recursion path); a non-empty list is a pure re-export cycle through `symbol`
    /// (route `GlobalScheme` through [`Key::ReexportInterface`]). Every cycle member projects the *same*
    /// sorted list, so they share one `ReexportInterface` memo.
    ReexportScc(Symbol),
    /// The converged exported-scheme sub-table for one cyclic re-export SCC, computed by a single bounded
    /// synchronous fixed-point body (`DESIGN.md` §5). Keyed by the canonical SCC member list. Owning the
    /// whole component in one body is what keeps the core's accidental-cycle guard intact: no
    /// `GlobalScheme`/`ReexportInterface` key ever re-enters itself.
    ReexportInterface(Vec<Symbol>),
    Typecheck(FileId),
    Diagnostics(FileId),
}

/// Project check configuration carried as the `Config` input. Minimal for R1a; the real `roughly.toml`
/// `[check]` surface plugs in here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub typing: bool,
    pub strict: bool,
    pub unused: bool,
}

/// The `Parse` value. [`Document`] is not `PartialEq`, but its tree is a pure function of the source, so
/// rope equality is a sound cutoff proxy: equal source ⇒ equal parse for everything downstream reads.
pub struct ParsedDocument(pub Document);

impl PartialEq for ParsedDocument {
    fn eq(&self, other: &Self) -> bool {
        self.0.rope() == other.0.rope()
    }
}

/// The `Diagnostics` value for one file. Holds the diagnostic classes the engine reproduces from
/// production's per-file pipeline: the local-naming diagnostics (`resolve_document_locally`), the raw
/// type-inference errors, and the rendered strict-mode `Unknown`-origin diagnostics. The differential
/// harness renders `type_errors` against the interner + fallback range and compares the type + strict
/// classes against production (`docs`: lint is unmodeled, and the cross-file *package*-naming diagnostics
/// live in `analysis`'s `pub(crate)` package-naming subsystem, unreachable here — both are excluded from
/// the differential and characterized in the R2 report). `type_errors` stays the raw `InferenceError`
/// list rather than a second rendered copy so there is one source of truth for the type-error set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiagnostics {
    pub naming: Vec<Diagnostic>,
    pub type_errors: Vec<InferenceError>,
    pub strict_diagnostics: Vec<Diagnostic>,
}

/// The R query group. Ambient host state (interner/parser/stubs) lives here behind `RefCell`; the engine
/// owns the memo table and revision clock.
pub struct RoughlyQueries {
    // A single reused `LoweringContext` holds the one shared, append-only interner. `lower` resets its
    // arena each call but keeps the interner, so a `Symbol` id is stable across files and recomputes.
    lowering: RefCell<LoweringContext>,
    parser: RefCell<Parser>,
    // The immutable stub library: the real embedded base corpus (`length`, `nchar`, `T`/`F`/`pi`, …),
    // loaded once against the shared interner so a bare base name resolves to its scheme exactly as
    // production does. It is set-once ambient state, not a query input; its symbols never enter the
    // package interface (a stub is not a package definition, so `DefiningItem` is `None` for it), so it
    // perturbs no per-symbol cutoff — the same isolation production asserts in `assert_stub_isolation`.
    stubs: StubLibrary,
    counters: Counters,
}

impl RoughlyQueries {
    pub fn new() -> RoughlyQueries {
        let lowering = RefCell::new(LoweringContext::new());
        // Load the stubs through the very interner every body interns through, so a user reference to a
        // base name and the stub's own binding share one `Symbol` id (no cross-interner mismatch is
        // representable), mirroring `Analysis::new_with_stub_library`.
        let stubs = StubLibrary::load(lowering.borrow_mut().interner_mut());
        RoughlyQueries {
            lowering,
            parser: RefCell::new(new_parser().expect("engine query group: R grammar should load")),
            stubs,
            counters: Counters::default(),
        }
    }

    /// Intern a name through the shared interner, so a test can name the [`Symbol`] a body produced.
    pub fn intern(&self, name: &str) -> Symbol {
        self.lowering.borrow_mut().interner_mut().intern(name)
    }

    /// Run `f` with the shared interner, so the differential harness can render an [`InferenceError`] into
    /// a [`Diagnostic`] (via `Diagnostic::from_inference_error`) using the exact interner the engine
    /// produced its [`Symbol`]s in.
    pub fn with_interner<R>(&self, f: impl FnOnce(&Interner) -> R) -> R {
        f(self.lowering.borrow().interner())
    }

    pub fn package_symbol_index_runs(&self) -> u64 {
        self.counters.package_symbol_index.get()
    }

    pub fn package_type_definitions_runs(&self) -> u64 {
        self.counters.package_type_definitions.get()
    }

    pub fn fallback_range_runs(&self) -> u64 {
        self.counters.fallback_range.get()
    }

    pub fn parse_runs(&self, file: FileId) -> u64 {
        per_key(&self.counters.parse, file)
    }

    pub fn lower_runs(&self, file: FileId) -> u64 {
        per_key(&self.counters.lower, file)
    }

    pub fn local_naming_runs(&self, file: FileId) -> u64 {
        per_key(&self.counters.local_naming, file)
    }

    pub fn exported_names_runs(&self, file: FileId) -> u64 {
        per_key(&self.counters.exported_names, file)
    }

    pub fn defining_item_runs(&self, symbol: Symbol) -> u64 {
        per_key(&self.counters.defining_item, symbol)
    }

    pub fn global_scheme_runs(&self, symbol: Symbol) -> u64 {
        per_key(&self.counters.global_scheme, symbol)
    }

    pub fn reexport_target_runs(&self, symbol: Symbol) -> u64 {
        per_key(&self.counters.reexport_target, symbol)
    }

    pub fn reexport_scc_runs(&self, symbol: Symbol) -> u64 {
        per_key(&self.counters.reexport_scc, symbol)
    }

    /// How many times the SCC fixed-point body ran for one canonical member list — the convergence
    /// witness: a cyclic interface is computed once and shared by every member, so this stays small (it
    /// never spins the round cap).
    pub fn reexport_interface_runs(&self, members: &[Symbol]) -> u64 {
        self.counters
            .reexport_interface
            .borrow()
            .get(members)
            .copied()
            .unwrap_or(0)
    }

    pub fn typecheck_runs(&self, file: FileId) -> u64 {
        per_key(&self.counters.typecheck, file)
    }

    pub fn diagnostics_runs(&self, file: FileId) -> u64 {
        per_key(&self.counters.diagnostics, file)
    }
}

impl Default for RoughlyQueries {
    fn default() -> RoughlyQueries {
        RoughlyQueries::new()
    }
}

impl QueryGroup for RoughlyQueries {
    type Key = Key;

    fn execute(&self, engine: &Engine<Self>, key: &Key) -> Stored {
        match key {
            Key::SourceText(_) | Key::DocumentKind(_) | Key::ProjectFiles | Key::Config => {
                panic!("input queries are never executed")
            }

            Key::Parse(file) => {
                bump(&self.counters.parse, *file);
                let text = engine.fetch::<String>(Key::SourceText(*file));
                let mut parser = self.parser.borrow_mut();
                let document = Document::parse(&mut parser, &text)
                    .expect("engine query: open document source should parse");
                Stored::new(ParsedDocument(document))
            }

            Key::Lower(file) => {
                bump(&self.counters.lower, *file);
                let parsed = engine.fetch::<ParsedDocument>(Key::Parse(*file));
                let mut lowering = self.lowering.borrow_mut();
                Stored::new(lower(&parsed.0, &mut lowering))
            }

            Key::LocalNaming(file) => {
                bump(&self.counters.local_naming, *file);
                let module = engine.fetch::<Module>(Key::Lower(*file));
                let kind = engine.fetch::<DocumentKind>(Key::DocumentKind(*file));
                let lowering = self.lowering.borrow();
                Stored::new(resolve_document_locally(
                    DocumentId(*file),
                    &module,
                    lowering.interner(),
                    *kind,
                ))
            }

            Key::ExportedNames(file) => {
                bump(&self.counters.exported_names, *file);
                let module = engine.fetch::<Module>(Key::Lower(*file));
                let naming = engine.fetch::<DocumentNamingComputation>(Key::LocalNaming(*file));
                // Names-only value: the file's package-definition name set, sorted. A body-only edit
                // leaves this set unchanged, so its recompute is value-eq and cuts off before the index.
                Stored::new(package_definition_names(&module, &naming.naming))
            }

            Key::PackageSymbolIndex => {
                self.counters
                    .package_symbol_index
                    .set(self.counters.package_symbol_index.get() + 1);
                let files = engine.fetch::<Vec<FileId>>(Key::ProjectFiles);
                // Path-last / last-writer-wins over the project files, in `ProjectFiles` order. A file
                // contributes only when its `DocumentKind` is `Package` (a script flip drops it exactly
                // as a deletion would), so the index reads `DocumentKind(f)` per file too.
                let mut winners: BTreeMap<Symbol, FileId> = BTreeMap::new();
                for file in files.iter() {
                    let kind = engine.fetch::<DocumentKind>(Key::DocumentKind(*file));
                    if *kind != DocumentKind::Package {
                        continue;
                    }
                    let names = engine.fetch::<Vec<Symbol>>(Key::ExportedNames(*file));
                    for name in names.iter() {
                        winners.insert(*name, *file);
                    }
                }
                Stored::new(winners)
            }

            Key::PackageTypeDefinitions => {
                self.counters
                    .package_type_definitions
                    .set(self.counters.package_type_definitions.get() + 1);
                // Fold every package module's top-level `@type`/`@alias` declarations, exactly as
                // production's `TypeDefinitionEnvironment::from_modules` over `package_document_ids`. A
                // file contributes only when its `DocumentKind` is `Package`, matching the index fold and
                // the package document set. The value is value-eq, so a body-only edit (no change to any
                // module's declarations) cuts off before any consumer re-runs.
                let files = engine.fetch::<Vec<FileId>>(Key::ProjectFiles);
                let mut modules = Vec::new();
                for file in files.iter() {
                    let kind = engine.fetch::<DocumentKind>(Key::DocumentKind(*file));
                    if *kind == DocumentKind::Package {
                        modules.push(engine.fetch::<Module>(Key::Lower(*file)));
                    }
                }
                Stored::new(TypeDefinitionEnvironment::from_modules(
                    modules.iter().map(|module| &**module),
                ))
            }

            Key::FallbackRange => {
                self.counters
                    .fallback_range
                    .set(self.counters.fallback_range.get() + 1);
                Stored::new(fallback_range(engine))
            }

            Key::DefiningItem(symbol) => {
                bump(&self.counters.defining_item, *symbol);
                let index = engine.fetch::<BTreeMap<Symbol, FileId>>(Key::PackageSymbolIndex);
                // Firewall: project this one symbol's winner. Value-eq per symbol means an index change
                // for some *other* symbol re-projects to the same value here and cuts off.
                Stored::new(index.get(symbol).copied())
            }

            Key::ReexportTarget(symbol) => {
                bump(&self.counters.reexport_target, *symbol);
                let defining = engine.fetch::<Option<FileId>>(Key::DefiningItem(*symbol));
                let target = match *defining {
                    Some(file) => {
                        let module = engine.fetch::<Module>(Key::Lower(file));
                        let naming =
                            engine.fetch::<DocumentNamingComputation>(Key::LocalNaming(file));
                        reexport_target_of(&module, &naming.naming, *symbol)
                    }
                    None => None,
                };
                Stored::new(target)
            }

            Key::ReexportScc(symbol) => {
                bump(&self.counters.reexport_scc, *symbol);
                // Functional graph: follow the single re-export out-edge from `symbol`. If the walk returns
                // to `symbol`, it is on a cycle (the visited chain is the SCC); if it dead-ends or meets a
                // cycle that does *not* include `symbol`, `symbol` sits on an acyclic tail (trivial SCC).
                Stored::new(reexport_scc_members(engine, *symbol))
            }

            Key::GlobalScheme(symbol) => {
                bump(&self.counters.global_scheme, *symbol);
                let scc = engine.fetch::<Vec<Symbol>>(Key::ReexportScc(*symbol));
                if scc.is_empty() {
                    // Acyclic: a direct definition or an acyclic re-export chain `a <- b <- …`. Inferring
                    // the winning file binds referenced globals via *their* `GlobalScheme` (recorded as a
                    // dependency), so a chain resolves by plain fetch recursion — unbounded depth, no round
                    // cap to truncate it — and the export falls out of `exported_value_schemes`.
                    let defining = engine.fetch::<Option<FileId>>(Key::DefiningItem(*symbol));
                    let scheme = match *defining {
                        Some(defining_file) => {
                            let (_check, exports) = infer_file(self, engine, defining_file);
                            exports
                                .into_iter()
                                .find(|export| export.symbol == *symbol)
                                .map(|export| export.type_scheme)
                        }
                        None => None,
                    };
                    Stored::new(scheme)
                } else {
                    // Cyclic: route through the SCC fixed-point body, which owns the whole component and
                    // never re-enters `fetch` on a `GlobalScheme`/`ReexportInterface` key — so the core's
                    // accidental-cycle guard stays intact for *real* graph bugs. Project this member out.
                    let interface = engine
                        .fetch::<BTreeMap<Symbol, TypeScheme>>(Key::ReexportInterface((*scc).clone()));
                    Stored::new(interface.get(symbol).cloned())
                }
            }

            Key::ReexportInterface(members) => {
                bump(&self.counters.reexport_interface, members.clone());
                Stored::new(resolve_reexport_interface(self, engine, members))
            }

            Key::Typecheck(file) => {
                bump(&self.counters.typecheck, *file);
                // Recorded so a config change re-checks the file; the value is unused in R1a check logic.
                let _config = engine.fetch::<Config>(Key::Config);
                let (check, _exports) = infer_file(self, engine, *file);
                Stored::new(check)
            }

            Key::Diagnostics(file) => {
                bump(&self.counters.diagnostics, *file);
                let check = engine.fetch::<ModuleCheck>(Key::Typecheck(*file));
                let naming = engine.fetch::<DocumentNamingComputation>(Key::LocalNaming(*file));
                let module = engine.fetch::<Module>(Key::Lower(*file));
                // Strict-mode diagnostics are rendered here (production renders them in `typecheck`'s
                // round 2 via the private `strict_origin_diagnostics`, ported verbatim below). Type errors
                // stay raw `InferenceError`s; the harness renders them against the interner + fallback
                // range so there is a single source of truth for the type-error set.
                let lowering = self.lowering.borrow();
                let strict_diagnostics =
                    strict_origin_diagnostics(&module, &check.strict_origins, lowering.interner());
                Stored::new(FileDiagnostics {
                    naming: naming.diagnostics.clone(),
                    type_errors: check.errors.clone(),
                    strict_diagnostics,
                })
            }
        }
    }
}

// Infer one file: the shared body of `GlobalScheme` (which projects one export out) and `Typecheck` (which
// keeps the whole `ModuleCheck`). It fetches `GlobalScheme(s)`/`DefiningItem(s)` for exactly the symbols
// `file` references, so both callers record dependencies on precisely the interface symbols they read —
// the per-symbol granularity `DESIGN.md` §3 requires. All fetches happen *before* the `lowering` borrow so
// no borrow is held across a recursive `fetch`.
fn infer_file(
    group: &RoughlyQueries,
    engine: &Engine<RoughlyQueries>,
    file: FileId,
) -> (ModuleCheck, Vec<ExportedValue>) {
    let module = engine.fetch::<Module>(Key::Lower(file));
    let naming = engine.fetch::<DocumentNamingComputation>(Key::LocalNaming(file));
    let local_naming = &naming.naming;

    let referenced: BTreeSet<Symbol> = local_naming.non_locals.values().copied().collect();
    let mut global_bindings: BTreeMap<Symbol, DocumentId> = BTreeMap::new();
    let mut imported_schemes: Vec<(Symbol, TypeScheme)> = Vec::new();
    for symbol in &referenced {
        let defining = engine.fetch::<Option<FileId>>(Key::DefiningItem(*symbol));
        if let Some(defining_file) = *defining {
            global_bindings.insert(*symbol, DocumentId(defining_file));
            // Every referenced package global is bound, matching production: a global whose winning file
            // produced no concrete export resolves to `Unknown` (production's interface-table fallback),
            // not to an unbound name — so the referrer's check sees the same environment either way.
            //
            // Mutual cross-file *value* references (e.g. file A's `a <- b(…)` and file B's `c <- a`) form a
            // cycle in the `GlobalScheme` graph that is *not* a re-export cycle (so `ReexportScc` does not
            // collapse it): resolving `GlobalScheme(a)` infers A, which needs `GlobalScheme(b)`, which
            // infers B, which needs `GlobalScheme(a)` again. We break the back-edge the way production's
            // interface fixed-point bootstraps an as-yet-unresolved global — to `Unknown` — by detecting
            // that the symbol's `GlobalScheme` is already on the recompute stack and not re-entering it.
            // For the common case (the cyclically-referenced exports carry annotations, so their schemes
            // are annotation-determined and independent of the bootstrap) this yields production's
            // converged scheme. (Where an *unannotated* mutual value cycle would need true iteration to
            // converge, a single bootstrap can differ; that residue is the documented remaining gap — see
            // `test_differential.rs`. Without this guard the engine *panics* on such input instead.)
            let scheme = if engine.is_computing(&Key::GlobalScheme(*symbol)) {
                TypeScheme::monomorphic(CoreType::Unknown)
            } else {
                let scheme = engine.fetch::<Option<TypeScheme>>(Key::GlobalScheme(*symbol));
                (*scheme)
                    .clone()
                    .unwrap_or_else(|| TypeScheme::monomorphic(CoreType::Unknown))
            };
            imported_schemes.push((*symbol, scheme));
        }
    }
    let package_naming = NamesGlobal { global_bindings };
    // The package-global type-definition environment. A package document checks against the package-wide
    // fold (`PackageTypeDefinitions`); a script additionally folds in its own module last, so its
    // script-local `@type`/`@alias` declarations shadow package definitions of the same name — exactly
    // production's split (`type_definitions` for package docs, `from_modules(package ++ own)` for scripts).
    let kind = engine.fetch::<DocumentKind>(Key::DocumentKind(file));
    let type_definitions = if *kind == DocumentKind::Package {
        (*engine.fetch::<TypeDefinitionEnvironment>(Key::PackageTypeDefinitions)).clone()
    } else {
        let files = engine.fetch::<Vec<FileId>>(Key::ProjectFiles);
        let mut package_modules = Vec::new();
        for other in files.iter() {
            let other_kind = engine.fetch::<DocumentKind>(Key::DocumentKind(*other));
            if *other_kind == DocumentKind::Package {
                package_modules.push(engine.fetch::<Module>(Key::Lower(*other)));
            }
        }
        TypeDefinitionEnvironment::from_modules(
            package_modules
                .iter()
                .map(|package_module| &**package_module)
                .chain([&*module]),
        )
    };

    let mut lowering = group.lowering.borrow_mut();
    let mut inference_state = inference_state_with_builtins_in_interner(lowering.interner_mut());
    group.stubs.seed_into(&mut inference_state);
    for (symbol, scheme) in imported_schemes {
        let imported = inference_state.import_scheme(&scheme);
        // The cross-file *definition* range bound here is synthetic. It is read only by
        // `exported_value_schemes` (to set a re-exported binding's `range`) and never by any
        // `InferenceError` or strict origin, so it cannot affect a `Diagnostics` value — the differential
        // harness confirms this empirically. Threading the real range would mean widening `GlobalScheme`'s
        // value beyond the scheme, breaking its type-only cutoff (the granularity tests in
        // `test_queries.rs` assert a body edit with an unchanged scheme does not re-typecheck referrers),
        // for zero diagnostic gain; the IDE go-to-definition use of the range is out of differential scope.
        inference_state.bind_global_scheme(symbol, imported, synthetic_range());
    }
    let check = inference_state.check_module_with_naming(
        DocumentId(file),
        &module,
        local_naming,
        &package_naming,
        &type_definitions,
    );
    let exports = inference_state.exported_value_schemes(&module, local_naming);
    (check, exports)
}

// The re-export out-edge of one package global: `Some(b)` iff `symbol`'s winning definition is a bare
// reference to another package global `b` (`a <- b`). The winner is the *last* top-level assignment of
// `symbol` that resolves to a package binding (last-writer-wins within the file), mirroring how
// `exported_value_schemes` picks the exported definition. A value that is anything but a free-symbol
// reference (a function, a literal, a call, or a reference resolving to a *local* binding) is not a
// re-export, so it has no out-edge and cannot lie on a re-export cycle.
fn reexport_target_of(
    module: &Module,
    local_naming: &NamesLocal,
    symbol: Symbol,
) -> Option<Symbol> {
    let mut winning_value: Option<ExpressionId> = None;
    collect_winning_assignment_value(
        module,
        local_naming,
        &module.expressions,
        symbol,
        &mut winning_value,
    );
    let value = winning_value?;
    match module.arena.get(value).kind {
        // A bare `Symbol` value that names dropped into `non_locals` resolves to a package global, not a
        // file-local binding — that, and only that, is a cross-file re-export edge.
        ExpressionKind::Symbol(_) => local_naming.non_locals.get(&value).copied(),
        _ => None,
    }
}

fn collect_winning_assignment_value(
    module: &Module,
    local_naming: &NamesLocal,
    expressions: &[ExpressionId],
    symbol: Symbol,
    winning_value: &mut Option<ExpressionId>,
) {
    for expression_id in expressions {
        match &module.arena.get(*expression_id).kind {
            ExpressionKind::Assign { target, value } if *target == symbol => {
                let resolves_to_binding = local_naming
                    .expression_resolutions
                    .get(expression_id)
                    .and_then(|binding_id| local_naming.bindings.get(binding_id))
                    .is_some();
                if resolves_to_binding {
                    *winning_value = Some(*value);
                }
            }
            ExpressionKind::Block { expressions, .. } => {
                collect_winning_assignment_value(
                    module,
                    local_naming,
                    expressions,
                    symbol,
                    winning_value,
                );
            }
            _ => {}
        }
    }
}

// The SCC of `symbol` over the functional re-export graph, as the canonical (sorted) member list (empty =
// acyclic). Each global has out-degree ≤ 1, so SCC detection is a chain walk: follow `ReexportTarget`
// (recording each edge as a dependency, so the SCC re-derives incrementally when an edge changes). The
// walk closes back to `symbol` only when `symbol` lies on the cycle; reaching a dead end or an interior
// repeat means `symbol` is on an acyclic tail.
fn reexport_scc_members(engine: &Engine<RoughlyQueries>, symbol: Symbol) -> Vec<Symbol> {
    let mut chain = vec![symbol];
    let mut seen = BTreeSet::from([symbol]);
    loop {
        let current = *chain.last().expect("re-export chain is never empty");
        let target = engine.fetch::<Option<Symbol>>(Key::ReexportTarget(current));
        match *target {
            None => return Vec::new(),
            Some(next) if next == symbol => {
                chain.sort();
                return chain;
            }
            Some(next) if seen.contains(&next) => return Vec::new(),
            Some(next) => {
                seen.insert(next);
                chain.push(next);
            }
        }
    }
}

// The bounded synchronous fixed-point for one cyclic re-export SCC (`DESIGN.md` §5), ported from
// `analysis.rs`'s `typecheck` package-interface loop. Every cycle member is a re-export whose target is
// another member, so the framework carries no concrete base and the fixed point is all-`Unknown` — the
// correct answer (a pure mutual re-export cycle holds no information). The iteration, the
// `#members + slack` round cap, and the oscillation guard are ported faithfully as the convergence
// machinery and the safety net for any non-monotone history. The body fetches only `ReexportTarget` for
// its members, so it is self-contained and never re-enters `GlobalScheme`/`ReexportInterface`.
fn resolve_reexport_interface(
    group: &RoughlyQueries,
    engine: &Engine<RoughlyQueries>,
    members: &[Symbol],
) -> BTreeMap<Symbol, TypeScheme> {
    let member_set: BTreeSet<Symbol> = members.iter().copied().collect();
    let mut targets: BTreeMap<Symbol, Option<Symbol>> = BTreeMap::new();
    for member in members {
        let target = engine.fetch::<Option<Symbol>>(Key::ReexportTarget(*member));
        targets.insert(*member, *target);
    }

    let unknown = TypeScheme::monomorphic(CoreType::Unknown);
    let mut table: BTreeMap<Symbol, TypeScheme> =
        members.iter().map(|member| (*member, unknown.clone())).collect();
    let mut pinned: BTreeSet<Symbol> = BTreeSet::new();
    let mut history: HashMap<Symbol, Vec<String>> = HashMap::new();

    // Round cap = `#members-in-scc + slack`, proportional to the SCC (not a fixed cap like M3's 32), so a
    // large cycle whose oscillation needs several rounds to detect still converges; the common case breaks
    // out the moment a round makes no change.
    let round_cap = members.len().saturating_add(2);
    for _round in 0..round_cap {
        let mut next = table.clone();
        for member in members {
            if pinned.contains(member) {
                next.insert(*member, unknown.clone());
                continue;
            }
            // A re-export member takes its target's current scheme; an out-of-set target cannot occur for
            // a genuine functional cycle and is treated as `Unknown` (defensive).
            let scheme = match targets.get(member).copied().flatten() {
                Some(target) if member_set.contains(&target) => table.get(&target).cloned(),
                _ => None,
            };
            next.insert(*member, scheme.unwrap_or_else(|| unknown.clone()));
        }

        // Oscillation guard: a member whose rendering returns to an earlier value while differing from the
        // previous round is swapping on a cycle, not progressing monotonically — pin it to `Unknown`,
        // which collapses the cycle and restores convergence. A pure re-export cycle never carries a
        // concrete value here, so this is the defensive net; the all-`Unknown` fixed point is reached
        // directly. Rendering reuses the shared interner (no `fetch` held across this borrow).
        {
            let lowering = group.lowering.borrow();
            for member in members {
                if pinned.contains(member) {
                    continue;
                }
                let rendered = render_type_scheme(
                    lowering.interner(),
                    next.get(member).unwrap_or(&unknown),
                );
                let entry = history.entry(*member).or_default();
                if entry.last().is_some_and(|last| *last != rendered) && entry.contains(&rendered) {
                    pinned.insert(*member);
                    next.insert(*member, unknown.clone());
                } else {
                    entry.push(rendered);
                }
            }
        }

        if next == table {
            return next;
        }
        table = next;
    }
    table
}

// The file's package-global definition names. Mirrors `analysis::naming::document_package_definitions`
// (which is `pub(crate)`, so it cannot be called from here) using only the public `NamesLocal` fields: a
// top-level `Assign` target whose expression resolves to a binding is a package global, recursing into
// bare top-level `{ }` blocks (which execute unconditionally) but not into `if`/`for`/`while`/function
// bodies. Returned sorted (a `BTreeSet`) so the value is order-stable for cutoff.
fn package_definition_names(module: &Module, local_naming: &NamesLocal) -> Vec<Symbol> {
    let mut names = BTreeSet::new();
    collect_package_definition_names(module, local_naming, &module.expressions, &mut names);
    names.into_iter().collect()
}

fn collect_package_definition_names(
    module: &Module,
    local_naming: &NamesLocal,
    expressions: &[ExpressionId],
    names: &mut BTreeSet<Symbol>,
) {
    for expression_id in expressions {
        match &module.arena.get(*expression_id).kind {
            ExpressionKind::Assign { target, .. } => {
                let resolves_to_binding = local_naming
                    .expression_resolutions
                    .get(expression_id)
                    .and_then(|binding_id| local_naming.bindings.get(binding_id))
                    .is_some();
                if resolves_to_binding {
                    names.insert(*target);
                }
            }
            ExpressionKind::Block { expressions, .. } => {
                collect_package_definition_names(module, local_naming, expressions, names);
            }
            _ => {}
        }
    }
}

// Renders each strict `Unknown` origin into a diagnostic, ported verbatim from `analysis.rs`'s private
// `strict_origin_diagnostics`. An origin that is the value of an assignment is phrased against the bound
// name (the actionable fix is to annotate that binding); any other origin is phrased against the
// expression itself. Kept byte-identical to production so the rendered messages compare equal.
fn strict_origin_diagnostics(
    module: &Module,
    strict_origins: &[StrictUnknownOrigin],
    interner: &Interner,
) -> Vec<Diagnostic> {
    if strict_origins.is_empty() {
        return Vec::new();
    }
    let mut assignment_targets = HashMap::new();
    for expression in module.arena.expressions() {
        if let ExpressionKind::Assign { target, value } = &expression.kind {
            assignment_targets.insert(*value, *target);
        }
    }
    strict_origins
        .iter()
        .map(|origin| {
            let message = if let Some(target) = assignment_targets.get(&origin.expression_id) {
                let name = interner.resolve(*target).unwrap_or("<unknown>");
                format!("strict mode: could not determine the type of `{name}`; add a type annotation")
            } else {
                match &origin.kind {
                    StrictOriginKind::UnsupportedConstruct => {
                        "strict mode: this expression has an undetermined type (`Unknown`)".to_owned()
                    }
                    StrictOriginKind::UndeterminedReference(symbol) => {
                        let name = interner.resolve(*symbol).unwrap_or("<unknown>");
                        format!(
                            "strict mode: could not determine the type of `{name}`; it has no known type"
                        )
                    }
                }
            };
            Diagnostic::strict(origin.range, message)
        })
        .collect()
}

// The package-wide fallback diagnostic range, byte `0..len` of the first package file's first line —
// `Analysis::fallback_range` verbatim. The first line is taken inclusive of its trailing newline, exactly
// as `rope().get_line(0)` yields it, and the end column is that same length (production's behaviour).
fn fallback_range(engine: &Engine<RoughlyQueries>) -> Range {
    let files = engine.fetch::<Vec<FileId>>(Key::ProjectFiles);
    for file in files.iter() {
        let kind = engine.fetch::<DocumentKind>(Key::DocumentKind(*file));
        if *kind != DocumentKind::Package {
            continue;
        }
        let text = engine.fetch::<String>(Key::SourceText(*file));
        let first_line_length = text.split_inclusive('\n').next().unwrap_or("").len();
        return Range {
            start_byte: 0,
            end_byte: first_line_length,
            start_point: Point { row: 0, column: 0 },
            end_point: Point {
                row: 0,
                column: first_line_length,
            },
        };
    }
    synthetic_range()
}

fn synthetic_range() -> Range {
    let point = Point { row: 0, column: 0 };
    Range {
        start_byte: 0,
        end_byte: 0,
        start_point: point,
        end_point: point,
    }
}

#[derive(Default)]
struct Counters {
    parse: RefCell<HashMap<FileId, u64>>,
    lower: RefCell<HashMap<FileId, u64>>,
    local_naming: RefCell<HashMap<FileId, u64>>,
    exported_names: RefCell<HashMap<FileId, u64>>,
    package_symbol_index: Cell<u64>,
    package_type_definitions: Cell<u64>,
    fallback_range: Cell<u64>,
    defining_item: RefCell<HashMap<Symbol, u64>>,
    global_scheme: RefCell<HashMap<Symbol, u64>>,
    reexport_target: RefCell<HashMap<Symbol, u64>>,
    reexport_scc: RefCell<HashMap<Symbol, u64>>,
    reexport_interface: RefCell<HashMap<Vec<Symbol>, u64>>,
    typecheck: RefCell<HashMap<FileId, u64>>,
    diagnostics: RefCell<HashMap<FileId, u64>>,
}

fn bump<K: Eq + std::hash::Hash>(counter: &RefCell<HashMap<K, u64>>, key: K) {
    *counter.borrow_mut().entry(key).or_default() += 1;
}

fn per_key<K: Eq + std::hash::Hash>(counter: &RefCell<HashMap<K, u64>>, key: K) -> u64 {
    counter.borrow().get(&key).copied().unwrap_or(0)
}
