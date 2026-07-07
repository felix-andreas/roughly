//! The R query group layered on the generic red-green core in `engine.rs`.
//!
//! This is the only part of the crate that depends on `analysis`. The query *bodies* do not reimplement
//! parse / lower / naming / inference — they call `analysis`'s public phase functions verbatim, so the
//! tree-sitter parser and the Hindley-Milner type core run inside query bodies. Each body reads its
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
//! changes only its actual referrers re-typecheck (reverse-dependency routing, emergent for free from
//! the dependency graph rather than a hand-maintained reverse index).
//!
//! # Ambient host state vs. inputs
//!
//! The shared [`Interner`] (held inside a reused [`LoweringContext`]), the tree-sitter [`Parser`], and the
//! stub library are *ambient* host state on the group struct behind `RefCell` — not query inputs — exactly
//! like salsa's interned/host state. The shared interner is what makes [`Symbol`] ids consistent across
//! files: every body interns through it, so a memoized value compares equal to a fresh recompute.

use {
    crate::{Engine, QueryGroup, Shared, Stored},
    analysis::{
        CheckConfig, LintConfig,
        diagnostic::{Diagnostic, render_type_scheme},
        document::{Document, DocumentId},
        hir::{DefinitionKind, ExpressionId, ExpressionKind, HirArena, Module},
        interner::{Interner, Symbol},
        lint::analyze as lint_analyze,
        lower::{LoweringContext, LoweringResult, lower_with_diagnostics},
        naming::{
            BindingInfo, DocumentKind, DocumentNamingComputation, NamesGlobal, NamesLocal,
            resolve_document_locally,
        },
        stdlib::{ProjectStubSource, StubLibrary},
        tree::new_parser,
        type_syntax::type_name_token_range,
        typecheck::{
            BUILTINS, ExportedValue, InferenceError, InferenceState, ModuleCheck,
            TypeDefinitionEnvironment, inference_state_with_builtins_in_interner,
        },
        types::{Annotation, CoreType, NamedTypeRef, SurfaceType, TypeScheme},
    },
    ropey::Rope,
    std::{
        cell::{Cell, Ref, RefCell},
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
    /// **Declarations-only** view of one file's module for the type environment fold: the file's lowered
    /// [`Module`] held by shared pointer but compared by its `@type`/`@alias` declarations only (see
    /// [`TypeDefinitionsModule`]). The cutoff seam that keeps a body edit from re-folding
    /// [`Key::PackageTypeDefinitions`], the type-checking analog of [`Key::FileTypeDefinitions`] for the
    /// naming index.
    TypeDefinitionsModule(FileId),
    /// The package-global type-definition environment: a fold of every package module's top-level
    /// `@type`/`@alias` declarations, the analog of production's `TypeDefinitionEnvironment::from_modules`
    /// over all package modules. Folds the per-file [`Key::TypeDefinitionsModule`] views (not `Lower`
    /// directly) so a non-type edit (a body change that leaves every package module's type declarations
    /// identical) cuts off at the view and **never re-folds here**; the value is also `TypeDefinitionEnvironment`
    /// (value-eq), so even a structural edit that leaves the declarations equal cuts off before
    /// `Typecheck`/`GlobalScheme` re-run. Scripts fold this plus their own module (script-local
    /// declarations shadow package ones) directly in `infer_file`.
    PackageTypeDefinitions,
    /// The package-wide fallback diagnostic range: byte `0..len` of the first package file's first line,
    /// matching `Analysis::fallback_range`. `Diagnostic::from_inference_error` uses it for the handful of
    /// inference errors that carry no specific source range, so the engine reproduces production's range
    /// for those exactly. Depends only on `ProjectFiles` and the first package file's `SourceText`; the
    /// range is value-eq, so an edit that does not change that first line cuts off.
    FallbackRange,
    /// The firewall: project one symbol's winning file out of the index. Value-eq per symbol.
    DefiningItem(Symbol),
    /// One file's full exported value schemes from a single whole-file inference. `GlobalScheme`
    /// projects single symbols out of this, so a file with k exports costs one inference per body
    /// edit instead of k.
    ExportedSchemes(FileId),
    /// The per-symbol exported scheme. Editing a function body recomputes only *its* `GlobalScheme`
    /// (value-eq per symbol — this is the interface firewall referrers cut off on).
    GlobalScheme(Symbol),
    /// The interface-dependency out-edges of one package global: the **other** package globals that
    /// computing `GlobalScheme(symbol)` reads. Computing a global's scheme infers its whole winning file,
    /// which binds *every* package global that file references (`infer_file`); so the edge set is exactly
    /// the referenced non-local names of `symbol`'s defining file that themselves resolve to package
    /// globals (have a [`Key::DefiningItem`]). Out-degree is *arbitrary* (a file may reference many
    /// globals), so this is a general directed graph — re-exports (`a <- b`, a single edge) are just the
    /// degenerate case. SCC detection over it is Tarjan, not a chain walk (see [`Key::SymbolScc`]).
    InterfaceDeps(Symbol),
    /// The strongly-connected component of the [`Key::InterfaceDeps`] graph containing `symbol`, as the
    /// **canonical (sorted)** member list. Empty means a trivial, non-self-looping SCC (acyclic — take the
    /// plain `GlobalScheme` fetch-recursion path); a non-empty list (size ≥ 2) is a cycle of mutually
    /// interface-dependent globals through `symbol` — a mutual re-export *or* a mutual value reference —
    /// (route `GlobalScheme` through [`Key::InterfaceScc`]). Every cycle member computes the *same* sorted
    /// list, so they share one `InterfaceScc` memo. Computed by Tarjan over the lazily-fetched edge graph.
    SymbolScc(Symbol),
    /// The converged exported-scheme sub-table for one cyclic interface SCC, computed by a single bounded
    /// synchronous fixed-point body porting production's package-interface loop (`analysis.rs::typecheck`).
    /// Keyed by the canonical SCC member list. Each round re-infers the members' winning files with the
    /// current SCC schemes bound for in-SCC references (and ordinary `GlobalScheme` fetches for out-of-SCC
    /// references, which are acyclic), bootstrapping every member to `Unknown` and pinning an oscillating
    /// member to `Unknown`. Owning the whole component in one body is what keeps the core's accidental-cycle
    /// guard intact: no `GlobalScheme`/`InterfaceScc`/`SymbolScc` key ever re-enters itself.
    InterfaceScc(Vec<Symbol>),

    // --- Package-naming interface -----------------------------------------------------------------
    // The cross-file naming diagnostics `analysis` computes in its `pub(crate)` package-naming
    // subsystem (`package_document_diagnostics`), reproduced here as fine-grained queries. The
    // rewrite decision allows the duplication: the production logic is unreachable across the crate
    // boundary, so the engine re-derives the same facts with the same value-eq / per-symbol cutoff
    // discipline as the type interface above.
    /// **Type-declarations-only** projection of one file: its top-level `@type`/`@alias` names mapped
    /// to their `(kind, arity)` sites, in declaration order. The type analog of [`Key::ExportedNames`]:
    /// a body edit that touches no type declaration recomputes to an equal value and cuts off before
    /// [`Key::PackageTypeIndex`] re-folds. (`analysis::naming::document_type_definitions`.)
    FileTypeDefinitions(FileId),
    /// The package-wide naming-level type index: each uniquely-defined `@type`/`@alias` name mapped to
    /// its info, plus the set of names defined by more than one package site (kept *out* of the
    /// resolved map and flagged as duplicates). The naming analog of [`Key::PackageSymbolIndex`],
    /// folded from [`Key::FileTypeDefinitions`] over package files. Value-eq, so a non-type edit cuts
    /// off. (`analysis::naming::build_type_index`.)
    PackageTypeIndex,
    /// The firewall projecting one type name's status (undefined / defined / duplicate) out of
    /// [`Key::PackageTypeIndex`]. Value-eq per name, so changing type `X` re-runs only the files that
    /// reference or define `X` — a type reverse-dependency hop, here emergent from the firewall.
    TypeNameStatus(Symbol),
    /// The package-wide ordered definer lists: each package-global name mapped to the files whose
    /// top-level bindings define it, in `ProjectFiles` (path) order — the full candidate list that
    /// [`Key::PackageSymbolIndex`] reduces to a last-writer winner. Folded from [`Key::ExportedNames`]
    /// over package files. Value-eq. (`analysis::naming::build_candidate_order`.)
    PackageCandidateOrder,
    /// The firewall projecting one name's ordered definer list out of [`Key::PackageCandidateOrder`].
    /// Value-eq per name (the overwrite analog of [`Key::DefiningItem`]): adding or removing a definer
    /// of `X` re-runs only `X`'s co-definers' overwrite diagnostics.
    DefinerOrder(Symbol),
    /// One file's package-naming diagnostics: could-not-resolve, builtin shadow, overwrite pairs,
    /// duplicate-type, and type-reference. Reproduces `analysis::naming::package_document_diagnostics`
    /// byte-for-byte (same wording, ranges, severities, codes). Depends on the file's own naming plus
    /// only the per-symbol interface facts it reads ([`Key::DefiningItem`], [`Key::DefinerOrder`],
    /// [`Key::TypeNameStatus`]), so a body edit elsewhere cuts off unless this file's resolution moved.
    PackageNamingDiagnostics(FileId),

    /// One file's lowering-phase diagnostics (`analysis::lower::lower_with_diagnostics`): tree-sitter
    /// syntax errors on a malformed file (`collect_syntax_errors`), or the lowering-pass diagnostics
    /// (deep-nesting, directive-mix) on a well-formed one. A pure function of the file's `Parse`, so it
    /// is file-local with no cross-file edge. Surfacing these is what lets the differential harness
    /// compare the lowering `SyntaxError` class instead of excluding it.
    LoweringDiagnostics(FileId),
    /// One file's lint diagnostics (`analysis::lint::analyze`): a pure function of the file's `Parse`
    /// and the `[lint]` config. File-local; the engine models the linter here so the differential can
    /// compare the `Lint` class with zero exclusions.
    Lint(FileId),
    Typecheck(FileId),
    Diagnostics(FileId),
}

/// Project configuration carried as the `Config` input: the `[check]` flags plus the `[lint]` settings.
/// Low churn (a `roughly.toml` edit), so folding lint here rather than into a separate input is the right
/// trade — a config change is rare, and keystroke edits touch `SourceText`, never this.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub check: CheckConfig,
    pub lint: LintConfig,
}

