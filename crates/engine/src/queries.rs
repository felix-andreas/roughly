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
        interner::Symbol,
        lower::{LoweringContext, lower},
        naming::{
            DocumentKind, DocumentNamingComputation, NamesGlobal, NamesLocal,
            resolve_document_locally,
        },
        stdlib::StubLibrary,
        tree::new_parser,
        typecheck::{
            ExportedValue, InferenceError, ModuleCheck, TypeDefinitionEnvironment,
            inference_state_with_builtins_in_interner,
        },
        types::TypeScheme,
    },
    analysis::diagnostic::Diagnostic,
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
    /// The firewall: project one symbol's winning file out of the index. Value-eq per symbol.
    DefiningItem(Symbol),
    /// The per-symbol exported scheme. Editing a function body recomputes only *its* `GlobalScheme`.
    GlobalScheme(Symbol),
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

/// The `Diagnostics` value: naming diagnostics plus type-inference errors for one file. Lint is deferred.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiagnostics {
    pub naming: Vec<Diagnostic>,
    pub type_errors: Vec<InferenceError>,
}

/// The R query group. Ambient host state (interner/parser/stubs) lives here behind `RefCell`; the engine
/// owns the memo table and revision clock.
pub struct RoughlyQueries {
    // A single reused `LoweringContext` holds the one shared, append-only interner. `lower` resets its
    // arena each call but keeps the interner, so a `Symbol` id is stable across files and recomputes.
    lowering: RefCell<LoweringContext>,
    parser: RefCell<Parser>,
    // The immutable stub library. Empty for R1a (see the module note); wiring the real base/CRAN stubs is
    // a set-once input whose `changed_at` never advances.
    stubs: StubLibrary,
    counters: Counters,
}

impl RoughlyQueries {
    pub fn new() -> RoughlyQueries {
        RoughlyQueries {
            lowering: RefCell::new(LoweringContext::new()),
            parser: RefCell::new(new_parser().expect("engine query group: R grammar should load")),
            stubs: StubLibrary::empty(),
            counters: Counters::default(),
        }
    }

    /// Intern a name through the shared interner, so a test can name the [`Symbol`] a body produced.
    pub fn intern(&self, name: &str) -> Symbol {
        self.lowering.borrow_mut().interner_mut().intern(name)
    }

    pub fn package_symbol_index_runs(&self) -> u64 {
        self.counters.package_symbol_index.get()
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

            Key::DefiningItem(symbol) => {
                bump(&self.counters.defining_item, *symbol);
                let index = engine.fetch::<BTreeMap<Symbol, FileId>>(Key::PackageSymbolIndex);
                // Firewall: project this one symbol's winner. Value-eq per symbol means an index change
                // for some *other* symbol re-projects to the same value here and cuts off.
                Stored::new(index.get(symbol).copied())
            }

            Key::GlobalScheme(symbol) => {
                bump(&self.counters.global_scheme, *symbol);
                let defining = engine.fetch::<Option<FileId>>(Key::DefiningItem(*symbol));
                // For a direct definition or an acyclic re-export `a <- b`, inferring the winning file
                // binds the referenced globals (via `GlobalScheme(b)`, recorded as a dependency) and the
                // export falls out of `exported_value_schemes`.
                //
                // R1b TODO (re-export *cycles*): `a <- b`, `b <- a` would make `GlobalScheme(a)` fetch
                // `GlobalScheme(b)` fetch `GlobalScheme(a)`, re-entering a key on the recompute stack and
                // tripping the core's accidental-cycle guard. The fixed-point body that owns the whole SCC
                // (`DESIGN.md` §5: bounded iteration with `Unknown`-pinning) plugs in *here* — a
                // `ReexportInterface(scc)` query this body projects from. Until then a genuine cycle
                // panics loudly rather than returning a silently-stale scheme; acyclic re-exports work.
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
                Stored::new(FileDiagnostics {
                    naming: naming.diagnostics.clone(),
                    type_errors: check.errors.clone(),
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
            let scheme = engine.fetch::<Option<TypeScheme>>(Key::GlobalScheme(*symbol));
            if let Some(scheme) = scheme.as_ref() {
                imported_schemes.push((*symbol, scheme.clone()));
            }
        }
    }
    let package_naming = NamesGlobal { global_bindings };
    // R1a gap: the package type-definition index is built from this file's own definitions only, where
    // production folds every package module (`TypeDefinitionEnvironment::from_modules`). Modelling it as a
    // separate package-global query (its own fold + cutoff) is deferred; nominal/alias references that
    // cross files are not yet resolved here. No `analysis` change is needed for that — it is a new query.
    let type_definitions = TypeDefinitionEnvironment::from_module(&module);

    let mut lowering = group.lowering.borrow_mut();
    let mut inference_state = inference_state_with_builtins_in_interner(lowering.interner_mut());
    group.stubs.seed_into(&mut inference_state);
    for (symbol, scheme) in imported_schemes {
        let imported = inference_state.import_scheme(&scheme);
        // R1a gap: the *definition* range used for a cross-file diagnostic is synthetic here. The real
        // range lives on the winning export; carrying it would widen `GlobalScheme`'s value beyond the
        // scheme and weaken its type-only cutoff, so it is deferred to the differential-validation slice.
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
    defining_item: RefCell<HashMap<Symbol, u64>>,
    global_scheme: RefCell<HashMap<Symbol, u64>>,
    typecheck: RefCell<HashMap<FileId, u64>>,
    diagnostics: RefCell<HashMap<FileId, u64>>,
}

fn bump<K: Eq + std::hash::Hash>(counter: &RefCell<HashMap<K, u64>>, key: K) {
    *counter.borrow_mut().entry(key).or_default() += 1;
}

fn per_key<K: Eq + std::hash::Hash>(counter: &RefCell<HashMap<K, u64>>, key: K) -> u64 {
    counter.borrow().get(&key).copied().unwrap_or(0)
}