/// Parses a source string into the `SourceText` input value. Hosts that already hold an
/// incrementally-maintained [`Document`] (the language server's open buffers) feed it directly
/// instead — that is the whole point of the parsed input: one parse per edit, done by the host.
pub fn parse_source_input(source: &str) -> ParsedDocument {
    let mut parser = analysis::tree::new_parser().expect("tree-sitter parser should initialize");
    let document = Document::parse(&mut parser, source).expect("engine input source should parse");
    ParsedDocument(document)
}

/// The `Parse` value. [`Document`] is not `PartialEq`, but its tree is a pure function of the source, so
/// rope equality is a sound cutoff proxy: equal source ⇒ equal parse for everything downstream reads.
pub struct ParsedDocument(pub Document);

impl PartialEq for ParsedDocument {
    fn eq(&self, other: &Self) -> bool {
        self.0.rope() == other.0.rope()
    }
}

/// The `TypeDefinitionsModule` value: a file's lowered [`Module`] held by shared pointer (no copy) but
/// compared **by its `@type`/`@alias` declarations only**. This is the type-checking analog of
/// [`Key::FileTypeDefinitions`] (the naming-level declarations-only cutoff): the lone all-files type
/// environment [`Key::PackageTypeDefinitions`] folds these views, and `TypeDefinitionEnvironment::from_modules`
/// reads nothing but `module.definitions`, so a body-only edit recomputes the view (its `Lower` changed)
/// to a value that compares equal and **cuts off before the package type environment re-folds**. Without
/// this the environment folded `Lower(f)` directly and re-folded on every keystroke (an O(package) recompute
/// for a body edit that changed no type), exactly the regression the names-only `ExportedNames` cutoff
/// avoids for the symbol index.
pub struct TypeDefinitionsModule(pub Shared<Module>);

impl PartialEq for TypeDefinitionsModule {
    fn eq(&self, other: &Self) -> bool {
        self.0.definitions == other.0.definitions
    }
}

/// The `Diagnostics` value for one file. Holds the diagnostic classes the engine reproduces from
/// production's per-file pipeline: the local-naming diagnostics (`resolve_document_locally`), the
/// cross-file *package*-naming diagnostics (`package_document_diagnostics`, re-derived as queries), the
/// raw type-inference errors, and the rendered strict-mode `Unknown`-origin diagnostics. The
/// differential harness renders `type_errors` against the interner + fallback range and compares every
/// class except lint against production: `naming` (local) is identical by construction, `package_naming`
/// reproduces production's wording/ranges, and the type + strict classes are the novel interface layer.
/// `type_errors` stays the raw `InferenceError` list rather than a second rendered copy so there is one
/// source of truth for the type-error set. The lowering-phase classes (`AnnotationError` and
/// lowering-originated `SyntaxError`) are unreachable: the public `lower` returns only the `Module` and
/// drops its diagnostics, so they live outside this value and the comparison (see the harness module doc).
/// `unused` holds the "assigned but never used" warnings (`DiagnosticCode::Unused`), synthesized from
/// `NamesLocal::unused_assignments`. Like `strict_diagnostics` and `type_errors` it is computed
/// unconditionally and gated by the consumer (production's `check_config.unused`), so it is kept
/// out of the always-emitted `naming` field — which stays the verbatim `resolve_document_locally` output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiagnostics {
    pub naming: Vec<Diagnostic>,
    pub package_naming: Vec<Diagnostic>,
    pub type_errors: Vec<InferenceError>,
    pub strict_diagnostics: Vec<Diagnostic>,
    // The file's own `#: @strict` directive, overriding the configured strict switch for this
    // file at the consumer's gate.
    pub strict_override: Option<bool>,
    pub unused: Vec<Diagnostic>,
    // Lowering-phase (syntax) and lint diagnostics: like production's `document_diagnostics`, both are
    // emitted *unconditionally* (not config-gated), so the consumer renders them directly.
    pub lowering: Vec<Diagnostic>,
    pub lint: Vec<Diagnostic>,
}

/// One package-global type name's resolved shape: the kind it was declared with and its type-parameter
/// arity. The engine's stand-in for `analysis::naming::TypeInfo` (which is `pub(crate)`), reusing the
/// public [`DefinitionKind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineTypeInfo {
    pub kind: DefinitionKind,
    pub arity: usize,
}

/// The package-wide naming-level type index ([`Key::PackageTypeIndex`]): the resolved unique-name map and
/// the duplicate set. A name defined by exactly one package site is in `resolved`; a name defined by two
/// or more sites is in `duplicates` and absent from `resolved` (references to it stay unresolved), exactly
/// as `analysis::naming::build_type_index` collates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeIndex {
    pub resolved: BTreeMap<Symbol, EngineTypeInfo>,
    pub duplicates: BTreeSet<Symbol>,
}

/// One type name's status projected out of [`TypeIndex`] by the [`Key::TypeNameStatus`] firewall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeNameStatus {
    Undefined,
    Defined(EngineTypeInfo),
    Duplicate,
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
    // perturbs no per-symbol cutoff — the same stub isolation the from-scratch checker keeps structurally.
    stubs: StubLibrary,
    // The builtins+stubs inference environment, built once on first use and cloned per inference —
    // rebuilding it seeded every whole-file inference with ~500 stub scheme imports otherwise.
    // Valid for the group's lifetime: builtins and the stub corpus are set-once ambient state.
    inference_template: RefCell<Option<InferenceState>>,
    counters: Counters,
}

impl RoughlyQueries {
    pub fn new() -> RoughlyQueries {
        Self::with_project_stubs(Vec::new())
    }

    // Builds the query group with project-supplied `.Rtypes` stub files folded over the shipped stub
    // corpus, so a project stub overrides a shipped one of the same name (and a file's name declares
    // its namespace for `pkg::name` reads). The stubs load through the very interner every body
    // interns through, so a user reference to a base name and the stub's own binding share one
    // `Symbol` id (no cross-interner mismatch is representable), mirroring
    // `Analysis::new_with_stub_library`. The assembled library stays a set-once input — it is read
    // once at construction and never re-read on an edit.
    pub fn with_project_stubs(project_stub_sources: Vec<ProjectStubSource>) -> RoughlyQueries {
        let lowering = RefCell::new(LoweringContext::new());
        let stubs = StubLibrary::load_with_overrides(
            lowering.borrow_mut().interner_mut(),
            &project_stub_sources,
        );
        RoughlyQueries {
            lowering,
            parser: RefCell::new(new_parser().expect("engine query group: R grammar should load")),
            stubs,
            inference_template: RefCell::new(None),
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

    /// An immutable borrow of the shared interner, held by an IDE orchestrator across its generic call.
    /// Unlike [`with_interner`](RoughlyQueries::with_interner) (which releases the borrow on return), this
    /// hands the borrow out so the engine-backed IDE view can keep `&Interner` alive while it renders. It is
    /// sound **only after** every `fetch` the feature needs has completed: a query body `borrow_mut`s the
    /// interner, so no body may run while this borrow is held (the prime-then-borrow discipline in
    /// `ide_view.rs`).
    pub fn interner_ref(&self) -> Ref<'_, Interner> {
        Ref::map(self.lowering.borrow(), |lowering| lowering.interner())
    }

    // The set-once stub library, so the IDE view can report a stdlib name's origin namespace on hover.
    pub fn stubs(&self) -> &StubLibrary {
        &self.stubs
    }

    pub fn package_symbol_index_runs(&self) -> u64 {
        self.counters.package_symbol_index.get()
    }

    pub fn package_type_definitions_runs(&self) -> u64 {
        self.counters.package_type_definitions.get()
    }

    pub fn type_definitions_module_runs(&self, file: FileId) -> u64 {
        per_key(&self.counters.type_definitions_module, file)
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

    pub fn exported_schemes_runs(&self, file: FileId) -> u64 {
        per_key(&self.counters.exported_schemes, file)
    }

    pub fn interface_deps_runs(&self, symbol: Symbol) -> u64 {
        per_key(&self.counters.interface_deps, symbol)
    }

    pub fn symbol_scc_runs(&self, symbol: Symbol) -> u64 {
        per_key(&self.counters.symbol_scc, symbol)
    }

    /// How many times the SCC fixed-point body ran for one canonical member list — the convergence
    /// witness: a cyclic interface is computed once and shared by every member, so this stays small (it
    /// never spins the round cap).
    pub fn interface_scc_runs(&self, members: &[Symbol]) -> u64 {
        self.counters
            .interface_scc
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

    pub fn file_type_definitions_runs(&self, file: FileId) -> u64 {
        per_key(&self.counters.file_type_definitions, file)
    }

    pub fn package_type_index_runs(&self) -> u64 {
        self.counters.package_type_index.get()
    }

    pub fn type_name_status_runs(&self, name: Symbol) -> u64 {
        per_key(&self.counters.type_name_status, name)
    }

    pub fn package_candidate_order_runs(&self) -> u64 {
        self.counters.package_candidate_order.get()
    }

    pub fn definer_order_runs(&self, name: Symbol) -> u64 {
        per_key(&self.counters.definer_order, name)
    }

    pub fn package_naming_diagnostics_runs(&self, file: FileId) -> u64 {
        per_key(&self.counters.package_naming_diagnostics, file)
    }

    pub fn lowering_diagnostics_runs(&self, file: FileId) -> u64 {
        per_key(&self.counters.lowering_diagnostics, file)
    }

    pub fn lint_runs(&self, file: FileId) -> u64 {
        per_key(&self.counters.lint, file)
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
                // The input already carries the parsed document — the host parses once (open files
                // maintain their tree incrementally across keystrokes), so this query only projects
                // it. Read through `fetch_optional`: a removed input (a tombstone) degrades to an
                // empty document instead of panicking, so a stale dependency edge walked into
                // `Parse(f)` after a deletion yields an empty parse rather than resurrecting the
                // removed input (tombstone hardening; see `Engine::fetch_optional`).
                match engine.fetch_optional::<ParsedDocument>(Key::SourceText(*file)) {
                    Some(parsed) => Stored::new(ParsedDocument(parsed.0.clone())),
                    None => {
                        let mut parser = self.parser.borrow_mut();
                        let document = Document::parse(&mut parser, "")
                            .expect("engine query: the empty tombstone source should parse");
                        Stored::new(ParsedDocument(document))
                    }
                }
            }

            Key::Lower(file) => {
                bump(&self.counters.lower, *file);
                // Replicate production's `lower_with_diagnostics` short-circuit (`analysis::lower`): a parse
                // whose tree-sitter tree carries any error node lowers to an *empty* `Module`, not the partial
                // error-recovery HIR the public `lower` would build. Production suppresses naming/type/strict
                // diagnostics on malformed input this way (only lowering syntax diagnostics, which the engine
                // structurally cannot reach, survive), so without this the engine would emit downstream
                // diagnostics off a partial tree that production drops — a real live-editing divergence, since
                // most keystrokes are transiently malformed. The same empty module is the tombstone-hardening
                // degradation: a removed parse (`fetch_optional` -> `None`, not reachable on today's graph
                // since `Parse` is derived, but defensive against a future fetch reorder) is treated like a
                // malformed one rather than panicking.
                // Projects the module out of the one `lower_with_diagnostics` run owned by
                // `LoweringDiagnostics` (the file used to be lowered twice — once here, once for
                // the diagnostics). The module's own value-eq stays the type-only cutoff the
                // granularity proofs rely on; the malformed-input empty-module semantics live in
                // `lower_with_diagnostics` itself.
                let result = engine.fetch::<LoweringResult>(Key::LoweringDiagnostics(*file));
                Stored::new(result.module.clone())
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

            Key::TypeDefinitionsModule(file) => {
                bump(&self.counters.type_definitions_module, *file);
                let module = engine.fetch::<Module>(Key::Lower(*file));
                Stored::new(TypeDefinitionsModule(module))
            }

            Key::PackageTypeDefinitions => {
                self.counters
                    .package_type_definitions
                    .set(self.counters.package_type_definitions.get() + 1);
                // Fold every package module's top-level `@type`/`@alias` declarations, exactly as
                // production's `TypeDefinitionEnvironment::from_modules` over `package_document_ids`. A
                // file contributes only when its `DocumentKind` is `Package`, matching the index fold and
                // the package document set. Folds the per-file `TypeDefinitionsModule` *views* rather than
                // `Lower` directly: a body edit recomputes the view but it compares equal on declarations
                // and cuts off, so this fold does not re-run on a keystroke that changes no type — the
                // O(package) recompute the names-only `ExportedNames` cutoff likewise spares the symbol index.
                let files = engine.fetch::<Vec<FileId>>(Key::ProjectFiles);
                let mut modules = Vec::new();
                for file in files.iter() {
                    let kind = engine.fetch::<DocumentKind>(Key::DocumentKind(*file));
                    if *kind == DocumentKind::Package {
                        modules.push(
                            engine
                                .fetch::<TypeDefinitionsModule>(Key::TypeDefinitionsModule(*file)),
                        );
                    }
                }
                let mut type_definitions =
                    TypeDefinitionEnvironment::from_modules(modules.iter().map(|view| &*view.0));
                self.stubs.seed_type_definitions(&mut type_definitions);
                Stored::new(type_definitions)
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

            Key::InterfaceDeps(symbol) => {
                bump(&self.counters.interface_deps, *symbol);
                Stored::new(interface_deps(engine, *symbol))
            }

            Key::SymbolScc(symbol) => {
                bump(&self.counters.symbol_scc, *symbol);
                // Tarjan over the lazily-fetched `InterfaceDeps` graph: explore the subgraph reachable from
                // `symbol` and return the SCC containing it (sorted), or empty when `symbol` is a trivial,
                // non-self-looping SCC (acyclic — keep the fetch-recursion fast path in `GlobalScheme`).
                Stored::new(symbol_scc(engine, *symbol))
            }

            Key::GlobalScheme(symbol) => {
                bump(&self.counters.global_scheme, *symbol);
                let scc = engine.fetch::<Vec<Symbol>>(Key::SymbolScc(*symbol));
                if scc.is_empty() {
                    // Acyclic: a direct definition or an acyclic chain. Inferring the winning file binds
                    // referenced globals via *their* `GlobalScheme` (recorded as a dependency), so a chain
                    // resolves by plain fetch recursion — unbounded depth, no round cap to truncate it — and
                    // the export falls out of `exported_value_schemes`.
                    let defining = engine.fetch::<Option<FileId>>(Key::DefiningItem(*symbol));
                    let scheme = match *defining {
                        Some(defining_file) => {
                            let exports = engine
                                .fetch::<Vec<ExportedValue>>(Key::ExportedSchemes(defining_file));
                            exports
                                .iter()
                                .find(|export| export.symbol == *symbol)
                                .map(|export| export.type_scheme.clone())
                        }
                        None => None,
                    };
                    Stored::new(scheme)
                } else {
                    // Cyclic: route through the SCC fixed-point body, which owns the whole component and
                    // never re-enters `fetch` on a `GlobalScheme`/`InterfaceScc`/`SymbolScc` key — so the
                    // core's accidental-cycle guard stays intact for *real* graph bugs. Project this member.
                    let interface = engine
                        .fetch::<BTreeMap<Symbol, TypeScheme>>(Key::InterfaceScc((*scc).clone()));
                    Stored::new(interface.get(symbol).cloned())
                }
            }

            Key::ExportedSchemes(file) => {
                bump(&self.counters.exported_schemes, *file);
                let (_check, exports) = infer_file(self, engine, *file, &no_overrides(), false);
                Stored::new(exports)
            }

            Key::InterfaceScc(members) => {
                bump(&self.counters.interface_scc, members.clone());
                Stored::new(resolve_interface_scc(self, engine, members))
            }

            Key::FileTypeDefinitions(file) => {
                bump(&self.counters.file_type_definitions, *file);
                let module = engine.fetch::<Module>(Key::Lower(*file));
                Stored::new(file_type_definitions(&module))
            }

            Key::PackageTypeIndex => {
                self.counters
                    .package_type_index
                    .set(self.counters.package_type_index.get() + 1);
                // Fold every package module's type declarations into the naming-level index, exactly as
                // `build_type_index` folds `document_type_definitions`: one site resolves the name, two or
                // more mark it a duplicate (and keep it unresolved). Reads `FileTypeDefinitions` (the
                // declarations-only cutoff) so a body edit that changes no declaration cuts off here.
                let files = engine.fetch::<Vec<FileId>>(Key::ProjectFiles);
                let mut sites: BTreeMap<Symbol, Vec<EngineTypeInfo>> = BTreeMap::new();
                for file in files.iter() {
                    let kind = engine.fetch::<DocumentKind>(Key::DocumentKind(*file));
                    if *kind != DocumentKind::Package {
                        continue;
                    }
                    let definitions = engine.fetch::<BTreeMap<Symbol, Vec<EngineTypeInfo>>>(
                        Key::FileTypeDefinitions(*file),
                    );
                    for (name, infos) in definitions.iter() {
                        sites
                            .entry(*name)
                            .or_default()
                            .extend(infos.iter().copied());
                    }
                }
                let mut resolved = BTreeMap::new();
                let mut duplicates = BTreeSet::new();
                for (name, site_list) in &sites {
                    match site_list.as_slice() {
                        [single] => {
                            resolved.insert(*name, *single);
                        }
                        [] => {}
                        _ => {
                            duplicates.insert(*name);
                        }
                    }
                }
                Stored::new(TypeIndex {
                    resolved,
                    duplicates,
                })
            }

            Key::TypeNameStatus(name) => {
                bump(&self.counters.type_name_status, *name);
                let index = engine.fetch::<TypeIndex>(Key::PackageTypeIndex);
                let status = if index.duplicates.contains(name) {
                    TypeNameStatus::Duplicate
                } else if let Some(info) = index.resolved.get(name) {
                    TypeNameStatus::Defined(*info)
                } else {
                    TypeNameStatus::Undefined
                };
                Stored::new(status)
            }

            Key::PackageCandidateOrder => {
                self.counters
                    .package_candidate_order
                    .set(self.counters.package_candidate_order.get() + 1);
                // Path-ordered definer lists: append each package file (in `ProjectFiles` order) onto the
                // candidate list of every name it defines. Reads the same `ExportedNames` cutoff as the
                // symbol index, so a body edit cuts off here too. (`build_candidate_order`.)
                let files = engine.fetch::<Vec<FileId>>(Key::ProjectFiles);
                let mut order: BTreeMap<Symbol, Vec<FileId>> = BTreeMap::new();
                for file in files.iter() {
                    let kind = engine.fetch::<DocumentKind>(Key::DocumentKind(*file));
                    if *kind != DocumentKind::Package {
                        continue;
                    }
                    let names = engine.fetch::<Vec<Symbol>>(Key::ExportedNames(*file));
                    for name in names.iter() {
                        order.entry(*name).or_default().push(*file);
                    }
                }
                Stored::new(order)
            }

            Key::DefinerOrder(name) => {
                bump(&self.counters.definer_order, *name);
                let order =
                    engine.fetch::<BTreeMap<Symbol, Vec<FileId>>>(Key::PackageCandidateOrder);
                Stored::new(order.get(name).cloned().unwrap_or_default())
            }

            Key::PackageNamingDiagnostics(file) => {
                bump(&self.counters.package_naming_diagnostics, *file);
                Stored::new(package_naming_diagnostics(self, engine, *file))
            }

            Key::LoweringDiagnostics(file) => {
                bump(&self.counters.lowering_diagnostics, *file);
                // The single `lower_with_diagnostics` run for the file: it short-circuits to an
                // empty module + `collect_syntax_errors` on a malformed tree and otherwise returns
                // the module with the lowering-pass diagnostics. `Lower` projects the module out.
                // A removed source degrades to an empty result (tombstone hardening).
                match engine.fetch_optional::<ParsedDocument>(Key::Parse(*file)) {
                    Some(parsed) => {
                        let mut lowering = self.lowering.borrow_mut();
                        Stored::new(lower_with_diagnostics(&parsed.0, &mut lowering))
                    }
                    None => Stored::new(LoweringResult {
                        module: Module::new(HirArena::new(), Vec::new(), Vec::new()),
                        diagnostics: Vec::new(),
                    }),
                }
            }

            Key::Lint(file) => {
                bump(&self.counters.lint, *file);
                // Lint is a pure function of the parse tree and the `[lint]` config; it needs no interner.
                let config = engine.fetch::<Config>(Key::Config);
                match engine.fetch_optional::<ParsedDocument>(Key::Parse(*file)) {
                    Some(parsed) => Stored::new(lint_analyze(&parsed.0, config.lint)),
                    None => Stored::new(Vec::<Diagnostic>::new()),
                }
            }

            Key::Typecheck(file) => {
                bump(&self.counters.typecheck, *file);
                // Recorded so a config change re-checks the file; the value is unused in the check logic.
                let _config = engine.fetch::<Config>(Key::Config);
                let (check, _exports) = infer_file(self, engine, *file, &no_overrides(), true);
                Stored::new(check)
            }

            Key::Diagnostics(file) => {
                bump(&self.counters.diagnostics, *file);
                let check = engine.fetch::<ModuleCheck>(Key::Typecheck(*file));
                let naming = engine.fetch::<DocumentNamingComputation>(Key::LocalNaming(*file));
                let module = engine.fetch::<Module>(Key::Lower(*file));
                // Fetched before the `lowering` borrow below: their bodies borrow the interner/parser too,
                // so re-entering them while holding the borrow would double-borrow the `RefCell`.
                let package_naming =
                    engine.fetch::<Vec<Diagnostic>>(Key::PackageNamingDiagnostics(*file));
                let lowering_result =
                    engine.fetch::<LoweringResult>(Key::LoweringDiagnostics(*file));
                let lint = engine.fetch::<Vec<Diagnostic>>(Key::Lint(*file));
                // Strict-mode diagnostics are rendered here (production renders them in `typecheck`'s
                // round 2 via the private `strict_origin_diagnostics`, ported verbatim below). Type errors
                // stay raw `InferenceError`s; the harness renders them against the interner + fallback
                // range so there is a single source of truth for the type-error set.
                let lowering = self.lowering.borrow();
                let strict_diagnostics = analysis::strict_origin_diagnostics(
                    &module,
                    &check.strict_origins,
                    lowering.interner(),
                );
                // Computed unconditionally (like `strict_diagnostics`); the consumer gates it on the `unused`
                // check, matching production's `check_config.unused` in `Analysis::document_diagnostics`.
                let unused =
                    analysis::naming::unused_diagnostics(&naming.naming, lowering.interner());
                Stored::new(FileDiagnostics {
                    naming: naming.diagnostics.clone(),
                    package_naming: (*package_naming).clone(),
                    type_errors: check.errors.clone(),
                    strict_diagnostics,
                    strict_override: module.strict_override,
                    unused,
                    lowering: lowering_result.diagnostics.clone(),
                    lint: (*lint).clone(),
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
//
// `scc_overrides` carries the in-SCC schemes when this runs inside the [`Key::InterfaceScc`] fixed-point:
// a referenced global that is a member of the SCC under resolution is bound to its *current* round scheme
// from the table (never fetched, since its `GlobalScheme` is the cyclic edge), while an out-of-SCC global
// is fetched through its `GlobalScheme` as usual (an acyclic dependency). For the ordinary callers it is
// empty, so every referenced global is fetched — the plain fetch-recursion path.
fn infer_file(
    group: &RoughlyQueries,
    engine: &Engine<RoughlyQueries>,
    file: FileId,
    scc_overrides: &BTreeMap<Symbol, TypeScheme>,
    record_expression_types: bool,
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
            // not to an unbound name — so the referrer's check sees the same environment either way. An
            // in-SCC reference (mutual re-export or mutual value reference) takes its current round scheme
            // from `scc_overrides` instead of fetching `GlobalScheme` — that fetch is the cyclic back-edge
            // the SCC fixed-point owns, so re-entering it is exactly what would trip the accidental-cycle
            // guard. Out-of-SCC references fetch `GlobalScheme` normally (an acyclic dependency).
            let scheme = match scc_overrides.get(symbol) {
                Some(override_scheme) => override_scheme.clone(),
                None => {
                    let scheme = engine.fetch::<Option<TypeScheme>>(Key::GlobalScheme(*symbol));
                    (*scheme)
                        .clone()
                        .unwrap_or_else(|| TypeScheme::monomorphic(CoreType::Unknown))
                }
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
                // The declarations-only view, like `PackageTypeDefinitions`: a body edit to a package
                // file does not re-fold this script's type environment.
                package_modules.push(
                    engine.fetch::<TypeDefinitionsModule>(Key::TypeDefinitionsModule(*other)),
                );
            }
        }
        let mut script_type_definitions = TypeDefinitionEnvironment::from_modules(
            package_modules
                .iter()
                .map(|view| &*view.0)
                .chain([&*module]),
        );
        group
            .stubs
            .seed_type_definitions(&mut script_type_definitions);
        script_type_definitions
    };

    let mut lowering = group.lowering.borrow_mut();
    let mut inference_state = {
        let mut template = group.inference_template.borrow_mut();
        template
            .get_or_insert_with(|| {
                let mut state = inference_state_with_builtins_in_interner(lowering.interner_mut());
                group.stubs.seed_into(&mut state);
                state
            })
            .clone()
    };
    // The authoritative `Typecheck` round records per-expression types (for the IDE's
    // `expression_types_by_id`) and strict `Unknown` origins; the `GlobalScheme`/SCC callers only need the
    // exported schemes and discard the `ModuleCheck`, so they leave recording off (the wasted-work note on
    // `record_strict_origin`), matching production, which records only in its round-2 authoritative check.
    if record_expression_types {
        inference_state.enable_expression_type_recording();
    }
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

// The empty override map for the ordinary `infer_file` callers (`GlobalScheme`'s acyclic path and
// `Typecheck`): with no in-SCC bindings, every referenced global is fetched through its `GlobalScheme`.
fn no_overrides() -> BTreeMap<Symbol, TypeScheme> {
    BTreeMap::new()
}

// The interface-dependency out-edges of one package global: the package globals that computing
// `GlobalScheme(symbol)` reads. `GlobalScheme(symbol)` infers `symbol`'s whole winning file (`infer_file`),
// which binds every package global that file references; so the edge set is exactly the file's referenced
// non-local names that themselves resolve to package globals (have a `DefiningItem`). This is the precise
// graph the real `GlobalScheme` fetches induce, so the SCC built from it captures every cyclic fetch —
// nothing slips past to trip the accidental-cycle guard.
//
// A **self-edge** (`symbol` in its own dep set) is real and kept: it arises when the winning file
// *forward-references* the global it defines (`beta <- alpha(1L)` above `alpha <- 1L`, where the same file
// is `alpha`'s winner), so the forward reference resolves to the package global `alpha` whose winner is
// this very file — `infer_file` then fetches `GlobalScheme(alpha)` while computing it. `symbol_scc` reads
// the self-edge as a self-referential singleton SCC and routes it through the fixed-point. Returned
// sorted/deduped (a `BTreeSet`) for an order-stable, cutoff-friendly value.
fn interface_deps(engine: &Engine<RoughlyQueries>, symbol: Symbol) -> Vec<Symbol> {
    let defining = engine.fetch::<Option<FileId>>(Key::DefiningItem(symbol));
    let Some(file) = *defining else {
        return Vec::new();
    };
    let naming = engine.fetch::<DocumentNamingComputation>(Key::LocalNaming(file));
    let referenced: BTreeSet<Symbol> = naming.naming.non_locals.values().copied().collect();
    let mut deps = BTreeSet::new();
    for candidate in referenced {
        if engine
            .fetch::<Option<FileId>>(Key::DefiningItem(candidate))
            .is_some()
        {
            deps.insert(candidate);
        }
    }
    deps.into_iter().collect()
}

// The SCC of `symbol` over the (arbitrary out-degree) `InterfaceDeps` graph, as the canonical (sorted)
// member list (empty = a trivial, non-self-looping SCC, i.e. acyclic). A non-trivial SCC is either size ≥ 2
// *or* a single symbol with a self-edge (`symbol` forward-references its own definition) — both route
// through the `InterfaceScc` fixed-point. Tarjan's algorithm, iterative (the graph depth is unbounded, so a
// recursive DFS could overflow the stack): explore the subgraph reachable from `symbol`, recording each
// `InterfaceDeps` read as a dependency so the SCC re-derives incrementally when an edge changes, and return
// the one SCC that contains `symbol`. Every member of that SCC computes the identical sorted list (Tarjan
// from any member finds the same component), so all members share one `InterfaceScc` memo.
fn symbol_scc(engine: &Engine<RoughlyQueries>, symbol: Symbol) -> Vec<Symbol> {
    let deps_of = |node: Symbol| (*engine.fetch::<Vec<Symbol>>(Key::InterfaceDeps(node))).clone();

    // A self-referential singleton: `symbol`'s own winning file forward-references `symbol`, so the SCC is
    // the non-trivial `{symbol}` even though Tarjan reports a size-1 component (a self-edge does not lower a
    // node's own lowlink). Detected from `symbol`'s direct edges and folded into the result below.
    let start_edges = deps_of(symbol);
    let start_self_loop = start_edges.contains(&symbol);

    let mut next_index = 0usize;
    let mut index: BTreeMap<Symbol, usize> = BTreeMap::new();
    let mut low: BTreeMap<Symbol, usize> = BTreeMap::new();
    let mut on_stack: BTreeSet<Symbol> = BTreeSet::new();
    let mut component_stack: Vec<Symbol> = Vec::new();
    let mut start_scc: Vec<Symbol> = Vec::new();

    struct Frame {
        node: Symbol,
        edges: Vec<Symbol>,
        next_edge: usize,
    }
    let mut frames: Vec<Frame> = Vec::new();

    index.insert(symbol, next_index);
    low.insert(symbol, next_index);
    next_index += 1;
    component_stack.push(symbol);
    on_stack.insert(symbol);
    frames.push(Frame {
        node: symbol,
        edges: start_edges,
        next_edge: 0,
    });

    while let Some(frame) = frames.last_mut() {
        let node = frame.node;
        if frame.next_edge < frame.edges.len() {
            let target = frame.edges[frame.next_edge];
            frame.next_edge += 1;
            if let std::collections::btree_map::Entry::Vacant(slot) = index.entry(target) {
                slot.insert(next_index);
                low.insert(target, next_index);
                next_index += 1;
                component_stack.push(target);
                on_stack.insert(target);
                let edges = deps_of(target);
                frames.push(Frame {
                    node: target,
                    edges,
                    next_edge: 0,
                });
            } else if on_stack.contains(&target) {
                let target_index = index[&target];
                let updated = low[&node].min(target_index);
                low.insert(node, updated);
            }
        } else {
            // All of `node`'s edges are walked; if it is an SCC root, pop its component off the stack.
            if low[&node] == index[&node] {
                let mut component = Vec::new();
                loop {
                    let member = component_stack
                        .pop()
                        .expect("component stack is non-empty at a root");
                    on_stack.remove(&member);
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                if component.contains(&symbol) {
                    component.sort();
                    start_scc = component;
                }
            }
            frames.pop();
            // Propagate this node's lowlink into its DFS parent (the tree-edge relaxation).
            if let Some(parent) = frames.last() {
                let parent_node = parent.node;
                let updated = low[&parent_node].min(low[&node]);
                low.insert(parent_node, updated);
            }
        }
    }

    match start_scc.len() {
        0 => Vec::new(),
        // A lone `symbol` is cyclic only when it forward-references its own definition (a self-edge);
        // otherwise it is a trivial, acyclic SCC.
        1 if start_self_loop => start_scc,
        1 => Vec::new(),
        _ => start_scc,
    }
}

// The bounded synchronous fixed-point for one cyclic interface SCC, ported from `analysis.rs`'s `typecheck`
// package-interface loop. Each round re-infers every member's winning file with the *current* round's SCC
// schemes bound for in-SCC references (via `infer_file`'s `scc_overrides`) and ordinary `GlobalScheme`
// fetches for out-of-SCC references (acyclic dependencies the engine memoizes), bootstrapping every member
// to `Unknown`. A pure mutual cycle (re-export or value reference) carries no concrete base, so it
// converges to `Unknown`; a member with its own annotation re-infers to that annotation independent of the
// others, breaking the cycle there — exactly production's behaviour. The `#members + slack` round cap and
// the oscillation guard are ported faithfully as the convergence machinery and the safety net for any
// non-monotone history. The body fetches only `DefiningItem`/`Lower`/`LocalNaming`/out-of-SCC
// `GlobalScheme` for its members' files, so it never re-enters `GlobalScheme`/`InterfaceScc`/`SymbolScc` on
// an in-SCC key — the core's accidental-cycle guard stays intact for genuine engine cycles.
fn resolve_interface_scc(
    group: &RoughlyQueries,
    engine: &Engine<RoughlyQueries>,
    members: &[Symbol],
) -> BTreeMap<Symbol, TypeScheme> {
    let member_set: BTreeSet<Symbol> = members.iter().copied().collect();
    // The distinct winning files of the members. Inferring one file yields every member it defines at once
    // (`exported_value_schemes` returns the whole file), so group by file to infer each once per round.
    let mut member_files: BTreeSet<FileId> = BTreeSet::new();
    for member in members {
        if let Some(file) = *engine.fetch::<Option<FileId>>(Key::DefiningItem(*member)) {
            member_files.insert(file);
        }
    }

    let unknown = TypeScheme::monomorphic(CoreType::Unknown);
    let mut table: BTreeMap<Symbol, TypeScheme> = members
        .iter()
        .map(|member| (*member, unknown.clone()))
        .collect();
    let mut pinned: BTreeSet<Symbol> = BTreeSet::new();
    let mut history: HashMap<Symbol, Vec<String>> = HashMap::new();

    // Round cap = `#members-in-scc + slack`, proportional to the SCC (not a fixed cap), matching production:
    // synchronous iteration over a monotone framework converges in `#members + 1` rounds, the slack covers
    // the extra rounds an oscillation needs to be detected and pinned; the common case breaks out the moment
    // a round makes no change.
    let round_cap = members.len().saturating_add(SCC_ROUND_SLACK);
    for _round in 0..round_cap {
        // The one host check point the core's per-`recompute` check cannot reach: this fixed-point owns its
        // whole component in a single `recompute` body and loops internally, so a newer edit must be able to
        // abandon it between rounds rather than waiting out the round cap. A no-op without an installed token.
        engine.check_cancelled();
        // Re-infer each member file once with the current table bound for in-SCC references, collecting the
        // fresh exported scheme of every member.
        let mut fresh: BTreeMap<Symbol, TypeScheme> = BTreeMap::new();
        for file in &member_files {
            let (_check, exports) = infer_file(group, engine, *file, &table, false);
            for export in exports {
                if member_set.contains(&export.symbol) {
                    fresh.insert(export.symbol, export.type_scheme);
                }
            }
        }

        let mut next = table.clone();
        for member in members {
            if pinned.contains(member) {
                next.insert(*member, unknown.clone());
            } else {
                next.insert(
                    *member,
                    fresh
                        .get(member)
                        .cloned()
                        .unwrap_or_else(|| unknown.clone()),
                );
            }
        }

        // Oscillation guard: a member whose rendering returns to an earlier value while differing from the
        // previous round is swapping on a cycle, not progressing monotonically — pin it to `Unknown`, which
        // collapses the cycle and restores convergence. A pure cycle reaches the all-`Unknown` fixed point
        // directly, so this is the defensive net. Rendering reuses the shared interner (no `fetch` held
        // across this borrow, and `infer_file`'s own `lowering` borrow has been released above).
        {
            let lowering = group.lowering.borrow();
            for member in members {
                if pinned.contains(member) {
                    continue;
                }
                let rendered =
                    render_type_scheme(lowering.interner(), next.get(member).unwrap_or(&unknown));
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

// Slack added to `#members` for the SCC fixed-point round cap (see `resolve_interface_scc`). Sized like
// production's package-interface loop so a legitimate cycle always converges within the bound.
const SCC_ROUND_SLACK: usize = 8;

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
        let kind = &module.arena.get(*expression_id).kind;
        if let Some(target) = kind.simple_assignment_target() {
            if top_level_binding(local_naming, *expression_id).is_some() {
                names.insert(target);
            }
        } else if let ExpressionKind::Block { expressions, .. } = kind {
            collect_package_definition_names(module, local_naming, expressions, names);
        }
    }
}

// One file's package-naming diagnostics, reproducing `analysis::naming::package_document_diagnostics`
// byte-for-byte. All cross-file facts are fetched up front: a fetch may recompute a query whose body
// borrows the shared interner, so none can happen while this body holds that borrow. The diagnostics are
// then built against the interner. The differential harness normalizes (sorts) the result, so the
// category emission order is immaterial — only the rendered (range, code, severity, message) multiset is.
fn package_naming_diagnostics(
    group: &RoughlyQueries,
    engine: &Engine<RoughlyQueries>,
    file: FileId,
) -> Vec<Diagnostic> {
    let module = engine.fetch::<Module>(Key::Lower(file));
    // The type-reference diagnostic narrows to the offending name by re-lexing the document, so the
    // parse (its rope) is fetched up front with the other cross-file facts, before the interner borrow.
    let parsed = engine.fetch::<ParsedDocument>(Key::Parse(file));
    let naming = engine.fetch::<DocumentNamingComputation>(Key::LocalNaming(file));
    let kind = engine.fetch::<DocumentKind>(Key::DocumentKind(file));
    let local_naming = &naming.naming;
    let is_script = *kind == DocumentKind::Script;

    // (D) inputs: the winning file (if any) of every referenced non-local symbol. A change to a symbol's
    // defined-ness flips this and re-runs the file, reproducing production's defined-ness reverse-dep hop.
    let mut defining: BTreeMap<Symbol, Option<FileId>> = BTreeMap::new();
    for symbol in local_naming.non_locals.values() {
        defining
            .entry(*symbol)
            .or_insert_with(|| *engine.fetch::<Option<FileId>>(Key::DefiningItem(*symbol)));
    }

    // (A) inputs (package only): the ordered definer list of every name this file *top-level*-assigns.
    // Block-nested definitions contribute to the index but never emit an overwrite diagnostic, so only
    // direct top-level assigns drive — and depend on — the candidate order (the precise reverse-dep edge).
    let mut top_level_targets: BTreeSet<Symbol> = BTreeSet::new();
    for expression_id in &module.expressions {
        if let Some(target) = module
            .arena
            .get(*expression_id)
            .kind
            .simple_assignment_target()
            && top_level_binding(local_naming, *expression_id).is_some()
        {
            top_level_targets.insert(target);
        }
    }
    let mut definer_order: BTreeMap<Symbol, Vec<FileId>> = BTreeMap::new();
    if !is_script {
        for name in &top_level_targets {
            definer_order.insert(
                *name,
                (*engine.fetch::<Vec<FileId>>(Key::DefinerOrder(*name))).clone(),
            );
        }
    }

    // (T)/(C) inputs: the status of every type name this file references, plus — for the duplicate-type
    // diagnostic, package only — every type name it declares. A script's own declarations shadow package
    // ones of the same name, so they are overlaid here and the package status is not consulted for them.
    let script_own: BTreeMap<Symbol, EngineTypeInfo> = if is_script {
        module
            .definitions
            .iter()
            .map(|definition| {
                (
                    definition.definition.name,
                    EngineTypeInfo {
                        kind: definition.definition.kind,
                        arity: definition.definition.type_parameters.len(),
                    },
                )
            })
            .collect()
    } else {
        BTreeMap::new()
    };
    let referenced_types = document_type_references(&module, local_naming);
    let mut resolved_types: BTreeMap<Symbol, EngineTypeInfo> = BTreeMap::new();
    for name in &referenced_types {
        if let Some(info) = script_own.get(name) {
            resolved_types.insert(*name, *info);
            continue;
        }
        if let TypeNameStatus::Defined(info) =
            *engine.fetch::<TypeNameStatus>(Key::TypeNameStatus(*name))
        {
            resolved_types.insert(*name, info);
        }
    }
    let mut duplicate_type_names: BTreeSet<Symbol> = BTreeSet::new();
    if !is_script {
        for definition in &module.definitions {
            let name = definition.definition.name;
            if matches!(
                *engine.fetch::<TypeNameStatus>(Key::TypeNameStatus(name)),
                TypeNameStatus::Duplicate
            ) {
                duplicate_type_names.insert(name);
            }
        }
    }

    // All cross-file fetches are done; hold the interner and build the rendered diagnostics.
    let lowering = group.lowering.borrow();
    let interner = lowering.interner();
    let mut diagnostics = Vec::new();

    // (T)/(B)/(A) apply only to package documents: scripts define no package globals and their type
    // declarations are local.
    if !is_script {
        // (T) duplicate-type: one per declaration whose name is defined by more than one package site.
        for definition in &module.definitions {
            if duplicate_type_names.contains(&definition.definition.name) {
                let name = interner
                    .resolve(definition.definition.name)
                    .unwrap_or("<unknown>");
                diagnostics.push(Diagnostic::syntax_error(
                    definition.range,
                    format!(
                        "invalid semantics: type name `{name}` is already defined by another top-level @type or @alias declaration in this package."
                    ),
                ));
            }
        }

        // (B) builtin shadow + (A) overwrite pairs, per top-level assignment. Overwrite combines the
        // file's position in the path-ordered candidate list with repeated assignments within the file.
        let mut occurrence_totals: BTreeMap<Symbol, usize> = BTreeMap::new();
        for expression_id in &module.expressions {
            if let Some(target) = module
                .arena
                .get(*expression_id)
                .kind
                .simple_assignment_target()
                && top_level_binding(local_naming, *expression_id).is_some()
            {
                *occurrence_totals.entry(target).or_default() += 1;
            }
        }
        let mut occurrences_seen: BTreeMap<Symbol, usize> = BTreeMap::new();
        for expression_id in &module.expressions {
            let Some(target) = module
                .arena
                .get(*expression_id)
                .kind
                .simple_assignment_target()
            else {
                continue;
            };
            let Some(binding) = top_level_binding(local_naming, *expression_id) else {
                continue;
            };

            // (B) The namespace-symbol shadow is dead in production (`is_namespace_symbol` is the constant
            // `false`), so only the builtin/stub shadow remains.
            if is_builtin(interner, target) || group.stubs.contains(target) {
                let name = interner.resolve(target).unwrap_or("<unknown>");
                diagnostics.push(Diagnostic::naming_warning(
                    binding.range,
                    format!("Top-level binding `{name}` shadows a builtin."),
                ));
            }

            let (document_position, candidate_count) = match definer_order.get(&target) {
                Some(candidates) => (
                    candidates
                        .iter()
                        .position(|candidate| *candidate == file)
                        .unwrap_or(0),
                    candidates.len(),
                ),
                None => (0, 1),
            };
            let occurrence_index = *occurrences_seen.entry(target).or_default();
            *occurrences_seen
                .get_mut(&target)
                .expect("occurrence counter exists") += 1;
            let occurrences_in_document = occurrence_totals.get(&target).copied().unwrap_or(1);
            let has_earlier = document_position > 0 || occurrence_index > 0;
            let has_later = document_position + 1 < candidate_count
                || occurrence_index + 1 < occurrences_in_document;

            if has_earlier {
                let name = interner.resolve(target).unwrap_or("<unknown>");
                diagnostics.push(Diagnostic::naming_warning(
                    binding.range,
                    format!(
                        "Top-level binding `{name}` overwrites an earlier top-level binding in this package."
                    ),
                ));
            }
            if has_later {
                let name = interner.resolve(target).unwrap_or("<unknown>");
                diagnostics.push(Diagnostic::naming_warning(
                    binding.range,
                    format!(
                        "Top-level binding `{name}` is overwritten by a later top-level binding in this package."
                    ),
                ));
            }
        }
    }

    // (C) type references — every document. Resolved names check arity / `@new`-on-alias; unresolved
    // names (undefined or package-duplicate, unless a script shadows them) raise the unknown-type error.
    resolve_module_type_references(
        &module,
        local_naming,
        &resolved_types,
        parsed.0.rope(),
        interner,
        &mut diagnostics,
    );

    // (D) could-not-resolve — every document. One per reference occurrence that resolves to neither a
    // package global nor a builtin nor a stub.
    for (expression_id, symbol) in &local_naming.non_locals {
        let resolves = defining.get(symbol).copied().flatten().is_some()
            || is_builtin(interner, *symbol)
            || group.stubs.contains(*symbol);
        if resolves {
            continue;
        }
        let range = module.arena.get(*expression_id).range;
        diagnostics.push(analysis::naming::unresolved_reference_diagnostic(
            interner,
            &group.stubs,
            *symbol,
            range,
        ));
    }

    // `pkg::name` validation: the diagnostic builder is the shared production function, so the
    // wording cannot drift; only the walk that decides *which* reads to validate lives here.
    for read in &local_naming.namespace_reads {
        let range = module.arena.get(read.expression_id).range;
        if let Some(diagnostic) =
            analysis::naming::namespace_read_diagnostic(interner, &group.stubs, read, range)
        {
            diagnostics.push(diagnostic);
        }
    }

    diagnostics
}

// Ported from `analysis::naming`'s `resolve_module_type_references` + `TypeResolver`: walk a document's
// `@type`/`@alias` representation types and inline annotations, diagnosing unresolved type names,
// type-argument arity mismatches, and `@new` applied to an alias. `resolved` is the effective resolved-type
// lookup (the package index, with a script's own declarations overlaid); every range and message matches
// production exactly.
fn resolve_module_type_references(
    module: &Module,
    local_naming: &NamesLocal,
    resolved: &BTreeMap<Symbol, EngineTypeInfo>,
    rope: &Rope,
    interner: &Interner,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for definition in &module.definitions {
        let local_type_parameters: BTreeSet<Symbol> = definition
            .definition
            .type_parameters
            .iter()
            .copied()
            .collect();
        resolve_surface_type(
            &definition.definition.surface_type,
            &local_type_parameters,
            definition.range,
            resolved,
            rope,
            interner,
            diagnostics,
        );
    }
    for expression_id in &local_naming.named_type_annotations {
        let Some(annotation) = module.arena.get(*expression_id).annotation.as_ref() else {
            continue;
        };
        match annotation.annotation() {
            Annotation::Type { surface_type, .. } => resolve_surface_type(
                surface_type,
                &BTreeSet::new(),
                annotation.range(),
                resolved,
                rope,
                interner,
                diagnostics,
            ),
            Annotation::New { nominal_type } => resolve_nominal_type_reference(
                nominal_type,
                annotation.range(),
                resolved,
                rope,
                interner,
                diagnostics,
            ),
        }
    }
}

fn resolve_nominal_type_reference(
    nominal_type: &NamedTypeRef,
    range: Range,
    resolved: &BTreeMap<Symbol, EngineTypeInfo>,
    rope: &Rope,
    interner: &Interner,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match resolved.get(&nominal_type.name) {
        Some(type_info) => {
            push_arity_diagnostic(
                nominal_type.name,
                type_info.arity,
                nominal_type.type_arguments.len(),
                range,
                interner,
                diagnostics,
            );
            if type_info.kind == DefinitionKind::Type {
                for type_argument in &nominal_type.type_arguments {
                    resolve_surface_type(
                        type_argument,
                        &BTreeSet::new(),
                        range,
                        resolved,
                        rope,
                        interner,
                        diagnostics,
                    );
                }
                return;
            }
            let name = interner
                .resolve(nominal_type.name)
                .unwrap_or("<unknown>")
                .to_owned();
            diagnostics.push(Diagnostic::syntax_error(
                range,
                format!(
                    "invalid semantics: `@new` requires a nominal type declared with `@type`, but `{name}` is an alias."
                ),
            ));
        }
        None => push_unknown_type_diagnostic(nominal_type.name, range, rope, interner, diagnostics),
    }

    for type_argument in &nominal_type.type_arguments {
        resolve_surface_type(
            type_argument,
            &BTreeSet::new(),
            range,
            resolved,
            rope,
            interner,
            diagnostics,
        );
    }
}

fn resolve_surface_type(
    surface_type: &SurfaceType,
    local_type_parameters: &BTreeSet<Symbol>,
    range: Range,
    resolved: &BTreeMap<Symbol, EngineTypeInfo>,
    rope: &Rope,
    interner: &Interner,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match surface_type {
        SurfaceType::Named(name, arguments) => {
            if local_type_parameters.contains(name) {
                return;
            }
            if let Some(type_info) = resolved.get(name) {
                push_arity_diagnostic(
                    *name,
                    type_info.arity,
                    arguments.len(),
                    range,
                    interner,
                    diagnostics,
                );
            } else {
                push_unknown_type_diagnostic(*name, range, rope, interner, diagnostics);
            }
            for argument in arguments {
                resolve_surface_type(
                    argument,
                    local_type_parameters,
                    range,
                    resolved,
                    rope,
                    interner,
                    diagnostics,
                );
            }
        }
        SurfaceType::Vector(inner_type)
        | SurfaceType::NamedVector(inner_type)
        | SurfaceType::List(inner_type)
        | SurfaceType::NamedList(inner_type) => resolve_surface_type(
            inner_type,
            local_type_parameters,
            range,
            resolved,
            rope,
            interner,
            diagnostics,
        ),
        SurfaceType::Union(members) => {
            for member in members {
                resolve_surface_type(
                    member,
                    local_type_parameters,
                    range,
                    resolved,
                    rope,
                    interner,
                    diagnostics,
                );
            }
        }
        SurfaceType::Record(fields) => {
            for field in fields {
                resolve_surface_type(
                    &field.value,
                    local_type_parameters,
                    range,
                    resolved,
                    rope,
                    interner,
                    diagnostics,
                );
            }
        }
        SurfaceType::Tuple(items) => {
            for item in items {
                resolve_surface_type(
                    item,
                    local_type_parameters,
                    range,
                    resolved,
                    rope,
                    interner,
                    diagnostics,
                );
            }
        }
        SurfaceType::Function(function_type) => {
            for parameter in &function_type.parameters {
                resolve_surface_type(
                    parameter,
                    local_type_parameters,
                    range,
                    resolved,
                    rope,
                    interner,
                    diagnostics,
                );
            }
            for parameter in &function_type.named_parameters {
                resolve_surface_type(
                    &parameter.value,
                    local_type_parameters,
                    range,
                    resolved,
                    rope,
                    interner,
                    diagnostics,
                );
            }
            if let Some(variadic) = &function_type.variadic {
                resolve_surface_type(
                    &variadic.element,
                    local_type_parameters,
                    range,
                    resolved,
                    rope,
                    interner,
                    diagnostics,
                );
            }
            resolve_surface_type(
                &function_type.return_type,
                local_type_parameters,
                range,
                resolved,
                rope,
                interner,
                diagnostics,
            );
        }
        SurfaceType::Binders(type_parameters, inner_type) => {
            if type_parameters.is_empty() {
                resolve_surface_type(
                    inner_type,
                    local_type_parameters,
                    range,
                    resolved,
                    rope,
                    interner,
                    diagnostics,
                );
                return;
            }
            let mut nested = local_type_parameters.clone();
            nested.extend(type_parameters.iter().map(|parameter| parameter.name));
            resolve_surface_type(
                inner_type,
                &nested,
                range,
                resolved,
                rope,
                interner,
                diagnostics,
            );
        }
        SurfaceType::Any | SurfaceType::Unknown | SurfaceType::Null | SurfaceType::Scalar(_) => {}
    }
}

fn push_unknown_type_diagnostic(
    symbol: Symbol,
    range: Range,
    rope: &Rope,
    interner: &Interner,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let name = interner.resolve(symbol).unwrap_or("<unknown>").to_owned();
    // Narrow the underline from the whole annotation to the offending type name when it can be located
    // in the `#:` notation; fall back to the annotation range otherwise. Matches production exactly.
    let range = type_name_token_range(rope, range, &name).unwrap_or(range);
    diagnostics.push(Diagnostic::naming_error(
        range,
        format!("I could not resolve type `{name}`."),
    ));
}

fn push_arity_diagnostic(
    symbol: Symbol,
    expected: usize,
    found: usize,
    range: Range,
    interner: &Interner,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if expected == found {
        return;
    }
    let name = interner.resolve(symbol).unwrap_or("<unknown>").to_owned();
    let message = if expected == 0 {
        format!("type `{name}` does not take type arguments, but found {found}.")
    } else {
        format!("generic type `{name}` expects {expected} type argument(s), but found {found}.")
    };
    diagnostics.push(Diagnostic::syntax_error(range, message));
}

// The named type references a document makes (`analysis::naming::document_type_references`): the set of
// type names looked up while resolving its `@type`/`@alias` representation types and inline annotations,
// excluding names bound as local type parameters at the reference site. Exactly the set whose
// `TypeNameStatus` the document's type-reference diagnostics depend on.
fn document_type_references(module: &Module, local_naming: &NamesLocal) -> BTreeSet<Symbol> {
    let mut references = BTreeSet::new();
    for definition in &module.definitions {
        let local_type_parameters: BTreeSet<Symbol> = definition
            .definition
            .type_parameters
            .iter()
            .copied()
            .collect();
        collect_surface_type_references(
            &definition.definition.surface_type,
            &local_type_parameters,
            &mut references,
        );
    }
    for expression_id in &local_naming.named_type_annotations {
        let Some(annotation) = module.arena.get(*expression_id).annotation.as_ref() else {
            continue;
        };
        match annotation.annotation() {
            Annotation::Type { surface_type, .. } => {
                collect_surface_type_references(surface_type, &BTreeSet::new(), &mut references);
            }
            Annotation::New { nominal_type } => {
                references.insert(nominal_type.name);
                for type_argument in &nominal_type.type_arguments {
                    collect_surface_type_references(
                        type_argument,
                        &BTreeSet::new(),
                        &mut references,
                    );
                }
            }
        }
    }
    references
}

fn collect_surface_type_references(
    surface_type: &SurfaceType,
    local_type_parameters: &BTreeSet<Symbol>,
    references: &mut BTreeSet<Symbol>,
) {
    match surface_type {
        SurfaceType::Named(name, arguments) => {
            if !local_type_parameters.contains(name) {
                references.insert(*name);
            }
            for argument in arguments {
                collect_surface_type_references(argument, local_type_parameters, references);
            }
        }
        SurfaceType::Vector(inner_type)
        | SurfaceType::NamedVector(inner_type)
        | SurfaceType::List(inner_type)
        | SurfaceType::NamedList(inner_type) => {
            collect_surface_type_references(inner_type, local_type_parameters, references);
        }
        SurfaceType::Union(members) => {
            for member in members {
                collect_surface_type_references(member, local_type_parameters, references);
            }
        }
        SurfaceType::Record(fields) => {
            for field in fields {
                collect_surface_type_references(&field.value, local_type_parameters, references);
            }
        }
        SurfaceType::Tuple(items) => {
            for item in items {
                collect_surface_type_references(item, local_type_parameters, references);
            }
        }
        SurfaceType::Function(function_type) => {
            for parameter in &function_type.parameters {
                collect_surface_type_references(parameter, local_type_parameters, references);
            }
            for parameter in &function_type.named_parameters {
                collect_surface_type_references(
                    &parameter.value,
                    local_type_parameters,
                    references,
                );
            }
            if let Some(variadic) = &function_type.variadic {
                collect_surface_type_references(
                    &variadic.element,
                    local_type_parameters,
                    references,
                );
            }
            collect_surface_type_references(
                &function_type.return_type,
                local_type_parameters,
                references,
            );
        }
        SurfaceType::Binders(type_parameters, inner_type) => {
            if type_parameters.is_empty() {
                collect_surface_type_references(inner_type, local_type_parameters, references);
                return;
            }
            let mut nested = local_type_parameters.clone();
            nested.extend(type_parameters.iter().map(|parameter| parameter.name));
            collect_surface_type_references(inner_type, &nested, references);
        }
        SurfaceType::Any | SurfaceType::Unknown | SurfaceType::Null | SurfaceType::Scalar(_) => {}
    }
}

// One file's type declarations as `name -> [(kind, arity)] site list`, in declaration order
// (`analysis::naming::document_type_definitions`). A document declaring the same name twice yields two
// sites, which makes the name a package duplicate.
fn file_type_definitions(module: &Module) -> BTreeMap<Symbol, Vec<EngineTypeInfo>> {
    let mut definitions: BTreeMap<Symbol, Vec<EngineTypeInfo>> = BTreeMap::new();
    for definition in &module.definitions {
        definitions
            .entry(definition.definition.name)
            .or_default()
            .push(EngineTypeInfo {
                kind: definition.definition.kind,
                arity: definition.definition.type_parameters.len(),
            });
    }
    definitions
}

// The binding a top-level assignment resolves to, if any (`analysis::naming`'s `top_level_binding`):
// shared by the package-definition membership rule and the overwrite/shadow loop.
fn top_level_binding(
    local_naming: &NamesLocal,
    expression_id: ExpressionId,
) -> Option<&BindingInfo> {
    let binding_id = local_naming
        .expression_resolutions
        .get(&expression_id)
        .copied()?;
    local_naming.bindings.get(&binding_id)
}

// Whether a symbol names one of the type checker's hardcoded builtins (`analysis::naming`'s
// `is_builtin_symbol`). Stub-corpus names are checked separately against the live `StubLibrary`.
fn is_builtin(interner: &Interner, symbol: Symbol) -> bool {
    interner.resolve(symbol).is_some_and(|name| {
        BUILTINS
            .iter()
            .any(|(builtin_name, _)| *builtin_name == name)
    })
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
        let parsed = engine.fetch::<ParsedDocument>(Key::SourceText(*file));
        let first_line_length = parsed
            .0
            .rope()
            .get_line(0)
            .map(|line| line.len_bytes())
            .unwrap_or(0);
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
    type_definitions_module: RefCell<HashMap<FileId, u64>>,
    package_type_definitions: Cell<u64>,
    fallback_range: Cell<u64>,
    defining_item: RefCell<HashMap<Symbol, u64>>,
    exported_schemes: RefCell<HashMap<FileId, u64>>,
    global_scheme: RefCell<HashMap<Symbol, u64>>,
    interface_deps: RefCell<HashMap<Symbol, u64>>,
    symbol_scc: RefCell<HashMap<Symbol, u64>>,
    interface_scc: RefCell<HashMap<Vec<Symbol>, u64>>,
    file_type_definitions: RefCell<HashMap<FileId, u64>>,
    package_type_index: Cell<u64>,
    type_name_status: RefCell<HashMap<Symbol, u64>>,
    package_candidate_order: Cell<u64>,
    definer_order: RefCell<HashMap<Symbol, u64>>,
    package_naming_diagnostics: RefCell<HashMap<FileId, u64>>,
    lowering_diagnostics: RefCell<HashMap<FileId, u64>>,
    lint: RefCell<HashMap<FileId, u64>>,
    typecheck: RefCell<HashMap<FileId, u64>>,
    diagnostics: RefCell<HashMap<FileId, u64>>,
}

fn bump<K: Eq + std::hash::Hash>(counter: &RefCell<HashMap<K, u64>>, key: K) {
    *counter.borrow_mut().entry(key).or_default() += 1;
}

fn per_key<K: Eq + std::hash::Hash>(counter: &RefCell<HashMap<K, u64>>, key: K) -> u64 {
    counter.borrow().get(&key).copied().unwrap_or(0)
}
